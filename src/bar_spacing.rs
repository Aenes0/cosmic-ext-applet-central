// SPDX-License-Identifier: GPL-3.0-only

//! How far apart the icons on the panel are: close, as on a laptop, or
//! further apart on a big desktop screen.

use cosmic::cosmic_config::{Config, ConfigGet, ConfigSet};

use crate::i18n::strings;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BarSpacing {
    #[default]
    Small,
    Medium,
    Large,
}

impl BarSpacing {
    pub const ALL: [BarSpacing; 3] = [BarSpacing::Small, BarSpacing::Medium, BarSpacing::Large];

    /// How it's saved in the configuration.
    pub fn id(self) -> &'static str {
        match self {
            BarSpacing::Small => "small",
            BarSpacing::Medium => "medium",
            BarSpacing::Large => "large",
        }
    }

    pub fn from_id(id: &str) -> Option<BarSpacing> {
        Self::ALL.into_iter().find(|s| s.id() == id)
    }

    pub fn name(self) -> &'static str {
        let t = strings();
        match self {
            BarSpacing::Small => t.spacing_small,
            BarSpacing::Medium => t.spacing_medium,
            BarSpacing::Large => t.spacing_large,
        }
    }

    /// Between the icons, from the theme's spacing.
    pub fn gap(self) -> u16 {
        let spacing = cosmic::theme::spacing();
        match self {
            BarSpacing::Small => spacing.space_xxs,
            BarSpacing::Medium => spacing.space_xs,
            BarSpacing::Large => spacing.space_s,
        }
    }

    /// The one picked, from the configuration.
    pub fn load() -> BarSpacing {
        Config::new(crate::APP_ID, 1)
            .and_then(|c| c.get::<String>("bar_spacing"))
            .ok()
            .and_then(|id| BarSpacing::from_id(&id))
            .unwrap_or_default()
    }

    pub fn save(self) {
        if let Err(err) =
            Config::new(crate::APP_ID, 1).and_then(|c| c.set("bar_spacing", self.id()))
        {
            tracing::warn!("cannot save the panel's spacing: {err}");
        }
    }
}
