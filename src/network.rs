use std::{
    collections::VecDeque,
    ffi::CStr,
    fs,
    io::{self, Read},
    mem,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    os::unix::net::UnixDatagram,
    path::Path,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, ensure};
use calloop::channel::Sender;

const COUNTER_INTERVAL: Duration = Duration::from_secs(1);
const PROBE_INTERVAL: Duration = Duration::from_secs(2);
const WAKE_BYTE: &[u8] = &[1];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NetworkKind {
    #[default]
    Offline,
    Ethernet,
    Wifi,
    Vpn,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NetworkSnapshot {
    pub interface: String,
    pub kind: NetworkKind,
    pub address: Option<IpAddr>,
    pub gateway: Option<IpAddr>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_per_second: Option<u64>,
    pub tx_per_second: Option<u64>,
    pub ping_ms: Option<f64>,
    pub loss_percent: Option<u8>,
    pub probe_error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct NetworkOptions {
    pub interface: Option<String>,
    pub ping_target: IpAddr,
}

pub struct NetworkHandle {
    control: UnixDatagram,
    visible: Arc<AtomicBool>,
    visibility_generation: Arc<AtomicU64>,
    refresh_generation: Arc<AtomicU64>,
    shutdown: Arc<AtomicBool>,
}

impl NetworkHandle {
    pub fn spawn(sender: Sender<NetworkSnapshot>, options: NetworkOptions) -> anyhow::Result<Self> {
        if let Some(interface) = options.interface.as_deref() {
            ensure!(
                valid_interface_name(interface),
                "invalid network interface name"
            );
        }

        let (control, worker_control) =
            UnixDatagram::pair().context("failed to create network worker control socket")?;
        control
            .set_nonblocking(true)
            .context("failed to configure network control socket")?;
        worker_control
            .set_nonblocking(true)
            .context("failed to configure network worker socket")?;
        let netlink = open_route_socket().context("failed to subscribe to route events")?;
        let visible = Arc::new(AtomicBool::new(false));
        let visibility_generation = Arc::new(AtomicU64::new(0));
        let refresh_generation = Arc::new(AtomicU64::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));

        let worker = Worker {
            sender,
            options,
            control: worker_control,
            netlink,
            visible: Arc::clone(&visible),
            visibility_generation: Arc::clone(&visibility_generation),
            refresh_generation: Arc::clone(&refresh_generation),
            shutdown: Arc::clone(&shutdown),
        };
        thread::Builder::new()
            .name("nibari-network".into())
            .spawn(move || worker.run())
            .context("failed to start network worker")?;

        Ok(Self {
            control,
            visible,
            visibility_generation,
            refresh_generation,
            shutdown,
        })
    }

    pub fn set_visible(&self, visible: bool) {
        if self.visible.swap(visible, Ordering::AcqRel) != visible {
            self.visibility_generation.fetch_add(1, Ordering::Release);
            self.wake();
        }
    }

    pub fn refresh(&self) {
        self.refresh_generation.fetch_add(1, Ordering::Release);
        self.wake();
    }

    fn wake(&self) {
        match self.control.send(WAKE_BYTE) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => log::debug!("network worker wake failed: {error}"),
        }
    }
}

impl Drop for NetworkHandle {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        self.wake();
    }
}

struct Worker {
    sender: Sender<NetworkSnapshot>,
    options: NetworkOptions,
    control: UnixDatagram,
    netlink: OwnedFd,
    visible: Arc<AtomicBool>,
    visibility_generation: Arc<AtomicU64>,
    refresh_generation: Arc<AtomicU64>,
    shutdown: Arc<AtomicBool>,
}

