use std::{
    ffi::OsString,
    fs,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use chrono::format::{Item, StrftimeItems};
use serde::Deserialize;

const DEFAULT_CONFIG: &str = include_str!("../config.example.toml");

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ClockPosition {
    #[default]
    Right,
    Center,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WeatherUnits {
    #[default]
    Metric,
    Imperial,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub height: u32,
    pub font_family: String,
    pub font_size: f32,
    pub background: String,
    pub foreground: String,
    pub padding: u32,
    pub tray_icon_size: u32,
    pub tray_spacing: u32,
    pub tray_drawer: bool,
    pub tray_drawer_duration_ms: u32,
    pub workspace_width: u32,
    pub workspace_focused_background: String,
    pub workspace_active_background: String,
    pub workspace_active_foreground: String,
    pub workspace_occupied_foreground: String,
    pub workspace_empty_foreground: String,
    pub workspace_urgent_foreground: String,
    pub media_enabled: bool,
    // Accept the removed option so existing configurations still start.
    #[serde(rename = "appmenu_enabled")]
    pub _removed_appmenu: serde::de::IgnoredAny,
    pub show_tasks: bool,
    pub task_icon_size: u32,
    pub task_spacing: u32,
    pub task_padding: u32,
    pub task_background: String,
    pub task_focused_background: String,
    pub task_urgent_background: String,
    pub task_foreground: String,
    pub task_focused_foreground: String,
    pub weather_enabled: bool,
    pub weather_location: String,
    pub weather_latitude: Option<f64>,
    pub weather_longitude: Option<f64>,
    pub weather_units: WeatherUnits,
    pub weather_refresh_minutes: u32,
    pub weather_background: String,
    pub weather_foreground: String,
    pub weather_muted: String,
    pub weather_accent: String,
    pub weather_border: String,
    pub network_enabled: bool,
    pub network_interface: String,
    pub network_ping_target: String,
    pub network_background: String,
    pub network_foreground: String,
    pub network_muted: String,
    pub network_accent: String,
    pub network_border: String,
    pub clock_format: String,
    pub clock_position: ClockPosition,
    pub calendar_enabled: bool,
    pub calendar_background: String,
    pub calendar_foreground: String,
    pub calendar_header: String,
    pub calendar_weekday: String,
    pub calendar_weekend: String,
    pub calendar_muted: String,
    pub calendar_today_background: String,
    pub calendar_today_foreground: String,
    pub calendar_border: String,

    pub icon_theme: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            height: 28,
            font_family: "Fira Code Medium".into(),
            font_size: 8.0,
            background: "#282828".into(),
            foreground: "#DCDCCC".into(),
            padding: 8,
            tray_icon_size: 18,
            tray_spacing: 6,
            tray_drawer: false,
            tray_drawer_duration_ms: 600,
            workspace_width: 28,
            workspace_focused_background: "#3C3836".into(),
            workspace_active_background: "#6F6F6F".into(),
            workspace_active_foreground: "#F0DFAF".into(),
            workspace_occupied_foreground: "#DCDCCC".into(),
            workspace_empty_foreground: "#6F6F6F".into(),
            workspace_urgent_foreground: "#CC9393".into(),
            media_enabled: true,
            _removed_appmenu: serde::de::IgnoredAny,
            show_tasks: false,
            task_icon_size: 18,
            task_spacing: 2,
            task_padding: 6,
            task_background: "#282828".into(),
            task_focused_background: "#3C3836".into(),
            task_urgent_background: "#282828".into(),
            task_foreground: "#DCDCCC".into(),
            task_focused_foreground: "#F0DFAF".into(),
            clock_format: "%a %b %d, %H:%M".into(),
            weather_enabled: false,
            weather_location: String::new(),
            weather_latitude: None,
            weather_longitude: None,
            weather_units: WeatherUnits::Metric,
            weather_refresh_minutes: 15,
            weather_background: "#282828F2".into(),
            weather_foreground: "#DCDCCC".into(),
            weather_muted: "#6F6F6F".into(),
            weather_accent: "#F0DFAF".into(),
            weather_border: "#6F6F6F".into(),
            network_enabled: false,
            network_interface: String::new(),
            network_ping_target: "1.1.1.1".into(),
            network_background: "#282828F2".into(),
            network_foreground: "#DCDCCC".into(),
            network_muted: "#6F6F6F".into(),
            network_accent: "#F0DFAF".into(),
            network_border: "#6F6F6F".into(),
            clock_position: ClockPosition::Right,
            calendar_enabled: true,
            calendar_background: "#282828F2".into(),
            calendar_foreground: "#DCDCCC".into(),
            calendar_header: "#F0DFAF".into(),
            calendar_weekday: "#DCDCCC".into(),
            calendar_weekend: "#CC9393".into(),
            calendar_muted: "#6F6F6F".into(),
            calendar_today_background: "#3C3836".into(),
            calendar_today_foreground: "#F0DFAF".into(),
            calendar_border: "#6F6F6F".into(),
            icon_theme: None,
        }
    }
}

