// SPDX-License-Identifier: GPL-3.0-only

//! What the popup shows: the quick settings, a page with more of one of them,
//! or the list of quick settings to edit.

use cosmic::{
    Element,
    applet::{menu_button, padded_control},
    iced::{self, Alignment, Background, Length},
    theme,
    widget::{self, button},
};

use crate::{
    app::{App, Dnd, Message, Page},
    media::Control,
    notifications::Notification,
    i18n::strings,
    tiles::{
        Tile, TileState, badge, battery_icon, chip_class, edit_class, edit_container, grid, icon,
        symbolic,
    },
};

/// A row of a page: icon, title, maybe a subtitle, maybe something at the end.
fn item<'a>(
    icon_name: &str,
    title: String,
    subtitle: Option<String>,
    trailing: Option<Element<'a, Message>>,
) -> widget::Button<'a, Message> {
    let mut text = widget::column::with_capacity(2).push(widget::text::body(title));
    if let Some(subtitle) = subtitle {
        text = text.push(widget::text::caption(subtitle));
    }
    let mut row = widget::row::with_capacity(4)
        .push(widget::icon::icon(icon(icon_name)).size(20))
        .push(text.width(Length::Fill))
        .spacing(theme::spacing().space_s)
        .align_y(Alignment::Center);
    if let Some(trailing) = trailing {
        row = row.push(trailing);
    }
    menu_button(row)
}

/// A time today as 15:30; another day with its date.
fn clock(timestamp: i64) -> String {
    use chrono::{Local, TimeZone};
    let Some(time) = Local.timestamp_opt(timestamp, 0).single() else {
        return String::new();
    };
    if time.date_naive() == Local::now().date_naive() {
        time.format("%H:%M").to_string()
    } else {
        time.format("%d/%m %H:%M").to_string()
    }
}

fn section<'a>(title: &'a str) -> Element<'a, Message> {
    padded_control(widget::text::heading(title)).into()
}

/// An app's icon by name, or a generic one.
fn app_icon(name: Option<&str>) -> widget::icon::Handle {
    match name {
        Some(path) if path.starts_with('/') => widget::icon::from_path(path.into()),
        Some(name) if !name.is_empty() => widget::icon::from_name(name.to_owned()).handle(),
        _ => icon("application-x-executable-symbolic"),
    }
}

fn check<'a>() -> Element<'a, Message> {
    widget::icon::icon(icon("object-select-symbolic"))
        .size(16)
        .into()
}

fn note<'a>(text: &'a str) -> Element<'a, Message> {
    padded_control(widget::text::body(text)).into()
}

fn dimmed(theme: &cosmic::Theme) -> iced::widget::text::Style {
    let mut color = theme.cosmic().background(theme.transparent).on;
    color.alpha *= 0.65;
    iced::widget::text::Style {
        color: Some(color.into()),
        ..Default::default()
    }
}

