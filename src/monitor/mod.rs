// SPDX-License-Identifier: GPL-3.0-only

//! The system monitor: its own applet, by the same program, at the bar's
//! other end; a popup as tall as the screen in the Central's panels, with
//! the energy used, the CPU, the GPU, memory, drives, network, sensors,
//! processes and the system. On the bar, what the user picks.

mod chart;
mod config;
mod energy;
mod panels;
mod sample;
mod texts;
mod weather;

use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};

use cosmic::{
    Element, Task, app,
    iced::{
        self, Alignment, Length, Limits, Subscription, event,
        platform_specific::shell::commands::popup::destroy_popup, window,
    },
    theme,
    widget::{self, button},
};

use chart::HISTORY;
use config::{BarItem, BarMode, Panel, Settings};
use widget::segmented_button::{Entity, SingleSelectModel};
use sample::{Info, Reading};

pub const APP_ID: &str = "io.github.Aenes0.CosmicCentral.Monitor";

/// As wide as the Central's, its mirror at the other end.
const WIDTH: f32 = crate::app::WIDTH;
const MAX_HEIGHT: f32 = crate::app::MAX_HEIGHT;
const SCREEN_MARGIN: f32 = 66.0;
/// How long an item past its limit stays on the bar after it's back under.
const OVER_LINGER: Duration = Duration::from_secs(10);
/// What of an item can be past its limit, and shows apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Part {
    Usage,
    Temp,
}

/// How an item's numbers sit on the bar.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Look {
    Normal,
    /// The power: smaller.
    Small,
    /// The network: smaller, one over the other.
    Stacked,
}

/// With the popup closed, how often the counting applet asks for the GPU's
/// energy counter; it counts all since, so nothing is lost between.
const GPU_ENERGY_EVERY: Duration = Duration::from_secs(60);
/// How often the weather is looked at: a file's date, mostly.
const WEATHER_TICK: Duration = Duration::from_secs(60);
/// After a failed reading, how long until the next try.
const WEATHER_RETRY: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    Cpu,
    Memory,
}

#[derive(Clone, Debug)]
pub enum Message {
    /// Time to read again.
    Tick,
    Reading(Box<Reading>),
    TogglePopup,
    PanelPress(window::Id),
    Closed(window::Id),
    Output(Option<String>, Option<i32>),
    ScreenHeight(Option<i32>),
    Resized(window::Id, iced::Size),
    /// Into editing or out of it.
    Edit(bool),
    ToggleFold(Panel),
    ToggleHidden(Panel),
    Shift(Panel, bool),
    ToggleBar(BarItem),
    BarMode(BarItem, Entity),
    ResetLayout,
    ToggleEnergySettings,
    TariffInput(String),
    RestInput(String),
    ResetEnergy,
    Sort(Sort),
    /// A program whose processes to end, asked first; none, not now.
    AskEnd(Option<String>),
    End(String),
    /// Time to look at the weather: what another applet wrote, or to read
    /// it again.
    WeatherTick,
    /// Read, or not; and whether it was tried (another applet may be at it).
    WeatherFetched(Option<Box<weather::Weather>>, bool),
    ToggleWeatherExpanded,
    /// Into choosing a place, or out of it.
    EditPlace(bool),
    PlaceInput(String),
    SearchPlace,
    PlacesFound(Vec<weather::Place>),
    PickPlace(weather::Place),
    None,
}

/// A program's processes together, as Windows lists apps.
#[derive(Clone, Debug)]
pub struct Group {
    pub name: String,
    /// Percent of the whole CPU.
    pub cpu: f32,
    pub memory: u64,
    pub pids: Vec<u32>,
    pub mine: bool,
}

/// What changed since the reading before.
#[derive(Clone, Debug, Default)]
pub struct Rates {
    pub cpu: f32,
    pub cores: Vec<f32>,
    pub cpu_watts: Option<f32>,
    pub gpu_watts: Option<f32>,
    /// Each drive's bytes read and written a second.
    pub disks: Vec<(String, f32, f32)>,
    pub read: f32,
    pub write: f32,
    pub down: f32,
    pub up: f32,
    pub groups: Vec<Group>,
}

#[derive(Default)]
pub struct History {
    pub cpu: VecDeque<f32>,
    pub gpu: VecDeque<f32>,
    pub memory: VecDeque<f32>,
    pub down: VecDeque<f32>,
    pub up: VecDeque<f32>,
    pub read: VecDeque<f32>,
    pub write: VecDeque<f32>,
    pub power: VecDeque<f32>,
}

/// A value to a chart's history, and those it missed while reading slower,
/// eased from the last: the chart's seconds stay seconds.
fn push(history: &mut VecDeque<f32>, value: f32, seconds: usize) {
    let last = history.back().copied().unwrap_or(value);
    let steps = seconds.clamp(1, HISTORY);
    for step in 1..=steps {
        history.push_back(last + (value - last) * step as f32 / steps as f32);
    }
    while history.len() > HISTORY {
        history.pop_front();
    }
}

