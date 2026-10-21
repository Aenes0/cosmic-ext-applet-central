// SPDX-License-Identifier: GPL-3.0-only

//! The weather drawn: the sky behind the numbers, in the colors of the hour
//! and the weather, with rain, snow, clouds or stars moving in it; and the
//! icons, the same way. They move for a few seconds after the popup opens,
//! slowing down to rest: each frame redraws the whole popup in software,
//! about half a core at 30 a second, so moving all the while would not do.

use std::time::{Duration, Instant};

use cosmic::{
    Element,
    iced::{
        Color, Length, Point, Rectangle, Size, Vector, mouse,
        widget::canvas::{self, Action, Event, Frame, Geometry, Path, Stroke, gradient},
        window,
    },
};

use super::Sky;

/// About 30 frames a second.
const FRAME: Duration = Duration::from_millis(33);
/// Seconds moving at full speed after opening, then slowing to rest.
const FULL_SPEED: f32 = 4.0;
const SLOWING: f32 = 2.0;

fn rgb(hex: u32) -> Color {
    Color::from_rgb8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

fn alpha(mut color: Color, a: f32) -> Color {
    color.a = a;
    color
}

/// The same scatter every frame: 0 to 1 for each number.
fn noise(i: u32) -> f32 {
    let x = (i as f32 * 12.9898 + 78.233).sin() * 43_758.547;
    x - x.floor()
}

/// The time of day as the sky shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Time {
    Dawn,
    Day,
    Dusk,
    Night,
}

/// The light of the moment: the time of day, how far the sun is across the
/// sky, and the moon.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light {
    pub time: Time,
    /// 0 at sunrise, 1 at sunset.
    pub sun: f32,
    /// The moon's phase: 0 new, 0.5 full.
    pub moon: f32,
    /// South of the equator, as in Belo Horizonte, the moon is lit on the
    /// other side.
    pub south: bool,
}

/// Seconds between two new moons, on average.
const SYNODIC_MONTH: f64 = 29.530_588_853 * 86_400.0;
/// A new moon: 6 January 2000, 18:14 UTC.
const NEW_MOON: i64 = 947_182_440;

/// The moon's phase at a moment: 0 new, 0.25 first quarter, 0.5 full.
pub fn moon_phase(at: i64) -> f32 {
    ((at - NEW_MOON) as f64 / SYNODIC_MONTH).rem_euclid(1.0) as f32
}

/// The sky's top and bottom colors, as Samsung's weather does.
pub fn backdrop(sky: Sky, time: Time) -> (Color, Color) {
    #[derive(PartialEq)]
    enum Kind {
        Clear,
        Partly,
        Cloudy,
        Wet,
        Thunder,
        Snow,
    }
    let kind = match sky {
        Sky::Clear => Kind::Clear,
        Sky::PartlyCloudy => Kind::Partly,
        Sky::Cloudy | Sky::Fog => Kind::Cloudy,
        Sky::Drizzle | Sky::Showers | Sky::Rain => Kind::Wet,
        Sky::Thunder => Kind::Thunder,
        Sky::Snow => Kind::Snow,
    };
    // Deep enough for white text at 4.5:1 or more, top and bottom (WCAG's
    // for small text), with room for the clouds drifting over; at dawn and
    // dusk, warm low down.
    let (top, bottom) = match (kind, time) {
        (Kind::Thunder, _) => (0x1E2538, 0x3A4560),
        (Kind::Clear, Time::Day) => (0x1D63C4, 0x3571B5),
        (Kind::Partly, Time::Day) => (0x2B64AE, 0x4571A9),
        (Kind::Cloudy, Time::Day) => (0x4A5B70, 0x5F7186),
        (Kind::Wet, Time::Day) => (0x34455A, 0x4C5D72),
        (Kind::Snow, Time::Day) => (0x4F6C92, 0x5A7190),
        (Kind::Clear | Kind::Partly, Time::Dawn) => (0x2D3F7C, 0xA35C44),
        (Kind::Clear | Kind::Partly, Time::Dusk) => (0x24306A, 0xB2523F),
        (Kind::Cloudy, Time::Dawn) => (0x3E4763, 0x876765),
        (Kind::Cloudy, Time::Dusk) => (0x383C58, 0x956162),
        (Kind::Wet, Time::Dawn) => (0x2F3747, 0x6E5C62),
        (Kind::Wet, Time::Dusk) => (0x2A3042, 0x6A5058),
        (Kind::Snow, Time::Dawn) => (0x3E4A70, 0x846676),
        (Kind::Snow, Time::Dusk) => (0x3A4268, 0x8C6277),
        (Kind::Clear | Kind::Partly, Time::Night) => (0x0D1838, 0x2A3E6F),
        (Kind::Cloudy, Time::Night) => (0x1D2738, 0x3D4A5E),
        (Kind::Wet, Time::Night) => (0x18212F, 0x35414F),
        (Kind::Snow, Time::Night) => (0x2A3650, 0x4C5E7E),
    };
    (rgb(top), rgb(bottom))
}

