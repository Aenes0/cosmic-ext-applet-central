// SPDX-License-Identifier: GPL-3.0-only

//! The clipboard's own process (`--clipboard`): the one client of the
//! compositor's data-control protocol. It writes down what's copied, keeps
//! all of the last copy's formats to offer them again once the app that
//! copied it is gone, and serves the applets over D-Bus.
//!
//! Two threads: the main one runs the Wayland loop; another the D-Bus side,
//! on a runtime of its own.

use std::{
    collections::HashMap,
    convert::Infallible,
    io::{ErrorKind, Write},
    os::{
        fd::{AsFd, AsRawFd, OwnedFd},
        unix::net::UnixStream,
    },
    path::PathBuf,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use cosmic::cctk::{
    sctk::reexports::{
        calloop::{self, channel},
        calloop_wayland_source::WaylandSource,
        protocols::ext::data_control::v1::client::{
            ext_data_control_device_v1::{self, ExtDataControlDeviceV1},
            ext_data_control_manager_v1::ExtDataControlManagerV1,
            ext_data_control_offer_v1::{self, ExtDataControlOfferV1},
            ext_data_control_source_v1::{self, ExtDataControlSourceV1},
        },
    },
    wayland_client::{
        self, Connection, Dispatch, Proxy, QueueHandle,
        globals::{GlobalListContents, registry_queue_init},
        protocol::{wl_registry, wl_seat::WlSeat},
    },
};
use futures::{StreamExt, channel::oneshot};
use tokio::sync::Notify;
use zbus::object_server::{InterfaceRef, SignalEmitter};

use super::{
    Store,
    keep::{self, Formats, HISTORY_MARK, Limits},
};

/// The service's name on the session bus.
pub const NAME: &str = "io.github.Aenes0.CosmicCentral.Clipboard";
/// Where it answers.
pub const PATH: &str = "/io/github/Aenes0/CosmicCentral/Clipboard";

/// What to do with something just put on the clipboard.
#[derive(Debug, PartialEq)]
pub enum Offer {
    /// Ours, from the history: nothing new.
    Ours,
    /// A password manager's: neither written down nor kept.
    Secret,
    /// Another app's: read in these formats, the primary one written down.
    Read { types: Vec<String>, primary: Option<String> },
}

/// The last copy's formats, to offer again when the clipboard empties.
#[derive(Default)]
pub struct Keeper {
    kept: Option<Formats>,
}

impl Keeper {
    pub fn offered(&mut self, types: &[String]) -> Offer {
        if keep::is_ours(types) {
            return Offer::Ours;
        }
        if keep::is_secret(types) {
            self.kept = None;
            return Offer::Secret;
        }
        let types = keep::readable(types);
        let primary = keep::primary(&types);
        Offer::Read { types, primary }
    }

    /// What was read of a copy; nothing read leaves the last one.
    pub fn received(&mut self, formats: Formats) {
        if !formats.is_empty() {
            self.kept = Some(formats);
        }
    }

    /// Copied again from the history.
    pub fn copied(&mut self, formats: Formats) {
        self.kept = Some(formats);
    }

    /// What to offer now that the clipboard is empty, marked ours.
    pub fn emptied(&self) -> Option<Formats> {
        let mut formats = self.kept.clone()?;
        formats.insert(HISTORY_MARK.to_string(), Arc::new(Vec::new()));
        Some(formats)
    }
}

/// The reads started, each numbered: only the newest one's result counts,
/// so a slow old copy doesn't land on a newer one.
#[derive(Default)]
struct Reads {
    newest: u64,
    pending: bool,
}

impl Reads {
    fn start(&mut self) -> u64 {
        self.newest += 1;
        self.pending = true;
        self.newest
    }

    /// Whether a result is the newest read's; an older one is dropped.
    fn finish(&mut self, read: u64) -> bool {
        let fresh = self.pending && read == self.newest;
        if fresh {
            self.pending = false;
        }
        fresh
    }

    fn pending(&self) -> bool {
        self.pending
    }

    /// Newer on the clipboard than what's being read: that's old already.
    fn supersede(&mut self) {
        self.newest += 1;
        self.pending = false;
    }
}

/// What a read back comes to.
struct Landed {
    /// What to write down: the copy was made, whichever read it is.
    record: Option<(String, Arc<Vec<u8>>)>,
    /// The newest read's: kept, and offered again if the clipboard emptied
    /// meanwhile. An older one isn't kept, so it never lands on a newer copy
    /// from the history nor on a password.
    fresh: bool,
}

fn land(reads: &mut Reads, keeper: &mut Keeper, read: Read) -> Landed {
    let fresh = reads.finish(read.read);
    let record = read.primary.and_then(|mime| {
        let bytes = read.formats.get(&mime)?.clone();
        Some((mime, bytes))
    });
    if fresh {
        keeper.received(read.formats);
    }
    Landed { record, fresh }
}

/// What a copy from the history puts on the clipboard: kept as the last
/// copy, and up to the top. None for an entry that's gone.
fn take(store: &Store, keeper: &mut Keeper, id: u64) -> Option<Formats> {
    let formats = store.get(id).as_ref().and_then(keep::entry_formats)?;
    keeper.copied(formats.clone());
    store.touch(id);
    Some(formats)
}

/// For the Wayland loop, from the D-Bus side.
pub enum Request {
    /// Put an entry on the clipboard; `done` once it's there.
    Copy { id: u64, done: oneshot::Sender<()> },
}

/// The history, over D-Bus.
pub struct Service {
    store: Arc<Store>,
    requests: channel::Sender<Request>,
}

#[zbus::interface(name = "io.github.Aenes0.CosmicCentral.Clipboard")]
impl Service {
    /// An unknown id (a stale panel) changes nothing.
    async fn copy(&self, id: u64, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) -> zbus::fdo::Result<()> {
        let (done, copied) = oneshot::channel();
        self.requests
            .send(Request::Copy { id, done })
            .map_err(|_| zbus::fdo::Error::Failed("the clipboard's Wayland loop is gone".into()))?;
        copied
            .await
            .map_err(|_| zbus::fdo::Error::Failed("the clipboard's Wayland loop is gone".into()))?;
        Self::changed(&emitter).await?;
        Ok(())
    }

    /// Pins an entry, or unpins it: the pinned stay through Clear and
    /// trimming. An unknown id changes nothing.
    async fn pin(&self, id: u64, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) -> zbus::fdo::Result<()> {
        self.store.pin(id);
        Self::changed(&emitter).await?;
        Ok(())
    }

    /// Removes an entry, pinned or not, with its picture. An unknown id
    /// changes nothing.
    async fn remove(&self, id: u64, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) -> zbus::fdo::Result<()> {
        self.store.remove(id);
        Self::changed(&emitter).await?;
        Ok(())
    }

    /// All but the pinned.
    async fn clear(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) -> zbus::fdo::Result<()> {
        self.store.clear();
        Self::changed(&emitter).await?;
        Ok(())
    }

    /// The history changed: read it again.
    #[zbus(signal)]
    async fn changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

/// Takes the name and serves the history. Fails with `NameTaken` when
/// another service has it.
pub async fn serve(conn: zbus::connection::Builder<'_>, service: Service) -> zbus::Result<zbus::Connection> {
    conn.name(NAME)?
        .replace_existing_names(false)
        .serve_at(PATH, service)?
        .build()
        .await
}

/// The process. A second one leaves without touching the history nor the
/// clipboard: the name is taken first.
pub fn main() -> ! {
    let store = Arc::new(Store::user());
    let (requests, requested) = channel::channel::<Request>();
    // A Notify, not a channel: what's written down while a Changed is being
    // sent comes to one more Changed, which is all the applets need.
    let changed = Arc::new(Notify::new());
    let changes = changed.clone();
    let (named, naming) = std::sync::mpsc::channel::<Result<(), String>>();
    let service = Service { store: store.clone(), requests };
    std::thread::spawn(move || bus(service, changes, named));
    match naming.recv() {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            tracing::error!("{err}");
            std::process::exit(1);
        }
        Err(_) => {
            tracing::error!("the clipboard's D-Bus thread stopped before taking its name");
            std::process::exit(1);
        }
    }
    store.tidy();
    match run(store, requested, changed) {
        Ok(never) => match never {},
        Err(err) => {
            tracing::error!("the clipboard lost the compositor: {err}");
            std::process::exit(1);
        }
    }
}

/// The D-Bus side: takes the name, tells how it went, then serves and
/// signals what the Wayland loop writes down.
fn bus(service: Service, changes: Arc<Notify>, named: std::sync::mpsc::Sender<Result<(), String>>) {
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(err) => {
            _ = named.send(Err(format!("the clipboard's D-Bus runtime didn't start: {err}")));
            return;
        }
    };
    runtime.block_on(async move {
        let served = match zbus::connection::Builder::session() {
            Ok(builder) => serve(builder, service).await,
            Err(err) => Err(err),
        };
        let conn = match served {
            Ok(conn) => conn,
            Err(zbus::Error::NameTaken) => {
                _ = named.send(Err(format!("another clipboard service already runs: {NAME} is taken")));
                return;
            }
            Err(err) => {
                _ = named.send(Err(format!("the clipboard couldn't serve on the session bus: {err}")));
                return;
            }
        };
        let iface = match conn.object_server().interface::<_, Service>(PATH).await {
            Ok(iface) => iface,
            Err(err) => {
                _ = named.send(Err(format!("the clipboard's D-Bus interface is missing: {err}")));
                return;
            }
        };
        // From here on, this side ending in any way (the bus gone, a panic)
        // ends the process.
        let _ends_all = EndsAll;
        _ = named.send(Ok(()));
        tokio::select! {
            never = signal(&iface, &changes) => match never {},
            () = closed(&conn) => tracing::error!("clipboard: the session bus went away"),
        }
    });
}

