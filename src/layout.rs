// SPDX-License-Identifier: GPL-3.0-only

//! How roomy the popup and the icons on the panel are: as a phone's, or
//! wider and airier for a big desktop screen.

use std::sync::atomic::{AtomicU8, Ordering};

use cosmic::cosmic_config::{Config, ConfigGet, ConfigSet};

use crate::i18n::strings;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Size {
    #[default]
    Default,
    Large,
    ExtraLarge,
}

impl Size {
    pub const ALL: [Size; 3] = [Size::Default, Size::Large, Size::ExtraLarge];

    /// How it's saved in the configuration.
    pub fn id(self) -> &'static str {
        match self {
            Size::Default => "default",
            Size::Large => "large",
            Size::ExtraLarge => "extra-large",
        }
    }

    pub fn from_id(id: &str) -> Option<Size> {
        Self::ALL.into_iter().find(|s| s.id() == id)
    }

    pub fn name(self) -> &'static str {
        let t = strings();
        match self {
            Size::Default => t.size_default,
            Size::Large => t.size_large,
            Size::ExtraLarge => t.size_extra_large,
        }
    }

    /// Of the theme's spacing, and of the things drawn.
    fn factors(self) -> (f32, f32) {
        match self {
            Size::Default => (1.0, 1.0),
            Size::Large => (1.4, 1.125),
            Size::ExtraLarge => (1.8, 1.25),
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn current() -> Size {
    Size::ALL[usize::from(CURRENT.load(Ordering::Relaxed)).min(Size::ALL.len() - 1)]
}

/// The size picked, from the configuration.
pub fn load() -> Size {
    let size = Config::new(crate::APP_ID, 1)
        .and_then(|c| c.get::<String>("size"))
        .ok()
        .and_then(|id| Size::from_id(&id))
        .unwrap_or_default();
    CURRENT.store(size as u8, Ordering::Relaxed);
    size
}

pub fn set(size: Size) {
    CURRENT.store(size as u8, Ordering::Relaxed);
    if let Err(err) = Config::new(crate::APP_ID, 1).and_then(|c| c.set("size", size.id())) {
        tracing::warn!("cannot save the size: {err}");
    }
}

/// The popup's measures at the size picked.
pub struct Metrics {
    pub width: f32,
    /// Around and between the sections of a panel.
    pub gap: u16,
    /// Between things close together: the sliders, the cards.
    pub near: u16,
    /// Between the popup's panels.
    pub panel_gap: f32,
    /// Between tiles.
    pub grid_gap: u16,
    pub tile_height: f32,
    /// A tile's inside, left and right.
    pub tile_padding: u16,
    pub icon: u16,
    /// The thick sliders' height, and their arrows'.
    pub slider: f32,
    /// The round buttons at the top.
    pub button: f32,
    pub cover: f32,
    pub card_padding: u16,
    pub app_icon: u16,
    /// Between the icons on the panel.
    pub bar_gap: u16,
}

pub fn metrics() -> Metrics {
    let size = current();
    let (space, scale) = size.factors();
    let spacing = cosmic::theme::spacing();
    let room = |v: u16| (f32::from(v) * space).round() as u16;
    let grow = |v: f32| (v * scale).round();
    Metrics {
        width: match size {
            Size::Default => 420.0,
            Size::Large => 500.0,
            Size::ExtraLarge => 580.0,
        },
        gap: room(spacing.space_s),
        near: room(spacing.space_xs),
        panel_gap: (8.0 * space).round(),
        grid_gap: room(spacing.space_xs),
        tile_height: grow(64.0),
        tile_padding: grow(14.0) as u16,
        icon: grow(20.0) as u16,
        slider: grow(44.0),
        button: grow(36.0),
        cover: grow(56.0),
        card_padding: grow(12.0) as u16,
        app_icon: grow(32.0) as u16,
        bar_gap: match size {
            Size::Default => spacing.space_xxs,
            Size::Large => spacing.space_xs,
            Size::ExtraLarge => spacing.space_s,
        },
    }
}
