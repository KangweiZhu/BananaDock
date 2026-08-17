//! The settings window itself: an ordinary xdg-shell window, drawn by hand.

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_registry,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        xdg::{
            window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
            XdgShell,
        },
        WaylandSurface,
    },
    shm::{
        slot::{Buffer, SlotPool},
        Shm, ShmHandler,
    },
};
use std::time::{Duration, Instant};

use tiny_skia::Pixmap;
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, QueueHandle,
};

use super::{
    paint,
    ui::{self, Control, Hit},
};
use crate::{config::Config, text::TextRenderer};

/// Shortest gap between saves while a slider is being dragged. Fast enough to
/// read as continuous, slow enough not to rewrite the file on every motion
/// event.
const WRITE_INTERVAL: Duration = Duration::from_millis(40);

/// Opens the settings window and runs until it is closed.
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let conn = Connection::connect_to_env()?;
    let (globals, mut event_queue) = registry_queue_init(&conn)?;
    let qh = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh)?;
    let xdg_shell = XdgShell::bind(&globals, &qh)?;
    let shm = Shm::bind(&globals, &qh)?;

    let path = Config::path().ok_or("no writable configuration directory")?;
    let config = Config::load(&path);
    let controls = ui::controls(&config);
    let (_, height) = ui::rows(&controls);

    let surface = compositor.create_surface(&qh);
    let window = xdg_shell.create_window(surface, WindowDecorations::RequestServer, &qh);
    window.set_title("kdock settings");
    // Matches the desktop entry, so the compositor can pair the window with it.
    window.set_app_id("kdock");
    let size = (ui::WINDOW_WIDTH as u32, height as u32);
    window.set_min_size(Some(size));
    window.set_max_size(Some(size));
    window.commit();

    let pool = SlotPool::new((size.0 * size.1 * 4) as usize, &shm)?;

    let mut app = Settings {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        seat_state: SeatState::new(&globals, &qh),
        shm,
        pool,
        buffer: None,
        window,
        pointer: None,
        text: TextRenderer::new(),
        path,
        config,
        controls,
        width: size.0,
        height: size.1,
        dragging: None,
        last_write: Instant::now() - WRITE_INTERVAL,
        hovered: None,
        exit: false,
    };

    while !app.exit {
        event_queue.blocking_dispatch(&mut app)?;
    }
    Ok(())
}

struct Settings {
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    shm: Shm,
    pool: SlotPool,
    buffer: Option<Buffer>,
    window: Window,
    pointer: Option<wl_pointer::WlPointer>,
    text: TextRenderer,

    path: std::path::PathBuf,
    config: Config,
    controls: Vec<Control>,
    width: u32,
    height: u32,
    /// Index of the slider being dragged, if any.
    dragging: Option<usize>,
    /// When the config file was last written, for rate-limiting a drag.
    last_write: Instant,
    hovered: Option<usize>,
    exit: bool,
}

impl Settings {
    /// Writes one setting and rebuilds the controls from what was written.
    ///
    /// The dock is watching the file, so saving *is* applying -- there is
    /// nothing to send it.
    fn commit_change(&mut self, key: &'static str, value: toml_edit::Value) {
        if let Err(e) = Config::save_settings(&self.path, &[(key, value)]) {
            eprintln!("kdock: could not save {}: {e}", self.path.display());
            return;
        }
        self.config = Config::load(&self.path);
        self.controls = ui::controls(&self.config);
    }

    /// Writes a slider's final value when the drag ends.
    ///
    /// The rate limit means the last motion before release may not have been
    /// saved, so this is not redundant with the writes during the drag.
    fn save_slider(&mut self, index: usize) {
        let Some(Control::Slider { key, value, .. }) = self.controls.get(index).cloned() else {
            return;
        };
        self.last_write = Instant::now();
        self.commit_change(key, (value as f64).into());
    }

    fn apply(&mut self, hit: Hit) {
        match hit {
            Hit::Toggle(i) => {
                let Some(Control::Toggle { key, value, .. }) = self.controls.get(i).cloned() else {
                    return;
                };
                self.commit_change(key, (!value).into());
            }
            Hit::Slider(i, raw) => {
                let Some(Control::Slider {
                    key, unit, value, ..
                }) = self.controls.get(i).cloned()
                else {
                    return;
                };
                let next = ui::quantise(raw, unit);
                if (next - value).abs() < f32::EPSILON {
                    return;
                }
                if let Some(Control::Slider { value, .. }) = self.controls.get_mut(i) {
                    *value = next;
                }

                // Written as the pointer moves, so the dock changes under the
                // hand rather than jumping when the drag ends. Rate-limited
                // because each write is a file rewrite the dock reloads: a
                // pointer can produce far more motion events per second than
                // anyone can see.
                let now = Instant::now();
                if now.duration_since(self.last_write) >= WRITE_INTERVAL {
                    self.last_write = now;
                    self.commit_change(key, (next as f64).into());
                }
            }
        }
    }

