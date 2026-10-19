// SPDX-License-Identifier: GPL-3.0-only

//! What's playing, from MPRIS: every player with something loaded, those
//! playing first.

use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    path::PathBuf,
    time::{Duration, Instant},
};

use cosmic::iced::{Subscription, stream};
use futures::{SinkExt, StreamExt, channel::mpsc};
use zbus::{
    MatchRule, MessageStream, fdo,
    zvariant::{ObjectPath, OwnedObjectPath, OwnedValue},
};

const PATH: &str = "/org/mpris/MediaPlayer2";
const PREFIX: &str = "org.mpris.MediaPlayer2.";

#[derive(Clone, Debug, PartialEq)]
pub struct Media {
    pub service: String,
    /// The player's name, like "Spotify".
    pub player: String,
    pub title: String,
    pub artist: String,
    /// The cover, saved locally.
    pub art: Option<PathBuf>,
    /// The cover's color, to tint the card with.
    pub color: Option<[u8; 3]>,
    pub playing: bool,
    pub track: Option<OwnedObjectPath>,
    /// Microseconds.
    pub length: i64,
    /// Microseconds, as of `at`.
    pub position: i64,
    pub at: Instant,
    pub can_next: bool,
    pub can_previous: bool,
}

impl Media {
    /// Where playback is now, counting from when it was read.
    pub fn position_now(&self) -> i64 {
        let elapsed = if self.playing {
            self.at.elapsed().as_micros() as i64
        } else {
            0
        };
        (self.position + elapsed).min(self.length.max(0))
    }
}

