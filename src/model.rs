//! Turning pinned launchers and open windows into the row of tiles to draw.
//!
//! macOS merges the two: a pinned app that is running is the same tile as its
//! windows, not a second one. So slots are keyed by the application, pinned
//! entries come first in their configured order, and anything running but not
//! pinned is appended.

use crate::{
    launchers::LauncherIndex,
    windows::{Toplevel, ToplevelId},
};

/// What a slot represents.
///
/// The row is not one uniform list: macOS puts a separator and the Trash after
/// the applications, and a separator behaves differently from an icon -- it is
/// narrow and must not magnify. Carrying the kind here is what lets the layout
/// treat them differently without a special case per position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SlotKind {
    #[default]
    App,
    Separator,
    /// A single minimised window, shown on its own to the right of the
    /// applications. macOS does this rather than folding the window into its
    /// application's icon, so a minimised window stays individually reachable.
    MinimizedWindow,
    Trash,
}

/// One tile in the dock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub kind: SlotKind,
    /// Desktop entry id when the app was recognised, otherwise its raw
    /// `app_id`. Identity for merging windows into tiles.
    pub key: String,
    pub label: String,
    pub icon_name: Option<String>,
    /// Open windows belonging to this application. Empty for a pinned app that
    /// is not running.
    pub windows: Vec<ToplevelId>,
    /// Any of this application's windows is focused.
    pub active: bool,
    pub pinned: bool,
    /// For a minimised window's tile: the handle its contents can be captured
    /// with, so the tile can show the window rather than its app icon.
    pub capture_key: Option<String>,
}

impl Slot {
    pub fn is_running(&self) -> bool {
        !self.windows.is_empty()
    }

    /// Separators are decoration; everything else responds to the pointer.
    pub fn is_interactive(&self) -> bool {
        self.kind != SlotKind::Separator
    }
}

/// Builds the row.
///
/// `pinned` holds desktop entry ids in the order they should appear.
pub fn build_slots(
    pinned: &[String],
    toplevels: &[Toplevel],
    index: &LauncherIndex,
    trash: Option<TrashState>,
) -> Vec<Slot> {
    build_slots_with(pinned, toplevels, index, trash, true)
}

/// `separate_minimized` mirrors macOS's "Minimize windows into application
/// icon" setting: with it off (the default there, and here) a minimised window
/// gets its own tile; with it on it only shows through its application's icon.
pub fn build_slots_with(
    pinned: &[String],
    toplevels: &[Toplevel],
    index: &LauncherIndex,
    trash: Option<TrashState>,
    separate_minimized: bool,
) -> Vec<Slot> {
    let mut slots: Vec<Slot> = pinned
        .iter()
        .map(|id| match index.by_id(id) {
            Some(l) => Slot {
                kind: SlotKind::App,
                key: l.id.clone(),
                label: l.name.clone(),
                icon_name: l.icon.clone(),
                windows: Vec::new(),
                active: false,
                pinned: true,
                capture_key: None,
            },
            // A pinned entry whose .desktop has since been uninstalled still
            // holds its place rather than silently vanishing.
            None => Slot {
                kind: SlotKind::App,
                key: id.clone(),
                label: id.clone(),
                icon_name: None,
                windows: Vec::new(),
                active: false,
                pinned: true,
                capture_key: None,
            },
        })
        .collect();

    for t in toplevels {
        let matched = index.match_app_id(&t.app_id);
        let key = matched.map_or_else(|| t.app_id.clone(), |l| l.id.clone());

        // An unidentifiable window would otherwise collapse every other
        // unidentifiable window into one tile.
        if key.is_empty() {
            continue;
        }

        match slots.iter_mut().find(|s| s.key == key) {
            Some(slot) => {
                slot.windows.push(t.id);
                slot.active |= t.active;
            }
            None => slots.push(Slot {
                kind: SlotKind::App,
                key,
                label: matched.map_or_else(|| t.app_id.clone(), |l| l.name.clone()),
                icon_name: matched.and_then(|l| l.icon.clone()),
                windows: vec![t.id],
                active: t.active,
                pinned: false,
                capture_key: None,
            }),
        }
    }

    let app_count = slots.len();

    // Everything past the separator: minimised windows first, then the Trash.
    let mut tail: Vec<Slot> = Vec::new();

    if separate_minimized {
        for t in toplevels.iter().filter(|t| t.minimized) {
            let matched = index.match_app_id(&t.app_id);
            tail.push(Slot {
                kind: SlotKind::MinimizedWindow,
                // Keyed by window, not by application: two minimised windows of
                // one application are two tiles.
                key: format!("\u{1}min:{}", t.id),
                // The window's own title, which is what distinguishes them.
                label: if t.title.is_empty() {
                    matched.map_or_else(|| t.app_id.clone(), |l| l.name.clone())
                } else {
                    t.title.clone()
                },
                icon_name: matched.and_then(|l| l.icon.clone()),
                windows: vec![t.id],
                active: false,
                pinned: false,
                capture_key: t.capture_key.clone(),
            });
        }
    }

    if let Some(trash) = trash {
        tail.push(Slot {
            kind: SlotKind::Trash,
            key: "\u{1}trash".to_owned(),
            label: "Trash".to_owned(),
            icon_name: Some(
                if trash.full {
                    "user-trash-full"
                } else {
                    "user-trash"
                }
                .to_owned(),
            ),
            windows: Vec::new(),
            active: false,
            pinned: true,
            capture_key: None,
        });
    }

    // A separator only earns its place when it has something on both sides.
    if !tail.is_empty() && app_count > 0 {
        slots.push(Slot {
            kind: SlotKind::Separator,
            key: "\u{1}separator".to_owned(),
            label: String::new(),
            icon_name: None,
            windows: Vec::new(),
            active: false,
            pinned: true,
            capture_key: None,
        });
    }
    slots.extend(tail);

    slots
}

