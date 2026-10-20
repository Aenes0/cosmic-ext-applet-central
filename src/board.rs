// SPDX-License-Identifier: GPL-3.0-only

//! The popup's main page as One UI draws its quick panel: the controls on
//! grids of five columns — tiles as icons, short pills or pills, brightness
//! standing or across — in up to three bubbles, each in groups between thin
//! lines; and edited in place: the panel stays, each item gets a "−", a tap
//! picks it, a handle resizes it, each bubble a "+", and the rest of the
//! editing, with saving it, opens at the panel's left.

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
        Kind::Brightness | Kind::Divider | Kind::Bubble => None,
    }
}

/// One part of a bubble: a group of items on a grid, or the line before the
/// next group (by its index in the list).
enum Part {
    Group(Vec<(usize, Placed)>),
    Line(usize),
}

/// A bubble: the index of the break that starts it (none for the first),
/// and its parts.
struct Bubble {
    start: Option<usize>,
    parts: Vec<Part>,
}

fn split_bubbles(items: &[Placed]) -> Vec<Bubble> {
    let mut bubbles = vec![Bubble { start: None, parts: vec![Part::Group(Vec::new())] }];
    for (index, placed) in items.iter().copied().enumerate() {
        let bubble = bubbles.last_mut().unwrap();
        match placed.kind {
            Kind::Bubble => bubbles.push(Bubble {
                start: Some(index),
                parts: vec![Part::Group(Vec::new())],
            }),
            Kind::Divider => {
                bubble.parts.push(Part::Line(index));
                bubble.parts.push(Part::Group(Vec::new()));
            }
            _ => {
                if let Some(Part::Group(group)) = bubble.parts.last_mut() {
                    group.push((index, placed));
                }
            }
        }
    }
    bubbles
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
        let mut panels = Vec::with_capacity(6);
        // Editing: saving or not, and what the picked icon does on the bar,
        // above all the rest, which stays.
        if editing {
            panels.push(self.edit_bar());
        }
        // The notifications on top, as Windows 11; the controls' bubbles;
        // what's playing at the bottom, by the bar, with the volume.
        panels.push(panel(self.notifications_panel()));
        for (number, bubble) in split_bubbles(&self.tiles).into_iter().enumerate() {
            let mut rows: Vec<Element<'_, Message>> = Vec::new();
            if number == 0 && !editing {
                rows.push(self.header());
                rows.extend(self.privacy_banner());
            }
            if editing {
                rows.push(self.bubble_title(number, bubble.start));
            }
            for part in bubble.parts {
                match part {
                    Part::Group(items) => {
                        if let Some(grid) = self.grid(&items, editing) {
                            rows.push(grid);
                        }
                    }
                    Part::Line(index) => rows.push(self.line(index, editing)),
                }
            }
            if editing {
                rows.push(self.add_here(number));
            }
            // A bubble left empty shows nothing, but while editing.
            if rows.is_empty() {
                continue;
            }
            // What can be added, just above the bubble whose "+" was pressed.
            if editing && self.adding && self.add_target == number {
                panels.push(self.adding_panel());
            }
            panels.push(panel(
                widget::column::with_children(rows)
                    .spacing(spacing.space_s)
                    .padding(spacing.space_m),
            ));
        }
        panels.push(panel(self.media_panel()));
        let column = widget::column::with_children(panels).spacing(PANEL_GAP);
        if editing {
            widget::scrollable(column).height(Length::Shrink).into()
        } else {
            column.into()
        }
    }

    /// A group of items on a grid; while editing every one shows, plain, to
    /// move or take off.
    fn grid(&self, items: &[(usize, Placed)], editing: bool) -> Option<Element<'_, Message>> {
        let mut shown: Vec<((u16, u16), u64, Element<'_, Message>)> = Vec::new();
        for &(index, placed) in items {
            let element = if editing {
                Some(self.edit_item(index, placed))
            } else {
                self.item(placed)
            };
            if let Some(element) = element {
                shown.push((placed.fit.span(), key(placed.kind), element));
            }
        }
        if shown.is_empty() {
            return None;
        }
        let sizes: Vec<(u16, u16)> = shown.iter().map(|(size, ..)| *size).collect();
        let mut cells = Cells::new(COLUMNS, ROW, gap());
        for (cell, (_, key, element)) in pack(&sizes).into_iter().zip(shown) {
            cells = cells.push_keyed(key, cell, element);
        }
        Some(cells.into())
    }

    /// A thin line between groups; while editing, one to drag or take off.
    fn line(&self, index: usize, editing: bool) -> Element<'_, Message> {
        if !editing {
            return widget::divider::horizontal::default().into();
        }
        let selected = self.drag.is_some_and(|d| d.index == index && d.moved);
        self.marker(index, widget::divider::horizontal::default().into(), selected)
    }

    /// While editing, a bubble's title: the first's name; the others', from
    /// their break, to drag or take off.
    fn bubble_title(&self, number: usize, start: Option<usize>) -> Element<'_, Message> {
        let tx = texts();
        let title = widget::row::with_children(vec![
            widget::icon::icon(icon("window-new-symbolic")).size(14).into(),
            widget::text::caption(format!("{} {}", tx.bubble_word, number + 1)).into(),
        ])
        .spacing(6)
        .align_y(Alignment::Center);
        match start {
            None => widget::container(title).padding([0, 4]).into(),
            Some(index) => self.marker(index, title.into(), false),
        }
    }

    /// A line's or a break's handle while editing: pressed, it drags; its
    /// "−" takes it off.
    fn marker<'a>(&'a self, index: usize, content: Element<'a, Message>, selected: bool) -> Element<'a, Message> {
        let body = widget::mouse_area(
            widget::container(content)
                .center_y(Length::Fixed(24.0))
                .width(Length::Fill)
                .padding([0, 12])
                .class(edit_container(selected)),
        )
        .on_press(Message::DragStart(index))
        .on_release(Message::DragEnd)
        .on_enter(Message::DragOver(index))
        .interaction(iced::mouse::Interaction::Grab);
        widget::row::with_children(vec![
            body.into(),
            badge("list-remove-symbolic", Message::RemoveTile(index)),
        ])
        .spacing(6)
        .align_y(Alignment::Center)
        .into()
    }

    /// A bubble's "+" while editing: where the sidebar adds.
    fn add_here(&self, number: usize) -> Element<'_, Message> {
        let tx = texts();
        let target = self.add_target == number && self.adding;
        button::custom(
            widget::row::with_children(vec![
                widget::icon::icon(icon("list-add-symbolic")).size(16).into(),
                widget::text::caption(if target { tx.adding_here } else { tx.add_here }).into(),
            ])
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .padding([8, 14])
        .width(Length::Fill)
        .class(edit_class(target))
        .on_press(Message::AddTarget(number))
        .into()
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
            Kind::Brightness => {
                if placed.fit == Fit::Vertical {
                    let (value, change, release) = self.brightness_level()?;
                    let level = Level::new(
                        widget::icon::icon(icon("display-brightness-symbolic")).size(20),
                        value,
                        change,
                    )
                    .vertical()
                    .joined()
                    .on_release(release);
                    // The arrow to every monitor's on top, a piece of its own.
                    Some(
                        widget::column::with_children(vec![
                            crate::tiles::slider_end(
                                "go-next-symbolic",
                                Message::ShowPage(Page::Displays),
                                true,
                            ),
                            standing(level, Page::Displays),
                        ])
                        .spacing(2)
                        .into(),
                    )
                } else {
                    self.brightness_slider()
                }
            }
            // Drawn by the bubbles, not on the grid.
            Kind::Divider | Kind::Bubble => None,
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

    /// Editing's bar, on top: cancel, start over, save; and a place that
    /// says what a tap on an icon does, or holds the picked one's settings.
    fn edit_bar(&self) -> Element<'_, Message> {
        let tx = texts();
        let spacing = theme::spacing();
        let buttons = widget::row::with_children(vec![
            button::custom(widget::text::body(tx.cancel))
                .padding([8, 14])
                .class(chip_class(false))
                .on_press(Message::CancelEdit)
                .into(),
            button::custom(
                widget::row::with_children(vec![
                    widget::icon::icon(icon("edit-undo-symbolic")).size(16).into(),
                    widget::text::body(tx.restore).into(),
                ])
                .spacing(spacing.space_xxs)
                .align_y(Alignment::Center),
            )
            .padding([8, 14])
            .class(chip_class(false))
            .on_press(Message::ResetTiles)
            .into(),
            widget::space::horizontal().into(),
            button::custom(
                widget::row::with_children(vec![
                    widget::icon::icon(icon("object-select-symbolic")).size(16).into(),
                    widget::text::body(tx.save).into(),
                ])
                .spacing(spacing.space_xxs)
                .align_y(Alignment::Center),
            )
            .padding([8, 16])
            .class(chip_class(true))
            .on_press(Message::SaveEdit)
            .into(),
        ])
        .spacing(spacing.space_xxs)
        .align_y(Alignment::Center);

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
            .into()
        };
        // The picked icon's settings, or what a tap on one does.
        let about: Element<'_, Message> =
            match self.selected_tile.and_then(|i| Some((i, *self.tiles.get(i)?))) {
                // Its size is the handle's to drag, on the grid.
                Some((_, placed)) if !placed.kind.splits() => {
                    let mut rows: Vec<Element<'_, Message>> = vec![
                        widget::row::with_children(vec![
                            widget::icon::icon(icon(placed.kind.icon())).size(16).into(),
                            widget::text::heading(placed.kind.name()).into(),
                        ])
                        .spacing(spacing.space_xs)
                        .align_y(Alignment::Center)
                        .into(),
                    ];
                    if let Some(item) = bar_item(placed.kind) {
                        let current = self.panel_icons.get(item);
                        rows.push(
                            widget::row::with_children(vec![
                                widget::text::caption(tx.on_bar)
                                    .class(theme::Text::Custom(dimmed))
                                    .width(Length::Fixed(72.0))
                                    .into(),
                                chips(
                                    item.choices()
                                        .iter()
                                        .map(|&show| {
                                            (show.name(), show == current, Message::SetBarIcon(item, show))
                                        })
                                        .collect(),
                                ),
                            ])
                            .align_y(Alignment::Center)
                            .into(),
                        );
                    }
                    widget::column::with_children(rows).spacing(spacing.space_xs).into()
                }
                _ => widget::row::with_children(vec![
                    widget::icon::icon(icon("dialog-information-symbolic")).size(16).into(),
                    widget::text::body(tx.bar_hint)
                        .class(theme::Text::Custom(dimmed))
                        .width(Length::Fill)
                        .into(),
                ])
                .spacing(spacing.space_xs)
                .align_y(Alignment::Center)
                .into(),
            };
        // Kept the same height, picked or not, so the panel doesn't jump.
        let about = widget::container(about)
            .padding([spacing.space_s, spacing.space_s])
            .width(Length::Fill)
            .center_y(Length::Fixed(100.0))
            .class(edit_container(false));
        panel(
            widget::column::with_children(vec![buttons.into(), about.into()])
                .spacing(spacing.space_s)
                .padding(spacing.space_m),
        )
    }

    /// What can be added, a panel above the bubble whose "+" was pressed;
    /// with the icons only on the bar, and the bar's spacing.
    pub fn adding_panel(&self) -> Element<'_, Message> {
        let t = strings();
        let tx = texts();
        let spacing = theme::spacing();
        let mut tools: Vec<Element<'_, Message>> = Vec::new();
        tools.push(
            widget::row::with_children(vec![
                widget::text::heading(format!("{} {}", tx.add_to, self.add_target + 1))
                    .width(Length::Fill)
                    .into(),
                button::icon(icon("window-close-symbolic"))
                    .padding(6)
                    .class(chip_class(false))
                    .on_press(Message::CloseAdding)
                    .into(),
            ])
            .align_y(Alignment::Center)
            .into(),
        );
        let missing: Vec<Kind> = Kind::all()
            .into_iter()
            .filter(|kind| !self.tiles.iter().any(|p| p.kind == *kind))
            .chain([Kind::Divider, Kind::Bubble])
            .collect();
        let add_chip = |kind: Kind| -> Element<'_, Message> {
            let mut trial = self.tiles.clone();
            trial.insert(self.insert_at(self.add_target, kind), Placed::new(kind));
            let allowed = crate::tiles::within_limits(&trial);
            widget::column::with_children(vec![
                button::custom(
                    widget::container(widget::icon::icon(icon(kind.icon())).size(20))
                        .center(Length::Fill),
                )
                .width(Length::Fixed(44.0))
                .height(Length::Fixed(44.0))
                .class(edit_class(false))
                .on_press_maybe(allowed.then_some(Message::AddItem(kind)))
                .into(),
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
            .into()
        };
        tools.push(small_grid(missing.into_iter().map(add_chip).collect(), 80.0));
        tools.push(
            widget::text::caption(tx.limit)
                .class(theme::Text::Custom(dimmed))
                .into(),
        );
        tools.push(widget::divider::horizontal::default().into());

        // Icons with no place on the grid: a tap goes always, while on, never.
        let in_grid = |kind: Kind| self.tiles.iter().any(|p| p.kind == kind);
        let mut bar_only = vec![BarItem::Privacy, BarItem::Notifications, BarItem::Peripherals];
        if self.battery.is_some() {
            bar_only.push(BarItem::Battery);
        }
        if !in_grid(Kind::Tile(crate::tiles::Tile::Wifi)) {
            bar_only.push(BarItem::Network);
        }
        // The volume lives with what's playing, never on the grid.
        bar_only.push(BarItem::Volume);
        tools.push(widget::text::heading(tx.bar_only).into());
        let cards = bar_only
            .into_iter()
            .map(|item| -> Element<'_, Message> {
                let show = self.panel_icons.get(item);
                let choices = item.choices();
                let next = choices
                    [(choices.iter().position(|s| *s == show).unwrap_or(0) + 1) % choices.len()];
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
                widget::tooltip(card, widget::text::body(item.name()), widget::tooltip::Position::Bottom)
                    .into()
            })
            .collect();
        tools.push(small_grid(cards, 60.0));
        tools.push(widget::divider::horizontal::default().into());

        // Further apart on a big screen.
        tools.push(widget::text::heading(t.bar_spacing).into());
        tools.push(
            widget::row::with_children(
                crate::bar_spacing::BarSpacing::ALL
                    .into_iter()
                    .map(|choice| -> Element<'_, Message> {
                        button::custom(widget::text::caption(choice.name()))
                            .padding([6, 12])
                            .class(chip_class(self.bar_spacing == choice))
                            .on_press(Message::SetBarSpacing(choice))
                            .into()
                    })
                    .collect::<Vec<_>>(),
            )
            .spacing(spacing.space_xxs)
            .into(),
        );

        panel(
            widget::column::with_children(tools)
                .spacing(spacing.space_s)
                .padding(spacing.space_m),
        )
    }

    /// Where an item added to a bubble goes: at its end, before the next
    /// bubble's break.
    pub(crate) fn insert_at(&self, bubble: usize, _kind: Kind) -> usize {
        self.tiles
            .iter()
            .enumerate()
            .filter(|(_, p)| p.kind == Kind::Bubble)
            .nth(bubble)
            .map_or(self.tiles.len(), |(index, _)| index)
    }
}

/// Small items in rows of four, in the sidebar.
fn small_grid<'a>(items: Vec<Element<'a, Message>>, height: f32) -> Element<'a, Message> {
    const PER_ROW: u16 = 5;
    let mut cells = Cells::new(PER_ROW, height, gap());
    for (n, item) in items.into_iter().enumerate() {
        let n = n as u16;
        cells = cells.push(
            crate::widgets::Cell {
                column: n % PER_ROW,
                row: n / PER_ROW,
                width: 1,
                height: 1,
            },
            item,
        );
    }
    cells.into()
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
