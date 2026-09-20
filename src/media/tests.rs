use super::*;

fn player(status: Status, position: i64, length: Option<i64>, now: Instant) -> Player {
    let mut player = Player::new(":1.1".into(), now);
    player.status = status;
    player.position = position;
    player.track = Some(Track {
        title: "Song".into(),
        artist: "Artist".into(),
        length,
        id: None,
    });
    player
}

#[test]
fn elapsed_uses_monotonic_rate_then_freezes_on_pause() {
    let now = Instant::now();
    let mut p = player(Status::Playing, 45_000_000, Some(160_000_000), now);
    p.rate = 2.0;
    let later = now + Duration::from_secs(2);
    assert_eq!(p.position_at(later), 49_000_000);
    p.apply(
        HashMap::from([(
            "PlaybackStatus".into(),
            OwnedValue::from(zbus::zvariant::Str::from("Paused")),
        )]),
        later,
    );
    assert_eq!(p.position_at(later + Duration::from_secs(90)), 49_000_000);
    assert_eq!(p.detail(later), "00:49 / 02:40 (31%) [Paused]");
}

#[test]
fn unknown_duration_and_clamped_positions_are_explicit() {
    let now = Instant::now();
    let mut p = player(Status::Paused, 49_000_000, None, now);
    assert_eq!(p.detail(now), "00:49 / --:-- [Paused]");
    p.track.as_mut().unwrap().length = Some(40_000_000);
    assert_eq!(p.detail(now), "00:40 / 00:40 (100%) [Paused]");
    p.position = -100;
    assert_eq!(p.position_at(now), 0);
}

#[test]
fn title_is_limited_to_32_characters_without_limiting_artist() {
    let now = Instant::now();
    let mut players = HashMap::new();
    let mut p = player(Status::Paused, 0, None, now);
    let track = p.track.as_mut().unwrap();
    track.artist = "A channel nickname that stays whole".into();
    track.title = "123456789012345678901234567890123".into();
    players.insert("spotify".into(), p);
    assert_eq!(
        snapshot(&players, now).unwrap().title,
        "A channel nickname that stays whole - 1234567890123456789012345678901…"
    );
}

#[test]
fn selection_prefers_status_then_activity_with_stable_name_tie() {
    let now = Instant::now();
    let mut players = HashMap::new();
    let mut paused = player(Status::Paused, 0, None, now);
    paused.activity = 99;
    players.insert("paused".into(), paused);
    players.insert("z-player".into(), player(Status::Playing, 0, None, now));
    players.insert("a-player".into(), player(Status::Playing, 0, None, now));
    assert_eq!(selected(&players).unwrap().0, "a-player");
    players.get_mut("z-player").unwrap().activity = 1;
    assert_eq!(selected(&players).unwrap().0, "z-player");
    players.get_mut("z-player").unwrap().track = None;
    assert_eq!(selected(&players).unwrap().0, "a-player");
}

#[test]
fn seek_rate_change_and_stop_preserve_correct_anchor() {
    let now = Instant::now();
    let mut p = player(Status::Playing, 10_000_000, None, now);
    p.apply(
        HashMap::from([("Rate".into(), OwnedValue::from(0.5_f64))]),
        now + Duration::from_secs(2),
    );
    assert_eq!(p.position_at(now + Duration::from_secs(4)), 13_000_000);
    p.seek(70_000_000, now + Duration::from_secs(4));
    assert_eq!(p.position_at(now + Duration::from_secs(6)), 71_000_000);
    p.apply(
        HashMap::from([(
            "PlaybackStatus".into(),
            OwnedValue::from(zbus::zvariant::Str::from("Stopped")),
        )]),
        now + Duration::from_secs(6),
    );
    assert_eq!(p.position_at(now + Duration::from_secs(20)), 0);
}

struct PrivateBus(std::process::Child, String);