/// When it was first drawn and when last, for the moving ones.
#[derive(Default)]
pub struct Clock {
    start: Option<Instant>,
    now: Option<Instant>,
}

/// The scene's time for the real time since it opened: the same at first,
/// then slower and slower, then still.
pub fn eased(real: f32) -> f32 {
    if real <= FULL_SPEED {
        return real;
    }
    let slowing = (real - FULL_SPEED).min(SLOWING);
    FULL_SPEED + slowing - slowing * slowing / (2.0 * SLOWING)
}

impl Clock {
    fn seconds(&self) -> f32 {
        match (self.start, self.now) {
            (Some(start), Some(now)) => eased(now.duration_since(start).as_secs_f32()),
            _ => 0.0,
        }
    }

    /// At each frame, the time, and the next one asked for while moving.
    fn tick<Message>(&mut self, event: &Event, moving: bool) -> Option<Action<Message>> {
        let Event::Window(window::Event::RedrawRequested(now)) = event else { return None };
        if !moving {
            return None;
        }
        let start = *self.start.get_or_insert(*now);
        self.now = Some(*now);
        let resting = now.duration_since(start).as_secs_f32() >= FULL_SPEED + SLOWING;
        (!resting).then(|| Action::request_redraw_at(*now + FRAME))
    }
}

/// The sky behind the weather: as large as what's on it.
pub struct Scene {
    pub sky: Sky,
    pub light: Light,
    pub radius: f32,
}

impl Scene {
    pub fn view<'a, Message: 'a>(self) -> Element<'a, Message> {
        canvas::Canvas::new(self).width(Length::Fill).height(Length::Fill).into()
    }
}

impl<Message> canvas::Program<Message, cosmic::Theme, cosmic::Renderer> for Scene {
    type State = Clock;

    fn update(&self, clock: &mut Clock, event: &Event, _: Rectangle, _: mouse::Cursor) -> Option<Action<Message>> {
        clock.tick(event, true)
    }