impl Worker {
    fn run(self) {
        let mut snapshot = connectivity_snapshot(self.options.interface.as_deref());
        if self.sender.send(snapshot.clone()).is_err() {
            return;
        }

        let mut visibility_generation = 0;
        let mut refresh_generation = 0;
        let mut connectivity_dirty = false;
        let mut sampler = RateSampler::default();
        let mut loss = LossWindow::default();
        let mut next_counter = Instant::now();
        let mut next_probe = Instant::now();
        let mut last_probe_started = None;
        let mut probe_enabled = true;

        loop {
            if self.shutdown.load(Ordering::Acquire) {
                break;
            }

            let generation = self.visibility_generation.load(Ordering::Acquire);
            let visible = self.visible.load(Ordering::Acquire);
            let mut changed = false;
            if generation != visibility_generation {
                visibility_generation = generation;
                if visible {
                    sampler = RateSampler::default();
                    loss = LossWindow::default();
                    probe_enabled = true;
                    changed |= clear_probe(&mut snapshot);
                    let now = Instant::now();
                    next_counter = now;
                    next_probe = probe_deadline(now, last_probe_started);
                    connectivity_dirty = true;
                }
            }

            let generation = self.refresh_generation.load(Ordering::Acquire);
            if generation != refresh_generation {
                refresh_generation = generation;
                connectivity_dirty = true;
                if visible {
                    probe_enabled = true;
                    changed |= clear_probe(&mut snapshot);
                    next_probe = probe_deadline(Instant::now(), last_probe_started);
                }
            }

            if connectivity_dirty {
                connectivity_dirty = false;
                let connection = connectivity_snapshot(self.options.interface.as_deref());
                if apply_connectivity(&mut snapshot, connection) {
                    changed = true;
                    sampler = RateSampler::default();
                    loss = LossWindow::default();
                    probe_enabled = true;
                    next_counter = Instant::now();
                    next_probe = probe_deadline(Instant::now(), last_probe_started);
                }
            }

            let now = Instant::now();
            if visible && now >= next_counter {
                next_counter = now + COUNTER_INTERVAL;
                changed |= update_counters(&mut snapshot, &mut sampler, now);
            }

            if visible
                && probe_enabled
                && snapshot.kind != NetworkKind::Offline
                && now >= next_probe
            {
                let interface = snapshot.interface.clone();
                let probe_generation = visibility_generation;
                last_probe_started = Some(now);
                next_probe = now + PROBE_INTERVAL;
                let outcome = run_probe(self.options.ping_target);
                if self.visible.load(Ordering::Acquire)
                    && self.visibility_generation.load(Ordering::Acquire) == probe_generation
                    && snapshot.interface == interface
                {
                    match outcome {
                        ProbeOutcome::Sent(rtt) => {
                            loss.record(rtt.is_some());
                            let loss_percent = loss.percent();
                            changed |= snapshot.ping_ms != rtt
                                || snapshot.loss_percent != loss_percent
                                || snapshot.probe_error.is_some();
                            snapshot.ping_ms = rtt;
                            snapshot.loss_percent = loss_percent;
                            snapshot.probe_error = None;
                        }
                        ProbeOutcome::Unavailable(error) => {
                            probe_enabled = false;
                            changed |= snapshot.ping_ms.is_some()
                                || snapshot.probe_error.as_ref() != Some(&error);
                            snapshot.ping_ms = None;
                            snapshot.probe_error = Some(error);
                        }
                    }
                }
            }

            if changed && self.sender.send(snapshot.clone()).is_err() {
                break;
            }

            let timeout = if visible {
                let mut deadline = next_counter;
                if probe_enabled && snapshot.kind != NetworkKind::Offline {
                    deadline = deadline.min(next_probe);
                }
                poll_timeout(deadline)
            } else {
                -1
            };
            let poll_result = poll_worker(&self.control, &self.netlink, timeout);
            match poll_result {
                Ok(PollResult {
                    control_ready,
                    route_ready,
                }) => {
                    if control_ready && !drain_control(&self.control) {
                        break;
                    }
                    if route_ready {
                        if !drain_netlink(&self.netlink) {
                            break;
                        }
                        connectivity_dirty = true;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => {
                    log::warn!("network event polling stopped: {error}");
                    break;
                }
            }
        }
    }
}

fn valid_interface_name(interface: &str) -> bool {
    !interface.is_empty()
        && interface.len() < libc::IFNAMSIZ
        && interface != "."
        && interface != ".."
        && interface
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-' | b':'))
}

fn open_route_socket() -> io::Result<OwnedFd> {
    let raw_fd = unsafe {
        // SAFETY: socket is called with valid Linux netlink constants and has no pointer arguments.
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_RAW | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            libc::NETLINK_ROUTE,
        )
    };
    if raw_fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe {
        // SAFETY: raw_fd was just returned as a new descriptor by socket and is owned here.
        OwnedFd::from_raw_fd(raw_fd)
    };
    let mut address: libc::sockaddr_nl = unsafe {
        // SAFETY: all-zero is a valid initial sockaddr_nl and every public field is set below.
        mem::zeroed()
    };
    address.nl_family = libc::AF_NETLINK as libc::sa_family_t;
    address.nl_groups = (libc::RTMGRP_LINK
        | libc::RTMGRP_IPV4_IFADDR
        | libc::RTMGRP_IPV4_ROUTE
        | libc::RTMGRP_IPV6_IFADDR
        | libc::RTMGRP_IPV6_ROUTE) as u32;
    let result = unsafe {
        // SAFETY: address points to a fully initialized sockaddr_nl for the duration of bind.
        libc::bind(
            fd.as_raw_fd(),
            (&raw const address).cast::<libc::sockaddr>(),
            mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}

struct PollResult {
    control_ready: bool,
    route_ready: bool,
}

fn poll_worker(control: &UnixDatagram, netlink: &OwnedFd, timeout: i32) -> io::Result<PollResult> {
    let mut descriptors = [
        libc::pollfd {
            fd: control.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: netlink.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    let result = unsafe {
        // SAFETY: descriptors is a valid writable array of pollfd values for this call.
        libc::poll(
            descriptors.as_mut_ptr(),
            descriptors.len() as libc::nfds_t,
            timeout,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    let fatal = libc::POLLHUP | libc::POLLNVAL;
    if descriptors
        .iter()
        .any(|descriptor| descriptor.revents & fatal != 0)
    {
        return Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "network event descriptor closed",
        ));
    }
    Ok(PollResult {
        control_ready: descriptors[0].revents & (libc::POLLIN | libc::POLLERR) != 0,
        route_ready: descriptors[1].revents & (libc::POLLIN | libc::POLLERR) != 0,
    })
}

fn drain_control(control: &UnixDatagram) -> bool {
    let mut buffer = [0; 64];
    loop {
        match control.recv(&mut buffer) {
            Ok(0) => return false,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return true,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                log::warn!("network control socket failed: {error}");
                return false;
            }
        }
    }
}

fn drain_netlink(netlink: &OwnedFd) -> bool {
    let mut buffer = [0_u8; 8192];
    loop {
        let received = unsafe {
            // SAFETY: buffer is valid for writes of its full length and netlink is an open socket.
            libc::recv(
                netlink.as_raw_fd(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                libc::MSG_DONTWAIT,
            )
        };
        if received > 0 {
            continue;
        }
        if received == 0 {
            return false;
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::EAGAIN) => return true,
            Some(libc::EINTR) => {}
            Some(libc::ENOBUFS) => {
                log::debug!("network route event buffer overflowed; refreshing state");
                return true;
            }
            _ => {
                log::warn!("network route event socket failed: {error}");
                return false;
            }
        }
    }
}

fn poll_timeout(deadline: Instant) -> i32 {
    let nanos = deadline
        .saturating_duration_since(Instant::now())
        .as_nanos();
    if nanos == 0 {
        0
    } else {
        nanos.div_ceil(1_000_000).min(i32::MAX as u128) as i32
    }
}

fn probe_deadline(now: Instant, last_started: Option<Instant>) -> Instant {
    last_started.map_or(now, |last| (last + PROBE_INTERVAL).max(now))
}

fn connectivity_snapshot(explicit_interface: Option<&str>) -> NetworkSnapshot {
    let ipv4 = fs::read_to_string("/proc/net/route")
        .map(|contents| parse_ipv4_routes(&contents))
        .unwrap_or_default();
    let ipv6 = fs::read_to_string("/proc/net/ipv6_route")
        .map(|contents| parse_ipv6_routes(&contents))
        .unwrap_or_default();
    let route = ipv4
        .into_iter()
        .chain(ipv6)
        .filter(|route| {
            valid_interface_name(&route.interface)
                && explicit_interface.is_none_or(|interface| route.interface == interface)
                && interface_is_live(&route.interface)
        })
        .min_by_key(|route| route.metric);

    match route {
        Some(route) => {
            let address = interface_address(&route.interface);
            NetworkSnapshot {
                kind: if address.is_some() {
                    interface_kind(&route.interface)
                } else {
                    NetworkKind::Offline
                },
                interface: route.interface,
                address,
                gateway: route.gateway,
                ..NetworkSnapshot::default()
            }
        }
        None => {
            let interface = explicit_interface.unwrap_or_default();
            NetworkSnapshot {
                interface: interface.into(),
                address: valid_interface_name(interface)
                    .then(|| interface_address(interface))
                    .flatten(),
                ..NetworkSnapshot::default()
            }
        }
    }
}

fn apply_connectivity(snapshot: &mut NetworkSnapshot, connection: NetworkSnapshot) -> bool {
    let changed = snapshot.interface != connection.interface
        || snapshot.kind != connection.kind
        || snapshot.address != connection.address
        || snapshot.gateway != connection.gateway;
    if snapshot.interface != connection.interface {
        snapshot.rx_bytes = 0;
        snapshot.tx_bytes = 0;
    }
    if changed {
        clear_probe(snapshot);
        snapshot.rx_per_second = None;
        snapshot.tx_per_second = None;
    }
    snapshot.interface = connection.interface;
    snapshot.kind = connection.kind;
    snapshot.address = connection.address;
    snapshot.gateway = connection.gateway;
    changed
}

fn interface_is_live(interface: &str) -> bool {
    let base = Path::new("/sys/class/net").join(interface);
    let flags = fs::read_to_string(base.join("flags"))
        .ok()
        .and_then(|value| u32::from_str_radix(value.trim().trim_start_matches("0x"), 16).ok())
        .is_some_and(|flags| flags & libc::IFF_UP as u32 != 0);
    if !flags {
        return false;
    }
    fs::read_to_string(base.join("operstate"))
        .map(|state| matches!(state.trim(), "up" | "unknown" | "dormant"))
        .unwrap_or(true)
}

struct IfAddrs(*mut libc::ifaddrs);

impl Drop for IfAddrs {
    fn drop(&mut self) {
        unsafe {
            // SAFETY: self.0 is the list returned by getifaddrs and is freed exactly once here.
            libc::freeifaddrs(self.0);
        }
    }
}

fn interface_address(interface: &str) -> Option<IpAddr> {
    let mut list = std::ptr::null_mut();
    let result = unsafe {
        // SAFETY: list is a valid out-pointer; successful ownership is transferred to IfAddrs.
        libc::getifaddrs(&mut list)
    };
    if result != 0 || list.is_null() {
        return None;
    }
    let list = IfAddrs(list);
    let mut current = list.0;
    let mut best = None;
    while !current.is_null() {
        let entry = unsafe {
            // SAFETY: current belongs to the live getifaddrs list and remains valid until list drops.
            &*current
        };
        if !entry.ifa_name.is_null()
            && !entry.ifa_addr.is_null()
            && entry.ifa_flags & libc::IFF_UP as u32 != 0
        {
            let name = unsafe {
                // SAFETY: getifaddrs guarantees ifa_name points to a NUL-terminated name.
                CStr::from_ptr(entry.ifa_name)
            };
            if name.to_bytes() == interface.as_bytes() {
                let address = unsafe {
                    // SAFETY: family selects the matching sockaddr layout supplied by getifaddrs.
                    match i32::from((*entry.ifa_addr).sa_family) {
                        libc::AF_INET => {
                            let address = &*entry.ifa_addr.cast::<libc::sockaddr_in>();
                            Some(IpAddr::V4(Ipv4Addr::from(
                                address.sin_addr.s_addr.to_ne_bytes(),
                            )))
                        }
                        libc::AF_INET6 => {
                            let address = &*entry.ifa_addr.cast::<libc::sockaddr_in6>();
                            Some(IpAddr::V6(Ipv6Addr::from(address.sin6_addr.s6_addr)))
                        }
                        _ => None,
                    }
                };
                if let Some(address) = address {
                    let rank = address_rank(address);
                    if rank != 0 && best.is_none_or(|(_, best_rank)| rank > best_rank) {
                        best = Some((address, rank));
                    }
                }
            }
        }
        current = entry.ifa_next;
    }
    best.map(|(address, _)| address)
}

fn address_rank(address: IpAddr) -> u8 {
    match address {
        IpAddr::V4(address) if address.is_unspecified() || address.is_loopback() => 0,
        IpAddr::V4(address) if address.is_link_local() => 1,
        IpAddr::V4(_) => 4,
        IpAddr::V6(address) if address.is_unspecified() || address.is_loopback() => 0,
        IpAddr::V6(address) if address.is_unicast_link_local() => 2,
        IpAddr::V6(_) => 3,
    }
}

fn interface_kind(interface: &str) -> NetworkKind {
    let base = Path::new("/sys/class/net").join(interface);
    let vpn_name = interface.starts_with("tun")
        || interface.starts_with("tap")
        || interface.starts_with("wg")
        || interface.starts_with("ppp")
        || interface.starts_with("tailscale");
    let virtual_none = fs::read_link(&base)
        .ok()
        .is_some_and(|target| target.to_string_lossy().contains("/devices/virtual/net/"))
        && fs::read_to_string(base.join("type"))
            .ok()
            .is_some_and(|kind| kind.trim() == "65534");
    if vpn_name || base.join("tun_flags").exists() || virtual_none {
        NetworkKind::Vpn
    } else if base.join("wireless").is_dir() {
        NetworkKind::Wifi
    } else {
        NetworkKind::Ethernet
    }
}

fn update_counters(
    snapshot: &mut NetworkSnapshot,
    sampler: &mut RateSampler,
    now: Instant,
) -> bool {
    if snapshot.kind == NetworkKind::Offline || !valid_interface_name(&snapshot.interface) {
        let changed = snapshot.rx_per_second.is_some() || snapshot.tx_per_second.is_some();
        snapshot.rx_per_second = None;
        snapshot.tx_per_second = None;
        return changed;
    }
    let base = Path::new("/sys/class/net")
        .join(&snapshot.interface)
        .join("statistics");
    let (Ok(rx), Ok(tx)) = (
        read_u64(base.join("rx_bytes")),
        read_u64(base.join("tx_bytes")),
    ) else {
        let changed = snapshot.rx_per_second.is_some() || snapshot.tx_per_second.is_some();
        snapshot.rx_per_second = None;
        snapshot.tx_per_second = None;
        return changed;
    };
    let (rx_rate, tx_rate) = sampler.sample(&snapshot.interface, rx, tx, now);
    let changed = snapshot.rx_bytes != rx
        || snapshot.tx_bytes != tx
        || snapshot.rx_per_second != rx_rate
        || snapshot.tx_per_second != tx_rate;
    snapshot.rx_bytes = rx;
    snapshot.tx_bytes = tx;
    snapshot.rx_per_second = rx_rate;
    snapshot.tx_per_second = tx_rate;
    changed
}

fn read_u64(path: impl AsRef<Path>) -> io::Result<u64> {
    let mut file = fs::File::open(path)?;
    let mut buffer = [0_u8; 32];
    let length = file.read(&mut buffer)?;
    let digits = buffer[..length]
        .iter()
        .copied()
        .take_while(u8::is_ascii_digit);
    let mut value = 0_u64;
    let mut found = false;
    for digit in digits {
        found = true;
        value = value
            .checked_mul(10)
            .and_then(|value| value.checked_add(u64::from(digit - b'0')))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "counter overflow"))?;
    }
    if found {
        Ok(value)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid counter",
        ))
    }
}

