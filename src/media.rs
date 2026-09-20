use std::{
    collections::HashMap,
    future::{pending, poll_fn},
    pin::Pin,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use smithay_client_toolkit::reexports::calloop::channel::Sender as UiSender;
use tokio::{sync::oneshot, task::JoinSet, time::timeout};
use zbus::{
    Connection, MatchRule, Message, MessageStream, export::futures_core::Stream, message::Type,
    zvariant::OwnedValue,
};

use crate::config::Config;

const PREFIX: &str = "org.mpris.MediaPlayer2.";
const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const CALL_TIMEOUT: Duration = Duration::from_millis(800);
const MAX_PLAYERS: usize = 64;
type Properties = HashMap<String, OwnedValue>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaSnapshot {
    pub title: String,
    pub detail: String,
}

pub struct MediaHandle {
    stop: Option<oneshot::Sender<()>>,
}

impl MediaHandle {
    pub fn spawn(events: UiSender<Option<Arc<MediaSnapshot>>>, _config: &Config) -> Self {
        let (stop, mut stopped) = oneshot::channel();
        thread::Builder::new().name("nibari-media".into()).spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all().build().expect("failed to create media runtime");
            runtime.block_on(async {
                loop {
                    let work = async {
                        let connection = timeout(Duration::from_secs(3), Connection::session()).await??;
                        connected(connection, &events).await
                    };
                    tokio::select! {
                        _ = &mut stopped => break,
                        result = work => {
                            if events.send(None).is_err() { break; }
                            if let Err(error) = result { log::debug!("media: {error}; reconnecting"); }
                        }
                    }
                    tokio::select! {
                        _ = &mut stopped => break,
                        _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                    }
                }
            });
        }).expect("failed to start media thread");
        Self { stop: Some(stop) }
    }
}

impl Drop for MediaHandle {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Status {
    Stopped,
    Paused,
    Playing,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Self::Playing => "Playing",
            Self::Paused => "Paused",
            Self::Stopped => "Stopped",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Track {
    title: String,
    artist: String,
    length: Option<i64>,
    id: Option<String>,
}

struct Player {
    owner: String,
    generation: u64,
    revision: u64,
    activity: u64,
    status: Status,
    track: Option<Track>,
    position: i64,
    anchor: Instant,
    rate: f64,
    loading: bool,
    fetch_task: Option<tokio::task::AbortHandle>,
}

impl Player {
    fn new(owner: String, now: Instant) -> Self {
        Self {
            owner,
            generation: 0,
            revision: 0,
            activity: 0,
            status: Status::Stopped,
            track: None,
            position: 0,
            anchor: now,
            rate: 1.0,
            loading: false,
            fetch_task: None,
        }
    }

    fn position_at(&self, now: Instant) -> i64 {
        let delta = if self.status == Status::Playing {
            now.saturating_duration_since(self.anchor).as_secs_f64() * self.rate * 1_000_000.0
        } else {
            0.0
        };
        let value = self.position.saturating_add(delta as i64).max(0);
        self.track
            .as_ref()
            .and_then(|track| track.length)
            .map_or(value, |length| value.min(length))
    }

    fn seek(&mut self, position: i64, now: Instant) {
        self.position = position.max(0);
        self.anchor = now;
    }

