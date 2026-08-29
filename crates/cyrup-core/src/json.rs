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
//!
//! ## Streaming callers
//!
//! PERF-001. A decoder accumulating `partial_json` across deltas used to call
//! [`parse_streaming_json_object`] on the WHOLE buffer once per delta, which is quadratic in the
//! delta count. **The streaming decoders no longer call anything here directly**: a tool block
//! carries a [`SharedStr`](crate::SharedStr) of the raw buffer and hands each snapshot a
//! [`LazyArgs`](crate::LazyArgs), which calls [`parse_streaming_json_object`] exactly once and
//! only if something actually reads the arguments. Most snapshots are never read, so most buffers
//! are never parsed at all.
//!
//! The incremental pieces below remain available for a caller that must know the running state of
//! a buffer without re-scanning it, and their semantics are unchanged:
//!
//! * [`JsonShape`] tracks structural completeness in O(bytes appended) — feed it only each new
//!   chunk, never the accumulated buffer.
//! * [`StreamingArgs`] additionally projects the recovered arguments from that running state.
//! * [`parse_streaming_json_object_incomplete`] is [`parse_streaming_json_object`] with pass 1
//!   ([`parse_json_with_repair`]) skipped. That pass CANNOT succeed on a structurally incomplete
//!   document — it is two failed `serde_json` parses plus a full [`repair_json`] materialisation —
//!   so skipping it is free, and passes 2/3 are byte-for-byte the same.
//!
//! Such a caller must gate on [`JsonShape::is_complete`] and use the full
//! [`parse_streaming_json_object`] whenever it reports `true`: the tolerant parser is not required
//! to agree with `serde_json` on a COMPLETE document (duplicate keys, numeric edge cases), so the
//! terminal parse must always take the strict path. [`LazyArgs`](crate::LazyArgs) gets this for
//! free by always calling [`parse_streaming_json_object`], whose own pass 1 IS that strict path.

use serde_json::{Map, Value};
use std::borrow::Cow;

const VALID_JSON_ESCAPES: &[char] = &['"', '\\', '/', 'b', 'f', 'n', 'r', 't', 'u'];

fn is_control_character(c: char) -> bool {
    (c as u32) <= 0x1f
}

/// The character at byte offset `at`, or `None` past the end / off a character boundary.
fn char_at(s: &str, at: usize) -> Option<char> {
    s.get(at..).and_then(|rest| rest.chars().next())
}

