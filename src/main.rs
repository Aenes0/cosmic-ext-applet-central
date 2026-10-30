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
mod panels;
mod services;
mod show_desktop;
mod slow;
mod texts;
mod tiles;
mod timing;
mod transition;
mod tray;
mod widgets;
mod workspaces;

pub const APP_ID: &str = "io.github.Aenes0.CosmicCentral";

fn main() -> cosmic::iced::Result {
    let args: Vec<String> = std::env::args().collect();
    let panels = args.iter().position(|arg| arg == "--panels");
    // A couple of threads do: an applet waits on events, it doesn't crunch;
    // one per core (24 here) only took memory. The panels' host, one.
    if panels.is_some() {
        // SAFETY: first thing, before any thread starts.
        unsafe { std::env::set_var("TOKIO_WORKER_THREADS", "1") };
    } else if std::env::var_os("TOKIO_WORKER_THREADS").is_none() {
        // SAFETY: first thing, before any thread starts.
        unsafe { std::env::set_var("TOKIO_WORKER_THREADS", "2") };
    }
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        // A write that fails is dropped: told on stderr, if that is what
        // failed, it would abort the program.
        .log_internal_errors(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,cosmic_ext_applet_central=info".into()),
        )
        .init();
    // The Central's and the monitor's panels, for every screen: one host,
    // hidden between uses; a second run only hands it what it was asked.
    if let Some(at) = panels {
        let request = panels::place::Request::from_args(&args[at + 1..]);
        // Run by its unit, it serves whatever runs already: it takes the
        // bus over, and one started by hand leaves.
        if !panels::service::under_unit() && panels::service::forward(request.as_ref()) {
            return Ok(());
        }
        panels::own_output();
        return panels::run(request);
    }
    // The same program is also the "show desktop" button, its own applet.
    if std::env::args().any(|arg| arg == "--show-desktop") {
        return cosmic::applet::run::<show_desktop::ShowDesktop>(());
    }
    // And the system monitor, at the bar's other end.
    if std::env::args().any(|arg| arg == "--monitor") {
        return cosmic::applet::run::<monitor::Monitor>(());
    }
    // And the tray of background apps.
    if std::env::args().any(|arg| arg == "--tray") {
        return tray::run();
    }
    // And the clipboard's history, a service of its own.
    if std::env::args().any(|arg| arg == "--clipboard") {
        clipboard::service::main()
    }
    // And the notifications' history, another service of its own.
    if std::env::args().any(|arg| arg == "--notifications") {
        notifications::service::main()
    }
    cosmic::applet::run::<app::App>(())
}
