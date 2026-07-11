use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};

use freedesktop_icons::{default_theme_gtk, lookup};
use tiny_skia::Pixmap;

use crate::config::Config;

pub struct IconLoader {
    theme: String,
    desktop_directories: Vec<PathBuf>,
}

impl IconLoader {
    pub fn new(config: &Config) -> Self {
        Self {
            theme: config
                .icon_theme
                .clone()
                .or_else(default_theme_gtk)
                .unwrap_or_else(|| "hicolor".into()),
            desktop_directories: desktop_directories(),
        }
    }

    pub fn load(&self, name: &str, extra_path: Option<&str>, target_size: u32) -> Option<Pixmap> {
        let direct_path = Path::new(name);
        if direct_path.is_absolute() {
            return load_icon_file(direct_path, target_size);
        }

        if let Some(base) = extra_path
            && let Some(path) = find_icon_file(Path::new(base), name, 4)
            && let Some(icon) = load_icon_file(&path, target_size)
        {
            return Some(icon);
        }

        lookup(name)
            .with_size(target_size.min(u16::MAX as u32) as u16)
            .with_theme(&self.theme)
            .with_cache()
            .find()
            .and_then(|path| load_icon_file(&path, target_size))
    }

    pub fn load_application(&self, app_id: &str, target_size: u32) -> Option<Pixmap> {
        desktop_icon_name(&self.desktop_directories, app_id)
            .and_then(|name| self.load(&name, None, target_size))
            .or_else(|| {
                desktop_icon_name_by_startup_wm_class(&self.desktop_directories, app_id)
                    .and_then(|name| self.load(&name, None, target_size))
            })
            .or_else(|| {
                application_icon_names(app_id)
                    .into_iter()
                    .find_map(|name| self.load(name.as_ref(), None, target_size))
            })
    }
}

fn application_icon_names(app_id: &str) -> Vec<Cow<'_, str>> {
    let app_id = app_id
        .strip_suffix(".desktop")
        .filter(|name| !name.is_empty())
        .unwrap_or(app_id);
    let lowercase = app_id.to_lowercase();
    let leaf = app_id.rsplit('.').next().unwrap_or(app_id);
    let lowercase_leaf = leaf.to_lowercase();

    [Cow::Borrowed(app_id), Cow::Owned(lowercase)]
        .into_iter()
        .chain((leaf != app_id).then_some(Cow::Borrowed(leaf)))
        .chain((lowercase_leaf != leaf).then_some(Cow::Owned(lowercase_leaf)))
        .fold(Vec::with_capacity(4), |mut names, name| {
            if !names.iter().any(|existing| existing == &name) {
                names.push(name);
            }
            names
        })
}

fn desktop_directories() -> Vec<PathBuf> {
    let user_directory = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".local/share"))
        })
        .into_iter()
        .map(|path| path.join("applications"));
    let system_directories = std::env::var_os("XDG_DATA_DIRS")
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into())
        .to_string_lossy()
        .split(':')
        .filter(|directory| !directory.is_empty())
        .map(PathBuf::from)
        .map(|path| path.join("applications"))
        .collect::<Vec<_>>();

    user_directory.chain(system_directories).collect()
}

fn desktop_icon_name(directories: &[PathBuf], app_id: &str) -> Option<String> {
    let mut file_names = vec![app_id.to_owned()];
    if app_id.ends_with(".desktop") {
        file_names.push(format!("{app_id}.desktop"));
    } else {
        file_names[0] = format!("{app_id}.desktop");
    }
    if file_names.iter().any(|file_name| file_name.contains('/')) {
        return None;
    }

    directories.iter().find_map(|directory| {
        file_names
            .iter()
            .map(|file_name| directory.join(file_name))
            .find_map(|path| std::fs::read_to_string(path).ok())
            .and_then(|source| parse_desktop_entry(&source).icon)
    })
}

#[derive(Default)]
struct DesktopEntry {
    icon: Option<String>,
    startup_wm_class: Option<String>,
}

fn parse_desktop_entry(source: &str) -> DesktopEntry {
    let mut entry = DesktopEntry::default();
    let mut in_desktop_entry = false;

    for line in source.lines().map(str::trim) {
        if line == "[Desktop Entry]" {
            in_desktop_entry = true;
            continue;
        }
        if in_desktop_entry && line.starts_with('[') {
            break;
        }
        if !in_desktop_entry {
            continue;
        }

        if let Some(value) = line.strip_prefix("Icon=") {
            let value = value.trim();
            if !value.is_empty() {
                entry.icon = Some(value.to_owned());
            }
        } else if let Some(value) = line.strip_prefix("StartupWMClass=") {
            let value = value.trim();
            if !value.is_empty() {
                entry.startup_wm_class = Some(value.to_owned());
            }
        }
    }

    entry
}

fn desktop_icon_name_by_startup_wm_class(directories: &[PathBuf], app_id: &str) -> Option<String> {
    for directory in directories {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("desktop"))
            {
                continue;
            }

            let Ok(source) = std::fs::read_to_string(path) else {
                continue;
            };
            let desktop_entry = parse_desktop_entry(&source);
            if desktop_entry.startup_wm_class.as_deref() == Some(app_id)
                && desktop_entry.icon.is_some()
            {
                return desktop_entry.icon;
            }
        }
    }

    None
}

fn find_icon_file(directory: &Path, name: &str, remaining_depth: u8) -> Option<PathBuf> {
    if remaining_depth == 0 {
        return None;
    }

    std::fs::read_dir(directory)
        .ok()?
        .flatten()
        .find_map(|entry| {
            let path = entry.path();
            match path.is_dir() {
                true => find_icon_file(&path, name, remaining_depth - 1),
                false if is_named_icon(&path, name) => Some(path),
                false => None,
            }
        })
}