/// Decode the four characters starting at byte offset `at` as a `\uXXXX` hex run, mirroring Pi's
/// `/^[0-9a-fA-F]{4}$/` test (`json-parse.ts:62` @v0.83.0). `None` when fewer than four bytes
/// remain or any of them is not a hex digit.
///
/// Byte-indexed rather than char-indexed (PERF-001): every caller reaches it immediately after an
/// ASCII `\u`, and a hex digit is ASCII by definition, so a byte offset and a character offset
/// coincide over the run this reads. Allocation-free — it used to `collect()` a four-char `String`.
fn hex4_at(s: &str, at: usize) -> Option<u32> {
    let digits = s.as_bytes().get(at..at.checked_add(4)?)?;
    if !digits.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let text = std::str::from_utf8(digits).ok()?;
    u32::from_str_radix(text, 16).ok()
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

/// Keep `json[at..at+len]` verbatim. A no-op while nothing has been rewritten yet — the untouched
/// prefix is still represented by the source string itself (see [`repair_json`]).
fn keep(out: &mut Option<String>, json: &str, at: usize, len: usize) {
    if let Some(buf) = out.as_mut()
        && let Some(end) = at.checked_add(len)
        && let Some(slice) = json.get(at..end)
    {
        buf.push_str(slice);
    }
}

/// Replace the source run starting at byte `at` with `replacement`, materialising the output buffer
/// (seeded with the verbatim prefix `json[..at]`) on the first call.
fn rewrite(out: &mut Option<String>, json: &str, at: usize, replacement: &str) {
    let buf = match out {
        Some(buf) => buf,
        none => {
            let mut seeded = String::with_capacity(json.len().saturating_add(16));
            if let Some(prefix) = json.get(..at) {
                seeded.push_str(prefix);
            }
            none.insert(seeded)
        }
    };
    buf.push_str(replacement);
}

/// Repairs malformed JSON string literals (Pi `repairJson`, json-parse.ts:32-83):
/// escapes raw control characters inside strings, and doubles backslashes before invalid escapes.
///
/// Returns [`Cow::Borrowed`] when no arm rewrote anything — the overwhelmingly common case, and the
/// only thing [`parse_json_with_repair`] actually needs to know (PERF-001). It used to build a full
/// `String` unconditionally and hand the caller a `repaired != json` memcmp to decide the same
/// question; both the build and the compare are now gone from the unchanged path.
pub fn repair_json(json: &str) -> Cow<'_, str> {
    let mut out: Option<String> = None;
    let mut in_string = false;
    let mut index = 0usize;

    while let Some(c) = char_at(json, index) {
        let clen = c.len_utf8();

        if !in_string {
            keep(&mut out, json, index, clen);
            if c == '"' {
                in_string = true;
            }
            index += clen;
            continue;
        }

        if c == '"' {
            keep(&mut out, json, index, clen);
            in_string = false;
            index += clen;
            continue;
        }

        if c == '\\' {
            let next_char = char_at(json, index + 1);
            match next_char {
                None => {
                    rewrite(&mut out, json, index, "\\\\");
                    index += 1;
                    continue;
                }
                Some('u') => {
                    if let Some(code) = hex4_at(json, index + 2) {
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
                            let low = (json.as_bytes().get(index + 6) == Some(&b'\\')
                                && json.as_bytes().get(index + 7) == Some(&b'u'))
                            .then(|| hex4_at(json, index + 8))
                            .flatten()
                            .filter(|lo| (0xDC00..=0xDFFF).contains(lo));
                            if low.is_some() {
                                // The full `\uXXXX\uXXXX` run, verbatim: 12 ASCII characters, so
                                // 12 bytes.
                                keep(&mut out, json, index, 12);
                                index += 12;
                            } else {
                                // Unpaired high surrogate: drop the escape entirely.
                                rewrite(&mut out, json, index, "");
                                index += 6;
                            }
                        } else if (0xDC00..=0xDFFF).contains(&code) {
                            // Unpaired low surrogate (a paired one was consumed above): drop it.
                            rewrite(&mut out, json, index, "");
                            index += 6;
                        } else {
                            // `\uXXXX` re-emitted exactly as it was read: 6 ASCII characters.
                            keep(&mut out, json, index, 6);
                            index += 6;
                        }
                        continue;
                    }
                    // Invalid hex run. Pi does NOT reach the doubling branch here: `u` is a member
                    // of `VALID_JSON_ESCAPES` (`json-parse.ts:3`), so control falls to `:69-73`
                    // and `\u` is emitted unchanged with 2 characters consumed (PROV-049).
                    keep(&mut out, json, index, 2);
                    index += 2;
                    continue;
                }
                Some(nc) if VALID_JSON_ESCAPES.contains(&nc) => {
                    // `\` + a valid escape character, both ASCII, emitted verbatim.
                    keep(&mut out, json, index, 1 + nc.len_utf8());
                    index += 1 + nc.len_utf8();
                    continue;
                }
                _ => {}
            }
            // Invalid escape: double the backslash, keep the next char for the following iteration.
            rewrite(&mut out, json, index, "\\\\");
            index += 1;
            continue;
        }

        if is_control_character(c) {
            rewrite(&mut out, json, index, &escape_control_character(c));
        } else {
            keep(&mut out, json, index, clen);
        }
        index += clen;
    }

    match out {
        Some(repaired) => Cow::Owned(repaired),
        None => Cow::Borrowed(json),
    }
}

