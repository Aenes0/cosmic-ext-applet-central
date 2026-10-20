// SPDX-License-Identifier: GPL-3.0-only

//! The popup's main page as One UI draws its quick panel: everything on one
//! grid of five columns — tiles as icons, short pills or pills, the sliders
//! standing or across, what's playing — and edited in place: the grid stays,
//! each item gets a "−", a tap picks it to resize, and what can be added
//! shows under it.

use cosmic::{
    Element,
    iced::{self, Alignment, Length},
    theme,
    widget::{self, button},
};

use crate::{
    app::{App, Message, Page},
    i18n::strings,
    layout::{BarItem, Show},
    pages::{PANEL_GAP, dimmed, panel},
    texts::texts,
    tiles::{
        COLUMNS, Fit, Kind, Placed, TileState, badge, chip_class, edit_class, edit_container,
        icon, pack, split,
    },
    widgets::{Cells, Handle, Level, Surface, inherit_colors},
};

/// A row of the grid.
const ROW: f32 = 60.0;

fn gap() -> f32 {
    f32::from(theme::spacing().space_xs)
}

/// A column and the gap after it, in pixels: how far a resize handle goes
/// for one column more.
pub fn column_step() -> f32 {
    let inner = crate::app::WIDTH - 2.0 * f32::from(theme::spacing().space_m) - 2.0;
    let columns = f32::from(COLUMNS);
    (inner - gap() * (columns - 1.0)) / columns + gap()
}

/// Who an item is on the grid, whatever its size or place, for it to slide
/// when moved.
fn key(kind: Kind) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    kind.hash(&mut hasher);
    hasher.finish()
}

/// What a grid item shows on the bar, if anything.
pub fn bar_item(kind: Kind) -> Option<BarItem> {
    match kind {
        Kind::Tile(crate::tiles::Tile::Wifi) => Some(BarItem::Network),
        Kind::Tile(tile) => Some(BarItem::Tile(tile)),
        Kind::Volume => Some(BarItem::Volume),
        Kind::Brightness => None,
    }
}

