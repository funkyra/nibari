mod content;
mod glyphs;
mod paint;

use super::{MenuHitbox, MenuSelection, PixelRect, Renderer};
use crate::bluetooth::{Action, Snapshot};
pub use paint::{Cache, Palette};
use paint::{Element, Primitive};
use tiny_skia::{LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke};

const WIDTH: u32 = 480;
const HEADER: i32 = 60;
const INSET: i32 = super::popup_chrome::CARD_INSET;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Root,
    Device(String),
    Profiles(String),
    Codecs(String),
}

impl Page {
    pub fn parent(&self) -> Self {
        match self {
            Self::Profiles(path) | Self::Codecs(path) => Self::Device(path.clone()),
            _ => Self::Root,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiAction {
    Page(Page),
    Act(Action),
    Reply(u64, bool),
    CancelPair,
}

struct Control {
    rect: PixelRect,
    enabled: bool,
    fixed: bool,
}

pub struct PreparedBluetooth {
    width: u32,
    height: u32,
    natural_height: u32,
    scale: u32,
    scroll: i32,
    page: Page,
    palette: Palette,
    elements: Vec<Element>,
    actions: Vec<UiAction>,
    controls: Vec<Control>,
    hitboxes: Vec<MenuHitbox>,
}

impl PreparedBluetooth {
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn constrain(&mut self, width: u32, height: u32) {
        self.width = self.width.min(width.max(1));
        self.height = self.natural_height.min(height.max(1)).min(720 * self.scale);
        self.scroll = self.scroll.min(self.max_scroll());
        self.rebuild_hitboxes();
    }

    fn max_scroll(&self) -> i32 {
        self.natural_height.saturating_sub(self.height) as i32
    }

    fn clip(&self, fixed: bool) -> PixelRect {
        let s = self.scale as i32;
        let top = if fixed { INSET } else { HEADER } * s;
        let bottom = if fixed {
            (HEADER * s).min(self.height as i32 - INSET * s)
        } else {
            self.height as i32 - INSET * s
        };
        PixelRect {
            x: INSET * s,
            y: top,
            width: (self.width as i32 - 2 * INSET * s).max(0),
            height: (bottom - top).max(0),
        }
    }

    fn control_rect(&self, control: &Control) -> PixelRect {
        PixelRect {
            y: control.rect.y - if control.fixed { 0 } else { self.scroll },
            ..control.rect
        }
    }

    fn rebuild_hitboxes(&mut self) {
        self.hitboxes = self
            .controls
            .iter()
            .enumerate()
            .filter_map(|(id, control)| {
                let rect = intersection(self.control_rect(control), self.clip(control.fixed))?;
                Some(MenuHitbox {
                    selection: MenuSelection::Item(id as i32),
                    enabled: control.enabled,
                    x: rect.x,
                    y: rect.y,
                    width: rect.width,
                    height: rect.height,
                })
            })
            .collect();
    }

    pub fn selection_at(&self, x: i32, y: i32) -> Option<MenuSelection> {
        self.hitboxes
            .iter()
            .find(|h| h.enabled && x >= h.x && y >= h.y && x < h.x + h.width && y < h.y + h.height)
            .map(|h| h.selection)
    }

    pub fn action(&self, id: i32) -> Option<&UiAction> {
        let id = usize::try_from(id).ok()?;
        self.controls
            .get(id)
            .filter(|c| c.enabled)
            .and_then(|_| self.actions.get(id))
    }

    pub fn next_selection(
        &self,
        current: Option<MenuSelection>,
        backwards: bool,
    ) -> Option<MenuSelection> {
        let choices = || {
            self.controls
                .iter()
                .enumerate()
                .filter(|(_, c)| c.enabled)
                .map(|(i, _)| MenuSelection::Item(i as i32))
        };
        let count = choices().count();
        if count == 0 {
            return None;
        }
        let current = choices().position(|s| Some(s) == current);
        let index = match (current, backwards) {
            (Some(i), false) => (i + 1) % count,
            (Some(i), true) => (i + count - 1) % count,
            (None, false) => 0,
            (None, true) => count - 1,
        };
        choices().nth(index)
    }

    pub fn scroll_by(&mut self, delta: i32) -> bool {
        let offset = self
            .scroll
            .saturating_add(delta)
            .clamp(0, self.max_scroll());
        if offset == self.scroll {
            return false;
        }
        self.scroll = offset;
        self.rebuild_hitboxes();
        true
    }

    pub fn ensure_visible(&mut self, selection: MenuSelection) -> bool {
        let MenuSelection::Item(id) = selection else {
            return false;
        };
        let Some(control) = usize::try_from(id)
            .ok()
            .and_then(|id| self.controls.get(id))
        else {
            return false;
        };
        if control.fixed {
            return false;
        }
        let rect = self.control_rect(control);
        let clip = self.clip(false);
        let delta = if rect.y < clip.y {
            rect.y - clip.y
        } else {
            (rect.y + rect.height - clip.y - clip.height).max(0)
        };
        self.scroll_by(delta)
    }

    pub fn preserve_view(
        &mut self,
        previous: &Self,
        selection: Option<MenuSelection>,
    ) -> Option<MenuSelection> {
        if self.page != previous.page {
            return None;
        }
        self.scroll = previous.scroll.min(self.max_scroll());
        self.rebuild_hitboxes();
        let Some(MenuSelection::Item(id)) = selection else {
            return None;
        };
        let action = previous.action(id)?;
        let id = self.actions.iter().position(|a| a == action)?;
        self.controls[id]
            .enabled
            .then_some(MenuSelection::Item(id as i32))
    }
}

fn intersection(a: PixelRect, b: PixelRect) -> Option<PixelRect> {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let right = (a.x + a.width).min(b.x + b.width);
    let bottom = (a.y + a.height).min(b.y + b.height);
    (right > x && bottom > y).then_some(PixelRect {
        x,
        y,
        width: right - x,
        height: bottom - y,
    })
}

impl Renderer {
    pub fn prepare_bluetooth(
        &mut self,
        state: &Snapshot,
        page: &Page,
        input: &str,
        scale: u32,
        width: u32,
    ) -> PreparedBluetooth {
        self.bluetooth_cache.begin();
        let popup = content::prepare(
            self,
            state,
            page,
            input,
            scale.max(1),
            width.clamp(180, WIDTH),
        );
        self.bluetooth_cache.finish();
        popup
    }
}
pub fn icon(size: u32, color: [u8; 4], connected: bool) -> Pixmap {
    if size <= 16 {
        return super::pixel_icons::icon(
            size,
            color,
            super::pixel_icons::Shape::Bluetooth { connected },
        );
    }
    let mut pixmap = Pixmap::new(size.max(1), size.max(1)).expect("Bluetooth icon");
    let mut path = PathBuilder::new();
    path.move_to(6.0, 6.0);
    path.line_to(17.0, 16.0);
    path.line_to(12.0, 21.0);
    path.line_to(12.0, 3.0);
    path.line_to(17.0, 8.0);
    path.line_to(6.0, 18.0);
    if connected {
        path.move_to(3.0, 11.0);
        path.line_to(3.0, 13.0);
        path.move_to(21.0, 11.0);
        path.line_to(21.0, 13.0);
    }
    let mut paint = Paint::default();
    paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
    let stroke = Stroke {
        width: 1.7,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Default::default()
    };
    let path = path.finish().unwrap();
    pixmap.stroke_path(
        &path,
        &paint,
        &stroke,
        super::fitted_icon_transform(&path, size, stroke.width),
        None,
    );
    pixmap
}

#[cfg(test)]
mod tests;
