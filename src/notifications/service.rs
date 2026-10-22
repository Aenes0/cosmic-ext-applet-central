// SPDX-License-Identifier: GPL-3.0-only

//! The notifications' own process (`--notifications`): the one that listens
//! to the bus, keeps the history on disk, and serves the applets over D-Bus.
//! Each applet reads the file when told `Changed`, and asks for its changes
//! here, so they all show the same history. No timer: it wakes for a
//! notification, a request, or the bus going away.

use std::sync::Arc;

use futures::StreamExt;
use zbus::object_server::{InterfaceRef, SignalEmitter};

use super::{NAME, PATH, Store, apply, watch};

/// What the applets talk to.
pub struct Service {
    store: Arc<Store>,
}

#[zbus::interface(name = "io.github.Aenes0.CosmicCentral.Notifications")]
impl Service {
    /// Drop one entry of the history, the live one or the restored one.
    async fn remove(
        &self,
        id: u32,
        restored: bool,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        if self.store.update(|list| super::remove(list, id, restored)).1 {
            Self::changed(&emitter).await?;
        }
        Ok(())
    }

    /// Empty the history. The applet closes the live ones on the daemon.
    async fn clear(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) -> zbus::fdo::Result<()> {
        if self.store.update(|list| _ = super::clear(list)).1 {
            Self::changed(&emitter).await?;
        }
        Ok(())
    }

    /// Everything in the history was shown: none counts as new.
    async fn seen(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) -> zbus::fdo::Result<()> {
        if self.store.update(|list| super::mark_seen(list)).1 {
            Self::changed(&emitter).await?;
        }
        Ok(())
    }

    /// The history on disk changed.
    #[zbus(signal)]
    async fn changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

/// Takes the name and serves the history. Fails with `NameTaken` when another
/// service already runs.
pub async fn serve(conn: zbus::connection::Builder<'_>, service: Service) -> zbus::Result<zbus::Connection> {
    conn.name(NAME)?
        .replace_existing_names(false)
        .serve_at(PATH, service)?
        .build()
        .await
}

/// The service: returns only to leave, with the reason.
pub fn main() -> ! {
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(err) => {
            tracing::error!("the notifications' runtime didn't start: {err}");
            std::process::exit(1);
        }
    };
    let reason = runtime.block_on(run());
    tracing::error!("{reason}");
    std::process::exit(1);
}

/// Serves and follows the bus till one of them ends: why it ended.
async fn run() -> String {
    let store = Arc::new(Store::user());
    let served = match zbus::connection::Builder::session() {
        Ok(builder) => serve(builder, Service { store: store.clone() }).await,
        Err(err) => Err(err),
    };
    let conn = match served {
        Ok(conn) => conn,
        Err(zbus::Error::NameTaken) => return format!("another notifications service already runs: {NAME} is taken"),
        Err(err) => return format!("the notifications couldn't serve on the session bus: {err}"),
    };
    let iface = match conn.object_server().interface::<_, Service>(PATH).await {
        Ok(iface) => iface,
        Err(err) => return format!("the notifications' D-Bus interface is missing: {err}"),
    };
    // Listening before the history is touched: nothing is missed meanwhile.
    let events = match watch::events().await {
        Ok(events) => events,
        Err(err) => return format!("cannot watch notifications on the bus: {err}"),
    };
    futures::pin_mut!(events);

    // What's on disk is from an earlier run: restored, its popups are gone.
    store.change(|list| {
        super::revive(list);
        watch::tidy(list);
    });
    announce(&iface).await;

    let mut gone = Box::pin(closed(&conn));
    loop {
        tokio::select! {
            event = events.next() => match event {
                Some(event) => {
                    // What changes nothing (the close of a passing pill) wakes nobody.
                    if store.update(|list| apply(list, event)).1 {
                        announce(&iface).await;
                    }
                }
                None => return "the notifications stream on the bus ended".to_owned(),
            },
            () = &mut gone => return "the session bus went away".to_owned(),
        }
    }
}

/// `Changed`, to every applet listening.
async fn announce(iface: &InterfaceRef<Service>) {
    if let Err(err) = Service::changed(iface.signal_emitter()).await {
        tracing::warn!("notifications: Changed not signalled: {err}");
    }
}

/// Till the connection's socket closes: its message stream ends then. Each
/// message coming in wakes this once; no timer.
async fn closed(conn: &zbus::Connection) {
    let mut messages = zbus::MessageStream::from(conn);
    while let Some(message) = messages.next().await {
        if let Err(err) = message {
            tracing::error!("notifications: the session bus connection broke: {err}");
            return;
        }
    }
}

/// A D-Bus of its own, for the tests: the session's isn't touched, and with no
/// service directories configured, nothing installed gets activated.
#[cfg(test)]
pub(super) mod testbus {
    use std::{
        io::{BufRead, BufReader},
        path::{Path, PathBuf},
        process::{Child, ChildStdout, Command, Stdio},
        sync::atomic::{AtomicUsize, Ordering},
    };

    const DAEMON: &str = "/usr/bin/dbus-daemon";

