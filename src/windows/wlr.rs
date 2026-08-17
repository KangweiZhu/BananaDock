//! Window tracking via `wlr-foreign-toplevel-management-v1`.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use smithay_client_toolkit::{dispatch2::Dispatch2, registry::GlobalProxy};
use wayland_client::{
    globals::GlobalList,
    protocol::{wl_seat, wl_surface},
    Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1 as handle, zwlr_foreign_toplevel_manager_v1 as manager,
};

use super::{Toplevel, ToplevelId, WindowSource};

/// Protocol version we ask for. v3 adds the `parent` event, which is how
/// dialogs get attributed to the window that owns them; compositors that only
/// offer less simply never send it.
const VERSION: u32 = 3;

#[derive(Debug, Default)]
struct ToplevelInner {
    /// Applied on `done`. The protocol sends title/app_id/state as a batch and
    /// only `done` makes them coherent, so applying them as they arrive would
    /// show torn state.
    pending: Toplevel,
    committed: bool,
}

#[doc(hidden)]
#[derive(Debug, Default, Clone)]
pub struct ToplevelData(Arc<Mutex<ToplevelInner>>);

/// User data for the manager global.
///
/// SCTK has a `GlobalData` marker for exactly this, but both it and `Dispatch2`
/// are foreign types, so using it here would trip the orphan rule. A local
/// marker is the whole fix.
#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
pub struct ManagerData;

/// Tracks every open window the compositor tells us about.
pub struct ForeignToplevelManager {
    proxy: GlobalProxy<manager::ZwlrForeignToplevelManagerV1>,
    /// Public view, in the order the compositor announced the windows.
    toplevels: Vec<Toplevel>,
    /// Parallel lookup for sending requests back.
    handles: HashMap<ToplevelId, handle::ZwlrForeignToplevelHandleV1>,
    /// `activate` is defined per-seat, so a seat has to be on hand before an
    /// icon click can do anything.
    seat: Option<wl_seat::WlSeat>,
}

impl ForeignToplevelManager {
    pub fn new<D>(globals: &GlobalList, qh: &QueueHandle<D>) -> Self
    where
        D: Dispatch<manager::ZwlrForeignToplevelManagerV1, ManagerData> + 'static,
    {
        Self {
            proxy: GlobalProxy::from(globals.bind(qh, 1..=VERSION, ManagerData)),
            toplevels: Vec::new(),
            handles: HashMap::new(),
            seat: None,
        }
    }

    /// Whether the compositor implements the protocol at all. Without it the
    /// dock can still show pinned launchers, but never a running window.
    pub fn is_available(&self) -> bool {
        self.proxy.get().is_ok()
    }

    pub fn set_seat(&mut self, seat: wl_seat::WlSeat) {
        self.seat = Some(seat);
    }

    fn handle_for(&self, id: ToplevelId) -> Option<&handle::ZwlrForeignToplevelHandleV1> {
        self.handles.get(&id)
    }
}

impl WindowSource for ForeignToplevelManager {
    fn toplevels(&self) -> &[Toplevel] {
        &self.toplevels
    }

    fn activate(&self, id: ToplevelId) {
        if let (Some(handle), Some(seat)) = (self.handle_for(id), self.seat.as_ref()) {
            handle.activate(seat);
        }
    }

    fn set_minimized(&self, id: ToplevelId, minimized: bool) {
        let Some(handle) = self.handle_for(id) else {
            return;
        };
        if minimized {
            handle.set_minimized();
        } else {
            handle.unset_minimized();
        }
    }

    fn close(&self, id: ToplevelId) {
        if let Some(handle) = self.handle_for(id) {
            handle.close();
        }
    }

    fn set_icon_rect(
        &self,
        id: ToplevelId,
        surface: &wl_surface::WlSurface,
        rect: (i32, i32, i32, i32),
    ) {
        if let Some(handle) = self.handle_for(id) {
            handle.set_rectangle(surface, rect.0, rect.1, rect.2, rect.3);
        }
    }
}

/// Implemented by the application so it learns when the window list moved.
pub trait ForeignToplevelHandler: Sized {
    fn foreign_toplevel_state(&mut self) -> &mut ForeignToplevelManager;

    /// The window list or any window's state changed.
    fn toplevels_changed(&mut self, conn: &Connection, qh: &QueueHandle<Self>);
}

