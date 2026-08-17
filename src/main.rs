//! A macOS-style dock for Wayland compositors that implement `wlr-layer-shell`.

mod config;
mod drops;
mod icons;
mod launchers;
mod layout;
mod menu;
mod metrics;
mod model;
mod render;
mod settings;
mod shell;
mod text;
mod thumbnails;
mod trash;
mod windows;

use std::{collections::HashMap, num::NonZeroU32, path::Path, time::Instant};

use smithay_client_toolkit::{
    activation::{ActivationHandler, ActivationState, RequestData},
    background_effect::{BackgroundEffectHandler, BackgroundEffectState},
    compositor::{CompositorHandler, CompositorState, FrameCallbackData, Region},
    data_device_manager::{
        data_device::{DataDevice, DataDeviceHandler},
        data_offer::{DataOfferHandler, DragOffer},
        DataDeviceManagerState,
    },
    delegate_registry,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{channel, EventLoop, LoopHandle, PostAction},
        calloop_wayland_source::WaylandSource,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{
            KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        xdg::popup::{Popup, PopupConfigure, PopupHandler},
        WaylandSurface,
    },
    shm::{Shm, ShmHandler},
};
use tiny_skia::Pixmap;
use wayland_client::{
    globals::registry_queue_init,
    protocol::wl_data_device_manager::DndAction,
    protocol::{wl_output, wl_pointer, wl_seat, wl_surface},
    Connection, QueueHandle,
};

use config::Config;
use icons::IconCache;
use launchers::LauncherIndex;
use layout::{Layout, SlotMetrics};
use menu::{MenuAction, MenuItem, MenuLayout};
use metrics::{Metrics, Palette};
use model::{Slot, SlotKind};
use shell::{LayerDock, MenuPopup, PopupShell, ScaleHandler, SurfaceScale};
use text::TextRenderer;
use windows::{wlr::ForeignToplevelHandler, ForeignToplevelManager, KwinWindows, WindowSource};

/// Horizontal slices per rounded end of the blur region. At the panel's usual
/// height each slice is a couple of pixels tall, which is below the point where
/// the stepping is visible against a blurred background.
const BLUR_CAP_SLICES: u32 = 16;

/// Height of the strip along the screen edge that brings a hidden dock back.
///
/// Deliberately spans only the panel's own width rather than the whole edge:
/// a full-width trigger would swallow clicks aimed at the bottom of every
/// window, which is a steep price for a dock that sits in the middle anyway.
const TRIGGER_STRIP_PX: f32 = 2.0;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Rendering a frame offscreen needs no compositor, which makes it the way
    // to check pixels against a reference screenshot during calibration -- and
    // the way to see the panel at all when a fullscreen window covers it.
    let mut args = std::env::args().skip(1);
    if let Some(flag) = args.next() {
        if flag == "--settings" {
            return settings::run();
        }
        if flag == "--dump-frame" {
            let path = args.next().ok_or("--dump-frame needs an output path")?;
            let width: u32 = args.next().map_or(Ok(1920), |v| v.parse())?;
            let ids: Vec<String> = args.collect();
            return dump_frame(&path, width, &ids);
        }
        if flag == "--dump-menu" {
            let path = args.next().ok_or("--dump-menu needs an output path")?;
            return dump_menu(&path);
        }
        return Err(format!("unknown argument: {flag}").into());
    }

    // Set up before anything spawns a thread. `Signals` masks the signals for
    // the calling thread only, and threads inherit the mask at creation -- so
    // installing this after the D-Bus connection starts its workers would leave
    // those threads unmasked, and the kernel would deliver SIGTERM to one of
    // them and take the default action instead.
    let signals = calloop::signals::Signals::new(&[
        calloop::signals::Signal::SIGTERM,
        calloop::signals::Signal::SIGINT,
    ]);

    let conn = Connection::connect_to_env()?;
    let (globals, event_queue) = registry_queue_init(&conn)?;
    let qh = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh)?;
    let layer_shell = LayerShell::bind(&globals, &qh).map_err(|e| {
        format!(
            "this compositor does not implement wlr-layer-shell, which the dock cannot work \
             without ({e}). GNOME/Mutter is known not to implement it."
        )
    })?;
    let shm = Shm::bind(&globals, &qh)?;

    let trash_dir = trash::trash_dir();
    let config_path = Config::path();
    let config = config_path.as_deref().map(Config::load).unwrap_or_default();
    let mut metrics = Metrics::default();
    config.apply_to(&mut metrics);
    let surface_height = metrics.surface_height().ceil() as u32;

    let output_state = OutputState::new(&globals, &qh);
    let surface = compositor.create_surface(&qh);
    let scale = SurfaceScale::new(&globals, &qh, &surface);
    if !scale.is_fractional() {
        eprintln!("kdock: no wp-fractional-scale-v1; falling back to whole-number output scaling.");
    }
    let background_effect = BackgroundEffectState::new(&globals, &qh);
    let blur_surface = background_effect.get_background_effect(&surface, &qh).ok();
    if blur_surface.is_none() {
        eprintln!(
            "kdock: no ext-background-effect-v1; the panel will not be frosted. \
             Hyprland provides blur through its own `blurls` setting instead."
        );
    }

    // The compositor picks the screen when none is named, which is normally
    // the focused one. Naming an output that is not attached is not fatal: the
    // dock appears on the default screen and moves when the output shows up.
    let chosen = config
        .output
        .as_deref()
        .and_then(|name| find_output(&output_state, name));
    if config.output.is_some() && chosen.is_none() {
        eprintln!(
            "kdock: no output named {:?} is attached; using the compositor's choice for now.",
            config.output.as_deref().unwrap_or_default()
        );
    }
    let layer =
        layer_shell.create_layer_surface(&qh, surface, Layer::Top, Some("dock"), chosen.as_ref());
    let dock = LayerDock::new(layer, &shm, surface_height)?;

    let toplevels = ForeignToplevelManager::new(&globals, &qh);

    // The channel carries every out-of-band wake-up: config edits, Trash
    // changes, and -- on KWin -- window lists pushed in from the compositor.
    let (tx, rx) = channel::channel();

    // Where the portable Wayland route is missing, try KWin's own protocol
    // next. It is only granted when the dock's .desktop file names the
    // interface, so it can be absent even on Plasma.
    let plasma = {
        let p = windows::PlasmaWindows::new(&globals, &qh);
        p.is_available().then_some(p)
    };

    // Last resort on KWin: drive a KWin script over D-Bus. Polled rather than
    // event-driven, and it has to inject a script, so it is only used when the
    // native protocol was not granted.
    let mut kwin = None;
    if !toplevels.is_available() && plasma.is_none() {
        let window_tx = tx.clone();
        // Shared with the D-Bus thread, which hands the queue to the script.
        let commands: windows::kwin::Commands = Default::default();
        match windows::kwin::start(
            Box::new(move |snapshot| {
                let _ = window_tx.send(Watched::Windows(snapshot));
            }),
            commands.clone(),
        ) {
            Ok(conn) => kwin = Some(KwinWindows::new(Some(conn), commands)),
            Err(e) => eprintln!(
                "kdock: no window source available ({e}). On Plasma, install \
                 kdock.desktop with \
                 X-KDE-Wayland-Interfaces=org_kde_plasma_window_management \
                 and an Exec= naming this binary."
            ),
        }
    }

    // Created before the app: dropped files are read through a loop source, so
    // the app needs a handle to the loop it is about to run on.
    let mut event_loop: EventLoop<App> = EventLoop::try_new()?;
    let loop_handle = event_loop.handle();

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        output_state,
        seat_state: SeatState::new(&globals, &qh),
        shm,
        compositor,
        dock,
        scale,
        background_effect,
        blur_surface,
        toplevels,
        plasma,
        kwin,
        launchers: LauncherIndex::load(),
        icons: IconCache::new(config.icon_theme.clone()),
        // Capturing goes through KWin's screenshot interface, which rides the
        // same session bus the window fallback uses.
        thumbnails: thumbnails::ThumbnailCache::new(zbus::blocking::Connection::session().ok()),
        events: tx.clone(),
        visible_slots: Vec::new(),
        departing: Vec::new(),
        entering: Vec::new(),
        minimize_targets_for: (Vec::new(), 0),
        pinned: config.pinned.clone(),
        config,
        config_path,
        trash: trash_dir
            .as_deref()
            .map(trash::read)
            .unwrap_or(model::TrashState { full: false }),
        trash_dir,
        metrics,
        palette: Palette::default(),
        pointer_surface_x: None,
        current_widths: Vec::new(),
        last_frame_ms: None,
        frame_pending: false,
        pointer: None,
        seat: None,
        activation: ActivationState::bind(&globals, &qh).ok(),
        drag_candidate: None,
        drag: None,
        data_device_state: DataDeviceManagerState::bind(&globals, &qh).ok(),
        data_device: None,
        drop_target: None,
        drop_buffer: Vec::new(),
        loop_handle,
        xdg_shell: PopupShell::bind(&globals, &qh).ok(),
        open_menu: None,
        menu_items: Vec::new(),
        menu_layout: MenuLayout {
            width: 0.0,
            height: 0.0,
            rows: Vec::new(),
        },
        text: TextRenderer::new(),
        revealed: 1.0,
        // Startup counts as the pointer having been away all along, so an
        // auto-hiding dock shows itself once and then slides out, rather than
        // sitting there until the pointer has visited and left.
        left_at: Some(Instant::now()),
        launching: HashMap::new(),
        first_configure: true,
        exit: false,
    };

    // calloop rather than a bare dispatch loop: the dock waits on the config
    // file and on dropped-data pipes, not only on Wayland events.
    WaylandSource::new(conn.clone(), event_queue).insert(event_loop.handle())?;

    event_loop
        .handle()
        .insert_source(rx, |event, _, app: &mut App| {
            if let channel::Event::Msg(what) = event {
                match what {
                    Watched::Config => app.reload_config(),
                    Watched::Trash => app.reload_trash(),
                    Watched::Windows(snapshot) => app.apply_kwin_windows(&snapshot),
                    Watched::Thumbnail(uuid, thumb) => app.apply_thumbnail(uuid, thumb),
                }
            }
        })
        .map_err(|e| format!("could not watch for changes: {e}"))?;

    // Routed through the loop so the KWin script gets unloaded on the way out,
    // rather than being left pushing at a bus name that has gone away.
    match signals {
        Ok(signals) => {
            event_loop
                .handle()
                .insert_source(signals, |_, _, app: &mut App| {
                    app.exit = true;
                })
                .map_err(|e| format!("could not listen for signals: {e}"))?;
        }
        Err(e) => eprintln!("kdock: could not install signal handling: {e}"),
    }

    // Held for the lifetime of the loop: dropping a watcher stops its events.
    let _config_watcher = app
        .config_path
        .as_deref()
        .and_then(|p| watch_file(p, tx.clone(), Watched::Config));
    let _trash_watcher = app
        .trash_dir
        .as_deref()
        .and_then(|p| watch_dir(p, tx, Watched::Trash));

    while !app.exit {
        if let Err(e) = event_loop.dispatch(None, &mut app) {
            // The compositor going away is how a session ends, not a failure
            // worth a broken-pipe message on the way out.
            if is_disconnect(&e) {
                break;
            }
            return Err(e.into());
        }
    }

    // A script left loaded keeps pushing to a bus name that no longer answers.
    // A hard kill skips this, which is why startup unloads before it loads.
    if let Some(conn) = app.kwin.as_ref().and_then(KwinWindows::connection) {
        windows::kwin::unload_script(conn);
    }

    Ok(())
}

