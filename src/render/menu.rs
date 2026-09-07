use system_tray::menu::{ToggleState, ToggleType};
use tiny_skia::{FillRule, LineCap, LineJoin, PathBuilder, Stroke};

use super::{
    Color, Paint, PixelRect, Pixmap, PixmapMut, Renderer, Transform, draw_premultiplied_clipped,
    fill_rect, rgba,
};
use crate::tray::TrayMenuEntry;

const SHADOW: i32 = 12;
const INSET: i32 = 6;
const TEXT_PADDING: i32 = 10;
const INDICATOR_WIDTH: i32 = 24;
const SEPARATOR_HEIGHT: i32 = 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuSelection {
    Item(i32),
    Back,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuHitbox {
    pub selection: MenuSelection,
    pub enabled: bool,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

pub struct PreparedMenu {
    width: u32,
    height: u32,
    natural_size: (u32, u32),
    content_top: i32,
    scroll_offset: i32,
    scale: u32,
    indicator_column: bool,
    entries: Vec<PreparedMenuEntry>,
    hitboxes: Vec<MenuHitbox>,
    ellipsis: Pixmap,
    disabled_ellipsis: Pixmap,
    back_ellipsis: Pixmap,
}

struct PreparedMenuEntry {
    selection: Option<MenuSelection>,
    enabled: bool,
    y: i32,
    height: i32,
    text: Option<Pixmap>,
    toggle_type: ToggleType,
    toggle_state: ToggleState,
    submenu: bool,
}

impl PreparedMenu {
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn constrain(&mut self, width: u32, height: u32) {
        self.width = self.natural_size.0.min(width.max(1));
        self.height = self.natural_size.1.min(height.max(1));
        self.scroll_offset = self.scroll_offset.min(self.max_scroll());
        self.rebuild_hitboxes();
    }

    pub fn scroll_by(&mut self, delta: i32) -> bool {
        let offset = self
            .scroll_offset
            .saturating_add(delta)
            .clamp(0, self.max_scroll());
        if offset == self.scroll_offset {
            return false;
        }
        self.scroll_offset = offset;
        self.rebuild_hitboxes();
        true
    }

    pub fn ensure_visible(&mut self, selection: MenuSelection) -> bool {
        let Some(entry) = self
            .entries
            .iter()
            .find(|entry| entry.selection == Some(selection))
        else {
            return false;
        };
        if selection == MenuSelection::Back {
            return false;
        }
        let top = entry.y - self.scroll_offset;
        let bottom = top + entry.height;
        let viewport = self.viewport();
        let delta = if top < viewport.y {
            top - viewport.y
        } else {
            (bottom - viewport.y - viewport.height).max(0)
        };
        self.scroll_by(delta)
    }

    fn max_scroll(&self) -> i32 {
        self.natural_size.1.saturating_sub(self.height) as i32
    }

    fn viewport(&self) -> PixelRect {
        let inset = (SHADOW + INSET) * self.scale as i32;
        PixelRect {
            x: inset,
            y: self.content_top,
            width: (self.width as i32 - 2 * inset).max(0),
            height: (self.height as i32 - inset - self.content_top).max(0),
        }
    }

    fn row_rect(&self, entry: &PreparedMenuEntry) -> PixelRect {
        let inset = (SHADOW + INSET) * self.scale as i32;
        PixelRect {
            x: inset,
            y: entry.y
                - if entry.y >= self.content_top {
                    self.scroll_offset
                } else {
                    0
                },
            width: (self.width as i32 - 2 * inset).max(0),
            height: entry.height,
        }
    }

    fn row_clip(&self, entry: &PreparedMenuEntry) -> PixelRect {
        if entry.y < self.content_top {
            self.row_rect(entry)
        } else {
            self.viewport()
        }
    }

    fn rebuild_hitboxes(&mut self) {
        self.hitboxes = self
            .entries
            .iter()
            .filter_map(|entry| {
                let selection = entry.selection?;
                let row = self.row_rect(entry);
                let clip = self.row_clip(entry);
                let top = row.y.max(clip.y);
                let bottom = (row.y + row.height).min(clip.y + clip.height);
                (bottom > top && row.width > 0).then_some(MenuHitbox {
                    selection,
                    enabled: entry.enabled,
                    x: row.x,
                    y: top,
                    width: row.width,
                    height: bottom - top,
                })
            })
            .collect();
    }

    pub fn selection_at(&self, x: i32, y: i32) -> Option<MenuSelection> {
        self.hitboxes
            .iter()
            .find(|row| {
                row.enabled
                    && x >= row.x
                    && x < row.x + row.width
                    && y >= row.y
                    && y < row.y + row.height
            })
            .map(|row| row.selection)
    }

    pub fn next_selection(
        &self,
        current: Option<MenuSelection>,
        backwards: bool,
    ) -> Option<MenuSelection> {
        let choices = || {
            self.entries
                .iter()
                .filter(|row| row.enabled)
                .filter_map(|row| row.selection)
        };
        let count = choices().count();
        if count == 0 {
            return None;
        }
        let index = current.and_then(|current| choices().position(|item| item == current));
        let next = match (index, backwards) {
            (Some(index), true) => (index + count - 1) % count,
            (Some(index), false) => (index + 1) % count,
            (None, true) => count - 1,
            (None, false) => 0,
        };
        choices().nth(next)
    }
}

impl Renderer {
    pub fn prepare_menu(
        &mut self,
        entries: &[TrayMenuEntry],
        scale: u32,
        title: Option<&str>,
    ) -> PreparedMenu {
        let scale = scale.max(1);
        let s = scale as i32;
        let font_size = self.style.font_size.max(13.0);
        let row_height = ((font_size * 1.25).ceil() as i32 + 14).max(32) * s;
        let mut y = (SHADOW + INSET) * s;
        let mut rows = Vec::with_capacity(entries.len() + 2);
        let indicator_column = entries
            .iter()
            .any(|entry| entry.toggle_type != ToggleType::CannotBeToggled);

        if let Some(title) = title {
            rows.push(PreparedMenuEntry {
                selection: Some(MenuSelection::Back),
                enabled: true,
                y,
                height: row_height,
                text: Some(self.menu_text(title, scale, self.style.workspace_active_foreground)),
                toggle_type: ToggleType::CannotBeToggled,
                toggle_state: ToggleState::Off,
                submenu: false,
            });
            y += row_height;
            rows.push(separator_row(y, s));
            y += SEPARATOR_HEIGHT * s;
        }

        let content_top = y;
        // Strip edge separators and collapse consecutive separators before layout.
        let mut pending_separator = false;
        let mut has_item = false;
        for entry in entries {
            if entry.separator {
                pending_separator = has_item;
                continue;
            }
            if pending_separator {
                rows.push(separator_row(y, s));
                y += SEPARATOR_HEIGHT * s;
                pending_separator = false;
            }
            rows.push(PreparedMenuEntry {
                selection: Some(MenuSelection::Item(entry.id)),
                enabled: entry.enabled,
                y,
                height: row_height,
                text: Some(self.menu_text(
                    &entry.label,
                    scale,
                    if entry.enabled {
                        self.style.foreground
                    } else {
                        self.style.workspace_empty_foreground
                    },
                )),
                toggle_type: entry.toggle_type,
                toggle_state: entry.toggle_state,
                submenu: !entry.submenu.is_empty(),
            });
            y += row_height;
            has_item = true;
        }

        let text_width = rows
            .iter()
            .filter_map(|row| row.text.as_ref())
            .map(Pixmap::width)
            .max()
            .unwrap_or(1);
        let gutter = if indicator_column || title.is_some() {
            INDICATOR_WIDTH
        } else {
            0
        };
        let chrome = 2 * (INSET + TEXT_PADDING) + gutter + INDICATOR_WIDTH;
        let card_width = (text_width + chrome as u32 * scale).clamp(220 * scale, 360 * scale);
        let width = card_width + 2 * SHADOW as u32 * scale;
        let height = (y + (INSET + SHADOW) * s).max((2 * (SHADOW + INSET) + 16) * s) as u32;
        let hitboxes = rows
            .iter()
            .filter_map(|row| {
                row.selection.map(|selection| MenuHitbox {
                    selection,
                    enabled: row.enabled,
                    x: (SHADOW + INSET) * s,
                    y: row.y,
                    width: width as i32 - 2 * (SHADOW + INSET) * s,
                    height: row.height,
                })
            })
            .collect();

        PreparedMenu {
            width,
            height,
            natural_size: (width, height),
            content_top,
            scroll_offset: 0,
            scale,
            indicator_column,
            entries: rows,
            hitboxes,
            ellipsis: self.menu_text("…", scale, self.style.foreground),
            disabled_ellipsis: self.menu_text("…", scale, self.style.workspace_empty_foreground),
            back_ellipsis: self.menu_text("…", scale, self.style.workspace_active_foreground),
        }
    }

    fn menu_text(&mut self, text: &str, scale: u32, color: [u8; 4]) -> Pixmap {
        // DBus labels are untrusted and can be very long or contain line breaks.
        let mut label: String = text
            .chars()
            .take(256)
            .map(|ch| if ch.is_control() { ' ' } else { ch })
            .collect();
        if text.chars().nth(256).is_some() {
            label.push('…');
        }
        self.rasterize_text_with_size(&label, scale, color, self.style.font_size.max(13.0))
    }

    pub fn draw_menu(
        &mut self,
        pixmap: &mut PixmapMut<'_>,
        menu: &PreparedMenu,
        hitboxes: &mut Vec<MenuHitbox>,
        selected: Option<MenuSelection>,
    ) {
        let s = menu.scale as i32;
        let background = mix(self.style.background, rgba(self.style.foreground), 0.025);
        let foreground = rgba(self.style.foreground);
        let accent = rgba(self.style.workspace_active_foreground);
        let border = mix(background, foreground, 0.17);
        let hover = mix(self.style.workspace_focused_background, accent, 0.08);
        let card = PixelRect {
            x: SHADOW * s,
            y: SHADOW * s,
            width: menu.width as i32 - 2 * SHADOW * s,
            height: menu.height as i32 - 2 * SHADOW * s,
        };
        pixmap.fill(Color::TRANSPARENT);
        for spread in (1..=8).rev() {
            rounded_rect(
                pixmap,
                PixelRect {
                    x: card.x - spread * s,
                    y: card.y - spread * s + 2 * s,
                    width: card.width + 2 * spread * s,
                    height: card.height + 2 * spread * s,
                },
                (10 + spread) as f32 * s as f32,
                Color::from_rgba8(0, 0, 0, 4),
            );
        }
        rounded_rect(pixmap, card, 10.0 * s as f32, border);
        rounded_rect(
            pixmap,
            PixelRect {
                x: card.x + s,
                y: card.y + s,
                width: card.width - 2 * s,
                height: card.height - 2 * s,
            },
            9.0 * s as f32,
            background,
        );

        hitboxes.clear();
        hitboxes.extend_from_slice(&menu.hitboxes);
        let row_width = (card.width - 2 * INSET * s).max(1) as u32;
        let row_height = menu
            .entries
            .iter()
            .map(|entry| entry.height)
            .max()
            .unwrap_or(1)
            .max(1) as u32;
        let Some(mut row_pixmap) = Pixmap::new(row_width, row_height) else {
            return;
        };
        for entry in &menu.entries {
            let target = menu.row_rect(entry);
            let viewport = menu.row_clip(entry);
            if target.y + target.height <= viewport.y || target.y >= viewport.y + viewport.height {
                continue;
            }
            let row = PixelRect {
                x: 0,
                y: 0,
                width: target.width,
                height: entry.height,
            };
            let Some(selection) = entry.selection else {
                let line_y = target.y + target.height / 2;
                if line_y >= viewport.y && line_y + s <= viewport.y + viewport.height {
                    fill_rect(
                        pixmap,
                        target.x + TEXT_PADDING * s,
                        line_y,
                        target.width - 2 * TEXT_PADDING * s,
                        s,
                        mix(background, foreground, 0.12),
                    );
                }
                continue;
            };
            row_pixmap.fill(Color::TRANSPARENT);
            let mut row_canvas = row_pixmap.as_mut();
            if entry.enabled && selected == Some(selection) {
                rounded_rect(
                    &mut row_canvas,
                    PixelRect {
                        y: row.y + s,
                        height: row.height - 2 * s,
                        ..row
                    },
                    5.0 * s as f32,
                    hover,
                );
                rounded_rect(
                    &mut row_canvas,
                    PixelRect {
                        x: row.x + 2 * s,
                        y: row.y + (row.height - 12 * s) / 2,
                        width: 2 * s,
                        height: 12 * s,
                    },
                    s as f32,
                    accent,
                );
            }
            let back = selection == MenuSelection::Back;
            let leading = row.x + TEXT_PADDING * s;
            let center_y = row.y + row.height / 2;
            let ink = if entry.enabled {
                accent
            } else {
                rgba(self.style.workspace_empty_foreground)
            };
            if back {
                chevron(&mut row_canvas, leading + 7 * s, center_y, s, true, ink);
            } else if entry.toggle_type != ToggleType::CannotBeToggled {
                draw_toggle(
                    &mut row_canvas,
                    (leading, center_y - 7 * s),
                    s,
                    entry,
                    ink,
                    background,
                );
            }
            if entry.submenu {
                chevron(
                    &mut row_canvas,
                    row.x + row.width - 14 * s,
                    center_y,
                    s,
                    false,
                    ink,
                );
            }
            let text_x = leading
                + if menu.indicator_column || back {
                    INDICATOR_WIDTH * s
                } else {
                    0
                };
            let clip = PixelRect {
                x: text_x,
                y: row.y,
                width: row.x + row.width - (TEXT_PADDING + INDICATOR_WIDTH) * s - text_x,
                height: row.height,
            };
            if let Some(text) = &entry.text {
                let ellipsis = if back {
                    &menu.back_ellipsis
                } else if entry.enabled {
                    &menu.ellipsis
                } else {
                    &menu.disabled_ellipsis
                };
                draw_label(&mut row_canvas, text, ellipsis, clip);
            }
            draw_premultiplied_clipped(
                pixmap,
                target.x,
                target.y,
                row_width,
                row_height,
                row_pixmap.data(),
                viewport,
            );
        }
        if menu.max_scroll() > 0 {
            let viewport = menu.viewport();
            let track_height = viewport.height;
            let content_height = track_height + menu.max_scroll();
            let thumb_height = ((track_height as f64 / content_height as f64 * track_height as f64)
                as i32)
                .max(16 * s)
                .min(track_height);
            let thumb_y = viewport.y
                + ((track_height - thumb_height) as f64 * menu.scroll_offset as f64
                    / menu.max_scroll() as f64) as i32;
            rounded_rect(
                pixmap,
                PixelRect {
                    x: card.x + card.width - 4 * s,
                    y: thumb_y,
                    width: 2 * s,
                    height: thumb_height,
                },
                s as f32,
                mix(background, foreground, 0.35),
            );
        }
    }
}

fn separator_row(y: i32, scale: i32) -> PreparedMenuEntry {
    PreparedMenuEntry {
        selection: None,
        enabled: false,
        y,
        height: SEPARATOR_HEIGHT * scale,
        text: None,
        toggle_type: ToggleType::CannotBeToggled,
        toggle_state: ToggleState::Off,
        submenu: false,
    }
}

fn mix(base: Color, tint: Color, amount: f32) -> Color {
    Color::from_rgba(
        base.red() * (1.0 - amount) + tint.red() * amount,
        base.green() * (1.0 - amount) + tint.green() * amount,
        base.blue() * (1.0 - amount) + tint.blue() * amount,
        1.0,
    )
    .expect("mixed color components stay in range")
}

fn rounded_rect(pixmap: &mut PixmapMut<'_>, rect: PixelRect, radius: f32, color: Color) {
    if rect.width <= 0 || rect.height <= 0 {
        return;
    }
    let (x, y, w, h) = (
        rect.x as f32,
        rect.y as f32,
        rect.width as f32,
        rect.height as f32,
    );
    let r = radius.min(w / 2.0).min(h / 2.0);
    let control = r * 0.552_284_8;
    let mut path = PathBuilder::new();
    path.move_to(x + r, y);
    path.line_to(x + w - r, y);
    path.cubic_to(x + w - r + control, y, x + w, y + r - control, x + w, y + r);
    path.line_to(x + w, y + h - r);
    path.cubic_to(
        x + w,
        y + h - r + control,
        x + w - r + control,
        y + h,
        x + w - r,
        y + h,
    );
    path.line_to(x + r, y + h);
    path.cubic_to(x + r - control, y + h, x, y + h - r + control, x, y + h - r);
    path.line_to(x, y + r);
    path.cubic_to(x, y + r - control, x + r - control, y, x + r, y);
    path.close();
    let mut paint = Paint::default();
    paint.set_color(color);
    pixmap.fill_path(
        &path.finish().expect("rounded rectangle path"),
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

fn stroke_points(pixmap: &mut PixmapMut<'_>, points: &[(f32, f32)], scale: i32, color: Color) {
    let mut path = PathBuilder::new();
    path.move_to(points[0].0, points[0].1);
    for &(x, y) in &points[1..] {
        path.line_to(x, y);
    }
    let mut paint = Paint::default();
    paint.set_color(color);
    let stroke = Stroke {
        width: 1.5 * scale as f32,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    pixmap.stroke_path(
        &path.finish().expect("menu indicator path"),
        &paint,
        &stroke,
        Transform::identity(),
        None,
    );
}

fn chevron(pixmap: &mut PixmapMut<'_>, x: i32, y: i32, scale: i32, back: bool, color: Color) {
    let d = if back { -1.0 } else { 1.0 };
    let (x, y, s) = (x as f32, y as f32, scale as f32);
    stroke_points(
        pixmap,
        &[
            (x - 2.0 * s * d, y - 4.0 * s),
            (x + 2.0 * s * d, y),
            (x - 2.0 * s * d, y + 4.0 * s),
        ],
        scale,
        color,
    );
}

fn draw_toggle(
    pixmap: &mut PixmapMut<'_>,
    origin: (i32, i32),
    scale: i32,
    entry: &PreparedMenuEntry,
    ink: Color,
    background: Color,
) {
    let (x, y) = origin;
    let radio = entry.toggle_type == ToggleType::Radio;
    let marked = entry.toggle_state != ToggleState::Off;
    let radius = if radio { 7.0 } else { 3.0 } * scale as f32;
    rounded_rect(
        pixmap,
        PixelRect {
            x,
            y,
            width: 14 * scale,
            height: 14 * scale,
        },
        radius,
        if marked {
            ink
        } else {
            mix(background, ink, 0.5)
        },
    );
    if !marked || radio {
        rounded_rect(
            pixmap,
            PixelRect {
                x: x + scale,
                y: y + scale,
                width: 12 * scale,
                height: 12 * scale,
            },
            (radius - scale as f32).max(0.0),
            background,
        );
    }
    if entry.toggle_state == ToggleState::Indeterminate {
        fill_rect(
            pixmap,
            x + 4 * scale,
            y + 6 * scale,
            6 * scale,
            2 * scale,
            if radio { ink } else { background },
        );
    } else if marked && radio {
        rounded_rect(
            pixmap,
            PixelRect {
                x: x + 4 * scale,
                y: y + 4 * scale,
                width: 6 * scale,
                height: 6 * scale,
            },
            3.0 * scale as f32,
            ink,
        );
    } else if marked {
        let (x, y, s) = (x as f32, y as f32, scale as f32);
        stroke_points(
            pixmap,
            &[
                (x + 3.5 * s, y + 7.0 * s),
                (x + 6.0 * s, y + 9.5 * s),
                (x + 10.5 * s, y + 4.5 * s),
            ],
            scale,
            background,
        );
    }
}

fn draw_label(pixmap: &mut PixmapMut<'_>, text: &Pixmap, ellipsis: &Pixmap, clip: PixelRect) {
    let truncated = text.width() as i32 > clip.width;
    let text_clip = PixelRect {
        width: if truncated {
            (clip.width - ellipsis.width() as i32).max(0)
        } else {
            clip.width
        },
        ..clip
    };
    draw_premultiplied_clipped(
        pixmap,
        clip.x,
        clip.y + (clip.height - text.height() as i32) / 2,
        text.width(),
        text.height(),
        text.data(),
        text_clip,
    );
    if truncated {
        draw_premultiplied_clipped(
            pixmap,
            clip.x + text_clip.width,
            clip.y + (clip.height - ellipsis.height() as i32) / 2,
            ellipsis.width(),
            ellipsis.height(),
            ellipsis.data(),
            clip,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn entry(id: i32, label: &str) -> TrayMenuEntry {
        TrayMenuEntry {
            id,
            label: label.into(),
            enabled: true,
            separator: false,
            submenu: Vec::new(),
            toggle_type: ToggleType::CannotBeToggled,
            toggle_state: ToggleState::Off,
        }
    }

    #[test]
    fn constrained_menu_scrolls_to_actions_without_moving_the_back_header() {
        let mut renderer = Renderer::new(&Config::default());
        let entries: Vec<_> = (1..=40).map(|id| entry(id, "Menu action")).collect();
        let mut menu = renderer.prepare_menu(&entries, 2, Some("Settings"));
        menu.constrain(400, 500);
        assert_eq!(menu.size(), (400, 500));
        assert!(
            !menu
                .hitboxes
                .iter()
                .any(|row| row.selection == MenuSelection::Item(40))
        );
        let back = menu.hitboxes[0];
        assert!(menu.ensure_visible(MenuSelection::Item(40)));
        let last = menu
            .hitboxes
            .iter()
            .find(|row| row.selection == MenuSelection::Item(40))
            .unwrap();
        assert_eq!(
            menu.selection_at(last.x + 1, last.y + last.height / 2),
            Some(MenuSelection::Item(40))
        );
        assert_eq!(menu.hitboxes[0], back);
        assert!(!menu.scroll_by(i32::MAX));
        assert!(menu.scroll_by(i32::MIN));
        assert!(!menu.scroll_by(-1));
        assert_eq!(menu.hitboxes[0], back);
    }

    #[test]
    fn selection_skips_disabled_rows_and_shadow_at_every_scale() {
        let mut renderer = Renderer::new(&Config::default());
        for scale in [1, 2, 3] {
            let mut disabled = entry(2, "Unavailable");
            disabled.enabled = false;
            let menu = renderer.prepare_menu(
                &[entry(1, "Open"), disabled, entry(3, "Quit")],
                scale,
                Some("Settings"),
            );
            assert_eq!(menu.selection_at(0, 0), None);
            assert_eq!(menu.next_selection(None, false), Some(MenuSelection::Back));
            assert_eq!(
                menu.next_selection(Some(MenuSelection::Back), false),
                Some(MenuSelection::Item(1))
            );
            assert_eq!(
                menu.next_selection(Some(MenuSelection::Item(1)), false),
                Some(MenuSelection::Item(3))
            );
            assert_eq!(
                menu.next_selection(Some(MenuSelection::Back), true),
                Some(MenuSelection::Item(3))
            );
            for row in &menu.hitboxes {
                assert_eq!(
                    menu.selection_at(row.x + row.width / 2, row.y + row.height / 2),
                    row.enabled.then_some(row.selection)
                );
                assert_eq!(menu.selection_at(row.x + row.width, row.y), None);
            }
        }
    }

    #[test]
    fn long_menu_labels_stay_inside_the_card() {
        let mut renderer = Renderer::new(&Config::default());
        let menu = renderer.prepare_menu(
            &[entry(1, &"An extremely long menu label ".repeat(100))],
            2,
            None,
        );
        assert_eq!(menu.size().0, (360 + 2 * SHADOW as u32) * 2);
        let mut pixmap = Pixmap::new(menu.width, menu.height).unwrap();
        renderer.draw_menu(
            &mut pixmap.as_mut(),
            &menu,
            &mut Vec::new(),
            Some(MenuSelection::Item(1)),
        );
        assert_eq!(
            pixmap
                .pixel(menu.width - 1, menu.height / 2)
                .unwrap()
                .alpha(),
            0
        );
        assert_eq!(
            menu.selection_at(menu.width as i32 - 1, menu.height as i32 / 2),
            None
        );
    }

    #[test]
    fn hover_only_changes_enabled_rows() {
        let mut renderer = Renderer::new(&Config::default());
        let mut disabled = entry(2, "Unavailable");
        disabled.enabled = false;
        let menu = renderer.prepare_menu(&[entry(1, "Open"), disabled], 1, None);
        let mut plain = Pixmap::new(menu.width, menu.height).unwrap();
        let mut hover = plain.clone();
        renderer.draw_menu(&mut plain.as_mut(), &menu, &mut Vec::new(), None);
        renderer.draw_menu(
            &mut hover.as_mut(),
            &menu,
            &mut Vec::new(),
            Some(MenuSelection::Item(2)),
        );
        assert_eq!(plain.data(), hover.data());
        renderer.draw_menu(
            &mut hover.as_mut(),
            &menu,
            &mut Vec::new(),
            Some(MenuSelection::Item(1)),
        );
        assert_ne!(plain.data(), hover.data());
    }

    #[test]
    fn radio_submenu_keeps_a_back_header_and_distinct_selected_state() {
        let mut renderer = Renderer::new(&Config::default());
        let entries: Vec<_> = ["Online", "Away", "Do not disturb", "Invisible"]
            .iter()
            .enumerate()
            .map(|(index, label)| {
                let mut item = entry(index as i32 + 10, label);
                item.toggle_type = ToggleType::Radio;
                item.toggle_state = if index == 0 {
                    ToggleState::On
                } else {
                    ToggleState::Off
                };
                item
            })
            .collect();
        let menu = renderer.prepare_menu(&entries, 2, Some("Status"));
        let mut pixmap = Pixmap::new(menu.width, menu.height).unwrap();
        renderer.draw_menu(&mut pixmap.as_mut(), &menu, &mut Vec::new(), None);
        assert_eq!(menu.hitboxes[0].selection, MenuSelection::Back);
        let on = menu.hitboxes[1];
        let off = menu.hitboxes[2];
        let indicator_x = (on.x + (TEXT_PADDING + 7) * 2) as u32;
        assert_ne!(
            pixmap.pixel(indicator_x, (on.y + on.height / 2) as u32),
            pixmap.pixel(indicator_x, (off.y + off.height / 2) as u32)
        );
        if let Ok(path) = std::env::var("NIBARI_SUBMENU_PREVIEW") {
            pixmap.save_png(path).unwrap();
        }
    }

    #[test]
    fn render_menu_preview() {
        let mut renderer = Renderer::new(&Config::default());
        let mut notifications = entry(2, "Notifications");
        notifications.toggle_type = ToggleType::Checkmark;
        notifications.toggle_state = ToggleState::On;
        let mut sound = entry(3, "Message sounds");
        sound.toggle_type = ToggleType::Checkmark;
        let mut status = entry(4, "Status");
        status.submenu = vec![entry(10, "Online")];
        let mut separator = entry(0, "");
        separator.separator = true;
        let mut disabled = entry(5, "Check for updates");
        disabled.enabled = false;
        let entries = [
            entry(1, "Open application"),
            separator.clone(),
            notifications,
            sound,
            status,
            separator,
            disabled,
            entry(6, "Preferences"),
            entry(7, "Quit"),
        ];
        let menu = renderer.prepare_menu(&entries, 2, None);
        let mut pixmap = Pixmap::new(menu.width, menu.height).unwrap();
        renderer.draw_menu(
            &mut pixmap.as_mut(),
            &menu,
            &mut Vec::new(),
            Some(MenuSelection::Item(4)),
        );
        assert_eq!(menu.hitboxes.len(), 7);
        assert!(
            pixmap
                .pixels()
                .iter()
                .any(|pixel| pixel.alpha() > 0 && pixel.alpha() < 255)
        );
        if let Ok(path) = std::env::var("NIBARI_MENU_PREVIEW") {
            pixmap.save_png(path).unwrap();
        }
    }
}
