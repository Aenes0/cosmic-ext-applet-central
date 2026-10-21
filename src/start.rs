// SPDX-License-Identifier: GPL-3.0-only

//! Windows 11's Start menu, in place of COSMIC's launcher (the Super key) and
//! its app library, both in one, as the 2025 one is: one page, the search on
//! top, already focused; the pinned apps, the open windows, the recommended
//! (apps just installed or used, files just changed) and all the apps, by
//! category, in a grid or a list; the user and the power button at the foot.
//!
//! Its own program, the same binary run with `--start`, a layer surface
//! over the screen, not an applet's popup: so the Super key, a shortcut,
//! can open it, and it gets the keyboard. The first run stays, hidden
//! between uses; the next ones ask it, over D-Bus, to show or hide, and
//! leave. `--start-button` is the button for the panel, which does the same.

use std::{
    collections::HashMap,
    os::{
        fd::{FromRawFd, RawFd},
        unix::net::UnixStream,
    },
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use cosmic::{
    Element, Task, app,
    cctk::{
        self,
        cosmic_protocols::{
            toplevel_info::v1::client::zcosmic_toplevel_handle_v1,
            toplevel_management::v1::client::zcosmic_toplevel_manager_v1,
        },
        sctk::{
            self,
            reexports::{calloop, calloop_wayland_source::WaylandSource},
            registry::{ProvidesRegistryState, RegistryState},
            seat::{Capability, SeatHandler, SeatState},
        },
        toplevel_info::{ToplevelInfoHandler, ToplevelInfoState},
        toplevel_management::{ToplevelManagerHandler, ToplevelManagerState},
        wayland_client::{
            Connection, QueueHandle, WEnum, globals::registry_queue_init, protocol::wl_seat::WlSeat,
        },
        wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
    },
    cosmic_config::{Config, ConfigGet, ConfigSet},
    desktop::{IconSourceExt, fde},
    iced::{
        self, Alignment, Background, Color, Length, Limits, Subscription,
        event::{self, wayland},
        futures::{self, SinkExt, channel::mpsc},
        keyboard::{self, Key, key::Named},
        platform_specific::{
            runtime::wayland::layer_surface::SctkLayerSurfaceSettings,
            shell::commands::layer_surface::{
                Anchor, KeyboardInteractivity, Layer, destroy_layer_surface, get_layer_surface,
            },
        },
        stream,
        widget::{scrollable, text::Wrapping},
        window,
    },
    theme,
    widget::{self, button, icon, text_input},
};

use crate::{i18n::strings, texts::texts};

pub const APP_ID: &str = "io.github.Aenes0.CosmicCentral.Start";
const BUS_NAME: &str = "io.github.Aenes0.CosmicCentral.Start";
const BUS_PATH: &str = "/io/github/Aenes0/CosmicCentral/Start";

const WIDTH: f32 = 760.0;
const HEIGHT: f32 = 800.0;
/// Room around the menu for its shadow.
const MARGIN: f32 = 16.0;
const SIDE: u16 = 40;
const FOOTER: f32 = 64.0;
const PINNED_COLUMNS: usize = 8;
const PINNED_ROWS: usize = 2;
const CARD_COLUMNS: usize = 3;
const CARD_ROWS: usize = 2;
const CATEGORY_COLUMNS: usize = 4;
/// How long it takes to rise from the panel.
const OPENING: Duration = Duration::from_millis(220);

static SEARCH: LazyLock<widget::Id> = LazyLock::new(|| widget::Id::new("start-search"));
static HOME: LazyLock<widget::Id> = LazyLock::new(|| widget::Id::new("start-home"));

/// Asks the running menu to show or hide; false when none runs, and this
/// one should be it.
pub fn toggle_running() -> bool {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
        return false;
    };
    runtime.block_on(async {
        let Ok(conn) = zbus::Connection::session().await else {
            return false;
        };
        conn.call_method(Some(BUS_NAME), BUS_PATH, Some(BUS_NAME), "Toggle", &())
            .await
            .is_ok()
    })
}

#[derive(Clone, Debug)]
pub struct AppEntry {
    id: String,
    name: String,
    wm_class: Option<String>,
    exec: Option<String>,
    terminal: bool,
    icon: icon::Handle,
    /// The name, the generic name, the keywords…, folded, to search in.
    haystack: String,
    folded_name: String,
    actions: Vec<(String, String)>,
    category: usize,
    /// When its desktop entry appeared, in seconds since the epoch.
    added: i64,
}

#[derive(Clone, Debug)]
pub struct Window {
    id: String,
    title: String,
    app_id: String,
    active: bool,
}

#[derive(Clone, Debug)]
pub struct RecentFile {
    path: String,
    name: String,
    icons: Vec<String>,
    when: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Home,
    Recommended,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllView {
    Category,
    Grid,
    List,
}

/// What a click opened a little menu for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    App(String),
    Window(String),
    File(String),
    Category(usize),
    User,
    Power,
}

/// A search result.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Hit {
    App(usize),
    Window(String),
    File(usize),
}

