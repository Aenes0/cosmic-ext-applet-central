// SPDX-License-Identifier: GPL-3.0-only

//! The quick settings that can go on the panel, and how they look.

use cosmic::{
    Element,
    iced::{self, Alignment, Background, Length, border::Radius},
    theme,
    widget::{self, button},
};

use crate::{
    app::Message,
    i18n::strings,
    widgets::{Round, Surface, inherit_colors, readable, tile_radius},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tile {
    Wifi,
    Vpn,
    Bluetooth,
    Power,
    DoNotDisturb,
    DarkMode,
    Airplane,
    Microphone,
    KeepAwake,
    Tiling,
    Magnifier,
    InvertColors,
    ScreenReader,
    HighContrast,
    Screenshot,
    Workspaces,
    Touchpad,
    HotEdge,
}

impl Tile {
    pub const ALL: [Tile; 18] = [
        Tile::Wifi,
        Tile::Vpn,
        Tile::Bluetooth,
        Tile::Power,
        Tile::DoNotDisturb,
        Tile::DarkMode,
        Tile::Airplane,
        Tile::Microphone,
        Tile::KeepAwake,
        Tile::Tiling,
        Tile::Magnifier,
        Tile::InvertColors,
        Tile::ScreenReader,
        Tile::HighContrast,
        Tile::Screenshot,
        Tile::Workspaces,
        Tile::Touchpad,
        Tile::HotEdge,
    ];

    /// How it's saved in the configuration.
    pub fn id(self) -> &'static str {
        match self {
            Tile::Wifi => "wifi",
            Tile::Vpn => "vpn",
            Tile::Bluetooth => "bluetooth",
            Tile::Power => "power",
            Tile::DoNotDisturb => "do-not-disturb",
            Tile::DarkMode => "dark-mode",
            Tile::Airplane => "airplane",
            Tile::Microphone => "microphone",
            Tile::KeepAwake => "keep-awake",
            Tile::Tiling => "tiling",
            Tile::Magnifier => "magnifier",
            Tile::InvertColors => "invert-colors",
            Tile::ScreenReader => "screen-reader",
            Tile::HighContrast => "high-contrast",
            Tile::Screenshot => "screenshot",
            Tile::Workspaces => "workspaces",
            Tile::Touchpad => "touchpad",
            Tile::HotEdge => "hot-edge",
        }
    }

    pub fn from_id(id: &str) -> Option<Tile> {
        Self::ALL.into_iter().find(|t| t.id() == id)
    }

    /// Whether what it needs is there to put it on: Hot Edge's only with the
    /// extension.
    pub fn available(self) -> bool {
        match self {
            Tile::HotEdge => crate::services::hot_edge_installed(),
            _ => true,
        }
    }

    pub fn name(self) -> &'static str {
        let t = strings();
        match self {
            Tile::Wifi => t.wifi,
            Tile::Vpn => t.vpn,
            Tile::Bluetooth => t.bluetooth,
            Tile::Power => t.power_mode,
            Tile::DoNotDisturb => t.do_not_disturb,
            Tile::DarkMode => t.dark_style,
            Tile::Airplane => t.airplane_mode,
            Tile::Microphone => t.microphone,
            Tile::KeepAwake => t.keep_awake,
            Tile::Tiling => t.tiling,
            Tile::Magnifier => t.magnifier,
            Tile::InvertColors => t.invert_colors,
            Tile::ScreenReader => t.screen_reader,
            Tile::HighContrast => t.high_contrast,
            Tile::Screenshot => t.screenshot,
            Tile::Workspaces => t.workspaces,
            Tile::Touchpad => t.touchpad,
            Tile::HotEdge => t.hot_edge,
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Tile::Wifi => "network-wireless-signal-good-symbolic",
            Tile::Vpn => "network-vpn-symbolic",
            Tile::Bluetooth => "bluetooth-active-symbolic",
            Tile::Power => "power-profile-balanced-symbolic",
            Tile::DoNotDisturb => "notification-disabled-symbolic",
            Tile::DarkMode => "weather-clear-night-symbolic",
            Tile::Airplane => "airplane-mode-symbolic",
            Tile::Microphone => "microphone-sensitivity-muted-symbolic",
            Tile::KeepAwake => "view-reveal-symbolic",
            Tile::Tiling => "com.system76.CosmicAppletTiling-symbolic",
            Tile::Magnifier => "zoom-in-symbolic",
            Tile::InvertColors => "preferences-color-symbolic",
            Tile::ScreenReader => "preferences-desktop-accessibility-symbolic",
            Tile::HighContrast => "color-select-symbolic",
            Tile::Screenshot => "accessories-screenshot-symbolic",
            Tile::Workspaces => "view-app-grid-symbolic",
            Tile::Touchpad => "input-touchpad-symbolic",
            Tile::HotEdge => "input-mouse-symbolic",
        }
    }
}

