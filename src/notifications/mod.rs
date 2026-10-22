// SPDX-License-Identifier: GPL-3.0-only

//! Notifications, as apps send them to the daemon, and their history.
//!
//! One process (`--notifications`, see `service`) watches the session bus and
//! is the only one writing the history file. The applets read that file, on
//! start and at each `Changed` signal, and ask the service for changes by
//! D-Bus: the history is the same on every one of them.
//!
//! Actions run through the daemon's `InvokeAction` method on the session bus,
//! not through the socket the panel gives applets marked
//! `X-NotificationsApplet`: see `watch`.

pub mod service;
mod watch;

use std::{
    path::PathBuf,
    sync::Mutex,
    time::{Duration, SystemTime},
};

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
    /// Kept from before the service started: the daemon has forgotten it, and
    /// its id may be another's now.
    #[serde(default)]
    pub restored: bool,
    /// Shown in a popup already: not counted as new. The same on all applets.
    #[serde(default)]
    pub seen: bool,
}

/// Kept at most, newest first.
pub const KEEP: usize = 100;

#[derive(Clone, Debug)]
pub enum Event {
    New(Notification),
    /// Gone: opened, or closed by its app or from here.
    Closed(u32),
    /// Its popup closed by hand. It stays, as a heads-up swiped away on
    /// Android does, but the daemon has let go of it.
    Dismissed(u32),
    /// The daemon was replaced: its ids and actions are gone with it.
    Reset,
}

/// What an event does to the history.
pub fn apply(list: &mut Vec<Notification>, event: Event) {
    match event {
        Event::New(notification) => {
            // A notification replacing another keeps its id, and is new.
            list.retain(|n| n.restored || n.id != notification.id);
            list.insert(0, notification);
            list.truncate(KEEP);
        }
        Event::Closed(id) => list.retain(|n| n.restored || n.id != id),
        Event::Dismissed(id) => {
            if let Some(notification) = list.iter_mut().find(|n| !n.restored && n.id == id) {
                notification.restored = true;
                notification.actions.clear();
            }
        }
        Event::Reset => {
            for notification in list {
                notification.restored = true;
                notification.actions.clear();
            }
        }
    }
}

/// Take one out, by its id and whether it's a kept one.
pub fn remove(list: &mut Vec<Notification>, id: u32, restored: bool) {
    list.retain(|n| (n.id, n.restored) != (id, restored));
}

/// Take all out. The ids of those still alive on the daemon come back.
pub fn clear(list: &mut Vec<Notification>) -> Vec<u32> {
    list.drain(..).filter(|n| !n.restored).map(|n| n.id).collect()
}

/// All have been shown.
pub fn mark_seen(list: &mut [Notification]) {
    for notification in list {
        notification.seen = true;
    }
}

/// What the service does with the history it finds when it starts: the daemon
/// has forgotten those, so no actions, and nothing in them is new.
pub fn revive(list: &mut [Notification]) {
    for notification in list {
        notification.restored = true;
        notification.actions.clear();
        notification.seen = true;
    }
}

/// Held from a change's reading to its writing: the service's tasks don't
/// lose each other's changes.
static WRITING: Mutex<()> = Mutex::new(());

