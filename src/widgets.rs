// SPDX-License-Identifier: GPL-3.0-only

//! The quick settings' own widgets, as phones draw them: a tile's surface,
//! which eases between off and on and gives a little as it's pressed, and a
//! thick slider with its icon inside.

use std::time::{Duration, Instant};

use cosmic::iced::{
    Border, Color, Event, Length, Padding, Point, Rectangle, Size, Vector,
    advanced::{
        Clipboard, Layout, Renderer as _, Shell, Widget, layout, mouse, overlay, renderer,
        widget::{Operation, Tree, tree},
    },
    window,
};

/// Off to on and back.
const LIGHT: Duration = Duration::from_millis(200);
/// Pressed in and let go.
const PRESS: Duration = Duration::from_millis(120);
const RELEASE: Duration = Duration::from_millis(260);
/// A tile's corners: rounded, as phones' tiles, unless the theme is squarer.
pub fn tile_radius(theme: &cosmic::Theme) -> f32 {
    theme.cosmic().radius_xl()[0].min(20.0)
}
/// How much rounder or squarer a pressed tile gets, of its radius.
const PRESSED_RADIUS: f32 = 0.55;

/// Eases from one value to another over a while.
#[derive(Clone, Copy)]
struct Ease {
    from: f32,
    to: f32,
    start: Instant,
    length: Duration,
}

impl Ease {
    fn at(value: f32) -> Self {
        Self {
            from: value,
            to: value,
            start: Instant::now(),
            length: Duration::ZERO,
        }
    }

    fn value(&self) -> f32 {
        if self.length.is_zero() {
            return self.to;
        }
        let t = (self.start.elapsed().as_secs_f32() / self.length.as_secs_f32()).min(1.0);
        // ease out cubic: quick, then settling
        let t = 1.0 - (1.0 - t).powi(3);
        self.from + (self.to - self.from) * t
    }

    fn moving(&self) -> bool {
        self.start.elapsed() < self.length
    }

    fn go(&mut self, to: f32, length: Duration) {
        if to != self.to {
            *self = Self {
                from: self.value(),
                to,
                start: Instant::now(),
                length,
            };
        }
    }
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

/// How bright a color looks, as WCAG measures it.
fn luminance(color: Color) -> f32 {
    let linear = |v: f32| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

fn contrast(a: Color, b: Color) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// What reads on `background`: the theme's `on`, unless it hardly shows
/// there, as its white on a white or light accent; then black or white,
/// whichever shows more.
pub fn readable(on: impl Into<Color>, background: impl Into<Color>) -> Color {
    let (on, background) = (on.into(), background.into());
    if contrast(on, background) >= 3.0 {
        return on;
    }
    let dark = Color::from_rgb(0.08, 0.08, 0.08);
    let light = Color::from_rgb(0.97, 0.97, 0.97);
    if contrast(dark, background) >= contrast(light, background) {
        dark
    } else {
        light
    }
}

/// For a container inside a tile or a colored button: it keeps the colors
/// they give, where COSMIC's plain one sets the panel's own, white on a
/// light accent or on the media card's white play button.
pub fn inherit_colors() -> cosmic::theme::Container<'static> {
    cosmic::theme::Container::custom(|_| cosmic::widget::container::Style::default())
}

/// The colors of a tile, off and on: background, hovered background, and
/// what's on it.
fn tile_colors(theme: &cosmic::Theme, on: bool) -> (Color, Color, Color) {
    let cosmic = theme.cosmic();
    let component = if on {
        &cosmic.accent_button
    } else {
        &cosmic.background(theme.transparent).component
    };
    (
        component.base.into(),
        component.hover.into(),
        readable(component.on, component.base),
    )
}

/// Which of a surface's corners are round: all, or one side's, for the two
/// halves of a split pill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Round {
    All,
    Start,
    End,
}

/// A tile's surface: lit when on, easing between off and on; a click (on
/// what inside doesn't take it) toggles it, a right click opens more of it.
pub struct Surface<'a, Message> {
    content: cosmic::Element<'a, Message>,
    on: bool,
    on_press: Option<Message>,
    on_right_press: Option<Message>,
    width: Length,
    height: Length,
    padding: Padding,
    /// Rounder than the theme's, or a pill: half the height.
    pill: bool,
    round: Round,
}

