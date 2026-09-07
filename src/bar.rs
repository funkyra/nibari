use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use calloop::signals::{Signal, Signals};
use calloop_wayland_source::WaylandSource;
use chrono::{
    Local, Timelike,
    format::{Fixed, Item, Numeric, StrftimeItems},
};
use smithay_client_toolkit::reexports::protocols::xdg::shell::client::xdg_positioner;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData},
    delegate_registry,
    output::{OutputHandler, OutputState},
    reexports::calloop::{
        EventLoop,
        channel::{self, Event as ChannelEvent},
        timer::{TimeoutAction, Timer},
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        xdg::{
            XdgPositioner, XdgShell,
            dialog::{Dialog, DialogHandler},
            popup::{Popup, PopupConfigure, PopupHandler},
            window::{Window, WindowConfigure, WindowHandler},
        },
    },
    shm::{
        Shm, ShmHandler,
        slot::{Buffer as ShmBuffer, SlotPool},
    },
};
use tiny_skia::PixmapMut;
use wayland_client::{
    Connection, QueueHandle, delegate_noop,
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_region, wl_seat, wl_shm, wl_surface},
};

use crate::{
    config::Config,
    niri::{
        self, FocusCommand, NiriEvent, NiriHandle, NiriModel, SurfaceContent,
        WORKSPACES_PER_OUTPUT, WindowTask, WorkspaceSlot,
    },
    render::{
        HitTarget, Hitbox, MenuHitbox, MenuSelection, PreparedMenu, RenderContent, Renderer,
        hit_target_at,
    },
    tray::{TrayEvent, TrayHandle, TrayIcon, TrayMenuEntry, TrayMenuPopup},
};

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

pub fn run(config: Config) -> Result<()> {
    // Block control signals before starting worker threads so they inherit the mask.
    let signals = Signals::new(&[Signal::SIGUSR1, Signal::SIGUSR2, Signal::SIGHUP])
        .context("failed to register control signals")?;
    let connection = Connection::connect_to_env().context("failed to connect to Wayland")?;
    let (globals, event_queue) =
        registry_queue_init(&connection).context("failed to obtain Wayland globals")?;
    let queue_handle = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &queue_handle)
        .context("compositor does not provide wl_compositor")?;
    let layer_shell = LayerShell::bind(&globals, &queue_handle)
        .context("compositor does not provide wlr-layer-shell")?;
    let xdg_shell =
        XdgShell::bind(&globals, &queue_handle).context("compositor does not provide xdg-shell")?;
    let shm =
        Shm::bind(&globals, &queue_handle).context("compositor does not provide shared memory")?;

    let mut event_loop: EventLoop<App> =
        EventLoop::try_new().context("failed to create the event loop")?;
    event_loop
        .handle()
        .insert_source(signals, |event, _, app| match event.signal() {
            Signal::SIGUSR1 => app.set_bars_hidden(!app.bars_hidden),
            Signal::SIGUSR2 => app.set_bars_hidden(true),
            Signal::SIGHUP => app.set_bars_hidden(false),
            _ => {}
        })
        .map_err(|error| {
            anyhow::anyhow!("failed to register signals with the event loop: {error}")
        })?;
    WaylandSource::new(connection, event_queue)
        .insert(event_loop.handle())
        .context("failed to attach Wayland to the event loop")?;

    let (tray_events, tray_channel) = channel::channel();
    let tray = TrayHandle::spawn(tray_events, &config);
    let (niri_events, niri_channel) = channel::channel();
    let niri = niri::spawn(niri_events, &config);
    let clock_granularity = clock_granularity(&config.clock_format);
    let renderer = Renderer::new(&config);
    let background_opaque = config.background_rgba()[3] == 255;

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &queue_handle),
        seat_state: SeatState::new(&globals, &queue_handle),
        compositor,
        layer_shell,
        xdg_shell,
        shm,
        queue_handle,
        config,
        renderer,
        background_opaque,
        bars_hidden: false,
        surfaces: Vec::new(),
        menu_surface: None,
        pointers: Vec::new(),
        keyboards: Vec::new(),
        pending_menu_grab: None,
        clock: String::new(),
        clock_granularity,
        tray_icons: Vec::new(),
        niri_model: Arc::new(NiriModel::default()),
        niri,
        tray,
    };
    app.update_clock();

    event_loop
        .handle()
        .insert_source(tray_channel, |event, _, app| {
            if let ChannelEvent::Msg(event) = event {
                match event {
                    TrayEvent::Items(items) if app.tray_icons != items => {
                        app.tray_icons = items;
                        app.mark_bars_dirty();
                    }
                    TrayEvent::Items(_) => {}
                    TrayEvent::Menu(menu) => app.show_menu(menu),
                }
            }
        })
        .map_err(|error| anyhow::anyhow!("failed to attach systray to the event loop: {error}"))?;

    event_loop
        .handle()
        .insert_source(niri_channel, |event, _, app| {
            if let ChannelEvent::Msg(event) = event {
                match event {
                    NiriEvent::State(model) => app.update_niri_model(model),
                    NiriEvent::WindowClosed(window_id) => {
                        app.renderer.evict_task_text(window_id);
                    }
                }
            }
        })
        .map_err(|error| anyhow::anyhow!("failed to attach niri IPC to the event loop: {error}"))?;

    event_loop
        .handle()
        .insert_source(Timer::from_duration(app.next_clock_delay()), |_, _, app| {
            app.update_clock();
            TimeoutAction::ToDuration(app.next_clock_delay())
        })
        .map_err(|error| anyhow::anyhow!("failed to start the clock timer: {error}"))?;

    loop {
        event_loop
            .dispatch(None, &mut app)
            .context("event loop error")?;
        app.draw_dirty()?;
    }
}

struct BarSurface {
    layer: LayerSurface,
    pool: SlotPool,
    output: wl_output::WlOutput,
    output_name: Option<String>,
    output_position: (i32, i32),
    output_size: (u32, u32),
    workspace_group: u8,
    content: SurfaceContent,
    logical_width: u32,
    logical_height: u32,
    scale: u32,
    configured: bool,
    dirty: bool,
    frame_pending: bool,
    hitboxes: Vec<Hitbox>,
    buffers: Vec<ShmBuffer>,
    format: wl_shm::Format,
}