fn clear_probe(snapshot: &mut NetworkSnapshot) -> bool {
    let changed = snapshot.ping_ms.is_some()
        || snapshot.loss_percent.is_some()
        || snapshot.probe_error.is_some();
    snapshot.ping_ms = None;
    snapshot.loss_percent = None;
    snapshot.probe_error = None;
    changed
}

fn run_probe(target: IpAddr) -> ProbeOutcome {
    match Command::new("ping")
        .args(["-n", "-c", "1", "-W", "1", "-w", "1"])
        .arg(target.to_string())
        .env("LC_ALL", "C")
        .output()
    {
        Ok(output) => parse_ping_output(
            &String::from_utf8_lossy(&output.stdout),
            &String::from_utf8_lossy(&output.stderr),
        ),
        Err(error) => ProbeOutcome::Unavailable(short_error(&format!("ping unavailable: {error}"))),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Route {
    interface: String,
    gateway: Option<IpAddr>,
    metric: u32,
}

fn parse_ipv4_routes(input: &str) -> Vec<Route> {
    input
        .lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() < 8 || fields[0] == "Iface" || fields[0] == "lo" {
                return None;
            }
            let destination = u32::from_str_radix(fields[1], 16).ok()?;
            let gateway = u32::from_str_radix(fields[2], 16).ok()?;
            let flags = u32::from_str_radix(fields[3], 16).ok()?;
            let metric = fields[6].parse().ok()?;
            let mask = u32::from_str_radix(fields[7], 16).ok()?;
            if destination != 0 || mask != 0 || flags & 1 == 0 || flags & 0x200 != 0 {
                return None;
            }
            Some(Route {
                interface: fields[0].into(),
                gateway: (gateway != 0)
                    .then(|| IpAddr::V4(std::net::Ipv4Addr::from(gateway.to_le_bytes()))),
                metric,
            })
        })
        .collect()
}

