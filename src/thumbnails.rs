//! Window thumbnails, for the tiles that stand for minimised windows.
//!
//! Contrary to what the protocol list suggests, this needs no PipeWire. KWin
//! exposes `org.kde.KWin.ScreenShot2.CaptureWindow`, which writes raw pixels
//! straight into a file descriptor the caller hands it and answers with the
//! geometry and format -- no stream to negotiate and no GPU buffer to import.
//!
//! Like the window protocol, it is gated on the desktop entry naming it:
//!
//! ```text
//! X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2
//! ```
//!
//! Without that the call fails with `NoAuthorized` and the tiles fall back to
//! their application's icon.
//!
//! KWin keeps a minimised window's last frame, so a capture taken after the
//! window is already hidden still returns its contents -- which is what makes
//! capturing lazily, when a tile first needs a picture, viable at all.

use std::{
    collections::HashMap,
    io::Read,
    os::{fd::AsFd, unix::net::UnixStream},
    sync::{Arc, Mutex},
};

use tiny_skia::{IntSize, Pixmap};
use zbus::zvariant::{Fd, OwnedValue, Value};

const KWIN_SERVICE: &str = "org.kde.KWin";
const SCREENSHOT_PATH: &str = "/org/kde/KWin/ScreenShot2";
const SCREENSHOT_IFACE: &str = "org.kde.KWin.ScreenShot2";

/// Longest edge a stored thumbnail keeps.
///
/// A tile is at most `large_size` logical pixels, doubled again on a HiDPI
/// output, so this leaves room to scale down from rather than up.
pub const THUMBNAIL_MAX: u32 = 256;

/// `QImage::Format_RGB32`, which carries no meaningful alpha.
const FORMAT_RGB32: u32 = 4;

/// A captured thumbnail, ready to draw.
#[derive(Debug, Clone)]
pub struct Thumbnail {
    pub pixmap: Arc<Pixmap>,
    /// Width divided by height, from the window's real size. The tile is sized
    /// from this so the picture is not distorted.
    pub aspect: f32,
}

/// Thumbnails captured so far, keyed by KWin's window uuid.
#[derive(Default)]
pub struct ThumbnailCache {
    ready: HashMap<String, Thumbnail>,
    /// Captures already asked for, so a redraw does not queue the same window
    /// again while its capture is still in flight.
    pending: Arc<Mutex<Vec<String>>>,
    /// Windows whose capture failed. Retrying every frame would hammer KWin
    /// with a call that is not going to start working.
    failed: Arc<Mutex<Vec<String>>>,
    connection: Option<zbus::blocking::Connection>,
}

impl ThumbnailCache {
    pub fn new(connection: Option<zbus::blocking::Connection>) -> Self {
        Self {
            connection,
            ..Default::default()
        }
    }

    pub fn get(&self, uuid: &str) -> Option<&Thumbnail> {
        self.ready.get(uuid)
    }

    pub fn insert(&mut self, uuid: String, thumbnail: Thumbnail) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.retain(|p| *p != uuid);
        }
        self.ready.insert(uuid, thumbnail);
    }

    /// Records that a capture came back empty-handed.
    pub fn mark_failed(&mut self, uuid: String) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.retain(|p| *p != uuid);
        }
        if let Ok(mut failed) = self.failed.lock() {
            if !failed.contains(&uuid) {
                failed.push(uuid);
            }
        }
    }

    /// Which captured windows are no longer wanted.
    pub fn keys_not_in(&self, wanted: &[String]) -> Vec<String> {
        self.ready
            .keys()
            .filter(|k| !wanted.contains(k))
            .cloned()
            .collect()
    }

    /// Drops everything about a window. Called when it stops being minimised,
    /// so the next minimise captures what the window looks like *then* rather
    /// than reusing a stale picture.
    pub fn forget(&mut self, uuid: &str) {
        self.ready.remove(uuid);
        if let Ok(mut failed) = self.failed.lock() {
            failed.retain(|f| f != uuid);
        }
    }

    /// Starts a capture unless one is already done, running, or known to fail.
    ///
    /// Capturing means a D-Bus round trip plus reading several megabytes, so it
    /// happens on its own thread; a redraw must not wait on it.
    pub fn request<F>(&self, uuid: &str, done: F)
    where
        F: FnOnce(String, Option<Thumbnail>) + Send + 'static,
    {
        if self.ready.contains_key(uuid) {
            return;
        }
        let Some(conn) = self.connection.clone() else {
            return;
        };
        {
            let Ok(mut pending) = self.pending.lock() else {
                return;
            };
            let already_failed = self
                .failed
                .lock()
                .map(|f| f.iter().any(|x| x == uuid))
                .unwrap_or(false);
            if already_failed || pending.iter().any(|p| p == uuid) {
                return;
            }
            pending.push(uuid.to_owned());
        }

        let uuid = uuid.to_owned();
        std::thread::spawn(move || {
            let thumbnail = capture(&conn, &uuid).map(|(pixmap, aspect)| Thumbnail {
                pixmap: Arc::new(pixmap),
                aspect,
            });
            done(uuid, thumbnail);
        });
    }
}

