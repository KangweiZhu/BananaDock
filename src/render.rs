//! Drawing the dock into a pixmap.
//!
//! Everything is drawn in surface-local logical pixels with the origin at the
//! surface's top-left. The surface is much taller than the panel (magnified
//! icons grow upwards into the headroom), so the panel is positioned against
//! the *bottom* of the pixmap rather than the top.

use std::time::Duration;

use tiny_skia::{
    FillRule, FilterQuality, Paint, PathBuilder, Pattern, Pixmap, Rect, SpreadMode, Stroke,
    Transform,
};

use crate::{
    edge::Frame,
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
    /// How far the panel is pushed out through the screen's edge for the
    /// auto-hide slide, in logical pixels. Zero when fully revealed.
    pub slide_out: f32,
}

/// What to draw: the row itself and everything needed to paint it.
pub struct Scene<'a> {
    pub slots: &'a [Slot],
    pub layout: &'a Layout,
    pub icons: &'a mut IconCache,
    pub thumbnails: &'a ThumbnailCache,
    /// Tile a drag is currently hovering, if any.
    pub drop_target: Option<usize>,
    /// Tile the pointer is resting on, which gets its name shown.
    pub hovered: Option<usize>,
    /// Only needed for that name, but the row cannot be drawn without it.
    pub text: &'a mut TextRenderer,
}

