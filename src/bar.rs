use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use calloop_wayland_source::WaylandSource;
use chrono::{
    Local, Timelike,
    format::{Fixed, Item, Numeric, StrftimeItems},
};
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
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
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
    protocol::{wl_output, wl_pointer, wl_region, wl_seat, wl_shm, wl_surface},
};

use crate::{
    config::Config,
    niri::{
        self, NiriEvent, NiriModel, SurfaceContent, WORKSPACES_PER_OUTPUT, WindowTask,
        WorkspaceSlot,
    },
    render::{Hitbox, MenuHitbox, PreparedMenu, RenderContent, Renderer},
    tray::{TrayEvent, TrayHandle, TrayIcon, TrayMenuEntry, TrayMenuPopup},
};

pub fn run(config: Config) -> Result<()> {
    let connection = Connection::connect_to_env().context("не удалось подключиться к Wayland")?;
    let (globals, event_queue) =
        registry_queue_init(&connection).context("не удалось получить Wayland globals")?;
    let queue_handle = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &queue_handle)
        .context("compositor не предоставляет wl_compositor")?;
    let layer_shell = LayerShell::bind(&globals, &queue_handle)
        .context("compositor не предоставляет wlr-layer-shell")?;
    let shm =
        Shm::bind(&globals, &queue_handle).context("compositor не предоставляет shared memory")?;

    let mut event_loop: EventLoop<App> =
        EventLoop::try_new().context("не удалось создать event loop")?;
    WaylandSource::new(connection, event_queue)
        .insert(event_loop.handle())
        .context("не удалось подключить Wayland к event loop")?;

    let (tray_events, tray_channel) = channel::channel();
    let tray = TrayHandle::spawn(tray_events, &config);
    let (niri_events, niri_channel) = channel::channel();
    niri::spawn(niri_events, &config);
    let clock_granularity = clock_granularity(&config.clock_format);
    let renderer = Renderer::new(&config);
    let background_opaque = config.background_rgba()[3] == 255;

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &queue_handle),
        seat_state: SeatState::new(&globals, &queue_handle),
        compositor,
        layer_shell,
        shm,
        queue_handle,
        config,
        renderer,
        background_opaque,
        surfaces: Vec::new(),
        menu_surface: None,
        pointers: Vec::new(),
        clock: String::new(),
        clock_granularity,
        tray_icons: Vec::new(),
        niri_model: Arc::new(NiriModel::default()),
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
        .map_err(|error| anyhow::anyhow!("не удалось подключить systray к event loop: {error}"))?;

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
        .map_err(|error| anyhow::anyhow!("не удалось подключить niri IPC к event loop: {error}"))?;

    event_loop
        .handle()
        .insert_source(Timer::from_duration(app.next_clock_delay()), |_, _, app| {
            app.update_clock();
            TimeoutAction::ToDuration(app.next_clock_delay())
        })
        .map_err(|error| anyhow::anyhow!("не удалось запустить таймер часов: {error}"))?;

    loop {
        event_loop
            .dispatch(None, &mut app)
            .context("ошибка event loop")?;
        app.draw_dirty()?;
    }
}

