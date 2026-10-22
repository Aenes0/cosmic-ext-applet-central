// SPDX-License-Identifier: GPL-3.0-only

//! Notifications, as apps send them to the daemon.
//!
//! They're seen on the session bus, as `dbus-monitor` sees them: the
//! `Notify` calls, and the daemon's answers with their ids. Not through the
//! socket the panel gives applets marked `X-NotificationsApplet`: when the
//! panel can't get one from the daemon, which happens after it restarts its
//! applets a few times, it doesn't start those applets at all, and the whole
//! applet was gone from the bar. Dismissed and opened ones, through the usual
//! `org.freedesktop.Notifications` signals. Passing ones (`transient`, or the
//! daemon's `osd` pills) stay out of the history.
//!
//! Actions run through the daemon's `InvokeAction` method on the session bus,
//! not through that socket either.

use std::{
    collections::{HashMap, HashSet},
    time::SystemTime,
};

use cosmic::iced::{Subscription, stream};
use futures::{SinkExt, StreamExt, channel::mpsc};
use zbus::{connection::Builder, zvariant::Value};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Notification {
    pub id: u32,
    pub app: String,
    pub icon: String,
    pub summary: String,
    pub body: String,
    /// Pairs of action key and label.
    pub actions: Vec<(String, String)>,
    pub time: SystemTime,
    /// Kept from before the applet started: the daemon has forgotten it, and
    /// its id may be another's now.
    #[serde(default)]
    pub restored: bool,
}

fn history_file() -> Option<std::path::PathBuf> {
    let dir = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/state"))
        })?;
    Some(dir.join(crate::APP_ID).join("notifications.json"))
}

/// The notifications kept from the last run.
pub fn load() -> Vec<Notification> {
    let Some(file) = history_file() else {
        return Vec::new();
    };
    let Ok(data) = std::fs::read(file) else {
        return Vec::new();
    };
    let mut notifications: Vec<Notification> = serde_json::from_slice(&data).unwrap_or_default();
    for notification in &mut notifications {
        notification.restored = true;
        notification.actions.clear();
        // Kept before icons came from the apps' entries, or a file since gone.
        notification.icon = app_icon(&notification.app, None, &notification.icon, None);
        notification.body = plain(&notification.body);
    }
    notifications
}

pub fn save(notifications: &[Notification]) {
    let Some(file) = history_file() else {
        return;
    };
    let result = file
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| {
            let data = serde_json::to_vec(notifications).map_err(std::io::Error::other)?;
            std::fs::write(&file, data)
        });
    if let Err(err) = result {
        tracing::warn!("cannot keep the notifications: {err}");
    }
}

#[derive(Clone, Debug)]
pub enum Event {
    New(Notification),
    /// Gone: opened, or closed by its app or from here.
    Closed(u32),
    /// Its popup closed by hand. It stays, as a heads-up swiped away on
    /// Android does, but the daemon has let go of it.
    Dismissed(u32),
}

/// `NotificationClosed`'s reason when the user closed it.
const DISMISSED_BY_USER: u32 = 2;

/// The daemon's own interface, on its usual name: actions run from here,
/// even of notifications whose popup has gone.
#[zbus::proxy(
    default_service = "org.freedesktop.Notifications",
    interface = "com.system76.NotificationsApplet",
    default_path = "/com/system76/NotificationsApplet"
)]
trait NotificationsApplet {
    fn invoke_action(&self, id: u32, action: &str) -> zbus::Result<()>;
}

#[zbus::proxy(
    default_service = "org.freedesktop.Notifications",
    interface = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
trait Notifications {
    fn close_notification(&self, id: u32) -> zbus::Result<()>;

    #[zbus(signal)]
    fn notification_closed(&self, id: u32, reason: u32) -> zbus::Result<()>;

    #[zbus(signal)]
    fn action_invoked(&self, id: u32, action_key: &str) -> zbus::Result<()>;
}

/// What the daemon and the bus tell, before it makes an event.
enum Signal {
    New(Notification),
    Invoked(u32),
    Closed(u32, u32),
}

