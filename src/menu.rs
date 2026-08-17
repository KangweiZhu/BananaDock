//! The right-click menu's contents.
//!
//! Follows macOS: open windows listed at the top, then the pinning toggle, then
//! Hide and Quit. A launcher that is not running gets a much shorter menu.
//!
//! One deliberate departure: macOS nests the pinning toggle inside an "Options"
//! submenu. A submenu means a second popup, its own grab, and hover-to-open
//! timing, all for a single item -- so it is flattened to a top-level entry
//! here. Everything else keeps macOS's ordering.

use crate::{
    model::{Slot, SlotKind},
    windows::{Capabilities, Toplevel, ToplevelId},
};

/// What choosing an item does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    /// Raise one specific window.
    ActivateWindow(ToplevelId),
    /// Bring every window of the application forward.
    ShowAllWindows,
    /// Minimise every window of the application, as macOS's Hide does.
    Hide,
    Quit,
    /// Launch an application that is not running.
    Open,
    /// Add to, or remove from, the pinned launchers.
    TogglePinned,
    OpenTrash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub label: String,
    /// `None` marks a separator, which is drawn but cannot be chosen.
    pub action: Option<MenuAction>,
    pub checked: bool,
}

impl MenuItem {
    fn action(label: impl Into<String>, action: MenuAction) -> Self {
        Self {
            label: label.into(),
            action: Some(action),
            checked: false,
        }
    }

    fn separator() -> Self {
        Self {
            label: String::new(),
            action: None,
            checked: false,
        }
    }

    pub fn is_separator(&self) -> bool {
        self.action.is_none()
    }
}

/// Builds the menu for one slot.
pub fn build_menu(
    slot: &Slot,
    toplevels: &[Toplevel],
    pinned: bool,
    caps: Capabilities,
) -> Vec<MenuItem> {
    if slot.kind == SlotKind::Trash {
        return vec![MenuItem::action("Open", MenuAction::OpenTrash)];
    }

    let mut items = Vec::new();

    // Open windows first, as macOS does. Titles come from the live toplevel
    // list rather than the slot, which only carries ids.
    let titles: Vec<&Toplevel> = slot
        .windows
        .iter()
        .filter_map(|id| toplevels.iter().find(|t| t.id == *id))
        .collect();

    for t in &titles {
        let label = if t.title.is_empty() {
            slot.label.clone()
        } else {
            t.title.clone()
        };
        items.push(MenuItem::action(label, MenuAction::ActivateWindow(t.id)));
    }
    if !items.is_empty() {
        items.push(MenuItem::separator());
    }

    items.push(MenuItem {
        label: "Keep in Dock".to_owned(),
        action: Some(MenuAction::TogglePinned),
        checked: pinned,
    });

    if slot.is_running() {
        items.push(MenuItem::action(
            "Show All Windows",
            MenuAction::ShowAllWindows,
        ));
        items.push(MenuItem::separator());
        if caps.minimize {
            items.push(MenuItem::action("Hide", MenuAction::Hide));
        }
        if caps.close {
            items.push(MenuItem::action("Quit", MenuAction::Quit));
        }
    } else {
        items.push(MenuItem::action("Open", MenuAction::Open));
    }

    items
}

/// Where every row of a menu sits, and how big the menu is.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuLayout {
    /// Logical size of the whole popup.
    pub width: f32,
    pub height: f32,
    /// Top edge and height of each row, parallel to the item list.
    pub rows: Vec<(f32, f32)>,
}

impl MenuLayout {
    /// Which item is under a point in menu-local logical coordinates.
    ///
    /// Separators are excluded: they occupy space but cannot be chosen, and
    /// returning one would let the highlight land on a divider.
    pub fn hit(&self, items: &[MenuItem], y: f32) -> Option<usize> {
        self.rows
            .iter()
            .enumerate()
            .find(|(i, (top, h))| {
                y >= *top && y < top + h && items.get(*i).is_some_and(|it| !it.is_separator())
            })
            .map(|(i, _)| i)
    }
}