/// One of the recommended: an app, or a file, with what to say under it.
enum Pick<'a> {
    App(&'a AppEntry, String),
    File(&'a RecentFile),
}

#[derive(Clone, Debug)]
pub enum Message {
    Toggle,
    Hide,
    Unfocused,
    Escape,
    Focus,
    Focused,
    Frame,
    Resized(f32),
    Apps(Arc<Vec<AppEntry>>),
    Recents(Vec<RecentFile>),
    Windows(Vec<Window>),
    WindowsReady(calloop::channel::Sender<Command>),
    Query(String),
    TypeAhead(String),
    Move(i32),
    Submit,
    Page(Page),
    ExpandPinned(bool),
    NextView,
    Launch(String),
    LaunchAction(String, String),
    Activate(String),
    CloseWindow(String),
    OpenFile(String),
    OpenLocation(String),
    Menu(Option<Target>),
    TogglePin(String),
    Spawn(&'static str, Vec<String>),
    Nothing,
}

#[derive(Clone, Debug)]
pub enum Command {
    Activate(String),
    Close(String),
}

pub struct StartMenu {
    core: app::Core,
    surface: Option<window::Id>,
    shown_at: Instant,
    /// The height the surface got: the screen, less the panel.
    room: f32,
    apps: Arc<Vec<AppEntry>>,
    recents: Vec<RecentFile>,
    /// The apps opened from here, the last first, with when.
    launched: Vec<(String, i64)>,
    windows: Vec<Window>,
    windows_tx: Option<calloop::channel::Sender<Command>>,
    pinned: Vec<String>,
    pinned_expanded: bool,
    all_view: AllView,
    query: String,
    selected: usize,
    page: Page,
    menu: Option<Target>,
    user: String,
    avatar: Option<PathBuf>,
    on_top: bool,
}

impl cosmic::Application for StartMenu {
    type Message = Message;
    type Executor = cosmic::executor::Default;
    type Flags = ();
    const APP_ID: &'static str = APP_ID;

    fn init(core: app::Core, _flags: ()) -> (Self, app::Task<Message>) {
        let mut menu = Self {
            core,
            surface: None,
            shown_at: Instant::now(),
            room: 0.0,
            apps: Arc::default(),
            recents: Vec::new(),
            launched: config().and_then(|c| c.get("launched").ok()).unwrap_or_default(),
            windows: Vec::new(),
            windows_tx: None,
            pinned: load_pinned(),
            pinned_expanded: false,
            all_view: match config().and_then(|c| c.get::<u8>("all_view").ok()) {
                Some(1) => AllView::Grid,
                Some(2) => AllView::List,
                _ => AllView::Category,
            },
            query: String::new(),
            selected: 0,
            page: Page::Home,
            menu: None,
            user: user_name(),
            avatar: avatar(),
            on_top: panel_on_top(),
        };
        let show = menu.show();
        // COSMIC_START_PREVIEW=recommended, power, user or a category's
        // number opens there; any other text searches it: to see them
        // without clicking.
        match std::env::var("COSMIC_START_PREVIEW").as_deref() {
            Ok("recommended") => menu.page = Page::Recommended,
            Ok("power") => menu.menu = Some(Target::Power),
            Ok("user") => menu.menu = Some(Target::User),
            Ok("grid") => menu.all_view = AllView::Grid,
            Ok("list") => menu.all_view = AllView::List,
            Ok("end") => {
                let end = Task::perform(tokio::time::sleep(Duration::from_millis(400)), |()| {
                    cosmic::action::app(Message::Nothing)
                })
                .chain(cosmic::iced::runtime::widget::operation::snap_to_end(HOME.clone()));
                return (menu, Task::batch([show, end]));
            }
            Ok(query) => match query.parse::<usize>() {
                Ok(category) => menu.menu = Some(Target::Category(category)),
                Err(_) => menu.query = query.to_owned(),
            },
            Err(_) => {}
        }
        (menu, show)
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
            Message::Toggle => {
                return if self.surface.is_some() { self.hide() } else { self.show() };
            }
            Message::Hide => return self.hide(),
            // Losing the keyboard as it opens is not someone leaving it.
            Message::Unfocused => {
                if self.shown_at.elapsed() > Duration::from_millis(400) {
                    return self.hide();
                }
            }
            Message::Escape => {
                // As on Windows: a menu open closes first, then the search
                // empties, then the page goes back, then it all goes.
                if self.menu.is_some() {
                    self.menu = None;
                } else if !self.query.is_empty() {
                    self.query.clear();
                    self.selected = 0;
                } else if self.page != Page::Home {
                    self.page = Page::Home;
                } else {
                    return self.hide();
                }
            }
            Message::Focus => return text_input::focus(SEARCH.clone()),
            Message::Focused => {
                if self.surface.is_some() {
                    return text_input::focus(SEARCH.clone());
                }
            }
            Message::Frame => {}
            Message::Resized(height) => self.room = height,
            Message::Apps(apps) => {
                self.apps = apps;
                self.selected = 0;
            }
            Message::Recents(recents) => self.recents = recents,
            Message::Windows(windows) => self.windows = windows,
            Message::WindowsReady(tx) => self.windows_tx = Some(tx),
            Message::Query(query) => {
                self.query = query;
                self.selected = 0;
                self.menu = None;
            }
            Message::TypeAhead(text) => {
                self.query.push_str(&text);
                self.selected = 0;
                return text_input::focus(SEARCH.clone());
            }
            Message::Move(step) => {
                let count = self.hits().len();
                if count > 0 {
                    self.selected =
                        (self.selected as i32 + step).rem_euclid(count as i32) as usize;
                }
            }
            Message::Submit => {
                if let Some(hit) = self.hits().get(self.selected).cloned() {
                    return self.open_hit(hit);
                }
            }
            Message::Page(page) => {
                self.page = page;
                self.menu = None;
            }
            Message::ExpandPinned(expanded) => self.pinned_expanded = expanded,
            Message::NextView => {
                self.all_view = match self.all_view {
                    AllView::Category => AllView::Grid,
                    AllView::Grid => AllView::List,
                    AllView::List => AllView::Category,
                };
                if let Some(config) = config() {
                    _ = config.set("all_view", self.all_view as u8);
                }
            }
            Message::Launch(id) => {
                if let Some(i) = self.apps.iter().position(|a| a.id == id) {
                    return self.launch(i, None);
                }
            }
            Message::LaunchAction(id, exec) => {
                if let Some(i) = self.apps.iter().position(|a| a.id == id) {
                    return self.launch(i, Some(exec));
                }
            }
            Message::Activate(id) => return self.activate(id),
            Message::CloseWindow(id) => {
                self.menu = None;
                if let Some(tx) = &self.windows_tx {
                    _ = tx.send(Command::Close(id));
                }
            }
            Message::OpenFile(path) => {
                crate::host::spawn("xdg-open", &[&path]);
                return self.hide();
            }
            Message::OpenLocation(path) => {
                let folder = Path::new(&path).parent().map(|p| p.to_string_lossy().into_owned());
                crate::host::spawn("xdg-open", &[&folder.unwrap_or(path)]);
                return self.hide();
            }
            Message::Menu(target) => self.menu = target,
            Message::TogglePin(id) => {
                if let Some(i) = self.pinned.iter().position(|p| *p == id) {
                    self.pinned.remove(i);
                } else {
                    self.pinned.push(id);
                }
                save_pinned(&self.pinned);
                self.menu = None;
            }
            Message::Spawn(program, args) => {
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                crate::host::spawn(program, &args);
                return self.hide();
            }
            Message::Nothing => {}
        }
        Task::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = vec![
            dbus_service(),
            windows_subscription(),
            event::listen_with(|event, status, _| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, text, .. }) => match key {
                    Key::Named(Named::Escape) => Some(Message::Escape),
                    Key::Named(Named::ArrowDown) => Some(Message::Move(1)),
                    Key::Named(Named::ArrowUp) => Some(Message::Move(-1)),
                    // Typing goes to the search, wherever the focus went.
                    _ if status == event::Status::Ignored => text
                        .filter(|t| !t.is_empty() && !t.chars().any(char::is_control))
                        .map(|t| Message::TypeAhead(t.to_string())),
                    _ => None,
                },
                iced::Event::Window(window::Event::Resized(size)) => {
                    Some(Message::Resized(size.height))
                }
                iced::Event::PlatformSpecific(event::PlatformSpecific::Wayland(
                    wayland::Event::Layer(layer, ..),
                )) => match layer {
                    wayland::LayerEvent::Focused => Some(Message::Focused),
                    wayland::LayerEvent::Unfocused => Some(Message::Unfocused),
                    _ => None,
                },
                _ => None,
            }),
        ];
        // A frame at a time while it rises.
        if self.surface.is_some() && self.shown_at.elapsed() < OPENING + Duration::from_millis(50) {
            subscriptions.push(window::frames().map(|_| Message::Frame));
        }
        Subscription::batch(subscriptions)
    }

    fn view(&self) -> Element<'_, Message> {
        widget::text("").into()
    }

    fn view_window(&self, id: window::Id) -> Element<'_, Message> {
        if self.surface != Some(id) {
            return widget::text("").into();
        }
        let p = palette(&cosmic::theme::active());
        let height = if self.room > 0.0 {
            HEIGHT.min(self.room - 2.0 * MARGIN)
        } else {
            HEIGHT
        };

        let body: Element<'_, Message> = if !self.query.trim().is_empty() {
            self.results(p)
        } else if self.page == Page::Recommended {
            self.recommended_page(p)
        } else {
            self.home(p)
        };
        let content = widget::column::with_children(vec![
            widget::container(self.search_bar(p))
                .padding([28, SIDE, 16, SIDE])
                .into(),
            widget::container(body).height(Length::Fill).into(),
            self.footer(p),
        ])
        .width(Length::Fixed(WIDTH))
        .height(Length::Fill);
        let mut layers: Vec<Element<'_, Message>> = vec![content.into()];
        if let Some(target) = &self.menu {
            layers.push(self.popup_menu(target, p));
        }
        let menu = widget::container(iced::widget::stack(layers))
            .width(Length::Fixed(WIDTH))
            .height(Length::Fixed(height))
            .class(theme::Container::custom(move |_| widget::container::Style {
                background: Some(Background::Color(p.bg)),
                text_color: Some(p.text),
                icon_color: Some(p.text),
                border: iced::Border {
                    radius: 8.0.into(),
                    width: 1.0,
                    color: p.border,
                },
                shadow: iced::Shadow {
                    color: Color::from_rgba(0.0, 0.0, 0.0, 0.45),
                    offset: iced::Vector::new(0.0, 6.0),
                    blur_radius: MARGIN,
                },
                ..Default::default()
            }));
        // Clicks on the menu stay there; anywhere else, they close it.
        let menu = widget::mouse_area(menu).on_press(Message::Nothing);

        // It rises from the panel, as on Windows: its top shows first and
        // the rest follows, as through a slot at the panel's edge.
        let t = (self.shown_at.elapsed().as_secs_f32() / OPENING.as_secs_f32()).min(1.0);
        let eased = 1.0 - (1.0 - t).powi(3);
        let full = height + 2.0 * MARGIN;
        let mut slot = widget::scrollable(widget::container(menu).padding(MARGIN))
            .direction(scrollable::Direction::Vertical(
                scrollable::Scrollbar::new().width(0.0).scroller_width(0.0),
            ))
            .height(Length::Fill);
        if self.on_top {
            slot = slot.anchor_bottom();
        }
        let slot = widget::container(slot)
            .width(Length::Fixed(WIDTH + 2.0 * MARGIN))
            .height(Length::Fixed((full * eased).max(1.0)))
            .clip(true);

        let backdrop = widget::mouse_area(
            widget::container(widget::space::horizontal())
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .on_press(Message::Hide)
        .on_right_press(Message::Hide);
        let placed = widget::container(slot)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(if self.on_top { Alignment::Start } else { Alignment::End });
        iced::widget::stack(vec![backdrop.into(), placed.into()]).into()
    }
}

impl StartMenu {
    fn show(&mut self) -> app::Task<Message> {
        if self.surface.is_some() {
            return Task::none();
        }
        let id = window::Id::unique();
        self.surface = Some(id);
        self.shown_at = Instant::now();
        self.query.clear();
        self.selected = 0;
        self.page = Page::Home;
        self.menu = None;
        self.pinned_expanded = false;
        self.on_top = panel_on_top();
        let surface = get_layer_surface(SctkLayerSurfaceSettings {
            id,
            layer: Layer::Top,
            keyboard_interactivity: KeyboardInteractivity::Exclusive,
            anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
            namespace: "central-start".into(),
            size: Some((None, None)),
            exclusive_zone: 0,
            size_limits: Limits::NONE.min_width(1.0).min_height(1.0),
            ..Default::default()
        });
        // Apps and files come and go: read them again each time, the old
        // lists showing meanwhile.
        let apps = Task::perform(
            async { tokio::task::spawn_blocking(load_apps).await.unwrap_or_default() },
            |apps| cosmic::action::app(Message::Apps(Arc::new(apps))),
        );
        let recents = Task::perform(
            async { tokio::task::spawn_blocking(load_recents).await.unwrap_or_default() },
            |recents| cosmic::action::app(Message::Recents(recents)),
        );
        let focus = Task::perform(tokio::time::sleep(Duration::from_millis(60)), |()| {
            cosmic::action::app(Message::Focus)
        });
        Task::batch([surface, apps, recents, focus])
    }

    fn hide(&mut self) -> app::Task<Message> {
        match self.surface.take() {
            Some(id) => destroy_layer_surface(id),
            None => Task::none(),
        }
    }

