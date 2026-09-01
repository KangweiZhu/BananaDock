//! Finding, decoding and caching application icons.
//!
//! Icons are rasterised once at a fixed high resolution and scaled down when
//! drawn, rather than re-rasterised per size. Magnification changes an icon's
//! size every frame, so a size-keyed cache would grow without bound and miss
//! constantly; one high-resolution copy plus a bilinear downscale is both
//! cheaper and what the Qt version effectively did.

use std::{collections::HashMap, path::Path};

use tiny_skia::{IntSize, Pixmap};

/// Resolution icons are cached at. Chosen to cover the largest size a fully
/// magnified icon reaches, rounded up to a size icon themes actually ship.
pub const RENDER_SIZE: u32 = 128;

#[derive(Default)]
pub struct IconCache {
    /// `None` records a lookup that already failed, so a missing icon is not
    /// searched for again on every frame.
    cache: HashMap<String, Option<Pixmap>>,
    theme: String,
}

impl IconCache {
    /// `override_theme` comes from the config; without one the desktop's own
    /// setting is used.
    pub fn new(override_theme: Option<String>) -> Self {
        let theme = override_theme
            .filter(|t| !t.is_empty())
            .unwrap_or_else(detect_icon_theme);
        Self {
            cache: HashMap::new(),
            theme,
        }
    }

    pub fn theme(&self) -> &str {
        &self.theme
    }

    /// Looks up an icon by its `Icon=` value, which may be a bare theme name or
    /// an absolute path.
    pub fn get(&mut self, name: &str) -> Option<&Pixmap> {
        if !self.cache.contains_key(name) {
            let loaded = load_icon(name, &self.theme);
            self.cache.insert(name.to_owned(), loaded);
        }
        self.cache.get(name).and_then(Option::as_ref)
    }
}

/// Whichever icon theme the session is set to.
///
/// There is no cross-desktop API for this, so the usual sources are tried in
/// turn: an explicit override, then KDE's config, then GTK's. `hicolor` is the
/// spec-mandated fallback every theme inherits, but on its own it yields very
/// few icons, so reaching it generally means the others were absent.
fn detect_icon_theme() -> String {
    if let Some(t) = std::env::var_os("XDG_ICON_THEME") {
        if let Some(t) = t.to_str().filter(|s| !s.is_empty()) {
            return t.to_owned();
        }
    }

    let config = crate::config::config_home();

    if let Some(t) = config
        .as_ref()
        .and_then(|c| ini_value(&c.join("kdeglobals"), "Icons", "Theme"))
    {
        return t;
    }

    if let Some(t) = config.as_ref().and_then(|c| {
        ini_value(
            &c.join("gtk-3.0/settings.ini"),
            "Settings",
            "gtk-icon-theme-name",
        )
    }) {
        return t;
    }

    "hicolor".to_owned()
}

/// Minimal INI lookup: finds `key` inside `[group]`.
fn ini_value(path: &Path, group: &str, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut in_group = false;

    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_group = name == group;
            continue;
        }
        if !in_group {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            if k.trim() == key {
                let v = v.trim();
                if !v.is_empty() {
                    return Some(v.to_owned());
                }
            }
        }
    }
    None
}

fn load_icon(name: &str, theme: &str) -> Option<Pixmap> {
    // `Icon=` is allowed to be an absolute path rather than a theme name.
    let path = if name.starts_with('/') {
        let p = std::path::PathBuf::from(name);
        p.exists().then_some(p)
    } else {
        freedesktop_icons::lookup(name)
            .with_size(RENDER_SIZE as u16)
            .with_theme(theme)
            .find()
            // Themes are required to inherit hicolor, but not every installation
            // gets that right, so ask for it explicitly before giving up.
            .or_else(|| {
                freedesktop_icons::lookup(name)
                    .with_size(RENDER_SIZE as u16)
                    .find()
            })
    }?;

    let data = std::fs::read(&path).ok()?;

    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"))
    {
        render_svg(&data)
    } else {
        decode_raster(&data)
    }
}

fn render_svg(data: &[u8]) -> Option<Pixmap> {
    let tree = resvg::usvg::Tree::from_data(data, &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = Pixmap::new(RENDER_SIZE, RENDER_SIZE)?;

    // Fit the drawing into the square without distorting it.
    let size = tree.size();
    let scale = (RENDER_SIZE as f32 / size.width()).min(RENDER_SIZE as f32 / size.height());
    let dx = (RENDER_SIZE as f32 - size.width() * scale) / 2.0;
    let dy = (RENDER_SIZE as f32 - size.height() * scale) / 2.0;

    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale).post_translate(dx, dy),
        &mut pixmap.as_mut(),
    );
    Some(pixmap)
}

fn decode_raster(data: &[u8]) -> Option<Pixmap> {
    let img = image::load_from_memory(data).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    pixmap_from_straight_rgba(img.into_raw(), w, h)
}

/// `image` hands back straight (non-premultiplied) alpha; tiny-skia's `Pixmap`
/// is premultiplied throughout. Skipping this step leaves translucent icon
/// edges too bright -- a halo that is easy to miss and hard to trace back.
fn pixmap_from_straight_rgba(mut data: Vec<u8>, w: u32, h: u32) -> Option<Pixmap> {
    for px in data.as_chunks_mut::<4>().0 {
        let a = px[3] as u32;
        px[0] = ((px[0] as u32 * a + 127) / 255) as u8;
        px[1] = ((px[1] as u32 * a + 127) / 255) as u8;
        px[2] = ((px[2] as u32 * a + 127) / 255) as u8;
    }
    Pixmap::from_vec(data, IntSize::from_wh(w, h)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiply_scales_colour_by_alpha() {
        // Half-transparent white must become half-intensity.
        let px = pixmap_from_straight_rgba(vec![255, 255, 255, 128], 1, 1).unwrap();
        let d = px.data();
        assert_eq!(d[3], 128);
        assert!((d[0] as i32 - 128).abs() <= 1, "got {}", d[0]);
    }

    #[test]
    fn fully_transparent_pixels_lose_their_colour() {
        let px = pixmap_from_straight_rgba(vec![255, 0, 0, 0], 1, 1).unwrap();
        assert_eq!(px.data(), &[0, 0, 0, 0]);
    }

    #[test]
    fn opaque_pixels_are_untouched() {
        let px = pixmap_from_straight_rgba(vec![10, 20, 30, 255], 1, 1).unwrap();
        assert_eq!(px.data(), &[10, 20, 30, 255]);
    }

    #[test]
    fn ini_lookup_reads_the_right_group() {
        let dir = std::env::temp_dir().join("bananadock-ini-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("kdeglobals");
        std::fs::write(
            &path,
            "[Other]\nTheme=wrong\n\n[Icons]\nTheme=breeze-dark\n",
        )
        .unwrap();

        assert_eq!(
            ini_value(&path, "Icons", "Theme").as_deref(),
            Some("breeze-dark")
        );
        assert_eq!(ini_value(&path, "Icons", "Missing"), None);
        std::fs::remove_file(&path).ok();
    }
}
