//! Synthesises pointer input, for testing the dock's gestures.
//!
//! The dock's most interesting behaviour -- the context menu, drag-to-reorder --
//! only happens in response to a real pointer, and there is no way to script one
//! through a compositor's normal interfaces. `wlr-virtual-pointer` exists for
//! exactly this, so this drives it from the command line.
//!
//! Meant to be run inside a nested compositor alongside the dock, never against
//! a session someone is using: it moves the actual pointer and presses actual
//! buttons.
//!
//! ```text
//! synth-input <screen-w> <screen-h> move <x> <y>
//! synth-input <screen-w> <screen-h> click <x> <y> [left|right]
//! synth-input <screen-w> <screen-h> drag <x1> <y1> <x2> <y2>
//! ```

use std::{thread::sleep, time::Duration};

use wayland_client::{
    globals::{registry_queue_init, GlobalListContents},
    protocol::{wl_pointer, wl_registry, wl_seat},
    Connection, Dispatch, QueueHandle,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

struct State;

macro_rules! ignore_events {
    ($($iface:ty),+ $(,)?) => {$(
        impl Dispatch<$iface, ()> for State {
            fn event(
                _: &mut Self,
                _: &$iface,
                _: <$iface as wayland_client::Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    )+};
}

// `registry_queue_init` keeps the registry itself, with the global list as its
// user data, so that one needs its own impl rather than the unit-data macro.
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

ignore_events!(
    wl_seat::WlSeat,
    ZwlrVirtualPointerManagerV1,
    ZwlrVirtualPointerV1,
);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!("usage: synth-input <screen-w> <screen-h> <move|click|drag> ...");
        std::process::exit(2);
    }
    let (sw, sh): (u32, u32) = (args[0].parse()?, args[1].parse()?);

    let conn = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
    let qh = queue.handle();

    let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=8, ())?;
    let manager: ZwlrVirtualPointerManagerV1 = globals.bind(&qh, 1..=2, ()).map_err(|e| {
        format!("this compositor has no wlr-virtual-pointer, so input cannot be scripted ({e})")
    })?;
    let pointer = manager.create_virtual_pointer(Some(&seat), &qh, ());

    // Absolute motion is in an arbitrary coordinate space the client defines by
    // passing its extent, so the screen size is given rather than assumed.
    let warp = |x: u32, y: u32| {
        pointer.motion_absolute(0, x, y, sw, sh);
        pointer.frame();
    };

    match args[2].as_str() {
        "move" => {
            warp(args[3].parse()?, args[4].parse()?);
        }
        "click" => {
            let button = match args.get(5).map(String::as_str) {
                Some("right") => BTN_RIGHT,
                _ => BTN_LEFT,
            };
            warp(args[3].parse()?, args[4].parse()?);
            queue.roundtrip(&mut State)?;
            // A press and its release in the same instant can be coalesced;
            // separating them makes the gesture unambiguous.
            sleep(Duration::from_millis(120));
            pointer.button(0, button, wl_pointer::ButtonState::Pressed);
            pointer.frame();
            queue.roundtrip(&mut State)?;
            sleep(Duration::from_millis(120));
            pointer.button(0, button, wl_pointer::ButtonState::Released);
            pointer.frame();
        }
        "drag" => {
            let (x1, y1): (u32, u32) = (args[3].parse()?, args[4].parse()?);
            let (x2, y2): (u32, u32) = (args[5].parse()?, args[6].parse()?);

            warp(x1, y1);
            queue.roundtrip(&mut State)?;
            sleep(Duration::from_millis(120));
            pointer.button(0, BTN_LEFT, wl_pointer::ButtonState::Pressed);
            pointer.frame();
            queue.roundtrip(&mut State)?;

            // Stepped rather than jumped: the dock only treats a press as a
            // drag once it has seen motion, and a single leap can be delivered
            // as one event that arrives before the press is processed.
            const STEPS: u32 = 20;
            for i in 1..=STEPS {
                let t = i as f32 / STEPS as f32;
                let x = x1 as f32 + (x2 as f32 - x1 as f32) * t;
                let y = y1 as f32 + (y2 as f32 - y1 as f32) * t;
                warp(x as u32, y as u32);
                queue.roundtrip(&mut State)?;
                sleep(Duration::from_millis(20));
            }

            sleep(Duration::from_millis(120));
            pointer.button(0, BTN_LEFT, wl_pointer::ButtonState::Released);
            pointer.frame();
        }
        other => {
            eprintln!("unknown command: {other}");
            std::process::exit(2);
        }
    }

    queue.roundtrip(&mut State)?;
    Ok(())
}
