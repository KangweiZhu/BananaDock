//! User configuration, and reloading it when the file changes.
//!
//! Only preferences live here; [`crate::metrics`] keeps the proportions
//! reverse-engineered from the reference screenshot, and its values are what
//! every setting starts from.
//!
//! A few of those proportions are also exposed as settings -- the artwork's
//! size, the panel's radius and padding -- because they are what people
//! actually want to change and leaving them fixed only pushed them into
//! editing constants and rebuilding. Anything left unset still follows the
//! reference, so the defaults remain the thing being replicated.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{edge::Edge, metrics::Metrics};

/// The XDG config directory, `~/.config` when unset.
pub fn config_home() -> Option<PathBuf> {
    match std::env::var_os("XDG_CONFIG_HOME") {
        Some(x) if !x.is_empty() => Some(PathBuf::from(x)),
        _ => Some(PathBuf::from(std::env::var_os("HOME")?).join(".config")),
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Legacy tile pitch in points -- the centre-to-centre spacing of icons.
    ///
    /// Superseded by `icon_spacing`, which names the quantity people actually
    /// reason about: the gap between icons. Still honoured when
    /// `icon_spacing` is unset, so old files keep their geometry.
    pub tile_size: f32,
    /// Size of the icon artwork in points.
    ///
    /// Purely horizontal together with `icon_spacing`: growing the icon widens
    /// the dock but never changes its height, unless the icon outgrows the
    /// panel entirely. Unset follows the reference proportion.
    pub icon_size: Option<f32>,
    /// Horizontal gap between icons, in points.
    ///
    /// Only spaces the row out -- the panel's height never follows it. Unset
    /// falls back to whatever `tile_size` leaves once the icon is taken out.
    pub icon_spacing: Option<f32>,
    /// Height of the panel, in points. Unset keeps the reference's 85pt.
    ///
    /// The panel deliberately holds this height while icons resize inside it;
    /// it only grows past it when the icon no longer fits.
    pub panel_height: Option<f32>,
    /// Pitch at full magnification.
    pub large_size: f32,
    pub magnification: bool,
    /// Cursor influence radius, in tiles.
    pub magnification_range: f32,
    pub auto_hide: bool,

    /// Panel corner radius as a fraction of its height. 0.5 is a capsule --
    /// the end caps become semicircles -- and 0 is a plain rectangle.
    pub panel_radius: f32,
    /// Space between the end icons and the panel's edges, in points. This is
    /// what makes the panel wider than the row it holds.
    pub panel_padding: f32,
    /// Which screen edge the dock sits on: `bottom`, `top`, `left` or
    /// `right`. Anything else is reported and the bottom is kept.
    pub position: String,
    /// Space between the dock and the screen edge it sits on, in points.
    pub bottom_gap: f32,
    /// Space between the dock and the windows above it, in points.
    ///
    /// Zero puts a maximised window's edge on the dock's top edge, as macOS
    /// does. This moves the windows; `bottom_gap` moves the dock.
    pub window_gap: f32,
    pub show_trash: bool,
    /// Whether a minimised window gets its own tile to the right of the
    /// applications, as macOS does by default, or only shows through its
    /// application's icon -- macOS's "Minimize windows into application icon".
    pub separate_minimized: bool,
    /// Overrides the icon theme detected from the desktop's own settings.
    pub icon_theme: Option<String>,
    /// Which screen to sit on, by connector name (`DP-1`, `eDP-1`, ...).
    /// Unset lets the compositor choose, which is usually the focused screen.
    pub output: Option<String>,
    /// Desktop entry ids, in the order they appear in the dock.
    pub pinned: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tile_size: 64.0,
            icon_size: None,
            icon_spacing: None,
            panel_height: None,
            large_size: 128.0,
            magnification: true,
            magnification_range: 2.5,
            auto_hide: false,
            panel_radius: 0.5,
            panel_padding: 8.0,
            position: "bottom".into(),
            bottom_gap: 8.0,
            window_gap: 0.0,
            show_trash: true,
            separate_minimized: true,
            icon_theme: None,
            output: None,
            pinned: Vec::new(),
        }
    }
}

impl Config {
    /// `$XDG_CONFIG_HOME/kdock/config.toml`, falling back to `~/.config`.
    pub fn path() -> Option<PathBuf> {
        Some(config_home()?.join("kdock").join("config.toml"))
    }