impl App {
    pub fn main_page(&self) -> Element<'_, Message> {
        self.board(false)
    }

    pub fn edit_page(&self) -> Element<'_, Message> {
        self.board(true)
    }

    fn board(&self, editing: bool) -> Element<'_, Message> {
        let spacing = theme::spacing();
        let mut controls: Vec<Element<'_, Message>> =
            vec![if editing { self.edit_header() } else { self.header() }];
        if !editing {
            controls.extend(self.privacy_banner());
        }
        controls.push(self.grid(editing));
        if editing {
            controls.extend(self.edit_tools());
        }
        let controls = panel(
            widget::column::with_children(controls)
                .spacing(spacing.space_s)
                .padding(spacing.space_m),
        );
        // As Windows 11: the notifications on top, what's playing in its own
        // panel, the controls at the bottom by the bar.
        let mut panels = Vec::with_capacity(3);
        if !editing {
            panels.push(panel(self.notifications_panel()));
            if let Some(card) = self.media_card() {
                panels.push(panel(widget::container(card).padding(spacing.space_s)));
            }
        }
        panels.push(controls);
        let column = widget::column::with_children(panels).spacing(PANEL_GAP);
        if editing {
            // The tools under the grid may outgrow the screen.
            widget::scrollable(column).height(Length::Shrink).into()
        } else {
            column.into()
        }
    }

    /// The grid; while editing every item shows, plain, to move or take off.
    fn grid(&self, editing: bool) -> Element<'_, Message> {
        let mut shown: Vec<((u16, u16), u64, Element<'_, Message>)> = Vec::new();
        for (index, placed) in self.tiles.iter().copied().enumerate() {
            let element = if editing {
                Some(self.edit_item(index, placed))
            } else {
                self.item(placed)
            };
            if let Some(element) = element {
                shown.push((placed.fit.span(), key(placed.kind), element));
            }
        }
        let sizes: Vec<(u16, u16)> = shown.iter().map(|(size, ..)| *size).collect();
        let mut cells = Cells::new(COLUMNS, ROW, gap());
        for (cell, (_, key, element)) in pack(&sizes).into_iter().zip(shown) {
            cells = cells.push_keyed(key, cell, element);
        }
        cells.into()
    }

    /// An item as it works; nothing for what isn't there, like media when
    /// nothing plays.
    fn item(&self, placed: Placed) -> Option<Element<'_, Message>> {
        match placed.kind {
            Kind::Tile(tile) => {
                let state = self.tile_state(tile)?;
                Some(match placed.fit {
                    Fit::Icon => icon_tile(state),
                    Fit::Short => short_tile(state),
                    _ => state.large(),
                })
            }
            Kind::Volume => {
                let volume = self.volume.as_ref()?;
                if placed.fit == Fit::Vertical {
                    let level = Level::new(
                        widget::icon::icon(icon(self.volume_icon())).size(20),
                        volume.level,
                        |level| Message::SetVolume(level * 100.0),
                    )
                    .vertical()
                    .on_release(Message::AdjustDone)
                    .on_icon_press(Some(Message::ToggleMute))
                    .dimmed(volume.muted);
                    Some(standing(level, Page::Sound))
                } else {
                    self.volume_slider()
                }
            }
            Kind::Brightness => {
                if placed.fit == Fit::Vertical {
                    let (value, change, release) = self.brightness_level()?;
                    let level = Level::new(
                        widget::icon::icon(icon("display-brightness-symbolic")).size(20),
                        value,
                        change,
                    )
                    .vertical()
                    .on_release(release);
                    Some(standing(level, Page::Displays))
                } else {
                    self.brightness_slider()
                }
            }
        }
    }

    /// An item while editing: its icon, its name if it has room, a "−" on
    /// its corner; press to drag it, or tap to pick it.
    fn edit_item(&self, index: usize, placed: Placed) -> Element<'_, Message> {
        let dragged = self.drag.is_some_and(|d| d.index == index && d.moved);
        let selected = self.selected_tile == Some(index) || dragged;
        let icon_name = match placed.kind {
            Kind::Tile(tile) => self.tile_state(tile).map_or(tile.icon(), |s| s.icon),
            kind => kind.icon(),
        };
        let mut content = widget::row::with_capacity(3)
            .push(widget::icon::icon(icon(icon_name)).size(20))
            .spacing(10)
            .align_y(Alignment::Center);
        if matches!(placed.fit, Fit::Pill | Fit::Horizontal) {
            content = content.push(
                widget::text::body(placed.kind.name())
                    .font(cosmic::font::semibold())
                    .ellipsize(iced::core::text::Ellipsize::End(
                        iced::core::text::EllipsizeHeightLimit::Lines(1),
                    )),
            );
        }
        if placed.fit == Fit::Short {
            content = content
                .push(widget::space::horizontal())
                .push(widget::icon::icon(icon("go-next-symbolic")).size(14));
        }
        let content = widget::container(content).center_y(Length::Fill);
        let content = match placed.fit {
            Fit::Icon | Fit::Vertical => content.center_x(Length::Fill),
            _ => content.width(Length::Fill).padding([0, 14]),
        };
        let body = widget::mouse_area(content.class(edit_container(selected)))
            .on_press(Message::DragStart(index))
            .on_release(Message::DragEnd)
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
        let mut layers: Vec<Element<'_, Message>> = vec![body.into(), corner.into()];
        // Picked, and with sizes to choose: a handle on its right edge to
        // drag it wider or narrower.
        if self.selected_tile == Some(index) && placed.kind.fits().len() > 1 {
            layers.push(
                widget::container(Handle::new(
                    move |dx| Message::ResizeDrag(index, dx),
                    Message::ResizeEnd,
                ))
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(iced::alignment::Horizontal::Right)
                .align_y(iced::alignment::Vertical::Center)
                .into(),
            );
        }
        iced::widget::stack(layers).into()
    }

    /// Editing's title row: back to the controls, start over, done.
    fn edit_header(&self) -> Element<'_, Message> {
        let t = strings();
        let spacing = theme::spacing();
        widget::row::with_children(vec![
            widget::text::title4(t.edit).width(Length::Fill).into(),
            widget::tooltip(
                button::icon(icon("edit-undo-symbolic"))
                    .padding(8)
                    .class(chip_class(false))
                    .on_press(Message::ResetTiles),
                widget::text::body(t.reset),
                widget::tooltip::Position::Bottom,
            )
            .into(),
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
        ])
        .spacing(spacing.space_xs)
        .align_y(Alignment::Center)
        .into()
    }

    /// Under the grid while editing: the picked item's size and its icon on
    /// the bar; what can be added; what shows only on the bar.
    fn edit_tools(&self) -> Vec<Element<'_, Message>> {
        let t = strings();
        let tx = texts();
        let spacing = theme::spacing();
        let mut tools: Vec<Element<'_, Message>> = Vec::new();

        let chips = |choices: Vec<(&'static str, bool, Message)>| -> Element<'_, Message> {
            widget::row::with_children(
                choices
                    .into_iter()
                    .map(|(label, chosen, message)| {
                        button::custom(widget::text::caption(label))
                            .padding([6, 12])
                            .class(chip_class(chosen))
                            .on_press(message)
                            .into()
                    })
                    .collect::<Vec<_>>(),
            )
            .spacing(spacing.space_xxs)
            .wrap()
            .into()
        };
        let label = |text: &'static str| -> Element<'_, Message> {
            widget::text::caption(text).class(theme::Text::Custom(dimmed)).into()
        };

        match self.selected_tile.and_then(|i| Some((i, *self.tiles.get(i)?))) {
            Some((index, placed)) => {
                tools.push(
                    widget::text::body(placed.kind.name())
                        .font(cosmic::font::semibold())
                        .into(),
                );
                if placed.kind.fits().len() > 1 {
                    tools.push(label(tx.size));
                    tools.push(chips(
                        placed
                            .kind
                            .fits()
                            .iter()
                            .map(|&fit| (fit.name(), fit == placed.fit, Message::SetFit(index, fit)))
                            .collect(),
                    ));
                }
                if let Some(item) = bar_item(placed.kind) {
                    let current = self.panel_icons.get(item);
                    tools.push(label(tx.on_bar));
                    tools.push(chips(
                        item.choices()
                            .iter()
                            .map(|&show| (show.name(), show == current, Message::SetBarIcon(item, show)))
                            .collect(),
                    ));
                }
            }
            None => tools.push(label(tx.tap_to_adjust)),
        }

        // What can go on, as round icons with "+".
        let missing: Vec<Kind> = Kind::all()
            .into_iter()
            .filter(|kind| !self.tiles.iter().any(|p| p.kind == *kind))
            .collect();
        if !missing.is_empty() {
            tools.push(widget::text::heading(t.add).into());
            let mut cells = Cells::new(COLUMNS, 84.0, gap());
            for (n, kind) in missing.into_iter().enumerate() {
                let n = n as u16;
                let chip = button::custom(
                    widget::container(widget::icon::icon(icon(kind.icon())).size(20))
                        .center(Length::Fill),
                )
                .width(Length::Fixed(48.0))
                .height(Length::Fixed(48.0))
                .class(edit_class(false))
                .on_press(Message::AddItem(kind));
                let item: Element<'_, Message> = widget::column::with_children(vec![
                    chip.into(),
                    widget::text::caption(kind.name())
                        .align_x(iced::alignment::Horizontal::Center)
                        .width(Length::Fill)
                        .ellipsize(iced::core::text::Ellipsize::End(
                            iced::core::text::EllipsizeHeightLimit::Lines(2),
                        ))
                        .into(),
                ])
                .align_x(Alignment::Center)
                .spacing(spacing.space_xxs)
                .into();
                cells = cells.push(
                    crate::widgets::Cell {
                        column: n % COLUMNS,
                        row: n / COLUMNS,
                        width: 1,
                        height: 1,
                    },
                    item,
                );
            }
            tools.push(cells.into());
        }

        // Icons with no place on the grid: a tap goes always, while on, never.
        let in_grid = |kind: Kind| self.tiles.iter().any(|p| p.kind == kind);
        let mut bar_only = vec![BarItem::Privacy, BarItem::Notifications, BarItem::Peripherals];
        if self.battery.is_some() {
            bar_only.push(BarItem::Battery);
        }
        if !in_grid(Kind::Tile(crate::tiles::Tile::Wifi)) {
            bar_only.push(BarItem::Network);
        }
        if !in_grid(Kind::Volume) {
            bar_only.push(BarItem::Volume);
        }
        tools.push(widget::text::heading(tx.bar_only).into());
        let mut cells = Cells::new(COLUMNS, 64.0, gap());
        for (n, item) in bar_only.into_iter().enumerate() {
            let n = n as u16;
            let show = self.panel_icons.get(item);
            let choices = item.choices();
            let next = choices[(choices.iter().position(|s| *s == show).unwrap_or(0) + 1) % choices.len()];
            let card = button::custom(
                widget::container(
                    widget::column::with_children(vec![
                        widget::icon::icon(icon(item.icon())).size(20).into(),
                        widget::text::caption(show.name()).into(),
                    ])
                    .align_x(Alignment::Center)
                    .spacing(4),
                )
                .center(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(0)
            .class(edit_class(show != Show::Never))
            .on_press(Message::SetBarIcon(item, next));
            cells = cells.push(
                crate::widgets::Cell {
                    column: n % COLUMNS,
                    row: n / COLUMNS,
                    width: 1,
                    height: 1,
                },
                widget::tooltip(card, widget::text::body(item.name()), widget::tooltip::Position::Bottom),
            );
        }
        tools.push(cells.into());

        // Further apart on a big screen.
        tools.push(widget::text::heading(t.bar_spacing).into());
        tools.push(chips(
            crate::bar_spacing::BarSpacing::ALL
                .into_iter()
                .map(|choice| (choice.name(), self.bar_spacing == choice, Message::SetBarSpacing(choice)))
                .collect(),
        ));
        tools
    }
}