/// Whether a loop error just means the compositor closed the connection.
///
/// The Wayland source reports this through calloop's own error type, and the
/// underlying `io::Error` may be either wrapped directly or boxed by the
/// source, so both shapes are checked.
fn is_disconnect(e: &smithay_client_toolkit::reexports::calloop::Error) -> bool {
    use smithay_client_toolkit::reexports::calloop::Error;

    fn is_hangup(kind: std::io::ErrorKind) -> bool {
        matches!(
            kind,
            std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::UnexpectedEof
        )
    }

    match e {
        Error::IoError(io) => is_hangup(io.kind()),
        Error::OtherError(other) => {
            if let Some(io) = other.downcast_ref::<std::io::Error>() {
                return is_hangup(io.kind());
            }
            // The Wayland source boxes a calloop error of its own, so the
            // hang-up arrives as OtherError(IoError(..)) rather than as a bare
            // io::Error.
            if let Some(inner) = other.downcast_ref::<Error>() {
                return is_disconnect(inner);
            }
            false
        }
        _ => false,
    }
}

/// What a filesystem event was about.
#[derive(Debug, Clone)]
enum Watched {
    Config,
    Trash,
    /// A window-list snapshot pushed in by the KWin script.
    Windows(String),
    /// A window thumbnail finished capturing, or failed to.
    Thumbnail(String, Option<thumbnails::Thumbnail>),
}

/// Watches one file for changes.
///
/// The *parent directory* is watched, not the file: editors overwrite by
/// writing a temporary file and renaming it over the target, which destroys the
/// inode a file watch is attached to. Watching the directory survives that, and
/// is also how a config that did not exist yet gets noticed when it appears.
fn watch_file(
    path: &Path,
    tx: channel::Sender<Watched>,
    what: Watched,
) -> Option<notify::RecommendedWatcher> {
    use notify::Watcher;

    let dir = path.parent()?.to_path_buf();
    std::fs::create_dir_all(&dir).ok()?;
    let target = path.to_path_buf();

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else { return };
        if event.paths.contains(&target) {
            let _ = tx.send(what.clone());
        }
    })
    .ok()?;

    watcher
        .watch(&dir, notify::RecursiveMode::NonRecursive)
        .ok()?;
    Some(watcher)
}

/// Watches a directory for anything appearing in or leaving it.
fn watch_dir(
    dir: &Path,
    tx: channel::Sender<Watched>,
    what: Watched,
) -> Option<notify::RecommendedWatcher> {
    use notify::Watcher;

    // Not created here: an absent Trash directory simply means nothing has been
    // deleted yet, and making one would be a side effect nobody asked for.
    if !dir.is_dir() {
        return None;
    }

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = tx.send(what.clone());
        }
    })
    .ok()?;

    watcher
        .watch(dir, notify::RecursiveMode::NonRecursive)
        .ok()?;
    Some(watcher)
}

/// Renders one frame to a PNG without touching the compositor.
///
/// The panel is drawn over a flat mid-grey stand-in for the wallpaper: the tint
/// is only 10% white and the compositor's blur is absent here, so on a
/// transparent background the result would be all but invisible. The grey is
/// synthetic -- it is not part of the dock.
fn dump_frame(path: &str, width: u32, ids: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let metrics = Metrics::default();
    let palette = Palette::default();
    let height = metrics.surface_height().ceil() as u32;

    let index = LauncherIndex::load();
    let mut icons = IconCache::new(None);
    // `KDOCK_TRASH=full|empty` previews the Trash tile offline.
    let trash = match std::env::var("KDOCK_TRASH").ok().as_deref() {
        Some("full") => Some(model::TrashState { full: true }),
        Some("empty") => Some(model::TrashState { full: false }),
        _ => None,
    };
    let slots = model::build_slots(ids, &[], &index, trash);
    let tile = metrics.pt(metrics.tile_size);

    // `KDOCK_CURSOR` places the pointer, in resting-layout coordinates, so the
    // magnification curve can be compared against a reference screenshot
    // without a live pointer.
    let cursor = std::env::var("KDOCK_CURSOR")
        .ok()
        .and_then(|v| v.parse::<f32>().ok());
    let sep = metrics.pt(metrics.separator_width);
    let slot_metrics: Vec<SlotMetrics> = slots
        .iter()
        .map(|s| match s.kind {
            model::SlotKind::Separator => SlotMetrics {
                rest_width: sep,
                magnifies: false,
            },
            _ => SlotMetrics {
                rest_width: tile,
                magnifies: true,
            },
        })
        .collect();
    let geometry = layout::layout(&slot_metrics, cursor, &metrics);

    // `KDOCK_SCALE` renders at an output scale, for checking HiDPI output
    // without a HiDPI display.
    let scale = std::env::var("KDOCK_SCALE")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(1.0);
    let mut pixmap = Pixmap::new(
        (width as f32 * scale) as u32,
        (height as f32 * scale).ceil() as u32,
    )
    .ok_or("could not allocate pixmap")?;
    pixmap.fill(tiny_skia::Color::from_rgba8(64, 66, 70, 255));
    let panel = render::draw_dock(
        render::Target {
            pixmap: &mut pixmap,
            logical: (width as f32, height as f32),
            scale,
            offset_y: 0.0,
        },
        &metrics,
        &palette,
        render::Scene {
            slots: &slots,
            layout: &geometry,
            icons: &mut icons,
            thumbnails: &thumbnails::ThumbnailCache::default(),
            drop_target: None,
        },
    );

    pixmap.save_png(path)?;
    println!(
        "wrote {path} ({width}x{height}); theme={}; {} slot(s); panel {:?}",
        icons.theme(),
        slots.len(),
        panel
    );
    for s in &slots {
        println!("  {} icon={:?}", s.key, s.icon_name);
    }
    Ok(())
}

