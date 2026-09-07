use std::{collections::HashMap, sync::Arc, thread};

use smithay_client_toolkit::reexports::calloop::channel::Sender as UiSender;
use system_tray::{
    client::{ActivateRequest, Client, Event, UpdateEvent},
    item::{IconPixmap, Status, StatusNotifierItem},
    menu::{MenuItem, MenuType, ToggleState, ToggleType, TrayMenu},
};
use tiny_skia::Pixmap;
use tokio::sync::{broadcast::error::RecvError, mpsc};
use zbus::{Connection, proxy};

use crate::{
    config::Config,
    icons::{IconLoader, fallback_icon, image_revision},
};

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayIcon {
    pub id: String,
    pub revision: u64,
    /// Premultiplied RGBA pixels.
    pub pixels: Arc<[u8]>,
    pub width: u32,
    pub height: u32,
}

pub enum TrayEvent {
    Items(Vec<TrayIcon>),
    Menu(TrayMenuPopup),
}

enum Command {
    Click {
        id: String,
        button: u32,
        x: i32,
        y: i32,
    },
    MenuItem {
        address: String,
        menu_path: String,
        item_id: i32,
    },
}

#[derive(Clone)]
pub struct TrayHandle {
    commands: mpsc::UnboundedSender<Command>,
}

impl TrayHandle {
    pub fn spawn(events: UiSender<TrayEvent>, config: &Config) -> Self {
        let (commands, mut receiver) = mpsc::unbounded_channel();
        let settings = IconSettings {
            target_size: config.tray_icon_size,
            loader: IconLoader::new(config),
        };

        thread::Builder::new()
            .name("nibari-tray".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to create tray runtime");

                loop {
                    match runtime.block_on(run(&events, &mut receiver, &settings)) {
                        Ok(()) if receiver.is_closed() => break,
                        Ok(()) => log::warn!("systray connection closed; reconnecting"),
                        Err(error) => log::warn!("systray: {error}; reconnecting"),
                    }
                    thread::sleep(std::time::Duration::from_secs(2));
                }
            })
            .expect("failed to start tray thread");

