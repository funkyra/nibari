pub use super::glyphs::Glyph;
use super::{HEADER, INSET, PreparedBluetooth, UiAction, intersection};
use crate::{
    bluetooth::Action,
    config::Config,
    render::{MenuHitbox, MenuSelection, PixelRect, Renderer, draw_premultiplied_clipped},
};
use std::sync::Arc;
use tiny_skia::{Color, FillRule, Mask, Paint, Path, PathBuilder, Pixmap, PixmapMut, Transform};

#[derive(Clone, Copy)]
pub struct Palette {
    pub background: Color,
    pub foreground: [u8; 4],
    pub accent: [u8; 4],
    pub switch_foreground: [u8; 4],
    pub muted: [u8; 4],
    pub border: Color,
    pub hover: Color,
    pub on: Color,
    pub connected: [u8; 4],
}

impl Palette {
    pub fn new(config: &Config) -> Self {
        let bg = config.color_rgba(&config.bluetooth.background);
        let foreground = config.color_rgba(&config.bluetooth.foreground);
        let accent = config.color_rgba(&config.bluetooth.accent);
        let switch_foreground =
            if u32::from(accent[0]) * 299 + u32::from(accent[1]) * 587 + u32::from(accent[2]) * 114
                >= 128_000
            {
                [24, 24, 24, 255]
            } else {
                [248, 248, 248, 255]
            };
        Self {
            background: rgba(bg),
            foreground,
            accent,
            switch_foreground,
            muted: config.color_rgba(&config.bluetooth.muted),
            border: rgba(config.color_rgba(&config.bluetooth.border)),
            hover: rgba(blend(bg, accent, 0.12)),
            on: rgba(blend(bg, accent, 0.24)),
            connected: accent,
        }
    }
}

fn blend(a: [u8; 4], b: [u8; 4], weight: f32) -> [u8; 4] {
    std::array::from_fn(|i| {
        if i == 3 {
            255
        } else {
            (a[i] as f32 * (1.0 - weight) + b[i] as f32 * weight) as u8
        }
    })
}

pub fn rgba(c: [u8; 4]) -> Color {
    Color::from_rgba8(c[0], c[1], c[2], c[3])
}

pub(super) enum Primitive {
    Rounded {
        radius: f32,
        fill: Color,
        border: Option<Color>,
    },
    Sprite {
        image: Arc<Pixmap>,
        x: i32,
        y: i32,
    },
}

pub(super) struct Element {
    pub rect: PixelRect,
    pub fixed: bool,
    pub primitive: Primitive,
}

struct TextEntry {
    text: String,
    size: f32,
    scale: u32,
    color: [u8; 4],
    used: bool,
    bitmap: Arc<Pixmap>,
}
struct IconEntry {
    glyph: Glyph,
    size: u32,
    color: [u8; 4],
    bitmap: Arc<Pixmap>,
}

#[derive(Default)]
pub struct Cache {
    text: Vec<TextEntry>,
    icons: Vec<IconEntry>,
}

impl Cache {
    pub fn begin(&mut self) {
        for entry in &mut self.text {
            entry.used = false;
        }
    }

    pub fn finish(&mut self) {
        self.text.retain(|e| e.used);
    }
}

impl Renderer {
    pub(super) fn bluetooth_text(
        &mut self,
        label: &str,
        size: f32,
        scale: u32,
        color: [u8; 4],
    ) -> Arc<Pixmap> {
        if let Some(entry) = self
            .bluetooth_cache
            .text
            .iter_mut()
            .find(|e| e.text == label && e.size == size && e.scale == scale && e.color == color)
        {
            entry.used = true;
            return entry.bitmap.clone();
        }
        let clean: String = label
            .chars()
            .take(256)
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let bitmap = Arc::new(self.rasterize_text_with_size(&clean, scale, color, size));
        self.bluetooth_cache.text.push(TextEntry {
            text: label.to_owned(),
            size,
            scale,
            color,
            used: true,
            bitmap: bitmap.clone(),
        });
        bitmap
    }

    pub(super) fn bluetooth_glyph(
        &mut self,
        glyph: Glyph,
        size: u32,
        color: [u8; 4],
    ) -> Arc<Pixmap> {
        if let Some(entry) = self
            .bluetooth_cache
            .icons
            .iter()
            .find(|e| e.glyph == glyph && e.size == size && e.color == color)
        {
            return entry.bitmap.clone();
        }
        let bitmap = glyph.rasterize(size, color);
        let bitmap = Arc::new(bitmap);
        self.bluetooth_cache.icons.push(IconEntry {
            glyph,
            size,
            color,
            bitmap: bitmap.clone(),
        });
        bitmap
    }

