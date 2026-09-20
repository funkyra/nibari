use super::{
    MenuHitbox, MenuSelection, PixelRect, Renderer, draw_premultiplied, draw_premultiplied_clipped,
    fill_rect,
};
use crate::{
    config::{Config, WeatherUnits},
    weather::{Condition, WeatherState, temperature},
};
use chrono::Local;
use tiny_skia::{
    Color, LineCap, LineJoin, Paint, PathBuilder, Pixmap, PixmapMut, Rect, Stroke, Transform,
};

const WIDTH: u32 = 480;
const HEIGHT: u32 = 202;
pub struct WeatherPalette {
    background: [u8; 4],
    foreground: [u8; 4],
    muted: [u8; 4],
    accent: [u8; 4],
    border: [u8; 4],
}
impl From<&Config> for WeatherPalette {
    fn from(c: &Config) -> Self {
        Self {
            background: c.color_rgba(&c.weather_background),
            foreground: c.color_rgba(&c.weather_foreground),
            muted: c.color_rgba(&c.weather_muted),
            accent: c.color_rgba(&c.weather_accent),
            border: c.color_rgba(&c.weather_border),
        }
    }
}
pub struct PreparedWeather {
    bitmap: Pixmap,
    width: u32,
    height: u32,
    scale: u32,
    refresh: MenuHitbox,
    accent: Color,
}
impl PreparedWeather {
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

fn temp(value: Option<f64>, units: WeatherUnits) -> String {
    value.map_or_else(|| "—".into(), |v| format!("{:.0}°", temperature(v, units)))
}

impl Renderer {
    pub fn prepare_weather(
        &mut self,
        state: &WeatherState,
        units: WeatherUnits,
        scale: u32,
        palette: &WeatherPalette,
    ) -> PreparedWeather {
        let scale = scale.max(1);
        let s = scale as i32;
        let mut bitmap = Pixmap::new(WIDTH * scale, HEIGHT * scale).expect("small weather card");
        let mut canvas = bitmap.as_mut();
        fill_rect(
            &mut canvas,
            6 * s,
            6 * s,
            468 * s,
            190 * s,
            rgba(palette.background),
        );
        outline(
            &mut canvas,
            PixelRect {
                x: 6 * s,
                y: 6 * s,
                width: 468 * s,
                height: 190 * s,
            },
            scale as f32,
            rgba(palette.border),
        );
        let refresh = MenuHitbox {
            selection: MenuSelection::Item(0),
            enabled: true,
            x: 442 * s,
            y: 14 * s,
            width: 24 * s,
            height: 24 * s,
        };
        let mut arrow = PathBuilder::new();
        arrow.move_to(460.0, 22.0);
        arrow.cubic_to(452.0, 16.0, 444.0, 23.0, 450.0, 30.0);
        arrow.move_to(460.0, 17.0);
        arrow.line_to(460.0, 23.0);
        arrow.line_to(454.0, 23.0);
        stroke(&mut canvas, arrow, 1.2, scale as f32, rgba(palette.muted));
        let rect = |x, y, w, h| PixelRect {
            x: x * s,
            y: y * s,
            width: w * s,
            height: h * s,
        };
        if let Some(report) = &state.report {
            let icon = weather_icon(
                report.condition,
                report.night,
                76 * scale,
                palette.foreground,
            );
            draw_premultiplied(
                &mut canvas,
                27 * s,
                33 * s,
                icon.width(),
                icon.height(),
                icon.data(),
            );
            let value = format!("{:.0}", temperature(report.temperature, units));
            let size = if value.len() > 3 { 36.0 } else { 50.0 };
            let value = self.rasterize_text_with_size(&value, scale, palette.foreground, size);
            draw_premultiplied_clipped(
                &mut canvas,
                112 * s,
                35 * s,
                value.width(),
                value.height(),
                value.data(),
                rect(112, 28, 132, 82),
            );
            let unit = if units == WeatherUnits::Imperial {
                "°F"
            } else {
                "°C"
            };
            let unit_x = (112 * s + value.width() as i32 + 3 * s).min(218 * s);
            self.weather_text(
                &mut canvas,
                unit,
                PixelRect {
                    x: unit_x,
                    y: 37 * s,
                    width: 30 * s,
                    height: 28 * s,
                },
                (18.0, palette.foreground, scale),
            );
            self.weather_text(
                &mut canvas,
                &report.location.to_uppercase(),
                rect(250, 28, 186, 20),
                (11.0, palette.muted, scale),
            );
            let wind = report.wind.map_or_else(
                || "—".into(),
                |v| {
                    if units == WeatherUnits::Imperial {
                        format!("{:.0} mph", v * 0.621371)
                    } else {
                        format!("{v:.0} km/h")
                    }
                },
            );
            let humidity = report
                .humidity
                .map_or_else(|| "—".into(), |v| format!("{v:.0}%"));
            let feels = report.feels.map_or_else(
                || "—".into(),
                |v| format!("{:.0}{unit}", temperature(v, units)),
            );
            for (x, label, value) in [
                (250, "FEELS", feels),
                (322, "WIND", wind),
                (400, "HUMID", humidity),
            ] {
                self.weather_text(
                    &mut canvas,
                    label,
                    rect(x, 60, 70, 17),
                    (10.0, palette.muted, scale),
                );
                self.weather_text(
                    &mut canvas,
                    &value,
                    rect(x, 81, 70, 20),
                    (12.0, palette.foreground, scale),
                );
            }
            fill_rect(
                &mut canvas,
                22 * s,
                120 * s,
                436 * s,
                s,
                rgba(palette.border),
            );
            for (index, day) in report.days.iter().take(3).enumerate() {
                let x = 31 + index as i32 * 147;
                let icon = weather_icon(day.condition, false, 28 * scale, palette.foreground);
                draw_premultiplied(
                    &mut canvas,
                    x * s,
                    138 * s,
                    icon.width(),
                    icon.height(),
                    icon.data(),
                );
                self.weather_text(
                    &mut canvas,
                    &day.date.format("%A").to_string().to_uppercase(),
                    rect(x + 35, 134, 107, 18),
                    (9.0, palette.muted, scale),
                );
                let high = temp(day.high, units);
                let low = temp(day.low, units);
                self.weather_text(
                    &mut canvas,
                    &high,
                    rect(x + 35, 152, 42, 19),
                    (12.0, palette.accent, scale),
                );
                self.weather_text(
                    &mut canvas,
                    &low,
                    rect(x + 79, 152, 52, 19),
                    (12.0, palette.foreground, scale),
                );
            }
            if report.days.is_empty() {
                self.weather_text(
                    &mut canvas,
                    "Forecast unavailable",
                    rect(22, 138, 436, 27),
                    (12.0, palette.muted, scale),
                );
            }
        } else {
            let icon = weather_icon(Condition::Unknown, false, 60 * scale, palette.muted);
            draw_premultiplied(
                &mut canvas,
                30 * s,
                44 * s,
                icon.width(),
                icon.height(),
                icon.data(),
            );
            let title = if state.error.is_some() {
                "Weather unavailable"
            } else {
                "Loading weather…"
            };
            self.weather_text(
                &mut canvas,
                title,
                rect(115, 48, 310, 28),
                (18.0, palette.foreground, scale),
            );
            let detail = state
                .error
                .as_deref()
                .unwrap_or("Finding location and fetching forecast");
            self.weather_text(
                &mut canvas,
                detail,
                rect(115, 83, 310, 32),
                (10.0, palette.muted, scale),
            );
        }
        let footer = if state.error.is_some() && state.report.is_some() {
            "Open-Meteo · update failed; showing saved data".into()
        } else if let Some(updated) = state.updated_at {
            format!(
                "Open-Meteo · updated {}",
                updated.with_timezone(&Local).format("%H:%M")
            )
        } else {
            "Open-Meteo".into()
        };
        self.weather_text(
            &mut canvas,
            &footer,
            rect(22, 178, 436, 14),
            (
                9.0,
                if state.error.is_some() {
                    palette.accent
                } else {
                    palette.muted
                },
                scale,
            ),
        );
        PreparedWeather {
            bitmap,
            width: WIDTH * scale,
            height: HEIGHT * scale,
            scale,
            refresh,
            accent: rgba(palette.accent),
        }
    }
    fn weather_text(
        &mut self,
        canvas: &mut PixmapMut<'_>,
        text: &str,
        rect: PixelRect,
        style: (f32, [u8; 4], u32),
    ) {
        let text = self.rasterize_text_with_size(text, style.2, style.1, style.0);
        draw_premultiplied_clipped(
            canvas,
            rect.x,
            rect.y + (rect.height - text.height() as i32) / 2,
            text.width(),
            text.height(),
            text.data(),
            rect,
        );
    }
    pub fn draw_weather(
        &mut self,
        canvas: &mut PixmapMut<'_>,
        weather: &PreparedWeather,
        hitboxes: &mut Vec<MenuHitbox>,
        selected: Option<MenuSelection>,
    ) {
        canvas.fill(Color::TRANSPARENT);
        draw_premultiplied(
            canvas,
            0,
            0,
            weather.bitmap.width(),
            weather.bitmap.height(),
            weather.bitmap.data(),
        );
        hitboxes.clear();
        if let Some(h) = weather.refresh_hitbox() {
            hitboxes.push(h);
            if selected == Some(h.selection) {
                outline(
                    canvas,
                    PixelRect {
                        x: h.x,
                        y: h.y,
                        width: h.width,
                        height: h.height,
                    },
                    weather.scale as f32,
                    weather.accent,
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
fn stroke(canvas: &mut PixmapMut<'_>, path: PathBuilder, width: f32, scale: f32, color: Color) {
    if let Some(path) = path.finish() {
        let mut paint = Paint::default();
        paint.set_color(color);
        canvas.stroke_path(
            &path,
            &paint,
            &Stroke {
                width,
                line_cap: LineCap::Round,
                line_join: LineJoin::Round,
                ..Stroke::default()
            },
            Transform::from_scale(scale, scale),
            None,
        );
    }
}
fn cloud(p: &mut PathBuilder) {
    p.move_to(7.0, 22.0);
    p.cubic_to(0.0, 22.0, 1.0, 14.0, 7.0, 14.0);
    p.cubic_to(8.0, 5.0, 20.0, 5.0, 22.0, 14.0);
    p.cubic_to(30.0, 12.0, 31.0, 22.0, 24.0, 22.0);
    p.close();
}
fn sun(p: &mut PathBuilder, cx: f32, cy: f32, r: f32, rays: bool) {
    p.push_circle(cx, cy, r);
    if rays {
        for i in 0..8 {
            let angle = i as f32 * std::f32::consts::FRAC_PI_4;
            let (y, x) = angle.sin_cos();
            p.move_to(cx + x * (r + 3.0), cy + y * (r + 3.0));
            p.line_to(cx + x * (r + 5.0), cy + y * (r + 5.0));
        }
    }
}
pub(super) fn weather_icon(condition: Condition, night: bool, size: u32, color: [u8; 4]) -> Pixmap {
    let mut icon = Pixmap::new(size.max(1), size.max(1)).expect("small weather icon");
    let mut p = PathBuilder::new();
    match condition {
        Condition::Clear => {
            if night {
                p.move_to(20.0, 3.0);
                p.cubic_to(5.0, 0.0, 0.0, 22.0, 15.0, 27.0);
                p.cubic_to(22.0, 29.0, 28.0, 23.0, 29.0, 18.0);
                p.cubic_to(16.0, 21.0, 12.0, 8.0, 20.0, 3.0);
                p.close();
            } else {
                sun(&mut p, 16.0, 16.0, 6.0, true);
            }
        }
        Condition::PartlyCloudy => {
            if night {
                p.move_to(21.0, 2.0);
                p.cubic_to(17.0, 7.0, 22.0, 11.0, 27.0, 9.0);
                p.cubic_to(25.0, 13.0, 24.0, 14.0, 22.0, 14.0);
            } else {
                p.move_to(18.0, 7.0);
                p.cubic_to(24.0, 1.0, 31.0, 8.0, 26.0, 13.0);
                for (x, y, x2, y2) in [
                    (23.0, 2.0, 23.0, 0.5),
                    (29.0, 4.0, 30.5, 2.5),
                    (29.0, 9.0, 31.0, 9.0),
                ] {
                    p.move_to(x, y);
                    p.line_to(x2, y2);
                }
            }
            cloud(&mut p);
        }
        Condition::Unknown => {
            p.move_to(11.0, 10.0);
            p.cubic_to(11.0, 2.0, 23.0, 2.0, 22.0, 10.0);
            p.cubic_to(22.0, 15.0, 16.0, 14.0, 16.0, 20.0);
            p.move_to(16.0, 26.0);
            p.line_to(16.0, 26.5);
        }
        Condition::Cloudy => cloud(&mut p),
        Condition::Fog => {
            cloud(&mut p);
            p.move_to(4.0, 26.0);
            p.line_to(26.0, 26.0);
            p.move_to(8.0, 30.0);
            p.line_to(22.0, 30.0);
        }
        Condition::Rain => {
            cloud(&mut p);
            for x in [9.0, 16.0, 23.0] {
                p.move_to(x, 25.0);
                p.line_to(x - 2.0, 30.0);
            }
        }
        Condition::Snow => {
            cloud(&mut p);
            for x in [9.0, 22.0] {
                p.move_to(x - 2.0, 27.0);
                p.line_to(x + 2.0, 29.0);
                p.move_to(x - 2.0, 29.0);
                p.line_to(x + 2.0, 27.0);
                p.move_to(x, 25.5);
                p.line_to(x, 30.5);
            }
        }
        Condition::Thunder => {
            cloud(&mut p);
            p.move_to(17.0, 23.0);
            p.line_to(13.0, 27.0);
            p.line_to(18.0, 27.0);
            p.line_to(14.0, 31.0);
        }
    }
    stroke(&mut icon.as_mut(), p, 1.6, size as f32 / 32.0, rgba(color));
    icon
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::Config,
        weather::{WeatherDay, WeatherReport},
    };
    use chrono::NaiveDate;
    use tiny_skia::Pixmap;

    fn palette() -> WeatherPalette {
        WeatherPalette {
            background: [24, 28, 36, 173],
            foreground: [232, 238, 245, 255],
            muted: [139, 153, 170, 255],
            accent: [116, 190, 255, 255],
            border: [75, 91, 112, 255],
        }
    }

    fn report(night: bool, condition: Condition) -> WeatherReport {
        WeatherReport {
            location: "Helsinki".into(),
            temperature: 9.0,
            feels: Some(7.0),
            wind: Some(4.0),
            humidity: Some(77.0),
            condition,
            night,
            days: (1..=3)
                .map(|day| WeatherDay {
                    date: NaiveDate::from_ymd_opt(2026, 9, 15 + day).unwrap(),
                    high: Some(10.0 + day as f64),
                    low: Some(5.0 + day as f64),
                    condition: Condition::Rain,
                })
                .collect(),
        }
    }

    fn state(report: Option<WeatherReport>, error: Option<&str>) -> WeatherState {
        WeatherState {
            report,
            error: error.map(str::to_owned),
            updated_at: None,
        }
    }

    #[test]
    fn prepared_weather_scales_to_popup_size_and_preserves_background_alpha() {
        let mut renderer = Renderer::new(&Config::default());
        for scale in [1, 2, 3] {
            let weather = renderer.prepare_weather(
                &state(Some(report(false, Condition::Rain)), None),
                WeatherUnits::Metric,
                scale,
                &palette(),
            );
            assert_eq!(weather.size(), (480 * scale, 202 * scale));
            assert_eq!(weather.scale, scale);
            assert_eq!(weather.bitmap.pixel(0, 0).unwrap().alpha(), 0);
            let interior = weather
                .bitmap
                .pixel(10 * scale, 115 * scale)
                .unwrap()
                .demultiply();
            assert_eq!(interior.alpha(), palette().background[3]);
        }
    }

    #[test]
    fn constrain_refresh_hitbox_is_clipped_to_popup_viewport() {
        let mut renderer = Renderer::new(&Config::default());
        let mut weather = renderer.prepare_weather(
            &state(Some(report(false, Condition::Clear)), None),
            WeatherUnits::Metric,
            1,
            &palette(),
        );
        weather.constrain(450, 30);
        assert_eq!(
            weather.refresh,
            MenuHitbox {
                selection: MenuSelection::Item(0),
                enabled: true,
                x: 442,
                y: 14,
                width: 24,
                height: 24,
            }
        );
        assert_eq!(weather.selection_at(449, 29), Some(MenuSelection::Item(0)));
        assert_eq!(weather.selection_at(450, 29), None);
        assert_eq!(weather.selection_at(449, 30), None);
        weather.constrain(100, 100);
        assert_eq!(weather.next_selection(), None);
    }

    #[test]
    fn loading_error_and_no_data_states_render_without_panic() {
        let mut renderer = Renderer::new(&Config::default());
        for state in [
            state(None, None),
            state(None, Some("Weather unavailable")),
            state(Some(report(true, Condition::Cloudy)), None),
        ] {
            let weather = renderer.prepare_weather(&state, WeatherUnits::Metric, 1, &palette());
            let mut canvas = Pixmap::new(weather.size().0, weather.size().1).unwrap();
            renderer.draw_weather(&mut canvas.as_mut(), &weather, &mut Vec::new(), None);
            assert!(canvas.data().iter().any(|alpha| *alpha != 0));
        }
    }

    #[test]
    fn icons_distinguish_day_night_and_weather_variants() {
        let colors = [232, 238, 245, 201];
        let icons: Vec<_> = [
            Condition::Clear,
            Condition::PartlyCloudy,
            Condition::Cloudy,
            Condition::Fog,
            Condition::Rain,
            Condition::Snow,
            Condition::Thunder,
        ]
        .into_iter()
        .map(|condition| weather_icon(condition, false, 32, colors))
        .collect();
        assert!(
            icons
                .iter()
                .all(|icon| icon.data().iter().any(|alpha| *alpha != 0))
        );
        assert!(
            icons
                .windows(2)
                .all(|pair| pair[0].data() != pair[1].data())
        );
        for condition in [Condition::Clear, Condition::PartlyCloudy] {
            let day = weather_icon(condition, false, 32, colors);
            let night = weather_icon(condition, true, 32, colors);
            assert!(day.data().iter().any(|alpha| *alpha != 0));
            assert_ne!(
                day.data(),
                night.data(),
                "{condition:?} day/night icon should differ"
            );
        }
    }

    #[test]
    #[ignore = "writes /tmp/nibari-weather-preview.png for explicit visual QA"]
    fn write_weather_preview() {
        let config = Config {
            weather_background: "#222222F2".into(),
            weather_foreground: "#C2C2B0".into(),
            weather_muted: "#8A8A7E".into(),
            weather_accent: "#D7C483".into(),
            weather_border: "#78824B".into(),
            ..Config::default()
        };
        let mut renderer = Renderer::new(&config);
        let weather = renderer.prepare_weather(
            &state(
                Some(WeatherReport {
                    location: "Helsinki".into(),
                    temperature: 9.0,
                    feels: Some(7.0),
                    wind: Some(4.0),
                    humidity: Some(77.0),
                    condition: Condition::PartlyCloudy,
                    night: false,
                    days: vec![
                        WeatherDay {
                            date: NaiveDate::from_ymd_opt(2026, 9, 16).unwrap(),
                            high: Some(16.0),
                            low: Some(15.0),
                            condition: Condition::Rain,
                        },
                        WeatherDay {
                            date: NaiveDate::from_ymd_opt(2026, 9, 17).unwrap(),
                            high: Some(16.0),
                            low: Some(13.0),
                            condition: Condition::Rain,
                        },
                        WeatherDay {
                            date: NaiveDate::from_ymd_opt(2026, 9, 18).unwrap(),
                            high: Some(16.0),
                            low: Some(12.0),
                            condition: Condition::Rain,
                        },
                    ],
                }),
                None,
            ),
            WeatherUnits::Metric,
            1,
            &palette(),
        );
        let mut canvas = Pixmap::new(weather.size().0, weather.size().1).unwrap();
        renderer.draw_weather(&mut canvas.as_mut(), &weather, &mut Vec::new(), None);
        canvas.save_png("/tmp/nibari-weather-preview.png").unwrap();
    }
}
