// SPDX-License-Identifier: GPL-3.0-only

mod app;
mod audio;
mod bar_spacing;
mod board;
mod calendar;
mod host;
mod i18n;
mod layout;
mod latest;
mod media;
mod notifications;
mod pages;
mod services;
mod texts;
mod tiles;
mod transition;
mod widgets;
mod workspaces;

pub const APP_ID: &str = "io.github.Aenes0.CosmicCentral";

fn main() -> cosmic::iced::Result {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,cosmic_ext_applet_central=info".into()),
        )
        .init();
    cosmic::applet::run::<app::App>(())
}