/// `JSON.parse` with a repair fallback (Pi `parseJsonWithRepair`, json-parse.ts:85-95).
pub fn parse_json_with_repair(json: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(json) {
        return Some(v);
    }
    // `Cow::Owned` iff some arm rewrote something, which is exactly the `repaired != json` test
    // this used to spell as a full string compare.
    match repair_json(json) {
        Cow::Owned(repaired) => serde_json::from_str::<Value>(&repaired).ok(),
        Cow::Borrowed(_) => None,
    }
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

/// [`parse_streaming_json_object`] for a caller that already knows the buffer is structurally
/// INCOMPLETE — i.e. [`JsonShape::is_complete`] reported `false` (PERF-001).
///
/// Pass 1 ([`parse_json_with_repair`]) is skipped because it cannot succeed on such a buffer:
/// `serde_json` rejects an unclosed container or an unterminated string, and [`repair_json`] never
/// adds or removes a structural character — it only rewrites *inside* string literals — so the
/// repaired retry cannot close anything either. Skipping it removes two failed whole-buffer
/// `serde_json` parses and a whole-buffer [`repair_json`] materialisation per delta.
///
/// Passes 2 and 3 are byte-for-byte those of [`parse_streaming_json`], which is what makes the
/// recovered value identical to the whole-buffer call. The empty/blank guard matches too: like
/// [`parse_streaming_json`], blankness is tested on a trimmed view while the parse itself sees the
/// UNTRIMMED buffer.
pub fn parse_streaming_json_object_incomplete(partial_json: &str) -> Map<String, Value> {
    if partial_json.trim().is_empty() {
        return Map::new();
    }
    let recovered = parse_partial(partial_json)
        .or_else(|| parse_partial(&repair_json(partial_json)))
        .unwrap_or(Value::Null);
    match recovered {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// Running structural state over a tool call's accumulated `partial_json` (PERF-001).
///
/// Fed only the bytes appended by each delta — never the accumulated buffer — so maintaining it
/// costs O(chunk), not O(buffer). [`Self::is_complete`] answers exactly one question: "could a
/// strict `serde_json` parse succeed here?", which is what decides whether a caller may take the
/// cheap [`parse_streaming_json_object_incomplete`] path.
///
/// A buffer that never opened a container is reported INCOMPLETE. That is deliberate and it is not
/// a lie: such a buffer decodes to a non-object scalar, and both parse paths map a non-object to an
/// empty map, so the two agree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JsonShape {
    depth: u32,
    opened: bool,
    in_string: bool,
    escaped: bool,
}

impl JsonShape {
    /// Advance the state across the bytes JUST appended. Feeding the accumulated buffer instead of
    /// the new chunk reintroduces the quadratic this type exists to remove.
    pub fn feed(&mut self, chunk: &str) {
        for c in chunk.chars() {
            match (self.in_string, self.escaped, c) {
                (true, true, _) => self.escaped = false,
                (true, false, '\\') => self.escaped = true,
                (true, false, '"') => self.in_string = false,
                (true, false, _) => {}
                (false, _, '"') => self.in_string = true,
                (false, _, '{' | '[') => {
                    self.depth = self.depth.saturating_add(1);
                    self.opened = true;
                }
                (false, _, '}' | ']') => self.depth = self.depth.saturating_sub(1),
                (false, _, _) => {}
            }
        }
    }

    /// `true` when every container opened has been closed and the scan is not inside a string
    /// literal — the precondition for a strict parse to have any chance.
    pub fn is_complete(&self) -> bool {
        self.opened && self.depth == 0 && !self.in_string
    }

    /// The tool-call `arguments` for `buffer`, taking the cheap path while the buffer is still
    /// incomplete and the strict path the moment it is not. Callers should prefer this to choosing
    /// between the two parse entry points by hand.
    pub fn parse_object(&self, buffer: &str) -> Map<String, Value> {
        if self.is_complete() {
            parse_streaming_json_object(Some(buffer))
        } else {
            parse_streaming_json_object_incomplete(buffer)
        }
    }
}

// --- incremental tool-argument parse (PERF-001) -----------------------------------------

/// In-progress escape state inside a streamed string literal.
///
/// Every variant is a point at which [`PartialParser::parse_string`] would still be mid-escape, and
/// each contributes NOTHING to the decoded value until it resolves — which is exactly what a
/// whole-buffer `parse_string` does when the buffer ends mid-escape.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Esc {
    /// Not in an escape.
    None,
    /// Consumed `\`.
    Backslash,
    /// Consumed `\u`, collecting up to four hex digits.
    Uni(String),
    /// Completed a high surrogate; awaiting the `\` of a possible low half.
    HighSur(u32),
    /// High surrogate + `\`; awaiting `u`.
    HighSurBs(u32),
    /// High surrogate + `\u`, collecting the low half's hex digits.
    HighSurU(u32, String),
}

/// A streamed string literal: the characters decoded so far plus any unresolved escape.
#[derive(Clone, Debug, PartialEq, Eq)]
struct StrState {
    decoded: String,
    esc: Esc,
}

impl StrState {
    fn new() -> Self {
        Self {
            decoded: String::new(),
            esc: Esc::None,
        }
    }
}

/// What the incremental scanner is in the middle of. Each variant names what the equivalent
/// whole-buffer `parse_object` loop would be waiting for at the same byte.
#[derive(Clone, Debug, PartialEq, Eq)]
enum St {
    /// The object loop top: whitespace, then `}` | `,` | `"`(key).
    LoopTop,
    /// Reading a key.
    Key(StrState),
    /// Key read; expecting whitespace then `:`.
    Colon(String),
    /// `:` read; expecting whitespace then the start of a value.
    ValueStart(String),
    /// Reading a string value for the held key.
    StrVal(String, StrState),
    /// Reading a numeric value; the raw run is accumulated verbatim.
    NumVal(String, String),
    /// Reading a `true`/`false`/`null` value; at most five characters are needed to decide.
    LitVal(String, String),
    /// `}` closed the object.
    Closed,
    /// Malformed: the whole-buffer parser would `break` out of its loop here no matter what
    /// follows, so nothing after this point can change the result.
    Stopped,
}

/// Incremental equivalent of `parse_partial` for the shape tool-call arguments actually take: a
/// top-level object whose values are strings, numbers, booleans or nulls (PERF-001).
///
/// This exists because re-parsing the accumulated buffer on every delta is quadratic, and the
/// dominant term is re-decoding one huge string value — a `write` tool call is a small object
/// wrapping the entire file content. Feeding only each delta's own bytes makes the SCAN O(1)
/// amortised. (Projecting the result still copies the decoded payload, which is a property of
/// `Map<String, Value>` being owned, not of this scanner.)
///
/// **No streaming decoder uses it any more**: [`LazyArgs`](crate::LazyArgs) removed the per-delta
/// parse outright rather than making it cheap, so there is nothing left to advance incrementally.
/// It remains for a caller that genuinely needs the running state of a buffer without re-scanning
/// it, and its equivalence to the whole-buffer parse — the property `LazyArgs` relies on — is
/// pinned by `LazyArgs`'s own tests across a matrix of buffers and chunkings.
///
/// **Anything it does not fully understand sets `bail`, and the caller falls back to the
/// whole-buffer parse from then on.** Nested objects and arrays bail, as does any malformed run
/// this scanner is not certain it reproduces. That fallback is what bounds the correctness risk:
/// a bailed buffer takes exactly the path it takes today.
#[derive(Clone, Debug)]
pub struct StreamingArgs {
    shape: JsonShape,
    /// Members whose value is fully parsed and closed.
    map: Map<String, Value>,
    st: St,
    /// Set once the fast path sees something outside the shape it claims; the caller must then use
    /// the whole-buffer parse.
    bail: bool,
    /// Whether the leading `{` has been consumed. Until it is, anything but whitespace bails.
    opened: bool,
}

impl Default for StreamingArgs {
    fn default() -> Self {
        Self {
            shape: JsonShape::default(),
            map: Map::new(),
            st: St::LoopTop,
            bail: false,
            opened: false,
        }
    }
}

impl StreamingArgs {
    /// Advance across the bytes JUST appended to the tool call's buffer.
    pub fn feed(&mut self, chunk: &str) {
        self.shape.feed(chunk);
        if self.bail {
            return;
        }
        for c in chunk.chars() {
            if self.bail {
                return;
            }
            self.step(c);
        }
    }

    /// `true` when the accumulated buffer is structurally complete (see [`JsonShape`]).
    pub fn is_complete(&self) -> bool {
        self.shape.is_complete()
    }

    /// The tool-call `arguments` for the buffer fed so far.
    ///
    /// A COMPLETE buffer always takes the strict whole-buffer path: the tolerant parser is not
    /// required to agree with `serde_json` on a finished document, so the settled value must come
    /// from the same code the terminal `toolcall_end` uses. An incomplete buffer takes the
    /// incremental projection, or the whole-buffer tolerant parse if this scanner bailed.
    pub fn object(&self, buffer: &str) -> Map<String, Value> {
        if self.shape.is_complete() {
            return parse_streaming_json_object(Some(buffer));
        }
        if self.bail {
            return parse_streaming_json_object_incomplete(buffer);
        }
        self.project()
    }

    /// The map the whole-buffer parser would return if the input ended right here.
    fn project(&self) -> Map<String, Value> {
        if !self.opened {
            return Map::new();
        }
        let mut out = self.map.clone();
        match &self.st {
            // A key with no colon yet, or a colon with no value yet, is DROPPED — the whole-buffer
            // parser breaks out of its loop without inserting.
            St::LoopTop | St::Key(_) | St::Colon(_) | St::ValueStart(_) | St::Closed
            | St::Stopped => {}
            St::StrVal(k, s) => {
                out.insert(k.clone(), Value::String(s.decoded.clone()));
            }
            St::NumVal(k, raw) => {
                if let Some(v) = finish_number(raw) {
                    out.insert(k.clone(), v);
                }
            }
            St::LitVal(k, raw) => {
                if let Some(v) = literal_prefix_value(raw) {
                    out.insert(k.clone(), v);
                }
            }
        }
        out
    }

    fn step(&mut self, c: char) {
        if !self.opened {
            if c.is_whitespace() {
                return;
            }
            if c == '{' {
                self.opened = true;
                self.st = St::LoopTop;
            } else {
                // Not a top-level object: arrays and scalars both project to an empty map, but
                // proving that per shape is not worth it — hand it back to the whole-buffer parse.
                self.bail = true;
            }
            return;
        }

        match std::mem::replace(&mut self.st, St::Stopped) {
            St::Closed => {
                // Trailing bytes after the object closed cannot change the recovered value.
                self.st = St::Closed;
            }
            St::Stopped => self.st = St::Stopped,

            St::LoopTop => {
                if c.is_whitespace() {
                    self.st = St::LoopTop;
                } else if c == '}' {
                    self.st = St::Closed;
                } else if c == ',' {
                    self.st = St::LoopTop;
                } else if c == '"' {
                    self.st = St::Key(StrState::new());
                } else {
                    self.st = St::Stopped;
                }
            }

            St::Key(mut s) => match feed_string(&mut s, c) {
                StrStep::Open => self.st = St::Key(s),
                StrStep::Closed => self.st = St::Colon(s.decoded),
                StrStep::Reprocess(pending) => {
                    self.st = St::Key(s);
                    self.replay(pending);
                }
            },

            St::Colon(k) => {
                if c.is_whitespace() {
                    self.st = St::Colon(k);
                } else if c == ':' {
                    self.st = St::ValueStart(k);
                } else {
                    self.st = St::Stopped;
                }
            }

            St::ValueStart(k) => {
                if c.is_whitespace() {
                    self.st = St::ValueStart(k);
                } else if c == '"' {
                    self.st = St::StrVal(k, StrState::new());
                } else if c == '-' || c.is_ascii_digit() {
                    self.st = St::NumVal(k, c.to_string());
                } else if c == 't' || c == 'f' || c == 'n' {
                    self.st = St::LitVal(k, c.to_string());
                    self.settle_literal();
                } else {
                    // `{`, `[`, or malformed. Nesting is out of this scanner's shape.
                    self.bail = true;
                }
            }

            St::StrVal(k, mut s) => match feed_string(&mut s, c) {
                StrStep::Open => self.st = St::StrVal(k, s),
                StrStep::Closed => {
                    self.map.insert(k, Value::String(s.decoded));
                    self.st = St::LoopTop;
                }
                StrStep::Reprocess(pending) => {
                    self.st = St::StrVal(k, s);
                    self.replay(pending);
                }
            },

            St::NumVal(k, mut raw) => {
                if c.is_ascii_digit() || c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-' {
                    raw.push(c);
                    self.st = St::NumVal(k, raw);
                } else {
                    match finish_number(&raw) {
                        Some(v) => {
                            self.map.insert(k, v);
                            self.st = St::LoopTop;
                            self.step(c); // the terminating character belongs to the loop top
                        }
                        // `parse_value` returned `None`, so the whole-buffer parser breaks and
                        // drops the dangling key.
                        None => self.st = St::Stopped,
                    }
                }
            }

            St::LitVal(k, mut raw) => {
                raw.push(c);
                self.st = St::LitVal(k, raw);
                self.settle_literal();
            }
        }
    }

    /// Resolve a literal once enough characters are present to decide.
    ///
    /// The whole-buffer `parse_bool_partial`/`parse_null_partial` test the ENTIRE remainder against
    /// the word, so a strict prefix is only accepted when the buffer ends there. Five characters is
    /// enough to tell a completed `false` from a prefix of one.
    fn settle_literal(&mut self) {
        let St::LitVal(k, raw) = &self.st else {
            return;
        };
        for (word, value) in [
            ("true", Value::Bool(true)),
            ("false", Value::Bool(false)),
            ("null", Value::Null),
        ] {
            if let Some(tail) = raw.strip_prefix(word) {
                let (k, tail) = (k.clone(), tail.to_string());
                self.map.insert(k, value);
                self.st = St::LoopTop;
                self.replay(tail);
                return;
            }
        }
        // Still a viable prefix? Keep accumulating; otherwise the whole-buffer parser would have
        // returned `None` and broken out, dropping the key.
        let viable = ["true", "false", "null"]
            .iter()
            .any(|w| w.starts_with(raw.as_str()));
        if !viable {
            self.st = St::Stopped;
        }
    }

    /// Re-drive characters that the whole-buffer parser would NOT have consumed.
    fn replay(&mut self, pending: String) {
        for c in pending.chars() {
            if self.bail {
                return;
            }
            self.step(c);
        }
    }
}

