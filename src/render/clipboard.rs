use super::fitted_icon_transform;
use tiny_skia::{Paint, PathBuilder, Pixmap, Stroke};

pub(super) fn icon(size: u32, color: [u8; 4]) -> Pixmap {
    if size <= 16 {
        return super::pixel_icons::icon(size, color, super::pixel_icons::Shape::Clipboard);
    }
    let mut pixmap = Pixmap::new(size.max(1), size.max(1)).expect("small clipboard icon");
    let mut path = PathBuilder::new();
    path.move_to(6.0, 4.0);
    path.line_to(4.0, 4.0);
    path.quad_to(3.0, 4.0, 3.0, 5.0);
    path.line_to(3.0, 15.0);
    path.quad_to(3.0, 16.0, 4.0, 16.0);
    path.line_to(14.0, 16.0);
    path.quad_to(15.0, 16.0, 15.0, 15.0);
    path.line_to(15.0, 5.0);
    path.quad_to(15.0, 4.0, 14.0, 4.0);
    path.line_to(12.0, 4.0);
    path.move_to(6.0, 3.0);
    path.line_to(7.5, 3.0);
    path.quad_to(7.5, 2.0, 9.0, 2.0);
    path.quad_to(10.5, 2.0, 10.5, 3.0);
    path.line_to(12.0, 3.0);
    path.line_to(12.0, 5.0);
    path.line_to(6.0, 5.0);
    path.close();
    path.move_to(6.0, 9.0);
    path.line_to(12.0, 9.0);
    path.move_to(6.0, 12.0);
    path.line_to(11.0, 12.0);

    let mut paint = Paint::default();
    paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
    let path = path.finish().expect("clipboard icon path");
    pixmap.stroke_path(
        &path,
        &paint,
        &Stroke {
            width: 1.4,
            ..Stroke::default()
        },
        fitted_icon_transform(&path, size, 1.4),
        None,
    );
    pixmap
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{network::NetworkKind, render::network::network_icon};

    fn occupied_rows(pixmap: &Pixmap) -> (usize, usize) {
        let mut first = None;
        let mut last = 0;
        for (y, row) in pixmap
            .data()
            .chunks_exact(pixmap.width() as usize * 4)
            .enumerate()
        {
            if row.chunks_exact(4).any(|pixel| pixel[3] != 0) {
                first.get_or_insert(y);
                last = y;
            }
        }
        (first.unwrap(), last)
    }

    #[test]
    fn clipboard_icon_aligns_vertically_with_network_icon() {
        for size in [16, 18, 36, 54] {
            let clipboard = icon(size, [255; 4]);
            let network = network_icon(NetworkKind::Ethernet, size, [255; 4]);
            let (clipboard_top, clipboard_bottom) = occupied_rows(&clipboard);
            let (network_top, network_bottom) = occupied_rows(&network);
            assert_eq!(
                clipboard_top + clipboard_bottom,
                network_top + network_bottom,
                "misaligned at {size}px"
            );
        }
    }
}
