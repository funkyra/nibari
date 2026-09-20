use super::calendar::{CalendarPalette, PreparedCalendar};
use super::network::PreparedNetwork;
use super::weather::PreparedWeather;
use super::{MenuHitbox, MenuSelection, PixmapMut, PreparedMenu, Renderer};
use crate::config::Config;
use chrono::NaiveDate;

pub enum PreparedPopup {
    Tray(PreparedMenu),
    Calendar(PreparedCalendar),
    Network(PreparedNetwork),
    Weather(PreparedWeather),
}

impl PreparedPopup {
    pub fn is_weather(&self) -> bool {
        matches!(self, Self::Weather(_))
    }
    pub fn centered(&self) -> bool {
        matches!(self, Self::Calendar(_) | Self::Weather(_))
    }
    pub fn is_tray(&self) -> bool {
        matches!(self, Self::Tray(_))
    }
    pub fn is_network(&self) -> bool {
        matches!(self, Self::Network(_))
    }
    pub fn size(&self) -> (u32, u32) {
        match self {
            Self::Tray(p) => p.size(),
            Self::Calendar(p) => p.size(),
            Self::Network(p) => p.size(),
            Self::Weather(p) => p.size(),
        }
    }
    pub fn constrain(&mut self, width: u32, height: u32) {
        match self {
            Self::Tray(p) => p.constrain(width, height),
            Self::Calendar(p) => p.constrain(width, height),
            Self::Network(p) => p.constrain(width, height),
            Self::Weather(p) => p.constrain(width, height),
        }
    }
    pub fn selection_at(&self, x: i32, y: i32) -> Option<MenuSelection> {
        match self {
            Self::Tray(p) => p.selection_at(x, y),
            Self::Calendar(p) => p.selection_at(x, y),
            Self::Network(p) => p.selection_at(x, y),
            Self::Weather(p) => p.selection_at(x, y),
        }
    }
    pub fn next_selection(
        &self,
        current: Option<MenuSelection>,
        backwards: bool,
    ) -> Option<MenuSelection> {
        match self {
            Self::Tray(p) => p.next_selection(current, backwards),
            Self::Calendar(p) => p.next_selection(current, backwards),
            Self::Network(p) => p.next_selection(),
            Self::Weather(p) => p.next_selection(),
        }
    }
    pub fn scroll_by(&mut self, delta: i32) -> bool {
        match self {
            Self::Tray(p) => p.scroll_by(delta),
            Self::Calendar(_) | Self::Network(_) | Self::Weather(_) => false,
        }
    }
    pub fn ensure_visible(&mut self, selection: MenuSelection) -> bool {
        match self {
            Self::Tray(p) => p.ensure_visible(selection),
            Self::Calendar(_) | Self::Network(_) | Self::Weather(_) => false,
        }
    }
    pub fn calendar_dates(&self) -> Option<(NaiveDate, NaiveDate)> {
        match self {
            Self::Calendar(p) => Some((p.month, p.today)),
            Self::Tray(_) | Self::Network(_) | Self::Weather(_) => None,
        }
    }
}

impl Renderer {
    pub fn draw_popup(
        &mut self,
        canvas: &mut PixmapMut<'_>,
        popup: &PreparedPopup,
        hitboxes: &mut Vec<MenuHitbox>,
        selected: Option<MenuSelection>,
    ) {
        match popup {
            PreparedPopup::Tray(p) => self.draw_menu(canvas, p, hitboxes, selected),
            PreparedPopup::Calendar(p) => self.draw_calendar(canvas, p, hitboxes, selected),
            PreparedPopup::Network(p) => self.draw_network(canvas, p, hitboxes, selected),
            PreparedPopup::Weather(p) => self.draw_weather(canvas, p, hitboxes, selected),
        }
    }
}

impl From<&Config> for CalendarPalette {
    fn from(config: &Config) -> Self {
        Self {
            background: config.color_rgba(&config.calendar_background),
            foreground: config.color_rgba(&config.calendar_foreground),
            header: config.color_rgba(&config.calendar_header),
            weekday: config.color_rgba(&config.calendar_weekday),
            weekend: config.color_rgba(&config.calendar_weekend),
            muted: config.color_rgba(&config.calendar_muted),
            today_background: config.color_rgba(&config.calendar_today_background),
            today_foreground: config.color_rgba(&config.calendar_today_foreground),
            border: config.color_rgba(&config.calendar_border),
        }
    }
}
