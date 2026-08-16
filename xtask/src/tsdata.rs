//! A scanner for the *data literal* subset of TypeScript that pi's generated
//! `packages/ai/src/providers/<provider>.models.ts` modules are written in.
//!
//! ## Why this exists at all
//!
//! pi gitignores `packages/ai/src/providers/data/` (`pi/.gitignore:11`), and from
//! `a9f6a3159` (`feat(ai): separate generated model data (#6765)`) onward every `*.models.ts` is a
//! two-line re-export of a JSON file that is not in git. Nine gap-analysis sweeps recorded, as
//! settled fact, that catalog accuracy was therefore "not statically auditable" (PROV-004,
//! PARITY-GAPS `OQ-5`). That is FALSE at `a9f6a3159`'s parent `b0c2a90e` — the revision cyrup's own
//! `catalog_manifest.json` names as its provenance floor — where the same modules are still full
//! data literals. Recovering them needs nothing but `git show` and this file (PROV-060).
//!
//! ## The subset, stated exactly
//!
//! The modules are machine-written by pi's `ai/scripts/generate-models.ts`, so the grammar is far
//! smaller than TypeScript's. Verified across all 35 modules at `b0c2a90e` (19_382 lines): no
//! template literals, no spreads, no block comments, no functions, no escape sequences and no
//! non-ASCII in any string. What does occur:
//!
//! * `// …` line comments and an `import type { Model } from "../types.ts";` header,
//! * one `export const <NAME>_MODELS = { … } as const;` binding,
//! * object literals whose keys are either `"quoted"` or bare identifiers, with trailing commas,
//! * array literals, double-quoted strings, numbers, `true`/`false`/`null`,
//! * a `satisfies Model<"…">` type assertion after each model object.
//!
//! Anything outside that subset is a hard error rather than a silent skip — a generator that
//! quietly drops a construct it does not understand is exactly the failure mode this whole item
//! exists to correct.
//!
//! Numbers are carried as their **source text**, never through `f64`. All 351 distinct numerals at
//! `b0c2a90e` are already in JavaScript's canonical `String(Number)` form, so preserving the text
//! reproduces what pi's own `JSON.stringify` would emit while making a float round-trip impossible.

use std::fmt::Write as _;

/// An ordered JSON value. Object keys keep **declaration order**, which is what makes the emitted
/// catalogs diffable against the ones already in the tree (`serde_json::Map` would sort them, and
/// its `preserve_order` feature is not safe to enable workspace-wide — see `Cargo.toml`).
#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Str(String),
    /// The numeral exactly as it appears in the source.
    Num(String),
    Bool(bool),
    Null,
    Arr(Vec<Val>),
    Obj(Vec<(String, Val)>),
}

