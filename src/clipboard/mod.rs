// SPDX-License-Identifier: GPL-3.0-only

//! The clipboard's history, as Windows' Win+V: what's copied, text or a
//! picture, kept to copy again, pinned or not.
//!
//! A service of its own (`service`) watches the compositor's clipboard and
//! writes down what's copied; the applets read the same file and ask the
//! service over D-Bus (`panel`) to copy, pin or delete.

use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    time::SystemTime,
};

use serde::{Deserialize, Serialize};

mod keep;
mod panel;
pub mod service;

pub use panel::{Action, ask, subscription};

/// Kept at most, besides the pinned.
const KEEP: usize = 15;
/// What a card shows of a text, at most: laying out the whole of each (64 KB
/// in all, once) took ~25 ms each time the panel came back.
const PREVIEW_CHARS: usize = 200;

const TEXT_TYPES: [&str; 5] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "TEXT", "STRING"];
const IMAGE_TYPE: &str = "image/png";

/// Something copied.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: u64,
    /// The text; or none, for a picture.
    pub text: Option<String>,
    /// The picture, saved as a PNG file.
    pub image: Option<PathBuf>,
    pub pinned: bool,
    /// Seconds since 1970.
    pub time: u64,
}

/// A picture's thumbnail, next to it: twice as tall as its card, so it
/// stays sharp. Decoding the picture itself, at full size, each time the
/// card is drawn again took a fifth of a second.
const THUMB_HEIGHT: u32 = 96;

fn thumbnail_path(image: &Path) -> PathBuf {
    image.with_extension("thumb.png")
}

/// Makes a picture's thumbnail, unless there is one. Written aside and moved
/// in place: an applet may be reading it meanwhile.
fn make_thumbnail(image: &Path) {
    let thumb = thumbnail_path(image);
    if thumb.exists() {
        return;
    }
    let Ok(picture) = image::open(image) else {
        return;
    };
    let small = if picture.height() > THUMB_HEIGHT {
        picture.thumbnail(u32::MAX, THUMB_HEIGHT)
    } else {
        picture
    };
    let tmp = thumb.with_extension(format!("{}.tmp", std::process::id()));
    if small.save_with_format(&tmp, image::ImageFormat::Png).is_ok() {
        _ = std::fs::rename(&tmp, &thumb);
    } else {
        _ = std::fs::remove_file(&tmp);
    }
}

/// What a card shows of a picture: its thumbnail, or itself while there is
/// none yet.
pub fn shown(image: &Path) -> PathBuf {
    let thumb = thumbnail_path(image);
    if thumb.exists() { thumb } else { image.to_owned() }
}

fn remove_image(image: &Path) {
    _ = std::fs::remove_file(image);
    _ = std::fs::remove_file(thumbnail_path(image));
}

/// The pinned kept, the rest the newest ones only; the pictures of those
/// left out, to remove.
fn prune(entries: &mut Vec<Entry>) -> Vec<PathBuf> {
    let mut unpinned = 0;
    let mut dropped = Vec::new();
    entries.retain(|entry| {
        if entry.pinned {
            return true;
        }
        unpinned += 1;
        let keep = unpinned <= KEEP;
        if !keep && let Some(image) = &entry.image {
            dropped.push(image.clone());
        }
        keep
    });
    dropped
}

