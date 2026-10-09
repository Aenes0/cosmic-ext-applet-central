// SPDX-License-Identifier: GPL-3.0-only

//! Notifications, as cosmic-notifications hands them to panel applets.
//!
//! The panel gives applets marked `X-NotificationsApplet` a socket to the
//! daemon in `COSMIC_NOTIFICATIONS`; new notifications come through it. They
//! are dismissed through the usual `org.freedesktop.Notifications`.

use std::{
    collections::HashMap,
    os::unix::io::{FromRawFd, RawFd},
    time::SystemTime,
};

use cosmic::iced::{Subscription, stream};
use futures::{SinkExt, StreamExt, channel::mpsc};
use tokio::sync::OnceCell;
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
    Closed(u32),
}

#[zbus::proxy(
    default_service = "com.system76.NotificationsApplet",
    interface = "com.system76.NotificationsApplet",
    default_path = "/com/system76/NotificationsApplet"
)]
trait NotificationsApplet {
    #[zbus(signal)]
    fn notify(
        &self,
        app_name: &str,
        id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: Vec<&str>,
        hints: HashMap<&str, Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<()>;

    fn invoke_action(&self, id: u32, action: String) -> zbus::Result<()>;
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
}

/// The connection to the daemon, if the panel gave us one.
async fn daemon() -> Option<NotificationsAppletProxy<'static>> {
    static DAEMON: OnceCell<Option<NotificationsAppletProxy<'static>>> = OnceCell::const_new();
    DAEMON
        .get_or_init(|| async {
            let fd: RawFd = std::env::var("COSMIC_NOTIFICATIONS").ok()?.parse().ok()?;
            let stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd) };
            stream.set_nonblocking(true).ok()?;
            let stream = tokio::net::UnixStream::from_std(stream).ok()?;
            let conn = Builder::socket(stream).p2p().build().await.ok()?;
            NotificationsAppletProxy::new(&conn).await.ok()
        })
        .await
        .clone()
}

pub fn subscription() -> Subscription<Event> {
    Subscription::run_with("notifications", |_| {
        stream::channel(50, |mut output: mpsc::Sender<Event>| async move {
            let Some(daemon) = daemon().await else {
                tracing::info!("no notifications socket from the panel");
                return futures::future::pending().await;
            };
            let Ok(session) = zbus::Connection::session().await else {
                return futures::future::pending().await;
            };
            let Ok(freedesktop) = NotificationsProxy::new(&session).await else {
                return futures::future::pending().await;
            };
            let (Ok(notify), Ok(closed)) = (
                daemon.receive_notify().await,
                freedesktop.receive_notification_closed().await,
            ) else {
                return futures::future::pending().await;
            };
            let notify = notify.filter_map(|signal| async move {
                let args = signal.args().ok()?;
                let actions = args
                    .actions
                    .chunks(2)
                    .filter_map(|pair| match pair {
                        [key, label] => Some((key.to_string(), label.to_string())),
                        _ => None,
                    })
                    .collect();
                Some(Event::New(Notification {
                    id: args.id,
                    app: args.app_name.to_owned(),
                    icon: args.app_icon.to_owned(),
                    summary: args.summary.to_owned(),
                    body: args.body.to_owned(),
                    actions,
                    time: SystemTime::now(),
                    restored: false,
                }))
            });
            let closed = closed.filter_map(|signal| async move {
                Some(Event::Closed(signal.args().ok()?.id))
            });
            let mut events = futures::stream::select(notify.boxed(), closed.boxed());
            while let Some(event) = events.next().await {
                if output.send(event).await.is_err() {
                    return;
                }
            }
            futures::future::pending().await
        })
    })
}

/// Run an action, `default` being a click on the notification.
pub async fn invoke(id: u32, action: String) {
    if let Some(daemon) = daemon().await {
        if let Err(err) = daemon.invoke_action(id, action).await {
            tracing::warn!("cannot run notification action: {err}");
        }
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