/// Renders a sample context menu to a PNG, for checking type and spacing
/// without a compositor.
fn dump_menu(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let metrics = Metrics::default();
    let palette = Palette::default();
    let mut text = text::TextRenderer::new();

    let slot = Slot {
        capture_key: None,
        kind: SlotKind::App,
        key: "demo".into(),
        label: "Firefox".into(),
        icon_name: None,
        windows: vec![1, 2],
        active: true,
        pinned: true,
    };
    let tops = vec![
        windows::Toplevel {
            id: 1,
            title: "Anthropic — Mozilla Firefox".into(),
            ..Default::default()
        },
        windows::Toplevel {
            id: 2,
            title: "Downloads".into(),
            ..Default::default()
        },
    ];
    // A preview of the full menu, so it shows every entry.
    let items = menu::build_menu(&slot, &tops, true, windows::Capabilities::default());
    let layout = menu::layout_menu(&items, &metrics, |s| {
        text.measure(s, metrics.pt(metrics.menu_font_size))
    });

    let mut pixmap = Pixmap::new(layout.width.ceil() as u32, layout.height.ceil() as u32)
        .ok_or("could not allocate pixmap")?;
    render::draw_menu(
        render::Target {
            pixmap: &mut pixmap,
            logical: (layout.width, layout.height),
            scale: 1.0,
            offset_y: 0.0,
        },
        &metrics,
        &palette,
        &items,
        &layout,
        Some(3),
        &mut text,
    );

    pixmap.save_png(path)?;
    println!("wrote {path} ({}x{})", layout.width, layout.height);
    Ok(())
}

struct App {
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    shm: Shm,
    compositor: CompositorState,
    dock: LayerDock,
    scale: SurfaceScale,
    /// Compositor-side frosted glass. Absent on compositors without
    /// `ext-background-effect-v1`, where the panel is simply not blurred --
    /// Hyprland does its own via `blurls`, others do none.
    background_effect: BackgroundEffectState,
    blur_surface: Option<
        wayland_protocols::ext::background_effect::v1::client::ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1,
    >,
    toplevels: ForeignToplevelManager,
    /// KWin's own protocol, when its .desktop grant is in place.
    plasma: Option<windows::PlasmaWindows>,
    /// Present only when neither Wayland route is available and KWin answered
    /// on D-Bus.
    kwin: Option<KwinWindows>,
    launchers: LauncherIndex,
    icons: IconCache,
    thumbnails: thumbnails::ThumbnailCache,
    /// Captures finish on their own threads and report back through here.
    events: channel::Sender<Watched>,
    /// The row on screen, which lags the target while tiles grow in or shrink
    /// away.
    visible_slots: Vec<Slot>,
    /// Per entry of `visible_slots`: on its way out.
    departing: Vec<bool>,
    /// Per entry of `visible_slots`: still growing in from nothing.
    entering: Vec<bool>,
    /// Window list and surface width the minimise targets were computed for.
    minimize_targets_for: (Vec<(crate::windows::ToplevelId, bool)>, u32),
    /// Desktop entry ids the user pinned, in order.
    pinned: Vec<String>,
    config: Config,
    config_path: Option<std::path::PathBuf>,
    trash: model::TrashState,
    trash_dir: Option<std::path::PathBuf>,
    metrics: Metrics,
    palette: Palette,

    /// Pointer position in *resting-layout* coordinates, or `None` when the
    /// pointer is away and the row should collapse.
    /// Pointer position in *surface* coordinates, not resting-layout ones.
    ///
    /// The conversion depends on how wide the row is, so storing the converted
    /// value would go stale the moment a tile joins or leaves: the row
    /// re-centres, and a magnification bulge computed against the old origin
    /// detaches from the pointer and hangs where the row used to be.
    pointer_surface_x: Option<f32>,
    /// Widths actually drawn. Eased toward the target layout rather than
    /// snapped to it, so the row glides instead of jumping between frames.
    current_widths: Vec<f32>,
    /// Timestamp of the last frame callback, for frame-rate-independent easing.
    last_frame_ms: Option<u32>,
    /// A frame callback is already in flight; asking for a second would double
    /// the animation rate.
    frame_pending: bool,
    pointer: Option<wl_pointer::WlPointer>,
    seat: Option<wl_seat::WlSeat>,
    /// Absent when the compositor has no `xdg-activation-v1`; launching still
    /// works, the new window just may not be given focus.
    activation: Option<ActivationState>,
    drag_candidate: Option<DragCandidate>,
    drag: Option<Drag>,

    /// Receiving files and launchers dropped from other applications.
    data_device_state: Option<DataDeviceManagerState>,
    data_device: Option<DataDevice>,
    /// Slot the drag is currently over, if any.
    drop_target: Option<usize>,
    /// Bytes read so far from an accepted drop.
    drop_buffer: Vec<u8>,
    loop_handle: LoopHandle<'static, App>,

    /// Present while a context menu is open.
    xdg_shell: Option<PopupShell>,
    open_menu: Option<MenuPopup>,
    menu_items: Vec<MenuItem>,
    menu_layout: MenuLayout,
    text: TextRenderer,

    /// 1.0 fully revealed, 0.0 fully slid out. Always 1.0 unless auto-hide is
    /// on.
    revealed: f32,
    /// When the pointer left. The dock waits before sliding out so that merely
    /// passing over it does not hide it.
    left_at: Option<Instant>,
    /// Applications spawned whose window has not shown up yet -- these are the
    /// ones bouncing. Keyed by slot, timed on our own clock rather than the
    /// compositor's frame clock, which only ticks while something is animating.
    launching: HashMap<String, Instant>,

    first_configure: bool,
    exit: bool,
}

impl App {
    /// Whichever window source this compositor supports.
    /// Whichever window source this compositor supports, best first.
    fn windows(&self) -> &dyn WindowSource {
        if let Some(plasma) = &self.plasma {
            return plasma;
        }
        match &self.kwin {
            Some(kwin) => kwin,
            None => &self.toplevels,
        }
    }

    fn debug_dump_windows(&self, source: &str) {
        if std::env::var_os("KDOCK_DEBUG").is_none() {
            return;
        }
        eprintln!(
            "-- {source}: {} window(s)",
            self.windows().toplevels().len()
        );
        for t in self.windows().toplevels() {
            eprintln!(
                "   [{}] app_id={:?} active={} min={} title={:?}",
                t.id, t.app_id, t.active, t.minimized, t.title
            );
        }
    }

    fn apply_thumbnail(&mut self, uuid: String, thumb: Option<thumbnails::Thumbnail>) {
        match thumb {
            Some(t) => self.thumbnails.insert(uuid, t),
            None => self.thumbnails.mark_failed(uuid),
        }
        self.draw();
    }

    /// Asks for the pictures the minimised tiles need, and drops the ones no
    /// tile wants any more.
    ///
    /// Forgetting on restore matters: the next time the window is minimised it
    /// should show what it looks like *then*, not a picture from last time.
    fn sync_thumbnails(&mut self, slots: &[Slot]) {
        let wanted: Vec<String> = slots
            .iter()
            .filter(|s| s.kind == SlotKind::MinimizedWindow)
            .filter_map(|s| s.capture_key.clone())
            .collect();

        for key in &wanted {
            let tx = self.events.clone();
            self.thumbnails.request(key, move |uuid, thumb| {
                let _ = tx.send(Watched::Thumbnail(uuid, thumb));
            });
        }

        for stale in self.thumbnails.keys_not_in(&wanted) {
            self.thumbnails.forget(&stale);
        }
    }

    fn apply_kwin_windows(&mut self, snapshot: &str) {
        if let Some(kwin) = self.kwin.as_mut() {
            kwin.apply(snapshot);
        }
        if std::env::var_os("KDOCK_DEBUG").is_some() {
            eprintln!(
                "-- kwin push: {} window(s)",
                self.windows().toplevels().len()
            );
            for t in self.windows().toplevels() {
                eprintln!(
                    "   [{}] app_id={:?} active={} min={} title={:?}",
                    t.id, t.app_id, t.active, t.minimized, t.title
                );
            }
        }
        self.draw();
    }

    /// The row as it should be, ignoring anything still animating out.
    fn target_slots(&self) -> Vec<Slot> {
        model::build_slots_with(
            &self.pinned,
            self.windows().toplevels(),
            &self.launchers,
            self.config.show_trash.then_some(self.trash),
            self.config.separate_minimized,
        )
    }

    /// The row actually on screen: the target plus whatever is still shrinking
    /// away. This is what gets drawn and hit-tested, so indices line up with
    /// the layout.
    fn slots(&self) -> Vec<Slot> {
        self.visible_slots.clone()
    }

    /// Brings the on-screen row in line with the target.
    ///
    /// Widths are carried across by key, not by position: a tile that has just
    /// appeared starts at nothing so it can grow, and one that is leaving keeps
    /// the width it had so it can shrink from there. Matching by position
    /// instead would hand a new tile the width of whatever used to sit at that
    /// index.
    fn sync_row(&mut self) {
        let target = self.target_slots();
        let merged = model::merge_rows(&self.visible_slots, &target);

        // The very first row has nothing to animate from, and nobody asked the
        // dock to make an entrance at login: adopt the resting widths whole.
        let first_row = self.visible_slots.is_empty();
        let rest = self.slot_metrics(&merged);

        let mut widths = Vec::with_capacity(merged.len());
        let mut entering = Vec::with_capacity(merged.len());

        for (i, slot) in merged.iter().enumerate() {
            let previous = self.visible_slots.iter().position(|s| s.key == slot.key);
            match previous {
                Some(j) => {
                    widths.push(self.current_widths.get(j).copied().unwrap_or(0.0));
                    // Still growing until it reaches full width.
                    entering.push(self.entering.get(j).copied().unwrap_or(false));
                }
                None if first_row => {
                    widths.push(rest.get(i).map_or(0.0, |m| m.rest_width));
                    entering.push(false);
                }
                None => {
                    // A tile joining an existing row grows out of nothing,
                    // parting its neighbours as it goes.
                    widths.push(0.0);
                    entering.push(true);
                }
            }
        }
        self.entering = entering;

        self.departing = merged
            .iter()
            .map(|s| model::is_departing(s, &target))
            .collect();
        self.visible_slots = merged;
        self.current_widths = widths;
    }

