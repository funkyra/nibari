use std::{collections::HashMap, path::PathBuf, process::Command};

use cosmic_text::{
    Align, Attrs, Buffer, Color as TextColor, Family, FontSystem, Metrics, Shaping, SwashCache,
};
use tiny_skia::{Color, Paint, PathBuilder, Pixmap, PixmapMut, Rect, Stroke, Transform};

pub mod calendar;
pub mod network;
mod popup;
pub mod weather;
pub use popup::PreparedPopup;
mod menu;
pub use menu::{MenuHitbox, MenuSelection, PreparedMenu};

use crate::{
    config::{ClockPosition, Config},
    media::MediaSnapshot,
    network::NetworkKind,
    niri::{WORKSPACE_LABELS, WORKSPACES_PER_OUTPUT, WindowTask, WorkspaceSlot},
    tray::TrayIcon,
    weather::Condition,
};

fn font_path(output: &str) -> Option<PathBuf> {
    output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(PathBuf::from)
}

fn distinct_font_paths(paths: [PathBuf; 2]) -> Vec<PathBuf> {
    paths.into_iter().fold(Vec::new(), |mut unique, path| {
        if !unique.contains(&path) {
            unique.push(path);
        }
        unique
    })
}

fn selected_font_paths([primary, fallback]: [Option<PathBuf>; 2]) -> Option<Vec<PathBuf>> {
    Some(distinct_font_paths([primary?, fallback?]))
}

fn fontconfig_font_path(pattern: &str) -> Option<PathBuf> {
    let output = match Command::new("fc-match")
        .args(["-f", "%{file}\n", pattern])
        .output()
    {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            log::warn!("fc-match failed for {pattern:?}: {}", output.status);
            return None;
        }
        Err(error) => {
            log::warn!("failed to start fc-match for {pattern:?}: {error}");
            return None;
        }
    };
    let output = match std::str::from_utf8(&output.stdout) {
        Ok(output) => output,
        Err(error) => {
            log::warn!("fc-match returned invalid UTF-8 for {pattern:?}: {error}");
            return None;
        }
    };
    let path = font_path(output);
    if path.is_none() {
        log::warn!("fc-match did not return a font path for {pattern:?}");
    }
    path
}

