use tiny_skia::{Color, FillRule, Paint, PathBuilder, PixmapMut, Stroke, Transform};

use super::PixelRect;

pub(super) const CARD_INSET: i32 = 8;
const CARD_RADIUS: f32 = 8.0;

pub(super) fn draw_card(
    canvas: &mut PixmapMut<'_>,
    scale: u32,
    background: Color,
    border: Color,
) -> Option<PixelRect> {
    let s = scale.max(1) as i32;
    let inset = CARD_INSET * s;
    let rect = PixelRect {
        x: inset,
        y: inset,
        width: canvas.width() as i32 - 2 * inset,
        height: canvas.height() as i32 - 2 * inset,
    };
    if rect.width <= s || rect.height <= s {
        return None;
    }

    let half_stroke = s as f32 / 2.0;
    let x = rect.x as f32 + half_stroke;
    let y = rect.y as f32 + half_stroke;
    let w = rect.width as f32 - 2.0 * half_stroke;
    let h = rect.height as f32 - 2.0 * half_stroke;
    let radius = (CARD_RADIUS * s as f32 - half_stroke)
        .min(w / 2.0)
        .min(h / 2.0);
    let control = radius * 0.552_284_8;
    let mut path = PathBuilder::new();
    path.move_to(x + radius, y);
    path.line_to(x + w - radius, y);
    path.cubic_to(
        x + w - radius + control,
        y,
        x + w,
        y + radius - control,
        x + w,
        y + radius,
    );
    path.line_to(x + w, y + h - radius);
    path.cubic_to(
        x + w,
        y + h - radius + control,
        x + w - radius + control,
        y + h,
        x + w - radius,
        y + h,
    );
    path.line_to(x + radius, y + h);
    path.cubic_to(
        x + radius - control,
        y + h,
        x,
        y + h - radius + control,
        x,
        y + h - radius,
    );
    path.line_to(x, y + radius);
    path.cubic_to(
        x,
        y + radius - control,
        x + radius - control,
        y,
        x + radius,
        y,
    );
    path.close();

    let path = path.finish()?;
    let mut paint = Paint::default();
    paint.set_color(background);
    canvas.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    paint.set_color(border);
    canvas.stroke_path(
        &path,
        &paint,
        &Stroke {
            width: s as f32,
            ..Stroke::default()
        },
        Transform::identity(),
        None,
    );
    Some(rect)
}
