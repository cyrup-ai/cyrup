//! [`IndexSegment`] — a single, already-encoded filesystem path component.
//!
//! Ports pi `runs/background/index-segment.ts` (59 LOC) in full.

use std::fmt::Write as _;

use sha2::{Digest, Sha256};

/// The `~sha256-` prefix marking a hashed (rather than URI-encoded) segment.
///
/// pi `HASHED_INDEX_SEGMENT_PREFIX` (`index-segment.ts:15`). `~` is deliberately outside the
/// `encodeURIComponent` unreserved set's overlap with a leading character an encoded value can
/// produce, so a hashed key can never collide with an encoded one.
const HASHED_INDEX_SEGMENT_PREFIX: &str = "~sha256-";

/// A single, already-encoded filesystem path component.
///
/// # The invariant, and why it is a type
///
/// The inner `String` is private and there is deliberately **no** `From<String>`,
/// `Deref<Target = str>`, `AsRef<Path>` or public field: [`IndexSegment::encode`] is the only way
/// to obtain one. A session id — which in cyrup, as in pi, is frequently a full `.jsonl` *path* —
/// therefore cannot reach `Path::join` without passing through the encoder. Every path builder in
/// [`crate::background::result_index`] accepts `&IndexSegment` and never `&str`, so "remember to
/// call encode first" stops being a convention a future caller can forget.
///
/// # This is an on-disk format, not an implementation detail
///
/// The encoding is byte-compatible with pi's: the same session id produces the same directory
/// name under both implementations. Diverging — even in a way that is individually reasonable on
/// Unix — makes cyrup's index unreadable by pi and vice versa, so the Windows-reserved-name and
/// trailing-extension rules below are ported despite cyrup targeting Unix.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IndexSegment(String);

impl IndexSegment {
    /// Keep opaque index keys below common filesystem component limits.
    ///
    /// pi `MAX_INDEX_SEGMENT_BYTES` (`index-segment.ts:14`).
    pub const MAX_BYTES: usize = 255;

    /// pi `encodeIndexSegment` (`index-segment.ts:32-41`): URI-encode, falling back to
    /// `~sha256-<hex>` when the result exceeds [`Self::MAX_BYTES`] or is non-portable.
    #[must_use]
    pub fn encode(value: &str) -> Self {
        Self::encode_bounded(value, Self::MAX_BYTES)
    }

    /// [`Self::encode`] with an explicit budget — used by callers that must reserve room for a
    /// suffix, e.g. `result-files.ts:16`'s `MAX_JSON_FILE_STEM_BYTES`, which is
    /// `MAX_INDEX_SEGMENT_BYTES` minus `".json"`.
    ///
    /// pi's `try`/`catch` around `encodeURIComponent` (`:34-38`) guards against a `URIError` on a
    /// lone surrogate. A Rust `&str` is always well-formed UTF-8 and cannot contain one, so that
    /// branch is unreachable here and is not ported as a fallible path — the hash fallback it led
    /// to is still reached by the length and portability checks below.
    #[must_use]
    pub fn encode_bounded(value: &str, max_bytes: usize) -> Self {
        let encoded = encode_uri_component(value);
        // `encodeURIComponent` output is pure ASCII, so byte length and char length agree; pi's
        // `Buffer.byteLength(encoded, "utf-8")` is `encoded.len()` here.
        if encoded.len() <= max_bytes && is_portable_segment(&encoded) {
            Self(encoded)
        } else {
            Self(hashed_segment(value))
        }
    }