struct BarSurface {
    layer: LayerSurface,
    pool: SlotPool,
    output: wl_output::WlOutput,
    output_name: Option<String>,
    output_position: (i32, i32),
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
    layer: LayerSurface,
    pool: SlotPool,
    address: String,
    menu_path: String,
    entries: Vec<TrayMenuEntry>,
    prepared: PreparedMenu,
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

struct App {
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    queue_handle: QueueHandle<Self>,
    config: Config,
    renderer: Renderer,
    background_opaque: bool,
    surfaces: Vec<BarSurface>,
    menu_surface: Option<MenuSurface>,
    pointers: Vec<(wl_seat::WlSeat, wl_pointer::WlPointer)>,
    clock: String,
    clock_granularity: ClockGranularity,
    tray_icons: Vec<TrayIcon>,
    niri_model: Arc<NiriModel>,
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
    fn add_output(&mut self, output: wl_output::WlOutput) {
        if self.surfaces.iter().any(|surface| surface.output == output) {
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
        let prepared = self.renderer.prepare_menu(&menu.items, target.scale);
        let (buffer_width, buffer_height) = prepared.size();
        let logical_width = buffer_width.div_ceil(target.scale).max(1);
        let logical_height = buffer_height.div_ceil(target.scale).max(1);
        let left = target
            .local_x
            .max(0)
            .min(target.logical_width.saturating_sub(logical_width) as i32);
        let top = self.config.height as i32;

        self.menu_surface = None;

        let surface = self.compositor.create_surface(&self.queue_handle);
        let layer = self.layer_shell.create_layer_surface(
            &self.queue_handle,
            surface,
            Layer::Overlay,
            Some("nibari-menu"),
            Some(&target.output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::LEFT);
        layer.set_margin(top, 0, 0, left);
        layer.set_size(logical_width, logical_height);
        layer.set_exclusive_zone(0);
        let _ = layer.set_buffer_scale(target.scale);
        layer.commit();

        let pool = match SlotPool::new(1, &self.shm) {
            Ok(pool) => pool,
            Err(error) => {
                log::error!("failed to create menu shared memory pool: {error}");
                return;
            }
        };

        self.menu_surface = Some(MenuSurface {
            layer,
            pool,
            address: menu.address,
            menu_path: menu.menu_path,
            entries: menu.items,
            prepared,
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
                        output: surface.output.clone(),
                        scale: surface.scale,
                        logical_width: surface.logical_width,
                        local_x,
                    })
            })
            .or_else(|| {
                self.surfaces
                    .iter()
                    .filter(|surface| surface.configured)
                    .min_by_key(|surface| (surface.output_position.0 - x).abs())
                    .map(|surface| MenuTarget {
                        output: surface.output.clone(),
                        scale: surface.scale,
                        logical_width: surface.logical_width,
                        local_x: x - surface.output_position.0,
                    })
            })
    }

    fn handle_click(&mut self, surface: &wl_surface::WlSurface, button: u32, x: f64, y: f64) {
        if let Some(menu) = self
            .menu_surface
            .as_ref()
            .filter(|menu| menu.layer.wl_surface() == surface)
        {
            let physical_x = (x * menu.scale as f64).floor() as i32;
            let physical_y = (y * menu.scale as f64).floor() as i32;
            let command = menu
                .hitboxes
                .iter()
                .find(|hitbox| {
                    hitbox.enabled
                        && physical_x >= hitbox.x
                        && physical_x < hitbox.x + hitbox.width
                        && physical_y >= hitbox.y
                        && physical_y < hitbox.y + hitbox.height
                })
                .map(|hitbox| (menu.address.clone(), menu.menu_path.clone(), hitbox.item_id));
            self.menu_surface = None;
            if let Some((address, menu_path, item_id)) = command {
                self.tray.menu_item(address, menu_path, item_id);
            }
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

        let physical_x = (x * bar.scale as f64).floor() as i32;
        let physical_y = (y * bar.scale as f64).floor() as i32;
        let Some(hitbox) = bar.hitboxes.iter().find(|hitbox| {
            physical_x >= hitbox.x
                && physical_x < hitbox.x + hitbox.width
                && physical_y >= hitbox.y
                && physical_y < hitbox.y + hitbox.height
        }) else {
            self.menu_surface = None;
            return;
        };
        let Some(icon) = self.tray_icons.get(hitbox.tray_index) else {
            self.menu_surface = None;
            return;
        };

        let global_x = bar.output_position.0 + x.round() as i32;
        let global_y = bar.output_position.1 + y.round() as i32;
        self.menu_surface = None;
        self.tray.click(icon.id.clone(), button, global_x, global_y);
    }
}

struct MenuTarget {
    output: wl_output::WlOutput,
    scale: u32,
    logical_width: u32,
    local_x: i32,
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
        .context("слишком большая ширина output")?;
    let height = surface
        .logical_height
        .checked_mul(surface.scale)
        .context("слишком большая высота bar")?;
    if width == 0 || height == 0 {
        return Ok(());
    }

