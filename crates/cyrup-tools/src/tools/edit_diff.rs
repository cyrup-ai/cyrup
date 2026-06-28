//! Line-ending/BOM handling + diff/patch generation for `edit` (R-03-018/019, arch-03 §6.4).

use similar::TextDiff;

const BOM: &str = "\u{feff}";

/// Original line-ending style, restored on write (R-03-018).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    Crlf,
    Cr,
}

impl LineEnding {
    fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::Crlf => "\r\n",
            LineEnding::Cr => "\r",
        }
    }
}

/// Strip a leading UTF-8 BOM, returning `(had_bom, body)`.
pub fn strip_bom(s: &str) -> (bool, &str) {
    match s.strip_prefix(BOM) {
        Some(rest) => (true, rest),
        None => (false, s),
    }
}

/// Detect the dominant line ending.
pub fn detect_line_ending(s: &str) -> LineEnding {
    if s.contains("\r\n") {
        LineEnding::Crlf
    } else if s.contains('\r') {
        LineEnding::Cr
    } else {
        LineEnding::Lf
    }
}

/// Normalize any line ending to LF for matching.
pub fn normalize_to_lf(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// Restore LF text to the original ending.
pub fn restore_line_endings(s: &str, ending: LineEnding) -> String {
    match ending {
        LineEnding::Lf => s.to_string(),
        other => s.replace('\n', other.as_str()),
    }
}

/// A unified patch (`@@ -a,b +c,d @@`) for `path` between LF-normalized `old` and `new`.
pub fn unified_patch(path: &str, old: &str, new: &str) -> String {
    TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string()
}

/// A `+`/`-`/` ` display diff between LF-normalized `old` and `new`.
pub fn display_diff(old: &str, new: &str) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut out = String::new();
    for change in diff.iter_all_changes() {
        let sign = match change.tag() {
            similar::ChangeTag::Delete => '-',
            similar::ChangeTag::Insert => '+',
            similar::ChangeTag::Equal => ' ',
        };
        out.push(sign);
        out.push_str(change.value());
        if !change.value().ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// 1-indexed first line that differs between LF-normalized `old` and `new` (None if identical).
pub fn first_changed_line(old: &str, new: &str) -> Option<usize> {
    for (i, (a, b)) in old.lines().zip(new.lines()).enumerate() {
        if a != b {
            return Some(i + 1);
        }
    }
    let old_count = old.lines().count();
    let new_count = new.lines().count();
    if old_count != new_count {
        Some(old_count.min(new_count) + 1)
    } else {
        None
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn bom_and_crlf_roundtrip() {
        let raw = "\u{feff}a\r\nb\r\n";
        let (bom, body) = strip_bom(raw);
        assert!(bom);
        assert_eq!(detect_line_ending(body), LineEnding::Crlf);
        let norm = normalize_to_lf(body);
        assert_eq!(norm, "a\nb\n");
        let restored = restore_line_endings(&norm, LineEnding::Crlf);
        assert_eq!(restored, "a\r\nb\r\n");
    }

    #[test]
    fn patch_and_first_line() {
        let old = "one\ntwo\nthree\n";
        let new = "one\nTWO\nthree\n";
        let patch = unified_patch("f.txt", old, new);
        assert!(patch.contains("@@"));
        assert!(patch.contains("-two"));
        assert!(patch.contains("+TWO"));
        assert_eq!(first_changed_line(old, new), Some(2));
    }
}
