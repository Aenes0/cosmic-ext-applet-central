// SPDX-License-Identifier: GPL-3.0-only

//! What is asked of the panels' host, and where a panel sits (pure).

use std::time::Duration;

use cosmic::applet::cosmic_panel_config::PanelAnchor;
use cosmic::iced::Alignment;

/// Between the bar and a panel, as COSMIC leaves under an applet's popup.
pub const GAP: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    /// The Central, at the bar's end.
    Central,
    /// The system monitor, at the bar's start.
    Monitor,
}

impl Which {
    pub fn parse(name: &str) -> Option<Which> {
        match name {
            "central" => Some(Which::Central),
            "monitor" => Some(Which::Monitor),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Which::Central => "central",
            Which::Monitor => "monitor",
        }
    }
}

/// A panel to show or hide, on a screen by its name (none: the active one);
/// with a page, opened on it instead of toggled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub which: Which,
    pub output: String,
    pub page: Option<String>,
}

impl Request {
    /// From what follows `--panels`: which, then the screen, then the page.
    pub fn from_args<S: AsRef<str>>(args: &[S]) -> Option<Request> {
        let mut args = args.iter().map(AsRef::as_ref).filter(|a| !a.starts_with("--"));
        let which = Which::parse(args.next()?)?;
        let output = args.next().unwrap_or_default().to_owned();
        let page = args.next().filter(|p| !p.is_empty()).map(str::to_owned);
        Some(Request { which, output, page })
    }

    /// The same as arguments, for starting the host with it.
    pub fn to_args(&self) -> Vec<String> {
        let mut args = vec![self.which.name().to_owned(), self.output.clone()];
        args.extend(self.page.clone());
        args
    }
}

/// What a request does, given what shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Open,
    Close,
    /// Another panel, or the same on another screen, shows: it goes, this comes.
    Switch,
    /// The same, open on a page.
    Page,
    /// The click on the bar that closed it, as the bar took the keyboard.
    Ignore,
}

/// `shown` is the panel open and its screen; `just_closed` the one that
/// closed a moment ago.
pub fn step(shown: Option<(Which, &str)>, just_closed: Option<(Which, &str)>, request: &Request) -> Step {
    let asked = (request.which, request.output.as_str());
    let paged = request.page.is_some();
    match shown {
        Some(shown) if shown == asked => {
            if paged {
                Step::Page
            } else {
                Step::Close
            }
        }
        Some(_) => Step::Switch,
        None if !paged && just_closed == Some(asked) => Step::Ignore,
        None => Step::Open,
    }
}

/// Where a panel sits on the screen less the bar: by the bar, the Central
/// at its end, the monitor at its start; across and down the screen.
pub fn corner(bar: PanelAnchor, which: Which) -> (Alignment, Alignment) {
    let along = match which {
        Which::Central => Alignment::End,
        Which::Monitor => Alignment::Start,
    };
    match bar {
        PanelAnchor::Bottom => (along, Alignment::End),
        PanelAnchor::Top => (along, Alignment::Start),
        PanelAnchor::Left => (Alignment::Start, along),
        PanelAnchor::Right => (Alignment::End, along),
    }
}

/// The screen's edges a panel's own surface holds to: the bar's, and the
/// end of it the panel sits at (top, bottom, left, right).
pub fn edges(bar: PanelAnchor, which: Which) -> [bool; 4] {
    let (x, y) = corner(bar, which);
    [y == Alignment::Start, y == Alignment::End, x == Alignment::Start, x == Alignment::End]
}

/// After a panel last opened, how long nothing that holds the GPU (finding
/// the monitors' DDC) is done.
pub const SETTLED: Duration = Duration::from_secs(10);