/// Captures one window and scales it down. Returns the thumbnail and the
/// window's real aspect ratio.
fn capture(conn: &zbus::blocking::Connection, uuid: &str) -> Option<(Pixmap, f32)> {
    // KWin writes into one end and this reads the other. A socket pair needs no
    // extra crate, and KWin only ever writes.
    let (mut reader, writer) = UnixStream::pair().ok()?;
    let options: HashMap<&str, Value> = HashMap::new();

    let reply = conn
        .call_method(
            Some(KWIN_SERVICE),
            SCREENSHOT_PATH,
            Some(SCREENSHOT_IFACE),
            "CaptureWindow",
            &(uuid, options, Fd::from(writer.as_fd())),
        )
        .ok()?;

    // Our copy of the write end has to go, or the read below never sees EOF:
    // it ends when *every* writer is gone, and KWin's is not the only one.
    drop(writer);

    let meta: HashMap<String, OwnedValue> = reply.body().deserialize().ok()?;
    let field = |k: &str| meta.get(k).and_then(|v| u32::try_from(v).ok());

    let width = field("width")?;
    let height = field("height")?;
    let stride = field("stride").unwrap_or(width * 4);
    let format = field("format").unwrap_or(0);
    if width == 0 || height == 0 {
        return None;
    }

    let mut raw = Vec::new();
    reader.read_to_end(&mut raw).ok()?;
    if raw.len() < (stride * height) as usize {
        return None;
    }

    let full = to_pixmap(&raw, width, height, stride, format)?;
    let aspect = width as f32 / height as f32;
    Some((downscale(&full, THUMBNAIL_MAX), aspect))
}

/// Repacks KWin's rows into a tiny-skia pixmap.
///
/// KWin hands back a `QImage` word of `0xAARRGGBB` in native byte order, so on
/// little-endian the bytes run B, G, R, A. tiny-skia wants R, G, B, A, already
/// premultiplied -- which `Format_ARGB32_Premultiplied` is, so only the channel
/// order changes.
fn to_pixmap(raw: &[u8], width: u32, height: u32, stride: u32, format: u32) -> Option<Pixmap> {
    let mut pixmap = Pixmap::new(width, height)?;
    let dst = pixmap.data_mut();

    for y in 0..height as usize {
        let row_start = y * stride as usize;
        let row = raw.get(row_start..row_start + width as usize * 4)?;
        for (x, px) in row.chunks_exact(4).enumerate() {
            let o = (y * width as usize + x) * 4;
            dst[o] = px[2];
            dst[o + 1] = px[1];
            dst[o + 2] = px[0];
            // Format_RGB32 leaves the alpha byte unset, and a zero there would
            // make the whole thumbnail invisible.
            dst[o + 3] = if format == FORMAT_RGB32 { 0xFF } else { px[3] };
        }
    }
    Some(pixmap)
}