/// Measures a menu.
///
/// `measure_text` returns a label's width in logical pixels; it is passed in so
/// this stays a pure function that can be tested without a font stack.
pub fn layout_menu(
    items: &[MenuItem],
    metrics: &crate::metrics::Metrics,
    mut measure_text: impl FnMut(&str) -> f32,
) -> MenuLayout {
    let pad = metrics.pt(metrics.menu_item_padding);
    let row_h = metrics.pt(metrics.menu_item_height);
    let sep_h = metrics.pt(metrics.menu_separator_height);

    // Room for the check mark on every row, so labels line up whether or not
    // an item happens to be checked.
    let check_column = pad;

    let widest = items
        .iter()
        .filter(|i| !i.is_separator())
        .map(|i| measure_text(&i.label))
        .fold(0.0, f32::max);
    let width = (widest + pad * 2.0 + check_column).max(metrics.pt(metrics.menu_min_width));

    let mut rows = Vec::with_capacity(items.len());
    let mut y = pad / 2.0;
    for item in items {
        let h = if item.is_separator() { sep_h } else { row_h };
        rows.push((y, h));
        y += h;
    }

    MenuLayout {
        width,
        height: y + pad / 2.0,
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(kind: SlotKind, windows: Vec<ToplevelId>) -> Slot {
        Slot {
            capture_key: None,
            kind,
            key: "app".into(),
            label: "App".into(),
            icon_name: None,
            windows,
            active: false,
            pinned: false,
        }
    }

    fn toplevel(id: ToplevelId, title: &str) -> Toplevel {
        Toplevel {
            id,
            title: title.to_owned(),
            ..Default::default()
        }
    }

    fn labels(items: &[MenuItem]) -> Vec<&str> {
        items.iter().map(|i| i.label.as_str()).collect()
    }

    /// A launcher that is not running gets the short menu -- no Hide or Quit
    /// for something with nothing to hide or quit.
    #[test]
    fn a_dormant_launcher_gets_only_pinning_and_open() {
        let items = build_menu(
            &slot(SlotKind::App, vec![]),
            &[],
            true,
            Capabilities::default(),
        );
        assert_eq!(labels(&items), vec!["Keep in Dock", "Open"]);
        assert!(items[0].checked);
    }

    #[test]
    fn a_running_app_lists_its_windows_first() {
        let tops = vec![toplevel(1, "Doc one"), toplevel(2, "Doc two")];
        let items = build_menu(
            &slot(SlotKind::App, vec![1, 2]),
            &tops,
            false,
            Capabilities::default(),
        );

        assert_eq!(labels(&items)[0..2], ["Doc one", "Doc two"]);
        assert!(items[2].is_separator());
        assert!(labels(&items).contains(&"Quit"));
        assert!(labels(&items).contains(&"Hide"));
    }

    #[test]
    fn choosing_a_window_targets_that_exact_window() {
        let tops = vec![toplevel(7, "Only")];
        let items = build_menu(
            &slot(SlotKind::App, vec![7]),
            &tops,
            false,
            Capabilities::default(),
        );
        assert_eq!(items[0].action, Some(MenuAction::ActivateWindow(7)));
    }

    /// A window with no title would otherwise render as a blank row.
    #[test]
    fn an_untitled_window_falls_back_to_the_application_name() {
        let tops = vec![toplevel(1, "")];
        let items = build_menu(
            &slot(SlotKind::App, vec![1]),
            &tops,
            false,
            Capabilities::default(),
        );
        assert_eq!(items[0].label, "App");
    }

    /// Ids with no matching toplevel are stale; listing them would offer to
    /// A backend that cannot minimise or close must not offer entries that
    /// would silently do nothing -- the whole point of the capability flags.
    #[test]
    fn a_limited_backend_hides_the_entries_it_cannot_honour() {
        let caps = Capabilities {
            minimize: false,
            close: false,
        };
        let items = build_menu(
            &slot(SlotKind::App, vec![1]),
            &[toplevel(1, "Live")],
            true,
            caps,
        );
        let labels = labels(&items);

        assert!(!labels.contains(&"Hide"), "{labels:?}");
        assert!(!labels.contains(&"Quit"), "{labels:?}");
        // The rest of the menu is unaffected.
        assert!(labels.contains(&"Live"), "{labels:?}");
    }

    #[test]
    fn a_capable_backend_offers_hide_and_quit() {
        let items = build_menu(
            &slot(SlotKind::App, vec![1]),
            &[toplevel(1, "Live")],
            true,
            Capabilities::default(),
        );
        let labels = labels(&items);
        assert!(labels.contains(&"Hide"), "{labels:?}");
        assert!(labels.contains(&"Quit"), "{labels:?}");
    }

    /// raise windows that no longer exist.
    #[test]
    fn windows_missing_from_the_toplevel_list_are_skipped() {
        let items = build_menu(
            &slot(SlotKind::App, vec![1, 2]),
            &[toplevel(1, "Live")],
            false,
            Capabilities::default(),
        );
        assert_eq!(labels(&items)[0], "Live");
        assert!(items[1].is_separator());
    }

    #[test]
    fn no_leading_separator_when_nothing_is_running() {
        let items = build_menu(
            &slot(SlotKind::App, vec![]),
            &[],
            false,
            Capabilities::default(),
        );
        assert!(
            !items[0].is_separator(),
            "a menu must not start with a divider"
        );
    }

    #[test]
    fn the_trash_gets_its_own_short_menu() {
        let items = build_menu(
            &slot(SlotKind::Trash, vec![]),
            &[],
            true,
            Capabilities::default(),
        );
        assert_eq!(labels(&items), vec!["Open"]);
        assert_eq!(items[0].action, Some(MenuAction::OpenTrash));
    }

    // -- layout ------------------------------------------------------------

    fn wide_menu() -> Vec<MenuItem> {
        vec![
            MenuItem::action("One", MenuAction::Open),
            MenuItem::separator(),
            MenuItem::action("Two", MenuAction::Quit),
        ]
    }

    #[test]
    fn rows_stack_without_gaps_and_the_menu_bounds_them() {
        let m = crate::metrics::Metrics::default();
        let items = wide_menu();
        let l = layout_menu(&items, &m, |s| s.len() as f32 * 8.0);

        for pair in l.rows.windows(2) {
            let (top, h) = pair[0];
            assert!((top + h - pair[1].0).abs() < 0.01, "rows must abut");
        }
        let (last_top, last_h) = *l.rows.last().unwrap();
        assert!(
            l.height >= last_top + last_h,
            "menu must contain its last row"
        );
    }

    /// A separator row is shorter than a real one; making them equal would
    /// leave a conspicuous gap in the middle of the menu.
    #[test]
    fn separator_rows_are_shorter_than_item_rows() {
        let m = crate::metrics::Metrics::default();
        let items = wide_menu();
        let l = layout_menu(&items, &m, |_| 10.0);
        assert!(l.rows[1].1 < l.rows[0].1);
    }

    #[test]
    fn a_long_label_widens_the_menu_past_its_minimum() {
        let m = crate::metrics::Metrics::default();
        let short = layout_menu(&wide_menu(), &m, |_| 10.0);
        assert_eq!(
            short.width,
            m.pt(m.menu_min_width),
            "short labels use the minimum"
        );

        let items = vec![MenuItem::action("x", MenuAction::Open)];
        let long = layout_menu(&items, &m, |_| 900.0);
        assert!(long.width > 900.0, "a long label must fit with padding");
    }

    #[test]
    fn hit_testing_finds_rows_and_refuses_separators() {
        let m = crate::metrics::Metrics::default();
        let items = wide_menu();
        let l = layout_menu(&items, &m, |_| 10.0);

        let mid = |i: usize| l.rows[i].0 + l.rows[i].1 / 2.0;
        assert_eq!(l.hit(&items, mid(0)), Some(0));
        assert_eq!(l.hit(&items, mid(2)), Some(2));
        assert_eq!(l.hit(&items, mid(1)), None, "a separator is not selectable");
        assert_eq!(l.hit(&items, -5.0), None);
        assert_eq!(l.hit(&items, l.height + 50.0), None);
    }

    /// Separators are drawn but must never be selectable.
    #[test]
    fn separators_carry_no_action() {
        let tops = vec![toplevel(1, "W")];
        let items = build_menu(
            &slot(SlotKind::App, vec![1]),
            &tops,
            false,
            Capabilities::default(),
        );
        for item in items.iter().filter(|i| i.is_separator()) {
            assert!(item.action.is_none());
        }
    }
}
