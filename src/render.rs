//! Drawing the dock into a pixmap.
//!
//! Everything is drawn in surface-local logical pixels with the origin at the
//! surface's top-left. The surface is much taller than the panel (magnified
//! icons grow upwards into the headroom), so the panel is positioned against
//! the *bottom* of the pixmap rather than the top.

use tiny_skia::{
    FillRule, FilterQuality, Paint, PathBuilder, Pattern, Pixmap, Rect, SpreadMode, Stroke,
    Transform,
};

use crate::{
    icons::IconCache,
    layout::Layout,
    menu::{MenuItem, MenuLayout},
    metrics::{Metrics, Palette},
    model::{Slot, SlotKind},
    text::TextRenderer,
    thumbnails::ThumbnailCache,
};

/// What is being drawn into, and at what resolution.
///
/// `pixmap` is `logical * scale` pixels; `logical` is the surface's size in
/// logical pixels, which is the space all the geometry is expressed in.
pub struct Target<'a> {
    pub pixmap: &'a mut Pixmap,
    pub logical: (f32, f32),
    pub scale: f32,
    /// How far down the panel is pushed for the auto-hide slide, in logical
    /// pixels. Zero when fully revealed.
    pub offset_y: f32,
}

/// What to draw: the row itself and everything needed to paint it.
pub struct Scene<'a> {
    pub slots: &'a [Slot],
    pub layout: &'a Layout,
    pub icons: &'a mut IconCache,
    pub thumbnails: &'a ThumbnailCache,
    /// Tile a drag is currently hovering, if any.
    pub drop_target: Option<usize>,
}

/// Where the panel sits inside the surface.
///
/// The panel is centred horizontally and pinned to the bottom edge, leaving
/// `panel_bottom_gap` between it and the screen edge.
pub fn panel_rect(surface_w: f32, surface_h: f32, metrics: &Metrics, content_w: f32) -> Rect {
    let w = content_w + metrics.pt(metrics.panel_padding_h) * 2.0;
    let h = metrics.pt(metrics.panel_height());
    let x = ((surface_w - w) / 2.0).max(0.0);
    let y = surface_h - metrics.pt(metrics.panel_bottom_gap) - h;

    Rect::from_xywh(x, y, w, h).expect("panel rect is non-degenerate")
}

