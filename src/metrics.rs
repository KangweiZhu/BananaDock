//! Every pixel value lives in this one file.
//!
//! How pixel-accurate replication works here: take a macOS Dock screenshot at a
//! known resolution and scale factor, measure each value, drop the real numbers
//! in below. No other module needs to change.
//!
//! Values are tagged with their provenance, which is what the calibration pass
//! keys off: `[known]` from Apple's documented defaults, `[reference]` measured
//! off a screenshot, `[derived]` computed from another value, `[measure]` still
//! a starting estimate awaiting a reference.
//!
//! Ported from the Qt implementation's `qml/Metrics.qml` (`git show
//! ee6971b:qml/Metrics.qml`). The ratios below were reverse-engineered from a
//! WWDC25 Tahoe press shot; the `[measure]` values were not.

/// Sizes are expressed in macOS points. `scale` converts points to the
/// surface's logical pixels and doubles as the Dock size preference -- macOS
/// exposes the same thing as a slider. This is NOT the HiDPI buffer scale:
/// output scaling is handled separately at buffer-allocation time.
#[derive(Debug, Clone, Copy)]
pub struct Metrics {
    pub scale: f32,

    // -- Icons -----------------------------------------------------------
    /// Layout PITCH -- the centre-to-centre spacing of icons -- not the size of
    /// the icon artwork. The reference shows those differ.
    pub tile_size: f32,
    /// Magnified peak pitch.
    pub large_size: f32,
    /// Icon artwork is smaller than its tile, leaving gaps between icons.
    /// Ratio measured off the reference: iconArt/pitch = 45/67.
    pub icon_size_ratio: f32,
    /// Influence radius of the cursor, in tiles.
    pub magnification_range: f32,
    pub magnification_enabled: bool,

    // -- Panel -----------------------------------------------------------
    pub panel_padding_h: f32,
    pub panel_bottom_gap: f32,
    /// Panel height relative to the tile pitch, measured off the reference as
    /// 89/67. With the 64pt default pitch this gives an 85pt panel.
    pub panel_height_ratio: f32,
    /// Icons sit slightly above centre; the extra room underneath is where the
    /// running dots go. Reference: 20px above, 24px below, of 89px.
    pub icon_top_pad_ratio: f32,
    /// Tahoe's dock is a capsule -- the end caps are semicircular, so the
    /// radius is exactly half the height.
    pub panel_radius_ratio: f32,
    /// Share of the panel's height the artwork occupies in the reference:
    /// 45 of 89, with 20 above and 24 below.
    ///
    /// The artwork's size can be set independently of the tile pitch, so this
    /// is what keeps the panel from being outgrown: past this fraction the
    /// panel gets taller rather than letting the icon push through its edges.
    pub icon_panel_fraction: f32,

    // -- Auto-hide ---------------------------------------------------------
    /// How long the pointer must be away before the dock slides out.
    pub auto_hide_delay_ms: u32,
    /// One slide, in or out.
    pub auto_hide_slide_ms: u32,

    // -- Separator ---------------------------------------------------------
    /// Slot width a separator occupies. It does not magnify.
    pub separator_width: f32,
    pub separator_line_width: f32,
    /// Vertical inset of the line from the panel's edges.
    pub separator_inset: f32,

    // -- Running indicator dot --------------------------------------------
    pub dot_size: f32,
    /// Dot centre to the panel's bottom edge.
    pub dot_bottom_margin: f32,

    // -- Context menu ------------------------------------------------------
    pub menu_radius: f32,
    pub menu_item_height: f32,
    /// Horizontal padding inside a row.
    pub menu_item_padding: f32,
    pub menu_font_size: f32,
    pub menu_min_width: f32,
    /// Inset of the highlight from the menu's edges.
    pub menu_highlight_inset: f32,
    pub menu_highlight_radius: f32,
    /// Vertical space a separator row occupies.
    pub menu_separator_height: f32,

    // -- Animation --------------------------------------------------------
    /// How tightly magnification tracks the cursor.
    pub magnify_duration_ms: u32,
    /// How long a tile takes to grow in or shrink away when the row changes.
    ///
    /// Deliberately slower than `magnify_duration_ms`: magnification has to
    /// track the pointer tightly, but a tile appearing is a change the eye
    /// should be able to follow.
    pub row_change_ms: u32,
    /// One launch hop.
    pub bounce_duration_ms: u32,
    pub bounce_rest_duration_ms: u32,
    pub bounce_height: f32,
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            scale: 1.0,

            tile_size: 64.0,          // [known] Apple default tile pitch, 64pt
            large_size: 128.0,        // [known] magnified peak pitch, Apple default 128pt
            icon_size_ratio: 0.67,    // [reference] 45/67
            magnification_range: 2.5, // [measure]
            magnification_enabled: true,

