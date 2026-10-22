// Copyright 2023 System76 <info@system76.com>, 2026 Aenes0
// SPDX-License-Identifier: GPL-3.0-only

use cosmic::{
    Task, app,
    applet::{
        cosmic_panel_config::PanelAnchor,
        token::subscription::{TokenRequest, TokenUpdate, activation_token_subscription},
    },
    cctk::sctk::reexports::calloop,
    cosmic_config::{self, CosmicConfigEntry},
    iced::{
        self, Event::Mouse, Subscription, event,
        mouse::{self, ScrollDelta},
        platform_specific::shell::commands::popup::destroy_popup,
        runtime::platform_specific::wayland::popup::SctkPopupSettings,
        window,
    },
    scroll::{DiscreteScrollDelta, DiscreteScrollState},
    surface,
};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use crate::tray::{
    components::{
        drag::{self, Origin},
        status_menu, view,
    },
    config::{CONFIG_ID, TrayConfig},
    dnd::TrayDrag,
    layout,
    subscriptions::{
        status_notifier_item::{StatusNotifierItem, StatusNotifierItemProxy},
        status_notifier_watcher,
    },
};

/// A press on the ^ this soon after the compositor closed the box is the same
/// click that closed it (the grab dismisses, then delivers the press): no reopen.
const DISMISS_CLICK_WINDOW: Duration = Duration::from_millis(300);

#[derive(Clone, Debug)]
pub enum Msg {
    None,
    Activate(usize),
    Closed(window::Id),
    // XXX don't use index (unique window id? or I guess that's created and destroyed)
    StatusMenu((usize, status_menu::Msg)),
    StatusNotifier(status_notifier_watcher::Event),
    TogglePopup(usize),
    Hovered(Option<usize>),
    WheelScrolled(ScrollDelta),
    Surface(surface::Action),
    ToggleOverflow,
    HoveredOverflow,
    Token(TokenUpdate),
    /// menu_id, Id, Title read from the item's D-Bus properties.
    Identity(usize, String, String),
    Config(TrayConfig),
    /// Left click on an icon in the box: closes the box, then activates.
    ActivateFromBox(usize),
    /// Right click on an icon in the box: closes the box, then opens its menu.
    MenuFromBox(usize),
    /// A drag source (bar or box) started (`on_start`) or ended (`on_finish`/`on_cancel`).
    DragStart(usize, Origin),
    DragEnd,
    /// The drop target over the bar: pointer in the applet surface.
    BarEnter(f64, f64),
    BarMotion(f64, f64),
    BarLeave,
    /// Dropped on the bar / on the ^; `None` when the data is not ours.
    DropOnBar(Option<TrayDrag>),
    DropOnArrow(Option<TrayDrag>),
}

#[derive(Default)]
pub(crate) struct App {
    pub(super) core: app::Core,
    connection: Option<zbus::Connection>,
    pub(super) menus: BTreeMap<usize, status_menu::State>,
    open_menu: Option<usize>,
    max_menu_id: usize,
    popup: Option<window::Id>,
    /// The box (opened by the ^).
    overflow_popup: Option<window::Id>,
    pub(super) hovered_menu: Option<usize>,
    /// When the compositor last closed the box (click outside, grab).
    box_dismissed_at: Option<Instant>,
    scroll_states: BTreeMap<usize, DiscreteScrollState>,
    token_tx: Option<calloop::channel::Sender<TokenRequest>>,
    pub(super) config: TrayConfig,
    config_handle: Option<cosmic_config::Config>,
    /// A drag started here (bar or box).
    pub(super) drag: Option<drag::Drag>,
    /// Gap under the pointer while a drag is over the bar.
    pub(super) preview_slot: Option<usize>,
}

impl App {
    fn next_menu_id(&mut self) -> usize {
        self.max_menu_id += 1;
        self.max_menu_id
    }

    pub(super) fn is_vertical(&self) -> bool {
        matches!(
            self.core.applet.anchor,
            PanelAnchor::Left | PanelAnchor::Right
        )
    }