    /// Reads the config, falling back to defaults.
    ///
    /// A missing file is normal -- the dock has to work before the user has
    /// written one. A malformed file is not: it is reported and the previous
    /// defaults are kept, so a stray keystroke during live editing does not
    /// blank the dock.
    pub fn load(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                eprintln!("kdock: cannot read {}: {e}", path.display());
                return Self::default();
            }
        };

        Self::parse(&text).unwrap_or_else(|e| {
            eprintln!("kdock: {} is invalid, using defaults: {e}", path.display());
            Self::default()
        })
    }

    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// Rewrites just the `pinned` array, leaving the rest of the file alone.
    ///
    /// Serialising the whole `Config` back would be far simpler, but the file
    /// is one a person writes by hand: round-tripping it through serde would
    /// silently delete their comments, reorder their keys, and expand every
    /// default they had deliberately left out. `toml_edit` preserves the
    /// document, so only the one array the dock owns actually changes.
    pub fn save_pinned(path: &Path, pinned: &[String]) -> std::io::Result<()> {
        let mut array = toml_edit::Array::new();
        for id in pinned {
            array.push(id.as_str());
        }
        edit(path, |doc| set_value(doc, "pinned", array.into()))
    }

    /// Writes a set of scalar settings, leaving the rest of the file alone.
    pub fn save_settings(
        path: &Path,
        settings: &[(&str, toml_edit::Value)],
    ) -> std::io::Result<()> {
        edit(path, |doc| {
            for (key, value) in settings {
                set_value(doc, key, value.clone());
            }
        })
    }

    /// The icon artwork size in effect: set, or following the reference
    /// proportion of the legacy pitch.
    pub fn effective_icon_size(&self) -> f32 {
        self.icon_size.unwrap_or(self.tile_size * 0.67).max(1.0)
    }

    /// The gap between icons in effect. Unset falls back to what the legacy
    /// pitch leaves once the icon is taken out, so old files keep their
    /// geometry.
    pub fn effective_icon_spacing(&self) -> f32 {
        self.icon_spacing
            .unwrap_or(self.tile_size - self.effective_icon_size())
            .max(0.0)
    }

    /// The edge the dock sits on. A spelling nobody recognises keeps the
    /// bottom rather than refusing the whole file, which would take every
    /// other setting down with it.
    pub fn edge(&self) -> Edge {
        Edge::parse(&self.position).unwrap_or_else(|| {
            eprintln!(
                "kdock: {:?} is not a dock position, using the bottom",
                self.position
            );
            Edge::Bottom
        })
    }

    /// Folds the user's preferences into the measured proportions.
    pub fn apply_to(&self, metrics: &mut Metrics) {
        // The pitch is derived, not set: the icon plus the gap beside it.
        let icon = self.effective_icon_size();
        metrics.tile_size = (icon + self.effective_icon_spacing()).max(1.0);
        metrics.large_size = self.large_size.max(metrics.tile_size);
        metrics.magnification_enabled = self.magnification;
        metrics.magnification_range = self.magnification_range.max(0.0);

        // Held as a ratio rather than an absolute size because magnification
        // scales the artwork with its slot: the icon is always the same
        // fraction of whatever width the slot currently has.
        metrics.icon_size_ratio = (icon / metrics.tile_size).clamp(0.05, 1.0);
        if let Some(height) = self.panel_height {
            metrics.panel_base_height = height.clamp(24.0, 512.0);
        }
        // Past half the height opposite corners would overlap, and the drawing
        // clamps anyway -- rejecting it here keeps the config honest.
        metrics.panel_radius_ratio = self.panel_radius.clamp(0.0, 0.5);
        metrics.panel_padding_h = self.panel_padding.max(0.0);
        metrics.edge = self.edge();
        metrics.panel_bottom_gap = self.bottom_gap.max(0.0);
        metrics.window_gap = self.window_gap.max(0.0);
    }
}

/// Replaces a value while keeping whatever was written around it.
///
/// Assigning through `doc[key]` would drop the value's decor, and the decor is
/// where a trailing `# comment` lives -- so editing one setting from the
/// settings window would quietly delete the note the user left beside it.
fn set_value(doc: &mut toml_edit::DocumentMut, key: &str, value: toml_edit::Value) {
    match doc.get_mut(key).and_then(|item| item.as_value_mut()) {
        Some(existing) => {
            let decor = existing.decor().clone();
            *existing = value;
            *existing.decor_mut() = decor;
        }
        None => doc[key] = toml_edit::Item::Value(value),
    }
}

