// SPDX-License-Identifier: GPL-3.0-only

//! Dragging icons: along the bar (reorder), onto the ^ (hide), and from the
//! box to the bar (show). The dragged content is the key (`dnd::TrayDrag`), so
//! a drop works between the instances of every monitor; the written config
//! carries the result to the others.

use cosmic::{
    Element, Task, app,
    iced::{Length, Vector, window},
    widget::{DndSource, Id, Space},
};

use super::{
    app::{App, Msg},
    status_menu,
};
use crate::tray::{dnd::TrayDrag, layout};

/// Fixed ids of the two drop targets: an id that changed between frames would
/// lose the offer in the middle of a drag (as `DND_FAVORITES` in the taskbar).
pub(super) const DND_BAR: u64 = u64::MAX;
pub(super) const DND_ARROW: u64 = u64::MAX - 1;

/// Pixels the pointer moves, button held, before a press becomes a drag.
const DRAG_THRESHOLD: f32 = 16.0;

/// Where a drag started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Bar,
    Box,
}

/// A drag started by this instance.
#[derive(Debug, Clone)]
pub(super) struct Drag {
    pub(super) menu_id: usize,
    pub(super) key: String,
    pub(super) origin: Origin,
}

/// One button-sized cell of the bar as drawn.
pub(super) enum Cell {
    Item(usize),
    /// The space that opens where the dragged icon will land.
    Gap,
}

/// The bar as drawn during (or outside) a drag.
pub(super) struct BarRow {
    pub(super) cells: Vec<Cell>,
    /// Sources of the dragged key, drawn with no size: the source widget must
    /// stay in the tree, or the end of the drag (`DragEnd`) never arrives.
    pub(super) ghosts: Vec<usize>,
}

impl App {
    fn key_of(&self, id: usize) -> Option<&str> {
        self.menus.get(&id)?.identity.key()
    }

    /// Bar keys as drawn (repeats included), without the occurrences of `exclude`.
    fn shown_keys(&self, split: &layout::Split, exclude: Option<&str>) -> Vec<String> {
        split
            .bar
            .iter()
            .filter_map(|&id| self.key_of(id))
            .filter(|key| Some(*key) != exclude)
            .map(str::to_owned)
            .collect()
    }

    /// The bar without the dragged icon, with the gap: under the pointer while
    /// it is over the bar; otherwise at the origin of a drag from the bar (a
    /// stable width); for a drag from the box into an empty bar, at 0 (else
    /// the bar has no size and nothing to drop on).
    pub(super) fn bar_row(&self, split: &layout::Split) -> BarRow {
        let dragged = self.drag.as_ref().map(|d| d.key.as_str());
        let from_bar = self.dragging_from_bar();
        let mut cells = Vec::with_capacity(split.bar.len() + 1);
        let mut origin = None;
        for &id in &split.bar {
            if dragged.is_some() && self.key_of(id) == dragged {
                origin.get_or_insert(cells.len());
            } else {
                cells.push(Cell::Item(id));
            }
        }
        let gap = self
            .preview_slot
            .or(origin.filter(|_| from_bar))
            .or_else(|| (self.drag.is_some() && !from_bar && cells.is_empty()).then_some(0));
        if let Some(gap) = gap {
            cells.insert(gap.min(cells.len()), Cell::Gap);
        }
        let ghosts = match &self.drag {
            Some(drag) if drag.origin == Origin::Bar => self
                .menus
                .iter()
                .filter(|(id, menu)| {
                    **id == drag.menu_id || menu.identity.key() == Some(drag.key.as_str())
                })
                .map(|(id, _)| *id)
                .collect(),
            _ => Vec::new(),
        };
        BarRow { cells, ghosts }
    }

    /// Gap under the pointer, `(x, y)` in the applet surface (the bar starts at 0).
    fn slot_at(&self, x: f64, y: f64) -> usize {
        let split = self.split();
        let dragged = self.drag.as_ref().map(|d| d.key.as_str());
        let len = self.shown_keys(&split, dragged).len();
        let pos = if self.is_vertical() { y } else { x };
        layout::drop_slot(len, self.button_len(), pos as f32)
    }

    /// A drop ends a drag from the box: the box closes. The drag is dropped
    /// here too, as the source may vanish with the box before its `DragEnd`.
    fn end_box_drag(&mut self) -> app::Task<Msg> {
        if self.drag.as_ref().is_some_and(|d| d.origin == Origin::Box) {
            self.drag = None;
            return self.close_box();
        }
        Task::none()
    }

    /// The box closed without us asking (grab): a drag from it lost its
    /// source, so its `DragEnd` will not come. The drop still works, as it
    /// carries the key.
    pub(super) fn box_closed(&mut self) -> app::Task<Msg> {
        if self.drag.as_ref().is_some_and(|d| d.origin == Origin::Box) {
            self.drag = None;
            return self.resize_window();
        }
        Task::none()
    }

