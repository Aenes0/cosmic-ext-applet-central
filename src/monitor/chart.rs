// SPDX-License-Identifier: GPL-3.0-only

//! The monitor's drawings: a soft area for what changes over the last
//! minute, as Windows 11's Task Manager draws it, thin bars for each CPU
//! thread, a bar in parts, and the days' energy.

use std::collections::VecDeque;

use cosmic::{
    Element,
    iced::{
        Color, Length, Point, Rectangle, Size, mouse,
        widget::canvas::{self, Frame, Geometry, Path, Stroke, gradient},
    },
};

/// How many readings a chart holds: a minute, a second each.
pub const HISTORY: usize = 60;

/// What a color stands for: each its own, the same everywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hue {
    Cpu,
    Gpu,
    Memory,
    Disk,
    Download,
    Upload,
    Energy,
    /// Something fine, warm, or hot: temperatures, how full.
    Fine,
    Warm,
    Hot,
}

impl Hue {
    /// Soft on a dark theme, deeper on a light one.
    pub fn color(self, dark: bool) -> Color {
        let (d, l): (u32, u32) = match self {
            Hue::Cpu => (0x5B9DFF, 0x2563D9),
            Hue::Gpu => (0x3DD68C, 0x15924F),
            Hue::Memory => (0xB48CFF, 0x7444D6),
            Hue::Disk => (0x3CC8D8, 0x0E8897),
            Hue::Download => (0xFF9F43, 0xD2690F),
            Hue::Upload => (0xFF6B9A, 0xD02F66),
            Hue::Energy => (0xFFC53D, 0xC07F00),
            Hue::Fine => (0x3DD68C, 0x15924F),
            Hue::Warm => (0xFFB547, 0xC77700),
            Hue::Hot => (0xFF5D5D, 0xD12C2C),
        };
        let rgb = if dark { d } else { l };
        Color::from_rgb8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
    }

    /// By how near the limit: green, then amber past 70 %, red past 90 %.
    pub fn level(fraction: f32) -> Hue {
        if fraction >= 0.9 {
            Hue::Hot
        } else if fraction >= 0.7 {
            Hue::Warm
        } else {
            Hue::Fine
        }
    }
}

pub fn dark(theme: &cosmic::Theme) -> bool {
    theme.cosmic().is_dark
}

/// A faint track, under what fills it.
fn track(theme: &cosmic::Theme) -> Color {
    let mut color: Color = theme.cosmic().background(theme.transparent).on.into();
    color.a = 0.08;
    color
}

fn with_alpha(mut color: Color, a: f32) -> Color {
    color.a = a;
    color
}

/// One line on an area chart.
pub struct Series<'a> {
    pub values: &'a VecDeque<f32>,
    pub hue: Hue,
}

/// Lines over the last minute, each filled softly down to the bottom.
pub struct Area<'a> {
    series: Vec<Series<'a>>,
    /// The top; when none, the highest value seen, with some room.
    max: Option<f32>,
}

impl<'a> Area<'a> {
    pub fn new(series: Vec<Series<'a>>, max: Option<f32>) -> Self {
        Self { series, max }
    }

    pub fn view<Message: 'a>(self, height: f32) -> Element<'a, Message> {
        canvas::Canvas::new(self)
            .width(Length::Fill)
            .height(Length::Fixed(height))
            .into()
    }

    /// The top of the chart: the max, or the highest value rounded up.
    fn top(&self) -> f32 {
        if let Some(max) = self.max {
            return max;
        }
        let highest = self
            .series
            .iter()
            .flat_map(|s| s.values.iter().copied())
            .fold(0.0f32, f32::max);
        nice_ceiling(highest * 1.15)
    }
}

/// 1, 2 or 5 times a power of ten, at least `value`.
pub fn nice_ceiling(value: f32) -> f32 {
    if value <= 0.0 {
        return 1.0;
    }
    let power = 10f32.powf(value.log10().floor());
    [1.0, 2.0, 2.5, 5.0, 10.0]
        .into_iter()
        .map(|step| step * power)
        .find(|v| *v >= value)
        .unwrap_or(10.0 * power)
}

