mod audio;
mod traffic;

use std::{
    collections::{HashMap, HashSet},
    future::{pending, poll_fn},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use smithay_client_toolkit::reexports::calloop::channel::Sender as UiSender;
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinSet,
    time::timeout,
};
use zbus::{
    Connection, MatchRule, MessageStream,
    export::futures_core::Stream,
    message::Type,
    zvariant::{OwnedObjectPath, OwnedValue},
};

const AGENT_PATH: &str = "/org/nibari/BluetoothAgent";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Adapter {
    pub path: String,
    pub name: String,
    pub powered: bool,
    pub discovering: bool,
    pub scan_owned: bool,
    pub rate: Option<(u64, u64)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Device {
    pub path: String,
    pub adapter: String,
    pub address: String,
    pub name: String,
    pub paired: bool,
    pub connected: bool,
    pub icon: String,
    pub battery: Option<u8>,
    pub audio: Option<audio::Card>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    Confirm,
    Pin,
    Passkey,
    Display,
}

impl PromptKind {
    pub fn accepts(self, input: &str) -> bool {
        match self {
            Self::Pin => {
                !input.is_empty()
                    && input.len() <= 16
                    && input.is_ascii()
                    && !input.chars().any(char::is_control)
            }
            Self::Passkey => input.len() == 6 && input.bytes().all(|b| b.is_ascii_digit()),
            Self::Confirm => true,
            Self::Display => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub id: u64,
    pub device: String,
    pub text: String,
    pub kind: PromptKind,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub available: bool,
    pub adapters: Vec<Adapter>,
    pub devices: Vec<Device>,
    pub busy: Option<Action>,
    pub prompt: Option<Prompt>,
    pub error: Option<String>,
    pub audio_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Power(String, bool),
    Scan(String, bool),
    Pair(String),
    Connect(String),
    Disconnect(String),
    Profile { device: String, name: String },
    Codec { device: String, name: String },
}

impl Snapshot {
    pub fn allows(&self, action: &Action) -> bool {
        if !self.available || self.busy.is_some() {
            return false;
        }
        match action {
            Action::Power(path, _) => self.adapters.iter().any(|a| &a.path == path),
            Action::Scan(path, _) => self.adapters.iter().any(|a| &a.path == path && a.powered),
            Action::Pair(path) => self.devices.iter().any(|d| &d.path == path && !d.paired),
            Action::Connect(path) => self
                .devices
                .iter()
                .any(|d| &d.path == path && d.paired && !d.connected),
            Action::Disconnect(path) => self.devices.iter().any(|d| &d.path == path && d.connected),
            Action::Profile { device, name } => self
                .devices
                .iter()
                .find(|d| &d.path == device && d.connected)
                .and_then(|d| d.audio.as_ref())
                .is_some_and(|c| c.profiles.iter().any(|p| &p.name == name && p.available)),
            Action::Codec { device, name } => self
                .devices
                .iter()
                .find(|d| &d.path == device && d.connected)
                .and_then(|d| d.audio.as_ref())
                .is_some_and(|c| c.codecs.iter().any(|p| &p.name == name)),
        }
    }

    pub fn icon_state(&self) -> u8 {
        if self.devices.iter().any(|d| d.connected) {
            2
        } else if self.adapters.iter().any(|a| a.powered) {
            1
        } else {
            0
        }
    }
}

enum Command {
    Visible(bool),
    Act(Action),
    Reply(u64, Option<String>),
    CancelPair,
}

pub struct BluetoothHandle {
    commands: mpsc::Sender<Command>,
    stop: Option<oneshot::Sender<()>>,
}

impl BluetoothHandle {
    pub fn spawn(events: UiSender<Arc<Snapshot>>) -> Result<Self> {
        let (commands, mut receiver) = mpsc::channel(32);
        let (stop, mut stopped) = oneshot::channel();
        thread::Builder::new().name("nibari-bluetooth".into()).spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()
                .expect("Bluetooth runtime");
            runtime.block_on(async {
                // Cleanup on cancellation is owned by the connection: dropping it releases
                // this client's BlueZ discovery session and agent registration.
                tokio::select! {
                    _ = &mut stopped => {},
                    result = worker(&events, &mut receiver) => {
                        if let Err(error) = result {
                            let _ = events.send(Arc::new(Snapshot { error: Some(format!("Bluetooth: {error}")), ..Default::default() }));
                        }
                    }
                }
            });
        })?;
        Ok(Self {
            commands,
            stop: Some(stop),
        })
    }

    pub fn set_visible(&self, visible: bool) {
        let _ = self.commands.try_send(Command::Visible(visible));
    }

    pub fn act(&self, action: Action) {
        let _ = self.commands.try_send(Command::Act(action));
    }

    pub fn cancel_pairing(&self) {
        let _ = self.commands.try_send(Command::CancelPair);
    }

    pub fn reply(&self, id: u64, input: Option<String>) {
        let _ = self.commands.try_send(Command::Reply(id, input));
    }
}

impl Drop for BluetoothHandle {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

struct AgentEvent {
    prompt: Option<Prompt>,
    reply: Option<oneshot::Sender<Option<String>>>,
}

struct Agent {
    events: mpsc::Sender<AgentEvent>,
    serial: AtomicU64,
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.bluez.Error")]
enum AgentError {
    Rejected(String),
    Canceled(String),
}

impl Agent {
    async fn request(
        &self,
        path: OwnedObjectPath,
        text: String,
        kind: PromptKind,
    ) -> std::result::Result<String, AgentError> {
        let (reply, received) = oneshot::channel();
        let prompt = Prompt {
            id: self.serial.fetch_add(1, Ordering::Relaxed),
            device: path.to_string(),
            text,
            kind,
        };
        self.events
            .send(AgentEvent {
                prompt: Some(prompt),
                reply: Some(reply),
            })
            .await
            .map_err(|_| AgentError::Canceled("Applet closed".into()))?;
        match timeout(Duration::from_secs(90), received).await {
            Ok(Ok(Some(input))) if kind.accepts(&input) => Ok(input),
            _ => Err(AgentError::Rejected("Pairing rejected or canceled".into())),
        }
    }

    async fn display(&self, path: OwnedObjectPath, text: String) {
        let prompt = Prompt {
            id: self.serial.fetch_add(1, Ordering::Relaxed),
            device: path.to_string(),
            text,
            kind: PromptKind::Display,
        };
        let _ = self
            .events
            .send(AgentEvent {
                prompt: Some(prompt),
                reply: None,
            })
            .await;
    }
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl Agent {
    async fn release(&self) {
        self.cancel().await;
    }

    async fn cancel(&self) {
        let _ = self
            .events
            .send(AgentEvent {
                prompt: None,
                reply: None,
            })
            .await;
    }

    async fn request_pin_code(
        &self,
        device: OwnedObjectPath,
    ) -> std::result::Result<String, AgentError> {
        self.request(device, "Enter the device PIN".into(), PromptKind::Pin)
            .await
    }

    async fn request_passkey(
        &self,
        device: OwnedObjectPath,
    ) -> std::result::Result<u32, AgentError> {
        self.request(
            device,
            "Enter the six-digit passkey".into(),
            PromptKind::Passkey,
        )
        .await?
        .parse()
        .map_err(|_| AgentError::Rejected("Invalid passkey".into()))
    }

    async fn display_pin_code(&self, device: OwnedObjectPath, pincode: String) {
        self.display(device, format!("Enter PIN {pincode} on the device"))
            .await;
    }

    async fn display_passkey(&self, device: OwnedObjectPath, passkey: u32, entered: u16) {
        self.display(
            device,
            format!("Enter {passkey:06} on the device ({entered}/6)"),
        )
        .await;
    }

    async fn request_confirmation(
        &self,
        device: OwnedObjectPath,
        passkey: u32,
    ) -> std::result::Result<(), AgentError> {
        self.request(
            device,
            format!("Does the device show {passkey:06}?"),
            PromptKind::Confirm,
        )
        .await
        .map(|_| ())
    }

    async fn request_authorization(
        &self,
        device: OwnedObjectPath,
    ) -> std::result::Result<(), AgentError> {
        self.request(
            device,
            "Allow pairing with this device?".into(),
            PromptKind::Confirm,
        )
        .await
        .map(|_| ())
    }

    async fn authorize_service(
        &self,
        device: OwnedObjectPath,
        uuid: String,
    ) -> std::result::Result<(), AgentError> {
        self.request(
            device,
            format!("Allow service {uuid}?"),
            PromptKind::Confirm,
        )
        .await
        .map(|_| ())
    }
}

async fn proxy<'a>(
    connection: &'a Connection,
    path: &'a str,
    interface: &'a str,
) -> Result<zbus::Proxy<'a>> {
    Ok(zbus::Proxy::new(connection, "org.bluez", path, interface).await?)
}

async fn unit_call(
    connection: &Connection,
    path: &str,
    interface: &str,
    method: &str,
) -> Result<()> {
    let p = proxy(connection, path, interface).await?;
    Ok(p.call::<_, _, ()>(method, &()).await?)
}

async fn operation(
    connection: Connection,
    action: Action,
    card: Option<audio::Card>,
) -> Result<()> {
    let duration = if matches!(action, Action::Pair(_)) {
        120
    } else {
        15
    };
    timeout(Duration::from_secs(duration), async {
        match action {
            Action::Power(path, enabled) => {
                proxy(&connection, &path, ADAPTER)
                    .await?
                    .set_property("Powered", enabled)
                    .await?
            }
            Action::Scan(path, enabled) => {
                unit_call(
                    &connection,
                    &path,
                    ADAPTER,
                    if enabled {
                        "StartDiscovery"
                    } else {
                        "StopDiscovery"
                    },
                )
                .await?
            }
            Action::Connect(path) => unit_call(&connection, &path, DEVICE, "Connect").await?,
            Action::Disconnect(path) => unit_call(&connection, &path, DEVICE, "Disconnect").await?,
            Action::Pair(path) => {
                unit_call(&connection, &path, DEVICE, "Pair").await?;
                unit_call(&connection, &path, DEVICE, "Connect").await?;
            }
            Action::Profile { name, .. } => {
                let card = card.context("Audio device disappeared")?;
                audio::pactl(&["set-card-profile", &card.name, &name]).await?;
            }
            Action::Codec { name, .. } => {
                let card = card.context("Audio device disappeared")?;
                let encoded = serde_json::to_string(&name)?;
                audio::pactl(&[
                    "send-message",
                    &card.message_path(),
                    "switch-codec",
                    &encoded,
                ])
                .await?;
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("Bluetooth operation timed out")?
}

fn string(props: &HashMap<String, OwnedValue>, key: &str) -> String {
    props
        .get(key)
        .and_then(|v| <&str>::try_from(v).ok())
        .unwrap_or("")
        .to_owned()
}

fn flag(props: &HashMap<String, OwnedValue>, key: &str) -> bool {
    props
        .get(key)
        .and_then(|v| bool::try_from(v).ok())
        .unwrap_or(false)
}

async fn refresh(connection: &Connection, state: &mut Snapshot) -> Result<()> {
    let objects: Objects = timeout(Duration::from_secs(3), async {
        let p = proxy(connection, "/", "org.freedesktop.DBus.ObjectManager").await?;
        Ok::<Objects, anyhow::Error>(p.call("GetManagedObjects", &()).await?)
    })
    .await??;
    let mut adapters = Vec::new();
    let mut devices = Vec::new();
    for (path, interfaces) in objects {
        if let Some(p) = interfaces.get(ADAPTER) {
            let rate = state
                .adapters
                .iter()
                .find(|a| a.path == path.as_str())
                .and_then(|a| a.rate);
            adapters.push(Adapter {
                path: path.to_string(),
                name: string(p, "Alias"),
                powered: flag(p, "Powered"),
                discovering: flag(p, "Discovering"),
                scan_owned: false,
                rate,
            });
        }
        if let Some(p) = interfaces.get(DEVICE) {
            let address = string(p, "Address");
            let name = string(p, "Alias");
            let adapter = p
                .get("Adapter")
                .and_then(|v| <&zbus::zvariant::ObjectPath>::try_from(v).ok())
                .map_or_else(
                    || {
                        path.as_str()
                            .rsplit_once('/')
                            .map_or("", |(p, _)| p)
                            .to_owned()
                    },
                    |p| p.to_string(),
                );
            let audio = state
                .devices
                .iter()
                .find(|d| d.path == path.as_str())
                .and_then(|d| d.audio.clone());
            let battery = interfaces
                .get("org.bluez.Battery1")
                .and_then(|p| p.get("Percentage"))
                .and_then(|v| u8::try_from(v).ok());
            devices.push(Device {
                path: path.to_string(),
                adapter,
                address: address.clone(),
                name: if name.is_empty() { address } else { name },
                paired: flag(p, "Paired"),
                connected: flag(p, "Connected"),
                icon: string(p, "Icon"),
                battery,
                audio,
            });
        }
    }
    adapters.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    devices.sort_unstable_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then(b.paired.cmp(&a.paired))
            .then(a.name.cmp(&b.name))
            .then(a.path.cmp(&b.path))
    });
    if !state.available {
        state.error = None;
    }
    state.available = true;
    state.adapters = adapters;
    state.devices = devices;
    Ok(())
}

async fn refresh_audio(devices: Vec<(String, String)>) -> Result<Vec<audio::Card>> {
    let mut cards = audio::cards().await?;
    cards.retain(|c| {
        devices
            .iter()
            .any(|(path, address)| c.matches(path, address))
    });
    for card in &mut cards {
        audio::load_codecs(card).await;
    }
    Ok(cards)
}

fn apply_audio(state: &mut Snapshot, result: Result<Vec<audio::Card>>) {
    for device in &mut state.devices {
        device.audio = None;
    }
    match result {
        Ok(mut cards) => {
            state.audio_error = None;
            for device in state.devices.iter_mut().filter(|d| d.connected) {
                if let Some(index) = cards
                    .iter()
                    .position(|c| c.matches(&device.path, &device.address))
                {
                    device.audio = Some(cards.swap_remove(index));
                }
            }
        }
        Err(error) => state.audio_error = Some(format!("PipeWire: {error}")),
    }
}

async fn signals(connection: &Connection, owner: bool) -> Result<MessageStream> {
    let builder = MatchRule::builder().msg_type(Type::Signal);
    let rule = if owner {
        builder
            .interface("org.freedesktop.DBus")?
            .member("NameOwnerChanged")?
            .arg(0, "org.bluez")?
            .build()
    } else {
        builder.sender("org.bluez")?.build()
    };
    Ok(MessageStream::for_match_rule(rule, connection, Some(128)).await?)
}

async fn worker(
    events: &UiSender<Arc<Snapshot>>,
    commands: &mut mpsc::Receiver<Command>,
) -> Result<()> {
    let connection = timeout(Duration::from_secs(5), Connection::system()).await??;
    connected_worker(connection, events, commands).await
}

async fn connected_worker(
    connection: Connection,
    events: &UiSender<Arc<Snapshot>>,
    commands: &mut mpsc::Receiver<Command>,
) -> Result<()> {
    let mut changes = signals(&connection, false).await?;
    let mut owners = signals(&connection, true).await?;
    let (agent_tx, mut agent_rx) = mpsc::channel(8);
    connection
        .object_server()
        .at(
            AGENT_PATH,
            Agent {
                events: agent_tx,
                serial: AtomicU64::new(1),
            },
        )
        .await?;
    let (audio_tx, mut audio_rx) = mpsc::channel(1);
    let mut audio_watch = JoinSet::new();
    let mut audio_tasks = JoinSet::new();
    let mut tasks = JoinSet::new();
    let mut cleanup = JoinSet::new();
    let mut state = Snapshot::default();
    let mut sent: Option<Arc<Snapshot>> = None;
    let mut visible = false;
    let mut registered = false;
    let mut dirty = true;
    let mut audio_dirty = false;
    let mut due = Instant::now();
    let mut sample_at = Instant::now();
    let mut reader = None;
    let mut meters: HashMap<String, traffic::Meter> = HashMap::new();
    let mut scans: HashMap<String, Instant> = HashMap::new();
    let mut reply: Option<oneshot::Sender<Option<String>>> = None;
    loop {
        if visible && audio_dirty && audio_tasks.is_empty() {
            let devices = state
                .devices
                .iter()
                .filter(|d| d.connected)
                .map(|d| (d.path.clone(), d.address.clone()))
                .collect();
            audio_tasks.spawn(refresh_audio(devices));
            audio_dirty = false;
        }
        for adapter in &mut state.adapters {
            adapter.scan_owned = scans.contains_key(&adapter.path);
        }
        if sent.as_deref() != Some(&state) {
            let snapshot = Arc::new(state.clone());
            if events.send(snapshot.clone()).is_err() {
                break;
            }
            sent = Some(snapshot);
        }
        let now = Instant::now();
        let next = if dirty {
            Some(due)
        } else if !state.available {
            Some(due.max(now))
        } else {
            None
        };
        let next = if visible {
            Some(next.map_or(sample_at, |n| n.min(sample_at)))
        } else {
            next
        };
        tokio::select! {
            command = commands.recv() => match command {
                None => break,
                Some(Command::Visible(value)) => {
                    visible = value;
                    if value {
                        dirty = true; audio_dirty = true; due = Instant::now(); sample_at = due;
                        reader = traffic::Reader::open().ok();
                        meters.clear();
                        if audio_watch.is_empty() { audio_watch.spawn(audio::watch(audio_tx.clone())); }
                    } else {
                        audio_watch = JoinSet::new(); audio_tasks = JoinSet::new(); audio_dirty = false;
                        reader = None; meters.clear();
                        for adapter in &mut state.adapters { adapter.rate = None; }
                        reply.take(); state.prompt = None;
                        if let Some(Action::Pair(path)) = &state.busy {
                            let c = connection.clone(); let path = path.clone();
                            cleanup.spawn(async move { timeout(Duration::from_secs(3), unit_call(&c, &path, DEVICE, "CancelPairing")).await });
                        }
                        for (path, _) in scans.drain() {
                            let c = connection.clone();
                            cleanup.spawn(async move { timeout(Duration::from_secs(3), unit_call(&c, &path, ADAPTER, "StopDiscovery")).await });
                        }
                    }
                }
                Some(Command::CancelPair) => {
                    reply.take(); state.prompt = None;
                    if let Some(Action::Pair(path)) = &state.busy {
                        let c = connection.clone(); let path = path.clone();
                        cleanup.spawn(async move { timeout(Duration::from_secs(3), unit_call(&c, &path, DEVICE, "CancelPairing")).await });
                    }
                }
                Some(Command::Reply(id, input)) => {
                    if state.prompt.as_ref().is_some_and(|p| p.id == id) {
                        if let Some(sender) = reply.take() { let _ = sender.send(input); }
                        else if let Some(Action::Pair(path)) = &state.busy {
                            let c = connection.clone(); let path = path.clone();
                            cleanup.spawn(async move { timeout(Duration::from_secs(3), unit_call(&c, &path, DEVICE, "CancelPairing")).await });
                        }
                        state.prompt = None;
                    }
                }
                Some(Command::Act(action)) => {
                    if !visible || !state.allows(&action) { continue; }
                    if matches!(action, Action::Pair(_)) && !registered {
                        state.error = Some("Bluetooth pairing agent is unavailable".into()); continue;
                    }
                    // Do not stop discovery sessions belonging to other applications.
                    if let Action::Scan(path, false) = &action && !scans.contains_key(path) { continue; }
                    let card = match &action {
                        Action::Profile { device, .. } | Action::Codec { device, .. } => state.devices.iter().find(|d| &d.path == device).and_then(|d| d.audio.clone()),
                        _ => None,
                    };
                    state.error = None;
                    state.busy = Some(action.clone());
                    let c = connection.clone();
                    tasks.spawn(async move { let result = operation(c, action.clone(), card).await; (action, result) });
                }
            },
            event = agent_rx.recv() => if let Some(event) = event
                && event.prompt.as_ref().is_none_or(|p| visible && matches!(&state.busy, Some(Action::Pair(path)) if *path == p.device)) {
                reply = event.reply;
                state.prompt = event.prompt;
            },
            result = tasks.join_next(), if !tasks.is_empty() => {
                state.busy = None; state.prompt = None; reply.take();
                match result {
                    Some(Ok((action, result))) => {
                        if let Action::Power(path, powered) = &action
                            && result.is_ok()
                            && let Some(adapter) = state.adapters.iter_mut().find(|a| &a.path == path)
                        {
                            adapter.powered = *powered;
                            if !*powered {
                                adapter.discovering = false;
                                adapter.rate = None;
                            }
                        }
                        if let Action::Scan(path, enabled) = &action && result.is_ok() {
                                if *enabled && visible { scans.insert(path.clone(), Instant::now() + Duration::from_secs(30)); }
                                else {
                                    scans.remove(path);
                                    if *enabled {
                                        let c = connection.clone(); let path = path.clone();
                                        cleanup.spawn(async move { timeout(Duration::from_secs(3), unit_call(&c, &path, ADAPTER, "StopDiscovery")).await });
                                    }
                                }
                        }
                        if let Err(error) = result { state.error = Some(error.to_string()); }
                    }
                    Some(Err(error)) => state.error = Some(error.to_string()),
                    None => {},
                }
                dirty = true; audio_dirty = true; due = Instant::now() + Duration::from_millis(150);
            },
            _ = cleanup.join_next(), if !cleanup.is_empty() => {},
            _ = audio_watch.join_next(), if !audio_watch.is_empty() => {},
            result = audio_tasks.join_next(), if !audio_tasks.is_empty() => {
                if visible && !audio_dirty && let Some(Ok(result)) = result { apply_audio(&mut state, result); }
            },
            message = poll_fn(|cx| std::pin::Pin::new(&mut changes).poll_next(cx)) => {
                match message { Some(Ok(_)) => {}, _ => bail!("Bluetooth bus disconnected") }
                if !dirty { due = Instant::now() + Duration::from_millis(150); }
                dirty = true;
                // Discovery RSSI changes must not spawn pactl for every nearby device.
            },
            message = poll_fn(|cx| std::pin::Pin::new(&mut owners).poll_next(cx)) => {
                match message { Some(Ok(_)) => {}, _ => bail!("Bluetooth bus disconnected") }
                registered = false; scans.clear(); meters.clear(); reply.take();
                state = Snapshot::default(); tasks = JoinSet::new(); audio_tasks = JoinSet::new(); cleanup = JoinSet::new();
                dirty = true; audio_dirty = true; due = Instant::now();
            },
            _ = audio_rx.recv() => { audio_dirty = true; dirty = true; due = Instant::now() + Duration::from_millis(150); },
            _ = async { match next { Some(at) => tokio::time::sleep_until(at.into()).await, None => pending::<()>().await } } => {
                let now = Instant::now();
                if (dirty || !state.available) && now >= due {
                    let previous_connections: HashSet<_> = state.devices.iter().filter(|d| d.connected).map(|d| d.path.clone()).collect();
                    match refresh(&connection, &mut state).await {
                        Ok(()) => {
                            if !registered {
                                let registration = async {
                                    let p = proxy(&connection, "/org/bluez", "org.bluez.AgentManager1").await?;
                                    p.call::<_, _, ()>("RegisterAgent", &(zbus::zvariant::ObjectPath::try_from(AGENT_PATH)?, "KeyboardDisplay")).await?;
                                    Ok::<_, anyhow::Error>(())
                                };
                                registered = timeout(Duration::from_secs(3), registration).await.is_ok_and(|r| r.is_ok());
                            }
                            if visible && (audio_dirty || state.devices.iter().filter(|d| d.connected).map(|d| d.path.clone()).collect::<HashSet<_>>() != previous_connections) {
                                audio_dirty = true;
                            }
                        }
                        Err(error) => {
                            state.available = false; state.adapters.clear(); state.devices.clear();
                            state.error = Some(format!("BlueZ unavailable: {error}"));
                            registered = false;
                        }
                    }
                    dirty = false; due = Instant::now() + Duration::from_secs(5);
                }
                if visible && now >= sample_at {
                    meters.retain(|path,_| state.adapters.iter().any(|a| &a.path == path));
                    for adapter in &mut state.adapters {
                        let meter = meters.entry(adapter.path.clone()).or_default();
                        adapter.rate = if adapter.powered {
                            match reader.as_ref().and_then(|r| r.read(&adapter.path).ok()) {
                                Some(counters) => meter.sample(counters, Instant::now()),
                                None => { meter.reset(); None }
                            }
                        } else { meter.reset(); None };
                    }
                    sample_at = Instant::now() + Duration::from_secs(1);
                    scans.retain(|path, expiry| {
                        if *expiry > now { return true; }
                        let c = connection.clone(); let path = path.clone();
                        cleanup.spawn(async move { timeout(Duration::from_secs(3), unit_call(&c, &path, ADAPTER, "StopDiscovery")).await });
                        false
                    });
                }
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    mod integration;

    #[test]
    fn stale_audio_choices_are_rejected_before_running_commands() {
        let state = Snapshot::default();
        assert!(!state.allows(&Action::Profile {
            device: "/gone".into(),
            name: "a2dp-sink".into()
        }));
        assert!(!state.allows(&Action::Codec {
            device: "/gone".into(),
            name: "6".into()
        }));
        assert!(!state.allows(&Action::Connect("/gone".into())));
    }

    #[test]
    fn passkeys_are_six_digits_and_pins_are_bounded() {
        assert!(PromptKind::Passkey.accepts("000123"));
        assert!(!PromptKind::Passkey.accepts("1234567"));
        assert!(!PromptKind::Passkey.accepts("12x"));
        assert!(!PromptKind::Pin.accepts(""));
        assert!(PromptKind::Pin.accepts("1234"));
        assert!(!PromptKind::Pin.accepts("12345678901234567"));
    }
}
