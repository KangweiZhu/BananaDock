//! Interpreting things dropped onto the dock.

use std::path::PathBuf;

/// MIME type file managers use for dragged files.
pub const URI_LIST: &str = "text/uri-list";

/// Parses a `text/uri-list` payload into local paths.
///
/// The format is CRLF-separated, `#` starts a comment line, and the URIs are
/// percent-encoded. Anything that is not a `file:` URI is dropped: the dock can
/// only act on local paths, and handing a remote URL to a launcher as if it
/// were a filename would produce a confusing failure inside the application
/// rather than here.
pub fn parse_uri_list(data: &str) -> Vec<PathBuf> {
    data.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.strip_prefix("file://"))
        .map(percent_decode)
        // A `file://host/path` URI keeps its leading slash after the authority;
        // an empty authority leaves the path starting at the slash already.
        .map(PathBuf::from)
        .collect()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }

    // Percent-decoding works on bytes, so the result may not be valid UTF-8;
    // a lossy conversion keeps a mangled name rather than losing the file.
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether a dropped path is a desktop entry, which pins rather than opens.
pub fn is_desktop_entry(path: &std::path::Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("desktop"))
}

/// The desktop entry id a `.desktop` path corresponds to.
pub fn desktop_id(path: &std::path::Path) -> Option<String> {
    path.file_stem().and_then(|s| s.to_str()).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_simple_list() {
        let list = "file:///home/a/one.txt\r\nfile:///home/a/two.txt\r\n";
        assert_eq!(
            parse_uri_list(list),
            vec![
                PathBuf::from("/home/a/one.txt"),
                PathBuf::from("/home/a/two.txt")
            ]
        );
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let list = "# a comment\r\n\r\nfile:///tmp/x\r\n";
        assert_eq!(parse_uri_list(list), vec![PathBuf::from("/tmp/x")]);
    }

    /// Spaces and other awkward characters arrive percent-encoded; leaving them
    /// that way would produce a path that does not exist.
    #[test]
    fn percent_escapes_are_decoded() {
        let list = "file:///tmp/my%20file%20(1).txt\r\n";
        assert_eq!(
            parse_uri_list(list),
            vec![PathBuf::from("/tmp/my file (1).txt")]
        );
    }

    #[test]
    fn multibyte_escapes_decode_to_utf8() {
        // A non-ASCII name, percent-encoded a byte at a time.
        let list = "file:///tmp/%E6%96%87%E4%BB%B6\r\n";
        assert_eq!(parse_uri_list(list), vec![PathBuf::from("/tmp/文件")]);
    }

    #[test]
    fn non_file_uris_are_dropped() {
        let list = "https://example.com/x\r\nfile:///tmp/y\r\n";
        assert_eq!(parse_uri_list(list), vec![PathBuf::from("/tmp/y")]);
    }

    /// A stray `%` that is not a valid escape must pass through rather than
    /// eating the characters after it.
    #[test]
    fn a_malformed_escape_is_left_alone() {
        let list = "file:///tmp/100%25\r\nfile:///tmp/bare%\r\n";
        assert_eq!(
            parse_uri_list(list),
            vec![PathBuf::from("/tmp/100%"), PathBuf::from("/tmp/bare%")]
        );
    }

    #[test]
    fn plain_newlines_work_as_well_as_crlf() {
        assert_eq!(
            parse_uri_list("file:///a\nfile:///b"),
            vec![PathBuf::from("/a"), PathBuf::from("/b")]
        );
    }

    #[test]
    fn an_empty_payload_yields_nothing() {
        assert!(parse_uri_list("").is_empty());
    }

    #[test]
    fn desktop_entries_are_recognised_by_extension() {
        assert!(is_desktop_entry(std::path::Path::new("/a/firefox.desktop")));
        assert!(is_desktop_entry(std::path::Path::new("/a/Firefox.DESKTOP")));
        assert!(!is_desktop_entry(std::path::Path::new("/a/notes.txt")));
    }

    #[test]
    fn desktop_id_is_the_file_stem() {
        assert_eq!(
            desktop_id(std::path::Path::new(
                "/usr/share/applications/org.kde.dolphin.desktop"
            )),
            Some("org.kde.dolphin".to_owned())
        );
    }
}