impl Val {
    /// Borrow the value at `key`, when this is an object that has one.
    pub fn get(&self, key: &str) -> Option<&Val> {
        match self {
            Val::Obj(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Borrow the string payload, when this is a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Val::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Replace the value at `key` **in place**, keeping its declaration position, and returning
    /// whether the key was there to replace. Position matters: the emitted catalogs are diffed
    /// against upstream's own key order, so a pinned value must not float to the end.
    pub fn set(&mut self, key: &str, value: Val) -> bool {
        match self {
            Val::Obj(entries) => match entries.iter_mut().find(|(k, _)| k == key) {
                Some(slot) => {
                    slot.1 = value;
                    true
                }
                None => false,
            },
            _ => false,
        }
    }

    /// Remove the entry at `key`, returning whether anything was removed.
    pub fn remove(&mut self, key: &str) -> bool {
        match self {
            Val::Obj(entries) => {
                let before = entries.len();
                entries.retain(|(k, _)| k != key);
                entries.len() != before
            }
            _ => false,
        }
    }

    /// Render as JSON with **tab** indentation and no trailing newline.
    ///
    /// Tabs are the form the largest embedded catalogs already use (`amazon-bedrock.json`,
    /// `vercel-ai-gateway.json`, `openai.json`, …) and the form pi's own sources use; the 21
    /// catalogs that were one-space-indented are normalised onto it by the first generated run, so
    /// that `gen-catalogs --check` has a single byte-exact target.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        self.write_json(&mut out, 0);
        out
    }

    fn write_json(&self, out: &mut String, depth: usize) {
        let pad = |out: &mut String, d: usize| {
            for _ in 0..d {
                out.push('\t');
            }
        };
        match self {
            Val::Str(s) => write_json_string(out, s),
            Val::Num(n) => out.push_str(n),
            Val::Bool(true) => out.push_str("true"),
            Val::Bool(false) => out.push_str("false"),
            Val::Null => out.push_str("null"),
            Val::Arr(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push_str("[\n");
                for (i, item) in items.iter().enumerate() {
                    pad(out, depth + 1);
                    item.write_json(out, depth + 1);
                    if i + 1 < items.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                pad(out, depth);
                out.push(']');
            }
            Val::Obj(entries) => {
                if entries.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push_str("{\n");
                for (i, (k, v)) in entries.iter().enumerate() {
                    pad(out, depth + 1);
                    write_json_string(out, k);
                    out.push_str(": ");
                    v.write_json(out, depth + 1);
                    if i + 1 < entries.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                pad(out, depth);
                out.push('}');
            }
        }
    }
}

/// `serde_json`-compatible string escaping (the catalogs contain no escapes today, but the emitter
/// must not be the thing that breaks the day one appears).
fn write_json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Parse one `*.models.ts` module and return its models in declaration order.
///
/// The returned vector is the value side of the `<NAME>_MODELS` record — pi's own
/// `Object.values(...)` view, which is the shape cyrup's `providers/catalog/*.json` files hold.
pub fn parse_models_module(src: &str) -> Result<Vec<Val>, String> {
    match parse_module_object(src)? {
        Val::Obj(entries) => Ok(entries.into_iter().map(|(_, v)| v).collect()),
        other => Err(format!("expected an object literal, got {other:?}")),
    }
}

/// Parse one generated module and return the whole `export const … = { … }` object.
///
/// `providers/<p>.models.ts` binds a flat `id -> Model` record; `image-models.generated.ts` binds a
/// `provider -> id -> ImagesModel` record (PROV-065), so that one needs the object rather than
/// [`parse_models_module`]'s flattened values.
pub fn parse_module_object(src: &str) -> Result<Val, String> {
    let start = src
        .find("export const")
        .ok_or_else(|| "no `export const` binding in module".to_string())?;
    let rest = src.get(start..).unwrap_or_default();
    let eq = rest
        .find('=')
        .ok_or_else(|| "`export const` binding has no `=`".to_string())?;
    let body = rest.get(eq + 1..).unwrap_or_default();

    let mut p = Parser {
        src: body.as_bytes(),
        pos: 0,
    };
    let value = p.value()?;
    p.skip_trivia_and_assertions();
    // Anything after the literal must be the `as const;` tail and nothing else.
    let tail = p.remaining().trim();
    let tail = tail.strip_prefix(';').unwrap_or(tail).trim();
    if !tail.is_empty() {
        return Err(format!("unexpected trailing source after the literal: {tail:?}"));
    }
    Ok(value)
}

/// Parse a whole JSON document into the same ordered [`Val`] tree.
///
/// JSON is a strict subset of the literal grammar this scanner already accepts, so the *same*
/// parser reads both sides — which is what lets `gen-catalogs --diff` compare an on-disk catalog
/// against a freshly extracted one field-by-field without a JSON dependency, and without one side
/// going through a different number representation from the other.
pub fn parse_json(src: &str) -> Result<Val, String> {
    let mut p = Parser {
        src: src.as_bytes(),
        pos: 0,
    };
    let value = p.value()?;
    p.skip_trivia();
    let tail = p.remaining().trim();
    if !tail.is_empty() {
        return Err(format!("unexpected trailing JSON after the document: {tail:?}"));
    }
    Ok(value)
}

/// The values of an object literal, in declaration order.
pub fn object_values(v: &Val) -> Result<Vec<Val>, String> {
    match v {
        Val::Obj(entries) => Ok(entries.iter().map(|(_, v)| v.clone()).collect()),
        other => Err(format!("expected an object literal, got {other:?}")),
    }
}

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn remaining(&self) -> &'a str {
        std::str::from_utf8(self.src.get(self.pos..).unwrap_or_default()).unwrap_or_default()
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn starts_with(&self, word: &str) -> bool {
        self.remaining().starts_with(word)
    }

    /// Skip whitespace and `//` line comments.
    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(c) if c.is_ascii_whitespace() => self.pos += 1,
                Some(b'/') if self.remaining().starts_with("//") => {
                    while let Some(c) = self.peek() {
                        self.pos += 1;
                        if c == b'\n' {
                            break;
                        }
                    }
                }
                _ => return,
            }
        }
    }

