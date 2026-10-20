// SPDX-License-Identifier: GPL-3.0-only

//! Windows' "show desktop": a thin button at the bar's very end; a click
//! minimizes every window, the next brings back those, the one in front
//! last. Its own applet, by the same program, put after the clock.

use std::os::{
    fd::{FromRawFd, RawFd},
    unix::net::UnixStream,
};

use cosmic::{
    Element, Task, app,
    cctk::{
        self,
        cosmic_protocols::toplevel_info::v1::client::zcosmic_toplevel_handle_v1,
        sctk::{
            self,
            reexports::{calloop, calloop_wayland_source::WaylandSource},
            registry::{ProvidesRegistryState, RegistryState},
        },
        toplevel_info::{ToplevelInfoHandler, ToplevelInfoState},
        toplevel_management::{ToplevelManagerHandler, ToplevelManagerState},
        wayland_client::{Connection, QueueHandle, WEnum, globals::registry_queue_init},
        wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
    },
    iced::{
        self, Length, Subscription,
        futures::{self, SinkExt, channel::mpsc},
        stream,
    },
    theme,
    widget,
};

pub const APP_ID: &str = "io.github.Aenes0.CosmicCentral.ShowDesktop";

#[derive(Clone, Debug)]
pub enum Message {
    Ready(calloop::channel::Sender<()>),
    Toggle,
}

#[derive(Default)]
pub struct ShowDesktop {
    core: app::Core,
    tx: Option<calloop::channel::Sender<()>>,
}

impl cosmic::Application for ShowDesktop {
    type Message = Message;
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = ();
    const APP_ID: &'static str = APP_ID;

    fn init(core: app::Core, _flags: ()) -> (Self, app::Task<Message>) {
        (Self { core, tx: None }, Task::none())
    }

    fn core(&self) -> &app::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut app::Core {
        &mut self.core
    }

    fn style(&self) -> Option<iced::theme::Style> {
        Some(cosmic::applet::style())
    }

    fn update(&mut self, message: Message) -> app::Task<Message> {
        match message {
            Message::Ready(tx) => self.tx = Some(tx),
            Message::Toggle => {
                if let Some(tx) = &self.tx {
                    _ = tx.send(());
                }
            }
        }
        Task::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::run_with(std::any::TypeId::of::<Message>(), |_| {
            stream::channel(4, |mut output: mpsc::Sender<Message>| async move {
                let (tx, rx) = calloop::channel::channel();
                _ = output.send(Message::Ready(tx)).await;
                std::thread::spawn(move || {
                    if let Err(err) = run(rx) {
                        tracing::warn!("show desktop stopped: {err}");
                    }
                });
                futures::future::pending::<()>().await;
            })
        })
    }

    fn view(&self) -> Element<'_, Message> {
        let applet = &self.core.applet;
        let horizontal = applet.is_horizontal();
        let size = applet.suggested_size(true).0 as f32 + 2.0 * applet.suggested_padding(true).1 as f32;
        // A sliver, as Windows has it: a thin line in it, the rest to aim at.
        let line = widget::container(widget::space::horizontal())
            .width(if horizontal { Length::Fixed(1.0) } else { Length::Fixed(size * 0.6) })
            .height(if horizontal { Length::Fixed(size * 0.6) } else { Length::Fixed(1.0) })
            .class(theme::Container::custom(|theme| widget::container::Style {
                background: Some(iced::Background::Color(
                    theme.cosmic().background(theme.transparent).divider.into(),
                )),
                ..Default::default()
            }));
        let button = widget::button::custom(
            widget::container(line).center(Length::Fill),
        )
        .width(if horizontal { Length::Fixed(10.0) } else { Length::Fixed(size) })
        .height(if horizontal { Length::Fixed(size) } else { Length::Fixed(10.0) })
        .padding(0)
        .class(theme::Button::AppletIcon)
        .on_press(Message::Toggle);
        applet.autosize_window(button).into()
    }
}