/// Dropped, by a return or a panic, once the name was ours: the process
/// leaves. Left running, the Wayland loop would go on writing down copies
/// while D-Bus activation starts a second service writing alongside it.
struct EndsAll;

impl Drop for EndsAll {
    fn drop(&mut self) {
        tracing::error!("clipboard: the D-Bus side stopped, and the service with it");
        std::process::exit(1);
    }
}

/// Changed, each time the Wayland loop writes something down.
async fn signal(iface: &InterfaceRef<Service>, changes: &Notify) -> Infallible {
    loop {
        changes.notified().await;
        if let Err(err) = Service::changed(iface.signal_emitter()).await {
            tracing::warn!("clipboard: Changed not signalled: {err}");
        }
    }
}

/// Till the connection's socket closes: its message stream ends then. Each
/// message coming in wakes this once; no timer.
async fn closed(conn: &zbus::Connection) {
    let mut messages = zbus::MessageStream::from(conn);
    while let Some(message) = messages.next().await {
        if let Err(err) = message {
            tracing::error!("clipboard: the session bus connection broke: {err}");
            return;
        }
    }
}

/// Writes all of `bytes` to an app asking for a format, giving up when it
/// reads nothing for `idle`: one that never reads nor closes doesn't hold
/// the thread and the bytes forever. Whether it all went.
fn send(fd: OwnedFd, bytes: &[u8], idle: Duration) -> bool {
    // SAFETY: fcntl on a file descriptor owned by `fd`, alive for the calls.
    let nonblocking = unsafe {
        let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFL);
        flags >= 0 && libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) >= 0
    };
    if !nonblocking {
        tracing::warn!("clipboard: can't make the pipe to send non-blocking, not sent");
        return false;
    }
    let timeout = i32::try_from(idle.as_millis()).unwrap_or(i32::MAX);
    let mut file = std::fs::File::from(fd);
    let mut rest = bytes;
    while !rest.is_empty() {
        match file.write(rest) {
            Ok(0) => return false,
            Ok(n) => rest = &rest[n..],
            Err(err) if err.kind() == ErrorKind::Interrupted => {}
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                let mut fds = [libc::pollfd { fd: file.as_raw_fd(), events: libc::POLLOUT, revents: 0 }];
                // SAFETY: `fds` is a valid array of one pollfd, alive for the call.
                let ready = unsafe { libc::poll(fds.as_mut_ptr(), 1, timeout) };
                if ready == 0 {
                    return false;
                }
                if ready < 0 && std::io::Error::last_os_error().kind() != ErrorKind::Interrupted {
                    return false;
                }
            }
            // The app closed its end, or the pipe broke.
            Err(_) => return false,
        }
    }
    true
}