    fn launch(&mut self, index: usize, exec: Option<String>) -> app::Task<Message> {
        let app = &self.apps[index];
        let Some(exec) = exec.or_else(|| app.exec.clone()) else {
            return self.hide();
        };
        let id = app.id.clone();
        let terminal = app.terminal;
        self.launched.retain(|(launched, _)| *launched != id);
        self.launched.insert(0, (id.clone(), now()));
        self.launched.truncate(12);
        if let Some(Err(err)) = config().map(|c| c.set("launched", self.launched.clone())) {
            tracing::warn!("cannot save the apps used: {err}");
        }
        let launch = Task::perform(
            async move {
                cosmic::desktop::spawn_desktop_exec(
                    exec,
                    std::iter::empty::<(String, String)>(),
                    Some(&id),
                    terminal,
                )
                .await;
            },
            |()| cosmic::action::app(Message::Nothing),
        );
        Task::batch([self.hide(), launch])
    }

    fn activate(&mut self, id: String) -> app::Task<Message> {
        if let Some(tx) = &self.windows_tx {
            _ = tx.send(Command::Activate(id));
        }
        self.hide()
    }

    fn open_hit(&mut self, hit: Hit) -> app::Task<Message> {
        match hit {
            Hit::App(i) => self.launch(i, None),
            Hit::Window(id) => self.activate(id),
            Hit::File(i) => {
                let path = self.recents[i].path.clone();
                cosmic::Application::update(self, Message::OpenFile(path))
            }
        }
    }

    fn app_for_window(&self, window: &Window) -> Option<&AppEntry> {
        let wanted = window.app_id.to_lowercase();
        if wanted.is_empty() {
            return None;
        }
        self.apps.iter().find(|a| a.id.to_lowercase() == wanted).or_else(|| {
            self.apps.iter().find(|a| {
                a.wm_class.as_deref().is_some_and(|c| c.to_lowercase() == wanted)
                    || a.id.to_lowercase().rsplit('.').next() == Some(wanted.as_str())
                    || a.id.to_lowercase().ends_with(&format!(".{wanted}"))
            })
        })
    }

    fn window_icon(&self, window: &Window) -> icon::Handle {
        match self.app_for_window(window) {
            Some(app) => app.icon.clone(),
            None => named_icon(&[window.app_id.as_str(), "application-default", "application-x-executable"]),
        }
    }

    /// The pinned apps that exist, in their order.
    fn pinned_apps(&self) -> Vec<&AppEntry> {
        self.pinned
            .iter()
            .filter_map(|id| find_app(&self.apps, id))
            .collect()
    }

    /// As Windows recommends: the apps just installed or opened, the files
    /// just changed, the latest first.
    fn picks(&self) -> Vec<(i64, Pick<'_>)> {
        let t = texts();
        let now = now();
        let mut picks: Vec<(i64, Pick<'_>)> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (id, when) in &self.launched {
            if let Some(app) = self.apps.iter().find(|a| a.id == *id)
                && seen.insert(app.id.as_str())
            {
                picks.push((*when, Pick::App(app, ago(*when))));
            }
        }
        // Only what came after the first look, not all there was then.
        let oldest = self.apps.iter().map(|a| a.added).min().unwrap_or(0);
        let mut added: Vec<&AppEntry> = self
            .apps
            .iter()
            .filter(|a| a.added > oldest + 3600 && now - a.added < 14 * 86400)
            .collect();
        added.sort_by_key(|a| -a.added);
        for app in added.into_iter().take(4) {
            if seen.insert(app.id.as_str()) {
                picks.push((app.added, Pick::App(app, t.recently_added.into())));
            }
        }
        picks.extend(self.recents.iter().map(|f| (f.when, Pick::File(f))));
        picks.sort_by_key(|(when, _)| -when);
        picks
    }

