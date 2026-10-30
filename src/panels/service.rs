// SPDX-License-Identifier: GPL-3.0-only

//! The host's one instance, on the session bus; and how the bar's applets
//! ask it, without waiting for it: a stuck host never holds the bar.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use cosmic::iced::{
    Subscription,
    futures::{self, SinkExt, channel::mpsc},
    stream,
};

use super::Message;
use super::place::{Request, Which};

pub const BUS_NAME: &str = "io.github.Aenes0.CosmicCentral.Panels";
pub const BUS_PATH: &str = "/io/github/Aenes0/CosmicCentral/Panels";

/// After starting the host, how long it is given to take its name before
/// another start is tried.
const STARTING: Duration = Duration::from_secs(3);
/// All an applet gives a request to reach a running host: past it, the
/// click is dropped (and logged).
const ASKING: Duration = Duration::from_secs(3);
/// All it gives the host's unit to come up on the bus, from cold.
const COMING_UP: Duration = Duration::from_secs(15);
/// The host's unit, which runs it with its limits (CPU, memory, watchdog).
pub const UNIT: &str = "central-panels.service";

/// Shows or hides a panel on the bar's screen, from an applet.
pub async fn toggle(which: Which, output: String) {
    ask(Request { which, output, page: None }).await;
}

/// Shows a panel on a page, from an applet.
pub async fn open(which: Which, output: String, page: String) {
    ask(Request { which, output, page: Some(page) }).await;
}

/// The bar's screen, as the panel tells its applets.
pub fn panel_output() -> String {
    std::env::var("COSMIC_PANEL_OUTPUT").unwrap_or_default()
}

/// Sent with no reply expected, so a stuck host never holds the applet (its
/// watchdog restarts it); all of it off the bar's UI, in its own task. When
/// the host isn't on the bus, its unit is started (not waited on) so it runs
/// within its limits, and the request goes once it's up, from cold too;
/// with no unit installed (a build run from its folder), the host is started
/// with the request by itself. A request that can't go is logged.
async fn ask(request: Request) {
    static BUS: tokio::sync::OnceCell<zbus::Connection> = tokio::sync::OnceCell::const_new();
    let result = async {
        let conn = BUS.get_or_try_init(zbus::Connection::session).await?;
        if !has_host(conn).await? {
            if !start_unit().await {
                static STARTED: Mutex<Option<Instant>> = Mutex::new(None);
                let mut started = STARTED.lock().unwrap_or_else(|e| e.into_inner());
                if started.is_none_or(|at| at.elapsed() > STARTING) {
                    *started = Some(Instant::now());
                    start_host(&request);
                }
                return Ok(());
            }
            if !coming_up(conn).await {
                tracing::warn!("{UNIT} didn't come up in {COMING_UP:?}: {request:?} dropped");
                return Ok(());
            }
        }
        match tokio::time::timeout(ASKING, conn.send(&message(&request)?)).await {
            Ok(sent) => sent,
            Err(_) => {
                tracing::warn!("the panels' host didn't take {request:?} in {ASKING:?}: dropped");
                Ok(())
            }
        }
    };
    if let Err(err) = result.await {
        tracing::warn!("the panels' host didn't take {request:?}: {err}");
    }
}

/// Waits for the host on the bus (the bus answers, not the host), up to
/// `COMING_UP`.
async fn coming_up(conn: &zbus::Connection) -> bool {
    let waiting = async {
        loop {
            if has_host(conn).await.unwrap_or(false) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    tokio::time::timeout(COMING_UP, waiting).await.is_ok()
}

/// The host's unit asked to start, not waited on (it is ready only once on
/// the bus); false if there is no such unit or systemd refused.
async fn start_unit() -> bool {
    let started = crate::host::command("systemctl", &[])
        .args(["--user", "start", "--no-block", UNIT])
        .output();
    let started = match tokio::time::timeout(ASKING, started).await {
        Ok(started) => started,
        Err(_) => {
            tracing::warn!("systemctl didn't answer in {ASKING:?}");
            return false;
        }
    };
    match started {
        Ok(output) if output.status.success() => true,
        Ok(output) => {
            tracing::debug!("{UNIT} not started: {}", String::from_utf8_lossy(&output.stderr).trim());
            false
        }
        Err(err) => {
            tracing::debug!("no systemctl: {err}");
            false
        }
    }
}

/// Whether the host is on the bus: the bus answers, not the host.
async fn has_host(conn: &zbus::Connection) -> zbus::Result<bool> {
    let name = zbus::names::BusName::try_from(BUS_NAME)?;
    Ok(zbus::fdo::DBusProxy::new(conn).await?.name_has_owner(name).await?)
}

fn message(request: &Request) -> zbus::Result<zbus::Message> {
    let call = |method: &'static str| -> zbus::Result<zbus::message::Builder<'static>> {
        zbus::Message::method_call(BUS_PATH, method)?
            .destination(BUS_NAME)?
            .interface(BUS_NAME)?
            .with_flags(zbus::message::Flags::NoReplyExpected)
    };
    let which = request.which.name();
    match &request.page {
        Some(page) => call("Open")?.build(&(which, request.output.as_str(), page.as_str())),
        None => call("Toggle")?.build(&(which, request.output.as_str())),
    }
}

/// The host, on its own, when it has no unit: not the applet's child to
/// wait for, nor in its process group, so the bar's restarts leave it be.
fn start_host(request: &Request) {
    use std::os::unix::process::CommandExt;
    let mut args = vec!["--panels".to_owned()];
    args.extend(request.to_args());
    if crate::host::in_flatpak() {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        crate::host::spawn("cosmic-ext-applet-central", &args);
        return;
    }
    let program = std::env::current_exe().unwrap_or_else(|_| "cosmic-ext-applet-central".into());
    let child = std::process::Command::new(program)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn();
    match child {
        // Reaped by a thread, not left a zombie.
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(err) => tracing::error!("cannot start the panels' host: {err}"),
    }
}

/// From the command line: hands the request to the running host and says
/// whether there is one, which this run then is not.
pub fn forward(request: Option<&Request>) -> bool {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
        return false;
    };
    runtime.block_on(async {
        let Ok(conn) = zbus::connection::Builder::session().map(|b| b.method_timeout(ASKING)) else {
            return false;
        };
        let Ok(conn) = conn.build().await else {
            return false;
        };
        if !has_host(&conn).await.unwrap_or(false) {
            return false;
        }
        if let Some(request) = request {
            let which = request.which.name();
            let output = request.output.as_str();
            let sent = match &request.page {
                Some(page) => {
                    conn.call_method(Some(BUS_NAME), BUS_PATH, Some(BUS_NAME), "Open", &(which, output, page.as_str()))
                        .await
                }
                None => conn.call_method(Some(BUS_NAME), BUS_PATH, Some(BUS_NAME), "Toggle", &(which, output)).await,
            };
            if let Err(err) = sent {
                tracing::warn!("the panels' host didn't take {request:?}: {err}");
            }
        }
        true
    })
}

struct Service(mpsc::Sender<Message>);

impl Service {
    async fn ask(&self, which: &str, output: String, page: Option<String>) -> zbus::fdo::Result<()> {
        let which = Which::parse(which)
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("no panel {which:?}: central or monitor")))?;
        _ = self.0.clone().send(Message::Request(Request { which, output, page })).await;
        Ok(())
    }
}