fn parse_ipv6_routes(input: &str) -> Vec<Route> {
    input
        .lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() < 10 || fields[9] == "lo" {
                return None;
            }
            let destination = parse_ipv6_hex(fields[0])?;
            let prefix = u8::from_str_radix(fields[1], 16).ok()?;
            let gateway = parse_ipv6_hex(fields[4])?;
            let metric = u32::from_str_radix(fields[5], 16).ok()?;
            let flags = u32::from_str_radix(fields[8], 16).ok()?;
            if !destination.is_unspecified() || prefix != 0 || flags & 1 == 0 || flags & 0x200 != 0
            {
                return None;
            }
            Some(Route {
                interface: fields[9].into(),
                gateway: (!gateway.is_unspecified()).then_some(IpAddr::V6(gateway)),
                metric,
            })
        })
        .collect()
}

fn parse_ipv6_hex(value: &str) -> Option<std::net::Ipv6Addr> {
    if value.len() != 32 {
        return None;
    }
    let mut octets = [0; 16];
    for (index, octet) in octets.iter_mut().enumerate() {
        *octet = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(octets.into())
}

#[derive(Default)]
struct RateSampler {
    previous: Option<CounterSample>,
}

struct CounterSample {
    interface: String,
    rx: u64,
    tx: u64,
    at: std::time::Instant,
}

impl RateSampler {
    fn sample(
        &mut self,
        interface: &str,
        rx: u64,
        tx: u64,
        now: std::time::Instant,
    ) -> (Option<u64>, Option<u64>) {
        let rates = self.previous.as_ref().and_then(|previous| {
            if previous.interface != interface || rx < previous.rx || tx < previous.tx {
                return None;
            }
            let nanos = now.checked_duration_since(previous.at)?.as_nanos();
            if nanos == 0 {
                return None;
            }
            let per_second = |delta: u64| {
                ((u128::from(delta) * 1_000_000_000) / nanos).min(u128::from(u64::MAX)) as u64
            };
            Some((
                Some(per_second(rx - previous.rx)),
                Some(per_second(tx - previous.tx)),
            ))
        });
        self.previous = Some(CounterSample {
            interface: interface.into(),
            rx,
            tx,
            at: now,
        });
        rates.unwrap_or((None, None))
    }
}

#[derive(Debug, PartialEq)]
enum ProbeOutcome {
    Sent(Option<f64>),
    Unavailable(String),
}

fn parse_ping_output(stdout: &str, stderr: &str) -> ProbeOutcome {
    let transmitted = stdout.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let count = fields.next()?.parse::<u32>().ok()?;
        (fields.next() == Some("packets")
            && fields
                .next()
                .is_some_and(|field| field.trim_end_matches(',') == "transmitted"))
        .then_some(count)
    });
    if transmitted.is_none_or(|count| count == 0) {
        let message = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        return ProbeOutcome::Unavailable(short_error(message));
    }

    let rtt = stdout.split_whitespace().find_map(|field| {
        field
            .strip_prefix("time=")
            .and_then(|value| value.parse::<f64>().ok())
            .or_else(|| {
                field
                    .strip_prefix("time<")
                    .and_then(|value| value.parse().ok())
            })
    });
    ProbeOutcome::Sent(rtt)
}