    fn hits(&self) -> Vec<Hit> {
        let query = fold(self.query.trim());
        if query.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(u8, &str, Hit)> = self
            .apps
            .iter()
            .enumerate()
            .filter_map(|(i, app)| {
                let score = if app.folded_name.starts_with(&query) {
                    0
                } else if app.folded_name.split([' ', '-', '_', '.']).any(|w| w.starts_with(&query)) {
                    1
                } else if app.folded_name.contains(&query) {
                    2
                } else if app.haystack.contains(&query) {
                    3
                } else {
                    return None;
                };
                Some((score, app.name.as_str(), Hit::App(i)))
            })
            .collect();
        // The closest first: "Configurações" before "Configurações do NVIDIA".
        scored.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| a.1.chars().count().cmp(&b.1.chars().count()))
                .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
        });
        let mut hits: Vec<Hit> = scored.into_iter().map(|(_, _, hit)| hit).take(10).collect();
        hits.extend(
            self.windows
                .iter()
                .filter(|w| fold(&w.title).contains(&query) || fold(&w.app_id).contains(&query))
                .map(|w| Hit::Window(w.id.clone())),
        );
        hits.extend(
            self.recents
                .iter()
                .enumerate()
                .filter(|(_, f)| fold(&f.name).contains(&query))
                .map(|(i, _)| Hit::File(i))
                .take(8),
        );
        hits
    }

    // The parts.

    fn search_bar(&self, p: Palette) -> Element<'_, Message> {
        let style = move |focused: bool| widget::text_input::Appearance {
            background: Background::Color(p.field),
            border_radius: 18.0.into(),
            border_offset: None,
            border_width: 1.0,
            border_color: if focused { p.accent } else { p.border },
            label_color: p.text,
            placeholder_color: p.dim,
            selected_text_color: Color::WHITE,
            icon_color: Some(p.dim),
            text_color: Some(p.text),
            selected_fill: p.accent,
        };
        widget::search_input(texts().start_search, &self.query)
            .id(SEARCH.clone())
            .size(14)
            .padding([8, 12])
            .on_input(Message::Query)
            .on_submit(|_| Message::Submit)
            .width(Length::Fill)
            .style(theme::TextInput::Custom {
                active: Box::new(move |_| style(false)),
                error: Box::new(move |_| style(false)),
                hovered: Box::new(move |_| style(false)),
                focused: Box::new(move |_| style(true)),
                disabled: Box::new(move |_| style(false)),
            })
            .into()
    }

    /// A section's title, with its button at the right ("Show all ⌄").
    fn header<'a>(
        title: &'a str,
        action: Option<(String, &'static str, Message)>,
        p: Palette,
    ) -> Element<'a, Message> {
        let mut row = widget::row::with_capacity(3)
            .align_y(Alignment::Center)
            .push(widget::text::heading(title))
            .push(widget::space::horizontal());
        if let Some((label, arrow, message)) = action {
            let inner = widget::row::with_children(vec![
                widget::text::caption(label).into(),
                widget::icon::from_name(arrow).size(12).into(),
            ])
            .spacing(6)
            .align_y(Alignment::Center);
            row = row.push(
                button::custom(inner)
                    .padding([4, 10])
                    .class(pill_class(p))
                    .on_press(message),
            );
        }
        widget::container(row).height(Length::Fixed(28.0)).into()
    }

    fn back_header<'a>(title: &'a str, p: Palette) -> Element<'a, Message> {
        Self::header(
            title,
            Some((texts().back.into(), "go-previous-symbolic", Message::Page(Page::Home))),
            p,
        )
    }

    /// An app as the grid has it: the icon, and the name under it.
    fn app_tile<'a>(&'a self, app: &'a AppEntry, p: Palette) -> Element<'a, Message> {
        let content = widget::column::with_children(vec![
            widget::icon(app.icon.clone()).size(32).into(),
            widget::text::caption(ellipsize(&app.name, 12))
                .width(Length::Fill)
                .align_x(Alignment::Center)
                .wrapping(Wrapping::None)
                .into(),
        ])
        .spacing(8)
        .align_x(Alignment::Center)
        .width(Length::Fill);
        widget::mouse_area(
            button::custom(content)
                .padding([12, 0, 6, 0])
                .width(Length::Fill)
                .class(flat_class(p, false))
                .on_press(Message::Launch(app.id.clone())),
        )
        .on_right_press(Message::Menu(Some(Target::App(app.id.clone()))))
        .into()
    }

    fn tile_grid<'a>(&'a self, apps: &[&'a AppEntry], columns: usize, p: Palette) -> Element<'a, Message> {
        let mut grid = widget::column::with_capacity(apps.len() / columns + 1).spacing(4);
        for chunk in apps.chunks(columns) {
            let mut row = widget::row::with_capacity(columns).spacing(2);
            for app in chunk {
                row = row.push(self.app_tile(app, p));
            }
            for _ in chunk.len()..columns {
                row = row.push(widget::space::horizontal().width(Length::Fill));
            }
            grid = grid.push(row);
        }
        grid.into()
    }

    /// A row of the recommended or the open: icon, a title, a line under.
    fn card<'a>(
        icon: icon::Handle,
        title: String,
        subtitle: String,
        selected: bool,
        on_press: Message,
        on_right: Message,
        p: Palette,
    ) -> Element<'a, Message> {
        let text = widget::column::with_children(vec![
            widget::text::body(ellipsize(&title, 22)).size(13).wrapping(Wrapping::None).into(),
            widget::text::caption(ellipsize(&subtitle, 28))
                .class(theme::Text::Color(p.dim))
                .wrapping(Wrapping::None)
                .into(),
        ])
        .spacing(1);
        let content = widget::row::with_children(vec![
            widget::icon(icon).size(32).into(),
            widget::container(text).width(Length::Fill).clip(true).into(),
        ])
        .spacing(12)
        .align_y(Alignment::Center);
        widget::mouse_area(
            button::custom(content)
                .padding([8, 10])
                .width(Length::Fill)
                .class(flat_class(p, selected))
                .on_press(on_press),
        )
        .on_right_press(on_right)
        .into()
    }

    fn card_grid<'a>(cards: Vec<Element<'a, Message>>) -> Element<'a, Message> {
        let mut cards = cards.into_iter().peekable();
        let mut grid = widget::column::with_capacity(4).spacing(4);
        while cards.peek().is_some() {
            let mut row = widget::row::with_capacity(CARD_COLUMNS).spacing(8);
            for _ in 0..CARD_COLUMNS {
                row = row.push(match cards.next() {
                    Some(card) => card,
                    None => widget::space::horizontal().width(Length::Fill).into(),
                });
            }
            grid = grid.push(row);
        }
        grid.into()
    }

    fn pick_card<'a>(&'a self, pick: Pick<'a>, p: Palette) -> Element<'a, Message> {
        match pick {
            Pick::App(app, subtitle) => Self::card(
                app.icon.clone(),
                app.name.clone(),
                subtitle,
                false,
                Message::Launch(app.id.clone()),
                Message::Menu(Some(Target::App(app.id.clone()))),
                p,
            ),
            Pick::File(file) => Self::card(
                file_icon(&file.icons),
                file.name.clone(),
                ago(file.when),
                false,
                Message::OpenFile(file.path.clone()),
                Message::Menu(Some(Target::File(file.path.clone()))),
                p,
            ),
        }
    }

    fn window_card<'a>(&'a self, window: &'a Window, p: Palette) -> Element<'a, Message> {
        let subtitle = self
            .app_for_window(window)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| window.app_id.clone());
        Self::card(
            self.window_icon(window),
            window.title.clone(),
            subtitle,
            window.active,
            Message::Activate(window.id.clone()),
            Message::Menu(Some(Target::Window(window.id.clone()))),
            p,
        )
    }

    fn home(&self, p: Palette) -> Element<'_, Message> {
        let t = texts();
        let mut sections: Vec<Element<'_, Message>> = Vec::new();

        // Pinned: two rows, all of them on "Show all".
        let pinned = self.pinned_apps();
        let more = pinned.len() > PINNED_COLUMNS * PINNED_ROWS;
        sections.push(Self::header(
            t.pinned,
            more.then(|| {
                if self.pinned_expanded {
                    (t.show_less.into(), "go-up-symbolic", Message::ExpandPinned(false))
                } else {
                    (t.show_all.into(), "go-down-symbolic", Message::ExpandPinned(true))
                }
            }),
            p,
        ));
        if pinned.is_empty() {
            sections.push(
                widget::container(widget::text::body(t.pin_hint).class(theme::Text::Color(p.dim)))
                    .padding([16, 0])
                    .center_x(Length::Fill)
                    .into(),
            );
        } else {
            let shown = if self.pinned_expanded { pinned.len() } else { PINNED_COLUMNS * PINNED_ROWS };
            let pinned: Vec<&AppEntry> = pinned.into_iter().take(shown).collect();
            sections.push(self.tile_grid(&pinned, PINNED_COLUMNS, p));
        }

        // The open windows.
        if !self.windows.is_empty() {
            sections.push(widget::space::vertical().height(Length::Fixed(12.0)).into());
            sections.push(Self::header(t.open_windows, None, p));
            let cards = self.windows.iter().map(|w| self.window_card(w, p)).collect();
            sections.push(Self::card_grid(cards));
        }

        // Recommended.
        let picks = self.picks();
        sections.push(widget::space::vertical().height(Length::Fixed(12.0)).into());
        sections.push(Self::header(
            t.recommended,
            (picks.len() > CARD_COLUMNS * CARD_ROWS).then(|| {
                (t.show_all.into(), "go-next-symbolic", Message::Page(Page::Recommended))
            }),
            p,
        ));
        if picks.is_empty() {
            sections.push(
                widget::container(widget::text::body(t.no_recommended).class(theme::Text::Color(p.dim)))
                    .padding([16, 0])
                    .center_x(Length::Fill)
                    .into(),
            );
        } else {
            let cards = picks
                .into_iter()
                .take(CARD_COLUMNS * CARD_ROWS)
                .map(|(_, pick)| self.pick_card(pick, p))
                .collect();
            sections.push(Self::card_grid(cards));
        }

        // All, by category, in a grid or in a list.
        sections.push(widget::space::vertical().height(Length::Fixed(12.0)).into());
        let view_name = t.views[self.all_view as usize];
        sections.push(Self::header(
            t.all,
            Some((format!("{}: {view_name}", t.view_as), "go-down-symbolic", Message::NextView)),
            p,
        ));
        sections.push(match self.all_view {
            AllView::Category => self.categories(p),
            AllView::Grid => self.all_grid(p),
            AllView::List => self.all_list(p),
        });

        widget::scrollable(
            widget::column::with_children(sections)
                .spacing(8)
                .padding([4, SIDE, 24, SIDE]),
        )
        .id(HOME.clone())
        .height(Length::Fill)
        .into()
    }

    fn recommended_page(&self, p: Palette) -> Element<'_, Message> {
        let cards = self
            .picks()
            .into_iter()
            .take(30)
            .map(|(_, pick)| self.pick_card(pick, p))
            .collect();
        widget::column::with_children(vec![
            widget::container(Self::back_header(texts().recommended, p))
                .padding([4, SIDE, 8, SIDE])
                .into(),
            widget::scrollable(widget::container(Self::card_grid(cards)).padding([0, SIDE, 24, SIDE]))
                .height(Length::Fill)
                .into(),
        ])
        .into()
    }

    /// The apps by category, as folders: four icons in a card, the name
    /// under it; a click opens it.
    fn categories(&self, p: Palette) -> Element<'_, Message> {
        let t = texts();
        let mut folders: Vec<Element<'_, Message>> = Vec::new();
        for (category, name) in t.categories.iter().enumerate() {
            let apps: Vec<&AppEntry> = self.apps.iter().filter(|a| a.category == category).collect();
            if apps.is_empty() {
                continue;
            }
            let icon = |i: usize| -> Element<'_, Message> {
                match apps.get(i) {
                    Some(app) => widget::icon(app.icon.clone()).size(32).into(),
                    None => widget::space::horizontal().width(Length::Fixed(32.0)).into(),
                }
            };
            let icons = widget::column::with_children(vec![
                widget::row::with_children(vec![icon(0), icon(1)]).spacing(20).into(),
                widget::row::with_children(vec![icon(2), icon(3)]).spacing(20).into(),
            ])
            .spacing(16);
            let card = button::custom(widget::container(icons).center(Length::Fill))
                .width(Length::Fill)
                .height(Length::Fixed(124.0))
                .class(folder_class(p))
                .on_press(Message::Menu(Some(Target::Category(category))));
            folders.push(
                widget::column::with_children(vec![
                    card.into(),
                    widget::text::caption(*name)
                        .width(Length::Fill)
                        .align_x(Alignment::Center)
                        .into(),
                ])
                .spacing(8)
                .width(Length::Fill)
                .into(),
            );
        }
        let mut folders = folders.into_iter().peekable();
        let mut grid = widget::column::with_capacity(3).spacing(16);
        while folders.peek().is_some() {
            let mut row = widget::row::with_capacity(CATEGORY_COLUMNS).spacing(12);
            for _ in 0..CATEGORY_COLUMNS {
                row = row.push(match folders.next() {
                    Some(folder) => folder,
                    None => widget::space::horizontal().width(Length::Fill).into(),
                });
            }
            grid = grid.push(row);
        }
        grid.into()
    }

    fn letter(app: &AppEntry) -> char {
        app.folded_name
            .chars()
            .next()
            .map_or('#', |c| if c.is_alphabetic() { c.to_ascii_uppercase() } else { '#' })
    }

    fn all_grid(&self, p: Palette) -> Element<'_, Message> {
        let mut column = widget::column::with_capacity(60).spacing(4);
        let apps: Vec<&AppEntry> = self.apps.iter().collect();
        for group in apps.chunk_by(|a, b| Self::letter(a) == Self::letter(b)) {
            column = column.push(
                widget::container(widget::text::heading(Self::letter(group[0]).to_string()))
                    .padding([8, 8, 0, 8]),
            );
            column = column.push(self.tile_grid(group, PINNED_COLUMNS, p));
        }
        column.into()
    }

    fn all_list(&self, p: Palette) -> Element<'_, Message> {
        let mut column = widget::column::with_capacity(self.apps.len() + 30).spacing(2);
        let mut letter = None;
        for app in self.apps.iter() {
            let first = Self::letter(app);
            if letter != Some(first) {
                letter = Some(first);
                column = column.push(
                    widget::container(widget::text::heading(first.to_string())).padding([10, 8, 4, 8]),
                );
            }
            column = column.push(self.app_row(app, false, p));
        }
        column.into()
    }

    fn app_row<'a>(&'a self, app: &'a AppEntry, selected: bool, p: Palette) -> Element<'a, Message> {
        let content = widget::row::with_children(vec![
            widget::icon(app.icon.clone()).size(24).into(),
            widget::text::body(app.name.as_str()).width(Length::Fill).into(),
        ])
        .spacing(14)
        .align_y(Alignment::Center);
        widget::mouse_area(
            button::custom(content)
                .padding([7, 10])
                .width(Length::Fill)
                .class(flat_class(p, selected))
                .on_press(Message::Launch(app.id.clone())),
        )
        .on_right_press(Message::Menu(Some(Target::App(app.id.clone()))))
        .into()
    }

    /// Results at the left, the chosen one at the right with what can be
    /// done with it, as Windows' search has them.
    fn results(&self, p: Palette) -> Element<'_, Message> {
        let t = texts();
        let hits = self.hits();
        let Some(chosen) = hits.get(self.selected.min(hits.len().saturating_sub(1))) else {
            return widget::container(widget::text::body(t.no_results).class(theme::Text::Color(p.dim)))
                .padding([40, 0])
                .center_x(Length::Fill)
                .into();
        };
        let mut list = widget::column::with_capacity(hits.len() + 4).spacing(2);
        let mut section = "";
        for (i, hit) in hits.iter().enumerate() {
            let selected = i == self.selected;
            let (heading, icon, title, message) = match hit {
                Hit::App(index) => {
                    let app = &self.apps[*index];
                    (t.apps, app.icon.clone(), app.name.clone(), Message::Launch(app.id.clone()))
                }
                Hit::Window(id) => {
                    let Some(window) = self.windows.iter().find(|w| w.id == *id) else { continue };
                    (t.open_windows, self.window_icon(window), window.title.clone(), Message::Activate(id.clone()))
                }
                Hit::File(index) => {
                    let file = &self.recents[*index];
                    (t.documents, file_icon(&file.icons), file.name.clone(), Message::OpenFile(file.path.clone()))
                }
            };
            let heading = if i == 0 { t.best_match } else { heading };
            if heading != section {
                section = heading;
                list = list.push(
                    widget::container(widget::text::heading(heading)).padding([if i == 0 { 0 } else { 10 }, 10, 4, 10]),
                );
            }
            let size = if i == 0 { 32 } else { 20 };
            let content = widget::row::with_children(vec![
                widget::icon(icon).size(size).into(),
                widget::text::body(title).wrapping(Wrapping::None).width(Length::Fill).into(),
            ])
            .spacing(12)
            .align_y(Alignment::Center);
            list = list.push(
                button::custom(widget::container(content).clip(true))
                    .padding(if i == 0 { [10, 10] } else { [7, 10] })
                    .width(Length::Fill)
                    .class(flat_class(p, selected))
                    .on_press(message),
            );
        }

        // The chosen one, large, and its actions.
        let (icon, title, kind, actions): (icon::Handle, String, String, Vec<(String, Message)>) = match chosen {
            Hit::App(index) => {
                let app = &self.apps[*index];
                let pinned = self.pinned.iter().any(|id| *id == app.id);
                let mut actions = vec![(t.open_app.to_owned(), Message::Launch(app.id.clone()))];
                actions.extend(app.actions.iter().map(|(name, exec)| {
                    (name.clone(), Message::LaunchAction(app.id.clone(), exec.clone()))
                }));
                actions.push((
                    if pinned { t.unpin_start } else { t.pin_start }.to_owned(),
                    Message::TogglePin(app.id.clone()),
                ));
                (app.icon.clone(), app.name.clone(), t.app_kind.to_owned(), actions)
            }
            Hit::Window(id) => {
                let window = self.windows.iter().find(|w| w.id == *id);
                (
                    window.map(|w| self.window_icon(w)).unwrap_or_else(|| named_icon(&["application-default"])),
                    window.map(|w| w.title.clone()).unwrap_or_default(),
                    t.window_kind.to_owned(),
                    vec![
                        (t.switch_to.to_owned(), Message::Activate(id.clone())),
                        (t.close_window.to_owned(), Message::CloseWindow(id.clone())),
                    ],
                )
            }
            Hit::File(index) => {
                let file = &self.recents[*index];
                (
                    file_icon(&file.icons),
                    file.name.clone(),
                    format!("{} · {}", t.file_kind, ago(file.when)),
                    vec![
                        (t.open_app.to_owned(), Message::OpenFile(file.path.clone())),
                        (t.open_location.to_owned(), Message::OpenLocation(file.path.clone())),
                    ],
                )
            }
        };
        let mut detail = widget::column::with_capacity(actions.len() + 4)
            .push(widget::icon(icon).size(64))
            .push(
                widget::text::title4(title)
                    .align_x(Alignment::Center)
                    .width(Length::Fill),
            )
            .push(widget::text::caption(kind).class(theme::Text::Color(p.dim)))
            .push(widget::space::vertical().height(Length::Fixed(8.0)))
            .spacing(6)
            .padding([24, 12])
            .align_x(Alignment::Center);
        for (label, message) in actions {
            let row = widget::row::with_children(vec![
                widget::icon::from_name("go-next-symbolic").size(12).into(),
                widget::text::body(label).into(),
            ])
            .spacing(12)
            .align_y(Alignment::Center);
            detail = detail.push(
                button::custom(row)
                    .padding([8, 12])
                    .width(Length::Fill)
                    .class(flat_class(p, false))
                    .on_press(message),
            );
        }
        let detail = widget::container(detail)
            .width(Length::FillPortion(5))
            .height(Length::Fill)
            .class(theme::Container::custom(move |_| widget::container::Style {
                background: Some(Background::Color(p.card)),
                border: iced::Border {
                    radius: 8.0.into(),
                    width: 1.0,
                    color: p.border,
                },
                ..Default::default()
            }));
        widget::row::with_children(vec![
            widget::scrollable(list).width(Length::FillPortion(6)).height(Length::Fill).into(),
            detail.into(),
        ])
        .spacing(16)
        .padding([4, SIDE, 20, SIDE])
        .into()
    }

    fn footer(&self, p: Palette) -> Element<'_, Message> {
        let s = strings();
        let t = texts();
        // The photo, if there is one; else the initial, on the accent.
        let avatar: Element<'_, Message> = match &self.avatar {
            Some(path) => widget::icon(icon::from_path(path.clone())).size(32).into(),
            None => widget::container(
                widget::text::body(
                    self.user.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default(),
                )
                .class(theme::Text::Color(Color::WHITE)),
            )
            .center(Length::Fixed(32.0))
            .class(theme::Container::custom(move |_| widget::container::Style {
                background: Some(Background::Color(p.accent)),
                border: iced::Border {
                    radius: 16.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }))
            .into(),
        };
        let user = widget::row::with_children(vec![avatar, widget::text::body(self.user.as_str()).into()])
            .spacing(12)
            .align_y(Alignment::Center);
        let round = |name: &'static str, message: Message| -> Element<'_, Message> {
            button::custom(widget::icon::from_name(name).size(16))
                .padding(10)
                .class(flat_class(p, false))
                .on_press(message)
                .into()
        };
        let row = widget::row::with_children(vec![
            button::custom(user)
                .padding([6, 12, 6, 6])
                .class(flat_class(p, false))
                .on_press(Message::Menu(Some(Target::User)))
                .into(),
            widget::space::horizontal().into(),
            widget::tooltip(
                round("folder-symbolic", Message::Spawn("xdg-open", vec![std::env::var("HOME").unwrap_or_default()])),
                widget::text::body(t.files),
                widget::tooltip::Position::Top,
            )
            .into(),
            widget::tooltip(
                round("emblem-system-symbolic", Message::Spawn("cosmic-settings", Vec::new())),
                widget::text::body(s.settings),
                widget::tooltip::Position::Top,
            )
            .into(),
            widget::tooltip(
                round("system-shutdown-symbolic", Message::Menu(Some(Target::Power))),
                widget::text::body(s.power_off),
                widget::tooltip::Position::Top,
            )
            .into(),
        ])
        .spacing(4)
        .align_y(Alignment::Center)
        .padding([0, SIDE - 12]);
        widget::container(row)
            .width(Length::Fill)
            .height(Length::Fixed(FOOTER))
            .align_y(Alignment::Center)
            .class(theme::Container::custom(move |_| widget::container::Style {
                background: Some(Background::Color(p.footer)),
                border: iced::Border {
                    radius: [0.0, 0.0, 8.0, 8.0].into(),
                    width: 0.0,
                    color: Color::TRANSPARENT,
                },
                ..Default::default()
            }))
            .into()
    }

    /// The little menus: an app's, a window's, a file's (right click), a
    /// category's apps, the user's and the power button's.
    fn popup_menu(&self, target: &Target, p: Palette) -> Element<'_, Message> {
        let s = strings();
        let t = texts();
        let item = |icon_name: Option<&'static str>, label: String, message: Message| -> Element<'_, Message> {
            let mut row = widget::row::with_capacity(2).spacing(12).align_y(Alignment::Center);
            if let Some(name) = icon_name {
                row = row.push(widget::icon::from_name(name).size(16));
            }
            button::custom(row.push(widget::text::body(label)))
                .padding([8, 12])
                .width(Length::Fill)
                .class(flat_class(p, false))
                .on_press(message)
                .into()
        };
        let mut items: Vec<Element<'_, Message>> = Vec::new();
        let mut title = None;
        let mut width = 260.0;
        // Where it shows: by the button that opened it, or in the middle.
        let mut place = (Alignment::Center, Alignment::Center);
        match target {
            Target::App(id) => {
                let Some(app) = self.apps.iter().find(|a| a.id == *id) else {
                    return widget::text("").into();
                };
                title = Some(app.name.clone());
                items.push(item(Some("media-playback-start-symbolic"), t.open_app.into(), Message::Launch(app.id.clone())));
                for (name, exec) in &app.actions {
                    items.push(item(None, name.clone(), Message::LaunchAction(app.id.clone(), exec.clone())));
                }
                let pinned = self.pinned.iter().any(|p| *p == app.id);
                items.push(item(
                    Some(if pinned { "view-pin-symbolic" } else { "pin-symbolic" }),
                    if pinned { t.unpin_start } else { t.pin_start }.into(),
                    Message::TogglePin(app.id.clone()),
                ));
            }
            Target::Window(id) => {
                let Some(window) = self.windows.iter().find(|w| w.id == *id) else {
                    return widget::text("").into();
                };
                title = Some(window.title.clone());
                items.push(item(Some("window-restore-symbolic"), t.switch_to.into(), Message::Activate(window.id.clone())));
                if let Some(app) = self.app_for_window(window) {
                    items.push(item(Some("window-new-symbolic"), t.new_window.into(), Message::Launch(app.id.clone())));
                }
                items.push(item(Some("window-close-symbolic"), t.close_window.into(), Message::CloseWindow(window.id.clone())));
            }
            Target::File(path) => {
                title = Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned());
                items.push(item(Some("document-open-symbolic"), t.open_app.into(), Message::OpenFile(path.clone())));
                items.push(item(Some("folder-open-symbolic"), t.open_location.into(), Message::OpenLocation(path.clone())));
            }
            Target::Category(category) => {
                title = t.categories.get(*category).map(|n| (*n).to_owned());
                let apps: Vec<&AppEntry> = self.apps.iter().filter(|a| a.category == *category).collect();
                width = 520.0;
                items.push(
                    widget::container(
                        widget::scrollable(self.tile_grid(&apps, 5, p)).height(Length::Shrink),
                    )
                    .max_height(HEIGHT * 0.6)
                    .into(),
                );
            }
            Target::User => {
                place = (Alignment::Start, Alignment::End);
                items.push(item(Some("system-users-symbolic"), t.account.into(), Message::Spawn("cosmic-settings", vec!["users".into()])));
                items.push(item(Some("system-lock-screen-symbolic"), s.lock.into(), Message::Spawn("loginctl", vec!["lock-session".into()])));
                items.push(item(Some("system-log-out-symbolic"), s.log_out.into(), Message::Spawn("cosmic-osd", vec!["log-out".into()])));
            }
            Target::Power => {
                place = (Alignment::End, Alignment::End);
                width = 200.0;
                items.push(item(Some("system-lock-screen-symbolic"), s.lock.into(), Message::Spawn("loginctl", vec!["lock-session".into()])));
                items.push(item(Some("system-suspend-symbolic"), s.suspend.into(), Message::Spawn("systemctl", vec!["suspend".into()])));
                items.push(item(Some("system-shutdown-symbolic"), s.power_off.into(), Message::Spawn("cosmic-osd", vec!["shutdown".into()])));
                items.push(item(Some("system-reboot-symbolic"), s.restart.into(), Message::Spawn("cosmic-osd", vec!["restart".into()])));
            }
        }
        let mut column = widget::column::with_capacity(items.len() + 1).spacing(2).padding(4);
        if let Some(title) = title {
            column = column.push(
                widget::container(widget::text::heading(title).wrapping(Wrapping::None))
                    .padding([8, 12])
                    .clip(true),
            );
        }
        for item in items {
            column = column.push(item);
        }
        let card = widget::container(column)
            .width(Length::Fixed(width))
            .class(theme::Container::custom(move |_| widget::container::Style {
                background: Some(Background::Color(p.popup)),
                text_color: Some(p.text),
                icon_color: Some(p.text),
                border: iced::Border {
                    radius: 8.0.into(),
                    width: 1.0,
                    color: p.border,
                },
                shadow: iced::Shadow {
                    color: Color::from_rgba(0.0, 0.0, 0.0, 0.4),
                    offset: iced::Vector::new(0.0, 4.0),
                    blur_radius: 16.0,
                },
                ..Default::default()
            }));
        // A folder dims the menu behind it, as on Windows; the others don't.
        let dim = matches!(target, Target::Category(_));
        let backdrop = widget::mouse_area(
            widget::container(widget::space::horizontal())
                .width(Length::Fill)
                .height(Length::Fill)
                .class(theme::Container::custom(move |_| widget::container::Style {
                    background: dim.then(|| Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.45))),
                    border: iced::Border {
                        radius: 8.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })),
        )
        .on_press(Message::Menu(None))
        .on_right_press(Message::Menu(None));
        let (x, y) = place;
        let bottom = if y == Alignment::End { FOOTER - 4.0 } else { 0.0 };
        let side = if x == Alignment::Center { 0.0 } else { SIDE as f32 - 16.0 };
        iced::widget::stack(vec![
            backdrop.into(),
            widget::container(widget::mouse_area(card).on_press(Message::Nothing))
                .width(Length::Fill)
                .height(Length::Fill)
                .padding(iced::Padding { top: 0.0, right: side, bottom, left: side })
                .align_x(x)
                .align_y(y)
                .into(),
        ])
        .into()
    }
}