/// Where each window's tile sits, or would sit once it is minimised.
///
/// The compositor reads this when the window *leaves* and reuses it for the
/// journey back, so telling it after the window is already minimised is too
/// late -- by then it has committed to whatever it was told last, which is the
/// application's icon. A window that is still on screen therefore has to be
/// told where its tile is *going* to appear.
///
/// Returns `(slot index, window id)` pairs. The slot index refers to the layout
/// built with that window minimised, which is why each entry carries its own.
pub fn minimize_targets(
    pinned: &[String],
    toplevels: &[Toplevel],
    index: &LauncherIndex,
    trash: Option<TrashState>,
    separate_minimized: bool,
) -> Vec<WindowTarget> {
    let mut out = Vec::new();

    for t in toplevels {
        // Already minimised: its tile exists, so use the layout as it stands.
        let hypothetical = if t.minimized { None } else { Some(t.id) };
        let slots = match hypothetical {
            None => build_slots_with(pinned, toplevels, index, trash, separate_minimized),
            Some(id) => {
                let pretend: Vec<Toplevel> = toplevels
                    .iter()
                    .map(|w| {
                        let mut w = w.clone();
                        if w.id == id {
                            w.minimized = true;
                        }
                        w
                    })
                    .collect();
                build_slots_with(pinned, &pretend, index, trash, separate_minimized)
            }
        };

        let slot = slots
            .iter()
            .position(|s| s.kind == SlotKind::MinimizedWindow && s.windows.contains(&t.id))
            // With the separate tiles switched off there is no such tile, so
            // the application's icon is the only place to point at.
            .or_else(|| {
                slots
                    .iter()
                    .position(|s| s.kind == SlotKind::App && s.windows.contains(&t.id))
            });

        if let Some(slot) = slot {
            out.push(WindowTarget {
                window: t.id,
                slot,
                slots,
            });
        }
    }

    out
}

/// One window and the row it would be laid out in.
#[derive(Debug, Clone)]
pub struct WindowTarget {
    pub window: ToplevelId,
    /// Index into `slots`.
    pub slot: usize,
    /// The row as it stands, or as it would stand with this window minimised.
    pub slots: Vec<Slot>,
}

/// Merges the row that should be on screen with the one that already is.
///
/// Tiles are matched by key rather than by position, so the row can be animated
/// rather than swapped: a tile that has just appeared can grow from nothing,
/// and one that is leaving stays in the list -- at its old place -- long enough
/// to shrink away. Without this a window minimising makes every neighbouring
/// icon jump sideways in a single frame.
///
/// Departing tiles keep their old index where it still exists, so they shrink
/// where they were rather than sliding to the end first.
pub fn merge_rows(current: &[Slot], target: &[Slot]) -> Vec<Slot> {
    let mut merged = target.to_vec();

    for (old_index, slot) in current.iter().enumerate() {
        if target.iter().any(|t| t.key == slot.key) {
            continue;
        }
        let at = old_index.min(merged.len());
        merged.insert(at, slot.clone());
    }

    merged
}