/// A tile as a round icon: a click toggles, a right click opens its page,
/// the name shows on hover.
fn icon_tile<'a>(state: TileState) -> Element<'a, Message> {
    let name = match &state.subtitle {
        Some(subtitle) => format!("{} · {subtitle}", state.title),
        None => state.title.to_owned(),
    };
    let surface = Surface::new(
        widget::container(widget::icon::icon(icon(state.icon)).size(20))
            .center(Length::Fill)
            .class(inherit_colors()),
        state.on,
    )
    .width(Length::Fixed(ROW))
    .height(Length::Fixed(ROW))
    .pill()
    .on_press(state.toggle)
    .on_right_press(state.details);
    widget::tooltip(
        widget::container(surface).center(Length::Fill),
        widget::text::body(name),
        widget::tooltip::Position::Bottom,
    )
    .into()
}

/// A tile as a short pill: its icon, which toggles it, and apart an arrow
/// to its page.
fn short_tile<'a>(state: TileState) -> Element<'a, Message> {
    let name = match &state.subtitle {
        Some(subtitle) => format!("{} · {subtitle}", state.title),
        None => state.title.to_owned(),
    };
    let pill = split(
        widget::icon::icon(icon(state.icon)).size(20).into(),
        true,
        state.on,
        state.toggle,
        state.details,
    );
    widget::tooltip(pill, widget::text::body(name), widget::tooltip::Position::Bottom).into()
}

/// A slider standing up; a right click opens its page.
fn standing<'a>(level: Level<'a, Message>, page: Page) -> Element<'a, Message> {
    widget::mouse_area(level).on_right_press(Message::ShowPage(page)).into()
}