/// What a card shows of a text: its start, on one line.
fn preview(text: &str) -> String {
    text.trim().chars().take(PREVIEW_CHARS).collect::<String>().replace('\n', " ⏎ ")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Held from a change's reading to its writing: the service's threads (the
/// compositor's, the bus's) don't lose each other's changes.
static WRITING: Mutex<()> = Mutex::new(());

/// The history on disk, in a directory: one file and the pictures beside it.
pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// The user's own: in the state directory.
    pub fn user() -> Store {
        let state = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        Store { dir: state.join(crate::APP_ID) }
    }

    /// In the given directory, for the tests.
    #[cfg(test)]
    pub fn at(dir: PathBuf) -> Store {
        Store { dir }
    }

    fn file(&self) -> PathBuf {
        self.dir.join("clipboard.json")
    }

    /// The history, and whether the file is there but could not be read as
    /// one (then it is empty here, and must not be written over unseen).
    fn read(&self) -> (Vec<Entry>, bool) {
        let path = self.file();
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Vec::new(), false),
            Err(e) => {
                tracing::warn!("clipboard history {} not read: {e}", path.display());
                return (Vec::new(), false);
            }
        };
        match serde_json::from_slice(&bytes) {
            Ok(entries) => (entries, false),
            Err(e) => {
                tracing::warn!("clipboard history {} is not valid: {e}", path.display());
                (Vec::new(), true)
            }
        }
    }

    pub fn load(&self) -> Vec<Entry> {
        self.read().0
    }

    fn save(&self, entries: &[Entry]) {
        if let Err(e) = std::fs::create_dir_all(&self.dir) {
            tracing::warn!("clipboard directory {} not made: {e}", self.dir.display());
            return;
        }
        let path = self.file();
        let tmp = path.with_extension("json.tmp");
        let json = match serde_json::to_vec(entries) {
            Ok(json) => json,
            Err(e) => {
                tracing::warn!("clipboard history not serialized: {e}");
                return;
            }
        };
        if let Err(e) = std::fs::write(&tmp, json) {
            tracing::warn!("clipboard history {} not written: {e}", tmp.display());
        } else if let Err(e) = std::fs::rename(&tmp, &path) {
            tracing::warn!("clipboard history {} not moved in place: {e}", path.display());
            _ = std::fs::remove_file(&tmp);
        }
    }

    /// Whether a picture's path (from the file) is one of this store's own:
    /// under `<dir>/clipboard/`, and not climbing out of it.
    fn owns(&self, image: &Path) -> bool {
        let inside = image.starts_with(self.dir.join("clipboard"));
        let climbs = image.components().any(|c| matches!(c, std::path::Component::ParentDir));
        inside && !climbs
    }

    fn remove_picture(&self, image: &Path) {
        if self.owns(image) {
            remove_image(image);
        } else {
            tracing::warn!("clipboard picture {} is outside the history; left alone", image.display());
        }
    }

    fn thumbnail_of(&self, image: &Path) {
        if self.owns(image) {
            make_thumbnail(image);
        } else {
            tracing::warn!("clipboard picture {} is outside the history; ignored", image.display());
        }
    }

    /// Changes the saved history, as it is on disk now: another process may
    /// have changed it. One change at a time, from reading to writing.
    pub fn change(&self, f: impl FnOnce(&mut Vec<Entry>)) -> Vec<Entry> {
        // A thread that panicked holding it left the file whole (it is moved
        // in place at the end): going on is safe.
        let _writing = WRITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (mut entries, unreadable) = self.read();
        if unreadable {
            // Keep what could not be read, for the user to look at.
            let (file, bad) = (self.file(), self.file().with_extension("json.bad"));
            if let Err(e) = std::fs::rename(&file, &bad) {
                tracing::warn!("clipboard history {} not set aside: {e}", file.display());
            }
        }
        f(&mut entries);
        for image in prune(&mut entries) {
            self.remove_picture(&image);
        }
        self.save(&entries);
        entries
    }

    /// Something just copied, at the top; the same text again only moves up.
    /// None when nothing was written down: blank or invalid text, a picture
    /// that could not be saved.
    pub fn record(&self, mime: &str, bytes: &[u8]) -> Option<Vec<Entry>> {
        let mut recorded = false;
        let entries = self.change(|entries| {
            let mut id = now() * 1000 + (entries.len() as u64 % 1000);
            while entries.iter().any(|e| e.id == id) {
                id += 1;
            }
            let mut entry = Entry { id, text: None, image: None, pinned: false, time: now() };
            if mime == IMAGE_TYPE {
                let pictures = self.dir.join("clipboard");
                if std::fs::create_dir_all(&pictures).is_err() {
                    return;
                }
                let path = pictures.join(format!("{id}.png"));
                if std::fs::write(&path, bytes).is_err() {
                    return;
                }
                make_thumbnail(&path);
                entry.image = Some(path);
            } else {
                let Ok(text) = String::from_utf8(bytes.to_vec()) else {
                    return;
                };
                if text.trim().is_empty() {
                    return;
                }
                recorded = true;
                // The same text again: up to the top, pinned as it was.
                if let Some(index) = entries.iter().position(|e| e.text.as_ref() == Some(&text)) {
                    let mut same = entries.remove(index);
                    same.time = now();
                    entries.insert(0, same);
                    return;
                }
                entry.text = Some(text);
            }
            recorded = true;
            entries.insert(0, entry);
        });
        recorded.then_some(entries)
    }

    /// Copied again from the history: up to the top.
    pub fn touch(&self, id: u64) -> Vec<Entry> {
        self.change(|entries| {
            if let Some(index) = entries.iter().position(|e| e.id == id) {
                let mut entry = entries.remove(index);
                entry.time = now();
                entries.insert(0, entry);
            }
        })
    }

    pub fn pin(&self, id: u64) -> Vec<Entry> {
        self.change(|entries| {
            if let Some(entry) = entries.iter_mut().find(|e| e.id == id) {
                entry.pinned = !entry.pinned;
            }
        })
    }

    pub fn remove(&self, id: u64) -> Vec<Entry> {
        self.change(|entries| {
            if let Some(entry) = entries.iter().find(|e| e.id == id)
                && let Some(image) = &entry.image
            {
                self.remove_picture(image);
            }
            entries.retain(|e| e.id != id);
        })
    }

    /// All but the pinned.
    pub fn clear(&self) -> Vec<Entry> {
        self.change(|entries| {
            for entry in entries.iter().filter(|e| !e.pinned) {
                if let Some(image) = &entry.image {
                    self.remove_picture(image);
                }
            }
            entries.retain(|e| e.pinned);
        })
    }

    pub fn get(&self, id: u64) -> Option<Entry> {
        self.load().into_iter().find(|e| e.id == id)
    }

    /// A history kept longer before is cut down to what's kept now, and the
    /// thumbnails the pictures copied before there were any lack are made.
    pub fn tidy(&self) {
        for entry in self.change(|_| {}) {
            if let Some(image) = &entry.image {
                self.thumbnail_of(image);
            }
        }
    }
}

