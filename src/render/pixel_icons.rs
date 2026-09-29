use crate::{network::NetworkKind, weather::Condition};
use tiny_skia::Pixmap;

const BLUETOOTH: [u16; 16] = [
    0x0000, 0x0100, 0x0700, 0x0900, 0x1110, 0x0920, 0x0540, 0x0380, 0x0380, 0x0540, 0x0920, 0x1110,
    0x0900, 0x0700, 0x0100, 0x0000,
];

const ETHERNET: [u16; 16] = [
    0x0000, 0x0000, 0x3ffc, 0x2554, 0x2554, 0x2554, 0x2554, 0x2004, 0x2004, 0x2004, 0x27e4, 0x2424,
    0x2424, 0x3ffc, 0x0000, 0x0000,
];

const CLIPBOARD: [u16; 16] = [
    0x0000, 0x0380, 0x0fe0, 0x3838, 0x2fe8, 0x2008, 0x2008, 0x2008, 0x2fe8, 0x2008, 0x2008, 0x27e8,
    0x2008, 0x2008, 0x3ff8, 0x0000,
];

const CLOUD: [u16; 16] = [
    0x0000, 0x0000, 0x0000, 0x0000, 0x0380, 0x0c40, 0x1020, 0x3020, 0x4038, 0x4004, 0x4004, 0x2004,
    0x3ff8, 0x0000, 0x0000, 0x0000,
];

pub(super) enum Shape {
    Bluetooth { connected: bool },
    Network(NetworkKind),
    Clipboard,
    Power,
    Weather { condition: Condition, night: bool },
}

pub(super) fn icon(size: u32, color: [u8; 4], shape: Shape) -> Pixmap {
    let size = size.clamp(1, 16);
    let mask = mask(shape);
    let mut icon = Pixmap::new(size, size).expect("small pixel icon");
    let alpha = color[3] as u32;
    let pixel = [
        ((color[0] as u32 * alpha + 127) / 255) as u8,
        ((color[1] as u32 * alpha + 127) / 255) as u8,
        ((color[2] as u32 * alpha + 127) / 255) as u8,
        color[3],
    ];
    for y in 0..size {
        let top = y * 16 / size;
        let bottom = (y + 1) * 16 / size;
        for x in 0..size {
            let left = x * 16 / size;
            let right = (x + 1) * 16 / size;
            if (top..bottom).any(|source_y| {
                (left..right).any(|source_x| mask[source_y as usize] & (1 << source_x) != 0)
            }) {
                let offset = ((y * size + x) * 4) as usize;
                icon.data_mut()[offset..offset + 4].copy_from_slice(&pixel);
            }
        }
    }
    icon
}

fn mask(shape: Shape) -> [u16; 16] {
    match shape {
        Shape::Bluetooth { connected } => {
            let mut icon = BLUETOOTH;
            if connected {
                line(&mut icon, 2, 7, 2, 9);
                line(&mut icon, 14, 7, 14, 9);
            }
            icon
        }
        Shape::Network(NetworkKind::Ethernet) => ETHERNET,
        Shape::Network(NetworkKind::Offline) => {
            let mut icon = ETHERNET;
            for y in 1..15 {
                let x = 15 - y;
                point(&mut icon, x - 1, y, false);
                point(&mut icon, x, y, false);
                point(&mut icon, x + 1, y, false);
            }
            line(&mut icon, 2, 14, 14, 2);
            icon
        }
        Shape::Network(NetworkKind::Wifi) => {
            let mut icon = [0; 16];
            line(&mut icon, 1, 6, 4, 3);
            line(&mut icon, 4, 3, 11, 3);
            line(&mut icon, 11, 3, 14, 6);
            line(&mut icon, 4, 9, 6, 7);
            line(&mut icon, 6, 7, 9, 7);
            line(&mut icon, 9, 7, 11, 9);
            line(&mut icon, 7, 12, 8, 12);
            icon
        }
        Shape::Network(NetworkKind::Vpn) => {
            let mut icon = [0; 16];
            line(&mut icon, 8, 1, 13, 3);
            line(&mut icon, 13, 3, 12, 10);
            line(&mut icon, 12, 10, 8, 14);
            line(&mut icon, 8, 14, 4, 10);
            line(&mut icon, 4, 10, 3, 3);
            line(&mut icon, 3, 3, 8, 1);
            line(&mut icon, 5, 8, 7, 10);
            line(&mut icon, 7, 10, 11, 6);
            icon
        }
        Shape::Clipboard => CLIPBOARD,
        Shape::Power => {
            let mut icon = [0; 16];
            line(&mut icon, 7, 1, 7, 7);
            line(&mut icon, 8, 1, 8, 7);
            for (y, left, right) in [
                (3, 4, 11),
                (4, 3, 12),
                (5, 2, 13),
                (6, 2, 13),
                (7, 2, 13),
                (8, 2, 13),
                (9, 2, 13),
                (10, 3, 12),
                (11, 4, 11),
                (12, 5, 10),
            ] {
                point(&mut icon, left, y, true);
                point(&mut icon, right, y, true);
            }
            line(&mut icon, 6, 13, 9, 13);
            icon
        }
        Shape::Weather { condition, night } => weather_mask(condition, night),
    }
}

