//! The settings window's controls: what they are, where they sit, and what a
//! click at a given point means.
//!
//! Deliberately hand-drawn rather than built on a toolkit. The window is a
//! column of toggles and sliders; a toolkit would bring in a dependency tree
//! larger than the dock itself to draw a dozen rectangles, and the dock already
//! carries everything needed -- tiny-skia to rasterise and cosmic-text to set
//! the labels.
//!
//! Geometry and hit testing live here, apart from the drawing, so the part that
//! decides *what a click did* can be tested without a compositor.

use crate::config::Config;

/// The edges a dock can sit on, in the order the choice shows them.
const EDGES: &[&str] = &["bottom", "left", "right", "top"];

/// Chrome of the settings window, in logical pixels.
pub const PADDING: f32 = 20.0;
pub const ROW_HEIGHT: f32 = 40.0;
pub const HEADING_HEIGHT: f32 = 44.0;
pub const TRACK_WIDTH: f32 = 180.0;
pub const TRACK_HEIGHT: f32 = 4.0;
pub const KNOB_RADIUS: f32 = 8.0;
pub const TOGGLE_WIDTH: f32 = 40.0;
pub const TOGGLE_HEIGHT: f32 = 22.0;
/// A choice needs room for every option at once, so it is wider than the
/// other controls' right-hand column.
pub const CHOICE_WIDTH: f32 = 240.0;
pub const CHOICE_HEIGHT: f32 = 24.0;
pub const WINDOW_WIDTH: f32 = 460.0;

/// One line in the window.
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    Heading(&'static str),
    Toggle {
        key: &'static str,
        label: &'static str,
        value: bool,
    },
    Slider {
        key: &'static str,
        label: &'static str,
        value: f32,
        min: f32,
        max: f32,
        /// Shown after the number, e.g. `pt`.
        unit: &'static str,
    },
    /// One of a handful of named settings, shown side by side.
    ///
    /// A slider cannot say "left", and four separate toggles would let the
    /// user pick two edges at once -- a state the dock has no meaning for.
    Choice {
        key: &'static str,
        label: &'static str,
        /// What is written to the file, in the order they are shown.
        options: &'static [&'static str],
        /// Index into `options`; past its end means nothing is selected.
        value: usize,
    },
}

impl Control {
    pub fn height(&self) -> f32 {
        match self {
            Control::Heading(_) => HEADING_HEIGHT,
            _ => ROW_HEIGHT,
        }
    }
}

/// The window's contents, built from the configuration as it stands.
///
/// Only scalar settings appear. The icon theme and output are free text, and
/// the pinned list is reordered by dragging in the dock itself, so neither
/// belongs in a column of toggles.
pub fn controls(config: &Config) -> Vec<Control> {
    vec![
        Control::Heading("Size"),
        Control::Slider {
            key: "icon_spacing",
            // The gap between icons, nothing more: it widens the row and
            // leaves the panel's height alone.
            label: "Icon spacing",
            value: config.effective_icon_spacing(),
            min: 0.0,
            max: 48.0,
            unit: "pt",
        },
        Control::Slider {
            key: "icon_size",
            label: "Icon size",
            value: config.effective_icon_size(),
            min: 16.0,
            // A fixed range, so this track never rescales while another
            // slider is being dragged -- a `max` tied to the spacing made
            // dragging the spacing visibly drag this knob too.
            max: 96.0,
            unit: "pt",
        },
        Control::Slider {
            key: "large_size",
            label: "Magnified spacing",
            value: config.large_size,
            min: 48.0,
            max: 160.0,
            unit: "pt",
        },
        Control::Toggle {
            key: "magnification",
            label: "Magnification",
            value: config.magnification,
        },
        Control::Slider {
            key: "magnification_range",
            label: "Magnification reach",
            value: config.magnification_range,
            min: 1.0,
            max: 5.0,
            unit: "tiles",
        },
        Control::Heading("Panel"),
        Control::Choice {
            key: "position",
            label: "Position",
            options: EDGES,
            value: EDGES
                .iter()
                .position(|e| *e == config.edge().name())
                .unwrap_or(0),
        },
        Control::Slider {
            key: "panel_height",
            label: "Height",
            // Fixed while icons resize inside it -- the whole point of the
            // setting -- so icons only force it up when they stop fitting.
            value: config.panel_height.unwrap_or(85.0),
            min: 48.0,
            max: 128.0,
            unit: "pt",
        },
        Control::Slider {
            key: "panel_radius",
            label: "Corner radius",
            value: config.panel_radius,
            min: 0.0,
            max: 0.5,
            unit: "of height",
        },
        Control::Slider {
            key: "panel_padding",
            label: "Edge padding",
            value: config.panel_padding,
            min: 0.0,
            max: 32.0,
            unit: "pt",
        },
        Control::Slider {
            key: "bottom_gap",
            // Moves the dock: how far it floats above the screen's edge.
            label: "Distance from edge",
            value: config.bottom_gap,
            min: 0.0,
            max: 48.0,
            unit: "pt",
        },
        Control::Slider {
            key: "window_gap",
            // Moves the windows: how much room they leave above the dock.
            label: "Distance from windows",
            value: config.window_gap,
            min: 0.0,
            max: 48.0,
            unit: "pt",
        },
        Control::Heading("Behaviour"),
        Control::Toggle {
            key: "auto_hide",
            label: "Hide until pointed at",
            value: config.auto_hide,
        },
        Control::Toggle {
            key: "show_trash",
            label: "Show Trash",
            value: config.show_trash,
        },
        Control::Toggle {
            key: "separate_minimized",
            label: "Minimised windows get their own tile",
            value: config.separate_minimized,
        },
    ]
}

