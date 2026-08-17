//! Magnification: working out how wide each slot is, then where it sits.
//!
//! Magnification is not each icon scaling on its own -- the whole row is laid
//! out again. First work out how wide every slot should be given its distance
//! from the cursor, then accumulate those widths to get each slot's position.
//! Neighbours get pushed aside, which is what makes it read as macOS rather
//! than as icons growing in place.
//!
//! Distances are measured against each slot's *resting* centre rather than its
//! deformed position. That breaks the feedback loop where position affects
//! scale and scale affects position, so the layout stays stable instead of
//! oscillating. The cost is that the icon under the cursor drifts slightly from
//! the pointer; calibrating that against a reference is a separate step.
//!
//! Ported from the Qt implementation's `relayout()`
//! (`git show ee6971b:qml/DockPanel.qml`).

use crate::metrics::Metrics;

/// What the layout needs to know about one slot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlotMetrics {
    /// Width at rest, in pixels.
    pub rest_width: f32,
    /// Separators keep their width; only icons grow.
    pub magnifies: bool,
}

/// Where a slot ended up.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SlotGeometry {
    pub x: f32,
    pub width: f32,
    /// How far the icon is lifted above its resting position, for the launch
    /// bounce. Does not affect the row's horizontal layout.
    pub lift: f32,
}

impl SlotGeometry {
    pub fn centre(&self) -> f32 {
        self.x + self.width / 2.0
    }

    pub fn contains(&self, x: f32) -> bool {
        x >= self.x && x < self.x + self.width
    }
}

/// The laid-out row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layout {
    pub slots: Vec<SlotGeometry>,
    /// Sum of all slot widths.
    pub content_width: f32,
}

impl Layout {
    /// Which slot is under a point in row-local coordinates.
    pub fn hit(&self, x: f32) -> Option<usize> {
        self.slots.iter().position(|s| s.contains(x))
    }

    /// Rebuilds positions from a set of widths.
    ///
    /// Needed because the widths actually drawn are eased toward the target
    /// layout rather than taken from it directly, and their positions have to
    /// be re-accumulated from the eased values or the row would tear apart.
    pub fn from_widths(widths: &[f32]) -> Self {
        let mut slots = Vec::with_capacity(widths.len());
        let mut total = 0.0;
        for &width in widths {
            slots.push(SlotGeometry {
                x: total,
                width,
                lift: 0.0,
            });
            total += width;
        }
        Layout {
            slots,
            content_width: total,
        }
    }

    pub fn widths(&self) -> Vec<f32> {
        self.slots.iter().map(|s| s.width).collect()
    }
}

/// Height of the launch bounce above the resting position.
///
/// The rise and the fall get opposite easings rather than one symmetric curve:
/// a real jump decelerates on the way up and accelerates on the way down.
/// Easing both halves the same way reads as floaty rather than as a jump.
///
/// The hop is followed by a pause, and the whole cycle repeats for as long as
/// the application is still starting.
pub fn bounce_offset(elapsed_ms: f32, metrics: &Metrics) -> f32 {
    let hop = metrics.bounce_duration_ms as f32;
    let cycle = hop + metrics.bounce_rest_duration_ms as f32;
    if hop <= 0.0 || cycle <= 0.0 || elapsed_ms < 0.0 {
        return 0.0;
    }

    let t = elapsed_ms % cycle;
    if t >= hop {
        // Resting on the ground between hops.
        return 0.0;
    }

    let height = metrics.pt(metrics.bounce_height);
    // The fall takes slightly longer than the rise, which is what stops the
    // motion looking mechanically symmetric.
    let rise = hop * 0.45;

    if t < rise {
        let p = t / rise;
        // Ease-out: fastest at take-off, stalling at the apex.
        height * (1.0 - (1.0 - p) * (1.0 - p))
    } else {
        let p = (t - rise) / (hop - rise);
        // Ease-in: hangs at the apex, then accelerates into the landing.
        height * (1.0 - p * p)
    }
}

/// The order slots would be in if a drag were dropped now.
///
/// Returns original indices in their new positions, so the caller can permute
/// its own parallel arrays and undo the permutation afterwards. Reordering the
/// real list mid-drag instead would mean the model changed on every pointer
/// motion, and a drop that is cancelled would have to be unwound.
pub fn drag_order(len: usize, from: usize, insert: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..len).collect();
    if from >= len {
        return order;
    }
    let item = order.remove(from);
    order.insert(insert.min(order.len()), item);
    order
}

