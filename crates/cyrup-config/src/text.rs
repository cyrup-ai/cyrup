//! Decoded-text helpers shared by every file reader in this crate.
//!
//! 1:1 port of pi `packages/coding-agent/src/utils/text.ts:7-9` @v0.87.1.

/// Remove a leading UTF-8 byte order mark from decoded text.
///
/// pi `utils/text.ts:7-9` — `stripBom` is `splitBom(content).text`, and `splitBom`
/// (`:2-4`) is `content.startsWith("﻿") ? content.slice(1) : content`. `slice(1)`
/// drops exactly ONE BOM code point, so a doubled BOM keeps its second one; this port
/// matches that with a single `strip_prefix`.
///
/// cyrup does not need `splitBom` itself: upstream's only consumer of the `bom` half is
/// its re-serializing writer, and every cyrup write path already emits BOM-free text.
#[must_use]
pub fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

#[cfg(test)]
mod tests {
    use super::strip_bom;

    #[test]
    fn strips_exactly_one_bom() {
        assert_eq!(strip_bom("\u{feff}{}"), "{}");
        // pi's `slice(1)` removes one BOM only — the second survives.
        assert_eq!(strip_bom("\u{feff}\u{feff}{}"), "\u{feff}{}");
    }

    #[test]
    fn leaves_bom_less_text_untouched() {
        assert_eq!(strip_bom("{}"), "{}");
        assert_eq!(strip_bom(""), "");
        // Only a LEADING mark counts (pi uses `startsWith`).
        assert_eq!(strip_bom("{}\u{feff}"), "{}\u{feff}");
    }
}