impl<'a, Message> Surface<'a, Message> {
    pub fn new(content: impl Into<cosmic::Element<'a, Message>>, on: bool) -> Self {
        Self {
            content: content.into(),
            on,
            on_press: None,
            on_right_press: None,
            width: Length::Fill,
            height: Length::Fill,
            padding: Padding::ZERO,
            pill: false,
            round: Round::All,
        }
    }

    /// Only these corners round; the others nearly square, against the
    /// other half.
    pub fn round(mut self, round: Round) -> Self {
        self.round = round;
        self
    }

    pub fn on_press(mut self, message: Option<Message>) -> Self {
        self.on_press = message;
        self
    }

    pub fn on_right_press(mut self, message: Option<Message>) -> Self {
        self.on_right_press = message;
        self
    }

    pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
        self.padding = padding.into();
        self
    }

    pub fn width(mut self, width: Length) -> Self {
        self.width = width;
        self
    }

    pub fn height(mut self, height: Length) -> Self {
        self.height = height;
        self
    }

    pub fn pill(mut self) -> Self {
        self.pill = true;
        self
    }
}

struct SurfaceState {
    lit: Ease,
    pressed: Ease,
    is_pressed: bool,
}

impl<Message: Clone> Widget<Message, cosmic::Theme, cosmic::Renderer> for Surface<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<SurfaceState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(SurfaceState {
            lit: Ease::at(if self.on { 1.0 } else { 0.0 }),
            pressed: Ease::at(0.0),
            is_pressed: false,
        })
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&mut self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<SurfaceState>();
        state.lit.go(if self.on { 1.0 } else { 0.0 }, LIGHT);
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }

    fn size(&self) -> Size<Length> {
        Size::new(self.width, self.height)
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &cosmic::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::padded(limits, self.width, self.height, self.padding, |limits| {
            self.content
                .as_widget_mut()
                .layout(&mut tree.children[0], renderer, limits)
        })
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &cosmic::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            child(layout),
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
        let state = tree.state.downcast_mut::<SurfaceState>();
        if let Event::Window(window::Event::RedrawRequested(_)) = event {
            if state.lit.moving() || state.pressed.moving() {
                shell.request_redraw();
            }
            return;
        }
        if shell.is_event_captured() {
            return;
        }
        let over = cursor.is_over(layout.bounds());
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) if over => {
                if self.on_press.is_some() {
                    state.is_pressed = true;
                    state.pressed.go(1.0, PRESS);
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if state.is_pressed => {
                state.is_pressed = false;
                state.pressed.go(0.0, RELEASE);
                shell.request_redraw();
                if over && let Some(message) = &self.on_press {
                    shell.publish(message.clone());
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) if over => {
                if let Some(message) = &self.on_right_press {
                    shell.publish(message.clone());
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => shell.request_redraw(),
            _ => {}
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut cosmic::Renderer,
        theme: &cosmic::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        if !bounds.intersects(viewport) {
            return;
        }
        let state = tree.state.downcast_ref::<SurfaceState>();
        let lit = state.lit.value();
        let hovered = cursor.is_over(bounds) && self.on_press.is_some();
        let (off_base, off_hover, off_on) = tile_colors(theme, false);
        let (on_base, on_hover, on_on) = tile_colors(theme, true);
        let background = if hovered {
            mix(off_hover, on_hover, lit)
        } else {
            mix(off_base, on_base, lit)
        };
        let foreground = mix(off_on, on_on, lit);
        let full = if self.pill {
            bounds.height / 2.0
        } else {
            tile_radius(theme).min(bounds.height / 2.0)
        };
        let radius = full * (1.0 - (1.0 - PRESSED_RADIUS) * state.pressed.value());
        // Where two halves meet: a little round, as One UI's split pills.
        let inner = 4.0f32.min(radius);
        let radius = match self.round {
            Round::All => [radius; 4],
            Round::Start => [radius, inner, inner, radius],
            Round::End => [inner, radius, radius, inner],
        };
        renderer.fill_quad(
            renderer::Quad {
                bounds,
                border: Border {
                    radius: radius.into(),
                    ..Border::default()
                },
                ..renderer::Quad::default()
            },
            background,
        );
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            &renderer::Style {
                icon_color: foreground,
                text_color: foreground,
                scale_factor: style.scale_factor,
            },
            child(layout),
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &cosmic::Renderer,
    ) -> mouse::Interaction {
        let inner = self.content.as_widget().mouse_interaction(
            &tree.children[0],
            child(layout),
            cursor,
            viewport,
            renderer,
        );
        if inner == mouse::Interaction::None
            && self.on_press.is_some()
            && cursor.is_over(layout.bounds())
        {
            mouse::Interaction::Pointer
        } else {
            inner
        }
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &cosmic::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], child(layout), renderer, operation);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &cosmic::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, cosmic::Theme, cosmic::Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            child(layout),
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, Message: Clone + 'a> From<Surface<'a, Message>> for cosmic::Element<'a, Message> {
    fn from(surface: Surface<'a, Message>) -> Self {
        Self::new(surface)
    }
}

/// A thick slider, its icon inside at the start: the part up to the level is
/// lit, a pill that starts as a circle around the icon. Drag anywhere or
/// scroll on it; a click on the icon does what it says (mute, say).
pub struct Level<'a, Message> {
    icon: cosmic::Element<'a, Message>,
    /// From 0 to 1.
    value: f32,
    on_change: Box<dyn Fn(f32) -> Message + 'a>,
    on_release: Option<Message>,
    on_icon_press: Option<Message>,
    /// Shown greyed, like a muted volume.
    dimmed: bool,
    height: f32,
    /// Standing up, filling from the bottom, the icon at the foot.
    vertical: bool,
    /// Low: the icon, then a thin rail with a round knob.
    compact: bool,
}

impl<'a, Message> Level<'a, Message> {
    pub fn new(
        icon: impl Into<cosmic::Element<'a, Message>>,
        value: f32,
        on_change: impl Fn(f32) -> Message + 'a,
    ) -> Self {
        Self {
            icon: icon.into(),
            value: value.clamp(0.0, 1.0),
            on_change: Box::new(on_change),
            on_release: None,
            on_icon_press: None,
            dimmed: false,
            height: 44.0,
            vertical: false,
            compact: false,
        }
    }

    /// Low, beside a folded player: the icon, then a thin rail with a knob.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self.height = 32.0;
        self
    }

    /// Standing up, as tall as it's given: filling from the bottom.
    pub fn vertical(mut self) -> Self {
        self.vertical = true;
        self
    }

    /// Its thickness: the icon's square's side.
    fn thickness(&self, bounds: Rectangle) -> f32 {
        if self.vertical { bounds.width } else { bounds.height }
    }

    /// Its length along which it fills.
    fn length(&self, bounds: Rectangle) -> f32 {
        if self.vertical { bounds.height } else { bounds.width }
    }

    pub fn on_release(mut self, message: Message) -> Self {
        self.on_release = Some(message);
        self
    }

    pub fn on_icon_press(mut self, message: Option<Message>) -> Self {
        self.on_icon_press = message;
        self
    }

    pub fn dimmed(mut self, dimmed: bool) -> Self {
        self.dimmed = dimmed;
        self
    }

    /// The square at the start, where the icon is: at the foot when standing.
    fn icon_bounds(&self, bounds: Rectangle) -> Rectangle {
        let side = self.thickness(bounds);
        if self.vertical {
            Rectangle::new(
                Point::new(bounds.x, bounds.y + bounds.height - side),
                Size::new(side, side),
            )
        } else {
            Rectangle::new(bounds.position(), Size::new(side, side))
        }
    }

    /// The level at the pointer: the lit pill's end follows it, past the
    /// icon's circle.
    fn level_at(&self, bounds: Rectangle, at: Point) -> f32 {
        let side = self.thickness(bounds);
        if self.compact {
            let start = bounds.x + side + COMPACT_KNOB;
            let span = (bounds.width - side - 2.0 * COMPACT_KNOB).max(1.0);
            return ((at.x - start) / span).clamp(0.0, 1.0);
        }
        let span = (self.length(bounds) - side).max(1.0);
        let along = if self.vertical {
            bounds.y + bounds.height - side / 2.0 - at.y
        } else {
            at.x - (bounds.x + side / 2.0)
        };
        (along / span).clamp(0.0, 1.0)
    }
}

