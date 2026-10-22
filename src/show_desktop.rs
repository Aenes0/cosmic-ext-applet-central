// SPDX-License-Identifier: GPL-3.0-only

//! Windows' "show desktop": a thin button at the bar's very end; a click
//! minimizes every window, the next brings back those, the one in front
//! last. Resting on it half a second only peeks: the windows come back as
//! the pointer leaves, unless a click pinned them. Its own applet, by the
//! same program, put after the clock.

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
    Ready(calloop::channel::Sender<Action>),
    Click,
    Enter,
    Leave,
    /// The pointer came to or left a window; ours only counts.
    Hover(cosmic::iced::window::Id, bool),
    PeekTimer(u64),
    /// The left button pressed on a window: the bar's own, ours. Taken from
    /// the window, as a button may miss it not knowing where the pointer is.
    Press(cosmic::iced::window::Id),
    None,
}

#[derive(Default)]
pub struct ShowDesktop {
    core: app::Core,
    tx: Option<calloop::channel::Sender<Action>>,
    peek: Peek,
}

impl ShowDesktop {
    fn send(&self, action: Action) {
        if action == Action::None {
            return;
        }
        if let Some(tx) = &self.tx {
            _ = tx.send(action);
        }
    }
}

impl cosmic::Application for ShowDesktop {
    type Message = Message;
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = ();
    const APP_ID: &'static str = APP_ID;

    fn init(mut core: app::Core, _flags: ()) -> (Self, app::Task<Message>) {
        // See-through, but no blur behind: the theme stays transparent while any
        // kind is set, and only layer surfaces, which there are none of here, blur.
        core.set_auto_blur(cosmic::core::Auto::System.into());
        (Self { core, tx: None, peek: Peek::default() }, Task::none())
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
            Message::Press(id) => {
                if Some(id) == self.core.main_window_id() {
                    return self.update(Message::Click);
                }
            }
            Message::Hover(id, inside) => {
                if Some(id) == self.core.main_window_id() {
                    return self.update(if inside { Message::Enter } else { Message::Leave });
                }
            }
            Message::Enter => {
                if let Some(seq) = self.peek.enter() {
                    // The only timer there is: one, when the pointer comes in.
                    return Task::future(async move {
                        tokio::time::sleep(PEEK_DELAY).await;
                        cosmic::action::app(Message::PeekTimer(seq))
                    });
                }
            }
            Message::PeekTimer(seq) => {
                let action = self.peek.timer(seq);
                self.send(action);
            }
            Message::Leave => {
                let action = self.peek.leave();
                self.send(action);
            }
            Message::Click => {
                let action = self.peek.click();
                self.send(action);
            }
            Message::None => {}
        }
        Task::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        let press = cosmic::iced::event::listen_with(|event, _, id| match event {
            cosmic::iced::Event::Mouse(cosmic::iced::mouse::Event::ButtonPressed(
                cosmic::iced::mouse::Button::Left,
            )) => Some(Message::Press(id)),
            // The applet's window is the button itself, so the pointer
            // entering or leaving it is the pointer on the button.
            cosmic::iced::Event::Mouse(cosmic::iced::mouse::Event::CursorEntered) => {
                Some(Message::Hover(id, true))
            }
            cosmic::iced::Event::Mouse(cosmic::iced::mouse::Event::CursorLeft) => {
                Some(Message::Hover(id, false))
            }
            _ => None,
        });
        let wayland = Subscription::run_with(std::any::TypeId::of::<Message>(), |_| {
            stream::channel(4, |mut output: mpsc::Sender<Message>| async move {
                let (tx, rx) = calloop::channel::channel::<Action>();
                _ = output.send(Message::Ready(tx)).await;
                std::thread::spawn(move || {
                    if let Err(err) = run(rx) {
                        tracing::warn!("show desktop stopped: {err}");
                    }
                });
                futures::future::pending::<()>().await;
            })
        });
        Subscription::batch([press, wayland])
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
        .on_press(Message::None);
        applet.autosize_window(button).into()
    }
}

/// What the peek asks the window thread to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Hide,
    Restore,
}

/// How long the pointer must rest on the button before the peek.
const PEEK_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    /// Pointer in, timer `seq` running.
    Waiting(u64),
    /// Windows hidden for as long as the pointer stays.
    Peeking,
    /// Hidden by a click: stays until the next click.
    Pinned,
}

/// The peek as a pure machine: each event says what to do next.
#[derive(Debug, Default)]
struct Peek {
    phase: Phase,
    seq: u64,
}

impl Peek {
    /// Pointer entered; the generation of the timer to start, if any.
    fn enter(&mut self) -> Option<u64> {
        if self.phase != Phase::Idle {
            return None;
        }
        self.seq += 1;
        self.phase = Phase::Waiting(self.seq);
        Some(self.seq)
    }

    /// A timer of generation `seq` fired.
    fn timer(&mut self, seq: u64) -> Action {
        if self.phase != Phase::Waiting(seq) {
            return Action::None;
        }
        self.phase = Phase::Peeking;
        Action::Hide
    }