impl Config {
    pub fn load_from_args(args: impl Iterator<Item = OsString>) -> Result<Self> {
        let mut args = args;
        let mut path = None;

        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("-c" | "--config") => {
                    let value = args.next().context("--config must be followed by a path")?;
                    path = Some(PathBuf::from(value));
                }
                Some("--print-default-config") => {
                    print!("{DEFAULT_CONFIG}");
                    std::process::exit(0);
                }
                Some("-h" | "--help") => {
                    println!(
                        "nibari [--config PATH]\nnibari --hide | --show | --toggle\n\n  -c, --config PATH          path to the config file\n      --print-default-config print the default config\n      --hide                 hide the running bar and release its space\n      --show                 show the running bar\n      --toggle               toggle the running bar's visibility"
                    );
                    std::process::exit(0);
                }
                _ => bail!("unknown argument: {}", arg.to_string_lossy()),
            }
        }

        Self::load(path.as_deref())
    }

    pub fn load(explicit_path: Option<&Path>) -> Result<Self> {
        let path = explicit_path.map(Path::to_path_buf).or_else(default_path);
        match path {
            Some(path) => Self::load_path(&path, explicit_path.is_some()),
            None => Ok(Self::default()),
        }
    }

    fn load_path(path: &Path, required: bool) -> Result<Self> {
        let source = match fs::read_to_string(path) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !required => {
                if let Err(error) = write_config(path, None, DEFAULT_CONFIG) {
                    log::warn!("could not create config {}: {error:#}", path.display());
                }
                return Ok(Self::default());
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to read config {}", path.display()));
            }
        };
        let config: Self = toml::from_str(&source)
            .with_context(|| format!("error in config {}", path.display()))?;
        config.validate()?;
        // Validate before touching the file. A failure to save defaults must not
        // prevent a valid (for example, read-only) configuration from loading.
        let update = || -> Result<()> {
            if let Some(updated) = completed_config(&source)? {
                toml::from_str::<Self>(&updated)?.validate()?;
                write_config(path, Some(&source), &updated)?;
                log::info!("added missing settings to {}", path.display());
            }
            Ok(())
        };
        if let Err(error) = update() {
            log::warn!("could not update config {}: {error:#}", path.display());
        }
        log::info!("config: {}", path.display());
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.show_tasks && self.clock_position != ClockPosition::Center {
            bail!("show_tasks requires clock_position = 'center'");
        }
        if !(16..=256).contains(&self.height) {
            bail!("height must be in the range 16..=256");
        }
        if !(6.0..=96.0).contains(&self.font_size) {
            bail!("font_size must be in the range 6..=96");
        }
        if self.font_family.trim().is_empty() {
            bail!("font_family cannot be empty");
        }
        if self.tray_icon_size == 0 || self.tray_icon_size > self.height {
            bail!("tray_icon_size must be greater than 0 and no greater than height");
        }
        if self.padding > 256 || self.tray_spacing > 256 {
            bail!("padding and tray_spacing must be no greater than 256");
        }
        if !(16..=128).contains(&self.workspace_width) {
            bail!("workspace_width must be in the range 16..=128");
        }
        if self.task_icon_size == 0 || self.task_icon_size > self.height {
            bail!("task_icon_size must be greater than 0 and no greater than height");
        }
        if self.task_spacing > 256 || self.task_padding > 256 {
            bail!("task_spacing and task_padding must be no greater than 256");
        }
        if StrftimeItems::new(&self.clock_format).any(|item| item == Item::Error) {
            bail!("clock_format contains an invalid strftime sequence");
        }

        match (self.weather_latitude, self.weather_longitude) {
            (None, None) => {}
            (Some(lat), Some(lon))
                if lat.is_finite()
                    && lon.is_finite()
                    && (-90.0..=90.0).contains(&lat)
                    && (-180.0..=180.0).contains(&lon) => {}
            _ => bail!(
                "weather_latitude and weather_longitude must both be set, within -90..90 and -180..180"
            ),
        }
        if !(5..=1440).contains(&self.weather_refresh_minutes) {
            bail!("weather_refresh_minutes must be in the range 5..=1440");
        }
        if self.weather_location.len() > 256 || self.weather_location.chars().any(char::is_control)
        {
            bail!(
                "weather_location must be a city name of at most 256 bytes, without control characters"
            );
        }
        if !self.network_interface.is_empty()
            && (self.network_interface.len() >= 16
                || self.network_interface == "."
                || self.network_interface == ".."
                || !self
                    .network_interface
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_.-:".contains(&c)))
        {
            bail!("network_interface must be an interface name or empty for automatic selection");
        }
        self.network_ping_target
            .parse::<std::net::IpAddr>()
            .context("network_ping_target must be a numeric IPv4 or IPv6 address")?;
        parse_color(&self.background).context("invalid background")?;
        parse_color(&self.foreground).context("invalid foreground")?;
        [
            (
                "workspace_focused_background",
                &self.workspace_focused_background,
            ),
            (
                "workspace_active_background",
                &self.workspace_active_background,
            ),
            (
                "workspace_active_foreground",
                &self.workspace_active_foreground,
            ),
            (
                "workspace_occupied_foreground",
                &self.workspace_occupied_foreground,
            ),
            (
                "workspace_empty_foreground",
                &self.workspace_empty_foreground,
            ),
            (
                "workspace_urgent_foreground",
                &self.workspace_urgent_foreground,
            ),
            ("calendar_background", &self.calendar_background),
            ("calendar_foreground", &self.calendar_foreground),
            ("calendar_header", &self.calendar_header),
            ("calendar_weekday", &self.calendar_weekday),
            ("calendar_weekend", &self.calendar_weekend),
            ("calendar_muted", &self.calendar_muted),
            ("calendar_today_background", &self.calendar_today_background),
            ("calendar_today_foreground", &self.calendar_today_foreground),
            ("calendar_border", &self.calendar_border),
            ("weather_background", &self.weather_background),
            ("weather_foreground", &self.weather_foreground),
            ("weather_muted", &self.weather_muted),
            ("weather_accent", &self.weather_accent),
            ("weather_border", &self.weather_border),
            ("network_background", &self.network_background),
            ("network_foreground", &self.network_foreground),
            ("network_muted", &self.network_muted),
            ("network_accent", &self.network_accent),
            ("network_border", &self.network_border),
            ("task_background", &self.task_background),
            ("task_focused_background", &self.task_focused_background),
            ("task_urgent_background", &self.task_urgent_background),
            ("task_foreground", &self.task_foreground),
            ("task_focused_foreground", &self.task_focused_foreground),
        ]
        .into_iter()
        .try_for_each(|(name, color)| {
            parse_color(color)
                .with_context(|| format!("invalid {name}"))
                .map(|_| ())
        })?;
        Ok(())
    }

    pub fn background_rgba(&self) -> [u8; 4] {
        parse_color(&self.background).expect("config was validated")
    }

    pub fn foreground_rgba(&self) -> [u8; 4] {
        parse_color(&self.foreground).expect("config was validated")
    }

    pub fn color_rgba(&self, color: &str) -> [u8; 4] {
        parse_color(color).expect("config was validated")
    }
}

