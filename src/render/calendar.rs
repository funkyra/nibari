use chrono::{Datelike, NaiveDate, TimeDelta};
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, PixmapMut, Stroke,
    Transform,
};

use super::{MenuHitbox, MenuSelection, PixelRect, Renderer, draw_premultiplied};

const WIDTH: u32 = 288;
const HEIGHT: u32 = 280;
const CARD_INSET: i32 = 6;
const CONTENT_LEFT: i32 = 15;
const COLUMN_WIDTH: i32 = 37;
const HEADER_TOP: i32 = 14;
const HEADER_HEIGHT: i32 = 40;
const WEEKDAY_CENTER_Y: i32 = 78;
const GRID_TOP: i32 = 94;
const ROW_HEIGHT: i32 = 28;
const HIGHLIGHT_SIZE: i32 = 26;

#[derive(Clone, Debug)]
pub struct CalendarPalette {
    pub background: [u8; 4],
    pub foreground: [u8; 4],
    pub header: [u8; 4],
    pub weekday: [u8; 4],
    pub weekend: [u8; 4],
    pub muted: [u8; 4],
    pub today_background: [u8; 4],
    pub today_foreground: [u8; 4],
    pub border: [u8; 4],
}

pub struct PreparedCalendar {
    pub month: NaiveDate,
    pub today: NaiveDate,
    width: u32,
    height: u32,
    natural_size: (u32, u32),
    scale: u32,
    bitmap: Pixmap,
    navigation: [MenuHitbox; 3],
    hover: Color,
}

impl PreparedCalendar {
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn constrain(&mut self, width: u32, height: u32) {
        self.width = self.natural_size.0.min(width.max(1));
        self.height = self.natural_size.1.min(height.max(1));
    }

    pub fn selection_at(&self, x: i32, y: i32) -> Option<MenuSelection> {
        self.navigation
            .iter()
            .filter_map(|hitbox| self.clipped_hitbox(*hitbox))
            .find(|hitbox| {
                x >= hitbox.x
                    && x < hitbox.x + hitbox.width
                    && y >= hitbox.y
                    && y < hitbox.y + hitbox.height
            })
            .map(|hitbox| hitbox.selection)
    }

    pub fn next_selection(
        &self,
        current: Option<MenuSelection>,
        backwards: bool,
    ) -> Option<MenuSelection> {
        const CHOICES: [MenuSelection; 3] = [
            MenuSelection::Item(-1),
            MenuSelection::Item(0),
            MenuSelection::Item(1),
        ];
        let current =
            current.and_then(|selection| CHOICES.iter().position(|item| *item == selection));
        let index = match (current, backwards) {
            (Some(index), true) => (index + CHOICES.len() - 1) % CHOICES.len(),
            (Some(index), false) => (index + 1) % CHOICES.len(),
            (None, true) => CHOICES.len() - 1,
            (None, false) => 0,
        };
        Some(CHOICES[index])
    }

    fn clipped_hitbox(&self, hitbox: MenuHitbox) -> Option<MenuHitbox> {
        let right = (hitbox.x + hitbox.width).min(self.width as i32);
        let bottom = (hitbox.y + hitbox.height).min(self.height as i32);
        let x = hitbox.x.max(0);
        let y = hitbox.y.max(0);
        (right > x && bottom > y).then_some(MenuHitbox {
            x,
            y,
            width: right - x,
            height: bottom - y,
            ..hitbox
        })
    }
}

pub fn shifted_month(month: NaiveDate, delta: i32) -> NaiveDate {
    let month = first_of_month(month);
    let index = i64::from(month.year()) * 12 + i64::from(month.month0());
    let min = i64::from(NaiveDate::MIN.year()) * 12 + i64::from(NaiveDate::MIN.month0());
    let max = i64::from(NaiveDate::MAX.year()) * 12 + i64::from(NaiveDate::MAX.month0());
    let target = index.saturating_add(i64::from(delta)).clamp(min, max);
    let year = target.div_euclid(12) as i32;
    let month = target.rem_euclid(12) as u32 + 1;
    NaiveDate::from_ymd_opt(year, month, 1).expect("clamped month is supported by chrono")
}

fn first_of_month(date: NaiveDate) -> NaiveDate {
    date.with_day(1)
        .expect("every supported month has a first day")
}