    /// Length of one button along the panel; always > 0.
    pub(super) fn button_len(&self) -> f32 {
        let len = f32::from(self.core.applet.suggested_size(true).0)
            + 2.0 * f32::from(self.core.applet.suggested_padding(true).1);
        len.max(1.0)
    }

    /// A drag started on the bar: the ^ shows up as a target.
    pub(super) fn dragging_from_bar(&self) -> bool {
        self.drag.as_ref().is_some_and(|d| d.origin == Origin::Bar)
    }

    /// Who goes on the bar and who goes in the box, right now.
    pub(super) fn split(&self) -> layout::Split {
        let items: Vec<layout::Present<'_>> = self
            .menus
            .iter()
            .map(|(id, menu)| (*id, menu.identity.key()))
            .collect();
        // With bounds for both axes the panel put us in its own overflow:
        // as upstream, only the capacity changes.
        let major_axis_len = self
            .core
            .applet
            .suggested_bounds
            .as_ref()
            .map(|c| if self.is_vertical() { c.height } else { c.width })
            .filter(|len| *len > 0.0);
        layout::split(
            &items,
            &self.config.visible,
            layout::capacity(major_axis_len, self.button_len()),
            self.dragging_from_bar(),
        )
    }

    /// Slot along the bar where the menu of `id` is anchored: its own button
    /// if on the bar, otherwise the ^.
    fn slot_of(&self, id: usize) -> usize {
        let split = self.split();
        split
            .bar
            .iter()
            .position(|&b| b == id)
            .unwrap_or(split.bar.len())
    }

    fn popup_settings(
        &self,
        parent: window::Id,
        popup_id: window::Id,
        slot: usize,
    ) -> SctkPopupSettings {
        let mut settings = self
            .core
            .applet
            .get_popup_settings(parent, popup_id, None, None, None);
        let offset = (slot as f32 * self.button_len()) as i32;
        if self.is_vertical() {
            settings.positioner.anchor_rect.y = offset;
        } else {
            settings.positioner.anchor_rect.x = offset;
        }
        settings
    }

    /// Fits the applet to the bar as drawn: icons, the gap of a drag, the ^.
    pub(super) fn resize_window(&self) -> app::Task<Msg> {
        let Some(main_id) = self.core.main_window_id() else {
            return Task::none();
        };
        let split = self.split();
        let n = self.bar_row(&split).cells.len() + usize::from(split.show_arrow);
        let len = self.button_len();
        let major = (len * n as f32).max(1.0);
        let size = if self.is_vertical() {
            iced::Size::new(len, major)
        } else {
            iced::Size::new(major, len)
        };
        window::resize(main_id, size)
    }

    /// After items, identities or the config change: resize, and close the
    /// box if nothing is left in it.
    fn relayout(&mut self) -> app::Task<Msg> {
        let mut cmds = vec![self.resize_window()];
        if self.overflow_popup.is_some() && self.split().boxed.is_empty() {
            cmds.push(self.close_box());
        }
        Task::batch(cmds)
    }

    /// Updates the visible list, writes it and resizes; no-op if unchanged.
    pub(super) fn set_visible(&mut self, visible: Vec<String>) -> app::Task<Msg> {
        if self.config.visible == visible {
            return Task::none();
        }
        match self.config_handle.as_ref() {
            Some(handle) => {
                if let Err(why) = self.config.set_visible(handle, visible) {
                    tracing::error!(why = why.to_string(), "Failed to write the visible list");
                }
            }
            None => self.config.visible = visible,
        }
        self.relayout()
    }

    pub(super) fn close_box(&mut self) -> app::Task<Msg> {
        match self.overflow_popup.take() {
            Some(popup_id) => destroy_popup(popup_id),
            None => Task::none(),
        }
    }

    fn close_menu(&mut self) -> app::Task<Msg> {
        if let Some(old) = self.open_menu.take()
            && let Some(menu) = self.menus.get(&old)
        {
            menu.closed();
        }
        match self.popup.take() {
            Some(popup_id) => destroy_popup(popup_id),
            None => Task::none(),
        }
    }