    /// No `<standard_session_servicedirs/>`: no service can be activated.
    const CONFIG: &str = "<busconfig><type>session</type><listen>unix:tmpdir=/tmp</listen><auth>EXTERNAL</auth>\
        <policy context=\"default\"><allow send_destination=\"*\" eavesdrop=\"true\"/>\
        <allow eavesdrop=\"true\"/><allow own=\"*\"/></policy></busconfig>";

    static COUNT: AtomicUsize = AtomicUsize::new(0);

    /// The bus, running while this lives.
    pub struct Bus {
        child: Child,
        address: String,
        config: PathBuf,
        // Held open: the daemon isn't written to a closed pipe.
        _stdout: BufReader<ChildStdout>,
    }

    impl Bus {
        /// None when there's no dbus-daemon: the test is skipped.
        pub fn start() -> Option<Bus> {
            if !Path::new(DAEMON).exists() {
                return None;
            }
            let config = std::env::temp_dir().join(format!(
                "central-notifications-bus-{}-{}.conf",
                std::process::id(),
                COUNT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::write(&config, CONFIG).expect("the bus's config wasn't written");
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
            Some(Bus { child, address, config, _stdout: stdout })
        }

        pub fn address(&self) -> &str {
            &self.address
        }
    }

    impl Drop for Bus {
        fn drop(&mut self) {
            _ = self.child.kill();
            _ = self.child.wait();
            _ = std::fs::remove_file(&self.config);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, time::Duration};

    use zbus::connection::Builder;

    use super::{super::{Action, HistoryProxy, Notification, request}, *};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("central-notifications-{name}-{}", std::process::id()));
        _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn note(id: u32, restored: bool) -> Notification {
        Notification {
            id,
            app: "app".to_owned(),
            icon: String::new(),
            summary: format!("summary {id}"),
            body: String::new(),
            actions: Vec::new(),
            time: std::time::SystemTime::UNIX_EPOCH,
            restored,
            seen: false,
        }
    }

    /// The next Changed signal, or a failure after a while.
    async fn next_change<S: futures::Stream + Unpin>(changes: &mut S) {
        tokio::time::timeout(Duration::from_secs(5), changes.next())
            .await
            .expect("no Changed signal")
            .expect("signal stream ended");
    }

    #[tokio::test]
    async fn methods_write_the_file_and_signal_changed() {
        let Some(bus) = testbus::Bus::start() else {
            eprintln!("skipped: no dbus-daemon");
            return;
        };
        let store = Arc::new(Store::at(scratch("methods")));
        store.change(|list| *list = vec![note(1, false), note(2, true), note(3, false)]);
        let _server = serve(Builder::address(bus.address()).unwrap(), Service { store: store.clone() })
            .await
            .unwrap();
        let client = Builder::address(bus.address()).unwrap().build().await.unwrap();
        let proxy = HistoryProxy::new(&client).await.unwrap();
        let mut changes = proxy.receive_changed().await.unwrap();

        proxy.seen().await.unwrap();
        assert!(store.load().iter().all(|n| n.seen));
        next_change(&mut changes).await;

        proxy.remove(2, true).await.unwrap();
        let ids: Vec<u32> = store.load().iter().map(|n| n.id).collect();
        assert_eq!(ids, [1, 3]);
        next_change(&mut changes).await;

        proxy.remove(1, false).await.unwrap();
        let ids: Vec<u32> = store.load().iter().map(|n| n.id).collect();
        assert_eq!(ids, [3]);
        next_change(&mut changes).await;

        proxy.clear().await.unwrap();
        assert!(store.load().is_empty());
        next_change(&mut changes).await;
    }

    #[tokio::test]
    async fn remove_keeps_the_other_kind_with_the_same_id() {
        let Some(bus) = testbus::Bus::start() else {
            eprintln!("skipped: no dbus-daemon");
            return;
        };
        let store = Arc::new(Store::at(scratch("samekey")));
        store.change(|list| *list = vec![note(7, false), note(7, true)]);
        let _server = serve(Builder::address(bus.address()).unwrap(), Service { store: store.clone() })
            .await
            .unwrap();
        let client = Builder::address(bus.address()).unwrap().build().await.unwrap();

        request(&client, Action::Remove(7, true)).await.unwrap();

        let left = store.load();
        assert_eq!(left.len(), 1);
        assert!(!left[0].restored);
    }

    #[tokio::test]
    async fn a_second_service_cannot_take_the_name() {
        let Some(bus) = testbus::Bus::start() else {
            eprintln!("skipped: no dbus-daemon");
            return;
        };
        let service = || Service { store: Arc::new(Store::at(scratch("second"))) };
        let _first = serve(Builder::address(bus.address()).unwrap(), service()).await.unwrap();

        let second = serve(Builder::address(bus.address()).unwrap(), service()).await;

        assert!(matches!(second, Err(zbus::Error::NameTaken)), "{second:?}");
    }

    #[tokio::test]
    async fn a_request_without_the_service_is_an_error() {
        let Some(bus) = testbus::Bus::start() else {
            eprintln!("skipped: no dbus-daemon");
            return;
        };
        let client = Builder::address(bus.address()).unwrap().build().await.unwrap();

        let asked = request(&client, Action::Seen).await;

        assert!(asked.is_err(), "{asked:?}");
    }
}