#[cfg(test)]
mod calendar_config_tests {
    use super::*;

    #[test]
    fn weather_config_validates_coordinate_pairs_units_and_refresh_interval() {
        for source in [
            "weather_enabled = true",
            "weather_location = 'Уфа'",
            "weather_latitude = 0.0\nweather_longitude = 0.0\nweather_units = 'imperial'\nweather_background = '#22222280'",
        ] {
            let c: Config = toml::from_str(source).unwrap();
            c.validate().unwrap();
        }
        for source in [
            "weather_latitude = 10.0",
            "weather_latitude = nan\nweather_longitude = 1.0",
            "weather_latitude = 91.0\nweather_longitude = 0.0",
            "weather_latitude = 0.0\nweather_longitude = -181.0",
            "weather_refresh_minutes = 0",
            "weather_foreground = 'bad'",
        ] {
            let c: Config = toml::from_str(source).unwrap();
            assert!(c.validate().is_err(), "{source}");
        }
        assert!(toml::from_str::<Config>("weather_units = 'kelvin'").is_err());
    }

    #[test]
    fn network_options_accept_numeric_probe_and_rgba_palette() {
        let config: Config = toml::from_str("network_enabled = true\nnetwork_interface = 'eth0'\nnetwork_ping_target = '1.1.1.1'\nnetwork_background = '#222222CC'").unwrap();
        config.validate().unwrap();
        for source in [
            "network_interface = '../eth0'",
            "network_ping_target = '--help'",
            "network_accent = 'invalid'",
        ] {
            let invalid: Config = toml::from_str(source).unwrap();
            assert!(invalid.validate().is_err());
        }
    }