fn month_grid(month: NaiveDate) -> [Option<NaiveDate>; 42] {
    let first = first_of_month(month);
    let leading = i64::from(first.weekday().num_days_from_monday());
    std::array::from_fn(|index| first.checked_add_signed(TimeDelta::days(index as i64 - leading)))
}

impl Renderer {
    pub fn prepare_calendar(
        &mut self,
        month: NaiveDate,
        today: NaiveDate,
        scale: u32,
        palette: &CalendarPalette,
    ) -> PreparedCalendar {
        let scale = scale.max(1);
        let month = first_of_month(month);
        let natural_size = (WIDTH * scale, HEIGHT * scale);
        let mut bitmap = Pixmap::new(natural_size.0, natural_size.1)
            .expect("calendar dimensions are small and non-zero");
        bitmap.fill(Color::TRANSPARENT);
        let mut canvas = bitmap.as_mut();
        let s = scale as i32;
        let card = PixelRect {
            x: CARD_INSET * s,
            y: CARD_INSET * s,
            width: (WIDTH as i32 - 2 * CARD_INSET) * s,
            height: (HEIGHT as i32 - 2 * CARD_INSET) * s,
        };
        rounded_rect(
            &mut canvas,
            card,
            12.0 * scale as f32,
            color(palette.background),
        );
        stroke_rounded_rect(
            &mut canvas,
            card,
            12.0 * scale as f32,
            scale as f32,
            color(palette.border),
        );

        let navigation = navigation_hitboxes(scale);
        let header_center_y = (HEADER_TOP + HEADER_HEIGHT / 2) * s;
        chevron(
            &mut canvas,
            34 * s,
            header_center_y,
            s,
            true,
            color(palette.foreground),
        );
        chevron(
            &mut canvas,
            254 * s,
            header_center_y,
            s,
            false,
            color(palette.foreground),
        );
        let heading = self.rasterize_text_with_size(
            &month.format("%B %Y").to_string(),
            scale,
            palette.header,
            self.style.font_size.clamp(15.0, 17.0),
        );
        draw_centered(&mut canvas, &heading, 144 * s, header_center_y);

        for (column, label) in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
            .into_iter()
            .enumerate()
        {
            let ink = if column >= 5 {
                palette.weekend
            } else {
                palette.weekday
            };
            let text = self.rasterize_text_with_size(
                label,
                scale,
                ink,
                self.style.font_size.clamp(13.0, 15.0),
            );
            draw_centered(
                &mut canvas,
                &text,
                column_center(column, s),
                WEEKDAY_CENTER_Y * s,
            );
        }

        for (index, date) in month_grid(month).into_iter().enumerate() {
            let Some(date) = date else {
                continue;
            };
            let column = index % 7;
            let row = index / 7;
            let center_x = column_center(column, s);
            let center_y = (GRID_TOP + row as i32 * ROW_HEIGHT + ROW_HEIGHT / 2) * s;
            let is_today = date == today;
            if is_today {
                rounded_rect(
                    &mut canvas,
                    PixelRect {
                        x: center_x - HIGHLIGHT_SIZE * s / 2,
                        y: center_y - HIGHLIGHT_SIZE * s / 2,
                        width: HIGHLIGHT_SIZE * s,
                        height: HIGHLIGHT_SIZE * s,
                    },
                    HIGHLIGHT_SIZE as f32 * scale as f32 / 2.0,
                    color(palette.today_background),
                );
            }
            let ink = if is_today {
                palette.today_foreground
            } else if date.year() != month.year() || date.month() != month.month() {
                palette.muted
            } else if date.weekday().number_from_monday() >= 6 {
                palette.weekend
            } else {
                palette.foreground
            };
            let text = self.rasterize_text_with_size(
                &date.day().to_string(),
                scale,
                ink,
                self.style.font_size.clamp(13.0, 15.0),
            );
            draw_centered(&mut canvas, &text, center_x, center_y);
        }

        PreparedCalendar {
            month,
            today,
            width: natural_size.0,
            height: natural_size.1,
            natural_size,
            scale,
            bitmap,
            navigation,
            hover: Color::from_rgba8(palette.header[0], palette.header[1], palette.header[2], 92),
        }
    }