impl PrivateBus {
    fn start() -> Self {
        use std::io::BufRead;
        let mut child = std::process::Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("private dbus-daemon");
        let mut address = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        Self(child, address.trim().into())
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct FixturePlayer {
    reads: Arc<std::sync::atomic::AtomicUsize>,
    delay_ms: std::sync::atomic::AtomicU64,
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl FixturePlayer {
    #[zbus(property)]
    async fn metadata(&self) -> Properties {
        let delay = self.delay_ms.load(std::sync::atomic::Ordering::SeqCst);
        if delay != 0 {
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
        HashMap::from([
            (
                "xesam:title".into(),
                OwnedValue::from(zbus::zvariant::Str::from("Fixture song")),
            ),
            (
                "xesam:artist".into(),
                OwnedValue::try_from(zbus::zvariant::Value::from(vec!["Fixture artist"])).unwrap(),
            ),
            // Spotify sends mpris:length with the D-Bus unsigned `t` type.
            ("mpris:length".into(), OwnedValue::from(160_000_000_u64)),
        ])
    }

    #[zbus(property)]
    fn playback_status(&self) -> &str {
        "Paused"
    }

    #[zbus(property)]
    fn position(&self) -> i64 {
        self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        49_000_000
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }
}

struct FixtureRoot;

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl FixtureRoot {
    #[zbus(property)]
    fn identity(&self) -> &str {
        "nibari-test-player"
    }

    #[zbus(property)]
    fn desktop_entry(&self) -> &str {
        "nibari-test-player"
    }
}

async fn fixture(
    bus: &PrivateBus,
    name: &str,
    delay: Duration,
    reads: Arc<std::sync::atomic::AtomicUsize>,
) -> Connection {
    zbus::connection::Builder::address(bus.1.as_str())
        .unwrap()
        .serve_at(PATH, FixtureRoot)
        .unwrap()
        .serve_at(
            PATH,
            FixturePlayer {
                reads,
                delay_ms: std::sync::atomic::AtomicU64::new(delay.as_millis() as u64),
            },
        )
        .unwrap()
        .name(name)
        .unwrap()
        .build()
        .await
        .unwrap()
}

async fn wait_snapshot(
    receiver: &smithay_client_toolkit::reexports::calloop::channel::Channel<
        Option<Arc<MediaSnapshot>>,
    >,
    accept: impl Fn(&Option<Arc<MediaSnapshot>>) -> bool,
) -> Option<Arc<MediaSnapshot>> {
    timeout(Duration::from_secs(4), async {
        loop {
            if let Ok(snapshot) = receiver.try_recv()
                && accept(&snapshot)
            {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("expected media snapshot")
}

async fn change(connection: &Connection, changed: Properties) {
    connection
        .emit_signal(
            None::<&str>,
            PATH,
            PROPERTIES,
            "PropertiesChanged",
            &(PLAYER, changed, Vec::<String>::new()),
        )
        .await
        .unwrap();
}

#[test]
fn private_bus_signals_drive_media_without_position_polling() {
    let bus = PrivateBus::start();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            use std::sync::atomic::{AtomicUsize, Ordering};
            let reads = Arc::new(AtomicUsize::new(0));
            let player = fixture(
                &bus,
                "org.mpris.MediaPlayer2.fixture",
                Duration::ZERO,
                reads.clone(),
            )
            .await;
            let bad = fixture(
                &bus,
                "org.mpris.MediaPlayer2.bad",
                Duration::from_secs(5),
                Arc::new(AtomicUsize::new(0)),
            )
            .await;
            let connection = zbus::connection::Builder::address(bus.1.as_str())
                .unwrap()
                .build()
                .await
                .unwrap();
            let (sender, receiver) = smithay_client_toolkit::reexports::calloop::channel::channel();
            let worker = tokio::spawn(async move { connected(connection, &sender).await });
            let initial = wait_snapshot(&receiver, Option::is_some).await.unwrap();
            assert_eq!(initial.title, "Fixture artist - Fixture song");
            assert_eq!(initial.detail, "00:49 / 02:40 (31%) [Paused]");
            let initial_reads = reads.load(Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(1200)).await;
            assert!(
                receiver.try_recv().is_err(),
                "paused playback must not publish periodic snapshots"
            );
            assert_eq!(reads.load(Ordering::SeqCst), initial_reads);

            change(
                &player,
                HashMap::from([
                    (
                        "PlaybackStatus".into(),
                        OwnedValue::from(zbus::zvariant::Str::from("Playing")),
                    ),
                    ("Rate".into(), OwnedValue::from(2_f64)),
                ]),
            )
            .await;
            wait_snapshot(&receiver, |snapshot| {
                snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.detail.ends_with("[Playing]"))
            })
            .await;
            let progressed = wait_snapshot(&receiver, |snapshot| {
                snapshot
                    .as_ref()
                    .is_some_and(|snapshot| !snapshot.detail.starts_with("00:49"))
            })
            .await
            .unwrap();
            assert!(progressed.detail.ends_with("[Playing]"));
            assert_eq!(
                reads.load(Ordering::SeqCst),
                initial_reads,
                "local progress must not poll D-Bus Position"
            );

            player
                .emit_signal(None::<&str>, PATH, PLAYER, "Seeked", &(70_000_000_i64,))
                .await
                .unwrap();
            wait_snapshot(&receiver, |snapshot| {
                snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.detail.starts_with("01:10"))
            })
            .await;
            change(
                &player,
                HashMap::from([(
                    "PlaybackStatus".into(),
                    OwnedValue::from(zbus::zvariant::Str::from("Paused")),
                )]),
            )
            .await;
            let paused = wait_snapshot(&receiver, |snapshot| {
                snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.detail.ends_with("[Paused]"))
            })
            .await
            .unwrap();
            tokio::time::sleep(Duration::from_millis(1200)).await;
            assert!(
                receiver.try_recv().is_err(),
                "paused Seeked position must freeze"
            );
            assert!(paused.detail.starts_with("01:10"));
            assert_eq!(reads.load(Ordering::SeqCst), initial_reads);
            player
                .release_name("org.mpris.MediaPlayer2.fixture")
                .await
                .unwrap();
            wait_snapshot(&receiver, Option::is_none).await;
            worker.abort();
            drop(bad);
        });
}

#[test]
fn private_bus_failed_player_recovers_on_event_and_invalid_metadata_disappears() {
    let bus = PrivateBus::start();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            use std::sync::atomic::{AtomicUsize, Ordering};
            let player = fixture(
                &bus,
                "org.mpris.MediaPlayer2.flaky",
                Duration::from_millis(1500),
                Arc::new(AtomicUsize::new(0)),
            )
            .await;
            let connection = zbus::connection::Builder::address(bus.1.as_str())
                .unwrap()
                .build()
                .await
                .unwrap();
            let (sender, receiver) = smithay_client_toolkit::reexports::calloop::channel::channel();
            let worker = tokio::spawn(async move { connected(connection, &sender).await });
            tokio::time::sleep(Duration::from_millis(1100)).await;
            assert!(
                receiver.try_recv().is_err(),
                "timed-out initial player must stay hidden"
            );
            let interface = player
                .object_server()
                .interface::<_, FixturePlayer>(PATH)
                .await
                .unwrap();
            interface.get().await.delay_ms.store(0, Ordering::SeqCst);
            change(
                &player,
                HashMap::from([(
                    "PlaybackStatus".into(),
                    OwnedValue::from(zbus::zvariant::Str::from("Paused")),
                )]),
            )
            .await;
            wait_snapshot(&receiver, Option::is_some).await;

            interface.get().await.delay_ms.store(1500, Ordering::SeqCst);
            player
                .emit_signal(
                    None::<&str>,
                    PATH,
                    PROPERTIES,
                    "PropertiesChanged",
                    &(PLAYER, Properties::new(), vec!["Metadata"]),
                )
                .await
                .unwrap();
            wait_snapshot(&receiver, Option::is_none).await;
            tokio::time::sleep(Duration::from_millis(1000)).await;
            assert!(
                receiver.try_recv().is_err(),
                "failed refresh must not restore invalid metadata"
            );
            // A new owner can reuse the same well-known name; pending old work must not revive it.
            change(
                &player,
                HashMap::from([(
                    "PlaybackStatus".into(),
                    OwnedValue::from(zbus::zvariant::Str::from("Paused")),
                )]),
            )
            .await;
            tokio::time::sleep(Duration::from_millis(50)).await;
            player
                .release_name("org.mpris.MediaPlayer2.flaky")
                .await
                .unwrap();
            let replacement = fixture(
                &bus,
                "org.mpris.MediaPlayer2.flaky",
                Duration::ZERO,
                Arc::new(AtomicUsize::new(0)),
            )
            .await;
            wait_snapshot(&receiver, Option::is_some).await;
            replacement
                .release_name("org.mpris.MediaPlayer2.flaky")
                .await
                .unwrap();
            wait_snapshot(&receiver, Option::is_none).await;
            tokio::time::sleep(Duration::from_millis(1600)).await;
            assert!(
                receiver.try_recv().is_err(),
                "late old-owner fetch must never restore a removed source"
            );
            worker.abort();
        });
}
