//! Captures one window's contents through KWin, for checking the route works.
//!
//! KWin exposes `org.kde.KWin.ScreenShot2.CaptureWindow`, which writes raw
//! pixels into a file descriptor the caller supplies and answers with the
//! geometry and format. That is the whole mechanism -- no PipeWire, no format
//! negotiation, no GPU buffer import.
//!
//! ```text
//! window-thumbnail '{uuid}' out.png
//! ```
//!
//! KWin only allows this to callers whose desktop entry says so, so running it
//! straight from `target/` answers `NoAuthorized`. To try it, install an entry
//! whose `Exec=` is the absolute path to this binary and which carries
//! `X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2`.
//!
//! The uuid is KWin's internal window id, braces included; it is the same one
//! `org_kde_plasma_window` reports and the same one that appears inside the
//! `WindowsRunner` match ids as `0_{uuid}`.

use std::{
    collections::HashMap,
    io::Read,
    os::{fd::AsFd, unix::net::UnixStream},
};

use zbus::zvariant::{Fd, OwnedValue, Value};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let uuid = args
        .next()
        .ok_or("usage: window-thumbnail '{uuid}' out.png")?;
    let out = args.next().unwrap_or_else(|| "thumbnail.png".to_owned());

    // KWin writes the image into one end and we read the other. A socket pair
    // stands in for a pipe: it needs no extra crate and KWin only ever writes.
    let (mut reader, writer) = UnixStream::pair()?;

    let conn = zbus::blocking::Connection::session()?;
    let options: HashMap<&str, Value> = HashMap::new();

    let reply = conn.call_method(
        Some("org.kde.KWin"),
        "/org/kde/KWin/ScreenShot2",
        Some("org.kde.KWin.ScreenShot2"),
        "CaptureWindow",
        &(uuid.as_str(), options, Fd::from(writer.as_fd())),
    )?;

    // Dropping our copy of the write end matters: the read below only ends when
    // every writer is gone, and KWin holding one is not enough.
    drop(writer);

    let meta: HashMap<String, OwnedValue> = reply.body().deserialize()?;
    println!("reply: {:?}", meta.keys().collect::<Vec<_>>());

    let get = |k: &str| -> Option<u32> { meta.get(k).and_then(|v| u32::try_from(v).ok()) };
    let width = get("width").ok_or("no width in reply")?;
    let height = get("height").ok_or("no height in reply")?;
    let stride = get("stride").unwrap_or(width * 4);
    let format = get("format").unwrap_or(0);
    println!("{width}x{height} stride={stride} format={format}");

    let mut raw = Vec::new();
    reader.read_to_end(&mut raw)?;
    println!("read {} bytes (expected {})", raw.len(), stride * height);

    // KWin hands back QImage::Format_ARGB32_Premultiplied (format 6) or
    // Format_RGB32 (4): a native-endian 0xAARRGGBB word, so little-endian byte
    // order B,G,R,A. tiny-skia wants premultiplied R,G,B,A.
    let mut pixmap = tiny_skia::Pixmap::new(width, height).ok_or("could not allocate a pixmap")?;
    let dst = pixmap.data_mut();
    for y in 0..height as usize {
        let row = &raw[y * stride as usize..][..width as usize * 4];
        for (x, px) in row.as_chunks::<4>().0.iter().enumerate() {
            let o = (y * width as usize + x) * 4;
            dst[o] = px[2];
            dst[o + 1] = px[1];
            dst[o + 2] = px[0];
            // Format_RGB32 leaves the alpha byte unset; treat it as opaque.
            dst[o + 3] = if format == 4 { 0xFF } else { px[3] };
        }
    }

    pixmap.save_png(&out)?;
    println!("wrote {out}");
    Ok(())
}