/// Outcome of feeding one character into a streamed string literal.
enum StrStep {
    /// Still inside the literal.
    Open,
    /// The closing quote was consumed.
    Closed,
    /// The literal is still open, but these characters must be re-driven from the top: the
    /// whole-buffer parser would not have consumed them as part of the escape it abandoned.
    Reprocess(String),
}

/// Feed one character into a streamed string literal, mirroring `PartialParser::parse_string` and
/// `decode_unicode_escape` exactly — including which characters an abandoned escape leaves
/// unconsumed for the surrounding loop to re-read.
fn feed_string(s: &mut StrState, c: char) -> StrStep {
    match std::mem::replace(&mut s.esc, Esc::None) {
        Esc::None => {
            if c == '"' {
                return StrStep::Closed;
            }
            if c == '\\' {
                s.esc = Esc::Backslash;
                return StrStep::Open;
            }
            s.decoded.push(c);
            StrStep::Open
        }
        Esc::Backslash => {
            match c {
                '"' => s.decoded.push('"'),
                '\\' => s.decoded.push('\\'),
                '/' => s.decoded.push('/'),
                'b' => s.decoded.push('\u{8}'),
                'f' => s.decoded.push('\u{c}'),
                'n' => s.decoded.push('\n'),
                'r' => s.decoded.push('\r'),
                't' => s.decoded.push('\t'),
                'u' => s.esc = Esc::Uni(String::new()),
                other => s.decoded.push(other),
            }
            StrStep::Open
        }
        Esc::Uni(mut hex) => {
            if c.is_ascii_hexdigit() {
                hex.push(c);
                if hex.len() == 4 {
                    resolve_unicode(s, &hex);
                } else {
                    s.esc = Esc::Uni(hex);
                }
                StrStep::Open
            } else {
                // A malformed hex run: `decode_unicode_escape` consumes the digits that ARE there,
                // appends nothing, and returns — so this character is read by the string loop.
                StrStep::Reprocess(c.to_string())
            }
        }
        Esc::HighSur(code) => {
            if c == '\\' {
                s.esc = Esc::HighSurBs(code);
                StrStep::Open
            } else {
                // Unpaired high surrogate: dropped, and the lookahead consumed nothing.
                let _ = code;
                StrStep::Reprocess(c.to_string())
            }
        }
        Esc::HighSurBs(code) => {
            if c == 'u' {
                s.esc = Esc::HighSurU(code, String::new());
                StrStep::Open
            } else {
                // The lookahead needed `\u`; it found `\` then something else, so the whole-buffer
                // parser consumes NEITHER and the surrounding loop re-reads both.
                let _ = code;
                StrStep::Reprocess(format!("\\{c}"))
            }
        }
        Esc::HighSurU(code, mut hex) => {
            if c.is_ascii_hexdigit() {
                hex.push(c);
                if hex.len() < 4 {
                    s.esc = Esc::HighSurU(code, hex);
                    return StrStep::Open;
                }
                let low = u32::from_str_radix(&hex, 16).unwrap_or(0);
                if (0xDC00..=0xDFFF).contains(&low) {
                    let combined = 0x1_0000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                    if let Some(ch) = char::from_u32(combined) {
                        s.decoded.push(ch);
                    }
                    return StrStep::Open;
                }
                // Not a low half: the high surrogate is dropped and its lookahead consumed
                // nothing, so this `\uXXXX` is re-read as an escape in its own right.
                resolve_unicode(s, &hex);
                StrStep::Open
            } else {
                // The low half's hex run is malformed: the high surrogate is dropped, the partial
                // run appends nothing, and this character is re-read.
                StrStep::Reprocess(c.to_string())
            }
        }
    }
}