    /// Opens the menu of `id`, closing any other and the box (two popups with
    /// a grab at once confuse the compositor); parent is always the main
    /// window. An item that is not on the bar anchors at the ^.
    fn open_menu_popup(&mut self, id: usize) -> app::Task<Msg> {
        let Some(parent) = self.core.main_window_id() else {
            return Task::none();
        };
        if !self.menus.contains_key(&id) {
            return Task::none();
        }
        let mut cmds = vec![self.close_menu(), self.close_box()];
        if let Some(menu) = self.menus.get(&id) {
            menu.opened();
        }
        self.open_menu = Some(id);
        cmds.push(cosmic::surface::surface_task(
            cosmic::surface::action::app_popup(
                |_| Default::default(),
                move |app: &mut Self| {
                    let popup_id = window::Id::unique();
                    app.popup = Some(popup_id);
                    let slot = app.slot_of(id);
                    app.popup_settings(parent, popup_id, slot)
                },
                None,
            ),
        ));
        Task::batch(cmds)
    }

    /// Opens the box, anchored at the ^, closing any open menu first.
    fn open_box(&mut self) -> app::Task<Msg> {
        let Some(parent) = self.core.main_window_id() else {
            return Task::none();
        };
        if self.split().boxed.is_empty() {
            return Task::none();
        }
        let close = self.close_menu();
        let open = cosmic::surface::surface_task(cosmic::surface::action::app_popup(
            |_| Default::default(),
            move |app: &mut Self| {
                let popup_id = window::Id::unique();
                let slot = app.split().bar.len();
                let mut settings = app.popup_settings(parent, popup_id, slot);
                settings.close_with_children = false;
                app.overflow_popup = Some(popup_id);
                settings
            },
            None,
        ));
        Task::batch([close, open])
    }
}

impl cosmic::Application for App {
    type Message = Msg;
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = ();
    const APP_ID: &'static str = crate::tray::APP_ID;