    fn draw(
        &self,
        clock: &Clock,
        renderer: &cosmic::Renderer,
        _theme: &cosmic::Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry<cosmic::Renderer>> {
        let mut frame = Frame::new(renderer, bounds.size());
        let (w, h) = (bounds.width, bounds.height);
        let t = clock.seconds();
        let light = self.light;
        let (top, bottom) = backdrop(self.sky, light.time);
        let shape = Path::rounded_rectangle(Point::ORIGIN, bounds.size(), self.radius.into());
        frame.fill(&shape, gradient::Linear::new(Point::ORIGIN, Point::new(w * 0.35, h)).add_stop(0.0, top).add_stop(1.0, bottom));
        let open_sky = matches!(self.sky, Sky::Clear | Sky::PartlyCloudy);
        // Inside, off the rounded corners.
        let inside = Rectangle::new(Point::new(0.0, self.radius * 0.5), Size::new(w, h - self.radius));
        frame.with_clip(inside, |frame| {
            let ih = inside.height;
            match light.time {
                Time::Night => {
                    // Behind clouds, a few, faint.
                    let (count, brightness) = match self.sky {
                        Sky::Clear => (46, 0.9),
                        Sky::PartlyCloudy => (32, 0.8),
                        Sky::Cloudy | Sky::Fog | Sky::Drizzle | Sky::Showers => (14, 0.45),
                        _ => (0, 0.0),
                    };
                    stars(frame, w, ih, t, count, brightness);
                    if open_sky {
                        // Off the buttons at the top right.
                        moon_in_sky(frame, Point::new(w * 0.64, ih * 0.17), light, t);
                        shooting_star(frame, w, ih, t);
                    }
                }
                Time::Day => {
                    let at = sun_at(w, ih, light.sun);
                    if open_sky {
                        sun_in_sky(frame, at, t);
                        if self.sky == Sky::PartlyCloudy {
                            sun_rays(frame, at, w, ih, t);
                        }
                    } else if matches!(self.sky, Sky::Cloudy | Sky::Fog) {
                        // Behind the clouds, where it is.
                        frame.fill(&Path::circle(at, 60.0), alpha(rgb(0xFFF3C4), 0.05));
                    }
                }
                Time::Dawn | Time::Dusk => {
                    let dawn = light.time == Time::Dawn;
                    horizon_glow(frame, w, ih, dawn);
                    if open_sky {
                        low_sun(frame, w, ih, dawn, t);
                    }
                }
            }
            if !matches!(self.sky, Sky::Clear) {
                drifting_clouds(frame, w, ih, t, self.sky);
            }
            match self.sky {
                Sky::Drizzle => rain(frame, w, ih, t, 16, 0.6),
                Sky::Showers => rain(frame, w, ih, t, 28, 0.85),
                Sky::Rain => rain(frame, w, ih, t, 42, 1.0),
                Sky::Thunder => {
                    rain(frame, w, ih, t, 46, 1.1);
                    let flash = flash(t, 7.0);
                    if flash > 0.0 {
                        frame.fill_rectangle(Point::ORIGIN, Size::new(w, ih), alpha(Color::WHITE, 0.22 * flash));
                    }
                }
                Sky::Snow => snow(frame, w, ih, t),
                Sky::Fog => fog(frame, w, ih, t),
                _ => {}
            }
        });
        vec![frame.into_geometry()]
    }
}

/// Where the sun is: rising on the left, high at noon, setting on the right,
/// along the top, where little is written.
fn sun_at(w: f32, h: f32, across: f32) -> Point {
    let height = (std::f32::consts::PI * across.clamp(0.0, 1.0)).sin();
    Point::new(w * (0.1 + 0.8 * across), h * (0.2 - 0.14 * height))
}

fn sun_in_sky(frame: &mut Frame, at: Point, t: f32) {
    let breath = 1.0 + 0.04 * (t * 0.9).sin();
    for (radius, a) in [(120.0, 0.035), (80.0, 0.05), (48.0, 0.07)] {
        frame.fill(&Path::circle(at, radius * breath), alpha(rgb(0xFFF3C4), a));
    }
    frame.fill(&Path::circle(at, 15.0), alpha(rgb(0xFFE7A0), 0.32));
}

/// Light through the gaps: soft beams fanning down from the sun.
fn sun_rays(frame: &mut Frame, at: Point, w: f32, h: f32, t: f32) {
    let length = w.max(h) * 1.1;
    for i in 0..5 {
        let angle = 0.45 + i as f32 * 0.42 + (t * 0.15 + i as f32).sin() * 0.04;
        let spread = 0.07 + noise(i + 300) * 0.05;
        let ray = Path::new(|p| {
            p.move_to(at);
            p.line_to(Point::new(at.x + (angle - spread).cos() * length, at.y + (angle - spread).sin() * length));
            p.line_to(Point::new(at.x + (angle + spread).cos() * length, at.y + (angle + spread).sin() * length));
            p.close();
        });
        frame.fill(&ray, alpha(rgb(0xFFF6D8), 0.035 + 0.015 * (t * 0.7 + i as f32).sin().abs()));
    }
}

/// The warm band low down at dawn and dusk, toward where the sun is.
fn horizon_glow(frame: &mut Frame, w: f32, h: f32, dawn: bool) {
    let x = if dawn { w * 0.18 } else { w * 0.82 };
    for (radius, a) in [(w * 0.95, 0.05), (w * 0.65, 0.06), (w * 0.38, 0.07)] {
        frame.fill(&Path::circle(Point::new(x, h + radius * 0.35), radius), alpha(rgb(0xFFB37A), a));
    }
}

/// The sun on the horizon, rising or setting slowly while it moves.
fn low_sun(frame: &mut Frame, w: f32, h: f32, dawn: bool, t: f32) {
    let drift = if dawn { -t * 1.2 } else { t * 1.2 };
    // Half under the horizon: only its top shows.
    let at = Point::new(if dawn { w * 0.16 } else { w * 0.84 }, h + 16.0 + drift);
    for (radius, a) in [(70.0, 0.05), (42.0, 0.07)] {
        frame.fill(&Path::circle(at, radius), alpha(rgb(0xFFC48A), a));
    }
    frame.fill(&Path::circle(at, 20.0), alpha(rgb(0xFFB06A), 0.30));
}

fn moon_in_sky(frame: &mut Frame, at: Point, light: Light, t: f32) {
    let breath = 1.0 + 0.05 * (t * 0.8).sin();
    for (radius, a) in [(70.0, 0.03), (42.0, 0.05), (26.0, 0.07)] {
        frame.fill(&Path::circle(at, radius * breath), alpha(rgb(0xE8EEFF), a));
    }
    moon_disc(frame, at, 13.0, light.moon, light.south, alpha(rgb(MOON), 0.9), alpha(rgb(MOON), 0.12));
}

/// Now and then, while it moves after opening, a streak across.
fn shooting_star(frame: &mut Frame, w: f32, h: f32, t: f32) {
    let (start, length) = (1.2, 0.8);
    if !(start..start + length).contains(&t) {
        return;
    }
    let p = (t - start) / length;
    let from = Point::new(w * 0.12, h * 0.06);
    let to = Point::new(w * 0.55, h * 0.3);
    let head = Point::new(from.x + (to.x - from.x) * p, from.y + (to.y - from.y) * p);
    let tail = Point::new(head.x - (to.x - from.x) * 0.18, head.y - (to.y - from.y) * 0.18);
    frame.stroke(
        &Path::line(tail, head),
        Stroke::default().with_color(alpha(Color::WHITE, 0.8 * (1.0 - p))).with_width(1.6).with_line_cap(canvas::LineCap::Round),
    );
}

/// The moon in its phase: the dark of it faint, the lit part bright, the
/// line between an ellipse.
pub fn moon_disc(frame: &mut Frame, center: Point, radius: f32, phase: f32, south: bool, lit: Color, dark: Color) {
    frame.fill(&Path::circle(center, radius), dark);
    // Waxing, lit on the right up north; waning, on the left.
    let waxing = phase < 0.5;
    let squeeze = (std::f32::consts::TAU * phase).cos();
    let mirrored = waxing == south;
    let steps = 24;
    let lit_part = Path::new(|p| {
        for i in 0..=steps {
            let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / steps as f32;
            let x = radius * a.cos();
            let point = Point::new(center.x + if mirrored { -x } else { x }, center.y + radius * a.sin());
            if i == 0 { p.move_to(point) } else { p.line_to(point) }
        }
        for i in (0..=steps).rev() {
            let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / steps as f32;
            let x = squeeze * radius * a.cos();
            p.line_to(Point::new(center.x + if mirrored { -x } else { x }, center.y + radius * a.sin()));
        }
        p.close();
    });
    frame.fill(&lit_part, lit);
}

/// A lightning's brightness: two quick flashes every `period` seconds.
fn flash(t: f32, period: f32) -> f32 {
    let phase = t % period;
    if phase < 0.10 {
        1.0 - phase / 0.10
    } else if (0.22..0.30).contains(&phase) {
        0.7
    } else {
        0.0
    }
}

fn stars(frame: &mut Frame, w: f32, h: f32, t: f32, count: u32, brightness: f32) {
    for i in 0..count {
        let at = Point::new(noise(i) * w, noise(i + 400) * h * 0.85);
        let twinkle = 0.35 + 0.65 * ((t * (0.6 + noise(i + 800) * 1.4) + noise(i + 1200) * 6.3).sin() * 0.5 + 0.5);
        let radius = 0.6 + noise(i + 1600) * 0.9;
        frame.fill(&Path::circle(at, radius), alpha(Color::WHITE, brightness * twinkle));
    }
}

fn drifting_clouds(frame: &mut Frame, w: f32, h: f32, t: f32, sky: Sky) {
    let (count, a) = match sky {
        Sky::PartlyCloudy => (2, 0.10),
        Sky::Fog => (3, 0.07),
        _ => (3, 0.08),
    };
    for i in 0..count {
        let speed = 0.012 + noise(i + 50) * 0.01;
        let x = ((noise(i + 70) + t * speed) % 1.4 - 0.2) * w;
        let y = h * (0.08 + 0.22 * i as f32);
        let size = 150.0 + noise(i + 90) * 90.0;
        cloud_shape(frame, Point::new(x, y), size, alpha(Color::WHITE, a));
    }
}

fn rain(frame: &mut Frame, w: f32, h: f32, t: f32, drops: u32, heaviness: f32) {
    for i in 0..drops {
        let speed = (260.0 + noise(i + 10) * 180.0) * heaviness;
        let length = (9.0 + noise(i + 20) * 9.0) * heaviness;
        let fall = h + length * 2.0;
        let y = (t * speed + noise(i + 30) * fall) % fall - length;
        // Slanting a little with the wind.
        let x = (noise(i) * 1.15 * w - y * 0.18).rem_euclid(w);
        let a = 0.22 + noise(i + 40) * 0.25;
        frame.stroke(
            &Path::line(Point::new(x, y), Point::new(x - length * 0.18, y + length)),
            Stroke::default().with_color(alpha(rgb(0xDCEBFF), a)).with_width(1.2).with_line_cap(canvas::LineCap::Round),
        );
    }
}

fn snow(frame: &mut Frame, w: f32, h: f32, t: f32) {
    for i in 0..44 {
        let speed = 18.0 + noise(i + 10) * 26.0;
        let fall = h + 8.0;
        let y = (t * speed + noise(i + 30) * fall) % fall - 4.0;
        let x = noise(i) * w + (t * 0.8 + noise(i + 60) * 6.3).sin() * 7.0;
        let radius = 1.0 + noise(i + 80) * 1.8;
        frame.fill(&Path::circle(Point::new(x, y), radius), alpha(Color::WHITE, 0.75));
    }
}

fn fog(frame: &mut Frame, w: f32, h: f32, t: f32) {
    for i in 0..4 {
        let y = h * (0.25 + 0.18 * i as f32);
        let shift = ((t * (6.0 + i as f32 * 3.0)) % (w * 0.6)) - w * 0.3;
        let band = Path::rounded_rectangle(Point::new(shift - w * 0.1, y), Size::new(w * 1.1, 9.0), 4.5.into());
        frame.fill(&band, alpha(Color::WHITE, 0.09));
    }
}

/// A cloud as wide as `size`, its base's middle at `at`: one shape, so a
/// see-through color isn't darker where its puffs meet.
fn cloud_shape(frame: &mut Frame, at: Point, size: f32, color: Color) {
    let s = size / 100.0;
    let path = Path::new(|p| {
        p.rounded_rectangle(Point::new(at.x - 42.0 * s, at.y - 14.0 * s), Size::new(84.0 * s, 28.0 * s), (14.0 * s).into());
        p.circle(Point::new(at.x - 18.0 * s, at.y - 12.0 * s), 20.0 * s);
        p.circle(Point::new(at.x + 12.0 * s, at.y - 20.0 * s), 26.0 * s);
        p.circle(Point::new(at.x + 32.0 * s, at.y - 6.0 * s), 14.0 * s);
    });
    frame.fill(&path, color);
}

/// A weather icon, drawn on 100 × 100 and scaled.
pub struct Icon {
    pub sky: Sky,
    pub day: bool,
    pub size: f32,
    /// Moving, for the big one; still, for the small ones and the bar.
    pub moving: bool,
    /// On the bar, over the panel of either theme: the clouds outlined.
    pub on_panel: bool,
    /// The moon's phase, 0 new to 0.5 full, and whether seen from the south.
    pub moon: f32,
    pub south: bool,
}

impl Icon {
    pub fn view<'a, Message: 'a>(self) -> Element<'a, Message> {
        let size = self.size;
        canvas::Canvas::new(self).width(Length::Fixed(size)).height(Length::Fixed(size)).into()
    }
}