    /// pi `indexSegmentAliases` (`:55-59`): the current write key first, then the pre-hash
    /// URI-encoded key when it still fits. Never empty.
    ///
    /// # The read/write asymmetry is the compatibility mechanism
    ///
    /// **Writes** use [`Self::encode`] alone — exactly one key. **Reads** fan out over this list.
    /// That asymmetry is what lets the encoding rules tighten over time without orphaning data:
    /// a value that used to encode to `foo.jsonl` and now hashes is still *found* at its old
    /// location, while everything newly written lands at the new one.
    ///
    /// Note the historical branch (pi `:43-52`) applies a deliberately **weaker** predicate than
    /// [`is_portable_segment`] — it omits the `%5C` and trailing-extension checks. Porting that
    /// asymmetry is the whole point: those two rules are the ones that were *added*, so the keys
    /// they now reject are precisely the keys that may already exist on disk.
    #[must_use]
    pub fn aliases(value: &str, max_bytes: usize) -> Vec<Self> {
        let current = Self::encode_bounded(value, max_bytes);
        match historically_readable_segment(value, max_bytes) {
            Some(historical) if historical != current.0 => vec![current, Self(historical)],
            _ => vec![current],
        }
    }

    /// [`Self::aliases`] at the default budget.
    #[must_use]
    pub fn read_aliases(value: &str) -> Vec<Self> {
        Self::aliases(value, Self::MAX_BYTES)
    }

