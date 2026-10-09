// SPDX-License-Identifier: GPL-3.0-only

//! Tiling of the active workspace on this panel's output, through COSMIC's
//! workspace protocol, as the tiling applet does it.

use std::os::{
    fd::{FromRawFd, RawFd},
    unix::net::UnixStream,
};

use cosmic::{
    cctk::{
        self,
        cosmic_protocols::workspace::v2::client::zcosmic_workspace_handle_v2::TilingState,
        sctk::{
            self,
            output::{OutputHandler, OutputState},
            reexports::{
                calloop, calloop_wayland_source::WaylandSource,
                protocols::ext::workspace::v1::client::ext_workspace_handle_v1,
            },
            registry::{ProvidesRegistryState, RegistryState},
        },
        wayland_client::{
            Connection, QueueHandle, WEnum, globals::registry_queue_init,
            protocol::wl_output::WlOutput,
        },
        workspace::{Workspace, WorkspaceHandler, WorkspaceState},
    },
    iced::{
        Subscription,
        futures::{self, SinkExt, channel::mpsc},
        stream,
    },
};

#[derive(Clone, Debug)]
pub enum Event {
    Ready(calloop::channel::Sender<bool>),
    /// Whether the active workspace tiles its windows.
    Tiling(bool),
}

pub fn subscription() -> Subscription<Event> {
    Subscription::run_with(std::any::TypeId::of::<Event>(), |_| {
        stream::channel(4, |mut output: mpsc::Sender<Event>| async move {
            let (tx, rx) = calloop::channel::channel();
            _ = output.send(Event::Ready(tx)).await;
            std::thread::spawn(move || {
                if let Err(err) = run(output, rx) {
                    tracing::warn!("workspace tracking stopped: {err}");
                }
            });
            futures::future::pending::<()>().await;
        })
    })
}

struct State {
    registry_state: RegistryState,
    output_state: OutputState,
    workspace_state: WorkspaceState,
    /// The output this panel is on, by name, and once known.
    output_name: String,
    output: Option<WlOutput>,
    tx: mpsc::Sender<Event>,
    last: Option<bool>,
}

impl State {
    fn active(&self) -> Option<&Workspace> {
        self.workspace_state.workspace_groups().find_map(|group| {
            if self.output.as_ref().is_some_and(|o| !group.outputs.contains(o)) {
                return None;
            }
            group
                .workspaces
                .iter()
                .filter_map(|handle| self.workspace_state.workspace_info(handle))
                .find(|w| w.state.contains(ext_workspace_handle_v1::State::Active))
        })
    }

    fn report(&mut self) {
        let tiling = self.active().and_then(|w| w.tiling).map(|t| {
            matches!(t, WEnum::Value(TilingState::TilingEnabled))
        });
        if let Some(tiling) = tiling.filter(|t| Some(*t) != self.last) {
            self.last = Some(tiling);
            _ = futures::executor::block_on(self.tx.send(Event::Tiling(tiling)));
        }
    }

    fn set_tiling(&mut self, tiling: bool) {
        let Some(handle) = self.active().and_then(|w| w.cosmic_handle.clone()) else {
            return;
        };
        handle.set_tiling_state(if tiling {
            TilingState::TilingEnabled
        } else {
            TilingState::FloatingOnly
        });
        if let Ok(manager) = self.workspace_state.workspace_manager().get() {
            manager.commit();
        }
    }
}

fn run(
    tx: mpsc::Sender<Event>,
    rx: calloop::channel::Channel<bool>,
) -> Result<(), Box<dyn std::error::Error>> {
    // The real compositor, not the panel's, which applets live in.
    let socket = std::env::var("X_PRIVILEGED_WAYLAND_SOCKET")
        .ok()
        .and_then(|fd| fd.parse::<RawFd>().ok())
        .map(|fd| unsafe { UnixStream::from_raw_fd(fd) });
    let conn = match socket {
        Some(socket) => Connection::from_socket(socket)?,
        None => Connection::connect_to_env()?,
    };
    let (globals, event_queue) = registry_queue_init(&conn)?;
    let qh = event_queue.handle();
    let mut event_loop = calloop::EventLoop::<State>::try_new()?;
    WaylandSource::new(conn, event_queue)
        .insert(event_loop.handle())
        .map_err(|err| err.error)?;
    event_loop
        .handle()
        .insert_source(rx, |event, (), state| {
            if let calloop::channel::Event::Msg(tiling) = event {
                state.set_tiling(tiling);
            }
        })
        .map_err(|err| err.error)?;

    let registry_state = RegistryState::new(&globals);
    let mut state = State {
        output_state: OutputState::new(&globals, &qh),
        workspace_state: WorkspaceState::new(&registry_state, &qh),
        registry_state,
        output_name: std::env::var("COSMIC_PANEL_OUTPUT").unwrap_or_default(),
        output: None,
        tx,
        last: None,
    };
    loop {
        event_loop.dispatch(None, &mut state)?;
    }
}

impl WorkspaceHandler for State {
    fn workspace_state(&mut self) -> &mut WorkspaceState {
        &mut self.workspace_state
    }

    fn done(&mut self) {
        self.report();
    }
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, output: WlOutput) {
        let name = self.output_state.info(&output).and_then(|i| i.name);
        if name.as_deref() == Some(self.output_name.as_str()) {
            self.output = Some(output);
            self.report();
        }
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}
}

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    sctk::registry_handlers![OutputState];
}

sctk::delegate_output!(State);
sctk::delegate_registry!(State);
cctk::delegate_workspace!(State);
