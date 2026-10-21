// SPDX-License-Identifier: GPL-3.0-only

//! The energy used, added up: by day, and since counting began. Kept in a
//! file, and counted by one applet only: the panel runs one on each screen,
//! and each counting would count it again. The one holding the lock counts;
//! the others read what it wrote.

use std::{
    collections::BTreeMap,
    fs::File,
    os::fd::AsRawFd,
    path::PathBuf,
    time::Instant,
};

use chrono::{Datelike, Local, NaiveDate};

/// Days kept, for the chart and the month.
const DAYS_KEPT: usize = 62;
/// How often the counting applet writes it down, in seconds.
const SAVE_EVERY: u64 = 60;

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Ledger {
    /// Watt-hours since `since`.
    pub total_wh: f64,
    pub since: Option<NaiveDate>,
    /// Watt-hours each day, by date.
    pub days: BTreeMap<NaiveDate, f64>,
}

impl Ledger {
    pub fn today_wh(&self) -> f64 {
        self.days.get(&Local::now().date_naive()).copied().unwrap_or(0.0)
    }

    pub fn month_wh(&self) -> f64 {
        let today = Local::now().date_naive();
        self.days
            .iter()
            .filter(|(day, _)| day.year() == today.year() && day.month() == today.month())
            .map(|(_, wh)| wh)
            .sum()
    }

    /// The last `n` days, oldest first, today last; a day without a count
    /// as zero.
    pub fn last_days(&self, n: i64) -> Vec<(NaiveDate, f64)> {
        let today = Local::now().date_naive();
        (0..n)
            .rev()
            .map(|back| {
                let day = today - chrono::Days::new(back as u64);
                (day, self.days.get(&day).copied().unwrap_or(0.0))
            })
            .collect()
    }

    fn add(&mut self, wh: f64) {
        let today = Local::now().date_naive();
        self.since.get_or_insert(today);
        self.total_wh += wh;
        *self.days.entry(today).or_default() += wh;
        while self.days.len() > DAYS_KEPT {
            self.days.pop_first();
        }
    }
}

fn dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("cosmic-central")
}

fn load() -> Ledger {
    std::fs::read(dir().join("energy.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save(ledger: &Ledger) {
    let dir = dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let Ok(json) = serde_json::to_vec(ledger) else { return };
    // Whole or not at all: written apart, then put in its place.
    let partial = dir.join("energy.json.part");
    if std::fs::write(&partial, json).is_ok() {
        _ = std::fs::rename(partial, dir.join("energy.json"));
    }
}

pub struct Meter {
    pub ledger: Ledger,
    /// Held while this applet is the one counting.
    lock: Option<File>,
    saved: Instant,
}

impl Default for Meter {
    fn default() -> Self {
        Self { ledger: load(), lock: None, saved: Instant::now() }
    }
}

impl Meter {
    /// Whether this one counts: it takes the lock if it's free, as when
    /// the applet that held it is gone.
    fn counting(&mut self) -> bool {
        if self.lock.is_some() {
            return true;
        }
        let dir = dir();
        _ = std::fs::create_dir_all(&dir);
        let Ok(file) = File::options().create(true).truncate(false).write(true).open(dir.join("energy.lock")) else {
            return false;
        };
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return false;
        }
        // From what the last one wrote down.
        self.ledger = load();
        self.lock = Some(file);
        true
    }

    /// Joules used since the last reading, counted if this one counts;
    /// else what the counting one wrote, read again.
    pub fn add(&mut self, joules: f64) {
        if !self.counting() {
            self.ledger = load();
            return;
        }
        // Asked by another screen's applet.
        if std::fs::remove_file(dir().join("energy.reset")).is_ok() {
            self.ledger = Ledger::default();
            save(&self.ledger);
        }
        if joules > 0.0 {
            self.ledger.add(joules / 3600.0);
        }
        if self.saved.elapsed().as_secs() >= SAVE_EVERY {
            self.saved = Instant::now();
            save(&self.ledger);
        }
    }

    /// Back to nothing, from today.
    pub fn reset(&mut self) {
        if self.counting() {
            self.ledger = Ledger::default();
            save(&self.ledger);
        } else {
            // The counting one does it, on its next reading.
            _ = std::fs::write(dir().join("energy.reset"), b"");
            self.ledger = Ledger::default();
        }
    }
}

impl Drop for Meter {
    fn drop(&mut self) {
        if self.lock.is_some() {
            save(&self.ledger);
        }
    }
}
