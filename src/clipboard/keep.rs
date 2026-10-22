// SPDX-License-Identifier: GPL-3.0-only

//! What's kept of something copied: all of the formats the app offers, read
//! at once, to offer them again when the app is gone.

use std::{
    collections::HashMap,
    io::{ErrorKind, PipeReader, Read},
    os::fd::{AsRawFd, RawFd},
    sync::Arc,
    time::Duration,
};

use super::{Entry, IMAGE_TYPE, TEXT_TYPES};

/// Ours: on what the history puts on the clipboard, not to write it down again.
pub const HISTORY_MARK: &str = "x-central-history";
/// A password manager's: what it copies is neither written down nor kept.
pub const SECRET_HINT: &str = "x-kde-passwordManagerHint";

/// X11's targets that aren't formats: nothing to read in them.
const META_TYPES: [&str; 7] = [
    "TARGETS",
    "MULTIPLE",
    "TIMESTAMP",
    "SAVE_TARGETS",
    "DELETE",
    "INSERT_PROPERTY",
    "INSERT_SELECTION",
];

/// Read from a pipe at a time, at most.
const CHUNK: usize = 64 * 1024;

/// A format's type, and its bytes.
pub type Formats = HashMap<String, Arc<Vec<u8>>>;

/// Put on the clipboard by the history itself.
pub fn is_ours(types: &[String]) -> bool {
    types.iter().any(|t| t == HISTORY_MARK)
}

/// Copied by a password manager.
pub fn is_secret(types: &[String]) -> bool {
    types.iter().any(|t| t == SECRET_HINT)
}

/// The formats worth reading: in the order offered, each once, without
/// X11's meta types nor the marks.
pub fn readable(types: &[String]) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    for mime in types {
        let skipped = META_TYPES.contains(&mime.as_str()) || mime == HISTORY_MARK || mime == SECRET_HINT;
        if !skipped && !kept.contains(mime) {
            kept.push(mime.clone());
        }
    }
    kept
}

/// The format written down in the history: text first, in the order of
/// `TEXT_TYPES`, then a PNG picture.
pub fn primary(types: &[String]) -> Option<String> {
    TEXT_TYPES
        .iter()
        .chain(std::iter::once(&IMAGE_TYPE))
        .find(|mime| types.iter().any(|t| t == *mime))
        .map(|mime| mime.to_string())
}

/// What to offer of something in the history: a text as every text type, a
/// picture as a PNG; with our mark, either. None for a picture that can't
/// be read, or an entry with neither.
pub fn entry_formats(entry: &Entry) -> Option<Formats> {
    let mut formats = Formats::new();
    if let Some(text) = &entry.text {
        let bytes = Arc::new(text.as_bytes().to_vec());
        for mime in TEXT_TYPES {
            formats.insert(mime.to_string(), bytes.clone());
        }
    } else if let Some(image) = &entry.image {
        let bytes = std::fs::read(image).ok()?;
        formats.insert(IMAGE_TYPE.to_string(), Arc::new(bytes));
    } else {
        return None;
    }
    formats.insert(HISTORY_MARK.to_string(), Arc::new(Vec::new()));
    Some(formats)
}

/// How much is read of something copied, and how long the app may be silent.
#[derive(Debug, Clone)]
pub struct Limits {
    /// Bigger than this, a format isn't kept.
    pub per_format: usize,
    /// All the formats together, at most.
    pub total: usize,
    /// Nothing on any pipe for this long: the rest is given up. No total
    /// deadline on purpose (a big PNG may take seconds); the read has its own thread.
    pub idle: Duration,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            per_format: 16 * 1024 * 1024,
            total: 32 * 1024 * 1024,
            idle: Duration::from_secs(2),
        }
    }
}

/// A format still being read.
struct Reading {
    mime: String,
    pipe: PipeReader,
    bytes: Vec<u8>,
}

/// What came of reading a pipe for now.
enum Outcome {
    /// More to come.
    Open,
    /// Read to its end.
    Done,
    /// Too big, or broken: given up.
    Dropped,
}