struct MenuSurface {
    popup: Popup,
    parent: LayerSurface,
    pool: SlotPool,
    address: String,
    menu_path: String,
    entries: Vec<TrayMenuEntry>,
    pages: Vec<MenuPage>,
    title: Option<String>,
    prepared: PreparedMenu,
    selected: Option<MenuSelection>,
    anchor_x: i32,
    output_width: u32,
    output_height: u32,
    reposition_token: u32,
    logical_width: u32,
    logical_height: u32,
    scale: u32,
    configured: bool,
    dirty: bool,
    frame_pending: bool,
    hitboxes: Vec<MenuHitbox>,
    buffers: Vec<ShmBuffer>,
    format: wl_shm::Format,
}

impl Drop for MenuSurface {
    fn drop(&mut self) {
        self.parent
            .set_keyboard_interactivity(KeyboardInteractivity::None);
        self.parent.commit();
    }
}

struct MenuPage {
    title: Option<String>,
    entries: Vec<TrayMenuEntry>,
}

struct App {
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    xdg_shell: XdgShell,
    shm: Shm,
    queue_handle: QueueHandle<Self>,
    config: Config,
    renderer: Renderer,
    background_opaque: bool,
    bars_hidden: bool,
    surfaces: Vec<BarSurface>,
    menu_surface: Option<MenuSurface>,
    pointers: Vec<(wl_seat::WlSeat, wl_pointer::WlPointer)>,
    keyboards: Vec<(wl_seat::WlSeat, wl_keyboard::WlKeyboard)>,
    pending_menu_grab: Option<(wl_seat::WlSeat, u32, i32, i32)>,
    clock: String,
    clock_granularity: ClockGranularity,
    tray_icons: Vec<TrayIcon>,
    niri_model: Arc<NiriModel>,
    niri: NiriHandle,
    tray: TrayHandle,
}

#[derive(Clone, Copy)]
struct FrameSpec<'a> {
    width: u32,
    height: u32,
    scale: u32,
    format: wl_shm::Format,
    clock: &'a str,
    keyboard_layout: &'a str,
    tray_icons: &'a [TrayIcon],
    workspaces: &'a [WorkspaceSlot; WORKSPACES_PER_OUTPUT],
    tasks: &'a [WindowTask],
}

#[derive(Clone, Copy)]
struct MenuFrame {
    width: u32,
    height: u32,
    format: wl_shm::Format,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ClockGranularity {
    Minute,
    Second,
    Subsecond,
}

impl App {
    fn set_bars_hidden(&mut self, hidden: bool) {
        if self.bars_hidden == hidden {
            return;
        }
        self.bars_hidden = hidden;
        if self.bars_hidden {
            self.menu_surface = None;
            self.pending_menu_grab = None;
            // Destroying the layer surfaces also releases their exclusive zones.
            self.surfaces.clear();
        } else {
            let outputs: Vec<_> = self.output_state.outputs().collect();
            for output in outputs {
                self.add_output(output);
            }
        }
        log::info!(
            "bar visibility: {}",
            if self.bars_hidden { "hidden" } else { "shown" }
        );
    }