/// What can go on the popup's grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Tile(Tile),
    Brightness,
}

impl Kind {
    /// Everything there can be, the sliders and media first.
    pub fn all() -> Vec<Kind> {
        let mut all = vec![Kind::Brightness];
        all.extend(Tile::ALL.into_iter().filter(|t| t.available()).map(Kind::Tile));
        all
    }

    fn id(self) -> &'static str {
        match self {
            Kind::Tile(tile) => tile.id(),
            Kind::Brightness => "brightness",
        }
    }

    fn from_id(id: &str) -> Option<Kind> {
        match id {
            "brightness" => Some(Kind::Brightness),
            _ => Tile::from_id(id).map(Kind::Tile),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Tile(tile) => tile.name(),
            Kind::Brightness => crate::texts::texts().brightness,
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Kind::Tile(tile) => tile.icon(),
            Kind::Brightness => "display-brightness-symbolic",
        }
    }

    /// The sizes it comes in, smallest first.
    pub fn fits(self) -> &'static [Fit] {
        match self {
            Kind::Tile(_) => &[Fit::Icon, Fit::Short, Fit::Pill],
            Kind::Brightness => &[Fit::Vertical, Fit::Horizontal],
        }
    }
}

/// How an item sits on the grid of five columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fit {
    /// A round icon, a column.
    Icon,
    /// A short pill, two columns: the icon, and an arrow to more.
    Short,
    /// A pill with its name and state, three columns.
    Pill,
    /// A slider standing a column wide, two rows tall.
    Vertical,
    /// A slider across.
    Horizontal,
}

impl Fit {
    fn id(self) -> &'static str {
        match self {
            Fit::Icon => "icon",
            Fit::Short => "short",
            Fit::Pill => "pill",
            Fit::Vertical => "vertical",
            Fit::Horizontal => "horizontal",
        }
    }

    fn from_id(id: &str) -> Option<Fit> {
        [Fit::Icon, Fit::Short, Fit::Pill, Fit::Vertical, Fit::Horizontal]
            .into_iter()
            .find(|f| f.id() == id)
    }

    pub fn name(self) -> &'static str {
        let t = crate::texts::texts();
        match self {
            Fit::Icon => t.fit_icon,
            Fit::Short => t.fit_short,
            Fit::Pill => t.fit_pill,
            Fit::Vertical => t.fit_vertical,
            Fit::Horizontal => t.fit_horizontal,
        }
    }

    /// Columns and rows it takes.
    pub fn span(self) -> (u16, u16) {
        match self {
            Fit::Icon => (1, 1),
            Fit::Short => (2, 1),
            Fit::Pill => (3, 1),
            Fit::Vertical => (1, 2),
            Fit::Horizontal => (COLUMNS, 1),
        }
    }
}

/// The popup's grid: five columns, so that a row holds five icons, or a pill
/// and a short pill.
pub const COLUMNS: u16 = 5;

/// An item on the grid, and its size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed {
    pub kind: Kind,
    pub fit: Fit,
}

impl Placed {
    /// `bluetooth:short`, `volume:vertical`.
    pub fn id(self) -> String {
        format!("{}:{}", self.kind.id(), self.fit.id())
    }

    /// Also the old ids: `wifi`, a big tile, and `wifi:small`.
    pub fn from_id(id: &str) -> Option<Placed> {
        let (kind, fit) = match id.split_once(':') {
            Some((kind, "small")) => (kind, Some(Fit::Icon)),
            Some((kind, fit)) => (kind, Fit::from_id(fit)),
            None => (id, None),
        };
        let kind = Kind::from_id(kind)?;
        let fit = fit.filter(|f| kind.fits().contains(f)).unwrap_or(Placed::new(kind).fit);
        Some(Placed { kind, fit })
    }