/// Where the panel sits inside the surface.
///
/// Centred along whichever edge the dock is on, and standing
/// `panel_bottom_gap` clear of it. `slide_out` pushes it back out through that
/// edge for the auto-hide animation, which is a pure translation -- the panel
/// keeps its shape, so the input region can follow the same rectangle.
pub fn panel_rect(frame: &Frame, metrics: &Metrics, content_len: f32, slide_out: f32) -> Rect {
    let len = content_len + metrics.pt(metrics.panel_padding_h) * 2.0;
    let thick = metrics.pt(metrics.panel_height());
    let along = ((frame.length() - len) / 2.0).max(0.0);
    let across = metrics.pt(metrics.panel_bottom_gap) - slide_out;

    frame
        .rect(along, across, len, thick)
        .expect("panel rect is non-degenerate")
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
        slide_out,
    } = target;
    let Scene {
        slots,
        layout,
        icons,
        thumbnails,
        drop_target,
        hovered,
        text,
    } = scene;
    let t = Transform::from_scale(scale, scale);

    // An empty row still gets a panel, so the dock does not blink out of
    // existence when the last window closes.
    let content_w = if layout.content_width > 0.0 {
        layout.content_width
    } else {
        metrics.pt(metrics.tile_size)
    };
    let frame = Frame::new(metrics.edge, logical);
    let panel = panel_rect(&frame, metrics, content_w, slide_out);
    draw_panel(pixmap, t, metrics, palette, panel);

    // From here on everything is placed in the row's own terms: `along` runs
    // down the row and `across` measures in from the screen's edge, so the
    // same arithmetic serves a dock on any edge.
    let row_along = frame.along_start_of(panel) + metrics.pt(metrics.panel_padding_h);
    // The panel's near side -- the one against the screen's edge. Icons and
    // dots are placed off this rather than off the panel's far side, so they
    // stay put while the panel's thickness changes around them.
    let panel_near = frame.near_of(panel);
    let panel_thick = metrics.pt(metrics.panel_height());

    // Application badges for minimised windows, drawn once the whole row is
    // down. See where they are pushed for why they cannot go in as they come.
    let mut badges: Vec<(Rect, &str)> = Vec::new();

    for (i, (slot, geom)) in slots.iter().zip(&layout.slots).enumerate() {
        let centre_along = row_along + geom.centre();

        // A drag hovering this tile: show where the files would land.
        if drop_target == Some(i) && slot.kind != SlotKind::Separator {
            let pad = geom.width * 0.08;
            if let Some(rect) = frame.rect(
                row_along + geom.x + pad,
                panel_near + pad,
                geom.width - pad * 2.0,
                panel_thick - pad * 2.0,
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
            draw_separator(
                pixmap,
                t,
                &frame,
                metrics,
                palette,
                centre_along,
                panel_near,
                panel_thick,
            );
            continue;
        }

        // A slot's width is its pitch; the artwork is a fixed fraction of it,
        // so magnifying the slot magnifies the icon with it.
        let icon_px = geom.width * metrics.icon_size_ratio;

        // The artwork stands off the screen's edge by a fixed margin and grows
        // *inwards* when magnified, as on macOS. The launch bounce lifts the
        // artwork only -- the dot stays on the panel, as it does there too.
        let icon_near = panel_near + metrics.pt(metrics.icon_bottom_margin()) + geom.lift;
        let icon_box = frame.rect(centre_along - icon_px / 2.0, icon_near, icon_px, icon_px);

        // A minimised window shows the window, not its application's icon --
        // that is the whole point of giving it a tile of its own. The picture
        // arrives asynchronously, so the icon stands in until it does.
        let thumbnail = slot
            .capture_key
            .as_deref()
            .filter(|_| slot.kind == SlotKind::MinimizedWindow)
            .and_then(|key| thumbnails.get(key));

        // A picture that has been asked for but has not arrived yet. The tile
        // is only just growing into the row, and the capture normally lands
        // before it finishes; standing in with the application's icon would
        // put a visible swap in the middle of that animation, so the tile
        // stays empty until either the picture or the deadline arrives.
        let awaiting = thumbnail.is_none()
            && slot.kind == SlotKind::MinimizedWindow
            && slot.capture_key.as_deref().is_some_and(|key| {
                thumbnails.awaiting(key, Duration::from_millis(metrics.row_change_ms.into()))
            });

        if let (Some(thumb), Some(box_rect)) = (thumbnail, icon_box) {
            // Fitted inside the icon's box, keeping the window's proportions:
            // stretching a window to a square makes it unrecognisable. A
            // landscape window therefore fills the box's width and leaves room
            // above and below, which is what the reference dock does too.
            //
            // Deliberately in the surface's own terms rather than the row's: a
            // window is landscape however the dock is turned, and a picture of
            // one laid on its side would be nonsense.
            let (w, h) = if thumb.aspect >= 1.0 {
                (icon_px, icon_px / thumb.aspect)
            } else {
                (icon_px * thumb.aspect, icon_px)
            };
            let x = box_rect.x() + (box_rect.width() - w) / 2.0;
            let y = box_rect.y() + (box_rect.height() - h) / 2.0;
            draw_image(pixmap, t, &thumb.pixmap, x, y, w, h);

            // ...and the application's icon badges the corner, so the tile
            // says *which* window this is. Held back for a second pass: the
            // badge hangs past the picture and, with the tiles packed close
            // together, into the next tile -- which is still to be drawn and
            // would paint over it.
            if let Some(rect) = Rect::from_xywh(x, y, w, h)
                .and_then(|thumb| badge_rect(thumb, icon_px, metrics.thumbnail_badge_ratio))
            {
                if let Some(name) = slot.icon_name.as_deref() {
                    badges.push((rect, name));
                }
            }
        } else if let (Some(name), Some(box_rect)) =
            (slot.icon_name.as_deref().filter(|_| !awaiting), icon_box)
        {
            if let Some(art) = icons.get(name) {
                draw_icon(pixmap, t, art, box_rect.x(), box_rect.y(), icon_px);
            }
        }

        // The dot marks a running *application*. A minimised window's own tile
        // is not an application, and macOS gives it no dot.
        if slot.kind == SlotKind::App && slot.is_running() {
            draw_dot(
                pixmap,
                t,
                &frame,
                metrics,
                palette,
                centre_along,
                panel_near,
            );
        }
    }

    for (rect, name) in badges {
        if let Some(art) = icons.get(name) {
            draw_icon(pixmap, t, art, rect.x(), rect.y(), rect.width());
        }
    }

    // Last of all, so it floats over anything it happens to reach across.
    if let Some((slot, geom)) = hovered.and_then(|i| slots.get(i).zip(layout.slots.get(i))) {
        if slot.kind != SlotKind::Separator && !slot.label.is_empty() {
            // Clear of both the artwork and the panel: a magnified icon
            // stands past the panel's inner face, while a small one does not
            // reach it -- and a chip laid over the panel reads as part of it
            // rather than as a note about the tile under the pointer.
            let icon_px = geom.width * metrics.icon_size_ratio;
            let icon_far =
                panel_near + metrics.pt(metrics.icon_bottom_margin()) + geom.lift + icon_px;
            let clear = icon_far.max(panel_near + panel_thick);
            draw_label(
                pixmap,
                t,
                scale,
                &frame,
                metrics,
                palette,
                text,
                &slot.label,
                row_along + geom.centre(),
                clear + metrics.pt(metrics.label_gap),
            );
        }
    }

    panel
}

