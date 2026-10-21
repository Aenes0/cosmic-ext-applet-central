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
mod monitor;
mod notifications;
mod pages;
mod services;
mod show_desktop;
mod texts;
mod tiles;
mod transition;
mod widgets;
mod workspaces;

pub const APP_ID: &str = "io.github.Aenes0.CosmicCentral";

fn main() -> cosmic::iced::Result {
    // A couple of threads do: an applet waits on events, it doesn't crunch;
    // one per core (24 here) only took memory.
    if std::env::var_os("TOKIO_WORKER_THREADS").is_none() {
        // SAFETY: first thing, before any thread starts.
        unsafe { std::env::set_var("TOKIO_WORKER_THREADS", "2") };
    }
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
    // And the system monitor, at the bar's other end.
    if std::env::args().any(|arg| arg == "--monitor") {
        return cosmic::applet::run::<monitor::Monitor>(());
    }
    cosmic::applet::run::<app::App>(())
}