fn is_named_icon(path: &Path, name: &str) -> bool {
    path.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("png") || extension.eq_ignore_ascii_case("svg")
    }) && path.file_stem().is_some_and(|stem| stem == name)
}

fn load_icon_file(path: &Path, target_size: u32) -> Option<Pixmap> {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some(extension) if extension.eq_ignore_ascii_case("png") => Pixmap::load_png(path).ok(),
        Some(extension) if extension.eq_ignore_ascii_case("svg") => {
            let data = std::fs::read(path).ok()?;
            render_svg(
                &data,
                target_size.saturating_mul(2).clamp(32, 256),
                path.parent(),
            )
        }
        _ => None,
    }
}

fn render_svg(data: &[u8], size: u32, resources_dir: Option<&Path>) -> Option<Pixmap> {
    let options = resvg::usvg::Options {
        resources_dir: resources_dir.map(Path::to_path_buf),
        ..resvg::usvg::Options::default()
    };
    let tree = resvg::usvg::Tree::from_data(data, &options).ok()?;
    let mut pixmap = Pixmap::new(size, size)?;
    let source = tree.size();
    let scale = (size as f32 / source.width()).min(size as f32 / source.height());
    let offset_x = (size as f32 - source.width() * scale) / 2.0;
    let offset_y = (size as f32 - source.height() * scale) / 2.0;
    let transform =
        resvg::tiny_skia::Transform::from_scale(scale, scale).post_translate(offset_x, offset_y);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Some(pixmap)
}

pub fn fallback_icon(seed: &str, size: u32) -> Pixmap {
    let hash = seed.bytes().fold(2_166_136_261_u32, |hash, byte| {
        (hash ^ byte as u32).wrapping_mul(16_777_619)
    });
    let color = [
        96 + (hash & 0x5f) as u8,
        96 + ((hash >> 8) & 0x5f) as u8,
        96 + ((hash >> 16) & 0x5f) as u8,
    ];
    let size = size.max(1);
    let border_width = (size / 16).max(1);
    let mut pixmap = Pixmap::new(size, size).expect("valid fallback icon size");

    pixmap
        .data_mut()
        .chunks_exact_mut(4)
        .enumerate()
        .for_each(|(offset, target)| {
            let x = offset as u32 % size;
            let y = offset as u32 / size;
            let border = x < border_width
                || y < border_width
                || x >= size - border_width
                || y >= size - border_width;
            let pixel = if border { [40, 40, 40] } else { color };
            target.copy_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
        });
    pixmap
}

pub fn image_revision(image: &Pixmap) -> u64 {
    image.data().iter().fold(
        0xcbf2_9ce4_8422_2325 ^ u64::from(image.width()),
        |hash, byte| (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3),
    ) ^ u64::from(image.height()).rotate_left(32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_icon_from_desktop_entry() {
        let source = "[Desktop Entry]\nName=Browser\nIcon=org.example.Browser\n[Other]\nIcon=no\n";
        assert_eq!(
            parse_desktop_entry(source).icon.as_deref(),
            Some("org.example.Browser")
        );
    }

    #[test]
    fn finds_icon_by_startup_wm_class() {
        let source = "[Desktop Entry]\nIcon=com.ayugram.desktop\nStartupWMClass=AyuGram\n[Desktop Action quit]\nIcon=application-exit\n";
        let entry = parse_desktop_entry(source);

        assert_eq!(entry.icon.as_deref(), Some("com.ayugram.desktop"));
        assert_eq!(entry.startup_wm_class.as_deref(), Some("AyuGram"));
    }

    #[test]
    fn finds_startup_wm_class_icon_in_desktop_directory() {
        let directory = std::env::temp_dir().join(format!(
            "nibari-icons-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let desktop_file = directory.join("com.ayugram.desktop.desktop");

        std::fs::write(
            &desktop_file,
            "[Desktop Entry]\nIcon=com.ayugram.desktop\nStartupWMClass=AyuGram\n",
        )
        .unwrap();
        assert_eq!(
            desktop_icon_name_by_startup_wm_class(std::slice::from_ref(&directory), "AyuGram")
                .as_deref(),
            Some("com.ayugram.desktop")
        );

        std::fs::write(
            &desktop_file,
            "[Desktop Entry]\nStartupWMClass=AyuGram\n[Desktop Action quit]\nIcon=application-exit\n",
        )
        .unwrap();
        assert_eq!(
            desktop_icon_name_by_startup_wm_class(std::slice::from_ref(&directory), "AyuGram"),
            None
        );

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn finds_desktop_file_when_application_id_ends_with_desktop() {
        let directory = std::env::temp_dir().join(format!(
            "nibari-desktop-file-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(
            directory.join("com.ayugram.desktop.desktop"),
            "[Desktop Entry]\nIcon=com.ayugram.desktop\n",
        )
        .unwrap();

        assert_eq!(
            desktop_icon_name(std::slice::from_ref(&directory), "com.ayugram.desktop").as_deref(),
            Some("com.ayugram.desktop")
        );

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn builds_application_icon_fallbacks_without_duplicates() {
        assert_eq!(
            application_icon_names("Org.Example.Firefox.desktop"),
            [
                "Org.Example.Firefox",
                "org.example.firefox",
                "Firefox",
                "firefox"
            ]
        );
    }

    #[test]
    fn renders_svg_icon() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16">
            <circle cx="8" cy="8" r="8" fill="#ff0000"/>
        </svg>"##;
        let icon = render_svg(svg, 32, None).unwrap();
        assert_eq!((icon.width(), icon.height()), (32, 32));
        assert!(icon.data().iter().any(|channel| *channel != 0));
    }
}