/// Top edge of each control, and the window height they add up to.
pub fn rows(controls: &[Control]) -> (Vec<f32>, f32) {
    let mut tops = Vec::with_capacity(controls.len());
    let mut y = PADDING;
    for c in controls {
        tops.push(y);
        y += c.height();
    }
    (tops, y + PADDING)
}

/// Where a control's interactive part sits, right-aligned in the window.
pub fn control_rect(control: &Control, top: f32, width: f32) -> Option<(f32, f32, f32, f32)> {
    match control {
        Control::Heading(_) => None,
        Control::Toggle { .. } => Some((
            width - PADDING - TOGGLE_WIDTH,
            top + (ROW_HEIGHT - TOGGLE_HEIGHT) / 2.0,
            TOGGLE_WIDTH,
            TOGGLE_HEIGHT,
        )),
        Control::Slider { .. } => Some((
            width - PADDING - TRACK_WIDTH,
            top + ROW_HEIGHT / 2.0 - KNOB_RADIUS,
            TRACK_WIDTH,
            KNOB_RADIUS * 2.0,
        )),
        Control::Choice { .. } => Some((
            width - PADDING - CHOICE_WIDTH,
            top + (ROW_HEIGHT - CHOICE_HEIGHT) / 2.0,
            CHOICE_WIDTH,
            CHOICE_HEIGHT,
        )),
    }
}

/// Where one option of a choice sits inside its control.
pub fn choice_segment(rect: (f32, f32, f32, f32), count: usize, i: usize) -> (f32, f32, f32, f32) {
    let (x, y, w, h) = rect;
    let each = w / count.max(1) as f32;
    (x + each * i as f32, y, each, h)
}

/// What a press at a point means.
#[derive(Debug, Clone, PartialEq)]
pub enum Hit {
    /// Flip a toggle.
    Toggle(usize),
    /// Drag a slider; carries the value the pointer lands on.
    Slider(usize, f32),
    /// Pick one option of a choice; carries which one.
    Choice(usize, usize),
}

/// Finds the control under a point.
///
/// A slider responds anywhere along its row, not only on the knob: aiming at an
/// 8px circle is needlessly fussy, and every real slider lets you click the
/// track.
pub fn hit(controls: &[Control], width: f32, x: f32, y: f32) -> Option<Hit> {
    let (tops, _) = rows(controls);

    for (i, (control, top)) in controls.iter().zip(&tops).enumerate() {
        if y < *top || y >= top + control.height() {
            continue;
        }
        match control {
            Control::Heading(_) => return None,
            Control::Toggle { .. } => {
                // The whole row toggles, so the label is a target too.
                return Some(Hit::Toggle(i));
            }
            Control::Slider { min, max, .. } => {
                let (tx, _, tw, _) = control_rect(control, *top, width)?;
                let fraction = ((x - tx) / tw).clamp(0.0, 1.0);
                return Some(Hit::Slider(i, min + (max - min) * fraction));
            }
            Control::Choice { options, .. } => {
                // Only the control itself, not the whole row: a choice has no
                // "flip it" reading, so a click on the label means nothing.
                let (cx, _, cw, _) = control_rect(control, *top, width)?;
                if x < cx || x >= cx + cw {
                    return None;
                }
                let n = options.len().max(1);
                let picked = (((x - cx) / cw) * n as f32) as usize;
                return Some(Hit::Choice(i, picked.min(n - 1)));
            }
        }
    }
    None
}

