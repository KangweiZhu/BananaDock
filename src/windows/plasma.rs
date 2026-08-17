//! Window tracking on KWin, over `org_kde_plasma_window_management`.
//!
//! KWin does implement a perfectly good window-management protocol; it just
//! does not hand it to any client that asks. A client gets it only if its
//! `.desktop` file names the interface:
//!
//! ```text
//! X-KDE-Wayland-Interfaces=org_kde_plasma_window_management
//! ```
//!
//! KWin maps the connecting process back to a desktop entry and checks that
//! list. Two things about the check are easy to get wrong, and both fail
//! silently -- the global simply never appears:
//!
//! * The value is parsed as a KConfig list, so entries are separated by
//!   **commas**, not by the semicolons the desktop-entry spec uses. A trailing
//!   semicolon makes the whole entry unmatchable.
//! * `Exec=` has to name the binary that actually runs, since that is how the
//!   process is traced back to the entry.
//!
//! Without the grant the dock falls back to [`super::kwin`], which drives a
//! KWin script over D-Bus. This module is preferred whenever it is available:
//! it is event-driven rather than polled, needs nothing injected into the
//! compositor, and can point KWin's minimise animation at the right dock slot.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use smithay_client_toolkit::{dispatch2::Dispatch2, registry::GlobalProxy};
use wayland_client::{globals::GlobalList, Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols_plasma::plasma_window_management::client::{
    org_kde_plasma_window as window, org_kde_plasma_window_management as manager,
};

use super::{Capabilities, Toplevel, ToplevelId, WindowSource};

/// v13 introduced `window_with_uuid`, which is how windows are announced here.
/// KWin 6 offers v20, but the bindings crate ships XML up to v18 and binding
/// above what the proxy knows is a hard error, so this is capped at 18.
const VERSION: u32 = 18;

/// Bits of `org_kde_plasma_window.state`.
mod state {
    pub const ACTIVE: u32 = 0x1;
    pub const MINIMIZED: u32 = 0x2;
    pub const SKIPTASKBAR: u32 = 0x1000;
}

#[derive(Debug, Default)]
struct WindowInner {
    uuid: String,
    app_id: String,
    title: String,
    state: u32,
    parent_uuid: Option<String>,
    /// Set once KWin has finished the opening burst of events.
    ready: bool,
    /// The compositor withdrew the window; it is on its way out.
    unmapped: bool,
}

#[doc(hidden)]
#[derive(Debug, Default, Clone)]
pub struct PlasmaWindowData(Arc<Mutex<WindowInner>>);

/// User data for the manager global. A local type, since implementing a foreign
/// trait for a foreign marker would trip the orphan rule.
#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
pub struct PlasmaManagerData;

/// Tracks windows through Plasma's window-management protocol.
pub struct PlasmaWindows {
    proxy: GlobalProxy<manager::OrgKdePlasmaWindowManagement>,
    /// Public view, in announcement order.
    toplevels: Vec<Toplevel>,
    /// Live protocol objects, for sending requests back.
    handles: HashMap<ToplevelId, window::OrgKdePlasmaWindow>,
}

impl PlasmaWindows {
    pub fn new<D>(globals: &GlobalList, qh: &QueueHandle<D>) -> Self
    where
        D: Dispatch<manager::OrgKdePlasmaWindowManagement, PlasmaManagerData> + 'static,
    {
        Self {
            proxy: GlobalProxy::from(globals.bind(qh, 1..=VERSION, PlasmaManagerData)),
            toplevels: Vec::new(),
            handles: HashMap::new(),
        }
    }

    /// Whether KWin granted the interface. False means the `.desktop` file is
    /// missing, unmatched, or does not name the interface.
    pub fn is_available(&self) -> bool {
        self.proxy.get().is_ok()
    }

    fn handle_for(&self, id: ToplevelId) -> Option<&window::OrgKdePlasmaWindow> {
        self.handles.get(&id)
    }

    /// Rebuilds the public list from whatever the tracked windows currently say.
    fn refresh(&mut self) {
        let mut out = Vec::with_capacity(self.handles.len());
        // Announcement order is what the row should follow, and `handles` is a
        // map, so walk the existing list first and append anything new.
        let mut seen: Vec<ToplevelId> = Vec::new();

        for existing in &self.toplevels {
            if let Some(t) = self.snapshot(existing.id) {
                out.push(t);
                seen.push(existing.id);
            }
        }
        let mut fresh: Vec<ToplevelId> = self
            .handles
            .keys()
            .copied()
            .filter(|id| !seen.contains(id))
            .collect();
        fresh.sort_unstable();
        for id in fresh {
            if let Some(t) = self.snapshot(id) {
                out.push(t);
            }
        }

        self.toplevels = out;
    }

    fn snapshot(&self, id: ToplevelId) -> Option<Toplevel> {
        let handle = self.handles.get(&id)?;
        let data = handle.data::<PlasmaWindowData>()?;
        let inner = data.0.lock().ok()?;

        // A window is only shown once KWin says its opening burst is done, and
        // never if it asked to stay out of task bars.
        if !inner.ready || inner.unmapped || inner.state & state::SKIPTASKBAR != 0 {
            return None;
        }

        Some(Toplevel {
            id,
            app_id: inner.app_id.clone(),
            title: inner.title.clone(),
            active: inner.state & state::ACTIVE != 0,
            minimized: inner.state & state::MINIMIZED != 0,
            // Parents arrive as uuids; resolve to our id where the parent is
            // itself a window we track.
            parent: inner
                .parent_uuid
                .as_deref()
                .and_then(|uuid| self.id_of_uuid(uuid)),
            // KWin's own window id, which is what its screenshot interface
            // takes.
            capture_key: Some(inner.uuid.clone()),
        })
    }

    fn id_of_uuid(&self, uuid: &str) -> Option<ToplevelId> {
        self.handles.iter().find_map(|(id, handle)| {
            let data = handle.data::<PlasmaWindowData>()?;
            let inner = data.0.lock().ok()?;
            (inner.uuid == uuid).then_some(*id)
        })
    }
}

impl WindowSource for PlasmaWindows {
    fn toplevels(&self) -> &[Toplevel] {
        &self.toplevels
    }

    fn capabilities(&self) -> Capabilities {
        // The protocol reports per-window whether it may be closed or
        // minimised, but the menu is built per application; treat the
        // capability as present and let individual requests be ignored.
        Capabilities::default()
    }

    fn activate(&self, id: ToplevelId) {
        if let Some(handle) = self.handle_for(id) {
            // Activating also clears minimisation: a window raised from the
            // dock should come back, not come back still hidden.
            handle.set_state(state::MINIMIZED, 0);
            handle.set_state(state::ACTIVE, state::ACTIVE);
        }
    }

    fn set_minimized(&self, id: ToplevelId, minimized: bool) {
        if let Some(handle) = self.handle_for(id) {
            handle.set_state(
                state::MINIMIZED,
                if minimized { state::MINIMIZED } else { 0 },
            );
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
        surface: &wayland_client::protocol::wl_surface::WlSurface,
        rect: (i32, i32, i32, i32),
    ) {
        if let Some(handle) = self.handle_for(id) {
            // Tells KWin where to fly the minimise animation.
            // The protocol types the rectangle as unsigned; a negative
            // coordinate would wrap, so clamp instead of casting blindly.
            handle.set_minimized_geometry(
                surface,
                rect.0.max(0) as u32,
                rect.1.max(0) as u32,
                rect.2.max(0) as u32,
                rect.3.max(0) as u32,
            );
        }
    }
}

/// Implemented by the application so it learns when the window list moved.
pub trait PlasmaWindowHandler: Sized {
    fn plasma_state(&mut self) -> &mut PlasmaWindows;
    fn plasma_windows_changed(&mut self, conn: &Connection, qh: &QueueHandle<Self>);
}

impl<D> Dispatch2<manager::OrgKdePlasmaWindowManagement, D> for PlasmaManagerData
where
    D: Dispatch<window::OrgKdePlasmaWindow, PlasmaWindowData> + PlasmaWindowHandler + 'static,
{
    fn event(
        &self,
        state: &mut D,
        proxy: &manager::OrgKdePlasmaWindowManagement,
        event: manager::Event,
        conn: &Connection,
        qh: &QueueHandle<D>,
    ) {
        // The uuid form supersedes the older `window` event, which only carries
        // an id that cannot be turned back into an object.
        if let manager::Event::WindowWithUuid { uuid, .. } = event {
            let handle = proxy.get_window_by_uuid(uuid.clone(), qh, PlasmaWindowData::default());
            if let Some(data) = handle.data::<PlasmaWindowData>() {
                if let Ok(mut inner) = data.0.lock() {
                    inner.uuid = uuid;
                }
            }
            let id = handle.id().protocol_id();
            state.plasma_state().handles.insert(id, handle);
            state.plasma_windows_changed(conn, qh);
        }
    }
}

impl<D> Dispatch2<window::OrgKdePlasmaWindow, D> for PlasmaWindowData
where
    D: PlasmaWindowHandler,
{
    fn event(
        &self,
        state: &mut D,
        proxy: &window::OrgKdePlasmaWindow,
        event: window::Event,
        conn: &Connection,
        qh: &QueueHandle<D>,
    ) {
        let id = proxy.id().protocol_id();
        let mut gone = false;

        if let Ok(mut inner) = self.0.lock() {
            match event {
                window::Event::TitleChanged { title } => inner.title = title,
                // `app_id` here is the desktop entry name where KWin knows it,
                // which is exactly what the launcher index matches on.
                window::Event::AppIdChanged { app_id } => inner.app_id = app_id,
                window::Event::StateChanged { flags } => inner.state = flags,
                window::Event::ParentWindow { parent } => {
                    inner.parent_uuid = parent.and_then(|p| {
                        p.data::<PlasmaWindowData>()
                            .and_then(|d| d.0.lock().ok().map(|i| i.uuid.clone()))
                    });
                }
                window::Event::InitialState => inner.ready = true,
                window::Event::Unmapped => {
                    inner.unmapped = true;
                    gone = true;
                }
                _ => return,
            }
        }

        if gone {
            let plasma = state.plasma_state();
            if let Some(handle) = plasma.handles.remove(&id) {
                handle.destroy();
            }
        }

        state.plasma_state().refresh();
        state.plasma_windows_changed(conn, qh);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The state bitfield is the whole reason this backend can tell an active
    /// window from a minimised one, and the constants are easy to transpose.
    #[test]
    fn state_bits_match_the_protocol() {
        assert_eq!(state::ACTIVE, 0x1);
        assert_eq!(state::MINIMIZED, 0x2);
        assert_eq!(state::SKIPTASKBAR, 0x1000);
    }

    #[test]
    fn state_bits_are_independent() {
        let flags = state::ACTIVE | state::SKIPTASKBAR;
        assert!(flags & state::ACTIVE != 0);
        assert!(flags & state::SKIPTASKBAR != 0);
        assert!(flags & state::MINIMIZED == 0);
    }
}