/// Reads, edits and writes the config file in place.
///
/// A missing file is treated as an empty one, so the first save works without
/// the user having created anything.
fn edit(path: &Path, apply: impl FnOnce(&mut toml_edit::DocumentMut)) -> std::io::Result<()> {
    let existing = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };

    let mut doc = existing.parse::<toml_edit::DocumentMut>().map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{} is not valid TOML: {e}", path.display()),
        )
    })?;
    apply(&mut doc);

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Written through a temporary file and renamed: a crash midway would
    // otherwise leave a truncated config, and the dock re-reads this file on
    // every change notification.
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, doc.to_string())?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Editing a setting must not eat the note the user left beside it.
    #[test]
    fn writing_a_value_keeps_the_comments_around_it() {
        let src = "# top\n\n# what this does\ntile_size = 64.0\n\nauto_hide = false # beside\n";
        let mut doc: toml_edit::DocumentMut = src.parse().unwrap();

        set_value(&mut doc, "tile_size", 80.0.into());
        set_value(&mut doc, "auto_hide", true.into());
        let out = doc.to_string();

        assert!(out.contains("# top"));
        assert!(out.contains("# what this does"));
        assert!(out.contains("# beside"), "trailing comment lost:\n{out}");
        assert!(out.contains("tile_size = 80.0"));
        assert!(out.contains("auto_hide = true"));
    }

    /// A setting the file does not mention yet is appended, not dropped.
    #[test]
    fn a_missing_key_is_added() {
        let mut doc: toml_edit::DocumentMut = "tile_size = 64.0\n".parse().unwrap();
        set_value(&mut doc, "separate_minimized", false.into());

        let out = doc.to_string();
        assert!(out.contains("separate_minimized = false"), "{out}");
        assert!(out.contains("tile_size = 64.0"));
    }

    /// Round-trip: what the settings window writes has to be what the dock
    /// then parses back.
    #[test]
    fn written_values_survive_a_reparse() {
        let mut doc: toml_edit::DocumentMut = "# note\ntile_size = 64.0\n".parse().unwrap();
        set_value(&mut doc, "tile_size", 96.0.into());
        set_value(&mut doc, "magnification", false.into());

        let parsed = Config::parse(&doc.to_string()).expect("still valid TOML");
        assert_eq!(parsed.tile_size, 96.0);
        assert!(!parsed.magnification);
    }

    #[test]
    fn an_empty_file_is_all_defaults() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    /// A partial file must keep the defaults for everything it omits, so users
    /// can set one value without restating the rest.
    #[test]
    fn a_partial_file_keeps_other_defaults() {
        let c = Config::parse("tile_size = 48").unwrap();
        assert_eq!(c.tile_size, 48.0);
        assert_eq!(c.large_size, Config::default().large_size);
        assert!(c.magnification);
    }

    #[test]
    fn an_output_can_be_named() {
        let c = Config::parse(r#"output = "DP-1""#).unwrap();
        assert_eq!(c.output.as_deref(), Some("DP-1"));
        assert_eq!(
            Config::default().output,
            None,
            "unset means the compositor picks"
        );
    }

    #[test]
    fn pinned_order_is_preserved() {
        let c = Config::parse(r#"pinned = ["b", "a", "c"]"#).unwrap();
        assert_eq!(c.pinned, vec!["b", "a", "c"]);
    }

    /// A typo should be reported rather than silently ignored -- a setting that
    /// quietly does nothing is worse than one that complains.
    #[test]
    fn unknown_keys_are_rejected() {
        assert!(Config::parse("tile_siz = 48").is_err());
    }

    #[test]
    fn a_malformed_file_falls_back_to_defaults() {
        let dir = std::env::temp_dir().join("kdock-cfg-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "this is not toml {{{").unwrap();

        assert_eq!(Config::load(&path), Config::default());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        let path = std::env::temp_dir().join("kdock-does-not-exist-9e3a.toml");
        assert_eq!(Config::load(&path), Config::default());
    }

    fn temp_config(name: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kdock-save-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, body).unwrap();
        path
    }

    /// The whole reason for `toml_edit`: a hand-written file keeps its comments
    /// and its other settings when the dock rewrites the pin list.
    #[test]
    fn saving_pinned_preserves_comments_and_other_keys() {
        let path = temp_config(
            "comments",
            "# my dock\ntile_size = 48  # smaller\n\nauto_hide = true\npinned = [\"old\"]\n",
        );

        Config::save_pinned(&path, &["a".into(), "b".into()]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();

        assert!(text.contains("# my dock"), "leading comment lost:\n{text}");
        assert!(text.contains("# smaller"), "inline comment lost:\n{text}");
        assert!(
            text.contains("auto_hide = true"),
            "other setting lost:\n{text}"
        );

        let reloaded = Config::load(&path);
        assert_eq!(reloaded.pinned, vec!["a", "b"]);
        assert_eq!(reloaded.tile_size, 48.0);
        assert!(reloaded.auto_hide);
    }

    #[test]
    fn saving_pinned_creates_a_file_that_does_not_exist_yet() {
        let dir = std::env::temp_dir().join("kdock-save-fresh");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("config.toml");

        Config::save_pinned(&path, &["only".into()]).unwrap();
        assert_eq!(Config::load(&path).pinned, vec!["only"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn saving_an_empty_list_clears_the_pins() {
        let path = temp_config("clear", "pinned = [\"a\", \"b\"]\n");
        Config::save_pinned(&path, &[]).unwrap();
        assert!(Config::load(&path).pinned.is_empty());
    }

    /// Refusing to write is better than overwriting a file the user is midway
    /// through editing.
    #[test]
    fn saving_over_a_malformed_file_fails_rather_than_clobbering_it() {
        let path = temp_config("broken", "this is not toml {{{");
        assert!(Config::save_pinned(&path, &["a".into()]).is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "this is not toml {{{"
        );
    }

    /// Nonsense sizes must not reach the layout, where a zero pitch would make
    /// every slot zero-width and a large_size below tile_size would invert
    /// magnification.
    #[test]
    fn applying_clamps_degenerate_sizes() {
        let mut m = Metrics::default();
        Config {
            tile_size: 0.0,
            large_size: 10.0,
            ..Config::default()
        }
        .apply_to(&mut m);

        assert!(m.tile_size >= 1.0);
        assert!(m.large_size >= m.tile_size, "magnification must not invert");
        assert!(m.max_scale() >= 1.0);
    }

    /// The two Size sliders are purely horizontal: spacing and icon size set
    /// the pitch between them, and neither moves the panel's height while the
    /// icon still fits.
    #[test]
    fn spacing_and_icon_size_are_purely_horizontal() {
        let apply = |doc: &str| {
            let mut m = Metrics::default();
            Config::parse(doc).unwrap().apply_to(&mut m);
            m
        };

        let narrow = apply("icon_size = 37.0\nicon_spacing = 10.0");
        let wide = apply("icon_size = 37.0\nicon_spacing = 40.0");
        assert!((narrow.tile_size - 47.0).abs() < 0.01);
        assert!((wide.tile_size - 77.0).abs() < 0.01);
        assert_eq!(narrow.panel_height(), wide.panel_height());

        let bigger_icon = apply("icon_size = 60.0\nicon_spacing = 10.0");
        assert!((bigger_icon.icon_size() - 60.0).abs() < 0.01);
        assert_eq!(
            bigger_icon.panel_height(),
            narrow.panel_height(),
            "an icon that fits must not change the height"
        );
    }

    /// An old file that only knows `tile_size` keeps its geometry.
    #[test]
    fn legacy_tile_size_still_sets_the_pitch() {
        let mut m = Metrics::default();
        Config::parse("tile_size = 47.0\nicon_size = 37.0")
            .unwrap()
            .apply_to(&mut m);
        assert!((m.tile_size - 47.0).abs() < 0.01);
        assert!((m.icon_size() - 37.0).abs() < 0.01);
    }

    /// The two gaps move different things: one the dock, one the windows.
    /// Reserving the same amount for both would put the dock's own float above
    /// the screen edge into the windows' pocket, or vice versa.
    #[test]
    fn the_two_gaps_move_the_dock_and_the_windows_separately() {
        let apply = |doc: &str| {
            let mut m = Metrics::default();
            Config::parse(doc).unwrap().apply_to(&mut m);
            m
        };

        let base = apply("panel_height = 80.0");
        let lifted = apply("panel_height = 80.0\nbottom_gap = 28.0");
        let roomy = apply("panel_height = 80.0\nwindow_gap = 28.0");

        // Raising the dock off the screen edge moves the dock, and windows
        // follow it up because the space underneath is still the dock's.
        assert_eq!(lifted.panel_bottom_gap - base.panel_bottom_gap, 20.0);
        assert_eq!(
            lifted.window_clearance() - base.window_clearance(),
            20.0,
            "the dock's float is reserved too"
        );
        // Asking for room above the dock moves only the windows.
        assert_eq!(roomy.panel_bottom_gap, base.panel_bottom_gap);
        assert_eq!(roomy.window_clearance() - base.window_clearance(), 28.0);
    }

    /// A negative gap would hand the compositor a nonsense strut.
    #[test]
    fn negative_gaps_are_clamped_away() {
        let mut m = Metrics::default();
        Config::parse("bottom_gap = -10.0\nwindow_gap = -10.0")
            .unwrap()
            .apply_to(&mut m);
        assert_eq!(m.panel_bottom_gap, 0.0);
        assert_eq!(m.window_gap, 0.0);
        assert_eq!(m.window_clearance(), m.pt(m.panel_height()));
    }

    /// `panel_height` is the one direct lever over the dock's height.
    #[test]
    fn panel_height_sets_the_base() {
        let mut m = Metrics::default();
        Config::parse("panel_height = 73.0")
            .unwrap()
            .apply_to(&mut m);
        assert_eq!(m.panel_height(), 73.0);
    }

    #[test]
    fn applying_carries_magnification_settings_through() {
        let mut m = Metrics::default();
        Config {
            magnification: false,
            magnification_range: 4.0,
            ..Config::default()
        }
        .apply_to(&mut m);

        assert!(!m.magnification_enabled);
        assert_eq!(m.magnification_range, 4.0);
        assert_eq!(m.max_scale(), 1.0);
    }
}
