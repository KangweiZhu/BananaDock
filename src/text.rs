//! Text rendering.
//!
//! Everything else in the dock is drawn as vector paths and scaled through
//! tiny-skia's transform, but glyphs are rasterised bitmaps: shaping and
//! hinting depend on the pixel size they are rendered at, so a glyph run cannot
//! be drawn once and scaled up without going soft. Text is therefore sized and
//! positioned in *device* pixels here, and callers multiply by the output scale
//! before calling in -- the one place in the codebase where that conversion is
//! the caller's job rather than the transform's.

use cosmic_text::{Attrs, Buffer, FontSystem, Metrics as TextMetrics, Shaping, SwashCache};
use tiny_skia::Pixmap;

use crate::metrics::Color;

/// Line height as a multiple of the font size. Menu rows set their own height,
/// so this only has to be loose enough not to clip ascenders and descenders.
const LINE_HEIGHT_RATIO: f32 = 1.25;

pub struct TextRenderer {
    font_system: FontSystem,
    cache: SwashCache,
}

impl Default for TextRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl TextRenderer {
    /// Loads the system fonts.
    ///
    /// This walks the font directories and is slow enough to be worth doing
    /// once at start-up rather than on the first menu.
    pub fn new() -> Self {
        Self {
            font_system: FontSystem::new(),
            cache: SwashCache::new(),
        }
    }

    fn shape(&mut self, text: &str, px: f32) -> Buffer {
        let mut buffer = Buffer::new(
            &mut self.font_system,
            TextMetrics::new(px, px * LINE_HEIGHT_RATIO),
        );
        let mut borrowed = buffer.borrow_with(&mut self.font_system);
        // No wrapping: menu rows are one line and the menu is sized to fit.
        borrowed.set_size(None, None);
        borrowed.set_text(text, &Attrs::new(), Shaping::Advanced, None);
        borrowed.shape_until_scroll(false);
        buffer
    }

    /// Width of `text` in device pixels at a device-pixel font size.
    pub fn measure(&mut self, text: &str, px: f32) -> f32 {
        let buffer = self.shape(text, px);
        buffer
            .layout_runs()
            .map(|run| run.line_w)
            .fold(0.0, f32::max)
    }

    /// Draws `text` with its left edge at `x` and the top of its line box at
    /// `y`, both in device pixels.
    pub fn draw(&mut self, pixmap: &mut Pixmap, text: &str, px: f32, x: f32, y: f32, color: Color) {
        let mut buffer = self.shape(text, px);
        let (ox, oy) = (x.round() as i32, y.round() as i32);
        let (pw, ph) = (pixmap.width() as i32, pixmap.height() as i32);
        let (r, g, b) = (
            (color.r * 255.0) as u8,
            (color.g * 255.0) as u8,
            (color.b * 255.0) as u8,
        );
        let text_alpha = color.a.clamp(0.0, 1.0);

        let data = pixmap.data_mut();
        let mut borrowed = buffer.borrow_with(&mut self.font_system);
        borrowed.draw(
            &mut self.cache,
            cosmic_text::Color::rgba(r, g, b, 255),
            |gx, gy, gw, gh, gcolor| {
                // The callback's alpha is the glyph's coverage; the requested
                // colour's own alpha multiplies on top of it.
                let coverage = gcolor.a() as f32 / 255.0 * text_alpha;
                if coverage <= 0.0 {
                    return;
                }
                let src = (gcolor.r(), gcolor.g(), gcolor.b());

                for row in 0..gh as i32 {
                    let py = oy + gy + row;
                    if py < 0 || py >= ph {
                        continue;
                    }
                    for col in 0..gw as i32 {
                        let pxx = ox + gx + col;
                        if pxx < 0 || pxx >= pw {
                            continue;
                        }
                        let i = ((py * pw + pxx) * 4) as usize;
                        blend_over(&mut data[i..i + 4], src, coverage);
                    }
                }
            },
        );
    }
}

/// Source-over of a straight-alpha colour onto a premultiplied pixel.
fn blend_over(dst: &mut [u8], src: (u8, u8, u8), alpha: f32) {
    let inv = 1.0 - alpha;
    for (i, s) in [src.0, src.1, src.2].into_iter().enumerate() {
        // The source is premultiplied on the way in, matching the destination.
        dst[i] = (s as f32 * alpha + dst[i] as f32 * inv)
            .round()
            .clamp(0.0, 255.0) as u8;
    }
    dst[3] = (alpha * 255.0 + dst[3] as f32 * inv)
        .round()
        .clamp(0.0, 255.0) as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blending_onto_transparent_yields_the_premultiplied_source() {
        let mut px = [0u8; 4];
        blend_over(&mut px, (255, 255, 255), 0.5);
        assert_eq!(px[3], 128);
        // Premultiplied: colour must not exceed alpha.
        assert!(px[0] <= px[3] + 1, "{} vs {}", px[0], px[3]);
    }

    #[test]
    fn full_coverage_replaces_the_destination() {
        let mut px = [10, 20, 30, 40];
        blend_over(&mut px, (255, 0, 0), 1.0);
        assert_eq!(px, [255, 0, 0, 255]);
    }

    #[test]
    fn zero_coverage_leaves_the_destination_alone() {
        let mut px = [10, 20, 30, 40];
        blend_over(&mut px, (255, 255, 255), 0.0);
        assert_eq!(px, [10, 20, 30, 40]);
    }

    /// Loading the system fonts and shaping a string is the part most likely to
    /// break on a machine with an unusual font setup, so exercise it directly.
    #[test]
    fn measuring_gives_a_sensible_width() {
        let mut t = TextRenderer::new();
        let narrow = t.measure("i", 16.0);
        let wide = t.measure("Show All Windows", 16.0);

        assert!(narrow > 0.0, "a glyph should have width");
        assert!(
            wide > narrow * 5.0,
            "a long string should be much wider: {wide} vs {narrow}"
        );
    }

    #[test]
    fn width_grows_with_font_size() {
        let mut t = TextRenderer::new();
        assert!(t.measure("Quit", 24.0) > t.measure("Quit", 12.0));
    }

    #[test]
    fn an_empty_string_measures_zero() {
        let mut t = TextRenderer::new();
        assert_eq!(t.measure("", 16.0), 0.0);
    }

    /// Drawing must actually put ink on the pixmap -- a silent no-op here would
    /// show up as an empty menu and nothing else.
    #[test]
    fn drawing_marks_the_pixmap() {
        let mut t = TextRenderer::new();
        let mut pixmap = Pixmap::new(200, 40).unwrap();
        t.draw(
            &mut pixmap,
            "Quit",
            16.0,
            4.0,
            4.0,
            Color::rgba(1.0, 1.0, 1.0, 1.0),
        );

        assert!(
            pixmap.pixels().iter().any(|p| p.alpha() > 0),
            "no glyphs were drawn"
        );
    }

    /// Text placed off the edge must clip rather than panic on an out-of-range
    /// index.
    #[test]
    fn drawing_outside_the_pixmap_is_clipped() {
        let mut t = TextRenderer::new();
        let mut pixmap = Pixmap::new(40, 20).unwrap();
        t.draw(
            &mut pixmap,
            "Quit",
            16.0,
            -100.0,
            -100.0,
            Color::rgba(1.0, 1.0, 1.0, 1.0),
        );
        t.draw(
            &mut pixmap,
            "Quit",
            16.0,
            500.0,
            500.0,
            Color::rgba(1.0, 1.0, 1.0, 1.0),
        );
    }
}