    /// As it goes on at first.
    pub fn new(kind: Kind) -> Placed {
        let fit = match kind {
            Kind::Tile(_) => Fit::Pill,
            Kind::Brightness => Fit::Horizontal,
        };
        Placed { kind, fit }
    }
}

/// Where each item goes, in order: the first free place it fits, from the
/// top left, so a small one fills a gap a bigger one left.
pub fn pack(sizes: &[(u16, u16)]) -> Vec<crate::widgets::Cell> {
    let mut taken: Vec<[bool; COLUMNS as usize]> = Vec::new();
    let mut cells = Vec::with_capacity(sizes.len());
    for &(width, height) in sizes {
        let width = width.clamp(1, COLUMNS);
        let free = |taken: &Vec<[bool; COLUMNS as usize]>, row: usize, column: usize| {
            (row..row + height as usize).all(|r| {
                (column..column + width as usize).all(|c| taken.get(r).is_none_or(|line| !line[c]))
            })
        };
        let mut row = 0;
        let (row, column) = loop {
            if let Some(column) =
                (0..=(COLUMNS - width) as usize).find(|&column| free(&taken, row, column))
            {
                break (row, column);
            }
            row += 1;
        };
        while taken.len() < row + height as usize {
            taken.push([false; COLUMNS as usize]);
        }
        for line in &mut taken[row..row + height as usize] {
            for cell in &mut line[column..column + width as usize] {
                *cell = true;
            }
        }
        cells.push(crate::widgets::Cell {
            column: column as u16,
            row: row as u16,
            width,
            height,
        });
    }
    cells
}

/// What a tile shows and does right now.
pub struct TileState {
    pub icon: &'static str,
    pub title: &'static str,
    pub subtitle: Option<String>,
    pub on: bool,
    pub toggle: Option<Message>,
    /// The page with more of it, behind the arrow.
    pub details: Option<Message>,
}

impl TileState {
    pub fn new(
        icon: &'static str,
        title: &'static str,
        subtitle: Option<String>,
        on: bool,
        toggle: Option<Message>,
        details: Option<Message>,
    ) -> Self {
        Self {
            icon,
            title,
            subtitle,
            on,
            toggle,
            details,
        }
    }

    pub fn large<'a>(self) -> Element<'a, Message> {
        tile(self.icon, self.title, self.subtitle, self.on, self.toggle, self.details)
    }
}

/// Lays out elements in rows of four columns, each taking its share.
pub fn grid<'a>(
    items: Vec<(Element<'a, Message>, u16)>,
    row_height: f32,
) -> Element<'a, Message> {
    const COLUMNS: u16 = 4;
    let spacing = theme::spacing().space_xs;
    let mut rows = widget::column::with_capacity(items.len()).spacing(spacing);
    let mut row: Vec<Element<'a, Message>> = Vec::new();
    let mut used = 0;
    let finish = |row: &mut Vec<Element<'a, Message>>, used: u16| {
        let mut children = std::mem::take(row);
        if used < COLUMNS {
            children.push(
                widget::space::horizontal()
                    .width(Length::FillPortion(COLUMNS - used))
                    .into(),
            );
        }
        widget::row::with_children(children)
            .spacing(spacing)
            .height(Length::Fixed(row_height))
    };
    for (element, columns) in items {
        if used + columns > COLUMNS {
            rows = rows.push(finish(&mut row, used));
            used = 0;
        }
        row.push(
            widget::container(element)
                .width(Length::FillPortion(columns))
                .height(Length::Fill)
                .into(),
        );
        used += columns;
    }
    if !row.is_empty() {
        rows = rows.push(finish(&mut row, used));
    }
    rows.into()
}

/// A round badge on a tile's corner, to take it off or put it on.
pub fn badge<'a>(icon_name: &str, message: Message) -> Element<'a, Message> {
    button::custom(
        widget::container(widget::icon::icon(icon(icon_name)).size(12))
            .center(Length::Fill)
            .class(inherit_colors()),
    )
        .width(Length::Fixed(22.0))
        .height(Length::Fixed(22.0))
        .class(tile_class(true))
        .on_press(message)
        .into()
}