    fn apply(&mut self, mut properties: Properties, now: Instant) -> bool {
        let relevant = ["Metadata", "PlaybackStatus", "Rate", "Position"]
            .iter()
            .any(|key| properties.contains_key(*key));
        if !relevant {
            return false;
        }
        self.position = self.position_at(now);
        self.anchor = now;
        if let Some(metadata) = properties.remove("Metadata") {
            let next = parse_track(metadata);
            let changed_track = match (&self.track, &next) {
                (Some(old), Some(new)) if old.id.is_some() && new.id.is_some() => old.id != new.id,
                (Some(old), Some(new)) => old.title != new.title || old.artist != new.artist,
                (None, None) => false,
                _ => true,
            };
            if changed_track {
                self.position = 0;
            }
            self.track = next;
        }
        if let Some(value) = properties
            .get("Rate")
            .and_then(|value| f64::try_from(value).ok())
            && value.is_finite()
            && value != 0.0
        {
            self.rate = value;
        }
        if let Some(status) = string(properties.get("PlaybackStatus")) {
            self.status = match status.as_str() {
                "Playing" => Status::Playing,
                "Paused" => Status::Paused,
                _ => Status::Stopped,
            };
            if self.status == Status::Stopped {
                self.position = 0;
            }
        }
        if let Some(position) = properties
            .get("Position")
            .and_then(|value| i64::try_from(value).ok())
        {
            self.position = position.max(0);
        }
        true
    }

