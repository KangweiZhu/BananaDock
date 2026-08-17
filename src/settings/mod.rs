//! The settings window.
//!
//! A separate process (`kdock --settings`) rather than a panel inside the dock:
//! the dock is a layer surface with no keyboard focus and a shape built around
//! a row of icons, and bolting a settings form onto it would compromise both.
//!
//! Changes are written straight to `config.toml`. The dock already watches that
//! file and re-reads it on every change, so there is nothing to notify and no
//! protocol between the two -- the file *is* the interface.

pub mod paint;
pub mod ui;
pub mod window;

pub use window::run;
