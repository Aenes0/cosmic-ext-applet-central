// SPDX-License-Identifier: GPL-3.0-only

//! What the user arranges: which icons the bar shows and when, the order of
//! the popup's sections, which of them are folded, and the main monitor.

use std::collections::{HashMap, HashSet};

use cosmic::cosmic_config::{Config, ConfigGet, ConfigSet};

use crate::{texts::texts, tiles::Tile};

fn config() -> Option<Config> {
    Config::new(crate::APP_ID, 1).ok()
}

fn get<T: serde::de::DeserializeOwned>(key: &str) -> Option<T> {
    config()?.get::<T>(key).ok()
}

fn set<T: serde::Serialize>(key: &str, value: T) {
    if let Some(Err(err)) = config().map(|c| c.set(key, value)) {
        tracing::warn!("cannot save {key}: {err}");
    }
}

/// When an icon shows on the bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Show {
    Always,
    /// Only while it's on: a tile switched on, a new notification, an app at
    /// the camera.
    Active,
    Never,
}

impl Show {
    fn id(self) -> &'static str {
        match self {
            Show::Always => "always",
            Show::Active => "active",
            Show::Never => "never",
        }
    }

    fn from_id(id: &str) -> Option<Show> {
        [Show::Always, Show::Active, Show::Never]
            .into_iter()
            .find(|s| s.id() == id)
    }

    pub fn name(self) -> &'static str {
        let t = texts();
        match self {
            Show::Always => t.always,
            Show::Active => t.when_active,
            Show::Never => t.never,
        }
    }
}

/// Something with an icon on the bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BarItem {
    Privacy,
    Notifications,
    Tile(Tile),
    Network,
    Volume,
    Peripherals,
    Battery,
    /// The time over the date, at the bar's end.
    Clock,
}

impl BarItem {
    fn id(self) -> String {
        match self {
            BarItem::Privacy => "privacy".into(),
            BarItem::Notifications => "notifications".into(),
            BarItem::Tile(tile) => tile.id().into(),
            BarItem::Network => "network".into(),
            BarItem::Volume => "volume".into(),
            BarItem::Peripherals => "peripherals".into(),
            BarItem::Battery => "battery".into(),
            BarItem::Clock => "clock".into(),
        }
    }

    pub fn name(self) -> &'static str {
        let t = texts();
        match self {
            BarItem::Privacy => t.privacy,
            BarItem::Notifications => crate::i18n::strings().notifications,
            BarItem::Tile(tile) => tile.name(),
            BarItem::Network => t.network,
            BarItem::Volume => crate::i18n::strings().volume,
            BarItem::Peripherals => t.peripherals,
            BarItem::Battery => crate::i18n::strings().battery,
            BarItem::Clock => t.clock,
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            BarItem::Privacy => "camera-web-symbolic",
            BarItem::Notifications => "notification-symbolic",
            BarItem::Tile(tile) => tile.icon(),
            BarItem::Network => "network-wired-symbolic",
            BarItem::Volume => "audio-volume-high-symbolic",
            BarItem::Peripherals => "input-mouse-symbolic",
            BarItem::Battery => "battery-good-symbolic",
            BarItem::Clock => "preferences-system-time-symbolic",
        }
    }

    /// The choices that make sense for it: the network is always there or
    /// never, a tile can show while it's off too.
    pub fn choices(self) -> &'static [Show] {
        match self {
            BarItem::Privacy => &[Show::Active, Show::Never],
            BarItem::Network
            | BarItem::Volume
            | BarItem::Peripherals
            | BarItem::Battery
            | BarItem::Clock => {
                &[Show::Always, Show::Never]
            }
            BarItem::Notifications | BarItem::Tile(_) => &[Show::Always, Show::Active, Show::Never],
        }
    }
}

/// When each icon shows, as picked; the rest as by default.
#[derive(Clone, Debug, Default)]
pub struct PanelIcons {
    picked: HashMap<String, Show>,
    /// The old switch, "show the tiles that are on": off hides them all.
    tiles_hidden: bool,
}

impl PanelIcons {
    pub fn load() -> PanelIcons {
        let picked = get::<HashMap<String, String>>("panel_icons")
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(id, show)| Some((id, Show::from_id(&show)?)))
            .collect();
        PanelIcons {
            picked,
            tiles_hidden: crate::services::HIDE_ACTIVE.get(),
        }
    }

    pub fn get(&self, item: BarItem) -> Show {
        if let Some(show) = self.picked.get(&item.id()) {
            return *show;
        }
        match item {
            BarItem::Privacy => Show::Active,
            BarItem::Notifications => Show::Always,
            BarItem::Tile(_) if self.tiles_hidden => Show::Never,
            BarItem::Tile(_) => Show::Active,
            _ => Show::Always,
        }
    }

    pub fn set(&mut self, item: BarItem, show: Show) {
        self.picked.insert(item.id(), show);
        let saved: HashMap<&str, &str> = self
            .picked
            .iter()
            .map(|(id, show)| (id.as_str(), show.id()))
            .collect();
        set("panel_icons", saved);
    }
}

/// A part of the popup under the grid, that folds away.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    Notifications,
    /// What's playing: folded, a line of it.
    Media,
    /// The month: folded, today only.
    Calendar,
    /// What was copied; folded at first, being long.
    Clipboard,
}

impl Section {
    pub const ALL: [Section; 4] =
        [Section::Notifications, Section::Media, Section::Calendar, Section::Clipboard];

    fn id(self) -> &'static str {
        match self {
            Section::Notifications => "notifications",
            Section::Media => "media",
            Section::Calendar => "calendar",
            Section::Clipboard => "clipboard",
        }
    }

    fn from_id(id: &str) -> Option<Section> {
        Self::ALL.into_iter().find(|s| s.id() == id)
    }

    pub fn icon(self) -> &'static str {
        match self {
            Section::Notifications => "notification-symbolic",
            Section::Media => "audio-x-generic-symbolic",
            Section::Calendar => "x-office-calendar-symbolic",
            Section::Clipboard => "edit-paste-symbolic",
        }
    }

    /// Folded until opened.
    fn folded_at_first(self) -> bool {
        matches!(self, Section::Clipboard)
    }
}

/// Which of the sections are folded.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub folded: HashSet<Section>,
}

impl Layout {
    /// Whether a section shows folded: as picked, else as it comes.
    pub fn is_folded(&self, section: Section) -> bool {
        self.folded.contains(&section) != section.folded_at_first()
    }

    pub fn load() -> Layout {
        let folded = get::<Vec<String>>("folded")
            .unwrap_or_default()
            .iter()
            .filter_map(|id| Section::from_id(id))
            .collect();
        Layout { folded }
    }

    pub fn toggle_fold(&mut self, section: Section) {
        if !self.folded.remove(&section) {
            self.folded.insert(section);
        }
        set("folded", self.folded.iter().map(|s| s.id()).collect::<Vec<_>>());
    }
}

/// Whether the old list of tiles was made a grid, with the sliders and
/// media on it.
pub fn grid_ready() -> bool {
    get::<bool>("grid_ready").unwrap_or(false)
}

pub fn set_grid_ready() {
    set("grid_ready", true);
}

/// The monitor whose brightness the main slider sets, by connector.
pub fn primary_display() -> Option<String> {
    get::<String>("primary_display")
}

pub fn set_primary_display(name: &str) {
    set("primary_display", name);
}