#[derive(Default)]
pub struct Monitor {
    core: app::Core,
    popup: Option<window::Id>,
    editing: bool,
    pub(crate) settings: Settings,
    pub(crate) info: Info,
    pub(crate) now: Option<Reading>,
    pub(crate) rates: Rates,
    pub(crate) history: History,
    pub(crate) meter: energy::Meter,
    /// While a reading is on its way, no other is asked for.
    reading: bool,
    pub(crate) sort: Option<Sort>,
    pub(crate) asking_end: Option<String>,
    pub(crate) energy_settings: bool,
    pub(crate) tariff_input: String,
    pub(crate) rest_input: String,
    screen_height: Option<i32>,
    screen_height_known: bool,
    button_size: Option<iced::Size>,
    closed_at: Option<Instant>,
    /// Each item with modes: its choice on the arranging.
    bar_models: Vec<(BarItem, SingleSelectModel)>,
    /// When each item's use and temperature were last past their limits.
    over_at: HashMap<(BarItem, Part), Instant>,
    /// The GPU's energy counter as last read, and when.
    gpu_energy: Option<(u64, Instant)>,
    pub(crate) weather: Option<weather::Weather>,
    /// When the written-down weather last changed, as last read.
    weather_seen: Option<std::time::SystemTime>,
    pub(crate) weather_fetching: bool,
    /// When reading it last failed, not to ask again and again offline.
    weather_failed_at: Option<Instant>,
    pub(crate) place_editing: bool,
    pub(crate) place_input: String,
    /// What a search found; none while not searched.
    pub(crate) place_results: Option<Vec<weather::Place>>,
    pub(crate) place_searching: bool,
}

/// The choices of each item with modes, the one set chosen.
fn bar_models(settings: &Settings) -> Vec<(BarItem, SingleSelectModel)> {
    BarItem::ALL
        .into_iter()
        .filter(|item| item.has_modes())
        .map(|item| {
            let mut model = SingleSelectModel::default();
            for mode in BarMode::ALL {
                let entity = model.insert().text(mode.name()).data(mode).id();
                if mode == settings.bar_mode(item) {
                    model.activate(entity);
                }
            }
            (item, model)
        })
        .collect()
}

impl Monitor {
    fn max_height(&self) -> f32 {
        self.screen_height
            .map_or(MAX_HEIGHT, |height| (height as f32 - SCREEN_MARGIN).max(480.0))
    }

    /// Whether processes are read: only while their panel shows.
    fn wants_processes(&self) -> bool {
        self.popup.is_some()
            && !self.editing
            && !self.settings.hidden.contains(&Panel::Processes)
            && !self.settings.is_folded(Panel::Processes)
    }

    fn read(&mut self) -> app::Task<Message> {
        if self.reading {
            return Task::none();
        }
        self.reading = true;
        let full = self.popup.is_some();
        let depth = sample::Depth {
            full,
            procs: self.wants_processes(),
            // Only the one counting needs it; while the popup shows, at
            // every reading, for the panel's numbers.
            gpu_energy: self.meter.counts()
                && (full || self.gpu_energy.is_none_or(|(_, at)| at.elapsed() >= GPU_ENERGY_EVERY)),
        };
        Task::future(async move {
            let reading = tokio::task::spawn_blocking(move || sample::reading(depth))
                .await
                .unwrap_or_else(|_| sample::reading(sample::Depth { procs: false, ..depth }));
            cosmic::action::app(Message::Reading(Box::new(reading)))
        })
    }

    /// Watts now: the CPU's and the GPU's, measured, and the rest, guessed.
    pub(crate) fn total_watts(&self) -> Option<f32> {
        let measured = [self.rates.cpu_watts, self.rates.gpu_watts];
        if measured.iter().all(Option::is_none) {
            return None;
        }
        Some(measured.iter().flatten().sum::<f32>() + self.settings.rest_watts as f32)
    }

    /// Whether an item shows on the bar now.
    fn on_bar(&self, item: BarItem) -> bool {
        if !item.has_modes() {
            return self.settings.shows_on_bar(item);
        }
        match self.settings.bar_mode(item) {
            BarMode::Never => false,
            BarMode::Always => true,
            BarMode::Over => self.lingers(item, Part::Usage) || self.lingers(item, Part::Temp),
        }
    }

