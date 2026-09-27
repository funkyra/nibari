use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize};

const TEST_ADAPTER: &str = "/org/bluez/hci0";
const TEST_DEVICE: &str = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF";

struct Bus(std::process::Child, String);

impl Bus {
    fn start() -> Self {
        use std::io::BufRead;
        let mut child = std::process::Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut address = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        Self(child, address.trim().into())
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct TestAdapter {
    powered: bool,
    reads: Arc<AtomicUsize>,
}

#[zbus::interface(name = "org.bluez.Adapter1")]
impl TestAdapter {
    #[zbus(property)]
    fn alias(&self) -> &str {
        "Test adapter"
    }

    #[zbus(property)]
    fn powered(&self) -> bool {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.powered
    }

    #[zbus(property)]
    fn set_powered(&mut self, value: bool) {
        self.powered = value;
    }

    #[zbus(property)]
    fn discovering(&self) -> bool {
        false
    }
}

struct TestDevice {
    connected: Arc<AtomicBool>,
}

#[zbus::interface(name = "org.bluez.Device1")]
impl TestDevice {
    #[zbus(property)]
    fn alias(&self) -> &str {
        "Test headphones"
    }

    #[zbus(property)]
    fn address(&self) -> &str {
        "AA:BB:CC:DD:EE:FF"
    }

    #[zbus(property)]
    fn adapter(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from(TEST_ADAPTER).unwrap()
    }

    #[zbus(property)]
    fn paired(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    async fn connect(
        &self,
        #[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        self.connected.store(true, Ordering::SeqCst);
        self.connected_changed(&emitter).await?;
        Ok(())
    }

    async fn disconnect(
        &self,
        #[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        self.connected.store(false, Ordering::SeqCst);
        self.connected_changed(&emitter).await?;
        Ok(())
    }
}

struct TestManager;

#[zbus::interface(name = "org.bluez.AgentManager1")]
impl TestManager {
    fn register_agent(&self, path: OwnedObjectPath, capability: &str) -> zbus::fdo::Result<()> {
        if path.as_str() != AGENT_PATH || capability != "KeyboardDisplay" {
            return Err(zbus::fdo::Error::InvalidArgs("agent registration".into()));
        }
        Ok(())
    }
}

async fn snapshot_when(
    receiver: &smithay_client_toolkit::reexports::calloop::channel::Channel<Arc<Snapshot>>,
    accept: impl Fn(&Snapshot) -> bool,
) -> Arc<Snapshot> {
    timeout(Duration::from_secs(4), async {
        loop {
            if let Ok(state) = receiver.try_recv()
                && accept(&state)
            {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Bluetooth snapshot")
}

#[test]
fn private_bus_control_and_signals_update_devices_without_idle_polling() {
    let bus = Bus::start();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let reads = Arc::new(AtomicUsize::new(0));
            let connected = Arc::new(AtomicBool::new(false));
            let server = zbus::connection::Builder::address(bus.1.as_str())
                .unwrap()
                .name("org.bluez")
                .unwrap()
                .serve_at("/", zbus::fdo::ObjectManager)
                .unwrap()
                .serve_at("/org/bluez", TestManager)
                .unwrap()
                .serve_at(
                    TEST_ADAPTER,
                    TestAdapter {
                        powered: true,
                        reads: reads.clone(),
                    },
                )
                .unwrap()
                .serve_at(
                    TEST_DEVICE,
                    TestDevice {
                        connected: connected.clone(),
                    },
                )
                .unwrap()
                .build()
                .await
                .unwrap();
            let connection = zbus::connection::Builder::address(bus.1.as_str())
                .unwrap()
                .build()
                .await
                .unwrap();
            let (events, snapshots) =
                smithay_client_toolkit::reexports::calloop::channel::channel();
            let (commands, mut receiver) = mpsc::channel(8);
            let worker_connection = connection.clone();
            let task = tokio::spawn(async move {
                connected_worker(worker_connection, &events, &mut receiver).await
            });
            let state = snapshot_when(&snapshots, |s| s.available && s.devices.len() == 1).await;
            assert_eq!(state.devices[0].adapter, TEST_ADAPTER);
            assert!(state.allows(&Action::Connect(TEST_DEVICE.into())));
            let count = reads.load(Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(1200)).await;
            assert_eq!(
                reads.load(Ordering::SeqCst),
                count,
                "closed popup must not poll BlueZ"
            );
            assert!(snapshots.try_recv().is_err());
            operation(
                connection.clone(),
                Action::Connect(TEST_DEVICE.into()),
                None,
            )
            .await
            .unwrap();
            let state = snapshot_when(&snapshots, |s| {
                s.devices.first().is_some_and(|d| d.connected)
            })
            .await;
            assert_eq!(state.icon_state(), 2);
            assert!(connected.load(Ordering::SeqCst));
            operation(
                connection.clone(),
                Action::Disconnect(TEST_DEVICE.into()),
                None,
            )
            .await
            .unwrap();
            snapshot_when(&snapshots, |s| {
                s.devices.first().is_some_and(|d| !d.connected)
            })
            .await;
            commands.send(Command::Visible(true)).await.unwrap();
            commands
                .send(Command::Act(Action::Power(TEST_ADAPTER.into(), false)))
                .await
                .unwrap();
            let pending = snapshot_when(&snapshots, |s| {
                matches!(&s.busy, Some(Action::Power(_, false)))
            })
            .await;
            assert!(pending.adapters[0].powered);
            let completed = snapshot_when(&snapshots, |s| s.busy.is_none()).await;
            assert!(!completed.adapters[0].powered);
            server.release_name("org.bluez").await.unwrap();
            snapshot_when(&snapshots, |s| !s.available && s.devices.is_empty()).await;
            task.abort();
        });
}

#[test]
fn audio_choices_and_popup_hitboxes_use_real_profile_and_codec_ids() {
    use crate::{
        config::Config,
        render::{PreparedPopup, Renderer, bluetooth::Page},
    };
    let mut card = audio::parse_cards(r#"[{"name":"bluez_card.AA_BB_CC_DD_EE_FF","properties":{"device.string":"AA:BB:CC:DD:EE:FF"},"active_profile":"a2dp-sink","profiles":{"a2dp-sink":{"description":"High Fidelity Playback (A2DP Sink)","available":true},"headset-head-unit":{"description":"Headset (HFP)","available":false}}}]"#).unwrap().remove(0);
    card.codecs = audio::parse_codecs(
        r#"[{"name":"1","description":"SBC"},{"name":"6","description":"LDAC"}]"#,
    )
    .unwrap();
    card.active_codec = Some("6".into());
    let mut state = Snapshot {
        available: true,
        adapters: vec![Adapter {
            path: TEST_ADAPTER.into(),
            name: "Bluetooth".into(),
            powered: true,
            rate: Some((2048, 98304)),
            ..Default::default()
        }],
        devices: vec![Device {
            path: TEST_DEVICE.into(),
            name: "Headphones".into(),
            adapter: TEST_ADAPTER.into(),
            address: "AA:BB:CC:DD:EE:FF".into(),
            connected: true,
            paired: true,
            icon: "audio-headphones".into(),
            battery: Some(80),
            audio: Some(card),
        }],
        ..Default::default()
    };
    assert!(state.allows(&Action::Codec {
        device: TEST_DEVICE.into(),
        name: "6".into()
    }));
    assert!(!state.allows(&Action::Codec {
        device: TEST_DEVICE.into(),
        name: "LDAC".into()
    }));
    assert!(!state.allows(&Action::Profile {
        device: TEST_DEVICE.into(),
        name: "headset-head-unit".into()
    }));
    state.devices.push(Device {
        path: "/org/bluez/hci0/dev_11_22_33_44_55_66".into(),
        adapter: TEST_ADAPTER.into(),
        name: "Keyboard".into(),
        icon: "input-keyboard".into(),
        paired: true,
        ..Default::default()
    });
    let mut renderer = Renderer::new(&Config::default());
    for scale in [1, 2, 3] {
        for page in [
            Page::Root,
            Page::Device(TEST_DEVICE.into()),
            Page::Profiles(TEST_DEVICE.into()),
            Page::Codecs(TEST_DEVICE.into()),
        ] {
            let mut popup =
                PreparedPopup::Bluetooth(renderer.prepare_bluetooth(&state, &page, "", scale, 480));
            popup.constrain(480 * scale, 700 * scale);
            let (w, h) = popup.size();
            let mut canvas = tiny_skia::Pixmap::new(w, h).unwrap();
            let mut hits = Vec::new();
            renderer.draw_popup(&mut canvas.as_mut(), &popup, &mut hits, None);
            for hit in hits.iter().filter(|h| h.enabled) {
                assert_eq!(
                    popup.selection_at(hit.x + hit.width / 2, hit.y + hit.height / 2),
                    Some(hit.selection)
                );
                assert!(hit.x >= 0 && hit.x + hit.width <= w as i32);
                assert!(hit.y >= 0 && hit.y + hit.height <= h as i32);
            }
        }
    }
    state.busy = Some(Action::Disconnect(TEST_DEVICE.into()));
    assert!(!state.allows(&Action::Codec {
        device: TEST_DEVICE.into(),
        name: "6".into()
    }));
}

#[test]
#[ignore = "read-only integration with the user's BlueZ and PipeWire services"]
fn live_read_only_snapshot_and_counters() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let connection = Connection::system().await.unwrap();
            let mut state = Snapshot::default();
            refresh(&connection, &mut state).await.unwrap();
            assert!(state.available);
            let devices = state
                .devices
                .iter()
                .filter(|d| d.connected)
                .map(|d| (d.path.clone(), d.address.clone()))
                .collect();
            apply_audio(&mut state, refresh_audio(devices).await);
            println!(
                "BlueZ: {} adapters, {} devices, {} connected, audio error: {:?}",
                state.adapters.len(),
                state.devices.len(),
                state.devices.iter().filter(|d| d.connected).count(),
                state.audio_error
            );
            for device in &state.devices {
                if let Some(card) = &device.audio {
                    println!(
                        "Audio card: {} profiles, {} codecs; active profile={}, active codec={:?}",
                        card.profiles.len(),
                        card.codecs.len(),
                        card.active_profile,
                        card.active_codec
                    );
                }
            }
            let reader = traffic::Reader::open().unwrap();
            for adapter in &state.adapters {
                let first = reader.read(&adapter.path).unwrap();
                let mut meter = traffic::Meter::default();
                meter.sample(first, Instant::now());
                tokio::time::sleep(Duration::from_millis(1100)).await;
                let rates = meter.sample(reader.read(&adapter.path).unwrap(), Instant::now());
                println!("{} RX/TX B/s: {:?}", adapter.path, rates);
                assert!(rates.is_some());
            }
        });
}