    fn init(core: app::Core, _flags: ()) -> (Self, app::Task<Msg>) {
        let mut core = core;
        // See-through, but no blur behind: the theme stays transparent while any
        // kind is set, and only layer surfaces, which there are none of here, blur.
        core.set_auto_blur(cosmic::core::Auto::System.into());
        let (config_handle, config) =
            match cosmic_config::Config::new(CONFIG_ID, TrayConfig::VERSION) {
                Ok(handle) => {
                    let config = TrayConfig::get_entry(&handle).unwrap_or_else(|(errors, c)| {
                        for why in errors.into_iter().filter(cosmic_config::Error::is_err) {
                            tracing::error!(why = why.to_string(), "Error reading config");
                        }
                        c
                    });
                    (Some(handle), config)
                }
                Err(why) => {
                    tracing::error!(why = why.to_string(), "Error opening config");
                    (None, TrayConfig::default())
                }
            };
        (
            Self {
                core,
                config,
                config_handle,
                ..Self::default()
            },
            Task::none(),
        )
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

    fn update(&mut self, message: Msg) -> app::Task<Msg> {
        match message {
            Msg::None => Task::none(),
            Msg::Activate(id) => {
                if let Some(token_tx) = self.token_tx.as_ref() {
                    let _ = token_tx.send(TokenRequest {
                        app_id: Self::APP_ID.to_string(),
                        exec: format!("activate:{}", id),
                    });
                } else if let Some(menu) = self.menus.get(&id) {
                    return activate(id, &menu.item, None);
                }
                Task::none()
            }
            Msg::ActivateFromBox(id) => {
                let close = self.close_box();
                Task::batch([close, self.update(Msg::Activate(id))])
            }
            Msg::MenuFromBox(id) => {
                let close = self.close_box();
                Task::batch([close, self.open_menu_popup(id)])
            }
            Msg::Closed(surface) => {
                if self.popup == Some(surface) {
                    self.popup = None;
                    self.open_menu = None;
                }
                // With a grab, a click outside closes the box without us
                // asking: forget it, or the next ^ would destroy a dead surface.
                if self.overflow_popup == Some(surface) {
                    self.overflow_popup = None;
                    self.hovered_menu = None;
                    self.box_dismissed_at = Some(Instant::now());
                    return self.box_closed();
                }
                Task::none()
            }
            Msg::StatusMenu((id, msg)) => match self.menus.get_mut(&id) {
                Some(state) => state
                    .update(msg, id, self.token_tx.as_ref())
                    .map(move |msg| cosmic::action::app(Msg::StatusMenu((id, msg)))),
                None => Task::none(),
            },
            Msg::StatusNotifier(event) => match event {
                status_notifier_watcher::Event::Connected(connection) => {
                    self.connection = Some(connection);
                    Task::none()
                }
                status_notifier_watcher::Event::Registered(name) => {
                    let (state, cmd) = status_menu::State::new(name);
                    let proxy = state.item.item_proxy().clone();
                    let existing = self
                        .menus
                        .iter()
                        .find(|(_, prev_state)| prev_state.name() == state.name())
                        .map(|(id, _)| *id);
                    // A re-registered name gets a fresh State (identity unknown
                    // again), so its identity is read again too.
                    let id = existing.unwrap_or_else(|| self.next_menu_id());
                    self.menus.insert(id, state);
                    app::Task::batch([
                        self.relayout(),
                        read_identity(id, proxy),
                        cmd.map(move |msg| cosmic::action::app(Msg::StatusMenu((id, msg)))),
                    ])
                }
                status_notifier_watcher::Event::Unregistered(name) => {
                    let mut cmds = Vec::new();
                    if let Some((id, _)) = self.menus.iter().find(|(_id, menu)| menu.name() == name)
                    {
                        let id = *id;
                        self.menus.remove(&id);
                        self.scroll_states.remove(&id);
                        if self.hovered_menu == Some(id) {
                            self.hovered_menu = None;
                        }
                        // Its drag source is gone with it: `DragEnd` will not
                        // come, and a stale drag would force the ^ and keep
                        // its key off the bar.
                        if self.drag.as_ref().is_some_and(|d| d.menu_id == id) {
                            self.drag = None;
                            self.preview_slot = None;
                        }
                        if self.open_menu == Some(id) {
                            self.open_menu = None;
                            if let Some(popup_id) = self.popup.take() {
                                cmds.push(destroy_popup(popup_id));
                            }
                        }
                    }
                    cmds.push(self.relayout());
                    Task::batch(cmds)
                }
                status_notifier_watcher::Event::Error(err) => {
                    tracing::error!("Status notifier error: {err}");
                    Task::none()
                }
            },
            Msg::Identity(id, sni_id, title) => {
                let Some(menu) = self.menus.get_mut(&id) else {
                    return Task::none();
                };
                menu.identity.learn(&sni_id, &title);
                self.relayout()
            }
            Msg::Config(config) => {
                if self.config == config {
                    return Task::none();
                }
                self.config = config;
                self.relayout()
            }
            Msg::TogglePopup(id) => {
                if !self.menus.contains_key(&id) {
                    return Task::none();
                }
                if self.open_menu.is_some() {
                    return self.close_menu();
                }
                self.open_menu_popup(id)
            }
            Msg::Token(u) => match u {
                TokenUpdate::Init(tx) => {
                    self.token_tx = Some(tx);
                    Task::none()
                }
                TokenUpdate::Finished => {
                    self.token_tx = None;
                    Task::none()
                }
                TokenUpdate::ActivationToken { token, exec: id } => {
                    if let Some(id_str) = id.strip_prefix("activate:") {
                        if let Ok(real_id) = id_str.parse::<usize>()
                            && let Some(menu) = self.menus.get(&real_id)
                        {
                            return activate(real_id, &menu.item, token.clone());
                        }
                        return Task::none();
                    }
                    if let Some(((state, id), token)) = str::parse(&id)
                        .ok()
                        .and_then(|id: usize| self.menus.get_mut(&id).map(|m| (m, id)))
                        .zip(token)
                    {
                        return state
                            .update(
                                status_menu::Msg::ClickToken(token),
                                id,
                                self.token_tx.as_ref(),
                            )
                            .map(move |msg| cosmic::action::app(Msg::StatusMenu((id, msg))));
                    }
                    Task::none()
                }
            },
            Msg::Hovered(id) => {
                self.hovered_menu = id;
                // With a menu open, hovering another icon switches to its menu.
                match (id, self.open_menu) {
                    (Some(id), Some(open)) if open != id => self.open_menu_popup(id),
                    _ => Task::none(),
                }
            }
            Msg::WheelScrolled(delta) => {
                let Some(id) = self.hovered_menu else {
                    return Task::none();
                };
                let discrete_delta = self.scroll_states.entry(id).or_default().update(delta);
                let Some((delta, orientation)) = discrete_scroll_delta(discrete_delta) else {
                    return Task::none();
                };
                let Some(menu) = self.menus.get(&id) else {
                    return Task::none();
                };
                scroll(id, menu.item.item_proxy().clone(), delta, orientation)
            }
            Msg::Surface(a) => cosmic::surface::surface_task(a),
            Msg::ToggleOverflow => {
                if let Some(popup_id) = self.overflow_popup.take() {
                    return destroy_popup(popup_id);
                }
                if self
                    .box_dismissed_at
                    .take()
                    .is_some_and(|t| t.elapsed() < DISMISS_CLICK_WINDOW)
                {
                    return Task::none();
                }
                self.open_box()
            }
            message @ (Msg::DragStart(..)
            | Msg::DragEnd
            | Msg::BarEnter(..)
            | Msg::BarMotion(..)
            | Msg::BarLeave
            | Msg::DropOnBar(_)
            | Msg::DropOnArrow(_)) => self.update_drag(message),
            Msg::HoveredOverflow => {
                self.hovered_menu = None;
                // With a menu open, hovering the ^ switches to the box.
                if self.overflow_popup.is_some() || self.open_menu.is_none() {
                    return Task::none();
                }
                self.open_box()
            }
        }
    }

    fn subscription(&self) -> Subscription<Msg> {
        let mut subscriptions = Vec::new();

        subscriptions.push(status_notifier_watcher::subscription().map(Msg::StatusNotifier));

        for (id, menu) in &self.menus {
            let is_open = self.open_menu == Some(*id);
            subscriptions.push(menu.subscription(is_open).with(*id).map(Msg::StatusMenu));
        }
        subscriptions.push(activation_token_subscription(0).map(Msg::Token));
        subscriptions.push(event::listen_with(|e, status, _| match (e, status) {
            (Mouse(mouse::Event::WheelScrolled { delta }), event::Status::Ignored) => {
                Some(Msg::WheelScrolled(delta))
            }
            _ => None,
        }));
        subscriptions.push(self.core.watch_config::<TrayConfig>(CONFIG_ID).map(|u| {
            for why in u.errors.into_iter().filter(cosmic_config::Error::is_err) {
                tracing::error!(why = why.to_string(), "Error watching config");
            }
            Msg::Config(u.config)
        }));

        iced::Subscription::batch(subscriptions)
    }

    fn view(&self) -> cosmic::Element<'_, Msg> {
        view::view(self)
    }

