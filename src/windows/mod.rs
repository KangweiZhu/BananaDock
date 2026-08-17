//! Where the dock's list of open windows comes from.
//!
//! This is the seam that decides which compositors the dock can work on, so it
//! is deliberately a trait rather than a single concrete type.
//!
//! Only `wlr-foreign-toplevel-management-v1` can actually drive a dock today:
//! the cross-compositor `ext-foreign-toplevel-list-v1` enumerates toplevels but
//! has no requests beyond `stop` and `destroy` -- no activate, no minimise, no
//! close -- so clicking an icon could not do anything. `xdg-activation-v1` does
//! not close that gap either, since activating a surface requires holding it,
//! and a foreign window's surface is not ours to hold.

pub mod kwin;
pub mod plasma;
pub mod wlr;

pub use kwin::KwinWindows;
pub use plasma::PlasmaWindows;
pub use wlr::ForeignToplevelManager;

/// Identifies a toplevel for the lifetime of its protocol object.
pub type ToplevelId = u32;

/// A window, reduced to what the dock needs to draw and act on it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Toplevel {
    pub id: ToplevelId,
    /// Used to match the window to a `.desktop` entry. May be empty.
    pub app_id: String,
    pub title: String,
    pub active: bool,
    pub minimized: bool,
    /// Set for dialogs and other windows owned by another toplevel.
    pub parent: Option<ToplevelId>,
}

/// What a given window source is actually able to do.
///
/// Not every backend can do everything: KWin exposes no route to minimise or
/// close a specific window, and a menu entry that silently does nothing is
/// worse than one that is not offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub minimize: bool,
    pub close: bool,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            minimize: true,
            close: true,
        }
    }
}

/// The operations a dock needs from whatever is tracking windows.
pub trait WindowSource {
    fn toplevels(&self) -> &[Toplevel];

    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }

    fn activate(&self, id: ToplevelId);
    fn set_minimized(&self, id: ToplevelId, minimized: bool);
    fn close(&self, id: ToplevelId);

    /// Tells the compositor which part of the dock represents this window, so
    /// its minimise animation flies to the right icon.
    fn set_icon_rect(
        &self,
        id: ToplevelId,
        surface: &wayland_client::protocol::wl_surface::WlSurface,
        rect: (i32, i32, i32, i32),
    );
}