    /// A separator is narrow and holds its width; everything else is a tile
    /// that magnifies.
    fn slot_metrics(&self, slots: &[Slot]) -> Vec<SlotMetrics> {
        slots
            .iter()
            .map(|s| match s.kind {
                SlotKind::Separator => SlotMetrics {
                    rest_width: self.metrics.pt(self.metrics.separator_width),
                    magnifies: false,
                },
                _ => SlotMetrics {
                    rest_width: self.metrics.pt(self.metrics.tile_size),
                    magnifies: true,
                },
            })
            .collect()
    }

    /// Left edge of the row when nothing is magnified.
    ///
    /// The pointer is mapped through the *resting* origin, not the current one.
    /// Mapping through the live origin would let the row's own deformation move
    /// the coordinate that drives that deformation -- a feedback loop that
    /// makes the icons shimmer under a stationary pointer.
    /// The pointer in resting-layout coordinates, for the row as it is *now*.
    ///
    /// Derived on demand rather than stored: the row changes width as tiles
    /// come and go, and a converted value cached at pointer-event time would
    /// then describe a place the pointer is no longer at.
    fn cursor_rest_x(&self, slots: &[Slot]) -> Option<f32> {
        self.pointer_surface_x
            .map(|x| x - self.rest_origin_x(slots))
    }

    fn rest_origin_x(&self, slots: &[Slot]) -> f32 {
        let pad = self.metrics.pt(self.metrics.panel_padding_h);
        let rest_content: f32 = self.slot_metrics(slots).iter().map(|s| s.rest_width).sum();
        layout::rest_origin_x(self.dock.width as f32, rest_content, pad)
    }

    /// Positions for the widths currently drawn, accounting for a drag.
    ///
    /// While an icon is in the air the rest of the row is laid out as if it had
    /// already been dropped, and the icon itself follows the pointer. That is
    /// what makes the gap open ahead of the drop rather than after it.
    fn geometry(&self) -> Layout {
        let widths = &self.current_widths;
        let Some(drag) = self.drag.as_ref() else {
            return Layout::from_widths(widths);
        };

        let order = layout::drag_order(widths.len(), drag.slot, drag.insert);
        let mut slots = vec![layout::SlotGeometry::default(); widths.len()];
        let mut total = 0.0;
        for &i in &order {
            slots[i] = layout::SlotGeometry {
                x: total,
                width: widths[i],
                lift: 0.0,
            };
            total += widths[i];
        }
        if let Some(g) = slots.get_mut(drag.slot) {
            g.x = drag.pointer_x - g.width / 2.0;
        }
        Layout {
            slots,
            content_width: total,
        }
    }

    /// Advances the eased widths one frame. Returns whether anything still moves.
    fn step_widths(&mut self, dt_ms: f32) -> bool {
        self.sync_row();
        let slots = self.slots();
        // Magnification is suppressed mid-drag: the row's job then is to part
        // and show where the icon will land, not to bulge under the pointer.
        let cursor = if self.drag.is_some() {
            None
        } else {
            self.cursor_rest_x(&slots)
        };
        let mut metrics = self.slot_metrics(&slots);
        // A tile on its way out is heading for no width at all; everything else
        // parts to make room for it, or closes over the space it leaves.
        for (m, departing) in metrics.iter_mut().zip(&self.departing) {
            if *departing {
                m.rest_width = 0.0;
                m.magnifies = false;
            }
        }
        let target = layout::layout(&metrics, cursor, &self.metrics);
        let target_widths = target.widths();

        if self.current_widths.len() != target_widths.len() {
            // sync_row keeps these aligned; a mismatch would mean a bug there.
            self.current_widths = target_widths;
            return false;
        }

        let magnify = self.metrics.magnify_duration_ms as f32;
        let row_change = self.metrics.row_change_ms as f32;
        let mut moving = false;
        let mut settled: Vec<usize> = Vec::new();
        for (i, (cur, &want)) in self
            .current_widths
            .iter_mut()
            .zip(&target_widths)
            .enumerate()
        {
            // A tile joining or leaving the row moves on its own, slower clock;
            // everything else is tracking the pointer and must stay snappy.
            let duration = if self.departing.get(i).copied().unwrap_or(false)
                || self.entering.get(i).copied().unwrap_or(false)
            {
                row_change
            } else {
                magnify
            };
            let next = layout::approach(*cur, want, dt_ms, duration);
            // Below a fraction of a pixel the difference cannot be drawn, so
            // settle exactly rather than easing forever.
            if (want - next).abs() < 0.05 {
                *cur = want;
                settled.push(i);
            } else {
                *cur = next;
                moving = true;
            }
        }
        for i in settled {
            if let Some(flag) = self.entering.get_mut(i) {
                *flag = false;
            }
        }

        if std::env::var_os("KDOCK_DEBUG_ANIM").is_some() {
            let w: Vec<i32> = self.current_widths.iter().map(|w| *w as i32).collect();
            let t: Vec<i32> = target_widths.iter().map(|w| *w as i32).collect();
            eprintln!("ANIM dt={dt_ms:.0} cur={w:?} tgt={t:?} moving={moving}");
        }

        // A tile that has finished shrinking has nothing left to draw and must
        // not keep taking up an index, or hit testing would answer with a tile
        // the user cannot see.
        let finished: Vec<usize> = self
            .departing
            .iter()
            .enumerate()
            .filter(|(i, d)| **d && self.current_widths.get(*i).is_some_and(|w| *w < 0.5))
            .map(|(i, _)| i)
            .collect();
        for i in finished.into_iter().rev() {
            self.visible_slots.remove(i);
            self.current_widths.remove(i);
            self.departing.remove(i);
            self.entering.remove(i);
        }

        moving
    }

    fn request_frame(&mut self, qh: &QueueHandle<Self>) {
        if self.frame_pending {
            return;
        }
        let surface = self.dock.layer().wl_surface().clone();
        surface.frame(qh, FrameCallbackData(surface.clone()));
        self.frame_pending = true;
        // The callback only fires after a commit, so the surface has to be
        // committed for the animation to keep ticking.
        self.dock.layer().commit();
    }

    fn draw(&mut self) {
        let (w, h) = (self.dock.width, self.dock.height);
        if w == 0 || h == 0 {
            return;
        }

        // Geometry stays logical; only the raster target grows with the output.
        let (bw, bh) = self.scale.buffer_size(w, h);
        let Some(mut pixmap) = Pixmap::new(bw, bh) else {
            eprintln!("kdock: could not allocate a {bw}x{bh} pixmap");
            return;
        };

        self.sync_row();
        let slots = self.slots();
        self.sync_thumbnails(&slots);
        let mut geometry = self.geometry();
        for (slot, geom) in slots.iter().zip(&mut geometry.slots) {
            if let Some(started) = self.launching.get(&slot.key) {
                geom.lift =
                    layout::bounce_offset(started.elapsed().as_millis() as f32, &self.metrics);
            }
        }

        let offset_y = (1.0 - self.revealed) * self.hide_distance();
        let panel = render::draw_dock(
            render::Target {
                pixmap: &mut pixmap,
                logical: (w as f32, h as f32),
                scale: self.scale.scale(),
                offset_y,
            },
            &self.metrics,
            &self.palette,
            render::Scene {
                slots: &slots,
                layout: &geometry,
                icons: &mut self.icons,
                thumbnails: &self.thumbnails,
                drop_target: self.drop_target,
            },
        );

        // Windows only need to avoid the panel at rest, not the magnification
        // headroom above it -- same as macOS. An auto-hiding dock reserves
        // nothing, so windows get the whole screen.
        let strut = if self.config.auto_hide {
            0.0
        } else {
            self.hide_distance()
        };
        self.dock.set_exclusive_zone(strut.round() as i32);

        // Only the panel takes pointer events; the rest of the surface is
        // transparent to the windows underneath. Once the panel has slid far
        // enough out, all that is left is a sliver along the screen edge to
        // catch the pointer that asks for it back.
        let region = if self.drag.is_some() {
            // A drag has to be able to leave the panel -- that gesture is how
            // an icon is removed -- and the pointer stops being ours the moment
            // it crosses outside the input region.
            (0, 0, w as i32, h as i32)
        } else if self.revealed < 0.5 {
            let strip = TRIGGER_STRIP_PX;
            (
                panel.x().round() as i32,
                (h as f32 - strip).round() as i32,
                panel.width().round() as i32,
                strip as i32,
            )
        } else {
            (
                panel.x().round() as i32,
                panel.y().round() as i32,
                panel.width().round() as i32,
                panel.height().round() as i32,
            )
        };
        self.dock.set_input_region(&self.compositor, &[region]);

        self.publish_icon_rects();
        self.set_blur(panel);

        // The viewport has to say what the buffer stands for before it is
        // attached, or the compositor sizes the surface by raw buffer pixels.
        self.scale.set_logical_size(w, h);
        if let Err(e) = self.dock.present(&pixmap) {
            eprintln!("kdock: present failed: {e}");
        }
    }

