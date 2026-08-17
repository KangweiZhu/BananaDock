//! User configuration, and reloading it when the file changes.
//!
//! Only preferences live here. The proportions reverse-engineered from the
//! reference screenshot -- how big the artwork is relative to its tile, how the
//! panel's height relates to the tile pitch, the capsule radius -- stay in
//! [`crate::metrics`] as design constants. Exposing those as settings would let
//! the dock be configured into something that is no longer the thing being
//! replicated, and would scatter the values the calibration pass has to edit.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::metrics::Metrics;

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
    /// Tile pitch in points -- the macOS Dock size slider.
    pub tile_size: f32,
    /// Pitch at full magnification.
    pub large_size: f32,
    pub magnification: bool,
    /// Cursor influence radius, in tiles.
    pub magnification_range: f32,
    pub auto_hide: bool,
    pub show_trash: bool,
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
            large_size: 128.0,
            magnification: true,
            magnification_range: 2.5,
            auto_hide: false,
            show_trash: true,
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

        let mut array = toml_edit::Array::new();
        for id in pinned {
            array.push(id.as_str());
        }
        doc["pinned"] = toml_edit::value(array);

        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Written through a temporary file and renamed: a crash midway would
        // otherwise leave a truncated config, and the dock reads this file on
        // every change notification.
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, doc.to_string())?;
        std::fs::rename(&tmp, path)
    }

    /// Folds the user's preferences into the measured proportions.
    pub fn apply_to(&self, metrics: &mut Metrics) {
        metrics.tile_size = self.tile_size.max(1.0);
        metrics.large_size = self.large_size.max(metrics.tile_size);
        metrics.magnification_enabled = self.magnification;
        metrics.magnification_range = self.magnification_range.max(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