fn short_error(message: &str) -> String {
    const MAX_CHARS: usize = 256;
    let end = message
        .char_indices()
        .nth(MAX_CHARS)
        .map_or(message.len(), |(index, _)| index);
    if end == 0 {
        "ping unavailable".into()
    } else {
        message[..end].into()
    }
}

#[derive(Default)]
struct LossWindow(VecDeque<bool>);

impl LossWindow {
    fn record(&mut self, received: bool) {
        if self.0.len() == 20 {
            self.0.pop_front();
        }
        self.0.push_back(received);
    }

    fn percent(&self) -> Option<u8> {
        (!self.0.is_empty()).then(|| {
            let lost = self.0.iter().filter(|received| !**received).count();
            ((lost * 100) / self.0.len()) as u8
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        net::{IpAddr, Ipv4Addr, Ipv6Addr},
        time::{Duration, Instant},
    };

    use super::*;

    #[test]
    fn ipv4_default_route_requires_a_zero_mask() {
        let input = "Iface Destination Gateway Flags RefCnt Use Metric Mask MTU Window IRTT\n\
                     tun0 00000000 00000000 0001 0 0 1 00000080 0 0 0\n\
                     eth0 00000000 0101A8C0 0003 0 0 50 00000000 0 0 0\n";

        assert_eq!(
            parse_ipv4_routes(input),
            vec![Route {
                interface: "eth0".into(),
                gateway: Some(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))),
                metric: 50,
            }]
        );
    }