#[derive(Default)]
struct LevelState {
    dragging: bool,
    /// Pressed on the icon, not moved yet: a click on it.
    on_icon: Option<Point>,
}

/// Moving this far from a press on the icon makes it a drag.
const DRAG_START: f32 = 4.0;
/// A compact level's knob's radius, and its rail's thickness.
const COMPACT_KNOB: f32 = 7.0;
const COMPACT_RAIL: f32 = 6.0;

impl<Message: Clone> Widget<Message, cosmic::Theme, cosmic::Renderer> for Level<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<LevelState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(LevelState::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.icon)]
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.icon));
    }

    fn size(&self) -> Size<Length> {
        if self.vertical {
            Size::new(Length::Fill, Length::Fill)
        } else {
            Size::new(Length::Fill, Length::Fixed(self.height))
        }
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &cosmic::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let height = if self.vertical { Length::Fill } else { Length::Fixed(self.height) };
        let size = limits
            .width(Length::Fill)
            .height(height)
            .resolve(Length::Fill, height, Size::ZERO);
        let side = if self.vertical { size.width } else { size.height };
        let square = Size::new(side, side);
        let icon = self
            .icon
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, &layout::Limits::new(Size::ZERO, square));
        let top = if self.vertical { size.height - side } else { 0.0 };
        let offset = Point::new(
            (square.width - icon.size().width) / 2.0,
            top + (square.height - icon.size().height) / 2.0,
        );
        layout::Node::with_children(size, vec![icon.move_to(offset)])
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &cosmic::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<LevelState>();
        let bounds = layout.bounds();
        let set = |shell: &mut Shell<'_, Message>, at: Point| {
            shell.publish((self.on_change)(self.level_at(bounds, at)));
        };
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(position) = cursor.position_over(bounds) else {
                    return;
                };
                if self.on_icon_press.is_some() && self.icon_bounds(bounds).contains(position) {
                    state.on_icon = Some(position);
                } else {
                    state.dragging = true;
                    set(shell, position);
                }
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if let Some(pressed) = state.on_icon
                    && position.distance(pressed) > DRAG_START
                {
                    state.on_icon = None;
                    state.dragging = true;
                }
                if state.dragging {
                    set(shell, *position);
                    shell.capture_event();
                }
                shell.request_redraw();
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if state.on_icon.take().is_some() {
                    if let Some(message) = &self.on_icon_press {
                        shell.publish(message.clone());
                    }
                    shell.capture_event();
                } else if state.dragging {
                    state.dragging = false;
                    if let Some(message) = &self.on_release {
                        shell.publish(message.clone());
                    }
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if cursor.is_over(bounds) => {
                let steps = crate::app::scroll_steps(*delta);
                if steps != 0.0 {
                    let value = (self.value + steps * 0.05).clamp(0.0, 1.0);
                    shell.publish((self.on_change)(value));
                    if let Some(message) = &self.on_release {
                        shell.publish(message.clone());
                    }
                }
                shell.capture_event();
            }
            _ => {}
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut cosmic::Renderer,
        theme: &cosmic::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let state = tree.state.downcast_ref::<LevelState>();
        let cosmic = theme.cosmic();
        let component = &cosmic.background(theme.transparent).component;
        let hovered = cursor.is_over(bounds) || state.dragging;
        if self.compact {
            return self.draw_compact(tree, renderer, theme, style, layout, cursor, viewport, hovered);
        }
        let track: Color = if hovered { component.hover } else { component.base }.into();
        let side = self.thickness(bounds);
        let radius = side / 2.0;
        renderer.fill_quad(
            renderer::Quad {
                bounds,
                border: Border {
                    radius: radius.into(),
                    ..Border::default()
                },
                ..renderer::Quad::default()
            },
            track,
        );
        let (lit, on): (Color, Color) = if self.dimmed {
            (component.pressed.into(), component.on.into())
        } else {
            let accent = &cosmic.accent_button;
            (accent.base.into(), readable(accent.on, accent.base))
        };
        // How far it's lit, along it, from the icon's end.
        let lit_length = side + (self.length(bounds) - side) * self.value;
        let lit_bounds = if self.vertical {
            Rectangle::new(
                Point::new(bounds.x, bounds.y + bounds.height - lit_length),
                Size::new(side, lit_length),
            )
        } else {
            Rectangle::new(bounds.position(), Size::new(lit_length, side))
        };
        renderer.fill_quad(
            renderer::Quad {
                bounds: lit_bounds,
                border: Border {
                    radius: radius.into(),
                    ..Border::default()
                },
                ..renderer::Quad::default()
            },
            lit,
        );
        // A knob at the lit part's end, like phones draw it, once it's past
        // the icon.
        if lit_length - side > 12.0 {
        let (knob, at) = if self.vertical {
            let knob = Size::new(side * 0.42, 4.0);
            let y = bounds.y + bounds.height - lit_length + radius / 2.0 - knob.height / 2.0 + 2.0;
            (knob, Point::new(bounds.center_x() - knob.width / 2.0, y))
        } else {
            let knob = Size::new(4.0, side * 0.42);
            let x = bounds.x + lit_length - radius / 2.0 - knob.width / 2.0 - 2.0;
            (knob, Point::new(x, bounds.center_y() - knob.height / 2.0))
        };
        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle::new(at, knob),
                border: Border {
                    radius: (knob.width.min(knob.height) / 2.0).into(),
                    ..Border::default()
                },
                ..renderer::Quad::default()
            },
            Color { a: 0.55, ..on },
        );
        }
        let icon_layout = layout
            .children()
            .next()
            .expect("laid out with its icon")
            .with_virtual_offset(layout.virtual_offset());
        self.icon.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            &renderer::Style {
                icon_color: on,
                text_color: on,
                scale_factor: style.scale_factor,
            },
            icon_layout,
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &cosmic::Renderer,
    ) -> mouse::Interaction {
        let state = tree.state.downcast_ref::<LevelState>();
        let bounds = layout.bounds();
        if state.dragging {
            mouse::Interaction::Grabbing
        } else if let Some(position) = cursor.position_over(bounds) {
            if self.on_icon_press.is_some() && self.icon_bounds(bounds).contains(position) {
                mouse::Interaction::Pointer
            } else {
                mouse::Interaction::Grab
            }
        } else {
            mouse::Interaction::None
        }
    }
}

