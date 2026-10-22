// SPDX-License-Identifier: GPL-3.0-only

//! The monitor's popup: a header, then each panel the user keeps, in their
//! order, folding at its title; and the page to arrange them.

use cosmic::{
    Element,
    iced::{self, Alignment, Background, Length},
    theme,
    widget::{self, button},
};

use super::{
    Monitor, Message, Sort, bytes,
    chart::{Area, Bars, Days, Hue, Series, Split},
    config::{BarItem, Panel},
    decimal,
    sample::{Chip, Temp},
    texts::texts,
    uptime,
};
use crate::{pages::dimmed, tiles::icon};

// Drawings the icon themes lack, symbolic: tinted as any other.

const CPU_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path fill="#000" fill-rule="evenodd" d="M5 1h1v2h1.5V1h1v2H10V1h1v2h.5A1.5 1.5 0 0 1 13 4.5V5h2v1h-2v1.5h2v1h-2V10h2v1h-2v.5a1.5 1.5 0 0 1-1.5 1.5H11v2h-1v-2H8.5v2h-1v-2H6v2H5v-2h-.5A1.5 1.5 0 0 1 3 11.5V11H1v-1h2V8.5H1v-1h2V6H1V5h2v-.5A1.5 1.5 0 0 1 4.5 3H5zm-.5 3a.5.5 0 0 0-.5.5v7a.5.5 0 0 0 .5.5h7a.5.5 0 0 0 .5-.5v-7a.5.5 0 0 0-.5-.5zM6 6h4v4H6z"/></svg>"##;
const GPU_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path fill="#000" fill-rule="evenodd" d="M1 3h1v1h11.5A1.5 1.5 0 0 1 15 5.5v5a1.5 1.5 0 0 1-1.5 1.5H6v2H3v-2H2v2H1zm2 2v6h10.5a.5.5 0 0 0 .5-.5v-5a.5.5 0 0 0-.5-.5zm7 .5a2.5 2.5 0 1 1 0 5 2.5 2.5 0 0 1 0-5zm0 1.5a1 1 0 1 0 0 2 1 1 0 0 0 0-2zM4 6h2v4H4z"/></svg>"##;
const MEMORY_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path fill="#000" fill-rule="evenodd" d="M1 4.5A1.5 1.5 0 0 1 2.5 3h11A1.5 1.5 0 0 1 15 4.5V11h-1v2h-1v-2h-2v2h-1v-2H6v2H5v-2H3v2H2v-2H1zM2.5 4a.5.5 0 0 0-.5.5V10h12V4.5a.5.5 0 0 0-.5-.5zM3.5 5.5h2v3h-2zm3.5 0h2v3H7zm3.5 0h2v3h-2z"/></svg>"##;
const THERMOMETER_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path fill="#000" fill-rule="evenodd" d="M8 1a2 2 0 0 0-2 2v6.27a3.5 3.5 0 1 0 4 0V3a2 2 0 0 0-2-2zm0 1a1 1 0 0 1 1 1v6.8l.25.15a2.5 2.5 0 1 1-2.5 0L7 9.8V3a1 1 0 0 1 1-1zM7.5 5h1v5.55a1.5 1.5 0 1 1-1 0z"/></svg>"##;
const BOLT_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path fill="#000" d="M9.5 1 3 9h4.5l-1 6L13 7H8.5z"/></svg>"##;
const INFO_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path fill="#000" fill-rule="evenodd" d="M8 1a7 7 0 1 0 0 14A7 7 0 0 0 8 1zm0 1a6 6 0 1 1 0 12A6 6 0 0 1 8 2zM7.25 7h1.5v4.5h-1.5zM8 4a.9.9 0 1 1 0 1.8A.9.9 0 0 1 8 4z"/></svg>"##;
const LIST_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path fill="#000" d="M2 3h2v2H2zm4 .25h8v1.5H6zM2 7h2v2H2zm4 .25h8v1.5H6zM2 11h2v2H2zm4 .25h8v1.5H6z"/></svg>"##;

fn svg(bytes: &'static [u8]) -> widget::icon::Handle {
    widget::icon::from_svg_bytes(bytes).symbolic(true)
}

pub fn icon_cpu() -> widget::icon::Handle {
    svg(CPU_SVG)
}
pub fn icon_gpu() -> widget::icon::Handle {
    svg(GPU_SVG)
}
pub fn icon_memory() -> widget::icon::Handle {
    svg(MEMORY_SVG)
}
pub fn icon_bolt() -> widget::icon::Handle {
    svg(BOLT_SVG)
}

fn panel_icon(panel: Panel) -> widget::icon::Handle {
    match panel {
        Panel::Energy => svg(BOLT_SVG),
        Panel::Cpu => svg(CPU_SVG),
        Panel::Gpu => svg(GPU_SVG),
        Panel::Memory => svg(MEMORY_SVG),
        Panel::Disks => icon("drive-harddisk-solidstate-symbolic"),
        Panel::Network => icon("network-transmit-receive-symbolic"),
        Panel::Sensors => svg(THERMOMETER_SVG),
        Panel::Processes => svg(LIST_SVG),
        Panel::System => svg(INFO_SVG),
    }
}

fn panel_hue(panel: Panel) -> Hue {
    match panel {
        Panel::Energy => Hue::Energy,
        Panel::Cpu => Hue::Cpu,
        Panel::Gpu => Hue::Gpu,
        Panel::Memory => Hue::Memory,
        Panel::Disks => Hue::Disk,
        Panel::Network => Hue::Download,
        Panel::Sensors => Hue::Warm,
        Panel::Processes => Hue::Upload,
        Panel::System => Hue::Cpu,
    }
}

