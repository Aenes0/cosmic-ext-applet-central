// SPDX-License-Identifier: GPL-3.0-only

//! How long a panel takes from being asked to open to its first view and
//! its first frame: logged with COSMIC_PANELS_TIMING=1, for comparing one
//! way of opening with another. Nothing is kept or logged without it.

use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

static ENABLED: LazyLock<bool> =
    LazyLock::new(|| std::env::var_os("COSMIC_PANELS_TIMING").is_some_and(|v| v != "0"));

/// The panel being opened, since when, and what of it was already logged.
static OPENING: Mutex<Option<Opening>> = Mutex::new(None);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    View,
    Frame,
}

#[derive(Clone, Copy, Debug)]
struct Opening {
    panel: &'static str,
    at: Instant,
    viewed: bool,
    drawn: bool,
}

pub fn enabled() -> bool {
    *ENABLED
}

/// A panel was asked to open.
pub fn opening(panel: &'static str) {
    if !enabled() {
        return;
    }
    if let Ok(mut opening) = OPENING.lock() {
        *opening = Some(Opening { panel, at: Instant::now(), viewed: false, drawn: false });
    }
}

/// A panel's view was built.
pub fn viewed(panel: &'static str) {
    log(panel, Mark::View);
}

/// A panel's surface was shown.
pub fn drawn(panel: &'static str) {
    log(panel, Mark::Frame);
}

fn log(panel: &'static str, mark: Mark) {
    if !enabled() {
        return;
    }
    let Ok(mut opening) = OPENING.lock() else {
        return;
    };
    if let Some(took) = first(&mut opening, panel, mark, Instant::now()) {
        let what = match mark {
            Mark::View => "first view",
            Mark::Frame => "shown",
        };
        tracing::info!("panel {panel}: {what} {:.1} ms after asked", took.as_secs_f64() * 1000.0);
    }
}

/// How long since `panel` was asked to open, the first time each mark
/// comes: the next ones, and those of another panel, give nothing.
fn first(opening: &mut Option<Opening>, panel: &str, mark: Mark, now: Instant) -> Option<Duration> {
    let open = opening.as_mut().filter(|o| o.panel == panel)?;
    let seen = match mark {
        Mark::View => &mut open.viewed,
        Mark::Frame => &mut open.drawn,
    };
    if *seen {
        return None;
    }
    *seen = true;
    Some(now.saturating_duration_since(open.at))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_mark_counts_once_and_only_for_the_panel_asked() {
        let at = Instant::now();
        let mut opening = Some(Opening { panel: "central", at, viewed: false, drawn: false });
        assert_eq!(first(&mut opening, "monitor", Mark::View, at), None);
        let later = at + Duration::from_millis(40);
        assert_eq!(first(&mut opening, "central", Mark::View, later), Some(Duration::from_millis(40)));
        assert_eq!(first(&mut opening, "central", Mark::View, later), None);
        let drawn = at + Duration::from_millis(90);
        assert_eq!(first(&mut opening, "central", Mark::Frame, drawn), Some(Duration::from_millis(90)));
        assert_eq!(first(&mut opening, "central", Mark::Frame, drawn), None);
    }

    #[test]
    fn nothing_asked_nothing_logged() {
        let mut opening = None;
        assert_eq!(first(&mut opening, "central", Mark::Frame, Instant::now()), None);
    }
}
