// SPDX-License-Identifier: GPL-3.0-only

//! The clock on the bar, as Windows 11 has it, the time over the date; and
//! the calendar in the popup: the day in words, and a month to page through,
//! today lit.

use chrono::{Datelike, Local, NaiveDate, Timelike};
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

/// A month: its year and number, 1 to 12.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Month {
    pub year: i32,
    pub month: u32,
}

impl Default for Month {
    fn default() -> Self {
        Month::now()
    }
}

impl Month {
    pub fn now() -> Month {
        let today = Local::now().date_naive();
        Month {
            year: today.year(),
            month: today.month(),
        }
    }

    /// One month on (`1`) or back (`-1`).
    pub fn shift(self, by: i32) -> Month {
        let index = self.year * 12 + self.month as i32 - 1 + by;
        Month {
            year: index.div_euclid(12),
            month: index.rem_euclid(12) as u32 + 1,
        }
    }

    fn first(self) -> NaiveDate {
        NaiveDate::from_ymd_opt(self.year, self.month, 1).expect("a month's first day")
    }

    fn days(self) -> u32 {
        let next = self.shift(1).first();
        (next - self.first()).num_days() as u32
    }
}

/// Every minute, as it turns: the clock wakes once a minute, no more.
pub fn minutes() -> cosmic::iced::Subscription<()> {
    use cosmic::iced::futures::SinkExt;
    cosmic::iced::Subscription::run_with("minutes", |_| {
        cosmic::iced::stream::channel(1, |mut output: cosmic::iced::futures::channel::mpsc::Sender<()>| async move {
            loop {
                let now = Local::now();
                let into = now.timestamp_subsec_millis() as u64 + u64::from(now.second()) * 1000;
                tokio::time::sleep(std::time::Duration::from_millis(60_000 - into.min(59_999))).await;
                if output.send(()).await.is_err() {
                    return;
                }
            }
        })
    })
}

/// The time and the date, for the bar: `18:01` over `20/10/26`.
pub fn bar_lines() -> (String, String) {
    let now = Local::now();
    (now.format("%H:%M").to_string(), now.format("%d/%m/%y").to_string())
}

/// Today in words: "terça-feira, 20 de outubro".
fn today_words() -> String {
    let t = texts();
    let today = Local::now().date_naive();
    let weekday = t.weekdays_long[today.weekday().num_days_from_sunday() as usize];
    let month = t.months[today.month0() as usize];
    format!("{weekday}, {} {} {month}", today.day(), t.of)
}

impl App {
    /// The calendar's panel: today in words, which folds the month away; and
    /// the month, with arrows to the ones around it.
    pub fn calendar_panel(&self) -> Element<'_, Message> {
        let spacing = theme::spacing();
        let folded = self.layout.is_folded(Section::Calendar);
        let header = button::custom(
            widget::row::with_children(vec![
                widget::icon::icon(icon("x-office-calendar-symbolic")).size(16).into(),
                widget::text::heading(today_words()).width(Length::Fill).into(),
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
        .on_press(Message::ToggleFold(Section::Calendar));
        let mut column = widget::column::with_capacity(3)
            .push(header)
            .spacing(spacing.space_xs);
        if !folded {
            column = column.push(self.month_view());
        }
        column.padding(spacing.space_s).into()
    }

    fn month_view(&self) -> Element<'_, Message> {
        let t = texts();
        let spacing = theme::spacing();
        let month = self.calendar_month;
        let today = Local::now().date_naive();
        let arrow = |name: &str, message: Message| -> Element<'_, Message> {
            button::icon(icon(name))
                .icon_size(14)
                .padding(4)
                .class(theme::Button::Text)
                .on_press(message)
                .into()
        };
        let mut title = widget::row::with_capacity(4)
            .push(
                widget::text::caption(format!("{} {}", t.months[month.month as usize - 1], month.year))
                    .font(cosmic::font::semibold())
                    .width(Length::Fill),
            )
            .align_y(Alignment::Center);
        if month != Month::now() {
            title = title.push(
                button::custom(widget::text::caption(t.today))
                    .padding([4, 10])
                    .class(chip_class(false))
                    .on_press(Message::CalendarToday),
            );
        }
        let title = title
            .push(arrow("go-previous-symbolic", Message::CalendarShift(-1)))
            .push(arrow("go-next-symbolic", Message::CalendarShift(1)));

        // The weeks, from Sunday, as calendars in Brazil start them.
        let cell = |content: Element<'static, Message>| -> Element<'static, Message> {
            widget::container(content)
                .center_x(Length::Fill)
                .center_y(Length::Fixed(24.0))
                .into()
        };
        let mut weeks = widget::column::with_capacity(7).spacing(0);
        weeks = weeks.push(widget::row::with_children(
            t.weekdays_short
                .iter()
                .map(|day| {
                    cell(widget::text::caption(*day).class(theme::Text::Custom(dimmed)).into())
                })
                .collect::<Vec<_>>(),
        ));
        let offset = month.first().weekday().num_days_from_sunday();
        let days = month.days();
        let mut row: Vec<Element<'_, Message>> = Vec::with_capacity(7);
        for slot in 0..(offset + days).div_ceil(7) * 7 {
            let day = slot as i64 - offset as i64 + 1;
            let element = if (1..=days as i64).contains(&day) {
                let date = NaiveDate::from_ymd_opt(month.year, month.month, day as u32);
                let is_today = date == Some(today);
                let number = widget::text::caption(day.to_string());
                if is_today {
                    cell(
                        widget::container(number)
                            .center(Length::Fixed(22.0))
                            .class(theme::Container::custom(|theme| {
                                let accent = &theme.cosmic().accent_button;
                                widget::container::Style {
                                    background: Some(cosmic::iced::Background::Color(accent.base.into())),
                                    text_color: Some(crate::widgets::readable(accent.on, accent.base)),
                                    border: cosmic::iced::Border {
                                        radius: 11.0.into(),
                                        ..Default::default()
                                    },
                                    ..Default::default()
                                }
                            }))
                            .into(),
                    )
                } else {
                    cell(number.into())
                }
            } else {
                cell(widget::text::body("").into())
            };
            row.push(element);
            if row.len() == 7 {
                weeks = weeks.push(widget::row::with_children(std::mem::take(&mut row)));
            }
        }
        widget::column::with_children(vec![title.into(), weeks.into()])
            .spacing(spacing.space_xxs)
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn months() {
        let october = Month { year: 2026, month: 10 };
        assert_eq!(october.days(), 31);
        assert_eq!(october.shift(3), Month { year: 2027, month: 1 });
        assert_eq!(october.shift(-10), Month { year: 2025, month: 12 });
        assert_eq!(Month { year: 2028, month: 2 }.days(), 29);
        // 1 October 2026 is a Thursday.
        assert_eq!(october.first().weekday().num_days_from_sunday(), 4);
    }
}