/// What an offer is offered as.
#[derive(Default)]
struct OfferData {
    types: Mutex<Vec<String>>,
}

/// A read's result, back on the loop.
struct Read {
    read: u64,
    primary: Option<String>,
    formats: Formats,
}

struct State {
    manager: ExtDataControlManagerV1,
    device: Option<ExtDataControlDeviceV1>,
    /// What the clipboard has from us, while it's ours.
    sources: HashMap<wayland_client::backend::ObjectId, Formats>,
    conn: Connection,
    qh: QueueHandle<State>,
    keeper: Keeper,
    reads: Reads,
    /// The clipboard emptied while a copy was still read: offered again
    /// once the read is back.
    emptied: bool,
    store: Arc<Store>,
    changed: Arc<Notify>,
    results: channel::Sender<Read>,
}

/// The Wayland loop, till the compositor goes away.
fn run(
    store: Arc<Store>,
    requests: channel::Channel<Request>,
    changed: Arc<Notify>,
) -> Result<Infallible, Box<dyn std::error::Error>> {
    // The compositor's own socket: not a WAYLAND_SOCKET handed down.
    let display = std::env::var("WAYLAND_DISPLAY")?;
    let path = if display.starts_with('/') {
        PathBuf::from(display)
    } else {
        PathBuf::from(std::env::var("XDG_RUNTIME_DIR")?).join(display)
    };
    let conn = Connection::from_socket(UnixStream::connect(path)?)?;
    let (globals, event_queue) = registry_queue_init::<State>(&conn)?;
    let qh = event_queue.handle();
    let manager: ExtDataControlManagerV1 = globals.bind(&qh, 1..=1, ())?;
    let seat: WlSeat = globals.bind(&qh, 1..=9, ())?;

    let mut event_loop = calloop::EventLoop::<State>::try_new()?;
    let (results, read) = channel::channel::<Read>();
    let mut state = State {
        device: Some(manager.get_data_device(&seat, &qh, ())),
        manager,
        sources: HashMap::new(),
        conn: conn.clone(),
        qh,
        keeper: Keeper::default(),
        reads: Reads::default(),
        emptied: false,
        store,
        changed,
        results,
    };
    WaylandSource::new(conn, event_queue)
        .insert(event_loop.handle())
        .map_err(|err| err.error)?;
    event_loop
        .handle()
        .insert_source(read, |event, (), state: &mut State| {
            if let channel::Event::Msg(read) = event {
                state.read_back(read);
            }
        })
        .map_err(|err| err.error)?;
    event_loop
        .handle()
        .insert_source(requests, |event, (), state: &mut State| {
            if let channel::Event::Msg(Request::Copy { id, done }) = event {
                if let Some(formats) = take(&state.store, &mut state.keeper, id) {
                    state.reads.supersede();
                    state.offer(formats);
                }
                _ = done.send(());
            }
        })
        .map_err(|err| err.error)?;
    loop {
        event_loop.dispatch(None, &mut state)?;
    }
}

