//! Drawing the settings window.
//!
//! Plain shapes on a plain background: the window is chrome around a config
//! file, and dressing it up would only make it look like it belongs to a
//! desktop it does not.

use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};

use super::ui::{self, Control};
use crate::{metrics::Color, render::rounded_rect, text::TextRenderer};

const BACKGROUND: Color = Color::rgba(0.14, 0.14, 0.15, 1.0);
const ROW_HOVER: Color = Color::rgba(1.0, 1.0, 1.0, 0.05);
const LABEL: Color = Color::rgba(1.0, 1.0, 1.0, 0.92);
const HEADING: Color = Color::rgba(1.0, 1.0, 1.0, 0.45);
const VALUE: Color = Color::rgba(1.0, 1.0, 1.0, 0.55);
const TRACK: Color = Color::rgba(1.0, 1.0, 1.0, 0.16);
const ACCENT: Color = Color::rgba(0.20, 0.52, 0.94, 1.0);
const KNOB: Color = Color::rgba(1.0, 1.0, 1.0, 0.95);

const LABEL_SIZE: f32 = 13.0;
const HEADING_SIZE: f32 = 11.0;

/// `TextRenderer::draw` places the *top of the line box*, not a baseline, so
/// centring text in a row means subtracting half a line height rather than
/// nudging a baseline down. Same factor the dock's own menu uses.
fn line_top(row_top: f32, row_height: f32, px: f32) -> f32 {
    row_top + row_height / 2.0 - px * 1.25 / 2.0
}

pub fn draw(
    pixmap: &mut Pixmap,
    text: &mut TextRenderer,
    controls: &[Control],
    width: f32,
    hovered: Option<usize>,
) {
    pixmap.fill(BACKGROUND.to_skia());
    let (tops, _) = ui::rows(controls);

    for (i, (control, top)) in controls.iter().zip(&tops).enumerate() {
        match control {
            Control::Heading(title) => {
                // Sits low in its taller row, so it reads as a lid on what
                // follows rather than as another setting.
                text.draw(
                    pixmap,
                    &title.to_uppercase(),
                    HEADING_SIZE,
                    ui::PADDING,
                    line_top(
                        top + ui::HEADING_HEIGHT / 2.0,
                        ui::HEADING_HEIGHT / 2.0,
                        HEADING_SIZE,
                    ),
                    HEADING,
                );
            }
            Control::Toggle { label, value, .. } => {
                if hovered == Some(i) {
                    highlight(pixmap, *top, width);
                }
                label_at(pixmap, text, label, *top);
                if let Some(rect) = ui::control_rect(control, *top, width) {
                    toggle(pixmap, rect, *value);
                }
            }
            Control::Slider {
                label,
                value,
                min,
                max,
                unit,
                ..
            } => {
                if hovered == Some(i) {
                    highlight(pixmap, *top, width);
                }
                label_at(pixmap, text, label, *top);
                if let Some(rect) = ui::control_rect(control, *top, width) {
                    let fraction = ((value - min) / (max - min)).clamp(0.0, 1.0);
                    slider(pixmap, rect, fraction);

                    // The number goes left of the track, where there is room
                    // whatever the label's length.
                    // Decimals have to match what `quantise` keeps, or the
                    // number shown would not be the number saved.
                    let shown = match *unit {
                        "tiles" => format!("{value:.1} {unit}"),
                        "of height" => format!("{value:.2} {unit}"),
                        _ => format!("{value:.0} {unit}"),
                    };
                    let w = text.measure(&shown, LABEL_SIZE);
                    text.draw(
                        pixmap,
                        &shown,
                        LABEL_SIZE,
                        rect.0 - w - 12.0,
                        line_top(*top, ui::ROW_HEIGHT, LABEL_SIZE),
                        VALUE,
                    );
                }
            }
        }
    }
}

fn label_at(pixmap: &mut Pixmap, text: &mut TextRenderer, label: &str, top: f32) {
    text.draw(
        pixmap,
        label,
        LABEL_SIZE,
        ui::PADDING,
        line_top(top, ui::ROW_HEIGHT, LABEL_SIZE),
        LABEL,
    );
}

fn highlight(pixmap: &mut Pixmap, top: f32, width: f32) {
    let Some(rect) = Rect::from_xywh(ui::PADDING / 2.0, top, width - ui::PADDING, ui::ROW_HEIGHT)
    else {
        return;
    };
    if let Some(path) = rounded_rect(rect, 6.0) {
        fill(pixmap, &path, ROW_HOVER);
    }
}

fn toggle(pixmap: &mut Pixmap, (x, y, w, h): (f32, f32, f32, f32), on: bool) {
    let Some(rect) = Rect::from_xywh(x, y, w, h) else {
        return;
    };
    if let Some(path) = rounded_rect(rect, h / 2.0) {
        fill(pixmap, &path, if on { ACCENT } else { TRACK });
    }

    let r = h / 2.0 - 3.0;
    let cx = if on { x + w - h / 2.0 } else { x + h / 2.0 };
    if let Some(path) = PathBuilder::from_circle(cx, y + h / 2.0, r) {
        fill(pixmap, &path, KNOB);
    }
}