    /// Re-reads the config file and adopts whatever changed.
    ///
    /// Editors often produce several filesystem events for one save, so this
    /// has to be cheap and idempotent: identical content is dropped without
    /// touching the surface, which also stops a save from restarting the
    /// magnification animation.
    fn reload_config(&mut self) {
        let Some(path) = self.config_path.as_deref() else {
            return;
        };
        let fresh = Config::load(path);
        if fresh == self.config {
            return;
        }

        // A different tile size changes how tall the surface has to be, and the
        // compositor has to be told before the next frame is drawn at the new
        // size.
        let old_height = self.metrics.surface_height().ceil() as u32;
        fresh.apply_to(&mut self.metrics);
        let new_height = self.metrics.surface_height().ceil() as u32;

        if self.config.icon_theme != fresh.icon_theme {
            self.icons = IconCache::new(fresh.icon_theme.clone());
        }
        self.pinned = fresh.pinned.clone();
        self.config = fresh;

        if new_height != old_height {
            self.dock.layer().set_size(0, new_height);
            self.dock.height = new_height;
        }
        // Slot count or sizes may both have moved; re-derive rather than ease.
        self.current_widths.clear();
        self.draw();
    }

    /// Opens the context menu for a slot.
    fn open_menu(&mut self, index: usize, serial: u32, qh: &QueueHandle<Self>) {
        let Some(xdg_shell) = self.xdg_shell.as_ref() else {
            return;
        };
        let slots = self.slots();
        let Some(slot) = slots.get(index) else {
            return;
        };
        if !slot.is_interactive() {
            return;
        }
        // A tile part-way through shrinking away stands for something that is
        // already gone; acting on it would target a window that no longer
        // exists.
        if self.departing.get(index).copied().unwrap_or(false) {
            return;
        }

        // A grabbing popup is defined to hold keyboard focus, but the dock's
        // layer surface says focus must never be given to it. Compositors are
        // entitled to resolve that by refusing the grab, which would silently
        // break click-outside-to-dismiss -- so focus is allowed for as long as
        // the menu is open and taken away again the moment it closes.
        self.set_keyboard_focusable(true);

        let pinned = self.pinned.contains(&slot.key);
        let items = menu::build_menu(
            slot,
            self.windows().toplevels(),
            pinned,
            self.windows().capabilities(),
        );
        if items.is_empty() {
            return;
        }

        let font_px = self.metrics.pt(self.metrics.menu_font_size);
        let text = &mut self.text;
        let layout = menu::layout_menu(&items, &self.metrics, |s| text.measure(s, font_px));

        // Anchor to the icon as currently drawn, so the menu points at what was
        // actually clicked rather than at the resting position.
        let geometry = Layout::from_widths(&self.current_widths);
        let Some(geom) = geometry.slots.get(index) else {
            return;
        };
        let row_x = self.row_origin_x();
        let panel_top = self.dock.height as f32
            - self.metrics.pt(self.metrics.panel_bottom_gap)
            - self.metrics.pt(self.metrics.panel_height());
        let anchor = (
            (row_x + geom.x).round() as i32,
            panel_top.round() as i32,
            geom.width.round() as i32,
            self.metrics.pt(self.metrics.panel_height()).round() as i32,
        );

        match MenuPopup::open(
            self.dock.layer(),
            xdg_shell,
            &self.compositor,
            &self.shm,
            qh,
            anchor,
            (layout.width.ceil() as u32, layout.height.ceil() as u32),
            index,
            self.seat.as_ref().map(|seat| (seat, serial)),
        ) {
            Ok(popup) => {
                self.menu_items = items;
                self.menu_layout = layout;
                self.open_menu = Some(popup);
            }
            Err(e) => eprintln!("kdock: could not open the menu: {e}"),
        }
    }

    /// Writes the pin list back to the config file.
    ///
    /// `config` is updated to match first, so the change notification this
    /// write provokes is recognised as our own and does not trigger a reload.
    fn save_pinned(&mut self) {
        self.config.pinned = self.pinned.clone();
        let Some(path) = self.config_path.as_deref() else {
            return;
        };
        if let Err(e) = Config::save_pinned(path, &self.pinned) {
            eprintln!("kdock: could not save the pinned list: {e}");
        }
    }

    fn close_menu(&mut self) {
        self.open_menu = None;
        self.menu_items.clear();
        self.set_keyboard_focusable(false);
    }

    /// Allows or forbids the dock taking keyboard focus.
    ///
    /// At rest the dock must never take it: a single click would otherwise
    /// steal input from whatever the user is actually typing into. The
    /// exception is a menu, which the protocol insists holds focus while its
    /// grab is active.
    fn set_keyboard_focusable(&self, focusable: bool) {
        use smithay_client_toolkit::shell::wlr_layer::SurfaceKind;
        use wayland_client::Proxy;

        // `on_demand` arrived in version 4; asking for it on an older
        // compositor is a protocol error, so leave those alone.
        let SurfaceKind::Wlr(surface) = self.dock.layer().kind() else {
            return;
        };
        if focusable && surface.version() < 4 {
            return;
        }

        self.dock.layer().set_keyboard_interactivity(if focusable {
            KeyboardInteractivity::OnDemand
        } else {
            KeyboardInteractivity::None
        });
        self.dock.layer().commit();
    }

    fn draw_menu(&mut self) {
        let Some(popup) = self.open_menu.as_mut() else {
            return;
        };
        if !popup.configured {
            return;
        }

        let scale = self.scale.scale();
        let (w, h) = (popup.width, popup.height);
        let (bw, bh) = (
            ((w as f32 * scale) as u32).max(1),
            ((h as f32 * scale) as u32).max(1),
        );
        let Some(mut pixmap) = Pixmap::new(bw, bh) else {
            return;
        };

        render::draw_menu(
            render::Target {
                pixmap: &mut pixmap,
                logical: (w as f32, h as f32),
                scale,
                offset_y: 0.0,
            },
            &self.metrics,
            &self.palette,
            &self.menu_items,
            &self.menu_layout,
            popup.highlighted,
            &mut self.text,
        );

        if let Err(e) = popup.present(&pixmap) {
            eprintln!("kdock: menu present failed: {e}");
        }
    }

    /// Carries out a chosen menu item.
    fn apply_menu_action(&mut self, action: MenuAction, slot_index: usize, qh: &QueueHandle<Self>) {
        let slots = self.slots();
        let Some(slot) = slots.get(slot_index).cloned() else {
            return;
        };

        match action {
            MenuAction::ActivateWindow(id) => self.windows().activate(id),
            MenuAction::ShowAllWindows => {
                for &id in &slot.windows {
                    self.windows().activate(id);
                }
            }
            MenuAction::Hide => {
                for &id in &slot.windows {
                    self.windows().set_minimized(id, true);
                }
            }
            MenuAction::Quit => {
                for &id in &slot.windows {
                    self.windows().close(id);
                }
            }
            MenuAction::Open => self.launch(&slot, None, qh),
            MenuAction::OpenTrash => open_trash(),
            MenuAction::TogglePinned => {
                if let Some(pos) = self.pinned.iter().position(|p| *p == slot.key) {
                    self.pinned.remove(pos);
                } else {
                    self.pinned.push(slot.key.clone());
                }
                self.save_pinned();
                self.current_widths.clear();
                self.draw();
            }
        }
    }

    /// Tells the compositor which part of the dock stands for each window.
    ///
    /// This is what makes a minimise animation fly into the right icon instead
    /// of into the middle of the screen. Coordinates are surface-local, which
    /// is what the protocol asks for.
    /// Tells the compositor which part of the dock stands for each window.
    ///
    /// This is what makes a minimise animation fly into the right tile instead
    /// of into the middle of the screen. Coordinates are surface-local, which
    /// is what the protocol asks for.
    ///
    /// A window still on screen is pointed at the tile it is *going* to get,
    /// not at its application's icon: KWin reads the geometry as the window
    /// leaves and reuses it for the journey back, so anything published after
    /// the fact arrives too late to matter.
    ///
    /// Only redone when the window list moves. The targets are resting
    /// positions -- magnification does not change where a window will land --
    /// so simulating a layout per window on every frame would be pure waste.
    fn publish_icon_rects(&mut self) {
        let signature: Vec<(crate::windows::ToplevelId, bool)> = self
            .windows()
            .toplevels()
            .iter()
            .map(|t| (t.id, t.minimized))
            .collect();
        let width = self.dock.width;
        if self.minimize_targets_for == (signature.clone(), width) {
            return;
        }
        self.minimize_targets_for = (signature, width);

        let targets = model::minimize_targets(
            &self.pinned,
            self.windows().toplevels(),
            &self.launchers,
            self.config.show_trash.then_some(self.trash),
            self.config.separate_minimized,
        );

        let surface = self.dock.layer().wl_surface();
        let logical_h = self.metrics.surface_height();

        for target in targets {
            // Each window gets the row as it would be with *that* window
            // minimised, so the tile positions have to come from its own layout.
            let metrics = self.slot_metrics(&target.slots);
            let geometry = layout::layout(&metrics, None, &self.metrics);
            let Some(geom) = geometry.slots.get(target.slot) else {
                continue;
            };

            let panel = render::panel_rect(
                width as f32,
                logical_h,
                &self.metrics,
                geometry.content_width,
            );
            let row_x = panel.x() + self.metrics.pt(self.metrics.panel_padding_h);

            self.windows().set_icon_rect(
                target.window,
                surface,
                (
                    (row_x + geom.x).round() as i32,
                    panel.y().round() as i32,
                    geom.width.round() as i32,
                    panel.height().round() as i32,
                ),
            );
        }
    }