/// Resolve a completed four-digit `\uXXXX` run, matching `decode_unicode_escape`'s surrogate rules.
fn resolve_unicode(s: &mut StrState, hex: &str) {
    let Ok(code) = u32::from_str_radix(hex, 16) else {
        return;
    };
    if (0xD800..=0xDBFF).contains(&code) {
        s.esc = Esc::HighSur(code);
        return;
    }
    if (0xDC00..=0xDFFF).contains(&code) {
        // Unpaired low surrogate: dropped.
        return;
    }
    if let Some(ch) = char::from_u32(code) {
        s.decoded.push(ch);
    }
}

/// `parse_number_partial`'s finish: the raw run, then the same run with a trailing partial
/// exponent/sign/dot trimmed.
fn finish_number(raw: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(raw) {
        return Some(v);
    }
    let trimmed = raw.trim_end_matches(['e', 'E', '+', '-', '.']);
    if trimmed.is_empty() {
        return None;
    }
    serde_json::from_str::<Value>(trimmed).ok()
}

/// The value a still-incomplete `true`/`false`/`null` prefix stands for.
fn literal_prefix_value(raw: &str) -> Option<Value> {
    if raw.is_empty() {
        return None;
    }
    for (word, value) in [
        ("true", Value::Bool(true)),
        ("false", Value::Bool(false)),
        ("null", Value::Null),
    ] {
        if word.starts_with(raw) {
            return Some(value);
        }
    }
    None
}

