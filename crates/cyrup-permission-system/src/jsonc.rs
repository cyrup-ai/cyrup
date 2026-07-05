//! JSONC → `serde_json::Value` (port of pi `jsonc-config.ts`, which uses `jsonc-parser` with
//! `allowTrailingComma: true`). pi's `jsonc-parser` accepts standard JSON plus `//` line comments,
//! `/* */` block comments, and trailing commas — and NOTHING else (it is not full JSON5: no
//! unquoted keys, no single-quoted strings). So the faithful port is a string-aware preprocessor
//! that strips exactly those three extensions and hands the result to `serde_json` — MORE faithful
//! than a JSON5 crate would be. On parse failure the caller (pi `loadGlobalConfig`,
//! `permission-manager.ts:670-681`) falls back to the `ask` policy with a warning; this module only
//! surfaces the error string.

use serde_json::Value;

use crate::ordered::OrderedValue;

/// Parse a JSONC document into a `Value` (pi `parseJsoncConfig`). Comments and one trailing comma
/// per array/object are stripped first; the remainder must be valid JSON. Order is NOT preserved —
/// use [`parse_ordered`] for policy files whose pattern-key order is load-bearing.
pub fn parse(input: &str) -> Result<Value, String> {
    let cleaned = strip_comments_and_trailing_commas(input);
    serde_json::from_str(&cleaned).map_err(|e| e.to_string())
}

/// Parse a JSONC document into an order-preserving [`OrderedValue`] (the policy-file path: pattern
/// key order within a category drives last-match-wins).
pub fn parse_ordered(input: &str) -> Result<OrderedValue, String> {
    let cleaned = strip_comments_and_trailing_commas(input);
    serde_json::from_str(&cleaned).map_err(|e| e.to_string())
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
}