    /// Asks the compositor to blur what is behind the panel.
    ///
    /// The region follows the capsule rather than its bounding box, so the
    /// frosted glass does not square off the panel's rounded ends. Coordinates
    /// are surface-local and logical, matching the input region.
    fn set_blur(&mut self, panel: tiny_skia::Rect) {
        let Some(effect) = self.blur_surface.as_ref() else {
            return;
        };
        let Ok(region) = Region::new(&self.compositor) else {
            return;
        };

        let radius = self.metrics.pt(self.metrics.panel_radius());
        for (x, y, w, h) in render::capsule_region(panel, radius, BLUR_CAP_SLICES) {
            region.add(x, y, w, h);
        }
        effect.set_blur_region(Some(region.wl_region()));
    }

    /// Distance the panel travels to leave the screen entirely.
    fn hide_distance(&self) -> f32 {
        self.metrics
            .pt(self.metrics.panel_height() + self.metrics.panel_bottom_gap)
    }

    /// Where the reveal animation is heading: out once the pointer has been
    /// away long enough, in the moment it comes back.
    fn reveal_target(&self) -> f32 {
        if !self.config.auto_hide {
            return 1.0;
        }
        match self.left_at {
            None => 1.0,
            Some(t) => {
                let delay = u128::from(self.metrics.auto_hide_delay_ms);
                if t.elapsed().as_millis() >= delay {
                    0.0
                } else {
                    1.0
                }
            }
        }
    }

    /// Reports a screen appearing or disappearing.
    ///
    /// Moving the dock between screens means tearing down the layer surface and
    /// building a new one -- `zwlr_layer_surface_v1` fixes its output at
    /// creation and has no request to change it. That is a larger surgery than
    /// it looks, since the blur, viewport and scale objects are all bound to the
    /// old surface, so for now the change is reported rather than performed.
    fn output_changed(&mut self, output: &wl_output::WlOutput) {
        let Some(wanted) = self.config.output.clone() else {
            return;
        };
        let matches = self.output_state.info(output).is_some_and(|info| {
            info.name
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(&wanted))
        });
        if matches {
            eprintln!(
                "kdock: output {wanted:?} was attached or removed; restart to move the dock there."
            );
        }
    }

    /// Re-checks whether the Trash is empty and redraws if the icon changes.
    fn reload_trash(&mut self) {
        let Some(dir) = self.trash_dir.as_deref() else {
            return;
        };
        let fresh = trash::read(dir);
        if fresh != self.trash {
            self.trash = fresh;
            self.draw();
        }
    }

    /// Left edge of the row as currently drawn.
    fn row_origin_x(&self) -> f32 {
        let pad = self.metrics.pt(self.metrics.panel_padding_h);
        let content: f32 = self.current_widths.iter().sum();
        let content = if content > 0.0 {
            content
        } else {
            self.metrics.pt(self.metrics.tile_size)
        };
        ((self.dock.width as f32 - (content + pad * 2.0)) / 2.0).max(0.0) + pad
    }

    fn slot_at(&self, surface_x: f32) -> Option<usize> {
        self.geometry().hit(surface_x - self.row_origin_x())
    }

    /// Promotes a press into a drag once it has moved, and tracks it after.
    fn update_drag(&mut self, surface_x: f32, surface_y: f32, slots: &[Slot]) {
        if let Some(candidate) = self.drag_candidate.clone() {
            if (surface_x - candidate.start_x).abs() < DRAG_THRESHOLD_PX {
                return;
            }
            let Some(slot) = slots.get(candidate.slot) else {
                return;
            };
            // Separators cannot be picked up, and the Trash is a fixture.
            if !slot.is_interactive() || slot.kind == SlotKind::Trash {
                self.drag_candidate = None;
                return;
            }
            self.drag = Some(Drag {
                slot: candidate.slot,
                key: slot.key.clone(),
                pointer_x: surface_x - self.row_origin_x(),
                insert: candidate.slot,
                outside: false,
            });
            self.drag_candidate = None;
        }

        // Everything the update needs is read before taking the mutable
        // borrow, since all of it hangs off `self`.
        let Some(from) = self.drag.as_ref().map(|d| d.slot) else {
            return;
        };
        let pointer_x = surface_x - self.row_origin_x();
        let insert = layout::insert_index(&self.slot_metrics(slots), from, pointer_x);

        // Dragged clear of the panel: releasing here unpins rather than moves.
        let panel_top = self.dock.height as f32
            - self.metrics.pt(self.metrics.panel_bottom_gap)
            - self.metrics.pt(self.metrics.panel_height());
        let outside = surface_y < panel_top - self.metrics.pt(self.metrics.tile_size);

        if let Some(drag) = self.drag.as_mut() {
            drag.pointer_x = pointer_x;
            drag.insert = insert;
            drag.outside = outside;
        }
    }

    /// Notes which icon an external drag is hovering, and tells the source we
    /// will take a file list.
    fn update_drop_target(
        &mut self,
        _device: &wayland_client::protocol::wl_data_device::WlDataDevice,
        x: f64,
    ) {
        let target = self.slot_at(x as f32);
        if let Some(offer) = self
            .data_device
            .as_ref()
            .and_then(|d| d.data().drag_offer())
        {
            let accepted =
                offer.with_mime_types(|types| types.iter().any(|t| t == drops::URI_LIST));
            offer.accept_mime_type(offer.serial, accepted.then(|| drops::URI_LIST.to_owned()));
            offer.set_actions(DndAction::Copy, DndAction::Copy);
        }

        if self.drop_target != target {
            self.drop_target = target;
            self.draw();
        }
    }

    /// Acts on a completed drop.
    fn finish_drop(&mut self, payload: &str, target: Option<usize>) {
        let paths = drops::parse_uri_list(payload);
        self.drop_target = None;
        if paths.is_empty() {
            self.draw();
            return;
        }

        let slots = self.slots();
        let slot = target.and_then(|i| slots.get(i)).cloned();

        match slot {
            // Onto the Trash: move the files there.
            Some(s) if s.kind == SlotKind::Trash => {
                let Some(dir) = self.trash_dir.clone() else {
                    return;
                };
                for path in &paths {
                    if let Err(e) = trash::move_to_trash(path, &dir) {
                        eprintln!("kdock: could not trash {}: {e}", path.display());
                    }
                }
                self.reload_trash();
            }
            // Onto an application: open the files with it.
            Some(s) if s.kind == SlotKind::App && !drops_are_launchers(&paths) => {
                self.open_with(&s, &paths);
            }
            // Anywhere else, or launchers dropped on the row: pin them.
            _ => {
                let mut added = false;
                for path in paths.iter().filter(|p| drops::is_desktop_entry(p)) {
                    if let Some(id) = drops::desktop_id(path) {
                        if !self.pinned.contains(&id) {
                            self.pinned.push(id);
                            added = true;
                        }
                    }
                }
                if added {
                    self.save_pinned();
                    self.current_widths.clear();
                }
            }
        }
        self.draw();
    }

    /// Launches an application with the dropped files as arguments.
    fn open_with(&mut self, slot: &Slot, paths: &[std::path::PathBuf]) {
        let Some(exec) = self
            .launchers
            .by_id(&slot.key)
            .and_then(|l| l.exec.as_deref())
        else {
            return;
        };
        let mut argv = launchers::exec_argv(exec);
        argv.extend(paths.iter().map(|p| p.to_string_lossy().into_owned()));
        self.spawn(
            PendingLaunch {
                key: slot.key.clone(),
                argv,
            },
            None,
        );
    }

    /// Commits a finished reorder, or removes an icon dragged out of the dock.
    fn finish_drag(&mut self) {
        let Some(drag) = self.drag.take() else {
            return;
        };

        // Only pinned entries have a stored order; dragging a window-only tile
        // has nothing to write back.
        let Some(from) = self.pinned.iter().position(|p| *p == drag.key) else {
            self.current_widths.clear();
            self.draw();
            return;
        };

        if drag.outside {
            self.pinned.remove(from);
        } else {
            // The row includes non-pinned tiles, so the drop position has to be
            // translated back into an index within the pin list.
            let slots = self.slots();
            let order = layout::drag_order(slots.len(), drag.slot, drag.insert);
            let mut pinned_order: Vec<String> = Vec::with_capacity(self.pinned.len());
            for &i in &order {
                if let Some(slot) = slots.get(i) {
                    if self.pinned.contains(&slot.key) {
                        pinned_order.push(slot.key.clone());
                    }
                }
            }
            if pinned_order.len() == self.pinned.len() {
                self.pinned = pinned_order;
            }
        }

        self.save_pinned();
        self.current_widths.clear();
        self.draw();
    }

    /// What a left click does, following macOS rather than the usual taskbar
    /// rules.
    ///
    /// Notably a dock icon never minimises an application -- that is a
    /// Windows/Linux taskbar convention. Clicking the frontmost app instead
    /// restores whichever of its windows are minimised.
    fn activate_slot(&mut self, index: usize, serial: Option<u32>, qh: &QueueHandle<Self>) {
        let slots = self.slots();
        let Some(slot) = slots.get(index) else {
            return;
        };
        if !slot.is_interactive() {
            return;
        }

        if slot.kind == SlotKind::Trash {
            open_trash();
            return;
        }

        // A minimised tile stands for one window, so it restores exactly that
        // one rather than the whole application.
        if slot.kind == SlotKind::MinimizedWindow {
            for &id in &slot.windows {
                self.windows().activate(id);
            }
            return;
        }

        if !slot.is_running() {
            let slot = slot.clone();
            self.launch(&slot, serial, qh);
            return;
        }

        if slot.active {
            for &id in &slot.windows {
                self.windows().set_minimized(id, false);
            }
            return;
        }

        // Raise every window of the application, not just the most recent one:
        // on macOS the whole app comes forward. The last activation wins focus.
        for &id in &slot.windows {
            self.windows().activate(id);
        }
    }

    /// Drops applications that have finished starting, or that never will.
    ///
    /// Without the timeout a launch that fails silently -- a missing binary, an
    /// app that exits immediately -- would leave its icon bouncing forever.
    fn prune_launching(&mut self) {
        const GIVE_UP: std::time::Duration = std::time::Duration::from_secs(20);
        let running: std::collections::HashSet<String> = self
            .slots()
            .iter()
            .filter(|s| s.is_running())
            .map(|s| s.key.clone())
            .collect();
        self.launching
            .retain(|key, started| !running.contains(key) && started.elapsed() < GIVE_UP);
    }

    fn launch(&mut self, slot: &Slot, serial: Option<u32>, qh: &QueueHandle<Self>) {
        let Some(exec) = self
            .launchers
            .by_id(&slot.key)
            .and_then(|l| l.exec.as_deref())
        else {
            eprintln!("kdock: {} has no Exec= to launch", slot.key);
            return;
        };

        let argv = launchers::exec_argv(exec);
        if argv.is_empty() {
            return;
        }
        let pending = PendingLaunch {
            key: slot.key.clone(),
            argv,
        };

        // Ask for an activation token first so the compositor lets the new
        // window take focus. Compositors reject tokens requested without a
        // recent input serial, which is why the click's serial is threaded
        // through to here.
        match (&self.activation, serial, &self.seat) {
            (Some(activation), Some(serial), Some(seat)) => {
                activation.request_token(
                    qh,
                    RequestData {
                        app_id: Some(pending.key.clone()),
                        seat_and_serial: Some((seat.clone(), serial)),
                        surface: Some(self.dock.layer().wl_surface().clone()),
                        udata: pending,
                    },
                );
            }
            // No activation protocol, or no serial to justify one: launch
            // anyway. The application still starts, it just may open behind.
            _ => self.spawn(pending, None),
        }
    }

    fn spawn(&mut self, pending: PendingLaunch, token: Option<String>) {
        let Some((program, args)) = pending.argv.split_first() else {
            return;
        };

        let mut cmd = std::process::Command::new(program);
        cmd.args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if let Some(token) = token {
            cmd.env("XDG_ACTIVATION_TOKEN", &token);
            // Still read by plenty of toolkits that predate xdg-activation.
            cmd.env("DESKTOP_STARTUP_ID", token);
        }

        // Detached: the application must outlive the dock, and must not be left
        // as a zombie for the dock to reap.
        match cmd.spawn() {
            Ok(mut child) => {
                self.launching.insert(pending.key, Instant::now());
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(e) => eprintln!("kdock: could not launch {program}: {e}"),
        }
    }
}