/// Windows 11's own colors, dark or light as the theme is; the accent is
/// the theme's.
#[derive(Clone, Copy)]
struct Palette {
    bg: Color,
    popup: Color,
    border: Color,
    hover: Color,
    selected: Color,
    card: Color,
    folder: Color,
    text: Color,
    dim: Color,
    footer: Color,
    field: Color,
    accent: Color,
}

fn palette(theme: &cosmic::Theme) -> Palette {
    let cosmic = theme.cosmic();
    let accent: Color = cosmic.accent_color().into();
    let rgba = |r: u8, g: u8, b: u8, a: f32| Color::from_rgba8(r, g, b, a);
    if cosmic.is_dark {
        Palette {
            bg: rgba(36, 36, 36, 1.0),
            popup: rgba(44, 44, 44, 1.0),
            border: rgba(255, 255, 255, 0.08),
            hover: rgba(255, 255, 255, 0.06),
            selected: rgba(255, 255, 255, 0.09),
            card: rgba(43, 43, 43, 1.0),
            folder: rgba(255, 255, 255, 0.05),
            text: rgba(255, 255, 255, 1.0),
            dim: rgba(255, 255, 255, 0.6),
            footer: rgba(28, 28, 28, 1.0),
            field: rgba(28, 28, 28, 1.0),
            accent,
        }
    } else {
        Palette {
            bg: rgba(243, 243, 243, 1.0),
            popup: rgba(249, 249, 249, 1.0),
            border: rgba(0, 0, 0, 0.09),
            hover: rgba(0, 0, 0, 0.04),
            selected: rgba(0, 0, 0, 0.06),
            card: rgba(250, 250, 250, 1.0),
            folder: rgba(255, 255, 255, 0.75),
            text: rgba(26, 26, 26, 1.0),
            dim: rgba(0, 0, 0, 0.6),
            footer: rgba(236, 236, 236, 1.0),
            field: rgba(255, 255, 255, 1.0),
            accent,
        }
    }
}