    #[test]
    fn accepts_centered_formatted_clock_and_rgba_calendar_palette() {
        let config: Config = toml::from_str("clock_position = 'center'\nclock_format = '%d.%m.%Y %H:%M'\ncalendar_background = '#222222CC'\ncalendar_enabled = true").unwrap();
        config.validate().unwrap();
        assert_eq!(config.clock_format, "%d.%m.%Y %H:%M");
    }

    #[test]
    fn rejects_invalid_clock_position_and_calendar_color() {
        assert!(toml::from_str::<Config>("clock_position = 'middle'").is_err());
        let config: Config = toml::from_str("calendar_border = 'invalid'").unwrap();
        assert!(config.validate().is_err());
    }
}

fn completed_config(source: &str) -> Result<Option<String>> {
    // Parse the user's TOML: comments, quoted keys and multiline values must not
    // be mistaken for settings. Only the bundled, flat template is scanned by line.
    let existing: toml::Table = toml::from_str(source)?;
    let mut updated: Option<String> = None;
    let mut block_start = 0;
    let mut offset = 0;
    for line in DEFAULT_CONFIG.split_inclusive('\n') {
        offset += line.len();
        let line = line.trim();
        if line.is_empty() {
            block_start = offset;
        } else if !line.starts_with('#') {
            let (key, _) = line
                .split_once('=')
                .context("invalid default config template")?;
            if !existing.contains_key(key.trim()) {
                let output = updated.get_or_insert_with(|| {
                    let mut output = source.to_owned();
                    if !output.ends_with('\n') {
                        output.push('\n');
                    }
                    output.push('\n');
                    output
                });
                output.push_str(&DEFAULT_CONFIG[block_start..offset]);
            }
            block_start = offset;
        }
    }
    Ok(updated)
}

fn write_config(path: &Path, original: Option<&str>, updated: &str) -> Result<()> {
    // Resolve existing symlinks so replacing the file preserves dotfile links.
    let target = if original.is_some() {
        fs::canonicalize(path)?
    } else {
        path.to_path_buf()
    };
    let parent = target.parent().context("config path has no parent")?;
    let permissions = if original.is_some() {
        let permissions = fs::metadata(&target)?.permissions();
        if permissions.readonly() {
            bail!("config is read-only");
        }
        Some(permissions)
    } else {
        fs::create_dir_all(parent)?;
        None
    };
    let temporary = parent.join(format!(".nibari-config-{}.tmp", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    let result = (|| -> Result<()> {
        file.write_all(updated.as_bytes())?;
        if let Some(permissions) = permissions {
            file.set_permissions(permissions)?;
        }
        file.sync_all()?;
        if let Some(original) = original {
            if fs::read_to_string(&target)? != original {
                bail!("config changed while loading; skipping update");
            }
            fs::rename(&temporary, &target)?;
        } else {
            // Publish without overwriting a file created by another process.
            fs::hard_link(&temporary, &target)?;
        }
        Ok(())
    })();
    let _ = fs::remove_file(&temporary);
    result
}

fn default_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(path).join("nibari/config.toml"));
    }

    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".config/nibari/config.toml"))
}