        Self { commands }
    }

    pub fn click(&self, id: String, button: u32, x: i32, y: i32) {
        if let Err(error) = self.commands.send(Command::Click { id, button, x, y }) {
            log::warn!("systray command channel is closed: {error}");
        }
    }

    pub fn menu_item(&self, address: String, menu_path: String, item_id: i32) {
        if let Err(error) = self.commands.send(Command::MenuItem {
            address,
            menu_path,
            item_id,
        }) {
            log::warn!("systray command channel is closed: {error}");
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayMenuPopup {
    pub address: String,
    pub menu_path: String,
    pub x: i32,
    pub y: i32,
    pub items: Vec<TrayMenuEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayMenuEntry {
    pub id: i32,
    pub label: String,
    pub enabled: bool,
    pub separator: bool,
    pub submenu: Vec<TrayMenuEntry>,
    pub toggle_type: ToggleType,
    pub toggle_state: ToggleState,
}

struct IconSettings {
    target_size: u32,
    loader: IconLoader,
}

async fn run(
    events: &UiSender<TrayEvent>,
    commands: &mut mpsc::UnboundedReceiver<Command>,
    settings: &IconSettings,
) -> anyhow::Result<()> {
    let client = Client::new().await?;
    let connection = Connection::session().await?;
    let mut updates = client.subscribe();
    let mut icon_cache = HashMap::new();
    let mut last_snapshot = None;

    publish_items(
        &client,
        events,
        settings,
        &mut icon_cache,
        &mut last_snapshot,
    );
    log::info!("systray: host registered");

    loop {
        tokio::select! {
            update = updates.recv() => {
                match update {
                    Ok(update) => {
                        if changed_icon_address(&update).is_some() {
                            publish_items(
                                &client,
                                events,
                                settings,
                                &mut icon_cache,
                                &mut last_snapshot,
                            );
                        }
                    }
                    Err(RecvError::Lagged(_)) => {
                        icon_cache.clear();
                        publish_items(
                            &client,
                            events,
                            settings,
                            &mut icon_cache,
                            &mut last_snapshot,
                        );
                    }
                    Err(RecvError::Closed) => break,
                }
            }
            command = commands.recv() => {
                let Some(command) = command else {
                    break;
                };
                if let Err(error) = execute_command(&client, &connection, events, command).await {
                    log::warn!("systray action failed: {error}");
                }
            }
        }
    }

    Ok(())
}

struct CachedIcon {
    signature: u64,
    icon: TrayIcon,
}

struct IconInput {
    item_id: String,
    theme_path: Option<String>,
    name: Option<String>,
    pixmap: Option<IconPixmap>,
}

impl IconInput {
    fn from_item(item: &StatusNotifierItem, target_size: u32) -> Self {
        let (name, pixmaps) = selected_icon(item);
        Self {
            item_id: item.id.clone(),
            theme_path: item.icon_theme_path.clone(),
            name: name.map(str::to_owned),
            pixmap: pixmaps.and_then(|pixmaps| select_pixmap(pixmaps, target_size).cloned()),
        }
    }
}

enum PreparedIcon {
    Cached(TrayIcon),
    Refresh(IconInput),
}

struct PreparedItem {
    address: String,
    signature: u64,
    icon: PreparedIcon,
}

fn changed_icon_address(update: &Event) -> Option<&str> {
    match update {
        Event::Add(address, _) | Event::Remove(address) => Some(address),
        Event::Update(
            address,
            UpdateEvent::Icon { .. } | UpdateEvent::AttentionIcon(_) | UpdateEvent::Status(_),
        ) => Some(address),
        Event::Update(_, _) => None,
    }
}

fn publish_items(
    client: &Client,
    events: &UiSender<TrayEvent>,
    settings: &IconSettings,
    cache: &mut HashMap<String, CachedIcon>,
    last_snapshot: &mut Option<u64>,
) {
    let items = client.items();
    let prepared = {
        let items = items.lock().expect("system-tray item lock poisoned");
        prepare_items(
            items
                .iter()
                .map(|(address, (item, _))| (address.as_str(), item)),
            settings.target_size,
            cache,
        )
    };
    let icons = realize_items(prepared, settings, cache);

    if snapshot_changed(&icons, last_snapshot) {
        log::info!("systray: {} item(s)", icons.len());
        let _ = events.send(TrayEvent::Items(icons));
    }
}

fn snapshot_changed(icons: &[TrayIcon], last_snapshot: &mut Option<u64>) -> bool {
    let mut snapshot = Fnv64::new();
    icons.iter().for_each(|icon| {
        snapshot.write(icon.id.as_bytes());
        snapshot.write_u64(icon.revision);
    });
    let snapshot = snapshot.finish();
    last_snapshot.replace(snapshot) != Some(snapshot)
}

fn prepare_items<'a>(
    items: impl IntoIterator<Item = (&'a str, &'a StatusNotifierItem)>,
    target_size: u32,
    cache: &mut HashMap<String, CachedIcon>,
) -> Vec<PreparedItem> {
    let mut prepared: Vec<_> = items
        .into_iter()
        .map(|(address, item)| {
            let signature = item_icon_signature(item);
            let icon = cache
                .get(address)
                .filter(|cached| cached.signature == signature)
                .map_or_else(
                    || PreparedIcon::Refresh(IconInput::from_item(item, target_size)),
                    |cached| PreparedIcon::Cached(cached.icon.clone()),
                );
            PreparedItem {
                address: address.to_owned(),
                signature,
                icon,
            }
        })
        .collect();
    prepared.sort_unstable_by(|left, right| left.address.cmp(&right.address));
    cache.retain(|address, _| {
        prepared
            .binary_search_by(|current| current.address.as_str().cmp(address))
            .is_ok()
    });
    prepared
}

fn realize_items(
    prepared: Vec<PreparedItem>,
    settings: &IconSettings,
    cache: &mut HashMap<String, CachedIcon>,
) -> Vec<TrayIcon> {
    prepared
        .into_iter()
        .map(|prepared| match prepared.icon {
            PreparedIcon::Cached(icon) => icon,
            PreparedIcon::Refresh(input) => {
                let icon = make_icon(&prepared.address, input, settings);
                cache.insert(
                    prepared.address,
                    CachedIcon {
                        signature: prepared.signature,
                        icon: icon.clone(),
                    },
                );
                icon
            }
        })
        .collect()
}

#[cfg(test)]
fn reconcile_items<'a>(
    items: impl IntoIterator<Item = (&'a str, &'a StatusNotifierItem)>,
    settings: &IconSettings,
    cache: &mut HashMap<String, CachedIcon>,
) -> Vec<TrayIcon> {
    let prepared = prepare_items(items, settings.target_size, cache);
    realize_items(prepared, settings, cache)
}

async fn execute_command(
    client: &Client,
    connection: &Connection,
    events: &UiSender<TrayEvent>,
    command: Command,
) -> anyhow::Result<()> {
    let (id, button, x, y) = match command {
        Command::MenuItem {
            address,
            menu_path,
            item_id,
        } => {
            client
                .activate(ActivateRequest::MenuItem {
                    address,
                    menu_path,
                    submenu_id: item_id,
                })
                .await?;
            return Ok(());
        }
        Command::Click { id, button, x, y } => (id, button, x, y),
    };

    let item_and_menu = client
        .items()
        .lock()
        .expect("system-tray item lock poisoned")
        .get(&id)
        .map(|(item, menu)| (item.clone(), menu.clone()));

    match button {
        BTN_LEFT
            if item_and_menu
                .as_ref()
                .is_some_and(|(item, _)| item.item_is_menu) =>
        {
            let item = item_and_menu.as_ref().map(|(item, _)| item);
            context_menu(
                connection,
                &id,
                item.and_then(|item| item.menu.as_deref()),
                x,
                y,
            )
            .await?;
        }
        BTN_LEFT => {
            client
                .activate(ActivateRequest::Default { address: id, x, y })
                .await?;
        }
        BTN_RIGHT => {
            if let Some((item, Some(menu))) = item_and_menu.as_ref()
                && let Some(menu_path) = item.menu.as_deref()
            {
                let popup = TrayMenuPopup {
                    address: item_destination(&id).to_owned(),
                    menu_path: menu_path.to_owned(),
                    x,
                    y,
                    items: menu_entries(menu),
                };
                if !popup.items.is_empty() {
                    let _ = events.send(TrayEvent::Menu(popup));
                    return Ok(());
                }
            }

            let item = item_and_menu.as_ref().map(|(item, _)| item);
            context_menu(
                connection,
                &id,
                item.and_then(|item| item.menu.as_deref()),
                x,
                y,
            )
            .await?;
        }
        BTN_MIDDLE => {
            client
                .activate(ActivateRequest::Secondary { address: id, x, y })
                .await?;
        }
        _ => {}
    }

    Ok(())
}

fn item_destination(address: &str) -> &str {
    split_item_address(address).map_or(address, |(destination, _)| destination)
}

fn menu_entries(menu: &TrayMenu) -> Vec<TrayMenuEntry> {
    menu.submenus.iter().filter_map(menu_entry).collect()
}

fn menu_entry(item: &MenuItem) -> Option<TrayMenuEntry> {
    if !item.visible {
        return None;
    }

    let separator = item.menu_type == MenuType::Separator;
    let label = if separator {
        String::new()
    } else {
        clean_menu_label(item.label.as_deref().unwrap_or_default())
    };

    let submenu: Vec<_> = item.submenu.iter().filter_map(menu_entry).collect();
    let is_submenu =
        item.children_display.as_deref() == Some("submenu") || !item.submenu.is_empty();
    (separator || !label.is_empty()).then_some(TrayMenuEntry {
        id: item.id,
        label,
        // Empty/lazy submenus are not leaf actions and must not emit a click.
        enabled: item.enabled && (!is_submenu || !submenu.is_empty()),
        separator,
        submenu,
        toggle_type: item.toggle_type,
        toggle_state: item.toggle_state,
    })
}

fn clean_menu_label(label: &str) -> String {
    let mut output = String::with_capacity(label.len());
    let mut chars = label.chars().peekable();
    while let Some(char) = chars.next() {
        if char == '_' {
            if chars.peek() == Some(&'_') {
                output.push('_');
                chars.next();
            }
        } else {
            output.push(char);
        }
    }
    output
}

#[proxy(
    interface = "org.kde.StatusNotifierItem",
    default_path = "/StatusNotifierItem"
)]
trait TrayItemActions {
    fn context_menu(&self, x: i32, y: i32) -> zbus::Result<()>;
}