/// A rounded rectangle. With `radius == height / 2` this is a capsule: the end
/// caps' two quarter-arcs join into a semicircle.
///
/// Built from cubic Béziers because tiny-skia's `PathBuilder` has no arc
/// primitive. `KAPPA` is the standard control-point offset for approximating a
/// quarter circle with a cubic.
pub fn rounded_rect(rect: Rect, radius: f32) -> Option<tiny_skia::Path> {
    const KAPPA: f32 = 0.552_284_8;

    let (x, y, w, h) = (rect.x(), rect.y(), rect.width(), rect.height());
    // A radius past half the shorter side would make opposite caps overlap.
    let r = radius.clamp(0.0, w.min(h) / 2.0);
    let k = r * KAPPA;

    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

/// Approximates a capsule with axis-aligned rectangles, for a `wl_region`.
///
/// A `wl_region` is a set of rectangles, so a rounded shape has to be built out
/// of horizontal slices. Without this the blur behind the dock would be a plain
/// rectangle and would visibly stick out past the panel's rounded ends.
///
/// Each slice is inset by the horizontal distance from the circle's centre
/// measured at the slice edge *nearest the corner*, which keeps the region
/// entirely inside the capsule. Insetting at the far edge instead would make it
/// bulge past the outline, and blur leaking beyond the panel is far more
/// obvious than blur falling a fraction short of it.
///
/// Ported from the Qt implementation's `BlurEffect::setCapsuleBlurRegion`
/// (`git show ee6971b:src/blureffect.cpp`).
pub fn capsule_region(rect: Rect, radius: f32, slices: u32) -> Vec<(i32, i32, i32, i32)> {
    // A radius beyond half the shorter side would make opposite caps overlap.
    let r = radius.clamp(0.0, rect.width().min(rect.height()) / 2.0);

    if r <= 0.5 || slices == 0 {
        return vec![(
            rect.x().round() as i32,
            rect.y().round() as i32,
            rect.width().round() as i32,
            rect.height().round() as i32,
        )];
    }

    let mut out = Vec::with_capacity(slices as usize * 2 + 1);

    // Every horizontal edge is rounded once and shared verbatim by the
    // rectangles on either side of it. Rounding a rectangle's position and its
    // height separately instead leaves one-pixel gaps between slices whenever
    // the slice pitch is fractional, and every such gap is a row the
    // compositor leaves unblurred -- a hairline across the panel.
    let band_top = (rect.y() + r).round() as i32;
    let band_bottom = (rect.bottom() - r).round() as i32;

    // Middle band: full width, between the two caps. Absent when the caps meet
    // in the middle, which is exactly the case for the panel's own capsule.
    if band_bottom > band_top {
        out.push((
            rect.x().round() as i32,
            band_top,
            rect.width().round() as i32,
            band_bottom - band_top,
        ));
    }

    for i in 0..slices {
        let y0 = r * i as f32 / slices as f32;
        let y1 = r * (i + 1) as f32 / slices as f32;

        let dy = r - y0;
        let inset = r - (r * r - dy * dy).max(0.0).sqrt();

        let x0 = (rect.x() + inset).round() as i32;
        let x1 = (rect.right() - inset).round() as i32;
        if x1 <= x0 {
            continue;
        }

        // Top cap slice, and its mirror in the bottom cap. A slice thinner
        // than a pixel collapses to nothing here; its row belongs to a
        // neighbour that shares the edge, so nothing is left uncovered.
        let t0 = (rect.y() + y0).round() as i32;
        let t1 = (rect.y() + y1).round() as i32;
        if t1 > t0 {
            out.push((x0, t0, x1 - x0, t1 - t0));
        }

        let b0 = (rect.bottom() - y1).round() as i32;
        let b1 = (rect.bottom() - y0).round() as i32;
        if b1 > b0 {
            out.push((x0, b0, x1 - x0, b1 - b0));
        }
    }

    out
}

/// Draws a complete dock frame, returning where the panel landed -- the caller
/// needs that rectangle for the input and blur regions.
///
/// All geometry is computed in *logical* pixels and only rasterisation happens
/// at the output's real resolution, via `scale`. Keeping it this way means the
/// pointer, the input region and the viewport all speak the same coordinates as
/// the layout; folding the output scale into the geometry instead would make
/// every one of those a separate conversion waiting to be got wrong.
///
/// `pixmap` is therefore `logical * scale` pixels, while `logical` and the
/// returned rectangle are in logical pixels.
pub fn draw_dock(
    target: Target<'_>,
    metrics: &Metrics,
    palette: &Palette,
    scene: Scene<'_>,
) -> Rect {
    let Target {
        pixmap,
        logical,
        scale,
        offset_y,
    } = target;
    let Scene {
        slots,
        layout,
        icons,
        thumbnails,
        drop_target,
    } = scene;
    let t = Transform::from_scale(scale, scale);

    // An empty row still gets a panel, so the dock does not blink out of
    // existence when the last window closes.
    let content_w = if layout.content_width > 0.0 {
        layout.content_width
    } else {
        metrics.pt(metrics.tile_size)
    };
    let panel = panel_rect(logical.0, logical.1, metrics, content_w);
    // Sliding out is a pure translation: the panel keeps its shape and simply
    // leaves the screen, so the input region can follow the same rectangle.
    let panel = Rect::from_xywh(
        panel.x(),
        panel.y() + offset_y,
        panel.width(),
        panel.height(),
    )
    .unwrap_or(panel);
    draw_panel(pixmap, t, metrics, palette, panel);

    let row_x = panel.x() + metrics.pt(metrics.panel_padding_h);
    // The dot sits against the panel, not against the icon, so it stays put
    // while the icon above it grows.
    let dot_row = panel.bottom();

    for (i, (slot, geom)) in slots.iter().zip(&layout.slots).enumerate() {
        let centre_x = row_x + geom.centre();

        // A drag hovering this tile: show where the files would land.
        if drop_target == Some(i) && slot.kind != SlotKind::Separator {
            let pad = geom.width * 0.08;
            if let Some(rect) = Rect::from_xywh(
                row_x + geom.x + pad,
                panel.y() + pad,
                geom.width - pad * 2.0,
                panel.height() - pad * 2.0,
            ) {
                if let Some(path) = rounded_rect(rect, metrics.pt(metrics.menu_highlight_radius)) {
                    let mut paint = Paint {
                        anti_alias: true,
                        ..Default::default()
                    };
                    paint.set_color(palette.drop_target.to_skia());
                    pixmap.fill_path(&path, &paint, FillRule::Winding, t, None);
                }
            }
        }

        if slot.kind == SlotKind::Separator {
            draw_separator(pixmap, t, metrics, palette, centre_x, panel);
            continue;
        }

        // A slot's width is its pitch; the artwork is a fixed fraction of it,
        // so magnifying the slot magnifies the icon with it.
        let icon_px = geom.width * metrics.icon_size_ratio;

        // Icons are bottom-aligned and grow upwards when magnified, as on macOS.
        // The launch bounce lifts the artwork only -- the dot stays on the
        // panel, as it does on macOS.
        let icon_bottom = panel.bottom() - metrics.pt(metrics.icon_bottom_margin()) - geom.lift;

        // A minimised window shows the window, not its application's icon --
        // that is the whole point of giving it a tile of its own. The picture
        // arrives asynchronously, so the icon stands in until it does.
        let thumbnail = slot
            .capture_key
            .as_deref()
            .filter(|_| slot.kind == SlotKind::MinimizedWindow)
            .and_then(|key| thumbnails.get(key));

        if let Some(thumb) = thumbnail {
            // Fitted inside the icon's box, keeping the window's proportions:
            // stretching a window to a square makes it unrecognisable.
            let (w, h) = if thumb.aspect >= 1.0 {
                (icon_px, icon_px / thumb.aspect)
            } else {
                (icon_px * thumb.aspect, icon_px)
            };
            draw_image(
                pixmap,
                t,
                &thumb.pixmap,
                centre_x - w / 2.0,
                icon_bottom - (icon_px + h) / 2.0,
                w,
                h,
            );
        } else if let Some(name) = slot.icon_name.as_deref() {
            if let Some(art) = icons.get(name) {
                draw_icon(
                    pixmap,
                    t,
                    art,
                    centre_x - icon_px / 2.0,
                    icon_bottom - icon_px,
                    icon_px,
                );
            }
        }

        // The dot marks a running *application*. A minimised window's own tile
        // is not an application, and macOS gives it no dot.
        if slot.kind == SlotKind::App && slot.is_running() {
            draw_dot(pixmap, t, metrics, palette, centre_x, dot_row);
        }
    }

    panel
}

/// Draws the right-click menu.
///
/// Text is the one thing here that cannot go through the canvas transform --
/// glyphs are rasterised at a fixed pixel size -- so labels are sized and
/// placed in device pixels while the shapes around them stay logical.
pub fn draw_menu(
    target: Target<'_>,
    metrics: &Metrics,
    palette: &Palette,
    items: &[MenuItem],
    layout: &MenuLayout,
    highlighted: Option<usize>,
    text: &mut TextRenderer,
) {
    let Target {
        pixmap,
        logical,
        scale,
        ..
    } = target;
    let t = Transform::from_scale(scale, scale);

    let Some(bounds) = Rect::from_xywh(0.0, 0.0, logical.0, logical.1) else {
        return;
    };
    let radius = metrics.pt(metrics.menu_radius);

    if let Some(path) = rounded_rect(bounds, radius) {
        let mut paint = Paint {
            anti_alias: true,
            ..Default::default()
        };
        paint.set_color(palette.menu_background.to_skia());
        pixmap.fill_path(&path, &paint, FillRule::Winding, t, None);
    }
    // Inset by half the stroke so the hairline sits inside the menu.
    if let Some(inner) = Rect::from_xywh(0.5, 0.5, logical.0 - 1.0, logical.1 - 1.0) {
        if let Some(path) = rounded_rect(inner, radius - 0.5) {
            let mut paint = Paint {
                anti_alias: true,
                ..Default::default()
            };
            paint.set_color(palette.menu_border.to_skia());
            pixmap.stroke_path(
                &path,
                &paint,
                &Stroke {
                    width: 1.0,
                    ..Default::default()
                },
                t,
                None,
            );
        }
    }

    let pad = metrics.pt(metrics.menu_item_padding);
    let font_px = metrics.pt(metrics.menu_font_size) * scale;

    for (i, (item, &(row_top, row_h))) in items.iter().zip(&layout.rows).enumerate() {
        if item.is_separator() {
            let y = row_top + row_h / 2.0;
            if let Some(line) = Rect::from_xywh(pad, y, logical.0 - pad * 2.0, 1.0) {
                let mut paint = Paint {
                    anti_alias: false,
                    ..Default::default()
                };
                paint.set_color(palette.menu_separator.to_skia());
                pixmap.fill_rect(line, &paint, t, None);
            }
            continue;
        }

        let selected = highlighted == Some(i);
        if selected {
            let inset = metrics.pt(metrics.menu_highlight_inset);
            if let Some(hl) = Rect::from_xywh(inset, row_top, logical.0 - inset * 2.0, row_h) {
                if let Some(path) = rounded_rect(hl, metrics.pt(metrics.menu_highlight_radius)) {
                    let mut paint = Paint {
                        anti_alias: true,
                        ..Default::default()
                    };
                    paint.set_color(palette.menu_highlight.to_skia());
                    pixmap.fill_path(&path, &paint, FillRule::Winding, t, None);
                }
            }
        }

        let colour = if selected {
            palette.menu_highlight_text
        } else {
            palette.menu_text
        };
        // Centre the line box in the row, then convert to device pixels.
        let line_h = font_px * 1.25;
        let baseline_top = (row_top + row_h / 2.0) * scale - line_h / 2.0;

        if item.checked {
            text.draw(
                pixmap,
                "✓",
                font_px,
                pad * scale * 0.4,
                baseline_top,
                colour,
            );
        }
        text.draw(
            pixmap,
            &item.label,
            font_px,
            (pad * 2.0) * scale,
            baseline_top,
            colour,
        );
    }
}

/// Blits a cached icon scaled to `size`.
///
/// A `Pattern` shader is used rather than `draw_pixmap` because the latter only
/// takes integer coordinates; magnification moves icons by fractions of a pixel
/// and rounding that away makes the row visibly judder.
fn draw_icon(pixmap: &mut Pixmap, t: Transform, art: &Pixmap, x: f32, y: f32, size: f32) {
    let Some(dest) = Rect::from_xywh(x, y, size, size) else {
        return;
    };
    // The pattern's own transform is in the same logical space as `dest`; the
    // canvas transform then takes both to device pixels.
    let fit = size / art.width() as f32;

    let paint = Paint {
        shader: Pattern::new(
            art.as_ref(),
            SpreadMode::Pad,
            FilterQuality::Bilinear,
            1.0,
            Transform::from_scale(fit, fit).post_translate(x, y),
        ),
        anti_alias: true,
        ..Default::default()
    };
    pixmap.fill_rect(dest, &paint, t, None);
}

/// Blits an image into an arbitrary rectangle.
///
/// Unlike [`draw_icon`] the source need not be square, so the two axes scale
/// independently -- the caller has already worked out a width and height that
/// preserve the source's proportions.
fn draw_image(pixmap: &mut Pixmap, t: Transform, art: &Pixmap, x: f32, y: f32, w: f32, h: f32) {
    let Some(dest) = Rect::from_xywh(x, y, w, h) else {
        return;
    };
    let (sx, sy) = (w / art.width() as f32, h / art.height() as f32);

    let paint = Paint {
        shader: Pattern::new(
            art.as_ref(),
            SpreadMode::Pad,
            FilterQuality::Bilinear,
            1.0,
            Transform::from_scale(sx, sy).post_translate(x, y),
        ),
        anti_alias: true,
        ..Default::default()
    };
    pixmap.fill_rect(dest, &paint, t, None);
}

/// The hairline that fences the Trash off from the applications.
fn draw_separator(
    pixmap: &mut Pixmap,
    t: Transform,
    metrics: &Metrics,
    palette: &Palette,
    centre_x: f32,
    panel: Rect,
) {
    let w = metrics.pt(metrics.separator_line_width);
    let inset = metrics.pt(metrics.separator_inset);
    let h = panel.height() - inset * 2.0;
    if h <= 0.0 {
        return;
    }

    let Some(rect) = Rect::from_xywh(centre_x - w / 2.0, panel.y() + inset, w, h) else {
        return;
    };
    // Rounded so the line does not read as a hard-edged bar at this width.
    let Some(path) = rounded_rect(rect, w / 2.0) else {
        return;
    };

    let mut paint = Paint {
        anti_alias: true,
        ..Default::default()
    };
    paint.set_color(palette.separator.to_skia());
    pixmap.fill_path(&path, &paint, FillRule::Winding, t, None);
}

/// The dot under a running application.
fn draw_dot(
    pixmap: &mut Pixmap,
    t: Transform,
    metrics: &Metrics,
    palette: &Palette,
    centre_x: f32,
    panel_bottom: f32,
) {
    let d = metrics.pt(metrics.dot_size);
    let cy = panel_bottom - metrics.pt(metrics.dot_bottom_margin);

    let Some(path) = PathBuilder::from_circle(centre_x, cy, d / 2.0) else {
        return;
    };
    let mut paint = Paint {
        anti_alias: true,
        ..Default::default()
    };
    paint.set_color(palette.dot.to_skia());
    pixmap.fill_path(&path, &paint, FillRule::Winding, t, None);
}

/// Paints the panel's translucent tint and its highlight stroke.
///
/// The frosted glass itself is the compositor's job -- this only layers the
/// material on top of whatever the blur produced.
pub fn draw_panel(
    pixmap: &mut Pixmap,
    t: Transform,
    metrics: &Metrics,
    palette: &Palette,
    panel: Rect,
) {
    let radius = metrics.pt(metrics.panel_radius());

    if let Some(path) = rounded_rect(panel, radius) {
        let mut paint = Paint {
            anti_alias: true,
            ..Default::default()
        };
        paint.set_color(palette.panel_tint.to_skia());
        pixmap.fill_path(&path, &paint, FillRule::Winding, t, None);
    }

    // Stroking centres the line on the path, so inset by half the width to keep
    // the hairline fully inside the capsule instead of straddling its edge.
    let bw = metrics.pt(palette.panel_border_width);
    let inset = bw / 2.0;
    let Some(inner) = Rect::from_xywh(
        panel.x() + inset,
        panel.y() + inset,
        panel.width() - bw,
        panel.height() - bw,
    ) else {
        return;
    };

    if let Some(path) = rounded_rect(inner, radius - inset) {
        let mut paint = Paint {
            anti_alias: true,
            ..Default::default()
        };
        paint.set_color(palette.panel_border.to_skia());
        let stroke = Stroke {
            width: bw,
            ..Default::default()
        };
        pixmap.stroke_path(&path, &paint, &stroke, t, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_is_centred_and_sits_above_the_screen_edge() {
        let m = Metrics::default();
        let surface_h = m.surface_height();
        let rect = panel_rect(1000.0, surface_h, &m, 400.0);

        let expected_w = 400.0 + m.pt(m.panel_padding_h) * 2.0;
        assert!((rect.width() - expected_w).abs() < 0.01);
        // Centred.
        assert!((rect.x() - (1000.0 - expected_w) / 2.0).abs() < 0.01);
        // The gap below the panel is preserved.
        assert!((surface_h - rect.bottom() - m.pt(m.panel_bottom_gap)).abs() < 0.01);
    }

    /// A capsule radius must not be clamped away -- panel_radius is exactly half
    /// the height, which is the boundary case of the clamp in `rounded_rect`.
    #[test]
    fn capsule_path_survives_a_half_height_radius() {
        let rect = Rect::from_xywh(0.0, 0.0, 200.0, 80.0).unwrap();
        let path = rounded_rect(rect, 40.0).expect("capsule path");
        assert!((path.bounds().width() - 200.0).abs() < 0.01);
        assert!((path.bounds().height() - 80.0).abs() < 0.01);
    }

    fn slot(running: bool) -> Slot {
        Slot {
            capture_key: None,
            kind: SlotKind::App,
            key: "test".into(),
            label: "test".into(),
            icon_name: None,
            windows: if running { vec![1] } else { vec![] },
            active: false,
            pinned: true,
        }
    }

    /// Reads the pixel at the dot's centre for a running and a non-running
    /// slot. The dot is small and easy to knock out of the panel entirely by a
    /// sign error in the vertical maths, which no compile check would catch.
    /// Renders one slot and returns the pixmap plus the panel rectangle.
    fn render_one_kind(kind: SlotKind, running: bool, scale: f32) -> (Pixmap, Rect, Metrics) {
        let m = Metrics::default();
        let (lw, lh) = (400.0, m.surface_height());
        let mut pixmap = Pixmap::new((lw * scale) as u32, (lh * scale).ceil() as u32).unwrap();
        let mut icons = IconCache::default();

        let slots = [Slot {
            kind,
            ..slot(running)
        }];
        let geom = crate::layout::layout(
            &[crate::layout::SlotMetrics {
                rest_width: m.pt(m.tile_size),
                magnifies: true,
            }],
            None,
            &m,
        );
        let panel = draw_dock(
            Target {
                pixmap: &mut pixmap,
                logical: (lw, lh),
                scale,
                offset_y: 0.0,
            },
            &m,
            &Palette::default(),
            Scene {
                slots: &slots,
                layout: &geom,
                icons: &mut icons,
                thumbnails: &ThumbnailCache::default(),
                drop_target: None,
            },
        );
        (pixmap, panel, m)
    }

    fn render_one(running: bool, scale: f32) -> (Pixmap, Rect, Metrics) {
        render_one_kind(SlotKind::App, running, scale)
    }

    fn dot_alpha_for(kind: SlotKind) -> u8 {
        let (pixmap, panel, m) = render_one_kind(kind, true, 1.0);
        let cx = (panel.x() + m.pt(m.panel_padding_h) + m.pt(m.tile_size) / 2.0) as u32;
        let cy = (panel.bottom() - m.pt(m.dot_bottom_margin)) as u32;
        pixmap.pixel(cx, cy).unwrap().alpha()
    }

    /// The dot means "this application is running". A minimised window's own
    /// tile is a window, not an application, and macOS gives it no dot -- so
    /// carrying a window id must not be enough to earn one.
    #[test]
    fn only_application_tiles_get_a_running_dot() {
        let app = dot_alpha_for(SlotKind::App);
        let minimized = dot_alpha_for(SlotKind::MinimizedWindow);

        assert!(
            app > 180,
            "an application tile should show its dot, got {app}"
        );
        assert!(
            minimized < app,
            "a minimised tile must not, got {minimized} vs {app}"
        );
    }

    fn dot_centre_alpha(running: bool) -> u8 {
        let (pixmap, panel, m) = render_one(running, 1.0);
        let cx = (panel.x() + m.pt(m.panel_padding_h) + m.pt(m.tile_size) / 2.0) as u32;
        let cy = (panel.bottom() - m.pt(m.dot_bottom_margin)) as u32;
        pixmap.pixel(cx, cy).unwrap().alpha()
    }

    /// At 2x the panel must cover four times the pixels while its *logical*
    /// rectangle is unchanged -- that split is what keeps the input region and
    /// the pointer in agreement with the layout.
    #[test]
    fn scaling_rasterises_larger_but_leaves_logical_geometry_alone() {
        let (px1, panel1, _) = render_one(false, 1.0);
        let (px2, panel2, _) = render_one(false, 2.0);

        assert_eq!(panel1, panel2, "logical geometry must not depend on scale");

        let opaque = |p: &Pixmap| p.pixels().iter().filter(|c| c.alpha() > 0).count();
        let (a, b) = (opaque(&px1), opaque(&px2));
        let ratio = b as f32 / a as f32;
        assert!(
            (3.5..4.5).contains(&ratio),
            "expected ~4x coverage, got {ratio}"
        );
    }

    /// A fractional scale must not crash or produce an empty frame.
    #[test]
    fn fractional_scale_still_draws() {
        let (px, _, _) = render_one(true, 1.5);
        assert!(px.pixels().iter().any(|c| c.alpha() > 0));
    }

    #[test]
    fn a_running_slot_draws_its_dot_inside_the_panel() {
        let running = dot_centre_alpha(true);
        let idle = dot_centre_alpha(false);

        // The dot is near-opaque white over a 10%-alpha panel, so it has to
        // lift the alpha well clear of the bare panel.
        assert!(running > idle, "running={running} idle={idle}");
        assert!(running > 180, "dot should be nearly opaque, got {running}");
    }

    /// Every slice must lie inside the capsule. Blur spilling past the panel's
    /// rounded ends is the exact artefact this slicing exists to prevent, and
    /// it is invisible in code review.
    #[test]
    fn capsule_region_stays_inside_the_outline() {
        let rect = Rect::from_xywh(0.0, 0.0, 400.0, 80.0).unwrap();
        let (r, cx, cy) = (40.0f32, 40.0f32, 40.0f32);

        for (x, y, w, h) in capsule_region(rect, r, 12) {
            let (x, y, w, h) = (x as f32, y as f32, w as f32, h as f32);
            assert!(x >= -0.5 && y >= -0.5, "slice starts outside: {x},{y}");
            assert!(x + w <= 400.5 && y + h <= 80.5, "slice ends outside");

            // Corners of a slice that fall within a cap's quadrant must be
            // inside that cap's circle.
            for (px, py) in [(x, y), (x + w, y), (x, y + h), (x + w, y + h)] {
                if px < cx && py < cy {
                    let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
                    assert!(d <= r + 1.5, "corner {px},{py} is {d} from centre, r={r}");
                }
            }
        }
    }

    #[test]
    fn capsule_region_covers_the_middle_band() {
        // Radius below half the height, so a real band exists between the caps.
        let rect = Rect::from_xywh(10.0, 5.0, 400.0, 100.0).unwrap();
        let rects = capsule_region(rect, 30.0, 8);
        assert_eq!(rects[0], (10, 35, 400, 40));
    }

    /// When the caps meet exactly there is no band to add, and a zero-height
    /// rectangle in a `wl_region` is just noise.
    #[test]
    fn no_degenerate_rectangles_are_emitted() {
        let rect = Rect::from_xywh(0.0, 0.0, 300.0, 80.0).unwrap();
        for (_, _, w, h) in capsule_region(rect, 40.0, 10) {
            assert!(w > 0 && h > 0, "degenerate rect {w}x{h}");
        }
    }

    /// Every row of the capsule must be covered by some slice. A missed row is
    /// composited unblurred and reads as a dark hairline across the panel.
    /// Fractional geometry is what exposed this -- these numbers reproduce a
    /// real configuration (tile 47pt, icon 37pt, radius ratio 0.26) whose
    /// slice pitch of ~1.19px used to leave a gap every fifth slice.
    #[test]
    fn capsule_region_leaves_no_horizontal_gaps() {
        let rect = Rect::from_xywh(20.0, 38.82, 300.0, 73.18).unwrap();
        let rects = capsule_region(rect, 19.03, 16);

        let top = rects.iter().map(|r| r.1).min().unwrap();
        let bottom = rects.iter().map(|r| r.1 + r.3).max().unwrap();
        for row in top..bottom {
            assert!(
                rects.iter().any(|&(_, y, _, h)| y <= row && row < y + h),
                "row {row} is not covered by any slice"
            );
        }
    }

    /// A radius small enough to be invisible degenerates to a plain rectangle
    /// rather than producing a pile of one-pixel slices.
    #[test]
    fn a_negligible_radius_gives_one_rectangle() {
        let rect = Rect::from_xywh(0.0, 0.0, 100.0, 40.0).unwrap();
        assert_eq!(capsule_region(rect, 0.0, 8), vec![(0, 0, 100, 40)]);
    }

    /// The panel's own radius is exactly half its height -- the boundary of the
    /// clamp -- so it has to survive that.
    #[test]
    fn a_half_height_radius_produces_a_real_capsule() {
        let rect = Rect::from_xywh(0.0, 0.0, 300.0, 80.0).unwrap();
        let rects = capsule_region(rect, 40.0, 10);
        assert!(rects.len() > 3, "expected sliced caps, got {}", rects.len());
        // Narrowest slice is at the very tip, widest is the middle band.
        let widest = rects.iter().map(|r| r.2).max().unwrap();
        assert_eq!(widest, 300);
    }

    /// Content narrower than the surface must never produce a negative origin.
    #[test]
    fn panel_wider_than_surface_clamps_to_zero() {
        let m = Metrics::default();
        let rect = panel_rect(100.0, m.surface_height(), &m, 4000.0);
        assert_eq!(rect.x(), 0.0);
    }
}