fn button_class(style: impl Fn(bool, bool) -> button::Style + Clone + 'static) -> theme::Button {
    let (active, hovered, pressed) = (style.clone(), style.clone(), style.clone());
    theme::Button::Custom {
        active: Box::new(move |_, _| active(false, false)),
        disabled: Box::new(move |_| style(false, false)),
        hovered: Box::new(move |_, _| hovered(true, false)),
        pressed: Box::new(move |_, _| pressed(true, true)),
    }
}

/// The lists' and the grid's: nothing until the mouse is on it.
fn flat_class(p: Palette, selected: bool) -> theme::Button {
    button_class(move |hovered, pressed| button::Style {
        background: (hovered || selected).then(|| {
            Background::Color(if pressed { p.card } else if hovered { p.hover } else { p.selected })
        }),
        border_radius: 4.0.into(),
        text_color: Some(p.text),
        icon_color: Some(p.text),
        ..button::Style::new()
    })
}

/// "Show all": a quiet button, a shade on the menu.
fn pill_class(p: Palette) -> theme::Button {
    button_class(move |hovered, _| button::Style {
        background: Some(Background::Color(if hovered { p.selected } else { p.folder })),
        border_radius: 4.0.into(),
        border_width: 1.0,
        border_color: p.border,
        text_color: Some(p.text),
        icon_color: Some(p.text),
        ..button::Style::new()
    })
}

/// The categories' cards.
fn folder_class(p: Palette) -> theme::Button {
    button_class(move |hovered, _| button::Style {
        background: Some(Background::Color(if hovered { p.selected } else { p.folder })),
        border_radius: 8.0.into(),
        border_width: 1.0,
        border_color: p.border,
        text_color: Some(p.text),
        icon_color: Some(p.text),
        ..button::Style::new()
    })
}

fn named_icon(names: &[&str]) -> icon::Handle {
    icon::from_name(names[0])
        .size(64)
        .fallback(Some(icon::IconFallback::Names(
            names[1..].iter().map(|n| (*n).to_owned().into()).collect(),
        )))
        .handle()
}

fn file_icon(names: &[String]) -> icon::Handle {
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    named_icon(&names)
}

/// Cut to `max` characters with "…", as Windows does names too long.
fn ellipsize(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let cut: String = text.chars().take(max - 1).collect();
        format!("{}…", cut.trim_end())
    }
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// "5 min ago", "3h ago", "Yesterday", as Windows says it.
fn ago(when: i64) -> String {
    let t = texts();
    let seconds = (now() - when).max(0);
    match seconds {
        0..60 => t.just_now.to_owned(),
        60..3600 => t.minutes_ago.replace("{}", &(seconds / 60).to_string()),
        3600..86400 => t.hours_ago.replace("{}", &(seconds / 3600).to_string()),
        86400..172800 => t.yesterday.to_owned(),
        _ => t.days_ago.replace("{}", &(seconds / 86400).to_string()),
    }
}