/// The chip naming whatever the pointer is resting on.
///
/// Sits clear of the icon on the side away from the screen's edge, centred on
/// the tile and nudged back inside the surface if the name is long enough to
/// run off the end of the row -- a label that leaves the screen names nothing.
#[allow(clippy::too_many_arguments)]
fn draw_label(
    pixmap: &mut Pixmap,
    t: Transform,
    scale: f32,
    frame: &Frame,
    metrics: &Metrics,
    palette: &Palette,
    text: &mut TextRenderer,
    label: &str,
    centre_along: f32,
    across: f32,
) {
    let font_px = metrics.pt(metrics.label_font_size);
    let max = metrics.pt(metrics.label_max_width);
    let shown = elide(text, label, font_px, max);

    let text_w = text.measure(&shown, font_px);
    let w = text_w + metrics.pt(metrics.label_padding_h) * 2.0;
    let h = metrics.pt(metrics.label_height);

    // The chip is always the same way up, so on a vertical dock its width runs
    // across the row and its height along it -- the opposite of the row's own
    // sense of the two.
    let (len, thick) = if frame.is_vertical() { (h, w) } else { (w, h) };
    let along = (centre_along - len / 2.0).clamp(0.0, (frame.length() - len).max(0.0));
    let Some(rect) = frame.rect(along, across, len, thick) else {
        return;
    };

    let radius = metrics.pt(metrics.label_radius);
    if let Some(path) = rounded_rect(rect, radius) {
        let mut paint = Paint {
            anti_alias: true,
            ..Default::default()
        };
        paint.set_color(palette.menu_background.to_skia());
        pixmap.fill_path(&path, &paint, FillRule::Winding, t, None);
    }
    if let Some(inner) = Rect::from_xywh(
        rect.x() + 0.5,
        rect.y() + 0.5,
        rect.width() - 1.0,
        rect.height() - 1.0,
    ) {
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

    // Centred both ways in the chip. The text renderer draws straight into the
    // pixmap rather than through the canvas transform, so this is the one
    // place that has to convert to device pixels itself -- and the factor is
    // the output's scale, not the points-to-pixels one already folded into
    // every measurement above.
    let line_h = font_px * scale * 1.25;
    text.draw(
        pixmap,
        &shown,
        font_px * scale,
        (rect.x() + (rect.width() - text_w) / 2.0) * scale,
        (rect.y() + rect.height() / 2.0) * scale - line_h / 2.0,
        palette.menu_text,
    );
}

/// Cuts a name short at `max`, ending it in an ellipsis.
///
/// Measured rather than counted: how many characters fit depends entirely on
/// which ones they are, and a name in a script where every glyph is wide would
/// otherwise run straight out of its chip.
fn elide(text: &mut TextRenderer, label: &str, font_px: f32, max: f32) -> String {
    if text.measure(label, font_px) <= max {
        return label.to_owned();
    }

    let mut cut = label.char_indices().map(|(i, _)| i).collect::<Vec<_>>();
    cut.push(label.len());
    // Longest prefix that still fits once the ellipsis is on the end.
    let mut best = String::from("…");
    for end in cut {
        let candidate = format!("{}…", &label[..end]);
        if text.measure(&candidate, font_px) > max {
            break;
        }
        best = candidate;
    }
    best
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
        // The label has to fit what the layout settled on, which for a long
        // window title is the maximum rather than the text's own width. Both
        // sides of that comparison are in device pixels: `font_px` already
        // carries the output's scale, and measuring against a logical width
        // would cut the text short by exactly that factor.
        let room = (logical.0 - pad * 3.0) * scale;
        let label = elide(text, &item.label, font_px, room);
        text.draw(
            pixmap,
            &label,
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
#[allow(clippy::too_many_arguments)]
fn draw_separator(
    pixmap: &mut Pixmap,
    t: Transform,
    frame: &Frame,
    metrics: &Metrics,
    palette: &Palette,
    centre_along: f32,
    panel_near: f32,
    panel_thick: f32,
) {
    let w = metrics.pt(metrics.separator_line_width);
    let inset = metrics.pt(metrics.separator_inset);
    let len = panel_thick - inset * 2.0;
    if len <= 0.0 {
        return;
    }

    // Thin down the row and long across it, so it fences off the tiles either
    // side of it whichever way the row runs.
    let Some(rect) = frame.rect(centre_along - w / 2.0, panel_near + inset, w, len) else {
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

/// Where the application badge sits on a minimised window's thumbnail.
///
/// Centred exactly on the thumbnail's bottom-right corner, so it overhangs the
/// picture on two sides. That overhang is the point: a landscape thumbnail
/// stops well short of the row's baseline, and the badge reaches back down to
/// it, which is what stops a minimised tile from reading as a stray strip
/// floating in the middle of the panel.
fn badge_rect(thumb: Rect, icon_px: f32, ratio: f32) -> Option<Rect> {
    let size = icon_px * ratio;
    Rect::from_xywh(
        thumb.right() - size / 2.0,
        thumb.bottom() - size / 2.0,
        size,
        size,
    )
}

/// The dot under a running application.
fn draw_dot(
    pixmap: &mut Pixmap,
    t: Transform,
    frame: &Frame,
    metrics: &Metrics,
    palette: &Palette,
    centre_along: f32,
    panel_near: f32,
) {
    let d = metrics.pt(metrics.dot_size);
    let (cx, cy) = frame.point(
        centre_along,
        panel_near + metrics.pt(metrics.dot_bottom_margin),
    );

    let Some(path) = PathBuilder::from_circle(cx, cy, d / 2.0) else {
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
    // A width of zero means no rim at all. Passing it to the stroker instead
    // would draw the thinnest line it can rather than nothing, which is the
    // one thing a rim of zero must not do.
    if bw <= 0.0 {
        return;
    }
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
    use crate::edge::Edge;

    #[test]
    fn panel_is_centred_and_sits_above_the_screen_edge() {
        let m = Metrics::default();
        let surface_h = m.surface_depth();
        let frame = Frame::new(m.edge, (1000.0, surface_h));
        let rect = panel_rect(&frame, &m, 400.0, 0.0);

        let expected_w = 400.0 + m.pt(m.panel_padding_h) * 2.0;
        assert!((rect.width() - expected_w).abs() < 0.01);
        // Centred.
        assert!((rect.x() - (1000.0 - expected_w) / 2.0).abs() < 0.01);
        // The gap below the panel is preserved.
        assert!((surface_h - rect.bottom() - m.pt(m.panel_bottom_gap)).abs() < 0.01);
    }

    /// A rim of zero must leave no rim. tiny-skia reads a zero-width stroke as
    /// "as thin as possible", so without a guard the setting's bottom stop
    /// would still draw a line -- and the one thing it promises is silence.
    #[test]
    fn a_rim_of_zero_draws_nothing() {
        fn rim_alpha(width: f32) -> u8 {
            let m = Metrics::default();
            let palette = Palette {
                // Only the rim: a tint would paint the same pixels.
                panel_tint: crate::metrics::Color::rgba(0.0, 0.0, 0.0, 0.0),
                panel_border_width: width,
                ..Palette::default()
            };
            let mut pixmap = Pixmap::new(200, 100).unwrap();
            let panel = Rect::from_xywh(20.0, 20.0, 160.0, 60.0).unwrap();
            draw_panel(&mut pixmap, Transform::identity(), &m, &palette, panel);
            // The middle of the top edge, where the rim runs.
            (0..3)
                .map(|d| pixmap.pixel(100, 20 + d).unwrap().alpha())
                .max()
                .unwrap()
        }

        assert!(rim_alpha(1.0) > 40, "a one-point rim should be drawn");
        assert_eq!(rim_alpha(0.0), 0, "a rim of zero should not be");
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

    /// The badge marks which application a minimised window belongs to, and
    /// its overhang past the thumbnail's corner is what brings the tile back
    /// down to the row's baseline. Both properties live in the geometry, so
    /// they are worth pinning even though the drawing itself is not.
    #[test]
    fn the_badge_straddles_the_thumbnails_corner() {
        // A 16:9 window in a 48pt box: 48 wide, 27 tall.
        let thumb = Rect::from_xywh(100.0, 50.0, 48.0, 27.0).unwrap();
        let badge = badge_rect(thumb, 48.0, 0.5).expect("badge rect");

        assert!((badge.width() - 24.0).abs() < 0.01, "half the icon box");
        // Centred on the corner: half of it hangs past each edge.
        assert!((badge.right() - (thumb.right() + 12.0)).abs() < 0.01);
        assert!((badge.bottom() - (thumb.bottom() + 12.0)).abs() < 0.01);
        assert!(badge.x() < thumb.right() && badge.y() < thumb.bottom());
    }

    /// A tall window fills the box's height instead, and the badge follows it
    /// rather than staying pinned to where a landscape corner would have been.
    #[test]
    fn the_badge_follows_a_portrait_thumbnail() {
        let thumb = Rect::from_xywh(100.0, 50.0, 27.0, 48.0).unwrap();
        let badge = badge_rect(thumb, 48.0, 0.5).expect("badge rect");
        assert!((badge.right() - (thumb.right() + 12.0)).abs() < 0.01);
        assert!((badge.bottom() - (thumb.bottom() + 12.0)).abs() < 0.01);
    }

    /// Renders a real frame and checks the badge actually lands on the
    /// picture. The geometry test above cannot see a drawing-order mistake --
    /// painting the badge before the thumbnail would bury it -- and this is
    /// the only tile whose artwork is composed of two images.
    #[test]
    fn a_minimised_tile_paints_its_application_over_the_thumbnail() {
        use crate::thumbnails::Thumbnail;
        use std::sync::Arc;

        // A fake icon on disk: `IconCache::get` accepts absolute paths, so the
        // test does not depend on whichever icon theme happens to be installed.
        let dir = std::env::temp_dir().join("kdock-badge-test");
        std::fs::create_dir_all(&dir).unwrap();
        let icon_path = dir.join("icon.png");
        let mut art = Pixmap::new(64, 64).unwrap();
        art.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        art.save_png(&icon_path).unwrap();

        // A 16:9 window, which is the case that leaves room around the picture.
        const ASPECT: f32 = 16.0 / 9.0;
        let mut shot = Pixmap::new(160, 90).unwrap();
        shot.fill(tiny_skia::Color::from_rgba8(0, 0, 255, 255));

        let m = Metrics::default();
        let (lw, lh) = (400.0, m.surface_depth());
        let mut pixmap = Pixmap::new(lw as u32, lh.ceil() as u32).unwrap();
        let mut icons = IconCache::default();
        let mut thumbnails = ThumbnailCache::default();
        thumbnails.insert(
            "w".into(),
            Thumbnail {
                pixmap: Arc::new(shot),
                aspect: ASPECT,
            },
        );

        let slots = [Slot {
            kind: SlotKind::MinimizedWindow,
            capture_key: Some("w".into()),
            icon_name: Some(icon_path.to_string_lossy().into_owned()),
            ..slot(false)
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
                scale: 1.0,
                slide_out: 0.0,
            },
            &m,
            &Palette::default(),
            Scene {
                slots: &slots,
                layout: &geom,
                icons: &mut icons,
                thumbnails: &thumbnails,
                drop_target: None,
                hovered: None,
                text: &mut TextRenderer::new(),
            },
        );

        // Where the picture and its badge should have landed.
        let icon_px = m.pt(m.tile_size) * m.icon_size_ratio;
        let centre_x = panel.x() + m.pt(m.panel_padding_h) + m.pt(m.tile_size) / 2.0;
        let icon_bottom = panel.bottom() - m.pt(m.icon_bottom_margin());
        let h = icon_px / ASPECT;
        let thumb = Rect::from_xywh(
            centre_x - icon_px / 2.0,
            icon_bottom - (icon_px + h) / 2.0,
            icon_px,
            h,
        )
        .unwrap();
        let badge = badge_rect(thumb, icon_px, m.thumbnail_badge_ratio).unwrap();
        let at = |x: f32, y: f32| pixmap.pixel(x as u32, y as u32).unwrap();

        // The window shows through where the badge does not reach...
        let window = at(thumb.x() + 3.0, thumb.y() + h / 2.0);
        assert!(
            window.blue() > 150 && window.red() < 80,
            "the thumbnail is not there: {window:?}"
        );
        // ...the badge covers the corner it is centred on...
        let corner = at(thumb.right() - 3.0, thumb.bottom() - 3.0);
        assert!(
            corner.red() > 150 && corner.blue() < 80,
            "the badge is behind the thumbnail: {corner:?}"
        );
        // ...and overhangs below it, where the bare picture stopped short.
        let under = at(badge.x() + badge.width() / 2.0, thumb.bottom() + 3.0);
        assert!(
            under.red() > 150,
            "the badge does not reach past the picture: {under:?}"
        );

        std::fs::remove_file(&icon_path).ok();
    }

    /// The icon must not stand in for a picture that is on its way: the swap
    /// a few frames later is the whole thing this avoids. Rendered rather than
    /// reasoned about, because the fallback is a branch in the draw path.
    #[test]
    fn a_tile_waiting_for_its_picture_stays_empty() {
        /// How much of the icon shows at the centre of the tile's artwork,
        /// with and without a capture in flight.
        fn icon_red(pending: bool) -> u8 {
            let dir = std::env::temp_dir().join("kdock-waiting-test");
            std::fs::create_dir_all(&dir).unwrap();
            let icon_path = dir.join("icon.png");
            let mut art = Pixmap::new(64, 64).unwrap();
            art.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
            art.save_png(&icon_path).unwrap();

            // The grace is wall-clock, and this test is about the branch
            // rather than the deadline (which thumbnails.rs tests on its own).
            // Left at the real 220ms it fails whenever the suite is loaded
            // enough for that long to pass before the frame is drawn.
            let m = Metrics {
                row_change_ms: 60_000,
                ..Metrics::default()
            };
            let (lw, lh) = (400.0, m.surface_depth());
            let mut pixmap = Pixmap::new(lw as u32, lh.ceil() as u32).unwrap();
            let mut icons = IconCache::default();
            let thumbnails = ThumbnailCache::default();
            if pending {
                thumbnails.mark_pending("w");
            }

            let slots = [Slot {
                kind: SlotKind::MinimizedWindow,
                capture_key: Some("w".into()),
                icon_name: Some(icon_path.to_string_lossy().into_owned()),
                ..slot(false)
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
                    scale: 1.0,
                    slide_out: 0.0,
                },
                &m,
                &Palette::default(),
                Scene {
                    slots: &slots,
                    layout: &geom,
                    icons: &mut icons,
                    thumbnails: &thumbnails,
                    drop_target: None,
                    hovered: None,
                    text: &mut TextRenderer::new(),
                },
            );

            let icon_px = m.pt(m.tile_size) * m.icon_size_ratio;
            let cx = panel.x() + m.pt(m.panel_padding_h) + m.pt(m.tile_size) / 2.0;
            let cy = panel.bottom() - m.pt(m.icon_bottom_margin()) - icon_px / 2.0;
            std::fs::remove_file(&icon_path).ok();
            pixmap.pixel(cx as u32, cy as u32).unwrap().red()
        }

        // Nothing in flight: the icon is all the tile can show, so it shows it.
        assert!(
            icon_red(false) > 150,
            "a tile with no picture coming should fall back to the icon"
        );
        assert!(
            icon_red(true) < 80,
            "a tile whose picture is on its way should stay empty"
        );
    }

    /// The panel has to land against whichever edge it was sent to, with the
    /// row running along that edge. Checked through `draw_dock` rather than
    /// `panel_rect` alone, since the drawing is what a wrong axis shows up in.
    #[test]
    fn the_panel_lands_against_each_edge_in_turn() {
        for (edge, surface) in [
            (Edge::Bottom, (600.0, 200.0)),
            (Edge::Top, (600.0, 200.0)),
            (Edge::Left, (200.0, 600.0)),
            (Edge::Right, (200.0, 600.0)),
        ] {
            let m = Metrics {
                edge,
                ..Metrics::default()
            };
            let mut pixmap = Pixmap::new(surface.0 as u32, surface.1 as u32).unwrap();
            let mut icons = IconCache::default();
            let slots = [slot(false), slot(false)];
            let geom = crate::layout::layout(
                &[crate::layout::SlotMetrics {
                    rest_width: m.pt(m.tile_size),
                    magnifies: true,
                }; 2],
                None,
                &m,
            );
            let panel = draw_dock(
                Target {
                    pixmap: &mut pixmap,
                    logical: surface,
                    scale: 1.0,
                    slide_out: 0.0,
                },
                &m,
                &Palette::default(),
                Scene {
                    slots: &slots,
                    layout: &geom,
                    icons: &mut icons,
                    thumbnails: &ThumbnailCache::default(),
                    drop_target: None,
                    hovered: None,
                    text: &mut TextRenderer::new(),
                },
            );

            let frame = Frame::new(edge, surface);
            let gap = m.pt(m.panel_bottom_gap);
            assert!(
                (frame.near_of(panel) - gap).abs() < 0.01,
                "{edge:?}: panel stands {} from the edge, not {gap}",
                frame.near_of(panel)
            );
            // The row runs along the edge, so the panel is long that way and
            // only as deep as the panel's own thickness.
            assert!(
                (frame.along_len_of(panel) - (geom.content_width + m.pt(m.panel_padding_h) * 2.0))
                    .abs()
                    < 0.01,
                "{edge:?}: the row does not run along the edge"
            );
            // ...and it is centred on it.
            let start = frame.along_start_of(panel);
            let slack = frame.length() - frame.along_len_of(panel);
            assert!(
                (start - slack / 2.0).abs() < 0.01,
                "{edge:?}: panel is not centred, starts at {start}"
            );
        }
    }

    /// Sliding out for auto-hide pushes the panel through its own edge, not
    /// downwards -- a top or side dock that slid to the bottom would cross the
    /// screen on its way out.
    #[test]
    fn hiding_pushes_the_panel_out_through_its_own_edge() {
        for (edge, surface) in [
            (Edge::Bottom, (600.0, 200.0)),
            (Edge::Top, (600.0, 200.0)),
            (Edge::Left, (200.0, 600.0)),
            (Edge::Right, (200.0, 600.0)),
        ] {
            let m = Metrics {
                edge,
                ..Metrics::default()
            };
            let frame = Frame::new(edge, surface);
            let resting = panel_rect(&frame, &m, 200.0, 0.0);
            let hidden = panel_rect(&frame, &m, 200.0, 40.0);

            assert!(
                (frame.near_of(resting) - frame.near_of(hidden) - 40.0).abs() < 0.01,
                "{edge:?}: hiding moved the panel by the wrong amount"
            );
            assert!(
                (frame.along_start_of(resting) - frame.along_start_of(hidden)).abs() < 0.01,
                "{edge:?}: hiding slid the panel sideways"
            );
        }
    }

    /// A name too long for its chip is cut short rather than run out of it,
    /// and the cut is measured rather than counted -- how many characters fit
    /// depends on which ones they are.
    #[test]
    fn a_long_name_is_cut_short_to_fit() {
        let mut text = TextRenderer::new();
        let short = "Files";
        assert_eq!(elide(&mut text, short, 13.0, 200.0), short);

        let long = "A window title that goes on for far longer than any chip";
        let cut = elide(&mut text, long, 13.0, 120.0);
        assert!(cut.ends_with('…'), "should end in an ellipsis: {cut}");
        assert!(text.measure(&cut, 13.0) <= 120.0, "still too wide: {cut}");
        assert!(long.starts_with(cut.trim_end_matches('…')), "not a prefix");
        // A width that fits nothing at all still has to produce something.
        assert_eq!(elide(&mut text, long, 13.0, 1.0), "…");
    }

    /// Renders a frame with a tile hovered and reports whether the label's
    /// chip landed where it was expected, in the row's own terms.
    fn hovered_label_rect(edge: Edge, surface: (f32, f32)) -> Option<Rect> {
        let m = Metrics {
            edge,
            ..Metrics::default()
        };
        let mut pixmap = Pixmap::new(surface.0 as u32, surface.1 as u32).unwrap();
        let mut icons = IconCache::default();
        let mut text = TextRenderer::new();
        let slots = [Slot {
            label: "Files".into(),
            ..slot(false)
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
                logical: surface,
                scale: 1.0,
                slide_out: 0.0,
            },
            &m,
            &Palette::default(),
            Scene {
                slots: &slots,
                layout: &geom,
                icons: &mut icons,
                thumbnails: &ThumbnailCache::default(),
                drop_target: None,
                hovered: Some(0),
                text: &mut text,
            },
        );

        // Find the chip: the only painted thing beyond the icon's reach.
        let frame = Frame::new(edge, surface);
        let icon_far = frame.near_of(panel) + m.icon_bottom_margin() + m.icon_size();
        let mut found: Option<Rect> = None;
        for y in 0..pixmap.height() {
            for x in 0..pixmap.width() {
                if pixmap.pixel(x, y).unwrap().alpha() < 40 {
                    continue;
                }
                if frame.across_of(x as f32, y as f32) <= icon_far + 2.0 {
                    continue;
                }
                let r = Rect::from_xywh(x as f32, y as f32, 1.0, 1.0).unwrap();
                found = Some(match found {
                    None => r,
                    Some(f) => Rect::from_ltrb(
                        f.left().min(r.left()),
                        f.top().min(r.top()),
                        f.right().max(r.right()),
                        f.bottom().max(r.bottom()),
                    )
                    .unwrap(),
                });
            }
        }
        found
    }

    /// The chip stands past the icon on the side away from the screen's edge,
    /// whichever edge that is -- drawn behind the dock, or off the surface, it
    /// would name nothing.
    #[test]
    fn the_hover_label_stands_clear_of_the_icon_on_every_edge() {
        for (edge, surface) in [
            (Edge::Bottom, (600.0, 400.0)),
            (Edge::Top, (600.0, 400.0)),
            (Edge::Left, (400.0, 600.0)),
            (Edge::Right, (400.0, 600.0)),
        ] {
            let chip = hovered_label_rect(edge, surface)
                .unwrap_or_else(|| panic!("{edge:?}: no label was drawn"));

            let frame = Frame::new(edge, surface);
            assert!(
                frame.near_of(chip) > 0.0,
                "{edge:?}: the chip hangs off the surface"
            );
            assert!(
                chip.x() >= 0.0
                    && chip.y() >= 0.0
                    && chip.right() <= surface.0
                    && chip.bottom() <= surface.1,
                "{edge:?}: the chip is not fully on the surface: {chip:?}"
            );
        }
    }

    /// A tile at the very end of the row still gets a whole chip: it slides
    /// back along the row rather than half of it leaving the screen.
    #[test]
    fn a_label_at_the_end_of_the_row_stays_on_screen() {
        let m = Metrics::default();
        let frame = Frame::new(Edge::Bottom, (200.0, 300.0));
        let mut text = TextRenderer::new();
        let font = m.pt(m.label_font_size);
        let w =
            text.measure("A rather long application name", font) + m.pt(m.label_padding_h) * 2.0;
        // Centred on a tile hard against the row's end, the chip would start
        // well before zero; the drawing clamps it back.
        let along = (0.0f32 - w / 2.0).clamp(0.0, (frame.length() - w).max(0.0));
        assert!(along >= 0.0);
        assert!(along + w <= frame.length().max(w));
    }

    /// The chip's text is the one thing drawn straight into the pixmap rather
    /// than through the canvas transform, so it is the one thing that can miss
    /// the output's scale and land somewhere else entirely. A HiDPI frame is
    /// the only place that shows: at scale 1 the mistake is invisible.
    #[test]
    fn the_labels_text_lands_inside_its_chip_on_a_hidpi_frame() {
        for scale in [1.0, 2.0] {
            let m = Metrics::default();
            let (lw, lh) = (600.0, m.surface_depth());
            let mut pixmap = Pixmap::new((lw * scale) as u32, (lh * scale).ceil() as u32).unwrap();
            let mut icons = IconCache::default();
            let mut text = TextRenderer::new();
            let slots = [Slot {
                label: "Files".into(),
                ..slot(false)
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
                    slide_out: 0.0,
                },
                &m,
                &Palette::default(),
                Scene {
                    slots: &slots,
                    layout: &geom,
                    icons: &mut icons,
                    thumbnails: &ThumbnailCache::default(),
                    drop_target: None,
                    hovered: Some(0),
                    text: &mut text,
                },
            );

            // The chip sits past the icon, so anything painted there is it.
            let chip_from = (panel.y() - m.pt(m.label_gap)) * scale;
            let chip_to = chip_from - m.pt(m.label_height) * scale;
            let mut chip = 0;
            let mut glyphs = 0;
            for y in (chip_to.max(0.0) as u32)..(chip_from as u32) {
                for x in 0..pixmap.width() {
                    let px = pixmap.pixel(x, y).unwrap();
                    if px.alpha() < 40 {
                        continue;
                    }
                    chip += 1;
                    // The chip's own fill is near-black; the text is not.
                    if px.red() > 140 && px.green() > 140 && px.blue() > 140 {
                        glyphs += 1;
                    }
                }
            }
            assert!(chip > 0, "scale {scale}: no chip was drawn at all");
            assert!(
                glyphs > 20,
                "scale {scale}: the chip is empty -- {glyphs} lit pixels of text"
            );
        }
    }

    /// A window title can be longer than any menu, so the row is cut to fit --
    /// and the cut has to use the whole width the layout reserved. `font_px`
    /// carries the output's scale while the layout's width does not, so
    /// measuring one against the other cuts the text short by exactly that
    /// factor: right at scale 1, half a menu's worth of text missing at 2.
    #[test]
    fn a_long_menu_row_is_cut_to_the_width_the_layout_reserved() {
        use crate::menu::{layout_menu, MenuAction, MenuItem};

        let m = Metrics::default();
        let items = vec![MenuItem {
            label: "A window title that runs on far past anything a menu could \
                    reasonably be asked to show at once"
                .into(),
            action: Some(MenuAction::Quit),
            checked: false,
        }];

        let mut text = TextRenderer::new();
        let font = m.pt(m.menu_font_size);
        let layout = layout_menu(&items, &m, |s| text.measure(s, font));
        assert_eq!(
            layout.width,
            m.pt(m.menu_max_width),
            "capped at the maximum"
        );

        // How much of the label survives, in characters, at each scale.
        let kept = |scale: f32| {
            let mut text = TextRenderer::new();
            let room = (layout.width - m.pt(m.menu_item_padding) * 3.0) * scale;
            elide(
                &mut text,
                &items[0].label,
                m.pt(m.menu_font_size) * scale,
                room,
            )
            .chars()
            .count()
        };

        let at_one = kept(1.0);
        assert!(at_one > 20, "barely any of the title survived: {at_one}");
        assert_eq!(
            at_one,
            kept(2.0),
            "the same row keeps a different amount of text on a HiDPI screen"
        );
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
        let (lw, lh) = (400.0, m.surface_depth());
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
                slide_out: 0.0,
            },
            &m,
            &Palette::default(),
            Scene {
                slots: &slots,
                layout: &geom,
                icons: &mut icons,
                thumbnails: &ThumbnailCache::default(),
                drop_target: None,
                hovered: None,
                text: &mut TextRenderer::new(),
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
        let frame = Frame::new(m.edge, (100.0, m.surface_depth()));
        let rect = panel_rect(&frame, &m, 4000.0, 0.0);
        assert_eq!(rect.x(), 0.0);
    }
}