// --- tolerant partial parser (equivalent of the `partial-json` package) -------------------------

/// Best-effort parse that recovers as much of an incomplete JSON document as possible. Returns
/// `None` only when nothing parseable is present at all.
fn parse_partial(input: &str) -> Option<Value> {
    let mut p = PartialParser {
        src: input,
        pos: 0,
    };
    p.skip_ws();
    let value = p.parse_value()?;
    Some(value)
}

/// A cursor over the source `&str` at a BYTE offset (PERF-001). It was a `&[char]` cursor, which
/// forced the caller to materialise a `Vec<char>` — four bytes per character of the whole buffer,
/// rebuilt on every delta. Nothing here needs random access by character index; every position this
/// scanner computes is either the current character or a fixed ASCII lookahead from it.
struct PartialParser<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> PartialParser<'a> {
    fn peek(&self) -> Option<char> {
        char_at(self.src, self.pos)
    }

    /// Advance past `c`, which must be the character [`Self::peek`] just returned.
    fn bump(&mut self, c: char) {
        self.pos += c.len_utf8();
    }

    /// The unconsumed remainder. Tied to `'a` rather than to `&self` so a caller can keep it across
    /// a `self.pos` write.
    fn rest(&self) -> &'a str {
        self.src.get(self.pos..).unwrap_or("")
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if !c.is_whitespace() {
                break;
            }
            self.bump(c);
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
            self.bump(c);
            match c {
                '"' => return Some(out), // closed
                '\\' => {
                    let Some(esc) = self.peek() else {
                        return Some(out);
                    };
                    self.bump(esc);
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
        let Some(code) = hex4_at(self.src, self.pos) else {
            // Malformed or truncated hex run: consume the digits that ARE present so they do not
            // leak into the decoded string as literals.
            while let Some(c) = self.peek() {
                if !c.is_ascii_hexdigit() {
                    break;
                }
                self.pos += 1; // a hex digit is ASCII: one byte
            }
            return;
        };
        self.pos += 4; // four ASCII hex digits

        if (0xD800..=0xDBFF).contains(&code) {
            let low = (self.peek() == Some('\\')
                && self.src.as_bytes().get(self.pos + 1) == Some(&b'u'))
            .then(|| hex4_at(self.src, self.pos + 2))
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
        while let Some(c) = self.peek() {
            if !(c.is_ascii_digit() || c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-') {
                break;
            }
            self.pos += 1; // every accepted character is ASCII
        }
        // Borrowed straight out of the source — this used to `collect()` a fresh `String`.
        let raw = self.src.get(start..self.pos).unwrap_or("");
        // Try the full slice, then progressively trim a trailing partial exponent/sign/dot.
        if let Ok(v) = serde_json::from_str::<Value>(raw) {
            return Some(v);
        }
        let trimmed = raw.trim_end_matches(['e', 'E', '+', '-', '.']);
        if let (false, Ok(v)) = (trimmed.is_empty(), serde_json::from_str::<Value>(trimmed)) {
            return Some(v);
        }
        None
    }

    /// PERF-001: `rest` is a borrowed view, not a `collect()`ed `String`. It used to materialise the
    /// ENTIRE remaining buffer on every call — and since `parse_partial` re-scans the whole buffer
    /// on every delta, a single `true`/`false` anywhere in a tool call's arguments cost one
    /// full-tail allocation per delta, not one per literal.
    fn parse_bool_partial(&mut self) -> Option<Value> {
        let rest = self.rest();
        for (word, value) in [("true", true), ("false", false)] {
            if rest.starts_with(word) {
                self.pos += word.len();
                return Some(Value::Bool(value));
            }
            // Partial literal (e.g. "tru"): accept as the intended value.
            if word.starts_with(rest) && !rest.is_empty() {
                self.pos = self.src.len();
                return Some(Value::Bool(value));
            }
        }
        None
    }

    /// See [`Self::parse_bool_partial`] for why `rest` is borrowed rather than collected.
    fn parse_null_partial(&mut self) -> Option<Value> {
        let rest = self.rest();
        if rest.starts_with("null") {
            self.pos += 4;
            return Some(Value::Null);
        }
        if "null".starts_with(rest) && !rest.is_empty() {
            self.pos = self.src.len();
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
