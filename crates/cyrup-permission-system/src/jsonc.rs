//! JSONC → `serde_json::Value` (port of pi `jsonc-config.ts`, which uses `jsonc-parser` with
//! `allowTrailingComma: true`). pi's `jsonc-parser` accepts standard JSON plus `//` line comments,
//! `/* */` block comments, and trailing commas — and NOTHING else (it is not full JSON5: no
//! unquoted keys, no single-quoted strings). So the faithful port is a string-aware preprocessor
//! that strips exactly those three extensions and hands the result to `serde_json` — MORE faithful
//! than a JSON5 crate would be. On parse failure the caller (pi `loadGlobalConfig`,
//! `permission-manager.ts:670-681`) falls back to the `ask` policy with a warning; this module only
//! surfaces the error string.

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::ordered::OrderedValue;

/// Parse a JSONC document into any `Deserialize` target. Comments and one trailing comma per
/// array/object are stripped first; the remainder must be valid JSON. The concrete-type wrappers
/// below ([`parse`], [`parse_ordered`]) exist for the two long-standing call sites; the save path
/// (`ext_config::ExtensionConfig::save`) deserializes its own lossless order-preserving document
/// type through this.
pub fn parse_into<T: DeserializeOwned>(input: &str) -> Result<T, String> {
    let cleaned = strip_comments_and_trailing_commas(input);
    serde_json::from_str(&cleaned).map_err(|e| e.to_string())
}

/// Parse a JSONC document into a `Value` (pi `parseJsoncConfig`). Order is NOT preserved — use
/// [`parse_ordered`] for policy files whose pattern-key order is load-bearing.
pub fn parse(input: &str) -> Result<Value, String> {
    parse_into(input)
}

/// Parse a JSONC document into an order-preserving [`OrderedValue`] (the policy-file path: pattern
/// key order within a category drives last-match-wins).
pub fn parse_ordered(input: &str) -> Result<OrderedValue, String> {
    parse_into(input)
}

/// Parse a JSONC document exactly like pi's `parseJsoncConfig(input, filePath, subject)`
/// (`jsonc-config.ts:26-35`): on parse failure the error is formatted as
/// `Failed to parse {subject} at '{file_path}' (...)`, matching pi's
/// `Failed to parse ${subject} at '${filePath}' (${formatJsoncParseSummary(...)})` wording. The
/// parenthesized detail comes from `serde_json`'s own error text (which already includes its own
/// line/column) rather than pi's `printParseErrorCode` + line/column computed from
/// `jsonc-parser`'s error array — an approved, explicit deviation: `serde_json` surfaces a single
/// error with its own error taxonomy, unlike `jsonc-parser`, so pi's "N more parse errors" suffix
/// and exact error-code text cannot be reproduced. Use this (or [`parse_ordered_config`]) instead
/// of bare [`parse`]/[`parse_ordered`] wherever the caller can surface the error to a user, so the
/// warning text mirrors pi's.
pub fn parse_config(input: &str, file_path: &str, subject: &str) -> Result<Value, String> {
    parse_config_into(input, file_path, subject)
}

/// Order-preserving counterpart of [`parse_config`]; see its docs for the error-format contract.
pub fn parse_ordered_config(
    input: &str,
    file_path: &str,
    subject: &str,
) -> Result<OrderedValue, String> {
    parse_config_into(input, file_path, subject)
}

/// Generic counterpart of [`parse_config`]: same pi-shaped error wrapper, any `Deserialize` target.
pub fn parse_config_into<T: DeserializeOwned>(
    input: &str,
    file_path: &str,
    subject: &str,
) -> Result<T, String> {
    parse_into(input).map_err(|err| format_parse_error(subject, file_path, &err))
}

/// Format a parse failure exactly like pi's `Failed to parse ${subject} at '${filePath}' (...)`
/// (`jsonc-config.ts:31`).
fn format_parse_error(subject: &str, file_path: &str, err: &str) -> String {
    format!("Failed to parse {subject} at '{file_path}' ({err})")
}