/// Where the history is kept: a directory with `notifications.json`.
pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// The user's own: in the state directory.
    pub fn user() -> Store {
        let state = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
            })
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        Store { dir: state.join(crate::APP_ID) }
    }

    #[cfg(test)]
    pub fn at(dir: PathBuf) -> Store {
        Store { dir }
    }

    fn file(&self) -> PathBuf {
        self.dir.join("notifications.json")
    }

    /// The file as it is, and what's wrong with it, if anything.
    fn read(&self) -> (Vec<Notification>, Problem) {
        let file = self.file();
        let data = match std::fs::read(&file) {
            Ok(data) => data,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return (Vec::new(), Problem::None);
            }
            Err(err) => {
                tracing::warn!("notification history {} not read: {err}", file.display());
                return (Vec::new(), Problem::Unreadable);
            }
        };
        match serde_json::from_slice(&data) {
            Ok(list) => (list, Problem::None),
            Err(err) => {
                tracing::warn!("notification history {} is not valid: {err}", file.display());
                (Vec::new(), Problem::Invalid)
            }
        }
    }

    /// The history now; empty when there's none or it's not valid.
    pub fn load(&self) -> Vec<Notification> {
        self.read().0
    }

    /// Written to a file beside, then moved in place: a reader sees the old
    /// history or the new one, never half of one.
    fn save(&self, list: &[Notification]) {
        let path = self.file();
        if let Err(err) = std::fs::create_dir_all(&self.dir) {
            tracing::warn!("notification history {} not made: {err}", self.dir.display());
            return;
        }
        let tmp = path.with_extension("json.tmp");
        let json = match serde_json::to_vec(list) {
            Ok(json) => json,
            Err(err) => {
                tracing::warn!("notification history not encoded: {err}");
                return;
            }
        };
        if let Err(err) = std::fs::write(&tmp, json) {
            tracing::warn!("notification history {} not written: {err}", tmp.display());
        } else if let Err(err) = std::fs::rename(&tmp, &path) {
            tracing::warn!("notification history {} not moved in place: {err}", path.display());
            _ = std::fs::remove_file(&tmp);
        }
    }

    /// Change what's on disk now, not what was read long ago, and write it
    /// down. The history comes back.
    pub fn change(&self, f: impl FnOnce(&mut Vec<Notification>)) -> Vec<Notification> {
        self.update(f).0
    }

    /// Like `change`, and whether the history is different now. Nothing is
    /// written when it isn't, nor when the file couldn't be read: that one
    /// stays as it is.
    pub fn update(&self, f: impl FnOnce(&mut Vec<Notification>)) -> (Vec<Notification>, bool) {
        // The file is moved in place at the end, so a panic elsewhere left it whole.
        let _writing = WRITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (mut list, problem) = self.read();
        match problem {
            Problem::None => {}
            Problem::Unreadable => return (list, false),
            Problem::Invalid => {
                let (file, bad) = (self.file(), self.file().with_extension("json.bad"));
                if let Err(err) = std::fs::rename(&file, &bad) {
                    tracing::warn!("notification history {} not put aside: {err}", file.display());
                    return (list, false);
                }
            }
        }
        let before = list.clone();
        f(&mut list);
        let changed = list != before || problem == Problem::Invalid;
        if changed {
            self.save(&list);
        }
        (list, changed)
    }
}

/// What keeps a history file from being used as it is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Problem {
    None,
    /// There, but not a history: put aside before it's written over.
    Invalid,
    /// There, and the system won't give it: left alone.
    Unreadable,
}

/// The notifications the service has kept.
pub fn load() -> Vec<Notification> {
    Store::user().load()
}

/// The service's name on the session bus, and where it serves.
pub const NAME: &str = "io.github.Aenes0.CosmicCentral.Notifications";
pub const PATH: &str = "/io/github/Aenes0/CosmicCentral/Notifications";

#[zbus::proxy(
    interface = "io.github.Aenes0.CosmicCentral.Notifications",
    default_service = "io.github.Aenes0.CosmicCentral.Notifications",
    default_path = "/io/github/Aenes0/CosmicCentral/Notifications"
)]
trait History {
    fn remove(&self, id: u32, restored: bool) -> zbus::Result<()>;
    fn clear(&self) -> zbus::Result<()>;
    fn seen(&self) -> zbus::Result<()>;
    #[zbus(signal)]
    fn changed(&self) -> zbus::Result<()>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Remove(u32, bool),
    Clear,
    Seen,
}

/// Ask the service, on this connection. An error when it isn't running.
pub async fn request(conn: &zbus::Connection, action: Action) -> zbus::Result<()> {
    let history = HistoryProxy::new(conn).await?;
    match action {
        Action::Remove(id, restored) => history.remove(id, restored).await,
        Action::Clear => history.clear().await,
        Action::Seen => history.seen().await,
    }
}

/// Ask the service on the session bus; a failure is only logged.
pub async fn ask(action: Action) {
    static BUS: tokio::sync::OnceCell<zbus::Connection> = tokio::sync::OnceCell::const_new();
    let result = async {
        // Long enough for D-Bus to start the service the first time.
        let session = || async {
            zbus::connection::Builder::session()?
                .method_timeout(Duration::from_secs(30))
                .build()
                .await
        };
        let conn = BUS.get_or_try_init(session).await?;
        request(conn, action).await
    };
    if let Err(err) = result.await {
        tracing::warn!("the notifications service didn't take {action:?}: {err}");
    }
}

