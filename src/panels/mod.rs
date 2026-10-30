// SPDX-License-Identifier: GPL-3.0-only

//! The Central's and the monitor's panels, for every screen, in a process of
//! their own (`--panels`), not in the bar's applets: one stuck there leaves
//! the bar and the compositor be. It stays running, hidden between uses, and
//! its applets on the bar ask it over D-Bus to show or hide a panel.
//!
//! A panel shows on a layer surface its own size, by the bar at its end (the
//! Central) or its start (the monitor). Escape closes it, and so does the
//! keyboard going elsewhere (a click on a window, on the bar). The start
//! menu's way, a transparent surface over the whole screen, was measured
//! here and cost far more (drawn on the CPU), so it's only an option. While
//! hidden, nothing is read, watched or drawn.

pub mod place;
pub mod service;
mod watchdog;

use std::time::{Duration, Instant};

use cosmic::{
    Element, Task, app,
    applet::cosmic_panel_config::PanelAnchor,
    cctk::sctk::reexports::client::protocol::wl_output::WlOutput,
    cosmic_config::{Config, ConfigGet},
    iced::{
        self, Length, Limits, Subscription, event,
        keyboard::{self, Key, key::Named},
        platform_specific::{
            runtime::wayland::layer_surface::{IcedMargin, IcedOutput, SctkLayerSurfaceSettings},
            shell::commands::layer_surface::{Anchor, KeyboardInteractivity, Layer, get_layer_surface},
        },
        window,
    },
    widget,
};

use crate::{app as central, app::App, monitor, monitor::Monitor};
use place::{Request, Step, Which};

pub const APP_ID: &str = "io.github.Aenes0.CosmicCentral.Panels";

/// A close this recent was the click on the bar asking for it again: the
/// bar took the keyboard first.
const JUST_CLOSED: Duration = Duration::from_millis(300);
/// Losing the keyboard this soon after showing is the opening, not someone
/// leaving it.
const SETTLING: Duration = Duration::from_millis(400);
/// After starting, when to find which monitors have DDC, if none is known.
const DETECT_AFTER: Duration = Duration::from_secs(45);
/// Tried again this much later while a panel shows or just opened.
const DETECT_AGAIN: Duration = Duration::from_secs(30);

#[derive(Clone, Debug)]
pub enum Message {
    Central(central::Message),
    Monitor(monitor::Message),
    Request(Request),
    Close,
    /// The panel's surface got the keyboard (logged only).
    Focused,
    Unfocused,
    Output(String, WlOutput, Option<i32>),
    OutputGone(WlOutput),
    /// A surface was configured: the panel's, for timing its opening.
    Shown(window::Id),
    /// Another host serves the bus.
    Quit,
    /// On the bus: up, for systemd.
    Ready,
    /// The UI's loop still turns: for systemd's watchdog.
    Beat,
    /// Time to find which monitors have DDC, if none is known.
    DetectDdc,
    /// A click on a panel, kept from the backdrop under it.
    Nothing,
}

/// The panel showing: which, on which screen, its surface and since when.
struct Shown {
    which: Which,
    output: String,
    id: window::Id,
    at: Instant,
}

/// A screen: its name, the protocol's object, and its height.
struct Output {
    name: String,
    output: WlOutput,
    height: Option<i32>,
}

pub struct Panels {
    core: app::Core,
    central: App,
    monitor: Monitor,
    outputs: Vec<Output>,
    shown: Option<Shown>,
    /// Asked before the screens were known.
    pending: Option<Request>,
    /// The panel that closed last, where, and when.
    closed: Option<(Which, String, Instant)>,
    /// When a panel last opened.
    opened_at: Option<Instant>,
    /// The bar's edge, read once: moving the bar restarts its applets, not this.
    bar: PanelAnchor,
    /// The surface over the whole screen, a click outside closing it, as
    /// the start menu's; or, the default, the panel's size only: drawn on
    /// the CPU, the whole screen cost 5–10× the time to show and several
    /// times the CPU, here and in the compositor (the cursor stuttered).
    /// COSMIC_PANELS_SURFACE=screen brings it back, for comparing.
    whole_screen: bool,
}