/// Remove `//`/`/* */` comments and trailing commas while preserving string-literal contents
/// exactly (a `//` or `,` inside a `"..."` is data, not syntax). Single-pass, byte-accurate for
/// UTF-8 because all structural characters handled here are ASCII. Uses `slice::get` throughout so a
/// truncated/malformed tail degrades to "copy what's there" rather than panicking (no-panic policy).
fn strip_comments_and_trailing_commas(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0usize;

    while let Some(&b) = bytes.get(i) {
        let next = bytes.get(i + 1).copied();
        match b {
            b'"' => {
                // Copy the whole string literal verbatim, honoring `\"` escapes.
                out.push(b'"');
                i += 1;
                while let Some(&c) = bytes.get(i) {
                    out.push(c);
                    if c == b'\\' {
                        // Copy the escaped char too (so `\"` does not close the string).
                        if let Some(&esc) = bytes.get(i + 1) {
                            out.push(esc);
                            i += 2;
                            continue;
                        }
                    } else if c == b'"' {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b'/' if next == Some(b'/') => {
                // Line comment: skip to end-of-line (keep the newline).
                i += 2;
                while let Some(&c) = bytes.get(i) {
                    if c == b'\n' {
                        break;
                    }
                    i += 1;
                }
            }
            b'/' if next == Some(b'*') => {
                // Block comment: skip to the closing `*/`.
                i += 2;
                while let Some(&c) = bytes.get(i) {
                    if c == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        break;
                    }
                    i += 1;
                }
                i = i.saturating_add(2).min(bytes.len());
            }
            b',' if next_significant_is_close(bytes, i + 1) => {
                // Trailing comma: drop it.
                i += 1;
            }
            _ => {
                out.push(b);
                i += 1;
            }
        }
    }

    // `out` was built from valid UTF-8 slices (whole string literals + individual bytes copied in
    // order), so it is valid UTF-8.
    String::from_utf8(out).unwrap_or_default()
}

/// Peek past whitespace and comments from `start`; return true iff the next significant byte is a
/// closing `]` or `}` (i.e. the comma at `start-1` is a trailing comma).
fn next_significant_is_close(bytes: &[u8], mut i: usize) -> bool {
    while let Some(&b) = bytes.get(i) {
        let next = bytes.get(i + 1).copied();
        match b {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'/' if next == Some(b'/') => {
                i += 2;
                while let Some(&c) = bytes.get(i) {
                    if c == b'\n' {
                        break;
                    }
                    i += 1;
                }
            }
            b'/' if next == Some(b'*') => {
                i += 2;
                while let Some(&c) = bytes.get(i) {
                    if c == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        break;
                    }
                    i += 1;
                }
                i = i.saturating_add(2).min(bytes.len());
            }
            b']' | b'}' => return true,
            _ => return false,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn parses_plain_json() {
        let v = parse(r#"{"bash": {"git *": "allow"}}"#).unwrap();
        assert_eq!(v["bash"]["git *"], serde_json::json!("allow"));
    }

    #[test]
    fn strips_line_and_block_comments_and_trailing_commas() {
        let src = r#"{
            // a line comment
            "bash": {
                "git *": "allow", /* inline */
                "rm -rf /": "deny",
            },
        }"#;
        let v = parse(src).unwrap();
        assert_eq!(v["bash"]["git *"], serde_json::json!("allow"));
        assert_eq!(v["bash"]["rm -rf /"], serde_json::json!("deny"));
    }

    #[test]
    fn does_not_treat_slashes_or_commas_inside_strings_as_syntax() {
        let src = r#"{"bash": {"echo //not-a-comment, still one key": "allow"}}"#;
        let v = parse(src).unwrap();
        assert_eq!(
            v["bash"]["echo //not-a-comment, still one key"],
            serde_json::json!("allow")
        );
    }

    #[test]
    fn malformed_is_err() {
        assert!(parse("{not json").is_err());
    }

    // Regression test for the missing pi `parseJsoncConfig` error format
    // (jsonc-config.ts:26-35): pre-fix, `parse`/`parse_ordered` returned the bare
    // `serde_json` error string with no subject or file path, so a malformed permission
    // config produced a warning indistinguishable from any other error and gave the user
    // no path to look at. `parse_config`/`parse_ordered_config` must reproduce pi's
    // `Failed to parse {subject} at '{file_path}' (...)` wrapper.
    #[test]
    fn parse_config_formats_error_like_pi_parse_jsonc_config() {
        let err = parse_config(
            "{not json",
            "/tmp/pi-permissions.jsonc",
            "permission config",
        )
        .expect_err("malformed JSONC must fail to parse");
        assert!(
            err.starts_with("Failed to parse permission config at '/tmp/pi-permissions.jsonc' ("),
            "unexpected error format: {err}"
        );
        assert!(err.ends_with(')'), "unexpected error format: {err}");
    }

    #[test]
    fn parse_ordered_config_formats_error_like_pi_parse_jsonc_config() {
        let err = parse_ordered_config("{not json", "/tmp/config.json", "permission-system config")
            .expect_err("malformed JSONC must fail to parse");
        assert!(
            err.starts_with("Failed to parse permission-system config at '/tmp/config.json' ("),
            "unexpected error format: {err}"
        );
    }
}