impl State {
    /// Puts these formats on the clipboard, as the copying app would.
    fn offer(&mut self, formats: Formats) {
        let source = self.manager.create_data_source(&self.qh, ());
        for mime in formats.keys() {
            source.offer(mime.clone());
        }
        self.sources.insert(source.id(), formats);
        if let Some(device) = &self.device {
            device.set_selection(Some(&source));
        }
        self.emptied = false;
        self.flush();
    }

    fn flush(&self) {
        if let Err(err) = self.conn.flush() {
            tracing::warn!("clipboard: the compositor's connection didn't flush: {err}");
        }
    }

    /// Reads all of an offer's formats at once, off the loop: the app
    /// sending them may take its time.
    fn read(&mut self, offer: &ExtDataControlOfferV1, types: Vec<String>, primary: Option<String>) {
        let mut pipes = Vec::with_capacity(types.len());
        for mime in types {
            let (reader, writer) = match std::io::pipe() {
                Ok(pipe) => pipe,
                Err(err) => {
                    tracing::warn!("clipboard: no pipe to read {mime}: {err}");
                    continue;
                }
            };
            offer.receive(mime.clone(), writer.as_fd());
            pipes.push((mime, reader));
        }
        if pipes.is_empty() {
            return;
        }
        self.flush();
        let read = self.reads.start();
        let results = self.results.clone();
        std::thread::spawn(move || {
            let formats = keep::read_all(pipes, primary.as_deref(), &Limits::default());
            _ = results.send(Read { read, primary, formats });
        });
    }