    pub(super) fn update_drag(&mut self, message: Msg) -> app::Task<Msg> {
        match message {
            Msg::DragStart(menu_id, origin) => {
                let Some(key) = self.key_of(menu_id).map(str::to_owned) else {
                    return Task::none();
                };
                self.drag = Some(Drag { menu_id, key, origin });
                self.preview_slot = None;
                // The dragged button never gets its `on_exit`: the wheel over
                // the gap would scroll the dragged item.
                self.hovered_menu = None;
                self.resize_window()
            }
            Msg::DragEnd => {
                self.preview_slot = None;
                let close = self.end_box_drag();
                self.drag = None;
                Task::batch([close, self.resize_window()])
            }
            Msg::BarEnter(x, y) => {
                self.preview_slot = Some(self.slot_at(x, y));
                self.resize_window()
            }
            Msg::BarMotion(x, y) => {
                self.preview_slot = Some(self.slot_at(x, y));
                Task::none()
            }
            Msg::BarLeave => {
                self.preview_slot = None;
                self.resize_window()
            }
            Msg::DropOnBar(data) => {
                let slot = self.preview_slot.take();
                let close = self.end_box_drag();
                let Some(TrayDrag(key)) = data else {
                    return Task::batch([close, self.resize_window()]);
                };
                let split = self.split();
                // The gap was measured without the dragged key only if the
                // drag is ours; from another instance the item (if on this
                // bar) was still in the queue.
                let measured_without = self.drag.as_ref().is_some_and(|d| d.key == key);
                let slot = match slot {
                    Some(slot) if !measured_without => {
                        let all = self.shown_keys(&split, None);
                        let all: Vec<&str> = all.iter().map(String::as_str).collect();
                        Some(layout::slot_without(&all, &key, slot))
                    }
                    slot => slot,
                };
                let shown = self.shown_keys(&split, Some(&key));
                let shown: Vec<&str> = shown.iter().map(String::as_str).collect();
                let slot = slot.unwrap_or(shown.len());
                let visible = layout::show_at(&self.config.visible, &shown, &key, slot);
                Task::batch([close, self.set_visible(visible), self.resize_window()])
            }
            Msg::DropOnArrow(data) => {
                self.preview_slot = None;
                let close = self.end_box_drag();
                let Some(TrayDrag(key)) = data else {
                    return Task::batch([close, self.resize_window()]);
                };
                let visible = layout::hide(&self.config.visible, &key);
                Task::batch([close, self.set_visible(visible), self.resize_window()])
            }
            _ => Task::none(),
        }
    }
}

/// Wraps an icon of the bar or of the box as a drag source; an item without a
/// key is not one (it cannot be placed).
pub(super) fn source<'a>(
    content: impl Into<Element<'a, Msg>>,
    menu_id: usize,
    menu: &status_menu::State,
    origin: Origin,
    window: Option<window::Id>,
) -> Element<'a, Msg> {
    let Some(key) = menu.identity.key().map(str::to_owned) else {
        return content.into();
    };
    let prefix = match origin {
        Origin::Bar => "tray-bar",
        Origin::Box => "tray-box",
    };
    let icon = menu.icon_handle().clone();
    let source = DndSource::with_id(content, Id::new(format!("{prefix}-{menu_id}")))
        .drag_threshold(DRAG_THRESHOLD)
        .drag_icon(move |_| {
            (
                cosmic::widget::icon(icon.clone()).into(),
                cosmic::iced::core::widget::tree::State::None,
                Vector::ZERO,
            )
        })
        .drag_content(move || TrayDrag(key.clone()))
        .on_start(Some(Msg::DragStart(menu_id, origin)))
        .on_finish(Some(Msg::DragEnd))
        .on_cancel(Some(Msg::DragEnd));
    match window {
        Some(window) => source.window(window).into(),
        None => source.into(),
    }
}

/// The space that opens during a drag: one button, fixed (never `Fill`
/// inside `autosize_window`).
pub(super) fn gap<'a>(len: f32) -> Element<'a, Msg> {
    Space::new()
        .width(Length::Fixed(len))
        .height(Length::Fixed(len))
        .into()
}

/// A source of the dragged key with no size (see `BarRow::ghosts`).
pub(super) fn ghost<'a>(
    menu_id: usize,
    menu: &status_menu::State,
    window: Option<window::Id>,
) -> Element<'a, Msg> {
    let empty = Space::new()
        .width(Length::Fixed(0.0))
        .height(Length::Fixed(0.0));
    source(empty, menu_id, menu, Origin::Bar, window)
}
