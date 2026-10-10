// SPDX-License-Identifier: GPL-3.0-only

//! Animates the popup when its page changes: the new page slides in, from the
//! right going into a page and from the left coming back, fading in. A popup
//! opening drops in a little.
//!
//! The popup's size is left alone: resizing it every frame makes the software
//! renderer present buffers of the wrong size, and abort.

use std::time::{Duration, Instant};

use cosmic::iced::{
    Color, Event, Length, Rectangle, Size, Vector,
    advanced::{
        Clipboard, Layout, Renderer as _, Shell, Widget, layout, mouse, overlay, renderer,
        widget::{Operation, Tree, tree},
    },
    window,
};

const DURATION: Duration = Duration::from_millis(240);
/// How far a page slides in from, in logical pixels.
const SLIDE: f32 = 48.0;
const DROP: f32 = 10.0;

pub struct Transition<'a, Message> {
    key: u64,
    depth: u8,
    content: cosmic::Element<'a, Message>,
}

impl<'a, Message> Transition<'a, Message> {
    /// `key` names the page; `depth` tells going in (deeper) from coming back.
    pub fn new(key: u64, depth: u8, content: impl Into<cosmic::Element<'a, Message>>) -> Self {
        Self {
            key,
            depth,
            content: content.into(),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Motion {
    Open,
    Forward,
    Back,
    /// Same depth: fade only.
    Swap,
}

struct State {
    key: u64,
    depth: u8,
    motion: Motion,
    start: Instant,
}

impl State {
    fn progress(&self) -> f32 {
        (self.start.elapsed().as_secs_f32() / DURATION.as_secs_f32()).min(1.0)
    }

    /// Ease out: fast, then settling.
    fn eased(&self) -> f32 {
        1.0 - (1.0 - self.progress()).powi(3)
    }

    fn animating(&self) -> bool {
        self.progress() < 1.0
    }
}

impl<Message> Widget<Message, cosmic::Theme, cosmic::Renderer> for Transition<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State {
            key: self.key,
            depth: self.depth,
            motion: Motion::Open,
            start: Instant::now(),
        })
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&mut self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<State>();
        if state.key != self.key {
            state.motion = match self.depth.cmp(&state.depth) {
                std::cmp::Ordering::Greater => Motion::Forward,
                std::cmp::Ordering::Less => Motion::Back,
                std::cmp::Ordering::Equal => Motion::Swap,
            };
            state.key = self.key;
            state.depth = self.depth;
            state.start = Instant::now();
        }
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &cosmic::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let node = self
            .content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        layout::Node::with_children(node.size(), vec![node])
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
        if let Event::Window(window::Event::RedrawRequested(_)) = event {
            let state = tree.state.downcast_ref::<State>();
            if state.animating() {
                shell.request_redraw();
            }
        }
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            content(layout),
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
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
        let state = tree.state.downcast_ref::<State>();
        let bounds = layout.bounds();
        if !state.animating() {
            self.content.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                content(layout),
                cursor,
                viewport,
            );
            return;
        }

        let rest = 1.0 - state.eased();
        let offset = match state.motion {
            Motion::Forward => Vector::new(SLIDE * rest, 0.0),
            Motion::Back => Vector::new(-SLIDE * rest, 0.0),
            Motion::Open => Vector::new(0.0, -DROP * rest),
            Motion::Swap => Vector::ZERO,
        };
        // The panels the page is made of: the veil is over them, not the
        // room between them, where the screen shows.
        let panels: Vec<Rectangle> = match content(layout).children().map(|c| c.bounds()).collect::<Vec<_>>() {
            panels if panels.is_empty() => vec![bounds],
            panels => panels,
        };
        let radius = theme.cosmic().corner_radii.radius_m;
        renderer.with_layer(bounds, |renderer| {
            renderer.with_translation(offset, |renderer| {
                self.content.as_widget().draw(
                    &tree.children[0],
                    renderer,
                    theme,
                    style,
                    content(layout),
                    cursor,
                    viewport,
                );
                // Fade in: a veil of the popup's own color, thinning out.
                let mut veil: Color = theme.cosmic().background(theme.transparent).base.into();
                veil.a *= rest;
                for panel in &panels {
                    renderer.fill_quad(
                        renderer::Quad {
                            bounds: *panel,
                            border: cosmic::iced::Border {
                                radius: radius.into(),
                                ..Default::default()
                            },
                            ..renderer::Quad::default()
                        },
                        veil,
                    );
                }
            });
        });
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &cosmic::Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            content(layout),
            cursor,
            viewport,
            renderer,
        )
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &cosmic::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            content(layout),
            renderer,
            operation,
        );
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
            content(layout),
            renderer,
            viewport,
            translation,
        )
    }
}

fn content(layout: Layout<'_>) -> Layout<'_> {
    layout
        .children()
        .next()
        .expect("laid out with its content")
        .with_virtual_offset(layout.virtual_offset())
}

impl<'a, Message: 'a> From<Transition<'a, Message>> for cosmic::Element<'a, Message> {
    fn from(transition: Transition<'a, Message>) -> Self {
        Self::new(transition)
    }
}
