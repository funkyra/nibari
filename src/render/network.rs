use super::{
    MenuHitbox, MenuSelection, PixelRect, Renderer, draw_premultiplied, draw_premultiplied_clipped,
    fill_rect,
};
use crate::{
    config::Config,
    network::{NetworkKind, NetworkSnapshot},
};
use std::net::IpAddr;
use tiny_skia::{
    Color, LineCap, LineJoin, Paint, PathBuilder, Pixmap, PixmapMut, Rect, Stroke, Transform,
};

const WIDTH: u32 = 400;
const HEIGHT: u32 = 196;

pub struct NetworkPalette {
    background: [u8; 4],
    foreground: [u8; 4],
    muted: [u8; 4],
    accent: [u8; 4],
    border: [u8; 4],
}
impl From<&Config> for NetworkPalette {
    fn from(c: &Config) -> Self {
        Self {
            background: c.color_rgba(&c.network_background),
            foreground: c.color_rgba(&c.network_foreground),
            muted: c.color_rgba(&c.network_muted),
            accent: c.color_rgba(&c.network_accent),
            border: c.color_rgba(&c.network_border),
        }
    }
}

pub struct PreparedNetwork {
    bitmap: Pixmap,
    width: u32,
    height: u32,
    scale: u32,
    refresh: MenuHitbox,
    accent: Color,
}
impl PreparedNetwork {
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    pub fn constrain(&mut self, width: u32, height: u32) {
        self.width = self.bitmap.width().min(width.max(1));
        self.height = self.bitmap.height().min(height.max(1));
    }
    fn refresh_hitbox(&self) -> Option<MenuHitbox> {
        let mut h = self.refresh;
        h.width = h.width.min(self.width as i32 - h.x);
        h.height = h.height.min(self.height as i32 - h.y);
        (h.width > 0 && h.height > 0).then_some(h)
    }
    pub fn selection_at(&self, x: i32, y: i32) -> Option<MenuSelection> {
        self.refresh_hitbox()
            .filter(|h| x >= h.x && x < h.x + h.width && y >= h.y && y < h.y + h.height)
            .map(|h| h.selection)
    }
    pub fn next_selection(&self) -> Option<MenuSelection> {
        self.refresh_hitbox().map(|h| h.selection)
    }
}

fn bytes(value: u64) -> String {
    if value < 1024 {
        return format!("{value} B");
    }
    let mut amount = value as f64;
    let mut unit = "B";
    for candidate in ["KiB", "MiB", "GiB", "TiB", "PiB", "EiB"] {
        amount /= 1024.0;
        unit = candidate;
        if amount < 1024.0 {
            break;
        }
    }
    format!("{amount:.1} {unit}")
}
fn rate(value: Option<u64>) -> String {
    value.map_or_else(|| "—".into(), |v| format!("{}/s", bytes(v)))
}

