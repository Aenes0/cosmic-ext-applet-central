// SPDX-License-Identifier: GPL-3.0-only

//! What the user picks for the monitor: what the bar shows, the panels'
//! order, which are hidden or folded, and the energy's price.

use std::collections::HashMap;

use cosmic::cosmic_config::{Config, ConfigGet, ConfigSet};

use super::texts::texts;

fn config() -> Option<Config> {
    Config::new(super::APP_ID, 1).ok()
}

fn get<T: serde::de::DeserializeOwned>(key: &str) -> Option<T> {
    config()?.get::<T>(key).ok()
}

fn set<T: serde::Serialize>(key: &str, value: T) {
    if let Some(Err(err)) = config().map(|c| c.set(key, value)) {
        tracing::warn!("cannot save {key}: {err}");
    }
}

/// A panel of the popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Panel {
    Energy,
    Cpu,
    Gpu,
    Memory,
    Disks,
    Network,
    Sensors,
    Processes,
    System,
}

impl Panel {
    pub const ALL: [Panel; 9] = [
        Panel::Energy,
        Panel::Cpu,
        Panel::Gpu,
        Panel::Memory,
        Panel::Disks,
        Panel::Network,
        Panel::Sensors,
        Panel::Processes,
        Panel::System,
    ];

    fn id(self) -> &'static str {
        match self {
            Panel::Energy => "energy",
            Panel::Cpu => "cpu",
            Panel::Gpu => "gpu",
            Panel::Memory => "memory",
            Panel::Disks => "disks",
            Panel::Network => "network",
            Panel::Sensors => "sensors",
            Panel::Processes => "processes",
            Panel::System => "system",
        }
    }

    fn from_id(id: &str) -> Option<Panel> {
        Self::ALL.into_iter().find(|p| p.id() == id)
    }

    pub fn name(self) -> &'static str {
        let t = texts();
        match self {
            Panel::Energy => t.energy,
            Panel::Cpu => t.cpu,
            Panel::Gpu => t.gpu,
            Panel::Memory => t.memory,
            Panel::Disks => t.disks,
            Panel::Network => t.network,
            Panel::Sensors => t.sensors,
            Panel::Processes => t.processes,
            Panel::System => t.system,
        }
    }
}

/// Something the bar can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BarItem {
    Weather,
    Cpu,
    Gpu,
    Memory,
    Power,
    Network,
}

impl BarItem {
    pub const ALL: [BarItem; 6] =
        [BarItem::Weather, BarItem::Cpu, BarItem::Gpu, BarItem::Memory, BarItem::Power, BarItem::Network];

    fn id(self) -> &'static str {
        match self {
            BarItem::Weather => "weather",
            BarItem::Cpu => "cpu",
            BarItem::Gpu => "gpu",
            BarItem::Memory => "memory",
            BarItem::Power => "power",
            BarItem::Network => "network",
        }
    }

    /// Shown never, always, or past its limit; the rest, on or off. The
    /// energy always shows.
    pub fn has_modes(self) -> bool {
        matches!(self, BarItem::Cpu | BarItem::Gpu | BarItem::Memory)
    }

    pub fn name(self) -> &'static str {
        let t = texts();
        match self {
            BarItem::Weather => t.bar_weather,
            BarItem::Cpu => t.bar_cpu,
            BarItem::Gpu => t.bar_gpu,
            BarItem::Memory => t.memory,
            BarItem::Power => t.bar_power,
            BarItem::Network => t.bar_network,
        }
    }
}

/// When an item with modes shows on the bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarMode {
    Never,
    Always,
    /// Past [`USAGE_LIMIT`] or [`TEMP_LIMIT`]; memory, past [`USAGE_LIMIT`]
    /// full.
    Over,
}

/// Percent of use, or of memory full, past which an item shows.
pub const USAGE_LIMIT: f32 = 70.0;
/// Degrees, for the CPU and the GPU.
pub const TEMP_LIMIT: f32 = 65.0;

impl BarMode {
    pub const ALL: [BarMode; 3] = [BarMode::Never, BarMode::Always, BarMode::Over];

    fn id(self) -> &'static str {
        match self {
            BarMode::Never => "never",
            BarMode::Always => "always",
            BarMode::Over => "over",
        }
    }

    pub fn name(self) -> &'static str {
        let t = texts();
        match self {
            BarMode::Never => t.bar_never,
            BarMode::Always => t.bar_always,
            BarMode::Over => t.bar_over,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Settings {
    /// The panels, in order.
    pub order: Vec<Panel>,
    pub hidden: Vec<Panel>,
    pub folded: Vec<Panel>,
    /// What the bar leaves out, of those on or off.
    pub bar_hidden: Vec<BarItem>,
    /// Of those with modes, the ones set.
    pub bar_modes: HashMap<BarItem, BarMode>,
    /// Price of a kWh, in the local money.
    pub tariff: Option<f64>,
    /// Watts the rest of the computer takes, guessed: board, memory,
    /// drives, fans, and what the power supply loses.
    pub rest_watts: f64,
    /// The weather's details and days shown, not just now and the hours.
    pub weather_expanded: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            order: Panel::ALL.to_vec(),
            hidden: Vec::new(),
            folded: Vec::new(),
            bar_hidden: Vec::new(),
            bar_modes: HashMap::new(),
            tariff: None,
            rest_watts: 0.0,
            weather_expanded: false,
        }
    }
}