/// Starts the host, showing `request` once up.
pub fn run(request: Option<Request>) -> cosmic::iced::Result {
    let settings = cosmic::app::Settings::default()
        .no_main_window(true)
        .exit_on_close(false)
        .transparent(true);
    cosmic::app::run::<Panels>(settings, request)
}

/// The bar the applets are on: the panel's edge. COSMIC's own default if
/// it can't be read.
fn bar_anchor() -> PanelAnchor {
    Config::new("com.system76.CosmicPanel.Panel", 1)
        .ok()
        .and_then(|config| config.get::<PanelAnchor>("anchor").ok())
        .unwrap_or_default()
}

/// COSMIC_QS_PREVIEW=sound, or COSMIC_MONITOR_PREVIEW=1|edit|place, shows
/// that panel by itself, for screenshots, on COSMIC_QS_PREVIEW_OUTPUT's
/// screen (none: the active one).
fn preview_from_env() -> Option<Request> {
    let output = std::env::var("COSMIC_QS_PREVIEW_OUTPUT").unwrap_or_default();
    if let Ok(page) = std::env::var("COSMIC_QS_PREVIEW") {
        return Some(Request { which: Which::Central, output, page: Some(page) });
    }
    let page = std::env::var("COSMIC_MONITOR_PREVIEW").ok()?;
    Some(Request { which: Which::Monitor, output, page: Some(page) })
}

/// A child's task as the host's.
fn lift<M: Send + 'static>(task: app::Task<M>, wrap: fn(M) -> Message) -> app::Task<Message> {
    task.map(move |action| match action {
        cosmic::Action::App(message) => cosmic::Action::App(wrap(message)),
        cosmic::Action::Cosmic(action) => cosmic::Action::Cosmic(action),
        cosmic::Action::None => cosmic::Action::None,
    })
}

impl Panels {
    fn output(&self, name: &str) -> Option<&Output> {
        self.outputs.iter().find(|o| o.name == name)
    }

    fn request(&mut self, request: Request) -> app::Task<Message> {
        // The screens come a moment after starting.
        if self.outputs.is_empty() {
            self.pending = Some(request);
            return Task::none();
        }
        let shown = self.shown.as_ref().map(|s| (s.which, s.output.as_str()));
        let just_closed = self
            .closed
            .as_ref()
            .filter(|(_, _, at)| at.elapsed() < JUST_CLOSED)
            .map(|(which, output, _)| (*which, output.as_str()));
        match place::step(shown, just_closed, &request) {
            Step::Open => self.show(request),
            Step::Close => self.hide(),
            Step::Switch => {
                let hide = self.hide();
                Task::batch([hide, self.show(request)])
            }
            Step::Page => {
                let page = request.page.as_deref().unwrap_or_default();
                match request.which {
                    Which::Central => lift(self.central.turn_to(page), Message::Central),
                    Which::Monitor => lift(self.monitor.turn_to(page), Message::Monitor),
                }
            }
            Step::Ignore => Task::none(),
        }
    }