async fn context_menu(
    connection: &Connection,
    address: &str,
    menu_path: Option<&str>,
    x: i32,
    y: i32,
) -> zbus::Result<()> {
    let Some((destination, paths)) = context_menu_targets(address, menu_path) else {
        return Err(zbus::Error::Failure(format!(
            "invalid StatusNotifierItem address: {address}"
        )));
    };

    let mut last_error = None;
    for path in paths {
        let result = TrayItemActionsProxy::builder(connection)
            .destination(destination.to_owned())?
            .path(path)?
            .build()
            .await?
            .context_menu(x, y)
            .await;
        match result {
            Ok(()) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
    }

    Err(last_error.unwrap_or_else(|| {
        zbus::Error::Failure(format!(
            "no ContextMenu target for StatusNotifierItem {address}"
        ))
    }))
}

fn split_item_address(address: &str) -> Option<(&str, &str)> {
    let slash = address.find('/')?;
    let (destination, path) = address.split_at(slash);
    (!destination.is_empty() && path.starts_with('/')).then_some((destination, path))
}

fn context_menu_targets<'a>(
    address: &'a str,
    menu_path: Option<&str>,
) -> Option<(&'a str, Vec<String>)> {
    if let Some((destination, path)) = split_item_address(address) {
        return Some((destination, vec![path.to_owned()]));
    }

    if address.trim().is_empty() {
        return None;
    }

    let mut paths = menu_path
        .and_then(infer_status_notifier_path_from_menu)
        .into_iter()
        .chain(std::iter::once("/StatusNotifierItem".to_owned()))
        .collect::<Vec<_>>();
    paths.dedup();
    Some((address, paths))
}