pub fn load() -> Vec<Entry> {
    Store::user().load()
}

/// A D-Bus of its own, for the tests: the session's isn't touched.
#[cfg(test)]
pub(crate) mod testbus {
    use std::{
        io::{BufRead, BufReader},
        path::Path,
        process::{Child, ChildStdout, Command, Stdio},
    };

    const DAEMON: &str = "/usr/bin/dbus-daemon";

    /// A session-like bus that starts nothing on its own.
    const CONFIG: &str = r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=/tmp</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#;

    /// The bus, running while this lives.
    pub struct Bus {
        child: Child,
        address: String,
        // Held open: the daemon isn't written to a closed pipe.
        _stdout: BufReader<ChildStdout>,
    }

    impl Bus {
        /// None when there's no dbus-daemon: the test is skipped.
        pub fn start() -> Option<Bus> {
            if !Path::new(DAEMON).exists() {
                return None;
            }
            // Not the session's config: its service directories would let a
            // call start the installed service on this bus.
            let config = std::env::temp_dir().join(format!("central-testbus-{}.conf", std::process::id()));
            std::fs::write(&config, CONFIG).expect("dbus-daemon's config");
            let mut child = Command::new(DAEMON)
                .arg(format!("--config-file={}", config.display()))
                .args(["--nofork", "--print-address"])
                .stdout(Stdio::piped())
                .spawn()
                .expect("dbus-daemon didn't start");
            let mut stdout = BufReader::new(child.stdout.take().expect("dbus-daemon's stdout"));
            let mut address = String::new();
            stdout.read_line(&mut address).expect("dbus-daemon's address");
            let address = address.trim().to_string();
            assert!(!address.is_empty(), "dbus-daemon printed no address");
            Some(Bus { child, address, _stdout: stdout })
        }

        pub fn address(&self) -> &str {
            &self.address
        }
    }

    impl Drop for Bus {
        fn drop(&mut self) {
            _ = self.child.kill();
            _ = self.child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("central-clipboard-{name}-{}", std::process::id()));
        _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn png(path: &Path, width: u32, height: u32) {
        image::RgbImage::new(width, height).save(path).unwrap();
    }

