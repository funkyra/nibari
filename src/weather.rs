use crate::config::{Config, WeatherUnits};
use anyhow::{Context, Result, bail, ensure};
use calloop::channel::Sender;
use chrono::{DateTime, NaiveDate, Utc};
use serde_json::Value;
use std::{
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Condition {
    #[default]
    Unknown,
    Clear,
    PartlyCloudy,
    Cloudy,
    Fog,
    Rain,
    Snow,
    Thunder,
}
impl Condition {
    fn from_code(code: Option<u64>) -> Self {
        match code {
            Some(0) => Self::Clear,
            Some(1 | 2) => Self::PartlyCloudy,
            Some(3) => Self::Cloudy,
            Some(45 | 48) => Self::Fog,
            Some(51..=67 | 80..=82) => Self::Rain,
            Some(71..=77 | 85 | 86) => Self::Snow,
            Some(95..=99) => Self::Thunder,
            _ => Self::Unknown,
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
struct Location {
    name: String,
    latitude: f64,
    longitude: f64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct WeatherReport {
    pub location: String,
    pub temperature: f64,
    pub feels: Option<f64>,
    pub wind: Option<f64>,
    pub humidity: Option<f64>,
    pub condition: Condition,
    pub night: bool,
    pub days: Vec<WeatherDay>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct WeatherDay {
    pub date: NaiveDate,
    pub high: Option<f64>,
    pub low: Option<f64>,
    pub condition: Condition,
}
#[derive(Clone, Debug, Default)]
pub struct WeatherState {
    pub report: Option<WeatherReport>,
    pub error: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
}
#[derive(Clone)]
pub struct WeatherOptions {
    location: String,
    coordinates: Option<(f64, f64)>,
    interval: Duration,
}
impl From<&Config> for WeatherOptions {
    fn from(c: &Config) -> Self {
        Self {
            location: c.weather_location.trim().into(),
            coordinates: c.weather_latitude.zip(c.weather_longitude),
            interval: Duration::from_secs(u64::from(c.weather_refresh_minutes) * 60),
        }
    }
}
pub struct WeatherHandle {
    wake: SyncSender<()>,
    shutdown: Arc<AtomicBool>,
}
impl WeatherHandle {
    pub fn spawn(sender: Sender<Arc<WeatherState>>, options: WeatherOptions) -> Result<Self> {
        let (wake, receiver) = mpsc::sync_channel(1);
        let shutdown = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&shutdown);
        thread::Builder::new()
            .name("nibari-weather".into())
            .spawn(move || {
                let mut state = WeatherState::default();
                let mut cached_location = None;
                let mut last_request = None;
                loop {
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    // A burst of clicks coalesces into one request, at most once a minute.
                    if last_request
                        .is_none_or(|at: Instant| at.elapsed() >= Duration::from_secs(60))
                    {
                        last_request = Some(Instant::now());
                        match fetch_report(&options, &mut cached_location) {
                            Ok(report) => {
                                state.report = Some(report);
                                state.error = None;
                                state.updated_at = Some(Utc::now());
                            }
                            Err(error) => {
                                log::warn!("weather update failed: {error:#}");
                                state.error = Some("Weather unavailable; retry in a moment".into());
                            }
                        }
                        if stop.load(Ordering::Acquire)
                            || sender.send(Arc::new(state.clone())).is_err()
                        {
                            break;
                        }
                    }
                    // Failed fetches retry after a minute. Normal idle refresh is 15 minutes.
                    let interval = if state.error.is_some() {
                        Duration::from_secs(60)
                    } else {
                        options.interval
                    };
                    let delay = interval
                        .saturating_sub(last_request.map_or(Duration::ZERO, |at| at.elapsed()));
                    match receiver.recv_timeout(delay) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .context("failed to start weather worker")?;
        Ok(Self { wake, shutdown })
    }
    pub fn refresh(&self) {
        let _ = self.wake.try_send(());
    }
}
impl Drop for WeatherHandle {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        let _ = self.wake.try_send(());
    }
}

fn fetch_report(options: &WeatherOptions, cached: &mut Option<Location>) -> Result<WeatherReport> {
    let location = if let Some((latitude, longitude)) = options.coordinates {
        Location {
            name: if options.location.is_empty() {
                format!("{latitude:.3}, {longitude:.3}")
            } else {
                options.location.clone()
            },
            latitude,
            longitude,
        }
    } else if options.location.is_empty() {
        // Re-detect on each full refresh so travel or VPN changes are reflected.
        parse_location(
            &get_json("https://ipwho.is/?fields=success,city,latitude,longitude,message")?,
            true,
        )?
    } else if let Some(location) = cached {
        location.clone()
    } else {
        let url = format!(
            "https://geocoding-api.open-meteo.com/v1/search?name={}&count=1&language=en&format=json",
            encode(&options.location)
        );
        let location = parse_location(&get_json(&url)?, false)?;
        *cached = Some(location.clone());
        location
    };
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={}&longitude={}&current=temperature_2m,apparent_temperature,relative_humidity_2m,wind_speed_10m,weather_code,is_day&daily=weather_code,temperature_2m_max,temperature_2m_min&forecast_days=4&timezone=auto",
        location.latitude, location.longitude
    );
    parse_report(&get_json(&url)?, location)
}
fn get_json(url: &str) -> Result<Value> {
    let output = Command::new("curl")
        .args([
            "--disable",
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "5",
            "--max-time",
            "12",
            "--max-filesize",
            "262144",
            "--url",
            url,
        ])
        .output()
        .context("curl is unavailable")?;
    ensure!(
        output.status.success(),
        "weather HTTPS request failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    ensure!(
        output.stdout.len() <= 262144,
        "weather response is too large"
    );
    serde_json::from_slice(&output.stdout).context("invalid weather JSON")
}
fn encode(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[(byte >> 4) as usize]));
            out.push(char::from(HEX[(byte & 15) as usize]));
        }
    }
    out
}
fn parse_location(value: &Value, auto: bool) -> Result<Location> {
    let row = if auto {
        ensure!(
            value["success"].as_bool() == Some(true),
            "automatic location unavailable"
        );
        value
    } else {
        value["results"]
            .as_array()
            .and_then(|v| v.first())
            .context("city not found; try coordinates")?
    };
    let latitude = number(&row["latitude"]).context("location has no latitude")?;
    let longitude = number(&row["longitude"]).context("location has no longitude")?;
    ensure!(
        (-90.0..=90.0).contains(&latitude) && (-180.0..=180.0).contains(&longitude),
        "invalid location coordinates"
    );
    let name = row[if auto { "city" } else { "name" }]
        .as_str()
        .unwrap_or_default()
        .trim();
    let name = if name.is_empty() {
        format!("{latitude:.3}, {longitude:.3}")
    } else {
        name.chars().filter(|c| !c.is_control()).take(100).collect()
    };
    Ok(Location {
        name,
        latitude,
        longitude,
    })
}
fn number(value: &Value) -> Option<f64> {
    value.as_f64().filter(|v| v.is_finite())
}
fn parse_report(value: &Value, location: Location) -> Result<WeatherReport> {
    if value["error"].as_bool() == Some(true) {
        bail!("weather service rejected request");
    }
    let current = &value["current"];
    let temperature =
        number(&current["temperature_2m"]).context("current temperature unavailable")?;
    let date = current["time"]
        .as_str()
        .and_then(|s| s.get(..10))
        .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        .context("weather date unavailable")?;
    let daily = &value["daily"];
    let days = daily["time"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(i, d)| {
            let day = NaiveDate::parse_from_str(d.as_str()?, "%Y-%m-%d").ok()?;
            (day > date).then(|| WeatherDay {
                date: day,
                high: number(&daily["temperature_2m_max"][i]),
                low: number(&daily["temperature_2m_min"][i]),
                condition: Condition::from_code(daily["weather_code"][i].as_u64()),
            })
        })
        .take(3)
        .collect();
    Ok(WeatherReport {
        location: location.name,
        temperature,
        feels: number(&current["apparent_temperature"]),
        wind: number(&current["wind_speed_10m"]).filter(|n| *n >= 0.0),
        humidity: number(&current["relative_humidity_2m"]).filter(|n| (0.0..=100.0).contains(n)),
        condition: Condition::from_code(current["weather_code"].as_u64()),
        night: current["is_day"].as_u64() == Some(0),
        days,
    })
}
pub fn temperature(value: f64, units: WeatherUnits) -> f64 {
    if units == WeatherUnits::Imperial {
        value * 1.8 + 32.0
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn location() -> Location {
        Location {
            name: "Helsinki".into(),
            latitude: 60.17,
            longitude: 24.94,
        }
    }
    #[test]
    fn city_query_encodes_separators_and_utf8() {
        assert_eq!(encode("New York&x=1"), "New%20York%26x%3D1");
        assert_eq!(encode("Уфа"), "%D0%A3%D1%84%D0%B0");
    }
    #[test]
    fn automatic_location_requires_success_and_valid_coordinates() {
        let v = json!({"success":true,"city":"Helsinki","latitude":60.17,"longitude":24.94});
        assert_eq!(parse_location(&v, true).unwrap(), location());
        for v in [
            json!({"success":false,"message":"quota"}),
            json!({"success":true,"latitude":91,"longitude":0}),
            json!({"success":true,"city":"No coordinates"}),
        ] {
            assert!(parse_location(&v, true).is_err());
        }
    }
    #[test]
    fn named_location_uses_geocoding_result_and_rejects_empty_results() {
        let v = json!({"results":[{"name":"Helsinki","latitude":60.17,"longitude":24.94}]});
        assert_eq!(parse_location(&v, false).unwrap(), location());
        assert!(parse_location(&json!({"results":[]}), false).is_err());
    }
    #[test]
    fn forecast_uses_location_date_and_keeps_missing_values_unknown() {
        let v = json!({"current":{"time":"2026-09-16T00:15","temperature_2m":9.4,"apparent_temperature":null},"daily":{"time":["2026-09-15","2026-09-16","2026-09-17","2026-09-18","2026-09-19","2026-09-20"],"temperature_2m_max":[10,11,12,null,14,15],"temperature_2m_min":[2,3,4,5,6,7]}});
        let r = parse_report(&v, location()).unwrap();
        assert_eq!(r.temperature, 9.4);
        assert_eq!(r.feels, None);
        assert_eq!(
            r.days
                .iter()
                .map(|d| d.date.to_string())
                .collect::<Vec<_>>(),
            ["2026-09-17", "2026-09-18", "2026-09-19"]
        );
        assert_eq!(r.days[0].high, Some(12.0));
        assert_eq!(r.days[1].high, None);
    }
    #[test]
    fn unavailable_current_weather_is_an_error_not_zero_degrees() {
        for v in [
            json!({}),
            json!({"error":true,"reason":"bad location"}),
            json!({"current":{"time":"2026-09-16T00:15","temperature_2m":null}}),
        ] {
            assert!(parse_report(&v, location()).is_err());
        }
    }

    #[test]
    fn temperature_conversion_handles_freezing_and_absolute_zero_points() {
        assert_eq!(temperature(0.0, WeatherUnits::Imperial), 32.0);
        assert_eq!(temperature(-40.0, WeatherUnits::Imperial), -40.0);
        assert_eq!(temperature(12.5, WeatherUnits::Metric), 12.5);
    }

    #[test]
    #[ignore = "requires live HTTPS access to Open-Meteo and IP geolocation services"]
    fn live_fetch_supports_auto_city_and_pinned_coordinates() {
        let options = [
            WeatherOptions {
                location: String::new(),
                coordinates: None,
                interval: Duration::from_secs(900),
            },
            WeatherOptions {
                location: "Helsinki".into(),
                coordinates: None,
                interval: Duration::from_secs(900),
            },
            WeatherOptions {
                location: "Pinned location".into(),
                coordinates: Some((60.1695, 24.9354)),
                interval: Duration::from_secs(900),
            },
        ];
        let mut cached = None;
        let auto = fetch_report(&options[0], &mut cached).expect("automatic location fetch");
        assert!(!auto.location.trim().is_empty());
        assert!(auto.temperature.is_finite());
        assert_eq!(auto.days.len(), 3);
        assert!(auto.days.windows(2).all(|days| days[0].date < days[1].date));
        assert!(
            auto.days
                .iter()
                .all(|day| day.date > Utc::now().date_naive())
        );

        let city = fetch_report(&options[1], &mut None).expect("Helsinki fetch");
        assert_eq!(city.location, "Helsinki");
        assert!(city.temperature.is_finite());
        assert_eq!(city.days.len(), 3);

        let pinned = fetch_report(&options[2], &mut None).expect("pinned coordinates fetch");
        assert_eq!(pinned.location, "Pinned location");
        assert!(pinned.temperature.is_finite());
        assert_eq!(pinned.days.len(), 3);
    }
}
