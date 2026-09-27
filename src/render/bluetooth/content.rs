use super::paint::{Glyph, rgba};
use super::{Control, Element, HEADER, INSET, Page, PreparedBluetooth, Primitive, UiAction};
use crate::{
    bluetooth::{Action, Adapter, Device, PromptKind, Snapshot},
    render::{PixelRect, Renderer},
};
use tiny_skia::Color;

fn rect(x: i32, y: i32, width: i32, height: i32) -> PixelRect {
    PixelRect {
        x,
        y,
        width,
        height,
    }
}

struct Builder<'a> {
    renderer: &'a mut Renderer,
    popup: PreparedBluetooth,
    width: i32,
    fixed: bool,
}

impl Builder<'_> {
    fn physical(&self, r: PixelRect) -> PixelRect {
        let s = self.popup.scale as i32;
        rect(r.x * s, r.y * s, r.width * s, r.height * s)
    }

    fn text(&mut self, label: &str, r: PixelRect, size: f32, color: [u8; 4], right: bool) {
        let r = self.physical(r);
        if r.width <= 0 || r.height <= 0 {
            return;
        }
        let bitmap = self
            .renderer
            .bluetooth_text(label, size, self.popup.scale, color);
        let y = r.y + (r.height - bitmap.height() as i32) / 2;
        if bitmap.width() as i32 > r.width {
            let ellipsis = self
                .renderer
                .bluetooth_text("…", size, self.popup.scale, color);
            let available = (r.width - ellipsis.width() as i32).max(0);
            self.popup.elements.push(Element {
                rect: rect(r.x, r.y, available, r.height),
                fixed: self.fixed,
                primitive: Primitive::Sprite {
                    image: bitmap,
                    x: r.x,
                    y,
                },
            });
            self.popup.elements.push(Element {
                rect: r,
                fixed: self.fixed,
                primitive: Primitive::Sprite {
                    x: r.x + available,
                    y: r.y + (r.height - ellipsis.height() as i32) / 2,
                    image: ellipsis,
                },
            });
        } else {
            let x = if right {
                r.x + r.width - bitmap.width() as i32
            } else {
                r.x
            };
            self.popup.elements.push(Element {
                rect: r,
                fixed: self.fixed,
                primitive: Primitive::Sprite {
                    image: bitmap,
                    x,
                    y,
                },
            });
        }
    }

    fn centered_text(&mut self, label: &str, r: PixelRect, size: f32, color: [u8; 4]) {
        let r = self.physical(r);
        let image = self
            .renderer
            .bluetooth_text(label, size, self.popup.scale, color);
        self.popup.elements.push(Element {
            rect: r,
            fixed: self.fixed,
            primitive: Primitive::Sprite {
                x: r.x + (r.width - image.width() as i32) / 2,
                y: r.y + (r.height - image.height() as i32) / 2,
                image,
            },
        });
    }

    fn glyph(&mut self, glyph: Glyph, x: i32, y: i32, size: i32, color: [u8; 4]) {
        let r = self.physical(rect(x, y, size, size));
        let image = self.renderer.bluetooth_glyph(glyph, r.width as u32, color);
        self.popup.elements.push(Element {
            rect: r,
            fixed: self.fixed,
            primitive: Primitive::Sprite {
                image,
                x: r.x,
                y: r.y,
            },
        });
    }

    fn rounded(&mut self, r: PixelRect, radius: f32, fill: Color, border: Option<Color>) {
        self.popup.elements.push(Element {
            rect: self.physical(r),
            fixed: self.fixed,
            primitive: Primitive::Rounded {
                radius: radius * self.popup.scale as f32,
                fill,
                border,
            },
        });
    }

    fn rule(&mut self, y: i32) {
        self.rounded(
            rect(28, y, self.width - 56, 1),
            0.0,
            self.popup.palette.border,
            None,
        );
    }

    fn hit(&mut self, r: PixelRect, action: UiAction, enabled: bool) {
        self.popup.actions.push(action);
        self.popup.controls.push(Control {
            rect: self.physical(r),
            enabled,
            fixed: self.fixed,
        });
    }

    fn button(&mut self, label: &str, r: PixelRect, action: UiAction, enabled: bool, accent: bool) {
        let p = self.popup.palette;
        self.rounded(
            r,
            7.0,
            if accent && enabled {
                p.on
            } else {
                p.background
            },
            Some(if accent && enabled {
                rgba(p.accent)
            } else {
                p.border
            }),
        );
        let color = if !enabled {
            p.muted
        } else if accent {
            p.accent
        } else {
            p.foreground
        };
        self.text(
            label,
            rect(r.x + 10, r.y, r.width - 20, r.height),
            12.0,
            color,
            false,
        );
        self.hit(r, action, enabled);
    }

    fn toggle(&mut self, adapter: &Adapter, state: &Snapshot, x: i32, y: i32) {
        let p = self.popup.palette;
        let action = Action::Power(adapter.path.clone(), !adapter.powered);
        let enabled = state.allows(&action);
        let pending_power = match &state.busy {
            Some(Action::Power(path, powered)) if path == &adapter.path => Some(*powered),
            _ => None,
        };
        let shown_powered = pending_power.unwrap_or(adapter.powered);
        let visual_enabled = enabled || pending_power.is_some();
        self.rounded(
            rect(x, y, 72, 28),
            1.0,
            if visual_enabled {
                rgba(p.accent)
            } else {
                p.border
            },
            None,
        );
        self.rounded(rect(x + 2, y + 2, 68, 24), 0.0, p.background, None);
        let active_x = x + if shown_powered { 36 } else { 2 };
        self.rounded(
            rect(active_x, y + 2, 34, 24),
            0.0,
            if visual_enabled { rgba(p.accent) } else { p.on },
            None,
        );
        self.rounded(rect(x + 35, y + 2, 2, 24), 0.0, p.border, None);
        for (label, offset, active) in [("OFF", 0, !shown_powered), ("ON", 36, shown_powered)] {
            self.centered_text(
                label,
                rect(x + offset, y, 36, 28),
                10.0,
                if active && visual_enabled {
                    p.switch_foreground
                } else {
                    p.muted
                },
            );
        }
        self.hit(rect(x, y, 72, 28), UiAction::Act(action), enabled);
    }

    fn header(&mut self, state: &Snapshot, page: &Page) {
        let p = self.popup.palette;
        self.fixed = true;
        if *page == Page::Root {
            self.glyph(Glyph::Bluetooth, 28, 23, 24, p.accent);
            self.text(
                "Bluetooth",
                rect(62, 20, self.width - 170, 30),
                20.0,
                p.foreground,
                false,
            );
            if let Some(adapter) = state.adapters.first() {
                let label = adapter.path.rsplit('/').next().unwrap_or("hci0");
                if self.width >= 400 {
                    self.rounded(rect(214, 27, 46, 23), 7.0, p.background, Some(p.border));
                    self.text(label, rect(221, 28, 32, 20), 12.0, p.muted, false);
                }
                self.toggle(adapter, state, self.width - 98, 24);
            }
        } else {
            self.glyph(Glyph::Back, 26, 27, 20, p.accent);
            self.hit(rect(22, 20, 32, 34), UiAction::Page(page.parent()), true);
            let title = match page {
                Page::Device(_) => "Device",
                Page::Profiles(_) => "Audio profile",
                Page::Codecs(_) => "Audio codec",
                _ => "Bluetooth",
            };
            self.text(
                title,
                rect(61, 21, self.width - 91, 28),
                18.0,
                p.foreground,
                false,
            );
        }
        self.rule(HEADER - 1);
        self.fixed = false;
    }

    fn notice(&mut self, text: &str, y: i32, accent: bool) -> i32 {
        // Break long errors/prompts into readable lines; do not hide pairing codes in an ellipsis.
        let max = ((self.width - 64) / 7).max(12) as usize;
        let clean: String = text
            .chars()
            .take(512)
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let chars: Vec<_> = clean.chars().collect();
        let mut y = y;
        for line in chars.chunks(max) {
            let text: String = line.iter().collect();
            self.text(
                &text,
                rect(28, y, self.width - 56, 22),
                12.0,
                if accent {
                    self.popup.palette.accent
                } else {
                    self.popup.palette.muted
                },
                false,
            );
            y += 22;
        }
        y + 8
    }

    fn traffic(&mut self, adapter: &Adapter, y: i32) -> i32 {
        let p = self.popup.palette;
        let half = (self.width - 56) / 2;
        let font = if self.width < 400 { 18.0 } else { 23.0 };
        for (index, (label, glyph, rate)) in [
            ("RECEIVE", Glyph::Rx, adapter.rate.map(|r| r.0)),
            ("TRANSMIT", Glyph::Tx, adapter.rate.map(|r| r.1)),
        ]
        .into_iter()
        .enumerate()
        {
            let x = 28 + index as i32 * half;
            self.text(label, rect(x, y + 9, half - 12, 18), 11.0, p.muted, false);
            self.glyph(glyph, x, y + 37, 20, p.foreground);
            let number = rate.map_or_else(|| "—".into(), |v| format!("{:.1}", v as f64 / 1024.0));
            self.text(
                &format!("{number} KiB/s"),
                rect(x + 27, y + 28, half - 33, 36),
                font,
                p.foreground,
                false,
            );
        }
        self.text(
            "Adapter traffic",
            rect(28, y + 62, self.width - 56, 18),
            11.0,
            p.muted,
            false,
        );
        self.rule(y + 87);
        y + 88
    }

    fn dropdown(&mut self, label: &str, value: &str, area: PixelRect, page: Page, enabled: bool) {
        let p = self.popup.palette;
        let PixelRect { x, y, width, .. } = area;
        let label_width = if label == "Profile" { 58 } else { 45 };
        self.text(label, rect(x, y, label_width - 5, 28), 12.0, p.muted, false);
        let r = rect(x + label_width, y, width - label_width, 28);
        self.rounded(r, 7.0, p.background, Some(p.border));
        self.text(
            value,
            rect(r.x + 10, y, r.width - 32, 28),
            12.0,
            if enabled { p.foreground } else { p.muted },
            false,
        );
        self.glyph(
            Glyph::Down,
            r.x + r.width - 22,
            y + 6,
            16,
            if enabled { p.accent } else { p.muted },
        );
        self.hit(r, UiAction::Page(page), enabled);
    }

    fn audio(&mut self, device: &Device, y: i32) -> i32 {
        let Some(card) = device.audio.as_ref().filter(|_| device.connected) else {
            return y;
        };
        let profile = if card.active_profile.starts_with("a2dp") {
            "A2DP"
        } else if card.active_profile.starts_with("headset") {
            "HFP / HSP"
        } else {
            card.active_profile.as_str()
        };
        let codec = card
            .codecs
            .iter()
            .find(|c| Some(&c.name) == card.active_codec.as_ref())
            .map_or("—", |c| c.description.as_str());
        if self.width >= 400 {
            let half = (self.width - 72) / 2;
            self.dropdown(
                "Profile",
                profile,
                rect(28, y, half, 28),
                Page::Profiles(device.path.clone()),
                true,
            );
            self.dropdown(
                "Codec",
                codec,
                rect(44 + half, y, half, 28),
                Page::Codecs(device.path.clone()),
                true,
            );
            y + 42
        } else {
            self.dropdown(
                "Profile",
                profile,
                rect(28, y, self.width - 56, 28),
                Page::Profiles(device.path.clone()),
                true,
            );
            self.dropdown(
                "Codec",
                codec,
                rect(28, y + 36, self.width - 56, 28),
                Page::Codecs(device.path.clone()),
                true,
            );
            y + 78
        }
    }

    fn device(&mut self, device: &Device, state: &Snapshot, y: i32, details: bool) -> i32 {
        let p = self.popup.palette;
        let glyph = if device.audio.is_some() {
            Glyph::Headphones
        } else {
            match device.icon.as_str() {
                "audio-headset" | "audio-headphones" => Glyph::Headphones,
                "input-keyboard" => Glyph::Keyboard,
                "input-mouse" => Glyph::Mouse,
                "audio-card" | "audio-speakers" => Glyph::Speaker,
                _ => Glyph::Device,
            }
        };
        self.glyph(glyph, 28, y + 16, 27, p.accent);
        if !details {
            self.hit(
                rect(24, y + 12, 35, 35),
                UiAction::Page(Page::Device(device.path.clone())),
                true,
            );
        }
        let (label, action) = if device.connected {
            ("Disconnect", Action::Disconnect(device.path.clone()))
        } else if device.paired {
            ("Connect", Action::Connect(device.path.clone()))
        } else {
            ("Pair", Action::Pair(device.path.clone()))
        };
        let action_x = self.width - 132;
        self.text(
            &device.name,
            rect(72, y + 10, action_x - 82, 24),
            15.0,
            p.foreground,
            false,
        );
        let status = if device.connected {
            "Connected"
        } else if device.paired {
            "Paired"
        } else {
            "Not paired"
        };
        let status_color = if device.connected {
            p.connected
        } else {
            p.muted
        };
        self.rounded(rect(73, y + 42, 7, 7), 3.5, rgba(status_color), None);
        self.text(
            status,
            rect(87, y + 34, action_x - 95, 24),
            11.0,
            status_color,
            false,
        );
        if let Some(battery) = device.battery
            && self.width >= 420
        {
            self.glyph(Glyph::Battery, 196, y + 38, 18, p.muted);
            self.text(
                &format!("{battery}%"),
                rect(219, y + 34, 48, 24),
                11.0,
                p.muted,
                false,
            );
        }
        self.button(
            label,
            rect(action_x, y + 20, 104, 28),
            UiAction::Act(action.clone()),
            state.allows(&action),
            false,
        );
        let mut bottom = self.audio(device, y + 66);
        if bottom == y + 66 {
            bottom = y + 70;
        }
        if details {
            bottom = self.notice(&device.address, bottom, false);
            if device.audio.is_none() && device.connected {
                bottom = self.notice(
                    state
                        .audio_error
                        .as_deref()
                        .unwrap_or("No audio card reported by PipeWire"),
                    bottom,
                    false,
                );
            }
        }
        self.rule(bottom);
        bottom + 1
    }

    fn footer(&mut self, adapter: &Adapter, state: &Snapshot, y: i32) -> i32 {
        let p = self.popup.palette;
        let count = state
            .devices
            .iter()
            .filter(|d| d.adapter == adapter.path && d.paired)
            .count();
        let label = if adapter.scan_owned {
            "Stop search"
        } else {
            "Search devices"
        };
        let action = Action::Scan(adapter.path.clone(), !adapter.scan_owned);
        let enabled = state.allows(&action);
        let right = self.width - 28;
        let top = if self.width < 400 { y + 27 } else { y + 5 };
        self.text(
            &format!("{count} saved devices"),
            rect(28, y + 9, 175, 28),
            11.0,
            p.muted,
            false,
        );
        self.rounded(
            rect(right - 126, top, 128, 36),
            7.0,
            p.background,
            Some(if enabled { rgba(p.accent) } else { p.border }),
        );
        self.text(
            label,
            rect(right - 114, top + 4, 104, 28),
            12.0,
            if enabled { p.accent } else { p.muted },
            false,
        );
        self.hit(
            rect(right - 126, top, 128, 36),
            UiAction::Act(action),
            enabled,
        );
        top + 47
    }

    fn choices(&mut self, state: &Snapshot, page: &Page, path: &str, mut y: i32) -> i32 {
        let p = self.popup.palette;
        let Some(device) = state.devices.iter().find(|d| d.path == path) else {
            return self.notice("Device is no longer available", y + 12, false);
        };
        y = self.notice(&device.name, y + 12, false);
        let Some(card) = device.audio.as_ref().filter(|_| device.connected) else {
            return self.notice("Audio device is disconnected", y, false);
        };
        match page {
            Page::Profiles(_) => {
                for profile in &card.profiles {
                    let action = Action::Profile {
                        device: path.into(),
                        name: profile.name.clone(),
                    };
                    self.choice(
                        &profile.description,
                        UiAction::Act(action.clone()),
                        state.allows(&action),
                        profile.name == card.active_profile,
                        y,
                    );
                    y += 42;
                }
            }
            Page::Codecs(_) => {
                if let Some(error) = &card.codec_error {
                    y = self.notice(error, y, true);
                }
                for codec in &card.codecs {
                    let action = Action::Codec {
                        device: path.into(),
                        name: codec.name.clone(),
                    };
                    self.choice(
                        &codec.description,
                        UiAction::Act(action.clone()),
                        state.allows(&action),
                        Some(&codec.name) == card.active_codec.as_ref(),
                        y,
                    );
                    y += 42;
                }
                if card.codecs.is_empty() {
                    y = self.notice("No selectable codecs for this profile", y, false);
                }
            }
            _ => {}
        }
        self.text(
            "Changes apply immediately",
            rect(28, y + 8, self.width - 56, 22),
            11.0,
            p.muted,
            false,
        );
        y + 36
    }

    fn choice(&mut self, label: &str, action: UiAction, enabled: bool, selected: bool, y: i32) {
        let p = self.popup.palette;
        self.rounded(
            rect(25, y, self.width - 50, 36),
            7.0,
            if selected { p.on } else { p.background },
            Some(if selected { rgba(p.accent) } else { p.border }),
        );
        self.rounded(
            rect(36, y + 11, 14, 14),
            7.0,
            p.background,
            Some(if selected { rgba(p.accent) } else { p.border }),
        );
        if selected {
            self.rounded(rect(40, y + 15, 6, 6), 3.0, rgba(p.accent), None);
        }
        self.text(
            label,
            rect(62, y + 2, self.width - 101, 32),
            13.0,
            if enabled { p.foreground } else { p.muted },
            false,
        );
        self.hit(rect(25, y, self.width - 50, 36), action, enabled);
    }
}

