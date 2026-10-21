// SPDX-License-Identifier: GPL-3.0-only

//! The weather in the monitor's header, as Samsung's cards show it: on the
//! sky of the hour, a big temperature, the icon moving, the next hours; and,
//! opened, the details and the days.

use chrono::{Datelike, Utc};
use cosmic::{
    Element,
    iced::{self, Alignment, Background, Color, Length},
    theme,
    widget::{self, button},
};

use super::{
    Day, Hour, Sky, Weather,
    scene::{Icon, Light, Scene, Time, moon_phase},
};
use crate::monitor::{Message, Monitor, texts::texts};

const RADIUS: f32 = 16.0;
const PADDING: u16 = 14;
const BIG_ICON: f32 = 76.0;
const HOUR_ICON: f32 = 26.0;
const DAY_ICON: f32 = 24.0;
/// The chance of rain: a light blue that still reads on the sky.
const RAIN_BLUE: Color = Color::from_rgb(0.80, 0.91, 1.0);

fn white(a: f32) -> theme::Text {
    theme::Text::Color(Color { a, ..Color::WHITE })
}

fn text<'a>(value: impl Into<String>, size: f32, a: f32) -> widget::Text<'a, cosmic::Theme> {
    widget::text(value.into()).size(size).class(white(a))
}

/// A button of the sky's glass: white on whatever color it is.
fn glass<'a>(name: &str, message: Message) -> Element<'a, Message> {
    let style = |hovered: bool| button::Style {
        background: hovered.then_some(Background::Color(Color { a: 0.18, ..Color::WHITE })),
        border_radius: 10.0.into(),
        icon_color: Some(Color::WHITE),
        text_color: Some(Color::WHITE),
        ..button::Style::new()
    };
    button::icon(crate::tiles::icon(name))
        .icon_size(16)
        .padding(5)
        .class(theme::Button::Custom {
            active: Box::new(move |_, _| style(false)),
            disabled: Box::new(move |_| style(false)),
            hovered: Box::new(move |_, _| style(true)),
            pressed: Box::new(move |_, _| style(true)),
        })
        .on_press(message)
        .into()
}

/// A glass button, what it does said on hovering.
fn glass_tip<'a>(name: &str, tip: &'a str, message: Message) -> Element<'a, Message> {
    widget::tooltip(glass(name, message), widget::text::body(tip), widget::tooltip::Position::Bottom).into()
}

/// A pane of smoked glass on the sky: darker, so what's on it reads more.
fn pane<'a>(content: impl Into<Element<'a, Message>>) -> widget::Container<'a, Message, cosmic::Theme> {
    widget::container(content).padding([8, 10]).class(theme::Container::custom(|_| widget::container::Style {
        background: Some(Background::Color(Color { a: 0.18, ..Color::BLACK })),
        border: iced::Border { radius: 12.0.into(), ..Default::default() },
        ..Default::default()
    }))
}

pub fn sky_name(sky: Sky) -> &'static str {
    texts().skies[sky as usize]
}

fn uv_level(uv: f32) -> &'static str {
    let level = match uv {
        u if u < 3.0 => 0,
        u if u < 6.0 => 1,
        u if u < 8.0 => 2,
        u if u < 11.0 => 3,
        _ => 4,
    };
    texts().uv_levels[level]
}

/// Hours and minutes there.
fn clock(at: i64, offset: i32) -> String {
    chrono::DateTime::from_timestamp(at + i64::from(offset), 0).map_or_else(String::new, |t| t.format("%H:%M").to_string())
}

fn hour_label(at: i64, offset: i32) -> String {
    chrono::DateTime::from_timestamp(at + i64::from(offset), 0).map_or_else(String::new, |t| t.format("%Hh").to_string())
}

fn degrees(value: f32) -> String {
    format!("{value:.0}°")
}

/// How long ago, short: 5 min, 2 h.
fn ago(seconds: i64) -> String {
    match seconds.max(0) / 60 {
        m if m < 60 => format!("{m} min"),
        m => format!("{} h", m / 60),
    }
}