    /// A read back: written down, and kept only if it's the newest.
    fn read_back(&mut self, read: Read) {
        let landed = land(&mut self.reads, &mut self.keeper, read);
        // A big PNG's thumbnail is made here, on the loop: it holds the loop
        // for a few hundred ms. Accepted.
        if let Some((mime, bytes)) = landed.record
            && self.store.record(&mime, &bytes).is_some()
        {
            self.changed.notify_one();
        }
        if landed.fresh && self.emptied {
            self.offer_kept();
        }
    }

    /// The clipboard is empty: the last copy goes back on it.
    fn offer_kept(&mut self) {
        match self.keeper.emptied() {
            Some(formats) => self.offer(formats),
            None => self.emptied = false,
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(_: &mut Self, _: &WlSeat, _: <WlSeat as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ExtDataControlManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ExtDataControlManagerV1,
        _: <ExtDataControlManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtDataControlDeviceV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtDataControlDeviceV1,
        event: ext_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_device_v1::Event::Selection { id: Some(offer) } => {
                state.emptied = false;
                let types = offer
                    .data::<OfferData>()
                    .map(|data| data.types.lock().unwrap_or_else(PoisonError::into_inner).clone())
                    .unwrap_or_default();
                match state.keeper.offered(&types) {
                    Offer::Read { types, primary } if !types.is_empty() => state.read(&offer, types, primary),
                    Offer::Secret => state.reads.supersede(),
                    Offer::Read { .. } | Offer::Ours => {}
                }
                offer.destroy();
            }
            ext_data_control_device_v1::Event::Selection { id: None } => {
                // What's being read comes first: it's the newest copy.
                if state.reads.pending() {
                    state.emptied = true;
                } else {
                    state.offer_kept();
                }
            }
            ext_data_control_device_v1::Event::Finished => {
                tracing::error!("clipboard: the compositor took the seat's data device away");
                std::process::exit(1);
            }
            // The primary selection (middle click) isn't kept; its offer is
            // let go all the same, not to pile up.
            ext_data_control_device_v1::Event::PrimarySelection { id: Some(offer) } => offer.destroy(),
            _ => {}
        }
    }

    wayland_client::event_created_child!(State, ExtDataControlDeviceV1, [
        ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ExtDataControlOfferV1, OfferData::default()),
    ]);
}

impl Dispatch<ExtDataControlOfferV1, OfferData> for State {
    fn event(
        _: &mut Self,
        _: &ExtDataControlOfferV1,
        event: ext_data_control_offer_v1::Event,
        data: &OfferData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_data_control_offer_v1::Event::Offer { mime_type } = event {
            data.types.lock().unwrap_or_else(PoisonError::into_inner).push(mime_type);
        }
    }
}

impl Dispatch<ExtDataControlSourceV1, ()> for State {
    fn event(
        state: &mut Self,
        source: &ExtDataControlSourceV1,
        event: ext_data_control_source_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let id = source.id();
        match event {
            ext_data_control_source_v1::Event::Send { mime_type, fd } => {
                if let Some(bytes) = state.sources.get(&id).and_then(|formats| formats.get(&mime_type)).cloned() {
                    // Written off the loop: the other end reads at its pace.
                    std::thread::spawn(move || {
                        if !send(fd, &bytes, Limits::default().idle) {
                            tracing::debug!("clipboard: {mime_type} not sent whole");
                        }
                    });
                }
            }
            ext_data_control_source_v1::Event::Cancelled => {
                state.sources.remove(&id);
                source.destroy();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc, time::Duration};

    use futures::StreamExt;
    use zbus::connection::Builder;

    use super::*;
    use crate::clipboard::{
        Store,
        keep::{HISTORY_MARK, SECRET_HINT},
        testbus,
    };

    #[zbus::proxy(
        interface = "io.github.Aenes0.CosmicCentral.Clipboard",
        default_service = "io.github.Aenes0.CosmicCentral.Clipboard",
        default_path = "/io/github/Aenes0/CosmicCentral/Clipboard"
    )]
    trait Clipboard {
        fn copy(&self, id: u64) -> zbus::Result<()>;
        fn pin(&self, id: u64) -> zbus::Result<()>;
        fn remove(&self, id: u64) -> zbus::Result<()>;
        fn clear(&self) -> zbus::Result<()>;
        #[zbus(signal)]
        fn changed(&self) -> zbus::Result<()>;
    }