pub(super) fn prepare(
    renderer: &mut Renderer,
    state: &Snapshot,
    page: &Page,
    input: &str,
    scale: u32,
    width: u32,
) -> PreparedBluetooth {
    let palette = renderer.bluetooth_palette;
    let mut b = Builder {
        renderer,
        width: width as i32,
        fixed: false,
        popup: PreparedBluetooth {
            width: width * scale,
            height: 1,
            natural_height: 1,
            scale,
            scroll: 0,
            page: page.clone(),
            palette,
            elements: Vec::new(),
            actions: Vec::new(),
            controls: Vec::new(),
            hitboxes: Vec::new(),
        },
    };
    b.header(state, page);
    let mut y = HEADER;
    if let Some(prompt) = &state.prompt {
        y = b.notice(&prompt.text, y + 16, true);
        if let Some(device) = state.devices.iter().find(|d| d.path == prompt.device) {
            y = b.notice(&device.name, y, false);
        }
        if matches!(prompt.kind, PromptKind::Pin | PromptKind::Passkey) {
            b.rounded(
                rect(28, y, width as i32 - 56, 38),
                6.0,
                palette.hover,
                Some(palette.border),
            );
            b.text(
                &format!("{input}▏"),
                rect(40, y, width as i32 - 80, 38),
                18.0,
                palette.foreground,
                false,
            );
            y = b.notice("Type the code, then press Enter", y + 44, false);
        }
        if prompt.kind != PromptKind::Display {
            b.button(
                "Confirm",
                rect(28, y, 108, 32),
                UiAction::Reply(prompt.id, true),
                prompt.kind.accepts(input),
                true,
            );
        }
        b.button(
            "Cancel",
            rect(width as i32 - 136, y, 108, 32),
            UiAction::Reply(prompt.id, false),
            true,
            false,
        );
        y += 50;
    } else {
        if let Some(error) = &state.error {
            y = b.notice(error, y + 8, true);
        }
        if let Some(action) = state
            .busy
            .as_ref()
            .filter(|action| !matches!(action, Action::Power(..)))
        {
            y = b.notice(
                if matches!(action, Action::Pair(_)) {
                    "Pairing…"
                } else {
                    "Applying change…"
                },
                y + 4,
                true,
            );
            if matches!(action, Action::Pair(_)) {
                b.button(
                    "Cancel",
                    rect(28, y, 108, 30),
                    UiAction::CancelPair,
                    true,
                    false,
                );
                y += 42;
            }
        }
        match page {
            Page::Root => {
                if !state.available {
                    y = b.notice("Bluetooth service unavailable", y + 22, false);
                } else if state.adapters.is_empty() {
                    y = b.notice("No Bluetooth adapter found", y + 22, false);
                }
                for (index, adapter) in state.adapters.iter().enumerate() {
                    if index > 0 {
                        b.text(
                            &adapter.name,
                            rect(28, y + 9, width as i32 - 120, 28),
                            15.0,
                            palette.foreground,
                            false,
                        );
                        b.toggle(adapter, state, width as i32 - 98, y + 12);
                        y += 50;
                    }
                    y = b.traffic(adapter, y);
                    if adapter.discovering {
                        y = b.notice("Searching for nearby devices…", y + 6, true);
                    }
                    let mut found = false;
                    for device in state.devices.iter().filter(|d| d.adapter == adapter.path) {
                        y = b.device(device, state, y, false);
                        found = true;
                    }
                    if !found {
                        y = b.notice("No devices yet. Start a search to pair.", y + 14, false);
                    }
                    y = b.footer(adapter, state, y);
                }
            }
            Page::Device(path) => {
                if let Some(device) = state.devices.iter().find(|d| &d.path == path) {
                    y = b.device(device, state, y + 6, true) + 18;
                } else {
                    y = b.notice("Device is no longer available", y + 18, false);
                }
            }
            Page::Profiles(path) | Page::Codecs(path) => {
                y = b.choices(state, page, path, y);
            }
        }
    }
    b.popup.natural_height = ((y + INSET).max(145) as u32) * scale;
    b.popup.height = b.popup.natural_height.min(720 * scale);
    b.popup.rebuild_hitboxes();
    b.popup
}