/// A rounded card, like the tiles when off.
fn card_container<'a>(
    content: impl Into<Element<'a, Message>>,
) -> widget::Container<'a, Message, cosmic::Theme> {
    widget::container(content)
        .padding(12)
        .width(Length::Fill)
        .class(theme::Container::custom(|theme| {
            let component = &theme.cosmic().background(theme.transparent).component;
            widget::container::Style {
                background: Some(Background::Color(component.base.into())),
                text_color: Some(component.on.into()),
                icon_color: Some(component.on.into()),
                border: iced::Border {
                    radius: theme.cosmic().radius_l().into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        }))
}

fn card_class() -> theme::Button {
    let style = |hovered: bool, theme: &cosmic::Theme| {
        let cosmic = theme.cosmic();
        let component = &cosmic.background(theme.transparent).component;
        button::Style {
            background: Some(Background::Color(
                if hovered { component.hover } else { component.base }.into(),
            )),
            border_radius: cosmic.radius_l().into(),
            text_color: Some(component.on.into()),
            icon_color: Some(component.on.into()),
            ..button::Style::new()
        }
    };
    theme::Button::Custom {
        active: Box::new(move |_, theme| style(false, theme)),
        disabled: Box::new(move |theme| style(false, theme)),
        hovered: Box::new(move |_, theme| style(true, theme)),
        pressed: Box::new(move |_, theme| style(true, theme)),
    }
}

/// The battery pill: lighter when hovered.
fn pill_class() -> theme::Button {
    let style = |hovered: bool, theme: &cosmic::Theme| {
        let cosmic = theme.cosmic();
        let component = &cosmic.background(theme.transparent).component;
        button::Style {
            background: Some(Background::Color(
                if hovered { component.hover } else { component.base }.into(),
            )),
            border_radius: cosmic.radius_xl().into(),
            text_color: Some(component.on.into()),
            icon_color: Some(component.on.into()),
            ..button::Style::new()
        }
    };
    theme::Button::Custom {
        active: Box::new(move |_, theme| style(false, theme)),
        disabled: Box::new(move |theme| style(false, theme)),
        hovered: Box::new(move |_, theme| style(true, theme)),
        pressed: Box::new(move |_, theme| style(true, theme)),
    }
}

impl App {
    /// A page with a way back, maybe a switch, and a way to COSMIC Settings.
    fn page<'a>(
        &'a self,
        title: &'a str,
        switch: Option<(bool, Message)>,
        body: Vec<Element<'a, Message>>,
        more: Option<(&'a str, &'static str)>,
    ) -> Element<'a, Message> {
        let mut header = widget::row::with_capacity(4)
            .push(
                button::icon(icon("go-previous-symbolic"))
                    .padding(8)
                    .on_press(Message::ShowPage(Page::Main)),
            )
            .push(
                widget::text::title4(title)
                    .width(Length::Fill),
            )
            .spacing(theme::spacing().space_xs)
            .align_y(Alignment::Center);
        if let Some((on, message)) = switch {
            header = header.push(widget::toggler(on).on_toggle(move |_| message.clone()));
        }

        let mut column = widget::column::with_capacity(5)
            .push(padded_control(header))
            .push(padded_control(widget::divider::horizontal::default()))
            .push(widget::scrollable(widget::column::with_children(body)).height(Length::Shrink));
        if let Some((label, page)) = more {
            column = column
                .push(padded_control(widget::divider::horizontal::default()))
                .push(
                    menu_button(widget::text::body(label))
                        .on_press(Message::Settings(Some(page))),
                );
        }
        column.into()
    }

    pub fn wifi_page(&self) -> Element<'_, Message> {
        let t = strings();
        let mut body = Vec::new();
        match &self.wifi_networks {
            _ if !self.network.wifi_enabled => body.push(note(t.wifi_off)),
            None => body.push(note(t.searching)),
            Some(networks) => {
                for (index, network) in networks.networks.iter().enumerate() {
                    let strength = match network.strength {
                        76.. => "network-wireless-signal-excellent-symbolic",
                        51..=75 => "network-wireless-signal-good-symbolic",
                        26..=50 => "network-wireless-signal-ok-symbolic",
                        _ => "network-wireless-signal-weak-symbolic",
                    };
                    let status = if network.active {
                        t.connected
                    } else if network.saved.is_some() {
                        t.saved
                    } else if network.secure {
                        t.secured
                    } else {
                        t.open
                    };
                    let trailing = if network.active {
                        Some(check())
                    } else if network.secure {
                        Some(
                            widget::icon::icon(icon("network-wireless-encrypted-symbolic"))
                                .size(16)
                                .into(),
                        )
                    } else {
                        None
                    };
                    body.push(
                        item(strength, network.ssid.clone(), Some(status.to_owned()), trailing)
                            .on_press_maybe(
                                (!network.active).then_some(Message::JoinWifi(index)),
                            )
                            .into(),
                    );
                }
            }
        }
        self.page(
            t.wifi,
            Some((self.network.wifi_enabled, Message::ToggleWifi)),
            body,
            Some((t.more_wifi, "wireless")),
        )
    }

    pub fn bluetooth_page(&self) -> Element<'_, Message> {
        let t = strings();
        let mut body = Vec::new();
        if !self.bluetooth.powered {
            body.push(note(t.bluetooth_off));
        } else if self.bluetooth.devices.is_empty() {
            body.push(note(t.no_devices));
        }
        if self.bluetooth.powered {
            for device in &self.bluetooth.devices {
                let status = if device.connected { t.connected } else { t.disconnected };
                // From BlueZ, or from UPower when the device talks its own protocol.
                let battery = device.battery.map(f64::from).or_else(|| {
                    self.peripherals
                        .iter()
                        .find(|p| p.name == device.name)
                        .map(|p| p.percentage)
                });
                let mut trailing = widget::row::with_capacity(3)
                    .spacing(theme::spacing().space_xs)
                    .align_y(Alignment::Center);
                if let Some(battery) = battery.filter(|_| device.connected) {
                    trailing = trailing
                        .push(widget::icon::icon(icon(&battery_icon(battery))).size(16))
                        .push(widget::text::caption(format!("{battery:.0} %")));
                }
                if device.connected {
                    trailing = trailing.push(check());
                }
                body.push(
                    item(
                        &symbolic(&device.icon),
                        device.name.clone(),
                        Some(status.to_owned()),
                        Some(trailing.into()),
                    )
                    .on_press(Message::ConnectBluetooth(
                        device.path.clone(),
                        !device.connected,
                    ))
                    .into(),
                );
            }
        }
        self.page(
            t.bluetooth,
            Some((self.bluetooth.powered, Message::ToggleBluetooth)),
            body,
            Some((t.more_bluetooth, "bluetooth")),
        )
    }

    pub fn power_page(&self) -> Element<'_, Message> {
        let t = strings();
        let mut body = Vec::new();
        if self.battery.is_some() || !self.peripherals.is_empty() {
            body.push(padded_control(widget::text::heading(t.batteries)).into());
            let level = |percentage: f64| -> Element<'_, Message> {
                widget::text::body(format!("{percentage:.0} %")).into()
            };
            if let Some(battery) = &self.battery {
                body.push(
                    padded_control(
                        widget::row::with_children(vec![
                            widget::icon::icon(icon(&symbolic(&battery.icon))).size(20).into(),
                            widget::text::body(t.this_computer).width(Length::Fill).into(),
                            level(battery.percentage),
                        ])
                        .spacing(theme::spacing().space_s)
                        .align_y(Alignment::Center),
                    )
                    .into(),
                );
            }
            for peripheral in &self.peripherals {
                body.push(
                    padded_control(
                        widget::row::with_children(vec![
                            widget::icon::icon(icon(peripheral.icon)).size(20).into(),
                            widget::text::body(peripheral.name.clone())
                                .width(Length::Fill)
                                .into(),
                            widget::icon::icon(icon(&battery_icon(peripheral.percentage)))
                                .size(16)
                                .into(),
                            level(peripheral.percentage),
                        ])
                        .spacing(theme::spacing().space_s)
                        .align_y(Alignment::Center),
                    )
                    .into(),
                );
            }
            if self.battery.is_some() {
                body.push(
                    padded_control(
                        widget::row::with_children(vec![
                            widget::text::body(t.show_percentage).width(Length::Fill).into(),
                            widget::toggler(self.show_percentage)
                                .on_toggle(|_| Message::ToggleShowPercentage)
                                .into(),
                        ])
                        .align_y(Alignment::Center),
                    )
                    .into(),
                );
            }
            body.push(padded_control(widget::divider::horizontal::default()).into());
        }
        if let Some(power) = &self.power {
            let profiles = [
                ("power-saver", "power-profile-power-saver-symbolic", t.power_saver, t.power_saver_about),
                ("balanced", "power-profile-balanced-symbolic", t.balanced, t.balanced_about),
                ("performance", "power-profile-performance-symbolic", t.performance, t.performance_about),
            ];
            for (id, icon_name, name, about) in profiles {
                if !power.available.iter().any(|p| p == id) {
                    continue;
                }
                let active = power.active == id;
                body.push(
                    item(icon_name, name.to_owned(), Some(about.to_owned()), active.then(check))
                        .on_press(Message::SetPowerProfile(id.to_owned()))
                        .into(),
                );
            }
        }
        self.page(t.power_mode, None, body, Some((t.more_power, "power")))
    }

    pub fn sound_page(&self) -> Element<'_, Message> {
        let t = strings();
        let spacing = theme::spacing();
        let mut body = vec![section(t.output)];
        if let Some((sinks, default)) = &self.sinks {
            for sink in sinks {
                let active = &sink.name == default;
                body.push(
                    item(sink_icon(&sink.name), sink.description.clone(), None, active.then(check))
                        .on_press(Message::SetSink(sink.name.clone()))
                        .into(),
                );
            }
        }

        // Each app's own volume.
        body.push(section(t.apps));
        let volume_icon = |muted: bool, level: f32| {
            if muted || level <= 0.0 {
                "audio-volume-muted-symbolic"
            } else if level < 0.34 {
                "audio-volume-low-symbolic"
            } else if level < 0.67 {
                "audio-volume-medium-symbolic"
            } else {
                "audio-volume-high-symbolic"
            }
        };
        match self.mixer.as_ref().map(|m| &m.streams) {
            Some(streams) if !streams.is_empty() => {
                for (index, stream) in streams.iter().enumerate() {
                    body.push(
                        padded_control(
                            widget::row::with_children(vec![
                                widget::icon::icon(app_icon(stream.icon.as_deref())).size(24).into(),
                                widget::column::with_children(vec![
                                    widget::text::caption(stream.app.clone()).into(),
                                    widget::slider(0.0..=100.0, stream.level * 100.0, move |v| {
                                        Message::SetStreamVolume(index, v)
                                    })
                                    .width(Length::Fill)
                                    .into(),
                                ])
                                .width(Length::Fill)
                                .into(),
                                button::icon(icon(volume_icon(stream.muted, stream.level)))
                                    .padding(8)
                                    .on_press(Message::ToggleStreamMute(index))
                                    .into(),
                            ])
                            .spacing(spacing.space_s)
                            .align_y(Alignment::Center),
                        )
                        .into(),
                    );
                }
            }
            Some(_) => body.push(note(t.no_apps)),
            None => body.push(note(t.loading)),
        }

        // The microphone in use, and how loud it hears.
        if let Some(mixer) = &self.mixer {
            body.push(section(t.input));
            if let Some(input) = &mixer.input {
                body.push(
                    padded_control(
                        widget::row::with_children(vec![
                            button::icon(icon(if input.muted {
                                "microphone-sensitivity-muted-symbolic"
                            } else {
                                "audio-input-microphone-symbolic"
                            }))
                            .padding(8)
                            .on_press(Message::ToggleInputMute)
                            .into(),
                            widget::slider(0.0..=100.0, input.level * 100.0, Message::SetInputVolume)
                                .width(Length::Fill)
                                .into(),
                        ])
                        .spacing(spacing.space_xs)
                        .align_y(Alignment::Center),
                    )
                    .into(),
                );
            }
            for source in &mixer.sources {
                body.push(
                    item(
                        "audio-input-microphone-symbolic",
                        source.description.clone(),
                        None,
                        (source.name == mixer.default_source).then(check),
                    )
                    .on_press(Message::SetSource(source.name.clone()))
                    .into(),
                );
            }
        }
        self.page(t.sound, None, body, Some((t.more_sound, "sound")))
    }

    pub fn dnd_page(&self) -> Element<'_, Message> {
        let t = strings();
        let mut body = Vec::new();
        let on = self.do_not_disturb;
        let options = [
            (t.dnd_forever, Dnd::Forever, on && self.dnd_until.is_none()),
            (t.dnd_hour, Dnd::For(3600), false),
            (t.dnd_hours, Dnd::For(3 * 3600), false),
            (t.dnd_tomorrow, Dnd::Tomorrow, false),
        ];
        for (label, dnd, current) in options {
            body.push(
                item(
                    "notifications-disabled-symbolic",
                    label.to_owned(),
                    None,
                    current.then(check),
                )
                .on_press(Message::SetDoNotDisturb(dnd))
                .into(),
            );
        }
        if let Some(until) = self.dnd_until.filter(|_| on) {
            body.insert(0, note_owned(format!("{} {}", t.until, clock(until))));
        }
        self.page(
            t.do_not_disturb,
            Some((on, Message::ToggleDoNotDisturb)),
            body,
            Some((t.more_dnd, "notifications")),
        )
    }

    pub fn session_page(&self) -> Element<'_, Message> {
        let t = strings();
        let body = [
            ("system-lock-screen-symbolic", t.lock, Message::Lock),
            ("system-suspend-symbolic", t.suspend, Message::Suspend),
            ("system-reboot-symbolic", t.restart, Message::Restart),
            ("system-shutdown-symbolic", t.power_off, Message::PowerOff),
            ("system-log-out-symbolic", t.log_out, Message::LogOut),
        ]
        .into_iter()
        .map(|(icon_name, label, message)| {
            item(icon_name, label.to_owned(), None, None).on_press(message).into()
        })
        .collect();
        self.page(t.session, None, body, None)
    }

    pub fn displays_page(&self) -> Element<'_, Message> {
        let t = strings();
        let spacing = theme::spacing();
        let mut body = Vec::new();
        let Some(displays) = &self.displays else {
            body.push(note(t.loading));
            return self.page(t.displays, None, body, Some((t.more_displays, "displays")));
        };
        for (index, display) in displays.iter().enumerate() {
            if index > 0 {
                body.push(padded_control(widget::divider::horizontal::default()).into());
            }
            let title = if display.internal() {
                t.built_in.to_owned()
            } else {
                format!("{} {}", display.make, display.model).trim().to_owned()
            };
            let subtitle = if display.enabled {
                format!(
                    "{} × {} · {:.0} Hz · {}",
                    display.width,
                    display.height,
                    display.refresh as f32 / 1000.0,
                    display.name
                )
            } else {
                format!("{} · {}", t.display_off, display.name)
            };
            body.push(
                padded_control(
                    widget::row::with_children(vec![
                        widget::icon::icon(icon(if display.internal() {
                            "computer-symbolic"
                        } else {
                            "video-display-symbolic"
                        }))
                        .size(20)
                        .into(),
                        widget::column::with_children(vec![
                            widget::text::body(title).font(cosmic::font::semibold()).into(),
                            widget::text::caption(subtitle).into(),
                        ])
                        .into(),
                    ])
                    .spacing(spacing.space_s)
                    .align_y(Alignment::Center),
                )
                .into(),
            );
            if !display.enabled {
                continue;
            }

            // Brightness: COSMIC's for the built-in screen, DDC/CI for monitors.
            let slider: Option<Element<'_, Message>> = if display.internal() {
                self.brightness.map(|(current, max)| {
                    widget::slider(0.0..=100.0, current as f32 * 100.0 / max as f32, Message::SetBrightness)
                        .width(Length::Fill)
                        .into()
                })
            } else {
                display.ddc.map(|(_, current, max)| {
                    widget::slider(0.0..=100.0, current as f32 * 100.0 / max.max(1) as f32, move |p| {
                        Message::SetDdcBrightness(index, p)
                    })
                    .on_release(Message::ApplyDdcBrightness(index))
                    .width(Length::Fill)
                    .into()
                })
            };
            match slider {
                Some(slider) => body.push(
                    padded_control(
                        widget::row::with_children(vec![
                            widget::icon::icon(icon("display-brightness-symbolic")).size(16).into(),
                            slider,
                        ])
                        .spacing(spacing.space_s)
                        .align_y(Alignment::Center),
                    )
                    .into(),
                ),
                None if !display.internal() => {
                    body.push(padded_control(widget::text::caption(t.no_ddc)).into())
                }
                None => {}
            }

            let chips = [1.0, 1.25, 1.5, 1.75, 2.0].map(|scale| -> Element<'_, Message> {
                button::custom(
                    widget::container(widget::text::caption(format!("{:.0}%", scale * 100.0)))
                        .center(Length::Fill),
                )
                .height(Length::Fixed(28.0))
                .width(Length::Fill)
                .class(chip_class((display.scale - scale).abs() < 0.01))
                .on_press(Message::SetScale(index, scale))
                .into()
            });
            body.push(
                padded_control(
                    widget::row::with_capacity(6)
                        .push(widget::text::caption(t.scale).width(Length::Fixed(56.0)))
                        .extend(chips)
                        .spacing(spacing.space_xxs)
                        .align_y(Alignment::Center),
                )
                .into(),
            );
        }
        self.page(t.displays, None, body, Some((t.more_displays, "displays")))
    }

    /// Edit the tiles in place: tap one to move or resize it, "−" takes it
    /// off, "+" puts one on.
    pub fn tiling_page(&self) -> Element<'_, Message> {
        let t = strings();
        let spacing = theme::spacing();
        let config = self.tiling_config;
        let mut body: Vec<Element<'_, Message>> = Vec::new();

        body.push(padded_control(widget::text::body(t.new_workspaces)).into());
        let choice = |label: &'static str, tile: bool| -> Element<'_, Message> {
            button::custom(
                widget::row::with_capacity(2)
                    .push_maybe((config.new_workspaces_tile == tile).then(|| {
                        widget::icon::icon(icon("object-select-symbolic")).size(16)
                    }))
                    .push(widget::text::body(label))
                    .spacing(spacing.space_xxs)
                    .align_y(Alignment::Center),
            )
            .padding([8, 12])
            .width(Length::Fill)
            .class(chip_class(config.new_workspaces_tile == tile))
            .on_press(Message::SetNewWorkspacesTile(tile))
            .into()
        };
        body.push(
            padded_control(
                widget::row::with_children(vec![
                    choice(t.tiled, true),
                    choice(t.floating, false),
                ])
                .spacing(spacing.space_xxs),
            )
            .into(),
        );
        body.push(padded_control(widget::divider::horizontal::default()).into());

        for (action, keys) in [
            (t.navigate_windows, "Super + ←↑↓→"),
            (t.move_window, "Shift + Super + ←↑↓→"),
            (t.toggle_floating, "Super + G"),
        ] {
            body.push(
                padded_control(
                    widget::row::with_children(vec![
                        widget::text::body(action).width(Length::Fill).into(),
                        widget::text::body(keys).class(theme::Text::Custom(dimmed)).into(),
                    ])
                    .align_y(Alignment::Center),
                )
                .into(),
            );
        }
        body.push(padded_control(widget::divider::horizontal::default()).into());
        body.push(
            padded_control(
                widget::row::with_children(vec![
                    widget::text::body(t.active_hint).width(Length::Fill).into(),
                    widget::toggler(config.active_hint)
                        .on_toggle(|_| Message::ToggleActiveHint)
                        .into(),
                ])
                .align_y(Alignment::Center),
            )
            .into(),
        );

        self.page(
            t.tiling,
            self.workspace_tiling.map(|on| (on, Message::ToggleTiling)),
            body,
            Some((t.more_tiling, "window-management")),
        )
    }

    pub fn edit_page(&self) -> Element<'_, Message> {
        let t = strings();
        let spacing = theme::spacing();

        let header = widget::row::with_children(vec![
            button::icon(icon("go-previous-symbolic"))
                .padding(8)
                .on_press(Message::ShowPage(Page::Main))
                .into(),
            widget::text::title4(t.edit_tiles).width(Length::Fill).into(),
            widget::tooltip(
                button::icon(icon("edit-undo-symbolic"))
                    .padding(8)
                    .class(chip_class(true))
                    .on_press(Message::ResetTiles),
                widget::text::body(t.reset),
                widget::tooltip::Position::Bottom,
            )
            .into(),
        ])
        .spacing(spacing.space_xs)
        .align_y(Alignment::Center);

        let items = self
            .tiles
            .iter()
            .enumerate()
            .map(|(index, placed)| {
                let dragged = self.drag.is_some_and(|d| d.index == index && d.moved);
                let selected = self.selected_tile == Some(index) || dragged;
                let state = self.tile_state(placed.tile);
                let icon_name = state.as_ref().map_or(placed.tile.icon(), |s| s.icon);
                let mut content = widget::row::with_capacity(2)
                    .push(widget::icon::icon(icon(icon_name)).size(16))
                    .spacing(spacing.space_s)
                    .align_y(Alignment::Center);
                if !placed.small {
                    content = content.push(
                        widget::text::body(placed.tile.name())
                            .font(cosmic::font::semibold())
                            .ellipsize(iced::core::text::Ellipsize::End(
                                iced::core::text::EllipsizeHeightLimit::Lines(1),
                            )),
                    );
                }
                // Press to drag it, or just click to choose it.
                let body = widget::mouse_area(
                    widget::container(content)
                        .center_y(Length::Fill)
                        .center_x(Length::Fill)
                        .padding([0, 10])
                        .class(edit_container(selected)),
                )
                .on_press(Message::DragStart(index))
                .on_enter(Message::DragOver(index))
                .interaction(if self.drag.is_some() {
                    iced::mouse::Interaction::Grabbing
                } else {
                    iced::mouse::Interaction::Grab
                });
                let corner = widget::container(badge("list-remove-symbolic", Message::RemoveTile(index)))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(iced::alignment::Horizontal::Right)
                    .align_y(iced::alignment::Vertical::Top);
                let tile: Element<'_, Message> =
                    iced::widget::stack(vec![body.into(), corner.into()]).into();
                (tile, placed.columns())
            })
            .collect();
        let shown = widget::container(grid(items, 56.0))
            .padding(spacing.space_xs)
            .class(theme::Container::custom(|theme| {
                let cosmic = theme.cosmic();
                widget::container::Style {
                    border: iced::Border {
                        radius: cosmic.radius_l().into(),
                        width: 1.0,
                        color: cosmic.accent_color().into(),
                    },
                    ..Default::default()
                }
            }));

        let mut column = widget::column::with_capacity(8)
            .push(header)
            .push(widget::text::caption(t.edit_hint).class(theme::Text::Custom(dimmed)))
            .push(shown)
            .spacing(spacing.space_s);

        // Move or resize the chosen tile.
        if let Some(placed) = self.selected_tile.and_then(|i| self.tiles.get(i).map(|p| (i, p))) {
            let (index, placed) = placed;
            let last = self.tiles.len() - 1;
            let control = |icon_name: &str, label: &'static str, message: Option<Message>| -> Element<'_, Message> {
                button::custom(
                    widget::row::with_children(vec![
                        widget::icon::icon(icon(icon_name)).size(16).into(),
                        widget::text::body(label).into(),
                    ])
                    .spacing(spacing.space_xxs)
                    .align_y(Alignment::Center),
                )
                .padding([6, 12])
                .class(chip_class(false))
                .on_press_maybe(message)
                .into()
            };
            column = column.push(
                widget::row::with_children(vec![
                    widget::text::body(placed.tile.name())
                        .font(cosmic::font::semibold())
                        .width(Length::Fill)
                        .into(),
                    control("go-previous-symbolic", t.earlier, (index > 0).then_some(Message::MoveTile(index, -1))),
                    control("go-next-symbolic", t.later, (index < last).then_some(Message::MoveTile(index, 1))),
                    if placed.small {
                        control("view-fullscreen-symbolic", t.large, Some(Message::ResizeTile(index)))
                    } else {
                        control("view-restore-symbolic", t.small, Some(Message::ResizeTile(index)))
                    },
                ])
                .spacing(spacing.space_xxs)
                .align_y(Alignment::Center),
            );
        }

        // What can go on, as round icons with "+".
        let missing: Vec<Tile> = Tile::ALL
            .into_iter()
            .filter(|tile| !self.tiles.iter().any(|p| p.tile == *tile))
            .collect();
        if !missing.is_empty() {
            column = column.push(widget::text::heading(t.add));
            let chips = missing
                .into_iter()
                .map(|tile| {
                    let chip = button::custom(
                        widget::container(widget::icon::icon(icon(tile.icon())).size(20))
                            .center(Length::Fill),
                    )
                    .width(Length::Fixed(52.0))
                    .height(Length::Fixed(52.0))
                    .class(edit_class(false))
                    .on_press(Message::AddTile(tile));
                    let corner = widget::container(badge("list-add-symbolic", Message::AddTile(tile)))
                        .width(Length::Fixed(60.0))
                        .height(Length::Fixed(56.0))
                        .align_x(iced::alignment::Horizontal::Right)
                        .align_y(iced::alignment::Vertical::Top);
                    let item: Element<'_, Message> = widget::column::with_children(vec![
                        iced::widget::stack(vec![
                            widget::container(chip)
                                .width(Length::Fixed(60.0))
                                .height(Length::Fixed(56.0))
                                .align_x(iced::alignment::Horizontal::Center)
                                .align_y(iced::alignment::Vertical::Bottom)
                                .into(),
                            corner.into(),
                        ])
                        .into(),
                        widget::text::caption(tile.name())
                            .align_x(iced::alignment::Horizontal::Center)
                            .width(Length::Fill)
                            .into(),
                    ])
                    .align_x(Alignment::Center)
                    .spacing(spacing.space_xxs)
                    .into();
                    (item, 1)
                })
                .collect();
            column = column.push(grid(chips, 104.0));
        }

        column = column.push(
            widget::row::with_children(vec![
                widget::text::body(t.show_active).width(Length::Fill).into(),
                widget::toggler(self.show_active)
                    .on_toggle(|_| Message::ToggleShowActive)
                    .into(),
            ])
            .padding([0, spacing.space_xxs])
            .align_y(Alignment::Center),
        );
        column = column.push(
            widget::row::with_children(vec![
                widget::space::horizontal().into(),
                button::custom(
                    widget::row::with_children(vec![
                        widget::icon::icon(icon("object-select-symbolic")).size(16).into(),
                        widget::text::body(t.done).into(),
                    ])
                    .spacing(spacing.space_xxs)
                    .align_y(Alignment::Center),
                )
                .padding([8, 16])
                .class(chip_class(true))
                .on_press(Message::ShowPage(Page::Main))
                .into(),
            ]),
        );

        widget::scrollable(column.padding(spacing.space_s)).height(Length::Shrink).into()
    }

    pub fn main_page(&self) -> Element<'_, Message> {
        let spacing = theme::spacing();
        let footer = widget::row::with_children(vec![
            widget::space::horizontal().into(),
            widget::tooltip(
                button::icon(icon("document-edit-symbolic"))
                    .padding(8)
                    .on_press(Message::ShowPage(Page::Edit)),
                widget::text::body(strings().edit),
                widget::tooltip::Position::Top,
            )
            .into(),
        ]);
        let mut sections = vec![self.header()];
        sections.extend(self.privacy_banner());
        sections.extend([self.sliders(), self.tiles()]);
        sections.extend(self.media_card());
        sections.extend(self.notifications_list());
        sections.push(footer.into());
        // Taller than the screen, with all of it open: it scrolls.
        widget::scrollable(
            widget::column::with_children(sections)
                .spacing(spacing.space_s)
                .padding([spacing.space_s, spacing.space_s, spacing.space_xxs, spacing.space_s]),
        )
        .height(Length::Shrink)
        .into()
    }

    /// Who is using the camera or the microphone.
    fn privacy_banner(&self) -> Option<Element<'_, Message>> {
        if self.privacy.is_empty() {
            return None;
        }
        let t = strings();
        let spacing = theme::spacing();
        let row = |name: &'static str, apps: &[String], doing: &'static str, warning: bool| {
            widget::row::with_children(vec![
                widget::icon::icon(icon(name))
                    .size(16)
                    .class(theme::Svg::Custom(std::rc::Rc::new(move |theme| {
                        iced::widget::svg::Style {
                            color: Some(if warning {
                                theme.cosmic().warning_color().into()
                            } else {
                                theme.cosmic().success_color().into()
                            }),
                        }
                    })))
                    .into(),
                widget::text::body(format!("{} · {doing}", apps.join(", "))).into(),
            ])
            .spacing(spacing.space_xs)
            .align_y(Alignment::Center)
            .into()
        };
        let mut rows: Vec<Element<'_, Message>> = Vec::new();
        if !self.privacy.camera.is_empty() {
            rows.push(row("camera-web-symbolic", &self.privacy.camera, t.using_camera, false));
        }
        if !self.privacy.microphone.is_empty() {
            rows.push(row(
                "audio-input-microphone-symbolic",
                &self.privacy.microphone,
                t.using_microphone,
                true,
            ));
        }
        Some(card_container(widget::column::with_children(rows).spacing(spacing.space_xxs)).into())
    }

    /// What's playing, with its cover, progress and controls.
    fn media_card(&self) -> Option<Element<'_, Message>> {
        let media = self.shown_media()?;
        let spacing = theme::spacing();
        let cover: Element<'_, Message> = match &media.art {
            Some(path) => widget::image(widget::image::Handle::from_path(path))
                .width(Length::Fixed(56.0))
                .height(Length::Fixed(56.0))
                .content_fit(iced::ContentFit::Cover)
                .border_radius(8.0)
                .into(),
            None => widget::container(widget::icon::icon(icon("audio-x-generic-symbolic")).size(28))
                .center(Length::Fixed(56.0))
                .class(theme::Container::custom(|theme| widget::container::Style {
                    background: Some(Background::Color(
                        theme.cosmic().background(theme.transparent).component.hover.into(),
                    )),
                    border: iced::Border {
                        radius: 8.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }))
                .into(),
        };
        let one_line = || {
            iced::core::text::Ellipsize::End(iced::core::text::EllipsizeHeightLimit::Lines(1))
        };
        let about = widget::column::with_children(vec![
            widget::text::body(media.title.clone())
                .font(cosmic::font::semibold())
                .ellipsize(one_line())
                .into(),
            widget::text::caption(media.artist.clone()).ellipsize(one_line()).into(),
            widget::text::caption(media.player.clone())
                .class(theme::Text::Custom(dimmed))
                .ellipsize(one_line())
                .into(),
        ])
        .width(Length::Fill);

        let length = media.length.max(1) as f32;
        let position = self.seeking.unwrap_or(media.position_now() as f32);
        let time = |micros: f32| {
            let seconds = (micros / 1_000_000.0) as u64;
            format!("{}:{:02}", seconds / 60, seconds % 60)
        };
        let progress = widget::row::with_children(vec![
            widget::text::caption(time(position)).into(),
            widget::slider(0.0..=length, position.min(length), Message::Seek)
                .on_release(Message::SeekDone)
                .width(Length::Fill)
                .into(),
            widget::text::caption(time(length)).into(),
        ])
        .spacing(spacing.space_xs)
        .align_y(Alignment::Center);

        let control = |icon_name: &str, message: Option<Message>, size: u16| -> Element<'_, Message> {
            button::icon(icon(icon_name))
                .icon_size(size)
                .padding(8)
                .on_press_maybe(message)
                .into()
        };
        let play = button::custom(
            widget::container(
                widget::icon::icon(icon(if media.playing {
                    "media-playback-pause-symbolic"
                } else {
                    "media-playback-start-symbolic"
                }))
                .size(20),
            )
            .center(Length::Fixed(40.0)),
        )
        .class(chip_class(true))
        .on_press(Message::MediaControl(Control::PlayPause));
        // The players, when more than one: the next one, round.
        let switcher: Element<'_, Message> = if self.media.len() > 1 {
            let index = self
                .media
                .iter()
                .position(|m| m.service == media.service)
                .unwrap_or(0);
            let next = &self.media[(index + 1) % self.media.len()];
            widget::tooltip(
                button::custom(
                    widget::row::with_children(vec![
                        widget::text::caption(format!("{}/{}", index + 1, self.media.len())).into(),
                        widget::icon::icon(icon("go-next-symbolic")).size(12).into(),
                    ])
                    .spacing(2)
                    .align_y(Alignment::Center),
                )
                .padding([6, 8])
                .class(theme::Button::Text)
                .on_press(Message::SelectPlayer(next.service.clone())),
                widget::text::body(next.player.clone()),
                widget::tooltip::Position::Top,
            )
            .into()
        } else {
            widget::space::horizontal().width(Length::Fixed(36.0)).into()
        };
        let outputs = button::icon(icon("audio-speakers-symbolic"))
            .padding(8)
            .selected(self.media_outputs)
            .on_press(Message::ToggleMediaOutputs);
        let controls = widget::row::with_children(vec![
            outputs.into(),
            widget::space::horizontal().into(),
            control(
                "media-skip-backward-symbolic",
                media.can_previous.then_some(Message::MediaControl(Control::Previous)),
                20,
            ),
            play.into(),
            control(
                "media-skip-forward-symbolic",
                media.can_next.then_some(Message::MediaControl(Control::Next)),
                20,
            ),
            widget::space::horizontal().into(),
            switcher,
        ])
        .spacing(spacing.space_m)
        .align_y(Alignment::Center);

        let mut card = widget::column::with_capacity(3)
            .push(
                widget::row::with_children(vec![cover, about.into()])
                    .spacing(spacing.space_s)
                    .align_y(Alignment::Center),
            )
            .spacing(spacing.space_xs);
        if media.length > 0 {
            card = card.push(progress);
        }
        card = card.push(controls);
        // Where the sound goes, picked right here.
        if self.media_outputs {
            if let Some((sinks, default)) = &self.sinks {
                for sink in sinks {
                    card = card.push(
                        item(
                            sink_icon(&sink.name),
                            sink.description.clone(),
                            None,
                            (&sink.name == default).then(check),
                        )
                        .on_press(Message::SetSink(sink.name.clone())),
                    );
                }
            }
        }
        Some(card_container(card).into())
    }

    /// The notifications received, newest first, with a way to clear them.
    fn notifications_list(&self) -> Option<Element<'_, Message>> {
        if self.notifications.is_empty() {
            return None;
        }
        let t = strings();
        let spacing = theme::spacing();
        let header = widget::row::with_children(vec![
            widget::text::heading(t.notifications).width(Length::Fill).into(),
            button::text(t.clear_all)
                .on_press(Message::ClearNotifications)
                .into(),
        ])
        .align_y(Alignment::Center);

        // An app's notifications together, where its newest is; only that one
        // until asked for the rest.
        let mut groups: Vec<(&str, Vec<&Notification>)> = Vec::new();
        for notification in &self.notifications {
            match groups.iter_mut().find(|(app, _)| *app == notification.app) {
                Some((_, group)) => group.push(notification),
                None => groups.push((&notification.app, vec![notification])),
            }
        }
        let mut cards: Vec<Element<'_, Message>> = Vec::new();
        for (app, group) in groups {
            let expanded = self.expanded.contains(app);
            let shown = if expanded { group.len() } else { 1 };
            for notification in &group[..shown] {
                cards.push(self.notification_card(notification));
            }
            if group.len() > 1 {
                let label = if expanded {
                    format!("{app} · {}", group.len())
                } else {
                    format!("+{} {} · {app}", group.len() - 1, t.more)
                };
                cards.push(
                    button::custom(
                        widget::row::with_children(vec![
                            widget::text::caption(label).class(theme::Text::Custom(dimmed)).into(),
                            widget::icon::icon(icon(if expanded {
                                "go-up-symbolic"
                            } else {
                                "go-down-symbolic"
                            }))
                            .size(12)
                            .into(),
                        ])
                        .spacing(spacing.space_xxs)
                        .align_y(Alignment::Center),
                    )
                    .padding([2, 12])
                    .class(theme::Button::Text)
                    .on_press(Message::ToggleGroup(app.to_owned()))
                    .into(),
                );
            }
        }
        let list = widget::scrollable(widget::column::with_children(cards).spacing(spacing.space_xs))
            .height(Length::Shrink);
        Some(
            widget::column::with_children(vec![
                header.into(),
                widget::container(list).max_height(320.0).into(),
            ])
            .spacing(spacing.space_xs)
            .into(),
        )
    }

    fn notification_card<'a>(&'a self, notification: &'a Notification) -> Element<'a, Message> {
        let t = strings();
        let spacing = theme::spacing();
        let app_icon = if notification.icon.starts_with('/') {
            widget::icon::from_path(notification.icon.clone().into())
        } else if notification.icon.is_empty() {
            icon("preferences-system-notifications-symbolic")
        } else {
            widget::icon::from_name(notification.icon.clone()).handle()
        };
        let elapsed = notification.time.elapsed().unwrap_or_default().as_secs();
        let ago = match elapsed {
            0..60 => t.now.to_owned(),
            60..3600 => format!("{} min", elapsed / 60),
            3600..86400 => format!("{} h", elapsed / 3600),
            _ => format!("{} d", elapsed / 86400),
        };
        let mut source = widget::row::with_capacity(2)
            .push(
                widget::text::caption(format!("{} · {ago}", notification.app))
                    .class(theme::Text::Custom(dimmed)),
            )
            .spacing(spacing.space_xxs)
            .align_y(Alignment::Center);
        // New since the popup was last open: an accent dot.
        if self.is_new(notification) {
            source = source.push(
                widget::container(widget::space::horizontal().width(Length::Fixed(0.0)))
                    .width(Length::Fixed(8.0))
                    .height(Length::Fixed(8.0))
                    .class(theme::Container::custom(|theme| widget::container::Style {
                        background: Some(Background::Color(theme.cosmic().accent_color().into())),
                        border: iced::Border {
                            radius: 4.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    })),
            );
        }
        let mut text = widget::column::with_capacity(3)
            .push(source)
            .push(widget::text::body(notification.summary.clone()).font(cosmic::font::semibold()));
        if !notification.body.is_empty() {
            text = text.push(widget::text::caption(notification.body.clone()).ellipsize(
                iced::core::text::Ellipsize::End(iced::core::text::EllipsizeHeightLimit::Lines(3)),
            ));
        }
        // Its buttons, like Reply or Mark as read; the default is the click.
        let actions: Vec<Element<'_, Message>> = notification
            .actions
            .iter()
            .filter(|(key, label)| key != "default" && !label.is_empty())
            .map(|(key, label)| {
                button::text(label.clone())
                    .class(theme::Button::Standard)
                    .on_press(Message::NotificationAction(notification.id, key.clone()))
                    .into()
            })
            .collect();
        if !actions.is_empty() {
            text = text.push(
                widget::row::with_children(actions)
                    .spacing(spacing.space_xxs)
                    .padding([spacing.space_xxs, 0, 0, 0]),
            );
        }
        let key = (notification.id, notification.restored);
        let content = widget::row::with_children(vec![
            widget::icon::icon(app_icon).size(32).into(),
            text.width(Length::Fill).into(),
            button::icon(icon("window-close-symbolic"))
                .padding(4)
                .on_press(Message::DismissNotification(key))
                .into(),
        ])
        .spacing(spacing.space_s);
        button::custom(content)
            .padding(12)
            .width(Length::Fill)
            .class(card_class())
            .on_press(Message::OpenNotification(key))
            .into()
    }

    fn header(&self) -> Element<'_, Message> {
        let t = strings();
        let mut row = widget::row::with_capacity(6)
            .spacing(theme::spacing().space_xs)
            .align_y(Alignment::Center);
        if let Some(battery) = &self.battery {
            // A way to the batteries and power modes.
            row = row.push(
                button::custom(
                    widget::row::with_children(vec![
                        widget::icon::icon(icon(&symbolic(&battery.icon))).size(16).into(),
                        widget::text::body(format!("{:.0} %", battery.percentage)).into(),
                    ])
                    .spacing(theme::spacing().space_xxs)
                    .align_y(Alignment::Center),
                )
                .padding([8, 14])
                .class(pill_class())
                .on_press(Message::ShowPage(Page::Power)),
            );
        }
        row = row.push(widget::space::horizontal());
        let action = |icon_name: &'static str, tooltip: &'static str, message| {
            widget::tooltip(
                button::icon(icon(icon_name))
                    .padding(10)
                    .class(theme::Button::Standard)
                    .on_press(message),
                widget::text::body(tooltip),
                widget::tooltip::Position::Bottom,
            )
        };
        row.push(action("accessories-screenshot-symbolic", t.screenshot, Message::Screenshot))
            .push(action("emblem-system-symbolic", t.settings, Message::Settings(None)))
            .push(action("system-lock-screen-symbolic", t.lock, Message::Lock))
            .push(action("system-shutdown-symbolic", t.session, Message::ShowPage(Page::Session)))
            .into()
    }

    fn sliders(&self) -> Element<'_, Message> {
        let mut column = widget::column::with_capacity(2).spacing(theme::spacing().space_xxs);
        let slider_row = |icon_name: &'static str,
                          press: Message,
                          value: f32,
                          change: fn(f32) -> Message,
                          more: Message| {
            widget::row::with_children(vec![
                button::icon(icon(icon_name)).padding(8).on_press(press).into(),
                widget::slider(0.0..=100.0, value, change)
                    .width(Length::Fill)
                    .into(),
                button::icon(icon("go-next-symbolic"))
                    .padding(8)
                    .on_press(more)
                    .into(),
            ])
            .spacing(theme::spacing().space_xs)
            .align_y(Alignment::Center)
        };
        if let Some(volume) = &self.volume {
            column = column.push(slider_row(
                self.volume_icon(),
                Message::ToggleMute,
                volume.level * 100.0,
                Message::SetVolume,
                Message::ShowPage(Page::Sound),
            ));
        }
        if let Some((current, max)) = self.brightness {
            column = column.push(slider_row(
                "display-brightness-symbolic",
                Message::Settings(Some("displays")),
                current as f32 * 100.0 / max as f32,
                Message::SetBrightness,
                Message::ShowPage(Page::Displays),
            ));
        }
        column.into()
    }

    /// The tiles that are on, for the bar: icon and what it says. Wi-Fi has
    /// its own.
    pub(crate) fn active_tiles(&self) -> Vec<(&'static str, String)> {
        Tile::ALL
            .into_iter()
            .filter(|tile| *tile != Tile::Wifi)
            .filter_map(|tile| self.tile_state(tile))
            .filter(|state| state.on)
            .map(|state| {
                let text = match &state.subtitle {
                    Some(subtitle) => format!("{} · {subtitle}", state.title),
                    None => state.title.to_owned(),
                };
                (state.icon, text)
            })
            .collect()
    }

    fn tile_state(&self, tile: Tile) -> Option<TileState> {
        let t = strings();
        Some(match tile {
            Tile::Wifi => {
                let wired =
                    matches!(&self.network.connection, Some((_, k)) if k == "802-3-ethernet");
                let subtitle = match &self.network.connection {
                    Some((name, _)) => name.clone(),
                    None if self.network.wifi_enabled => t.not_connected.to_owned(),
                    None => t.off.to_owned(),
                };
                TileState::new(
                    self.network_icon(),
                    if wired { t.network } else { t.wifi },
                    Some(subtitle),
                    self.network.wifi_enabled || wired,
                    Some(Message::ToggleWifi),
                    Some(Message::ShowPage(Page::Wifi)),
                )
            }
            Tile::Bluetooth => {
                self.bluetooth.adapter.as_ref()?;
                let subtitle = match self.bluetooth.connected.as_slice() {
                    [] if self.bluetooth.powered => t.on.to_owned(),
                    [] => t.off.to_owned(),
                    [one] => one.clone(),
                    many => format!("{} {}", many.len(), t.devices),
                };
                TileState::new(
                    if self.bluetooth.powered {
                        "bluetooth-active-symbolic"
                    } else {
                        "bluetooth-disabled-symbolic"
                    },
                    t.bluetooth,
                    Some(subtitle),
                    self.bluetooth.powered,
                    Some(Message::ToggleBluetooth),
                    Some(Message::ShowPage(Page::Bluetooth)),
                )
            }
            Tile::Power => {
                let power = self.power.as_ref().filter(|p| p.available.len() > 1)?;
                let (icon_name, name) = match power.active.as_str() {
                    "power-saver" => ("power-profile-power-saver-symbolic", t.power_saver),
                    "performance" => ("power-profile-performance-symbolic", t.performance),
                    _ => ("power-profile-balanced-symbolic", t.balanced),
                };
                TileState::new(
                    icon_name,
                    t.power_mode,
                    Some(name.to_owned()),
                    power.active != "balanced",
                    Some(Message::NextPowerProfile),
                    Some(Message::ShowPage(Page::Power)),
                )
            }
            Tile::DoNotDisturb => TileState::new(
                tile.icon(),
                t.do_not_disturb,
                self.dnd_until
                    .filter(|_| self.do_not_disturb)
                    .map(|until| format!("{} {}", t.until, clock(until))),
                self.do_not_disturb,
                Some(Message::ToggleDoNotDisturb),
                Some(Message::ShowPage(Page::DoNotDisturb)),
            ),
            Tile::DarkMode => TileState::new(
                tile.icon(),
                t.dark_style,
                None,
                self.dark_mode,
                Some(Message::ToggleDarkMode),
                None,
            ),
            Tile::Airplane => TileState::new(
                tile.icon(),
                t.airplane_mode,
                None,
                self.airplane_mode(),
                Some(Message::ToggleAirplaneMode),
                None,
            ),
            Tile::Microphone => {
                let muted = self.microphone?;
                TileState::new(
                    if muted {
                        "microphone-sensitivity-muted-symbolic"
                    } else {
                        "audio-input-microphone-symbolic"
                    },
                    t.microphone,
                    None,
                    muted,
                    Some(Message::ToggleMicrophone),
                    None,
                )
            }
            Tile::KeepAwake => TileState::new(
                tile.icon(),
                t.keep_awake,
                None,
                self.inhibit.is_some(),
                Some(Message::ToggleKeepAwake),
                None,
            ),
            Tile::Tiling => TileState::new(
                tile.icon(),
                t.tiling,
                None,
                self.workspace_tiling?,
                Some(Message::ToggleTiling),
                Some(Message::ShowPage(Page::Tiling)),
            ),
            Tile::Magnifier => TileState::new(
                tile.icon(),
                t.magnifier,
                None,
                self.magnifier?,
                Some(Message::ToggleMagnifier),
                None,
            ),
            Tile::InvertColors => TileState::new(
                tile.icon(),
                t.invert_colors,
                None,
                self.screen_filter?.0,
                Some(Message::ToggleInvertColors),
                None,
            ),
            Tile::ScreenReader => TileState::new(
                tile.icon(),
                t.screen_reader,
                None,
                self.screen_reader?,
                Some(Message::ToggleScreenReader),
                None,
            ),
            Tile::HighContrast => TileState::new(
                tile.icon(),
                t.high_contrast,
                None,
                self.high_contrast(),
                Some(Message::ToggleHighContrast),
                None,
            ),
            // Shortcuts: never lit.
            Tile::Screenshot => TileState::new(
                tile.icon(),
                t.screenshot,
                None,
                false,
                Some(Message::Screenshot),
                None,
            ),
            Tile::Workspaces => TileState::new(
                tile.icon(),
                t.workspaces,
                None,
                false,
                Some(Message::OpenWorkspaces),
                None,
            ),
        })
    }

    fn tiles(&self) -> Element<'_, Message> {
        let items = self
            .tiles
            .iter()
            .filter_map(|placed| {
                let state = self.tile_state(placed.tile)?;
                let element = if placed.small { state.small() } else { state.large() };
                Some((element, placed.columns()))
            })
            .collect();
        grid(items, 56.0)
    }
}

fn note_owned<'a>(text: String) -> Element<'a, Message> {
    padded_control(widget::text::body(text)).into()
}

fn sink_icon(name: &str) -> &'static str {
    if name.contains("HDMI") || name.contains("hdmi") {
        "video-display-symbolic"
    } else if name.contains("bluez") {
        "audio-headphones-symbolic"
    } else {
        "audio-speakers-symbolic"
    }
}
