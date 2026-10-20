// SPDX-License-Identifier: GPL-3.0-only

//! The clipboard's history, as Windows' Win+V: what's copied, text or a
//! picture, kept to copy again, pinned or not.
//!
//! Each applet (one a panel, a screen) talks to the compositor itself, as a
//! client of its own outside the panel's sandbox, over the data-control
//! protocol; one of them, holding a lock, writes down what's copied, and all
//! read it from the same file.

use std::{
    collections::HashMap,
    io::{Read, Write},
    os::{
        fd::{AsFd, OwnedFd},
        unix::net::UnixStream,
    },
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

use cosmic::{
    cctk::{
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
    },
    iced::{
        Subscription,
        futures::{self, SinkExt, channel::mpsc},
        stream,
    },
};
use serde::{Deserialize, Serialize};

/// Kept at most, besides the pinned.
const KEEP: usize = 50;
/// Bigger than this isn't kept.
const LARGEST: usize = 16 * 1024 * 1024;

const TEXT_TYPES: [&str; 5] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "TEXT", "STRING"];
const IMAGE_TYPE: &str = "image/png";

/// Something copied.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: u64,
    /// The text; or none, for a picture.
    pub text: Option<String>,
    /// The picture, saved as a PNG file.
    pub image: Option<PathBuf>,
    pub pinned: bool,
    /// Seconds since 1970.
    pub time: u64,
}

#[derive(Clone, Debug)]
pub enum Event {
    Ready(channel::Sender<Request>),
    /// Newest first, pinned or not.
    History(Vec<Entry>),
}

/// For the compositor's connection: put this on the clipboard.
#[derive(Clone, Debug)]
pub enum Request {
    Copy(Entry),
}

fn dir() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    state.join(crate::APP_ID)
}

fn file() -> PathBuf {
    dir().join("clipboard.json")
}