impl<Message> canvas::Program<Message, cosmic::Theme, cosmic::Renderer> for Icon {
    type State = Clock;

    fn update(&self, clock: &mut Clock, event: &Event, _: Rectangle, _: mouse::Cursor) -> Option<Action<Message>> {
        clock.tick(event, self.moving)
    }

    fn draw(
        &self,
        clock: &Clock,
        renderer: &cosmic::Renderer,
        _theme: &cosmic::Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry<cosmic::Renderer>> {
        let mut frame = Frame::new(renderer, bounds.size());
        let t = if self.moving { clock.seconds() } else { 0.0 };
        frame.with_save(|frame| {
            frame.scale(bounds.width.min(bounds.height) / 100.0);
            self.paint(frame, t);
        });
        vec![frame.into_geometry()]
    }
}

const SUN: u32 = 0xFFC93C;
const MOON: u32 = 0xF4E7B5;
const DROP: u32 = 0x7CC4FF;
const BOLT: u32 = 0xFFD23F;

impl Icon {
    fn cloud(&self, frame: &mut Frame, at: Point, size: f32, back: bool, dark: bool) {
        let color = match (self.on_panel, back, dark) {
            (_, _, true) => rgb(0x8E99AB),
            (true, true, _) => rgb(0xAEB9C7),
            (true, false, _) => rgb(0xDDE4EC),
            (false, true, _) => alpha(rgb(0xE3EAF2), 0.85),
            (false, false, _) => Color::WHITE,
        };
        if self.on_panel {
            // A rim, to stand out on a light panel.
            cloud_shape(frame, Point::new(at.x, at.y + 1.6), size + 4.0, rgb(0x7A8798));
        }
        cloud_shape(frame, at, size, color);
    }