    fn add_output(&mut self, output: wl_output::WlOutput) {
        if self.bars_hidden || self.surfaces.iter().any(|surface| surface.output == output) {
            return;
        }

        let info = self.output_state.info(&output);
        let scale = info
            .as_ref()
            .map_or(1, |info| info.scale_factor.max(1) as u32);
        let output_position = info
            .as_ref()
            .and_then(|info| info.logical_position)
            .unwrap_or_default();
        let output_name = info.as_ref().and_then(|info| info.name.clone());
        let output_size = info
            .as_ref()
            .and_then(|info| info.logical_size)
            .map(|(width, height)| (width.max(0) as u32, height.max(0) as u32))
            .unwrap_or_default();
        let content = self.niri_model.surface_content(output_name.as_deref(), 0);

        let surface = self.compositor.create_surface(&self.queue_handle);
        let layer = self.layer_shell.create_layer_surface(
            &self.queue_handle,
            surface,
            Layer::Top,
            Some("nibari"),
            Some(&output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::LEFT | Anchor::RIGHT);
        layer.set_size(0, self.config.height);
        layer.set_exclusive_zone(self.config.height as i32);
        let _ = layer.set_buffer_scale(scale);
        layer.commit();

        let pool = match SlotPool::new(1, &self.shm) {
            Ok(pool) => pool,
            Err(error) => {
                log::error!("failed to create shared memory pool: {error}");
                return;
            }
        };

        log::info!(
            "output added: {} scale={scale}",
            output_name.as_deref().unwrap_or("unknown")
        );
        self.surfaces.push(BarSurface {
            layer,
            pool,
            output,
            output_name,
            output_position,
            output_size,
            workspace_group: 0,
            content,
            logical_width: 0,
            logical_height: self.config.height,
            scale,
            configured: false,
            dirty: true,
            frame_pending: false,
            hitboxes: Vec::new(),
            buffers: Vec::with_capacity(2),
            format: wl_shm::Format::Argb8888,
        });
        self.assign_workspace_groups();
    }

    fn update_output_info(&mut self, output: &wl_output::WlOutput) {
        let Some(info) = self.output_state.info(output) else {
            return;
        };
        let niri_model = &self.niri_model;

        self.surfaces
            .iter_mut()
            .filter(|surface| &surface.output == output)
            .for_each(|surface| {
                surface.output_position = info.logical_position.unwrap_or_default();
                surface.output_size = info
                    .logical_size
                    .map(|(width, height)| (width.max(0) as u32, height.max(0) as u32))
                    .unwrap_or_default();
                let output_changed = surface.output_name != info.name;
                surface.output_name.clone_from(&info.name);
                if output_changed {
                    surface.content = niri_model
                        .surface_content(surface.output_name.as_deref(), surface.workspace_group);
                }
                let scale = info.scale_factor.max(1) as u32;
                if surface.scale != scale {
                    surface.scale = scale;
                    let _ = surface.layer.set_buffer_scale(scale);
                    surface.buffers.clear();
                    surface.dirty = true;
                }
                surface.dirty |= output_changed;
            });
        self.assign_workspace_groups();
    }

    fn assign_workspace_groups(&mut self) {
        let groups = workspace_groups(
            self.surfaces
                .iter()
                .map(|surface| (surface.output_position, surface.output_name.clone())),
        );
        let niri_model = &self.niri_model;
        self.surfaces
            .iter_mut()
            .zip(groups)
            .for_each(|(surface, group)| {
                if surface.workspace_group != group {
                    surface.workspace_group = group;
                    surface.content = niri_model
                        .surface_content(surface.output_name.as_deref(), surface.workspace_group);
                    surface.dirty = true;
                }
            });
    }

    fn update_niri_model(&mut self, next_model: Arc<NiriModel>) {
        self.niri_model = next_model;
        self.surfaces.iter_mut().for_each(|surface| {
            let next = self
                .niri_model
                .surface_content(surface.output_name.as_deref(), surface.workspace_group);
            if content_changed(Some(&surface.content), &next) {
                surface.content = next;
                surface.dirty = true;
            }
        });
    }

    fn update_clock(&mut self) {
        let clock = Local::now().format(&self.config.clock_format).to_string();
        if clock != self.clock {
            self.clock = clock;
            self.mark_bars_dirty();
        }
    }

    fn next_clock_delay(&self) -> Duration {
        let now = Local::now();
        delay_until_next_tick(self.clock_granularity, now.second(), now.nanosecond())
    }

    fn mark_bars_dirty(&mut self) {
        self.surfaces
            .iter_mut()
            .for_each(|surface| surface.dirty = true);
    }

    fn draw_dirty(&mut self) -> Result<()> {
        let queue_handle = &self.queue_handle;
        let clock = &self.clock;
        let tray_icons = &self.tray_icons;
        let renderer = &mut self.renderer;
        let format = preferred_shm_format(self.shm.formats());

        self.surfaces
            .iter_mut()
            .filter(|surface| surface.configured && surface.dirty && !surface.frame_pending)
            .try_for_each(|surface| {
                if surface.format != format {
                    surface.format = format;
                    surface.buffers.clear();
                }
                draw_surface(surface, renderer, clock, tray_icons, queue_handle)
            })?;

        if let Some(menu) = self.menu_surface.as_mut()
            && menu.configured
            && menu.dirty
            && !menu.frame_pending
        {
            if menu.format != format {
                menu.format = format;
                menu.buffers.clear();
            }
            draw_menu_surface(menu, renderer, queue_handle)?;
        }

        Ok(())
    }

    fn show_menu(&mut self, menu: TrayMenuPopup) {
        let Some(target) = self.menu_target(menu.x, menu.y) else {
            return;
        };
        let Some((_, _, grab_x, grab_y)) = self.pending_menu_grab.as_ref() else {
            log::warn!("ignoring tray menu without a matching pointer grab");
            return;
        };
        if (*grab_x, *grab_y) != (menu.x, menu.y) {
            log::warn!("ignoring tray menu with a stale pointer grab");
            return;
        }
        let (seat, serial, _, _) = self
            .pending_menu_grab
            .take()
            .expect("validated tray menu grab remains available");
        let mut prepared = self.renderer.prepare_menu(&menu.items, target.scale, None);
        constrain_menu(
            &mut prepared,
            target.scale,
            target.logical_width,
            target.logical_height.saturating_sub(self.config.height),
        );
        let (buffer_width, buffer_height) = prepared.size();
        let logical_width =
            bounded_dimension(buffer_width.div_ceil(target.scale), target.logical_width);
        let logical_height =
            bounded_dimension(buffer_height.div_ceil(target.scale), target.logical_height);
        let left = target
            .local_x
            .max(0)
            .min(target.logical_width.saturating_sub(logical_width) as i32);
        let top = menu_top(self.config.height, logical_height, target.logical_height);

        self.menu_surface = None;

        let positioner =
            match menu_positioner(&self.xdg_shell, left, top, logical_width, logical_height) {
                Ok(positioner) => positioner,
                Err(error) => {
                    log::error!("failed to create tray menu positioner: {error}");
                    return;
                }
            };
        let surface = self.compositor.create_surface(&self.queue_handle);
        let popup = match Popup::from_surface(
            None,
            &positioner,
            &self.queue_handle,
            surface,
            &self.xdg_shell,
        ) {
            Ok(popup) => popup,
            Err(error) => {
                log::error!("failed to create tray menu popup: {error}");
                return;
            }
        };
        target.parent.get_popup(popup.xdg_popup());

        let pool = match SlotPool::new(1, &self.shm) {
            Ok(pool) => pool,
            Err(error) => {
                log::error!("failed to create menu shared memory pool: {error}");
                return;
            }
        };

        // Niri grants keyboard grabs only to popups whose parent accepts focus.
        // Restore the panel's normal non-interactive state when this menu drops.
        target
            .parent
            .set_keyboard_interactivity(KeyboardInteractivity::OnDemand);
        target.parent.commit();
        popup.xdg_popup().grab(&seat, serial);
        popup.wl_surface().set_buffer_scale(target.scale as i32);
        popup.wl_surface().commit();

        self.menu_surface = Some(MenuSurface {
            popup,
            parent: target.parent,
            pool,
            address: menu.address,
            menu_path: menu.menu_path,
            entries: menu.items,
            pages: Vec::new(),
            title: None,
            prepared,
            selected: None,
            anchor_x: target.local_x,
            output_width: target.logical_width,
            output_height: target.logical_height,
            reposition_token: 1,
            logical_width,
            logical_height,
            scale: target.scale,
            configured: false,
            dirty: true,
            frame_pending: false,
            hitboxes: Vec::new(),
            buffers: Vec::with_capacity(2),
            format: wl_shm::Format::Argb8888,
        });
    }

    fn menu_target(&self, x: i32, y: i32) -> Option<MenuTarget> {
        self.surfaces
            .iter()
            .filter(|surface| surface.configured)
            .find_map(|surface| {
                let local_x = x - surface.output_position.0;
                let local_y = y - surface.output_position.1;
                (local_x >= 0
                    && local_y >= 0
                    && local_x < surface.logical_width as i32
                    && local_y < surface.logical_height as i32)
                    .then(|| MenuTarget {
                        parent: surface.layer.clone(),
                        scale: surface.scale,
                        logical_width: surface.logical_width,
                        logical_height: surface.output_size.1,
                        local_x,
                    })
            })
            .or_else(|| {
                self.surfaces
                    .iter()
                    .filter(|surface| surface.configured)
                    .min_by_key(|surface| (surface.output_position.0 - x).abs())
                    .map(|surface| MenuTarget {
                        parent: surface.layer.clone(),
                        scale: surface.scale,
                        logical_width: surface.logical_width,
                        logical_height: surface.output_size.1,
                        local_x: x - surface.output_position.0,
                    })
            })
    }

    fn update_menu_page(&mut self) {
        let Some(menu) = self.menu_surface.as_mut() else {
            return;
        };
        menu.prepared =
            self.renderer
                .prepare_menu(&menu.entries, menu.scale, menu.title.as_deref());
        constrain_menu(
            &mut menu.prepared,
            menu.scale,
            menu.output_width,
            menu.output_height.saturating_sub(self.config.height),
        );
        let (buffer_width, buffer_height) = menu.prepared.size();
        let logical_width = bounded_dimension(buffer_width.div_ceil(menu.scale), menu.output_width);
        let logical_height =
            bounded_dimension(buffer_height.div_ceil(menu.scale), menu.output_height);
        let left = menu
            .anchor_x
            .max(0)
            .min(menu.output_width.saturating_sub(logical_width) as i32);
        let top = menu_top(self.config.height, logical_height, menu.output_height);

        menu.logical_width = logical_width;
        menu.logical_height = logical_height;
        menu.selected = None;
        menu.hitboxes.clear();
        menu.buffers.clear();
        menu.dirty = true;
        match menu_positioner(&self.xdg_shell, left, top, logical_width, logical_height) {
            Ok(positioner) => {
                menu.popup.reposition(&positioner, menu.reposition_token);
                menu.reposition_token = menu.reposition_token.wrapping_add(1).max(1);
            }
            Err(error) => log::warn!("failed to reposition tray menu: {error}"),
        }
    }

    fn go_back_menu(&mut self) {
        let Some(menu) = self.menu_surface.as_mut() else {
            return;
        };
        let Some(page) = menu.pages.pop() else {
            self.menu_surface = None;
            return;
        };
        menu.title = page.title;
        menu.entries = page.entries;
        self.update_menu_page();
    }

    fn open_submenu(&mut self, item_id: i32) {
        let Some(menu) = self.menu_surface.as_mut() else {
            return;
        };
        let Some(entry) = menu
            .entries
            .iter()
            .find(|entry| entry.id == item_id && entry.enabled && !entry.submenu.is_empty())
            .cloned()
        else {
            return;
        };
        menu.pages.push(MenuPage {
            title: menu.title.take(),
            entries: std::mem::take(&mut menu.entries),
        });
        menu.title = Some(entry.label);
        menu.entries = entry.submenu;
        self.update_menu_page();
    }

    fn apply_menu_action(&mut self, action: MenuInputAction) {
        match action {
            MenuInputAction::None => {}
            MenuInputAction::Dismiss => self.menu_surface = None,
            MenuInputAction::Back => self.go_back_menu(),
            MenuInputAction::OpenSubmenu(item_id) => self.open_submenu(item_id),
            MenuInputAction::Activate(item_id) => {
                let command = self
                    .menu_surface
                    .as_ref()
                    .map(|menu| (menu.address.clone(), menu.menu_path.clone(), item_id));
                self.menu_surface = None;
                if let Some((address, menu_path, item_id)) = command {
                    self.tray.menu_item(address, menu_path, item_id);
                }
            }
        }
    }

    fn handle_menu_key(&mut self, keysym: Keysym) {
        let Some(menu) = self.menu_surface.as_mut() else {
            return;
        };
        match keysym {
            Keysym::Up | Keysym::Down => {
                let selected = menu
                    .prepared
                    .next_selection(menu.selected, keysym == Keysym::Up);
                if menu.selected != selected {
                    menu.selected = selected;
                    menu.dirty = true;
                }
                if let Some(selected) = selected
                    && menu.prepared.ensure_visible(selected)
                {
                    menu.dirty = true;
                }
            }
            Keysym::Escape => self.menu_surface = None,
            Keysym::Left if !menu.pages.is_empty() => self.go_back_menu(),
            Keysym::Right => {
                let item_id = match menu.selected {
                    Some(MenuSelection::Item(item_id)) => item_id,
                    _ => return,
                };
                if menu
                    .entries
                    .iter()
                    .any(|entry| entry.id == item_id && !entry.submenu.is_empty())
                {
                    self.open_submenu(item_id);
                }
            }
            Keysym::Return | Keysym::KP_Enter => {
                let selection = menu.selected;
                let has_submenu = selection.is_some_and(|selection| match selection {
                    MenuSelection::Item(item_id) => menu
                        .entries
                        .iter()
                        .any(|entry| entry.id == item_id && !entry.submenu.is_empty()),
                    MenuSelection::Back => false,
                });
                let action =
                    menu_input_action(BTN_LEFT, selection, has_submenu, !menu.pages.is_empty());
                self.apply_menu_action(action);
            }
            _ => {}
        }
    }

    fn handle_click(&mut self, surface: &wl_surface::WlSurface, button: u32, x: f64, y: f64) {
        if let Some(menu) = self
            .menu_surface
            .as_ref()
            .filter(|menu| menu.popup.wl_surface() == surface)
        {
            let physical_x = (x * menu.scale as f64).floor() as i32;
            let physical_y = (y * menu.scale as f64).floor() as i32;
            let selection = menu.prepared.selection_at(physical_x, physical_y);
            let has_submenu = selection.is_some_and(|selection| match selection {
                MenuSelection::Item(item_id) => menu
                    .entries
                    .iter()
                    .any(|entry| entry.id == item_id && !entry.submenu.is_empty()),
                MenuSelection::Back => false,
            });
            let action = menu_input_action(button, selection, has_submenu, !menu.pages.is_empty());
            self.apply_menu_action(action);
            return;
        }

        let Some(bar) = self
            .surfaces
            .iter()
            .find(|bar| bar.layer.wl_surface() == surface)
        else {
            self.menu_surface = None;
            return;
        };

        self.menu_surface = None;
        match hit_target_at(&bar.hitboxes, bar.scale, x, y) {
            Some(HitTarget::Workspace { id, index }) if button == BTN_LEFT => {
                self.niri.focus(FocusCommand::Workspace {
                    id,
                    index,
                    output: bar.output_name.clone(),
                });
            }
            Some(HitTarget::Window(id)) if button == BTN_LEFT => {
                self.niri.focus(FocusCommand::Window(id));
            }
            Some(HitTarget::Tray(index)) => {
                if let Some(icon) = self.tray_icons.get(index) {
                    let global_x = bar.output_position.0 + x.round() as i32;
                    let global_y = bar.output_position.1 + y.round() as i32;
                    self.tray.click(icon.id.clone(), button, global_x, global_y);
                }
            }
            _ => {}
        }
    }
}

struct MenuTarget {
    parent: LayerSurface,
    scale: u32,
    logical_width: u32,
    logical_height: u32,
    local_x: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuInputAction {
    None,
    Dismiss,
    Back,
    OpenSubmenu(i32),
    Activate(i32),
}

fn menu_input_action(
    button: u32,
    selection: Option<MenuSelection>,
    has_submenu: bool,
    can_go_back: bool,
) -> MenuInputAction {
    match button {
        BTN_RIGHT if can_go_back => MenuInputAction::Back,
        BTN_RIGHT => MenuInputAction::Dismiss,
        BTN_LEFT => match selection {
            Some(MenuSelection::Back) => MenuInputAction::Back,
            Some(MenuSelection::Item(id)) if has_submenu => MenuInputAction::OpenSubmenu(id),
            Some(MenuSelection::Item(id)) => MenuInputAction::Activate(id),
            None => MenuInputAction::None,
        },
        _ => MenuInputAction::None,
    }
}

fn bounded_dimension(preferred: u32, output_bound: u32) -> u32 {
    if output_bound == 0 {
        preferred.max(1)
    } else {
        preferred.clamp(1, output_bound)
    }
}

fn constrain_menu(menu: &mut PreparedMenu, scale: u32, logical_width: u32, logical_height: u32) {
    if logical_width > 0 && logical_height > 0 {
        menu.constrain(
            logical_width.saturating_mul(scale),
            logical_height.saturating_mul(scale),
        );
    }
}

fn menu_top(panel_height: u32, menu_height: u32, output_height: u32) -> i32 {
    if output_height == 0 {
        panel_height as i32
    } else {
        panel_height.min(output_height.saturating_sub(menu_height)) as i32
    }
}

fn menu_positioner(
    xdg_shell: &XdgShell,
    left: i32,
    top: i32,
    width: u32,
    height: u32,
) -> Result<XdgPositioner> {
    let positioner = XdgPositioner::new(xdg_shell)?;
    positioner.set_size(width as i32, height as i32);
    positioner.set_anchor_rect(left, top, 1, 1);
    positioner.set_anchor(xdg_positioner::Anchor::TopLeft);
    positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
    positioner.set_constraint_adjustment(
        xdg_positioner::ConstraintAdjustment::FlipX
            | xdg_positioner::ConstraintAdjustment::SlideX
            | xdg_positioner::ConstraintAdjustment::FlipY
            | xdg_positioner::ConstraintAdjustment::SlideY,
    );
    Ok(positioner)
}

fn workspace_groups(outputs: impl IntoIterator<Item = ((i32, i32), Option<String>)>) -> Vec<u8> {
    let mut outputs: Vec<_> = outputs.into_iter().enumerate().collect();
    outputs.sort_unstable_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)));
    let mut groups = vec![0; outputs.len()];
    outputs
        .into_iter()
        .enumerate()
        .for_each(|(rank, (original_index, _))| {
            groups[original_index] = rank.min(1) as u8;
        });
    groups
}