    fn detail(&self, now: Instant) -> String {
        let elapsed = self.position_at(now);
        let status = self.status.label();
        match self.track.as_ref().and_then(|track| track.length) {
            Some(length) => {
                let percent = ((i128::from(elapsed) * 100 + i128::from(length) / 2)
                    / i128::from(length))
                .clamp(0, 100);
                format!(
                    "{} / {} ({percent}%) [{status}]",
                    clock(elapsed),
                    clock(length)
                )
            }
            None => format!("{} / --:-- [{status}]", clock(elapsed)),
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        if let Some(task) = self.fetch_task.take() {
            task.abort();
        }
    }
}

fn string(value: Option<&OwnedValue>) -> Option<String> {
    value
        .and_then(|value| <&str>::try_from(value).ok())
        .map(str::to_owned)
}

fn parse_track(value: OwnedValue) -> Option<Track> {
    let mut values = Properties::try_from(value).ok()?;
    let title = string(values.get("xesam:title")).unwrap_or_default();
    let artist = values
        .remove("xesam:artist")
        .and_then(|value| Vec::<String>::try_from(value).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|artist| !artist.trim().is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    if title.trim().is_empty() && artist.is_empty() {
        return None;
    }
    let length = values
        .get("mpris:length")
        .and_then(microseconds)
        .filter(|length| *length > 0);
    let id = values
        .get("mpris:trackid")
        .and_then(|value| <&zbus::zvariant::ObjectPath<'_>>::try_from(value).ok())
        .map(ToString::to_string);
    Some(Track {
        title,
        artist,
        length,
        id,
    })
}

fn microseconds(value: &OwnedValue) -> Option<i64> {
    i64::try_from(value).ok().or_else(|| {
        u64::try_from(value)
            .ok()
            .and_then(|length| i64::try_from(length).ok())
    })
}

fn clock(micros: i64) -> String {
    let seconds = micros.max(0) / 1_000_000;
    if seconds >= 3600 {
        format!(
            "{:02}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    }
}

fn selected(players: &HashMap<String, Player>) -> Option<(&str, &Player)> {
    players
        .iter()
        .filter(|(_, player)| player.track.is_some())
        .max_by(|(a_name, a), (b_name, b)| {
            a.status
                .cmp(&b.status)
                .then(a.activity.cmp(&b.activity))
                .then_with(|| b_name.cmp(a_name))
        })
        .map(|(name, player)| (name.as_str(), player))
}

fn snapshot(players: &HashMap<String, Player>, now: Instant) -> Option<Arc<MediaSnapshot>> {
    let (_, player) = selected(players)?;
    let track = player.track.as_ref()?;
    let title = match (track.artist.is_empty(), track.title.is_empty()) {
        (false, false) => format!("{} - {}", track.artist, truncated_title(&track.title)),
        (false, true) => track.artist.clone(),
        _ => truncated_title(&track.title),
    };
    Some(Arc::new(MediaSnapshot {
        title,
        detail: player.detail(now),
    }))
}

fn truncated_title(title: &str) -> String {
    const LIMIT: usize = 32;
    let mut chars = title.chars();
    let mut visible = chars.by_ref().take(LIMIT).collect::<String>();
    if chars.next().is_some() {
        let cut = visible
            .char_indices()
            .nth(LIMIT - 1)
            .map_or(visible.len(), |(index, _)| index);
        visible.truncate(cut);
        visible.push('…');
    }
    visible
}

enum Update {
    Owner {
        name: String,
        epoch: u64,
        owner: Option<String>,
    },
    Loaded {
        name: String,
        owner: String,
        generation: u64,
        revision: u64,
        result: anyhow::Result<Properties>,
    },
}

async fn next(stream: &mut MessageStream) -> anyhow::Result<Message> {
    poll_fn(|cx| Pin::new(&mut *stream).poll_next(cx))
        .await
        .ok_or_else(|| anyhow::anyhow!("MPRIS signal stream closed"))?
        .map_err(Into::into)
}

async fn subscribe(
    connection: &Connection,
    interface: &str,
    member: &str,
    path: &str,
) -> anyhow::Result<MessageStream> {
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .interface(interface)?
        .member(member)?
        .path(path)?
        .build();
    Ok(timeout(
        CALL_TIMEOUT,
        MessageStream::for_match_rule(rule, connection, Some(256)),
    )
    .await??)
}

async fn properties(
    connection: &Connection,
    owner: &str,
    interface: &str,
) -> anyhow::Result<Properties> {
    let reply = timeout(
        CALL_TIMEOUT,
        connection.call_method(Some(owner), PATH, Some(PROPERTIES), "GetAll", &(interface,)),
    )
    .await??;
    Ok(reply.body().deserialize()?)
}

fn fetch(jobs: &mut JoinSet<Update>, connection: &Connection, name: &str, player: &mut Player) {
    if player.loading {
        return;
    }
    player.loading = true;
    let (connection, name, owner, revision) = (
        connection.clone(),
        name.to_owned(),
        player.owner.clone(),
        player.revision,
    );
    let generation = player.generation;
    player.fetch_task = Some(jobs.spawn(async move {
        let result = properties(&connection, &owner, PLAYER).await;
        Update::Loaded {
            name,
            owner,
            generation,
            revision,
            result,
        }
    }));
}

async fn connected(
    connection: Connection,
    events: &UiSender<Option<Arc<MediaSnapshot>>>,
) -> anyhow::Result<()> {
    // Subscribe before discovery: queued owner events supersede any in-flight discovery result.
    let mut owners = subscribe(
        &connection,
        "org.freedesktop.DBus",
        "NameOwnerChanged",
        "/org/freedesktop/DBus",
    )
    .await?;
    let mut changes = subscribe(&connection, PROPERTIES, "PropertiesChanged", PATH).await?;
    let mut seeks = subscribe(&connection, PLAYER, "Seeked", PATH).await?;
    let bus = zbus::fdo::DBusProxy::new(&connection).await?;
    let names = timeout(CALL_TIMEOUT, bus.list_names()).await??;
    let mut jobs = JoinSet::new();
    let mut players = HashMap::<String, Player>::new();
    let mut epochs = HashMap::<String, u64>::new();
    let mut activity = 0_u64;
    for name in names
        .into_iter()
        .filter(|name| name.as_str().starts_with(PREFIX))
        .take(MAX_PLAYERS)
    {
        let connection = connection.clone();
        let name = name.to_string();
        jobs.spawn(async move {
            let result = async {
                let bus = zbus::fdo::DBusProxy::new(&connection).await?;
                Ok::<_, anyhow::Error>(bus.get_name_owner(name.as_str().try_into()?).await?)
            };
            let owner = timeout(CALL_TIMEOUT, result)
                .await
                .ok()
                .and_then(Result::ok)
                .map(|owner| owner.to_string());
            Update::Owner {
                name,
                epoch: 0,
                owner,
            }
        });
    }
    let mut last = None;
    let mut next_tick = Instant::now() + Duration::from_secs(1);
    loop {
        let ticking =
            selected(&players).is_some_and(|(_, player)| player.status == Status::Playing);
        let tick = async {
            if ticking {
                tokio::time::sleep_until(next_tick.into()).await;
            } else {
                pending::<()>().await;
            }
        };
        tokio::select! {
            biased;
            message = next(&mut owners) => {
                let message = message?;
                if message.header().sender().map(|name| name.as_str()) != Some("org.freedesktop.DBus") { continue; }
                let Ok((name, _, owner)) = message.body().deserialize::<(String, String, String)>() else { continue; };
                if !name.starts_with(PREFIX) { continue; }
                *epochs.entry(name.clone()).or_default() += 1;
                players.remove(&name);
                if !owner.is_empty() && players.len() < MAX_PLAYERS {
                    let player = players.entry(name.clone()).or_insert_with(|| Player::new(owner, Instant::now()));
                    player.generation = epochs[&name];
                    activity += 1;
                    player.activity = activity;
                    fetch(&mut jobs, &connection, &name, player);
                }
            }
            message = next(&mut changes) => {
                let message = message?;
                let Ok((interface, properties, invalidated)) = message.body().deserialize::<(String, Properties, Vec<String>)>() else { continue; };
                if interface != PLAYER { continue; }
                let header = message.header();
                let Some(sender) = header.sender() else { continue; };
                for (name, player) in &mut players {
                    if player.owner != sender.as_str() { continue; }
                    let data = properties.iter().filter_map(|(key, value)| value.try_clone().ok().map(|value| (key.clone(), value))).collect();
                    let relevant = interface == PLAYER && player.apply(data, Instant::now());
                    let invalid = invalidated.iter().any(|key| matches!(key.as_str(), "Metadata" | "PlaybackStatus" | "Rate" | "Position"));
                    if invalid {
                        // An invalidated value is unknown, not the previous track indefinitely.
                        player.track = None;
                    }
                    if relevant || invalid {
                        player.revision += 1;
                        activity += 1;
                        player.activity = activity;
                    }
                    if invalid || (relevant && player.track.is_none()) { fetch(&mut jobs, &connection, name, player); }
                }
            }
            message = next(&mut seeks) => {
                let message = message?;
                let Ok((position,)) = message.body().deserialize::<(i64,)>() else { continue; };
                let header = message.header();
                for player in players.values_mut().filter(|player| header.sender().is_some_and(|sender| player.owner == sender.as_str())) {
                    player.seek(position, Instant::now());
                    player.revision += 1;
                    activity += 1;
                    player.activity = activity;
                }
            }
            update = jobs.join_next(), if !jobs.is_empty() => {
                match update {
                    Some(Ok(Update::Owner { name, epoch, owner: Some(owner) }))
                        if epochs.get(&name).copied().unwrap_or_default() == epoch && !players.contains_key(&name) && players.len() < MAX_PLAYERS => {
                        let player = players.entry(name.clone()).or_insert_with(|| Player::new(owner, Instant::now()));
                        fetch(&mut jobs, &connection, &name, player);
                    }
                    Some(Ok(Update::Loaded { name, owner, generation, revision, result })) => {
                        if let Some(player) = players.get_mut(&name).filter(|player| player.owner == owner && player.generation == generation) {
                            player.loading = false;
                            player.fetch_task = None;
                            if player.revision != revision { fetch(&mut jobs, &connection, &name, player); }
                            else if let Ok(properties) = result {
                                player.apply(properties, Instant::now());
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ = tick => { next_tick = Instant::now() + Duration::from_secs(1); }
        }
        let now = Instant::now();
        if !ticking {
            next_tick = now + Duration::from_secs(1);
        }
        let current = snapshot(&players, now);
        if current != last {
            events
                .send(current.clone())
                .map_err(|_| anyhow::anyhow!("media UI channel closed"))?;
            last = current;
        }
    }
}

#[cfg(test)]
mod tests;