fn infer_status_notifier_path_from_menu(menu_path: &str) -> Option<String> {
    menu_path
        .contains("DbusMenu")
        .then(|| menu_path.replacen("DbusMenu", "StatusNotifierItem", 1))
}

fn make_icon(address: &str, input: IconInput, settings: &IconSettings) -> TrayIcon {
    let image = input
        .name
        .as_deref()
        .and_then(|name| {
            settings
                .loader
                .load(name, input.theme_path.as_deref(), settings.target_size)
        })
        .or_else(|| {
            input
                .pixmap
                .as_ref()
                .and_then(|pixmap| load_pixmap(std::slice::from_ref(pixmap), settings.target_size))
        })
        .unwrap_or_else(|| fallback_icon(&input.item_id, settings.target_size));

    TrayIcon {
        id: address.to_owned(),
        revision: image_revision(&image),
        pixels: Arc::from(image.data()),
        width: image.width(),
        height: image.height(),
    }
}

fn item_icon_signature(item: &StatusNotifierItem) -> u64 {
    let mut hash = Fnv64::new();
    hash.write_u8(item.status as u8);
    hash.write(item.id.as_bytes());
    hash.write_option(item.icon_theme_path.as_deref());
    let (name, pixmaps) = selected_icon(item);
    hash.write_option(name);
    hash.write_pixmaps(pixmaps);
    hash.finish()
}

fn selected_icon(item: &StatusNotifierItem) -> (Option<&str>, Option<&[IconPixmap]>) {
    if item.status == Status::NeedsAttention {
        (
            item.attention_icon_name
                .as_deref()
                .or(item.icon_name.as_deref()),
            item.attention_icon_pixmap
                .as_deref()
                .or(item.icon_pixmap.as_deref()),
        )
    } else {
        (item.icon_name.as_deref(), item.icon_pixmap.as_deref())
    }
}

struct Fnv64(u64);