fn content_changed(previous: Option<&SurfaceContent>, next: &SurfaceContent) -> bool {
    previous != Some(next)
}

fn draw_surface(
    surface: &mut BarSurface,
    renderer: &mut Renderer,
    clock: &str,
    tray_icons: &[TrayIcon],
    queue_handle: &QueueHandle<App>,
) -> Result<()> {
    let width = surface
        .logical_width
        .checked_mul(surface.scale)
        .context("output width is too large")?;
    let height = surface
        .logical_height
        .checked_mul(surface.scale)
        .context("bar height is too large")?;
    if width == 0 || height == 0 {
        return Ok(());
    }

    let stride = width.checked_mul(4).context("stride overflow")?;
    let content = &surface.content;
    let frame = FrameSpec {
        width,
        height,
        scale: surface.scale,
        format: surface.format,
        clock,
        keyboard_layout: content.keyboard_layout.as_ref(),
        tray_icons,
        workspaces: &content.workspaces,
        tasks: content.tasks.as_ref(),
    };
    let wayland_surface = surface.layer.wl_surface();
    let mut reusable = None;
    for (index, buffer) in surface.buffers.iter().enumerate() {
        if buffer.height() == height as i32
            && buffer.stride() == stride as i32
            && let Some(canvas) = buffer.canvas(&mut surface.pool)
        {
            render_canvas(canvas, renderer, &mut surface.hitboxes, frame)?;
            reusable = Some(index);
            break;
        }
    }

    if let Some(index) = reusable {
        surface.buffers[index]
            .attach_to(wayland_surface)
            .context("failed to reattach the Wayland buffer")?;
    } else {
        let (buffer, canvas) = surface
            .pool
            .create_buffer(width as i32, height as i32, stride as i32, surface.format)
            .context("failed to create the Wayland buffer")?;
        render_canvas(canvas, renderer, &mut surface.hitboxes, frame)?;
        buffer
            .attach_to(wayland_surface)
            .context("failed to attach the Wayland buffer")?;
        surface.buffers.push(buffer);
    }
    wayland_surface.damage_buffer(0, 0, width as i32, height as i32);
    wayland_surface.frame(queue_handle, FrameCallbackData(wayland_surface.clone()));
    wayland_surface.commit();

    surface.dirty = false;
    surface.frame_pending = true;
    Ok(())
}

