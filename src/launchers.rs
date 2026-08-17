//! Desktop entries, and matching a window's `app_id` to one.
//!
//! Matching is the fiddly part. A Wayland `app_id` is whatever the toolkit felt
//! like reporting -- sometimes the reverse-DNS desktop file id
//! (`org.kde.dolphin`), sometimes the bare binary name (`dolphin`), sometimes
//! the X11 `WM_CLASS` an app kept for compatibility (`Navigator`), sometimes
//! nothing at all. There is no authoritative mapping, so this is a ladder of
//! heuristics ordered most- to least-specific, and it is expected to grow as
//! misbehaving applications turn up. That is why it is a pure function over a
//! slice: new rules can be pinned down by a test without a desktop session.

use freedesktop_desktop_entry::{desktop_entries, get_languages_from_env};

/// A `.desktop` entry, reduced to what the dock uses.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Launcher {
    /// Desktop file id without the extension, e.g. `org.kde.dolphin`.
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    /// `Exec=` verbatim, field codes and all.
    pub exec: Option<String>,
    pub startup_wm_class: Option<String>,
    /// Entries flagged `NoDisplay` are not offered to the user, but may still
    /// be the right match for a window that is already open.
    pub no_display: bool,
}

/// Every desktop entry on the system.
#[derive(Debug, Default)]
pub struct LauncherIndex {
    entries: Vec<Launcher>,
}

impl LauncherIndex {
    /// Reads the XDG application directories.
    pub fn load() -> Self {
        let locales = get_languages_from_env();
        let entries = desktop_entries(&locales)
            .into_iter()
            .map(|e| Launcher {
                id: e.appid.clone(),
                name: e
                    .name(&locales)
                    .map(|c| c.into_owned())
                    .unwrap_or_else(|| e.appid.clone()),
                icon: e.icon().map(str::to_owned),
                exec: e.exec().map(str::to_owned),
                startup_wm_class: e.startup_wm_class().map(str::to_owned),
                no_display: e.no_display(),
            })
            .collect();

        Self { entries }
    }

    pub fn by_id(&self, id: &str) -> Option<&Launcher> {
        self.entries.iter().find(|e| e.id.eq_ignore_ascii_case(id))
    }

    pub fn match_app_id(&self, app_id: &str) -> Option<&Launcher> {
        match_app_id(app_id, &self.entries)
    }
}