/// Which of `texts().categories` an app goes in, by its desktop entry.
fn category_of(categories: &[&str]) -> usize {
    let has = |names: &[&str]| categories.iter().any(|c| names.iter().any(|n| c.eq_ignore_ascii_case(n)));
    if has(&["Game"]) {
        5
    } else if has(&["Development", "IDE"]) {
        6
    } else if has(&["Chat", "InstantMessaging", "Email", "IRCClient", "Telephony", "VideoConference", "Feed"]) {
        1
    } else if has(&["Office", "WordProcessor", "Spreadsheet", "Presentation", "Calendar", "ContactManagement", "TextEditor", "Viewer"]) {
        0
    } else if has(&["WebBrowser", "Network", "FileTransfer", "P2P"]) {
        2
    } else if has(&["Graphics", "2DGraphics", "3DGraphics", "Photography", "VectorGraphics", "RasterGraphics"]) {
        3
    } else if has(&["AudioVideo", "Audio", "Video", "Player", "Music", "Recorder"]) {
        4
    } else if has(&["System", "Settings", "PackageManager", "Monitor", "TerminalEmulator", "HardwareSettings"]) {
        7
    } else if has(&["Utility", "Accessories", "FileManager", "Archiving", "Compression"]) {
        8
    } else {
        9
    }
}

/// The user's photo, where AccountsService or `~/.face` keep it.
fn avatar() -> Option<PathBuf> {
    let user = std::env::var("USER").unwrap_or_default();
    let home = std::env::var("HOME").unwrap_or_default();
    [
        PathBuf::from(format!("/var/lib/AccountsService/icons/{user}")),
        Path::new(&home).join(".face"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// The files changed lately: the ones apps say they opened
/// (`recently-used.xbel`, also the Flatpaks'), and the newest in the
/// usual folders, which COSMIC's apps don't say.
fn load_recents() -> Vec<RecentFile> {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let mut found: HashMap<String, RecentFile> = HashMap::new();
    let mut add = |file: RecentFile| {
        if found.get(&file.path).is_none_or(|old| old.when < file.when) {
            found.insert(file.path.clone(), file);
        }
    };

    let mut xbels = vec![home.join(".local/share/recently-used.xbel")];
    if let Ok(apps) = std::fs::read_dir(home.join(".var/app")) {
        xbels.extend(apps.flatten().map(|a| a.path().join("data/recently-used.xbel")));
    }
    for xbel in xbels {
        let Ok(text) = std::fs::read_to_string(&xbel) else { continue };
        for bookmark in text.split("<bookmark ").skip(1) {
            let attr = |name: &str| {
                let start = bookmark.find(&format!("{name}=\""))? + name.len() + 2;
                let end = bookmark[start..].find('"')? + start;
                Some(&bookmark[start..end])
            };
            let Some(path) = attr("href").and_then(|h| h.strip_prefix("file://")).map(unescape) else {
                continue;
            };
            let when = ["visited", "modified", "added"]
                .iter()
                .filter_map(|a| attr(a))
                .filter_map(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
                .map(|d| d.timestamp())
                .max()
                .unwrap_or(0);
            let mime = bookmark
                .find("mime-type type=\"")
                .map(|i| &bookmark[i + 16..])
                .and_then(|rest| rest.split('"').next());
            if Path::new(&path).is_file() {
                add(recent(&path, when, mime));
            }
        }
    }

    // The newest in the user's folders, and a level down (Screenshots…).
    let cutoff = now() - 30 * 86400;
    for dir in user_dirs(&home) {
        let mut stack = vec![(dir, 0)];
        while let Some((dir, depth)) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if entry.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                let Ok(meta) = entry.metadata() else { continue };
                if meta.is_dir() {
                    if depth < 1 {
                        stack.push((path, depth + 1));
                    }
                    continue;
                }
                let when = meta
                    .modified()
                    .ok()
                    .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs() as i64);
                if when > cutoff {
                    add(recent(&path.to_string_lossy(), when, None));
                }
            }
        }
    }
    let mut files: Vec<RecentFile> = found.into_values().collect();
    files.sort_by_key(|f| -f.when);
    files.truncate(40);
    files
}

fn recent(path: &str, when: i64, mime: Option<&str>) -> RecentFile {
    let name = Path::new(path)
        .file_name()
        .map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned());
    let extension = Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let generic = match extension.as_str() {
        "pdf" => "application-pdf",
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "svg" | "bmp" | "heic" | "avif" => "image-x-generic",
        "mp4" | "mkv" | "webm" | "mov" | "avi" => "video-x-generic",
        "mp3" | "flac" | "ogg" | "opus" | "wav" | "m4a" => "audio-x-generic",
        "odt" | "doc" | "docx" | "rtf" => "x-office-document",
        "ods" | "xls" | "xlsx" | "csv" => "x-office-spreadsheet",
        "odp" | "ppt" | "pptx" => "x-office-presentation",
        "zip" | "tar" | "gz" | "xz" | "zst" | "7z" | "rar" | "flatpakref" | "deb" | "rpm" => "package-x-generic",
        "html" | "htm" => "text-html",
        "sh" | "py" | "rs" | "js" | "ts" | "c" | "cpp" | "h" => "text-x-script",
        _ => "text-x-generic",
    };
    let mut icons = Vec::new();
    if let Some(mime) = mime {
        icons.push(mime.replace('/', "-"));
    }
    icons.push(generic.to_owned());
    icons.push("text-x-generic".to_owned());
    RecentFile { path: path.to_owned(), name, icons, when }
}

/// "%20" back to " ", as the bookmarks keep paths.
fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = text.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Desktop, Downloads, Documents, Pictures, Videos and Music, as
/// `user-dirs.dirs` names them in the user's language.
fn user_dirs(home: &Path) -> Vec<PathBuf> {
    let config = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join(".config"));
    let text = std::fs::read_to_string(config.join("user-dirs.dirs")).unwrap_or_default();
    let mut dirs: Vec<PathBuf> = text
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            let wanted = ["DESKTOP", "DOWNLOAD", "DOCUMENTS", "PICTURES", "VIDEOS", "MUSIC"];
            if !wanted.iter().any(|w| key.trim() == format!("XDG_{w}_DIR")) {
                return None;
            }
            let value = value.trim().trim_matches('"').replace("$HOME", &home.to_string_lossy());
            let path = PathBuf::from(value);
            (path != home).then_some(path)
        })
        .collect();
    if dirs.is_empty() {
        dirs = ["Desktop", "Downloads", "Documents", "Pictures"].iter().map(|d| home.join(d)).collect();
    }
    dirs
}

/// Lower case, without accents, to find "configuracoes" in "Configurações".
fn fold(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            c => c,
        })
        .collect()
}

/// The languages to read names in, "pt_BR" then "pt": without the
/// ".UTF-8" that LANG has, which no desktop entry is translated to.
fn locales() -> Vec<String> {
    let mut locales = Vec::new();
    let language = std::env::var("LANGUAGE").unwrap_or_default();
    let others = ["LC_ALL", "LC_MESSAGES", "LANG"].map(|v| std::env::var(v).unwrap_or_default());
    for locale in language.split(':').chain(others.iter().map(String::as_str)) {
        let locale = locale.split(['.', '@']).next().unwrap_or("");
        if locale.is_empty() || locale == "C" || locale == "POSIX" {
            continue;
        }
        for candidate in [locale, locale.split('_').next().unwrap_or(locale)] {
            if !locales.iter().any(|l| l == candidate) {
                locales.push(candidate.to_owned());
            }
        }
    }
    locales
}

fn find_app<'a>(apps: &'a [AppEntry], id: &str) -> Option<&'a AppEntry> {
    let wanted = id.to_lowercase();
    apps.iter().find(|a| a.id.to_lowercase() == wanted).or_else(|| {
        apps.iter().find(|a| {
            a.wm_class.as_deref().is_some_and(|c| c.to_lowercase() == wanted)
                || a.id.to_lowercase().ends_with(&format!(".{wanted}"))
        })
    })
}

fn load_apps() -> Vec<AppEntry> {
    let locales = locales();
    let desktop = fde::current_desktop();
    let mut seen = std::collections::HashSet::new();
    let mut apps: Vec<AppEntry> = fde::Iter::new(fde::default_paths())
        .filter_map(|path| fde::DesktopEntry::from_path(path, Some(&locales)).ok())
        .filter(|de| seen.insert(de.id().to_owned()))
        .filter(|de| !de.hidden() && !de.no_display() && de.exec().is_some())
        .filter(|de| de.type_().is_none_or(|t| t == "Application"))
        .filter(|de| {
            let Some(desktop) = &desktop else { return true };
            let shown = de
                .only_show_in()
                .is_none_or(|only| only.iter().any(|o| desktop.iter().any(|d| d.eq_ignore_ascii_case(o))));
            let hidden = de
                .not_show_in()
                .is_some_and(|not| not.iter().any(|n| desktop.iter().any(|d| d.eq_ignore_ascii_case(n))));
            shown && !hidden
        })
        .map(|de| {
            let name = de.name(&locales).map(|n| n.to_string()).unwrap_or_else(|| de.appid.clone());
            let mut haystack = vec![name.clone(), de.appid.clone()];
            haystack.extend(de.generic_name(&locales).map(|g| g.to_string()));
            haystack.extend(de.comment(&locales).map(|c| c.to_string()));
            haystack.extend(de.keywords(&locales).unwrap_or_default().into_iter().map(|k| k.to_string()));
            haystack.extend(de.exec().map(|e| e.split_whitespace().next().unwrap_or("").to_string()));
            let icon = fde::IconSource::from_unknown(de.icon().unwrap_or(&de.appid)).as_cosmic_icon();
            let actions = de
                .actions()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|action| {
                    let name = de.action_entry_localized(action, "Name", &locales)?;
                    let exec = de.action_entry(action, "Exec")?;
                    Some((name.to_string(), exec.to_string()))
                })
                .collect();
            AppEntry {
                id: de.appid.clone(),
                folded_name: fold(&name),
                haystack: fold(&haystack.join(" ")),
                name,
                wm_class: de.startup_wm_class().map(str::to_owned),
                exec: de.exec().map(str::to_owned),
                terminal: de.terminal(),
                icon,
                actions,
                category: category_of(&de.categories().unwrap_or_default()),
                added: std::fs::metadata(&de.path)
                    .and_then(|m| m.created().or_else(|_| m.modified()))
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs() as i64),
            }
        })
        .collect();
    apps.sort_by(|a, b| a.folded_name.cmp(&b.folded_name));
    apps
}