    fn view_window(&self, surface: window::Id) -> cosmic::Element<'_, Msg> {
        if self.overflow_popup == Some(surface) {
            return view::view_box(self, surface);
        }

        let theme = self.core.system_theme();
        let cosmic = theme.cosmic();
        let corners = cosmic.corner_radii;
        let pad = corners.radius_m[0];
        match self.open_menu.and_then(|id| Some((id, self.menus.get(&id)?))) {
            Some((id, menu)) => self
                .core
                .applet
                .popup_container(
                    cosmic::widget::container(
                        menu.popup_view().map(move |msg| Msg::StatusMenu((id, msg))),
                    )
                    .padding([pad, 0.]),
                )
                .into(),
            None => iced::widget::text("").into(),
        }
    }

    fn on_close_requested(&self, id: window::Id) -> Option<Msg> {
        Some(Msg::Closed(id))
    }
}

/// Reads Id and Title once, together, so a Title cannot fix the key before the Id.
fn read_identity(id: usize, proxy: StatusNotifierItemProxy<'static>) -> app::Task<Msg> {
    Task::future(async move {
        let sni_id = proxy.id().await.unwrap_or_else(|why| {
            tracing::warn!("Could not read Id of item {id}: {why}");
            String::new()
        });
        let title = proxy.title().await.unwrap_or_else(|why| {
            tracing::warn!("Could not read Title of item {id}: {why}");
            String::new()
        });
        cosmic::action::app(Msg::Identity(id, sni_id, title))
    })
}

