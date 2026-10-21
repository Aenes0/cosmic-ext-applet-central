// Copyright 2023 System76 <info@system76.com>, 2026 Aenes0
// SPDX-License-Identifier: GPL-3.0-only

//! The tray, as on Windows 11: background apps (StatusNotifierItem) on the
//! bar, the ones not chosen in a box under a ^; dragging picks which show.
//! Its own applet, by the same program (a fork of cosmic-applet-status-area).

mod components;
mod config;
mod dnd;
mod identity;
mod layout;
mod subscriptions;

/// The id it had as its own program: the panel's entry and the config keep it.
pub const APP_ID: &str = "io.github.Aenes0.CosmicTray";

pub fn run() -> cosmic::iced::Result {
    tracing::info!("Starting tray applet with version {}", env!("CARGO_PKG_VERSION"));
    cosmic::applet::run::<components::app::App>(())
}