    pub fn draw_calendar(
        &mut self,
        canvas: &mut PixmapMut<'_>,
        calendar: &PreparedCalendar,
        hitboxes: &mut Vec<MenuHitbox>,
        selected: Option<MenuSelection>,
    ) {
        canvas.fill(Color::TRANSPARENT);
        draw_premultiplied(
            canvas,
            0,
            0,
            calendar.bitmap.width(),
            calendar.bitmap.height(),
            calendar.bitmap.data(),
        );

        hitboxes.clear();
        hitboxes.reserve(calendar.navigation.len());
        hitboxes.extend(
            calendar
                .navigation
                .iter()
                .filter_map(|hitbox| calendar.clipped_hitbox(*hitbox)),
        );

        if let Some(hitbox) = calendar
            .navigation
            .iter()
            .find(|hitbox| Some(hitbox.selection) == selected)
        {
            stroke_rounded_rect(
                canvas,
                PixelRect {
                    x: hitbox.x + calendar.scale as i32 * 2,
                    y: hitbox.y + calendar.scale as i32 * 2,
                    width: hitbox.width - calendar.scale as i32 * 4,
                    height: hitbox.height - calendar.scale as i32 * 4,
                },
                7.0 * calendar.scale as f32,
                calendar.scale as f32,
                calendar.hover,
            );
        }
    }
}

fn navigation_hitboxes(scale: u32) -> [MenuHitbox; 3] {
    let s = scale as i32;
    [
        MenuHitbox {
            selection: MenuSelection::Item(-1),
            enabled: true,
            x: 14 * s,
            y: HEADER_TOP * s,
            width: 40 * s,
            height: HEADER_HEIGHT * s,
        },
        MenuHitbox {
            selection: MenuSelection::Item(0),
            enabled: true,
            x: 54 * s,
            y: HEADER_TOP * s,
            width: 180 * s,
            height: HEADER_HEIGHT * s,
        },
        MenuHitbox {
            selection: MenuSelection::Item(1),
            enabled: true,
            x: 234 * s,
            y: HEADER_TOP * s,
            width: 40 * s,
            height: HEADER_HEIGHT * s,
        },
    ]
}

fn column_center(column: usize, scale: i32) -> i32 {
    (CONTENT_LEFT + column as i32 * COLUMN_WIDTH + COLUMN_WIDTH / 2) * scale
}

fn color(rgba: [u8; 4]) -> Color {
    Color::from_rgba8(rgba[0], rgba[1], rgba[2], rgba[3])
}

fn draw_centered(canvas: &mut PixmapMut<'_>, text: &Pixmap, center_x: i32, center_y: i32) {
    draw_premultiplied(
        canvas,
        center_x - text.width() as i32 / 2,
        center_y - text.height() as i32 / 2,
        text.width(),
        text.height(),
        text.data(),
    );
}

