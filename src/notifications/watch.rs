// SPDX-License-Identifier: GPL-3.0-only

//! The bus side of the notifications, used by the service alone.
//!
//! They're seen on the session bus, as `dbus-monitor` sees them: the
//! `Notify` calls, and the daemon's answers with their ids. Not through the
//! socket the panel gives applets marked `X-NotificationsApplet`: when the
//! panel can't get one from the daemon, which happens after it restarts its
//! applets a few times, it doesn't start those applets at all, and the whole
//! applet was gone from the bar. Dismissed and opened ones, through the usual
//! `org.freedesktop.Notifications` signals. Passing ones (`transient`, or the
//! daemon's `osd` pills) stay out of the history.

use std::{
    collections::{HashMap, HashSet},
    time::SystemTime,
};

use futures::StreamExt;
use zbus::{connection::Builder, zvariant::Value};

use super::{Event, Notification};

/// `NotificationClosed`'s reason when the user closed it.
const DISMISSED_BY_USER: u32 = 2;

/// What the daemon and the bus tell, before it makes an event.
enum Signal {
    New(Notification),
    Invoked(u32),
    Closed(u32, u32),
    /// The daemon's name changed hands.
    Reset,
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

/// The longest an app's text is kept, in characters: a message on the bus can
/// be far larger, and the history is read by every applet at each change.
const SHORT: usize = 256;
const BODY: usize = 4096;
/// The most buttons a notification keeps.
const ACTIONS: usize = 8;

/// At most `max` characters.
fn cap(text: String, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => text[..end].to_owned(),
        None => text,
    }
}

/// What the session bus tells about the notifications, seen as a monitor, in
/// the order the bus delivered it: each `Notify` call, made whole by the
/// daemon's answer, which gives its id; the daemon's `NotificationClosed` and
/// `ActionInvoked`; and the daemon's name changing hands. One connection, so
/// a notification and its closing can't be told out of order. Ends when the
/// connection does.
async fn bus_signals(
    builder: Builder<'_>,
) -> zbus::Result<impl futures::Stream<Item = Signal> + use<>> {
    let conn = builder.build().await?;
    zbus::fdo::MonitoringProxy::new(&conn)
        .await?
        .become_monitor(
            &[
                "type='method_call',interface='org.freedesktop.Notifications',member='Notify'"
                    .try_into()?,
                "type='method_return',sender='org.freedesktop.Notifications'".try_into()?,
                "type='signal',sender='org.freedesktop.Notifications',interface='org.freedesktop.Notifications'"
                    .try_into()?,
                "type='signal',sender='org.freedesktop.DBus',interface='org.freedesktop.DBus',member='NameOwnerChanged',arg0='org.freedesktop.Notifications'"
                    .try_into()?,
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
            let message = match message {
                Ok(message) => message,
                Err(err) => {
                    tracing::warn!("a message on the bus wasn't read: {err}");
                    return None;
                }
            };
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
                        app: cap(app, SHORT),
                        summary: cap(summary, SHORT),
                        body: cap(plain(&body), BODY),
                        actions: actions
                            .chunks(2)
                            .filter_map(|pair| match pair {
                                [key, label] => Some((cap(key.clone(), SHORT), cap(label.clone(), SHORT))),
                                _ => None,
                            })
                            .take(ACTIONS)
                            .collect(),
                        time: SystemTime::now(),
                        restored: false,
                        seen: false,
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
                zbus::message::Type::Signal => {
                    let interface = header.interface()?.as_str();
                    let member = header.member()?.as_str();
                    match (interface, member) {
                        ("org.freedesktop.Notifications", "NotificationClosed") => {
                            let (id, reason): (u32, u32) = message.body().deserialize().ok()?;
                            Some(Signal::Closed(id, reason))
                        }
                        ("org.freedesktop.Notifications", "ActionInvoked") => {
                            let (id, _): (u32, String) = message.body().deserialize().ok()?;
                            Some(Signal::Invoked(id))
                        }
                        ("org.freedesktop.DBus", "NameOwnerChanged") => {
                            // Calls waiting were for the daemon that's gone.
                            waiting.clear();
                            Some(Signal::Reset)
                        }
                        _ => None,
                    }
                }
                _ => None,
            }
        })();
        async move { signal }
    }))
}

/// The events of the notifications, as they happen. An error when the
/// watching can't be set up; the stream ends if the bus does.
pub async fn events() -> zbus::Result<impl futures::Stream<Item = Event>> {
    events_on(Builder::session()?).await
}

/// `events`, on the bus the builder is for.
async fn events_on(builder: Builder<'_>) -> zbus::Result<impl futures::Stream<Item = Event>> {
    let signals = bus_signals(builder).await?;
    let mut opened = HashSet::new();
    Ok(signals.filter_map(move |signal| {
        let event = event(&mut opened, signal);
        async move { event }
    }))
}