    let stride = width.checked_mul(4).context("переполнение stride")?;
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
            .context("не удалось повторно прикрепить Wayland buffer")?;
    } else {
        let (buffer, canvas) = surface
            .pool
            .create_buffer(width as i32, height as i32, stride as i32, surface.format)
            .context("не удалось создать Wayland buffer")?;
        render_canvas(canvas, renderer, &mut surface.hitboxes, frame)?;
        buffer
            .attach_to(wayland_surface)
            .context("не удалось прикрепить Wayland buffer")?;
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
        .context("слишком большая ширина tray menu")?;
    let height = surface
        .logical_height
        .checked_mul(surface.scale)
        .context("слишком большая высота tray menu")?;
    if width == 0 || height == 0 {
        return Ok(());
    }

    let stride = width.checked_mul(4).context("переполнение menu stride")?;
    let wayland_surface = surface.layer.wl_surface();
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
                frame,
            )?;
            reusable = Some(index);
            break;
        }
    }

    if let Some(index) = reusable {
        surface.buffers[index]
            .attach_to(wayland_surface)
            .context("не удалось повторно прикрепить Wayland menu buffer")?;
    } else {
        let (buffer, canvas) = surface
            .pool
            .create_buffer(width as i32, height as i32, stride as i32, surface.format)
            .context("не удалось создать Wayland menu buffer")?;
        render_menu_canvas(
            canvas,
            renderer,
            &mut surface.hitboxes,
            &surface.prepared,
            frame,
        )?;
        buffer
            .attach_to(wayland_surface)
            .context("не удалось прикрепить Wayland menu buffer")?;
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
    let mut pixmap = PixmapMut::from_bytes(canvas, frame.width, frame.height)
        .context("некорректный размер pixmap")?;
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
    frame: MenuFrame,
) -> Result<()> {
    let mut pixmap = PixmapMut::from_bytes(canvas, frame.width, frame.height)
        .context("некорректный размер menu pixmap")?;
    renderer.draw_menu(&mut pixmap, menu, hitboxes);
    convert_canvas_for_wayland(&mut pixmap, frame.format);
    Ok(())
}

fn convert_canvas_for_wayland(pixmap: &mut PixmapMut<'_>, format: wl_shm::Format) {
    if format == wl_shm::Format::Argb8888 {
        // tiny-skia: RGBA; wl_shm ARGB8888 на little-endian: BGRA.
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

        let entries = self
            .menu_surface
            .as_ref()
            .filter(|menu| menu.layer.wl_surface() == wayland_surface)
            .map(|menu| menu.entries.clone());
        if let Some(entries) = entries {
            let prepared = self.renderer.prepare_menu(&entries, factor);
            let menu = self
                .menu_surface
                .as_mut()
                .expect("menu remains available while rebuilding scale");
            menu.scale = factor;
            let _ = menu.layer.set_buffer_scale(factor);
            menu.prepared = prepared;
            menu.buffers.clear();
            menu.dirty = true;
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
            .filter(|menu| menu.layer.wl_surface() == wayland_surface)
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
        self.surfaces.retain(|surface| surface.output != output);
        self.menu_surface = None;
        self.assign_workspace_groups();
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        if self
            .menu_surface
            .as_ref()
            .is_some_and(|menu| &menu.layer == layer)
        {
            self.menu_surface = None;
            return;
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
        if let Some(menu) = self
            .menu_surface
            .as_mut()
            .filter(|menu| &menu.layer == layer)
        {
            let width = if configure.new_size.0 > 0 {
                configure.new_size.0
            } else {
                menu.logical_width
            };
            let height = if configure.new_size.1 > 0 {
                configure.new_size.1
            } else {
                menu.logical_height
            };
            if menu.logical_width != width || menu.logical_height != height {
                menu.buffers.clear();
            }
            menu.logical_width = width;
            menu.logical_height = height;
            menu.configured = width > 0 && height > 0;
            menu.dirty = true;
            return;
        }

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

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState];
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
                Ok(pointer) => self.pointers.push((seat, pointer)),
                Err(error) => log::warn!("failed to create Wayland pointer: {error}"),
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
    }
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        events
            .iter()
            .filter_map(|event| match event.kind {
                PointerEventKind::Press { button, .. } => Some((event, button)),
                _ => None,
            })
            .for_each(|(event, button)| {
                self.handle_click(&event.surface, button, event.position.0, event.position.1);
            });
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
