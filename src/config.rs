use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use chrono::format::{Item, StrftimeItems};
use serde::Deserialize;

const DEFAULT_CONFIG: &str = include_str!("../config.example.toml");

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
    pub workspace_width: u32,
    pub workspace_focused_background: String,
    pub workspace_active_background: String,
    pub workspace_active_foreground: String,
    pub workspace_occupied_foreground: String,
    pub workspace_empty_foreground: String,
    pub workspace_urgent_foreground: String,
    pub task_icon_size: u32,
    pub task_spacing: u32,
    pub task_padding: u32,
    pub task_background: String,
    pub task_focused_background: String,
    pub task_urgent_background: String,
    pub task_foreground: String,
    pub task_focused_foreground: String,
    pub clock_format: String,
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
            workspace_width: 28,
            workspace_focused_background: "#3C3836".into(),
            workspace_active_background: "#6F6F6F".into(),
            workspace_active_foreground: "#F0DFAF".into(),
            workspace_occupied_foreground: "#DCDCCC".into(),
            workspace_empty_foreground: "#6F6F6F".into(),
            workspace_urgent_foreground: "#CC9393".into(),
            task_icon_size: 18,
            task_spacing: 2,
            task_padding: 6,
            task_background: "#282828".into(),
            task_focused_background: "#3C3836".into(),
            task_urgent_background: "#282828".into(),
            task_foreground: "#DCDCCC".into(),
            task_focused_foreground: "#F0DFAF".into(),
            clock_format: "%a %b %d, %H:%M".into(),
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

        let config = match path {
            Some(path) if path.exists() => {
                let source = fs::read_to_string(&path)
                    .with_context(|| format!("failed to read {}", path.display()))?;
                let config: Self = toml::from_str(&source)
                    .with_context(|| format!("error in config {}", path.display()))?;
                log::info!("config: {}", path.display());
                config
            }
            Some(path) if explicit_path.is_some() => {
                bail!("config not found: {}", path.display());
            }
            _ => {
                log::info!("config: using default values");
                Self::default()
            }
        };

        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
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
