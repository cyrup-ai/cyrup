//! The `--session-roots` wire encoding (pi `src/inspectors/session-roots-codec.ts`, 31 lines
//! @v0.68.0).
//!
//! # Why the payload is base64 at all — pi's own rationale, ported verbatim
//!
//! > Windows PowerShell has no backslash-escape for embedded double quotes: a
//! > double-quoted string literal only recognizes `` `" `` or `""` to embed a
//! > literal quote, so a naively-quoted JSON array (which is full of `"` and
//! > `\` characters) gets truncated or split into multiple argv tokens the
//! > moment PowerShell tokenizes the `pane run` command line.
//! >
//! > Base64 has no quotes, backslashes, or spaces for any shell to mangle, so
//! > encoding the `--session-roots` payload sidesteps quoting entirely. It is
//! > also plain ASCII, so it never hits shellQuote's Windows quoting branch in
//! > a way that could still fail as new characters are added upstream.
//!
//! That paragraph is the whole reason this module exists rather than the roots being passed as a
//! repeated `--session-root` flag or a raw JSON argument: the launch string this crate builds
//! ([`super::actions::launch_for`]) is handed to a TERMINAL HOST to type into a shell
//! (`herdr pane run <id> <displayCommand>`, `inspectors/herdr/actions.ts:111`), so it survives one
//! more round of shell tokenisation than an argv this process spawns itself ever would.
//!
//! The encoding is standard (padded) base64 of the UTF-8 JSON array, byte-for-byte what
//! `Buffer.from(JSON.stringify(roots), "utf-8").toString("base64")` produces, so a pi-written
//! launch string decodes here and a cyrup-written one decodes in pi.

use std::path::PathBuf;

use base64::Engine as _;

/// The single stable validation error [`decode_session_roots`] raises.
///
/// pi collapses every failure mode — not base64, not JSON, not an array, an array holding a
/// non-string — into ONE throw (`session-roots-codec.ts:27-30`, whose `catch` comment reads
/// *"Report one stable validation error below."*). The message is model-visible: it is what
/// `--session-roots` rejection prints out of the inspector runner's argv parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("--session-roots must be a base64-encoded JSON array of strings.")]
pub struct SessionRootsDecodeError;

/// pi `encodeSessionRoots` (`session-roots-codec.ts:13-15`).
///
/// Paths are rendered with [`std::path::Path::to_string_lossy`] because the wire form is JSON, which has no
/// representation for a non-UTF-8 path component. That is not a narrowing of upstream: pi's roots
/// are JavaScript strings, i.e. already Unicode, so a path this crate could not render is one pi
/// could not have carried either.
#[must_use]
pub fn encode_session_roots(roots: &[PathBuf]) -> String {
    let as_strings: Vec<String> = roots
        .iter()
        .map(|root| root.to_string_lossy().into_owned())
        .collect();
    // `serde_json::to_string` on a `Vec<String>` cannot fail, but this crate denies `unwrap`:
    // an empty array is the only honest fallback and is what an empty roots list encodes to
    // anyway, so the two paths agree.
    let json = serde_json::to_string(&as_strings).unwrap_or_else(|_| "[]".to_string());
    base64::engine::general_purpose::STANDARD.encode(json.as_bytes())
}

/// pi `decodeSessionRoots` (`session-roots-codec.ts:23-31`) — the inverse, used by the inspector
/// runner's argv parser.
///
/// # Errors
///
/// [`SessionRootsDecodeError`] when `raw` is not base64, is not UTF-8, is not JSON, is not a JSON
/// array, or is an array with a non-string member. pi's `parseStringArray`
/// (`session-roots-codec.ts:17-20`) rejects the last case explicitly rather than coercing, and so
/// does this.
pub fn decode_session_roots(raw: &str) -> Result<Vec<PathBuf>, SessionRootsDecodeError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(raw.as_bytes())
        .map_err(|_| SessionRootsDecodeError)?;
    let text = String::from_utf8(bytes).map_err(|_| SessionRootsDecodeError)?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| SessionRootsDecodeError)?;
    let serde_json::Value::Array(items) = value else {
        return Err(SessionRootsDecodeError);
    };
    let mut roots = Vec::with_capacity(items.len());
    for item in items {
        let serde_json::Value::String(root) = item else {
            return Err(SessionRootsDecodeError);
        };
        roots.push(PathBuf::from(root));
    }
    Ok(roots)
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

    /// T-CODEC-1 — the byte-exact wire form. GUT `encode_session_roots` to return the plain JSON
    /// (drop the base64 hop) and this goes RED on the literal, which is what a pi-written launch
    /// string would fail to decode against.
    #[test]
    fn encoding_is_standard_base64_of_the_json_array() {
        let encoded = encode_session_roots(&[PathBuf::from("/a"), PathBuf::from("/b")]);
        // `["/a","/b"]` — 11 bytes — in standard, padded base64.
        assert_eq!(encoded, "WyIvYSIsIi9iIl0=");
    }

    /// The empty list is a valid payload, not an omission: `launch_for` always emits the flag.
    #[test]
    fn an_empty_root_list_encodes_to_the_empty_json_array() {
        let encoded = encode_session_roots(&[]);
        assert_eq!(encoded, "W10=");
        assert_eq!(
            decode_session_roots(&encoded).unwrap(),
            Vec::<PathBuf>::new()
        );
    }

    /// T-CODEC-2 — round trip, including the characters the PowerShell rationale is about.
    /// GUT the decoder's base64 hop and this goes RED.
    #[test]
    fn roots_round_trip_through_the_encoding() {
        let roots = vec![
            PathBuf::from("/home/user/with space/and\"quote"),
            PathBuf::from("C:\\Users\\back\\slash"),
            PathBuf::from("/plain"),
        ];
        let decoded = decode_session_roots(&encode_session_roots(&roots)).unwrap();
        assert_eq!(decoded, roots);
    }

    /// The whole point of the module: the encoded payload contains nothing a shell tokenises.
    #[test]
    fn the_encoded_payload_has_no_quote_backslash_or_space() {
        let encoded = encode_session_roots(&[PathBuf::from("/a b\"c\\d")]);
        assert!(
            !encoded.contains('"') && !encoded.contains('\\') && !encoded.contains(' '),
            "base64 payload must be shell-inert: {encoded}"
        );
    }

    /// T-CODEC-3 — every rejection funnels to pi's ONE sentence. GUT any arm to `Ok(vec![])` and
    /// the matching row goes RED.
    #[test]
    fn every_malformed_payload_reports_one_stable_sentence() {
        let not_base64 = "!!!!not base64!!!!";
        let not_json = base64::engine::general_purpose::STANDARD.encode("not json");
        let not_an_array = base64::engine::general_purpose::STANDARD.encode(r#"{"a":1}"#);
        let not_all_strings = base64::engine::general_purpose::STANDARD.encode(r#"["/a",7]"#);
        let not_utf8 = base64::engine::general_purpose::STANDARD.encode([0xff_u8, 0xfe]);
        for raw in [
            not_base64.to_string(),
            not_json,
            not_an_array,
            not_all_strings,
            not_utf8,
        ] {
            let err = decode_session_roots(&raw).expect_err("must refuse");
            assert_eq!(
                err.to_string(),
                "--session-roots must be a base64-encoded JSON array of strings."
            );
        }
    }
}