impl Renderer {
    pub fn prepare_network(
        &mut self,
        snapshot: &NetworkSnapshot,
        target: IpAddr,
        scale: u32,
        palette: &NetworkPalette,
    ) -> PreparedNetwork {
        let scale = scale.max(1);
        let s = scale as i32;
        let ip = snapshot
            .address
            .map_or_else(|| "—".into(), |v| v.to_string());
        let gateway = snapshot
            .gateway
            .map_or_else(|| "—".into(), |v| v.to_string());
        let expanded = ip.len() > 15 || gateway.len() > 15;
        let extra = if expanded { 19 } else { 0 };
        let height = HEIGHT + extra;
        let mut bitmap = Pixmap::new(WIDTH * scale, height * scale).expect("small network card");
        let mut canvas = bitmap.as_mut();
        fill_rect(
            &mut canvas,
            6 * s,
            6 * s,
            388 * s,
            (height as i32 - 12) * s,
            rgba(palette.background),
        );
        outline(
            &mut canvas,
            PixelRect {
                x: 6 * s,
                y: 6 * s,
                width: 388 * s,
                height: (height as i32 - 12) * s,
            },
            scale as f32,
            rgba(palette.border),
        );
        let icon = network_icon(snapshot.kind, 22 * scale, palette.foreground);
        draw_premultiplied(
            &mut canvas,
            18 * s,
            20 * s,
            icon.width(),
            icon.height(),
            icon.data(),
        );
        let title = match snapshot.kind {
            NetworkKind::Ethernet => "Ethernet",
            NetworkKind::Wifi => "Wi-Fi",
            NetworkKind::Vpn => "VPN",
            NetworkKind::Offline => "Disconnected",
        };
        self.network_text(
            &mut canvas,
            title,
            PixelRect {
                x: 52 * s,
                y: 17 * s,
                width: 286 * s,
                height: 21 * s,
            },
            14.0,
            palette.foreground,
            scale,
            false,
        );
        let status = if snapshot.interface.is_empty() {
            "NO ACTIVE CONNECTION".into()
        } else {
            format!(
                "{} · {}",
                if snapshot.kind == NetworkKind::Offline {
                    "OFFLINE"
                } else {
                    "CONNECTED"
                },
                snapshot.interface
            )
        };
        self.network_text(
            &mut canvas,
            &status,
            PixelRect {
                x: 52 * s,
                y: 38 * s,
                width: 286 * s,
                height: 16 * s,
            },
            10.0,
            palette.muted,
            scale,
            false,
        );
        let refresh = MenuHitbox {
            selection: MenuSelection::Item(0),
            enabled: true,
            x: 350 * s,
            y: 15 * s,
            width: 32 * s,
            height: 36 * s,
        };
        let mut path = PathBuilder::new();
        path.move_to(374.0 * scale as f32, 27.0 * scale as f32);
        path.cubic_to(
            365.0 * scale as f32,
            18.0 * scale as f32,
            353.0 * scale as f32,
            29.0 * scale as f32,
            362.0 * scale as f32,
            38.0 * scale as f32,
        );
        path.move_to(374.0 * scale as f32, 21.0 * scale as f32);
        path.line_to(374.0 * scale as f32, 28.0 * scale as f32);
        path.line_to(367.0 * scale as f32, 28.0 * scale as f32);
        stroke(&mut canvas, path, 1.5 * scale as f32, rgba(palette.accent));
        let ping = snapshot
            .ping_ms
            .map_or_else(|| "—".into(), |v| format!("{v:.0} ms"));
        let loss = snapshot
            .loss_percent
            .map_or_else(|| "—".into(), |v| format!("{v}%"));
        let rows = [
            ("Ping", ping, "Packet Loss", loss),
            (
                "Receiving",
                rate(snapshot.rx_per_second),
                "Sending",
                rate(snapshot.tx_per_second),
            ),
            (
                "Downloaded",
                bytes(snapshot.rx_bytes),
                "Uploaded",
                bytes(snapshot.tx_bytes),
            ),
            ("IP Address", ip, "Gateway", gateway),
        ];
        for (row, (left_label, left_value, right_label, right_value)) in
            rows.iter().take(if expanded { 3 } else { 4 }).enumerate()
        {
            let y = (65 + row as i32 * 19) * s;
            for (x, label, value) in [
                (20, left_label, left_value),
                (210, right_label, right_value),
            ] {
                self.network_text(
                    &mut canvas,
                    label,
                    PixelRect {
                        x: x * s,
                        y,
                        width: 80 * s,
                        height: 19 * s,
                    },
                    10.0,
                    palette.muted,
                    scale,
                    false,
                );
                self.network_text(
                    &mut canvas,
                    value,
                    PixelRect {
                        x: (x + 80) * s,
                        y,
                        width: 98 * s,
                        height: 19 * s,
                    },
                    11.0,
                    palette.foreground,
                    scale,
                    true,
                );
            }
        }
        if expanded {
            for (row, (label, value)) in [("IP Address", &rows[3].1), ("Gateway", &rows[3].3)]
                .into_iter()
                .enumerate()
            {
                let y = (122 + row as i32 * 19) * s;
                self.network_text(
                    &mut canvas,
                    label,
                    PixelRect {
                        x: 20 * s,
                        y,
                        width: 80 * s,
                        height: 19 * s,
                    },
                    10.0,
                    palette.muted,
                    scale,
                    false,
                );
                self.network_text(
                    &mut canvas,
                    value,
                    PixelRect {
                        x: 100 * s,
                        y,
                        width: 278 * s,
                        height: 19 * s,
                    },
                    11.0,
                    palette.foreground,
                    scale,
                    true,
                );
            }
        }
        let extra = extra as i32;
        fill_rect(
            &mut canvas,
            20 * s,
            (148 + extra) * s,
            358 * s,
            s,
            rgba(palette.border),
        );
        let probe = if snapshot.probe_error.is_some() {
            format!("Ping {target}: unavailable")
        } else {
            format!("Ping {target} · loss over last 20 probes")
        };
        self.network_text(
            &mut canvas,
            &probe,
            PixelRect {
                x: 20 * s,
                y: (153 + extra) * s,
                width: 358 * s,
                height: 16 * s,
            },
            9.0,
            palette.muted,
            scale,
            false,
        );
        self.network_text(
            &mut canvas,
            "Traffic totals since interface started",
            PixelRect {
                x: 20 * s,
                y: (170 + extra) * s,
                width: 358 * s,
                height: 16 * s,
            },
            9.0,
            palette.muted,
            scale,
            false,
        );
        PreparedNetwork {
            bitmap,
            width: WIDTH * scale,
            height: height * scale,
            scale,
            refresh,
            accent: rgba(palette.accent),
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn network_text(
        &mut self,
        canvas: &mut PixmapMut<'_>,
        text: &str,
        rect: PixelRect,
        size: f32,
        color: [u8; 4],
        scale: u32,
        right: bool,
    ) {
        let text = self.rasterize_text_with_size(text, scale, color, size);
        let x = if right {
            rect.x + rect.width - text.width() as i32
        } else {
            rect.x
        };
        draw_premultiplied_clipped(
            canvas,
            x,
            rect.y + (rect.height - text.height() as i32) / 2,
            text.width(),
            text.height(),
            text.data(),
            rect,
        );
    }
    pub fn draw_network(
        &mut self,
        canvas: &mut PixmapMut<'_>,
        network: &PreparedNetwork,
        hitboxes: &mut Vec<MenuHitbox>,
        selected: Option<MenuSelection>,
    ) {
        canvas.fill(Color::TRANSPARENT);
        draw_premultiplied(
            canvas,
            0,
            0,
            network.bitmap.width(),
            network.bitmap.height(),
            network.bitmap.data(),
        );
        hitboxes.clear();
        if let Some(h) = network.refresh_hitbox() {
            hitboxes.push(h);
            if Some(h.selection) == selected {
                outline(
                    canvas,
                    PixelRect {
                        x: h.x,
                        y: h.y,
                        width: h.width,
                        height: h.height,
                    },
                    network.scale as f32,
                    network.accent,
                );
            }
        }
    }
}

fn rgba(c: [u8; 4]) -> Color {
    Color::from_rgba8(c[0], c[1], c[2], c[3])
}
fn outline(canvas: &mut PixmapMut<'_>, r: PixelRect, width: f32, color: Color) {
    if let Some(rect) = Rect::from_xywh(r.x as f32, r.y as f32, r.width as f32, r.height as f32) {
        let mut paint = Paint::default();
        paint.set_color(color);
        canvas.stroke_path(
            &PathBuilder::from_rect(rect),
            &paint,
            &Stroke {
                width,
                ..Stroke::default()
            },
            Transform::identity(),
            None,
        );
    }
}
fn stroke(canvas: &mut PixmapMut<'_>, p: PathBuilder, width: f32, color: Color) {
    if let Some(p) = p.finish() {
        let mut paint = Paint::default();
        paint.set_color(color);
        canvas.stroke_path(
            &p,
            &paint,
            &Stroke {
                width,
                line_cap: LineCap::Round,
                line_join: LineJoin::Round,
                ..Stroke::default()
            },
            Transform::identity(),
            None,
        );
    }
}

pub(super) fn network_icon(kind: NetworkKind, size: u32, color: [u8; 4]) -> Pixmap {
    let mut pixmap = Pixmap::new(size.max(1), size.max(1)).expect("small network icon");
    let s = size as f32 / 18.0;
    let mut p = PathBuilder::new();
    match kind {
        NetworkKind::Wifi => {
            p.move_to(2.0 * s, 7.0 * s);
            p.quad_to(9.0 * s, 0.0 * s, 16.0 * s, 7.0 * s);
            p.move_to(5.0 * s, 10.0 * s);
            p.quad_to(9.0 * s, 6.0 * s, 13.0 * s, 10.0 * s);
            p.move_to(8.0 * s, 13.0 * s);
            p.line_to(10.0 * s, 13.0 * s);
        }
        NetworkKind::Vpn => {
            p.move_to(9.0 * s, 2.0 * s);
            p.line_to(15.0 * s, 4.0 * s);
            p.line_to(14.0 * s, 11.0 * s);
            p.quad_to(12.0 * s, 15.0 * s, 9.0 * s, 16.0 * s);
            p.quad_to(6.0 * s, 15.0 * s, 4.0 * s, 11.0 * s);
            p.line_to(3.0 * s, 4.0 * s);
            p.close();
            p.move_to(6.0 * s, 8.0 * s);
            p.line_to(8.0 * s, 10.0 * s);
            p.line_to(12.0 * s, 6.0 * s);
        }
        NetworkKind::Ethernet | NetworkKind::Offline => {
            p.move_to(3.0 * s, 2.0 * s);
            p.line_to(15.0 * s, 2.0 * s);
            p.line_to(15.0 * s, 14.0 * s);
            p.line_to(3.0 * s, 14.0 * s);
            p.close();
            for x in [6.0, 9.0, 12.0] {
                p.move_to(x * s, 5.0 * s);
                p.line_to(x * s, 8.0 * s);
            }
            p.move_to(7.0 * s, 14.0 * s);
            p.line_to(7.0 * s, 11.0 * s);
            p.line_to(11.0 * s, 11.0 * s);
            p.line_to(11.0 * s, 14.0 * s);
            if kind == NetworkKind::Offline {
                p.move_to(1.0 * s, 16.0 * s);
                p.line_to(17.0 * s, 1.0 * s);
            }
        }
    }
    stroke(&mut pixmap.as_mut(), p, 1.4 * s, rgba(color));
    pixmap
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn traffic_units_distinguish_unknown_from_idle_and_scale_large_totals() {
        assert_eq!(rate(None), "—");
        assert_eq!(rate(Some(0)), "0 B/s");
        assert_eq!(bytes(1024), "1.0 KiB");
        assert_eq!(bytes(1572864), "1.5 MiB");
        assert_eq!(bytes(1099511627776), "1.0 TiB");
    }
    #[test]
    fn card_preserves_alpha_and_exposes_only_refresh_action() {
        let c = Config {
            network_background: "#22222280".into(),
            ..Config::default()
        };
        let mut renderer = Renderer::new(&c);
        for scale in [1, 2, 3] {
            let mut card = renderer.prepare_network(
                &NetworkSnapshot::default(),
                "1.1.1.1".parse().unwrap(),
                scale,
                &NetworkPalette::from(&c),
            );
            assert_eq!(
                card.bitmap.pixel(10 * scale, 100 * scale).unwrap().alpha(),
                128
            );
            assert_eq!(
                card.selection_at(360 * scale as i32, 30 * scale as i32),
                Some(MenuSelection::Item(0))
            );
            card.constrain(100 * scale, 100 * scale);
            assert_eq!(card.next_selection(), None);
        }
    }
    #[test]
    #[ignore = "writes an explicit network preview for visual QA"]
    fn write_network_preview() {
        let c = Config {
            network_background: "#222222F2".into(),
            network_foreground: "#C2C2B0".into(),
            network_muted: "#8A8A7E".into(),
            network_accent: "#D7C483".into(),
            network_border: "#78824B".into(),
            ..Config::default()
        };
        let mut renderer = Renderer::new(&c);
        let snapshot = NetworkSnapshot {
            interface: "eth0".into(),
            kind: NetworkKind::Ethernet,
            address: Some("10.0.2.15".parse().unwrap()),
            gateway: Some("10.0.2.2".parse().unwrap()),
            rx_bytes: 34288435,
            tx_bytes: 704614,
            rx_per_second: Some(175),
            tx_per_second: Some(160),
            ping_ms: Some(56.0),
            loss_percent: Some(0),
            probe_error: None,
        };
        let card = renderer.prepare_network(
            &snapshot,
            "1.1.1.1".parse().unwrap(),
            1,
            &NetworkPalette::from(&c),
        );
        card.bitmap
            .save_png("/tmp/nibari-network-preview.png")
            .unwrap();
    }
}