const RECONNECT: Duration = Duration::from_secs(5);

/// One `()` per `Changed` signal, and one when it starts listening, for what
/// changed meanwhile. Waits and reconnects if the bus or the subscription fails.
pub fn subscription() -> cosmic::iced::Subscription<()> {
    use cosmic::iced::{Subscription, stream};
    use futures::{SinkExt, StreamExt, channel::mpsc};

    Subscription::run_with("notification-changes", |_| {
        stream::channel(4, |mut output: mpsc::Sender<()>| async move {
            loop {
                let listening = async {
                    let conn = zbus::Connection::session().await?;
                    HistoryProxy::new(&conn).await?.receive_changed().await
                };
                match listening.await {
                    Ok(mut changes) => {
                        if output.send(()).await.is_err() {
                            return;
                        }
                        while changes.next().await.is_some() {
                            if output.send(()).await.is_err() {
                                return;
                            }
                        }
                    }
                    Err(err) => tracing::warn!("cannot follow the notifications service: {err}"),
                }
                tokio::time::sleep(RECONNECT).await;
            }
        })
    })
}

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

    fn note(id: u32, restored: bool) -> Notification {
        Notification {
            id,
            app: "app".into(),
            icon: String::new(),
            summary: format!("summary {id}"),
            body: String::new(),
            actions: vec![("default".into(), "Open".into())],
            time: SystemTime::UNIX_EPOCH,
            restored,
            seen: false,
        }
    }

    fn ids(list: &[Notification]) -> Vec<(u32, bool)> {
        list.iter().map(|n| (n.id, n.restored)).collect()
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("central-notifications-{name}-{}", std::process::id()));
        _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn new_goes_first() {
        let mut list = vec![note(1, false)];
        apply(&mut list, Event::New(note(2, false)));
        assert_eq!(ids(&list), [(2, false), (1, false)]);
    }

    #[test]
    fn a_replaced_id_keeps_one_entry_and_is_new_again() {
        let mut old = note(1, false);
        old.seen = true;
        let mut list = vec![old, note(2, false)];
        let mut replacement = note(1, false);
        replacement.summary = "again".into();
        apply(&mut list, Event::New(replacement));
        assert_eq!(ids(&list), [(1, false), (2, false)]);
        assert_eq!(list[0].summary, "again");
        assert!(!list[0].seen);
    }

    #[test]
    fn a_restored_one_with_the_same_id_as_a_new_one_survives() {
        let mut list = vec![note(1, true)];
        apply(&mut list, Event::New(note(1, false)));
        assert_eq!(ids(&list), [(1, false), (1, true)]);
    }

    #[test]
    fn closed_never_removes_a_restored_one() {
        let mut list = vec![note(1, true), note(1, false), note(2, false)];
        apply(&mut list, Event::Closed(1));
        assert_eq!(ids(&list), [(1, true), (2, false)]);
        apply(&mut list, Event::Closed(1));
        assert_eq!(ids(&list), [(1, true), (2, false)]);
    }

    #[test]
    fn dismissed_keeps_it_as_restored_without_actions() {
        let mut list = vec![note(1, false), note(2, false)];
        apply(&mut list, Event::Dismissed(2));
        assert_eq!(ids(&list), [(1, false), (2, true)]);
        assert!(list[1].actions.is_empty());
        assert!(!list[0].actions.is_empty());
    }

    #[test]
    fn dismissed_leaves_a_restored_one_and_an_unknown_id_alone() {
        let mut list = vec![note(1, true), note(2, false)];
        let before = list.clone();
        apply(&mut list, Event::Dismissed(1));
        apply(&mut list, Event::Dismissed(99));
        assert_eq!(list, before);
    }

    #[test]
    fn dismissed_takes_the_live_one_not_the_restored_with_its_id() {
        let mut list = vec![note(1, false), note(1, true)];
        apply(&mut list, Event::Dismissed(1));
        assert_eq!(ids(&list), [(1, true), (1, true)]);
        assert!(list[0].actions.is_empty());
    }

    #[test]
    fn at_most_keep_newest_first() {
        let mut list = Vec::new();
        for id in 0..(KEEP as u32 + 5) {
            apply(&mut list, Event::New(note(id, false)));
        }
        assert_eq!(list.len(), KEEP);
        assert_eq!(list[0].id, KEEP as u32 + 4);
        assert_eq!(list[KEEP - 1].id, 5);
    }

    #[test]
    fn remove_takes_the_key_only() {
        let mut list = vec![note(1, false), note(1, true), note(2, true)];
        remove(&mut list, 1, true);
        assert_eq!(ids(&list), [(1, false), (2, true)]);
        remove(&mut list, 2, false);
        assert_eq!(ids(&list), [(1, false), (2, true)]);
        remove(&mut list, 9, false);
        assert_eq!(ids(&list), [(1, false), (2, true)]);
    }

    #[test]
    fn clear_empties_and_gives_the_live_ids() {
        let mut list = vec![note(3, false), note(2, true), note(1, false)];
        assert_eq!(clear(&mut list), [3, 1]);
        assert!(list.is_empty());
        assert!(clear(&mut list).is_empty());
    }

    #[test]
    fn mark_seen_marks_all() {
        let mut list = vec![note(1, false), note(2, true)];
        mark_seen(&mut list);
        assert!(list.iter().all(|n| n.seen));
    }

    #[test]
    fn revive_makes_all_kept_and_seen_without_actions() {
        let mut list = vec![note(1, false), note(2, true)];
        revive(&mut list);
        assert!(list.iter().all(|n| n.restored && n.seen && n.actions.is_empty()));
    }

    #[test]
    fn a_file_from_before_seen_still_reads() {
        let dir = scratch("old-format");
        let json = r#"[{"id":7,"app":"a","icon":"","summary":"s","body":"b","actions":[],"time":{"secs_since_epoch":0,"nanos_since_epoch":0}}]"#;
        std::fs::write(dir.join("notifications.json"), json).unwrap();
        let list = Store::at(dir).load();
        assert_eq!(list.len(), 1);
        assert!(!list[0].restored && !list[0].seen);
    }

    #[test]
    fn missing_file_is_an_empty_history() {
        let store = Store::at(scratch("missing"));
        assert!(store.load().is_empty());
    }

    #[test]
    fn change_writes_by_tmp_and_rename() {
        let dir = scratch("tmp-rename");
        let store = Store::at(dir.clone());
        let list = store.change(|l| l.push(note(1, false)));
        assert_eq!(list.len(), 1);
        assert!(dir.join("notifications.json").exists());
        assert!(!dir.join("notifications.json.tmp").exists());
        assert_eq!(store.load(), list);
    }

    #[test]
    fn change_makes_the_directory() {
        let dir = scratch("makes-dir").join("deeper");
        let store = Store::at(dir.clone());
        store.change(|l| l.push(note(1, false)));
        assert_eq!(store.load().len(), 1);
    }

    #[test]
    fn an_invalid_file_is_put_aside_not_written_over() {
        let dir = scratch("invalid");
        std::fs::write(dir.join("notifications.json"), b"{not json").unwrap();
        let store = Store::at(dir.clone());
        assert!(store.load().is_empty());
        // Reading alone doesn't touch it.
        assert!(!dir.join("notifications.json.bad").exists());
        let list = store.change(|l| l.push(note(1, false)));
        assert_eq!(list.len(), 1);
        assert_eq!(std::fs::read(dir.join("notifications.json.bad")).unwrap(), b"{not json");
        assert_eq!(store.load().len(), 1);
    }

    #[test]
    fn changes_from_two_threads_are_all_kept() {
        let store = std::sync::Arc::new(Store::at(scratch("race")));
        let threads: Vec<_> = (0..2u32)
            .map(|t| {
                let store = store.clone();
                std::thread::spawn(move || {
                    for i in 0..20 {
                        store.change(|l| l.push(note(t * 100 + i, false)));
                    }
                })
            })
            .collect();
        threads.into_iter().for_each(|t| t.join().unwrap());
        assert_eq!(store.load().len(), 40);
    }

    #[test]
    fn two_stores_on_one_directory_see_each_other() {
        let dir = scratch("two-stores");
        let (a, b) = (Store::at(dir.clone()), Store::at(dir));
        a.change(|l| l.push(note(1, false)));
        assert_eq!(b.load().len(), 1);
        let list = b.change(|l| l.push(note(2, false)));
        assert_eq!(list.len(), 2);
        assert_eq!(a.load(), list);
    }

    #[test]
    fn change_works_on_what_is_on_disk_now() {
        let dir = scratch("fresh");
        let (a, b) = (Store::at(dir.clone()), Store::at(dir));
        a.change(|l| l.push(note(1, false)));
        b.change(|l| remove(l, 1, false));
        // `a` never loaded again, and still doesn't bring the removed one back.
        a.change(|l| l.push(note(2, false)));
        assert_eq!(ids(&b.load()), [(2, false)]);
    }

    #[test]
    fn new_with_a_restored_entry_id_keeps_both_and_puts_new_on_top() {
        let restored = Notification {
            id: 1,
            app: "app".into(),
            icon: String::new(),
            summary: "restored".into(),
            body: String::new(),
            actions: Vec::new(),
            time: SystemTime::UNIX_EPOCH,
            restored: true,
            seen: true,
        };
        let fresh = Notification {
            id: 1,
            app: "app".into(),
            icon: String::new(),
            summary: "fresh".into(),
            body: String::new(),
            actions: vec![("default".into(), "Open".into())],
            time: SystemTime::UNIX_EPOCH,
            restored: false,
            seen: false,
        };
        let mut list = vec![restored.clone()];
        apply(&mut list, Event::New(fresh.clone()));
        assert_eq!(list, vec![fresh, restored]);
    }

    #[test]
    fn new_event_replacing_a_live_id_leaves_one_entry_at_front_unseen() {
        let old = Notification {
            id: 1,
            app: "app".into(),
            icon: String::new(),
            summary: "old".into(),
            body: String::new(),
            actions: vec![("default".into(), "Open".into())],
            time: SystemTime::UNIX_EPOCH,
            restored: false,
            seen: true,
        };
        let other = Notification {
            id: 2,
            app: "app".into(),
            icon: String::new(),
            summary: "other".into(),
            body: String::new(),
            actions: vec![("default".into(), "Open".into())],
            time: SystemTime::UNIX_EPOCH,
            restored: false,
            seen: false,
        };
        let replacement = Notification {
            id: 1,
            app: "app".into(),
            icon: String::new(),
            summary: "again".into(),
            body: String::new(),
            actions: vec![("default".into(), "Open".into())],
            time: SystemTime::UNIX_EPOCH,
            restored: false,
            seen: false,
        };
        let mut list = vec![old, other];

        apply(&mut list, Event::New(replacement.clone()));

        assert_eq!(list.iter().filter(|n| n.id == 1).count(), 1);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0], replacement);
        assert_eq!(list[0].id, 1);
        assert!(!list[0].restored);
        assert!(!list[0].seen);
        assert_eq!(list[1].id, 2);
    }

    #[test]
    fn closed_event_never_removes_a_restored_entry_with_the_same_id() {
        let restored = Notification {
            id: 7,
            app: "app".into(),
            icon: String::new(),
            summary: "kept from before".into(),
            body: String::new(),
            actions: Vec::new(),
            time: SystemTime::UNIX_EPOCH,
            restored: true,
            seen: true,
        };
        let live = Notification {
            id: 7,
            app: "app".into(),
            icon: String::new(),
            summary: "live".into(),
            body: String::new(),
            actions: vec![("default".into(), "Open".into())],
            time: SystemTime::UNIX_EPOCH,
            restored: false,
            seen: false,
        };
        let mut list = vec![restored.clone(), live];

        apply(&mut list, Event::Closed(7));

        assert_eq!(list, vec![restored]);
    }

    #[test]
    fn dismissed_of_unknown_id_changes_nothing_and_dismissed_of_live_id_keeps_it_restored_without_actions() {
        let live = |id: u32| Notification {
            id,
            app: "app".into(),
            icon: String::new(),
            summary: format!("summary {id}"),
            body: String::new(),
            actions: vec![("default".into(), "Open".into())],
            time: std::time::SystemTime::UNIX_EPOCH,
            restored: false,
            seen: false,
        };
        let mut list = vec![live(1), live(2)];
        let before = list.clone();

        // Unknown id: nothing changes.
        apply(&mut list, Event::Dismissed(99));
        assert_eq!(list, before);

        // Live id: kept, marked restored, actions cleared; the other one untouched.
        apply(&mut list, Event::Dismissed(2));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0], before[0]);
        assert_eq!(list[1].id, 2);
        assert_eq!(list[1].restored, true);
        assert_eq!(list[1].actions, Vec::<(String, String)>::new());
        assert_eq!(list[1].summary, before[1].summary);
    }

    #[test]
    fn applying_new_105_times_keeps_the_100_newest_first() {
        let note = |id: u32| Notification {
            id,
            app: "app".into(),
            icon: String::new(),
            summary: format!("summary {id}"),
            body: String::new(),
            actions: vec![("default".into(), "Open".into())],
            time: SystemTime::UNIX_EPOCH,
            restored: false,
            seen: false,
        };
        let mut list = Vec::new();
        for id in 0..105u32 {
            apply(&mut list, Event::New(note(id)));
        }
        assert_eq!(KEEP, 100);
        assert_eq!(list.len(), KEEP);
        let ids: Vec<u32> = list.iter().map(|n| n.id).collect();
        let expected: Vec<u32> = (5..105u32).rev().collect();
        assert_eq!(ids, expected);
        assert_eq!(list[0].id, 104);
        assert_eq!(list[KEEP - 1].id, 5);
    }

    #[test]
    fn clear_returns_only_live_ids_and_empties_the_list() {
        let mut list = vec![note(3, false), note(2, true), note(1, false)];
        let live = clear(&mut list);
        assert_eq!(live, [3, 1]);
        assert!(list.is_empty());
        assert!(clear(&mut list).is_empty());

        let mut restored_only = vec![note(5, true), note(4, true)];
        assert!(clear(&mut restored_only).is_empty());
        assert!(restored_only.is_empty());
    }

    #[test]
    fn remove_restored_takes_only_the_restored_one_when_a_live_one_shares_its_id() {
        let mut list = vec![note(1, false), note(1, true), note(2, true)];
        remove(&mut list, 1, true);
        assert_eq!(ids(&list), [(1, false), (2, true)]);
        assert_eq!(list[0].summary, "summary 1");
        assert!(!list[0].restored);
    }

    #[test]
    fn revive_restores_all_clears_actions_and_marks_seen_and_old_json_loads_unseen() {
        // revive: every kept entry becomes restored, loses its actions and is seen.
        let mut list = vec![
            Notification {
                id: 1,
                app: "app".into(),
                icon: String::new(),
                summary: "live".into(),
                body: String::new(),
                actions: vec![("default".into(), "Open".into())],
                time: SystemTime::UNIX_EPOCH,
                restored: false,
                seen: false,
            },
            Notification {
                id: 2,
                app: "app".into(),
                icon: String::new(),
                summary: "already restored".into(),
                body: String::new(),
                actions: vec![("default".into(), "Open".into())],
                time: SystemTime::UNIX_EPOCH,
                restored: true,
                seen: false,
            },
        ];
        revive(&mut list);
        assert_eq!(list.len(), 2);
        for n in &list {
            assert_eq!(n.restored, true);
            assert_eq!(n.seen, true);
            assert_eq!(n.actions, Vec::<(String, String)>::new());
        }
        assert_eq!(list[0].id, 1);
        assert_eq!(list[1].id, 2);

        // Old format: a JSON written before the `seen` field existed loads with seen = false.
        let dir = std::env::temp_dir().join(format!(
            "qs-notif-test-{}-old-json-seen-default",
            std::process::id()
        ));
        _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let json = r#"[{"id":7,"app":"a","icon":"","summary":"s","body":"b","actions":[],"time":{"secs_since_epoch":0,"nanos_since_epoch":0}}]"#;
        std::fs::write(dir.join("notifications.json"), json).unwrap();
        let loaded = Store::at(dir.clone()).load();
        _ = std::fs::remove_dir_all(&dir);

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, 7);
        assert_eq!(loaded[0].restored, false);
        assert_eq!(loaded[0].seen, false);
    }

    #[test]
    fn change_on_invalid_file_puts_it_aside_and_starts_empty() {
        let dir = std::env::temp_dir().join(format!(
            "qs-notif-test-{}-invalid-json-put-aside",
            std::process::id()
        ));
        _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let original: &[u8] = b"{not json";
        std::fs::write(dir.join("notifications.json"), original).unwrap();
        let store = Store::at(dir.clone());

        // The closure must see an empty history, not the invalid file's content.
        let mut seen_len = usize::MAX;
        let list = store.change(|l| {
            seen_len = l.len();
        });

        assert_eq!(seen_len, 0);
        assert_eq!(list.len(), 0);
        assert_eq!(
            std::fs::read(dir.join("notifications.json.bad")).unwrap(),
            original
        );
        assert!(!dir.join("notifications.json.tmp").exists());
        assert_eq!(std::fs::read(dir.join("notifications.json")).unwrap(), b"[]");
        assert_eq!(store.load().len(), 0);

        _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_change_through_one_store_is_what_the_other_loads() {
        let dir = std::env::temp_dir().join(format!(
            "qs-notif-test-{}-shared-history",
            std::process::id()
        ));
        _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let writer = Store::at(dir.clone());
        let reader = Store::at(dir.clone());
        let note = Notification {
            id: 7,
            app: "app".into(),
            icon: String::new(),
            summary: "summary 7".into(),
            body: String::new(),
            actions: vec![("default".into(), "Open".into())],
            time: SystemTime::UNIX_EPOCH,
            restored: false,
            seen: false,
        };

        let after_new = writer.change(|l| apply(l, Event::New(note.clone())));
        assert_eq!(after_new, vec![note.clone()]);
        assert_eq!(reader.load(), after_new);
        assert_eq!(reader.load(), writer.load());

        let after_remove = reader.change(|l| remove(l, 7, false));
        assert!(after_remove.is_empty());
        assert_eq!(writer.load(), after_remove);
        assert!(reader.load().is_empty());

        _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_after_mark_seen_is_the_only_unseen_one() {
        let mut list = vec![note(1, false)];
        mark_seen(&mut list);
        apply(&mut list, Event::New(note(2, false)));
        assert_eq!(ids(&list), [(2, false), (1, false)]);
        let seen: Vec<(u32, bool)> = list.iter().map(|n| (n.id, n.seen)).collect();
        assert_eq!(seen, [(2, false), (1, true)]);
        let unseen: Vec<u32> = list.iter().filter(|n| !n.seen).map(|n| n.id).collect();
        assert_eq!(unseen, [2]);
    }

    #[test]
    fn loading_a_missing_store_dir_is_empty_and_creates_nothing() {
        let dir = std::env::temp_dir().join(format!(
            "qs-notif-test-{}-missing-store-dir",
            std::process::id()
        ));
        _ = std::fs::remove_dir_all(&dir);
        assert!(!dir.exists());

        let store = Store::at(dir.clone());
        let list = store.load();

        assert_eq!(list, Vec::<Notification>::new());
        assert!(!dir.exists());
        assert!(!dir.join("notifications.json").exists());

        _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_cannot_be_read_is_left_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("io-error");
        let file = dir.join("notifications.json");
        let original = serde_json::to_vec(&vec![note(1, false)]).unwrap();
        std::fs::write(&file, &original).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&file).is_ok() {
            // Running as root: the mode doesn't stop the reading.
            return;
        }
        let store = Store::at(dir.clone());

        let (_, changed) = store.update(|l| l.push(note(2, false)));

        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!changed);
        assert_eq!(std::fs::read(&file).unwrap(), original);
    }

    #[test]
    fn a_change_that_changes_nothing_is_not_written() {
        let dir = scratch("no-op");
        let store = Store::at(dir.clone());
        store.update(|l| l.push(note(1, false)));
        let before = std::fs::metadata(dir.join("notifications.json")).unwrap().modified().unwrap();
        std::thread::sleep(Duration::from_millis(20));

        let (list, changed) = store.update(|l| remove(l, 99, false));

        assert!(!changed);
        assert_eq!(list.len(), 1);
        let after = std::fs::metadata(dir.join("notifications.json")).unwrap().modified().unwrap();
        assert_eq!(before, after);
        let (_, changed) = store.update(|l| remove(l, 1, false));
        assert!(changed);
    }

    #[test]
    fn a_new_daemon_leaves_the_live_ones_restored() {
        let mut list = vec![Notification { actions: vec![("a".into(), "A".into())], ..note(1, false) }, note(2, true)];
        apply(&mut list, Event::Reset);
        assert!(list.iter().all(|n| n.restored && n.actions.is_empty()));
        assert_eq!(list.len(), 2);
    }
}