/// Reads all the formats at once, never waiting on one while the app writes
/// another. Only those read to their end are kept, and none empty; over the
/// limits, as `keep_done` says.
pub fn read_all(pipes: Vec<(String, PipeReader)>, primary: Option<&str>, limits: &Limits) -> Formats {
    let mut done = Formats::new();
    let mut open: Vec<Reading> = pipes
        .into_iter()
        .filter(|(mime, pipe)| {
            let ok = set_nonblocking(pipe.as_raw_fd());
            if !ok {
                tracing::warn!("clipboard: can't make the pipe of {mime} non-blocking, skipped");
            }
            ok
        })
        .map(|(mime, pipe)| Reading { mime, pipe, bytes: Vec::new() })
        .collect();
    let timeout = i32::try_from(limits.idle.as_millis()).unwrap_or(i32::MAX);
    let mut chunk = vec![0u8; CHUNK];

    while !open.is_empty() {
        let mut fds: Vec<libc::pollfd> = open
            .iter()
            .map(|r| libc::pollfd { fd: r.pipe.as_raw_fd(), events: libc::POLLIN, revents: 0 })
            .collect();
        // SAFETY: `fds` is a valid array of `fds.len()` pollfd, alive for the call.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == ErrorKind::Interrupted {
                continue;
            }
            tracing::warn!("clipboard: poll failed, the rest is given up: {error}");
            break;
        }
        if ready == 0 {
            // Silence: what isn't done by now is given up.
            break;
        }

        let mut still_open = Vec::with_capacity(open.len());
        for (mut reading, fd) in open.into_iter().zip(&fds) {
            if fd.revents == 0 {
                still_open.push(reading);
                continue;
            }
            let outcome = if fd.revents & libc::POLLNVAL != 0 {
                Outcome::Dropped
            } else {
                drain(&mut reading, &mut chunk, limits.per_format)
            };
            match outcome {
                Outcome::Open => still_open.push(reading),
                Outcome::Done => keep_done(&mut done, reading.mime, reading.bytes, primary, limits.total),
                Outcome::Dropped => {}
            }
        }
        open = still_open;
    }
    done
}

/// Reads what's in a pipe now, up to its end or till it would wait.
fn drain(reading: &mut Reading, chunk: &mut [u8], per_format: usize) -> Outcome {
    loop {
        match reading.pipe.read(chunk) {
            Ok(0) => return Outcome::Done,
            Ok(n) => {
                if reading.bytes.len() + n > per_format {
                    return Outcome::Dropped;
                }
                reading.bytes.extend_from_slice(&chunk[..n]);
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) if e.kind() == ErrorKind::WouldBlock => return Outcome::Open,
            Err(_) => return Outcome::Dropped,
        }
    }
}

/// A format read to its end, kept unless empty. Over the total: the
/// primary one makes room, dropping the others, biggest first; another is
/// dropped itself.
fn keep_done(done: &mut Formats, mime: String, bytes: Vec<u8>, primary: Option<&str>, total: usize) {
    if bytes.is_empty() {
        return;
    }
    let mut used: usize = done.values().map(|b| b.len()).sum();
    if used + bytes.len() > total {
        // Not even alone: dropped, without sending the others away for nothing.
        if primary != Some(mime.as_str()) || bytes.len() > total {
            return;
        }
        let mut others: Vec<(String, usize)> = done.iter().map(|(m, b)| (m.clone(), b.len())).collect();
        others.sort_by_key(|other| std::cmp::Reverse(other.1));
        for (other, size) in others {
            if used + bytes.len() <= total {
                break;
            }
            done.remove(&other);
            used -= size;
        }
        if used + bytes.len() > total {
            return;
        }
    }
    done.insert(mime, Arc::new(bytes));
}