impl Fnv64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn new() -> Self {
        Self(Self::OFFSET)
    }

    fn write(&mut self, bytes: &[u8]) {
        self.write_u64(bytes.len() as u64);
        self.0 = bytes.iter().fold(self.0, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(Self::PRIME)
        });
    }

    fn write_option(&mut self, value: Option<&str>) {
        match value {
            Some(value) => {
                self.write_u8(1);
                self.write(value.as_bytes());
            }
            None => self.write_u8(0),
        }
    }

    fn write_pixmaps(&mut self, pixmaps: Option<&[IconPixmap]>) {
        let Some(pixmaps) = pixmaps else {
            self.write_u64(0);
            return;
        };
        self.write_u64(pixmaps.len() as u64);
        pixmaps.iter().for_each(|pixmap| {
            self.write_u32(pixmap.width as u32);
            self.write_u32(pixmap.height as u32);
            self.write(&pixmap.pixels);
        });
    }

    fn write_u8(&mut self, value: u8) {
        self.0 = (self.0 ^ u64::from(value)).wrapping_mul(Self::PRIME);
    }

    fn write_u32(&mut self, value: u32) {
        self.write(&value.to_le_bytes());
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = value.to_le_bytes().into_iter().fold(self.0, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(Self::PRIME)
        });
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

fn load_pixmap(pixmaps: &[IconPixmap], target_size: u32) -> Option<Pixmap> {
    let best = select_pixmap(pixmaps, target_size)?;

    let mut pixmap = Pixmap::new(best.width as u32, best.height as u32)?;
    for (source, target) in best
        .pixels
        .chunks_exact(4)
        .zip(pixmap.data_mut().chunks_exact_mut(4))
    {
        let alpha = source[0];
        target[0] = premultiply(source[1], alpha);
        target[1] = premultiply(source[2], alpha);
        target[2] = premultiply(source[3], alpha);
        target[3] = alpha;
    }
    Some(pixmap)
}

fn select_pixmap(pixmaps: &[IconPixmap], target_size: u32) -> Option<&IconPixmap> {
    pixmaps
        .iter()
        .filter(|icon| {
            icon.width > 0
                && icon.height > 0
                && icon.width <= 1024
                && icon.height <= 1024
                && icon.pixels.len() >= icon.width as usize * icon.height as usize * 4
        })
        .min_by_key(|icon| {
            let size = icon.width.max(icon.height) as i64;
            (size - target_size as i64).unsigned_abs()
        })
}