/// Passing ones, kept out of the history: `transient` (true, or a number
/// other than 0, as some apps send it) or the daemon's `osd` pills.
fn skip_in_history(hints: &HashMap<String, zbus::zvariant::OwnedValue>) -> bool {
    let transient = hints.get("transient").is_some_and(|value| match &**value {
        Value::Bool(b) => *b,
        Value::U8(n) => *n != 0,
        Value::I16(n) => *n != 0,
        Value::U16(n) => *n != 0,
        Value::I32(n) => *n != 0,
        Value::U32(n) => *n != 0,
        Value::I64(n) => *n != 0,
        Value::U64(n) => *n != 0,
        _ => false,
    });
    let osd = hints
        .get("x-estilo")
        .is_some_and(|value| matches!(&**value, Value::Str(s) if s.as_str() == "osd"));
    transient || osd
}

/// New notifications, seen on the session bus as a monitor: each `Notify`
/// call, made whole by the daemon's answer, which gives its id. Only those
/// calls and the daemon's answers are seen, a few messages a notification.
async fn bus_notifications() -> zbus::Result<impl futures::Stream<Item = Signal>> {
    let conn = Builder::session()?.build().await?;
    zbus::fdo::MonitoringProxy::new(&conn)
        .await?
        .become_monitor(
            &[
                "type='method_call',interface='org.freedesktop.Notifications',member='Notify'"
                    .try_into()?,
                "type='method_return',sender='org.freedesktop.Notifications'".try_into()?,
            ],
            0,
        )
        .await?;
    type Notify = (
        String,
        u32,
        String,
        String,
        String,
        Vec<String>,
        HashMap<String, zbus::zvariant::OwnedValue>,
        i32,
    );
    // Calls waiting for their answer, by who called and the call's number.
    let mut waiting: HashMap<(String, u32), Notification> = HashMap::new();
    Ok(zbus::MessageStream::from(conn).filter_map(move |message| {
        let signal = (|| {
            let message = message.ok()?;
            let header = message.header();
            match header.message_type() {
                zbus::message::Type::MethodCall => {
                    let (app, _, icon, summary, body, actions, hints, _): Notify =
                        message.body().deserialize().ok()?;
                    if skip_in_history(&hints) {
                        return None;
                    }
                    let hint = |key: &str| {
                        hints.get(key).and_then(|v| String::try_from(v.clone()).ok())
                    };
                    let notification = Notification {
                        id: 0,
                        icon: app_icon(
                            &app,
                            hint("desktop-entry").as_deref(),
                            &icon,
                            hint("image-path").as_deref(),
                        ),
                        app,
                        summary,
                        body: plain(&body),
                        actions: actions
                            .chunks(2)
                            .filter_map(|pair| match pair {
                                [key, label] => Some((key.clone(), label.clone())),
                                _ => None,
                            })
                            .collect(),
                        time: SystemTime::now(),
                        restored: false,
                    };
                    let caller = header.sender()?.to_string();
                    // Not answered for long: the daemon's gone, say.
                    if waiting.len() > 64 {
                        waiting.clear();
                    }
                    waiting.insert((caller, header.primary().serial_num().get()), notification);
                    None
                }
                zbus::message::Type::MethodReturn => {
                    let caller = header.destination()?.to_string();
                    let serial = header.reply_serial()?.get();
                    let mut notification = waiting.remove(&(caller, serial))?;
                    notification.id = message.body().deserialize::<u32>().ok()?;
                    Some(Signal::New(notification))
                }
                _ => None,
            }
        })();
        async move { signal }
    }))
}

pub fn subscription() -> Subscription<Event> {
    Subscription::run_with("notifications", |_| {
        stream::channel(50, |mut output: mpsc::Sender<Event>| async move {
            let notify = match bus_notifications().await {
                Ok(notify) => notify,
                Err(err) => {
                    tracing::warn!("cannot watch notifications on the bus: {err}");
                    return futures::future::pending().await;
                }
            };
            let Ok(session) = zbus::Connection::session().await else {
                return futures::future::pending().await;
            };
            let Ok(freedesktop) = NotificationsProxy::new(&session).await else {
                return futures::future::pending().await;
            };
            let (Ok(closed), Ok(invoked)) = (
                freedesktop.receive_notification_closed().await,
                freedesktop.receive_action_invoked().await,
            ) else {
                return futures::future::pending().await;
            };
            let closed = closed.filter_map(|signal| async move {
                let args = signal.args().ok()?;
                Some(Signal::Closed(args.id, args.reason))
            });
            let invoked = invoked.filter_map(|signal| async move {
                Some(Signal::Invoked(signal.args().ok()?.id))
            });
            let mut signals =
                futures::stream::select_all([notify.boxed(), closed.boxed(), invoked.boxed()]);
            let mut opened = HashSet::new();
            while let Some(signal) = signals.next().await {
                let Some(event) = event(&mut opened, signal) else {
                    continue;
                };
                if output.send(event).await.is_err() {
                    return;
                }
            }
            futures::future::pending().await
        })
    })
}