/// Whether the GPU may be held a moment now: no panel showing, none opened
/// lately.
pub fn quiet(showing: bool, since_opened: Option<Duration>) -> bool {
    !showing && since_opened.is_none_or(|since| since >= SETTLED)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(which: Which, output: &str, page: Option<&str>) -> Request {
        Request { which, output: output.into(), page: page.map(Into::into) }
    }

    #[test]
    fn names_go_both_ways() {
        for which in [Which::Central, Which::Monitor] {
            assert_eq!(Which::parse(which.name()), Some(which));
        }
        assert_eq!(Which::parse("Central"), None);
        assert_eq!(Which::parse(""), None);
    }

    #[test]
    fn requests_come_from_the_arguments_and_go_back_to_them() {
        assert_eq!(Request::from_args::<&str>(&[]), None);
        assert_eq!(Request::from_args(&["nothing"]), None);
        assert_eq!(Request::from_args(&["central"]), Some(request(Which::Central, "", None)));
        assert_eq!(Request::from_args(&["monitor", "DP-3"]), Some(request(Which::Monitor, "DP-3", None)));
        let paged = request(Which::Central, "HDMI-A-1", Some("sound"));
        assert_eq!(Request::from_args(&["central", "HDMI-A-1", "sound"]), Some(paged.clone()));
        assert_eq!(Request::from_args(&paged.to_args()), Some(paged));
        // Flags among them are not the panel's.
        assert_eq!(Request::from_args(&["--panels", "central", "DP-3"]), Some(request(Which::Central, "DP-3", None)));
    }

    #[test]
    fn the_same_panel_on_the_same_screen_toggles() {
        let asked = request(Which::Central, "DP-3", None);
        assert_eq!(step(None, None, &asked), Step::Open);
        assert_eq!(step(Some((Which::Central, "DP-3")), None, &asked), Step::Close);
    }

    #[test]
    fn another_panel_or_screen_switches() {
        let asked = request(Which::Central, "DP-3", None);
        assert_eq!(step(Some((Which::Monitor, "DP-3")), None, &asked), Step::Switch);
        assert_eq!(step(Some((Which::Central, "HDMI-A-1")), None, &asked), Step::Switch);
    }

    #[test]
    fn the_click_that_closed_it_does_not_open_it_again() {
        let asked = request(Which::Central, "DP-3", None);
        assert_eq!(step(None, Some((Which::Central, "DP-3")), &asked), Step::Ignore);
        // Another one opens, as does a page asked for.
        assert_eq!(step(None, Some((Which::Monitor, "DP-3")), &asked), Step::Open);
        let paged = request(Which::Central, "DP-3", Some("sound"));
        assert_eq!(step(None, Some((Which::Central, "DP-3")), &paged), Step::Open);
    }

    #[test]
    fn a_page_asked_of_the_panel_showing_turns_it() {
        let paged = request(Which::Central, "DP-3", Some("sound"));
        assert_eq!(step(Some((Which::Central, "DP-3")), None, &paged), Step::Page);
    }

    #[test]
    fn a_panel_surface_holds_to_the_bar_and_its_end() {
        // top, bottom, left, right
        assert_eq!(edges(PanelAnchor::Bottom, Which::Central), [false, true, false, true]);
        assert_eq!(edges(PanelAnchor::Bottom, Which::Monitor), [false, true, true, false]);
        assert_eq!(edges(PanelAnchor::Top, Which::Monitor), [true, false, true, false]);
        assert_eq!(edges(PanelAnchor::Right, Which::Central), [false, true, false, true]);
    }

    #[test]
    fn the_gpu_is_held_only_with_no_panel_showing_or_just_opened() {
        assert!(quiet(false, None));
        assert!(quiet(false, Some(SETTLED)));
        assert!(!quiet(true, None));
        assert!(!quiet(false, Some(Duration::from_secs(3))));
    }

    #[test]
    fn panels_sit_by_the_bar_at_its_ends() {
        use Alignment::{End, Start};
        assert_eq!(corner(PanelAnchor::Bottom, Which::Central), (End, End));
        assert_eq!(corner(PanelAnchor::Bottom, Which::Monitor), (Start, End));
        assert_eq!(corner(PanelAnchor::Top, Which::Central), (End, Start));
        assert_eq!(corner(PanelAnchor::Top, Which::Monitor), (Start, Start));
        assert_eq!(corner(PanelAnchor::Left, Which::Central), (Start, End));
        assert_eq!(corner(PanelAnchor::Right, Which::Monitor), (End, Start));
    }
}