    fn t(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn formats(pairs: &[(&str, &[u8])]) -> Formats {
        pairs.iter().map(|(mime, bytes)| (mime.to_string(), Arc::new(bytes.to_vec()))).collect()
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("central-service-{name}-{}", std::process::id()));
        _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_secret_clears_what_was_kept() {
        let mut keeper = Keeper { kept: Some(formats(&[("text/plain", b"a")])) };

        assert_eq!(keeper.offered(&t(&["text/plain", SECRET_HINT])), Offer::Secret);

        assert_eq!(keeper.emptied(), None);
    }

    #[test]
    fn our_own_offer_changes_nothing() {
        let kept = formats(&[("text/plain", b"a")]);
        let mut keeper = Keeper { kept: Some(kept.clone()) };

        assert_eq!(keeper.offered(&t(&["text/plain", HISTORY_MARK])), Offer::Ours);

        assert_eq!(keeper.kept, Some(kept));
    }

    #[test]
    fn another_offer_is_read_in_its_readable_formats() {
        let mut keeper = Keeper::default();

        assert_eq!(
            keeper.offered(&t(&["TARGETS", "text/html", "UTF8_STRING"])),
            Offer::Read { types: t(&["text/html", "UTF8_STRING"]), primary: Some("UTF8_STRING".to_string()) }
        );
    }

    #[test]
    fn an_empty_read_keeps_the_last_copy() {
        let a = formats(&[("text/plain", b"a")]);
        let mut keeper = Keeper::default();
        keeper.received(a.clone());

        keeper.received(Formats::new());

        assert_eq!(keeper.kept, Some(a));
    }

    #[test]
    fn copy_again_is_what_comes_back() {
        let mut keeper = Keeper::default();
        keeper.received(formats(&[("text/plain", b"a"), ("text/html", b"<b>a</b>")]));

        keeper.copied(formats(&[("text/plain", b"b")]));

        let back = keeper.emptied().unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back["text/plain"].as_slice(), b"b");
        assert!(back.contains_key(HISTORY_MARK));
    }

    #[test]
    fn only_the_newest_read_counts() {
        let mut reads = Reads::default();
        let old = reads.start();
        let new = reads.start();

        assert!(!reads.finish(old));
        assert!(reads.pending());
        assert!(reads.finish(new));
        assert!(!reads.pending());

        let superseded = reads.start();
        reads.supersede();
        assert!(!reads.pending());
        assert!(!reads.finish(superseded));
    }

    fn read_of(read: u64, bytes: &[u8]) -> Read {
        Read { read, primary: Some("text/plain".to_string()), formats: formats(&[("text/plain", bytes)]) }
    }

    #[test]
    fn a_read_overtaken_by_a_copy_is_written_down_but_not_kept() {
        let mut reads = Reads::default();
        let mut keeper = Keeper::default();
        let old = reads.start();
        // A copy from the history while `old` is still read.
        keeper.copied(formats(&[("text/plain", b"from history")]));
        reads.supersede();

        let landed = land(&mut reads, &mut keeper, read_of(old, b"copied"));

        let (mime, bytes) = landed.record.expect("the copy is written down");
        assert_eq!(mime, "text/plain");
        assert_eq!(bytes.as_slice(), b"copied");
        assert!(!landed.fresh);
        assert_eq!(keeper.emptied().unwrap()["text/plain"].as_slice(), b"from history");
    }

    #[test]
    fn a_read_overtaken_by_a_password_is_not_kept_over_it() {
        let mut reads = Reads::default();
        let mut keeper = Keeper::default();
        let old = reads.start();
        assert_eq!(keeper.offered(&t(&["text/plain", SECRET_HINT])), Offer::Secret);
        reads.supersede();

        let landed = land(&mut reads, &mut keeper, read_of(old, b"before the password"));

        assert!(landed.record.is_some());
        assert!(!landed.fresh);
        assert_eq!(keeper.emptied(), None);
    }

    #[test]
    fn the_newest_read_is_written_down_and_kept() {
        let mut reads = Reads::default();
        let mut keeper = Keeper::default();
        let read = reads.start();

        let landed = land(&mut reads, &mut keeper, read_of(read, b"a"));

        assert!(landed.record.is_some());
        assert!(landed.fresh);
        assert_eq!(keeper.emptied().unwrap()["text/plain"].as_slice(), b"a");
    }