fn activate(
    id: usize,
    item: &StatusNotifierItem,
    activation_token: Option<String>,
) -> Task<cosmic::Action<Msg>> {
    if item.is_menu() {
        return Task::done(cosmic::action::app(Msg::TogglePopup(id)));
    }
    let item_proxy = item.item_proxy().clone();
    Task::future(async move {
        if let Some(t) = activation_token {
            match item_proxy.provide_xdg_activation_token(t).await {
                Ok(_) => {
                    tracing::debug!("Token provided successfully to {}", id)
                }
                Err(e) => tracing::error!("Failed to provide token to {}: {}", id, e),
            }
        }
        match item_proxy.activate(0, 0).await {
            Ok(_) => cosmic::action::app(Msg::None),
            Err(err) => {
                tracing::error!("Activate failed: {}", err);
                cosmic::action::app(Msg::TogglePopup(id))
            }
        }
    })
}

fn scroll(
    id: usize,
    item_proxy: crate::tray::subscriptions::status_notifier_item::StatusNotifierItemProxy<'static>,
    delta: i32,
    orientation: &'static str,
) -> Task<cosmic::Action<Msg>> {
    Task::future(async move {
        match item_proxy.scroll(delta, orientation).await {
            Ok(_) => cosmic::action::app(Msg::None),
            Err(err) => {
                tracing::error!("Scroll failed for {}: {}", id, err);
                cosmic::action::app(Msg::None)
            }
        }
    })
}

fn discrete_scroll_delta(delta: DiscreteScrollDelta) -> Option<(i32, &'static str)> {
    if delta.y != 0 {
        Some((delta.y as i32, "vertical"))
    } else if delta.x != 0 {
        Some((delta.x as i32, "horizontal"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discrete_scroll_prefers_vertical_delta() {
        assert_eq!(
            discrete_scroll_delta(cosmic::scroll::DiscreteScrollDelta { x: 4, y: -2 }),
            Some((-2, "vertical"))
        );
    }

    #[test]
    fn discrete_scroll_uses_horizontal_delta_when_vertical_is_zero() {
        assert_eq!(
            discrete_scroll_delta(cosmic::scroll::DiscreteScrollDelta { x: 3, y: 0 }),
            Some((3, "horizontal"))
        );
    }

    #[test]
    fn discrete_scroll_ignores_zero_delta() {
        assert_eq!(
            discrete_scroll_delta(cosmic::scroll::DiscreteScrollDelta { x: 0, y: 0 }),
            None
        );
    }

    #[test]
    fn pixel_scroll_accumulates_before_emitting_discrete_delta() {
        let mut state = cosmic::scroll::DiscreteScrollState::default();

        let first = state.update(ScrollDelta::Pixels { x: 0.0, y: 12.0 });
        assert_eq!(discrete_scroll_delta(first), None);

        let second = state.update(ScrollDelta::Pixels { x: 0.0, y: 12.0 });
        assert_eq!(discrete_scroll_delta(second), Some((1, "vertical")));
    }
}