/// Finds an attached output by connector name.
///
/// Matched case-insensitively against the name the compositor reports (`DP-1`,
/// `eDP-1`), falling back to the human-readable description so that a user who
/// copied a name out of their compositor's own output list still gets a match.
fn find_output(
    outputs: &OutputState,
    name: &str,
) -> Option<wayland_client::protocol::wl_output::WlOutput> {
    outputs.outputs().find(|o| {
        outputs.info(o).is_some_and(|info| {
            info.name
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
                || info
                    .description
                    .as_deref()
                    .is_some_and(|d| d.eq_ignore_ascii_case(name))
        })
    })
}

/// Whether every dropped path is a desktop entry, which means "pin these"
/// rather than "open these with the application underneath".
fn drops_are_launchers(paths: &[std::path::PathBuf]) -> bool {
    !paths.is_empty() && paths.iter().all(|p| drops::is_desktop_entry(p))
}

/// Opens the Trash in the desktop's file manager.
///
/// `xdg-open` is the portable entry point; `trash:///` is the URI the major
/// file managers agree on for it.
fn open_trash() {
    if let Err(e) = std::process::Command::new("xdg-open")
        .arg("trash:///")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        eprintln!("kdock: could not open the Trash: {e}");
    }
}

/// A press on an icon that has not yet moved far enough to be a drag.
#[derive(Debug, Clone)]
struct DragCandidate {
    slot: usize,
    start_x: f32,
    serial: u32,
}

/// A reorder in progress.
#[derive(Debug, Clone)]
struct Drag {
    /// Index in the row the icon came from.
    slot: usize,
    key: String,
    /// Pointer position in row-local coordinates.
    pointer_x: f32,
    /// Where it would land if dropped now.
    insert: usize,
    /// Pointer has left the panel: dropping here removes the icon.
    outside: bool,
}

/// How far the pointer must travel before a press becomes a drag.
///
/// Without a threshold every click would jitter the row by a pixel or two on
/// the way to being a click.
const DRAG_THRESHOLD_PX: f32 = 5.0;

/// A launch waiting on its activation token.
#[derive(Debug, Clone)]
struct PendingLaunch {
    key: String,
    argv: Vec<String>,
}

impl ActivationHandler for App {
    type RequestUdata = PendingLaunch;

    fn new_token(&mut self, token: String, data: &RequestData<PendingLaunch>) {
        self.spawn(data.udata.clone(), Some(token));
    }
}

impl DataDeviceHandler for App {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        device: &wayland_client::protocol::wl_data_device::WlDataDevice,
        x: f64,
        _y: f64,
        _surface: &wl_surface::WlSurface,
    ) {
        self.update_drop_target(device, x);
    }

    fn motion(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        device: &wayland_client::protocol::wl_data_device::WlDataDevice,
        x: f64,
        _y: f64,
    ) {
        self.update_drop_target(device, x);
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wayland_client::protocol::wl_data_device::WlDataDevice,
    ) {
        self.drop_target = None;
        self.draw();
    }

    fn selection(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wayland_client::protocol::wl_data_device::WlDataDevice,
    ) {
        // The clipboard is none of the dock's business.
    }

    fn drop_performed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wayland_client::protocol::wl_data_device::WlDataDevice,
    ) {
        let Some(offer) = self
            .data_device
            .as_ref()
            .and_then(|d| d.data().drag_offer())
        else {
            return;
        };
        let target = self.drop_target;
        self.drop_buffer.clear();

        let pipe = match offer.receive(drops::URI_LIST.to_owned()) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("kdock: could not read the drop: {e}");
                return;
            }
        };

        // The source writes asynchronously, so the pipe is drained through the
        // event loop rather than blocked on here.
        let offer = offer.clone();
        let _ = self
            .loop_handle
            .insert_source(pipe, move |_, file, app: &mut App| {
                use std::io::Read;
                // SAFETY: the file is only read; the descriptor stays owned by the
                // source.
                let file: &mut std::fs::File = unsafe { file.get_mut() };
                let mut chunk = [0u8; 4096];

                match file.read(&mut chunk) {
                    Ok(0) => {
                        let text = String::from_utf8_lossy(&app.drop_buffer).into_owned();
                        app.finish_drop(&text, target);
                        offer.finish();
                        offer.destroy();
                        PostAction::Remove
                    }
                    Ok(n) => {
                        app.drop_buffer.extend_from_slice(&chunk[..n]);
                        PostAction::Continue
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => PostAction::Continue,
                    Err(e) => {
                        eprintln!("kdock: error reading the drop: {e}");
                        offer.finish();
                        offer.destroy();
                        PostAction::Remove
                    }
                }
            });
    }
}

