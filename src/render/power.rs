use super::fitted_icon_transform;
use tiny_skia::{Paint, PathBuilder, Pixmap, Stroke};

pub(super) fn icon(size: u32, color: [u8; 4]) -> Pixmap {
    if size <= 16 {
        return super::pixel_icons::icon(size, color, super::pixel_icons::Shape::Power);
    }

    let mut pixmap = Pixmap::new(size, size).expect("small power icon");
    let mut path = PathBuilder::new();
    path.move_to(11.5, 4.0);
    path.cubic_to(13.5, 5.1, 14.5, 7.1, 14.5, 9.0);
    path.cubic_to(14.5, 12.6, 11.6, 15.0, 8.0, 15.0);
    path.cubic_to(4.4, 15.0, 1.5, 12.6, 1.5, 9.0);
    path.cubic_to(1.5, 7.1, 2.5, 5.1, 4.5, 4.0);
    path.move_to(8.0, 1.0);
    path.line_to(8.0, 8.0);
    let path = path.finish().expect("power icon path");
    let mut paint = Paint::default();
    paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
    pixmap.stroke_path(
        &path,
        &paint,
        &Stroke {
            width: 1.7,
            ..Stroke::default()
        },
        fitted_icon_transform(&path, size, 1.7),
        None,
    );
    pixmap
}