/// Splits an `Exec=` line into a command and its arguments.
///
/// Desktop entries carry field codes -- `%f` for a file, `%U` for URLs, `%i`
/// for the icon, and so on -- which the spec says to substitute or drop. The
/// dock launches applications with no documents, so every code is dropped;
/// passing them through literally would hand the application an argument called
/// `%U`. Quoted arguments are honoured, and `%%` is an escaped percent sign.
pub fn exec_argv(exec: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut has_token = false;
    let mut chars = exec.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '"' | '\'' if quote.is_none() => {
                quote = Some(c);
                has_token = true;
            }
            c if Some(c) == quote => quote = None,
            '%' => match chars.next() {
                Some('%') => {
                    current.push('%');
                    has_token = true;
                }
                // A field code standing alone; drop it and the token with it.
                Some(_) => {}
                None => {}
            },
            c if c.is_whitespace() && quote.is_none() => {
                if has_token && !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
                current.clear();
                has_token = false;
            }
            c => {
                current.push(c);
                has_token = true;
            }
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Strips a reverse-DNS prefix: `org.kde.dolphin` becomes `dolphin`.
///
/// Only applied when the string actually looks like reverse-DNS, so a name that
/// merely contains a dot (`gimp-2.10`) is left alone.
fn last_segment(s: &str) -> &str {
    if s.matches('.').count() >= 2 {
        s.rsplit('.').next().unwrap_or(s)
    } else {
        s
    }
}

/// Finds the desktop entry a window belongs to.
///
/// Rules run most-specific first; the first hit wins.
pub fn match_app_id<'a>(app_id: &str, entries: &'a [Launcher]) -> Option<&'a Launcher> {
    if app_id.is_empty() {
        return None;
    }

    // 1. StartupWMClass is the only field that exists specifically to answer
    //    this question, so it outranks everything.
    if let Some(e) = entries.iter().find(|e| {
        e.startup_wm_class
            .as_deref()
            .is_some_and(|c| c.eq_ignore_ascii_case(app_id))
    }) {
        return Some(e);
    }

    // 2. The app_id is the desktop file id outright.
    if let Some(e) = entries.iter().find(|e| e.id.eq_ignore_ascii_case(app_id)) {
        return Some(e);
    }

    // 3. One side is reverse-DNS and the other is the bare name. Compare the
    //    trailing segments -- this is what catches `org.kde.dolphin` against a
    //    `dolphin.desktop`, and the reverse.
    let short = last_segment(app_id);
    if let Some(e) = entries
        .iter()
        .find(|e| last_segment(&e.id).eq_ignore_ascii_case(short))
    {
        return Some(e);
    }

    // 4. Some applications report a WM_CLASS that differs from both, but whose
    //    trailing segment still lines up (Firefox reports `Navigator` on X11
    //    but `firefox` on Wayland; Chromium-based apps vary by build).
    if let Some(e) = entries.iter().find(|e| {
        e.startup_wm_class
            .as_deref()
            .is_some_and(|c| last_segment(c).eq_ignore_ascii_case(short))
    }) {
        return Some(e);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launcher(id: &str, wm_class: Option<&str>) -> Launcher {
        Launcher {
            id: id.to_owned(),
            name: id.to_owned(),
            startup_wm_class: wm_class.map(str::to_owned),
            ..Default::default()
        }
    }

    #[test]
    fn startup_wm_class_outranks_the_id() {
        // Both could match; the StartupWMClass entry has to win.
        let entries = vec![
            launcher("firefox", None),
            launcher("org.mozilla.dev", Some("firefox")),
        ];
        assert_eq!(
            match_app_id("firefox", &entries).unwrap().id,
            "org.mozilla.dev"
        );
    }

    #[test]
    fn exact_id_matches_case_insensitively() {
        let entries = vec![launcher("org.kde.Dolphin", None)];
        assert_eq!(
            match_app_id("org.kde.dolphin", &entries).unwrap().id,
            "org.kde.Dolphin"
        );
    }

    /// The case the plan calls out: reverse-DNS app_id, bare desktop file.
    #[test]
    fn reverse_dns_app_id_finds_a_bare_desktop_file() {
        let entries = vec![launcher("dolphin", None)];
        assert_eq!(
            match_app_id("org.kde.dolphin", &entries).unwrap().id,
            "dolphin"
        );
    }

    /// ...and the same in reverse: bare app_id, reverse-DNS desktop file.
    #[test]
    fn bare_app_id_finds_a_reverse_dns_desktop_file() {
        let entries = vec![launcher("org.gnome.Nautilus", None)];
        assert_eq!(
            match_app_id("nautilus", &entries).unwrap().id,
            "org.gnome.Nautilus"
        );
    }

    /// A version number is not a reverse-DNS prefix; stripping it would make
    /// `gimp-2.10` match anything ending in `10`.
    #[test]
    fn a_single_dot_is_not_treated_as_reverse_dns() {
        assert_eq!(last_segment("gimp-2.10"), "gimp-2.10");
        assert_eq!(last_segment("org.kde.dolphin"), "dolphin");
    }

    #[test]
    fn exec_drops_field_codes() {
        assert_eq!(exec_argv("dolphin %u"), vec!["dolphin"]);
        assert_eq!(
            exec_argv("firefox --new-window %U"),
            vec!["firefox", "--new-window"]
        );
        assert_eq!(exec_argv("app -i %i -c %c"), vec!["app", "-i", "-c"]);
    }

    #[test]
    fn exec_honours_quoted_arguments() {
        assert_eq!(
            exec_argv(r#"/opt/My App/run --flag "two words" %f"#),
            vec!["/opt/My", "App/run", "--flag", "two words"]
        );
    }

    /// `%%` is an escaped percent, not a field code.
    #[test]
    fn exec_unescapes_double_percent() {
        assert_eq!(exec_argv("app --fmt %%s"), vec!["app", "--fmt", "%s"]);
    }

    #[test]
    fn exec_of_an_empty_line_is_empty() {
        assert!(exec_argv("").is_empty());
        assert!(exec_argv("   ").is_empty());
    }

    #[test]
    fn unmatched_app_id_yields_nothing() {
        let entries = vec![launcher("dolphin", None)];
        assert!(match_app_id("some-unknown-app", &entries).is_none());
    }

    /// Compositors do report an empty app_id, and it must not match the first
    /// entry that happens to have an empty field.
    #[test]
    fn empty_app_id_never_matches() {
        let entries = vec![launcher("", None), launcher("dolphin", None)];
        assert!(match_app_id("", &entries).is_none());
    }
}
