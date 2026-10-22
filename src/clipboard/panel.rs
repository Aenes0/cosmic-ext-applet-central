// SPDX-License-Identifier: GPL-3.0-only

//! The clipboard's panel: the history's cards, in the board.

use super::{Entry, preview, shown};

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
                // A few show, the others scroll in their own panel.
                let shown = crate::board::CLIPBOARD_SHOWN as f32;
                column = column.push(
                    widget::container(
                        widget::scrollable(widget::column::with_children(cards).spacing(spacing.space_xxs))
                            .height(Length::Shrink)
                            .width(Length::Fill),
                    )
                    .max_height(shown * crate::board::CLIPBOARD_HEIGHT + (shown - 1.0) * f32::from(spacing.space_xxs)),
                );
            }
        }
        column.padding(spacing.space_s).into()
    }

    fn clipboard_card<'a>(&'a self, entry: &'a Entry) -> Element<'a, Message> {
        let t = texts();
        let spacing = theme::spacing();
        let content: Element<'_, Message> = match (&entry.text, &entry.image) {
            (Some(text), _) => widget::text::body(preview(text))
                .ellipsize(cosmic::iced::core::text::Ellipsize::End(
                    cosmic::iced::core::text::EllipsizeHeightLimit::Lines(2),
                ))
                .width(Length::Fill)
                .into(),
            (None, Some(path)) => widget::row::with_children(vec![
                widget::image(widget::image::Handle::from_path(shown(path)))
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

/// The service's side of D-Bus, as the applets see it.
#[zbus::proxy(
    interface = "io.github.Aenes0.CosmicCentral.Clipboard",
    default_service = "io.github.Aenes0.CosmicCentral.Clipboard",
    default_path = "/io/github/Aenes0/CosmicCentral/Clipboard"
)]
trait History {
    fn copy(&self, id: u64) -> zbus::Result<()>;
    fn pin(&self, id: u64) -> zbus::Result<()>;
    fn remove(&self, id: u64) -> zbus::Result<()>;
    fn clear(&self) -> zbus::Result<()>;
    #[zbus(signal)]
    fn changed(&self) -> zbus::Result<()>;
}

/// What the applet asks of the service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Copy(u64),
    Pin(u64),
    Remove(u64),
    Clear,
}

/// Ask the service, on this connection. An error when it isn't running.
pub async fn request(conn: &zbus::Connection, action: Action) -> zbus::Result<()> {
    let history = HistoryProxy::new(conn).await?;
    match action {
        Action::Copy(id) => history.copy(id).await,
        Action::Pin(id) => history.pin(id).await,
        Action::Remove(id) => history.remove(id).await,
        Action::Clear => history.clear().await,
    }
}

/// Ask the service on the session bus; a failure is only logged.
pub async fn ask(action: Action) {
    static BUS: tokio::sync::OnceCell<zbus::Connection> = tokio::sync::OnceCell::const_new();
    let result = async {
        // Long enough for D-Bus to start the service the first time.
        let session = || async { zbus::connection::Builder::session()?.method_timeout(std::time::Duration::from_secs(30)).build().await };
        let conn = BUS.get_or_try_init(session).await?;
        request(conn, action).await
    };
    if let Err(err) = result.await {
        tracing::warn!("the clipboard service didn't take {action:?}: {err}");
    }
}

/// Wait before trying the bus again.
const RECONNECT: std::time::Duration = std::time::Duration::from_secs(5);

/// One `()` per `Changed` signal, and one when it starts listening, for what
/// changed meanwhile. Waits and reconnects if the bus or the subscription fails.
pub fn subscription() -> cosmic::iced::Subscription<()> {
    use cosmic::iced::{Subscription, stream};
    use futures::{SinkExt, StreamExt, channel::mpsc};

    Subscription::run_with("clipboard-changes", |_| {
        stream::channel(4, |mut output: mpsc::Sender<()>| async move {
            loop {
                let listening = async {
                    let conn = zbus::Connection::session().await?;
                    HistoryProxy::new(&conn).await?.receive_changed().await
                };
                match listening.await {
                    Ok(mut changes) => {
                        if output.send(()).await.is_err() {
                            return;
                        }
                        while changes.next().await.is_some() {
                            if output.send(()).await.is_err() {
                                return;
                            }
                        }
                    }
                    Err(err) => tracing::warn!("cannot follow the clipboard service: {err}"),
                }
                tokio::time::sleep(RECONNECT).await;
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard::testbus;

    #[tokio::test]
    async fn a_request_without_the_service_is_an_error_not_a_panic() {
        let Some(bus) = testbus::Bus::start() else {
            eprintln!("skipped: no dbus-daemon");
            return;
        };
        // Nobody on this bus owns the clipboard's name.
        let conn = zbus::connection::Builder::address(bus.address()).unwrap().build().await.unwrap();
        assert!(request(&conn, Action::Pin(1)).await.is_err());
    }
}
