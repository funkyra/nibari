use std::{collections::VecDeque, sync::Arc, thread, time::Duration};

use smithay_client_toolkit::reexports::calloop::channel::Sender as UiSender;
use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};
use wl_clipboard_rs::copy::{MimeType, Options, Source};
use wl_clipboard_watch::{Config as WatchConfig, Event, Transfer, Watcher};

const MAX_ENTRY_BYTES: usize = 5 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Content {
    Text,
    Png,
}

pub struct Entry {
    pub id: i32,
    pub content: Content,
    pub bytes: Arc<Vec<u8>>,
    pub preview: String,
    pub thumbnail: Option<Pixmap>,
}

impl Entry {
    pub fn text(text: String) -> Self {
        let preview: String = text
            .chars()
            .take(80)
            .map(|ch| if ch.is_control() { ' ' } else { ch })
            .collect();
        Self {
            id: 0,
            content: Content::Text,
            bytes: Arc::new(text.into_bytes()),
            preview,
            thumbnail: None,
        }
    }

    pub fn png(bytes: Vec<u8>) -> Option<Self> {
        let size = bytes.get(16..24)?;
        if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return None;
        }
        let width = u32::from_be_bytes(size[0..4].try_into().ok()?);
        let height = u32::from_be_bytes(size[4..8].try_into().ok()?);
        if width == 0 || height == 0 || width.checked_mul(height)? > 4_000_000 {
            return None;
        }
        let thumbnail = thumbnail(&bytes)?;
        Some(Self {
            id: 0,
            content: Content::Png,
            bytes: Arc::new(bytes),
            preview: format!("Image {width}×{height}"),
            thumbnail: Some(thumbnail),
        })
    }
}

fn thumbnail(bytes: &[u8]) -> Option<Pixmap> {
    let source = Pixmap::decode_png(bytes).ok()?;
    let factor = 22.0 / source.width().max(source.height()) as f32;
    let width = (source.width() as f32 * factor).round().max(1.0) as u32;
    let height = (source.height() as f32 * factor).round().max(1.0) as u32;
    let mut thumbnail = Pixmap::new(width, height)?;
    thumbnail.draw_pixmap(
        0,
        0,
        source.as_ref(),
        &PixmapPaint {
            quality: FilterQuality::Bicubic,
            ..Default::default()
        },
        Transform::from_scale(
            width as f32 / source.width() as f32,
            height as f32 / source.height() as f32,
        ),
        None,
    );
    Some(thumbnail)
}

pub struct History {
    pub entries: VecDeque<Entry>,
    max_items: usize,
    next_id: i32,
}

impl History {
    pub fn new(max_items: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(max_items),
            max_items,
            next_id: 0,
        }
    }

    pub fn push(&mut self, mut entry: Entry) -> bool {
        if entry.bytes.is_empty() || self.max_items == 0 {
            return false;
        }
        if self
            .entries
            .front()
            .is_some_and(|current| current.content == entry.content && current.bytes == entry.bytes)
        {
            return false;
        }
        if let Some(index) = self
            .entries
            .iter()
            .position(|current| current.content == entry.content && current.bytes == entry.bytes)
        {
            self.entries.remove(index);
        }
        entry.id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.entries.push_front(entry);
        self.entries.truncate(self.max_items);
        true
    }

    pub fn get(&self, id: i32) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.id == id)
    }
}

pub fn spawn(events: UiSender<Entry>) {
    if let Err(error) = thread::Builder::new()
        .name("nibari-clipboard".into())
        .spawn(move || watch(events))
    {
        log::warn!("failed to start clipboard watcher: {error}");
    }
}

fn watch(events: UiSender<Entry>) {
    loop {
        let config = WatchConfig::new(MAX_ENTRY_BYTES, Duration::from_secs(5))
            .expect("valid clipboard transfer limits");
        match Watcher::connect_with(config) {
            Ok(mut watcher) => loop {
                let selection = match watcher.next_event() {
                    Ok(Event::Selection(selection)) => selection,
                    Ok(Event::Cleared) => continue,
                    Err(error) => {
                        log::warn!("clipboard watcher disconnected: {error}");
                        break;
                    }
                };
                let mime = if selection.offers("image/png") {
                    "image/png"
                } else if selection.offers("text/plain;charset=utf-8") {
                    "text/plain;charset=utf-8"
                } else if selection.offers("text/plain") {
                    "text/plain"
                } else if selection.offers("UTF8_STRING") {
                    "UTF8_STRING"
                } else {
                    continue;
                };
                match watcher.receive(&selection, mime) {
                    Ok(Transfer::Complete(bytes)) => {
                        let entry = if mime == "image/png" {
                            Entry::png(bytes)
                        } else {
                            String::from_utf8(bytes).ok().map(Entry::text)
                        };
                        if let Some(entry) = entry
                            && events.send(entry).is_err()
                        {
                            return;
                        }
                    }
                    Ok(Transfer::Stale) => {}
                    Err(error) => log::warn!("failed to read clipboard: {error}"),
                }
            },
            Err(error) => log::warn!("failed to connect clipboard watcher: {error}"),
        }
        thread::sleep(Duration::from_secs(2));
    }
}

pub fn copy(entry: &Entry) {
    let bytes = Arc::clone(&entry.bytes);
    let content = entry.content;
    if let Err(error) = thread::Builder::new()
        .name("nibari-clipboard-copy".into())
        .spawn(move || {
            let mime = match content {
                Content::Text => MimeType::Text,
                Content::Png => MimeType::Specific("image/png".into()),
            };
            let source = Source::Bytes(bytes.as_slice().to_vec().into_boxed_slice());
            if let Err(error) = Options::new().copy(source, mime) {
                log::warn!("failed to copy clipboard entry: {error}");
            }
        })
    {
        log::warn!("failed to start clipboard copy: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_keeps_the_newest_distinct_entries_within_the_limit() {
        let mut history = History::new(2);
        history.push(Entry::text("first".into()));
        history.push(Entry::text("second".into()));
        history.push(Entry::text("first".into()));
        assert_eq!(history.entries.len(), 2);
        assert_eq!(history.entries[0].preview, "first");
        assert_eq!(history.entries[1].preview, "second");
        history.push(Entry::text("third".into()));
        assert_eq!(history.entries.len(), 2);
        assert_eq!(history.entries[0].preview, "third");
        assert_eq!(history.entries[1].preview, "first");
    }

    #[test]
    fn png_entry_keeps_image_bytes_and_prepares_a_thumbnail() {
        let mut source = Pixmap::new(4, 2).unwrap();
        source.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        let png = source.encode_png().unwrap();
        let entry = Entry::png(png.clone()).unwrap();

        assert_eq!(entry.content, Content::Png);
        assert_eq!(entry.bytes.as_slice(), png);
        assert_eq!(entry.preview, "Image 4×2");
        assert!(
            entry
                .thumbnail
                .as_ref()
                .unwrap()
                .data()
                .iter()
                .any(|&byte| byte != 0)
        );
    }

    #[test]
    #[ignore = "requires a running Wayland compositor with data control"]
    fn connects_to_wayland_clipboard_events() {
        Watcher::connect().unwrap();
    }
}