fn parse_color(value: &str) -> Result<[u8; 4]> {
    let hex = value
        .strip_prefix('#')
        .context("expected a color in the form #RRGGBB or #RRGGBBAA")?;
    if hex.len() != 6 && hex.len() != 8 {
        bail!("expected a color in the form #RRGGBB or #RRGGBBAA");
    }

    let channel = |offset| {
        u8::from_str_radix(&hex[offset..offset + 2], 16)
            .context("color contains a non-hexadecimal digit")
    };

    Ok([
        channel(0)?,
        channel(2)?,
        channel(4)?,
        if hex.len() == 8 { channel(6)? } else { 255 },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "nibari-config-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn startup_adds_missing_settings_preserving_user_text_and_values() {
        let directory = TestDirectory::new();
        let path = directory.0.join("config.toml");
        let source = "# My settings\n\"height\" = 32\ntray_drawer = true\ntray_drawer_duration_ms = 123\n# trailing comment";
        fs::write(&path, source).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();

        let config = Config::load(Some(&path)).unwrap();
        let updated = fs::read_to_string(&path).unwrap();
        let table: toml::Table = toml::from_str(&updated).unwrap();
        assert!(
            table.contains_key("show_tasks"),
            "startup should persist missing defaults"
        );
        assert!(updated.starts_with(source));
        assert_eq!(config.height, 32);
        assert!(config.tray_drawer);
        assert_eq!(config.tray_drawer_duration_ms, 123);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        for key in toml::from_str::<toml::Table>(DEFAULT_CONFIG)
            .unwrap()
            .keys()
        {
            assert!(table.contains_key(key), "missing setting: {key}");
        }

        let metadata = fs::metadata(&path).unwrap();
        Config::load(Some(&path)).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), updated);
        assert_eq!(fs::metadata(&path).unwrap().ino(), metadata.ino());
        assert_eq!(
            fs::metadata(&path).unwrap().modified().unwrap(),
            metadata.modified().unwrap()
        );
    }

    #[test]
    fn startup_updates_symlink_target_and_parses_multiline_values() {
        let directory = TestDirectory::new();
        let target = directory.0.join("settings.toml");
        let path = directory.0.join("config.toml");
        let source = "clock_format = '''time\ntray_drawer = true\n'''\n# tray_drawer = true\n";
        fs::write(&target, source).unwrap();
        symlink("settings.toml", &path).unwrap();

        Config::load(Some(&path)).unwrap();
        assert!(fs::symlink_metadata(&path).unwrap().is_symlink());
        let updated = fs::read_to_string(&target).unwrap();
        assert!(updated.starts_with(source));
        let table: toml::Table = toml::from_str(&updated).unwrap();
        assert_eq!(
            table.get("tray_drawer").and_then(toml::Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn startup_leaves_invalid_configs_untouched() {
        let directory = TestDirectory::new();
        let path = directory.0.join("config.toml");
        for source in ["height = 1", "height = ", "unknown = true"] {
            fs::write(&path, source).unwrap();
            assert!(Config::load(Some(&path)).is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), source);
        }
    }

    #[test]
    fn startup_creates_default_config_but_rejects_missing_explicit_path() {
        let directory = TestDirectory::new();
        let path = directory.0.join("nibari/config.toml");
        assert!(Config::load(Some(&path)).is_err());
        assert!(!path.exists());
        Config::load_path(&path, false).unwrap();
        let source = fs::read_to_string(&path).unwrap();
        toml::from_str::<Config>(&source)
            .unwrap()
            .validate()
            .unwrap();
        assert!(completed_config(&source).unwrap().is_none());
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn startup_loads_read_only_config_without_modifying_it() {
        let directory = TestDirectory::new();
        let path = directory.0.join("config.toml");
        let source = "tray_drawer = true\n";
        fs::write(&path, source).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        assert!(Config::load(Some(&path)).unwrap().tray_drawer);
        assert_eq!(fs::read_to_string(&path).unwrap(), source);
    }

    #[test]
    fn config_write_does_not_overwrite_edits_made_since_loading() {
        let directory = TestDirectory::new();
        let path = directory.0.join("config.toml");
        let latest = "height = 40\n";
        fs::write(&path, latest).unwrap();
        assert!(write_config(&path, Some("height = 28\n"), DEFAULT_CONFIG).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), latest);
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
        assert!(write_config(&path, None, DEFAULT_CONFIG).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), latest);
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
    }

    #[test]
    fn startup_loads_config_when_temporary_file_cannot_be_created() {
        let directory = TestDirectory::new();
        let path = directory.0.join("config.toml");
        let temporary = directory
            .0
            .join(format!(".nibari-config-{}.tmp", std::process::id()));
        fs::write(&temporary, "leave me alone").unwrap();
        fs::write(&path, "height = 40\n").unwrap();
        assert_eq!(Config::load(Some(&path)).unwrap().height, 40);
        assert_eq!(fs::read_to_string(&path).unwrap(), "height = 40\n");
        assert_eq!(fs::read_to_string(&temporary).unwrap(), "leave me alone");
    }

    #[test]
    fn parses_rgb_and_rgba() {
        assert_eq!(parse_color("#123456").unwrap(), [0x12, 0x34, 0x56, 0xff]);
        assert_eq!(parse_color("#12345678").unwrap(), [0x12, 0x34, 0x56, 0x78]);
    }

    #[test]
    fn rejects_unknown_keys() {
        let error = toml::from_str::<Config>("unknown = true").unwrap_err();
        assert!(error.to_string().contains("unknown"));
    }

    #[test]
    fn tray_drawer_options_are_accepted() {
        for duration in [0, 600] {
            toml::from_str::<Config>(&format!(
                "tray_drawer = true\ntray_drawer_duration_ms = {duration}"
            ))
            .unwrap()
            .validate()
            .unwrap();
        }
    }

    #[test]
    fn tasks_require_a_centered_clock_and_default_to_hidden() {
        let config =
            toml::from_str::<Config>("show_tasks = true\nclock_position = 'center'").unwrap();
        config.validate().unwrap();
        assert!(config.show_tasks);

        let invalid = toml::from_str::<Config>("show_tasks = true").unwrap();
        assert!(
            invalid
                .validate()
                .unwrap_err()
                .to_string()
                .contains("clock_position")
        );

        assert!(!toml::from_str::<Config>("").unwrap().show_tasks);
        assert!(!toml::from_str::<Config>(DEFAULT_CONFIG).unwrap().show_tasks);
    }

    #[test]
    fn obsolete_menu_option_does_not_disable_media_or_break_existing_configs() {
        for value in ["true", "false"] {
            let config: Config = toml::from_str(&format!("appmenu_enabled = {value}")).unwrap();
            config.validate().unwrap();
            assert!(config.media_enabled);
        }
        assert!(
            !toml::from_str::<Config>("media_enabled = false")
                .unwrap()
                .media_enabled
        );
    }

    #[test]
    fn default_clock_format_is_valid() {
        Config::default().validate().unwrap();
    }

    #[test]
    fn bundled_example_is_a_valid_config() {
        toml::from_str::<Config>(DEFAULT_CONFIG)
            .unwrap()
            .validate()
            .unwrap();
    }

    #[test]
    fn default_colors_use_static_zenburn_palette() {
        let config = Config::default();

        assert_eq!(config.background, "#282828");
        assert_eq!(config.foreground, "#DCDCCC");
        assert_eq!(config.workspace_focused_background, "#3C3836");
        assert_eq!(config.workspace_active_background, "#6F6F6F");
        assert_eq!(config.workspace_active_foreground, "#F0DFAF");
        assert_eq!(config.workspace_urgent_foreground, "#CC9393");
        assert_eq!(config.task_focused_background, "#3C3836");
        assert_eq!(config.task_focused_foreground, "#F0DFAF");
    }
}