    /// What another applet wrote down; and the weather read again when
    /// it's old or of another place, by one applet of the three.
    fn weather_tick(&mut self, force: bool) -> app::Task<Message> {
        let seen = weather::modified();
        if seen.is_some() && seen != self.weather_seen {
            if let Some(written) = weather::load() {
                self.weather = Some(written);
            }
            self.weather_seen = seen;
        }
        let place = config::weather_place();
        let now = chrono::Utc::now().timestamp();
        let due = force
            || self.weather.as_ref().is_none_or(|w| w.place != place || w.is_stale(now));
        let resting = self.weather_failed_at.is_some_and(|at| at.elapsed() < WEATHER_RETRY);
        if !due || self.weather_fetching || (resting && !force) {
            return Task::none();
        }
        self.weather_fetching = true;
        Task::future(async move {
            let Some(lock) = weather::FetchLock::take() else {
                return cosmic::action::app(Message::WeatherFetched(None, false));
            };
            let fetched = weather::fetch(place).await;
            drop(lock);
            cosmic::action::app(Message::WeatherFetched(fetched.map(Box::new), true))
        })
    }

    /// Whether this part of an item was past its limit lately.
    fn lingers(&self, item: BarItem, part: Part) -> bool {
        self.over_at.get(&(item, part)).is_some_and(|at| at.elapsed() < OVER_LINGER)
    }

    /// Whether this part of an item shows, the item on the bar: past its
    /// limit, only what went past.
    fn shows_part(&self, item: BarItem, part: Part) -> bool {
        self.settings.bar_mode(item) != BarMode::Over || self.lingers(item, part)
    }

    /// Of an item with modes: whether its use, and its temperature, are past
    /// their limits now.
    fn past_limits(&self, item: BarItem) -> (bool, bool) {
        let usage = |percent: f32| percent >= config::USAGE_LIMIT;
        let temp = |t: Option<f32>| t.is_some_and(|t| t >= config::TEMP_LIMIT);
        let now = self.now.as_ref();
        match item {
            BarItem::Cpu => (usage(self.rates.cpu), temp(now.and_then(|n| panels::cpu_temp(&n.chips)))),
            BarItem::Gpu => now
                .and_then(|n| n.gpu.as_ref())
                .map_or((false, false), |gpu| (usage(gpu.usage), temp(gpu.temp))),
            BarItem::Memory => now.map_or((false, false), |n| {
                (usage(n.memory.used as f32 / n.memory.total.max(1) as f32 * 100.0), false)
            }),
            _ => (false, false),
        }
    }

    fn take(&mut self, new: Reading) {
        let Some(old) = self.now.take() else {
            self.now = Some(new);
            return;
        };
        let dt = new.at.duration_since(old.at).as_secs_f32().max(0.001);
        let seconds = dt.round() as usize;
        let mut rates = Rates::default();
        // The CPU: all, then each thread.
        let busy = |a: &sample::CpuTimes, b: &sample::CpuTimes| {
            let total = b.total.saturating_sub(a.total);
            if total == 0 {
                0.0
            } else {
                b.busy.saturating_sub(a.busy) as f32 / total as f32 * 100.0
            }
        };
        let mut cpus = old.cpu.iter().zip(&new.cpu).map(|(a, b)| busy(a, b));
        rates.cpu = cpus.next().unwrap_or(0.0);
        rates.cores = cpus.collect();
        // Energy: the CPU's counters, the GPU's.
        let mut joules = 0.0f64;
        if let (Some(a), Some(b)) = (&old.rapl, &new.rapl) {
            let used: u64 = a
                .iter()
                .zip(b)
                .map(|((a, range), (b, _))| if b >= a { b - a } else { range - a + b })
                .sum();
            joules += used as f64 / 1e6;
            rates.cpu_watts = Some(used as f32 / 1e6 / dt);
        }
        if let Some(gpu) = &new.gpu {
            rates.gpu_watts = gpu.watts;
            let guessed = f64::from(gpu.watts.unwrap_or(0.0)) * f64::from(dt.min(30.0));
            joules += match (self.gpu_energy, gpu.energy_mj) {
                // All since it was last read.
                (Some((a, _)), Some(b)) if b >= a => (b - a) as f64 / 1000.0,
                // Not read this time: the next one counts this too.
                (Some(_), None) => 0.0,
                // The first, or after the driver restarted.
                _ => guessed,
            };
            if let Some(b) = gpu.energy_mj {
                self.gpu_energy = Some((b, Instant::now()));
            }
        }
        // The rest, guessed; not for the time asleep.
        if rates.cpu_watts.is_some() || rates.gpu_watts.is_some() {
            joules += self.settings.rest_watts * f64::from(dt.min(30.0));
        }
        self.meter.add(joules, self.popup.is_some());
        // Drives and network, a second.
        for (name, read, written) in &new.disks {
            if let Some((_, r0, w0)) = old.disks.iter().find(|(n, ..)| n == name) {
                let r = read.saturating_sub(*r0) as f32 * 512.0 / dt;
                let w = written.saturating_sub(*w0) as f32 * 512.0 / dt;
                rates.read += r;
                rates.write += w;
                rates.disks.push((name.clone(), r, w));
            }
        }
        rates.down = new.net.0.saturating_sub(old.net.0) as f32 / dt;
        rates.up = new.net.1.saturating_sub(old.net.1) as f32 / dt;
        // Processes, by program: their share of all the CPU's time.
        if let Some(procs) = &new.procs {
            let all_ticks = new.cpu.first().zip(old.cpu.first()).map_or(0, |(b, a)| b.total.saturating_sub(a.total));
            let mut groups: HashMap<&str, Group> = HashMap::new();
            for (pid, proc) in procs {
                let before = old.procs.as_ref().and_then(|p| p.get(pid)).map_or(proc.ticks, |p| p.ticks);
                let cpu = if all_ticks == 0 {
                    0.0
                } else {
                    proc.ticks.saturating_sub(before) as f32 / all_ticks as f32 * 100.0
                };
                let group = groups.entry(&proc.name).or_insert_with(|| Group {
                    name: proc.name.clone(),
                    cpu: 0.0,
                    memory: 0,
                    pids: Vec::new(),
                    mine: true,
                });
                group.cpu += cpu;
                group.memory += proc.memory;
                group.pids.push(*pid);
                group.mine &= proc.mine;
            }
            rates.groups = groups.into_values().collect();
        }
        // The charts.
        push(&mut self.history.cpu, rates.cpu, seconds);
        if let Some(gpu) = &new.gpu {
            push(&mut self.history.gpu, gpu.usage, seconds);
        }
        let memory = &new.memory;
        push(&mut self.history.memory, memory.used as f32 / memory.total.max(1) as f32 * 100.0, seconds);
        push(&mut self.history.down, rates.down, seconds);
        push(&mut self.history.up, rates.up, seconds);
        push(&mut self.history.read, rates.read, seconds);
        push(&mut self.history.write, rates.write, seconds);
        self.rates = rates;
        if let Some(watts) = self.total_watts() {
            push(&mut self.history.power, watts, seconds);
        }
        self.now = Some(new);
        for item in BarItem::ALL.into_iter().filter(|i| i.has_modes()) {
            let (usage, temp) = self.past_limits(item);
            for (part, past) in [(Part::Usage, usage), (Part::Temp, temp)] {
                if past {
                    self.over_at.insert((item, part), Instant::now());
                }
            }
        }
    }