fn draw_menu_surface(
    surface: &mut MenuSurface,
    renderer: &mut Renderer,
    queue_handle: &QueueHandle<App>,
) -> Result<()> {
    let width = surface
        .logical_width
        .checked_mul(surface.scale)
        .context("tray menu width is too large")?;
    let height = surface
        .logical_height
        .checked_mul(surface.scale)
        .context("tray menu height is too large")?;
    if width == 0 || height == 0 {
        return Ok(());
    }

    let stride = width.checked_mul(4).context("menu stride overflow")?;
    let wayland_surface = surface.popup.wl_surface();
    let frame = MenuFrame {
        width,
        height,
        format: surface.format,
    };
    let mut reusable = None;
    for (index, buffer) in surface.buffers.iter().enumerate() {
        if buffer.height() == height as i32
            && buffer.stride() == stride as i32
            && let Some(canvas) = buffer.canvas(&mut surface.pool)
        {
            render_menu_canvas(
                canvas,
                renderer,
                &mut surface.hitboxes,
                &surface.prepared,
                surface.selected,
                frame,
            )?;
            reusable = Some(index);
            break;
        }
    }

    if let Some(index) = reusable {
        surface.buffers[index]
            .attach_to(wayland_surface)
            .context("failed to reattach the Wayland menu buffer")?;
    } else {
        let (buffer, canvas) = surface
            .pool
            .create_buffer(width as i32, height as i32, stride as i32, surface.format)
            .context("failed to create the Wayland menu buffer")?;
        render_menu_canvas(
            canvas,
            renderer,
            &mut surface.hitboxes,
            &surface.prepared,
            surface.selected,
            frame,
        )?;
        buffer
            .attach_to(wayland_surface)
            .context("failed to attach the Wayland menu buffer")?;
        surface.buffers.push(buffer);
    }

    wayland_surface.damage_buffer(0, 0, width as i32, height as i32);
    wayland_surface.frame(queue_handle, FrameCallbackData(wayland_surface.clone()));
    wayland_surface.commit();

    surface.dirty = false;
    surface.frame_pending = true;
    Ok(())
}