/// COSMIC_MONITOR_SKY=noite-chuva (or amanhecer-limpo, dia-parcial,
/// entardecer-nublado…, the moon's phase after, as noite-limpo-0.3) shows
/// that sky instead of the real one, for screenshots.
fn preview() -> Option<(Time, Sky, Option<f32>)> {
    static PREVIEW: std::sync::OnceLock<Option<(Time, Sky, Option<f32>)>> = std::sync::OnceLock::new();
    *PREVIEW.get_or_init(|| {
        let value = std::env::var("COSMIC_MONITOR_SKY").ok()?;
        let mut parts = value.split('-');
        let time = match parts.next()? {
            "amanhecer" => Time::Dawn,
            "dia" => Time::Day,
            "entardecer" => Time::Dusk,
            "noite" => Time::Night,
            _ => return None,
        };
        let sky = match parts.next()? {
            "limpo" => Sky::Clear,
            "parcial" => Sky::PartlyCloudy,
            "nublado" => Sky::Cloudy,
            "neblina" => Sky::Fog,
            "garoa" => Sky::Drizzle,
            "pancadas" => Sky::Showers,
            "chuva" => Sky::Rain,
            "trovoada" => Sky::Thunder,
            "neve" => Sky::Snow,
            _ => return None,
        };
        Some((time, sky, parts.next().and_then(|m| m.parse().ok())))
    })
}

/// The sky and the light shown now: the weather's, or the preview's.
fn sky_and_light(weather: Option<&Weather>, now: i64) -> (Sky, Light) {
    let (sky, mut light) = match weather {
        Some(w) => (w.now.sky, w.light(now)),
        None => (Sky::PartlyCloudy, Light { time: Time::Day, sun: 0.4, moon: moon_phase(now), south: true }),
    };
    let Some((time, preview_sky, moon)) = preview() else { return (sky, light) };
    light.time = time;
    light.sun = match time {
        Time::Dawn => 0.0,
        Time::Dusk => 1.0,
        _ => 0.3,
    };
    if let Some(moon) = moon {
        light.moon = moon;
    }
    (preview_sky, light)
}

