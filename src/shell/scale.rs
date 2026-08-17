//! Output scaling for the dock's surface.
//!
//! Qt handled this invisibly; here it has to be done by hand. Two protocols are
//! involved: `wp-fractional-scale-v1` reports the scale the output actually
//! wants (in 120ths, so 1.5 arrives as 180), and `wp-viewporter` declares what
//! logical size the buffer stands for. Together they let the dock rasterise at
//! the display's real resolution while every coordinate it reasons about stays
//! logical.
//!
//! Without fractional-scale the surface falls back to `wl_surface.set_buffer_scale`,
//! which only does whole numbers. The two mechanisms are mutually exclusive:
//! setting a buffer scale while a viewport destination is in force is a
//! protocol error, so only one path is ever active.

use smithay_client_toolkit::dispatch2::Dispatch2;
use wayland_client::{
    globals::GlobalList, protocol::wl_surface, Connection, Dispatch, QueueHandle,
};
use wayland_protocols::wp::{
    fractional_scale::v1::client::{wp_fractional_scale_manager_v1, wp_fractional_scale_v1},
    viewporter::client::{wp_viewport, wp_viewporter},
};

/// The protocol reports scale in 120ths of a unit.
const SCALE_DENOMINATOR: f32 = 120.0;

#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
pub struct ScaleData;

/// Tracks and applies the surface's scale.
pub struct SurfaceScale {
    viewport: Option<wp_viewport::WpViewport>,
    _fractional: Option<wp_fractional_scale_v1::WpFractionalScaleV1>,
    scale: f32,
}

impl SurfaceScale {
    /// Sets up fractional scaling for `surface` when the compositor offers it.
    pub fn new<D>(
        globals: &GlobalList,
        qh: &QueueHandle<D>,
        surface: &wl_surface::WlSurface,
    ) -> Self
    where
        D: Dispatch<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, ScaleData>
            + Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, ScaleData>
            + Dispatch<wp_viewporter::WpViewporter, ScaleData>
            + Dispatch<wp_viewport::WpViewport, ScaleData>
            + 'static,
    {
        let viewport = globals
            .bind::<wp_viewporter::WpViewporter, _, _>(qh, 1..=1, ScaleData)
            .ok()
            .map(|viewporter| viewporter.get_viewport(surface, qh, ScaleData));

        // A viewport is what makes a fractional buffer size meaningful, so
        // fractional scaling is only worth asking for if we got one.
        let fractional = viewport.as_ref().and_then(|_| {
            globals
                .bind::<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, _, _>(
                    qh,
                    1..=1,
                    ScaleData,
                )
                .ok()
                .map(|mgr| mgr.get_fractional_scale(surface, qh, ScaleData))
        });

        Self {
            viewport,
            _fractional: fractional,
            scale: 1.0,
        }
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Whether the compositor is driving the scale through fractional-scale.
    pub fn is_fractional(&self) -> bool {
        self._fractional.is_some()
    }

    /// Returns true when the scale actually moved.
    pub fn set_scale(&mut self, scale: f32) -> bool {
        let scale = scale.clamp(0.25, 8.0);
        if (scale - self.scale).abs() < f32::EPSILON {
            return false;
        }
        self.scale = scale;
        true
    }

    /// Declares what logical size the next buffer stands for.
    pub fn set_logical_size(&self, width: u32, height: u32) {
        if let Some(viewport) = &self.viewport {
            viewport.set_destination(width as i32, height as i32);
        }
    }

    /// Buffer dimensions for a logical size at the current scale.
    pub fn buffer_size(&self, width: u32, height: u32) -> (u32, u32) {
        (
            ((width as f32 * self.scale).round() as u32).max(1),
            ((height as f32 * self.scale).round() as u32).max(1),
        )
    }
}

/// Implemented by the application to learn about scale changes.
pub trait ScaleHandler: Sized {
    fn scale_changed(&mut self, conn: &Connection, qh: &QueueHandle<Self>, scale: f32);
}

impl<D: ScaleHandler> Dispatch2<wp_fractional_scale_v1::WpFractionalScaleV1, D> for ScaleData {
    fn event(
        &self,
        state: &mut D,
        _: &wp_fractional_scale_v1::WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        conn: &Connection,
        qh: &QueueHandle<D>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            state.scale_changed(conn, qh, scale as f32 / SCALE_DENOMINATOR);
        }
    }
}

// The remaining three interfaces are event-less; they still need a Dispatch so
// the objects can be created.
impl<D> Dispatch2<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, D> for ScaleData {
    fn event(
        &self,
        _: &mut D,
        _: &wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        _: wp_fractional_scale_manager_v1::Event,
        _: &Connection,
        _: &QueueHandle<D>,
    ) {
    }
}

impl<D> Dispatch2<wp_viewporter::WpViewporter, D> for ScaleData {
    fn event(
        &self,
        _: &mut D,
        _: &wp_viewporter::WpViewporter,
        _: wp_viewporter::Event,
        _: &Connection,
        _: &QueueHandle<D>,
    ) {
    }
}

impl<D> Dispatch2<wp_viewport::WpViewport, D> for ScaleData {
    fn event(
        &self,
        _: &mut D,
        _: &wp_viewport::WpViewport,
        _: wp_viewport::Event,
        _: &Connection,
        _: &QueueHandle<D>,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bare(scale: f32) -> SurfaceScale {
        SurfaceScale {
            viewport: None,
            _fractional: None,
            scale,
        }
    }

    #[test]
    fn buffer_size_follows_the_scale() {
        assert_eq!(bare(1.0).buffer_size(100, 50), (100, 50));
        assert_eq!(bare(2.0).buffer_size(100, 50), (200, 100));
        // 1.5x rounds rather than truncating, so a 145pt surface is 218px, not 217.
        assert_eq!(bare(1.5).buffer_size(100, 145), (150, 218));
    }

    #[test]
    fn a_zero_logical_size_still_yields_a_usable_buffer() {
        assert_eq!(bare(1.0).buffer_size(0, 0), (1, 1));
    }

    #[test]
    fn setting_the_same_scale_reports_no_change() {
        let mut s = bare(1.0);
        assert!(!s.set_scale(1.0));
        assert!(s.set_scale(1.5));
        assert!(!s.set_scale(1.5));
    }

    /// A compositor sending nonsense must not make the dock allocate a buffer
    /// of absurd size or of zero size.
    #[test]
    fn absurd_scales_are_clamped() {
        let mut s = bare(1.0);
        s.set_scale(0.0);
        assert!(s.scale() >= 0.25);
        s.set_scale(1000.0);
        assert!(s.scale() <= 8.0);
    }
}