fn font_system(font_family: &str) -> FontSystem {
    let Some(paths) = selected_font_paths([
        fontconfig_font_path(font_family),
        fontconfig_font_path("sans-serif:lang=ja"),
    ]) else {
        log::warn!("using the complete system font database");
        return FontSystem::new();
    };
    let mut database = cosmic_text::fontdb::Database::new();
    if paths
        .into_iter()
        .all(|path| match database.load_font_file(&path) {
            Ok(()) => true,
            Err(error) => {
                log::warn!("failed to load font {}: {error}", path.display());
                false
            }
        })
    {
        FontSystem::new_with_locale_and_db("en-US".into(), database)
    } else {
        log::warn!("using the complete system font database");
        FontSystem::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitTarget {
    Workspace { id: Option<u64>, index: u8 },
    Window(u64),
    Tray(usize),
    TrayDrawer,
    Clock,
    Network,
    Weather,
}

#[derive(Clone, Copy, Debug)]
pub struct Hitbox {
    pub target: HitTarget,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

pub fn hit_target_at(hitboxes: &[Hitbox], scale: u32, x: f64, y: f64) -> Option<HitTarget> {
    let x = (x * f64::from(scale.max(1))).floor() as i32;
    let y = (y * f64::from(scale.max(1))).floor() as i32;
    hitboxes
        .iter()
        .rev()
        .find(|hitbox| {
            x >= hitbox.x
                && x < hitbox.x + hitbox.width
                && y >= hitbox.y
                && y < hitbox.y + hitbox.height
        })
        .map(|hitbox| hitbox.target)
}

#[derive(Clone, Copy)]
pub struct RenderContent<'a> {
    pub scale: u32,
    pub clock: &'a str,
    pub keyboard_layout: &'a str,
    pub tray: &'a [TrayIcon],
    pub tray_reveal: f32,
    pub workspaces: &'a [WorkspaceSlot; WORKSPACES_PER_OUTPUT],
    pub tasks: &'a [WindowTask],
    pub media: Option<&'a MediaSnapshot>,
}

struct Style {
    font_family: Box<str>,
    font_size: f32,
    background: Color,
    foreground: [u8; 4],
    padding: u32,
    tray_icon_size: u32,
    tray_spacing: u32,
    tray_drawer: bool,
    clock_position: ClockPosition,
    calendar_enabled: bool,
    weather_enabled: bool,
    weather_foreground: [u8; 4],
    network_enabled: bool,
    network_foreground: [u8; 4],
    network_muted: [u8; 4],
    workspace_width: u32,
    workspace_focused_background: Color,
    workspace_active_background: Color,
    workspace_active_foreground: [u8; 4],
    workspace_occupied_foreground: [u8; 4],
    workspace_empty_foreground: [u8; 4],
    workspace_urgent_foreground: [u8; 4],
    media_enabled: bool,
    task_icon_size: u32,
    task_spacing: u32,
    task_padding: u32,
    task_background: Color,
    task_focused_background: Color,
    task_urgent_background: Color,
    task_foreground: [u8; 4],
    task_focused_foreground: [u8; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BarLayout {
    clock_x: i32,
    clock_y: i32,
    keyboard_x: i32,
    network_x: Option<i32>,
    weather_x: Option<i32>,
    keyboard_y: i32,
    keyboard_width: i32,
    tray_left: i32,
    icon_y: i32,
    icon_size: i32,
    spacing: i32,
}

impl BarLayout {
    fn calculate(
        canvas: (i32, i32),
        clock: (i32, i32),
        keyboard: (i32, i32),
        tray_len: usize,
        scale: u32,
        style: &Style,
    ) -> Self {
        let scale = scale.max(1) as i32;
        let padding = style.padding as i32 * scale;
        let spacing = style.tray_spacing as i32 * scale;
        let icon_size = style.tray_icon_size as i32 * scale;
        let centered = style.clock_position == ClockPosition::Center;
        let weather_width = if style.weather_enabled {
            spacing + icon_size
        } else {
            0
        };
        let clock_x = if centered {
            (canvas.0 - clock.0) / 2
        } else {
            (canvas.0 - padding - clock.0 - weather_width).max(0)
        };
        let right_anchor = if centered {
            canvas.0 - padding
        } else {
            clock_x
        };
        let tray_width = icon_size * tray_len as i32 + spacing * tray_len.saturating_sub(1) as i32;
        let mut tray_left = right_anchor - if centered { 0 } else { spacing } - tray_width;
        let keyboard_width = keyboard.0;
        let keyboard_anchor = if tray_len == 0 {
            right_anchor
        } else {
            tray_left
        };
        let mut keyboard_x = if keyboard_width == 0 {
            keyboard_anchor
        } else {
            keyboard_anchor - spacing - keyboard_width
        };

        let network_x = style.network_enabled.then(|| {
            let x = right_anchor - if centered { 0 } else { spacing } - icon_size;
            keyboard_x = x - if keyboard_width > 0 {
                spacing + keyboard_width
            } else {
                0
            };
            tray_left = keyboard_x
                - if tray_len > 0 {
                    spacing + tray_width
                } else {
                    0
                };
            x
        });
        Self {
            weather_x: style.weather_enabled.then_some(clock_x + clock.0 + spacing),
            network_x,
            clock_x,
            clock_y: (canvas.1 - clock.1) / 2,
            keyboard_x,
            keyboard_y: (canvas.1 - keyboard.1) / 2,
            keyboard_width,
            tray_left,
            icon_y: (canvas.1 - icon_size) / 2,
            icon_size,
            spacing,
        }
    }

    fn icon_x(self, index: usize) -> i32 {
        self.tray_left + index as i32 * (self.icon_size + self.spacing)
    }

    fn hitbox(self, tray_index: usize) -> Hitbox {
        Hitbox {
            target: HitTarget::Tray(tray_index),
            x: self.icon_x(tray_index),
            y: self.icon_y,
            width: self.icon_size,
            height: self.icon_size,
        }
    }
}

struct ClockBitmap {
    text: String,
    pixmap: Pixmap,
}

struct TextBitmap {
    text: String,
    scale: u32,
    color: [u8; 4],
    reserved_width: u32,
    pixmap: Pixmap,
}

struct IconBitmap {
    id: String,
    scale: u32,
    revision: u64,
    pixels: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WorkspaceBitmapKey {
    index: u8,
    scale: u32,
    color: [u8; 4],
}

struct WorkspaceBitmap {
    key: WorkspaceBitmapKey,
    pixmap: Pixmap,
}

struct TaskTextBitmap {
    id: u64,
    label: String,
    scale: u32,
    color: [u8; 4],
    natural_width: u32,
    width_limit: Option<u32>,
    pixmap: Pixmap,
}

struct MediaBitmap {
    scale: u32,
    title: String,
    title_pixmap: Pixmap,
    detail: String,
    detail_pixmap: Pixmap,
    separator_pixmap: Pixmap,
}

struct TaskIconBitmap {
    id: u64,
    scale: u32,
    revision: u64,
    pixels: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PixelRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TaskLayout {
    left: i32,
    height: i32,
    item_width: i32,
    remainder: i32,
    spacing: i32,
    count: i32,
}

impl TaskLayout {
    fn calculate(left: i32, right: i32, height: i32, count: usize, spacing: i32) -> Option<Self> {
        let count = i32::try_from(count).ok()?;
        if count == 0 || right <= left {
            return None;
        }
        let spacing = spacing.max(0);
        let gaps = spacing.saturating_mul(count.saturating_sub(1));
        let content_width = (right - left - gaps).max(0);
        if content_width < count {
            return None;
        }

        Some(Self {
            left,
            height,
            item_width: content_width / count,
            remainder: content_width % count,
            spacing,
            count,
        })
    }

    fn item(self, index: usize) -> Option<PixelRect> {
        let index = i32::try_from(index).ok()?;
        if index >= self.count {
            return None;
        }
        let extra_before = index.min(self.remainder);
        Some(PixelRect {
            x: self.left + index * (self.item_width + self.spacing) + extra_before,
            y: 0,
            width: self.item_width + i32::from(index < self.remainder),
            height: self.height,
        })
    }
}

pub struct Renderer {
    font_system: FontSystem,
    swash_cache: SwashCache,
    style: Style,
    clock_cache: HashMap<u32, ClockBitmap>,
    keyboard_cache: Option<TextBitmap>,
    weather_condition: (Condition, bool),
    weather_icon_cache: Vec<(u32, Pixmap)>,
    network_kind: NetworkKind,
    network_icon_cache: Vec<(u32, Pixmap)>,
    icon_cache: Vec<IconBitmap>,
    workspace_cache: Vec<WorkspaceBitmap>,
    task_text_cache: Vec<TaskTextBitmap>,
    task_icon_cache: Vec<TaskIconBitmap>,
    media_cache: Vec<MediaBitmap>,
}

impl Renderer {
    pub fn new(config: &Config) -> Self {
        let background = config.background_rgba();
        Self {
            font_system: font_system(&config.font_family),
            swash_cache: SwashCache::new(),
            style: Style {
                font_family: config.font_family.clone().into_boxed_str(),
                font_size: config.font_size,
                background: Color::from_rgba8(
                    background[0],
                    background[1],
                    background[2],
                    background[3],
                ),
                foreground: config.foreground_rgba(),
                padding: config.padding,
                tray_icon_size: config.tray_icon_size,
                tray_spacing: config.tray_spacing,
                tray_drawer: config.tray_drawer,
                clock_position: config.clock_position,
                calendar_enabled: config.calendar_enabled,
                weather_enabled: config.weather_enabled,
                weather_foreground: config.color_rgba(&config.weather_foreground),
                network_enabled: config.network_enabled,
                network_foreground: config.color_rgba(&config.network_foreground),
                network_muted: config.color_rgba(&config.network_muted),
                workspace_width: config.workspace_width,
                workspace_focused_background: rgba(
                    config.color_rgba(&config.workspace_focused_background),
                ),
                workspace_active_background: rgba(
                    config.color_rgba(&config.workspace_active_background),
                ),
                workspace_active_foreground: config.color_rgba(&config.workspace_active_foreground),
                workspace_occupied_foreground: config
                    .color_rgba(&config.workspace_occupied_foreground),
                workspace_empty_foreground: config.color_rgba(&config.workspace_empty_foreground),
                workspace_urgent_foreground: config.color_rgba(&config.workspace_urgent_foreground),
                media_enabled: config.media_enabled,
                task_icon_size: config.task_icon_size,
                task_spacing: config.task_spacing,
                task_padding: config.task_padding,
                task_background: rgba(config.color_rgba(&config.task_background)),
                task_focused_background: rgba(config.color_rgba(&config.task_focused_background)),
                task_urgent_background: rgba(config.color_rgba(&config.task_urgent_background)),
                task_foreground: config.color_rgba(&config.task_foreground),
                task_focused_foreground: config.color_rgba(&config.task_focused_foreground),
            },
            clock_cache: HashMap::new(),
            keyboard_cache: None,
            weather_condition: (Condition::Unknown, false),
            weather_icon_cache: Vec::new(),
            network_kind: NetworkKind::Offline,
            network_icon_cache: Vec::new(),
            icon_cache: Vec::new(),
            workspace_cache: Vec::new(),
            task_text_cache: Vec::new(),
            task_icon_cache: Vec::new(),
            media_cache: Vec::new(),
        }
    }

    pub fn evict_window(&mut self, window_id: u64) {
        self.task_text_cache.retain(|entry| entry.id != window_id);
        self.task_icon_cache.retain(|entry| entry.id != window_id);
    }

    pub fn retain_tray_icons(&mut self, tray: &[TrayIcon]) {
        self.icon_cache
            .retain(|cached| tray.iter().any(|icon| icon.id == cached.id));
    }

    pub fn set_weather_condition(&mut self, condition: Condition, night: bool) -> bool {
        if self.weather_condition == (condition, night) {
            return false;
        }
        self.weather_condition = (condition, night);
        self.weather_icon_cache.clear();
        true
    }

    pub fn set_network_kind(&mut self, kind: NetworkKind) -> bool {
        if self.network_kind == kind {
            return false;
        }
        self.network_kind = kind;
        self.network_icon_cache.clear();
        true
    }

    pub fn draw(
        &mut self,
        pixmap: &mut PixmapMut<'_>,
        content: RenderContent<'_>,
        hitboxes: &mut Vec<Hitbox>,
    ) {
        let RenderContent {
            scale,
            clock,
            keyboard_layout,
            tray,
            tray_reveal,
            workspaces,
            tasks,
            media,
        } = content;
        let scale = scale.max(1);
        self.cache_clock(clock, scale);
        self.cache_keyboard_layout(keyboard_layout, scale);
        let clock_size = {
            let cached = &self.clock_cache[&scale].pixmap;
            (cached.width() as i32, cached.height() as i32)
        };
        let keyboard_size = self.keyboard_cache.as_ref().map_or((0, 0), |cached| {
            (cached.reserved_width as i32, cached.pixmap.height() as i32)
        });
        let mut layout = BarLayout::calculate(
            (pixmap.width() as i32, pixmap.height() as i32),
            clock_size,
            keyboard_size,
            tray.len(),
            scale,
            &self.style,
        );

        let centered = self.style.clock_position == ClockPosition::Center;
        let right_anchor = if centered {
            pixmap.width() as i32 - self.style.padding as i32 * scale as i32
        } else {
            layout.clock_x
        };
        let clock_right = layout.clock_x + clock_size.0;
        let right_min = if centered {
            let media_gap = layout
                .weather_x
                .map_or(layout.spacing, |_| layout.spacing * 2);
            (layout
                .weather_x
                .map_or(clock_right, |x| x + layout.icon_size)
                + media_gap)
                .max(0)
        } else {
            0
        };

        // Keep the drawer at the right edge when the clock is centered.
        let drawer_handle = (self.style.tray_drawer && !tray.is_empty()).then(|| {
            if layout.network_x.is_none() {
                layout.keyboard_x = if layout.keyboard_width > 0 {
                    right_anchor - layout.spacing - layout.keyboard_width
                } else {
                    right_anchor
                };
            }
            let handle = layout.keyboard_x - layout.spacing - layout.icon_size;
            let full_width =
                layout.icon_size * tray.len() as i32 + layout.spacing * tray.len() as i32;
            let visible_width = (full_width as f32 * tray_reveal.clamp(0.0, 1.0)).round() as i32;
            let left = handle - visible_width;
            layout.tray_left = left;
            handle
        });

        let background = self.style.background;
        pixmap.fill(background);
        hitboxes.clear();
        let workspace_right = if centered {
            (layout.clock_x - layout.spacing).max(0)
        } else {
            pixmap.width() as i32
        };
        let mut right_content_left = right_anchor;
        if !tray.is_empty() {
            right_content_left = right_content_left.min(layout.tray_left);
        }
        if layout.keyboard_width > 0 {
            right_content_left = right_content_left.min(layout.keyboard_x);
        }
        if let Some(x) = layout.network_x {
            right_content_left = right_content_left.min(x);
        }
        let workspace_right = if centered {
            workspace_right
        } else {
            (right_content_left - layout.spacing).max(0)
        };
        self.draw_workspaces(pixmap, scale, workspaces, hitboxes, workspace_right);
        self.draw_tasks(pixmap, scale, tasks, layout, workspaces.len(), hitboxes);
        let media_left = if centered {
            right_min
        } else {
            self.style.workspace_width as i32 * workspaces.len() as i32 * scale as i32
                + layout.spacing
        };
        self.draw_media(
            pixmap,
            scale,
            media,
            media_left,
            right_content_left - layout.spacing,
        );
        let cached_clock = &self.clock_cache[&scale].pixmap;
        draw_premultiplied(
            pixmap,
            layout.clock_x,
            layout.clock_y,
            cached_clock.width(),
            cached_clock.height(),
            cached_clock.data(),
        );
        if self.style.calendar_enabled && clock_size.0 > 0 {
            hitboxes.push(Hitbox {
                target: HitTarget::Clock,
                x: layout.clock_x.max(0),
                y: 0,
                width: (clock_right.min(pixmap.width() as i32) - layout.clock_x.max(0)).max(0),
                height: pixmap.height() as i32,
            });
        }
        if let Some(x) = layout.weather_x {
            let visible_left = x.max(0);
            let visible_right = (x + layout.icon_size).min(pixmap.width() as i32);
            if visible_right > visible_left {
                let size = layout.icon_size as u32;
                let index = self
                    .weather_icon_cache
                    .iter()
                    .position(|(cached_size, _)| *cached_size == size)
                    .unwrap_or_else(|| {
                        self.weather_icon_cache.push((
                            size,
                            weather::weather_icon(
                                self.weather_condition.0,
                                self.weather_condition.1,
                                size,
                                self.style.weather_foreground,
                            ),
                        ));
                        self.weather_icon_cache.len() - 1
                    });
                let icon = &self.weather_icon_cache[index].1;
                draw_premultiplied_clipped(
                    pixmap,
                    x,
                    layout.icon_y,
                    size,
                    size,
                    icon.data(),
                    PixelRect {
                        x: visible_left,
                        y: 0,
                        width: visible_right - visible_left,
                        height: pixmap.height() as i32,
                    },
                );
                hitboxes.push(Hitbox {
                    target: HitTarget::Weather,
                    x: visible_left,
                    y: 0,
                    width: visible_right - visible_left,
                    height: pixmap.height() as i32,
                });
            }
        }
        if let Some(cached_keyboard) = &self.keyboard_cache {
            draw_premultiplied_clipped(
                pixmap,
                layout.keyboard_x + layout.keyboard_width - cached_keyboard.pixmap.width() as i32,
                layout.keyboard_y,
                cached_keyboard.pixmap.width(),
                cached_keyboard.pixmap.height(),
                cached_keyboard.pixmap.data(),
                PixelRect {
                    x: right_min,
                    y: 0,
                    width: (pixmap.width() as i32 - right_min).max(0),
                    height: pixmap.height() as i32,
                },
            );
        }
        if let Some(x) = layout.network_x {
            let visible_left = x.max(right_min);
            let visible_right = (x + layout.icon_size).min(pixmap.width() as i32);
            if visible_right > visible_left {
                let size = layout.icon_size as u32;
                let index = self
                    .network_icon_cache
                    .iter()
                    .position(|(cached_size, _)| *cached_size == size)
                    .unwrap_or_else(|| {
                        let color = if self.network_kind == NetworkKind::Offline {
                            self.style.network_muted
                        } else {
                            self.style.network_foreground
                        };
                        self.network_icon_cache
                            .push((size, network::network_icon(self.network_kind, size, color)));
                        self.network_icon_cache.len() - 1
                    });
                let icon = &self.network_icon_cache[index].1;
                draw_premultiplied_clipped(
                    pixmap,
                    x,
                    layout.icon_y,
                    size,
                    size,
                    icon.data(),
                    PixelRect {
                        x: visible_left,
                        y: 0,
                        width: visible_right - visible_left,
                        height: pixmap.height() as i32,
                    },
                );
                hitboxes.push(Hitbox {
                    target: HitTarget::Network,
                    x: visible_left,
                    y: 0,
                    width: visible_right - visible_left,
                    height: pixmap.height() as i32,
                });
            }
        }
        if tray.is_empty() {
            self.icon_cache.clear();
            return;
        }

        self.retain_tray_icons(tray);
        hitboxes.reserve(tray.len() + usize::from(drawer_handle.is_some()));
        let clip_right = if let Some(handle) = drawer_handle {
            // One continuous hover region, including icon gaps and bar padding.
            hitboxes.push(Hitbox {
                target: HitTarget::TrayDrawer,
                x: layout.tray_left.max(right_min),
                y: 0,
                width: (handle + layout.icon_size - layout.tray_left.max(right_min)).max(0),
                height: pixmap.height() as i32,
            });
            if handle >= right_min {
                self.draw_drawer_handle(
                    pixmap,
                    handle,
                    layout.icon_y,
                    layout.icon_size,
                    tray_reveal,
                );
            }
            handle - layout.spacing
        } else {
            pixmap.width() as i32
        };
        let clip = PixelRect {
            x: layout.tray_left.max(right_min),
            y: 0,
            width: (clip_right - layout.tray_left.max(right_min)).max(0),
            height: pixmap.height() as i32,
        };

        for (index, icon) in tray.iter().enumerate() {
            let mut hitbox = layout.hitbox(index);
            let visible_left = hitbox.x.max(clip.x);
            let visible_right = (hitbox.x + hitbox.width).min(clip_right);
            if visible_right <= visible_left {
                continue;
            }
            let cached_index = self.cached_icon(icon, scale, layout.icon_size as u32);
            draw_premultiplied_clipped(
                pixmap,
                hitbox.x,
                hitbox.y,
                hitbox.width as u32,
                hitbox.height as u32,
                &self.icon_cache[cached_index].pixels,
                clip,
            );
            hitbox.x = visible_left;
            hitbox.width = visible_right - visible_left;
            hitboxes.push(hitbox);
        }
    }

    fn draw_drawer_handle(
        &self,
        pixmap: &mut PixmapMut<'_>,
        x: i32,
        y: i32,
        size: i32,
        progress: f32,
    ) {
        // A geometric chevron works with any configured font, including without Nerd Fonts.
        let center_x = x as f32 + size as f32 / 2.0;
        let center_y = y as f32 + size as f32 / 2.0;
        let radius = size as f32 * 0.22;
        let direction = 1.0 - 2.0 * progress.clamp(0.0, 1.0);
        let mut path = PathBuilder::new();
        path.move_to(center_x + radius * direction / 2.0, center_y - radius);
        path.line_to(center_x - radius * direction / 2.0, center_y);
        path.line_to(center_x + radius * direction / 2.0, center_y + radius);
        let mut paint = Paint::default();
        let [r, g, b, a] = self.style.foreground;
        paint.set_color_rgba8(r, g, b, a);
        pixmap.stroke_path(
            &path.finish().expect("chevron has three points"),
            &paint,
            &Stroke {
                width: (size as f32 / 10.0).max(1.0),
                ..Stroke::default()
            },
            Transform::identity(),
            None,
        );
    }

    fn cache_keyboard_layout(&mut self, text: &str, scale: u32) {
        let text = text.trim();
        if text.is_empty() {
            self.keyboard_cache = None;
            return;
        }

        let color = self.style.foreground;
        let stale = self.keyboard_cache.as_ref().is_none_or(|cached| {
            cached.text != text || cached.scale != scale || cached.color != color
        });
        if stale {
            let pixmap = self.rasterize_text(text, scale, color);
            let reserved_width = ["RU", "EN"]
                .into_iter()
                .map(|label| self.rasterize_text(label, scale, color).width())
                .chain(std::iter::once(pixmap.width()))
                .max()
                .unwrap_or(0);
            self.keyboard_cache = Some(TextBitmap {
                text: text.to_owned(),
                scale,
                color,
                reserved_width,
                pixmap,
            });
        }
    }

    fn cache_clock(&mut self, text: &str, scale: u32) {
        let stale = self
            .clock_cache
            .get(&scale)
            .is_none_or(|cached| cached.text != text);
        if stale {
            let bitmap = self.rasterize_clock(text, scale);
            self.clock_cache.insert(scale, bitmap);
        }
    }

    fn rasterize_clock(&mut self, text: &str, scale: u32) -> ClockBitmap {
        ClockBitmap {
            text: text.to_owned(),
            pixmap: self.rasterize_text(text, scale, self.style.foreground),
        }
    }

    fn rasterize_text(&mut self, text: &str, scale: u32, color: [u8; 4]) -> Pixmap {
        self.rasterize_text_with_size(text, scale, color, self.style.font_size)
    }

    fn rasterize_text_with_size(
        &mut self,
        text: &str,
        scale: u32,
        color: [u8; 4],
        font_size: f32,
    ) -> Pixmap {
        let font_size = font_size * scale as f32;
        let line_height = (font_size * 1.25).ceil();
        let metrics = Metrics::new(font_size, line_height);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_size(Some(10_000.0), Some(line_height));
        buffer.set_text(
            text,
            &Attrs::new().family(Family::Name(&self.style.font_family)),
            Shaping::Advanced,
            Some(Align::Left),
        );
        buffer.shape_until_scroll(&mut self.font_system, false);

        let width = buffer
            .layout_runs()
            .fold(0.0_f32, |width, run| width.max(run.line_w))
            .ceil()
            .max(1.0) as u32;
        let height = line_height.max(1.0) as u32;
        let mut pixmap = Pixmap::new(width, height).expect("validated text bitmap size");
        let text_color = TextColor::rgba(color[0], color[1], color[2], color[3]);
        blend_text(
            &mut pixmap.as_mut(),
            &mut buffer,
            &mut self.font_system,
            &mut self.swash_cache,
            text_color,
        );

        pixmap
    }

    fn rasterize_ellipsized_text(
        &mut self,
        text: &str,
        scale: u32,
        color: [u8; 4],
        max_width: u32,
    ) -> Pixmap {
        let ellipsis = self.rasterize_text("…", scale, color);
        if ellipsis.width() >= max_width {
            let mut clipped = Pixmap::new(max_width.max(1), ellipsis.height())
                .expect("validated ellipsis bitmap size");
            draw_premultiplied_clipped(
                &mut clipped.as_mut(),
                0,
                0,
                ellipsis.width(),
                ellipsis.height(),
                ellipsis.data(),
                PixelRect {
                    x: 0,
                    y: 0,
                    width: max_width as i32,
                    height: ellipsis.height() as i32,
                },
            );
            return clipped;
        }

        let character_ends = text
            .char_indices()
            .map(|(index, character)| index + character.len_utf8())
            .collect::<Vec<_>>();
        let mut lower = 0;
        let mut upper = character_ends.len();
        let mut best = ellipsis;
        while lower < upper {
            let count = (lower + upper).div_ceil(2);
            let mut candidate = String::with_capacity(character_ends[count - 1] + '…'.len_utf8());
            candidate.push_str(&text[..character_ends[count - 1]]);
            candidate.push('…');
            let pixmap = self.rasterize_text(&candidate, scale, color);
            if pixmap.width() <= max_width {
                lower = count;
                best = pixmap;
            } else {
                upper = count - 1;
            }
        }
        best
    }

    fn draw_workspaces(
        &mut self,
        pixmap: &mut PixmapMut<'_>,
        scale: u32,
        workspaces: &[WorkspaceSlot],
        hitboxes: &mut Vec<Hitbox>,
        right_limit: i32,
    ) {
        let width = (self.style.workspace_width * scale) as i32;
        let height = pixmap.height() as i32;

        for (offset, workspace) in workspaces.iter().enumerate() {
            let x = offset as i32 * width;
            let visible_width = width.min(right_limit - x).min(pixmap.width() as i32 - x);
            if visible_width <= 0 {
                break;
            }
            hitboxes.push(Hitbox {
                target: HitTarget::Workspace {
                    id: workspace.id,
                    index: offset as u8 + 1,
                },
                x,
                y: 0,
                width: visible_width,
                height,
            });
            if let Some(background) = workspace_background(&self.style, *workspace) {
                fill_rect(pixmap, x, 0, visible_width, height, background);
            }

            let color = workspace_foreground(&self.style, *workspace);
            // Workspace indices start at 1; 0 shares one cached dot across active slots.
            let label_index = if workspace.is_focused || workspace.is_active {
                0
            } else {
                workspace.index
            };
            let cache_index = self.cached_workspace_label(label_index, scale, color);
            let label = &self.workspace_cache[cache_index].pixmap;
            draw_premultiplied_clipped(
                pixmap,
                x + (width - label.width() as i32) / 2,
                (height - label.height() as i32) / 2,
                label.width(),
                label.height(),
                label.data(),
                PixelRect {
                    x,
                    y: 0,
                    width: visible_width,
                    height,
                },
            );

            if workspace.is_occupied {
                let indicator_width = (width / 3).max(scale as i32 * 4);
                fill_rect(
                    pixmap,
                    x + (width - indicator_width) / 2,
                    height - (2 * scale) as i32,
                    indicator_width.min(right_limit - x - (width - indicator_width) / 2),
                    (2 * scale) as i32,
                    rgba(color),
                );
            }
        }
    }

    fn cached_workspace_label(&mut self, index: u8, scale: u32, color: [u8; 4]) -> usize {
        let key = WorkspaceBitmapKey {
            index,
            scale,
            color,
        };
        self.workspace_cache
            .iter()
            .position(|cached| cached.key == key)
            .unwrap_or_else(|| {
                let label = if index == 0 {
                    "●"
                } else {
                    WORKSPACE_LABELS[index as usize - 1]
                };
                let mut pixmap = self.rasterize_text(label, scale, color);
                if index != 0 && !has_visible_pixels(&pixmap) {
                    const FALLBACK_LABELS: [&str; 10] =
                        ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"];
                    pixmap = self.rasterize_text(
                        FALLBACK_LABELS[index.saturating_sub(1) as usize],
                        scale,
                        color,
                    );
                }
                self.workspace_cache.push(WorkspaceBitmap { key, pixmap });
                self.workspace_cache.len() - 1
            })
    }

    fn draw_tasks(
        &mut self,
        pixmap: &mut PixmapMut<'_>,
        scale: u32,
        tasks: &[WindowTask],
        bar: BarLayout,
        workspace_count: usize,
        hitboxes: &mut Vec<Hitbox>,
    ) {
        if self.style.clock_position != ClockPosition::Center {
            return;
        }
        let scale_i32 = scale as i32;
        let left = self.style.workspace_width as i32 * workspace_count as i32 * scale_i32;
        let right = bar.clock_x - bar.spacing - self.style.task_spacing as i32 * scale_i32;
        let Some(layout) = TaskLayout::calculate(
            left,
            right,
            pixmap.height() as i32,
            tasks.len(),
            self.style.task_spacing as i32 * scale_i32,
        ) else {
            return;
        };

        let edge_label_width = if tasks.len() > 1 {
            let first = &tasks[0];
            let first_index =
                self.ensure_task_label(first, scale, task_foreground(&self.style, first));
            let first_width = self.task_text_cache[first_index].natural_width;
            let last = &tasks[tasks.len() - 1];
            let last_index =
                self.ensure_task_label(last, scale, task_foreground(&self.style, last));
            Some(first_width.min(self.task_text_cache[last_index].natural_width))
        } else {
            None
        };

        tasks.iter().enumerate().for_each(|(index, task)| {
            let Some(rect) = layout.item(index) else {
                return;
            };
            hitboxes.push(Hitbox {
                target: HitTarget::Window(task.id),
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
            });
            fill_rect(
                pixmap,
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                task_background(&self.style, task),
            );

            let padding = self.style.task_padding as i32 * scale_i32;
            let icon_size = (self.style.task_icon_size * scale) as i32;
            let color = task_foreground(&self.style, task);
            let width_limit = edge_label_width.filter(|_| index == 0 || index + 1 == tasks.len());
            let label_index = self.cached_task_label(task, scale, color, width_limit);
            let natural_label_width = self.task_text_cache[label_index].pixmap.width() as i32;
            let inner_width = (rect.width - padding * 2).max(0);
            let visible_label_width = if inner_width >= icon_size + padding {
                natural_label_width.min(inner_width - icon_size - padding)
            } else {
                0
            };
            let content_width = icon_size
                + if visible_label_width > 0 {
                    padding + visible_label_width
                } else {
                    0
                };
            let icon_x = rect.x + (rect.width - content_width) / 2;
            let icon_y = (rect.height - icon_size) / 2;
            let icon_index = self.cached_task_icon(task, scale, icon_size as u32);
            draw_premultiplied_clipped(
                pixmap,
                icon_x,
                icon_y,
                icon_size as u32,
                icon_size as u32,
                &self.task_icon_cache[icon_index].pixels,
                rect,
            );

            let label = &self.task_text_cache[label_index].pixmap;
            let label_x = icon_x + icon_size + padding;
            draw_premultiplied_clipped(
                pixmap,
                label_x,
                (rect.height - label.height() as i32) / 2,
                label.width(),
                label.height(),
                label.data(),
                PixelRect {
                    x: label_x,
                    y: rect.y,
                    width: visible_label_width,
                    height: rect.height,
                },
            );

            if task.is_urgent {
                fill_rect(
                    pixmap,
                    rect.x,
                    rect.height - 2 * scale_i32,
                    rect.width,
                    2 * scale_i32,
                    self.style.task_urgent_background,
                );
            }
        });
    }

    fn draw_media(
        &mut self,
        pixmap: &mut PixmapMut<'_>,
        scale: u32,
        media: Option<&MediaSnapshot>,
        left: i32,
        right: i32,
    ) {
        let Some(media) = media.filter(|_| self.style.media_enabled) else {
            self.media_cache.clear();
            return;
        };
        let left = left.max(0);
        let right = right.min(pixmap.width() as i32);
        if right <= left {
            return;
        }
        let color = self.style.foreground;
        // Position ticks only rebuild the detail bitmap; metadata stays cached.
        let index = if let Some(index) = self
            .media_cache
            .iter()
            .position(|entry| entry.scale == scale)
        {
            if self.media_cache[index].title != media.title {
                let bitmap = self.rasterize_text(&media.title, scale, color);
                self.media_cache[index].title.clone_from(&media.title);
                self.media_cache[index].title_pixmap = bitmap;
            }
            if self.media_cache[index].detail != media.detail {
                let bitmap = self.rasterize_text(&media.detail, scale, color);
                self.media_cache[index].detail.clone_from(&media.detail);
                self.media_cache[index].detail_pixmap = bitmap;
            }
            index
        } else {
            let title_pixmap = self.rasterize_text(&media.title, scale, color);
            let detail_pixmap = self.rasterize_text(&media.detail, scale, color);
            let separator_pixmap = self.rasterize_text(" - ", scale, color);
            self.media_cache.push(MediaBitmap {
                scale,
                title: media.title.clone(),
                title_pixmap,
                detail: media.detail.clone(),
                detail_pixmap,
                separator_pixmap,
            });
            self.media_cache.len() - 1
        };
        let cached = &self.media_cache[index];
        let height = pixmap.height() as i32;
        let clip = PixelRect {
            x: left,
            y: 0,
            width: right - left,
            height,
        };
        let gap = cached.separator_pixmap.width() as i32;
        let detail_width = cached.detail_pixmap.width() as i32;
        // Reserve elapsed/duration, percentage and playback state before clipping the title.
        let title_width = (right - left - detail_width - gap)
            .max(0)
            .min(cached.title_pixmap.width() as i32);
        let content_width = if title_width > 0 {
            title_width + gap + detail_width
        } else {
            detail_width.min(right - left)
        };
        let text_left = left + (right - left - content_width) / 2;
        let detail_x = if title_width > 0 {
            text_left + title_width + gap
        } else {
            (right - detail_width).max(text_left)
        };
        draw_premultiplied_clipped(
            pixmap,
            text_left,
            (height - cached.title_pixmap.height() as i32) / 2,
            cached.title_pixmap.width(),
            cached.title_pixmap.height(),
            cached.title_pixmap.data(),
            PixelRect {
                x: text_left,
                y: 0,
                width: title_width,
                height,
            },
        );
        if title_width > 0 {
            draw_premultiplied_clipped(
                pixmap,
                text_left + title_width,
                (height - cached.separator_pixmap.height() as i32) / 2,
                cached.separator_pixmap.width(),
                cached.separator_pixmap.height(),
                cached.separator_pixmap.data(),
                clip,
            );
        }
        draw_premultiplied_clipped(
            pixmap,
            detail_x,
            (height - cached.detail_pixmap.height() as i32) / 2,
            cached.detail_pixmap.width(),
            cached.detail_pixmap.height(),
            cached.detail_pixmap.data(),
            PixelRect {
                x: text_left,
                y: 0,
                width: right - text_left,
                height,
            },
        );
    }

    fn ensure_task_label(&mut self, task: &WindowTask, scale: u32, color: [u8; 4]) -> usize {
        if let Some(index) = self
            .task_text_cache
            .iter()
            .position(|cached| cached.id == task.id && cached.scale == scale)
        {
            if self.task_text_cache[index].label != task.label
                || self.task_text_cache[index].color != color
            {
                let pixmap = self.rasterize_text(&task.label, scale, color);
                self.task_text_cache[index] = TaskTextBitmap {
                    id: task.id,
                    label: task.label.clone(),
                    scale,
                    color,
                    natural_width: pixmap.width(),
                    width_limit: None,
                    pixmap,
                };
            }
            return index;
        }

        let pixmap = self.rasterize_text(&task.label, scale, color);
        trim_cache(&mut self.task_text_cache);
        self.task_text_cache.push(TaskTextBitmap {
            id: task.id,
            label: task.label.clone(),
            scale,
            color,
            natural_width: pixmap.width(),
            width_limit: None,
            pixmap,
        });
        self.task_text_cache.len() - 1
    }

    fn cached_task_label(
        &mut self,
        task: &WindowTask,
        scale: u32,
        color: [u8; 4],
        width_limit: Option<u32>,
    ) -> usize {
        let index = self.ensure_task_label(task, scale, color);
        let width_limit =
            width_limit.filter(|limit| self.task_text_cache[index].natural_width > *limit);
        if self.task_text_cache[index].width_limit == width_limit {
            return index;
        }

        let pixmap = if let Some(max_width) = width_limit {
            self.rasterize_ellipsized_text(&task.label, scale, color, max_width)
        } else {
            self.rasterize_text(&task.label, scale, color)
        };
        self.task_text_cache[index].pixmap = pixmap;
        self.task_text_cache[index].width_limit = width_limit;
        index
    }

    fn cached_task_icon(&mut self, task: &WindowTask, scale: u32, target_size: u32) -> usize {
        if let Some(index) = self
            .task_icon_cache
            .iter()
            .position(|cached| cached.id == task.id && cached.scale == scale)
        {
            if self.task_icon_cache[index].revision != task.icon.revision {
                self.task_icon_cache[index].revision = task.icon.revision;
                self.task_icon_cache[index].pixels = scale_pixels(
                    &task.icon.pixels,
                    task.icon.width,
                    task.icon.height,
                    target_size,
                );
            }
            return index;
        }

        let pixels = scale_pixels(
            &task.icon.pixels,
            task.icon.width,
            task.icon.height,
            target_size,
        );
        trim_cache(&mut self.task_icon_cache);
        self.task_icon_cache.push(TaskIconBitmap {
            id: task.id,
            scale,
            revision: task.icon.revision,
            pixels,
        });
        self.task_icon_cache.len() - 1
    }

    fn cached_icon(&mut self, icon: &TrayIcon, scale: u32, target_size: u32) -> usize {
        if let Some(index) = self
            .icon_cache
            .iter()
            .position(|cached| cached.id == icon.id && cached.scale == scale)
        {
            if self.icon_cache[index].revision != icon.revision {
                self.icon_cache[index].revision = icon.revision;
                self.icon_cache[index].pixels = scale_icon(icon, target_size);
            }
            return index;
        }

        self.icon_cache.push(IconBitmap {
            id: icon.id.clone(),
            scale,
            revision: icon.revision,
            pixels: scale_icon(icon, target_size),
        });
        self.icon_cache.len() - 1
    }
}

fn has_visible_pixels(pixmap: &Pixmap) -> bool {
    pixmap.data().chunks_exact(4).any(|pixel| pixel[3] != 0)
}

fn rgba(color: [u8; 4]) -> Color {
    Color::from_rgba8(color[0], color[1], color[2], color[3])
}

fn workspace_background(style: &Style, workspace: WorkspaceSlot) -> Option<Color> {
    match (workspace.is_focused, workspace.is_active) {
        (true, _) => Some(style.workspace_focused_background),
        (false, true) => Some(style.workspace_active_background),
        (false, false) => None,
    }
}

fn workspace_foreground(style: &Style, workspace: WorkspaceSlot) -> [u8; 4] {
    match (
        workspace.is_urgent,
        workspace.is_focused || workspace.is_active,
        workspace.is_occupied,
    ) {
        (true, _, _) => style.workspace_urgent_foreground,
        (false, true, _) => style.workspace_active_foreground,
        (false, false, true) => style.workspace_occupied_foreground,
        (false, false, false) => style.workspace_empty_foreground,
    }
}

fn task_background(style: &Style, task: &WindowTask) -> Color {
    match (task.is_focused, task.is_urgent) {
        (true, _) => style.task_focused_background,
        (false, true) => style.task_urgent_background,
        (false, false) => style.task_background,
    }
}

fn task_foreground(style: &Style, task: &WindowTask) -> [u8; 4] {
    match task.is_focused {
        true => style.task_focused_foreground,
        false => style.task_foreground,
    }
}

fn trim_cache<T>(cache: &mut Vec<T>) {
    const MAX_CACHE_ITEMS: usize = 512;
    if cache.len() >= MAX_CACHE_ITEMS {
        cache.drain(..MAX_CACHE_ITEMS / 2);
    }
}

fn fill_rect(pixmap: &mut PixmapMut<'_>, x: i32, y: i32, width: i32, height: i32, color: Color) {
    let Some(rect) = Rect::from_xywh(x as f32, y as f32, width as f32, height as f32) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = false;
    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
}

fn blend_text(
    pixmap: &mut PixmapMut<'_>,
    buffer: &mut Buffer,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    color: TextColor,
) {
    let width = pixmap.width() as i32;
    let height = pixmap.height() as i32;
    let data = pixmap.data_mut();

    buffer.draw(font_system, swash_cache, color, |x, y, w, h, color| {
        let (r, g, b, alpha) = (color.r(), color.g(), color.b(), color.a());
        if alpha == 0 {
            return;
        }

        for target_y in y.max(0)..(y + h as i32).min(height) {
            for target_x in x.max(0)..(x + w as i32).min(width) {
                let offset = ((target_y * width + target_x) * 4) as usize;
                blend_pixel(&mut data[offset..offset + 4], r, g, b, alpha);
            }
        }
    });
}

fn scale_icon(icon: &TrayIcon, target_size: u32) -> Vec<u8> {
    scale_pixels(&icon.pixels, icon.width, icon.height, target_size)
}

fn scale_pixels(source: &[u8], source_width: u32, source_height: u32, target_size: u32) -> Vec<u8> {
    if source_width == target_size && source_height == target_size {
        return source.to_vec();
    }
    let mut scaled = vec![0; (target_size * target_size * 4) as usize];
    for y in 0..target_size {
        let source_y = y * source_height / target_size;
        for x in 0..target_size {
            let source_x = x * source_width / target_size;
            let source_offset = ((source_y * source_width + source_x) * 4) as usize;
            let target_offset = ((y * target_size + x) * 4) as usize;
            scaled[target_offset..target_offset + 4]
                .copy_from_slice(&source[source_offset..source_offset + 4]);
        }
    }
    scaled
}

fn draw_premultiplied(
    pixmap: &mut PixmapMut<'_>,
    target_x: i32,
    target_y: i32,
    source_width: u32,
    source_height: u32,
    source: &[u8],
) {
    draw_premultiplied_clipped(
        pixmap,
        target_x,
        target_y,
        source_width,
        source_height,
        source,
        PixelRect {
            x: 0,
            y: 0,
            width: pixmap.width() as i32,
            height: pixmap.height() as i32,
        },
    );
}

fn draw_premultiplied_clipped(
    pixmap: &mut PixmapMut<'_>,
    target_x: i32,
    target_y: i32,
    source_width: u32,
    source_height: u32,
    source: &[u8],
    clip: PixelRect,
) {
    let canvas_width = pixmap.width() as i32;
    let canvas_height = pixmap.height() as i32;
    let clip_left = clip.x.max(0);
    let clip_top = clip.y.max(0);
    let clip_right = (clip.x + clip.width).min(canvas_width);
    let clip_bottom = (clip.y + clip.height).min(canvas_height);
    let source_left = (clip_left - target_x).max(0);
    let source_top = (clip_top - target_y).max(0);
    let source_right = (clip_right - target_x).min(source_width as i32);
    let source_bottom = (clip_bottom - target_y).min(source_height as i32);
    if source_left >= source_right || source_top >= source_bottom {
        return;
    }
    let target = pixmap.data_mut();

    for source_y in source_top..source_bottom {
        let y = target_y + source_y;
        for source_x in source_left..source_right {
            let x = target_x + source_x;
            let source_offset = ((source_y * source_width as i32 + source_x) * 4) as usize;
            let alpha = source[source_offset + 3];
            if alpha == 0 {
                continue;
            }

            let target_offset = ((y * canvas_width + x) * 4) as usize;
            if alpha == 255 {
                target[target_offset..target_offset + 4]
                    .copy_from_slice(&source[source_offset..source_offset + 4]);
            } else {
                blend_premultiplied(
                    &mut target[target_offset..target_offset + 4],
                    &source[source_offset..source_offset + 4],
                );
            }
        }
    }
}

#[inline]
fn blend_pixel(target: &mut [u8], r: u8, g: u8, b: u8, alpha: u8) {
    let alpha = alpha as u32;
    let inverse = 255 - alpha;
    target[0] = ((r as u32 * alpha + target[0] as u32 * inverse) / 255) as u8;
    target[1] = ((g as u32 * alpha + target[1] as u32 * inverse) / 255) as u8;
    target[2] = ((b as u32 * alpha + target[2] as u32 * inverse) / 255) as u8;
    target[3] = (alpha + target[3] as u32 * inverse / 255).min(255) as u8;
}

#[inline]
fn blend_premultiplied(target: &mut [u8], source: &[u8]) {
    let inverse = 255 - source[3] as u32;
    target[0] = (source[0] as u32 + target[0] as u32 * inverse / 255).min(255) as u8;
    target[1] = (source[1] as u32 + target[1] as u32 * inverse / 255).min(255) as u8;
    target[2] = (source[2] as u32 + target[2] as u32 * inverse / 255).min(255) as u8;
    target[3] = (source[3] as u32 + target[3] as u32 * inverse / 255).min(255) as u8;
}

#[cfg(test)]
mod tests {
    #[test]
    fn weather_follows_clock_without_changing_its_center() {
        for scale in [1, 2, 3] {
            let s = scale as i32;
            for centered in [true, false] {
                let config = Config {
                    weather_enabled: true,
                    network_enabled: true,
                    clock_position: if centered {
                        ClockPosition::Center
                    } else {
                        ClockPosition::Right
                    },
                    ..Config::default()
                };
                let renderer = Renderer::new(&config);
                for count in [0, 1, 12] {
                    let layout = BarLayout::calculate(
                        (1920 * s, 28 * s),
                        (200 * s, 14 * s),
                        (30 * s, 14 * s),
                        count,
                        scale,
                        &renderer.style,
                    );
                    if centered {
                        assert_eq!(layout.clock_x, 860 * s);
                    }
                    let weather_x = layout.weather_x.unwrap();
                    assert_eq!(weather_x, layout.clock_x + 206 * s);
                    assert!(weather_x + layout.icon_size <= 1920 * s);
                    if !centered {
                        assert_eq!(weather_x + layout.icon_size, 1912 * s);
                    }
                }
            }
        }
    }
    use std::sync::Arc;

    use super::*;
    use crate::{niri::ApplicationIcon, tray::TrayMenuEntry};

    #[test]
    fn centered_clock_uses_canvas_center_independent_of_side_content() {
        let config: Config = toml::from_str("clock_position = 'center'").unwrap();
        let renderer = Renderer::new(&config);
        for scale in [1, 2, 3] {
            for tray_len in [0, 1, 20] {
                for keyboard_width in [0, 32, 100] {
                    let s = scale as i32;
                    let layout = BarLayout::calculate(
                        (1920 * s, 28 * s),
                        (200 * s, 15 * s),
                        (keyboard_width * s, 15 * s),
                        tray_len,
                        scale,
                        &renderer.style,
                    );
                    assert_eq!(layout.clock_x, 860 * s);
                    if tray_len > 0 {
                        assert_eq!(
                            layout.icon_x(tray_len - 1) + layout.icon_size,
                            (1920 - config.padding as i32) * s
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn centered_clock_keeps_pixels_and_click_area_clear_when_sides_are_crowded() {
        for (drawer, network_enabled) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            for weather_enabled in [false, true] {
                let config = Config {
                    clock_position: ClockPosition::Center,
                    tray_drawer: drawer,
                    network_enabled,
                    weather_enabled,
                    workspace_width: 24,
                    ..Config::default()
                };
                let mut renderer = Renderer::new(&config);
                let workspaces = std::array::from_fn(|index| WorkspaceSlot {
                    index: index as u8 + 1,
                    is_occupied: true,
                    ..WorkspaceSlot::default()
                });
                let tasks = [WindowTask {
                    id: 1,
                    label: "Window".into(),
                    app_id: None,
                    is_focused: true,
                    is_urgent: false,
                    icon: ApplicationIcon {
                        revision: 1,
                        pixels: Arc::from([0, 255, 0, 255]),
                        width: 1,
                        height: 1,
                    },
                }];
                let tray: Vec<_> = (0..40)
                    .map(|index| TrayIcon {
                        id: index.to_string(),
                        revision: 1,
                        pixels: Arc::from([255, 0, 0, 255]),
                        width: 1,
                        height: 1,
                    })
                    .collect();
                for scale in [1, 2, 3] {
                    for width in [240, 800] {
                        let mut canvas = Pixmap::new(width * scale, config.height * scale).unwrap();
                        let content = RenderContent {
                            scale,
                            clock: "Tue Sep 15, 03:45",
                            keyboard_layout: "EN",
                            tray: &[],
                            tray_reveal: 0.0,
                            workspaces: &workspaces,
                            tasks: &[],
                            media: None,
                        };
                        let mut hitboxes = Vec::new();
                        renderer.draw(&mut canvas.as_mut(), content, &mut hitboxes);
                        let clock = *hitboxes
                            .iter()
                            .find(|hit| hit.target == HitTarget::Clock)
                            .unwrap();
                        let baseline = canvas.clone();
                        for reveal in [0.0, 0.5, 1.0] {
                            renderer.draw(
                                &mut canvas.as_mut(),
                                RenderContent {
                                    tray: &tray,
                                    tray_reveal: reveal,
                                    tasks: &tasks,
                                    ..content
                                },
                                &mut hitboxes,
                            );
                            let next = hitboxes
                                .iter()
                                .find(|hit| hit.target == HitTarget::Clock)
                                .unwrap();
                            assert_eq!((next.x, next.width), (clock.x, clock.width));
                            for y in 0..canvas.height() {
                                for x in clock.x..clock.x + clock.width {
                                    assert_eq!(
                                        canvas.pixel(x as u32, y),
                                        baseline.pixel(x as u32, y)
                                    );
                                }
                            }
                            assert!(
                                hitboxes
                                    .iter()
                                    .filter(|hit| hit.target != HitTarget::Clock && hit.width > 0)
                                    .all(|hit| hit.x + hit.width <= clock.x
                                        || hit.x >= clock.x + clock.width)
                            );
                            assert_eq!(
                                hit_target_at(&hitboxes, scale, f64::from(width) / 2.0, 1.0),
                                Some(HitTarget::Clock)
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn network_slot_stays_right_of_keyboard_and_does_not_shift_with_tray() {
        for position in [ClockPosition::Center, ClockPosition::Right] {
            let config = Config {
                clock_position: position,
                network_enabled: true,
                ..Config::default()
            };
            let renderer = Renderer::new(&config);
            for scale in [1, 2, 3] {
                let s = scale as i32;
                for tray_len in [0, 1, 20] {
                    let layout = BarLayout::calculate(
                        (1920 * s, 28 * s),
                        (200 * s, 15 * s),
                        (32 * s, 15 * s),
                        tray_len,
                        scale,
                        &renderer.style,
                    );
                    let x = layout.network_x.unwrap();
                    assert_eq!(
                        x,
                        if position == ClockPosition::Center {
                            1894 * s
                        } else {
                            1688 * s
                        }
                    );
                    assert_eq!(
                        layout.keyboard_x + layout.keyboard_width + layout.spacing,
                        x
                    );
                    assert_eq!(
                        layout.clock_x,
                        if position == ClockPosition::Center {
                            860 * s
                        } else {
                            1712 * s
                        }
                    );
                    if tray_len > 0 {
                        assert!(layout.icon_x(tray_len - 1) + layout.icon_size < layout.keyboard_x);
                    }
                }
            }
        }
    }

    #[test]
    fn tray_drawer_hides_icon_pixels_and_click_targets() {
        let config = Config {
            tray_drawer: true,
            clock_position: ClockPosition::Center,
            show_tasks: true,
            ..Config::default()
        };
        let mut renderer = Renderer::new(&config);
        let tray = [TrayIcon {
            id: "hidden".into(),
            revision: 1,
            pixels: Arc::from([255, 0, 0, 255]),
            width: 1,
            height: 1,
        }];
        let mut pixmap = Pixmap::new(800, 28).unwrap();
        let mut hitboxes = Vec::new();
        renderer.draw(
            &mut pixmap.as_mut(),
            RenderContent {
                scale: 1,
                clock: "12:00",
                keyboard_layout: "EN",
                tray: &tray,
                tray_reveal: 0.0,
                workspaces: &Default::default(),
                tasks: &[],
                media: None,
            },
            &mut hitboxes,
        );
        assert!(
            !hitboxes
                .iter()
                .any(|hit| matches!(hit.target, HitTarget::Tray(_)))
        );
        assert!(
            !pixmap
                .data()
                .chunks_exact(4)
                .any(|pixel| pixel == [255, 0, 0, 255])
        );
        assert!(
            renderer.icon_cache.is_empty(),
            "hidden icons should not be rasterized"
        );
    }

    #[test]
    fn tray_drawer_keeps_handle_fixed_and_clips_icons_at_each_scale() {
        let config = Config {
            tray_drawer: true,
            clock_position: ClockPosition::Center,
            show_tasks: true,
            ..Config::default()
        };
        let mut renderer = Renderer::new(&config);
        let tray = [
            TrayIcon {
                id: "red".into(),
                revision: 1,
                pixels: Arc::from([255, 0, 0, 255]),
                width: 1,
                height: 1,
            },
            TrayIcon {
                id: "blue".into(),
                revision: 1,
                pixels: Arc::from([0, 0, 255, 255]),
                width: 1,
                height: 1,
            },
        ];
        let workspaces = Default::default();
        let tasks = [WindowTask {
            id: 1,
            label: "Terminal".into(),
            app_id: None,
            is_focused: false,
            is_urgent: false,
            icon: ApplicationIcon {
                revision: 1,
                pixels: Arc::from([128, 128, 128, 255]),
                width: 1,
                height: 1,
            },
        }];
        for scale in [1, 2] {
            let mut pixmap = Pixmap::new(800 * scale, 28 * scale).unwrap();
            let mut hitboxes = Vec::new();
            let mut handle_right = None;
            for (progress, expected_width, expected_icons) in
                [(0.0, 18, 0), (0.25, 30, 1), (0.5, 42, 1), (1.0, 66, 2)]
            {
                renderer.draw(
                    &mut pixmap.as_mut(),
                    RenderContent {
                        scale,
                        clock: "12:00",
                        keyboard_layout: "EN",
                        tray: &tray,
                        tray_reveal: progress,
                        workspaces: &workspaces,
                        tasks: &tasks,
                        media: None,
                    },
                    &mut hitboxes,
                );
                let group = *hitboxes
                    .iter()
                    .find(|hit| hit.target == HitTarget::TrayDrawer)
                    .unwrap();
                let right = group.x + group.width;
                let keyboard_width =
                    renderer.keyboard_cache.as_ref().unwrap().reserved_width as i32;
                assert_eq!(
                    right + 2 * config.tray_spacing as i32 * scale as i32 + keyboard_width,
                    (800 - config.padding) as i32 * scale as i32
                );
                let task = hitboxes
                    .iter()
                    .find(|hit| hit.target == HitTarget::Window(1))
                    .unwrap();
                assert!(
                    task.x + task.width <= group.x,
                    "tasks must end before the drawer"
                );
                assert_eq!(group.width, expected_width * scale as i32);
                assert_eq!(*handle_right.get_or_insert(right), right);
                assert_eq!(
                    hitboxes
                        .iter()
                        .filter(|hit| matches!(hit.target, HitTarget::Tray(_)))
                        .count(),
                    expected_icons
                );
                for hit in hitboxes
                    .iter()
                    .filter(|hit| matches!(hit.target, HitTarget::Tray(_)))
                {
                    assert!(hit.x >= group.x);
                    assert!(hit.x + hit.width <= right - 24 * scale as i32);
                    assert_eq!(
                        hit_target_at(&hitboxes, scale, f64::from(hit.x) / f64::from(scale), 14.0),
                        Some(hit.target)
                    );
                }
                // The entire group, including gaps and padding, stays hoverable.
                for x in group.x..right {
                    assert!(matches!(
                        hit_target_at(&hitboxes, scale, f64::from(x) / f64::from(scale), 14.0),
                        Some(HitTarget::TrayDrawer | HitTarget::Tray(_))
                    ));
                    assert_eq!(
                        hit_target_at(&hitboxes, scale, f64::from(x) / f64::from(scale), 0.0),
                        Some(HitTarget::TrayDrawer)
                    );
                }
                assert!(
                    !pixmap
                        .data()
                        .chunks_exact(4)
                        .any(|pixel| pixel == [0, 0, 255, 255])
                        || progress == 1.0
                );
                let red_pixels = pixmap
                    .data()
                    .chunks_exact(4)
                    .filter(|pixel| *pixel == [255, 0, 0, 255])
                    .count();
                let red_width = if progress == 0.0 {
                    0
                } else if progress == 0.25 {
                    6
                } else {
                    18
                };
                assert_eq!(red_pixels, red_width * 18 * (scale * scale) as usize);
            }
            renderer.draw(
                &mut pixmap.as_mut(),
                RenderContent {
                    scale,
                    clock: "12:00",
                    keyboard_layout: "EN",
                    tray: &[],
                    tray_reveal: 1.0,
                    workspaces: &workspaces,
                    tasks: &[],
                    media: None,
                },
                &mut hitboxes,
            );
            assert!(
                !hitboxes
                    .iter()
                    .any(|hit| matches!(hit.target, HitTarget::TrayDrawer | HitTarget::Tray(_)))
            );
        }
    }

    fn menu_entry(id: i32, label: &str, enabled: bool) -> TrayMenuEntry {
        TrayMenuEntry {
            id,
            label: label.into(),
            enabled,
            separator: false,
            submenu: Vec::new(),
            toggle_type: Default::default(),
            toggle_state: Default::default(),
        }
    }

    fn separator() -> TrayMenuEntry {
        TrayMenuEntry {
            id: 0,
            label: String::new(),
            enabled: false,
            separator: true,
            submenu: Vec::new(),
            toggle_type: Default::default(),
            toggle_state: Default::default(),
        }
    }

    #[test]
    fn font_path_uses_the_first_nonempty_line() {
        assert_eq!(
            font_path("\n /fonts/ui.ttf \nignored\n"),
            Some(std::path::PathBuf::from("/fonts/ui.ttf"))
        );
    }

    #[test]
    fn distinct_font_paths_loads_a_shared_file_once() {
        let path = std::path::PathBuf::from("/fonts/shared.ttf");
        assert_eq!(
            distinct_font_paths([path.clone(), path]),
            vec![std::path::PathBuf::from("/fonts/shared.ttf")]
        );
    }

    #[test]
    fn selected_font_paths_requires_both_matches() {
        assert_eq!(
            selected_font_paths([Some(std::path::PathBuf::from("/fonts/ui.ttf")), None]),
            None
        );
    }

    #[test]
    fn menu_surface_has_transparent_outer_corners() {
        let mut renderer = Renderer::new(&Config::default());
        let menu = renderer.prepare_menu(&[menu_entry(1, "Open", true)], 1, None);
        let (width, height) = menu.size();
        let mut pixmap = Pixmap::new(width, height).unwrap();
        renderer.draw_menu(&mut pixmap.as_mut(), &menu, &mut Vec::new(), None);
        assert_eq!(pixmap.pixel(0, 0).unwrap().alpha(), 0);
        assert_eq!(pixmap.pixel(width / 2, height / 2).unwrap().alpha(), 255);
    }

    #[test]
    fn menu_separators_are_compact_and_outer_separators_removed() {
        let mut renderer = Renderer::new(&Config::default());
        let plain = renderer.prepare_menu(
            &[menu_entry(1, "Open", true), menu_entry(2, "Quit", true)],
            1,
            None,
        );
        let grouped = renderer.prepare_menu(
            &[
                separator(),
                menu_entry(1, "Open", true),
                separator(),
                separator(),
                menu_entry(2, "Quit", true),
                separator(),
            ],
            1,
            None,
        );
        assert_eq!(grouped.size().1 - plain.size().1, 9);
    }

    #[test]
    fn prepared_menu_supplies_size_and_enabled_hitboxes() {
        let mut renderer = Renderer::new(&Config::default());
        let entries = [
            menu_entry(1, "Open", true),
            separator(),
            menu_entry(2, "Quit", false),
        ];
        let menu = renderer.prepare_menu(&entries, 1, None);
        let (width, height) = menu.size();
        let mut pixmap = Pixmap::new(width, height).unwrap();
        let mut hitboxes = Vec::new();

        renderer.draw_menu(&mut pixmap.as_mut(), &menu, &mut hitboxes, None);

        assert_eq!(hitboxes.len(), 2);
        assert_eq!(hitboxes[0].selection, MenuSelection::Item(1));
        assert!(!hitboxes[1].enabled);
    }

    #[test]
    fn icon_scaling_reuses_exact_pixels() {
        let icon = TrayIcon {
            id: "test".into(),
            revision: 1,
            pixels: Arc::from([1, 2, 3, 255]),
            width: 1,
            height: 1,
        };
        assert_eq!(scale_icon(&icon, 1), [1, 2, 3, 255]);
        assert_eq!(scale_icon(&icon, 2), [1, 2, 3, 255].repeat(4));
    }

    #[test]
    fn layout_places_tray_left_of_clock() {
        let style = Style {
            font_family: "".into(),
            font_size: 13.0,
            background: Color::BLACK,
            foreground: [255; 4],
            padding: 8,
            tray_icon_size: 18,
            tray_spacing: 6,
            tray_drawer: false,
            clock_position: ClockPosition::Right,
            calendar_enabled: true,
            weather_enabled: false,
            weather_foreground: [220, 220, 204, 255],
            network_enabled: false,
            network_foreground: [255; 4],
            network_muted: [128; 4],
            workspace_width: 28,
            workspace_focused_background: Color::BLACK,
            workspace_active_background: Color::BLACK,
            workspace_active_foreground: [255; 4],
            workspace_occupied_foreground: [200; 4],
            workspace_empty_foreground: [100; 4],
            workspace_urgent_foreground: [255, 0, 0, 255],
            media_enabled: false,
            task_icon_size: 18,
            task_spacing: 2,
            task_padding: 6,
            task_background: Color::BLACK,
            task_focused_background: Color::BLACK,
            task_urgent_background: Color::BLACK,
            task_foreground: [200; 4],
            task_focused_foreground: [255; 4],
        };
        let layout = BarLayout::calculate((1920, 28), (130, 17), (0, 0), 2, 1, &style);
        assert_eq!(layout.clock_x, 1782);
        assert!(layout.icon_x(1) + layout.icon_size < layout.clock_x);
    }

    #[test]
    fn layout_places_keyboard_left_of_tray() {
        let style = Style {
            font_family: "".into(),
            font_size: 13.0,
            background: Color::BLACK,
            foreground: [255; 4],
            padding: 8,
            tray_icon_size: 18,
            tray_spacing: 6,
            tray_drawer: false,
            clock_position: ClockPosition::Right,
            calendar_enabled: true,
            weather_enabled: false,
            weather_foreground: [220, 220, 204, 255],
            network_enabled: false,
            network_foreground: [255; 4],
            network_muted: [128; 4],
            workspace_width: 28,
            workspace_focused_background: Color::BLACK,
            workspace_active_background: Color::BLACK,
            workspace_active_foreground: [255; 4],
            workspace_occupied_foreground: [200; 4],
            workspace_empty_foreground: [100; 4],
            workspace_urgent_foreground: [255, 0, 0, 255],
            media_enabled: false,
            task_icon_size: 18,
            task_spacing: 2,
            task_padding: 6,
            task_background: Color::BLACK,
            task_focused_background: Color::BLACK,
            task_urgent_background: Color::BLACK,
            task_foreground: [200; 4],
            task_focused_foreground: [255; 4],
        };
        let layout = BarLayout::calculate((1920, 28), (130, 17), (18, 13), 2, 1, &style);

        assert_eq!(
            layout.keyboard_x + layout.keyboard_width + layout.spacing,
            layout.tray_left
        );
        assert!(layout.keyboard_x < layout.tray_left);
    }

    #[test]
    fn keyboard_layout_reserves_same_width_for_ru_and_en() {
        let mut renderer = Renderer::new(&Config::default());

        renderer.cache_keyboard_layout("RU", 1);
        let ru_width = renderer.keyboard_cache.as_ref().unwrap().reserved_width;

        renderer.cache_keyboard_layout("EN", 1);
        let en_width = renderer.keyboard_cache.as_ref().unwrap().reserved_width;

        assert_eq!(ru_width, en_width);
    }

    #[test]
    fn task_layout_distributes_all_available_pixels() {
        let layout = TaskLayout::calculate(10, 103, 28, 3, 2).unwrap();
        assert_eq!(layout.item(0).unwrap().width, 30);
        assert_eq!(layout.item(1).unwrap().width, 30);
        let last = layout.item(2).unwrap();
        assert_eq!(last.width, 29);
        assert_eq!(last.x + last.width, 103);
    }

    #[test]
    fn task_icon_and_label_are_centered_as_one_group() {
        let config = Config {
            clock_position: ClockPosition::Center,
            show_tasks: true,
            ..Config::default()
        };
        let mut renderer = Renderer::new(&config);
        let workspaces = Default::default();
        let tasks = [WindowTask {
            id: 1,
            label: "I".into(),
            app_id: None,
            is_focused: false,
            is_urgent: false,
            icon: ApplicationIcon {
                revision: 1,
                pixels: Arc::from([0, 0, 255, 255]),
                width: 1,
                height: 1,
            },
        }];
        let mut pixmap = Pixmap::new(1600, config.height).unwrap();
        let mut hitboxes = Vec::new();

        renderer.draw(
            &mut pixmap.as_mut(),
            RenderContent {
                scale: 1,
                clock: "12:34",
                keyboard_layout: "",
                tray: &[],
                tray_reveal: 0.0,
                workspaces: &workspaces,
                tasks: &tasks,
                media: None,
            },
            &mut hitboxes,
        );

        let task = hitboxes
            .iter()
            .find(|hit| hit.target == HitTarget::Window(1))
            .unwrap();
        let label_width = renderer.task_text_cache[0].pixmap.width() as i32;
        let content_width = config.task_icon_size as i32 + config.task_padding as i32 + label_width;
        let expected_icon_x =
            task.x + ((task.width - content_width) / 2).max(config.task_padding as i32);
        let icon_y = (config.height - config.task_icon_size) / 2;
        let actual_icon_x = (task.x..task.x + task.width)
            .find(|&x| {
                pixmap.pixel(x as u32, icon_y).unwrap()
                    == tiny_skia::PremultipliedColorU8::from_rgba(0, 0, 255, 255).unwrap()
            })
            .unwrap();

        assert_eq!(actual_icon_x, expected_icon_x);
        assert!(
            (actual_icon_x * 2
                + config.task_icon_size as i32
                + config.task_padding as i32
                + label_width
                - (task.x * 2 + task.width))
                .abs()
                <= 1
        );
    }

    #[test]
    fn task_icon_stays_centered_when_the_label_does_not_fit() {
        let config = Config {
            workspace_width: 16,
            background: "#00000000".into(),
            task_background: "#00000000".into(),
            task_focused_background: "#00000000".into(),
            clock_position: ClockPosition::Center,
            show_tasks: true,
            ..Config::default()
        };
        let mut renderer = Renderer::new(&config);
        let workspaces = Default::default();
        let tasks: [WindowTask; 5] = std::array::from_fn(|index| WindowTask {
            id: index as u64 + 1,
            label: "Terminal".into(),
            app_id: None,
            is_focused: false,
            is_urgent: false,
            icon: ApplicationIcon {
                revision: 1,
                pixels: Arc::from([0, 0, 255, 255]),
                width: 1,
                height: 1,
            },
        });
        let mut pixmap = Pixmap::new(400, config.height).unwrap();
        let mut hitboxes = Vec::new();

        renderer.draw(
            &mut pixmap.as_mut(),
            RenderContent {
                scale: 1,
                clock: "12:34",
                keyboard_layout: "",
                tray: &[],
                tray_reveal: 0.0,
                workspaces: &workspaces,
                tasks: &tasks,
                media: None,
            },
            &mut hitboxes,
        );

        let task = hitboxes
            .iter()
            .find(|hit| hit.target == HitTarget::Window(1))
            .unwrap();
        let icon_y = (config.height - config.task_icon_size) / 2;
        let blue = tiny_skia::PremultipliedColorU8::from_rgba(0, 0, 255, 255).unwrap();
        let visible_icon_x = (task.x..task.x + task.width)
            .filter(|&x| pixmap.pixel(x as u32, icon_y).unwrap() == blue)
            .collect::<Vec<_>>();

        assert!(!visible_icon_x.is_empty());
        let actual_center = visible_icon_x[0] + visible_icon_x[visible_icon_x.len() - 1] + 1;
        let expected_center = task.x * 2 + task.width;
        assert!(
            (actual_center - expected_center).abs() <= 1,
            "visible icon center {actual_center}, task center {expected_center}, task {task:?}, pixels {visible_icon_x:?}"
        );
    }

    #[test]
    fn edge_task_labels_use_the_shorter_edge_width() {
        let config = Config {
            clock_position: ClockPosition::Center,
            show_tasks: true,
            ..Config::default()
        };
        let mut renderer = Renderer::new(&config);
        let workspaces = Default::default();
        let labels = [
            "Kitty",
            "The middle application title remains independent",
            "A much longer title on the right edge",
        ];
        let tasks: [WindowTask; 3] = std::array::from_fn(|index| WindowTask {
            id: index as u64 + 1,
            label: labels[index].into(),
            app_id: None,
            is_focused: false,
            is_urgent: false,
            icon: ApplicationIcon {
                revision: 1,
                pixels: Arc::from([0, 0, 255, 255]),
                width: 1,
                height: 1,
            },
        });
        let mut pixmap = Pixmap::new(2400, config.height).unwrap();
        let mut hitboxes = Vec::new();

        renderer.draw(
            &mut pixmap.as_mut(),
            RenderContent {
                scale: 1,
                clock: "12:34",
                keyboard_layout: "",
                tray: &[],
                tray_reveal: 0.0,
                workspaces: &workspaces,
                tasks: &tasks,
                media: None,
            },
            &mut hitboxes,
        );

        let width = |id| {
            renderer
                .task_text_cache
                .iter()
                .find(|cached| cached.id == id)
                .unwrap()
                .pixmap
                .width()
        };
        assert!(width(2) > width(1));
        assert!(width(3) <= width(1));
    }

    #[test]
    fn task_text_eviction_removes_every_scale_and_keeps_capacity() {
        let mut renderer = Renderer::new(&Config::default());
        renderer.task_text_cache.reserve(8);
        for (id, scale) in [(7, 1), (7, 2), (9, 1)] {
            renderer.task_icon_cache.push(TaskIconBitmap {
                id,
                scale,
                revision: 1,
                pixels: vec![255; 4],
            });
            renderer.task_text_cache.push(TaskTextBitmap {
                id,
                label: format!("window-{id}"),
                scale,
                color: [255; 4],
                natural_width: 1,
                width_limit: None,
                pixmap: Pixmap::new(1, 1).unwrap(),
            });
        }
        let capacity = renderer.task_text_cache.capacity();

        renderer.evict_window(7);

        assert_eq!(
            renderer
                .task_text_cache
                .iter()
                .map(|entry| (entry.id, entry.scale))
                .collect::<Vec<_>>(),
            [(9, 1)]
        );
        assert_eq!(renderer.task_text_cache.capacity(), capacity);
        assert_eq!(
            renderer
                .task_icon_cache
                .iter()
                .map(|entry| (entry.id, entry.scale))
                .collect::<Vec<_>>(),
            [(9, 1)]
        );
    }

    #[test]
    fn tray_removal_evicts_all_scales_without_drawing() {
        let mut renderer = Renderer::new(&Config::default());
        let icon = |id: &str| TrayIcon {
            id: id.into(),
            revision: 1,
            pixels: Arc::from([255; 4]),
            width: 1,
            height: 1,
        };
        let removed = icon("removed");
        let retained = icon("retained");
        renderer.cached_icon(&removed, 1, 18);
        renderer.cached_icon(&removed, 2, 36);
        renderer.cached_icon(&retained, 1, 18);
        renderer.retain_tray_icons(std::slice::from_ref(&retained));
        assert_eq!(renderer.icon_cache.len(), 1);
        assert_eq!(renderer.icon_cache[0].id, "retained");
        renderer.retain_tray_icons(&[]);
        assert!(renderer.icon_cache.is_empty());
    }

    #[test]
    fn task_list_stays_between_workspaces_and_centered_clock() {
        let config = Config {
            workspace_width: 24,
            task_spacing: 2,
            task_focused_background: "#123456".into(),
            clock_position: ClockPosition::Center,
            show_tasks: true,
            ..Config::default()
        };
        let mut renderer = Renderer::new(&config);
        let workspaces = std::array::from_fn(|offset| WorkspaceSlot {
            index: offset as u8 + 1,
            ..WorkspaceSlot::default()
        });
        let tasks = [WindowTask {
            id: 1,
            label: "Terminal".into(),
            app_id: Some("terminal".into()),
            is_focused: true,
            is_urgent: false,
            icon: ApplicationIcon {
                revision: 1,
                pixels: Arc::from([0, 0, 255, 255]),
                width: 1,
                height: 1,
            },
        }];
        let mut pixmap = Pixmap::new(800, config.height).unwrap();
        let mut hitboxes = Vec::new();

        renderer.draw(
            &mut pixmap.as_mut(),
            RenderContent {
                scale: 1,
                clock: "Sun Jul 05, 10:42",
                keyboard_layout: "",
                tray: &[],
                tray_reveal: 0.0,
                workspaces: &workspaces,
                tasks: &tasks,
                media: None,
            },
            &mut hitboxes,
        );

        let task = hitboxes
            .iter()
            .find(|hit| hit.target == HitTarget::Window(1))
            .unwrap();
        assert_eq!(
            task.x,
            config.workspace_width as i32 * WORKSPACES_PER_OUTPUT as i32
        );
        let task_start = task.x as usize * 4;
        assert_eq!(
            &pixmap.data()[task_start..task_start + 4],
            &[0x12, 0x34, 0x56, 255]
        );
        assert_eq!(hitboxes.len(), WORKSPACES_PER_OUTPUT + tasks.len() + 1);
        let clock = hitboxes
            .iter()
            .find(|hit| hit.target == HitTarget::Clock)
            .unwrap();
        assert!(task.x >= config.workspace_width as i32 * WORKSPACES_PER_OUTPUT as i32);
        assert_eq!(
            clock.x - task.x - task.width,
            (config.tray_spacing + config.task_spacing) as i32
        );
    }

    #[test]
    fn active_workspace_marker_replaces_labels_and_preserves_occupancy() {
        let config = Config {
            height: 24,
            workspace_width: 24,
            background: "#00000000".into(),
            workspace_focused_background: "#00000000".into(),
            workspace_active_background: "#00000000".into(),
            workspace_active_foreground: "#FFFFFFFF".into(),
            workspace_occupied_foreground: "#FFFFFFFF".into(),
            workspace_empty_foreground: "#FFFFFFFF".into(),
            ..Config::default()
        };
        let mut renderer = Renderer::new(&config);
        for scale in [1, 2, 3] {
            let mut render = |workspace| {
                let mut pixmap = Pixmap::new(24 * scale, 24 * scale).unwrap();
                let mut hitboxes = Vec::new();
                renderer.draw_workspaces(
                    &mut pixmap.as_mut(),
                    scale,
                    &[workspace],
                    &mut hitboxes,
                    (24 * scale) as i32,
                );
                assert_eq!(hitboxes.len(), 1);
                assert_eq!(hitboxes[0].width, (24 * scale) as i32);
                pixmap
            };
            let first = WorkspaceSlot {
                index: 1,
                ..WorkspaceSlot::default()
            };
            let label = render(first);
            let selected = render(WorkspaceSlot {
                is_focused: true,
                ..first
            });
            let other_selected = render(WorkspaceSlot {
                index: 2,
                is_active: true,
                ..WorkspaceSlot::default()
            });
            assert!(has_visible_pixels(&selected));
            assert_ne!(
                selected.data(),
                label.data(),
                "selection must replace the label"
            );
            assert_eq!(
                selected.data(),
                other_selected.data(),
                "all selected workspaces use the same dot"
            );
            assert_eq!(
                render(first).data(),
                label.data(),
                "deselection must restore the cached label"
            );

            for active in [false, true] {
                let empty = render(WorkspaceSlot {
                    is_active: active,
                    ..first
                });
                let occupied = render(WorkspaceSlot {
                    is_active: active,
                    is_occupied: true,
                    ..first
                });
                let underline_start = (22 * scale * 24 * scale * 4) as usize;
                assert_eq!(
                    &empty.data()[..underline_start],
                    &occupied.data()[..underline_start]
                );
                for y in 22 * scale..24 * scale {
                    for x in 0..24 * scale {
                        let expected = if (8 * scale..16 * scale).contains(&x) {
                            255
                        } else {
                            0
                        };
                        assert_eq!(occupied.pixel(x, y).unwrap().alpha(), expected);
                        assert_eq!(empty.pixel(x, y).unwrap().alpha(), 0);
                    }
                }
                assert_eq!(occupied.pixel(0, 0).unwrap().alpha(), 0);
            }
        }
    }

    #[test]
    fn workspaces_tasks_and_tray_are_drawn_in_separate_regions() {
        let config = Config {
            workspace_focused_background: "#654321".into(),
            task_focused_background: "#123456".into(),
            clock_position: ClockPosition::Center,
            show_tasks: true,
            ..Config::default()
        };
        let mut renderer = Renderer::new(&config);
        let workspaces = std::array::from_fn(|offset| WorkspaceSlot {
            index: offset as u8 + 1,
            is_focused: offset == 0,
            ..WorkspaceSlot::default()
        });
        let tasks = [WindowTask {
            id: 1,
            label: "Terminal".into(),
            app_id: Some("terminal".into()),
            is_focused: true,
            is_urgent: false,
            icon: ApplicationIcon {
                revision: 1,
                pixels: Arc::from([0, 0, 255, 255]),
                width: 1,
                height: 1,
            },
        }];
        let tray = [TrayIcon {
            id: "tray".into(),
            revision: 1,
            pixels: Arc::from([255, 0, 0, 255]),
            width: 1,
            height: 1,
        }];
        let mut pixmap = Pixmap::new(800, config.height).unwrap();
        let mut hitboxes = Vec::new();

        renderer.draw(
            &mut pixmap.as_mut(),
            RenderContent {
                scale: 1,
                clock: "Sun Jul 05, 10:42",
                keyboard_layout: "RU",
                tray: &tray,
                tray_reveal: 0.0,
                workspaces: &workspaces,
                tasks: &tasks,
                media: None,
            },
            &mut hitboxes,
        );

        assert_eq!(&pixmap.data()[..4], &[0x65, 0x43, 0x21, 255]);
        let task_offset = hitboxes
            .iter()
            .find(|hit| hit.target == HitTarget::Window(1))
            .unwrap()
            .x as usize
            * 4;
        assert_eq!(
            &pixmap.data()[task_offset..task_offset + 4],
            &[0x12, 0x34, 0x56, 255]
        );
        assert!(
            pixmap
                .data()
                .chunks_exact(4)
                .any(|pixel| pixel == [255, 0, 0, 255])
        );
        assert_eq!(
            hitboxes.len(),
            WORKSPACES_PER_OUTPUT + tasks.len() + tray.len() + 1
        );
        assert_eq!(hitboxes.last().unwrap().target, HitTarget::Tray(0));
    }

    #[test]
    fn rendered_buttons_hit_the_correct_targets_at_each_scale() {
        let config = Config {
            clock_position: ClockPosition::Center,
            show_tasks: true,
            ..Config::default()
        };
        let mut renderer = Renderer::new(&config);
        let workspaces = std::array::from_fn(|offset| WorkspaceSlot {
            id: Some(100 + offset as u64),
            index: offset as u8 + 6,
            ..WorkspaceSlot::default()
        });
        let tasks = [42, 99].map(|id| WindowTask {
            id,
            label: "Terminal".into(),
            app_id: None,
            is_focused: false,
            is_urgent: false,
            icon: ApplicationIcon {
                revision: 1,
                pixels: Arc::from([0, 0, 255, 255]),
                width: 1,
                height: 1,
            },
        });
        let mut hitboxes = Vec::new();

        for scale in [1, 2, 3] {
            let mut pixmap = Pixmap::new(800 * scale, config.height * scale).unwrap();
            let content = RenderContent {
                scale,
                clock: "12:34",
                keyboard_layout: "RU",
                tray: &[],
                tray_reveal: 0.0,
                workspaces: &workspaces,
                tasks: &tasks,
                media: None,
            };
            renderer.draw(&mut pixmap.as_mut(), content, &mut hitboxes);

            for (offset, workspace) in workspaces.iter().enumerate() {
                let x = f64::from(config.workspace_width) * (offset as f64 + 0.5);
                assert_eq!(
                    hit_target_at(&hitboxes, scale, x, 0.5),
                    Some(HitTarget::Workspace {
                        id: workspace.id,
                        index: offset as u8 + 1
                    })
                );
            }
            for hitbox in &hitboxes[WORKSPACES_PER_OUTPUT..] {
                for physical_x in [hitbox.x, hitbox.x + hitbox.width - 1] {
                    let x = (f64::from(physical_x) + 0.5) / f64::from(scale);
                    assert_eq!(hit_target_at(&hitboxes, scale, x, 0.5), Some(hitbox.target));
                }
            }
            assert_eq!(
                hitboxes[WORKSPACES_PER_OUTPUT].target,
                HitTarget::Window(42)
            );
            assert_eq!(
                hitboxes[WORKSPACES_PER_OUTPUT + 1].target,
                HitTarget::Window(99)
            );
            let first = hitboxes[WORKSPACES_PER_OUTPUT];
            let gap_x = f64::from(first.x + first.width) / f64::from(scale);
            assert_eq!(hit_target_at(&hitboxes, scale, gap_x, 0.5), None);
            assert_eq!(hit_target_at(&hitboxes, scale, -0.1, 0.5), None);
            assert_eq!(
                hit_target_at(&hitboxes, scale, 0.5, f64::from(config.height)),
                None
            );

            renderer.draw(
                &mut pixmap.as_mut(),
                RenderContent {
                    tasks: &[],
                    ..content
                },
                &mut hitboxes,
            );
            assert_eq!(hitboxes.len(), WORKSPACES_PER_OUTPUT + 1);
            assert_eq!(hit_target_at(&hitboxes, scale, gap_x - 1.0, 0.5), None);

            let mut narrow =
                Pixmap::new(config.workspace_width * scale, config.height * scale).unwrap();
            renderer.draw(&mut narrow.as_mut(), content, &mut hitboxes);
            assert!(
                hitboxes
                    .iter()
                    .all(|hitbox| !matches!(hitbox.target, HitTarget::Window(_)))
            );
        }
    }

    fn sample_media() -> MediaSnapshot {
        MediaSnapshot {
            title: "Artist - A long track title ".repeat(12),
            detail: "00:49 / 02:40 (31%) [Paused]".into(),
        }
    }

    #[test]
    fn media_truncates_title_before_detail_and_never_draws_outside_its_region() {
        let mut renderer = Renderer::new(&Config::default());
        let media = sample_media();
        for scale in [1, 2, 3] {
            let left = 20 * scale as i32;
            let right = 420 * scale as i32;
            let mut canvas = Pixmap::new(460 * scale, 28 * scale).unwrap();
            renderer.draw_media(&mut canvas.as_mut(), scale, Some(&media), left, right);
            let cached = renderer
                .media_cache
                .iter()
                .find(|entry| entry.scale == scale)
                .unwrap();
            let detail_x = right - cached.detail_pixmap.width() as i32;
            let detail_y = (canvas.height() - cached.detail_pixmap.height()) / 2;
            assert!(cached.title_pixmap.width() > 400 * scale);
            for y in 0..canvas.height() {
                for x in 0..canvas.width() {
                    if (x as i32) < left || x as i32 >= right {
                        assert_eq!(canvas.pixel(x, y).unwrap().alpha(), 0);
                    }
                }
            }
            for y in 0..cached.detail_pixmap.height() {
                for x in 0..cached.detail_pixmap.width() {
                    assert_eq!(
                        canvas.pixel(detail_x as u32 + x, detail_y + y),
                        cached.detail_pixmap.pixel(x, y),
                        "time and state must remain complete"
                    );
                }
            }
        }
    }

    #[test]
    fn media_reuses_title_on_ticks_and_releases_cache_without_player() {
        let mut renderer = Renderer::new(&Config::default());
        let mut media = sample_media();
        let mut canvas = Pixmap::new(800, 28).unwrap();
        renderer.draw_media(&mut canvas.as_mut(), 1, Some(&media), 0, 800);
        let title = renderer.media_cache[0].title_pixmap.data().as_ptr();
        let detail = renderer.media_cache[0].detail_pixmap.data().as_ptr();
        renderer.draw_media(&mut canvas.as_mut(), 1, Some(&media), 0, 800);
        assert_eq!(
            renderer.media_cache[0].detail_pixmap.data().as_ptr(),
            detail
        );
        media.detail = "00:50 / 02:40 (31%) [Playing]".into();
        renderer.draw_media(&mut canvas.as_mut(), 1, Some(&media), 0, 800);
        assert_eq!(renderer.media_cache[0].title_pixmap.data().as_ptr(), title);
        assert_eq!(renderer.media_cache[0].detail, media.detail);
        renderer.draw_media(&mut canvas.as_mut(), 1, None, 0, 800);
        assert!(renderer.media_cache.is_empty());
    }

    #[test]
    fn media_keeps_clock_workspaces_and_right_status_pixels_unchanged() {
        let media = sample_media();
        let workspaces = Default::default();
        for position in [ClockPosition::Center, ClockPosition::Right] {
            let config = Config {
                clock_position: position,
                weather_enabled: true,
                network_enabled: true,
                ..Config::default()
            };
            let mut renderer = Renderer::new(&config);
            for scale in [1, 2, 3] {
                let mut canvas = Pixmap::new(1600 * scale, config.height * scale).unwrap();
                let mut hits = Vec::new();
                let content = RenderContent {
                    scale,
                    clock: "12:34",
                    keyboard_layout: "EN",
                    tray: &[],
                    tray_reveal: 0.0,
                    workspaces: &workspaces,
                    tasks: &[],
                    media: None,
                };
                renderer.draw(&mut canvas.as_mut(), content, &mut hits);
                let baseline = canvas.clone();
                let clock = *hits
                    .iter()
                    .find(|hit| hit.target == HitTarget::Clock)
                    .unwrap();
                let left = if position == ClockPosition::Center {
                    hits.iter()
                        .find(|hit| hit.target == HitTarget::Weather)
                        .unwrap()
                        .x
                        + config.tray_icon_size as i32 * scale as i32
                } else {
                    config.workspace_width as i32 * WORKSPACES_PER_OUTPUT as i32 * scale as i32
                };
                let right = renderer
                    .keyboard_cache
                    .as_ref()
                    .map(|entry| {
                        let network_left =
                            (1600 - config.padding - config.tray_icon_size) as i32 * scale as i32;
                        let anchor = if position == ClockPosition::Center {
                            network_left
                        } else {
                            clock.x
                                - (config.tray_spacing + config.tray_icon_size) as i32
                                    * scale as i32
                        };
                        anchor
                            - config.tray_spacing as i32 * scale as i32
                            - entry.reserved_width as i32
                    })
                    .unwrap();
                renderer.draw(
                    &mut canvas.as_mut(),
                    RenderContent {
                        media: Some(&media),
                        ..content
                    },
                    &mut hits,
                );
                assert_ne!(canvas.data(), baseline.data());
                for y in 0..canvas.height() {
                    for x in 0..canvas.width() {
                        if (x as i32) < left || x as i32 >= right {
                            assert_eq!(canvas.pixel(x, y), baseline.pixel(x, y));
                        }
                    }
                }
                if position == ClockPosition::Center {
                    assert!(((clock.x * 2 + clock.width) - canvas.width() as i32).abs() <= 1);
                }
            }
        }
    }
}