    #[test]
    fn a_reader_that_never_reads_is_given_up() {
        let (reader, writer) = std::io::pipe().unwrap();
        let bytes = vec![7u8; 1024 * 1024];
        let started = std::time::Instant::now();

        let sent = send(OwnedFd::from(writer), &bytes, Duration::from_millis(200));

        assert!(!sent);
        assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
        drop(reader);
    }

    #[test]
    fn a_reader_gets_all_of_it() {
        use std::io::Read as _;
        let (mut reader, writer) = std::io::pipe().unwrap();
        let bytes: Vec<u8> = (0..300_000u32).map(|i| i as u8).collect();
        let reading = std::thread::spawn(move || {
            let mut got = Vec::new();
            reader.read_to_end(&mut got).unwrap();
            got
        });

        assert!(send(OwnedFd::from(writer), &bytes, Duration::from_millis(200)));

        assert_eq!(reading.join().unwrap(), bytes);
    }

    #[tokio::test]
    async fn the_bus_going_away_is_noticed() {
        let Some(bus) = testbus::Bus::start() else {
            eprintln!("skipped: no dbus-daemon");
            return;
        };
        let service = Service {
            store: Arc::new(Store::at(scratch("gone"))),
            requests: calloop::channel::channel::<Request>().0,
        };
        let conn = serve(Builder::address(bus.address()).unwrap(), service).await.unwrap();

        drop(bus);

        tokio::time::timeout(Duration::from_secs(5), closed(&conn)).await.expect("the bus closing went unnoticed");
    }

    #[test]
    fn a_copy_from_the_history_is_kept_and_goes_to_the_top() {
        let store = Store::at(scratch("take"));
        store.record("text/plain", b"one").unwrap();
        store.record("text/plain", b"two").unwrap();
        let one = store.load()[1].id;
        let mut keeper = Keeper::default();

        let formats = take(&store, &mut keeper, one).unwrap();

        assert_eq!(formats["text/plain"].as_slice(), b"one");
        assert_eq!(keeper.kept, Some(formats));
        assert_eq!(store.load()[0].id, one);
        assert!(take(&store, &mut keeper, 12345).is_none());
    }

    /// The next Changed signal, or a failure after a while.
    async fn next_change(changes: &mut ChangedStream) {
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
        store.record("text/plain", b"one").unwrap();
        store.record("text/plain", b"two").unwrap();
        let (two, one) = (store.load()[0].id, store.load()[1].id);
        let (requests, channel) = calloop::channel::channel::<Request>();
        let _server = serve(Builder::address(bus.address()).unwrap(), Service { store: store.clone(), requests })
            .await
            .unwrap();
        let client = Builder::address(bus.address()).unwrap().build().await.unwrap();
        let proxy = ClipboardProxy::new(&client).await.unwrap();
        let mut changes = proxy.receive_changed().await.unwrap();

        proxy.pin(one).await.unwrap();
        proxy.remove(two).await.unwrap();

        let entries = store.load();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, one);
        assert!(entries[0].pinned);
        next_change(&mut changes).await;
        next_change(&mut changes).await;

        // A copy goes to the Wayland loop: a thread stands for it here.
        store.record("text/plain", b"three").unwrap();
        let looping = {
            let store = store.clone();
            std::thread::spawn(move || {
                let Ok(Request::Copy { id, done }) = channel.recv() else {
                    panic!("no copy requested");
                };
                let taken = take(&store, &mut Keeper::default(), id).is_some();
                _ = done.send(());
                taken
            })
        };
        proxy.copy(one).await.unwrap();

        assert!(looping.join().unwrap());
        assert_eq!(store.load()[0].id, one);
        next_change(&mut changes).await;
    }

    #[tokio::test]
    async fn a_second_service_cannot_take_the_name() {
        let Some(bus) = testbus::Bus::start() else {
            eprintln!("skipped: no dbus-daemon");
            return;
        };
        let service = || Service {
            store: Arc::new(Store::at(scratch("second"))),
            requests: calloop::channel::channel::<Request>().0,
        };
        let _first = serve(Builder::address(bus.address()).unwrap(), service()).await.unwrap();

        let second = serve(Builder::address(bus.address()).unwrap(), service()).await;

        assert!(matches!(second, Err(zbus::Error::NameTaken)), "{second:?}");
    }
}
