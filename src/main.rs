// SPDX-License-Identifier: GPL-3.0-only

mod app;
mod audio;
mod bar_spacing;
mod board;
mod calendar;
mod clipboard;
mod host;
mod i18n;
mod layout;
mod latest;
mod media;
mod notifications;
mod pages;
mod services;
mod show_desktop;
mod start;
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
    // The same program is also the "show desktop" button, its own applet.
    if std::env::args().any(|arg| arg == "--show-desktop") {
        return cosmic::applet::run::<show_desktop::ShowDesktop>(());
    }
    // And the start menu, which stays running, hidden between uses: a
    // second run only asks the first to show or hide.
    if std::env::args().any(|arg| arg == "--start") {
        if start::toggle_running() {
            return Ok(());
        }
        let settings = cosmic::app::Settings::default()
            .no_main_window(true)
            .exit_on_close(false)
            .transparent(true);
        return cosmic::app::run::<start::StartMenu>(settings, ());
    }
    if std::env::args().any(|arg| arg == "--start-button") {
        return cosmic::applet::run::<start::StartButton>(());
    }
    cosmic::applet::run::<app::App>(())
}
