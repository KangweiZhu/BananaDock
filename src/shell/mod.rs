//! The dock's Wayland surface.
//!
//! Positioning a surface at a screen edge and reserving space for it is only
//! possible through `wlr-layer-shell`; there is no portable alternative, which
//! is what bounds the set of supported compositors (KWin, Hyprland, niri, sway,
//! Wayfire, river, cosmic -- but not GNOME, whose Mutter does not implement it).
//!
//! Only one implementation exists today, so this module deliberately exposes a
//! concrete type rather than a trait: the second backend would have to thread a
//! `QueueHandle<D>` through every method, and guessing that shape before there
//! is a second caller would produce the wrong abstraction. The seam that
//! matters for portability is `crate::windows`, where the protocol genuinely
//! varies.

pub mod layer;
pub mod popup;
pub mod scale;

pub use layer::LayerDock;
pub use popup::{MenuPopup, PopupShell};
pub use scale::{ScaleHandler, SurfaceScale};