impl<Message> canvas::Program<Message, cosmic::Theme, cosmic::Renderer> for Area<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &cosmic::Renderer,
        theme: &cosmic::Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry<cosmic::Renderer>> {
        let mut frame = Frame::new(renderer, bounds.size());
        let (w, h) = (bounds.width, bounds.height);
        let dark = dark(theme);
        // The background: rounded, a shade off the panel; lines at the
        // quarters.
        let radius = 10.0;
        frame.fill(
            &Path::rounded_rectangle(Point::ORIGIN, bounds.size(), radius.into()),
            track(theme),
        );
        let mut grid: Color = theme.cosmic().background(theme.transparent).on.into();
        grid.a = 0.06;
        for quarter in 1..4 {
            let y = (h * quarter as f32 / 4.0).round() + 0.5;
            frame.stroke(
                &Path::line(Point::new(0.0, y), Point::new(w, y)),
                Stroke::default().with_color(grid).with_width(1.0),
            );
        }
        let top = self.top().max(f32::EPSILON);
        let step = w / (HISTORY - 1) as f32;
        // Room so the line isn't cut at the top or the bottom.
        let (pad_top, pad_bottom) = (3.0, 1.5);
        for series in &self.series {
            let n = series.values.len();
            if n < 2 {
                continue;
            }
            let color = series.hue.color(dark);
            let points: Vec<Point> = series
                .values
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let x = w - (n - 1 - i) as f32 * step;
                    let fraction = (v / top).clamp(0.0, 1.0);
                    Point::new(x, pad_top + (h - pad_top - pad_bottom) * (1.0 - fraction))
                })
                .collect();
            let line = smooth(&points, None);
            let area = smooth(&points, Some(h));
            let fill = gradient::Linear::new(Point::new(0.0, 0.0), Point::new(0.0, h))
                .add_stop(0.0, with_alpha(color, if dark { 0.42 } else { 0.32 }))
                .add_stop(1.0, with_alpha(color, 0.02));
            frame.fill(&area, fill);
            frame.stroke(
                &line,
                Stroke::default()
                    .with_color(color)
                    .with_width(1.75)
                    .with_line_join(canvas::LineJoin::Round)
                    .with_line_cap(canvas::LineCap::Round),
            );
            // The newest, a dot.
            if let Some(last) = points.last() {
                frame.fill(&Path::circle(*last, 2.5), color);
            }
        }
        vec![frame.into_geometry()]
    }
}

/// A line through the points, eased at each, never past them; closed down
/// to `floor` when given, to be filled.
fn smooth(points: &[Point], floor: Option<f32>) -> Path {
    Path::new(|p| {
        let first = points[0];
        match floor {
            Some(y) => {
                p.move_to(Point::new(first.x, y));
                p.line_to(first);
            }
            None => p.move_to(first),
        }
        for pair in points.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let mid = Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
            p.quadratic_curve_to(a, mid);
        }
        let last = *points.last().unwrap();
        p.line_to(last);
        if let Some(y) = floor {
            p.line_to(Point::new(last.x, y));
            p.close();
        }
    })
}

/// Thin bars side by side, each filling up: one per CPU thread.
pub struct Bars<'a> {
    pub values: &'a [f32],
    pub hue: Hue,
}

impl<'a> Bars<'a> {
    pub fn view<Message: 'a>(self, height: f32) -> Element<'a, Message> {
        canvas::Canvas::new(self)
            .width(Length::Fill)
            .height(Length::Fixed(height))
            .into()
    }
}

impl<Message> canvas::Program<Message, cosmic::Theme, cosmic::Renderer> for Bars<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &cosmic::Renderer,
        theme: &cosmic::Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry<cosmic::Renderer>> {
        let mut frame = Frame::new(renderer, bounds.size());
        let n = self.values.len().max(1) as f32;
        let gap = if n > 24.0 { 3.0 } else { 4.0 };
        let width = ((bounds.width - gap * (n - 1.0)) / n).max(1.0);
        let color = self.hue.color(dark(theme));
        let track = track(theme);
        let radius = (width / 2.0).min(3.0);
        for (i, value) in self.values.iter().enumerate() {
            let x = i as f32 * (width + gap);
            frame.fill(
                &Path::rounded_rectangle(Point::new(x, 0.0), Size::new(width, bounds.height), radius.into()),
                track,
            );
            let filled = (bounds.height * (value / 100.0).clamp(0.0, 1.0)).max(width.min(3.0));
            // Brighter the busier.
            let alpha = 0.45 + 0.55 * (value / 100.0).clamp(0.0, 1.0);
            frame.fill(
                &Path::rounded_rectangle(
                    Point::new(x, bounds.height - filled),
                    Size::new(width, filled),
                    radius.into(),
                ),
                with_alpha(color, alpha),
            );
        }
        vec![frame.into_geometry()]
    }
}

/// A bar in parts, each its share and color, over a faint track.
pub struct Split {
    pub parts: Vec<(f32, Color)>,
}

impl Split {
    pub fn view<'a, Message: 'a>(self, height: f32) -> Element<'a, Message> {
        canvas::Canvas::new(self)
            .width(Length::Fill)
            .height(Length::Fixed(height))
            .into()
    }

    /// One part, colored by how full.
    pub fn level<'a, Message: 'a>(fraction: f32, hue: Option<Hue>, height: f32) -> Element<'a, Message> {
        Meter { fraction, hue }.view(height)
    }
}