/// Rounds a slider value to something worth writing to the file.
///
/// Sub-pixel precision in a config file is noise: it makes the file ugly and
/// the difference is invisible.
pub fn quantise(value: f32, unit: &str) -> f32 {
    match unit {
        "tiles" => (value * 10.0).round() / 10.0,
        // A radius runs 0..0.5, so whole numbers would leave two usable stops.
        "of height" => (value * 100.0).round() / 100.0,
        _ => value.round(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sliders_and_toggles() -> Vec<Control> {
        controls(&Config::default())
    }

    /// Every control has to name a field the config actually has. `Config`
    /// denies unknown fields, so a typo here shows up as a parse failure --
    /// which is the whole point: otherwise the window would write a key the
    /// dock silently refuses to read.
    #[test]
    fn every_control_names_a_real_config_field() {
        let mut doc = String::new();
        for c in sliders_and_toggles() {
            match c {
                Control::Heading(_) => {}
                Control::Toggle { key, .. } => doc.push_str(&format!("{key} = true\n")),
                Control::Slider { key, min, .. } => doc.push_str(&format!("{key} = {min:?}\n")),
                Control::Choice { key, options, .. } => {
                    doc.push_str(&format!("{key} = {:?}\n", options[0]))
                }
            }
        }
        Config::parse(&doc).expect("a control names a field the config does not have");
    }

    /// The ranges have to admit the defaults, or opening the window would show
    /// every slider pinned to an end and writing one would change the value.
    #[test]
    fn the_defaults_sit_inside_the_offered_ranges() {
        for c in sliders_and_toggles() {
            if let Control::Slider {
                key,
                value,
                min,
                max,
                ..
            } = c
            {
                assert!(
                    (min..=max).contains(&value),
                    "{key}: default {value} is outside {min}..={max}"
                );
            }
        }
    }

    #[test]
    fn rows_stack_without_overlapping() {
        let controls = sliders_and_toggles();
        let (tops, height) = rows(&controls);

        for (i, (c, top)) in controls.iter().zip(&tops).enumerate() {
            if let Some(next) = tops.get(i + 1) {
                assert!(
                    (top + c.height() - next).abs() < 0.01,
                    "row {i} leaves a gap or overlaps"
                );
            }
        }
        assert!(height > *tops.last().unwrap());
    }

    /// Found by key rather than by position: the row order changes whenever a
    /// setting is added, and a hard-coded index only fails later.
    fn index_of(controls: &[Control], wanted: &str) -> usize {
        controls
            .iter()
            .position(|c| match c {
                Control::Heading(_) => false,
                Control::Toggle { key, .. }
                | Control::Slider { key, .. }
                | Control::Choice { key, .. } => *key == wanted,
            })
            .unwrap_or_else(|| panic!("no control for {wanted}"))
    }

    /// The icon-size slider must not move when the spacing does. Its `max` was
    /// once tied to `tile_size`, which rescaled the track on every spacing
    /// write and made dragging one slider visibly drag the other's knob.
    #[test]
    fn changing_the_spacing_leaves_the_icon_slider_alone() {
        let narrow = Config::parse("icon_spacing = 4.0\nicon_size = 37.0").unwrap();
        let wide = Config::parse("icon_spacing = 48.0\nicon_size = 37.0").unwrap();

        let a = controls(&narrow);
        let b = controls(&wide);
        let icon = index_of(&a, "icon_size");
        assert_eq!(a[icon], b[index_of(&b, "icon_size")]);
    }

    /// A choice is picked by the segment under the pointer, so every option
    /// has to be reachable and the label beside them must not swallow clicks.
    #[test]
    fn a_choice_picks_the_option_under_the_pointer() {
        let controls = sliders_and_toggles();
        let (tops, _) = rows(&controls);
        let i = index_of(&controls, "position");
        let top = tops[i];
        let y = top + ROW_HEIGHT / 2.0;
        let (cx, _, cw, _) = control_rect(&controls[i], top, WINDOW_WIDTH).unwrap();
        let Control::Choice { options, .. } = controls[i] else {
            panic!("position should be a choice")
        };

        for (n, _) in options.iter().enumerate() {
            let centre = cx + cw * (n as f32 + 0.5) / options.len() as f32;
            assert_eq!(
                hit(&controls, WINDOW_WIDTH, centre, y),
                Some(Hit::Choice(i, n)),
                "option {n} is not reachable"
            );
        }
        // The far right edge belongs to the last option, not to one past it.
        assert_eq!(
            hit(&controls, WINDOW_WIDTH, cx + cw - 0.5, y),
            Some(Hit::Choice(i, options.len() - 1))
        );
        // ...and the label does nothing, unlike a toggle's row.
        assert_eq!(hit(&controls, WINDOW_WIDTH, PADDING, y), None);
    }

    #[test]
    fn a_click_on_a_toggle_row_flips_that_toggle() {
        let controls = sliders_and_toggles();
        let (tops, _) = rows(&controls);
        let i = index_of(&controls, "magnification");
        let y = tops[i] + ROW_HEIGHT / 2.0;

        assert_eq!(
            hit(&controls, WINDOW_WIDTH, WINDOW_WIDTH / 2.0, y),
            Some(Hit::Toggle(i))
        );
    }

    /// Clicking the far ends of a track gives the extremes, which is what makes
    /// the control usable without dragging.
    #[test]
    fn a_slider_maps_its_track_onto_its_range() {
        let controls = sliders_and_toggles();
        let (tops, _) = rows(&controls);
        let i = index_of(&controls, "icon_size");
        let top = tops[i];
        let y = top + ROW_HEIGHT / 2.0;
        let (tx, _, tw, _) = control_rect(&controls[i], top, WINDOW_WIDTH).unwrap();

        let Some(Hit::Slider(_, low)) = hit(&controls, WINDOW_WIDTH, tx, y) else {
            panic!("no slider hit at the left end")
        };
        let Some(Hit::Slider(_, high)) = hit(&controls, WINDOW_WIDTH, tx + tw, y) else {
            panic!("no slider hit at the right end")
        };

        assert!((low - 16.0).abs() < 0.01, "got {low}");
        assert!((high - 96.0).abs() < 0.01, "got {high}");
    }

    /// Dragging past the track must not produce a value outside the range --
    /// a negative icon size would take the dock down with it.
    #[test]
    fn dragging_beyond_the_track_clamps() {
        let controls = sliders_and_toggles();
        let (tops, _) = rows(&controls);
        let y = tops[index_of(&controls, "icon_size")] + ROW_HEIGHT / 2.0;

        let Some(Hit::Slider(_, v)) = hit(&controls, WINDOW_WIDTH, -500.0, y) else {
            panic!("no hit")
        };
        assert!((v - 16.0).abs() < 0.01, "got {v}");

        let Some(Hit::Slider(_, v)) = hit(&controls, WINDOW_WIDTH, 5000.0, y) else {
            panic!("no hit")
        };
        assert!((v - 96.0).abs() < 0.01, "got {v}");
    }

    #[test]
    fn headings_swallow_nothing() {
        let controls = sliders_and_toggles();
        let (tops, _) = rows(&controls);
        let y = tops[0] + HEADING_HEIGHT / 2.0;
        assert_eq!(hit(&controls, WINDOW_WIDTH, WINDOW_WIDTH / 2.0, y), None);
    }

    #[test]
    fn a_click_outside_every_row_hits_nothing() {
        let controls = sliders_and_toggles();
        let (_, height) = rows(&controls);
        assert_eq!(hit(&controls, WINDOW_WIDTH, 10.0, height + 50.0), None);
        assert_eq!(hit(&controls, WINDOW_WIDTH, 10.0, 0.0), None);
    }

    #[test]
    fn quantising_keeps_the_file_tidy() {
        assert_eq!(quantise(64.4, "pt"), 64.0);
        assert_eq!(quantise(2.46, "tiles"), 2.5);
    }
}