    #[test]
    fn ipv4_routes_reject_down_reject_and_loopback_entries() {
        let input = "Iface Destination Gateway Flags RefCnt Use Metric Mask MTU Window IRTT\n\
                     eth0 00000000 0101A8C0 0000 0 0 1 00000000 0 0 0\n\
                     eth1 00000000 0100000A 0201 0 0 2 00000000 0 0 0\n\
                     lo 00000000 00000000 0001 0 0 0 00000000 0 0 0\n";

        assert!(parse_ipv4_routes(input).is_empty());
    }

    #[test]
    fn lowest_route_metric_wins() {
        let input = "Iface Destination Gateway Flags RefCnt Use Metric Mask MTU Window IRTT\n\
                     eth0 00000000 0101A8C0 0003 0 0 600 00000000 0 0 0\n\
                     wg0 00000000 00000000 0001 0 0 5 00000000 0 0 0\n";
        let routes = parse_ipv4_routes(input);

        assert_eq!(
            routes
                .iter()
                .min_by_key(|route| route.metric)
                .unwrap()
                .interface,
            "wg0"
        );
    }

    #[test]
    fn ipv6_default_route_requires_zero_destination_and_prefix() {
        let input = concat!(
            "00000000000000000000000000000000 01 00000000000000000000000000000000 00 ",
            "00000000000000000000000000000000 00000001 00000000 00000000 00000001 tun0\n",
            "00000000000000000000000000000000 00 00000000000000000000000000000000 00 ",
            "fe800000000000000000000000000001 00000064 00000000 00000000 00000003 eth0\n",
        );

        assert_eq!(
            parse_ipv6_routes(input),
            vec![Route {
                interface: "eth0".into(),
                gateway: Some(IpAddr::V6("fe80::1".parse::<Ipv6Addr>().unwrap())),
                metric: 100,
            }]
        );
    }