impl DataOfferHandler for App {
    fn source_actions(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        offer: &mut DragOffer,
        _: DndAction,
    ) {
        // Copy throughout: dropping on the dock never moves the original.
        offer.set_actions(DndAction::Copy, DndAction::Copy);
    }

    fn selected_action(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &mut DragOffer,
        _: DndAction,
    ) {
    }
}

impl PopupHandler for App {
    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Popup, cfg: PopupConfigure) {
        if let Some(menu) = self.open_menu.as_mut() {
            // The compositor may shrink the menu to fit the screen; honour what
            // it grants rather than what was asked for.
            if cfg.width > 0 && cfg.height > 0 {
                menu.width = cfg.width as u32;
                menu.height = cfg.height as u32;
            }
            menu.configured = true;
        }
        self.draw_menu();
    }

    fn done(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Popup) {
        // Dismissed by the compositor -- a click outside, or a grab broken.
        self.close_menu();
    }
}

impl BackgroundEffectHandler for App {
    fn background_effect_state(&mut self) -> &mut BackgroundEffectState {
        &mut self.background_effect
    }

    fn update_capabilities(&mut self) {
        // Capabilities arrive after the global is bound; nothing to react to
        // beyond noting that blur is on offer at all.
    }
}

impl ScaleHandler for App {
    fn scale_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, scale: f32) {
        if self.scale.set_scale(scale) {
            self.draw();
        }
    }
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        let mut changed = false;
        let mut right_clicked = None;
        let mut menu_choice = None;
        let mut menu_redraw = false;

        for event in events {
            // Events for the menu's own surface drive the menu, not the dock.
            if self
                .open_menu
                .as_ref()
                .is_some_and(|m| &event.surface == m.wl_surface())
            {
                match event.kind {
                    PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                        let hit = self
                            .menu_layout
                            .hit(&self.menu_items, event.position.1 as f32);
                        if let Some(menu) = self.open_menu.as_mut() {
                            if menu.highlighted != hit {
                                menu.highlighted = hit;
                                menu_redraw = true;
                            }
                        }
                    }
                    PointerEventKind::Leave { .. } => {
                        if let Some(menu) = self.open_menu.as_mut() {
                            if menu.highlighted.is_some() {
                                menu.highlighted = None;
                                menu_redraw = true;
                            }
                        }
                    }
                    // Acting on release rather than press matches how menus
                    // behave everywhere else, and lets a press-drag-release
                    // gesture pick an item.
                    PointerEventKind::Release { button: 0x110, .. } => {
                        if let Some(menu) = self.open_menu.as_ref() {
                            menu_choice = menu
                                .highlighted
                                .and_then(|i| self.menu_items.get(i))
                                .and_then(|i| i.action.clone())
                                .map(|a| (a, menu.slot_index));
                        }
                    }
                    _ => {}
                }
                continue;
            }

            if &event.surface != self.dock.layer().wl_surface() {
                continue;
            }
            match event.kind {
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                    let slots = self.slots();
                    self.pointer_surface_x = Some(event.position.0 as f32);
                    self.left_at = None;
                    self.update_drag(event.position.0 as f32, event.position.1 as f32, &slots);
                    changed = true;
                }
                PointerEventKind::Leave { .. } => {
                    self.pointer_surface_x = None;
                    self.left_at = Some(Instant::now());
                    changed = true;
                }
                PointerEventKind::Release { button: 0x110, .. } => {
                    if self.drag.is_some() {
                        self.finish_drag();
                    } else if let Some(c) = self.drag_candidate.take() {
                        // A press that never became a drag is a click, and
                        // acting on release is what lets the two be told apart.
                        self.activate_slot(c.slot, Some(c.serial), qh);
                    }
                    self.drag_candidate = None;
                    changed = true;
                }
                // 0x110 is BTN_LEFT, 0x111 BTN_RIGHT.
                PointerEventKind::Press {
                    button: 0x110,
                    serial,
                    ..
                } => {
                    self.drag_candidate =
                        self.slot_at(event.position.0 as f32)
                            .map(|slot| DragCandidate {
                                slot,
                                start_x: event.position.0 as f32,
                                serial,
                            });
                }
                PointerEventKind::Press {
                    button: 0x111,
                    serial,
                    ..
                } => {
                    right_clicked = self.slot_at(event.position.0 as f32).map(|i| (i, serial));
                }
                _ => {}
            }
        }

        if let Some((action, slot_index)) = menu_choice {
            self.close_menu();
            self.apply_menu_action(action, slot_index, qh);
            changed = true;
        } else if menu_redraw {
            self.draw_menu();
        }

        if let Some((index, serial)) = right_clicked {
            self.open_menu(index, serial, qh);
        }
        if changed {
            self.request_frame(qh);
        }
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.exit = true;
    }

    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        // A zero dimension means the compositor left the choice to us; fall
        // back to what we asked for.
        self.dock.width =
            NonZeroU32::new(configure.new_size.0).map_or(self.dock.width, |v| v.get());
        self.dock.height =
            NonZeroU32::new(configure.new_size.1).map_or(self.dock.height, |v| v.get());

        let first = self.first_configure;
        self.first_configure = false;
        self.draw();

        // Nothing else has asked for a frame yet, so the initial slide-out
        // would never get a tick to run on.
        if first && self.config.auto_hide {
            self.request_frame(qh);
        }
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        // Only relevant without fractional-scale; when that protocol is in play
        // it is the authority and this event would fight it.
        if !self.scale.is_fractional() && self.scale.set_scale(new_factor as f32) {
            self.dock.layer().wl_surface().set_buffer_scale(new_factor);
            self.draw();
        }
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        time: u32,
    ) {
        self.frame_pending = false;

        // Idle time is not animation time. The dock asks for no frames while
        // nothing moves, so the gap since the last callback is however long it
        // sat still -- and feeding that to the easing makes a fresh animation
        // finish in its first step. The clock is cleared when the dock settles,
        // so a resumed animation starts from a nominal frame instead.
        //
        // The cap covers the other case: a stall mid-animation. It has to stay
        // well under `magnify_duration_ms`, or one dropped frame still swallows
        // the whole animation.
        let dt = self
            .last_frame_ms
            .map_or(16.0, |prev| time.wrapping_sub(prev) as f32)
            .clamp(1.0, 32.0);
        self.last_frame_ms = Some(time);

        self.prune_launching();
        let target = self.reveal_target();
        let next = layout::approach(
            self.revealed,
            target,
            dt,
            self.metrics.auto_hide_slide_ms as f32,
        );
        let sliding = if (target - next).abs() < 0.002 {
            self.revealed = target;
            false
        } else {
            self.revealed = next;
            true
        };
        // A pending hide has to keep frames coming, or the countdown would
        // stall the moment the magnification settles.
        let waiting_to_hide = self.config.auto_hide && self.left_at.is_some() && target > 0.0;
        let moving =
            self.step_widths(dt) || !self.launching.is_empty() || sliding || waiting_to_hide;
        self.draw();
        if moving {
            self.request_frame(qh);
        } else {
            // Settled. Forget the clock so the next animation starts from a
            // nominal frame rather than from however long the dock sat idle.
            self.last_frame_ms = None;
        }
    }

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

impl windows::plasma::PlasmaWindowHandler for App {
    fn plasma_state(&mut self) -> &mut windows::PlasmaWindows {
        self.plasma
            .as_mut()
            .expect("the handler only runs while the protocol is bound")
    }

    fn plasma_windows_changed(&mut self, _: &Connection, qh: &QueueHandle<Self>) {
        self.debug_dump_windows("plasma");
        self.draw();
        // Tiles grow in and shrink away over several frames, so the row moving
        // has to start the clock.
        self.request_frame(qh);
    }
}

impl ForeignToplevelHandler for App {
    fn foreign_toplevel_state(&mut self) -> &mut ForeignToplevelManager {
        &mut self.toplevels
    }

    fn toplevels_changed(&mut self, _: &Connection, qh: &QueueHandle<Self>) {
        if std::env::var_os("KDOCK_DEBUG").is_some() {
            eprintln!("-- {} toplevel(s)", self.windows().toplevels().len());
            for t in self.windows().toplevels() {
                eprintln!(
                    "   [{}] app_id={:?} active={} min={} parent={:?} title={:?}",
                    t.id, t.app_id, t.active, t.minimized, t.parent, t.title
                );
            }
        }
        self.draw();
        // Tiles grow in and shrink away over several frames, so the row moving
        // has to start the clock.
        self.request_frame(qh);
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat) {
        // `activate` is defined per-seat, so the window source needs one before
        // a click can raise anything.
        self.toplevels.set_seat(seat.clone());
        if let Some(state) = self.data_device_state.as_ref() {
            self.data_device = Some(state.get_data_device(qh, &seat));
        }
        self.seat = Some(seat);
    }

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            match self.seat_state.get_pointer(qh, &seat) {
                Ok(p) => self.pointer = Some(p),
                Err(e) => eprintln!("kdock: no pointer: {e}"),
            }
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

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
        self.output_changed(&output);
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        self.output_changed(&output);
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

delegate_registry!(App);

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

smithay_client_toolkit::delegate_dispatch2!(App);
