//! `valid_git_ref` — pi `validGitRef` (`scripted-workflow.ts:14-19`) and its verbatim refusal
//! text. Hand-rolled per SCOPE_3 §A.3's disposition: both upstream regexes here are a length test
//! plus character scans, so no regex engine appears in this file (and `fancy-regex` must not —
//! SCOPE_3f §4.5).

/// pi `BASE_REF_VALIDATION_ERROR` (`scripted-workflow.ts:13`), byte-verbatim. Its three call
/// sites prefix it differently — `` `{owner} {MSG}` `` (`:1547`), `` `runs.run('{key}') {MSG}` ``
/// (`:2155`), and bare (guest `:648`) — so the constant carries NO prefix.
pub const BASE_REF_VALIDATION_ERROR: &str = "baseRef must be a valid Git ref: use HEAD or a supported named ref (for example, refs/heads/main). Full 40/64-character commit IDs and revision expressions are unsupported.";

/// Is `ref_text` an acceptable `baseRef`? pi `validGitRef` (`scripted-workflow.ts:14-19`).
///
/// Eight grounds for rejection, in source order (the two a naive port drops are #2's *UTF-8
/// byte* unit — upstream measures `Buffer.byteLength`, not `.length` — and #5's deliberate
/// rejection of full commit ids):
///
/// 1. empty, or exactly `"@"` (the upstream `typeof` arm is the `&str` parameter type here);
/// 2. more than 1024 **UTF-8 bytes**;
/// 3. starts with `/`, ends with `/`, or contains `//`;
/// 4. contains `..` or `@{`;
/// 5. a full 40- or 64-hex-digit commit id — rejected **on purpose** (`:16`);
/// 6. any of `[ ] \ ~ ^ : ? *`, `U+0000..=U+0020`, or `U+007F` (`:17`);
/// 7. ends with `.` or `.lock` (`:17`);
/// 8. every `/`-separated component is non-empty, not `.`/`..`, has no leading `.`, and no
///    trailing `.` or `.lock` (`:18`).
#[must_use]
pub fn valid_git_ref(ref_text: &str) -> bool {
    // Grounds 1–4.
    if ref_text.is_empty()
        || ref_text == "@"
        || ref_text.len() > 1024
        || ref_text.starts_with('/')
        || ref_text.ends_with('/')
        || ref_text.contains("//")
        || ref_text.contains("..")
        || ref_text.contains("@{")
    {
        return false;
    }
    // Ground 5: `/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/i` — a length test plus `is_ascii_hexdigit`.
    // `is_ascii_hexdigit` accepts exactly `[0-9a-fA-F]`, the `i`-flagged upstream class.
    if (ref_text.len() == 40 || ref_text.len() == 64)
        && ref_text.chars().all(|c| c.is_ascii_hexdigit())
    {
        return false;
    }
    // Ground 6: `/[[\]\\~^:?*\u0000-\u0020\u007f]/u` — one `chars().any`.
    if ref_text.chars().any(|c| {
        matches!(c, '[' | ']' | '\\' | '~' | '^' | ':' | '?' | '*' | '\u{7f}') || c <= '\u{20}'
    }) {
        return false;
    }
    // Ground 7.
    if ref_text.ends_with('.') || ref_text.ends_with(".lock") {
        return false;
    }
    // Ground 8.
    ref_text.split('/').all(|component| {
        !component.is_empty()
            && component != "."
            && component != ".."
            && !component.starts_with('.')
            && !component.ends_with('.')
            && !component.ends_with(".lock")
    })
}

#[cfg(test)]
mod tests {
    use super::{BASE_REF_VALIDATION_ERROR, valid_git_ref};

    #[test]
    fn accepts_the_documented_forms() {
        for ok in [
            "HEAD",
            "main",
            "refs/heads/main",
            "release-1.2",
            "feature/a_b",
            "v1.0.0",
        ] {
            assert!(valid_git_ref(ok), "{ok} should be accepted");
        }
    }

    #[test]
    fn ground_1_empty_and_bare_at() {
        assert!(!valid_git_ref(""));
        assert!(!valid_git_ref("@"));
        // "@" only alone is rejected by ground 1; "v@1" survives it.
        assert!(valid_git_ref("v@1"));
    }

    #[test]
    fn ground_2_utf8_bytes_not_chars() {
        // 512 two-byte chars = 512 UTF-16 units (upstream `.length` would pass a 1024-unit test)
        // but 1024 UTF-8 bytes — the boundary; one more byte rejects.
        let at_limit = "é".repeat(512);
        assert!(valid_git_ref(&at_limit));
        let over = format!("{at_limit}a");
        assert!(!valid_git_ref(&over));
    }

    #[test]
    fn ground_3_and_4_slashes_dotdot_atbrace() {
        for bad in ["/lead", "trail/", "a//b", "a..b", "a@{b"] {
            assert!(!valid_git_ref(bad), "{bad} should be rejected");
        }
    }

    #[test]
    fn ground_5_full_commit_ids_rejected_on_purpose() {
        let sha40 = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(sha40.len(), 40);
        assert!(!valid_git_ref(sha40));
        assert!(
            !valid_git_ref(&sha40.to_uppercase()),
            "the upstream regex is /i"
        );
        let sha64 = "a".repeat(64);
        assert!(!valid_git_ref(&sha64));
        // 39 hex digits is NOT a full commit id and passes this ground.
        assert!(valid_git_ref(&"a".repeat(39)));
    }

    #[test]
    fn ground_6_forbidden_characters() {
        for bad in [
            "a[b", "a]b", "a\\b", "a~b", "a^b", "a:b", "a?b", "a*b", "a b", "a\tb", "a\u{7f}b",
            "a\u{0}b",
        ] {
            assert!(!valid_git_ref(bad), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn ground_7_and_8_dot_suffixes_and_components() {
        for bad in [
            "end.",
            "end.lock",
            "a/.hidden",
            "a/mid./b",
            "a/x.lock/b",
            "a/./b",
        ] {
            assert!(!valid_git_ref(bad), "{bad} should be rejected");
        }
    }

    #[test]
    fn error_text_is_verbatim() {
        assert!(BASE_REF_VALIDATION_ERROR.starts_with("baseRef must be a valid Git ref:"));
        assert!(BASE_REF_VALIDATION_ERROR.ends_with("revision expressions are unsupported."));
    }
}