/// What a signal means here. A click on a popup runs its action, then
/// closes it as the user would: those opened go, the others stay.
fn event(opened: &mut HashSet<u32>, signal: Signal) -> Option<Event> {
    Some(match signal {
        Signal::New(notification) => Event::New(notification),
        Signal::Invoked(id) => {
            opened.insert(id);
            return None;
        }
        Signal::Closed(id, DISMISSED_BY_USER) if !opened.contains(&id) => Event::Dismissed(id),
        Signal::Closed(id, _) => {
            opened.remove(&id);
            Event::Closed(id)
        }
    })
}

/// The icon a notification shows, by name or as a file: its app's, from the
/// app's desktop entry, as GNOME shows it; else the one it came with.
/// Browsers send theirs as a `file://` URI, often to a file soon gone.
fn app_icon(app: &str, desktop_entry: Option<&str>, icon: &str, image: Option<&str>) -> String {
    if let Some(icon) = desktop_icon(app, desktop_entry) {
        return icon;
    }
    [Some(icon), image]
        .into_iter()
        .flatten()
        .map(|icon| icon.strip_prefix("file://").unwrap_or(icon))
        .find(|icon| {
            !icon.is_empty() && (!icon.starts_with('/') || std::path::Path::new(icon).exists())
        })
        .unwrap_or_default()
        .to_owned()
}

/// An app's icon from its desktop entry: by the entry's name, which apps
/// may say, or by the app's name.
fn desktop_icon(app: &str, desktop_entry: Option<&str>) -> Option<String> {
    use std::{collections::HashMap, sync::Mutex};
    struct Entry {
        id: String,
        name: String,
        wm_class: String,
        icon: String,
    }
    static ENTRIES: Mutex<Option<Vec<Entry>>> = Mutex::new(None);
    static FOUND: Mutex<Option<HashMap<(String, Option<String>), Option<String>>>> =
        Mutex::new(None);

    let key = (app.to_owned(), desktop_entry.map(str::to_owned));
    if let Some(found) = FOUND.lock().ok()?.get_or_insert_default().get(&key) {
        return found.clone();
    }
    let mut entries = ENTRIES.lock().ok()?;
    let entries = entries.get_or_insert_with(|| {
        let home = std::env::var("HOME").unwrap_or_default();
        let data_home = std::env::var("XDG_DATA_HOME")
            .unwrap_or_else(|_| format!("{home}/.local/share"));
        let data_dirs = std::env::var("XDG_DATA_DIRS")
            .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_owned());
        let dirs = std::iter::once(data_home)
            .chain(data_dirs.split(':').map(str::to_owned))
            .chain([
                format!("{home}/.local/share/flatpak/exports/share"),
                "/var/lib/flatpak/exports/share".to_owned(),
                // the host's, from a Flatpak sandbox
                "/run/host/usr/share".to_owned(),
            ]);
        let mut entries = Vec::new();
        for dir in dirs {
            let Ok(files) = std::fs::read_dir(format!("{dir}/applications")) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().is_none_or(|e| e != "desktop") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let mut entry = Entry {
                    id: path.file_stem().unwrap_or_default().to_string_lossy().to_lowercase(),
                    name: String::new(),
                    wm_class: String::new(),
                    icon: String::new(),
                };
                // [Desktop Entry] only, not its actions'.
                for line in text
                    .lines()
                    .skip_while(|l| l.trim() != "[Desktop Entry]")
                    .skip(1)
                    .take_while(|l| !l.starts_with('['))
                {
                    match line.split_once('=') {
                        Some(("Name", value)) => entry.name = value.trim().to_lowercase(),
                        Some(("StartupWMClass", value)) => {
                            entry.wm_class = value.trim().to_lowercase()
                        }
                        Some(("Icon", value)) => entry.icon = value.trim().to_owned(),
                        _ => {}
                    }
                }
                if !entry.icon.is_empty() {
                    entries.push(entry);
                }
            }
        }
        entries
    });
    let app = app.trim().to_lowercase();
    let wanted = desktop_entry.map(|e| e.trim_end_matches(".desktop").to_lowercase());
    let found = wanted
        .and_then(|id| {
            entries
                .iter()
                .find(|e| e.id == id)
                .or_else(|| entries.iter().find(|e| e.id.rsplit('.').next() == Some(&id)))
        })
        .or_else(|| {
            if app.is_empty() {
                return None;
            }
            let dashed = app.replace(' ', "-");
            entries
                .iter()
                .find(|e| e.name == app)
                .or_else(|| entries.iter().find(|e| e.wm_class == app || e.id == dashed))
                .or_else(|| entries.iter().find(|e| e.id.rsplit('.').next() == Some(&dashed)))
        })
        .map(|e| e.icon.clone());
    FOUND
        .lock()
        .ok()?
        .get_or_insert_default()
        .insert(key, found.clone());
    found
}