pub fn load() -> Vec<Entry> {
    std::fs::read(file())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save(entries: &[Entry]) {
    _ = std::fs::create_dir_all(dir());
    let path = file();
    let tmp = path.with_extension("json.tmp");
    if let Ok(json) = serde_json::to_vec(entries)
        && std::fs::write(&tmp, json).is_ok()
    {
        _ = std::fs::rename(tmp, path);
    }
}

/// Changes the saved history, as it is on disk now: another screen's applet
/// may have changed it.
pub fn change(f: impl FnOnce(&mut Vec<Entry>)) -> Vec<Entry> {
    let mut entries = load();
    f(&mut entries);
    // The pinned kept, the rest the newest ones only; their pictures too.
    let mut unpinned = 0;
    entries.retain(|entry| {
        if entry.pinned {
            return true;
        }
        unpinned += 1;
        let keep = unpinned <= KEEP;
        if !keep && let Some(image) = &entry.image {
            _ = std::fs::remove_file(image);
        }
        keep
    });
    save(&entries);
    entries
}

/// Copied again from the history: up to the top.
pub fn touch(id: u64) -> Vec<Entry> {
    change(|entries| {
        if let Some(index) = entries.iter().position(|e| e.id == id) {
            let mut entry = entries.remove(index);
            entry.time = now();
            entries.insert(0, entry);
        }
    })
}

pub fn pin(id: u64) -> Vec<Entry> {
    change(|entries| {
        if let Some(entry) = entries.iter_mut().find(|e| e.id == id) {
            entry.pinned = !entry.pinned;
        }
    })
}

pub fn remove(id: u64) -> Vec<Entry> {
    change(|entries| {
        if let Some(entry) = entries.iter().find(|e| e.id == id)
            && let Some(image) = &entry.image
        {
            _ = std::fs::remove_file(image);
        }
        entries.retain(|e| e.id != id);
    })
}

/// All but the pinned.
pub fn clear() -> Vec<Entry> {
    change(|entries| {
        for entry in entries.iter().filter(|e| !e.pinned) {
            if let Some(image) = &entry.image {
                _ = std::fs::remove_file(image);
            }
        }
        entries.retain(|e| e.pinned);
    })
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Something just copied, at the top; the same again only moves up.
fn record(mime: &str, bytes: Vec<u8>) -> Vec<Entry> {
    change(|entries| {
        let mut entry = Entry {
            id: now() * 1000 + (entries.len() as u64 % 1000),
            text: None,
            image: None,
            pinned: false,
            time: now(),
        };
        if mime == IMAGE_TYPE {
            _ = std::fs::create_dir_all(dir().join("clipboard"));
            let path = dir().join("clipboard").join(format!("{}.png", entry.id));
            if std::fs::write(&path, &bytes).is_err() {
                return;
            }
            entry.image = Some(path);
        } else {
            let Ok(text) = String::from_utf8(bytes) else {
                return;
            };
            if text.trim().is_empty() {
                return;
            }
            // The same text again: up to the top, pinned as it was.
            if let Some(index) = entries.iter().position(|e| e.text.as_ref() == Some(&text)) {
                let mut same = entries.remove(index);
                same.time = now();
                entries.insert(0, same);
                return;
            }
            entry.text = Some(text);
        }
        entries.insert(0, entry);
    })
}

pub fn subscription() -> Subscription<Event> {
    Subscription::run_with(std::any::TypeId::of::<Event>(), |_| {
        stream::channel(8, |mut output: mpsc::Sender<Event>| async move {
            let (tx, rx) = channel::channel();
            _ = output.send(Event::Ready(tx)).await;
            _ = output.send(Event::History(load())).await;
            let watcher = output.clone();
            std::thread::spawn(move || {
                if let Err(err) = run(output, rx) {
                    tracing::warn!("clipboard history stopped: {err}");
                }
            });
            // What the applet writing it down saved, the others show.
            std::thread::spawn(move || watch(watcher));
            futures::future::pending::<()>().await;
        })
    })
}

/// Tells when the saved history changes, by its time.
fn watch(mut output: mpsc::Sender<Event>) {
    let mut seen = None;
    loop {
        let modified = std::fs::metadata(file()).and_then(|m| m.modified()).ok();
        if modified != seen {
            seen = modified;
            if futures::executor::block_on(output.send(Event::History(load()))).is_err() {
                return;
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Whether this applet writes the history down: the one holding the lock,
/// while it runs.
fn writer() -> Option<std::fs::File> {
    let path = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into())
        + "/central-clipboard.lock";
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .ok()?;
    file.try_lock().ok()?;
    Some(file)
}

/// What an offer is offered as.
#[derive(Default)]
struct OfferData {
    types: Mutex<Vec<String>>,
}

/// What the clipboard has from us now, while it's ours.
struct Source {
    mime_bytes: HashMap<String, Arc<Vec<u8>>>,
}

struct State {
    manager: ExtDataControlManagerV1,
    device: Option<ExtDataControlDeviceV1>,
    /// Whether this one writes the history down.
    writer: Option<std::fs::File>,
    received: channel::Sender<(String, Vec<u8>)>,
    sources: HashMap<wayland_client::backend::ObjectId, Source>,
    conn: Connection,
}

fn run(
    output: mpsc::Sender<Event>,
    requests: channel::Channel<Request>,
) -> Result<(), Box<dyn std::error::Error>> {
    // The compositor itself, as a client of its own: the panel's socket is
    // the applet's window's, and the data-control protocol isn't on it.
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
    let (received_tx, received_rx) = channel::channel::<(String, Vec<u8>)>();
    let mut state = State {
        device: Some(manager.get_data_device(&seat, &qh, ())),
        manager,
        writer: writer(),
        received: received_tx,
        sources: HashMap::new(),
        conn: conn.clone(),
    };
    WaylandSource::new(conn, event_queue)
        .insert(event_loop.handle())
        .map_err(|err| err.error)?;
    let mut history = output.clone();
    event_loop
        .handle()
        .insert_source(received_rx, move |event, (), _state| {
            if let channel::Event::Msg((mime, bytes)) = event {
                let entries = record(&mime, bytes);
                _ = futures::executor::block_on(history.send(Event::History(entries)));
            }
        })
        .map_err(|err| err.error)?;
    let qh2 = qh.clone();
    event_loop
        .handle()
        .insert_source(requests, move |event, (), state: &mut State| {
            if let channel::Event::Msg(Request::Copy(entry)) = event {
                state.copy(&entry, &qh2);
            }
        })
        .map_err(|err| err.error)?;
    loop {
        // Another screen's applet that wrote it down may be gone: this one
        // takes over.
        if state.writer.is_none() {
            state.writer = writer();
        }
        event_loop.dispatch(Duration::from_secs(30), &mut state)?;
    }
}

impl State {
    /// Puts an entry back on the clipboard, as the copying app would.
    fn copy(&mut self, entry: &Entry, qh: &QueueHandle<State>) {
        let mut mime_bytes: HashMap<String, Arc<Vec<u8>>> = HashMap::new();
        if let Some(text) = &entry.text {
            let bytes = Arc::new(text.clone().into_bytes());
            for mime in TEXT_TYPES {
                mime_bytes.insert(mime.to_owned(), bytes.clone());
            }
        } else if let Some(image) = &entry.image {
            let Ok(bytes) = std::fs::read(image) else {
                return;
            };
            mime_bytes.insert(IMAGE_TYPE.to_owned(), Arc::new(bytes));
        }
        if mime_bytes.is_empty() {
            return;
        }
        // Marked ours, not to be written down again as new.
        mime_bytes.insert("x-central-history".to_owned(), Arc::new(Vec::new()));
        let source = self.manager.create_data_source(qh, ());
        for mime in mime_bytes.keys() {
            source.offer(mime.clone());
        }
        self.sources.insert(
            source.id(),
            Source { mime_bytes },
        );
        if let Some(device) = &self.device {
            device.set_selection(Some(&source));
        }
        _ = self.conn.flush();
    }
}

/// Reads what an offer sends through a pipe, off the event loop: the app
/// sending it may take its time.
fn receive(offer: &ExtDataControlOfferV1, mime: String, conn: &Connection, to: channel::Sender<(String, Vec<u8>)>) {
    let Ok((mut reader, writer)) = std::io::pipe() else {
        return;
    };
    offer.receive(mime.clone(), writer.as_fd());
    drop(writer);
    _ = conn.flush();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 64 * 1024];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    bytes.extend_from_slice(&chunk[..n]);
                    if bytes.len() > LARGEST {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
        if !bytes.is_empty() {
            _ = to.send((mime, bytes));
        }
    });
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
        conn: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_device_v1::Event::Selection { id: Some(offer) } => {
                // Only the applet writing it down reads it.
                if state.writer.is_none() {
                    offer.destroy();
                    return;
                }
                let types = offer
                    .data::<OfferData>()
                    .map(|data| data.types.lock().unwrap().clone())
                    .unwrap_or_default();
                // Copied by us, back from the history: nothing new.
                let ours = types.iter().any(|t| t == "x-central-history");
                let mime = TEXT_TYPES
                    .iter()
                    .find(|t| types.iter().any(|offered| offered == *t))
                    .map(|t| (*t).to_owned())
                    .or_else(|| types.iter().find(|t| *t == IMAGE_TYPE).cloned());
                if let (Some(mime), false) = (mime, ours) {
                    receive(&offer, mime, conn, state.received.clone());
                }
                offer.destroy();
            }
            ext_data_control_device_v1::Event::Finished => {
                state.device = None;
            }
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
            data.types.lock().unwrap().push(mime_type);
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
                if let Some(bytes) = state
                    .sources
                    .get(&id)
                    .and_then(|s| s.mime_bytes.get(&mime_type))
                    .cloned()
                {
                    // Written off the loop: the other end reads at its pace.
                    std::thread::spawn(move || {
                        let mut file = std::fs::File::from(OwnedFd::from(fd));
                        _ = file.write_all(&bytes);
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

use cosmic::{
    Element,
    iced::{Alignment, Length},
    theme,
    widget::{self, button},
};

use crate::{
    app::{App, Message},
    layout::Section,
    pages::dimmed,
    texts::texts,
    tiles::{chip_class, icon},
};

impl App {
    /// What was copied, its own panel: newest first, a click copies it again;
    /// each can be pinned, which keeps it, or deleted.
    pub fn clipboard_panel(&self) -> Element<'_, Message> {
        let t = texts();
        let spacing = theme::spacing();
        let section = Section::Clipboard;
        let folded = self.layout.is_folded(section);
        let mut header = widget::row::with_capacity(3)
            .push(
                button::custom(
                    widget::row::with_children(vec![
                        widget::icon::icon(icon(section.icon())).size(16).into(),
                        widget::text::heading(format!("{} ({})", t.clipboard, self.clipboard.len()))
                            .width(Length::Fill)
                            .into(),
                        widget::icon::icon(icon(if folded { "go-down-symbolic" } else { "go-up-symbolic" }))
                            .size(16)
                            .into(),
                    ])
                    .spacing(spacing.space_xs)
                    .align_y(Alignment::Center),
                )
                .padding([6, spacing.space_xxs])
                .width(Length::Fill)
                .class(theme::Button::Text)
                .on_press(Message::ToggleFold(section)),
            )
            .spacing(spacing.space_xs)
            .align_y(Alignment::Center);
        if !folded && self.clipboard.iter().any(|e| !e.pinned) {
            header = header.push(
                button::custom(
                    widget::row::with_children(vec![
                        widget::icon::icon(icon("edit-clear-all-symbolic")).size(16).into(),
                        widget::text::caption(t.clear).into(),
                    ])
                    .spacing(spacing.space_xxs)
                    .align_y(Alignment::Center),
                )
                .padding([6, 12])
                .class(chip_class(false))
                .on_press(Message::ClearClipboard),
            );
        }
        let mut column = widget::column::with_capacity(2).push(header).spacing(spacing.space_xs);
        if !folded {
            if self.clipboard.is_empty() {
                column = column.push(
                    widget::text::body(t.clipboard_empty)
                        .class(theme::Text::Custom(dimmed))
                        .width(Length::Fill),
                );
            } else {
                let cards: Vec<Element<'_, Message>> =
                    self.clipboard.iter().map(|entry| self.clipboard_card(entry)).collect();
                column = column.push(
                    widget::scrollable(widget::column::with_children(cards).spacing(spacing.space_xxs))
                        .height(Length::Shrink)
                        .width(Length::Fill),
                );
            }
        }
        column.padding(spacing.space_s).into()
    }

    fn clipboard_card<'a>(&'a self, entry: &'a Entry) -> Element<'a, Message> {
        let t = texts();
        let spacing = theme::spacing();
        let content: Element<'_, Message> = match (&entry.text, &entry.image) {
            (Some(text), _) => widget::text::body(text.trim().replace('\n', " ⏎ "))
                .ellipsize(cosmic::iced::core::text::Ellipsize::End(
                    cosmic::iced::core::text::EllipsizeHeightLimit::Lines(2),
                ))
                .width(Length::Fill)
                .into(),
            (None, Some(path)) => widget::row::with_children(vec![
                widget::image(widget::image::Handle::from_path(path))
                    .height(Length::Fixed(48.0))
                    .content_fit(cosmic::iced::ContentFit::Contain)
                    .border_radius(6.0)
                    .into(),
                widget::text::caption(t.image).class(theme::Text::Custom(dimmed)).into(),
            ])
            .spacing(spacing.space_xs)
            .align_y(Alignment::Center)
            .width(Length::Fill)
            .into(),
            (None, None) => widget::text::body("").into(),
        };
        let small = |name: &str, tip: &'static str, message: Message, lit: bool| -> Element<'_, Message> {
            widget::tooltip(
                button::icon(icon(name))
                    .icon_size(14)
                    .padding(6)
                    .class(if lit { chip_class(true) } else { theme::Button::Text })
                    .on_press(message),
                widget::text::body(tip),
                widget::tooltip::Position::Top,
            )
            .into()
        };
        let row = widget::row::with_children(vec![
            content,
            small(
                "view-pin-symbolic",
                if entry.pinned { t.unpin } else { t.pin },
                Message::PinEntry(entry.id),
                entry.pinned,
            ),
            small("user-trash-symbolic", t.delete, Message::DeleteEntry(entry.id), false),
        ])
        .spacing(spacing.space_xxs)
        .align_y(Alignment::Center);
        button::custom(row)
            .padding([8, 10])
            .width(Length::Fill)
            .class(crate::tiles::edit_class(false))
            .on_press(Message::CopyEntry(entry.id))
            .into()
    }
}
