use std::{collections::HashMap, path::PathBuf, process::Command};

use cosmic_text::{
    Align, Attrs, Buffer, Color as TextColor, Family, FontSystem, Metrics, Shaping, SwashCache,
};
use tiny_skia::{Color, Paint, Pixmap, PixmapMut, Rect, Transform};

mod menu;
pub use menu::{MenuHitbox, MenuSelection, PreparedMenu};

use crate::{
    config::Config,
    niri::{WORKSPACE_LABELS, WORKSPACES_PER_OUTPUT, WindowTask, WorkspaceSlot},
    tray::TrayIcon,
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

const KEYBOARD_TASK_GAP: i32 = 4;

fn task_right_edge(
    right_content_left: i32,
    task_spacing: i32,
    scale: u32,
    keyboard_width: i32,
) -> i32 {
    let scale = scale.max(1) as i32;
    let keyboard_gap = if keyboard_width > 0 {
        KEYBOARD_TASK_GAP * scale
    } else {
        0
    };
    right_content_left - task_spacing * scale - keyboard_gap
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitTarget {
    Workspace { id: Option<u64>, index: u8 },
    Window(u64),
    Tray(usize),
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
    pub workspaces: &'a [WorkspaceSlot; WORKSPACES_PER_OUTPUT],
    pub tasks: &'a [WindowTask],
}

struct Style {
    font_family: Box<str>,
    font_size: f32,
    background: Color,
    foreground: [u8; 4],
    padding: u32,
    tray_icon_size: u32,
    tray_spacing: u32,
    workspace_width: u32,
    workspace_focused_background: Color,
    workspace_active_background: Color,
    workspace_active_foreground: [u8; 4],
    workspace_occupied_foreground: [u8; 4],
    workspace_empty_foreground: [u8; 4],
    workspace_urgent_foreground: [u8; 4],
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
        let clock_x = (canvas.0 - padding - clock.0).max(0);
        let tray_width = icon_size * tray_len as i32 + spacing * tray_len.saturating_sub(1) as i32;
        let tray_left = clock_x - spacing - tray_width;
        let keyboard_width = keyboard.0;
        let keyboard_anchor = if tray_len == 0 { clock_x } else { tray_left };
        let keyboard_x = if keyboard_width == 0 {
            keyboard_anchor
        } else {
            keyboard_anchor - spacing - keyboard_width
        };

        Self {
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
    pixmap: Pixmap,
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
    icon_cache: Vec<IconBitmap>,
    workspace_cache: Vec<WorkspaceBitmap>,
    task_text_cache: Vec<TaskTextBitmap>,
    task_icon_cache: Vec<TaskIconBitmap>,
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
            icon_cache: Vec::new(),
            workspace_cache: Vec::new(),
            task_text_cache: Vec::new(),
            task_icon_cache: Vec::new(),
        }
    }

    pub fn evict_task_text(&mut self, window_id: u64) {
        self.task_text_cache.retain(|entry| entry.id != window_id);
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
            workspaces,
            tasks,
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
        let layout = BarLayout::calculate(
            (pixmap.width() as i32, pixmap.height() as i32),
            clock_size,
            keyboard_size,
            tray.len(),
            scale,
            &self.style,
        );

        let background = self.style.background;
        pixmap.fill(background);
        hitboxes.clear();
        self.draw_workspaces(pixmap, scale, workspaces, hitboxes);
        self.draw_tasks(pixmap, scale, workspaces.len(), tasks, layout, hitboxes);
        let cached_clock = &self.clock_cache[&scale].pixmap;
        draw_premultiplied(
            pixmap,
            layout.clock_x,
            layout.clock_y,
            cached_clock.width(),
            cached_clock.height(),
            cached_clock.data(),
        );
        if let Some(cached_keyboard) = &self.keyboard_cache {
            draw_premultiplied(
                pixmap,
                layout.keyboard_x + layout.keyboard_width - cached_keyboard.pixmap.width() as i32,
                layout.keyboard_y,
                cached_keyboard.pixmap.width(),
                cached_keyboard.pixmap.height(),
                cached_keyboard.pixmap.data(),
            );
        }
        if tray.is_empty() {
            self.icon_cache.clear();
            return;
        }

        self.icon_cache
            .retain(|cached| tray.iter().any(|icon| icon.id == cached.id));
        hitboxes.reserve(tray.len());

        for (index, icon) in tray.iter().enumerate() {
            let hitbox = layout.hitbox(index);
            let cached_index = self.cached_icon(icon, scale, layout.icon_size as u32);
            draw_premultiplied(
                pixmap,
                hitbox.x,
                hitbox.y,
                hitbox.width as u32,
                hitbox.height as u32,
                &self.icon_cache[cached_index].pixels,
            );
            hitboxes.push(hitbox);
        }
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

    fn draw_workspaces(
        &mut self,
        pixmap: &mut PixmapMut<'_>,
        scale: u32,
        workspaces: &[WorkspaceSlot],
        hitboxes: &mut Vec<Hitbox>,
    ) {
        let width = (self.style.workspace_width * scale) as i32;
        let height = pixmap.height() as i32;

        for (offset, workspace) in workspaces.iter().enumerate() {
            let x = offset as i32 * width;
            hitboxes.push(Hitbox {
                target: HitTarget::Workspace {
                    id: workspace.id,
                    index: offset as u8 + 1,
                },
                x,
                y: 0,
                width: width.min(pixmap.width() as i32 - x).max(0),
                height,
            });
            if let Some(background) = workspace_background(&self.style, *workspace) {
                fill_rect(pixmap, x, 0, width, height, background);
            }

            let color = workspace_foreground(&self.style, *workspace);
            let cache_index = self.cached_workspace_label(workspace.index, scale, color);
            let label = &self.workspace_cache[cache_index].pixmap;
            draw_premultiplied(
                pixmap,
                x + (width - label.width() as i32) / 2,
                (height - label.height() as i32) / 2,
                label.width(),
                label.height(),
                label.data(),
            );

            if workspace.is_occupied {
                let indicator_width = (width / 3).max(scale as i32 * 4);
                fill_rect(
                    pixmap,
                    x + (width - indicator_width) / 2,
                    height - (2 * scale) as i32,
                    indicator_width,
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
                let label = WORKSPACE_LABELS[index.saturating_sub(1) as usize];
                let mut pixmap = self.rasterize_text(label, scale, color);
                if !has_visible_pixels(&pixmap) {
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
        workspace_count: usize,
        tasks: &[WindowTask],
        bar: BarLayout,
        hitboxes: &mut Vec<Hitbox>,
    ) {
        let scale_i32 = scale as i32;
        let left = self.style.workspace_width as i32 * workspace_count as i32 * scale_i32;
        let right_content_left = if bar.keyboard_width > 0 {
            bar.keyboard_x
        } else if bar.tray_left < bar.clock_x - bar.spacing {
            bar.tray_left
        } else {
            bar.clock_x
        };
        let right = task_right_edge(
            right_content_left,
            self.style.task_spacing as i32,
            scale,
            bar.keyboard_width,
        );
        let Some(layout) = TaskLayout::calculate(
            left,
            right,
            pixmap.height() as i32,
            tasks.len(),
            self.style.task_spacing as i32 * scale_i32,
        ) else {
            return;
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
            let icon_x = rect.x + padding;
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

            let color = task_foreground(&self.style, task);
            let label_index = self.cached_task_label(task, scale, color);
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
                    width: (rect.x + rect.width - padding - label_x).max(0),
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

    fn cached_task_label(&mut self, task: &WindowTask, scale: u32, color: [u8; 4]) -> usize {
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
            pixmap,
        });
        self.task_text_cache.len() - 1
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
    use std::sync::Arc;

    use super::*;
    use crate::{niri::ApplicationIcon, tray::TrayMenuEntry};

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
    fn tasks_leave_four_extra_pixels_before_keyboard_layout() {
        assert_eq!(task_right_edge(400, 2, 1, 80), 394);
        assert_eq!(task_right_edge(400, 2, 1, 0), 398);
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
            workspace_width: 28,
            workspace_focused_background: Color::BLACK,
            workspace_active_background: Color::BLACK,
            workspace_active_foreground: [255; 4],
            workspace_occupied_foreground: [200; 4],
            workspace_empty_foreground: [100; 4],
            workspace_urgent_foreground: [255, 0, 0, 255],
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
            workspace_width: 28,
            workspace_focused_background: Color::BLACK,
            workspace_active_background: Color::BLACK,
            workspace_active_foreground: [255; 4],
            workspace_occupied_foreground: [200; 4],
            workspace_empty_foreground: [100; 4],
            workspace_urgent_foreground: [255, 0, 0, 255],
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
    fn task_text_eviction_removes_every_scale_and_keeps_capacity() {
        let mut renderer = Renderer::new(&Config::default());
        renderer.task_text_cache.reserve(8);
        for (id, scale) in [(7, 1), (7, 2), (9, 1)] {
            renderer.task_text_cache.push(TaskTextBitmap {
                id,
                label: format!("window-{id}"),
                scale,
                color: [255; 4],
                pixmap: Pixmap::new(1, 1).unwrap(),
            });
        }
        let capacity = renderer.task_text_cache.capacity();

        renderer.evict_task_text(7);

        assert_eq!(
            renderer
                .task_text_cache
                .iter()
                .map(|entry| (entry.id, entry.scale))
                .collect::<Vec<_>>(),
            [(9, 1)]
        );
        assert_eq!(renderer.task_text_cache.capacity(), capacity);
    }

    #[test]
    fn task_list_starts_immediately_after_workspaces() {
        let config = Config {
            workspace_width: 24,
            task_spacing: 2,
            task_focused_background: "#123456".into(),
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
                workspaces: &workspaces,
                tasks: &tasks,
            },
            &mut hitboxes,
        );

        let task_start = (config.workspace_width * WORKSPACES_PER_OUTPUT as u32 * 4) as usize;
        assert_eq!(
            &pixmap.data()[task_start..task_start + 4],
            &[0x12, 0x34, 0x56, 255]
        );
        assert_eq!(hitboxes.len(), WORKSPACES_PER_OUTPUT + tasks.len());
        assert_eq!(hitboxes[WORKSPACES_PER_OUTPUT].x, task_start as i32 / 4);
    }

    #[test]
    fn workspaces_tasks_and_tray_are_drawn_in_separate_regions() {
        let config = Config {
            workspace_focused_background: "#654321".into(),
            task_focused_background: "#123456".into(),
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
                workspaces: &workspaces,
                tasks: &tasks,
            },
            &mut hitboxes,
        );

        assert_eq!(&pixmap.data()[..4], &[0x65, 0x43, 0x21, 255]);
        let task_offset = (config.workspace_width * WORKSPACES_PER_OUTPUT as u32 * 4) as usize;
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
            WORKSPACES_PER_OUTPUT + tasks.len() + tray.len()
        );
        assert_eq!(hitboxes.last().unwrap().target, HitTarget::Tray(0));
    }

    #[test]
    fn rendered_buttons_hit_the_correct_targets_at_each_scale() {
        let config = Config::default();
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
                workspaces: &workspaces,
                tasks: &tasks,
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
            assert_eq!(hitboxes.len(), WORKSPACES_PER_OUTPUT);
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
}