    fn close_popup(&mut self) -> app::Task<Message> {
        self.editing = false;
        self.asking_end = None;
        match self.popup.take() {
            Some(id) => destroy_popup(id),
            None => Task::none(),
        }
    }
}

/// The popup under the whole button, from its start: the Central's mirror.
fn line_up(
    positioner: &mut iced::platform_specific::runtime::wayland::popup::SctkPositioner,
    panel: cosmic::applet::cosmic_panel_config::PanelAnchor,
    button: iced::Size,
) {
    use cosmic::{
        applet::cosmic_panel_config::PanelAnchor,
        cctk::sctk::reexports::protocols::xdg::shell::client::xdg_positioner::{Anchor, Gravity},
    };
    positioner.anchor_rect.width = button.width.round() as i32;
    positioner.anchor_rect.height = button.height.round() as i32;
    (positioner.anchor, positioner.gravity) = match panel {
        PanelAnchor::Top => (Anchor::BottomLeft, Gravity::BottomRight),
        PanelAnchor::Bottom => (Anchor::TopLeft, Gravity::TopRight),
        PanelAnchor::Left => (Anchor::TopRight, Gravity::BottomRight),
        PanelAnchor::Right => (Anchor::TopLeft, Gravity::BottomLeft),
    };
}

fn popup_limits(max_height: f32) -> Limits {
    Limits::NONE
        .min_width(WIDTH)
        .max_width(WIDTH)
        .min_height(1.0)
        .max_height(max_height)
}

/// A number with the decimal mark the language uses.
pub(crate) fn decimal(value: f64, digits: usize) -> String {
    let text = format!("{value:.digits$}");
    if texts::texts().comma { text.replace('.', ",") } else { text }
}

/// Bytes in the unit that fits: 812 MB, 1,4 GB.
pub(crate) fn bytes(value: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = value;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    let digits = if unit == 0 || value >= 100.0 { 0 } else if value >= 10.0 { 1 } else { 1 };
    format!("{} {}", decimal(value, digits), UNITS[unit])
}

/// Bytes a second, short for the bar: 1,2M, 34K; never more than three
/// digits, for the bar's room.
pub(crate) fn rate_short(value: f32) -> String {
    let value = f64::from(value);
    if value >= 1000.0 * 1024.0 {
        format!("{}M", decimal(value / 1024.0 / 1024.0, if value >= 10.0 * 1024.0 * 1024.0 { 0 } else { 1 }))
    } else if value >= 1024.0 {
        format!("{}K", decimal(value / 1024.0, 0))
    } else if value >= 1000.0 {
        "1K".to_owned()
    } else {
        format!("{}B", decimal(value, 0))
    }
}