fn premultiply(channel: u8, alpha: u8) -> u8 {
    ((channel as u16 * alpha as u16 + 127) / 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tray_item(id: &str, pixel: u8) -> StatusNotifierItem {
        StatusNotifierItem {
            id: id.to_owned(),
            category: Default::default(),
            title: None,
            status: Status::Active,
            window_id: 0,
            icon_theme_path: None,
            icon_name: None,
            icon_pixmap: Some(vec![IconPixmap {
                width: 1,
                height: 1,
                pixels: vec![255, pixel, 0, 0],
            }]),
            overlay_icon_name: None,
            overlay_icon_pixmap: None,
            attention_icon_name: None,
            attention_icon_pixmap: None,
            attention_movie_name: None,
            tool_tip: None,
            item_is_menu: false,
            menu: None,
        }
    }

    fn icon_settings() -> IconSettings {
        IconSettings {
            target_size: 16,
            loader: IconLoader::new(&Config::default()),
        }
    }

    #[test]
    fn unchanged_item_reuses_pixel_allocation() {
        let item = tray_item("test", 10);
        let settings = icon_settings();
        let mut cache = HashMap::new();
        let first = reconcile_items([("address", &item)], &settings, &mut cache);
        let second = reconcile_items([("address", &item)], &settings, &mut cache);

        assert!(Arc::ptr_eq(&first[0].pixels, &second[0].pixels));
    }

    #[test]
    fn unchanged_snapshot_does_not_request_another_ui_event() {
        let item = tray_item("test", 10);
        let settings = icon_settings();
        let mut cache = HashMap::new();
        let icons = reconcile_items([("address", &item)], &settings, &mut cache);
        let mut last_snapshot = None;

        assert!(snapshot_changed(&icons, &mut last_snapshot));
        assert!(!snapshot_changed(&icons, &mut last_snapshot));
    }

    #[test]
    fn changed_icon_refreshes_cached_pixels() {
        let mut item = tray_item("test", 10);
        let settings = icon_settings();
        let mut cache = HashMap::new();
        let first = reconcile_items([("address", &item)], &settings, &mut cache);
        item.icon_pixmap.as_mut().unwrap()[0].pixels[1] = 20;
        let second = reconcile_items([("address", &item)], &settings, &mut cache);

        assert!(!Arc::ptr_eq(&first[0].pixels, &second[0].pixels));
        assert_ne!(first[0].revision, second[0].revision);
    }

    #[test]
    fn removed_items_disappear_and_release_cached_pixels() {
        let item = tray_item("test", 10);
        let settings = icon_settings();
        let mut cache = HashMap::new();
        let icons = reconcile_items([("address", &item)], &settings, &mut cache);
        let pixels = Arc::downgrade(&icons[0].pixels);
        drop(icons);

        assert!(reconcile_items([], &settings, &mut cache).is_empty());
        assert!(cache.is_empty());
        assert!(pixels.upgrade().is_none());
    }

    #[test]
    fn selected_pixmap_is_copied_without_copying_other_variants() {
        let mut item = tray_item("test", 10);
        item.icon_pixmap.as_mut().unwrap().push(IconPixmap {
            width: 64,
            height: 64,
            pixels: vec![255; 64 * 64 * 4],
        });

        let input = IconInput::from_item(&item, 16);
        assert_eq!(input.pixmap.as_ref().map(|pixmap| pixmap.width), Some(1));
        assert_eq!(
            input.pixmap.as_ref().map(|pixmap| pixmap.pixels.len()),
            Some(4)
        );
    }

    #[test]
    fn missing_named_icon_falls_back_to_selected_pixmap() {
        let mut item = tray_item("test", 77);
        item.icon_name = Some("nibari-test-icon-that-does-not-exist".into());
        let settings = icon_settings();
        let input = IconInput::from_item(&item, settings.target_size);

        let icon = make_icon("address", input, &settings);
        assert_eq!(&*icon.pixels, &[77, 0, 0, 255]);
    }

    #[test]
    fn attention_status_selects_attention_icon_with_normal_fallback() {
        let mut item = tray_item("test", 10);
        item.attention_icon_pixmap = Some(vec![IconPixmap {
            width: 1,
            height: 1,
            pixels: vec![255, 99, 0, 0],
        }]);
        item.status = Status::NeedsAttention;
        let attention = IconInput::from_item(&item, 16);
        assert_eq!(attention.pixmap.unwrap().pixels[1], 99);

        item.attention_icon_pixmap = None;
        let normal_fallback = IconInput::from_item(&item, 16);
        assert_eq!(normal_fallback.pixmap.unwrap().pixels[1], 10);
    }

    fn dbus_menu_item(id: i32, label: &str) -> MenuItem {
        MenuItem {
            id,
            label: Some(label.to_owned()),
            enabled: true,
            visible: true,
            ..Default::default()
        }
    }

    #[test]
    fn empty_submenus_cannot_be_activated_as_leaf_actions() {
        let mut parent = dbus_menu_item(1, "Status");
        parent.children_display = Some("submenu".into());
        assert!(!menu_entry(&parent).unwrap().enabled);
        let mut child = dbus_menu_item(2, "Hidden status");
        child.visible = false;
        parent.submenu.push(child);
        assert!(!menu_entry(&parent).unwrap().enabled);
        parent.submenu[0].visible = true;
        assert!(menu_entry(&parent).unwrap().enabled);
    }

    #[test]
    fn menu_conversion_preserves_nested_actions() {
        let mut parent = dbus_menu_item(1, "_Settings");
        let mut child = dbus_menu_item(2, "_Notifications");
        let mut disabled = dbus_menu_item(3, "_Quiet mode");
        disabled.enabled = false;
        child.submenu.push(disabled);
        let mut hidden = dbus_menu_item(4, "Hidden");
        hidden.visible = false;
        child.submenu.push(hidden);
        child.submenu.push(dbus_menu_item(5, ""));
        let mut separator = dbus_menu_item(6, "Ignored separator label");
        separator.menu_type = MenuType::Separator;
        child.submenu.push(separator);
        parent.submenu.push(child);
        let with_child = menu_entry(&parent);

        parent.submenu.clear();
        assert_ne!(with_child, menu_entry(&parent));

        let entry = with_child.unwrap();
        assert_eq!(entry.label, "Settings");
        assert_eq!(entry.submenu.len(), 1);
        let child = &entry.submenu[0];
        assert_eq!((child.id, child.label.as_str()), (2, "Notifications"));
        assert_eq!(child.submenu.len(), 2);
        let disabled = &child.submenu[0];
        assert_eq!((disabled.id, disabled.label.as_str()), (3, "Quiet mode"));
        assert!(!disabled.enabled);
        assert!(disabled.submenu.is_empty());
        assert!(child.submenu[1].separator);
        assert!(child.submenu[1].label.is_empty());
    }

    #[test]
    fn menu_conversion_preserves_toggle_state() {
        let mut item = dbus_menu_item(1, "_Notifications");
        item.toggle_type = ToggleType::Checkmark;
        item.toggle_state = ToggleState::On;
        let checked = menu_entry(&item);

        item.toggle_state = ToggleState::Off;
        assert_ne!(checked, menu_entry(&item));
        assert_eq!(checked.unwrap().toggle_state, ToggleState::On);
        assert_eq!(menu_entry(&item).unwrap().toggle_state, ToggleState::Off);
        item.toggle_state = ToggleState::Indeterminate;
        assert_eq!(
            menu_entry(&item).unwrap().toggle_state,
            ToggleState::Indeterminate
        );
    }

    #[test]
    fn menu_conversion_preserves_toggle_type() {
        let mut item = dbus_menu_item(1, "_Notifications");
        item.toggle_type = ToggleType::Checkmark;
        let checkbox = menu_entry(&item);

        item.toggle_type = ToggleType::Radio;
        assert_ne!(checkbox, menu_entry(&item));
        assert_eq!(checkbox.unwrap().toggle_type, ToggleType::Checkmark);
        assert_eq!(menu_entry(&item).unwrap().toggle_type, ToggleType::Radio);
    }

    #[test]
    fn converts_argb_to_premultiplied_rgba() {
        let icon = IconPixmap {
            width: 1,
            height: 1,
            pixels: vec![128, 200, 100, 50],
        };
        let pixmap = load_pixmap(&[icon], 16).unwrap();
        assert_eq!(pixmap.data(), &[100, 50, 25, 128]);
    }

    #[test]
    fn ignores_invalid_pixmaps() {
        let icon = IconPixmap {
            width: 16,
            height: 16,
            pixels: vec![0; 4],
        };
        assert!(load_pixmap(&[icon], 16).is_none());
    }

    #[test]
    fn splits_status_notifier_item_address_with_custom_object_path() {
        assert_eq!(
            split_item_address(":1.197/org/chromium/StatusNotifierItem/1"),
            Some((":1.197", "/org/chromium/StatusNotifierItem/1"))
        );
        assert_eq!(
            split_item_address(":1.255/StatusNotifierItem"),
            Some((":1.255", "/StatusNotifierItem"))
        );
        assert_eq!(split_item_address(":1.255"), None);
    }

    #[test]
    fn infers_chromium_status_notifier_path_from_dbus_menu_path() {
        assert_eq!(
            infer_status_notifier_path_from_menu("/org/chromium/DbusMenu/1").as_deref(),
            Some("/org/chromium/StatusNotifierItem/1")
        );
        assert_eq!(infer_status_notifier_path_from_menu("/MenuBar"), None);
    }

    #[test]
    fn context_menu_targets_fall_back_from_short_address() {
        assert_eq!(
            context_menu_targets(":1.197", Some("/org/chromium/DbusMenu/1")),
            Some((
                ":1.197",
                vec![
                    "/org/chromium/StatusNotifierItem/1".to_owned(),
                    "/StatusNotifierItem".to_owned()
                ]
            ))
        );
        assert_eq!(
            context_menu_targets(":1.255", Some("/MenuBar")),
            Some((":1.255", vec!["/StatusNotifierItem".to_owned()]))
        );
    }

    #[test]
    fn dbus_menu_labels_strip_accelerator_markers() {
        assert_eq!(clean_menu_label("_Open _Telegram"), "Open Telegram");
        assert_eq!(
            clean_menu_label("Escaped __ underscore"),
            "Escaped _ underscore"
        );
    }
}