#[zbus::interface(name = "io.github.Aenes0.CosmicCentral.Panels")]
impl Service {
    /// Shows the panel (central or monitor) on the screen named, or hides
    /// it if it shows there; an empty name is the active screen.
    async fn toggle(&self, which: &str, output: String) -> zbus::fdo::Result<()> {
        self.ask(which, output, None).await
    }

    /// Shows the panel on a page (the Central's: sound, wifi…; the
    /// monitor's: edit, place).
    async fn open(&self, which: &str, output: String, page: String) -> zbus::fdo::Result<()> {
        self.ask(which, output, Some(page)).await
    }

    /// Hides whatever panel shows.
    async fn close(&self) {
        _ = self.0.clone().send(Message::Close).await;
    }
}

/// How a host holds the bus name: the one its unit runs takes it over from
/// any other; one started by hand gives it up to the unit's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NamePolicy {
    pub replace_existing: bool,
    pub allow_replacement: bool,
}

pub fn name_policy(under_unit: bool) -> NamePolicy {
    NamePolicy { replace_existing: under_unit, allow_replacement: !under_unit }
}

/// Whether systemd runs this process as a unit that waits for its READY.
pub fn under_unit() -> bool {
    std::env::var_os("NOTIFY_SOCKET").is_some()
}

/// The host on the bus, for as long as it runs. Another one already there
/// (and not to be replaced), or the name taken over by the unit's host,
/// this one quits.
pub(super) fn dbus_service() -> Subscription<Message> {
    struct Bus;
    Subscription::run_with(std::any::TypeId::of::<Bus>(), |_| {
        stream::channel(4, |mut output: mpsc::Sender<Message>| async move {
            use futures::StreamExt;
            use zbus::fdo::{RequestNameFlags, RequestNameReply};
            let policy = name_policy(under_unit());
            let serving = async {
                let conn = zbus::connection::Builder::session()?
                    .serve_at(BUS_PATH, Service(output.clone()))?
                    .build()
                    .await?;
                let flags = if policy.replace_existing {
                    RequestNameFlags::DoNotQueue | RequestNameFlags::ReplaceExisting
                } else {
                    RequestNameFlags::DoNotQueue | RequestNameFlags::AllowReplacement
                };
                let reply = conn.request_name_with_flags(BUS_NAME, flags).await?;
                let lost = zbus::fdo::DBusProxy::new(&conn).await?.receive_name_lost().await?;
                zbus::Result::Ok((conn, reply, lost))
            }
            .await;
            match serving {
                Ok((conn, RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner, mut lost)) => {
                    let _conn = conn;
                    _ = output.send(Message::Ready).await;
                    while let Some(signal) = lost.next().await {
                        if signal.args().is_ok_and(|args| args.name() == BUS_NAME) {
                            tracing::info!("the panels' unit took the bus over: leaving");
                            break;
                        }
                    }
                    _ = output.send(Message::Quit).await;
                }
                Ok((_, reply, _)) => {
                    tracing::info!("another host serves the panels ({reply:?}): leaving");
                    _ = output.send(Message::Quit).await;
                }
                Err(err) => {
                    tracing::error!("cannot serve the panels on D-Bus: {err}");
                    _ = output.send(Message::Quit).await;
                }
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_units_host_takes_the_name_over_and_one_by_hand_gives_it_up() {
        assert_eq!(name_policy(true), NamePolicy { replace_existing: true, allow_replacement: false });
        assert_eq!(name_policy(false), NamePolicy { replace_existing: false, allow_replacement: true });
    }
}