/// A body as text: without the markup some apps send (links, bold…), and
/// with its entities read.
fn plain(body: &str) -> String {
    let mut text = String::with_capacity(body.len());
    let mut in_tag = false;
    for c in body.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            c if !in_tag => text.push(c),
            _ => {}
        }
    }
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
        .trim()
        .to_owned()
}

/// Run an action, `default` being a click on the notification.
pub async fn invoke(id: u32, action: String) {
    let result = async {
        let session = zbus::Connection::session().await?;
        NotificationsAppletProxy::new(&session)
            .await?
            .invoke_action(id, &action)
            .await
    };
    if let Err(err) = result.await {
        tracing::warn!("cannot run notification action: {err}");
    }
}

pub async fn dismiss(ids: Vec<u32>) {
    let Ok(session) = zbus::Connection::session().await else {
        return;
    };
    let Ok(proxy) = NotificationsProxy::new(&session).await else {
        return;
    };
    for id in ids {
        _ = proxy.close_notification(id).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup() {
        assert_eq!(
            plain("Ready: <a href=\"https://claude.ai/\">claude.ai</a> &amp; <b>more</b>"),
            "Ready: claude.ai & more"
        );
    }

    #[test]
    fn popups_closed() {
        let mut opened = HashSet::new();
        let mut close = |signal| event(&mut opened, signal);
        // Closed by hand on its popup: kept.
        assert!(matches!(close(Signal::Closed(1, 2)), Some(Event::Dismissed(1))));
        // Clicked: its action, then closed twice, as cosmic-notifications does.
        assert!(close(Signal::Invoked(2)).is_none());
        assert!(matches!(close(Signal::Closed(2, 2)), Some(Event::Closed(2))));
        assert!(matches!(close(Signal::Closed(2, 2)), Some(Event::Dismissed(2))));
        // Closed by its app: gone.
        assert!(matches!(close(Signal::Closed(3, 3)), Some(Event::Closed(3))));
    }

    #[test]
    fn history_skips_transient_and_osd() {
        fn hints(pairs: &[(&str, Value<'_>)]) -> HashMap<String, zbus::zvariant::OwnedValue> {
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.try_to_owned().unwrap()))
                .collect()
        }
        let transient = |value: Value<'static>| hints(&[("transient", value)]);
        assert!(skip_in_history(&transient(Value::from(true))));
        assert!(skip_in_history(&transient(Value::from(1i32))));
        assert!(skip_in_history(&transient(Value::from(1u8))));
        assert!(!skip_in_history(&transient(Value::from(false))));
        assert!(!skip_in_history(&transient(Value::from(0i32))));
        assert!(skip_in_history(&hints(&[("x-estilo", Value::from("osd"))])));
        assert!(!skip_in_history(&hints(&[("x-estilo", Value::from("cartao"))])));
        assert!(!skip_in_history(&HashMap::new()));
    }

    #[test]
    fn sent_icon() {
        assert_eq!(app_icon("", None, "dialog-information", None), "dialog-information");
        assert_eq!(app_icon("", None, "file:///nowhere/logo.png", None), "");
    }
}