/// Box-averages down to at most `max` on the longer edge.
///
/// A window is an order of magnitude larger than the tile it ends up in, and
/// sampling that down bilinearly drops most of the pixels on the floor, which
/// turns fine detail like text into aliased noise. Averaging every source pixel
/// that lands in a destination pixel is what keeps it readable.
fn downscale(src: &Pixmap, max: u32) -> Pixmap {
    let (sw, sh) = (src.width(), src.height());
    let longest = sw.max(sh);
    if longest <= max {
        return src.clone();
    }

    let scale = max as f32 / longest as f32;
    let dw = ((sw as f32 * scale).round() as u32).max(1);
    let dh = ((sh as f32 * scale).round() as u32).max(1);

    let Some(mut out) = Pixmap::new(dw, dh) else {
        return src.clone();
    };
    let s = src.data();
    let d = out.data_mut();

    for y in 0..dh as usize {
        let y0 = y * sh as usize / dh as usize;
        let y1 = (((y + 1) * sh as usize) / dh as usize).max(y0 + 1);
        for x in 0..dw as usize {
            let x0 = x * sw as usize / dw as usize;
            let x1 = (((x + 1) * sw as usize) / dw as usize).max(x0 + 1);

            let (mut r, mut g, mut b, mut a, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let o = (sy * sw as usize + sx) * 4;
                    r += s[o] as u32;
                    g += s[o + 1] as u32;
                    b += s[o + 2] as u32;
                    a += s[o + 3] as u32;
                    n += 1;
                }
            }
            let o = (y * dw as usize + x) * 4;
            d[o] = (r / n) as u8;
            d[o + 1] = (g / n) as u8;
            d[o + 2] = (b / n) as u8;
            d[o + 3] = (a / n) as u8;
        }
    }

    Pixmap::from_vec(d.to_vec(), IntSize::from_wh(dw, dh).unwrap()).unwrap_or(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// KWin's rows are BGRA; getting this wrong swaps red and blue in every
    /// thumbnail, which is obvious on screen but invisible in review.
    #[test]
    fn bgra_rows_become_rgba() {
        // One opaque red pixel, as KWin would send it.
        let raw = [0x00, 0x00, 0xFF, 0xFF];
        let px = to_pixmap(&raw, 1, 1, 4, 6).unwrap();
        assert_eq!(px.data(), &[0xFF, 0x00, 0x00, 0xFF]);
    }

    /// `Format_RGB32` carries a zero alpha byte; taking it at face value would
    /// make the thumbnail entirely transparent.
    #[test]
    fn rgb32_is_forced_opaque() {
        let raw = [0x10, 0x20, 0x30, 0x00];
        let px = to_pixmap(&raw, 1, 1, 4, FORMAT_RGB32).unwrap();
        assert_eq!(px.data()[3], 0xFF);
    }

    #[test]
    fn argb_keeps_its_alpha() {
        let raw = [0x10, 0x20, 0x30, 0x80];
        let px = to_pixmap(&raw, 1, 1, 4, 6).unwrap();
        assert_eq!(px.data()[3], 0x80);
    }

    /// Rows are padded to `stride`, which is not always `width * 4`.
    #[test]
    fn padding_between_rows_is_skipped() {
        // 1x2 image, 8-byte stride: 4 bytes of pixel then 4 of padding.
        let raw = [
            0x00, 0x00, 0xFF, 0xFF, 0xDE, 0xAD, 0xBE, 0xEF, // row 0 + padding
            0xFF, 0x00, 0x00, 0xFF, 0xDE, 0xAD, 0xBE, 0xEF, // row 1 + padding
        ];
        let px = to_pixmap(&raw, 1, 2, 8, 6).unwrap();
        assert_eq!(px.data()[0..4], [0xFF, 0x00, 0x00, 0xFF]);
        assert_eq!(px.data()[4..8], [0x00, 0x00, 0xFF, 0xFF]);
    }

    #[test]
    fn downscaling_keeps_the_aspect_ratio() {
        let src = Pixmap::new(1920, 1080).unwrap();
        let out = downscale(&src, 256);
        assert_eq!(out.width(), 256);
        assert_eq!(out.height(), 144);
    }

    #[test]
    fn an_already_small_image_is_left_alone() {
        let src = Pixmap::new(100, 50).unwrap();
        let out = downscale(&src, 256);
        assert_eq!((out.width(), out.height()), (100, 50));
    }

    /// Averaging, not sampling: a checkerboard must come out mid-grey rather
    /// than picking whichever pixel happened to land on the sample point.
    #[test]
    fn downscaling_averages_rather_than_samples() {
        let mut src = Pixmap::new(4, 4).unwrap();
        let d = src.data_mut();
        for i in 0..16 {
            let v = if (i / 4 + i % 4) % 2 == 0 { 0 } else { 255 };
            d[i * 4] = v;
            d[i * 4 + 1] = v;
            d[i * 4 + 2] = v;
            d[i * 4 + 3] = 255;
        }
        let out = downscale(&src, 1);
        assert_eq!((out.width(), out.height()), (1, 1));
        let grey = out.data()[0];
        assert!((120..=135).contains(&grey), "expected mid-grey, got {grey}");
    }
}
