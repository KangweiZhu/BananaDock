//! Whether the Trash holds anything, per the XDG Trash specification.
//!
//! Only emptiness matters here -- the icon switches between two artworks -- so
//! this deliberately does not enumerate or parse trashinfo files. It answers
//! "is there at least one entry" and stops, which keeps a Trash holding
//! thousands of files as cheap to check as an empty one.

use std::path::PathBuf;

use crate::model::TrashState;

/// `$XDG_DATA_HOME/Trash/files`, falling back to `~/.local/share`.
///
/// Only the home trash is considered. The spec also allows per-filesystem trash
/// directories on other mounts, but those hold files the user trashed from
/// removable media, which is not what the Dock's Trash represents.
pub fn trash_dir() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_DATA_HOME") {
        Some(x) if !x.is_empty() => PathBuf::from(x),
        _ => PathBuf::from(std::env::var_os("HOME")?)
            .join(".local")
            .join("share"),
    };
    Some(base.join("Trash").join("files"))
}

/// Reads the current state.
///
/// A missing directory means an empty Trash: nothing has ever been deleted, so
/// the directory has not been created yet.
pub fn read(dir: &std::path::Path) -> TrashState {
    let full = std::fs::read_dir(dir)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false);
    TrashState { full }
}

/// Moves a file or directory to the Trash, per the XDG Trash specification.
///
/// The entry is *renamed* into place, never copied. A rename across
/// filesystems fails, and this reports that failure rather than falling back to
/// copy-then-delete: a copy that half-succeeds and is followed by a delete is
/// how files get lost, and the dock is not the right place to take that risk.
///
/// A `.trashinfo` file recording the original path and the deletion time is
/// written first. Without it the file still sits in the Trash, but no file
/// manager can put it back.
pub fn move_to_trash(path: &std::path::Path, trash_files: &std::path::Path) -> std::io::Result<()> {
    let Some(trash_root) = trash_files.parent() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "trash files directory has no parent",
        ));
    };
    let info_dir = trash_root.join("info");
    std::fs::create_dir_all(trash_files)?;
    std::fs::create_dir_all(&info_dir)?;

    let original = path.canonicalize()?;
    let stem = original
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "unnamed file"))?;

    let name = unique_name(trash_files, &info_dir, stem);
    let info_path = info_dir.join(format!("{name}.trashinfo"));

    // The info file is created exclusively, which is what reserves the name
    // against another trashing operation racing for it.
    let info = format!(
        "[Trash Info]\nPath={}\nDeletionDate={}\n",
        uri_escape(&original.to_string_lossy()),
        deletion_date(std::time::SystemTime::now())
    );
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&info_path)?;
        f.write_all(info.as_bytes())?;
    }

    match std::fs::rename(&original, trash_files.join(&name)) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Leave no orphaned info file behind for an entry that never moved.
            let _ = std::fs::remove_file(&info_path);
            Err(e)
        }
    }
}

/// Finds a name not already taken in either Trash directory.
fn unique_name(files: &std::path::Path, info: &std::path::Path, stem: &str) -> String {
    if !files.join(stem).exists() && !info.join(format!("{stem}.trashinfo")).exists() {
        return stem.to_owned();
    }

    let (base, ext) = match stem.rsplit_once('.') {
        Some((b, e)) if !b.is_empty() => (b, format!(".{e}")),
        _ => (stem, String::new()),
    };
    for n in 1u32.. {
        let candidate = format!("{base}.{n}{ext}");
        if !files.join(&candidate).exists() && !info.join(format!("{candidate}.trashinfo")).exists()
        {
            return candidate;
        }
    }
    unreachable!("the loop returns before exhausting u32")
}

/// Percent-escapes the characters the spec requires in a `Path=` value.
fn uri_escape(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Formats a timestamp as the local-time ISO 8601 the spec asks for.
///
/// Written out by hand rather than pulling in a date library for one string.
/// The conversion is Howard Hinnant's civil-from-days, which is exact for any
/// date the epoch can represent.
fn deletion_date(time: std::time::SystemTime) -> String {
    let secs = time
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days);

    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}")
}