    /// Skip trivia plus any `satisfies <Type>` / `as const` assertion that follows a value.
    ///
    /// The type expression is skipped by balancing `<`/`>`, which is enough for the single form the
    /// generator emits (`satisfies Model<"openai-completions">`) and refuses to run off the end.
    fn skip_trivia_and_assertions(&mut self) {
        loop {
            self.skip_trivia();
            if self.starts_with("satisfies") {
                self.pos += "satisfies".len();
                self.skip_trivia();
                // identifier
                while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c == b'.')
                {
                    self.pos += 1;
                }
                self.skip_trivia();
                if self.peek() == Some(b'<') {
                    let mut depth = 0usize;
                    while let Some(c) = self.peek() {
                        self.pos += 1;
                        match c {
                            b'<' => depth += 1,
                            b'>' => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                continue;
            }
            if self.starts_with("as const") {
                self.pos += "as const".len();
                continue;
            }
            return;
        }
    }

    fn value(&mut self) -> Result<Val, String> {
        self.skip_trivia();
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Val::Str(self.string()?)),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.number(),
            Some(_) if self.starts_with("true") => {
                self.pos += 4;
                Ok(Val::Bool(true))
            }
            Some(_) if self.starts_with("false") => {
                self.pos += 5;
                Ok(Val::Bool(false))
            }
            Some(_) if self.starts_with("null") => {
                self.pos += 4;
                Ok(Val::Null)
            }
            Some(c) => Err(format!(
                "unsupported construct at byte {}: {:?} — this scanner covers only pi's generated \
                 data-literal subset, and silently skipping it would drop catalog data",
                self.pos, c as char
            )),
            None => Err("unexpected end of module while reading a value".to_string()),
        }
    }

    fn object(&mut self) -> Result<Val, String> {
        self.pos += 1; // '{'
        let mut entries: Vec<(String, Val)> = Vec::new();
        loop {
            self.skip_trivia();
            match self.peek() {
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Val::Obj(entries));
                }
                Some(b',') => {
                    self.pos += 1;
                    continue;
                }
                None => return Err("unexpected end of module inside an object".to_string()),
                _ => {}
            }
            let key = if self.peek() == Some(b'"') {
                self.string()?
            } else {
                self.identifier()?
            };
            self.skip_trivia();
            if self.peek() != Some(b':') {
                return Err(format!("expected `:` after key {key:?} at byte {}", self.pos));
            }
            self.pos += 1;
            let value = self.value()?;
            self.skip_trivia_and_assertions();
            entries.push((key, value));
        }
    }

    fn array(&mut self) -> Result<Val, String> {
        self.pos += 1; // '['
        let mut items = Vec::new();
        loop {
            self.skip_trivia();
            match self.peek() {
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Val::Arr(items));
                }
                Some(b',') => {
                    self.pos += 1;
                    continue;
                }
                None => return Err("unexpected end of module inside an array".to_string()),
                _ => {}
            }
            let value = self.value()?;
            self.skip_trivia_and_assertions();
            items.push(value);
        }
    }

    fn identifier(&mut self) -> Result<String, String> {
        let start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric() || c == b'_' || c == b'$') {
            self.pos += 1;
        }
        if start == self.pos {
            return Err(format!("expected an object key at byte {start}"));
        }
        Ok(self
            .src
            .get(start..self.pos)
            .and_then(|b| std::str::from_utf8(b).ok())
            .unwrap_or_default()
            .to_string())
    }

    fn string(&mut self) -> Result<String, String> {
        self.pos += 1; // opening quote
        let mut out = String::new();
        loop {
            let c = self
                .peek()
                .ok_or_else(|| "unterminated string literal".to_string())?;
            self.pos += 1;
            match c {
                b'"' => return Ok(out),
                b'\\' => {
                    let esc = self
                        .peek()
                        .ok_or_else(|| "unterminated escape sequence".to_string())?;
                    self.pos += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'b' => out.push('\u{08}'),
                        b'f' => out.push('\u{0c}'),
                        b'u' => {
                            let hex = self
                                .src
                                .get(self.pos..self.pos + 4)
                                .and_then(|b| std::str::from_utf8(b).ok())
                                .ok_or_else(|| "truncated \\u escape".to_string())?;
                            self.pos += 4;
                            let cp = u32::from_str_radix(hex, 16)
                                .map_err(|e| format!("bad \\u escape {hex:?}: {e}"))?;
                            out.push(char::from_u32(cp).ok_or_else(|| {
                                format!("\\u{hex} is not a scalar value (surrogate pairs unported)")
                            })?);
                        }
                        other => {
                            return Err(format!("unsupported escape `\\{}`", other as char));
                        }
                    }
                }
                _ => {
                    // Multi-byte UTF-8 continuation bytes flow through untouched.
                    let start = self.pos - 1;
                    let end = self.pos;
                    if let Some(chunk) = self.src.get(start..end)
                        && let Ok(s) = std::str::from_utf8(chunk)
                    {
                        out.push_str(s);
                    } else {
                        // Continuation byte: back up and take the whole char.
                        let rest = std::str::from_utf8(self.src.get(start..).unwrap_or_default())
                            .map_err(|e| format!("invalid UTF-8 in string literal: {e}"))?;
                        let ch = rest
                            .chars()
                            .next()
                            .ok_or_else(|| "invalid UTF-8 in string literal".to_string())?;
                        out.push(ch);
                        self.pos = start + ch.len_utf8();
                    }
                }
            }
        }
    }

    fn number(&mut self) -> Result<Val, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_digit() || c == b'.' || c == b'e' || c == b'E' || c == b'+' || c == b'-')
        {
            // `-` is only part of a numeral directly after an exponent marker.
            if matches!(self.peek(), Some(b'-') | Some(b'+')) {
                let prev = self.pos.checked_sub(1).and_then(|i| self.src.get(i)).copied();
                if !matches!(prev, Some(b'e') | Some(b'E')) {
                    break;
                }
            }
            self.pos += 1;
        }
        let text = self
            .src
            .get(start..self.pos)
            .and_then(|b| std::str::from_utf8(b).ok())
            .unwrap_or_default();
        if text.is_empty() {
            return Err(format!("expected a numeral at byte {start}"));
        }
        Ok(Val::Num(text.to_string()))
    }
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

    const SAMPLE: &str = r#"