    fn draw(&mut self, qh: &QueueHandle<Self>) {
        let (w, h) = (self.width, self.height);
        if w == 0 || h == 0 {
            return;
        }
        let Some(mut pixmap) = Pixmap::new(w, h) else {
            return;
        };

        paint::draw(
            &mut pixmap,
            &mut self.text,
            &self.controls,
            w as f32,
            self.hovered,
        );

        let stride = w as i32 * 4;

        // `canvas` yields None while the compositor still holds the buffer, so
        // a second one gets allocated -- ordinary double buffering. Skipping
        // the frame instead leaves the window frozen for as long as the
        // compositor keeps hold, which during a drag is most of the time.
        let reusable = match &self.buffer {
            Some(buffer) => self.pool.canvas(buffer).is_some(),
            None => false,
        };
        let (buffer, canvas) = if reusable {
            let buffer = self.buffer.take().expect("checked just above");
            let canvas = self.pool.canvas(&buffer).expect("checked just above");
            (buffer, canvas)
        } else {
            match self
                .pool
                .create_buffer(w as i32, h as i32, stride, wl_shm::Format::Argb8888)
            {
                Ok(pair) => pair,
                Err(e) => {
                    eprintln!("kdock: could not allocate a buffer: {e}");
                    return;
                }
            }
        };
        crate::shell::layer::copy_to_argb8888(pixmap.data(), canvas);
        self.buffer = Some(buffer);

        let surface = self.window.wl_surface();
        surface.damage_buffer(0, 0, w as i32, h as i32);
        let _ = self
            .buffer
            .as_ref()
            .expect("just assigned")
            .attach_to(surface);
        self.window.commit();
        let _ = qh;
    }
}

impl WindowHandler for Settings {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        self.exit = true;
    }

    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &Window,
        configure: WindowConfigure,
        _serial: u32,
    ) {
        // The window is fixed-size, so a compositor suggestion of zero means
        // "you choose" -- and anything else is still bounded by min == max.
        if let (Some(w), Some(h)) = configure.new_size {
            self.width = w.get();
            self.height = h.get();
            self.buffer = None;
        }
        self.draw(qh);
    }
}

impl PointerHandler for Settings {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        let mut dirty = false;
        for event in events {
            if event.surface != *self.window.wl_surface() {
                continue;
            }
            let (x, y) = (event.position.0 as f32, event.position.1 as f32);
            let hit = ui::hit(&self.controls, self.width as f32, x, y);

            match event.kind {
                PointerEventKind::Motion { .. } => {
                    let hovered = hit.as_ref().map(|h| match h {
                        Hit::Toggle(i) | Hit::Slider(i, _) => *i,
                    });
                    if hovered != self.hovered {
                        self.hovered = hovered;
                        dirty = true;
                    }
                    // A slider keeps following the pointer once grabbed, even
                    // past the ends of its track.
                    if let Some(i) = self.dragging {
                        if let Some(Hit::Slider(_, v)) = ui::hit(
                            &self.controls,
                            self.width as f32,
                            x,
                            slider_row_centre(&self.controls, i),
                        ) {
                            self.apply(Hit::Slider(i, v));
                            dirty = true;
                        }
                    }
                }
                PointerEventKind::Leave { .. } => {
                    self.hovered = None;
                    dirty = true;
                }
                PointerEventKind::Press { button: 0x110, .. } => {
                    if let Some(hit) = hit {
                        if let Hit::Slider(i, _) = hit {
                            self.dragging = Some(i);
                        }
                        self.apply(hit);
                        dirty = true;
                    }
                }
                PointerEventKind::Release { button: 0x110, .. } => {
                    if let Some(i) = self.dragging.take() {
                        self.save_slider(i);
                        dirty = true;
                    }
                }
                _ => {}
            }
        }
        if dirty {
            self.draw(qh);
        }
    }
}

/// Vertical centre of a control's row, for continuing a drag that has wandered
/// off the row.
fn slider_row_centre(controls: &[Control], index: usize) -> f32 {
    let (tops, _) = ui::rows(controls);
    tops.get(index).map_or(0.0, |t| t + ui::ROW_HEIGHT / 2.0)
}

impl CompositorHandler for Settings {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl SeatHandler for Settings {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            self.pointer = self.seat_state.get_pointer(qh, &seat).ok();
        }
    }
    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            if let Some(p) = self.pointer.take() {
                p.release();
            }
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl OutputHandler for Settings {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ShmHandler for Settings {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

delegate_registry!(Settings);

impl ProvidesRegistryState for Settings {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

smithay_client_toolkit::delegate_dispatch2!(Settings);