fn weather_mask(condition: Condition, night: bool) -> [u16; 16] {
    match condition {
        Condition::Unknown => {
            let mut icon = [0; 16];
            line(&mut icon, 5, 5, 6, 3);
            line(&mut icon, 6, 3, 10, 3);
            line(&mut icon, 10, 3, 11, 5);
            line(&mut icon, 11, 5, 8, 8);
            line(&mut icon, 8, 8, 8, 10);
            point(&mut icon, 8, 13, true);
            icon
        }
        Condition::Clear if night => {
            let mut icon = [0; 16];
            line(&mut icon, 10, 2, 7, 3);
            line(&mut icon, 7, 3, 5, 5);
            line(&mut icon, 5, 5, 4, 8);
            line(&mut icon, 4, 8, 5, 11);
            line(&mut icon, 5, 11, 7, 13);
            line(&mut icon, 7, 13, 10, 14);
            line(&mut icon, 10, 14, 8, 11);
            line(&mut icon, 8, 11, 7, 8);
            line(&mut icon, 7, 8, 8, 5);
            line(&mut icon, 8, 5, 10, 2);
            icon
        }
        Condition::Clear => {
            let mut icon = [0; 16];
            for (x, y) in [
                (8, 1),
                (8, 2),
                (8, 13),
                (8, 14),
                (1, 8),
                (2, 8),
                (13, 8),
                (14, 8),
                (3, 3),
                (12, 3),
                (3, 12),
                (12, 12),
            ] {
                point(&mut icon, x, y, true);
            }
            for (x1, y1, x2, y2) in [
                (8, 5, 10, 6),
                (10, 6, 11, 8),
                (11, 8, 10, 10),
                (10, 10, 8, 11),
                (8, 11, 6, 10),
                (6, 10, 5, 8),
                (5, 8, 6, 6),
                (6, 6, 8, 5),
            ] {
                line(&mut icon, x1, y1, x2, y2);
            }
            icon
        }
        Condition::PartlyCloudy => {
            let mut icon = CLOUD;
            if night {
                line(&mut icon, 12, 1, 10, 2);
                line(&mut icon, 10, 2, 10, 4);
                line(&mut icon, 10, 4, 12, 5);
                line(&mut icon, 12, 5, 14, 3);
            } else {
                line(&mut icon, 10, 2, 12, 1);
                line(&mut icon, 12, 1, 14, 3);
                line(&mut icon, 14, 3, 13, 5);
                point(&mut icon, 8, 2, true);
                point(&mut icon, 14, 1, true);
            }
            icon
        }
        Condition::Cloudy => CLOUD,
        Condition::Fog => {
            let mut icon = CLOUD;
            line(&mut icon, 3, 14, 13, 14);
            line(&mut icon, 5, 15, 11, 15);
            icon
        }
        Condition::Rain => {
            let mut icon = shift_cloud_up();
            for x in [5, 9, 13] {
                line(&mut icon, x, 12, x - 1, 15);
            }
            icon
        }
        Condition::Snow => {
            let mut icon = shift_cloud_up();
            for x in [5, 11] {
                line(&mut icon, x - 1, 13, x + 1, 15);
                line(&mut icon, x + 1, 13, x - 1, 15);
            }
            icon
        }
        Condition::Thunder => {
            let mut icon = shift_cloud_up();
            line(&mut icon, 9, 11, 6, 14);
            line(&mut icon, 6, 14, 10, 14);
            line(&mut icon, 10, 14, 8, 15);
            icon
        }
    }
}

fn shift_cloud_up() -> [u16; 16] {
    let mut icon = [0; 16];
    icon[..13].copy_from_slice(&CLOUD[3..]);
    icon
}

fn point(icon: &mut [u16; 16], x: i32, y: i32, on: bool) {
    if (0..16).contains(&x) && (0..16).contains(&y) {
        let bit = 1 << x;
        if on {
            icon[y as usize] |= bit;
        } else {
            icon[y as usize] &= !bit;
        }
    }
}

fn line(icon: &mut [u16; 16], mut x: i32, mut y: i32, end_x: i32, end_y: i32) {
    let dx = (end_x - x).abs();
    let dy = -(end_y - y).abs();
    let step_x = if x < end_x { 1 } else { -1 };
    let step_y = if y < end_y { 1 } else { -1 };
    let mut error = dx + dy;
    loop {
        point(icon, x, y, true);
        if x == end_x && y == end_y {
            break;
        }
        let doubled = error * 2;
        if doubled >= dy {
            error += dy;
            x += step_x;
        }
        if doubled <= dx {
            error += dx;
            y += step_y;
        }
    }
}