fn config() -> Option<Config> {
    Config::new(APP_ID, 1).ok()
}

/// What was pinned; the first time, what the dock has, and Files, the
/// Terminal and Settings.
fn load_pinned() -> Vec<String> {
    if let Some(pinned) = config().and_then(|c| c.get::<Vec<String>>("pinned").ok()) {
        return pinned;
    }
    let mut pinned: Vec<String> = Config::new("com.system76.CosmicAppList", 1)
        .ok()
        .and_then(|c| c.get::<Vec<String>>("favorites").ok())
        .unwrap_or_default();
    for id in ["com.system76.CosmicFiles", "com.system76.CosmicTerm", "com.system76.CosmicSettings", "com.system76.CosmicStore", "com.system76.CosmicEdit"] {
        if !pinned.iter().any(|p| p == id) {
            pinned.push(id.into());
        }
    }
    pinned
}

fn save_pinned(pinned: &[String]) {
    if let Some(Err(err)) = config().map(|c| c.set("pinned", pinned.to_vec())) {
        tracing::warn!("cannot save the pinned apps: {err}");
    }
}

/// Whether the panel is at the top, for the menu to open from there.
fn panel_on_top() -> bool {
    let config = std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| std::path::Path::new(&h).join(".config")))
        .unwrap_or_default();
    std::fs::read_to_string(config.join("cosmic/com.system76.CosmicPanel.Panel/v1/anchor"))
        .is_ok_and(|anchor| anchor.trim() == "Top")
}

/// The user's full name, as the system has it, else the login.
fn user_name() -> String {
    // SAFETY: getpwuid returns a pointer into static storage, read at once.
    let full = unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() || (*pw).pw_gecos.is_null() {
            None
        } else {
            std::ffi::CStr::from_ptr((*pw).pw_gecos)
                .to_str()
                .ok()
                .map(|g| g.split(',').next().unwrap_or("").trim().to_owned())
                .filter(|g| !g.is_empty())
        }
    };
    full.or_else(|| std::env::var("USER").ok()).unwrap_or_default()
}

struct Service(mpsc::Sender<Message>);

#[zbus::interface(name = "io.github.Aenes0.CosmicCentral.Start")]
impl Service {
    async fn toggle(&self) {
        _ = self.0.clone().send(Message::Toggle).await;
    }
}

fn dbus_service() -> Subscription<Message> {
    struct Bus;
    Subscription::run_with(std::any::TypeId::of::<Bus>(), |_| {
        stream::channel(4, |output: mpsc::Sender<Message>| async move {
            let conn = async {
                zbus::connection::Builder::session()?
                    .name(BUS_NAME)?
                    .serve_at(BUS_PATH, Service(output))?
                    .build()
                    .await
            }
            .await;
            match conn {
                // Kept for as long as the menu runs.
                Ok(conn) => {
                    let _conn = conn;
                    futures::future::pending::<()>().await;
                }
                Err(err) => tracing::error!("cannot serve the start menu on D-Bus: {err}"),
            }
        })
    })
}

fn windows_subscription() -> Subscription<Message> {
    struct Windows;
    Subscription::run_with(std::any::TypeId::of::<Windows>(), |_| {
        stream::channel(8, |mut output: mpsc::Sender<Message>| async move {
            let (tx, rx) = calloop::channel::channel();
            _ = output.send(Message::WindowsReady(tx)).await;
            std::thread::spawn(move || {
                if let Err(err) = run_windows(output, rx) {
                    tracing::warn!("window list stopped: {err}");
                }
            });
            futures::future::pending::<()>().await;
        })
    })
}

struct State {
    registry_state: RegistryState,
    seat_state: SeatState,
    info: ToplevelInfoState,
    manager: ToplevelManagerState,
    tx: mpsc::Sender<Message>,
    /// The windows by when they opened, as the list shows them.
    order: Vec<String>,
    dirty: bool,
}

impl State {
    fn report(&mut self) {
        if !std::mem::take(&mut self.dirty) {
            return;
        }
        use zcosmic_toplevel_handle_v1::State as S;
        let mut by_id: HashMap<String, Window> = self
            .info
            .toplevels()
            .map(|t| {
                (
                    t.identifier.clone(),
                    Window {
                        id: t.identifier.clone(),
                        title: t.title.clone(),
                        app_id: t.app_id.clone(),
                        active: t.state.contains(&S::Activated),
                    },
                )
            })
            .collect();
        self.order.retain(|id| by_id.contains_key(id));
        let mut new: Vec<&String> = by_id.keys().filter(|id| !self.order.contains(id)).collect();
        new.sort();
        let new: Vec<String> = new.into_iter().cloned().collect();
        self.order.extend(new);
        let windows = self.order.iter().filter_map(|id| by_id.remove(id)).collect();
        _ = futures::executor::block_on(self.tx.send(Message::Windows(windows)));
    }

    fn handle(&self, id: &str) -> Option<zcosmic_toplevel_handle_v1::ZcosmicToplevelHandleV1> {
        self.info
            .toplevels()
            .find(|t| t.identifier == id)
            .and_then(|t| t.cosmic_toplevel.clone())
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::Activate(id) => {
                let Some(handle) = self.handle(&id) else { return };
                // The menu lets go of the keyboard first.
                std::thread::sleep(Duration::from_millis(60));
                self.manager.manager.unset_minimized(&handle);
                if let Some(seat) = self.seat_state.seats().next() {
                    self.manager.manager.activate(&handle, &seat);
                }
            }
            Command::Close(id) => {
                if let Some(handle) = self.handle(&id) {
                    self.manager.manager.close(&handle);
                }
            }
        }
    }
}

fn run_windows(
    tx: mpsc::Sender<Message>,
    rx: calloop::channel::Channel<Command>,
) -> Result<(), Box<dyn std::error::Error>> {
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
        seat_state: SeatState::new(&globals, &qh),
        registry_state,
        info,
        manager,
        tx,
        order: Vec::new(),
        dirty: true,
    };
    let mut event_loop = calloop::EventLoop::<State>::try_new()?;
    WaylandSource::new(conn, event_queue)
        .insert(event_loop.handle())
        .map_err(|err| err.error)?;
    event_loop
        .handle()
        .insert_source(rx, |event, (), state| {
            if let calloop::channel::Event::Msg(command) = event {
                state.command(command);
            }
        })
        .map_err(|err| err.error)?;
    loop {
        event_loop.dispatch(None, &mut state)?;
        state.report();
    }
}

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    sctk::registry_handlers![SeatState];
}

impl SeatHandler for State {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}
    fn new_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat, _: Capability) {}
    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat, _: Capability) {}
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}
}

impl ToplevelInfoHandler for State {
    fn toplevel_info_state(&mut self) -> &mut ToplevelInfoState {
        &mut self.info
    }
    fn new_toplevel(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &ExtForeignToplevelHandleV1) {
        self.dirty = true;
    }
    fn update_toplevel(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &ExtForeignToplevelHandleV1) {
        self.dirty = true;
    }
    fn toplevel_closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &ExtForeignToplevelHandleV1) {
        self.dirty = true;
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
        _: Vec<WEnum<zcosmic_toplevel_manager_v1::ZcosmicToplelevelManagementCapabilitiesV1>>,
    ) {
    }
}

cctk::delegate_toplevel_info!(State);
cctk::delegate_toplevel_manager!(State);
sctk::delegate_seat!(State);
sctk::delegate_registry!(State);

/// The panel's button: four squares, as Windows has it; a click opens the
/// menu, or closes it.
#[derive(Default)]
pub struct StartButton {
    core: app::Core,
}

impl cosmic::Application for StartButton {
    type Message = ();
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = ();
    const APP_ID: &'static str = "io.github.Aenes0.CosmicCentral.StartButton";

    fn init(core: app::Core, _flags: ()) -> (Self, app::Task<()>) {
        (Self { core }, Task::none())
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

    fn update(&mut self, (): ()) -> app::Task<()> {
        if let Ok(exe) = std::env::current_exe() {
            crate::host::spawn(&exe.to_string_lossy(), &["--start"]);
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, ()> {
        let applet = &self.core.applet;
        let (major, minor) = applet.suggested_padding(true);
        let size = applet.suggested_size(true).0 as f32;
        let square = (size - 2.0) / 2.0;
        let tile = || {
            widget::container(widget::space::horizontal())
                .width(Length::Fixed(square))
                .height(Length::Fixed(square))
                .class(theme::Container::custom(|theme| widget::container::Style {
                    background: Some(Background::Color(theme.cosmic().accent_color().into())),
                    border: iced::Border {
                        radius: 2.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }))
        };
        let logo = widget::column::with_children(vec![
            widget::row::with_children(vec![tile().into(), tile().into()]).spacing(2).into(),
            widget::row::with_children(vec![tile().into(), tile().into()]).spacing(2).into(),
        ])
        .spacing(2);
        let horizontal = applet.is_horizontal();
        let button = button::custom(logo)
            .padding(if horizontal { [minor, major] } else { [major, minor] })
            .class(theme::Button::AppletIcon)
            .on_press(());
        applet.autosize_window(button).into()
    }
}