    #[test]
    fn a_thumbnail_is_as_tall_as_twice_the_card_and_keeps_the_shape() {
        let dir = scratch("thumb");
        let image = dir.join("1.png");
        png(&image, 1677, 919);

        make_thumbnail(&image);

        let thumb = image::open(thumbnail_path(&image)).unwrap();
        assert_eq!(thumb.height(), THUMB_HEIGHT);
        assert_eq!(thumb.width(), 175);
    }

    #[test]
    fn a_small_picture_is_not_blown_up() {
        let dir = scratch("small");
        let image = dir.join("1.png");
        png(&image, 40, 20);

        make_thumbnail(&image);

        let thumb = image::open(thumbnail_path(&image)).unwrap();
        assert_eq!((thumb.width(), thumb.height()), (40, 20));
    }

    #[test]
    fn shown_is_the_thumbnail_when_there_is_one_else_the_picture() {
        let dir = scratch("shown");
        let image = dir.join("1.png");
        png(&image, 300, 200);
        assert_eq!(shown(&image), image);

        make_thumbnail(&image);

        assert_eq!(shown(&image), thumbnail_path(&image));
    }

    fn entry(id: u64, pinned: bool) -> Entry {
        Entry { id, text: Some(format!("{id}")), image: None, pinned, time: 0 }
    }

    #[test]
    fn the_history_keeps_fifteen_besides_the_pinned() {
        let mut entries: Vec<Entry> = (0..20).map(|id| entry(id, id == 18)).collect();

        prune(&mut entries);

        let ids: Vec<u64> = entries.iter().map(|e| e.id).collect();
        assert_eq!(ids, (0..15).chain([18]).collect::<Vec<_>>());
    }

    #[test]
    fn a_long_text_is_cut_before_it_is_laid_out() {
        let text = "a".repeat(64 * 1024);

        assert_eq!(preview(&text).chars().count(), PREVIEW_CHARS);
    }

    #[test]
    fn a_short_text_shows_whole_with_its_lines_marked() {
        assert_eq!(preview("  one\ntwo  "), "one ⏎ two");
    }

    #[test]
    fn removing_a_picture_takes_its_thumbnail_too() {
        let dir = scratch("remove");
        let image = dir.join("1.png");
        png(&image, 300, 200);
        make_thumbnail(&image);

        remove_image(&image);

        assert!(!image.exists());
        assert!(!thumbnail_path(&image).exists());
    }

    #[test]
    fn record_puts_new_text_on_top_and_the_same_text_only_moves_up_keeping_the_pin() {
        let store = Store::at(scratch("record"));
        store.record("text/plain", b"a").unwrap();
        store.record("text/plain", b"b").unwrap();
        let id_a = store.load().iter().find(|e| e.text.as_deref() == Some("a")).unwrap().id;
        store.pin(id_a);
        let entries = store.record("text/plain", b"a").unwrap();
        assert_eq!(entries.iter().map(|e| e.text.clone().unwrap()).collect::<Vec<_>>(), ["a", "b"]);
        assert!(entries[0].pinned);
        assert_eq!(entries[0].id, id_a);
    }