/// The CPU's temperature, as its chip names it.
pub fn cpu_temp(chips: &[Chip]) -> Option<f32> {
    chips
        .iter()
        .filter(|c| super::sample::CPU_CHIPS.contains(&c.kind.as_str()))
        .flat_map(|c| &c.temps)
        .find(|t| matches!(t.label.as_str(), "Tctl" | "Tdie" | "Package id 0") || t.label.starts_with("temp"))
        .map(|t| t.celsius)
}

/// An icon on a soft patch of its color, as phones' settings have them.
fn badge<'a>(handle: widget::icon::Handle, hue: Hue, size: f32) -> Element<'a, Message> {
    widget::container(
        widget::icon::icon(handle)
            .size((size * 0.55) as u16)
            .class(theme::Svg::Custom(std::rc::Rc::new(move |theme| iced::widget::svg::Style {
                color: Some(hue.color(theme.cosmic().is_dark)),
            }))),
    )
    .center(Length::Fixed(size))
    .class(theme::Container::custom(move |theme| {
        let mut color = hue.color(theme.cosmic().is_dark);
        color.a = 0.16;
        widget::container::Style {
            background: Some(Background::Color(color)),
            border: iced::Border { radius: (size * 0.32).into(), ..Default::default() },
            ..Default::default()
        }
    }))
    .into()
}

/// A dot of a color, before what it stands for.
fn dot<'a>(hue: Hue) -> Element<'a, Message> {
    widget::container(widget::space::horizontal())
        .width(Length::Fixed(8.0))
        .height(Length::Fixed(8.0))
        .class(theme::Container::custom(move |theme| widget::container::Style {
            background: Some(Background::Color(hue.color(theme.cosmic().is_dark))),
            border: iced::Border { radius: 4.0.into(), ..Default::default() },
            ..Default::default()
        }))
        .into()
}

fn faint<'a>(text: impl Into<String>) -> widget::Text<'a, cosmic::Theme> {
    widget::text::caption(text.into()).class(theme::Text::Custom(dimmed))
}

/// A tile of a number: what it is, small, over it; as Windows' Task Manager.
fn stat<'a>(label: &str, value: String, hue: Option<Hue>) -> Element<'a, Message> {
    let value = widget::text::body(value).font(cosmic::font::semibold());
    let value: Element<'a, Message> = match hue {
        Some(hue) => widget::row::with_children(vec![dot(hue), value.into()])
            .spacing(6)
            .align_y(Alignment::Center)
            .into(),
        None => value.into(),
    };
    widget::container(
        widget::column::with_children(vec![faint(label.to_owned()).into(), value]).spacing(2),
    )
    .padding([8, 10])
    .width(Length::Fill)
    .class(theme::Container::custom(|theme| {
        let component = &crate::widgets::card(theme.cosmic());
        widget::container::Style {
            background: Some(Background::Color(component.base.into())),
            border: iced::Border { radius: 10.0.into(), ..Default::default() },
            ..Default::default()
        }
    }))
    .into()
}

fn stats<'a>(items: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    widget::row::with_children(items).spacing(6).into()
}

/// A big number and what it's in, small, beside it.
fn big<'a>(value: String, unit: String, hue: Hue, dark: bool) -> Element<'a, Message> {
    widget::row::with_children(vec![
        widget::text(value)
            .size(30)
            .font(cosmic::font::bold())
            .class(theme::Text::Color(hue.color(dark)))
            .into(),
        widget::text::body(unit).class(theme::Text::Custom(dimmed)).into(),
    ])
    .spacing(4)
    .align_y(Alignment::End)
    .into()
}

/// A line: a name, then its value at the end.
fn line<'a>(name: String, value: String) -> Element<'a, Message> {
    widget::row::with_children(vec![
        faint(name).width(Length::Fill).into(),
        widget::text::caption(value).into(),
    ])
    .align_y(Alignment::Center)
    .into()
}

fn temperature_hue(temp: &Temp) -> Hue {
    let limit = temp.critical.unwrap_or(95.0).max(60.0);
    Hue::level(temp.celsius / limit)
}

fn speed(value: f32) -> String {
    format!("{}/s", bytes(f64::from(value)))
}

fn money(value: f64) -> String {
    format!("{} {}", texts().currency, decimal(value, 2))
}

fn kwh(wh: f64) -> String {
    let kwh = wh / 1000.0;
    format!("{} kWh", decimal(kwh, if kwh >= 100.0 { 0 } else if kwh >= 10.0 { 1 } else { 2 }))
}

/// A choice among a few, lit when picked.
fn chip<'a>(label: &str, on: bool, message: Message) -> Element<'a, Message> {
    button::custom(widget::text::caption(label.to_owned()))
        .padding([4, 10])
        .class(crate::tiles::chip_class(on))
        .on_press(message)
        .into()
}