impl<Message> Level<'_, Message> {
    /// The low level: the icon, a round button's worth of room, then a thin
    /// rail lit up to a round knob.
    #[allow(clippy::too_many_arguments)]
    fn draw_compact(
        &self,
        tree: &Tree,
        renderer: &mut cosmic::Renderer,
        theme: &cosmic::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        hovered: bool,
    ) {
        let bounds = layout.bounds();
        let cosmic = theme.cosmic();
        let component = &cosmic.background(theme.transparent).component;
        let side = bounds.height;
        // Round behind the icon, lighter when hovered, as the tiles.
        let over_icon = cursor.position_over(self.icon_bounds(bounds)).is_some();
        let (lit, on): (Color, Color) = if self.dimmed {
            (component.pressed.into(), component.on.into())
        } else {
            let accent = &cosmic.accent_button;
            (accent.base.into(), readable(accent.on, accent.base))
        };
        renderer.fill_quad(
            renderer::Quad {
                bounds: self.icon_bounds(bounds),
                border: Border {
                    radius: (side / 2.0).into(),
                    ..Border::default()
                },
                ..renderer::Quad::default()
            },
            Color::from(if over_icon { component.hover } else { component.base }),
        );
        let rail = Rectangle::new(
            Point::new(bounds.x + side + COMPACT_KNOB, bounds.center_y() - COMPACT_RAIL / 2.0),
            Size::new((bounds.width - side - 2.0 * COMPACT_KNOB).max(0.0), COMPACT_RAIL),
        );
        let track: Color = if hovered { component.hover } else { component.base }.into();
        let round = Border {
            radius: (COMPACT_RAIL / 2.0).into(),
            ..Border::default()
        };
        renderer.fill_quad(
            renderer::Quad { bounds: rail, border: round, ..renderer::Quad::default() },
            track,
        );
        let x = rail.x + rail.width * self.value;
        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle::new(rail.position(), Size::new(x - rail.x, rail.height)),
                border: round,
                ..renderer::Quad::default()
            },
            lit,
        );
        let knob = if hovered { COMPACT_KNOB + 1.0 } else { COMPACT_KNOB };
        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle::new(
                    Point::new(x - knob, bounds.center_y() - knob),
                    Size::new(2.0 * knob, 2.0 * knob),
                ),
                border: Border {
                    radius: knob.into(),
                    width: 2.0,
                    color: lit,
                },
                ..renderer::Quad::default()
            },
            Color::from(cosmic.background(theme.transparent).base),
        );
        let icon_layout = layout
            .children()
            .next()
            .expect("laid out with its icon")
            .with_virtual_offset(layout.virtual_offset());
        let icon_color: Color = if self.dimmed { on } else { lit };
        self.icon.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            &renderer::Style {
                icon_color,
                text_color: icon_color,
                scale_factor: style.scale_factor,
            },
            icon_layout,
            cursor,
            viewport,
        );
    }
}