/// Where a dragged slot would land, given the pointer's position in
/// resting-layout coordinates.
///
/// Measured against the resting centres of the *other* slots: the dragged one
/// is out of the row while it is in the air, so including it would make the gap
/// chase the pointer by one position.
pub fn insert_index(slots: &[SlotMetrics], from: usize, pointer_x: f32) -> usize {
    let mut acc = 0.0;
    let mut index = 0;

    for (i, s) in slots.iter().enumerate() {
        if i == from {
            continue;
        }
        let centre = acc + s.rest_width / 2.0;
        if pointer_x < centre {
            return index;
        }
        acc += s.rest_width;
        index += 1;
    }
    index
}

/// Left edge of the row when nothing is magnified.
///
/// The row is centred, so this moves whenever the row's resting width changes
/// -- which is what makes caching a pointer position converted through it a
/// trap: a tile joining or leaving shifts the origin, and a value converted
/// before the change then describes somewhere the pointer is not.
pub fn rest_origin_x(surface_w: f32, rest_content_w: f32, padding: f32) -> f32 {
    (surface_w - (rest_content_w + padding * 2.0)) / 2.0 + padding
}

/// Moves `current` toward `target` by one frame's worth of easing.
///
/// Exponential rather than a fixed-duration tween because the target moves
/// continuously as the pointer does: a tween would have to be restarted every
/// frame, and restarting an ease-out every frame produces a visible stutter.
/// This is frame-rate independent, so a dropped frame slows nothing down.
pub fn approach(current: f32, target: f32, dt_ms: f32, duration_ms: f32) -> f32 {
    if duration_ms <= 0.0 || dt_ms <= 0.0 {
        return target;
    }
    // Time constant set so the gap is ~95% closed after `duration_ms`.
    let tau = duration_ms / 3.0;
    current + (target - current) * (1.0 - (-dt_ms / tau).exp())
}