    #[test]
    fn rates_use_actual_elapsed_time() {
        let start = Instant::now();
        let mut sampler = RateSampler::default();
        assert_eq!(sampler.sample("eth0", 1_000, 5_000, start), (None, None));

        assert_eq!(
            sampler.sample("eth0", 4_000, 6_500, start + Duration::from_millis(1500)),
            (Some(2_000), Some(1_000))
        );
    }

    #[test]
    fn rates_reset_on_interface_or_counter_reset() {
        let start = Instant::now();
        let mut sampler = RateSampler::default();
        sampler.sample("eth0", 10_000, 20_000, start);

        assert_eq!(
            sampler.sample("wg0", 12_000, 23_000, start + Duration::from_secs(1)),
            (None, None)
        );
        assert_eq!(
            sampler.sample("wg0", 2_000, 3_000, start + Duration::from_secs(2)),
            (None, None)
        );
        assert_eq!(
            sampler.sample("wg0", 2_500, 4_000, start + Duration::from_secs(3)),
            (Some(500), Some(1_000))
        );
    }

    #[test]
    fn ping_success_parses_rtt() {
        let stdout = "64 bytes from 1.1.1.1: icmp_seq=1 ttl=58 time=12.4 ms\n\
                      1 packets transmitted, 1 received, 0% packet loss, time 0ms\n";

        assert_eq!(
            parse_ping_output(stdout, ""),
            ProbeOutcome::Sent(Some(12.4))
        );
    }