/// A tile while editing: plain, outlined when chosen or dragged.
pub fn edit_container(selected: bool) -> theme::Container<'static> {
    theme::Container::custom(move |theme| {
        let cosmic = theme.cosmic();
        let component = &cosmic.background(theme.transparent).component;
        widget::container::Style {
            background: Some(Background::Color(
                if selected { component.hover } else { component.base }.into(),
            )),
            text_color: Some(component.on.into()),
            icon_color: Some(component.on.into()),
            border: cosmic::iced::Border {
                radius: tile_radius(theme).into(),
                width: if selected { 2.0 } else { 0.0 },
                color: cosmic.accent_color().into(),
            },
            ..Default::default()
        }
    })
}

/// A button like a tile while editing: plain, outlined when chosen.
pub fn edit_class(selected: bool) -> theme::Button {
    let style = move |hovered: bool, theme: &cosmic::Theme| {
        let cosmic = theme.cosmic();
        let component = &cosmic.background(theme.transparent).component;
        button::Style {
            background: Some(Background::Color(
                if hovered { component.hover } else { component.base }.into(),
            )),
            border_radius: (tile_radius(theme) * 0.8).into(),
            border_width: if selected { 2.0 } else { 0.0 },
            border_color: cosmic.accent_color().into(),
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

/// A symbolic icon by name. COSMIC's icon theme lacks some GNOME names, like
/// the power profiles'; those come from Adwaita's files.
pub fn icon(name: &str) -> widget::icon::Handle {
    use std::{collections::HashMap, path::PathBuf, sync::Mutex};
    static FOUND: Mutex<Option<HashMap<String, Option<PathBuf>>>> = Mutex::new(None);

    // The system's icons, and in a Flatpak sandbox where it shows them.
    let find = |theme: &str| {
        ["/usr/share/icons", "/run/host/share/icons", "/app/share/icons"]
            .iter()
            .map(|root| PathBuf::from(root).join(theme))
            .find_map(|dir| {
                ["scalable", "symbolic"].iter().find_map(|kind| {
                    std::fs::read_dir(dir.join(kind)).ok()?.flatten().find_map(|context| {
                        let file = context.path().join(format!("{name}.svg"));
                        file.exists().then_some(file)
                    })
                })
            })
    };
    let adwaita = FOUND
        .lock()
        .unwrap()
        .get_or_insert_default()
        .entry(name.to_owned())
        .or_insert_with(|| find("Cosmic").map_or_else(|| find("Adwaita"), |_| None))
        .clone();
    match adwaita {
        Some(path) => widget::icon::from_path(path).symbolic(true),
        None => widget::icon::from_name(name.to_owned()).symbolic(true).handle(),
    }
}

/// A battery's level, as Adwaita draws it.
pub fn battery_icon(percentage: f64) -> String {
    let level = ((percentage / 10.0).round() as u32 * 10).min(100);
    format!("battery-level-{level}-symbolic")
}

/// UPower and BlueZ name full-color icons.
pub fn symbolic(name: &str) -> String {
    match name.strip_suffix("-symbolic") {
        Some(_) => name.to_owned(),
        None => format!("{name}-symbolic"),
    }
}

/// A quick setting, lit when on: its icon, name and state, which toggle it;
/// and, apart on the pill's other side, an arrow to more of it (a right
/// click anywhere goes there too).
pub fn tile<'a>(
    icon_name: &str,
    title: &'a str,
    subtitle: Option<String>,
    on: bool,
    toggle: Option<Message>,
    details: Option<Message>,
) -> Element<'a, Message> {
    let one_line =
        || iced::core::text::Ellipsize::End(iced::core::text::EllipsizeHeightLimit::Lines(1));
    let mut text = widget::column::with_capacity(2).push(
        widget::text::body(title)
            .font(cosmic::font::semibold())
            .ellipsize(one_line()),
    );
    if let Some(subtitle) = subtitle {
        text = text.push(widget::text::caption(subtitle).ellipsize(one_line()));
    }
    let content = widget::row::with_capacity(2)
        .push(widget::icon::icon(icon(icon_name)).size(20))
        .push(text.width(Length::Fill))
        .spacing(12)
        .align_y(Alignment::Center);
    split(content.into(), false, on, toggle, details)
}

/// A pill in two, as One UI has them: what it is, which toggles it; a thin
/// gap; an arrow to its page. `centered` puts the start's content in its
/// middle, as the short pill's lone icon.
pub fn split<'a>(
    content: Element<'a, Message>,
    centered: bool,
    on: bool,
    toggle: Option<Message>,
    details: Option<Message>,
) -> Element<'a, Message> {
    let start = widget::container(content).center_y(Length::Fill).class(inherit_colors());
    let start = if centered { start.center_x(Length::Fill) } else { start.width(Length::Fill) };
    let Some(details) = details else {
        return Surface::new(start, on)
            .pill()
            .padding([0, 18])
            .on_press(toggle)
            .into();
    };
    let start = Surface::new(start, on)
        .pill()
        .round(Round::Start)
        .padding(if centered { [0, 0, 0, 6] } else { [0, 10, 0, 18] })
        .on_press(toggle)
        .on_right_press(Some(details.clone()));
    let end = Surface::new(
        widget::container(widget::icon::icon(icon("go-next-symbolic")).size(16))
            .center(Length::Fill)
            .class(inherit_colors()),
        on,
    )
    .pill()
    .round(Round::End)
    .width(Length::Fixed(44.0))
    .padding([0, 4, 0, 0])
    .on_press(Some(details.clone()))
    .on_right_press(Some(details));
    widget::row::with_children(vec![start.into(), end.into()])
        .spacing(2)
        .into()
}