    /// Borrows the encoded component. Crate-internal: outside this crate an `IndexSegment` is only
    /// ever handed to a path builder, never inspected.
    #[must_use]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for IndexSegment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// `~sha256-<lowercase hex>` of the **pre-encoding** value.
///
/// pi `hashedSegment` (`index-segment.ts:18-20`). Hashing the raw value rather than the encoded
/// one is load-bearing: it means the hash is stable even if the encoder changes.
fn hashed_segment(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(HASHED_INDEX_SEGMENT_PREFIX.len() + digest.len() * 2);
    out.push_str(HASHED_INDEX_SEGMENT_PREFIX);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// JavaScript `encodeURIComponent`.
///
/// Unreserved set: `A-Z a-z 0-9 - _ . ! ~ * ' ( )`. Everything else becomes `%XX` over the value's
/// UTF-8 bytes, with **uppercase** hex — `encodeURIComponent("\\")` is `"%5C"`, not `"%5c"`, and
/// the `%5C` portability check below is written against that.
///
/// Hand-rolled rather than taken from `percent-encoding`: that crate's `NON_ALPHANUMERIC` set does
/// not match `encodeURIComponent`, so a custom `AsciiSet` would be required regardless, and this
/// is an on-disk format better pinned by the unit tests below than by a third party's set
/// definition.
///
/// `pub(crate)` because [`crate::background::completion_replay`] addresses its files with the RAW
/// encoder rather than through [`IndexSegment`] — pi `completion-replay.ts:41-43` and
/// `async-retention.ts:505` both do, and the `~sha256-` fallback [`IndexSegment::encode_bounded`]
/// applies would put a long or non-portable run id's record at a name pi cannot find.
pub(crate) fn encode_uri_component(value: &str) -> String {
    const UNRESERVED_PUNCT: &[u8] = b"-_.!~*'()";
    let mut out = String::with_capacity(value.len());
    for &byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || UNRESERVED_PUNCT.contains(&byte) {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// JavaScript `decodeURIComponent`, returning `None` exactly where JS throws `URIError`.
///
/// The inverse of [`encode_uri_component`], needed by
/// [`crate::background::completion_replay`]'s directory sweep to recover a run id from a file name
/// (pi `runIdFromReplayFile`, `completion-replay.ts:149-157`). Two rejection cases, both of which
/// upstream converts to `undefined` via its `try`/`catch`:
///
/// * a malformed escape (`%`, `%A`, `%ZZ`) — JS throws `URIError`;
/// * an escape sequence that decodes to invalid UTF-8 — JS also throws `URIError`, and Rust cannot
///   construct the `String` either. Decoding into `Vec<u8>` and validating ONCE at the end is what
///   makes a multi-byte character spread across several `%XX` escapes decode correctly; validating
///   per-escape would reject every non-ASCII round-trip.
///
/// Deliberately permissive about escape CASE (`%2f` decodes like `%2F`), exactly as JS is. That is
/// safe only because the caller re-encodes and compares against the original file name
/// (pi `:153`'s `safeRunFile(runId) === file`) — the round-trip, not this decoder, is what rejects
/// a non-canonical name.
pub(crate) fn decode_uri_component(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes.get(index) {
            Some(b'%') => {
                let high = bytes.get(index + 1).copied().and_then(hex_nibble)?;
                let low = bytes.get(index + 2).copied().and_then(hex_nibble)?;
                out.push(high * 16 + low);
                index += 3;
            }
            Some(&byte) => {
                out.push(byte);
                index += 1;
            }
            None => break,
        }
    }
    String::from_utf8(out).ok()
}

/// One hexadecimal digit's value, or `None` for anything else — the `%XX` half of
/// [`decode_uri_component`].
fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// pi `isPortableSegment` (`index-segment.ts:22-30`).
///
/// Encoded backslashes and trailing file extensions are not portable: on Windows, `readdir` of a
/// directory whose name ends in `.jsonl` or contains `%5C` can fail with `EPERM`, and pi session
/// ids are routinely full `.jsonl` paths.
fn is_portable_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.ends_with('.')
        && !is_windows_reserved_name(value)
        && !contains_encoded_backslash(value)
        && !has_trailing_extension(value)
}

/// pi `WINDOWS_RESERVED_NAME` (`index-segment.ts:16`):
/// `/^(?:con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/i`.
fn is_windows_reserved_name(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    // The `(?:\.|$)` tail: the token must be followed by a dot or nothing at all.
    let terminated_at = |index: usize| matches!(bytes.get(index), None | Some(b'.'));

    for token in ["con", "prn", "aux", "nul"] {
        if lower.starts_with(token) && terminated_at(token.len()) {
            return true;
        }
    }
    for prefix in ["com", "lpt"] {
        if lower.starts_with(prefix)
            && bytes.get(prefix.len()).is_some_and(u8::is_ascii_digit)
            && bytes.get(prefix.len()) != Some(&b'0')
            && terminated_at(prefix.len() + 1)
        {
            return true;
        }
    }
    false
}

/// pi `/%5C/i` (`index-segment.ts:28`) — an encoded backslash anywhere in the segment.
fn contains_encoded_backslash(value: &str) -> bool {
    value.to_ascii_lowercase().contains("%5c")
}

/// pi `/\.[A-Za-z][A-Za-z0-9]{0,7}$/` (`index-segment.ts:29`) — a trailing file extension of 1..=8
/// characters whose first character is alphabetic.
///
/// Only the LAST dot can begin a match: the pattern is anchored at the end and every character
/// after the dot must be alphanumeric, so an earlier dot's run would have to contain the later dot
/// and fail.
fn has_trailing_extension(value: &str) -> bool {
    let Some(dot) = value.rfind('.') else {
        return false;
    };
    let Some(ext) = value.get(dot + 1..) else {
        return false;
    };
    if ext.is_empty() || ext.len() > 8 {
        return false;
    }
    let mut chars = ext.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphabetic() && chars.all(|c| c.is_ascii_alphanumeric())
}

/// pi `historicallyReadableSegment` (`index-segment.ts:43-52`).
///
/// Deliberately weaker than [`is_portable_segment`]: no `%5C` check, no trailing-extension check.
/// See [`IndexSegment::aliases`] for why.
fn historically_readable_segment(value: &str, max_bytes: usize) -> Option<String> {
    let encoded = encode_uri_component(value);
    if encoded.is_empty()
        || encoded == "."
        || encoded == ".."
        || encoded.ends_with('.')
        || is_windows_reserved_name(&encoded)
    {
        return None;
    }
    if encoded.len() > max_bytes {
        return None;
    }
    Some(encoded)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    // --- encodeURIComponent parity -------------------------------------------------------

    #[test]
    fn encode_uri_component_matches_javascript_for_the_unreserved_set() {
        // encodeURIComponent leaves exactly these untouched.
        let unreserved = "ABCabc019-_.!~*'()";
        assert_eq!(encode_uri_component(unreserved), unreserved);
    }

    #[test]
    fn encode_uri_component_percent_encodes_with_uppercase_hex() {
        // The `%5C` portability rule is written against uppercase output.
        assert_eq!(encode_uri_component("\\"), "%5C");
        assert_eq!(encode_uri_component("/"), "%2F");
        assert_eq!(encode_uri_component(" "), "%20");
        assert_eq!(encode_uri_component(":"), "%3A");
    }

    #[test]
    fn encode_uri_component_encodes_utf8_bytes_individually() {
        // JS: encodeURIComponent("é") === "%C3%A9"
        assert_eq!(encode_uri_component("é"), "%C3%A9");
        // JS: encodeURIComponent("☃") === "%E2%98%83"
        assert_eq!(encode_uri_component("☃"), "%E2%98%83");
    }

    #[test]
    fn a_session_path_encodes_to_the_shape_pi_produces() {
        // JS: encodeURIComponent("/home/u/s.jsonl") === "%2Fhome%2Fu%2Fs.jsonl"
        assert_eq!(
            encode_uri_component("/home/u/s.jsonl"),
            "%2Fhome%2Fu%2Fs.jsonl"
        );
    }

    // --- portability predicate ------------------------------------------------------------

    #[test]
    fn dot_and_dotdot_and_trailing_dot_are_not_portable() {
        assert!(!is_portable_segment(""));
        assert!(!is_portable_segment("."));
        assert!(!is_portable_segment(".."));
        assert!(!is_portable_segment("foo."));
    }

    #[test]
    fn windows_reserved_names_are_rejected_bare_and_before_a_dot() {
        for name in [
            "con", "CON", "prn", "aux", "nul", "com1", "COM9", "lpt1", "lpt9",
        ] {
            assert!(is_windows_reserved_name(name), "{name} must be reserved");
        }
        assert!(is_windows_reserved_name("con.txt"));
        assert!(is_windows_reserved_name("COM4.log"));
        // Not reserved: com0 is outside [1-9], and a longer name merely starting with the token.
        assert!(!is_windows_reserved_name("com0"));
        assert!(!is_windows_reserved_name("console"));
        assert!(!is_windows_reserved_name("com10"));
        assert!(!is_windows_reserved_name("nulls"));
    }

    #[test]
    fn encoded_backslash_is_rejected_case_insensitively() {
        assert!(contains_encoded_backslash("a%5Cb"));
        assert!(contains_encoded_backslash("a%5cb"));
        assert!(!contains_encoded_backslash("a%5Db"));
    }

    #[test]
    fn trailing_extension_matches_one_to_eight_chars_starting_alphabetic() {
        assert!(has_trailing_extension("a.j"));
        assert!(has_trailing_extension("a.jsonl"));
        assert!(has_trailing_extension("a.abcdefgh")); // 8 == the {0,7} bound
        assert!(!has_trailing_extension("a.abcdefghi")); // 9 is too long
        assert!(!has_trailing_extension("a.1json")); // first char must be alphabetic
        assert!(!has_trailing_extension("a.")); // empty extension
        assert!(!has_trailing_extension("plain"));
        // An earlier dot cannot rescue a non-matching last segment.
        assert!(!has_trailing_extension("a.b.c-d"));
    }

    // --- encode ---------------------------------------------------------------------------

    #[test]
    fn a_short_portable_value_keeps_its_encoded_form() {
        assert_eq!(IndexSegment::encode("sess-a").as_str(), "sess-a");
    }

    #[test]
    fn a_jsonl_session_path_falls_back_to_the_hash() {
        // The real, common case: a full session path ends in `.jsonl`, which is non-portable,
        // so the write key is the hash — this is the branch that makes the index Windows-safe.
        let seg = IndexSegment::encode("/home/u/s.jsonl");
        assert!(
            seg.as_str().starts_with(HASHED_INDEX_SEGMENT_PREFIX),
            "got {seg}"
        );
        assert_eq!(seg.as_str().len(), HASHED_INDEX_SEGMENT_PREFIX.len() + 64);
    }

    #[test]
    fn an_over_long_value_falls_back_to_the_hash() {
        let long = "a".repeat(IndexSegment::MAX_BYTES + 1);
        assert!(
            IndexSegment::encode(&long)
                .as_str()
                .starts_with(HASHED_INDEX_SEGMENT_PREFIX)
        );
    }

    #[test]
    fn the_hash_is_over_the_raw_value_not_the_encoded_one() {
        // Stability property: the digest must not move if the encoder changes.
        let mut hasher = Sha256::new();
        hasher.update(b"/home/u/s.jsonl");
        let digest = hasher.finalize();
        let mut expected = String::from(HASHED_INDEX_SEGMENT_PREFIX);
        for byte in digest {
            let _ = write!(expected, "{byte:02x}");
        }
        assert_eq!(IndexSegment::encode("/home/u/s.jsonl").as_str(), expected);
    }

    #[test]
    fn encoding_is_deterministic() {
        assert_eq!(IndexSegment::encode("x/y"), IndexSegment::encode("x/y"));
    }

    #[test]
    fn distinct_values_do_not_collide() {
        assert_ne!(IndexSegment::encode("a"), IndexSegment::encode("b"));
        assert_ne!(
            IndexSegment::encode("/home/a.jsonl"),
            IndexSegment::encode("/home/b.jsonl")
        );
    }

    // --- aliases --------------------------------------------------------------------------

    #[test]
    fn aliases_are_never_empty_and_lead_with_the_write_key() {
        let aliases = IndexSegment::read_aliases("/home/u/s.jsonl");
        assert!(!aliases.is_empty());
        assert_eq!(aliases[0], IndexSegment::encode("/home/u/s.jsonl"));
    }

    #[test]
    fn a_jsonl_path_exposes_its_pre_hash_key_as_a_read_alias() {
        // This is the compatibility mechanism: the value now hashes, but a directory written
        // before the trailing-extension rule existed still lives at the encoded name.
        let aliases = IndexSegment::read_aliases("/home/u/s.jsonl");
        assert_eq!(aliases.len(), 2, "expected write key + historical key");
        assert!(aliases[0].as_str().starts_with(HASHED_INDEX_SEGMENT_PREFIX));
        assert_eq!(aliases[1].as_str(), "%2Fhome%2Fu%2Fs.jsonl");
    }

    #[test]
    fn a_portable_value_has_exactly_one_alias() {
        // current == historical, so pi returns a single-element list rather than a duplicate.
        assert_eq!(IndexSegment::read_aliases("sess-a").len(), 1);
    }

    #[test]
    fn an_over_long_value_has_no_historical_alias() {
        // The historical branch also enforces the byte budget, so there is nothing to fall back to.
        let long = "a".repeat(IndexSegment::MAX_BYTES + 1);
        assert_eq!(
            IndexSegment::aliases(&long, IndexSegment::MAX_BYTES).len(),
            1
        );
    }

    #[test]
    fn the_historical_predicate_is_weaker_than_the_portable_one() {
        // A `%5C`-containing value is non-portable (so it hashes) but IS historically readable.
        let aliases = IndexSegment::read_aliases("a\\b");
        assert_eq!(aliases.len(), 2);
        assert!(aliases[0].as_str().starts_with(HASHED_INDEX_SEGMENT_PREFIX));
        assert_eq!(aliases[1].as_str(), "a%5Cb");
    }

    #[test]
    fn a_windows_reserved_value_has_no_historical_alias() {
        // Reserved names are rejected by BOTH predicates, so there is no readable fallback.
        assert_eq!(IndexSegment::read_aliases("con").len(), 1);
    }

    #[test]
    fn encode_bounded_reserves_room_for_a_suffix() {
        // `result-files.ts:16`'s MAX_JSON_FILE_STEM_BYTES pattern: a value that fits at 255 but
        // not at 250 must hash under the smaller budget.
        let value = "a".repeat(252);
        assert!(
            !IndexSegment::encode_bounded(&value, 255)
                .as_str()
                .starts_with(HASHED_INDEX_SEGMENT_PREFIX)
        );
        assert!(
            IndexSegment::encode_bounded(&value, 250)
                .as_str()
                .starts_with(HASHED_INDEX_SEGMENT_PREFIX)
        );
    }
}