    #[test]
    fn ping_loss_counts_only_reported_transmission() {
        let stdout = "PING 1.1.1.1 (1.1.1.1) 56(84) bytes of data.\n\
                      1 packets transmitted, 0 received, 100% packet loss, time 0ms\n";

        assert_eq!(parse_ping_output(stdout, ""), ProbeOutcome::Sent(None));
    }

    #[test]
    fn ping_without_a_transmission_is_unavailable() {
        assert_eq!(
            parse_ping_output("", "ping: socket: Operation not permitted\n"),
            ProbeOutcome::Unavailable("ping: socket: Operation not permitted".into())
        );
    }

    #[test]
    fn rolling_loss_keeps_only_the_last_twenty_sent_probes() {
        let mut loss = LossWindow::default();
        loss.record(false);
        for _ in 0..20 {
            loss.record(true);
        }

        assert_eq!(loss.percent(), Some(0));
        loss.record(false);
        assert_eq!(loss.percent(), Some(5));
    }

    #[test]
    fn network_kind_defaults_to_offline() {
        assert_eq!(NetworkKind::default(), NetworkKind::Offline);
    }

    #[test]
    fn connection_changes_discard_old_measurements() {
        let mut snapshot = NetworkSnapshot {
            interface: "eth0".into(),
            kind: NetworkKind::Ethernet,
            ping_ms: Some(12.0),
            loss_percent: Some(0),
            rx_per_second: Some(100),
            tx_per_second: Some(200),
            ..NetworkSnapshot::default()
        };
        let offline = NetworkSnapshot {
            interface: "eth0".into(),
            ..NetworkSnapshot::default()
        };
        assert!(apply_connectivity(&mut snapshot, offline));
        assert_eq!(snapshot.ping_ms, None);
        assert_eq!(snapshot.loss_percent, None);
        assert_eq!(snapshot.rx_per_second, None);
        assert_eq!(snapshot.tx_per_second, None);
    }
}