    #[test]
    fn an_unreadable_file_is_kept_aside_not_overwritten() {
        let dir = scratch("bad");
        let store = Store::at(dir.clone());
        std::fs::write(dir.join("clipboard.json"), b"{ truncated").unwrap();

        // Reading alone leaves it be.
        assert!(store.load().is_empty());
        assert!(dir.join("clipboard.json").exists());
        assert!(!dir.join("clipboard.json.bad").exists());

        store.record("text/plain", b"x").unwrap();

        assert_eq!(std::fs::read(dir.join("clipboard.json.bad")).unwrap(), b"{ truncated");
        let entries = store.load();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].text.as_deref(), Some("x"));
    }

    #[test]
    fn a_picture_outside_the_history_directory_is_not_removed() {
        let dir = scratch("outside");
        let elsewhere = scratch("outside-target");
        let victim = elsewhere.join("precious.png");
        png(&victim, 10, 10);
        let store = Store::at(dir.clone());
        store.change(|e| {
            e.push(Entry { id: 1, text: None, image: Some(victim.clone()), pinned: false, time: 0 });
            // A path that climbs out of the directory too.
            let sneaky = dir.join("clipboard").join("..").join("..").join(victim.strip_prefix("/").unwrap());
            e.push(Entry { id: 2, text: None, image: Some(sneaky), pinned: false, time: 0 });
        });

        store.clear();
        store.remove(1);

        assert!(victim.exists());
    }

    #[test]
    fn clear_removes_the_picture_and_thumbnail_of_an_unpinned_entry() {
        let dir = scratch("clear-picture");
        let store = Store::at(dir.clone());
        let png_file = dir.join("src.png");
        png(&png_file, 300, 200);
        let entries = store.record(IMAGE_TYPE, &std::fs::read(&png_file).unwrap()).unwrap();
        let image = entries[0].image.clone().unwrap();
        assert!(image.exists() && thumbnail_path(&image).exists());

        store.clear();

        assert!(!image.exists());
        assert!(!thumbnail_path(&image).exists());
    }

    #[test]
    fn blank_or_invalid_text_is_not_recorded() {
        let store = Store::at(scratch("blank"));
        assert!(store.record("text/plain", b"  \n").is_none());
        assert!(store.record("text/plain", &[0xff, 0xfe]).is_none());
        assert!(store.load().is_empty());
    }

    #[test]
    fn a_picture_is_saved_with_its_thumbnail() {
        let dir = scratch("picture");
        let store = Store::at(dir.clone());
        let png_file = dir.join("src.png");
        png(&png_file, 300, 200);
        let entries = store.record(IMAGE_TYPE, &std::fs::read(&png_file).unwrap()).unwrap();
        let image = entries[0].image.clone().unwrap();
        assert!(image.starts_with(dir.join("clipboard")));
        assert!(thumbnail_path(&image).exists());
    }

    #[test]
    fn pin_remove_clear_and_touch_change_the_file() {
        let dir = scratch("changes");
        let store = Store::at(dir.clone());
        let png_file = dir.join("src.png");
        png(&png_file, 300, 200);
        store.record("text/plain", b"one").unwrap();
        store.record("text/plain", b"two").unwrap();
        store.record("text/plain", b"three").unwrap();
        let by_text = |text: &str| store.load().into_iter().find(|e| e.text.as_deref() == Some(text)).unwrap();
        let (one, two) = (by_text("one").id, by_text("two").id);

        // Pin toggles.
        store.pin(one);
        assert!(by_text("one").pinned);
        store.pin(one);
        assert!(!by_text("one").pinned);

        // Touch takes it to the top.
        store.touch(one);
        assert_eq!(store.load()[0].id, one);

        // Remove deletes the entry, and its picture with the thumbnail.
        let entries = store.record(IMAGE_TYPE, &std::fs::read(&png_file).unwrap()).unwrap();
        let (picture_id, image) = (entries[0].id, entries[0].image.clone().unwrap());
        assert!(image.exists() && thumbnail_path(&image).exists());
        store.remove(picture_id);
        assert!(store.load().iter().all(|e| e.id != picture_id));
        assert!(!image.exists());
        assert!(!thumbnail_path(&image).exists());

        // Clear keeps only the pinned.
        store.pin(two);
        let left = store.clear();
        assert_eq!(left.iter().map(|e| e.id).collect::<Vec<_>>(), [two]);
        assert_eq!(store.load(), left);
    }

    #[test]
    fn changes_from_two_threads_are_all_kept() {
        let store = std::sync::Arc::new(Store::at(scratch("race")));
        let threads: Vec<_> = (0..2u64)
            .map(|t| {
                let store = store.clone();
                std::thread::spawn(move || {
                    for i in 0..20 {
                        store.change(|e| {
                            e.push(Entry { id: t * 100 + i, text: Some(format!("{t}-{i}")), image: None, pinned: true, time: 0 });
                        });
                    }
                })
            })
            .collect();
        threads.into_iter().for_each(|t| t.join().unwrap());
        assert_eq!(store.load().len(), 40);
    }
}