    pub fn draw_bluetooth(
        &mut self,
        canvas: &mut PixmapMut<'_>,
        popup: &PreparedBluetooth,
        hitboxes: &mut Vec<MenuHitbox>,
        selected: Option<MenuSelection>,
    ) {
        let s = popup.scale as i32;
        canvas.fill(Color::TRANSPARENT);
        let rect = PixelRect {
            x: INSET * s,
            y: INSET * s,
            width: popup.width as i32 - 2 * INSET * s,
            height: popup.height as i32 - 2 * INSET * s,
        };
        rounded(canvas, rect, 11.0 * s as f32, popup.palette.border, None);
        rounded(
            canvas,
            PixelRect {
                x: rect.x + s,
                y: rect.y + s,
                width: rect.width - 2 * s,
                height: rect.height - 2 * s,
            },
            10.0 * s as f32,
            popup.palette.background,
            None,
        );
        hitboxes.clear();
        hitboxes.extend_from_slice(&popup.hitboxes);
        for fixed in [false, true] {
            let clip = popup.clip(fixed);
            let mut mask = Mask::new(canvas.width(), canvas.height()).expect("popup mask");
            if let Some(rect) = tiny_skia::Rect::from_xywh(
                clip.x as f32,
                clip.y as f32,
                clip.width as f32,
                clip.height as f32,
            ) {
                mask.fill_path(
                    &PathBuilder::from_rect(rect),
                    FillRule::Winding,
                    false,
                    Transform::identity(),
                );
            }
            for element in popup.elements.iter().filter(|e| e.fixed == fixed) {
                let offset = if fixed { 0 } else { popup.scroll };
                let rect = PixelRect {
                    y: element.rect.y - offset,
                    ..element.rect
                };
                let Some(clipped) = intersection(rect, clip) else {
                    continue;
                };
                match &element.primitive {
                    Primitive::Rounded {
                        radius,
                        fill,
                        border,
                    } => {
                        if let Some(border) = border {
                            rounded(canvas, rect, *radius, *border, Some(&mask));
                            rounded(
                                canvas,
                                PixelRect {
                                    x: rect.x + s,
                                    y: rect.y + s,
                                    width: rect.width - 2 * s,
                                    height: rect.height - 2 * s,
                                },
                                (*radius - s as f32).max(0.0),
                                *fill,
                                Some(&mask),
                            );
                        } else {
                            rounded(canvas, rect, *radius, *fill, Some(&mask));
                        }
                    }
                    Primitive::Sprite { image, x, y } => draw_premultiplied_clipped(
                        canvas,
                        *x,
                        *y - offset,
                        image.width(),
                        image.height(),
                        image.data(),
                        clipped,
                    ),
                }
            }
            if let Some(MenuSelection::Item(id)) = selected
                && let Some(index) = usize::try_from(id).ok()
                && let Some(control) = popup.controls.get(index)
                && control.fixed == fixed
                && control.enabled
            {
                let rect = popup.control_rect(control);
                // A thin outline keeps the prepared text readable on both hover and keyboard focus.
                let mut paint = Paint::default();
                paint.set_color(rgba(popup.palette.accent));
                let radius = if matches!(
                    popup.actions.get(index),
                    Some(UiAction::Act(Action::Power(..)))
                ) {
                    0.0
                } else {
                    5.0 * s as f32
                };
                if let Some(path) = rounded_path(rect, radius) {
                    canvas.stroke_path(
                        &path,
                        &paint,
                        &tiny_skia::Stroke {
                            width: s as f32,
                            ..Default::default()
                        },
                        Transform::identity(),
                        Some(&mask),
                    );
                }
            }
        }
        if popup.max_scroll() > 0 {
            let track = (popup.height as i32 - HEADER * s - INSET * s).max(1);
            let size = (track as f32 * track as f32 / (track + popup.max_scroll()) as f32)
                .max(18.0 * s as f32) as i32;
            let y = HEADER * s + (track - size) * popup.scroll / popup.max_scroll();
            rounded(
                canvas,
                PixelRect {
                    x: popup.width as i32 - 18 * s,
                    y,
                    width: 2 * s,
                    height: size,
                },
                s as f32,
                rgba(popup.palette.muted),
                None,
            );
        }
    }
}

fn rounded_path(rect: PixelRect, radius: f32) -> Option<Path> {
    if rect.width <= 0 || rect.height <= 0 {
        return None;
    }
    let x = rect.x as f32;
    let y = rect.y as f32;
    let w = rect.width as f32;
    let h = rect.height as f32;
    let r = radius.min(w / 2.0).min(h / 2.0);
    let mut p = PathBuilder::new();
    p.move_to(x + r, y);
    p.line_to(x + w - r, y);
    p.quad_to(x + w, y, x + w, y + r);
    p.line_to(x + w, y + h - r);
    p.quad_to(x + w, y + h, x + w - r, y + h);
    p.line_to(x + r, y + h);
    p.quad_to(x, y + h, x, y + h - r);
    p.line_to(x, y + r);
    p.quad_to(x, y, x + r, y);
    p.close();
    p.finish()
}

fn rounded(
    canvas: &mut PixmapMut<'_>,
    rect: PixelRect,
    radius: f32,
    color: Color,
    mask: Option<&Mask>,
) {
    if let Some(path) = rounded_path(rect, radius) {
        let mut paint = Paint::default();
        paint.set_color(color);
        canvas.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            mask,
        );
    }
}