#[zbus::proxy(interface = "org.mpris.MediaPlayer2.Player", default_path = "/org/mpris/MediaPlayer2")]
trait Player {
    fn play_pause(&self) -> zbus::Result<()>;
    fn next(&self) -> zbus::Result<()>;
    fn previous(&self) -> zbus::Result<()>;
    fn set_position(&self, track: &ObjectPath<'_>, position: i64) -> zbus::Result<()>;
    #[zbus(property)]
    fn playback_status(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn metadata(&self) -> zbus::Result<HashMap<String, OwnedValue>>;
    #[zbus(property)]
    fn position(&self) -> zbus::Result<i64>;
    #[zbus(property)]
    fn can_go_next(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn can_go_previous(&self) -> zbus::Result<bool>;
}

#[zbus::proxy(interface = "org.mpris.MediaPlayer2", default_path = "/org/mpris/MediaPlayer2")]
trait MediaPlayer {
    #[zbus(property)]
    fn identity(&self) -> zbus::Result<String>;
}

async fn player(conn: &zbus::Connection, service: &str) -> zbus::Result<PlayerProxy<'static>> {
    PlayerProxy::builder(conn)
        .destination(service.to_owned())?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
}

fn text(metadata: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    let value = metadata.get(key)?;
    String::try_from(value.clone())
        .ok()
        .or_else(|| Vec::<String>::try_from(value.clone()).ok().map(|v| v.join(", ")))
}

/// A square local copy of the cover: players give a `file://` path or a web
/// address, and browsers a video's wide thumbnail, the art in its middle.
async fn cover(url: &str) -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?
        .join(crate::APP_ID)
        .join("covers");
    let local = url.strip_prefix("file://").map(PathBuf::from);
    if local.is_none() && !url.starts_with("http") {
        return None;
    }
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    // Browsers write each cover over the same file.
    if let Some(path) = &local {
        crate::host::modified(path).await?.hash(&mut hasher);
    }
    let name = format!("{:016x}", hasher.finish());
    let square = dir.join(format!("{name}.png"));
    if square.exists() {
        return Some(square);
    }
    tokio::fs::create_dir_all(&dir).await.ok()?;
    let bytes = match &local {
        Some(path) => crate::host::read(path).await?,
        None => {
            let output = crate::host::command("curl", &[])
                .args(["-sfL", "--max-time", "5", url])
                .output()
                .await
                .ok()?;
            if !output.status.success() {
                return None;
            }
            output.stdout
        }
    };
    let saved = square.clone();
    let cropped = tokio::task::spawn_blocking(move || {
        // Browsers save it without an extension: tell the format by its bytes.
        let image = image::load_from_memory(&bytes)?;
        let side = image.width().min(image.height());
        image
            .crop_imm((image.width() - side) / 2, (image.height() - side) / 2, side, side)
            .save(&saved)?;
        Ok::<(), image::ImageError>(())
    })
    .await;
    match cropped {
        Ok(Ok(())) => Some(square),
        Ok(Err(err)) => {
            tracing::debug!("cannot crop the cover: {err}");
            // Shown as it is, if the sandbox lets it be seen.
            local.filter(|_| !crate::host::in_flatpak())
        }
        Err(_) => None,
    }
}

/// A cover's color: its pixels averaged, the more colorful counting more,
/// as phones tint their players. Kept next to the cover, once worked out.
async fn cover_color(path: &std::path::Path) -> Option<[u8; 3]> {
    let kept = path.with_extension("rgb");
    if let Ok(bytes) = tokio::fs::read(&kept).await
        && let [r, g, b] = bytes[..]
    {
        return Some([r, g, b]);
    }
    let path = path.to_owned();
    let color = tokio::task::spawn_blocking(move || {
        let image = image::open(&path).ok()?.thumbnail(24, 24).to_rgb8();
        let (mut sum, mut weights) = ([0.0f32; 3], 0.0f32);
        for pixel in image.pixels() {
            let [r, g, b] = pixel.0.map(f32::from);
            let saturation = (r.max(g).max(b) - r.min(g).min(b)) / 255.0;
            let weight = 0.05 + saturation * saturation;
            sum = [sum[0] + r * weight, sum[1] + g * weight, sum[2] + b * weight];
            weights += weight;
        }
        (weights > 0.0).then(|| sum.map(|channel| (channel / weights).round() as u8))
    })
    .await
    .ok()??;
    // only for our own copies, in the cache
    if kept.starts_with(std::env::temp_dir()) || kept.to_string_lossy().contains(crate::APP_ID) {
        _ = tokio::fs::write(&kept, color).await;
    }
    Some(color)
}

async fn read(conn: &zbus::Connection, service: &str) -> zbus::Result<Media> {
    let proxy = player(conn, service).await?;
    let metadata = proxy.metadata().await.unwrap_or_default();
    let art = match text(&metadata, "mpris:artUrl") {
        Some(url) => cover(&url).await,
        None => None,
    };
    let color = match &art {
        Some(path) => cover_color(path).await,
        None => None,
    };
    let identity = MediaPlayerProxy::builder(conn)
        .destination(service.to_owned())?
        .build()
        .await?
        .identity()
        .await
        .unwrap_or_else(|_| service.trim_start_matches(PREFIX).to_owned());
    Ok(Media {
        service: service.to_owned(),
        player: identity,
        title: text(&metadata, "xesam:title").unwrap_or_default(),
        artist: text(&metadata, "xesam:artist").unwrap_or_default(),
        art,
        color,
        playing: proxy.playback_status().await? == "Playing",
        track: metadata
            .get("mpris:trackid")
            .and_then(|v| OwnedObjectPath::try_from(v.clone()).ok()),
        length: metadata
            .get("mpris:length")
            .and_then(|v| i64::try_from(v.clone()).ok().or_else(|| u64::try_from(v.clone()).ok().map(|l| l as i64)))
            .unwrap_or(0),
        position: proxy.position().await.unwrap_or(0),
        at: Instant::now(),
        can_next: proxy.can_go_next().await.unwrap_or(false),
        can_previous: proxy.can_go_previous().await.unwrap_or(false),
    })
}

/// The players with something loaded, those playing first.
async fn players(conn: &zbus::Connection) -> Vec<Media> {
    let Ok(dbus) = fdo::DBusProxy::new(conn).await else {
        return Vec::new();
    };
    let names = dbus.list_names().await.unwrap_or_default();
    let mut players = Vec::new();
    // playerctld mirrors another player.
    for name in names
        .iter()
        .filter(|n| n.starts_with(PREFIX) && !n.ends_with(".playerctld"))
    {
        let media = match read(conn, name).await {
            Ok(media) => media,
            Err(err) => {
                tracing::debug!("cannot read {name}: {err}");
                continue;
            }
        };
        if !media.title.is_empty() {
            players.push(media);
        }
    }
    // Stable: the order players came in otherwise.
    players.sort_by_key(|m| !m.playing);
    players
}

pub fn subscription() -> Subscription<Vec<Media>> {
    Subscription::run_with("media", |_| {
        stream::channel(4, |mut output: mpsc::Sender<Vec<Media>>| async move {
            let Ok(conn) = zbus::Connection::session().await else {
                return futures::future::pending().await;
            };
            let rules = [
                MatchRule::builder()
                    .msg_type(zbus::message::Type::Signal)
                    .path(PATH)
                    .unwrap()
                    .interface("org.freedesktop.DBus.Properties")
                    .unwrap()
                    .build(),
                // A jump in what's playing, which changes no property.
                MatchRule::builder()
                    .msg_type(zbus::message::Type::Signal)
                    .path(PATH)
                    .unwrap()
                    .interface("org.mpris.MediaPlayer2.Player")
                    .unwrap()
                    .member("Seeked")
                    .unwrap()
                    .build(),
                // Players come and go. Only theirs: all of the bus's would
                // fill the queue and hold up the other signals.
                MatchRule::builder()
                    .msg_type(zbus::message::Type::Signal)
                    .sender("org.freedesktop.DBus")
                    .unwrap()
                    .member("NameOwnerChanged")
                    .unwrap()
                    .arg0ns("org.mpris.MediaPlayer2")
                    .unwrap()
                    .build(),
            ];
            let mut streams = Vec::new();
            for rule in rules {
                if let Ok(stream) = MessageStream::for_match_rule(rule, &conn, Some(64)).await {
                    streams.push(stream);
                }
            }
            let mut changes = futures::stream::select_all(streams);
            loop {
                if output.send(players(&conn).await).await.is_err() {
                    return;
                }
                if changes.next().await.is_none() {
                    return futures::future::pending().await;
                }
                tokio::time::sleep(Duration::from_millis(150)).await;
                while futures::FutureExt::now_or_never(changes.next())
                    .flatten()
                    .is_some()
                {}
            }
        })
    })
}

#[derive(Clone, Copy, Debug)]
pub enum Control {
    PlayPause,
    Next,
    Previous,
}

pub async fn control(service: String, control: Control) -> zbus::Result<()> {
    let conn = zbus::Connection::session().await?;
    let proxy = player(&conn, &service).await?;
    match control {
        Control::PlayPause => proxy.play_pause().await,
        Control::Next => proxy.next().await,
        Control::Previous => proxy.previous().await,
    }
}

pub async fn seek(service: String, track: OwnedObjectPath, position: i64) -> zbus::Result<()> {
    let conn = zbus::Connection::session().await?;
    player(&conn, &service)
        .await?
        .set_position(&track, position)
        .await
}