impl<'a, Message: Clone + 'a> From<Level<'a, Message>> for cosmic::Element<'a, Message> {
    fn from(level: Level<'a, Message>) -> Self {
        Self::new(level)
    }
}

/// A handle to drag: a pill on an item's edge, as One UI's to resize a
/// tile. Tells how far it's been dragged across since pressed, then when
/// let go.
pub struct Handle<'a, Message> {
    on_drag: Box<dyn Fn(f32) -> Message + 'a>,
    on_release: Message,
}

impl<'a, Message> Handle<'a, Message> {
    pub fn new(on_drag: impl Fn(f32) -> Message + 'a, on_release: Message) -> Self {
        Self {
            on_drag: Box::new(on_drag),
            on_release,
        }
    }
}

#[derive(Default)]
struct HandleState {
    /// Where it was pressed, while dragged.
    from: Option<f32>,
}

/// The handle's size: thin and tall, with room around it to catch.
const HANDLE: Size = Size::new(16.0, 36.0);

impl<Message: Clone> Widget<Message, cosmic::Theme, cosmic::Renderer> for Handle<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<HandleState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(HandleState::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(HANDLE.width), Length::Fixed(HANDLE.height))
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &cosmic::Renderer,
        _limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(HANDLE)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &cosmic::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<HandleState>();
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(position) = cursor.position_over(layout.bounds()) {
                    state.from = Some(position.x);
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if let Some(from) = state.from {
                    shell.publish((self.on_drag)(position.x - from));
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if state.from.take().is_some() {
                    shell.publish(self.on_release.clone());
                    shell.capture_event();
                }
            }
            _ => {}
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut cosmic::Renderer,
        theme: &cosmic::Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let state = tree.state.downcast_ref::<HandleState>();
        let accent: Color = theme.cosmic().accent_color().into();
        let active = state.from.is_some() || cursor.is_over(bounds);
        let bar = Size::new(if active { 6.0 } else { 5.0 }, bounds.height - 8.0);
        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle::new(
                    Point::new(bounds.center_x() - bar.width / 2.0, bounds.y + 4.0),
                    bar,
                ),
                border: Border {
                    radius: (bar.width / 2.0).into(),
                    width: 1.5,
                    color: theme.cosmic().background(theme.transparent).base.into(),
                },
                ..renderer::Quad::default()
            },
            accent,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &cosmic::Renderer,
    ) -> mouse::Interaction {
        let state = tree.state.downcast_ref::<HandleState>();
        if state.from.is_some() || cursor.is_over(layout.bounds()) {
            mouse::Interaction::ResizingHorizontally
        } else {
            mouse::Interaction::None
        }
    }
}

