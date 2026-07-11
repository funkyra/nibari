use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    thread,
    time::Duration,
};

use niri_ipc::{
    Event, Request, Response,
    socket::Socket,
    state::{EventStreamState, EventStreamStatePart},
};
use smithay_client_toolkit::reexports::calloop::channel::Sender as UiSender;

use crate::{
    config::Config,
    icons::{IconLoader, fallback_icon, image_revision},
};

pub const WORKSPACE_LABELS: [&str; 10] =
    ["一", "二", "三", "四", "五", "六", "七", "八", "九", "零"];
pub const WORKSPACES_PER_OUTPUT: usize = 5;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NiriModel {
    workspaces: WorkspaceModel,
    task_lists: Vec<TaskList>,
    keyboard_layout: Arc<str>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct WorkspaceModel {
    workspaces: Vec<WorkspaceInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WorkspaceInfo {
    index: u8,
    output: Option<String>,
    is_active: bool,
    is_focused: bool,
    is_occupied: bool,
    is_urgent: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceSlot {
    pub index: u8,
    pub is_active: bool,
    pub is_focused: bool,
    pub is_occupied: bool,
    pub is_urgent: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceContent {
    pub workspaces: [WorkspaceSlot; WORKSPACES_PER_OUTPUT],
    pub tasks: Arc<[WindowTask]>,
    pub keyboard_layout: Arc<str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TaskList {
    output: Option<String>,
    is_focused: bool,
    tasks: Arc<[WindowTask]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowTask {
    pub id: u64,
    pub label: String,
    pub app_id: Option<String>,
    pub is_focused: bool,
    pub is_urgent: bool,
    pub icon: ApplicationIcon,
}

#[derive(Clone, Debug)]
pub struct ApplicationIcon {
    pub revision: u64,
    /// Premultiplied RGBA pixels.
    pub pixels: Arc<[u8]>,
    pub width: u32,
    pub height: u32,
}

impl PartialEq for ApplicationIcon {
    fn eq(&self, other: &Self) -> bool {
        self.revision == other.revision && self.width == other.width && self.height == other.height
    }
}

impl Eq for ApplicationIcon {}

impl NiriModel {
    pub fn surface_content(&self, output: Option<&str>, output_group: u8) -> SurfaceContent {
        SurfaceContent {
            workspaces: self.slots_for(output, output_group),
            tasks: self.tasks_for(output, output_group),
            keyboard_layout: Arc::clone(&self.keyboard_layout),
        }
    }

    pub fn slots_for(
        &self,
        output: Option<&str>,
        output_group: u8,
    ) -> [WorkspaceSlot; WORKSPACES_PER_OUTPUT] {
        std::array::from_fn(|offset| {
            let workspace_index = offset as u8 + 1;
            let label_index = workspace_index + output_group.min(1) * WORKSPACES_PER_OUTPUT as u8;
            self.workspaces
                .workspace_for(workspace_index, output, output_group)
                .map(|workspace| WorkspaceSlot {
                    index: label_index,
                    is_active: workspace.is_active,
                    is_focused: workspace.is_focused,
                    is_occupied: workspace.is_occupied,
                    is_urgent: workspace.is_urgent,
                })
                .unwrap_or(WorkspaceSlot {
                    index: label_index,
                    ..WorkspaceSlot::default()
                })
        })
    }

    fn tasks_for(&self, output: Option<&str>, output_group: u8) -> Arc<[WindowTask]> {
        output
            .and_then(|output| task_list_for_output(&self.task_lists, output))
            .or_else(|| self.task_lists.get(output_group.min(1) as usize))
            .or_else(|| self.task_lists.iter().find(|list| list.is_focused))
            .map_or_else(|| Arc::from([]), |list| Arc::clone(&list.tasks))
    }
}

fn task_list_for_output<'a>(task_lists: &'a [TaskList], output: &str) -> Option<&'a TaskList> {
    task_lists
        .binary_search_by(|list| list.output.as_deref().cmp(&Some(output)))
        .ok()
        .map(|index| &task_lists[index])
}

impl WorkspaceModel {
    fn workspace_for(
        &self,
        index: u8,
        output: Option<&str>,
        output_group: u8,
    ) -> Option<&WorkspaceInfo> {
        output
            .and_then(|output| self.workspace_for_output(index, output))
            .or_else(|| {
                self.workspaces
                    .iter()
                    .filter(|workspace| workspace.index == index)
                    .nth(output_group.min(1) as usize)
            })
            .or_else(|| {
                self.workspaces
                    .iter()
                    .filter(|workspace| workspace.index == index)
                    .max_by_key(|workspace| (workspace.is_focused, workspace.is_active))
            })
    }

    fn workspace_for_output(&self, index: u8, output: &str) -> Option<&WorkspaceInfo> {
        self.workspaces
            .binary_search_by(|workspace| {
                workspace
                    .output
                    .as_deref()
                    .cmp(&Some(output))
                    .then(workspace.index.cmp(&index))
            })
            .ok()
            .map(|position| &self.workspaces[position])
    }
}

pub enum NiriEvent {
    State(Arc<NiriModel>),
}

pub fn spawn(events: UiSender<NiriEvent>, config: &Config) {
    let config = config.clone();
    thread::Builder::new()
        .name("nibari-niri".into())
        .spawn(move || listen_forever(events, config))
        .expect("failed to start niri IPC thread");
}

fn listen_forever(events: UiSender<NiriEvent>, config: Config) {
    let mut icons = ApplicationIconCache::new(&config);
    loop {
        if let Err(error) = listen(&events, &mut icons) {
            log::warn!("niri IPC: {error}; reconnecting");
            thread::sleep(Duration::from_secs(2));
        }
    }
}

fn listen(events: &UiSender<NiriEvent>, icons: &mut ApplicationIconCache) -> anyhow::Result<()> {
    let mut socket = Socket::connect()?;
    match socket.send(Request::EventStream)? {
        Ok(Response::Handled) => log::info!("niri IPC: event stream connected"),
        Ok(response) => anyhow::bail!("unexpected response: {response:?}"),
        Err(message) => anyhow::bail!("{message}"),
    }

    let mut state = EventStreamState::default();
    let mut previous = Arc::new(NiriModel::default());
    let mut read_event = socket.read_events();

    loop {
        let event = read_event()?;
        let relevant = affects_workspace_view(&event);
        let _ = state.apply(event);
        if relevant {
            let next = Arc::new(niri_model(&state, icons));
            if next.as_ref() != previous.as_ref() {
                events.send(NiriEvent::State(Arc::clone(&next)))?;
                previous = next;
            }
        }
    }
}

fn affects_workspace_view(event: &Event) -> bool {
    matches!(
        event,
        Event::WorkspacesChanged { .. }
            | Event::WorkspaceUrgencyChanged { .. }
            | Event::WorkspaceActivated { .. }
            | Event::WorkspaceActiveWindowChanged { .. }
            | Event::WindowsChanged { .. }
            | Event::WindowOpenedOrChanged { .. }
            | Event::WindowClosed { .. }
            | Event::WindowFocusChanged { .. }
            | Event::WindowUrgencyChanged { .. }
            | Event::WindowLayoutsChanged { .. }
            | Event::KeyboardLayoutsChanged { .. }
            | Event::KeyboardLayoutSwitched { .. }
    )
}

fn niri_model(state: &EventStreamState, icons: &mut ApplicationIconCache) -> NiriModel {
    let workspaces = workspace_model(state);
    let task_lists = task_lists(state, icons);
    icons.retain_open_applications(state);
    NiriModel {
        workspaces,
        task_lists,
        keyboard_layout: Arc::from(keyboard_layout(state)),
    }
}

fn keyboard_layout(state: &EventStreamState) -> String {
    state
        .keyboard_layouts
        .keyboard_layouts
        .as_ref()
        .and_then(|layouts| layouts.names.get(layouts.current_idx as usize))
        .map(|name| short_keyboard_layout(name))
        .unwrap_or_default()
}

fn short_keyboard_layout(name: &str) -> String {
    let name = name.trim();
    let label = match name {
        "English (US)" => "US",
        "Russian" => "RU",
        _ => name
            .rsplit_once('(')
            .and_then(|(_, suffix)| suffix.strip_suffix(')'))
            .map(str::trim)
            .filter(|suffix| !suffix.is_empty() && suffix.len() <= 4)
            .unwrap_or(name),
    };

    label
        .chars()
        .filter(|char| char.is_alphanumeric())
        .take(4)
        .collect::<String>()
        .to_lowercase()
}

fn workspace_model(state: &EventStreamState) -> WorkspaceModel {
    let occupied: HashSet<_> = state
        .windows
        .windows
        .values()
        .filter_map(|window| window.workspace_id)
        .collect();
    let mut workspaces: Vec<_> = state
        .workspaces
        .workspaces
        .values()
        .filter(|workspace| (1..=10).contains(&workspace.idx))
        .map(|workspace| WorkspaceInfo {
            index: workspace.idx,
            output: workspace.output.clone(),
            is_active: workspace.is_active,
            is_focused: workspace.is_focused,
            is_occupied: occupied.contains(&workspace.id),
            is_urgent: workspace.is_urgent,
        })
        .collect();
    workspaces.sort_unstable_by(|left, right| {
        left.output
            .cmp(&right.output)
            .then(left.index.cmp(&right.index))
    });
    WorkspaceModel { workspaces }
}

fn window_tape_order_key(
    window_id: u64,
    position: Option<(usize, usize)>,
) -> (u8, usize, usize, u64) {
    match position {
        Some((column, tile)) => (0, column, tile, window_id),
        None => (1, 0, 0, window_id),
    }
}

fn task_lists(state: &EventStreamState, icons: &mut ApplicationIconCache) -> Vec<TaskList> {
    let mut windows_by_workspace: HashMap<_, Vec<_>> =
        HashMap::with_capacity(state.windows.windows.len());
    state
        .windows
        .windows
        .values()
        .filter_map(|window| {
            window
                .workspace_id
                .map(|workspace_id| (workspace_id, window))
        })
        .for_each(|(workspace_id, window)| {
            windows_by_workspace
                .entry(workspace_id)
                .or_default()
                .push(window);
        });
    let mut workspaces: Vec<_> = state
        .workspaces
        .workspaces
        .values()
        .filter(|workspace| workspace.is_active)
        .collect();
    workspaces.sort_unstable_by(|left, right| left.output.cmp(&right.output));

    workspaces
        .into_iter()
        .map(|workspace| {
            let mut windows = windows_by_workspace
                .remove(&workspace.id)
                .unwrap_or_default();
            windows.sort_unstable_by_key(|window| {
                window_tape_order_key(window.id, window.layout.pos_in_scrolling_layout)
            });

            let mut tasks = Vec::with_capacity(windows.len());
            windows.into_iter().for_each(|window| {
                tasks.push(WindowTask {
                    id: window.id,
                    label: window_label(window.title.as_deref(), window.app_id.as_deref()),
                    app_id: window.app_id.clone(),
                    is_focused: window.is_focused,
                    is_urgent: window.is_urgent,
                    icon: icons.icon_for(window.app_id.as_deref(), window.id),
                });
            });

            TaskList {
                output: workspace.output.clone(),
                is_focused: workspace.is_focused,
                tasks: Arc::from(tasks),
            }
        })
        .collect()
}

fn window_label(title: Option<&str>, app_id: Option<&str>) -> String {
    let label = title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .or_else(|| app_id.map(str::trim).filter(|app_id| !app_id.is_empty()))
        .unwrap_or("Untitled");
    let mut chars = label.chars();
    let mut shortened: String = chars.by_ref().take(256).collect();
    if chars.next().is_some() {
        shortened.push('…');
    }
    shortened
}

struct ApplicationIconCache {
    loader: IconLoader,
    target_size: u32,
    icons: HashMap<String, ApplicationIcon>,
}

impl ApplicationIconCache {
    fn new(config: &Config) -> Self {
        Self {
            loader: IconLoader::new(config),
            target_size: config.task_icon_size,
            icons: HashMap::new(),
        }
    }

    fn icon_for(&mut self, app_id: Option<&str>, window_id: u64) -> ApplicationIcon {
        if let Some(app_id) = app_id.filter(|app_id| !app_id.is_empty()) {
            if let Some(icon) = self.icons.get(app_id) {
                return icon.clone();
            }

            let image = self
                .loader
                .load_application(app_id, self.target_size)
                .unwrap_or_else(|| fallback_icon(app_id, self.target_size));
            let icon = application_icon(image);
            self.icons.insert(app_id.to_owned(), icon.clone());
            return icon;
        }

        let key = format!("@window:{window_id}");
        if let Some(icon) = self.icons.get(&key) {
            return icon.clone();
        }

        let icon = application_icon(fallback_icon(&key, self.target_size));
        self.icons.insert(key, icon.clone());
        icon
    }

    fn retain_open_applications(&mut self, state: &EventStreamState) {
        let (window_ids, app_ids) = open_application_keys(state);
        self.icons
            .retain(|key, _| is_open_icon_key(key, &window_ids, &app_ids));
    }
}

fn application_icon(image: tiny_skia::Pixmap) -> ApplicationIcon {
    ApplicationIcon {
        revision: image_revision(&image),
        pixels: Arc::from(image.data()),
        width: image.width(),
        height: image.height(),
    }
}

fn open_application_keys(state: &EventStreamState) -> (HashSet<u64>, HashSet<&str>) {
    let window_ids = state.windows.windows.keys().copied().collect();
    let app_ids = state
        .windows
        .windows
        .values()
        .filter_map(|window| window.app_id.as_deref().filter(|app_id| !app_id.is_empty()))
        .collect();
    (window_ids, app_ids)
}

fn is_open_icon_key(key: &str, window_ids: &HashSet<u64>, app_ids: &HashSet<&str>) -> bool {
    key.strip_prefix("@window:")
        .and_then(|id| id.parse::<u64>().ok())
        .map_or_else(|| app_ids.contains(key), |id| window_ids.contains(&id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_icon() -> ApplicationIcon {
        ApplicationIcon {
            revision: 1,
            pixels: Arc::from([1, 2, 3, 255]),
            width: 1,
            height: 1,
        }
    }

    fn sample_task(id: u64, label: &str) -> WindowTask {
        WindowTask {
            id,
            label: label.into(),
            app_id: Some("example".into()),
            is_focused: id == 1,
            is_urgent: false,
            icon: sample_icon(),
        }
    }

    #[test]
    fn open_icon_keys_keep_live_application_and_window_entries() {
        let windows = HashSet::from([42_u64]);
        let applications = HashSet::from(["terminal"]);

        assert!(is_open_icon_key("terminal", &windows, &applications));
        assert!(is_open_icon_key("@window:42", &windows, &applications));
        assert!(!is_open_icon_key("closed", &windows, &applications));
        assert!(!is_open_icon_key("@window:7", &windows, &applications));
    }

    fn two_output_model() -> NiriModel {
        NiriModel {
            workspaces: WorkspaceModel {
                workspaces: vec![
                    WorkspaceInfo {
                        index: 1,
                        output: Some("DP-1".into()),
                        is_active: true,
                        is_focused: true,
                        is_occupied: true,
                        is_urgent: false,
                    },
                    WorkspaceInfo {
                        index: 1,
                        output: Some("HDMI-A-1".into()),
                        is_active: true,
                        is_focused: false,
                        is_occupied: false,
                        is_urgent: false,
                    },
                ],
            },
            task_lists: vec![
                TaskList {
                    output: Some("DP-1".into()),
                    is_focused: true,
                    tasks: Arc::from([sample_task(1, "Terminal")]),
                },
                TaskList {
                    output: Some("HDMI-A-1".into()),
                    is_focused: false,
                    tasks: Arc::from([]),
                },
            ],
            keyboard_layout: Arc::from("us"),
        }
    }

    #[test]
    fn surface_content_selects_data_for_its_output() {
        let model = two_output_model();

        let dp = model.surface_content(Some("DP-1"), 0);
        let hdmi = model.surface_content(Some("HDMI-A-1"), 1);

        assert_eq!(dp.workspaces[0].index, 1);
        assert!(dp.workspaces[0].is_focused);
        assert_eq!(dp.tasks[0].label, "Terminal");
        assert_eq!(hdmi.workspaces[0].index, 6);
        assert!(hdmi.tasks.is_empty());
    }

    #[test]
    fn surface_content_equality_is_local_to_output_but_includes_keyboard_layout() {
        let first = two_output_model();
        let mut changed = first.clone();
        changed.task_lists[0].tasks = Arc::from([sample_task(2, "Editor")]);

        assert_ne!(
            first.surface_content(Some("DP-1"), 0),
            changed.surface_content(Some("DP-1"), 0)
        );
        assert_eq!(
            first.surface_content(Some("HDMI-A-1"), 1),
            changed.surface_content(Some("HDMI-A-1"), 1)
        );
        changed.keyboard_layout = Arc::from("ru");
        assert_ne!(
            first.surface_content(Some("HDMI-A-1"), 1),
            changed.surface_content(Some("HDMI-A-1"), 1)
        );
    }

    #[test]
    fn tape_order_places_tiled_windows_left_to_right_before_floating_windows() {
        let mut windows = [
            (80, None),
            (30, Some((3, 1))),
            (20, Some((1, 2))),
            (10, Some((1, 1))),
            (40, Some((2, 1))),
            (70, None),
        ];

        windows.sort_unstable_by_key(|(id, position)| window_tape_order_key(*id, *position));

        assert_eq!(windows.map(|(id, _)| id), [10, 20, 40, 30, 70, 80]);
    }

    #[test]
    fn labels_have_requested_order() {
        assert_eq!(
            WORKSPACE_LABELS,
            ["一", "二", "三", "四", "五", "六", "七", "八", "九", "零"]
        );
    }

    #[test]
    fn slots_are_filtered_by_output() {
        let model = NiriModel {
            workspaces: WorkspaceModel {
                workspaces: vec![
                    WorkspaceInfo {
                        index: 1,
                        output: Some("DP-1".into()),
                        is_active: true,
                        is_focused: true,
                        is_occupied: true,
                        is_urgent: false,
                    },
                    WorkspaceInfo {
                        index: 1,
                        output: Some("HDMI-A-1".into()),
                        is_active: true,
                        is_focused: false,
                        is_occupied: false,
                        is_urgent: false,
                    },
                ],
            },
            task_lists: vec![],
            keyboard_layout: Arc::from(""),
        };

        let dp = model.slots_for(Some("DP-1"), 0);
        let hdmi = model.slots_for(Some("HDMI-A-1"), 1);
        assert!(dp[0].is_focused && dp[0].is_occupied);
        assert!(hdmi[0].is_active && !hdmi[0].is_focused);
        assert_eq!(dp[4].index, 5);
        assert_eq!(hdmi[0].index, 6);
        assert_eq!(hdmi[4].index, 10);
    }

    #[test]
    fn task_lists_are_filtered_by_output() {
        let icon = ApplicationIcon {
            revision: 1,
            pixels: Arc::from([1, 2, 3, 255]),
            width: 1,
            height: 1,
        };
        let model = NiriModel {
            workspaces: WorkspaceModel::default(),
            task_lists: vec![
                TaskList {
                    output: Some("DP-1".into()),
                    is_focused: true,
                    tasks: Arc::from([WindowTask {
                        id: 1,
                        label: "Terminal".into(),
                        app_id: Some("terminal".into()),
                        is_focused: true,
                        is_urgent: false,
                        icon: icon.clone(),
                    }]),
                },
                TaskList {
                    output: Some("HDMI-A-1".into()),
                    is_focused: false,
                    tasks: Arc::from([]),
                },
            ],
            keyboard_layout: Arc::from(""),
        };

        assert_eq!(model.tasks_for(Some("DP-1"), 0)[0].label, "Terminal");
        assert!(model.tasks_for(Some("HDMI-A-1"), 1).is_empty());
    }

    #[test]
    fn title_is_preferred_with_application_id_as_fallback() {
        assert_eq!(
            window_label(Some("  Browser  "), Some("firefox")),
            "Browser"
        );
        assert_eq!(window_label(Some(" "), Some("firefox")), "firefox");
        assert_eq!(window_label(None, None), "Untitled");
    }

    #[test]
    fn keyboard_layout_names_are_shortened_for_bar() {
        assert_eq!(short_keyboard_layout("English (US)"), "us");
        assert_eq!(short_keyboard_layout("Russian"), "ru");
        assert_eq!(short_keyboard_layout("German (DE)"), "de");
    }
}