/// Days since 1970-01-01 to a calendar date.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;

    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("bananadock-trash-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn an_empty_directory_reads_as_empty() {
        let d = temp("empty");
        assert!(!read(&d).full);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn any_entry_makes_it_full() {
        let d = temp("full");
        std::fs::write(d.join("something"), b"x").unwrap();
        assert!(read(&d).full);
        std::fs::remove_dir_all(&d).ok();
    }

    /// A Trash that has never been used has no directory at all, and that is
    /// not an error.
    #[test]
    fn a_missing_directory_reads_as_empty() {
        let d = std::env::temp_dir().join("bananadock-trash-nonexistent-7c1f");
        let _ = std::fs::remove_dir_all(&d);
        assert!(!read(&d).full);
    }

    // -- moving to the trash -----------------------------------------------

    /// Pinned against known dates; an off-by-one here would write a wrong
    /// deletion date into every trashinfo file and nothing would complain.
    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        // 2000-03-01, just past a leap day in a century leap year.
        assert_eq!(civil_from_days(11017), (2000, 3, 1));
        assert_eq!(civil_from_days(19723), (2024, 1, 1));
    }

    #[test]
    fn deletion_date_is_iso_8601() {
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        assert_eq!(deletion_date(t), "2023-11-14T22:13:20");
    }

    #[test]
    fn uri_escape_encodes_what_the_spec_requires() {
        assert_eq!(uri_escape("/home/a/my file.txt"), "/home/a/my%20file.txt");
        assert_eq!(uri_escape("/tmp/a-b_c.d~e"), "/tmp/a-b_c.d~e");
    }

    #[test]
    fn moving_puts_the_file_in_files_and_writes_its_info() {
        let root = temp("move");
        let files = root.join("Trash/files");
        std::fs::create_dir_all(&files).unwrap();
        let victim = root.join("doomed.txt");
        std::fs::write(&victim, b"bye").unwrap();

        move_to_trash(&victim, &files).unwrap();

        assert!(!victim.exists(), "original should be gone");
        assert_eq!(std::fs::read(files.join("doomed.txt")).unwrap(), b"bye");

        let info = std::fs::read_to_string(root.join("Trash/info/doomed.txt.trashinfo")).unwrap();
        assert!(info.starts_with("[Trash Info]"), "{info}");
        assert!(info.contains("Path=/"), "{info}");
        assert!(info.contains("DeletionDate=20"), "{info}");
        std::fs::remove_dir_all(&root).ok();
    }

    /// Two files of the same name must not overwrite one another in the Trash.
    #[test]
    fn a_name_collision_gets_a_suffix() {
        let root = temp("collide");
        let files = root.join("Trash/files");
        std::fs::create_dir_all(&files).unwrap();

        for dir in ["a", "b"] {
            let sub = root.join(dir);
            std::fs::create_dir_all(&sub).unwrap();
            let victim = sub.join("same.txt");
            std::fs::write(&victim, dir.as_bytes()).unwrap();
            move_to_trash(&victim, &files).unwrap();
        }

        assert!(files.join("same.txt").exists());
        assert!(
            files.join("same.1.txt").exists(),
            "second file needs a distinct name"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn trashing_something_that_does_not_exist_fails_cleanly() {
        let root = temp("missing");
        let files = root.join("Trash/files");
        std::fs::create_dir_all(&files).unwrap();

        assert!(move_to_trash(&root.join("nope.txt"), &files).is_err());
        // No orphaned info file for a move that never happened.
        assert!(!root.join("Trash/info/nope.txt.trashinfo").exists());
        std::fs::remove_dir_all(&root).ok();
    }

    /// A trashed directory counts just as much as a trashed file.
    #[test]
    fn a_subdirectory_counts_as_content() {
        let d = temp("subdir");
        std::fs::create_dir(d.join("a-folder")).unwrap();
        assert!(read(&d).full);
        std::fs::remove_dir_all(&d).ok();
    }
}