impl<'a, Message: Clone + 'a> From<Handle<'a, Message>> for cosmic::Element<'a, Message> {
    fn from(handle: Handle<'a, Message>) -> Self {
        Self::new(handle)
    }
}

/// Where an element sits on a grid, in cells: column, row, width, height.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub column: u16,
    pub row: u16,
    pub width: u16,
    pub height: u16,
}

/// Elements laid out on a grid of equal columns and rows, each in its own
/// cells: as phones' quick panels, where a slider can stand two rows tall
/// beside the tiles.
pub struct Cells<'a, Message> {
    children: Vec<cosmic::Element<'a, Message>>,
    cells: Vec<Cell>,
    /// Who each child is, to ease it from where it was to where it goes.
    keys: Vec<u64>,
    columns: u16,
    row_height: f32,
    gap: f32,
}

impl<'a, Message> Cells<'a, Message> {
    pub fn new(columns: u16, row_height: f32, gap: f32) -> Self {
        Self {
            children: Vec::new(),
            cells: Vec::new(),
            keys: Vec::new(),
            columns: columns.max(1),
            row_height,
            gap,
        }
    }

    pub fn push(self, cell: Cell, child: impl Into<cosmic::Element<'a, Message>>) -> Self {
        let key = u64::MAX - self.children.len() as u64;
        self.push_keyed(key, cell, child)
    }

