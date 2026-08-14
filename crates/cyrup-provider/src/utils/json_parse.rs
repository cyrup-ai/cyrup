//! Best-effort JSON parsing for streamed/truncated tool-call arguments (1:1 with Pi
//! `utils/json-parse.ts`).
//!
//! Two behaviours from Pi:
//! 1. [`repair_json`] — repairs malformed string literals (escapes raw control chars, doubles
//!    backslashes before invalid escapes). Faithful port of `json-parse.ts:32-83`.
//! 2. [`parse_streaming_json`] — always returns a value, recovering as much of an *incomplete* JSON
//!    document as possible (Pi delegates this to the `partial-json` npm package; this module
//!    implements an equivalent tolerant recursive-descent parser). Faithful port of the control
//!    flow in `json-parse.ts:85-124`.
//!
//! The prior cyrup behaviour (`parse_partial_json` returning `{}` on any failure) discarded a
//! truncated tool call's arguments; [`parse_streaming_json_object`] recovers them.

use serde_json::{Map, Value};

const VALID_JSON_ESCAPES: &[char] = &['"', '\\', '/', 'b', 'f', 'n', 'r', 't', 'u'];

fn is_control_character(c: char) -> bool {
    (c as u32) <= 0x1f
}

/// Decode the four characters starting at `at` as a `\uXXXX` hex run, mirroring Pi's
/// `/^[0-9a-fA-F]{4}$/` test (`json-parse.ts:62` @v0.83.0). `None` when fewer than four characters
/// remain or any of them is not a hex digit.
fn hex4_at(chars: &[char], at: usize) -> Option<u32> {
    let digits: String = chars.get(at..at.checked_add(4)?)?.iter().collect();
    if !digits.chars().all(|d| d.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(&digits, 16).ok()
}

fn escape_control_character(c: char) -> String {
    match c {
        '\u{8}' => "\\b".to_string(),
        '\u{c}' => "\\f".to_string(),
        '\n' => "\\n".to_string(),
        '\r' => "\\r".to_string(),
        '\t' => "\\t".to_string(),
        other => format!("\\u{:04x}", other as u32),
    }
}

/// Repairs malformed JSON string literals (Pi `repairJson`, json-parse.ts:32-83):
/// escapes raw control characters inside strings, and doubles backslashes before invalid escapes.
pub fn repair_json(json: &str) -> String {
    let chars: Vec<char> = json.chars().collect();
    let mut repaired = String::new();
    let mut in_string = false;
    let mut index = 0;

    while let Some(&c) = chars.get(index) {
        if !in_string {
            repaired.push(c);
            if c == '"' {
                in_string = true;
            }
            index += 1;
            continue;
        }

        if c == '"' {
            repaired.push(c);
            in_string = false;
            index += 1;
            continue;
        }

        if c == '\\' {
            let next_char = chars.get(index + 1).copied();
            match next_char {
                None => {
                    repaired.push_str("\\\\");
                    index += 1;
                    continue;
                }
                Some('u') => {
                    if let Some(code) = hex4_at(&chars, index + 2) {
                        // Well-formed `\uXXXX`. Pi emits it verbatim (`json-parse.ts:63-65`)
                        // because `JSON.parse` accepts an unpaired surrogate escape and yields a
                        // JS string holding the lone code unit, which is then dropped on the way
                        // back out by `sanitizeSurrogates`
                        // (`packages/ai/src/utils/sanitize-unicode.ts:21-25` @v0.83.0).
                        //
                        // CYRUP-DELTA (pi `json-parse.ts:60-67` + `sanitize-unicode.ts:21-25`
                        // @v0.83.0): a Rust `String` is well-formed UTF-8 by type invariant and
                        // `serde_json` hard-errors on a lone surrogate escape, so passing it
                        // through verbatim would make `repaired == json` and fail the whole parse
                        // (PROV-048). We apply `sanitizeSurrogates` semantics here, at the escape
                        // level: a *paired* surrogate escape is emitted unchanged, an unpaired one
                        // is dropped — the same end state pi reaches one step later.
                        if (0xD800..=0xDBFF).contains(&code) {
                            let low = (chars.get(index + 6) == Some(&'\\')
                                && chars.get(index + 7) == Some(&'u'))
                            .then(|| hex4_at(&chars, index + 8))
                            .flatten()
                            .filter(|lo| (0xDC00..=0xDFFF).contains(lo));
                            if low.is_some() {
                                let pair: String =
                                    chars.iter().skip(index).take(12).collect();
                                repaired.push_str(&pair);
                                index += 12;
                            } else {
                                // Unpaired high surrogate: drop the escape entirely.
                                index += 6;
                            }
                        } else if (0xDC00..=0xDFFF).contains(&code) {
                            // Unpaired low surrogate (a paired one was consumed above): drop it.
                            index += 6;
                        } else {
                            let unicode_digits: String =
                                chars.iter().skip(index + 2).take(4).collect();
                            repaired.push_str(&format!("\\u{unicode_digits}"));
                            index += 6;
                        }
                        continue;
                    }
                    // Invalid hex run. Pi does NOT reach the doubling branch here: `u` is a member
                    // of `VALID_JSON_ESCAPES` (`json-parse.ts:3`), so control falls to `:69-73`
                    // and `\u` is emitted unchanged with 2 characters consumed (PROV-049).
                    repaired.push_str("\\u");
                    index += 2;
                    continue;
                }
                Some(nc) if VALID_JSON_ESCAPES.contains(&nc) => {
                    repaired.push('\\');
                    repaired.push(nc);
                    index += 2;
                    continue;
                }
                _ => {}
            }
            // Invalid escape: double the backslash, keep the next char for the following iteration.
            repaired.push_str("\\\\");
            index += 1;
            continue;
        }

        if is_control_character(c) {
            repaired.push_str(&escape_control_character(c));
        } else {
            repaired.push(c);
        }
        index += 1;
    }

    repaired
}

/// `JSON.parse` with a repair fallback (Pi `parseJsonWithRepair`, json-parse.ts:85-95).
pub fn parse_json_with_repair(json: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(json) {
        return Some(v);
    }
    let repaired = repair_json(json);
    if repaired != json {
        return serde_json::from_str::<Value>(&repaired).ok();
    }
    None
}

/// Parse potentially-incomplete streaming JSON, always returning a value (Pi `parseStreamingJson`,
/// json-parse.ts:104-124). Order: strict-parse-with-repair, then tolerant partial parse, then a
/// repaired tolerant partial parse, then `null` (Pi's final `{}` is represented by `Value::Null`
/// here; callers that need an object use [`parse_streaming_json_object`]).
pub fn parse_streaming_json(partial_json: Option<&str>) -> Value {
    let trimmed = match partial_json {
        Some(s) if !s.trim().is_empty() => s,
        _ => return Value::Null,
    };

    if let Some(v) = parse_json_with_repair(trimmed) {
        return v;
    }
    if let Some(v) = parse_partial(trimmed) {
        return v;
    }
    if let Some(v) = parse_partial(&repair_json(trimmed)) {
        return v;
    }
    Value::Null
}

/// Like [`parse_streaming_json`] but guarantees a JSON object (Pi's tool-call `arguments` are always
/// `Record<string, any>`). A recovered non-object (or nothing) yields an empty map.
pub fn parse_streaming_json_object(partial_json: Option<&str>) -> Map<String, Value> {
    match parse_streaming_json(partial_json) {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

// --- tolerant partial parser (equivalent of the `partial-json` package) -------------------------

/// Best-effort parse that recovers as much of an incomplete JSON document as possible. Returns
/// `None` only when nothing parseable is present at all.
fn parse_partial(input: &str) -> Option<Value> {
    let chars: Vec<char> = input.chars().collect();
    let mut p = PartialParser {
        chars: &chars,
        pos: 0,
    };
    p.skip_ws();
    let value = p.parse_value()?;
    Some(value)
}

struct PartialParser<'a> {
    chars: &'a [char],
    pos: usize,
}

impl PartialParser<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_whitespace()) {
            self.pos += 1;
        }
    }

    /// Parse any value at the current position. Returns `None` when nothing parseable starts here.
    fn parse_value(&mut self) -> Option<Value> {
        self.skip_ws();
        match self.peek()? {
            '{' => Some(self.parse_object()),
            '[' => Some(self.parse_array()),
            '"' => self.parse_string().map(Value::String),
            't' | 'f' => self.parse_bool_partial(),
            'n' => self.parse_null_partial(),
            c if c == '-' || c.is_ascii_digit() => self.parse_number_partial(),
            _ => None,
        }
    }

    fn parse_object(&mut self) -> Value {
        let mut map = Map::new();
        self.pos += 1; // consume '{'
        loop {
            self.skip_ws();
            match self.peek() {
                None => break, // truncated → return what we have
                Some('}') => {
                    self.pos += 1;
                    break;
                }
                Some(',') => {
                    self.pos += 1;
                    continue;
                }
                Some('"') => {}
                _ => break, // malformed
            }
            // Parse the key.
            let Some(key) = self.parse_string() else {
                break;
            };
            self.skip_ws();
            if self.peek() != Some(':') {
                // Key without a colon/value (truncated mid-key): drop it (Pi/partial-json behaviour).
                break;
            }
            self.pos += 1; // consume ':'
            self.skip_ws();
            match self.parse_value() {
                Some(value) => {
                    map.insert(key, value);
                }
                None => break, // truncated right after the colon: drop the dangling key
            }
        }
        Value::Object(map)
    }

    fn parse_array(&mut self) -> Value {
        let mut arr = Vec::new();
        self.pos += 1; // consume '['
        loop {
            self.skip_ws();
            match self.peek() {
                None => break,
                Some(']') => {
                    self.pos += 1;
                    break;
                }
                Some(',') => {
                    self.pos += 1;
                    continue;
                }
                _ => {}
            }
            match self.parse_value() {
                Some(value) => arr.push(value),
                None => break,
            }
        }
        Value::Array(arr)
    }

    /// Parse a string literal, tolerating an unterminated trailing string (returns what was read).
    fn parse_string(&mut self) -> Option<String> {
        if self.peek() != Some('"') {
            return None;
        }
        self.pos += 1; // consume opening quote
        let mut out = String::new();
        while let Some(c) = self.peek() {
            self.pos += 1;
            match c {
                '"' => return Some(out), // closed
                '\\' => {
                    let Some(esc) = self.peek() else {
                        return Some(out);
                    };
                    self.pos += 1;
                    match esc {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => self.decode_unicode_escape(&mut out),
                        other => out.push(other),
                    }
                }
                other => out.push(other),
            }
        }
        // Unterminated: best-effort return of what we accumulated.
        Some(out)
    }

    /// Decode a `\uXXXX` escape whose leading `\u` has already been consumed, appending the
    /// decoded character to `out` and advancing `self.pos` past everything consumed.
    ///
    /// Pi delegates this path to the `partial-json` npm package, which hands the completed document
    /// to `JSON.parse` (`json-parse.ts:113`,`:117` @v0.83.0); `JSON.parse` combines a surrogate
    /// PAIR into one astral character per the JSON spec, so pi's recovered arguments keep it.
    /// Combining here reproduces that (PROV-050). An UNPAIRED surrogate is dropped, matching
    /// `sanitizeSurrogates` (`sanitize-unicode.ts:21-25` @v0.83.0) and [`repair_json`]'s arm, so the
    /// two `\u` decoders in this module agree.
    fn decode_unicode_escape(&mut self, out: &mut String) {
        let Some(code) = hex4_at(self.chars, self.pos) else {
            // Malformed or truncated hex run: consume the digits that ARE present so they do not
            // leak into the decoded string as literals.
            while matches!(self.peek(), Some(c) if c.is_ascii_hexdigit()) {
                self.pos += 1;
            }
            return;
        };
        self.pos += 4;

        if (0xD800..=0xDBFF).contains(&code) {
            let low = (self.peek() == Some('\\') && self.chars.get(self.pos + 1) == Some(&'u'))
                .then(|| hex4_at(self.chars, self.pos + 2))
                .flatten()
                .filter(|lo| (0xDC00..=0xDFFF).contains(lo));
            if let Some(lo) = low {
                self.pos += 6;
                let combined = 0x1_0000 + ((code - 0xD800) << 10) + (lo - 0xDC00);
                if let Some(ch) = char::from_u32(combined) {
                    out.push(ch);
                }
            }
            // Unpaired high surrogate: dropped.
            return;
        }
        if (0xDC00..=0xDFFF).contains(&code) {
            // Unpaired low surrogate: dropped.
            return;
        }
        if let Some(ch) = char::from_u32(code) {
            out.push(ch);
        }
    }

    fn parse_number_partial(&mut self) -> Option<Value> {
        let start = self.pos;
        if self.peek() == Some('-') {
            self.pos += 1;
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_digit() || c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-')
        {
            self.pos += 1;
        }
        let raw: String = self
            .chars
            .get(start..self.pos)
            .unwrap_or(&[])
            .iter()
            .collect();
        // Try the full slice, then progressively trim a trailing partial exponent/sign/dot.
        if let Ok(v) = serde_json::from_str::<Value>(&raw) {
            return Some(v);
        }
        let trimmed = raw.trim_end_matches(['e', 'E', '+', '-', '.']);
        if let (false, Ok(v)) = (trimmed.is_empty(), serde_json::from_str::<Value>(trimmed)) {
            return Some(v);
        }
        None
    }

    fn parse_bool_partial(&mut self) -> Option<Value> {
        let rest: String = self.chars.iter().skip(self.pos).collect();
        for (word, value) in [("true", true), ("false", false)] {
            if rest.starts_with(word) {
                self.pos += word.len();
                return Some(Value::Bool(value));
            }
            // Partial literal (e.g. "tru"): accept as the intended value.
            if word.starts_with(&rest) && !rest.is_empty() {
                self.pos = self.chars.len();
                return Some(Value::Bool(value));
            }
        }
        None
    }

    fn parse_null_partial(&mut self) -> Option<Value> {
        let rest: String = self.chars.iter().skip(self.pos).collect();
        if rest.starts_with("null") {
            self.pos += 4;
            return Some(Value::Null);
        }
        if "null".starts_with(&rest) && !rest.is_empty() {
            self.pos = self.chars.len();
            return Some(Value::Null);
        }
        None
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn complete_json_parses_normally() {
        let v = parse_streaming_json(Some(r#"{"path":"a.txt","n":5}"#));
        assert_eq!(v["path"], "a.txt");
        assert_eq!(v["n"], 5);
    }

    #[test]
    fn empty_or_blank_returns_null() {
        assert_eq!(parse_streaming_json(None), Value::Null);
        assert_eq!(parse_streaming_json(Some("   ")), Value::Null);
        assert!(parse_streaming_json_object(Some("")).is_empty());
    }

    /// The core #28 fix: a truncated tool-call argument string recovers its partial value rather
    /// than collapsing to `{}`.
    #[test]
    fn recovers_truncated_string_value() {
        let obj = parse_streaming_json_object(Some(r#"{"path": "foo/ba"#));
        assert_eq!(obj.get("path").and_then(Value::as_str), Some("foo/ba"));
    }

    #[test]
    fn recovers_truncated_after_complete_pair() {
        let obj = parse_streaming_json_object(Some(r#"{"a": 1, "b": "hel"#));
        assert_eq!(obj.get("a").and_then(Value::as_i64), Some(1));
        assert_eq!(obj.get("b").and_then(Value::as_str), Some("hel"));
    }

    #[test]
    fn drops_dangling_key_without_value() {
        let obj = parse_streaming_json_object(Some(r#"{"a": 1, "b""#));
        assert_eq!(obj.get("a").and_then(Value::as_i64), Some(1));
        assert!(!obj.contains_key("b"));
    }

    #[test]
    fn recovers_truncated_array() {
        let v = parse_streaming_json(Some(r#"{"items": [1, 2, 3"#));
        assert_eq!(v["items"], serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn repair_escapes_raw_control_chars() {
        // A raw newline inside a string is invalid JSON; repair escapes it so it parses.
        let raw = "{\"k\": \"line1\nline2\"}";
        let repaired = repair_json(raw);
        let v: Value = serde_json::from_str(&repaired).unwrap();
        assert_eq!(v["k"], "line1\nline2");
    }

    #[test]
    fn repair_doubles_invalid_escape() {
        // `\x` is not a valid JSON escape; repair doubles the backslash.
        let raw = r#"{"k":"a\xb"}"#;
        let repaired = repair_json(raw);
        let v: Value = serde_json::from_str(&repaired).unwrap();
        assert_eq!(v["k"], "a\\xb");
    }

    #[test]
    fn parse_streaming_json_object_non_object_is_empty() {
        assert!(parse_streaming_json_object(Some("[1,2,3]")).is_empty());
        assert!(parse_streaming_json_object(Some("\"just a string\"")).is_empty());
    }

    #[test]
    fn recovers_partial_literal() {
        let v = parse_streaming_json(Some(r#"{"flag": tru"#));
        assert_eq!(v["flag"], Value::Bool(true));
    }

    /// PROV-048. A lone-surrogate escape in a provider SSE frame must not kill the parse.
    /// `serde_json` rejects `\ud83d` where `JSON.parse` accepts it, and the old `repair_json`
    /// re-emitted the escape verbatim, so `repaired == json` and `parse_json_with_repair` returned
    /// `None` — which both SSE decoders treat as fatal to the whole turn.
    #[test]
    fn lone_surrogate_escape_is_dropped_not_fatal() {
        let frame = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi \ud83d there"}}"#;
        let parsed = parse_json_with_repair(frame).expect("lone surrogate must not be fatal");
        assert_eq!(parsed["delta"]["text"], "hi  there");
    }

    /// PROV-048 regression guard: a *paired* surrogate escape is an ordinary astral character and
    /// must survive untouched (`sanitizeSurrogates` leaves pairs alone).
    #[test]
    fn paired_surrogate_escape_survives() {
        let parsed = parse_json_with_repair(r#"{"t":"\ud83d\ude00"}"#).expect("pair must parse");
        assert_eq!(parsed["t"], "😀");
        // A lone LOW surrogate is dropped too.
        let parsed = parse_json_with_repair(r#"{"t":"a\ude00b"}"#).expect("lone low must parse");
        assert_eq!(parsed["t"], "ab");
    }

    /// PROV-049. Pi's `VALID_JSON_ESCAPES` contains `"u"` (`json-parse.ts:3`), so an invalid hex
    /// run falls to `:69-73` and `\u` is emitted UNCHANGED — making the whole repair a no-op for a
    /// blob like an unescaped Windows path, so `parseJsonWithRepair` rethrows and pi falls through
    /// to the partial parser. cyrup used to double the backslash and successfully parse a
    /// *different* argument value.
    #[test]
    fn invalid_unicode_escape_repairs_to_a_no_op() {
        let raw = r#"{"p":"C:\users\bob"}"#;
        assert_eq!(repair_json(raw), raw, "repair must be a no-op, as pi's is");
        assert!(
            parse_json_with_repair(raw).is_none(),
            "a no-op repair must not be retried"
        );
        assert_eq!(repair_json(r#"{"k":"\uZZZZ"}"#), r#"{"k":"\uZZZZ"}"#);
    }

    /// PROV-050. `JSON.parse` (which `partial-json` hands the completed document to) combines a
    /// surrogate pair into one astral character; decoding the halves independently deleted it.
    #[test]
    fn partial_parser_keeps_astral_characters() {
        let obj = parse_streaming_json_object(Some(r#"{"msg":"hi \ud83d\ude00"#));
        assert_eq!(obj.get("msg").and_then(Value::as_str), Some("hi 😀"));
    }

    /// PROV-050, second arm: a malformed/truncated `\u` run must not leak its raw hex digits into
    /// the decoded value, and a genuinely unpaired surrogate is dropped (agreeing with PROV-048).
    #[test]
    fn partial_parser_does_not_leak_malformed_escapes() {
        let obj = parse_streaming_json_object(Some(r#"{"msg":"x \u12"#));
        assert_eq!(obj.get("msg").and_then(Value::as_str), Some("x "));
        let obj = parse_streaming_json_object(Some(r#"{"msg":"x \ud83d y"#));
        assert_eq!(obj.get("msg").and_then(Value::as_str), Some("x  y"));
    }
}