/// A panel's title, which folds it: in the text's color, a shade behind
/// it under the pointer.
fn title_class() -> theme::Button {
    let style = |hovered: bool, theme: &cosmic::Theme| {
        let cosmic = theme.cosmic();
        let background = cosmic.background(theme.transparent);
        button::Style {
            background: hovered.then(|| Background::Color(background.component.hover.into())),
            border_radius: 10.0.into(),
            text_color: Some(background.on.into()),
            icon_color: Some(background.on.into()),
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

/// Programs that ending would end the session, or the desktop with it.
const VITAL: &[&str] = &[
    "systemd", "cosmic-comp", "cosmic-session", "cosmic-panel", "cosmic-launcher", "cosmic-greeter",
    "cosmic-osd", "cosmic-settings-daemon", "cosmic-idle", "cosmic-notifications", "cosmic-bg",
    "cosmic-ext-applet-central", "Xwayland", "pipewire", "pipewire-pulse", "wireplumber",
    "dbus-daemon", "dbus-broker", "dbus-broker-launch", "gnome-keyring-daemon", "xdg-desktop-portal",
    "xdg-desktop-portal-cosmic", "xdg-document-portal", "xdg-permission-store", "at-spi-bus-launcher",
];

fn small_button<'a>(name: &str, message: Message) -> Element<'a, Message> {
    button::icon(icon(name))
        .icon_size(16)
        .padding(4)
        .class(theme::Button::Text)
        .on_press(message)
        .into()
}

impl Monitor {
    fn dark(&self) -> bool {
        self.core.system_theme().cosmic().is_dark
    }

    pub fn main_view(&self) -> Element<'_, Message> {
        let mut panels: Vec<Element<'_, Message>> = Vec::new();
        for panel in self.settings.order.iter().copied() {
            if self.settings.hidden.contains(&panel) {
                continue;
            }
            // Nothing to say for a GPU that isn't there.
            if panel == Panel::Gpu && self.now.as_ref().is_some_and(|n| n.gpu.is_none()) {
                continue;
            }
            panels.push(self.panel(panel));
        }
        // As tall as the screen, as the Central: the header stays, the
        // panels scroll under it.
        let list = widget::scrollable(
            widget::column::with_children(panels).spacing(crate::pages::PANEL_GAP),
        )
        .height(Length::Fill);
        widget::column::with_children(vec![self.header_and_weather(), list.into()])
            .spacing(crate::pages::PANEL_GAP)
            .height(Length::Fixed(self.max_height()))
            .into()
    }

    /// The machine's name and for how long it's up; editing.
    fn header_row(&self) -> Element<'_, Message> {
        let t = texts();
        let spacing = theme::spacing();
        let up = self.now.as_ref().map(|n| format!("{} {}", t.up_for, uptime(n.uptime))).unwrap_or_default();
        Element::from(
            widget::row::with_children(vec![
                badge(icon("utilities-system-monitor-symbolic"), Hue::Cpu, 30.0),
                widget::column::with_children(vec![
                    widget::text::heading(t.title).into(),
                    faint(format!("{} · {up}", self.info.hostname)).into(),
                ])
                .width(Length::Fill)
                .into(),
                widget::tooltip(
                    small_button("document-edit-symbolic", Message::Edit(true)),
                    widget::text::body(t.edit),
                    widget::tooltip::Position::Bottom,
                )
                .into(),
            ])
            .spacing(spacing.space_s)
            .align_y(Alignment::Center)
            .padding([spacing.space_xs, spacing.space_s]),
        )
    }

    /// The header, and under it the weather.
    fn header_and_weather(&self) -> Element<'_, Message> {
        let spacing = theme::spacing();
        crate::pages::panel(
            widget::column::with_children(vec![
                self.header_row(),
                widget::container(self.weather_block()).padding([0, spacing.space_xxs, spacing.space_xxs, spacing.space_xxs]).into(),
            ])
            .spacing(spacing.space_xxs),
        )
    }

    /// A panel, its title on top, which folds it; what it says, under.
    fn panel(&self, panel: Panel) -> Element<'_, Message> {
        let spacing = theme::spacing();
        let folded = self.settings.is_folded(panel);
        let (summary, extra) = self.summary(panel);
        let mut title = widget::row::with_capacity(5)
            .push(badge(panel_icon(panel), panel_hue(panel), 26.0))
            .push(widget::text::heading(panel.name()).width(Length::Fill))
            .push(widget::text::body(summary).class(theme::Text::Custom(dimmed)))
            .spacing(spacing.space_xs)
            .align_y(Alignment::Center);
        title = title.push(
            widget::icon::icon(icon(if folded { "go-down-symbolic" } else { "go-up-symbolic" })).size(14),
        );
        let title = button::custom(title)
            .padding([4, 4])
            .width(Length::Fill)
            .class(title_class())
            .on_press(Message::ToggleFold(panel));
        let header: Element<'_, Message> = match extra {
            Some(extra) => widget::row::with_children(vec![title.into(), extra])
                .spacing(2)
                .align_y(Alignment::Center)
                .into(),
            None => title.into(),
        };
        let mut column = widget::column::with_capacity(2).push(header).spacing(spacing.space_xs);
        if !folded {
            column = column.push(
                widget::container(self.body(panel)).padding([0, spacing.space_xxs, spacing.space_xxs, spacing.space_xxs]),
            );
        }
        crate::pages::panel(column.padding(spacing.space_s))
    }

    /// What a panel says in its title, folded or not; and a button there.
    fn summary(&self, panel: Panel) -> (String, Option<Element<'_, Message>>) {
        let now = self.now.as_ref();
        match panel {
            Panel::Energy => (
                self.total_watts().map(|w| format!("{w:.0} W")).unwrap_or_default(),
                Some(small_button(
                    if self.energy_settings { "window-close-symbolic" } else { "emblem-system-symbolic" },
                    Message::ToggleEnergySettings,
                )),
            ),
            Panel::Cpu => (format!("{:.0} %", self.rates.cpu), None),
            Panel::Gpu => (now.and_then(|n| n.gpu.as_ref()).map(|g| format!("{:.0} %", g.usage)).unwrap_or_default(), None),
            Panel::Memory => (
                now.map(|n| format!("{:.0} %", n.memory.used as f64 / n.memory.total.max(1) as f64 * 100.0))
                    .unwrap_or_default(),
                None,
            ),
            Panel::Disks => (format!("{}", speed(self.rates.read + self.rates.write)), None),
            Panel::Network => (format!("↓ {}", speed(self.rates.down)), None),
            Panel::Sensors => (
                now.and_then(|n| cpu_temp(&n.chips)).map(|t| format!("{t:.0} °C")).unwrap_or_default(),
                None,
            ),
            Panel::Processes => {
                let count: usize = self.rates.groups.iter().map(|g| g.pids.len()).sum();
                ((count > 0).then(|| count.to_string()).unwrap_or_default(), None)
            }
            Panel::System => (String::new(), None),
        }
    }

    fn body(&self, panel: Panel) -> Element<'_, Message> {
        let Some(now) = self.now.as_ref() else {
            return faint(texts().reading).into();
        };
        match panel {
            Panel::Energy => self.energy(),
            Panel::Cpu => self.cpu(now),
            Panel::Gpu => self.gpu(now),
            Panel::Memory => self.memory(now),
            Panel::Disks => self.disks(now),
            Panel::Network => self.network(now),
            Panel::Sensors => self.sensors(now),
            Panel::Processes => self.processes(),
            Panel::System => self.system(now),
        }
    }

    /// Watts now, large, of what; the minute's chart; the energy today, this
    /// month and in all, with what it cost; the last two weeks.
    fn energy(&self) -> Element<'_, Message> {
        let t = texts();
        let spacing = theme::spacing();
        let dark = self.dark();
        let mut column = widget::column::with_capacity(8).spacing(spacing.space_s);
        let total = self.total_watts();
        let parts: Vec<(Hue, &str, Option<f32>, bool)> = vec![
            (Hue::Cpu, t.cpu, self.rates.cpu_watts, false),
            (Hue::Gpu, t.gpu, self.rates.gpu_watts, false),
            (Hue::Fine, t.rest, Some(self.settings.rest_watts as f32).filter(|w| *w > 0.0), true),
        ];
        let legend = widget::column::with_children(
            parts
                .iter()
                .map(|(hue, name, watts, guessed)| {
                    let value = match watts {
                        Some(w) if *guessed => format!("~{w:.0} W"),
                        Some(w) => format!("{w:.0} W"),
                        None => "—".to_owned(),
                    };
                    widget::row::with_children(vec![
                        dot(*hue),
                        faint(*name).width(Length::Fixed(44.0)).into(),
                        widget::text::caption(value).font(cosmic::font::semibold()).into(),
                    ])
                    .spacing(6)
                    .align_y(Alignment::Center)
                    .into()
                })
                .collect::<Vec<_>>(),
        )
        .spacing(2);
        column = column.push(
            widget::row::with_children(vec![
                widget::column::with_children(vec![
                    big(total.map_or("—".to_owned(), |w| format!("{w:.0}")), "W".to_owned(), Hue::Energy, dark),
                    faint(if self.settings.rest_watts > 0.0 { t.whole_pc_now } else { t.cpu_gpu_now }).into(),
                ])
                .width(Length::Fill)
                .into(),
                legend.into(),
            ])
            .align_y(Alignment::Center),
        );
        column = column.push(Area::new(vec![Series { values: &self.history.power, hue: Hue::Energy }], None).view(64.0));
        let ledger = &self.meter.ledger;
        let card = |label: &str, wh: f64| -> Element<'_, Message> {
            let mut lines = vec![
                faint(label.to_owned()).into(),
                widget::text::body(kwh(wh)).font(cosmic::font::semibold()).into(),
            ];
            if let Some(tariff) = self.settings.tariff {
                lines.push(
                    widget::text::caption(money(wh / 1000.0 * tariff))
                        .class(theme::Text::Color(Hue::Energy.color(dark)))
                        .into(),
                );
            }
            widget::container(widget::column::with_children(lines).spacing(1))
                .padding([8, 10])
                .width(Length::Fill)
                .class(theme::Container::custom(|theme| {
                    let component = &crate::widgets::card(theme.cosmic());
                    widget::container::Style {
                        background: Some(Background::Color(component.base.into())),
                        border: iced::Border { radius: 10.0.into(), ..Default::default() },
                        ..Default::default()
                    }
                }))
                .into()
        };
        column = column.push(stats(vec![
            card(t.today, ledger.today_wh()),
            card(t.this_month, ledger.month_wh()),
            card(t.total, ledger.total_wh),
        ]));
        let days: Vec<(String, f64)> = ledger
            .last_days(14)
            .into_iter()
            .map(|(day, wh)| {
                use chrono::Datelike;
                (t.weekday_letters[day.weekday().num_days_from_sunday() as usize].to_owned(), wh)
            })
            .collect();
        let since = ledger.since.map(|d| format!("{} {}", t.since, d.format("%d/%m/%Y"))).unwrap_or_default();
        column = column.push(
            widget::column::with_children(vec![
                widget::row::with_children(vec![
                    faint(t.last_14_days).width(Length::Fill).into(),
                    faint(since).into(),
                ])
                .into(),
                Days { days }.view(52.0),
            ])
            .spacing(4),
        );
        if self.rates.cpu_watts.is_none() && self.now.as_ref().is_some_and(|n| n.rapl.is_none()) {
            column = column.push(
                widget::row::with_children(vec![
                    widget::icon::icon(icon("dialog-information-symbolic")).size(14).into(),
                    faint(t.rapl_hint).into(),
                ])
                .spacing(6)
                .align_y(Alignment::Center),
            );
        }
        if self.settings.tariff.is_none() && !self.energy_settings {
            column = column.push(
                button::custom(
                    widget::row::with_children(vec![
                        widget::icon::icon(icon("emblem-system-symbolic")).size(14).into(),
                        widget::text::caption(t.set_tariff).into(),
                    ])
                    .spacing(6)
                    .align_y(Alignment::Center),
                )
                .padding([6, 12])
                .class(crate::tiles::chip_class(false))
                .on_press(Message::ToggleEnergySettings),
            );
        }
        if self.energy_settings {
            let field = |label: &str, placeholder: &str, value: &str, on_input: fn(String) -> Message| -> Element<'_, Message> {
                widget::column::with_children(vec![
                    faint(label.to_owned()).into(),
                    widget::text_input(placeholder.to_owned(), value.to_owned())
                        .on_input(on_input)
                        .width(Length::Fill)
                        .into(),
                ])
                .spacing(4)
                .width(Length::Fill)
                .into()
            };
            column = column.push(
                widget::row::with_children(vec![
                    field(t.tariff_label, "0,85", &self.tariff_input, Message::TariffInput),
                    field(t.rest_label, "45", &self.rest_input, Message::RestInput),
                ])
                .spacing(spacing.space_s),
            );
            column = column.push(faint(t.rest_about));
            column = column.push(
                widget::row::with_children(vec![
                    widget::space::horizontal().into(),
                    button::text(t.reset_count)
                        .class(theme::Button::Destructive)
                        .on_press(Message::ResetEnergy)
                        .into(),
                ]),
            );
        }
        column.into()
    }

    fn cpu<'a>(&'a self, now: &'a super::sample::Reading) -> Element<'a, Message> {
        let t = texts();
        let dark = self.dark();
        let info = &self.info;
        let mut column = widget::column::with_capacity(6).spacing(theme::spacing().space_s);
        column = column.push(
            widget::row::with_children(vec![
                big(format!("{:.0}", self.rates.cpu), "%".to_owned(), Hue::Cpu, dark),
                widget::space::horizontal().into(),
                widget::column::with_children(vec![
                    widget::text::caption(info.cpu_model.clone()).into(),
                    faint(format!("{} {} · {} threads", info.cores, t.cores, info.threads)).into(),
                ])
                .align_x(Alignment::End)
                .into(),
            ])
            .align_y(Alignment::Center),
        );
        column = column.push(Area::new(vec![Series { values: &self.history.cpu, hue: Hue::Cpu }], Some(100.0)).view(70.0));
        let temp = cpu_temp(&now.chips);
        column = column.push(stats(vec![
            stat(t.temperature, temp.map_or("—".into(), |t| format!("{t:.0} °C")), temp.map(|c| Hue::level(c / 95.0))),
            stat(
                t.clock,
                now.frequency.map_or("—".into(), |(avg, _)| format!("{} GHz", decimal(f64::from(avg) / 1000.0, 2))),
                None,
            ),
            stat(t.power, self.rates.cpu_watts.map_or("—".into(), |w| format!("{w:.0} W")), None),
            stat(t.load, decimal(f64::from(now.load[0]), 2), None),
        ]));
        if !self.rates.cores.is_empty() {
            column = column.push(
                widget::column::with_children(vec![
                    faint(t.per_thread).into(),
                    Bars { values: &self.rates.cores, hue: Hue::Cpu }.view(44.0),
                ])
                .spacing(4),
            );
        }
        column.into()
    }

    fn gpu<'a>(&'a self, now: &'a super::sample::Reading) -> Element<'a, Message> {
        let t = texts();
        let dark = self.dark();
        let Some(gpu) = &now.gpu else {
            return faint(t.no_gpu).into();
        };
        let mut column = widget::column::with_capacity(6).spacing(theme::spacing().space_s);
        column = column.push(
            widget::row::with_children(vec![
                big(format!("{:.0}", gpu.usage), "%".to_owned(), Hue::Gpu, dark),
                widget::space::horizontal().into(),
                widget::column::with_children(vec![
                    widget::text::caption(gpu.name.clone()).into(),
                    faint(gpu.watts_limit.map_or(String::new(), |l| format!("{} {l:.0} W", t.limit))).into(),
                ])
                .align_x(Alignment::End)
                .into(),
            ])
            .align_y(Alignment::Center),
        );
        column = column.push(Area::new(vec![Series { values: &self.history.gpu, hue: Hue::Gpu }], Some(100.0)).view(70.0));
        let power = gpu.watts.map_or("—".into(), |w| format!("{w:.0} W"));
        column = column.push(stats(vec![
            stat(t.temperature, gpu.temp.map_or("—".into(), |t| format!("{t:.0} °C")), gpu.temp.map(|c| Hue::level(c / 90.0))),
            stat(t.power, power, None),
            stat(t.clock, gpu.clock.map_or("—".into(), |c| format!("{} GHz", decimal(f64::from(c) / 1000.0, 2))), None),
            stat(t.fan, gpu.fan.map_or("—".into(), |f| format!("{f} %")), None),
        ]));
        if gpu.vram_total > 0 {
            let fraction = gpu.vram_used as f32 / gpu.vram_total as f32;
            column = column.push(
                widget::column::with_children(vec![
                    line(t.vram.to_owned(), format!("{} / {}", bytes(gpu.vram_used as f64), bytes(gpu.vram_total as f64))),
                    Split::level(fraction, Some(Hue::Gpu), 8.0),
                ])
                .spacing(4),
            );
        }
        column.into()
    }

    fn memory<'a>(&'a self, now: &'a super::sample::Reading) -> Element<'a, Message> {
        let t = texts();
        let dark = self.dark();
        let memory = &now.memory;
        let total = memory.total.max(1) as f32;
        let mut column = widget::column::with_capacity(6).spacing(theme::spacing().space_s);
        column = column.push(
            widget::row::with_children(vec![
                big(decimal(memory.used as f64 / 1024f64.powi(3), 1), format!("/ {}", bytes(memory.total as f64)), Hue::Memory, dark),
                widget::space::horizontal().into(),
                widget::column::with_children(vec![
                    faint(t.available).into(),
                    widget::text::caption(bytes(memory.total.saturating_sub(memory.used) as f64))
                        .font(cosmic::font::semibold())
                        .into(),
                ])
                .align_x(Alignment::End)
                .into(),
            ])
            .align_y(Alignment::Center),
        );
        column = column.push(Area::new(vec![Series { values: &self.history.memory, hue: Hue::Memory }], Some(100.0)).view(60.0));
        let mut cache = Hue::Memory.color(dark);
        cache.a = 0.38;
        column = column.push(Split {
            parts: vec![
                (memory.used as f32 / total, Hue::Memory.color(dark)),
                (memory.cache as f32 / total, cache),
            ],
        }
        .view(10.0));
        let free = memory.total.saturating_sub(memory.used + memory.cache);
        column = column.push(
            widget::row::with_children(vec![
                widget::row::with_children(vec![dot(Hue::Memory), faint(format!("{} {}", t.in_use, bytes(memory.used as f64))).into()])
                    .spacing(6).align_y(Alignment::Center).into(),
                widget::space::horizontal().into(),
                faint(format!("{} {}", t.cache, bytes(memory.cache as f64))).into(),
                widget::space::horizontal().into(),
                faint(format!("{} {}", t.free, bytes(free as f64))).into(),
            ])
            .align_y(Alignment::Center),
        );
        if memory.swap_total > 0 {
            let fraction = memory.swap_used as f32 / memory.swap_total as f32;
            let mut lines = vec![
                line(t.swap.to_owned(), format!("{} / {}", bytes(memory.swap_used as f64), bytes(memory.swap_total as f64))),
                Split::level(fraction, Some(Hue::Memory), 6.0),
            ];
            if let Some((original, compressed)) = memory.zram {
                lines.push(faint(format!(
                    "zram: {} {} {}",
                    bytes(original as f64),
                    t.compressed_to,
                    bytes(compressed as f64)
                )).into());
            }
            column = column.push(widget::column::with_children(lines).spacing(4));
        }
        column.into()
    }

    fn disks<'a>(&'a self, now: &'a super::sample::Reading) -> Element<'a, Message> {
        let t = texts();
        let mut column = widget::column::with_capacity(8).spacing(theme::spacing().space_s);
        column = column.push(
            widget::row::with_children(vec![
                widget::row::with_children(vec![dot(Hue::Disk), widget::text::caption(format!("{} {}", t.read, speed(self.rates.read))).into()])
                    .spacing(6).align_y(Alignment::Center).into(),
                widget::space::horizontal().into(),
                widget::row::with_children(vec![dot(Hue::Upload), widget::text::caption(format!("{} {}", t.write, speed(self.rates.write))).into()])
                    .spacing(6).align_y(Alignment::Center).into(),
            ]),
        );
        column = column.push(
            Area::new(
                vec![
                    Series { values: &self.history.read, hue: Hue::Disk },
                    Series { values: &self.history.write, hue: Hue::Upload },
                ],
                None,
            )
            .view(56.0),
        );
        // Each drive: its model and temperature, what it's doing.
        for (name, read, write) in &self.rates.disks {
            let chip = now.chips.iter().find(|c| c.disk.as_deref() == Some(name.as_str()));
            let model = chip
                .map(|c| c.model.clone())
                .filter(|m| !m.is_empty())
                .or_else(|| self.info.disk_models.iter().find(|(n, _)| n == name).map(|(_, m)| m.clone()))
                .unwrap_or_else(|| name.clone());
            let temp = chip.and_then(|c| c.temps.first());
            let mut row = widget::row::with_capacity(4)
                .push(widget::icon::icon(icon("drive-harddisk-solidstate-symbolic")).size(16))
                .push(
                    widget::column::with_children(vec![
                        widget::text::caption(model).into(),
                        faint(format!("{name} · ↓ {} · ↑ {}", speed(*read), speed(*write))).into(),
                    ])
                    .width(Length::Fill),
                )
                .spacing(8)
                .align_y(Alignment::Center);
            if let Some(temp) = temp {
                row = row.push(
                    widget::row::with_children(vec![dot(temperature_hue(temp)), widget::text::caption(format!("{:.0} °C", temp.celsius)).into()])
                        .spacing(4)
                        .align_y(Alignment::Center),
                );
            }
            column = column.push(row);
        }
        // How full each place is.
        for mount in &now.mounts {
            let fraction = mount.used as f32 / mount.total.max(1) as f32;
            column = column.push(
                widget::column::with_children(vec![
                    line(
                        mount.path.clone(),
                        format!("{} / {} · {:.0} %", bytes(mount.used as f64), bytes(mount.total as f64), fraction * 100.0),
                    ),
                    Split::level(fraction, None, 6.0),
                ])
                .spacing(4),
            );
        }
        column.into()
    }

    fn network<'a>(&'a self, now: &'a super::sample::Reading) -> Element<'a, Message> {
        let t = texts();
        let dark = self.dark();
        let mut column = widget::column::with_capacity(5).spacing(theme::spacing().space_s);
        let side = |hue: Hue, arrow: &str, label: &str, value: f32| -> Element<'_, Message> {
            widget::column::with_children(vec![
                widget::row::with_children(vec![dot(hue), faint(label.to_owned()).into()])
                    .spacing(6)
                    .align_y(Alignment::Center)
                    .into(),
                widget::text(format!("{arrow} {}", speed(value)))
                    .size(18)
                    .font(cosmic::font::semibold())
                    .class(theme::Text::Color(hue.color(dark)))
                    .into(),
            ])
            .spacing(2)
            .width(Length::Fill)
            .into()
        };
        column = column.push(widget::row::with_children(vec![
            side(Hue::Download, "↓", t.download, self.rates.down),
            side(Hue::Upload, "↑", t.upload, self.rates.up),
        ]));
        column = column.push(
            Area::new(
                vec![
                    Series { values: &self.history.down, hue: Hue::Download },
                    Series { values: &self.history.up, hue: Hue::Upload },
                ],
                None,
            )
            .view(64.0),
        );
        let mut items = Vec::new();
        if let Some((name, address)) = &now.route {
            items.push(stat(t.interface, name.clone(), None));
            if let Some(address) = address {
                items.push(stat(t.address, address.clone(), None));
            }
        }
        if !items.is_empty() {
            column = column.push(stats(items));
        }
        column = column.push(line(
            t.since_boot.to_owned(),
            format!("↓ {} · ↑ {}", bytes(now.net.0 as f64), bytes(now.net.1 as f64)),
        ));
        column.into()
    }

    fn sensors<'a>(&'a self, now: &'a super::sample::Reading) -> Element<'a, Message> {
        let t = texts();
        let mut column = widget::column::with_capacity(12).spacing(theme::spacing().space_s);
        let row = |label: String, temp: &Temp| -> Element<'_, Message> {
            let limit = temp.critical.unwrap_or(95.0).max(60.0);
            widget::row::with_children(vec![
                faint(label).width(Length::Fixed(110.0)).into(),
                widget::container(Split::level(temp.celsius / limit, Some(temperature_hue(temp)), 6.0))
                    .width(Length::Fill)
                    .center_y(Length::Fixed(16.0))
                    .into(),
                widget::text::caption(format!("{:.0} °C", temp.celsius))
                    .width(Length::Fixed(44.0))
                    .align_x(iced::alignment::Horizontal::Right)
                    .into(),
            ])
            .spacing(8)
            .align_y(Alignment::Center)
            .into()
        };
        let group = |title: String, rows: Vec<Element<'a, Message>>| -> Element<'a, Message> {
            let mut children = vec![widget::text::caption(title).font(cosmic::font::semibold()).into()];
            children.extend(rows);
            widget::column::with_children(children).spacing(4).into()
        };
        let mut drives = 0;
        for chip in &now.chips {
            let title = match chip.kind.as_str() {
                "k10temp" | "coretemp" | "zenpower" => format!("{} · {}", t.cpu, self.info.cpu_model),
                "nvme" | "drivetemp" => {
                    drives += 1;
                    if chip.model.is_empty() { format!("{} {drives}", t.drive) } else { chip.model.clone() }
                }
                "amdgpu" | "nouveau" => t.gpu.to_owned(),
                kind if kind.starts_with("r8169") || kind.contains("igb") || kind.contains("e1000") || kind.starts_with("iwlwifi") || kind.starts_with("mt7") => {
                    t.network_card.to_owned()
                }
                "acpitz" => t.board.to_owned(),
                kind => kind.to_owned(),
            };
            // "temp1" says nothing: a chip's lone one is its temperature.
            let mut rows: Vec<Element<'_, Message>> = chip
                .temps
                .iter()
                .map(|temp| {
                    let label = if temp.label.starts_with("temp") { t.temperature.to_owned() } else { temp.label.clone() };
                    row(label, temp)
                })
                .collect();
            for (label, rpm) in &chip.fans {
                rows.push(line(label.clone(), format!("{rpm} RPM")));
            }
            column = column.push(group(title, rows));
        }
        if let Some(gpu) = &now.gpu {
            if let Some(celsius) = gpu.temp {
                let temp = Temp { label: t.gpu.to_owned(), celsius, critical: Some(90.0) };
                let mut rows = vec![row(t.core.to_owned(), &temp)];
                if let Some(fan) = gpu.fan {
                    rows.push(line(t.fan.to_owned(), format!("{fan} %")));
                }
                // Owned: the temperature was made here.
                column = column.push(group(gpu.name.clone(), rows));
            }
        }
        column.into()
    }

    fn processes(&self) -> Element<'_, Message> {
        let t = texts();
        let spacing = theme::spacing();
        let sort = self.sort.unwrap_or(Sort::Cpu);
        let mut groups: Vec<&super::Group> = self.rates.groups.iter().collect();
        match sort {
            Sort::Cpu => groups.sort_by(|a, b| b.cpu.total_cmp(&a.cpu).then(b.memory.cmp(&a.memory))),
            Sort::Memory => groups.sort_by(|a, b| b.memory.cmp(&a.memory)),
        }
        let mut column = widget::column::with_capacity(12).spacing(spacing.space_xxs);
        column = column.push(
            widget::row::with_children(vec![
                faint(t.sort_by).width(Length::Fill).into(),
                chip(t.cpu, sort == Sort::Cpu, Message::Sort(Sort::Cpu)),
                chip(t.memory, sort == Sort::Memory, Message::Sort(Sort::Memory)),
            ])
            .spacing(6)
            .align_y(Alignment::Center),
        );
        if groups.is_empty() {
            column = column.push(faint(t.reading));
        }
        let dark = self.dark();
        for group in groups.into_iter().take(8) {
            if self.asking_end.as_deref() == Some(group.name.as_str()) {
                let question = if group.pids.len() > 1 {
                    format!("{} {} ({} {})?", t.end, group.name, group.pids.len(), t.processes_word)
                } else {
                    format!("{} {}?", t.end, group.name)
                };
                column = column.push(
                    widget::container(
                        widget::row::with_children(vec![
                            widget::text::caption(question).width(Length::Fill).into(),
                            button::text(t.cancel).on_press(Message::AskEnd(None)).into(),
                            button::text(t.end).class(theme::Button::Destructive).on_press(Message::End(group.name.clone())).into(),
                        ])
                        .spacing(6)
                        .align_y(Alignment::Center),
                    )
                    .padding([6, 8])
                    .class(theme::Container::custom(|theme| {
                        let component = &crate::widgets::card(theme.cosmic());
                        widget::container::Style {
                            background: Some(Background::Color(component.base.into())),
                            border: iced::Border { radius: 10.0.into(), ..Default::default() },
                            ..Default::default()
                        }
                    })),
                );
                continue;
            }
            // Its initial, on its own color: the name picks it.
            let initial = group.name.chars().next().unwrap_or('?').to_uppercase().to_string();
            let hues = [Hue::Cpu, Hue::Gpu, Hue::Memory, Hue::Disk, Hue::Download, Hue::Upload, Hue::Energy];
            let hue = hues[group.name.bytes().map(usize::from).sum::<usize>() % hues.len()];
            let avatar = widget::container(
                widget::text::caption(initial).font(cosmic::font::bold()).class(theme::Text::Color(hue.color(dark))),
            )
            .center(Length::Fixed(26.0))
            .class(theme::Container::custom(move |theme| {
                let mut color = hue.color(theme.cosmic().is_dark);
                color.a = 0.16;
                widget::container::Style {
                    background: Some(Background::Color(color)),
                    border: iced::Border { radius: 13.0.into(), ..Default::default() },
                    ..Default::default()
                }
            }));
            let name = if group.pids.len() > 1 {
                format!("{} ({})", group.name, group.pids.len())
            } else {
                group.name.clone()
            };
            let end: Element<'_, Message> = if group.mine && !VITAL.contains(&group.name.as_str()) {
                button::icon(icon("window-close-symbolic"))
                    .icon_size(12)
                    .padding(4)
                    .class(theme::Button::Text)
                    .on_press(Message::AskEnd(Some(group.name.clone())))
                    .into()
            } else {
                widget::space::horizontal().width(Length::Fixed(20.0)).into()
            };
            let cpu_text = widget::text::caption(format!("{} %", decimal(f64::from(group.cpu), 1)))
                .width(Length::Fixed(48.0))
                .align_x(iced::alignment::Horizontal::Right);
            let memory_text = widget::text::caption(bytes(group.memory as f64))
                .width(Length::Fixed(64.0))
                .align_x(iced::alignment::Horizontal::Right);
            let (cpu_text, memory_text) = match sort {
                Sort::Cpu => (cpu_text.font(cosmic::font::semibold()), memory_text.class(theme::Text::Custom(dimmed))),
                Sort::Memory => (cpu_text.class(theme::Text::Custom(dimmed)), memory_text.font(cosmic::font::semibold())),
            };
            column = column.push(
                widget::row::with_children(vec![
                    avatar.into(),
                    widget::text::caption(name).width(Length::Fill).into(),
                    cpu_text.into(),
                    memory_text.into(),
                    end,
                ])
                .spacing(8)
                .padding([2, 0])
                .align_y(Alignment::Center),
            );
        }
        column.into()
    }

    fn system<'a>(&'a self, now: &'a super::sample::Reading) -> Element<'a, Message> {
        let t = texts();
        let info = &self.info;
        widget::column::with_children(vec![
            line(t.computer.to_owned(), info.hostname.clone()),
            line(t.os.to_owned(), info.os.clone()),
            line(t.kernel.to_owned(), info.kernel.clone()),
            line(t.processor.to_owned(), info.cpu_model.clone()),
            line(t.uptime.to_owned(), uptime(now.uptime)),
            line(t.tasks.to_owned(), now.tasks.to_string()),
            line(
                t.load_average.to_owned(),
                now.load.iter().map(|l| decimal(f64::from(*l), 2)).collect::<Vec<_>>().join(" · "),
            ),
        ])
        .spacing(6)
        .into()
    }

    /// Arranging: each panel shown or not, a place up or down; what the bar
    /// shows.
    pub fn edit_view(&self) -> Element<'_, Message> {
        let t = texts();
        let spacing = theme::spacing();
        let mut panels = widget::column::with_capacity(12).spacing(2);
        let last = self.settings.order.len().saturating_sub(1);
        for (at, panel) in self.settings.order.iter().copied().enumerate() {
            let shown = !self.settings.hidden.contains(&panel);
            let arrow = |name: &str, message: Option<Message>| -> Element<'_, Message> {
                button::icon(icon(name))
                    .icon_size(14)
                    .padding(4)
                    .class(theme::Button::Text)
                    .on_press_maybe(message)
                    .into()
            };
            panels = panels.push(
                widget::row::with_children(vec![
                    badge(panel_icon(panel), panel_hue(panel), 26.0),
                    widget::text::body(panel.name()).width(Length::Fill).into(),
                    arrow("go-up-symbolic", (at > 0).then_some(Message::Shift(panel, false))),
                    arrow("go-down-symbolic", (at < last).then_some(Message::Shift(panel, true))),
                    widget::toggler(shown).on_toggle(move |_| Message::ToggleHidden(panel)).into(),
                ])
                .spacing(spacing.space_xs)
                .padding([2, 0])
                .align_y(Alignment::Center),
            );
        }
        let mut bar = widget::column::with_capacity(6).spacing(2);
        // The power always shows; the others never, always or past their
        // limit, but the network, on or off.
        for item in BarItem::ALL.into_iter().filter(|i| *i != BarItem::Power) {
            let name = widget::text::body(item.name());
            bar = bar.push(match self.bar_models.iter().find(|(i, _)| *i == item) {
                // Under its name, as wide as the popup.
                Some((_, model)) => widget::column::with_children(vec![
                    name.into(),
                    widget::segmented_control::horizontal(model)
                        .on_activate(move |entity| Message::BarMode(item, entity))
                        .into(),
                ])
                .spacing(spacing.space_xxs)
                .padding([4, 0])
                .into(),
                None => Element::from(
                    widget::row::with_children(vec![
                        name.width(Length::Fill).into(),
                        widget::toggler(self.settings.shows_on_bar(item))
                            .on_toggle(move |_| Message::ToggleBar(item))
                            .into(),
                    ])
                    .padding([4, 0])
                    .align_y(Alignment::Center),
                ),
            });
        }
        bar = bar.push(faint(t.bar_limits));
        let content = widget::column::with_children(vec![
            widget::row::with_children(vec![
                widget::text::heading(t.arrange).width(Length::Fill).into(),
                button::text(t.restore).on_press(Message::ResetLayout).into(),
                button::text(t.done).class(theme::Button::Suggested).on_press(Message::Edit(false)).into(),
            ])
            .spacing(spacing.space_xs)
            .align_y(Alignment::Center)
            .into(),
            faint(t.panels_hint).into(),
            panels.into(),
            widget::divider::horizontal::light().into(),
            widget::text::heading(t.on_bar).into(),
            bar.into(),
        ])
        .spacing(spacing.space_s)
        .padding(spacing.space_m);
        crate::pages::panel(widget::scrollable(content).height(Length::Shrink))
    }
}
