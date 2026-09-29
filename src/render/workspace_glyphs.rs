use tiny_skia::Pixmap;

const GRID: u32 = 16;

pub(super) fn glyph(index: u8, font_size: f32, scale: u32, color: [u8; 4]) -> Pixmap {
    let size = (font_size.round() as u32)
        .max(1)
        .saturating_mul(scale.max(1));
    let mask = mask(index);
    let mut pixmap = Pixmap::new(size, size).expect("workspace glyph");
    let alpha = color[3] as u32;
    let pixel = [
        ((color[0] as u32 * alpha + 127) / 255) as u8,
        ((color[1] as u32 * alpha + 127) / 255) as u8,
        ((color[2] as u32 * alpha + 127) / 255) as u8,
        color[3],
    ];

    for y in 0..size {
        let top = y * GRID / size;
        let bottom = ((y + 1) * GRID / size).max(top + 1);
        for x in 0..size {
            let left = x * GRID / size;
            let right = ((x + 1) * GRID / size).max(left + 1);
            let filled = (top..bottom).any(|source_y| {
                (left..right).any(|source_x| mask[source_y as usize] & (1 << source_x) != 0)
            });
            if filled {
                let offset = ((y * size + x) * 4) as usize;
                pixmap.data_mut()[offset..offset + 4].copy_from_slice(&pixel);
            }
        }
    }
    pixmap
}

fn mask(index: u8) -> [u16; GRID as usize] {
    let mut rows = [0; GRID as usize];
    // Workspace indices 1..=10 represent 一 二 三 四 五 六 七 八 九 零.
    match index {
        1 => line(&mut rows, 2, 8, 13, 8),
        2 => {
            line(&mut rows, 4, 5, 11, 5);
            line(&mut rows, 2, 11, 13, 11);
        }
        3 => {
            line(&mut rows, 4, 3, 11, 3);
            line(&mut rows, 3, 8, 12, 8);
            line(&mut rows, 2, 13, 13, 13);
        }
        4 => {
            line(&mut rows, 2, 2, 13, 2);
            line(&mut rows, 2, 2, 2, 13);
            line(&mut rows, 13, 2, 13, 13);
            line(&mut rows, 2, 13, 13, 13);
            line(&mut rows, 6, 5, 6, 9);
            line(&mut rows, 9, 5, 9, 9);
            line(&mut rows, 6, 9, 4, 10);
            line(&mut rows, 9, 9, 11, 10);
        }
        5 => {
            line(&mut rows, 3, 2, 12, 2);
            line(&mut rows, 4, 7, 11, 7);
            line(&mut rows, 5, 5, 4, 11);
            line(&mut rows, 11, 7, 10, 11);
            line(&mut rows, 2, 13, 13, 13);
        }
        6 => {
            point(&mut rows, 8, 2);
            line(&mut rows, 2, 6, 13, 6);
            line(&mut rows, 6, 9, 3, 13);
            line(&mut rows, 9, 9, 12, 13);
        }
        7 => {
            line(&mut rows, 2, 5, 13, 5);
            line(&mut rows, 6, 3, 6, 11);
            line(&mut rows, 6, 11, 8, 13);
            line(&mut rows, 8, 13, 12, 13);
        }
        8 => {
            line(&mut rows, 7, 4, 3, 13);
            line(&mut rows, 9, 4, 13, 13);
        }
        9 => {
            line(&mut rows, 6, 3, 6, 8);
            line(&mut rows, 3, 6, 11, 6);
            line(&mut rows, 11, 6, 11, 11);
            line(&mut rows, 6, 8, 3, 13);
            line(&mut rows, 11, 11, 12, 13);
        }
        10 => {
            line(&mut rows, 4, 2, 11, 2);
            line(&mut rows, 7, 3, 7, 5);
            line(&mut rows, 2, 4, 13, 4);
            line(&mut rows, 2, 5, 2, 7);
            line(&mut rows, 13, 5, 13, 7);
            line(&mut rows, 5, 6, 5, 7);
            line(&mut rows, 10, 6, 10, 7);
            line(&mut rows, 7, 8, 4, 10);
            line(&mut rows, 8, 8, 11, 10);
            line(&mut rows, 2, 10, 13, 10);
            line(&mut rows, 4, 12, 11, 12);
            line(&mut rows, 7, 12, 7, 13);
            line(&mut rows, 11, 12, 10, 13);
        }
        _ => unreachable!("workspace index must be 1..=10"),
    }
    rows
}

fn point(rows: &mut [u16; GRID as usize], x: i32, y: i32) {
    rows[y as usize] |= 1 << x;
}

fn line(rows: &mut [u16; GRID as usize], x1: i32, y1: i32, x2: i32, y2: i32) {
    let dx = (x2 - x1).abs();
    let dy = -(y2 - y1).abs();
    let step_x = (x2 - x1).signum();
    let step_y = (y2 - y1).signum();
    let (mut x, mut y, mut error) = (x1, y1, dx + dy);
    loop {
        point(rows, x, y);
        if x == x2 && y == y2 {
            break;
        }
        let twice = error * 2;
        if twice >= dy {
            error += dy;
            x += step_x;
        }
        if twice <= dx {
            error += dx;
            y += step_y;
        }
    }
}
