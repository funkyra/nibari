//! Small outline glyphs for the Bluetooth popup.
//! Shapes are kept in native paths so each cached bitmap needs no SVG parsing.
use tiny_skia::{LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Bluetooth,
    Headphones,
    Keyboard,
    Mouse,
    Speaker,
    Device,
    Down,
    Back,
    Rx,
    Tx,
    Battery,
}

fn rounded(p: &mut PathBuilder, x: f32, y: f32, w: f32, h: f32, r: f32) {
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
}

impl Glyph {
    pub fn rasterize(self, size: u32, color: [u8; 4]) -> Pixmap {
        let mut bitmap = Pixmap::new(size.max(1), size.max(1)).expect("Bluetooth glyph");
        let mut p = PathBuilder::new();
        match self {
            Self::Bluetooth => {
                p.move_to(7.0, 8.0);
                p.line_to(17.0, 16.0);
                p.line_to(12.0, 20.0);
                p.line_to(12.0, 4.0);
                p.line_to(17.0, 8.0);
                p.line_to(7.0, 16.0);
            }
            Self::Headphones => {
                p.move_to(4.0, 15.0);
                p.line_to(4.0, 12.0);
                p.cubic_to(4.0, 1.3, 20.0, 1.3, 20.0, 12.0);
                p.line_to(20.0, 15.0);
                rounded(&mut p, 4.0, 13.0, 5.0, 7.0, 2.0);
                rounded(&mut p, 15.0, 13.0, 5.0, 7.0, 2.0);
            }
            Self::Keyboard => {
                rounded(&mut p, 2.0, 6.0, 20.0, 12.0, 2.0);
                for x in [6.0, 10.0, 14.0, 18.0] {
                    p.push_circle(x, 10.0, 0.25);
                }
                p.push_circle(6.0, 14.0, 0.25);
                p.push_circle(18.0, 14.0, 0.25);
                p.move_to(10.0, 14.0);
                p.line_to(14.0, 14.0);
            }
            Self::Mouse => {
                rounded(&mut p, 6.0, 3.0, 12.0, 18.0, 4.0);
                p.move_to(12.0, 7.0);
                p.line_to(12.0, 11.0);
            }
            Self::Speaker => {
                rounded(&mut p, 5.0, 3.0, 14.0, 18.0, 2.0);
                p.push_circle(12.0, 14.0, 3.0);
                p.push_circle(12.0, 7.0, 0.3);
            }
            Self::Device => {
                rounded(&mut p, 3.0, 4.0, 15.0, 14.0, 1.0);
                rounded(&mut p, 13.0, 8.0, 8.0, 12.0, 1.0);
                p.move_to(16.0, 10.0);
                p.line_to(18.0, 10.0);
            }
            Self::Down => {
                p.move_to(6.0, 9.0);
                p.line_to(12.0, 15.0);
                p.line_to(18.0, 9.0);
            }
            Self::Back => {
                p.move_to(15.0, 6.0);
                p.line_to(9.0, 12.0);
                p.line_to(15.0, 18.0);
            }
            Self::Rx => {
                p.move_to(12.0, 5.0);
                p.line_to(12.0, 19.0);
                p.move_to(6.0, 13.0);
                p.line_to(12.0, 19.0);
                p.line_to(18.0, 13.0);
            }
            Self::Tx => {
                p.move_to(12.0, 19.0);
                p.line_to(12.0, 5.0);
                p.move_to(6.0, 11.0);
                p.line_to(12.0, 5.0);
                p.line_to(18.0, 11.0);
            }
            Self::Battery => {
                rounded(&mut p, 4.0, 7.0, 15.0, 10.0, 2.0);
                p.move_to(20.0, 10.0);
                p.line_to(20.0, 14.0);
                for x in [7.0, 10.0, 13.0] {
                    p.move_to(x, 10.0);
                    p.line_to(x, 14.0);
                }
            }
        }
        let mut paint = Paint::default();
        paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
        if let Some(path) = p.finish() {
            bitmap.stroke_path(
                &path,
                &paint,
                &Stroke {
                    width: 2.0,
                    line_cap: LineCap::Round,
                    line_join: LineJoin::Round,
                    ..Stroke::default()
                },
                Transform::from_scale(size as f32 / 24.0, size as f32 / 24.0),
                None,
            );
        }
        bitmap
    }
}