/// Lays out the row.
///
/// `cursor_rest_x` is the pointer's position in *resting-layout* coordinates,
/// or `None` when the pointer is away and the row should collapse.
pub fn layout(slots: &[SlotMetrics], cursor_rest_x: Option<f32>, metrics: &Metrics) -> Layout {
    // Resting centres have to be accumulated rather than derived from the
    // index, because the slots are not all the same width.
    let mut rest_centres = Vec::with_capacity(slots.len());
    let mut acc = 0.0;
    for s in slots {
        rest_centres.push(acc + s.rest_width / 2.0);
        acc += s.rest_width;
    }

    let magnifying = cursor_rest_x.filter(|_| metrics.magnification_enabled);
    let radius = metrics.magnification_range * metrics.pt(metrics.tile_size);
    let max_scale = metrics.max_scale();

    let mut out = Vec::with_capacity(slots.len());
    let mut total = 0.0;

    for (i, s) in slots.iter().enumerate() {
        let mut scale = 1.0;

        if let Some(cursor) = magnifying {
            if s.magnifies && radius > 0.0 {
                let d = (cursor - rest_centres[i]).abs();
                if d < radius {
                    // Raised cosine: peaks at max_scale directly under the
                    // cursor and decays to exactly 1 at the edge, with a
                    // continuous first derivative -- so the row has no visible
                    // kink where the influence ends.
                    scale = 1.0
                        + (max_scale - 1.0)
                            * 0.5
                            * ((std::f32::consts::PI * (d / radius)).cos() + 1.0);
                }
            }
        }

        let width = s.rest_width * scale;
        out.push(SlotGeometry {
            x: total,
            width,
            lift: 0.0,
        });
        total += width;
    }

    Layout {
        slots: out,
        content_width: total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiles(n: usize, m: &Metrics) -> Vec<SlotMetrics> {
        vec![
            SlotMetrics {
                rest_width: m.pt(m.tile_size),
                magnifies: true
            };
            n
        ]
    }

    #[test]
    fn without_a_cursor_every_slot_is_at_rest() {
        let m = Metrics::default();
        let l = layout(&tiles(5, &m), None, &m);

        let tile = m.pt(m.tile_size);
        assert!(l.slots.iter().all(|s| (s.width - tile).abs() < 0.001));
        assert!((l.content_width - tile * 5.0).abs() < 0.001);
        // Positions accumulate.
        assert!((l.slots[3].x - tile * 3.0).abs() < 0.001);
    }

    /// The slot directly under the cursor reaches full magnification, and the
    /// row gets wider overall.
    #[test]
    fn the_slot_under_the_cursor_peaks() {
        let m = Metrics::default();
        let tile = m.pt(m.tile_size);
        let slots = tiles(5, &m);
        // Centre of slot 2 in resting coordinates.
        let l = layout(&slots, Some(tile * 2.5), &m);

        let peak = tile * m.max_scale();
        assert!(
            (l.slots[2].width - peak).abs() < 0.01,
            "got {}",
            l.slots[2].width
        );
        assert!(
            l.content_width > tile * 5.0,
            "magnifying must widen the row"
        );
    }

    /// Falloff reaches exactly 1.0 at the influence radius; a discontinuity
    /// there would show up as a visible kink as the pointer crosses it.
    #[test]
    fn falloff_returns_to_rest_at_the_radius() {
        let m = Metrics::default();
        let tile = m.pt(m.tile_size);
        let radius = m.magnification_range * tile;
        let slots = tiles(1, &m);

        // Cursor exactly one radius from the single slot's resting centre.
        let l = layout(&slots, Some(tile / 2.0 + radius), &m);
        assert!(
            (l.slots[0].width - tile).abs() < 0.001,
            "got {}",
            l.slots[0].width
        );
    }

    #[test]
    fn magnification_is_symmetric_about_the_cursor() {
        let m = Metrics::default();
        let tile = m.pt(m.tile_size);
        let slots = tiles(7, &m);
        // Cursor at the centre of the middle slot.
        let l = layout(&slots, Some(tile * 3.5), &m);

        assert!((l.slots[2].width - l.slots[4].width).abs() < 0.01);
        assert!((l.slots[1].width - l.slots[5].width).abs() < 0.01);
    }

    /// Separators must keep their width -- the whole reason slots carry a
    /// `magnifies` flag rather than the layout assuming a uniform row.
    #[test]
    fn non_magnifying_slots_keep_their_width() {
        let m = Metrics::default();
        let tile = m.pt(m.tile_size);
        let sep_w = m.pt(20.0);
        let slots = vec![
            SlotMetrics {
                rest_width: tile,
                magnifies: true,
            },
            SlotMetrics {
                rest_width: sep_w,
                magnifies: false,
            },
            SlotMetrics {
                rest_width: tile,
                magnifies: true,
            },
        ];

        // Park the cursor right on the separator.
        let l = layout(&slots, Some(tile + sep_w / 2.0), &m);
        assert!((l.slots[1].width - sep_w).abs() < 0.001);
        // ...while its neighbours still grow.
        assert!(l.slots[0].width > tile);
    }

    #[test]
    fn magnification_disabled_leaves_the_row_at_rest() {
        let m = Metrics {
            magnification_enabled: false,
            ..Metrics::default()
        };
        let tile = m.pt(m.tile_size);
        let l = layout(&tiles(5, &m), Some(tile * 2.5), &m);

        assert!(l.slots.iter().all(|s| (s.width - tile).abs() < 0.001));
    }

    #[test]
    fn hit_testing_finds_the_slot_under_a_point() {
        let m = Metrics::default();
        let tile = m.pt(m.tile_size);
        let l = layout(&tiles(4, &m), None, &m);

        assert_eq!(l.hit(tile * 0.5), Some(0));
        assert_eq!(l.hit(tile * 2.5), Some(2));
        assert_eq!(l.hit(-1.0), None);
        assert_eq!(l.hit(tile * 4.0 + 1.0), None);
    }

    #[test]
    fn from_widths_reaccumulates_positions() {
        let l = Layout::from_widths(&[10.0, 20.0, 5.0]);
        assert_eq!(l.slots[0].x, 0.0);
        assert_eq!(l.slots[1].x, 10.0);
        assert_eq!(l.slots[2].x, 30.0);
        assert_eq!(l.content_width, 35.0);
    }

    #[test]
    fn approach_closes_most_of_the_gap_over_the_duration() {
        let after = approach(0.0, 100.0, 90.0, 90.0);
        assert!(after > 90.0 && after < 100.0, "got {after}");
    }

    /// Frame-rate independence: two half-steps must land where one full step
    /// does, or the animation would run at different speeds on different
    /// refresh rates.
    #[test]
    fn approach_is_frame_rate_independent() {
        let one_step = approach(0.0, 100.0, 32.0, 90.0);
        let two_steps = approach(approach(0.0, 100.0, 16.0, 90.0), 100.0, 16.0, 90.0);
        assert!(
            (one_step - two_steps).abs() < 0.01,
            "{one_step} vs {two_steps}"
        );
    }

    /// Why the caller has to cap `dt_ms` well below the duration: a single step
    /// longer than the animation finishes it outright. The dock asks for no
    /// frames while it is still, so an uncapped gap since the last callback is
    /// however long it sat idle -- and feeding that in makes a fresh animation
    /// land on its first frame, which looks exactly like no animation at all.
    #[test]
    fn a_step_longer_than_the_duration_finishes_it_outright() {
        let after = approach(0.0, 100.0, 100.0, 90.0);
        assert!(after > 96.0, "expected essentially finished, got {after}");
    }

    /// At a plausible frame time the same animation has barely started, which
    /// is what leaves room for the frames in between to be seen.
    #[test]
    fn one_frame_moves_only_part_of_the_way() {
        let after = approach(0.0, 100.0, 16.0, 220.0);
        assert!(
            (5.0..30.0).contains(&after),
            "expected a small first step, got {after}"
        );
    }

    /// Losing a tile shifts the resting origin by half that tile, because the
    /// row stays centred. A pointer position converted before the change and
    /// reused after it is therefore off by exactly that much -- the reason the
    /// conversion is done on demand rather than stored.
    #[test]
    fn the_resting_origin_moves_when_the_row_loses_a_tile() {
        let (surface, pad, tile) = (1920.0, 8.0, 64.0);
        let before = rest_origin_x(surface, tile * 4.0, pad);
        let after = rest_origin_x(surface, tile * 3.0, pad);

        assert!(
            (after - before - tile / 2.0).abs() < 0.01,
            "{before} -> {after}"
        );
    }

    /// The same physical pointer position has to keep meaning the same place.
    #[test]
    fn converting_on_demand_keeps_the_pointer_where_it_is() {
        let (surface, pad, tile) = (1920.0, 8.0, 64.0);
        let pointer = 1000.0;

        let wide = pointer - rest_origin_x(surface, tile * 4.0, pad);
        let narrow = pointer - rest_origin_x(surface, tile * 3.0, pad);

        // Different resting coordinates, but both name the same screen pixel.
        assert!(wide > narrow);
        assert!((wide + rest_origin_x(surface, tile * 4.0, pad) - pointer).abs() < 0.01);
        assert!((narrow + rest_origin_x(surface, tile * 3.0, pad) - pointer).abs() < 0.01);
    }

    #[test]
    fn approach_with_no_duration_snaps() {
        assert_eq!(approach(0.0, 100.0, 16.0, 0.0), 100.0);
    }

    #[test]
    fn bounce_starts_and_ends_on_the_ground() {
        let m = Metrics::default();
        assert_eq!(bounce_offset(0.0, &m), 0.0);
        // End of the hop, before the pause.
        let hop = m.bounce_duration_ms as f32;
        assert!(bounce_offset(hop - 0.01, &m).abs() < 0.5);
    }

    #[test]
    fn bounce_reaches_its_full_height_at_the_apex() {
        let m = Metrics::default();
        let apex = m.bounce_duration_ms as f32 * 0.45;
        let h = bounce_offset(apex, &m);
        assert!((h - m.pt(m.bounce_height)).abs() < 0.01, "got {h}");
    }

    /// The icon must sit still between hops, not hover.
    #[test]
    fn bounce_rests_between_hops() {
        let m = Metrics::default();
        let during_pause = m.bounce_duration_ms as f32 + 10.0;
        assert_eq!(bounce_offset(during_pause, &m), 0.0);
    }

    #[test]
    fn bounce_repeats_every_cycle() {
        let m = Metrics::default();
        let cycle = (m.bounce_duration_ms + m.bounce_rest_duration_ms) as f32;
        let apex = m.bounce_duration_ms as f32 * 0.45;
        assert!((bounce_offset(apex, &m) - bounce_offset(apex + cycle, &m)).abs() < 0.01);
    }

    /// The rise decelerates and the fall accelerates -- a real jump, rather
    /// than the floaty look a single symmetric easing gives.
    ///
    /// Checked as distance covered in the first quarter of each half: leaving
    /// the ground it should already be past a quarter of the height, while
    /// leaving the apex it should barely have dropped.
    #[test]
    fn the_rise_decelerates_and_the_fall_accelerates() {
        let m = Metrics::default();
        let h = m.pt(m.bounce_height);
        let hop = m.bounce_duration_ms as f32;
        let rise = hop * 0.45;

        let climbed = bounce_offset(rise * 0.25, &m);
        assert!(
            climbed > h * 0.25,
            "rise should be front-loaded, got {climbed} of {h}"
        );

        let dropped = h - bounce_offset(rise + (hop - rise) * 0.25, &m);
        assert!(
            dropped < h * 0.25,
            "fall should be back-loaded, dropped {dropped} of {h}"
        );
    }

    /// The apex comes before the midpoint of the hop: the fall is given more
    /// time than the rise.
    #[test]
    fn the_fall_lasts_longer_than_the_rise() {
        let m = Metrics::default();
        let hop = m.bounce_duration_ms as f32;
        let apex_t = (0..1000)
            .map(|i| i as f32 * hop / 1000.0)
            .max_by(|a, b| {
                bounce_offset(*a, &m)
                    .partial_cmp(&bounce_offset(*b, &m))
                    .unwrap()
            })
            .unwrap();
        assert!(apex_t < hop / 2.0, "apex at {apex_t} of {hop}");
    }

    // -- dragging ----------------------------------------------------------

    #[test]
    fn dragging_right_shifts_the_slots_it_passes() {
        assert_eq!(drag_order(4, 0, 2), vec![1, 2, 0, 3]);
    }

    #[test]
    fn dragging_left_shifts_the_other_way() {
        assert_eq!(drag_order(4, 3, 1), vec![0, 3, 1, 2]);
    }

    #[test]
    fn dropping_where_it_started_changes_nothing() {
        assert_eq!(drag_order(4, 2, 2), vec![0, 1, 2, 3]);
    }

    /// Every slot must appear exactly once, whatever the indices -- a
    /// permutation that loses or duplicates an entry would drop an icon.
    #[test]
    fn drag_order_is_always_a_permutation() {
        for from in 0..5 {
            for insert in 0..6 {
                let mut got = drag_order(5, from, insert);
                got.sort_unstable();
                assert_eq!(got, vec![0, 1, 2, 3, 4], "from={from} insert={insert}");
            }
        }
    }

    #[test]
    fn an_out_of_range_source_leaves_the_order_alone() {
        assert_eq!(drag_order(3, 9, 0), vec![0, 1, 2]);
    }

    #[test]
    fn insert_index_tracks_the_pointer_across_the_row() {
        let m = Metrics::default();
        let tile = m.pt(m.tile_size);
        let slots = tiles(4, &m);

        // Dragging slot 0: the remaining three occupy 0..3 tiles.
        assert_eq!(insert_index(&slots, 0, 0.0), 0);
        assert_eq!(insert_index(&slots, 0, tile * 0.6), 1);
        assert_eq!(insert_index(&slots, 0, tile * 2.9), 3);
    }

    /// Past the right-hand end the slot lands last, not out of bounds.
    #[test]
    fn insert_index_clamps_at_the_end() {
        let m = Metrics::default();
        let slots = tiles(4, &m);
        let far = m.pt(m.tile_size) * 100.0;
        assert_eq!(
            insert_index(&slots, 1, far),
            3,
            "three others means indices 0..=3"
        );
    }

    #[test]
    fn an_empty_row_lays_out_without_panicking() {
        let m = Metrics::default();
        let l = layout(&[], Some(100.0), &m);
        assert!(l.slots.is_empty());
        assert_eq!(l.content_width, 0.0);
    }
}