    fn show(&mut self, request: Request) -> app::Task<Message> {
        crate::timing::opening(request.which.name());
        let id = window::Id::unique();
        let screen = self.output(&request.output);
        let height = screen.and_then(|o| o.height);
        let output = screen.map_or(IcedOutput::Active, |o| IcedOutput::Output(o.output.clone()));
        // Over the screen but the bar (its exclusive zone keeps it out):
        // a click on the bar is the bar's.
        let surface = if self.whole_screen {
            get_layer_surface(SctkLayerSurfaceSettings {
                id,
                layer: Layer::Top,
                keyboard_interactivity: KeyboardInteractivity::OnDemand,
                anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
                output,
                namespace: "central-panels".into(),
                size: Some((None, None)),
                exclusive_zone: 0,
                size_limits: Limits::NONE.min_width(1.0).min_height(1.0),
                ..Default::default()
            })
        } else {
            // The panel's size only, by the bar at its end.
            let [top, bottom, left, right] = place::edges(self.bar, request.which);
            let mut anchor = Anchor::empty();
            anchor.set(Anchor::TOP, top);
            anchor.set(Anchor::BOTTOM, bottom);
            anchor.set(Anchor::LEFT, left);
            anchor.set(Anchor::RIGHT, right);
            let gap = place::GAP as i32;
            get_layer_surface(SctkLayerSurfaceSettings {
                id,
                layer: Layer::Top,
                keyboard_interactivity: KeyboardInteractivity::OnDemand,
                anchor,
                output,
                namespace: "central-panels".into(),
                margin: IcedMargin { top: gap, right: gap, bottom: gap, left: gap },
                size: None,
                exclusive_zone: 0,
                size_limits: Limits::NONE.min_width(1.0).min_height(1.0),
                ..Default::default()
            })
        };
        // The theme as it is now, for what the panels read of it.
        let core = self.core.clone();
        let page = request.page.as_deref();
        let panel = match request.which {
            Which::Central => {
                *cosmic::Application::core_mut(&mut self.central) = core;
                lift(self.central.show_hosted(id, page, height), Message::Central)
            }
            Which::Monitor => {
                *cosmic::Application::core_mut(&mut self.monitor) = core;
                lift(self.monitor.show_hosted(id, page, height), Message::Monitor)
            }
        };
        tracing::debug!("showing the {} on {:?}: {id:?}", request.which.name(), request.output);
        self.shown = Some(Shown { which: request.which, output: request.output, id, at: Instant::now() });
        self.opened_at = Some(Instant::now());
        Task::batch([surface, panel])
    }

    fn hide(&mut self) -> app::Task<Message> {
        let Some(shown) = self.shown.take() else {
            return Task::none();
        };
        tracing::debug!("hiding the {}: {:?}", shown.which.name(), shown.id);
        self.closed = Some((shown.which, shown.output, Instant::now()));
        match shown.which {
            Which::Central => lift(self.central.close_hosted(), Message::Central),
            Which::Monitor => lift(self.monitor.close_hosted(), Message::Monitor),
        }
    }

    /// A panel that closed itself (it ran something, say) is no longer shown.
    fn follow_child(&mut self) {
        let Some(shown) = &self.shown else {
            return;
        };
        let surface = match shown.which {
            Which::Central => self.central.surface(),
            Which::Monitor => self.monitor.surface(),
        };
        if surface != Some(shown.id)
            && let Some(shown) = self.shown.take()
        {
            self.closed = Some((shown.which, shown.output, Instant::now()));
        }
    }
}

impl cosmic::Application for Panels {
    type Message = Message;
    // One worker thread, not one per core: there is little to do.
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = Option<Request>;
    const APP_ID: &'static str = APP_ID;