impl Monitor {
    /// The weather block, on its sky.
    pub(crate) fn weather_block(&self) -> Element<'_, Message> {
        let now = Utc::now().timestamp();
        let (sky, light) = sky_and_light(self.weather.as_ref(), now);
        let content: Element<'_, Message> = if self.place_editing {
            self.place_view()
        } else {
            match &self.weather {
                Some(weather) => self.weather_view(weather, now),
                None => self.weather_waiting(),
            }
        };
        iced::widget::stack(vec![widget::container(content).padding(PADDING).width(Length::Fill).into()])
            .push_under(Scene { sky, light, radius: RADIUS }.view())
            .clip(true)
            .into()
    }

    fn weather_waiting(&self) -> Element<'_, Message> {
        let t = texts();
        let message = if self.weather_fetching { t.weather_loading } else { t.weather_offline };
        widget::row::with_children(vec![
            text(message, 14.0, 0.9).width(Length::Fill).into(),
            glass_tip("mark-location-symbolic", t.change_place, Message::EditPlace(true)),
        ])
        .align_y(Alignment::Center)
        .into()
    }

    fn weather_view<'a>(&'a self, weather: &'a Weather, now: i64) -> Element<'a, Message> {
        let t = texts();
        let today = weather.today(now);
        let (sky, light) = sky_and_light(Some(weather), now);
        // The place, a pin before it; the controls at the end.
        let place = widget::row::with_children(vec![
            widget::icon::icon(crate::tiles::icon("mark-location-symbolic"))
                .size(13)
                .class(theme::Svg::Custom(std::rc::Rc::new(|_| iced::widget::svg::Style {
                    color: Some(Color { a: 0.85, ..Color::WHITE }),
                })))
                .into(),
            text(weather.place.name.clone(), 13.0, 0.9).wrapping(iced::widget::text::Wrapping::None).into(),
        ])
        .spacing(4)
        .align_y(Alignment::Center)
        .width(Length::Fill);
        let (expand, expand_tip) = if self.settings.weather_expanded {
            ("go-up-symbolic", t.less_weather)
        } else {
            ("go-down-symbolic", t.more_weather)
        };
        let top = widget::row::with_children(vec![
            place.into(),
            glass_tip("mark-location-symbolic", t.change_place, Message::EditPlace(true)),
            glass_tip(expand, expand_tip, Message::ToggleWeatherExpanded),
        ])
        .spacing(2)
        .align_y(Alignment::Center);

        // Now: the temperature big and light, the sky, the highs and lows;
        // the icon moving at the side.
        let mut summary = vec![sky_name(sky).to_owned()];
        if let Some(today) = today {
            summary.push(format!("{} {} · {} {}", t.max, degrees(today.max), t.min, degrees(today.min)));
        }
        let mut now_column = widget::column::with_capacity(4)
            .push(widget::text(degrees(weather.now.temp)).size(54).font(cosmic::font::light()).class(white(1.0)))
            .push(text(summary.join(" · "), 13.0, 0.92));
        if let Some(feels) = weather.now.feels {
            now_column = now_column.push(text(format!("{} {}", t.feels_like, degrees(feels)), 12.0, 0.88));
        }
        if let Some((low, high)) = weather.now.spread {
            now_column = now_column.push(
                pane(text(format!("⚠ {}: {}–{}", t.sources_disagree, degrees(low), degrees(high)), 11.0, 0.9))
                    .padding([2, 8]),
            );
        }
        let now_row = widget::row::with_children(vec![
            now_column.width(Length::Fill).into(),
            Icon {
                sky,
                day: light.time != Time::Night,
                size: BIG_ICON,
                moving: true,
                on_panel: false,
                moon: light.moon,
                south: light.south,
            }
            .view(),
        ])
        .align_y(Alignment::Center);

        let mut body = widget::column::with_capacity(6)
            .push(top)
            .push(now_row)
            .push(self.hours_strip(weather, now))
            .spacing(10);
        if self.settings.weather_expanded {
            let sources: Vec<&str> = weather
                .sources
                .iter()
                .map(|s| match s {
                    super::Source::OpenMeteo => "Open-Meteo",
                    super::Source::MetNorway => "MET Norway",
                    super::Source::Wttr => "wttr.in",
                })
                .collect();
            let footer = format!("{} · {} {}", sources.join(" · "), t.updated, ago(now - weather.fetched_at));
            body = body
                .push(self.details(weather, today))
                .push(self.days_list(weather, now))
                .push(text(footer, 10.5, 0.82));
        }
        body.into()
    }

    /// The next hours, side by side, scrolling sideways.
    fn hours_strip<'a>(&'a self, weather: &'a Weather, now: i64) -> Element<'a, Message> {
        let t = texts();
        let hours: Vec<Element<'_, Message>> = weather
            .hours_from(now)
            .take(super::HOURS)
            .enumerate()
            .map(|(at, hour)| self.hour_cell(hour, at == 0, weather, t.now))
            .collect();
        // A thin bar under, not to weigh on the sky.
        widget::scrollable(widget::row::with_children(hours).spacing(4).padding([0, 0, 6, 0]))
            .direction(iced::widget::scrollable::Direction::Horizontal(
                iced::widget::scrollable::Scrollbar::new().width(3).scroller_width(3),
            ))
            .width(Length::Fill)
            .into()
    }

    fn hour_cell<'a>(&self, hour: &Hour, first: bool, weather: &Weather, now_label: &str) -> Element<'a, Message> {
        let label = if first { now_label.to_owned() } else { hour_label(hour.at, weather.utc_offset) };
        let rain = hour.rain_chance.filter(|c| *c >= 20.0).map_or(String::from(" "), |c| format!("{c:.0}%"));
        widget::column::with_children(vec![
            text(label, 11.5, 0.88).into(),
            Icon {
                sky: hour.sky,
                day: hour.day,
                size: HOUR_ICON,
                moving: false,
                on_panel: false,
                moon: moon_phase(hour.at),
                south: weather.place.lat < 0.0,
            }
            .view(),
            text(degrees(hour.temp), 13.0, 1.0).into(),
            widget::text(rain).size(10.5).class(theme::Text::Color(RAIN_BLUE)).into(),
        ])
        .spacing(3)
        .align_x(Alignment::Center)
        .width(Length::Fixed(44.0))
        .into()
    }

    /// Humidity, wind, rain, UV, the sun: two rows of three.
    fn details<'a>(&self, weather: &Weather, today: Option<&Day>) -> Element<'a, Message> {
        let t = texts();
        let dash = || "—".to_owned();
        let cell = |label: &str, value: String| -> Element<'a, Message> {
            pane(
                widget::column::with_children(vec![text(label.to_owned(), 10.5, 0.85).into(), text(value, 14.0, 1.0).into()])
                    .spacing(2),
            )
            .width(Length::Fill)
            .into()
        };
        let offset = weather.utc_offset;
        let uv = today.and_then(|d| d.uv).map_or_else(dash, |uv| format!("{uv:.0} · {}", uv_level(uv)));
        let first = widget::row::with_children(vec![
            cell(t.humidity, weather.now.humidity.map_or_else(dash, |h| format!("{h:.0}%"))),
            cell(t.wind, weather.now.wind_kmh.map_or_else(dash, |w| format!("{w:.0} km/h"))),
            cell(t.rain, today.and_then(|d| d.rain_chance).map_or_else(dash, |c| format!("{c:.0}%"))),
        ])
        .spacing(6);
        let second = widget::row::with_children(vec![
            cell(t.uv, uv),
            cell(t.sunrise, today.and_then(|d| d.sunrise).map_or_else(dash, |s| clock(s, offset))),
            cell(t.sunset, today.and_then(|d| d.sunset).map_or_else(dash, |s| clock(s, offset))),
        ])
        .spacing(6);
        widget::column::with_children(vec![first.into(), second.into()]).spacing(6).into()
    }

    /// The days: the week's lowest to highest as one scale, each day's span
    /// a bar on it, as Samsung's.
    fn days_list<'a>(&self, weather: &Weather, now: i64) -> Element<'a, Message> {
        let t = texts();
        let days: Vec<&Day> = weather.days_from(now).take(super::DAYS).collect();
        let low = days.iter().map(|d| d.min).fold(f32::INFINITY, f32::min);
        let high = days.iter().map(|d| d.max).fold(f32::NEG_INFINITY, f32::max);
        let rows: Vec<Element<'_, Message>> = days
            .iter()
            .enumerate()
            .map(|(at, day)| {
                let name = if at == 0 { t.today.to_owned() } else { t.weekdays[day.date.weekday().num_days_from_sunday() as usize].to_owned() };
                let rain = day.rain_chance.filter(|c| *c >= 20.0).map_or(String::new(), |c| format!("{c:.0}%"));
                widget::row::with_children(vec![
                    text(name, 13.0, 1.0).width(Length::Fixed(46.0)).into(),
                    Icon {
                        sky: day.sky,
                        day: true,
                        size: DAY_ICON,
                        moving: false,
                        on_panel: false,
                        moon: 0.5,
                        south: false,
                    }
                    .view(),
                    widget::text(rain)
                        .size(11.0)
                        .class(theme::Text::Color(RAIN_BLUE))
                        .width(Length::Fixed(36.0))
                        .into(),
                    text(degrees(day.min), 13.0, 0.85).width(Length::Fixed(32.0)).into(),
                    span_bar(day.min, day.max, low, high),
                    text(degrees(day.max), 13.0, 1.0).width(Length::Fixed(32.0)).into(),
                ])
                .spacing(6)
                .align_y(Alignment::Center)
                .into()
            })
            .collect();
        pane(widget::column::with_children(rows).spacing(6)).width(Length::Fill).into()
    }

    /// A CEP or a city: typed, searched, picked.
    fn place_view(&self) -> Element<'_, Message> {
        let t = texts();
        let input = widget::text_input(t.place_hint.to_owned(), self.place_input.clone())
            .on_input(Message::PlaceInput)
            .on_submit(|_| Message::SearchPlace)
            .width(Length::Fill);
        let mut column = widget::column::with_capacity(4)
            .push(
                widget::row::with_children(vec![
                    text(t.change_place, 14.0, 1.0).width(Length::Fill).into(),
                    glass("window-close-symbolic", Message::EditPlace(false)),
                ])
                .align_y(Alignment::Center),
            )
            .push(
                widget::row::with_children(vec![
                    input.into(),
                    widget::button::suggested(t.search).on_press(Message::SearchPlace).into(),
                ])
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .spacing(8);
        if self.place_searching {
            column = column.push(text(t.searching_place, 12.0, 0.88));
        } else if let Some(places) = &self.place_results {
            if places.is_empty() {
                column = column.push(text(t.nothing_found, 12.0, 0.88));
            }
            for place in places {
                let label = widget::column::with_children(vec![
                    text(place.name.clone(), 13.0, 1.0).into(),
                    text(place.detail.clone(), 11.0, 0.85).into(),
                ]);
                let style = |hovered: bool| button::Style {
                    background: Some(Background::Color(Color { a: if hovered { 0.30 } else { 0.18 }, ..Color::BLACK })),
                    border_radius: 12.0.into(),
                    ..button::Style::new()
                };
                column = column.push(
                    button::custom(label)
                        .width(Length::Fill)
                        .padding([6, 10])
                        .class(theme::Button::Custom {
                            active: Box::new(move |_, _| style(false)),
                            disabled: Box::new(move |_| style(false)),
                            hovered: Box::new(move |_, _| style(true)),
                            pressed: Box::new(move |_, _| style(true)),
                        })
                        .on_press(Message::PickPlace(place.clone())),
                );
            }
        }
        column.into()
    }
}