// This file is auto-generated by scripts/generate-models.ts
// Do not edit manually - run 'npm run generate-models' to update

import type { Model } from "../types.ts";

export const XAI_MODELS = {
	"grok-4.5": {
		id: "grok-4.5",
		name: "Grok 4.5",
		api: "openai-responses",
		provider: "xai",
		baseUrl: "https://api.x.ai/v1",
		compat: {"supportsLongCacheRetention":false},
		reasoning: true,
		thinkingLevelMap: {"off":null,"minimal":null},
		input: ["text", "image"],
		cost: {
			input: 2,
			output: 6,
			cacheRead: 0.5,
			cacheWrite: 0,
		},
		contextWindow: 500000,
		maxTokens: 500000,
	} satisfies Model<"openai-responses">,
} as const;
"#;

    #[test]
    fn parses_the_generated_subset_in_declaration_order() {
        let models = parse_models_module(SAMPLE).unwrap();
        assert_eq!(models.len(), 1);
        let m = &models[0];
        assert_eq!(m.get("id").and_then(Val::as_str), Some("grok-4.5"));
        assert_eq!(m.get("api").and_then(Val::as_str), Some("openai-responses"));
        let Val::Obj(entries) = m else { panic!("object") };
        let keys: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "id",
                "name",
                "api",
                "provider",
                "baseUrl",
                "compat",
                "reasoning",
                "thinkingLevelMap",
                "input",
                "cost",
                "contextWindow",
                "maxTokens"
            ],
            "declaration order is what makes the emitted catalogs diffable"
        );
    }

    #[test]
    fn null_inside_thinking_level_map_survives() {
        let models = parse_models_module(SAMPLE).unwrap();
        let map = models[0].get("thinkingLevelMap").unwrap();
        assert_eq!(map.get("off"), Some(&Val::Null));
        assert_eq!(map.get("minimal"), Some(&Val::Null));
    }

    #[test]
    fn numerals_keep_their_source_text() {
        let models = parse_models_module(SAMPLE).unwrap();
        let cost = models[0].get("cost").unwrap();
        assert_eq!(cost.get("cacheRead"), Some(&Val::Num("0.5".into())));
        assert_eq!(cost.get("cacheWrite"), Some(&Val::Num("0".into())));
        assert_eq!(
            models[0].get("contextWindow"),
            Some(&Val::Num("500000".into())),
            "a f64 round-trip would be free to render this as 500000.0"
        );
    }

    #[test]
    fn emits_tab_indented_json() {
        let models = parse_models_module(SAMPLE).unwrap();
        let json = Val::Arr(models).to_json();
        assert!(json.starts_with("[\n\t{\n\t\t\"id\": \"grok-4.5\","), "{json}");
        assert!(json.contains("\t\t\"compat\": {\n\t\t\t\"supportsLongCacheRetention\": false\n\t\t},"));
        assert!(json.ends_with("\n]"));
    }

    /// The scanner must REFUSE what it does not model rather than skip it — a generator that
    /// silently drops a construct is how catalog data goes missing without a diff.
    #[test]
    fn unsupported_constructs_are_errors_not_skips() {
        let src = "export const X = { a: { id: someIdentifier } } as const;";
        let err = parse_models_module(src).unwrap_err();
        assert!(err.contains("unsupported construct"), "{err}");
    }

    #[test]
    fn remove_drops_only_the_named_key() {
        let mut v = Val::Obj(vec![
            ("a".into(), Val::Num("1".into())),
            ("b".into(), Val::Num("2".into())),
        ]);
        assert!(v.remove("a"));
        assert!(!v.remove("a"));
        assert_eq!(v, Val::Obj(vec![("b".into(), Val::Num("2".into()))]));
    }
}