    fn leave(&mut self) -> Action {
        match self.phase {
            Phase::Waiting(_) => {
                self.phase = Phase::Idle;
                Action::None
            }
            Phase::Peeking => {
                self.phase = Phase::Idle;
                Action::Restore
            }
            Phase::Idle | Phase::Pinned => Action::None,
        }
    }

    fn click(&mut self) -> Action {
        match self.phase {
            Phase::Idle | Phase::Waiting(_) => {
                self.phase = Phase::Pinned;
                Action::Hide
            }
            Phase::Peeking => {
                self.phase = Phase::Pinned;
                Action::None
            }
            Phase::Pinned => {
                self.phase = Phase::Idle;
                Action::Restore
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_then_timer_hides() {
        let mut p = Peek::default();
        let seq = p.enter().expect("timer");
        assert_eq!(p.timer(seq), Action::Hide);
        assert_eq!(p.phase, Phase::Peeking);
    }

    #[test]
    fn leave_before_timer_cancels() {
        let mut p = Peek::default();
        let seq = p.enter().unwrap();
        assert_eq!(p.leave(), Action::None);
        assert_eq!(p.timer(seq), Action::None);
        assert_eq!(p.phase, Phase::Idle);
    }

    #[test]
    fn stale_timer_after_reenter_does_nothing() {
        let mut p = Peek::default();
        let old = p.enter().unwrap();
        p.leave();
        let new = p.enter().unwrap();
        assert_ne!(old, new);
        assert_eq!(p.timer(old), Action::None);
        assert_eq!(p.timer(new), Action::Hide);
    }

    #[test]
    fn leave_while_peeking_restores() {
        let mut p = Peek::default();
        let seq = p.enter().unwrap();
        p.timer(seq);
        assert_eq!(p.leave(), Action::Restore);
        assert_eq!(p.phase, Phase::Idle);
    }

    #[test]
    fn click_while_peeking_pins_and_leave_keeps_hidden() {
        let mut p = Peek::default();
        let seq = p.enter().unwrap();
        p.timer(seq);
        assert_eq!(p.click(), Action::None);
        assert_eq!(p.phase, Phase::Pinned);
        assert_eq!(p.leave(), Action::None);
        assert_eq!(p.phase, Phase::Pinned);
    }

    #[test]
    fn click_when_pinned_restores() {
        let mut p = Peek::default();
        let seq = p.enter().unwrap();
        p.timer(seq);
        p.click();
        assert_eq!(p.click(), Action::Restore);
        assert_eq!(p.phase, Phase::Idle);
    }

    #[test]
    fn enter_when_pinned_does_nothing() {
        let mut p = Peek::default();
        assert_eq!(p.click(), Action::Hide);
        assert_eq!(p.phase, Phase::Pinned);
        assert_eq!(p.enter(), None);
        assert_eq!(p.leave(), Action::None);
        assert_eq!(p.phase, Phase::Pinned);
    }

    #[test]
    fn click_without_peek_hides_and_pins() {
        let mut p = Peek::default();
        assert_eq!(p.click(), Action::Hide);
        assert_eq!(p.phase, Phase::Pinned);
    }

    #[test]
    fn click_while_waiting_hides_and_cancels_timer() {
        let mut p = Peek::default();
        let seq = p.enter().unwrap();
        assert_eq!(p.click(), Action::Hide);
        assert_eq!(p.timer(seq), Action::None);
        assert_eq!(p.phase, Phase::Pinned);
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
    fn hide(&mut self) {
        use zcosmic_toplevel_handle_v1::State as S;
        let shown: Vec<_> = self
            .info
            .toplevels()
            .filter(|t| !t.state.contains(&S::Minimized))
            .filter_map(|t| Some((t.foreign_toplevel.clone(), t.cosmic_toplevel.clone()?, t.state.contains(&S::Activated))))
            .collect();
        if shown.is_empty() {
            return;
        }
        let mut order: Vec<_> = shown.iter().filter(|(_, _, active)| !active).cloned().collect();
        order.extend(shown.iter().filter(|(_, _, active)| *active).cloned());
        self.hidden = order.iter().map(|(handle, ..)| handle.clone()).collect();
        for (_, cosmic, _) in &order {
            self.manager.manager.set_minimized(cosmic);
        }
    }

    /// Back what we hid, the active one last, on top.
    fn restore(&mut self) {
        for handle in std::mem::take(&mut self.hidden) {
            if let Some(cosmic) = self.info.info(&handle).and_then(|t| t.cosmic_toplevel.clone()) {
                self.manager.manager.unset_minimized(&cosmic);
            }
        }
    }
}

fn run(rx: calloop::channel::Channel<Action>) -> Result<(), Box<dyn std::error::Error>> {
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
            if let calloop::channel::Event::Msg(action) = event {
                match action {
                    Action::Hide => state.hide(),
                    Action::Restore => state.restore(),
                    Action::None => {}
                }
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
