//! The context menu's surface.
//!
//! A layer surface cannot simply draw a menu inside itself: the dock's surface
//! is only as tall as the panel plus magnification headroom, and a menu opens
//! *upwards* past that edge, so it would be clipped away. It has to be a real
//! popup.
//!
//! `xdg_popup` is the right primitive rather than a second layer surface
//! because the grab comes with it: the compositor routes input to the popup and
//! dismisses it when the user clicks elsewhere, which would otherwise have to
//! be reimplemented -- badly -- by watching for clicks outside a rectangle.
//!
//! Popups parented to a layer surface are created with a null xdg parent and
//! then adopted via `zwlr_layer_surface_v1.get_popup` before the first commit.

use smithay_client_toolkit::{
    compositor::CompositorState,
    error::GlobalError,
    globals::{GlobalData, ProvidesBoundGlobal},
    shell::{
        wlr_layer::LayerSurface,
        xdg::{popup::Popup, XdgPositioner},
    },
    shm::{
        slot::{Buffer, SlotPool},
        Shm,
    },
};
use tiny_skia::Pixmap;
use wayland_client::{
    globals::{BindError, GlobalList},
    protocol::{wl_seat, wl_shm},
    Dispatch, QueueHandle,
};
use wayland_protocols::xdg::shell::client::{
    xdg_positioner::{Anchor, ConstraintAdjustment, Gravity},
    xdg_wm_base,
};

/// The version of `xdg_wm_base` SCTK's popup helpers are written against.
const XDG_WM_BASE_VERSION: u32 = 6;

/// The `xdg_wm_base` binding, held only so popups can be created.
///
/// SCTK's `XdgShell` would bind this too, but it also pulls in the toplevel
/// window and decoration machinery and demands a `WindowHandler` impl. The dock
/// never creates a toplevel window, so implementing that trait would be dead
/// code standing in for a capability the program does not have.
pub struct PopupShell {
    wm_base: xdg_wm_base::XdgWmBase,
}

impl PopupShell {
    pub fn bind<D>(globals: &GlobalList, qh: &QueueHandle<D>) -> Result<Self, BindError>
    where
        D: Dispatch<xdg_wm_base::XdgWmBase, GlobalData> + 'static,
    {
        Ok(Self {
            wm_base: globals.bind(qh, 1..=XDG_WM_BASE_VERSION, GlobalData)?,
        })
    }
}

// SCTK's helpers ask for different compat versions -- the positioner wants 6,
// the popup constructor 5 -- so the binding advertises itself at both. The
// proxy is the same either way; the constant only states the minimum the caller
// relies on.
impl ProvidesBoundGlobal<xdg_wm_base::XdgWmBase, XDG_WM_BASE_VERSION> for PopupShell {
    fn bound_global(&self) -> Result<xdg_wm_base::XdgWmBase, GlobalError> {
        Ok(self.wm_base.clone())
    }
}

impl ProvidesBoundGlobal<xdg_wm_base::XdgWmBase, 5> for PopupShell {
    fn bound_global(&self) -> Result<xdg_wm_base::XdgWmBase, GlobalError> {
        Ok(self.wm_base.clone())
    }
}

/// A menu popup, and the shm plumbing to present to it.
pub struct MenuPopup {
    popup: Popup,
    pool: SlotPool,
    buffer: Option<Buffer>,
    buffer_size: (i32, i32),
    /// Logical size the menu was created at.
    pub width: u32,
    pub height: u32,
    /// Which slot the menu belongs to, so its actions can be applied.
    pub slot_index: usize,
    /// Row under the pointer, if any.
    pub highlighted: Option<usize>,
    /// Set once the compositor has configured it; drawing earlier is a
    /// protocol error.
    pub configured: bool,
}

impl MenuPopup {
    /// Opens a menu of `size` anchored to `anchor` -- the icon's rectangle in
    /// the dock surface's coordinates.
    #[allow(clippy::too_many_arguments)]
    pub fn open<D>(
        layer: &LayerSurface,
        xdg_shell: &PopupShell,
        compositor: &CompositorState,
        shm: &Shm,
        qh: &QueueHandle<D>,
        anchor: (i32, i32, i32, i32),
        size: (u32, u32),
        slot_index: usize,
        grab: Option<(&wl_seat::WlSeat, u32)>,
    ) -> Result<Self, Box<dyn std::error::Error>>
    where
        D: wayland_client::Dispatch<
                wayland_client::protocol::wl_surface::WlSurface,
                smithay_client_toolkit::compositor::SurfaceData<()>,
            > + wayland_client::Dispatch<
                wayland_protocols::xdg::shell::client::xdg_surface::XdgSurface,
                smithay_client_toolkit::shell::xdg::popup::PopupData,
            > + wayland_client::Dispatch<
                wayland_protocols::xdg::shell::client::xdg_popup::XdgPopup,
                smithay_client_toolkit::shell::xdg::popup::PopupData,
            > + 'static,
    {
        let positioner = XdgPositioner::new(xdg_shell)?;
        positioner.set_size(size.0 as i32, size.1 as i32);
        positioner.set_anchor_rect(anchor.0, anchor.1, anchor.2.max(1), anchor.3.max(1));
        // Anchor to the top of the icon and grow upwards: the dock sits at the
        // bottom of the screen, so there is no room below it.
        positioner.set_anchor(Anchor::Top);
        positioner.set_gravity(Gravity::Top);
        // Near the screen edges, slide the menu along rather than flipping it
        // downwards off the bottom of the display.
        positioner
            .set_constraint_adjustment(ConstraintAdjustment::SlideX | ConstraintAdjustment::FlipY);

        let surface = compositor.create_surface(qh);
        let popup = Popup::from_surface(None, &positioner, qh, surface, xdg_shell)?;
        // Adopt it before the first commit, or the compositor raises
        // `invalid_popup_parent`.
        layer.get_popup(popup.xdg_popup());

        // The grab must be taken before the surface is mapped, and it is what
        // makes a click anywhere else dismiss the menu. It has to cite the
        // press that opened it; compositors reject a grab justified by a stale
        // event.
        if let Some((seat, serial)) = grab {
            popup.xdg_popup().grab(seat, serial);
        }

        popup.wl_surface().commit();

        let pool = SlotPool::new((size.0 * size.1).max(1) as usize * 4, shm)?;

        Ok(Self {
            popup,
            pool,
            buffer: None,
            buffer_size: (0, 0),
            width: size.0,
            height: size.1,
            slot_index,
            highlighted: None,
            configured: false,
        })
    }

    pub fn wl_surface(&self) -> &wayland_client::protocol::wl_surface::WlSurface {
        self.popup.wl_surface()
    }

    pub fn present(&mut self, pixmap: &Pixmap) -> Result<(), Box<dyn std::error::Error>> {
        let w = pixmap.width() as i32;
        let h = pixmap.height() as i32;
        let stride = w * 4;

        if self.buffer_size != (w, h) {
            self.buffer = None;
            self.buffer_size = (w, h);
        }

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

        super::layer::copy_to_argb8888(pixmap.data(), canvas);
        self.buffer = Some(buffer);

        let surface = self.popup.wl_surface();
        surface.damage_buffer(0, 0, w, h);
        self.buffer
            .as_ref()
            .expect("just assigned")
            .attach_to(surface)?;
        surface.commit();

        Ok(())
    }
}