fn rounded_path(rect: PixelRect, radius: f32) -> Option<Path> {
    if rect.width <= 0 || rect.height <= 0 {
        return None;
    }
    let (x, y, width, height) = (
        rect.x as f32,
        rect.y as f32,
        rect.width as f32,
        rect.height as f32,
    );
    let radius = radius.min(width / 2.0).min(height / 2.0);
    let control = radius * 0.552_284_8;
    let mut path = PathBuilder::new();
    path.move_to(x + radius, y);
    path.line_to(x + width - radius, y);
    path.cubic_to(
        x + width - radius + control,
        y,
        x + width,
        y + radius - control,
        x + width,
        y + radius,
    );
    path.line_to(x + width, y + height - radius);
    path.cubic_to(
        x + width,
        y + height - radius + control,
        x + width - radius + control,
        y + height,
        x + width - radius,
        y + height,
    );
    path.line_to(x + radius, y + height);
    path.cubic_to(
        x + radius - control,
        y + height,
        x,
        y + height - radius + control,
        x,
        y + height - radius,
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
    path.finish()
}

fn rounded_rect(canvas: &mut PixmapMut<'_>, rect: PixelRect, radius: f32, color: Color) {
    let Some(path) = rounded_path(rect, radius) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color(color);
    canvas.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

fn stroke_rounded_rect(
    canvas: &mut PixmapMut<'_>,
    rect: PixelRect,
    radius: f32,
    width: f32,
    color: Color,
) {
    let Some(path) = rounded_path(rect, radius) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color(color);
    let stroke = Stroke {
        width,
        ..Stroke::default()
    };
    canvas.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
}

fn chevron(canvas: &mut PixmapMut<'_>, x: i32, y: i32, scale: i32, backwards: bool, color: Color) {
    let direction = if backwards { -1.0 } else { 1.0 };
    let (x, y, scale) = (x as f32, y as f32, scale as f32);
    let mut path = PathBuilder::new();
    path.move_to(x - 3.0 * direction * scale, y - 5.0 * scale);
    path.line_to(x + 2.0 * direction * scale, y);
    path.line_to(x - 3.0 * direction * scale, y + 5.0 * scale);
    let mut paint = Paint::default();
    paint.set_color(color);
    let stroke = Stroke {
        width: 1.75 * scale,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    canvas.stroke_path(
        &path.finish().expect("calendar chevron path"),
        &paint,
        &stroke,
        Transform::identity(),
        None,
    );
}

#[cfg(test)]
mod tests {
    use chrono::{Datelike, NaiveDate};
    use tiny_skia::Pixmap;

    use super::*;
    use crate::{config::Config, render::Renderer};

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn palette() -> CalendarPalette {
        CalendarPalette {
            background: [21, 23, 28, 128],
            foreground: [220, 220, 220, 255],
            header: [245, 245, 245, 255],
            weekday: [205, 205, 205, 255],
            weekend: [224, 155, 94, 255],
            muted: [92, 96, 105, 255],
            today_background: [126, 156, 216, 255],
            today_foreground: [12, 15, 20, 255],
            border: [75, 79, 88, 255],
        }
    }

    #[test]
    fn leap_february_uses_monday_first_six_week_grid() {
        let grid = month_grid(date(2024, 2, 18));

        assert_eq!(grid[0], Some(date(2024, 1, 29)));
        assert_eq!(grid[3], Some(date(2024, 2, 1)));
        assert_eq!(grid[31], Some(date(2024, 2, 29)));
        assert_eq!(grid[32], Some(date(2024, 3, 1)));
        assert_eq!(grid[41], Some(date(2024, 3, 10)));
    }

    #[test]
    fn month_starting_monday_occupies_first_cell() {
        let grid = month_grid(date(2023, 5, 29));

        assert_eq!(grid[0], Some(date(2023, 5, 1)));
        assert_eq!(grid[6], Some(date(2023, 5, 7)));
        assert_eq!(grid[41], Some(date(2023, 6, 11)));
    }

    #[test]
    fn december_grid_and_month_shift_cross_year_boundary() {
        let grid = month_grid(date(2024, 12, 31));

        assert_eq!(grid[0], Some(date(2024, 11, 25)));
        assert_eq!(grid[6], Some(date(2024, 12, 1)));
        assert_eq!(grid[37], Some(date(2025, 1, 1)));
        assert_eq!(shifted_month(date(2024, 12, 31), 1), date(2025, 1, 1));
        assert_eq!(shifted_month(date(2025, 1, 31), -1), date(2024, 12, 1));
    }

    #[test]
    fn date_bounds_clamp_month_navigation_and_leave_unrepresentable_cells_empty() {
        let min_month = date(NaiveDate::MIN.year(), NaiveDate::MIN.month(), 1);
        let max_month = date(NaiveDate::MAX.year(), NaiveDate::MAX.month(), 1);

        assert_eq!(shifted_month(NaiveDate::MIN, -1), min_month);
        assert_eq!(shifted_month(NaiveDate::MAX, 1), max_month);
        assert!(month_grid(NaiveDate::MIN).contains(&Some(NaiveDate::MIN)));
        assert!(month_grid(NaiveDate::MAX).contains(&Some(NaiveDate::MAX)));
        assert!(month_grid(NaiveDate::MIN).iter().any(Option::is_none));
        assert!(month_grid(NaiveDate::MAX).iter().any(Option::is_none));
    }

    #[test]
    fn prepared_calendar_scales_and_normalizes_its_month() {
        let mut renderer = Renderer::new(&Config::default());

        for scale in [1, 2, 3] {
            let calendar =
                renderer.prepare_calendar(date(2024, 2, 29), date(2024, 2, 18), scale, &palette());
            assert_eq!(calendar.month, date(2024, 2, 1));
            assert_eq!(calendar.today, date(2024, 2, 18));
            assert_eq!(calendar.size(), (288 * scale, 280 * scale));
        }
    }

    #[test]
    fn header_hitboxes_navigate_in_visual_order() {
        let mut renderer = Renderer::new(&Config::default());
        let calendar =
            renderer.prepare_calendar(date(2024, 2, 1), date(2024, 2, 18), 1, &palette());

        assert_eq!(calendar.selection_at(30, 34), Some(MenuSelection::Item(-1)));
        assert_eq!(calendar.selection_at(144, 34), Some(MenuSelection::Item(0)));
        assert_eq!(calendar.selection_at(258, 34), Some(MenuSelection::Item(1)));
        assert_eq!(
            calendar.next_selection(None, false),
            Some(MenuSelection::Item(-1))
        );
        assert_eq!(
            calendar.next_selection(Some(MenuSelection::Item(-1)), false),
            Some(MenuSelection::Item(0))
        );
        assert_eq!(
            calendar.next_selection(Some(MenuSelection::Item(-1)), true),
            Some(MenuSelection::Item(1))
        );
    }

    #[test]
    fn constraints_clip_navigation_hitboxes() {
        let mut renderer = Renderer::new(&Config::default());
        let mut calendar =
            renderer.prepare_calendar(date(2024, 2, 1), date(2024, 2, 18), 2, &palette());

        calendar.constrain(300, 70);

        assert_eq!(calendar.size(), (300, 70));
        assert_eq!(calendar.selection_at(60, 68), Some(MenuSelection::Item(-1)));
        assert_eq!(calendar.selection_at(299, 68), Some(MenuSelection::Item(0)));
        assert_eq!(calendar.selection_at(300, 68), None);
        assert_eq!(calendar.selection_at(60, 70), None);
    }

    #[test]
    fn rendered_today_has_highlight_while_another_day_does_not() {
        let colors = palette();
        let mut renderer = Renderer::new(&Config::default());
        let calendar = renderer.prepare_calendar(date(2024, 2, 1), date(2024, 2, 18), 1, &colors);

        assert!(rect_contains_color(
            &calendar.bitmap,
            (241, 150, 28, 28),
            colors.today_background,
        ));
        assert!(!rect_contains_color(
            &calendar.bitmap,
            (130, 150, 28, 28),
            colors.today_background,
        ));
    }

    #[test]
    fn transparent_card_keeps_exact_configured_alpha_at_an_unpainted_interior_pixel() {
        let colors = palette();
        let mut renderer = Renderer::new(&Config::default());
        let calendar = renderer.prepare_calendar(date(2024, 2, 1), date(2024, 2, 18), 1, &colors);

        let pixel = calendar.bitmap.pixel(144, 62).unwrap();
        assert_eq!(pixel.alpha(), colors.background[3]);
        assert_eq!(calendar.bitmap.pixel(0, 0).unwrap().alpha(), 0);
    }

    #[test]
    fn drawing_reuses_prepared_bitmap_and_publishes_navigation_hitboxes() {
        let mut renderer = Renderer::new(&Config::default());
        let calendar =
            renderer.prepare_calendar(date(2024, 2, 1), date(2024, 2, 18), 1, &palette());
        let mut target = Pixmap::new(calendar.size().0, calendar.size().1).unwrap();
        let mut hitboxes = Vec::new();

        renderer.draw_calendar(
            &mut target.as_mut(),
            &calendar,
            &mut hitboxes,
            Some(MenuSelection::Item(1)),
        );

        assert_eq!(hitboxes.len(), 3);
        assert_eq!(hitboxes[0].selection, MenuSelection::Item(-1));
        assert!(target.data().iter().any(|channel| *channel != 0));
    }

    #[test]
    #[ignore = "writes /tmp/nibari-calendar-preview.png for explicit visual QA"]
    fn write_calendar_preview() {
        let mut renderer = Renderer::new(&Config::default());
        let calendar =
            renderer.prepare_calendar(date(2024, 2, 1), date(2024, 2, 18), 1, &palette());
        let mut target = Pixmap::new(calendar.size().0, calendar.size().1).unwrap();

        renderer.draw_calendar(&mut target.as_mut(), &calendar, &mut Vec::new(), None);
        target.save_png("/tmp/nibari-calendar-preview.png").unwrap();
    }

    fn rect_contains_color(pixmap: &Pixmap, rect: (u32, u32, u32, u32), color: [u8; 4]) -> bool {
        let (x, y, width, height) = rect;
        (y..y + height).any(|y| {
            (x..x + width).any(|x| {
                let pixel = pixmap.pixel(x, y).unwrap().demultiply();
                pixel.red() == color[0]
                    && pixel.green() == color[1]
                    && pixel.blue() == color[2]
                    && pixel.alpha() == color[3]
            })
        })
    }
}