struct State {
    registry_state: RegistryState,
    info: ToplevelInfoState,
    manager: ToplevelManagerState,
    /// What a click minimized, to bring back; the one in front last.
    hidden: Vec<ExtForeignToplevelHandleV1>,
}

impl State {
    fn toggle(&mut self) {
        use zcosmic_toplevel_handle_v1::State as S;
        let shown: Vec<_> = self
            .info
            .toplevels()
            .filter(|t| !t.state.contains(&S::Minimized))
            .filter_map(|t| Some((t.foreign_toplevel.clone(), t.cosmic_toplevel.clone()?, t.state.contains(&S::Activated))))
            .collect();
        if shown.is_empty() {
            // Nothing shows: back what we hid, the active one last, on top.
            for handle in std::mem::take(&mut self.hidden) {
                if let Some(cosmic) = self.info.info(&handle).and_then(|t| t.cosmic_toplevel.clone()) {
                    self.manager.manager.unset_minimized(&cosmic);
                }
            }
            return;
        }
        let mut order: Vec<_> = shown.iter().filter(|(_, _, active)| !active).cloned().collect();
        order.extend(shown.iter().filter(|(_, _, active)| *active).cloned());
        self.hidden = order.iter().map(|(handle, ..)| handle.clone()).collect();
        for (_, cosmic, _) in &order {
            self.manager.manager.set_minimized(cosmic);
        }
    }
}

fn run(rx: calloop::channel::Channel<()>) -> Result<(), Box<dyn std::error::Error>> {
    // The real compositor, which knows the windows, through the socket the
    // panel gives applets that ask for it.
    let socket = std::env::var("X_PRIVILEGED_WAYLAND_SOCKET")
        .ok()
        .and_then(|fd| fd.parse::<RawFd>().ok())
        .map(|fd| unsafe { UnixStream::from_raw_fd(fd) });
    let conn = match socket {
        Some(socket) => Connection::from_socket(socket)?,
        None => Connection::connect_to_env()?,
    };
    let (globals, event_queue) = registry_queue_init(&conn)?;
    let qh = event_queue.handle();
    let registry_state = RegistryState::new(&globals);
    let info = ToplevelInfoState::try_new(&registry_state, &qh).ok_or("no toplevel info")?;
    let manager = ToplevelManagerState::try_new(&registry_state, &qh).ok_or("no toplevel manager")?;
    let mut state = State {
        registry_state,
        info,
        manager,
        hidden: Vec::new(),
    };
    let mut event_loop = calloop::EventLoop::<State>::try_new()?;
    WaylandSource::new(conn, event_queue)
        .insert(event_loop.handle())
        .map_err(|err| err.error)?;
    event_loop
        .handle()
        .insert_source(rx, |event, (), state| {
            if let calloop::channel::Event::Msg(()) = event {
                state.toggle();
            }
        })
        .map_err(|err| err.error)?;
    loop {
        event_loop.dispatch(None, &mut state)?;
    }
}

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    sctk::registry_handlers!();
}

impl ToplevelInfoHandler for State {
    fn toplevel_info_state(&mut self) -> &mut ToplevelInfoState {
        &mut self.info
    }
    fn new_toplevel(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &ExtForeignToplevelHandleV1) {}
    fn update_toplevel(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &ExtForeignToplevelHandleV1) {}
    fn toplevel_closed(&mut self, _: &Connection, _: &QueueHandle<Self>, toplevel: &ExtForeignToplevelHandleV1) {
        self.hidden.retain(|h| h != toplevel);
    }
}

impl ToplevelManagerHandler for State {
    fn toplevel_manager_state(&mut self) -> &mut ToplevelManagerState {
        &mut self.manager
    }
    fn capabilities(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: Vec<WEnum<cctk::cosmic_protocols::toplevel_management::v1::client::zcosmic_toplevel_manager_v1::ZcosmicToplelevelManagementCapabilitiesV1>>,
    ) {
    }
}

cctk::delegate_toplevel_info!(State);
cctk::delegate_toplevel_manager!(State);
sctk::delegate_registry!(State);
