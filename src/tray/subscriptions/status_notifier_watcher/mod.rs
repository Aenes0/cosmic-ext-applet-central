// Copyright 2023 System76 <info@system76.com>, 2026 Aenes0
// SPDX-License-Identifier: GPL-3.0-only

//! Client side of the system's status notifier watcher. The watcher itself is
//! the system's (`com.system76.CosmicStatusNotifierWatcher`, a user unit): this
//! applet never serves it.

use std::any::TypeId;

use cosmic::iced::{self, Subscription};
use futures::{StreamExt, stream};

use crate::tray::subscriptions::status_notifier_item::StatusNotifierItem;

mod client;

#[derive(Clone, Debug)]
pub enum Event {
    Connected(zbus::Connection),
    Registered(StatusNotifierItem),
    Unregistered(String),
    Error(String), // XXX
}

enum State {
    NotConnected,
    Connected(client::EventStream),
    Failed,
}

pub fn subscription() -> iced::Subscription<Event> {
    pub struct MyID;
    Subscription::run_with(TypeId::of::<MyID>(), |_| {
        stream::unfold(State::NotConnected, |state| async move {
            match state {
                State::NotConnected => match connect().await {
                    Ok((connection, stream)) => {
                        Some((Event::Connected(connection), State::Connected(stream)))
                    }
                    Err(err) => Some((Event::Error(err.to_string()), State::Failed)),
                },
                State::Connected(mut stream) => stream
                    .next()
                    .await
                    .map(|event| (event, State::Connected(stream))),
                State::Failed => None,
            }
        })
    })
}

async fn connect() -> zbus::Result<(zbus::Connection, client::EventStream)> {
    // Connect to session dbus socket
    let connection = zbus::Connection::session().await?;

    // Tell the system's watcher this applet is alive: it drops applets that
    // left the bus and exits when none is left.
    if let Err(err) = cosmic_register(&connection).await {
        tracing::error!("Failed to register with the status notifier watcher: {err}");
    }

    // Connect client and listen for registered/unregistered
    let stream = client::watch(&connection).await?;

    Ok((connection, stream))
}

/// Register with the daemon, once now and again whenever its owner changes.
pub async fn cosmic_register(conn: &zbus::Connection) -> zbus::Result<()> {
    let cosmic_watcher = CosmicAppletStatusNotifierWatcherProxy::new(conn).await?;
    cosmic_watcher.register_applet().await?;
    let mut stream = cosmic_watcher.inner().receive_owner_changed().await?;
    tokio::spawn(async move {
        while let Some(value) = stream.next().await {
            if let Some(_unique_name) = value {
                // Register with new owner
                let _ = cosmic_watcher.register_applet().await;
            }
        }
    });
    Ok(())
}

#[zbus::proxy(
    interface = "com.system76.CosmicStatusNotifierWatcher",
    default_service = "com.system76.CosmicStatusNotifierWatcher",
    default_path = "/CosmicStatusNotifierWatcher"
)]
trait CosmicAppletStatusNotifierWatcher {
    async fn register_applet(&self) -> zbus::Result<()>;
}