/// Kept from before icons came from the apps' entries, or a file since gone:
/// the icon and the body, as they'd be made now.
pub fn tidy(list: &mut [Notification]) {
    for notification in list {
        notification.icon = app_icon(&notification.app, None, &notification.icon, None);
        notification.body = plain(&notification.body);
    }
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
        Signal::Reset => {
            opened.clear();
            Event::Reset
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

    #[test]
    fn tidy_reads_markup_and_drops_gone_icons() {
        let mut list = vec![Notification {
            id: 1,
            app: String::new(),
            icon: "file:///nowhere/logo.png".into(),
            summary: "s".into(),
            body: "<b>hi</b> &amp; bye".into(),
            actions: Vec::new(),
            time: SystemTime::UNIX_EPOCH,
            restored: true,
            seen: true,
        }];
        tidy(&mut list);
        assert_eq!(list[0].icon, "");
        assert_eq!(list[0].body, "hi & bye");
    }

    #[test]
    fn a_new_daemon_forgets_the_clicks() {
        let mut opened = HashSet::new();
        assert!(event(&mut opened, Signal::Invoked(5)).is_none());
        assert!(matches!(event(&mut opened, Signal::Reset), Some(Event::Reset)));
        // The click was on the old daemon's id 5.
        assert!(matches!(event(&mut opened, Signal::Closed(5, 2)), Some(Event::Dismissed(5))));
    }

    #[test]
    fn long_text_is_cut_on_a_character() {
        assert_eq!(cap("abc".to_owned(), 5), "abc");
        assert_eq!(cap("abcdef".to_owned(), 3), "abc");
        assert_eq!(cap("ááááá".to_owned(), 2), "áá");
        assert_eq!(cap(String::new(), 0), "");
    }

    /// A stand-in for the daemon, on the test bus.
    struct Daemon;

    #[zbus::interface(name = "org.freedesktop.Notifications")]
    impl Daemon {
        #[allow(clippy::too_many_arguments)]
        fn notify(
            &self,
            _app: String,
            _replaces: u32,
            _icon: String,
            _summary: String,
            _body: String,
            _actions: Vec<String>,
            _hints: HashMap<String, zbus::zvariant::OwnedValue>,
            _timeout: i32,
        ) -> u32 {
            7
        }

        #[zbus(signal)]
        async fn notification_closed(
            emitter: &zbus::object_server::SignalEmitter<'_>,
            id: u32,
            reason: u32,
        ) -> zbus::Result<()>;
    }

    async fn next(events: &mut (impl futures::Stream<Item = Event> + Unpin)) -> Event {
        tokio::time::timeout(std::time::Duration::from_secs(5), events.next())
            .await
            .expect("no event")
            .expect("events ended")
    }

    async fn daemon(address: &str) -> zbus::Connection {
        Builder::address(address)
            .unwrap()
            .name("org.freedesktop.Notifications")
            .unwrap()
            .serve_at("/org/freedesktop/Notifications", Daemon)
            .unwrap()
            .build()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn the_bus_gives_a_notification_its_closing_and_a_new_daemon() {
        let Some(bus) = super::super::service::testbus::Bus::start() else {
            eprintln!("skipped: no dbus-daemon");
            return;
        };
        let first = daemon(bus.address()).await;
        let mut events = Box::pin(events_on(Builder::address(bus.address()).unwrap()).await.unwrap());
        let app = Builder::address(bus.address()).unwrap().build().await.unwrap();

        let id: u32 = app
            .call_method(
                Some("org.freedesktop.Notifications"),
                "/org/freedesktop/Notifications",
                Some("org.freedesktop.Notifications"),
                "Notify",
                &("app", 0u32, "", "hello", "<b>body</b>", Vec::<String>::new(), HashMap::<String, Value<'_>>::new(), -1i32),
            )
            .await
            .unwrap()
            .body()
            .deserialize()
            .unwrap();
        assert_eq!(id, 7);
        match next(&mut events).await {
            Event::New(n) => assert_eq!((n.id, n.summary.as_str(), n.body.as_str()), (7, "hello", "body")),
            other => panic!("{other:?}"),
        }

        first
            .emit_signal(None::<&str>, "/org/freedesktop/Notifications", "org.freedesktop.Notifications", "NotificationClosed", &(7u32, 3u32))
            .await
            .unwrap();
        assert!(matches!(next(&mut events).await, Event::Closed(7)));

        // The daemon is replaced: its name changes hands.
        drop(first);
        assert!(matches!(next(&mut events).await, Event::Reset));
        let _second = daemon(bus.address()).await;
        assert!(matches!(next(&mut events).await, Event::Reset));
    }
}