fn render_canvas(
    canvas: &mut [u8],
    renderer: &mut Renderer,
    hitboxes: &mut Vec<Hitbox>,
    frame: FrameSpec<'_>,
) -> Result<()> {
    let mut pixmap =
        PixmapMut::from_bytes(canvas, frame.width, frame.height).context("invalid pixmap size")?;
    renderer.draw(
        &mut pixmap,
        RenderContent {
            scale: frame.scale,
            clock: frame.clock,
            keyboard_layout: frame.keyboard_layout,
            tray: frame.tray_icons,
            workspaces: frame.workspaces,
            tasks: frame.tasks,
        },
        hitboxes,
    );

    convert_canvas_for_wayland(&mut pixmap, frame.format);
    Ok(())
}

fn render_menu_canvas(
    canvas: &mut [u8],
    renderer: &mut Renderer,
    hitboxes: &mut Vec<MenuHitbox>,
    menu: &PreparedMenu,
    selected: Option<MenuSelection>,
    frame: MenuFrame,
) -> Result<()> {
    let mut pixmap = PixmapMut::from_bytes(canvas, frame.width, frame.height)
        .context("invalid menu pixmap size")?;
    renderer.draw_menu(&mut pixmap, menu, hitboxes, selected);
    convert_canvas_for_wayland(&mut pixmap, frame.format);
    Ok(())
}

fn convert_canvas_for_wayland(pixmap: &mut PixmapMut<'_>, format: wl_shm::Format) {
    if format == wl_shm::Format::Argb8888 {
        // tiny-skia: RGBA; wl_shm ARGB8888 on little-endian systems: BGRA.
        pixmap
            .data_mut()
            .chunks_exact_mut(4)
            .for_each(|pixel| pixel.swap(0, 2));
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        wayland_surface: &wl_surface::WlSurface,
        factor: i32,
    ) {
        let factor = factor.max(1) as u32;
        if let Some(surface) = self
            .surfaces
            .iter_mut()
            .find(|surface| surface.layer.wl_surface() == wayland_surface)
        {
            surface.scale = factor;
            let _ = surface.layer.set_buffer_scale(factor);
            surface.buffers.clear();
            surface.dirty = true;
            return;
        }

        let is_menu = self
            .menu_surface
            .as_ref()
            .filter(|menu| menu.popup.wl_surface() == wayland_surface)
            .is_some();
        if is_menu {
            let menu = self.menu_surface.as_mut().expect("menu remains available");
            menu.scale = factor;
            menu.popup.wl_surface().set_buffer_scale(factor as i32);
            self.update_menu_page();
        }
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        wayland_surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        if let Some(surface) = self
            .surfaces
            .iter_mut()
            .find(|surface| surface.layer.wl_surface() == wayland_surface)
        {
            surface.frame_pending = false;
            return;
        }

        if let Some(menu) = self
            .menu_surface
            .as_mut()
            .filter(|menu| menu.popup.wl_surface() == wayland_surface)
        {
            menu.frame_pending = false;
        }
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
        self.add_output(output);
    }

    fn update_output(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        self.update_output_info(&output);
    }

    fn output_destroyed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        self.menu_surface = None;
        self.surfaces.retain(|surface| surface.output != output);
        self.assign_workspace_groups();
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        if self
            .menu_surface
            .as_ref()
            .is_some_and(|menu| &menu.parent == layer)
        {
            self.menu_surface = None;
        }
        self.surfaces.retain(|surface| &surface.layer != layer);
        self.assign_workspace_groups();
    }

    fn configure(
        &mut self,
        _: &Connection,
        queue_handle: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let Some(surface) = self
            .surfaces
            .iter_mut()
            .find(|surface| &surface.layer == layer)
        else {
            return;
        };

        let width = if configure.new_size.0 > 0 {
            configure.new_size.0
        } else {
            surface.logical_width
        };
        let height = if configure.new_size.1 > 0 {
            configure.new_size.1
        } else {
            self.config.height
        };
        if surface.logical_width != width || surface.logical_height != height {
            surface.buffers.clear();
        }
        surface.logical_width = width;
        surface.logical_height = height;
        surface.configured = surface.logical_width > 0;
        surface.dirty = true;

        if self.background_opaque && surface.configured {
            let region = self
                .compositor
                .wl_compositor()
                .create_region(queue_handle, ());
            region.add(0, 0, width as i32, height as i32);
            surface.layer.set_opaque_region(Some(&region));
            region.destroy();
        }
    }
}