    /// A child known by `key`: moved elsewhere, it slides there.
    pub fn push_keyed(
        mut self,
        key: u64,
        cell: Cell,
        child: impl Into<cosmic::Element<'a, Message>>,
    ) -> Self {
        self.cells.push(cell);
        self.keys.push(key);
        self.children.push(child.into());
        self
    }

    fn rows(&self) -> u16 {
        self.cells.iter().map(|c| c.row + c.height).max().unwrap_or(0)
    }
}

/// A grid child sliding from where it was to its new place.
#[derive(Clone, Copy)]
struct Slide {
    from: Point,
    to: Point,
    start: Instant,
}

/// How long a grid child takes to slide to its new place.
const SLIDE: Duration = Duration::from_millis(240);

impl Slide {
    fn at(&self) -> Point {
        let t = (self.start.elapsed().as_secs_f32() / SLIDE.as_secs_f32()).min(1.0);
        // ease out cubic, as the tiles light up
        let t = 1.0 - (1.0 - t).powi(3);
        Point::new(
            self.from.x + (self.to.x - self.from.x) * t,
            self.from.y + (self.to.y - self.from.y) * t,
        )
    }

    fn moving(&self) -> bool {
        self.start.elapsed() < SLIDE && self.from != self.to
    }
}

#[derive(Default)]
struct CellsState {
    slides: std::collections::HashMap<u64, Slide>,
}

