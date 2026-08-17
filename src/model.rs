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
            }),
        }
    }

    // The Trash goes last, behind a separator -- and only a separator when
    // there is actually something to separate it from.
    if let Some(trash) = trash {
        if !slots.is_empty() {
            slots.push(Slot {
                kind: SlotKind::Separator,
                key: "\u{1}separator".to_owned(),
                label: String::new(),
                icon_name: None,
                windows: Vec::new(),
                active: false,
                pinned: true,
            });
        }
        slots.push(Slot {
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
        });
    }

    slots
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
