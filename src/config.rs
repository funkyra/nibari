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

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub bar: BarConfig,
    pub workspaces: WorkspacesConfig,
    pub tasks: TasksConfig,
    pub media: MediaConfig,
    pub tray: TrayConfig,
    pub menu: MenuConfig,
    pub clock: ClockConfig,
    pub calendar: CalendarConfig,
    pub weather: WeatherConfig,
    pub bluetooth: BluetoothConfig,
    pub network: NetworkConfig,
    pub clipboard: ClipboardConfig,
    pub power: PowerConfig,
    pub icons: IconsConfig,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BarConfig {
    pub height: u32,
    pub font_family: String,
    pub font_size: f32,
    pub background: String,
    pub foreground: String,
    pub icon_foreground: String,
    pub icon_muted: String,
    pub padding: u32,
}

impl Default for BarConfig {
    fn default() -> Self {
        Self {
            height: 28,
            font_family: "Fira Code Medium".into(),
            font_size: 8.0,
            background: "#282828".into(),
            foreground: "#DCDCCC".into(),
            icon_foreground: "#DCDCCC".into(),
            icon_muted: "#6F6F6F".into(),
            padding: 8,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkspacesConfig {
    pub width: u32,
    pub font_size: Option<f32>,
    pub focused_background: String,
    pub active_background: String,
    pub active_foreground: String,
    pub occupied_foreground: String,
    pub empty_foreground: String,
    pub urgent_foreground: String,
}

impl Default for WorkspacesConfig {
    fn default() -> Self {
        Self {
            width: 28,
            font_size: None,
            focused_background: "#3C3836".into(),
            active_background: "#6F6F6F".into(),
            active_foreground: "#F0DFAF".into(),
            occupied_foreground: "#DCDCCC".into(),
            empty_foreground: "#6F6F6F".into(),
            urgent_foreground: "#CC9393".into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TasksConfig {
    pub enabled: bool,
    pub icon_size: u32,
    pub spacing: u32,
    pub clock_spacing: Option<u32>,
    pub padding: u32,
    pub background: String,
    pub focused_background: String,
    pub urgent_background: String,
    pub foreground: String,
    pub focused_foreground: String,
}

impl Default for TasksConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            icon_size: 18,
            spacing: 2,
            clock_spacing: None,
            padding: 6,
            background: "#282828".into(),
            focused_background: "#3C3836".into(),
            urgent_background: "#282828".into(),
            foreground: "#DCDCCC".into(),
            focused_foreground: "#F0DFAF".into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MediaConfig {
    pub enabled: bool,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TrayConfig {
    pub icon_size: u32,
    pub spacing: u32,
    pub drawer: bool,
    pub drawer_duration_ms: u32,
}

impl Default for TrayConfig {
    fn default() -> Self {
        Self {
            icon_size: 18,
            spacing: 6,
            drawer: false,
            drawer_duration_ms: 600,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MenuConfig {
    pub background: String,
    pub foreground: String,
    pub muted: String,
    pub accent: String,
    pub border: String,
}

impl Default for MenuConfig {
    fn default() -> Self {
        Self {
            background: "#282828F2".into(),
            foreground: "#DCDCCC".into(),
            muted: "#6F6F6F".into(),
            accent: "#F0DFAF".into(),
            border: "#6F6F6F".into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClockConfig {
    pub format: String,
    pub position: ClockPosition,
}

impl Default for ClockConfig {
    fn default() -> Self {
        Self {
            format: "%a %b %d, %H:%M".into(),
            position: ClockPosition::Right,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CalendarConfig {
    pub enabled: bool,
    pub background: String,
    pub foreground: String,
    pub header: String,
    pub weekday: String,
    pub weekend: String,
    pub muted: String,
    pub today_background: String,
    pub today_foreground: String,
    pub border: String,
}

impl Default for CalendarConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            background: "#282828F2".into(),
            foreground: "#DCDCCC".into(),
            header: "#F0DFAF".into(),
            weekday: "#DCDCCC".into(),
            weekend: "#CC9393".into(),
            muted: "#6F6F6F".into(),
            today_background: "#3C3836".into(),
            today_foreground: "#F0DFAF".into(),
            border: "#6F6F6F".into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WeatherConfig {
    pub enabled: bool,
    pub location: String,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub units: WeatherUnits,
    pub refresh_minutes: u32,
    pub background: String,
    pub foreground: String,
    pub muted: String,
    pub accent: String,
    pub border: String,
}

impl Default for WeatherConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            location: String::new(),
            latitude: None,
            longitude: None,
            units: WeatherUnits::Metric,
            refresh_minutes: 15,
            background: "#282828F2".into(),
            foreground: "#DCDCCC".into(),
            muted: "#6F6F6F".into(),
            accent: "#F0DFAF".into(),
            border: "#6F6F6F".into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BluetoothConfig {
    pub enabled: bool,
    pub background: String,
    pub foreground: String,
    pub muted: String,
    pub accent: String,
    pub border: String,
}

impl Default for BluetoothConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            background: "#282828F2".into(),
            foreground: "#DCDCCC".into(),
            muted: "#6F6F6F".into(),
            accent: "#F0DFAF".into(),
            border: "#6F6F6F".into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkConfig {
    pub enabled: bool,
    pub interface: String,
    pub ping_target: String,
    pub background: String,
    pub foreground: String,
    pub muted: String,
    pub accent: String,
    pub border: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClipboardConfig {
    pub max_items: usize,
}

impl Default for ClipboardConfig {
    fn default() -> Self {
        Self { max_items: 5 }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PowerConfig {
    pub enabled: bool,
    pub shutdown: String,
    pub restart: String,
    pub suspend: String,
    pub hibernate: String,
}

impl Default for PowerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            shutdown: "systemctl poweroff".into(),
            restart: "systemctl reboot".into(),
            suspend: "systemctl suspend".into(),
            hibernate: "systemctl hibernate".into(),
        }
    }
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interface: String::new(),
            ping_target: "1.1.1.1".into(),
            background: "#282828F2".into(),
            foreground: "#DCDCCC".into(),
            muted: "#6F6F6F".into(),
            accent: "#F0DFAF".into(),
            border: "#6F6F6F".into(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IconsConfig {
    pub theme: Option<String>,
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
        if !(1..=100).contains(&self.clipboard.max_items) {
            bail!("clipboard.max_items must be in the range 1..=100");
        }
        if self.tasks.enabled && self.clock.position != ClockPosition::Center {
            bail!("tasks.enabled requires clock.position = 'center'");
        }
        if !(16..=256).contains(&self.bar.height) {
            bail!("height must be in the range 16..=256");
        }
        if !(6.0..=96.0).contains(&self.bar.font_size) {
            bail!("font_size must be in the range 6..=96");
        }
        if self.bar.font_family.trim().is_empty() {
            bail!("font_family cannot be empty");
        }
        if self.tray.icon_size == 0 || self.tray.icon_size > self.bar.height {
            bail!("tray_icon_size must be greater than 0 and no greater than height");
        }
        if self.bar.padding > 256 || self.tray.spacing > 256 {
            bail!("padding and tray_spacing must be no greater than 256");
        }
        if !(16..=128).contains(&self.workspaces.width) {
            bail!("workspace_width must be in the range 16..=128");
        }
        if self
            .workspaces
            .font_size
            .is_some_and(|size| !(6.0..=96.0).contains(&size))
        {
            bail!("workspaces.font_size must be in the range 6..=96");
        }
        if self.tasks.icon_size == 0 || self.tasks.icon_size > self.bar.height {
            bail!("task_icon_size must be greater than 0 and no greater than height");
        }
        if self.tasks.spacing > 256 || self.tasks.padding > 256 {
            bail!("task_spacing and task_padding must be no greater than 256");
        }
        if self
            .tasks
            .clock_spacing
            .is_some_and(|spacing| spacing > 512)
        {
            bail!("tasks.clock_spacing must be no greater than 512");
        }
        if StrftimeItems::new(&self.clock.format).any(|item| item == Item::Error) {
            bail!("clock_format contains an invalid strftime sequence");
        }

        match (self.weather.latitude, self.weather.longitude) {
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
        if !(5..=1440).contains(&self.weather.refresh_minutes) {
            bail!("weather_refresh_minutes must be in the range 5..=1440");
        }
        if self.weather.location.len() > 256 || self.weather.location.chars().any(char::is_control)
        {
            bail!(
                "weather_location must be a city name of at most 256 bytes, without control characters"
            );
        }
        if !self.network.interface.is_empty()
            && (self.network.interface.len() >= 16
                || self.network.interface == "."
                || self.network.interface == ".."
                || !self
                    .network
                    .interface
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_.-:".contains(&c)))
        {
            bail!("network_interface must be an interface name or empty for automatic selection");
        }
        self.network
            .ping_target
            .parse::<std::net::IpAddr>()
            .context("network_ping_target must be a numeric IPv4 or IPv6 address")?;
        parse_color(&self.bar.background).context("invalid bar.background")?;
        parse_color(&self.bar.foreground).context("invalid bar.foreground")?;
        parse_color(&self.bar.icon_foreground).context("invalid bar.icon_foreground")?;
        parse_color(&self.bar.icon_muted).context("invalid bar.icon_muted")?;
        [
            (
                "workspaces.focused_background",
                &self.workspaces.focused_background,
            ),
            (
                "workspaces.active_background",
                &self.workspaces.active_background,
            ),
            (
                "workspaces.active_foreground",
                &self.workspaces.active_foreground,
            ),
            (
                "workspaces.occupied_foreground",
                &self.workspaces.occupied_foreground,
            ),
            (
                "workspaces.empty_foreground",
                &self.workspaces.empty_foreground,
            ),
            (
                "workspaces.urgent_foreground",
                &self.workspaces.urgent_foreground,
            ),
            ("calendar.background", &self.calendar.background),
            ("calendar.foreground", &self.calendar.foreground),
            ("calendar.header", &self.calendar.header),
            ("calendar.weekday", &self.calendar.weekday),
            ("calendar.weekend", &self.calendar.weekend),
            ("calendar.muted", &self.calendar.muted),
            ("calendar.today_background", &self.calendar.today_background),
            ("calendar.today_foreground", &self.calendar.today_foreground),
            ("calendar.border", &self.calendar.border),
            ("weather.background", &self.weather.background),
            ("weather.foreground", &self.weather.foreground),
            ("weather.muted", &self.weather.muted),
            ("weather.accent", &self.weather.accent),
            ("weather.border", &self.weather.border),
            ("bluetooth.background", &self.bluetooth.background),
            ("bluetooth.foreground", &self.bluetooth.foreground),
            ("bluetooth.muted", &self.bluetooth.muted),
            ("bluetooth.accent", &self.bluetooth.accent),
            ("bluetooth.border", &self.bluetooth.border),
            ("network.background", &self.network.background),
            ("network.foreground", &self.network.foreground),
            ("network.muted", &self.network.muted),
            ("network.accent", &self.network.accent),
            ("network.border", &self.network.border),
            ("menu.background", &self.menu.background),
            ("menu.foreground", &self.menu.foreground),
            ("menu.muted", &self.menu.muted),
            ("menu.accent", &self.menu.accent),
            ("menu.border", &self.menu.border),
            ("tasks.background", &self.tasks.background),
            ("tasks.focused_background", &self.tasks.focused_background),
            ("tasks.urgent_background", &self.tasks.urgent_background),
            ("tasks.foreground", &self.tasks.foreground),
            ("tasks.focused_foreground", &self.tasks.focused_foreground),
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
        parse_color(&self.bar.background).expect("config was validated")
    }

    pub fn foreground_rgba(&self) -> [u8; 4] {
        parse_color(&self.bar.foreground).expect("config was validated")
    }

    pub fn color_rgba(&self, color: &str) -> [u8; 4] {
        parse_color(color).expect("config was validated")
    }
}

#[cfg(test)]
mod calendar_config_tests {
    use super::*;

    #[test]
    fn nested_tables_are_accepted_and_flat_keys_are_rejected() {
        let config: Config = toml::from_str(
            "[bar]\nheight = 32\n\n[clock]\nposition = 'center'\n\n[tasks]\nenabled = true",
        )
        .unwrap();
        config.validate().unwrap();

        assert!(toml::from_str::<Config>("height = 32").is_err());
    }

    #[test]
    fn weather_config_validates_coordinate_pairs_units_and_refresh_interval() {
        for source in [
            "[weather]\nenabled = true",
            "[weather]\nlocation = 'Málaga'",
            "[weather]\nlatitude = 0.0\nlongitude = 0.0\nunits = 'imperial'\nbackground = '#22222280'",
        ] {
            let c: Config = toml::from_str(source).unwrap();
            c.validate().unwrap();
        }
        for source in [
            "[weather]\nlatitude = 10.0",
            "[weather]\nlatitude = nan\nlongitude = 1.0",
            "[weather]\nlatitude = 91.0\nlongitude = 0.0",
            "[weather]\nlatitude = 0.0\nlongitude = -181.0",
            "[weather]\nrefresh_minutes = 0",
            "[weather]\nforeground = 'bad'",
        ] {
            let c: Config = toml::from_str(source).unwrap();
            assert!(c.validate().is_err(), "{source}");
        }
        assert!(toml::from_str::<Config>("[weather]\nunits = 'kelvin'").is_err());
    }

    #[test]
    fn network_options_accept_numeric_probe_and_rgba_palette() {
        let config: Config = toml::from_str("[network]\nenabled = true\ninterface = 'eth0'\nping_target = '1.1.1.1'\nbackground = '#222222CC'").unwrap();
        config.validate().unwrap();
        for source in [
            "[network]\ninterface = '../eth0'",
            "[network]\nping_target = '--help'",
            "[network]\naccent = 'invalid'",
        ] {
            let invalid: Config = toml::from_str(source).unwrap();
            assert!(invalid.validate().is_err());
        }
    }

    #[test]
    fn bluetooth_and_tray_menu_accept_independent_palettes() {
        let source = "[network]\nbackground = '#112233FF'\n\n[bluetooth]\nbackground = '#332211F2'\naccent = '#FFAA77FF'\n\n[menu]\nbackground = '#221133F2'\naccent = '#77AAFFFF'";
        let config: Config = toml::from_str(source).unwrap();
        config.validate().unwrap();
    }

    #[test]
    fn accepts_centered_formatted_clock_and_rgba_calendar_palette() {
        let config: Config = toml::from_str("[clock]\nposition = 'center'\nformat = '%d.%m.%Y %H:%M'\n\n[calendar]\nbackground = '#222222CC'\nenabled = true").unwrap();
        config.validate().unwrap();
        assert_eq!(config.clock.format, "%d.%m.%Y %H:%M");
    }

    #[test]
    fn rejects_invalid_clock_position_and_calendar_color() {
        assert!(toml::from_str::<Config>("[clock]\nposition = 'middle'").is_err());
        let config: Config = toml::from_str("[calendar]\nborder = 'invalid'").unwrap();
        assert!(config.validate().is_err());
    }
}

fn completed_config(source: &str) -> Result<Option<String>> {
    let existing: toml::Table = toml::from_str(source)?;
    let defaults: toml::Table = toml::from_str(DEFAULT_CONFIG)?;
    let missing = defaults
        .into_iter()
        .filter(|(name, _)| !existing.contains_key(name))
        .collect::<toml::Table>();
    if missing.is_empty() {
        return Ok(None);
    }
    let mut updated = source.to_owned();
    if !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push('\n');
    updated.push_str(&toml::to_string_pretty(&missing)?);
    Ok(Some(updated))
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

    #[test]
    fn task_clock_spacing_is_configurable() {
        assert!(toml::from_str::<Config>("[tasks]\nclock_spacing = 13").is_ok());
        let too_large: Config = toml::from_str("[tasks]\nclock_spacing = 513").unwrap();
        assert!(too_large.validate().is_err());
    }

    #[test]
    fn power_commands_can_be_configured() {
        let config: Config =
            toml::from_str("[power]\nshutdown = 'loginctl poweroff'\nrestart = ''\n").unwrap();
        assert_eq!(config.power.shutdown, "loginctl poweroff");
        assert!(config.power.restart.is_empty());
        assert_eq!(config.power.suspend, "systemctl suspend");
    }

    #[test]
    fn clipboard_history_limit_is_configurable() {
        assert!(toml::from_str::<Config>("[clipboard]\nmax_items = 2").is_ok());
        assert_eq!(Config::default().clipboard.max_items, 5);
        assert!(
            toml::from_str::<Config>("[clipboard]\nmax_items = 0")
                .unwrap()
                .validate()
                .is_err()
        );
    }

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
        let source = "# My settings\n[bar]\nheight = 32\n\n[tray]\ndrawer = true\ndrawer_duration_ms = 123\n# trailing comment";
        fs::write(&path, source).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();

        let config = Config::load(Some(&path)).unwrap();
        let updated = fs::read_to_string(&path).unwrap();
        let table: toml::Table = toml::from_str(&updated).unwrap();
        assert!(
            table.contains_key("tasks"),
            "startup should persist missing defaults"
        );
        assert!(updated.starts_with(source));
        assert_eq!(config.bar.height, 32);
        assert!(config.tray.drawer);
        assert_eq!(config.tray.drawer_duration_ms, 123);
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
        let source = "[clock]\nformat = '''time\ntray_drawer = true\n'''\n# tray_drawer = true\n";
        fs::write(&target, source).unwrap();
        symlink("settings.toml", &path).unwrap();

        Config::load(Some(&path)).unwrap();
        assert!(fs::symlink_metadata(&path).unwrap().is_symlink());
        let updated = fs::read_to_string(&target).unwrap();
        assert!(updated.starts_with(source));
        let table: toml::Table = toml::from_str(&updated).unwrap();
        assert_eq!(
            table
                .get("tray")
                .and_then(toml::Value::as_table)
                .and_then(|tray| tray.get("drawer"))
                .and_then(toml::Value::as_bool),
            Some(false),
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
        let source = "[tray]\ndrawer = true\n";
        fs::write(&path, source).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        assert!(Config::load(Some(&path)).unwrap().tray.drawer);
        assert_eq!(fs::read_to_string(&path).unwrap(), source);
    }

    #[test]
    fn config_write_does_not_overwrite_edits_made_since_loading() {
        let directory = TestDirectory::new();
        let path = directory.0.join("config.toml");
        let latest = "[bar]\nheight = 40\n";
        fs::write(&path, latest).unwrap();
        assert!(write_config(&path, Some("[bar]\nheight = 28\n"), DEFAULT_CONFIG).is_err());
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
        fs::write(&path, "[bar]\nheight = 40\n").unwrap();
        assert_eq!(Config::load(Some(&path)).unwrap().bar.height, 40);
        assert_eq!(fs::read_to_string(&path).unwrap(), "[bar]\nheight = 40\n");
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
                "[tray]\ndrawer = true\ndrawer_duration_ms = {duration}"
            ))
            .unwrap()
            .validate()
            .unwrap();
        }
    }

    #[test]
    fn tasks_require_a_centered_clock_and_default_to_hidden() {
        let config =
            toml::from_str::<Config>("[tasks]\nenabled = true\n\n[clock]\nposition = 'center'")
                .unwrap();
        config.validate().unwrap();
        assert!(config.tasks.enabled);

        let invalid = toml::from_str::<Config>("[tasks]\nenabled = true").unwrap();
        assert!(
            invalid
                .validate()
                .unwrap_err()
                .to_string()
                .contains("clock.position")
        );

        assert!(!toml::from_str::<Config>("").unwrap().tasks.enabled);
        assert!(
            !toml::from_str::<Config>(DEFAULT_CONFIG)
                .unwrap()
                .tasks
                .enabled
        );
    }

    #[test]
    fn rejects_obsolete_menu_option_and_accepts_nested_media_option() {
        assert!(toml::from_str::<Config>("appmenu_enabled = true").is_err());
        assert!(
            !toml::from_str::<Config>("[media]\nenabled = false")
                .unwrap()
                .media
                .enabled
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

        assert_eq!(config.bar.background, "#282828");
        assert_eq!(config.bar.foreground, "#DCDCCC");
        assert_eq!(config.workspaces.focused_background, "#3C3836");
        assert_eq!(config.workspaces.active_background, "#6F6F6F");
        assert_eq!(config.workspaces.active_foreground, "#F0DFAF");
        assert_eq!(config.workspaces.urgent_foreground, "#CC9393");
        assert_eq!(config.tasks.focused_background, "#3C3836");
        assert_eq!(config.tasks.focused_foreground, "#F0DFAF");
    }
}