            panel_padding_h: 8.0,        // [measure]
            panel_bottom_gap: 8.0,       // [measure]
            panel_height_ratio: 1.33,    // [reference] 89/67
            icon_top_pad_ratio: 0.225,   // [reference] 20/89
            panel_radius_ratio: 0.5,     // [reference] semicircular end caps
            icon_panel_fraction: 0.5056, // [reference] 45/89

            auto_hide_delay_ms: 500, // [measure]
            auto_hide_slide_ms: 180, // [measure]

            separator_width: 20.0,     // [measure]
            separator_line_width: 1.0, // [measure]
            separator_inset: 14.0,     // [measure]

            dot_size: 4.0,          // [measure]
            dot_bottom_margin: 6.0, // [measure]

            menu_radius: 8.0,           // [measure]
            menu_item_height: 24.0,     // [measure]
            menu_item_padding: 12.0,    // [measure]
            menu_font_size: 13.0,       // [measure]
            menu_min_width: 200.0,      // [measure]
            menu_highlight_inset: 4.0,  // [measure]
            menu_highlight_radius: 5.0, // [measure]
            menu_separator_height: 9.0, // [measure]

            magnify_duration_ms: 90,      // [measure]
            row_change_ms: 220,           // [measure]
            bounce_duration_ms: 620,      // [measure]
            bounce_rest_duration_ms: 180, // [measure]
            bounce_height: 28.0,          // [measure]
        }
    }
}

impl Metrics {
    /// Points to logical pixels.
    pub fn pt(&self, value: f32) -> f32 {
        value * self.scale
    }

    pub fn icon_size(&self) -> f32 {
        self.tile_size * self.icon_size_ratio
    }

    /// The panel is sized from the tile pitch, but never smaller than the
    /// artwork needs.
    ///
    /// Without the second term a large icon simply grows through the panel:
    /// `icon_bottom_margin` goes negative and the artwork hangs out of both
    /// edges. Letting the panel grow instead keeps the margins the reference
    /// calls for whatever size the icon is set to.
    pub fn panel_height(&self) -> f32 {
        let from_pitch = self.tile_size * self.panel_height_ratio;
        let from_icon = if self.icon_panel_fraction > 0.0 {
            self.icon_size() / self.icon_panel_fraction
        } else {
            0.0
        };
        from_pitch.max(from_icon)
    }

    pub fn panel_radius(&self) -> f32 {
        self.panel_height() * self.panel_radius_ratio
    }

    /// Gap between the bottom of the icon artwork and the bottom of the panel.
    pub fn icon_bottom_margin(&self) -> f32 {
        self.panel_height() * (1.0 - self.icon_top_pad_ratio) - self.icon_size()
    }

    /// Peak magnification factor. 1.0 when magnification is off.
    pub fn max_scale(&self) -> f32 {
        if self.magnification_enabled {
            self.large_size / self.tile_size
        } else {
            1.0
        }
    }

    /// Icon artwork at full magnification.
    pub fn max_icon_size(&self) -> f32 {
        self.large_size * self.icon_size_ratio
    }

    /// The surface has to contain the panel, the gap below it, and the headroom
    /// a fully magnified icon needs as it grows upward past the panel's top
    /// edge. The surface is deliberately much taller than the panel; the
    /// surplus is transparent and excluded from the input region so it does not
    /// swallow clicks meant for the windows underneath.
    pub fn surface_height(&self) -> f32 {
        let icon_reach = self.icon_bottom_margin() + self.max_icon_size();
        self.pt(self.panel_bottom_gap + self.panel_height().max(icon_reach) + self.bounce_height)
    }
}

/// Colours of the panel material.
///
/// The actual frosted glass is composited by the compositor behind the surface
/// via `ext-background-effect-v1`; what follows is only the translucent tint and
/// highlight stroke layered on top of it. The glass is far more transparent than
/// it first appears -- in the reference the wallpaper's colour reads clearly
/// through the panel. Most of the look comes from the blur, not from this tint,
/// so keeping it light matters: raise it and the blur gets washed out.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub panel_tint: Color,
    /// [reference] A bright hairline runs along both the top and bottom edges.
    pub panel_border: Color,
    pub panel_border_width: f32,
    pub dot: Color,
    pub separator: Color,
    /// Behind an icon a drag is hovering over.
    pub drop_target: Color,

    // -- Context menu ------------------------------------------------------
    /// Deliberately near-opaque: the dock's glass comes from the compositor,
    /// but a popup is a separate surface and would need its own blur region to
    /// match. Until that is wired up, opaque looks less wrong than flatly
    /// translucent.
    pub menu_background: Color,
    pub menu_border: Color,
    pub menu_text: Color,
    pub menu_highlight: Color,
    pub menu_highlight_text: Color,
    pub menu_separator: Color,
}