impl<Message: Clone> Widget<Message, cosmic::Theme, cosmic::Renderer> for Cells<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<CellsState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(CellsState::default())
    }

    fn children(&self) -> Vec<Tree> {
        self.children.iter().map(Tree::new).collect()
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(&mut self.children);
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &cosmic::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let width = limits.max().width;
        let columns = f32::from(self.columns);
        let column_width = ((width - self.gap * (columns - 1.0)) / columns).max(1.0);
        let rows = self.rows();
        let height = if rows == 0 {
            0.0
        } else {
            f32::from(rows) * self.row_height + f32::from(rows - 1) * self.gap
        };
        let state = tree.state.downcast_mut::<CellsState>();
        let now = Instant::now();
        let mut seen = std::collections::HashSet::new();
        for (key, cell) in self.keys.iter().zip(&self.cells) {
            let to = Point::new(
                f32::from(cell.column) * (column_width + self.gap),
                f32::from(cell.row) * (self.row_height + self.gap),
            );
            seen.insert(*key);
            match state.slides.get_mut(key) {
                Some(slide) if slide.to != to => {
                    *slide = Slide {
                        from: slide.at(),
                        to,
                        start: now,
                    };
                }
                Some(_) => {}
                None => {
                    state.slides.insert(*key, Slide { from: to, to, start: now });
                }
            }
        }
        state.slides.retain(|key, _| seen.contains(key));
        let nodes = self
            .children
            .iter_mut()
            .zip(&mut tree.children)
            .zip(&self.cells)
            .map(|((child, tree), cell)| {
                let size = Size::new(
                    f32::from(cell.width) * column_width + f32::from(cell.width.saturating_sub(1)) * self.gap,
                    f32::from(cell.height) * self.row_height
                        + f32::from(cell.height.saturating_sub(1)) * self.gap,
                );
                let at = Point::new(
                    f32::from(cell.column) * (column_width + self.gap),
                    f32::from(cell.row) * (self.row_height + self.gap),
                );
                child
                    .as_widget_mut()
                    .layout(tree, renderer, &layout::Limits::new(size, size))
                    .move_to(at)
            })
            .collect();
        layout::Node::with_children(Size::new(width, height), nodes)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &cosmic::Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.container(None, layout.bounds());
        operation.traverse(&mut |operation| {
            self.children
                .iter_mut()
                .zip(&mut tree.children)
                .zip(layout.children())
                .for_each(|((child, state), c_layout)| {
                    child.as_widget_mut().operate(
                        state,
                        c_layout.with_virtual_offset(layout.virtual_offset()),
                        renderer,
                        operation,
                    );
                });
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &cosmic::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if let Event::Window(window::Event::RedrawRequested(_)) = event
            && tree.state.downcast_ref::<CellsState>().slides.values().any(Slide::moving)
        {
            shell.request_redraw();
        }
        for ((child, tree), c_layout) in self
            .children
            .iter_mut()
            .zip(&mut tree.children)
            .zip(layout.children())
        {
            child.as_widget_mut().update(
                tree,
                event,
                c_layout.with_virtual_offset(layout.virtual_offset()),
                cursor,
                renderer,
                clipboard,
                shell,
                viewport,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &cosmic::Renderer,
    ) -> mouse::Interaction {
        self.children
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .map(|((child, tree), c_layout)| {
                child.as_widget().mouse_interaction(
                    tree,
                    c_layout.with_virtual_offset(layout.virtual_offset()),
                    cursor,
                    viewport,
                    renderer,
                )
            })
            .max()
            .unwrap_or_default()
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut cosmic::Renderer,
        theme: &cosmic::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let slides = &tree.state.downcast_ref::<CellsState>().slides;
        for (((child, state), c_layout), key) in self
            .children
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .zip(&self.keys)
        {
            // Drawn where it's sliding through, a little off its place.
            let offset = slides.get(key).map_or(Vector::ZERO, |slide| {
                let at = slide.at();
                Vector::new(at.x - slide.to.x, at.y - slide.to.y)
            });
            let c_layout = c_layout.with_virtual_offset(layout.virtual_offset());
            if !(c_layout.bounds() + offset).intersects(viewport) {
                continue;
            }
            renderer.with_translation(offset, |renderer| {
                child.as_widget().draw(state, renderer, theme, style, c_layout, cursor, viewport);
            });
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &cosmic::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, cosmic::Theme, cosmic::Renderer>> {
        overlay::from_children(&mut self.children, tree, layout, renderer, viewport, translation)
    }
}

impl<'a, Message: Clone + 'a> From<Cells<'a, Message>> for cosmic::Element<'a, Message> {
    fn from(cells: Cells<'a, Message>) -> Self {
        Self::new(cells)
    }
}

fn child(layout: Layout<'_>) -> Layout<'_> {
    layout
        .children()
        .next()
        .expect("laid out with its content")
        .with_virtual_offset(layout.virtual_offset())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readable_on_any_accent() {
        let white = Color::from_rgb(1.0, 1.0, 1.0);
        // White on a white or light cyan accent: dark instead.
        assert!(luminance(readable(white, white)) < 0.1);
        assert!(luminance(readable(white, Color::from_rgb(0.58, 0.92, 0.92))) < 0.1);
        // The cyan of #5, under the panel's light grey.
        let grey = Color::from_rgb(0.78, 0.78, 0.78);
        assert!(luminance(readable(grey, Color::from_rgb(0.39, 0.82, 0.87))) < 0.1);
        // The theme's pick, where it shows.
        let blue = Color::from_rgb(0.1, 0.3, 0.7);
        assert_eq!(readable(white, blue), white);
    }
}