    fn sun(&self, frame: &mut Frame, center: Point, radius: f32, t: f32) {
        let turn = t * 0.35;
        for i in 0..8 {
            let angle = turn + i as f32 * std::f32::consts::FRAC_PI_4;
            let (sin, cos) = angle.sin_cos();
            let from = Point::new(center.x + cos * radius * 1.35, center.y + sin * radius * 1.35);
            let to = Point::new(center.x + cos * radius * 1.75, center.y + sin * radius * 1.75);
            frame.stroke(
                &Path::line(from, to),
                Stroke::default().with_color(rgb(SUN)).with_width(radius * 0.22).with_line_cap(canvas::LineCap::Round),
            );
        }
        frame.fill(&Path::circle(center, radius), rgb(SUN));
    }

    fn moon(&self, frame: &mut Frame, center: Point, radius: f32, t: f32) {
        let glow = 0.16 + 0.06 * (t * 1.2).sin();
        frame.fill(&Path::circle(center, radius * 1.35), alpha(rgb(MOON), glow));
        // Its dark part shows a little, so a new moon is still a moon.
        let dark = if self.on_panel { alpha(rgb(0x8A97A8), 0.45) } else { alpha(rgb(MOON), 0.25) };
        moon_disc(frame, center, radius, self.moon, self.south, rgb(MOON), dark);
    }

