//! The `wlr-layer-shell` surface the dock lives on.

use smithay_client_toolkit::{
    compositor::{CompositorState, Region},
    shell::{
        wlr_layer::{Anchor, KeyboardInteractivity, LayerSurface},
        WaylandSurface,
    },
    shm::{
        slot::{Buffer, SlotPool},
        Shm,
    },
};
use tiny_skia::Pixmap;
use wayland_client::protocol::wl_shm;

use crate::edge::Edge;

/// The dock's layer surface, plus the shm plumbing needed to present to it.
pub struct LayerDock {
    layer: LayerSurface,
    pool: SlotPool,
    /// Kept across frames so magnification does not allocate a fresh slot 60
    /// times a second.
    buffer: Option<Buffer>,
    buffer_size: (i32, i32),
    /// Logical size the compositor last configured us to.
    pub width: u32,
    pub height: u32,
}

impl LayerDock {
    /// Configures a freshly created layer surface and performs the initial
    /// empty commit.
    ///
    /// Takes an already-created `LayerSurface` rather than creating one: doing
    /// so would make this generic over the app state, dragging a pair of
    /// `Dispatch` bounds through every call site for no benefit. The caller
    /// creates the surface; all the policy still lives here.
    ///
    /// The surface spans the full length of the edge it sits on and is much
    /// deeper than the panel, so that magnification and the launch bounce
    /// never have to resize it -- a resize means a configure round-trip, which
    /// would show up as a hitch mid-animation. The surplus area is transparent
    /// and is masked out of the input region so it does not swallow clicks.
    pub fn new(
        layer: LayerSurface,
        shm: &Shm,
        edge: Edge,
        depth: u32,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::anchor(&layer, edge, depth);

        // The dock must never take keyboard focus, or clicking it would steal
        // input from the window the user is actually working in.
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);

        // A layer surface is only mapped after an initial commit with no
        // buffer attached; the compositor answers with the first configure.
        layer.commit();

        // Sized for the first frame; the pool grows itself when a larger slot
        // is requested.
        let pool = SlotPool::new(1920 * depth as usize * 4, shm)?;

        // The dimension the anchors leave to the compositor starts at zero and
        // is filled in by the first configure.
        let (width, height) = if edge.is_vertical() {
            (depth, 0)
        } else {
            (0, depth)
        };

        Ok(Self {
            layer,
            pool,
            buffer: None,
            buffer_size: (0, 0),
            width,
            height,
        })
    }

    /// Pins the surface to an edge, leaving the length along it to the
    /// compositor.
    ///
    /// Anchoring to three edges is what makes the compositor hand over the
    /// output's full length; a zero in `set_size` means "whatever the anchors
    /// imply", so only the depth is ours to state.
    fn anchor(layer: &LayerSurface, edge: Edge, depth: u32) {
        let (anchor, size) = match edge {
            Edge::Bottom => (Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT, (0, depth)),
            Edge::Top => (Anchor::TOP | Anchor::LEFT | Anchor::RIGHT, (0, depth)),
            Edge::Left => (Anchor::LEFT | Anchor::TOP | Anchor::BOTTOM, (depth, 0)),
            Edge::Right => (Anchor::RIGHT | Anchor::TOP | Anchor::BOTTOM, (depth, 0)),
        };
        layer.set_anchor(anchor);
        layer.set_size(size.0, size.1);
    }

    /// Moves the dock to another edge, or changes how deep it reaches.
    ///
    /// The compositor answers with a configure carrying the new size, which is
    /// where `width` and `height` catch up -- setting them here would describe
    /// a surface that does not exist yet.
    pub fn reanchor(&mut self, edge: Edge, depth: u32) {
        Self::anchor(&self.layer, edge, depth);
        if edge.is_vertical() {
            self.width = depth;
        } else {
            self.height = depth;
        }
    }

    pub fn layer(&self) -> &LayerSurface {
        &self.layer
    }

    /// Depth of the strut other windows must keep clear of.
    ///
    /// The surface reaches far deeper than this -- windows only need to avoid
    /// the panel at its resting size, not the magnification headroom past it,
    /// which is what macOS does too.
    pub fn set_exclusive_zone(&self, px: i32) {
        self.layer.set_exclusive_zone(px);
    }

    /// Restricts which part of the surface accepts pointer events.
    ///
    /// Without this the transparent headroom around and above the panel would
    /// swallow clicks meant for the windows underneath.
    pub fn set_input_region(&self, compositor: &CompositorState, rects: &[(i32, i32, i32, i32)]) {
        let Ok(region) = Region::new(compositor) else {
            return;
        };
        for &(x, y, w, h) in rects {
            region.add(x, y, w, h);
        }
        self.layer
            .wl_surface()
            .set_input_region(Some(region.wl_region()));
    }

    /// Uploads a pixmap and commits it.
    ///
    /// The pixmap is in buffer pixels; the caller is responsible for having
    /// declared the matching logical size through the viewport first.
    pub fn present(&mut self, pixmap: &Pixmap) -> Result<(), Box<dyn std::error::Error>> {
        let w = pixmap.width() as i32;
        let h = pixmap.height() as i32;
        let stride = w * 4;

        if self.buffer_size != (w, h) {
            self.buffer = None;
            self.buffer_size = (w, h);
        }

        // `canvas` yields None while the compositor still holds the buffer, in
        // which case a second one gets allocated -- ordinary double buffering.
        let reusable = match &self.buffer {
            Some(buffer) => self.pool.canvas(buffer).is_some(),
            None => false,
        };

        let (buffer, canvas) = if reusable {
            let buffer = self.buffer.take().expect("checked just above");
            let canvas = self.pool.canvas(&buffer).expect("checked just above");
            (buffer, canvas)
        } else {
            self.pool
                .create_buffer(w, h, stride, wl_shm::Format::Argb8888)?
        };

        copy_to_argb8888(pixmap.data(), canvas);
        self.buffer = Some(buffer);

        let surface = self.layer.wl_surface();
        surface.damage_buffer(0, 0, w, h);
        self.buffer
            .as_ref()
            .expect("just assigned")
            .attach_to(surface)?;
        self.layer.commit();

        Ok(())
    }
}

/// tiny-skia stores premultiplied RGBA bytes; `wl_shm`'s `Argb8888` is a
/// native-endian `0xAARRGGBB` word. Same premultiplied channel values, so this
/// is purely a repack -- on little-endian it amounts to swapping R and B.
pub fn copy_to_argb8888(src: &[u8], dst: &mut [u8]) {
    for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
        let argb =
            ((s[3] as u32) << 24) | ((s[0] as u32) << 16) | ((s[1] as u32) << 8) | (s[2] as u32);
        d.copy_from_slice(&argb.to_ne_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Getting this repack wrong is invisible in code review and produces
    /// swapped-channel output at runtime, so pin it down.
    #[test]
    fn rgba_repacks_to_native_endian_argb() {
        // Opaque pure red in tiny-skia's RGBA order.
        let src = [0xFF, 0x00, 0x00, 0xFF];
        let mut dst = [0u8; 4];
        copy_to_argb8888(&src, &mut dst);

        assert_eq!(u32::from_ne_bytes(dst), 0xFFFF_0000);
    }

    #[test]
    fn repack_preserves_alpha_and_all_channels() {
        // r=0x10 g=0x20 b=0x30 a=0x40
        let src = [0x10, 0x20, 0x30, 0x40];
        let mut dst = [0u8; 4];
        copy_to_argb8888(&src, &mut dst);

        assert_eq!(u32::from_ne_bytes(dst), 0x4010_2030);
    }
}