    fn init(mut core: app::Core, request: Option<Request>) -> (Self, app::Task<Message>) {
        // Looks as the applets' popups did: the theme as theirs, see-through,
        // but no blur, which on a layer surface would be the whole screen's.
        core.set_app_type(cosmic::core::AppType::Applet);
        // (A layer surface blurs only with Auto::System: none is asked for;
        // measured, turning this off too changed nothing.)
        core.set_auto_blur(cosmic::core::Auto::Popup.into());
        let panels = Self {
            central: App::hosted(core.clone()),
            monitor: Monitor::hosted(core.clone()),
            core,
            outputs: Vec::new(),
            shown: None,
            pending: request.or_else(preview_from_env),
            closed: None,
            opened_at: None,
            bar: bar_anchor(),
            whole_screen: std::env::var("COSMIC_PANELS_SURFACE").as_deref() == Ok("screen"),
        };
        // Which monitors have DDC, for the brightness sliders: found once,
        // well after starting, if not known from before (see DetectDdc).
        let detect = Task::perform(tokio::time::sleep(DETECT_AFTER), |()| cosmic::action::app(Message::DetectDdc));
        (panels, detect)
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
            Message::Central(message) => {
                let task = lift(cosmic::Application::update(&mut self.central, message), Message::Central);
                self.follow_child();
                return task;
            }
            Message::Monitor(message) => {
                let task = lift(cosmic::Application::update(&mut self.monitor, message), Message::Monitor);
                self.follow_child();
                return task;
            }
            Message::Request(request) => return self.request(request),
            Message::Close => return self.hide(),
            Message::Focused => tracing::debug!("the panel has the keyboard"),
            Message::Unfocused => {
                if self.shown.as_ref().is_some_and(|s| s.at.elapsed() > SETTLING) {
                    return self.hide();
                }
            }
            Message::Output(name, output, height) => {
                match self.outputs.iter_mut().find(|o| o.output == output) {
                    Some(known) => {
                        known.name = name;
                        known.height = height.or(known.height);
                    }
                    None => self.outputs.push(Output { name, output, height }),
                }
                if let Some(request) = self.pending.take() {
                    return self.request(request);
                }
            }
            Message::OutputGone(output) => self.outputs.retain(|o| o.output != output),
            Message::Shown(id) => {
                if let Some(shown) = self.shown.as_ref().filter(|s| s.id == id) {
                    crate::timing::drawn(shown.which.name());
                }
            }
            Message::Quit => std::process::exit(0),
            Message::Ready => watchdog::ready(),
            Message::Beat => watchdog::beat(),
            Message::DetectDdc => {
                // Finding out holds the GPU's I²C, and the compositor's
                // rendering with it, for a moment: never while opening.
                if !place::quiet(self.shown.is_some(), self.opened_at.map(|at| at.elapsed())) {
                    return Task::perform(tokio::time::sleep(DETECT_AGAIN), |()| {
                        cosmic::action::app(Message::DetectDdc)
                    });
                }
                return Task::perform(
                    crate::slow::bounded(crate::slow::Slow::Ddc, crate::services::detect_ddc_if_unknown()),
                    |known| {
                        tracing::debug!("monitors with DDC known: {known:?}");
                        cosmic::action::app(Message::Nothing)
                    },
                );
            }
            Message::Nothing => {}
        }
        Task::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = vec![
            service::dbus_service(),
            // The screens: where a panel can show, and how tall each is.
            event::listen_with(|event, _, _| {
                use iced::event::wayland::{Event as Wayland, OutputEvent};
                match event {
                    iced::Event::PlatformSpecific(iced::event::PlatformSpecific::Wayland(Wayland::Output(
                        OutputEvent::Created(Some(info)) | OutputEvent::InfoUpdate(info),
                        output,
                    ))) => {
                        let height = info.logical_size.map(|(_, h)| h);
                        info.name.map(|name| Message::Output(name, output, height))
                    }
                    iced::Event::PlatformSpecific(iced::event::PlatformSpecific::Wayland(Wayland::Output(
                        OutputEvent::Removed,
                        output,
                    ))) => Some(Message::OutputGone(output)),
                    _ => None,
                }
            }),
        ];
        // Under systemd's watchdog only: the beat, from this loop.
        if let Some(period) = watchdog::period() {
            subscriptions.push(iced::time::every(period).map(|_| Message::Beat));
        }
        // Only while a panel shows: its own, and Escape or leaving it.
        if let Some(shown) = &self.shown {
            subscriptions.push(event::listen_with(|event, _, window| {
                use iced::event::wayland::{Event as Wayland, LayerEvent};
                match event {
                    // A layer surface's first configure.
                    iced::Event::Window(window::Event::Opened { .. }) => Some(Message::Shown(window)),
                    iced::Event::Keyboard(keyboard::Event::KeyPressed {
                        key: Key::Named(Named::Escape),
                        ..
                    }) => Some(Message::Close),
                    iced::Event::PlatformSpecific(iced::event::PlatformSpecific::Wayland(Wayland::Layer(
                        LayerEvent::Unfocused,
                        ..,
                    ))) => Some(Message::Unfocused),
                    iced::Event::PlatformSpecific(iced::event::PlatformSpecific::Wayland(Wayland::Layer(
                        LayerEvent::Focused,
                        ..,
                    ))) => Some(Message::Focused),
                    _ => None,
                }
            }));
            subscriptions.push(match shown.which {
                Which::Central => cosmic::Application::subscription(&self.central).map(Message::Central),
                Which::Monitor => cosmic::Application::subscription(&self.monitor).map(Message::Monitor),
            });
        }
        Subscription::batch(subscriptions)
    }

    fn view(&self) -> Element<'_, Message> {
        widget::text("").into()
    }

    fn view_window(&self, id: window::Id) -> Element<'_, Message> {
        let Some(shown) = self.shown.as_ref().filter(|s| s.id == id) else {
            return widget::text("").into();
        };
        let panel = match shown.which {
            Which::Central => cosmic::Application::view_window(&self.central, id).map(Message::Central),
            Which::Monitor => cosmic::Application::view_window(&self.monitor, id).map(Message::Monitor),
        };
        if !self.whole_screen {
            // The surface takes the panel's size, as it changes.
            return cosmic::widget::autosize::autosize(panel, widget::Id::new("central-panels-autosize")).into();
        }
        // Clicks on the panel stay there; anywhere else, they close it.
        let panel = widget::mouse_area(panel).on_press(Message::Nothing);
        let (x, y) = place::corner(self.bar, shown.which);
        let placed = widget::container(panel)
            .padding(place::GAP)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(x)
            .align_y(y);
        let backdrop = widget::mouse_area(
            widget::container(widget::space::horizontal())
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .on_press(Message::Close)
        .on_right_press(Message::Close);
        iced::widget::stack(vec![backdrop.into(), placed.into()]).into()
    }

    fn on_close_requested(&self, id: window::Id) -> Option<Message> {
        let shown = self.shown.as_ref().filter(|s| s.id == id)?;
        Some(match shown.which {
            Which::Central => Message::Central(central::Message::Closed(id)),
            Which::Monitor => Message::Monitor(monitor::Message::Closed(id)),
        })
    }
}

/// The host outlives what started it (an applet, a terminal), and so the
/// pipe its output went to: once that closed, the next warning written
/// would abort it. Its own line to the journal instead, as systemd-cat; or
/// nowhere. As the start menu does.
pub fn own_output() {
    use std::{
        io::Write,
        os::{fd::AsRawFd, unix::net::UnixStream},
    };
    let journal = UnixStream::connect("/run/systemd/journal/stdout").and_then(|mut stream| {
        stream.shutdown(std::net::Shutdown::Read)?;
        // Identifier, unit, priority (info), level prefix, and no copies
        // to syslog, the kernel log or the console.
        stream.write_all(b"cosmic-ext-applet-central-panels\n\n6\n0\n0\n0\n0\n")?;
        Ok(std::os::fd::OwnedFd::from(stream))
    });
    let target = match journal {
        Ok(fd) => fd,
        Err(_) => match std::fs::OpenOptions::new().write(true).open("/dev/null") {
            Ok(file) => file.into(),
            Err(_) => return,
        },
    };
    // SAFETY: duplicates an open descriptor over stdout and stderr.
    unsafe {
        libc::dup2(target.as_raw_fd(), 1);
        libc::dup2(target.as_raw_fd(), 2);
    }
}