    fn sun_or_moon(&self, frame: &mut Frame, center: Point, radius: f32, t: f32) {
        if self.day {
            self.sun(frame, center, radius, t);
        } else {
            self.moon(frame, center, radius, t);
        }
    }

    fn drops(&self, frame: &mut Frame, count: u32, length: f32, t: f32) {
        for i in 0..count {
            let x = 30.0 + i as f32 * (44.0 / count.max(2) as f32 - 1.0) * 1.1;
            let phase = (t * 1.6 + i as f32 * 0.37) % 1.0;
            let y = 66.0 + phase * 20.0;
            let a = if self.moving { 1.0 - phase * 0.8 } else { 1.0 };
            frame.stroke(
                &Path::line(Point::new(x, y), Point::new(x - 3.0, y + length)),
                Stroke::default().with_color(alpha(rgb(DROP), a)).with_width(5.0).with_line_cap(canvas::LineCap::Round),
            );
        }
    }

    fn paint(&self, frame: &mut Frame, t: f32) {
        let bob = if self.moving { (t * 0.9).sin() * 2.0 } else { 0.0 };
        let front = Point::new(54.0, 64.0 + bob);
        match self.sky {
            Sky::Clear => self.sun_or_moon(frame, Point::new(50.0, 50.0), 22.0, t),
            Sky::PartlyCloudy => {
                self.sun_or_moon(frame, Point::new(38.0, 36.0), 17.0, t);
                self.cloud(frame, Point::new(58.0, 72.0 + bob), 66.0, false, false);
            }
            Sky::Cloudy => {
                self.cloud(frame, Point::new(38.0, 48.0 - bob * 0.5), 58.0, true, false);
                self.cloud(frame, Point::new(58.0, 70.0 + bob), 70.0, false, false);
            }
            Sky::Fog => {
                self.cloud(frame, Point::new(50.0, 50.0 + bob), 70.0, false, false);
                for (i, y) in [66.0, 77.0, 88.0].into_iter().enumerate() {
                    let shift = if self.moving { ((t + i as f32) * 1.3).sin() * 4.0 } else { 0.0 };
                    frame.stroke(
                        &Path::line(Point::new(20.0 + shift, y), Point::new(80.0 + shift, y)),
                        Stroke::default()
                            .with_color(alpha(if self.on_panel { rgb(0x8A97A8) } else { Color::WHITE }, 0.85))
                            .with_width(5.0)
                            .with_line_cap(canvas::LineCap::Round),
                    );
                }
            }
            Sky::Drizzle => {
                self.drops(frame, 3, 7.0, t);
                self.cloud(frame, front, 72.0, false, false);
            }
            Sky::Showers => {
                self.sun_or_moon(frame, Point::new(34.0, 30.0), 15.0, t);
                self.drops(frame, 3, 11.0, t);
                self.cloud(frame, front, 70.0, false, false);
            }
            Sky::Rain => {
                self.drops(frame, 4, 13.0, t);
                self.cloud(frame, front, 76.0, false, false);
            }
            Sky::Thunder => {
                self.drops(frame, 3, 11.0, t);
                let lit = if self.moving { 0.45 + 0.55 * (1.0 - flash(t, 3.2)).min(1.0) } else { 1.0 };
                let bolt = Path::new(|p| {
                    p.move_to(Point::new(56.0, 58.0));
                    p.line_to(Point::new(44.0, 80.0));
                    p.line_to(Point::new(53.0, 80.0));
                    p.line_to(Point::new(47.0, 98.0));
                    p.line_to(Point::new(66.0, 72.0));
                    p.line_to(Point::new(56.0, 72.0));
                    p.line_to(Point::new(62.0, 58.0));
                    p.close();
                });
                self.cloud(frame, Point::new(54.0, 60.0 + bob), 76.0, false, true);
                frame.fill(&bolt, alpha(rgb(BOLT), lit));
            }
            Sky::Snow => {
                for i in 0..4 {
                    let phase = if self.moving { (t * 0.6 + i as f32 * 0.31) % 1.0 } else { 0.4 };
                    let at = Point::new(30.0 + i as f32 * 13.0, 72.0 + phase * 18.0);
                    frame.with_save(|frame| {
                        frame.translate(Vector::new(at.x, at.y));
                        frame.rotate(t * 0.8 + i as f32);
                        for arm in 0..3 {
                            let angle = arm as f32 * std::f32::consts::FRAC_PI_3;
                            let (sin, cos) = angle.sin_cos();
                            frame.stroke(
                                &Path::line(Point::new(-cos * 5.0, -sin * 5.0), Point::new(cos * 5.0, sin * 5.0)),
                                Stroke::default().with_color(rgb(0xE8F3FF)).with_width(2.2).with_line_cap(canvas::LineCap::Round),
                            );
                        }
                    });
                }
                self.cloud(frame, Point::new(54.0, 60.0 + bob), 74.0, false, false);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_moves_then_slows_then_rests() {
        assert_eq!(eased(2.0), 2.0);
        let (a, b, c) = (eased(4.5) - eased(4.0), eased(5.5) - eased(5.0), eased(9.0) - eased(7.0));
        assert!(a > b && b > 0.0, "slowing: {a} then {b}");
        assert_eq!(c, 0.0);
        assert_eq!(eased(6.0), FULL_SPEED + SLOWING / 2.0);
    }

    #[test]
    fn the_moon_phase_by_the_date() {
        let day = 86_400;
        // 2000-01-06 18:14 UTC, new; a week on, about first quarter; 15 days,
        // about full.
        assert!(moon_phase(NEW_MOON) < 0.001);
        assert!((moon_phase(NEW_MOON + 7 * day + day / 3) - 0.25).abs() < 0.02);
        assert!((moon_phase(NEW_MOON + 14 * day + 18 * 3600) - 0.5).abs() < 0.02);
        // The full moon of 17 October 2024, 11:26 UTC.
        assert!((moon_phase(1_729_164_360) - 0.5).abs() < 0.03, "{}", moon_phase(1_729_164_360));
    }

    #[test]
    fn the_scatter_stays_between_zero_and_one() {
        assert!((0..5000).map(noise).all(|n| (0.0..1.0).contains(&n)));
    }

    #[test]
    fn lightning_flashes_briefly_then_stays_dark() {
        assert!(flash(0.0, 7.0) > 0.9);
        assert!(flash(0.25, 7.0) > 0.0);
        assert_eq!(flash(3.0, 7.0), 0.0);
        assert!(flash(7.02, 7.0) > 0.5);
    }

    /// WCAG's contrast of white on a color.
    fn contrast_with_white(color: Color) -> f32 {
        let linear = |c: f32| if c <= 0.039_28 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        let luminance = 0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b);
        1.05 / (luminance + 0.05)
    }

    #[test]
    fn white_text_reads_on_every_sky() {
        let skies = [
            Sky::Clear,
            Sky::PartlyCloudy,
            Sky::Cloudy,
            Sky::Fog,
            Sky::Drizzle,
            Sky::Showers,
            Sky::Rain,
            Sky::Thunder,
            Sky::Snow,
        ];
        for sky in skies {
            for time in [Time::Dawn, Time::Day, Time::Dusk, Time::Night] {
                let (top, bottom) = backdrop(sky, time);
                for color in [top, bottom] {
                    let contrast = contrast_with_white(color);
                    assert!(contrast >= 4.5, "{sky:?}, {time:?}: {contrast:.1}:1");
                }
            }
        }
    }

    #[test]
    fn night_skies_are_darker_than_day_ones() {
        let luma = |c: Color| 0.3 * c.r + 0.6 * c.g + 0.1 * c.b;
        for sky in [Sky::Clear, Sky::Cloudy, Sky::Rain, Sky::Snow] {
            assert!(luma(backdrop(sky, Time::Night).0) < luma(backdrop(sky, Time::Day).0), "{sky:?}");
        }
    }
}