impl PopupHandler for App {
    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        popup: &Popup,
        configure: PopupConfigure,
    ) {
        let Some(menu) = self
            .menu_surface
            .as_mut()
            .filter(|menu| &menu.popup == popup)
        else {
            return;
        };
        let width = u32::try_from(configure.width).unwrap_or(menu.logical_width);
        let height = u32::try_from(configure.height).unwrap_or(menu.logical_height);
        if menu.logical_width != width || menu.logical_height != height {
            menu.buffers.clear();
        }
        menu.prepared.constrain(
            width.saturating_mul(menu.scale),
            height.saturating_mul(menu.scale),
        );
        menu.logical_width = width;
        menu.logical_height = height;
        menu.configured = width > 0 && height > 0;
        menu.dirty = true;
    }

    fn done(&mut self, _: &Connection, _: &QueueHandle<Self>, popup: &Popup) {
        if self
            .menu_surface
            .as_ref()
            .is_some_and(|menu| &menu.popup == popup)
        {
            self.menu_surface = None;
        }
    }
}

impl WindowHandler for App {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {}

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &Window,
        _: WindowConfigure,
        _: u32,
    ) {
    }
}

impl DialogHandler for App {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Dialog) {}

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &Dialog,
        _: WindowConfigure,
        _: u32,
    ) {
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState, SeatState];
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, seat: wl_seat::WlSeat) {
        self.pointers.retain(|(current, pointer)| {
            if current == &seat {
                pointer.release();
                false
            } else {
                true
            }
        });
        self.keyboards.retain(|(current, keyboard)| {
            if current == &seat {
                keyboard.release();
                false
            } else {
                true
            }
        });
    }

    fn new_capability(
        &mut self,
        _: &Connection,
        queue_handle: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer
            && !self.pointers.iter().any(|(current, _)| current == &seat)
        {
            match self.seat_state.get_pointer(queue_handle, &seat) {
                Ok(pointer) => self.pointers.push((seat.clone(), pointer)),
                Err(error) => log::warn!("failed to create Wayland pointer: {error}"),
            }
        }
        if capability == Capability::Keyboard
            && !self.keyboards.iter().any(|(current, _)| current == &seat)
        {
            match self.seat_state.get_keyboard(queue_handle, &seat, None) {
                Ok(keyboard) => self.keyboards.push((seat, keyboard)),
                Err(error) => log::warn!("failed to create Wayland keyboard: {error}"),
            }
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            self.pointers.retain(|(current, pointer)| {
                if current == &seat {
                    pointer.release();
                    false
                } else {
                    true
                }
            });
        }
        if capability == Capability::Keyboard {
            self.keyboards.retain(|(current, keyboard)| {
                if current == &seat {
                    keyboard.release();
                    false
                } else {
                    true
                }
            });
        }
    }
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
        if let Some(menu) = self
            .menu_surface
            .as_mut()
            .filter(|menu| menu.popup.wl_surface() == surface)
        {
            let selected = menu.prepared.next_selection(None, false);
            if menu.selected != selected {
                menu.selected = selected;
                menu.dirty = true;
            }
        }
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        if self
            .menu_surface
            .as_ref()
            .is_some_and(|menu| menu.popup.wl_surface() == surface)
        {
            self.menu_surface = None;
        }
    }

    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.handle_menu_key(event.keysym);
    }

    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        if matches!(event.keysym, Keysym::Up | Keysym::Down) {
            self.handle_menu_key(event.keysym);
        }
    }

    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: Modifiers,
        _: RawModifiers,
        _: u32,
    ) {
    }
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            let is_menu = self
                .menu_surface
                .as_ref()
                .is_some_and(|menu| menu.popup.wl_surface() == &event.surface);
            match event.kind {
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } if is_menu => {
                    if let Some(menu) = self.menu_surface.as_mut() {
                        let x = (event.position.0 * menu.scale as f64).floor() as i32;
                        let y = (event.position.1 * menu.scale as f64).floor() as i32;
                        let selected = menu.prepared.selection_at(x, y);
                        if menu.selected != selected {
                            menu.selected = selected;
                            menu.dirty = true;
                        }
                    }
                }
                PointerEventKind::Leave { .. } if is_menu => {
                    if let Some(menu) = self.menu_surface.as_mut()
                        && menu.selected.take().is_some()
                    {
                        menu.dirty = true;
                    }
                }
                PointerEventKind::Axis { vertical, .. } if is_menu => {
                    if let Some(menu) = self.menu_surface.as_mut() {
                        let logical_delta = if vertical.absolute != 0.0 {
                            vertical.absolute
                        } else if vertical.value120 != 0 {
                            f64::from(vertical.value120) * 32.0 / 120.0
                        } else {
                            f64::from(vertical.discrete) * 32.0
                        };
                        let physical_delta = (logical_delta * f64::from(menu.scale))
                            .round()
                            .clamp(f64::from(i32::MIN), f64::from(i32::MAX))
                            as i32;
                        if menu.prepared.scroll_by(physical_delta) {
                            menu.selected = None;
                            menu.dirty = true;
                        }
                    }
                }
                PointerEventKind::Press { button, serial, .. } => {
                    self.pending_menu_grab = None;
                    if button == BTN_RIGHT
                        && self
                            .surfaces
                            .iter()
                            .any(|bar| bar.layer.wl_surface() == &event.surface)
                    {
                        let bar = self
                            .surfaces
                            .iter()
                            .find(|bar| bar.layer.wl_surface() == &event.surface)
                            .expect("bar surface was checked above");
                        let global_x = bar.output_position.0 + event.position.0.round() as i32;
                        let global_y = bar.output_position.1 + event.position.1.round() as i32;
                        self.pending_menu_grab = self
                            .pointers
                            .iter()
                            .find(|(_, current)| current == pointer)
                            .map(|(seat, _)| (seat.clone(), serial, global_x, global_y));
                    }
                    self.handle_click(&event.surface, button, event.position.0, event.position.1)
                }
                _ => {}
            }
        }
    }
}