/// Seconds as 3 d 4 h, or 4 h 12 min.
pub(crate) fn uptime(seconds: f64) -> String {
    let minutes = (seconds / 60.0) as u64;
    let (days, hours, minutes) = (minutes / 1440, minutes / 60 % 24, minutes % 60);
    if days > 0 {
        format!("{days} d {hours} h")
    } else if hours > 0 {
        format!("{hours} h {minutes} min")
    } else {
        format!("{minutes} min")
    }
}

impl cosmic::Application for Monitor {
    type Message = Message;
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = ();
    const APP_ID: &'static str = APP_ID;

    fn init(core: app::Core, _flags: ()) -> (Self, app::Task<Message>) {
        let settings = Settings::load();
        let mut monitor = Self {
            core,
            tariff_input: settings.tariff.map(|t| decimal(t, 2)).unwrap_or_default(),
            rest_input: if settings.rest_watts > 0.0 { decimal(settings.rest_watts, 0) } else { String::new() },
            bar_models: bar_models(&settings),
            settings,
            info: sample::info(),
            ..Self::default()
        };
        // COSMIC_MONITOR_PREVIEW=1 opens the popup by itself, for screenshots;
        // =edit, on the arranging; =place, on choosing the weather's place. COSMIC_QS_PREVIEW_OUTPUT=DP-3 keeps it to
        // the panel on that screen.
        let on_output = std::env::var("COSMIC_QS_PREVIEW_OUTPUT").map_or(true, |output| {
            std::env::var("COSMIC_PANEL_OUTPUT").is_ok_and(|panel| panel == output)
        });
        let preview = std::env::var("COSMIC_MONITOR_PREVIEW").ok().filter(|_| on_output);
        monitor.editing = preview.as_deref() == Some("edit");
        // =place, on choosing where the weather is, "Curitiba" searched.
        if preview.as_deref() == Some("place") {
            monitor.place_editing = true;
            monitor.place_input = "Curitiba".into();
        }
        let task = match preview {
            Some(_) => Task::batch([
                monitor.read(),
                Task::future(async {
                    tokio::time::sleep(Duration::from_millis(2500)).await;
                    cosmic::action::app(Message::TogglePopup)
                }),
            ]),
            None => monitor.read(),
        };
        let weather = monitor.weather_tick(false);
        let search = if monitor.place_editing {
            Task::future(async { cosmic::action::app(Message::SearchPlace) })
        } else {
            Task::none()
        };
        (monitor, Task::batch([task, weather, search]))
    }

    fn core(&self) -> &app::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut app::Core {
        &mut self.core
    }

    fn style(&self) -> Option<iced::theme::Style> {
        Some(cosmic::applet::style())
    }

