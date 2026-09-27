use std::{
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    time::Instant,
};

#[derive(Default)]
pub struct Meter {
    previous: Option<((u32, u32), Instant)>,
}

impl Meter {
    pub fn reset(&mut self) {
        self.previous = None;
    }

    pub fn sample(&mut self, counters: (u32, u32), now: Instant) -> Option<(u64, u64)> {
        let (previous, time) = self.previous.replace((counters, now))?;
        let elapsed = now.saturating_duration_since(time).as_secs_f64();
        if elapsed == 0.0 {
            return None;
        }
        // A reset or 32-bit wrap invalidates one interval, never spikes the display.
        Some((
            (counters.0.checked_sub(previous.0)? as f64 / elapsed) as u64,
            (counters.1.checked_sub(previous.1)? as f64 / elapsed) as u64,
        ))
    }
}

// Linux uapi bluetooth/hci.h. This ioctl only reads adapter statistics.
#[repr(C)]
#[derive(Default)]
struct HciInfo {
    id: u16,
    name: [u8; 8],
    address: [u8; 6],
    flags: u32,
    kind: u8,
    features: [u8; 8],
    packet_type: u32,
    link_policy: u32,
    link_mode: u32,
    acl_mtu: u16,
    acl_packets: u16,
    sco_mtu: u16,
    sco_packets: u16,
    stats: [u32; 10],
}

pub struct Reader(OwnedFd);

impl Reader {
    pub fn open() -> io::Result<Self> {
        // SAFETY: socket has no pointer arguments; successful fd ownership transfers once.
        let fd =
            unsafe { libc::socket(libc::AF_BLUETOOTH, libc::SOCK_RAW | libc::SOCK_CLOEXEC, 1) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    pub fn read(&self, path: &str) -> io::Result<(u32, u32)> {
        let id = path
            .rsplit('/')
            .next()
            .and_then(|s| s.strip_prefix("hci"))
            .and_then(|s| s.parse::<u16>().ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid adapter path"))?;
        let mut info = HciInfo {
            id,
            ..Default::default()
        };
        // _IOR('H', 211, int), size is int (not hci_dev_info) in Linux's ABI.
        const HCIGETDEVINFO: libc::c_ulong = 0x800448d3;
        // SAFETY: kernel writes one correctly aligned, initialized hci_dev_info.
        if unsafe { libc::ioctl(self.0.as_raw_fd(), HCIGETDEVINFO, &mut info) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((info.stats[8], info.stats[9]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn throughput_uses_elapsed_time_and_discards_counter_resets() {
        let now = Instant::now();
        let mut meter = Meter::default();
        assert_eq!(meter.sample((100, 200), now), None);
        assert_eq!(
            meter.sample((2148, 1224), now + Duration::from_secs(2)),
            Some((1024, 512))
        );
        assert_eq!(meter.sample((0, 0), now + Duration::from_secs(3)), None);
        assert_eq!(
            meter.sample((1024, 2048), now + Duration::from_secs(4)),
            Some((1024, 2048))
        );
    }

    #[test]
    fn failed_samples_do_not_turn_into_zero_traffic() {
        let mut meter = Meter::default();
        let now = Instant::now();
        meter.sample((500, 600), now);
        meter.reset();
        assert_eq!(
            meter.sample((900, 1200), now + Duration::from_secs(1)),
            None
        );
    }
}