/// Straight non-premultiplied RGBA, 0.0-1.0.
#[derive(Debug, Clone, Copy)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub fn to_skia(self) -> tiny_skia::Color {
        tiny_skia::Color::from_rgba(self.r, self.g, self.b, self.a)
            .expect("colour components are in range")
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            panel_tint: Color::rgba(1.0, 1.0, 1.0, 0.10),
            panel_border: Color::rgba(1.0, 1.0, 1.0, 0.30),
            panel_border_width: 1.0, // [measure]
            dot: Color::rgba(1.0, 1.0, 1.0, 0.85),
            separator: Color::rgba(1.0, 1.0, 1.0, 0.25),
            drop_target: Color::rgba(1.0, 1.0, 1.0, 0.22),

            menu_background: Color::rgba(0.16, 0.16, 0.17, 0.98),
            menu_border: Color::rgba(1.0, 1.0, 1.0, 0.12),
            menu_text: Color::rgba(1.0, 1.0, 1.0, 0.92),
            menu_highlight: Color::rgba(0.20, 0.48, 0.95, 1.0),
            menu_highlight_text: Color::rgba(1.0, 1.0, 1.0, 1.0),
            menu_separator: Color::rgba(1.0, 1.0, 1.0, 0.12),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// However large the artwork is set, it has to keep the reference's
    /// breathing room inside the panel rather than growing through its edges.
    #[test]
    fn a_large_icon_makes_the_panel_grow_instead_of_overflowing() {
        for ratio in [0.3, 0.5, 0.67, 0.9, 1.0] {
            let m = Metrics {
                icon_size_ratio: ratio,
                ..Metrics::default()
            };
            let margin = m.icon_bottom_margin();
            assert!(
                margin >= 0.0,
                "ratio {ratio}: icon hangs {margin} below the panel"
            );
            // ...and it has to fit above, too.
            let top = m.panel_height() - margin - m.icon_size();
            assert!(
                top >= 0.0,
                "ratio {ratio}: icon overshoots the top by {top}"
            );
        }
    }

    /// The default proportions must not change: the panel grows only when the
    /// artwork is set larger than the reference calls for.
    #[test]
    fn the_reference_proportions_are_untouched_at_the_default() {
        let m = Metrics::default();
        assert!(
            (m.panel_height() - m.tile_size * m.panel_height_ratio).abs() < 0.5,
            "panel height drifted from the reference: {}",
            m.panel_height()
        );
    }

    /// Growing the panel is what keeps the margins in proportion, so the space
    /// above and below should stay in step as the icon grows.
    #[test]
    fn the_margins_stay_in_proportion_as_the_icon_grows() {
        let share = |ratio: f32| {
            let m = Metrics {
                icon_size_ratio: ratio,
                ..Metrics::default()
            };
            m.icon_bottom_margin() / m.panel_height()
        };
        // Beyond the reference the panel is driven by the icon, so the bottom
        // margin settles at a fixed share of the height.
        assert!(
            (share(0.9) - share(1.0)).abs() < 0.02,
            "{} vs {}",
            share(0.9),
            share(1.0)
        );
    }

    /// The derived values are what the whole layout hangs off, and they are
    /// easy to break silently when the ratios get recalibrated.
    #[test]
    fn derived_values_match_the_reference_ratios() {
        let m = Metrics::default();

        assert!(
            (m.panel_height() - 85.12).abs() < 0.01,
            "{}",
            m.panel_height()
        );
        assert!((m.icon_size() - 42.88).abs() < 0.01, "{}", m.icon_size());
        // Capsule: radius is exactly half the height.
        assert!((m.panel_radius() - m.panel_height() / 2.0).abs() < f32::EPSILON);
        assert!((m.max_scale() - 2.0).abs() < f32::EPSILON);
    }

    /// A magnified icon reaches higher than the panel's top edge -- that
    /// headroom is the entire reason the surface is taller than the panel.
    #[test]
    fn surface_is_tall_enough_for_a_magnified_icon() {
        let m = Metrics::default();
        let icon_reach = m.icon_bottom_margin() + m.max_icon_size();

        assert!(
            icon_reach > m.panel_height(),
            "magnified icon should overflow the panel"
        );
        assert!(m.surface_height() >= m.pt(icon_reach + m.panel_bottom_gap));
    }

    #[test]
    fn magnification_can_be_switched_off() {
        let m = Metrics {
            magnification_enabled: false,
            ..Metrics::default()
        };
        assert!((m.max_scale() - 1.0).abs() < f32::EPSILON);
    }
}