    fn update(&mut self, message: Message) -> app::Task<Message> {
        match message {
            Message::Tick => return self.read(),
            Message::Reading(reading) => {
                self.reading = false;
                self.take(*reading);
            }
            Message::TogglePopup => {
                if self.popup.is_some() {
                    return self.close_popup();
                }
                let Some(parent) = self.core.main_window_id() else {
                    return Task::none();
                };
                let id = window::Id::unique();
                self.popup = Some(id);
                let open = cosmic::surface::surface_task(cosmic::surface::action::app_popup(
                    |_: &Self| Default::default(),
                    move |monitor: &mut Self| {
                        let mut settings = monitor.core.applet.get_popup_settings(parent, id, None, None, None);
                        settings.positioner.size_limits = popup_limits(monitor.max_height());
                        if let Some(size) = monitor.button_size {
                            line_up(&mut settings.positioner, monitor.core.applet.anchor, size);
                        }
                        settings
                    },
                    None,
                ));
                // Fresh numbers, and the processes, at once; the weather,
                // if it's old.
                let weather = self.weather_tick(false);
                return Task::batch([open, self.read(), weather]);
            }
            Message::PanelPress(id) => {
                let just_closed = self.closed_at.is_some_and(|at| at.elapsed() < Duration::from_millis(250));
                if Some(id) == self.core.main_window_id() && !just_closed {
                    return self.update(Message::TogglePopup);
                }
            }
            Message::Closed(id) => {
                if self.popup == Some(id) {
                    self.closed_at = Some(Instant::now());
                    self.popup = None;
                    self.editing = false;
                    self.asking_end = None;
                }
            }
            Message::Output(name, height) => {
                let panel = std::env::var("COSMIC_PANEL_OUTPUT").unwrap_or_default();
                if name.as_deref() == Some(panel.as_str())
                    || (panel.is_empty() || panel == "all") && self.screen_height.is_none()
                {
                    if !self.screen_height_known {
                        self.screen_height = height;
                    }
                    if let Some(name) = name.filter(|_| !self.screen_height_known) {
                        return Task::future(async move {
                            cosmic::action::app(Message::ScreenHeight(crate::services::screen_height(name).await))
                        });
                    }
                }
            }
            Message::ScreenHeight(height) => {
                if height.is_some() {
                    self.screen_height = height;
                    self.screen_height_known = true;
                }
            }
            Message::Resized(id, size) => {
                if Some(id) == self.core.main_window_id() {
                    self.button_size = Some(size);
                }
            }
            Message::Edit(on) => {
                self.editing = on;
                self.asking_end = None;
                if !on {
                    return self.read();
                }
            }
            Message::ToggleFold(panel) => {
                self.settings.toggle_fold(panel);
                if panel == Panel::Processes {
                    return self.read();
                }
            }
            Message::ToggleHidden(panel) => self.settings.toggle_hidden(panel),
            Message::Shift(panel, down) => self.settings.shift(panel, down),
            Message::ToggleBar(item) => self.settings.toggle_bar(item),
            Message::BarMode(item, entity) => {
                if let Some((_, model)) = self.bar_models.iter_mut().find(|(i, _)| *i == item) {
                    model.activate(entity);
                    if let Some(mode) = model.data::<BarMode>(entity).copied() {
                        self.settings.set_bar_mode(item, mode);
                    }
                }
            }
            Message::ResetLayout => {
                self.settings.reset();
                self.bar_models = bar_models(&self.settings);
            }
            Message::ToggleEnergySettings => self.energy_settings = !self.energy_settings,
            Message::TariffInput(text) => {
                let value = text.trim().replace(',', ".").parse::<f64>().ok();
                self.settings.set_tariff(value);
                self.tariff_input = text;
            }
            Message::RestInput(text) => {
                let value = text.trim().replace(',', ".").parse::<f64>().unwrap_or(0.0);
                self.settings.set_rest_watts(value);
                self.rest_input = text;
            }
            Message::ResetEnergy => self.meter.reset(),
            Message::Sort(sort) => self.sort = Some(sort),
            Message::AskEnd(name) => self.asking_end = name,
            Message::End(name) => {
                self.asking_end = None;
                if let Some(group) = self.rates.groups.iter().find(|g| g.name == name) {
                    for pid in &group.pids {
                        sample::terminate(*pid);
                    }
                }
                return self.read();
            }
            Message::WeatherTick => return self.weather_tick(false),
            Message::WeatherFetched(fetched, tried) => {
                self.weather_fetching = false;
                match fetched {
                    Some(fetched) => {
                        self.weather = Some(*fetched);
                        self.weather_seen = weather::modified();
                        self.weather_failed_at = None;
                    }
                    None if tried => self.weather_failed_at = Some(Instant::now()),
                    // Another applet is at it: its file, at the next tick.
                    None => {}
                }
            }
            Message::ToggleWeatherExpanded => self.settings.toggle_weather_expanded(),
            Message::EditPlace(editing) => {
                self.place_editing = editing;
                self.place_input.clear();
                self.place_results = None;
                self.place_searching = false;
            }
            Message::PlaceInput(text) => self.place_input = text,
            Message::SearchPlace => {
                let query = self.place_input.trim().to_owned();
                if query.is_empty() || self.place_searching {
                    return Task::none();
                }
                self.place_searching = true;
                return Task::future(async move {
                    cosmic::action::app(Message::PlacesFound(weather::place::search(query).await))
                });
            }
            Message::PlacesFound(places) => {
                self.place_searching = false;
                self.place_results = Some(places);
            }
            Message::PickPlace(place) => {
                config::set_weather_place(&place);
                self.place_editing = false;
                self.place_results = None;
                self.place_input.clear();
                self.weather_failed_at = None;
                return self.weather_tick(true);
            }
            Message::None => {}
        }
        Task::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        // A second while looked at; five while not, for the bar and the
        // energy, which counts all the time.
        let every = if self.popup.is_some() { Duration::from_secs(1) } else { Duration::from_secs(5) };
        Subscription::batch([
            iced::time::every(every).map(|_| Message::Tick),
            iced::time::every(WEATHER_TICK).map(|_| Message::WeatherTick),
            event::listen_with(|event, _, _| {
                use iced::event::wayland::{Event as Wayland, OutputEvent};
                match event {
                    iced::Event::PlatformSpecific(iced::event::PlatformSpecific::Wayland(Wayland::Output(
                        OutputEvent::Created(Some(info)) | OutputEvent::InfoUpdate(info),
                        _,
                    ))) => Some(Message::Output(info.name.clone(), info.logical_size.map(|(_, h)| h))),
                    _ => None,
                }
            }),
            window::resize_events().map(|(id, size)| Message::Resized(id, size)),
            event::listen_with(|event, _, id| match event {
                iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)) => {
                    Some(Message::PanelPress(id))
                }
                _ => None,
            }),
        ])
    }

    fn view(&self) -> Element<'_, Message> {
        let applet = &self.core.applet;
        let size = applet.suggested_size(true).0;
        let (major, minor) = applet.suggested_padding(true);
        let horizontal = applet.is_horizontal();
        let now = self.now.as_ref();
        // Each item: its icon, in its color, and its numbers on one line, as
        // wide as the widest they can be (a template under each, unseen, in
        // the same font), so the bar doesn't shift or wrap as they change.
        // A number past its limit in amber; the power smaller, the network
        // too and stacked.
        let dark = theme::active().cosmic().is_dark;
        let item = |icon: widget::icon::Handle,
                    hue: chart::Hue,
                    parts: Vec<(String, &'static str, bool)>,
                    look: Look|
         -> Element<'_, Message> {
            let icon = widget::icon::icon(icon)
                .size(size)
                .class(theme::Svg::Custom(std::rc::Rc::new(move |theme| {
                    iced::widget::svg::Style { color: Some(hue.color(theme.cosmic().is_dark)) }
                })));
            let one_line = iced::widget::text::Wrapping::None;
            let text = |text: String| {
                if look == Look::Normal { applet.text(text) } else { widget::text::caption(text) }
            };
            let hot = |past: bool| {
                if past {
                    theme::Text::Color(chart::Hue::Warm.color(dark))
                } else {
                    theme::Text::Default
                }
            };
            let numbers = |number: String, widest: &str, past: bool| {
                iced::widget::stack(vec![
                    text(widest.to_owned())
                        .wrapping(one_line)
                        .class(theme::Text::Color(iced::Color::TRANSPARENT))
                        .into(),
                    text(number).wrapping(one_line).class(hot(past)).into(),
                ])
            };
            if horizontal && look == Look::Stacked {
                // Tight, so the two lines fit the bar.
                let lines: Vec<Element<'_, Message>> =
                    parts.into_iter().map(|(n, widest, past)| numbers(n, widest, past).into()).collect();
                return widget::row::with_children(vec![
                    icon.into(),
                    widget::column::with_children(lines).spacing(-2).into(),
                ])
                .spacing(4)
                .align_y(Alignment::Center)
                .into();
            }
            if horizontal {
                let mut row = widget::row::with_capacity(parts.len() + 1).push(icon);
                for (at, (number, widest, past)) in parts.into_iter().enumerate() {
                    // Between the numbers, a space of their own.
                    let space = if at > 0 { " " } else { "" };
                    row = row.push(numbers(format!("{space}{number}"), &format!("{space}{widest}"), past));
                }
                row.spacing(4).align_y(Alignment::Center).into()
            } else {
                let past = parts.iter().any(|(_, _, past)| *past);
                let numbers: Vec<String> = parts.into_iter().map(|(number, ..)| number).collect();
                widget::column::with_children(vec![
                    icon.into(),
                    widget::text::caption(numbers.join(" ")).wrapping(one_line).class(hot(past)).into(),
                ])
                .align_x(Alignment::Center)
                .into()
            }
        };
        let degrees = |t: Option<f32>| t.map(|t| format!("{t:.0}°"));
        let mut items: Vec<Element<'_, Message>> = Vec::new();
        for bar_item in BarItem::ALL {
            if !self.on_bar(bar_item) {
                continue;
            }
            let (usage_past, temp_past) = self.past_limits(bar_item);
            let (show_usage, show_temp) =
                (self.shows_part(bar_item, Part::Usage), self.shows_part(bar_item, Part::Temp));
            let usage_and_temp = |usage: f32, temp: Option<f32>| {
                let mut parts = Vec::new();
                if show_usage {
                    parts.push((format!("{usage:.0}%"), "100%", usage_past));
                }
                if let Some(temp) = degrees(temp).filter(|_| show_temp) {
                    parts.push((temp, "100°", temp_past));
                }
                parts
            };
            match bar_item {
                BarItem::Weather => {
                    if let Some(weather) = &self.weather {
                        let light = weather.light(chrono::Utc::now().timestamp());
                        let icon = weather::scene::Icon {
                            sky: weather.now.sky,
                            day: light.time != weather::scene::Time::Night,
                            size: f32::from(size),
                            moving: false,
                            on_panel: true,
                            moon: light.moon,
                            south: light.south,
                        }
                        .view();
                        let temp = format!("{:.0}°", weather.now.temp);
                        let one_line = iced::widget::text::Wrapping::None;
                        items.push(if horizontal {
                            widget::row::with_children(vec![
                                icon,
                                iced::widget::stack(vec![
                                    applet
                                        .text("-00°")
                                        .wrapping(one_line)
                                        .class(theme::Text::Color(iced::Color::TRANSPARENT))
                                        .into(),
                                    applet.text(temp).wrapping(one_line).into(),
                                ])
                                .into(),
                            ])
                            .spacing(4)
                            .align_y(Alignment::Center)
                            .into()
                        } else {
                            widget::column::with_children(vec![
                                icon,
                                widget::text::caption(temp).wrapping(one_line).into(),
                            ])
                            .align_x(Alignment::Center)
                            .into()
                        });
                    }
                }
                BarItem::Cpu => items.push(item(
                    panels::icon_cpu(),
                    chart::Hue::Cpu,
                    usage_and_temp(self.rates.cpu, now.and_then(|n| panels::cpu_temp(&n.chips))),
                    Look::Normal,
                )),
                BarItem::Gpu => {
                    if let Some(gpu) = now.and_then(|n| n.gpu.as_ref()) {
                        items.push(item(
                            panels::icon_gpu(),
                            chart::Hue::Gpu,
                            usage_and_temp(gpu.usage, gpu.temp),
                            Look::Normal,
                        ));
                    }
                }
                BarItem::Memory => {
                    if let Some(memory) = now.map(|n| &n.memory) {
                        items.push(item(
                            panels::icon_memory(),
                            chart::Hue::Memory,
                            vec![(
                                format!("{} GB", decimal(memory.used as f64 / 1024f64.powi(3), 1)),
                                "00,0 GB",
                                usage_past,
                            )],
                            Look::Normal,
                        ));
                    }
                }
                BarItem::Power => {
                    if let Some(watts) = self.total_watts() {
                        items.push(item(
                            panels::icon_bolt(),
                            chart::Hue::Energy,
                            vec![(format!("{watts:.0} W"), "0000 W", false)],
                            Look::Small,
                        ));
                    }
                }
                BarItem::Network => items.push(item(
                    crate::tiles::icon("network-transmit-receive-symbolic"),
                    chart::Hue::Download,
                    vec![
                        (format!("↓{}", rate_short(self.rates.down)), "↓999K", false),
                        (format!("↑{}", rate_short(self.rates.up)), "↑999K", false),
                    ],
                    Look::Stacked,
                )),
            }
        }
        if items.is_empty() {
            items.push(
                widget::icon::icon(crate::tiles::icon("utilities-system-monitor-symbolic"))
                    .size(size)
                    .into(),
            );
        }
        let gap = 10;
        let content: Element<'_, Message> = if horizontal {
            widget::row::with_children(items).spacing(gap).align_y(Alignment::Center).into()
        } else {
            widget::column::with_children(items).spacing(gap).align_x(Alignment::Center).into()
        };
        // As tall as the bar, padding and all, the numbers centered in it: the
        // network's two lines take more than the icon's height, but not the
        // bar's, which they would otherwise stretch.
        let content = if horizontal {
            widget::container(content).center_y(Length::Fixed(f32::from(size + 2 * minor)))
        } else {
            widget::container(content).padding([0, minor])
        };
        let button = button::custom(content)
        .padding(if horizontal { [0, major] } else { [major, 0] })
        .class(theme::Button::AppletIcon)
        .on_press(Message::None);
        applet.autosize_window(button).into()
    }

    fn view_window(&self, id: window::Id) -> Element<'_, Message> {
        if self.popup != Some(id) {
            return widget::text("").into();
        }
        let content = if self.editing { self.edit_view() } else { self.main_view() };
        cosmic::widget::autosize::autosize(
            widget::container(content).width(Length::Fixed(WIDTH)),
            cosmic::widget::Id::new("cosmic-applet-autosize"),
        )
        .limits(popup_limits(self.max_height()))
        .into()
    }

    fn on_close_requested(&self, id: window::Id) -> Option<Message> {
        Some(Message::Closed(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn past_its_limit_only_the_part_that_went_past_shows() {
        let mut monitor = Monitor::default();
        monitor.over_at.insert((BarItem::Cpu, Part::Temp), Instant::now());

        assert!(monitor.on_bar(BarItem::Cpu));
        assert!(monitor.shows_part(BarItem::Cpu, Part::Temp));
        assert!(!monitor.shows_part(BarItem::Cpu, Part::Usage));
        assert!(!monitor.on_bar(BarItem::Gpu));
    }

    #[test]
    fn a_part_hides_once_it_has_been_back_under_long_enough() {
        let mut monitor = Monitor::default();
        let long_ago = Instant::now() - OVER_LINGER - Duration::from_secs(1);
        monitor.over_at.insert((BarItem::Cpu, Part::Usage), long_ago);

        assert!(!monitor.on_bar(BarItem::Cpu));
        assert!(!monitor.shows_part(BarItem::Cpu, Part::Usage));
    }

    #[test]
    fn always_shows_both_parts() {
        let mut monitor = Monitor::default();
        monitor.settings.bar_modes.insert(BarItem::Cpu, BarMode::Always);

        assert!(monitor.on_bar(BarItem::Cpu));
        assert!(monitor.shows_part(BarItem::Cpu, Part::Usage));
        assert!(monitor.shows_part(BarItem::Cpu, Part::Temp));
    }
}