fn slider(pixmap: &mut Pixmap, (x, y, w, h): (f32, f32, f32, f32), fraction: f32) {
    let cy = y + h / 2.0;
    let track_y = cy - ui::TRACK_HEIGHT / 2.0;

    if let Some(rect) = Rect::from_xywh(x, track_y, w, ui::TRACK_HEIGHT) {
        if let Some(path) = rounded_rect(rect, ui::TRACK_HEIGHT / 2.0) {
            fill(pixmap, &path, TRACK);
        }
    }
    // The filled part shows how far along the range the value sits.
    if fraction > 0.0 {
        if let Some(rect) = Rect::from_xywh(x, track_y, w * fraction, ui::TRACK_HEIGHT) {
            if let Some(path) = rounded_rect(rect, ui::TRACK_HEIGHT / 2.0) {
                fill(pixmap, &path, ACCENT);
            }
        }
    }
    if let Some(path) = PathBuilder::from_circle(x + w * fraction, cy, ui::KNOB_RADIUS) {
        fill(pixmap, &path, KNOB);
    }
}

fn fill(pixmap: &mut Pixmap, path: &tiny_skia::Path, color: Color) {
    let mut paint = Paint {
        anti_alias: true,
        ..Default::default()
    };
    paint.set_color(color.to_skia());
    pixmap.fill_path(path, &paint, FillRule::Winding, Transform::identity(), None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn render() -> Pixmap {
        let controls = ui::controls(&Config::default());
        let (_, h) = ui::rows(&controls);
        let mut pixmap = Pixmap::new(ui::WINDOW_WIDTH as u32, h as u32).unwrap();
        let mut text = TextRenderer::new();
        draw(&mut pixmap, &mut text, &controls, ui::WINDOW_WIDTH, None);
        if let Ok(path) = std::env::var("KDOCK_DUMP_SETTINGS") {
            pixmap.save_png(path).unwrap();
        }
        pixmap
    }

    fn is_background(p: tiny_skia::PremultipliedColorU8) -> bool {
        let bg = BACKGROUND.to_skia().to_color_u8();
        p.red() == bg.red() && p.green() == bg.green() && p.blue() == bg.blue()
    }

    /// The window's own margins have to be respected on both sides, or a
    /// control would sit flush against the frame.
    #[test]
    fn nothing_is_drawn_into_the_margins() {
        let pixmap = render();
        let left = (ui::PADDING / 2.0) as u32;
        let right = pixmap.width() - left;

        for y in 0..pixmap.height() {
            for x in 0..left {
                assert!(
                    is_background(pixmap.pixel(x, y).unwrap()),
                    "something is drawn at x={x}, inside the left margin"
                );
            }
            for x in right..pixmap.width() {
                assert!(
                    is_background(pixmap.pixel(x, y).unwrap()),
                    "something is drawn at x={x}, inside the right margin"
                );
            }
        }
    }

    /// The label has to sit level with the control it belongs to. `draw` takes
    /// the top of the line box rather than a baseline, and treating it as a
    /// baseline puts every label about half a line low -- visible, but easy to
    /// stare past.
    #[test]
    fn labels_are_vertically_centred_in_their_rows() {
        let pixmap = render();
        let controls = ui::controls(&Config::default());
        let (tops, _) = ui::rows(&controls);

        for (control, top) in controls.iter().zip(&tops) {
            if !matches!(control, Control::Toggle { .. } | Control::Slider { .. }) {
                continue;
            }
            // Ink in the label column only, so the control on the right cannot
            // stand in for a label that is not there.
            let rows: Vec<u32> = ((*top as u32)..((top + control.height()) as u32))
                .filter(|y| {
                    (ui::PADDING as u32..200).any(|x| !is_background(pixmap.pixel(x, *y).unwrap()))
                })
                .collect();

            let (first, last) = (rows[0] as f32, *rows.last().unwrap() as f32);
            let ink_centre = (first + last) / 2.0;
            let row_centre = top + control.height() / 2.0;
            assert!(
                (ink_centre - row_centre).abs() < 4.0,
                "label ink centred at {ink_centre}, row centre is {row_centre}"
            );
        }
    }

    /// Every row has to actually put something on screen; a control that draws
    /// nothing would leave a silent gap in the window.
    #[test]
    fn every_row_draws_something() {
        let pixmap = render();
        let controls = ui::controls(&Config::default());
        let (tops, _) = ui::rows(&controls);

        for (control, top) in controls.iter().zip(&tops) {
            let y0 = *top as u32;
            let y1 = (top + control.height()) as u32;
            let painted = (y0..y1.min(pixmap.height()))
                .any(|y| (0..pixmap.width()).any(|x| !is_background(pixmap.pixel(x, y).unwrap())));
            assert!(painted, "a row between y={y0} and y={y1} is blank");
        }
    }
}