impl<Message> canvas::Program<Message, cosmic::Theme, cosmic::Renderer> for Split {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &cosmic::Renderer,
        theme: &cosmic::Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry<cosmic::Renderer>> {
        let mut frame = Frame::new(renderer, bounds.size());
        let radius = bounds.height / 2.0;
        frame.fill(
            &Path::rounded_rectangle(Point::ORIGIN, bounds.size(), radius.into()),
            track(theme),
        );
        // The parts one after another, clipped to the track's round ends.
        let mut x = 0.0;
        let total = self.parts.len();
        for (i, (fraction, color)) in self.parts.iter().enumerate() {
            let width = bounds.width * fraction.clamp(0.0, 1.0);
            if width < 0.5 {
                continue;
            }
            let (left, right) = (if x == 0.0 { radius } else { 0.0 }, if i + 1 == total || x + width >= bounds.width - 0.5 { radius } else { 0.0 });
            let radii = cosmic::iced::border::Radius {
                top_left: left,
                bottom_left: left,
                top_right: right,
                bottom_right: right,
            };
            frame.fill(
                &Path::rounded_rectangle(Point::new(x, 0.0), Size::new(width.max(bounds.height), bounds.height), radii),
                *color,
            );
            x += width;
        }
        vec![frame.into_geometry()]
    }
}

/// A rounded bar filled up to a fraction.
struct Meter {
    fraction: f32,
    /// Its color; by level when none.
    hue: Option<Hue>,
}

impl Meter {
    fn view<'a, Message: 'a>(self, height: f32) -> Element<'a, Message> {
        canvas::Canvas::new(self)
            .width(Length::Fill)
            .height(Length::Fixed(height))
            .into()
    }
}

impl<Message> canvas::Program<Message, cosmic::Theme, cosmic::Renderer> for Meter {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &cosmic::Renderer,
        theme: &cosmic::Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry<cosmic::Renderer>> {
        let mut frame = Frame::new(renderer, bounds.size());
        let radius = bounds.height / 2.0;
        frame.fill(
            &Path::rounded_rectangle(Point::ORIGIN, bounds.size(), radius.into()),
            track(theme),
        );
        let fraction = self.fraction.clamp(0.0, 1.0);
        if fraction > 0.0 {
            let hue = self.hue.unwrap_or_else(|| Hue::level(fraction));
            let width = (bounds.width * fraction).max(bounds.height);
            frame.fill(
                &Path::rounded_rectangle(Point::ORIGIN, Size::new(width, bounds.height), radius.into()),
                hue.color(dark(theme)),
            );
        }
        vec![frame.into_geometry()]
    }
}

/// The energy of each of the last days, as bars, today brightest, with
/// the weekday's letter under each.
pub struct Days {
    pub days: Vec<(String, f64)>,
}

impl Days {
    pub fn view<'a, Message: 'a>(self, height: f32) -> Element<'a, Message> {
        canvas::Canvas::new(self)
            .width(Length::Fill)
            .height(Length::Fixed(height))
            .into()
    }
}

impl<Message> canvas::Program<Message, cosmic::Theme, cosmic::Renderer> for Days {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &cosmic::Renderer,
        theme: &cosmic::Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry<cosmic::Renderer>> {
        let mut frame = Frame::new(renderer, bounds.size());
        let n = self.days.len().max(1) as f32;
        let gap = 6.0;
        let label = 14.0;
        let width = (bounds.width - gap * (n - 1.0)) / n;
        let high = self.days.iter().map(|(_, wh)| *wh).fold(0.0, f64::max).max(1.0) as f32;
        let color = Hue::Energy.color(dark(theme));
        let mut text_color: Color = theme.cosmic().background(theme.transparent).on.into();
        text_color.a = 0.6;
        let bars = bounds.height - label;
        for (i, (letter, wh)) in self.days.iter().enumerate() {
            let x = i as f32 * (width + gap);
            let today = i + 1 == self.days.len();
            frame.fill(
                &Path::rounded_rectangle(Point::new(x, 0.0), Size::new(width, bars), 4.0.into()),
                track(theme),
            );
            let filled = (bars * (*wh as f32 / high)).max(if *wh > 0.0 { 3.0 } else { 0.0 });
            if filled > 0.0 {
                frame.fill(
                    &Path::rounded_rectangle(
                        Point::new(x, bars - filled),
                        Size::new(width, filled),
                        4.0.into(),
                    ),
                    with_alpha(color, if today { 1.0 } else { 0.55 }),
                );
            }
            frame.fill_text(canvas::Text {
                content: letter.clone(),
                position: Point::new(x + width / 2.0, bounds.height - label / 2.0),
                color: if today { color } else { text_color },
                size: 10.0.into(),
                align_x: cosmic::iced::alignment::Horizontal::Center.into(),
                align_y: cosmic::iced::alignment::Vertical::Center,
                ..Default::default()
            });
        }
        vec![frame.into_geometry()]
    }
}