/// A slider's other piece, as a split pill's: an arrow to its page, at the
/// end across or on top standing.
pub fn slider_end<'a>(icon_name: &str, message: Message, standing: bool) -> Element<'a, Message> {
    let surface = Surface::new(
        widget::container(widget::icon::icon(icon(icon_name)).size(16))
            .center(Length::Fill)
            .class(inherit_colors()),
        false,
    )
    .pill()
    .on_press(Some(message.clone()))
    .on_right_press(Some(message));
    if standing {
        surface.round(Round::Top).height(Length::Fixed(36.0)).into()
    } else {
        surface.round(Round::End).width(Length::Fixed(44.0)).into()
    }
}

/// A small choice among a few, lit when chosen.
pub fn chip_class(on: bool) -> theme::Button {
    tile_class(on)
}

fn tile_class(on: bool) -> theme::Button {
    let style = move |hovered: bool, theme: &cosmic::Theme| {
        let cosmic = theme.cosmic();
        let component = if on {
            &cosmic.accent_button
        } else {
            &cosmic.background(theme.transparent).component
        };
        let radius = Radius::from(cosmic.radius_xl()[0]);
        button::Style {
            background: Some(Background::Color(
                if hovered { component.hover } else { component.base }.into(),
            )),
            border_radius: radius,
            text_color: Some(readable(component.on, component.base)),
            icon_color: Some(readable(component.on, component.base)),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_into_gaps() {
        // Two vertical sliders beside a pill fill its row and the next.
        let cells = pack(&[(1, 2), (1, 2), (3, 1), (1, 1), (5, 1)]);
        assert_eq!((cells[0].column, cells[0].row), (0, 0));
        assert_eq!((cells[1].column, cells[1].row), (1, 0));
        assert_eq!((cells[2].column, cells[2].row), (2, 0));
        assert_eq!((cells[3].column, cells[3].row), (2, 1));
        assert_eq!((cells[4].column, cells[4].row), (0, 2));
    }

    #[test]
    fn old_ids() {
        assert_eq!(
            Placed::from_id("wifi:small"),
            Some(Placed { kind: Kind::Tile(Tile::Wifi), fit: Fit::Icon })
        );
        assert_eq!(Placed::from_id("bluetooth").map(|p| p.fit), Some(Fit::Pill));
        assert_eq!(Placed::from_id("brightness:vertical").map(|p| p.fit), Some(Fit::Vertical));
        assert_eq!(Placed::from_id("brightness:pill").map(|p| p.fit), Some(Fit::Horizontal));
        // The volume lives with what's playing now.
        assert_eq!(Placed::from_id("volume:vertical"), None);
    }
}