/// A day's span on the week's scale, cool to warm.
fn span_bar<'a>(min: f32, max: f32, low: f32, high: f32) -> Element<'a, Message> {
    let range = (high - low).max(1.0);
    let start = ((min - low) / range).clamp(0.0, 1.0);
    let end = ((max - low) / range).clamp(start, 1.0);
    let portion = |f: f32| ((f * 100.0).round() as u16).max(1);
    let track = |a: f32| {
        theme::Container::custom(move |_| widget::container::Style {
            background: Some(Background::Color(Color { a, ..Color::WHITE })),
            border: iced::Border { radius: 3.0.into(), ..Default::default() },
            ..Default::default()
        })
    };
    let lit = widget::container(widget::Space::new().width(Length::Fill).height(Length::Fixed(5.0)))
        .width(Length::FillPortion(portion(end - start)))
        .class(theme::Container::custom(|_| widget::container::Style {
            background: Some(Background::Gradient(iced::Gradient::Linear(
                iced::gradient::Linear::new(iced::Radians(std::f32::consts::FRAC_PI_2))
                    .add_stop(0.0, Color::from_rgb8(0x7C, 0xC4, 0xFF))
                    .add_stop(1.0, Color::from_rgb8(0xFF, 0xB3, 0x47)),
            ))),
            border: iced::Border { radius: 3.0.into(), ..Default::default() },
            ..Default::default()
        }));
    widget::container(
        widget::row::with_children(vec![
            widget::Space::new().width(Length::FillPortion(portion(start))).height(Length::Fixed(5.0)).into(),
            lit.into(),
            widget::Space::new().width(Length::FillPortion(portion(1.0 - end))).height(Length::Fixed(5.0)).into(),
        ]),
    )
    .width(Length::Fill)
    .class(track(0.18))
    .into()
}