delegate_registry!(App);
smithay_client_toolkit::delegate_dispatch2!(App);
delegate_noop!(App: ignore wl_region::WlRegion);

fn clock_granularity(format: &str) -> ClockGranularity {
    StrftimeItems::new(format).fold(ClockGranularity::Minute, |current, item| {
        current.max(match item {
            Item::Numeric(Numeric::Nanosecond, _)
            | Item::Fixed(
                Fixed::Nanosecond | Fixed::Nanosecond3 | Fixed::Nanosecond6 | Fixed::Nanosecond9,
            ) => ClockGranularity::Subsecond,
            Item::Numeric(Numeric::Second | Numeric::Timestamp, _)
            | Item::Fixed(Fixed::RFC2822 | Fixed::RFC3339) => ClockGranularity::Second,
            _ => ClockGranularity::Minute,
        })
    })
}

fn delay_until_next_tick(granularity: ClockGranularity, second: u32, nanosecond: u32) -> Duration {
    let nanosecond = u64::from(nanosecond);
    let delay = match granularity {
        ClockGranularity::Subsecond => Duration::from_millis(16),
        ClockGranularity::Second => {
            Duration::from_nanos(1_000_000_000_u64.saturating_sub(nanosecond))
        }
        ClockGranularity::Minute => {
            let seconds = u64::from(60_u32.saturating_sub(second));
            Duration::from_nanos((seconds * 1_000_000_000).saturating_sub(nanosecond).max(1))
        }
    };
    delay.saturating_add(Duration::from_millis(1))
}

fn preferred_shm_format(formats: &[wl_shm::Format]) -> wl_shm::Format {
    if formats.contains(&wl_shm::Format::Abgr8888) {
        wl_shm::Format::Abgr8888
    } else {
        wl_shm::Format::Argb8888
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_pointer_buttons_have_distinct_actions() {
        assert_eq!(
            menu_input_action(BTN_LEFT, Some(MenuSelection::Item(7)), false, true),
            MenuInputAction::Activate(7)
        );
        assert_eq!(
            menu_input_action(BTN_LEFT, Some(MenuSelection::Item(7)), true, true),
            MenuInputAction::OpenSubmenu(7)
        );
        assert_eq!(
            menu_input_action(BTN_LEFT, Some(MenuSelection::Back), false, true),
            MenuInputAction::Back
        );
        assert_eq!(
            menu_input_action(BTN_RIGHT, Some(MenuSelection::Item(7)), false, true),
            MenuInputAction::Back
        );
        assert_eq!(
            menu_input_action(BTN_RIGHT, None, false, false),
            MenuInputAction::Dismiss
        );
        assert_eq!(
            menu_input_action(0x112, Some(MenuSelection::Item(7)), false, true),
            MenuInputAction::None
        );
    }

    #[test]
    fn menu_keyboard_actions_match_pointer_semantics() {
        assert_eq!(
            menu_input_action(BTN_LEFT, None, false, true),
            MenuInputAction::None
        );
        assert_eq!(
            menu_input_action(BTN_LEFT, Some(MenuSelection::Back), false, false),
            MenuInputAction::Back
        );
    }

    #[test]
    fn menu_dimensions_only_clamp_to_known_output_bounds() {
        assert_eq!(bounded_dimension(320, 1920), 320);
        assert_eq!(bounded_dimension(2200, 1920), 1920);
        assert_eq!(bounded_dimension(320, 0), 320);
        assert_eq!(bounded_dimension(0, 1080), 1);
        assert_eq!(menu_top(28, 400, 1080), 28);
        assert_eq!(menu_top(28, 1070, 1080), 10);
        assert_eq!(menu_top(28, 400, 0), 28);
    }

    fn sample_content(keyboard_layout: &str, task_id: u64) -> crate::niri::SurfaceContent {
        crate::niri::SurfaceContent {
            workspaces: std::array::from_fn(|index| WorkspaceSlot {
                index: index as u8 + 1,
                ..WorkspaceSlot::default()
            }),
            tasks: std::sync::Arc::from([WindowTask {
                id: task_id,
                label: "Application".into(),
                app_id: Some("example".into()),
                is_focused: true,
                is_urgent: false,
                icon: crate::niri::ApplicationIcon {
                    revision: 1,
                    pixels: std::sync::Arc::from([1, 2, 3, 255]),
                    width: 1,
                    height: 1,
                },
            }]),
            keyboard_layout: std::sync::Arc::from(keyboard_layout),
        }
    }

    #[test]
    fn content_changed_marks_initial_and_different_snapshots_only() {
        let first = sample_content("us", 1);
        let same = sample_content("us", 1);
        let changed = sample_content("us", 2);

        assert!(content_changed(None, &first));
        assert!(!content_changed(Some(&first), &same));
        assert!(content_changed(Some(&first), &changed));
    }

    #[test]
    fn content_changed_detects_keyboard_layout_changes() {
        let english = sample_content("us", 1);
        let russian = sample_content("ru", 1);

        assert!(content_changed(Some(&english), &russian));
    }

    #[test]
    fn clock_timer_matches_format_precision() {
        assert_eq!(
            clock_granularity("%a %b %d, %H:%M"),
            ClockGranularity::Minute
        );
        assert_eq!(clock_granularity("%H:%M:%S"), ClockGranularity::Second);
        assert_eq!(
            clock_granularity("%H:%M:%S%.3f"),
            ClockGranularity::Subsecond
        );
        assert_eq!(
            delay_until_next_tick(ClockGranularity::Minute, 42, 500_000_000),
            Duration::from_millis(17_501)
        );
    }

    #[test]
    fn rgba_shm_format_is_preferred_when_available() {
        assert_eq!(
            preferred_shm_format(&[wl_shm::Format::Argb8888]),
            wl_shm::Format::Argb8888
        );
        assert_eq!(
            preferred_shm_format(&[wl_shm::Format::Argb8888, wl_shm::Format::Abgr8888]),
            wl_shm::Format::Abgr8888
        );
    }

    #[test]
    fn workspace_groups_follow_physical_output_order() {
        let groups = workspace_groups([
            ((1920, 0), Some("right".into())),
            ((0, 0), Some("left".into())),
        ]);
        assert_eq!(groups, [1, 0]);
    }
}