/// Whether a tile in a merged row is on its way out.
pub fn is_departing(slot: &Slot, target: &[Slot]) -> bool {
    !target.iter().any(|t| t.key == slot.key)
}

/// Whether the Trash currently holds anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrashState {
    pub full: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toplevel(id: ToplevelId, app_id: &str, active: bool) -> Toplevel {
        Toplevel {
            id,
            app_id: app_id.to_owned(),
            active,
            ..Default::default()
        }
    }

    /// Nothing pinned, nothing running: an empty row, not a panic.
    #[test]
    fn empty_inputs_give_an_empty_row() {
        let slots = build_slots(&[], &[], &LauncherIndex::default(), None);
        assert!(slots.is_empty());
    }

    /// Two windows of one application share a tile -- the macOS behaviour, and
    /// the whole point of keying slots by application.
    #[test]
    fn windows_of_one_app_merge_into_a_single_slot() {
        let tops = vec![toplevel(1, "myapp", false), toplevel(2, "myapp", true)];
        let slots = build_slots(&[], &tops, &LauncherIndex::default(), None);

        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].windows, vec![1, 2]);
        assert!(slots[0].active, "focus on any window marks the tile active");
    }

    #[test]
    fn unmatched_windows_keep_their_app_id_as_the_key() {
        let tops = vec![toplevel(1, "some-unknown-app", false)];
        let slots = build_slots(&[], &tops, &LauncherIndex::default(), None);

        assert_eq!(slots[0].key, "some-unknown-app");
        assert!(!slots[0].pinned);
        assert!(slots[0].is_running());
    }

    /// A window reporting no app_id must not become a catch-all tile that every
    /// other anonymous window piles into.
    #[test]
    fn windows_without_an_app_id_are_dropped() {
        let tops = vec![toplevel(1, "", false), toplevel(2, "", false)];
        let slots = build_slots(&[], &tops, &LauncherIndex::default(), None);
        assert!(slots.is_empty());
    }

    /// A pinned entry with no matching .desktop still holds its position.
    #[test]
    fn pinned_entries_survive_an_uninstalled_desktop_file() {
        let slots = build_slots(&["ghost".to_owned()], &[], &LauncherIndex::default(), None);

        assert_eq!(slots.len(), 1);
        assert!(slots[0].pinned);
        assert!(!slots[0].is_running());
    }

    fn minimized(id: ToplevelId, app_id: &str, title: &str) -> Toplevel {
        Toplevel {
            id,
            app_id: app_id.to_owned(),
            title: title.to_owned(),
            minimized: true,
            ..Default::default()
        }
    }

    /// The macOS layout: applications, separator, minimised windows, Trash.
    #[test]
    fn minimized_windows_sit_between_the_separator_and_the_trash() {
        let tops = vec![minimized(1, "myapp", "Doc")];
        let slots = build_slots(
            &[],
            &tops,
            &LauncherIndex::default(),
            Some(TrashState { full: false }),
        );

        let kinds: Vec<_> = slots.iter().map(|s| s.kind).collect();
        assert_eq!(
            kinds,
            vec![
                SlotKind::App,
                SlotKind::Separator,
                SlotKind::MinimizedWindow,
                SlotKind::Trash
            ]
        );
    }

    /// A minimised window keeps its application's tile too -- the app is still
    /// running, so its icon and running dot stay put.
    #[test]
    fn a_minimized_window_does_not_remove_its_application_tile() {
        let tops = vec![minimized(1, "myapp", "Doc")];
        let slots = build_slots(&[], &tops, &LauncherIndex::default(), None);

        assert_eq!(slots[0].kind, SlotKind::App);
        assert!(slots[0].is_running());
        // The separator divides the two regions, so the tile is not at index 1.
        assert!(slots.iter().any(|s| s.kind == SlotKind::MinimizedWindow));
    }

    /// Two minimised windows of one application are two tiles, unlike the
    /// application tiles themselves which merge.
    #[test]
    fn minimized_windows_are_per_window_not_per_application() {
        let tops = vec![minimized(1, "myapp", "One"), minimized(2, "myapp", "Two")];
        let slots = build_slots(&[], &tops, &LauncherIndex::default(), None);

        let mins: Vec<_> = slots
            .iter()
            .filter(|s| s.kind == SlotKind::MinimizedWindow)
            .collect();
        assert_eq!(mins.len(), 2);
        assert_eq!(mins[0].label, "One");
        assert_eq!(mins[1].label, "Two");
        assert_ne!(mins[0].key, mins[1].key);
    }

    /// Each tile has to target its own window, or clicking one would restore
    /// the wrong one.
    #[test]
    fn a_minimized_tile_targets_exactly_its_window() {
        let tops = vec![minimized(7, "myapp", "Doc")];
        let slots = build_slots(&[], &tops, &LauncherIndex::default(), None);
        let tile = slots
            .iter()
            .find(|s| s.kind == SlotKind::MinimizedWindow)
            .unwrap();

        assert_eq!(tile.windows, vec![7]);
    }

    /// A window with no title still needs something readable on its tile.
    #[test]
    fn an_untitled_minimized_window_falls_back_to_the_app_name() {
        let tops = vec![minimized(1, "myapp", "")];
        let slots = build_slots(&[], &tops, &LauncherIndex::default(), None);
        let tile = slots
            .iter()
            .find(|s| s.kind == SlotKind::MinimizedWindow)
            .unwrap();

        assert_eq!(tile.label, "myapp");
    }

    /// The macOS "minimize into application icon" setting.
    #[test]
    fn folding_into_the_app_icon_drops_the_separate_tiles() {
        let tops = vec![minimized(1, "myapp", "Doc")];
        let slots = build_slots_with(&[], &tops, &LauncherIndex::default(), None, false);

        assert!(slots.iter().all(|s| s.kind != SlotKind::MinimizedWindow));
        assert!(slots[0].is_running(), "the app tile still shows it running");
    }

    /// Nothing to separate means no separator: a lone Trash must not get one.
    #[test]
    fn a_separator_needs_something_on_both_sides() {
        let slots = build_slots(
            &[],
            &[],
            &LauncherIndex::default(),
            Some(TrashState { full: false }),
        );
        assert_eq!(
            slots.iter().map(|s| s.kind).collect::<Vec<_>>(),
            vec![SlotKind::Trash]
        );
    }

    fn app_slot(key: &str) -> Slot {
        Slot {
            kind: SlotKind::App,
            key: key.to_owned(),
            label: key.to_owned(),
            icon_name: None,
            windows: vec![1],
            active: false,
            pinned: false,
            capture_key: None,
        }
    }

    #[test]
    fn merging_keeps_the_target_row_when_nothing_changed() {
        let row = vec![app_slot("a"), app_slot("b")];
        let merged = merge_rows(&row, &row);
        assert_eq!(merged.len(), 2);
        assert!(merged.iter().all(|s| !is_departing(s, &row)));
    }

    /// A tile that is leaving has to stay in the row, or its neighbours snap
    /// into the gap in a single frame.
    #[test]
    fn a_removed_tile_is_kept_so_it_can_shrink_away() {
        let current = vec![app_slot("a"), app_slot("b"), app_slot("c")];
        let target = vec![app_slot("a"), app_slot("c")];
        let merged = merge_rows(&current, &target);

        assert_eq!(
            merged.iter().map(|s| s.key.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "c"],
            "the departing tile should shrink where it was"
        );
        assert!(is_departing(&merged[1], &target));
        assert!(!is_departing(&merged[0], &target));
    }

    #[test]
    fn a_new_tile_is_present_and_not_departing() {
        let current = vec![app_slot("a")];
        let target = vec![app_slot("a"), app_slot("new")];
        let merged = merge_rows(&current, &target);

        assert_eq!(merged.len(), 2);
        assert!(!is_departing(&merged[1], &target));
    }

    /// Several tiles can leave at once -- quitting an application with two
    /// minimised windows, for instance.
    #[test]
    fn several_departing_tiles_all_survive_the_merge() {
        let current = vec![app_slot("a"), app_slot("b"), app_slot("c")];
        let target = vec![app_slot("b")];
        let merged = merge_rows(&current, &target);

        assert_eq!(merged.len(), 3);
        assert_eq!(
            merged.iter().filter(|s| is_departing(s, &target)).count(),
            2
        );
    }

    /// A tile leaving from the end must not be dropped for want of an index.
    #[test]
    fn a_tile_leaving_the_end_is_still_kept() {
        let current = vec![app_slot("a"), app_slot("b")];
        let target = vec![app_slot("a")];
        let merged = merge_rows(&current, &target);

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[1].key, "b");
    }

    /// The point of the whole exercise: a window still on screen is told where
    /// its tile is *going* to be, because the compositor stops listening once
    /// the window is gone.
    #[test]
    fn a_visible_window_is_pointed_at_its_future_tile() {
        let tops = vec![toplevel(1, "myapp", true)];
        let targets = minimize_targets(&[], &tops, &LauncherIndex::default(), None, true);

        assert_eq!(targets.len(), 1);
        let t = &targets[0];
        assert_eq!(t.window, 1);
        assert_eq!(
            t.slots[t.slot].kind,
            SlotKind::MinimizedWindow,
            "a visible window should aim at the tile it will get, not its app icon"
        );
    }

    /// An already-minimised window points at the tile it actually has.
    #[test]
    fn a_minimized_window_points_at_its_real_tile() {
        let tops = vec![minimized(1, "myapp", "Doc")];
        let targets = minimize_targets(&[], &tops, &LauncherIndex::default(), None, true);

        let t = &targets[0];
        assert_eq!(t.slots[t.slot].kind, SlotKind::MinimizedWindow);
        assert!(t.slots[t.slot].windows.contains(&1));
    }

    /// The predicted tile must be the one for *that* window, not just any
    /// minimised tile that happens to exist.
    #[test]
    fn prediction_picks_the_right_tile_among_several() {
        let tops = vec![
            minimized(1, "a", "Already"),
            toplevel(2, "b", false),
            minimized(3, "c", "AlsoAlready"),
        ];
        let targets = minimize_targets(&[], &tops, &LauncherIndex::default(), None, true);

        let t = targets.iter().find(|t| t.window == 2).unwrap();
        assert!(
            t.slots[t.slot].windows.contains(&2),
            "pointed at someone else's tile"
        );
        // Three minimised tiles once window 2 joins them.
        assert_eq!(
            t.slots
                .iter()
                .filter(|s| s.kind == SlotKind::MinimizedWindow)
                .count(),
            3
        );
    }

    /// With separate tiles switched off there is no future tile, so the
    /// application icon is the only honest target.
    #[test]
    fn folded_mode_falls_back_to_the_application_icon() {
        let tops = vec![toplevel(1, "myapp", true)];
        let targets = minimize_targets(&[], &tops, &LauncherIndex::default(), None, false);

        let t = &targets[0];
        assert_eq!(t.slots[t.slot].kind, SlotKind::App);
    }

    #[test]
    fn trash_comes_last_behind_a_separator() {
        let slots = build_slots(
            &["a".to_owned()],
            &[],
            &LauncherIndex::default(),
            Some(TrashState { full: false }),
        );

        let kinds: Vec<_> = slots.iter().map(|s| s.kind).collect();
        assert_eq!(
            kinds,
            vec![SlotKind::App, SlotKind::Separator, SlotKind::Trash]
        );
        assert_eq!(slots[2].icon_name.as_deref(), Some("user-trash"));
    }

    /// Nothing to separate from means no separator -- a lone divider at the
    /// left edge of the row would look like a mistake.
    #[test]
    fn an_otherwise_empty_row_gets_no_separator() {
        let slots = build_slots(
            &[],
            &[],
            &LauncherIndex::default(),
            Some(TrashState { full: true }),
        );

        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].kind, SlotKind::Trash);
        assert_eq!(slots[0].icon_name.as_deref(), Some("user-trash-full"));
    }

    #[test]
    fn separators_are_not_interactive() {
        let slots = build_slots(
            &["a".to_owned()],
            &[],
            &LauncherIndex::default(),
            Some(TrashState { full: false }),
        );
        assert!(!slots[1].is_interactive());
        assert!(slots[0].is_interactive() && slots[2].is_interactive());
    }

    /// Pinned order is the user's, and running-but-unpinned apps go after.
    #[test]
    fn pinned_come_first_in_order_then_the_rest() {
        let pinned = vec!["b".to_owned(), "a".to_owned()];
        let tops = vec![toplevel(1, "zzz", false)];
        let slots = build_slots(&pinned, &tops, &LauncherIndex::default(), None);

        assert_eq!(
            slots.iter().map(|s| s.key.as_str()).collect::<Vec<_>>(),
            vec!["b", "a", "zzz"]
        );
    }
}