fn panels(key: &str) -> Option<Vec<Panel>> {
    get::<Vec<String>>(key).map(|ids| ids.iter().filter_map(|id| Panel::from_id(id)).collect())
}

fn save_panels(key: &str, panels: &[Panel]) {
    set(key, panels.iter().map(|p| p.id()).collect::<Vec<_>>());
}

impl Settings {
    pub fn load() -> Settings {
        let mut order = panels("order").unwrap_or_default();
        // New ones at the end.
        for panel in Panel::ALL {
            if !order.contains(&panel) {
                order.push(panel);
            }
        }
        let bar_hidden = get::<Vec<String>>("bar_hidden")
            .unwrap_or_default()
            .iter()
            .filter_map(|id| BarItem::ALL.into_iter().find(|b| b.id() == id))
            .collect::<Vec<_>>();
        let mut bar_modes: HashMap<BarItem, BarMode> = get::<HashMap<String, String>>("bar_modes")
            .unwrap_or_default()
            .iter()
            .filter_map(|(item, mode)| {
                let item = BarItem::ALL.into_iter().find(|b| b.id() == item)?;
                let mode = BarMode::ALL.into_iter().find(|m| m.id() == mode)?;
                Some((item, mode))
            })
            .collect();
        // Hidden before there were modes: still.
        for item in &bar_hidden {
            if item.has_modes() {
                bar_modes.entry(*item).or_insert(BarMode::Never);
            }
        }
        Settings {
            order,
            hidden: panels("hidden").unwrap_or_default(),
            folded: panels("folded").unwrap_or_default(),
            bar_hidden: bar_hidden.into_iter().filter(|b| !b.has_modes()).collect(),
            bar_modes,
            tariff: get::<f64>("tariff").filter(|t| *t > 0.0),
            rest_watts: get::<f64>("rest_watts").unwrap_or(0.0).max(0.0),
            weather_expanded: get::<bool>("weather_expanded").unwrap_or(false),
        }
    }

    pub fn is_folded(&self, panel: Panel) -> bool {
        self.folded.contains(&panel)
    }

    pub fn toggle_fold(&mut self, panel: Panel) {
        toggle(&mut self.folded, panel);
        save_panels("folded", &self.folded);
    }

    pub fn toggle_hidden(&mut self, panel: Panel) {
        toggle(&mut self.hidden, panel);
        save_panels("hidden", &self.hidden);
    }

    /// A panel a place up or down.
    pub fn shift(&mut self, panel: Panel, down: bool) {
        let Some(at) = self.order.iter().position(|p| *p == panel) else { return };
        let to = if down { at + 1 } else { at.wrapping_sub(1) };
        if to < self.order.len() {
            self.order.swap(at, to);
            save_panels("order", &self.order);
        }
    }

    /// Of those on or off.
    pub fn shows_on_bar(&self, item: BarItem) -> bool {
        item == BarItem::Power || !self.bar_hidden.contains(&item)
    }

    pub fn toggle_bar(&mut self, item: BarItem) {
        toggle(&mut self.bar_hidden, item);
        set("bar_hidden", self.bar_hidden.iter().map(|b| b.id()).collect::<Vec<_>>());
    }

    /// Of those with modes: past the limit, unless set.
    pub fn bar_mode(&self, item: BarItem) -> BarMode {
        self.bar_modes.get(&item).copied().unwrap_or(BarMode::Over)
    }

    pub fn set_bar_mode(&mut self, item: BarItem, mode: BarMode) {
        self.bar_modes.insert(item, mode);
        self.save_bar_modes();
    }

    fn save_bar_modes(&self) {
        let modes: HashMap<&str, &str> =
            self.bar_modes.iter().map(|(item, mode)| (item.id(), mode.id())).collect();
        set("bar_modes", modes);
        // What came before modes, emptied of theirs.
        set("bar_hidden", self.bar_hidden.iter().map(|b| b.id()).collect::<Vec<_>>());
    }

    pub fn toggle_weather_expanded(&mut self) {
        self.weather_expanded = !self.weather_expanded;
        set("weather_expanded", self.weather_expanded);
    }

    pub fn set_tariff(&mut self, tariff: Option<f64>) {
        self.tariff = tariff.filter(|t| *t > 0.0);
        set("tariff", self.tariff.unwrap_or(0.0));
    }

    pub fn set_rest_watts(&mut self, watts: f64) {
        self.rest_watts = watts.max(0.0);
        set("rest_watts", self.rest_watts);
    }

    /// The order, the panels and the bar as they come.
    pub fn reset(&mut self) {
        let defaults = Settings::default();
        self.order = defaults.order;
        self.hidden.clear();
        self.folded.clear();
        self.bar_hidden.clear();
        self.bar_modes.clear();
        save_panels("order", &self.order);
        save_panels("hidden", &[]);
        save_panels("folded", &[]);
        self.save_bar_modes();
    }
}

fn toggle<T: PartialEq>(list: &mut Vec<T>, item: T) {
    match list.iter().position(|i| *i == item) {
        Some(at) => {
            list.remove(at);
        }
        None => list.push(item),
    }
}

/// Where the weather is, as last chosen on any screen's applet; at first,
/// CEP 30330-330.
pub fn weather_place() -> super::weather::Place {
    get::<super::weather::Place>("weather_place").unwrap_or_default()
}

pub fn set_weather_place(place: &super::weather::Place) {
    set("weather_place", place);
}