/// Reading it won't wait for the app. False if it can't be set.
fn set_nonblocking(fd: RawFd) -> bool {
    // SAFETY: fcntl on a file descriptor owned by a live PipeReader.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        flags >= 0 && libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) >= 0
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{PipeWriter, Write},
        path::PathBuf,
        time::Instant,
    };

    use super::*;

    fn t(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn pipe() -> (PipeReader, PipeWriter) {
        std::io::pipe().unwrap()
    }

    /// A pipe with these bytes in it, closed.
    fn written(bytes: &[u8]) -> PipeReader {
        let (reader, mut writer) = pipe();
        writer.write_all(bytes).unwrap();
        reader
    }

    #[test]
    fn readable_drops_x11_meta_types_and_our_marks() {
        assert_eq!(
            readable(&t(&["TARGETS", "text/html", "UTF8_STRING", "x-central-history", "SAVE_TARGETS", "text/html"])),
            t(&["text/html", "UTF8_STRING"])
        );
    }

    #[test]
    fn primary_prefers_text_in_its_order_then_png() {
        assert_eq!(primary(&t(&["image/png", "STRING", "text/plain"])), Some("text/plain".to_string()));
        assert_eq!(primary(&t(&["image/jpeg", "image/png"])), Some("image/png".to_string()));
        assert_eq!(primary(&t(&["text/html"])), None);
    }

    #[test]
    fn a_secret_is_neither_recorded_nor_kept() {
        let types = t(&["text/plain", "x-kde-passwordManagerHint"]);
        assert!(is_secret(&types));
        assert!(!is_secret(&t(&["text/plain"])));
        assert!(!readable(&types).contains(&SECRET_HINT.to_string()));
    }

    #[test]
    fn our_own_offer_is_known_by_its_mark() {
        assert!(is_ours(&t(&["text/plain", "x-central-history"])));
        assert!(!is_ours(&t(&["text/plain"])));
    }

    #[test]
    fn entry_formats_offers_text_in_every_text_type_with_our_mark() {
        let entry = Entry { id: 1, text: Some("olá".into()), image: None, pinned: false, time: 0 };

        let formats = entry_formats(&entry).unwrap();

        assert_eq!(formats.len(), TEXT_TYPES.len() + 1);
        for mime in TEXT_TYPES {
            assert_eq!(formats[mime].as_slice(), "olá".as_bytes());
        }
        assert!(formats[HISTORY_MARK].is_empty());
    }

    #[test]
    fn entry_formats_offers_a_picture_as_png_with_our_mark() {
        let dir = std::env::temp_dir().join(format!("central-keep-picture-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("1.png");
        std::fs::write(&path, b"not really a png").unwrap();
        let entry = Entry { id: 1, text: None, image: Some(path), pinned: false, time: 0 };

        let formats = entry_formats(&entry).unwrap();

        assert_eq!(formats.len(), 2);
        assert_eq!(formats[IMAGE_TYPE].as_slice(), b"not really a png");
        assert!(formats[HISTORY_MARK].is_empty());
        std::fs::remove_dir_all(&dir).unwrap();

        let gone = Entry { image: Some(PathBuf::from("/nonexistent/1.png")), ..entry.clone() };
        assert!(entry_formats(&gone).is_none());
        let nothing = Entry { image: None, ..entry };
        assert!(entry_formats(&nothing).is_none());
    }

    #[test]
    fn pipes_written_out_of_order_do_not_block() {
        const SIZE: usize = 200 * 1024;
        let (r1, w1) = pipe();
        let (r2, w2) = pipe();
        let (r3, w3) = pipe();
        let writer = std::thread::spawn(move || {
            for (mut w, byte) in [(w3, 3u8), (w1, 1), (w2, 2)] {
                w.write_all(&vec![byte; SIZE]).unwrap();
            }
        });

        let formats = read_all(
            vec![("one".into(), r1), ("two".into(), r2), ("three".into(), r3)],
            None,
            &Limits::default(),
        );
        writer.join().unwrap();

        assert_eq!(formats.len(), 3);
        assert_eq!(formats["one"].as_slice(), vec![1u8; SIZE].as_slice());
        assert_eq!(formats["two"].as_slice(), vec![2u8; SIZE].as_slice());
        assert_eq!(formats["three"].as_slice(), vec![3u8; SIZE].as_slice());
    }

    #[test]
    fn a_format_over_its_limit_is_dropped_the_others_kept() {
        let limits = Limits { per_format: 1000, ..Limits::default() };

        let formats = read_all(
            vec![("a".into(), written(&[7; 2000])), ("b".into(), written(&[8; 10]))],
            None,
            &limits,
        );

        assert_eq!(formats.len(), 1);
        assert_eq!(formats["b"].as_slice(), &[8; 10]);
    }

    #[test]
    fn the_recorded_format_survives_the_total_limit() {
        let limits = Limits { per_format: 1000, total: 1500, ..Limits::default() };
        // The bitmap written and closed first: it's done before the PNG.
        let bmp = written(&[1; 900]);
        let png = written(&[2; 900]);

        let formats = read_all(
            vec![("image/bmp".into(), bmp), ("image/png".into(), png)],
            Some("image/png"),
            &limits,
        );

        assert_eq!(formats["image/png"].len(), 900);
        assert!(!formats.contains_key("image/bmp"));
    }

    #[test]
    fn another_format_over_the_total_is_dropped() {
        let limits = Limits { per_format: 1000, total: 1500, ..Limits::default() };

        let formats = read_all(
            vec![("text/plain".into(), written(&[1; 900])), ("text/html".into(), written(&[2; 900]))],
            Some("text/plain"),
            &limits,
        );

        assert_eq!(formats.len(), 1);
        assert_eq!(formats["text/plain"].len(), 900);
    }

    #[test]
    fn an_empty_format_is_not_kept() {
        let formats = read_all(
            vec![("a".into(), written(b"")), ("b".into(), written(b"x"))],
            None,
            &Limits::default(),
        );

        assert_eq!(formats.len(), 1);
        assert!(formats.contains_key("b"));
    }

    #[test]
    fn a_silent_pipe_gives_up_after_the_idle_time() {
        let limits = Limits { idle: Duration::from_millis(200), ..Limits::default() };
        let (silent, _held_open) = pipe();
        let start = Instant::now();

        let formats = read_all(vec![("a".into(), written(b"hello")), ("b".into(), silent)], None, &limits);

        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(formats.len(), 1);
        assert_eq!(formats["a"].as_slice(), b"hello");
    }

    #[test]
    fn a_slow_writer_is_read_whole() {
        let limits = Limits { idle: Duration::from_millis(300), ..Limits::default() };
        let (reader, mut writer) = pipe();
        let thread = std::thread::spawn(move || {
            for _ in 0..10 {
                writer.write_all(&[5]).unwrap();
                std::thread::sleep(Duration::from_millis(150));
            }
        });

        let formats = read_all(vec![("a".into(), reader)], None, &limits);
        thread.join().unwrap();

        assert_eq!(formats["a"].as_slice(), &[5; 10]);
    }

    #[test]
    fn a_talking_pipe_keeps_a_quiet_one_alive() {
        let limits = Limits { idle: Duration::from_millis(300), ..Limits::default() };
        let (ra, mut wa) = pipe();
        let (rb, mut wb) = pipe();
        let talker = std::thread::spawn(move || {
            for _ in 0..7 {
                wa.write_all(&[1]).unwrap();
                std::thread::sleep(Duration::from_millis(150));
            }
        });
        let quiet = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(900));
            wb.write_all(&[2; 5]).unwrap();
        });

        let formats = read_all(vec![("a".into(), ra), ("b".into(), rb)], None, &limits);
        talker.join().unwrap();
        quiet.join().unwrap();

        assert_eq!(formats["a"].len(), 7);
        assert_eq!(formats["b"].as_slice(), &[2; 5]);
    }

    #[test]
    fn over_the_total_the_largest_others_go_first() {
        let limits = Limits { per_format: 1000, total: 1500, ..Limits::default() };

        let formats = read_all(
            vec![
                ("image/bmp".into(), written(&[1; 900])),
                ("image/jpeg".into(), written(&[2; 300])),
                ("image/png".into(), written(&[3; 900])),
            ],
            Some("image/png"),
            &limits,
        );

        assert!(formats.contains_key("image/png"));
        assert!(formats.contains_key("image/jpeg"));
        assert!(!formats.contains_key("image/bmp"));
    }

    #[test]
    fn a_primary_over_the_total_drops_nothing_else() {
        let limits = Limits { per_format: 2000, total: 1500, ..Limits::default() };

        let formats = read_all(
            vec![("image/bmp".into(), written(&[1; 900])), ("image/png".into(), written(&[2; 1600]))],
            Some("image/png"),
            &limits,
        );

        assert!(formats.contains_key("image/bmp"));
        assert!(!formats.contains_key("image/png"));
    }

    #[test]
    fn the_default_limits() {
        let limits = Limits::default();
        assert_eq!(limits.per_format, 16 * 1024 * 1024);
        assert_eq!(limits.total, 32 * 1024 * 1024);
        assert_eq!(limits.idle, Duration::from_secs(2));
    }
}