impl<D> Dispatch2<manager::ZwlrForeignToplevelManagerV1, D> for ManagerData
where
    D: Dispatch<handle::ZwlrForeignToplevelHandleV1, ToplevelData>
        + ForeignToplevelHandler
        + 'static,
{
    fn event(
        &self,
        _state: &mut D,
        _proxy: &manager::ZwlrForeignToplevelManagerV1,
        event: manager::Event,
        _conn: &Connection,
        _qh: &QueueHandle<D>,
    ) {
        match event {
            // The handle arrives here already created; its own events carry
            // everything about it, so there is nothing to record yet.
            manager::Event::Toplevel { .. } => {}
            // `finished` is a destructor event: the object is already gone
            // server-side, so there is nothing to send.
            manager::Event::Finished => {}
            _ => {}
        }
    }

    wayland_client::event_created_child!(D, manager::ZwlrForeignToplevelManagerV1, [
        manager::EVT_TOPLEVEL_OPCODE => (handle::ZwlrForeignToplevelHandleV1, Default::default())
    ]);
}

impl<D> Dispatch2<handle::ZwlrForeignToplevelHandleV1, D> for ToplevelData
where
    D: ForeignToplevelHandler,
{
    fn event(
        &self,
        state: &mut D,
        proxy: &handle::ZwlrForeignToplevelHandleV1,
        event: handle::Event,
        conn: &Connection,
        qh: &QueueHandle<D>,
    ) {
        let id = proxy.id().protocol_id();

        match event {
            handle::Event::Title { title } => self.0.lock().unwrap().pending.title = title,
            handle::Event::AppId { app_id } => self.0.lock().unwrap().pending.app_id = app_id,
            handle::Event::State { state: flags } => {
                let mut inner = self.0.lock().unwrap();
                let states = decode_states(&flags);
                inner.pending.active = states.contains(&handle::State::Activated);
                inner.pending.minimized = states.contains(&handle::State::Minimized);
            }
            handle::Event::Parent { parent } => {
                self.0.lock().unwrap().pending.parent = parent.map(|p| p.id().protocol_id());
            }
            handle::Event::Done => {
                let mut inner = self.0.lock().unwrap();
                inner.pending.id = id;
                let snapshot = inner.pending.clone();
                let first = !inner.committed;
                inner.committed = true;
                drop(inner);

                let mgr = state.foreign_toplevel_state();
                if first {
                    mgr.handles.insert(id, proxy.clone());
                    mgr.toplevels.push(snapshot);
                } else if let Some(slot) = mgr.toplevels.iter_mut().find(|t| t.id == id) {
                    *slot = snapshot;
                }
                state.toplevels_changed(conn, qh);
            }
            handle::Event::Closed => {
                let mgr = state.foreign_toplevel_state();
                mgr.toplevels.retain(|t| t.id != id);
                mgr.handles.remove(&id);
                proxy.destroy();
                state.toplevels_changed(conn, qh);
            }
            // OutputEnter/OutputLeave matter only once the dock is per-screen.
            _ => {}
        }
    }
}

/// The `state` event carries its flags as an array of native-endian `u32`s
/// rather than a bitfield, so it has to be unpacked by hand.
fn decode_states(raw: &[u8]) -> Vec<handle::State> {
    raw.chunks_exact(4)
        .filter_map(|c| {
            let v = u32::from_ne_bytes([c[0], c[1], c[2], c[3]]);
            handle::State::try_from(v).ok()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_multi_state_array() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&(handle::State::Maximized as u32).to_ne_bytes());
        raw.extend_from_slice(&(handle::State::Activated as u32).to_ne_bytes());

        let states = decode_states(&raw);
        assert!(states.contains(&handle::State::Activated));
        assert!(states.contains(&handle::State::Maximized));
        assert!(!states.contains(&handle::State::Minimized));
    }

    #[test]
    fn empty_state_array_means_no_states() {
        assert!(decode_states(&[]).is_empty());
    }

    /// A compositor advertising a state this build does not know must not take
    /// the whole array down with it.
    #[test]
    fn unknown_state_values_are_skipped() {
        let raw = 9999u32.to_ne_bytes();
        assert!(decode_states(&raw).is_empty());
    }
}
