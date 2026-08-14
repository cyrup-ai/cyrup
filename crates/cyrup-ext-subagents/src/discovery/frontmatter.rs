//! Hand-rolled YAML-subset frontmatter parser (func-SA §5.1 R-SA-005/006/018; arch-SA §6.2.3).
//!
//! This is a deliberate **line-oriented parser for the exact permissive grammar
//! `pi-subagents`' own `src/agents/frontmatter.ts::parseFrontmatter` implements** — flat
//! `key: value` pairs plus **one level** of block-indent values (an empty-valued `key:` line
//! followed by more-indented continuation lines, common-leading-whitespace-stripped and stored as
//! a single newline-joined string), plus **folded block scalars** (`key: >` / `key: >-`, folded per
//! [`fold_block`]) and **block lists** (`- item` lines, normalized by [`parse_frontmatter_list`]).
//! It is **not** a general YAML parser: no anchors, no literal-block indicator (`|`), no
//! nested-block-of-blocks. Per arch-SA §6.2.3 / §12
//! item 2, a general YAML crate MAY be substituted later only if verified byte-for-byte compatible
//! against this exact grammar — until then this hand-rolled parser is the source of truth.
//!
//! Two layers live in this module:
//!
//! 1. [`parse_frontmatter_block`] — the low-level line-oriented block parser, a direct, faithful
//!    port of `frontmatter.ts::parseFrontmatter`. Returns the raw `key -> value` string map plus
//!    the trimmed markdown body. Reused as-is by `discovery/chains.rs` for `.chain.md` files
//!    (same frontmatter grammar, func-SA §4.1), which is why it is `pub(crate)` rather than
//!    private to this file.
//! 2. [`parse_agent_file`] — the agent-specific layer: required-field silent-skip (R-SA-005),
//!    package-identifier validation with whole-file skip on failure (R-SA-006), tool-list
//!    splitting (`mcp:`-prefixed vs. everything else), name-sensitive `systemPromptMode`/
//!    `inheritProjectContext` defaults (R-SA-018), and `AgentDefinition` construction including
//!    `present_fields`/`extra_fields` round-trip bookkeeping.
//!
//! # Faithfulness notes (verified against `pi-subagents/src/agents/{frontmatter,agents,
//! identity,agent-serializer}.ts`)
//!
//! - Required fields are exactly `name` and `description`; either missing means the whole file is
//!   silently skipped (R-SA-005) — no error, no diagnostic, discovery of siblings continues
//!   (owned by this file's caller in `discovery/mod.rs`, which simply skips a `None` return).
//! - `package` normalization (lowercase, whitespace -> `-`, strip non `[a-z0-9.-]`, collapse
//!   repeats, trim leading/trailing `-`/`.`) then validated against
//!   `^[a-z0-9][a-z0-9-]*(?:\.[a-z0-9][a-z0-9-]*)*$`; a failure skips the **entire** file
//!   (R-SA-006), not merely the `package` field.
//! - `systemPromptMode`/`inheritProjectContext` discovery-time defaults are computed from the
//!   agent's **local name** (pre-packaging) — `Append`/`true` only when local name is exactly
//!   `"delegate"`, else `Replace`/`false` (R-SA-018) — applying even when the agent is packaged.
//! - List fields (`tools`, `defaultReads`, `skill`/`skills`, `fallbackModels`, `extensions`,
//!   `subagentOnlyExtensions`) accept comma-separated OR block-list (`- item`) syntax via
//!   [`parse_frontmatter_list`]; each entry is trimmed and empties are dropped.
//! - Boolean-ish fields (`inheritProjectContext`, `inheritSkills`, `defaultProgress`,
//!   `interactive`) accept literal `"true"`/`"false"` strings only; anything else is treated as
//!   "not stated" and falls through to that field's own default.
//! - `maxSubagentDepth` parses as a non-negative integer; anything that does not parse to a
//!   non-negative integer is dropped (treated as absent), matching source's
//!   `Number.isInteger(n) && n >= 0` guard.
//! - Unknown frontmatter keys are preserved verbatim into `extra_fields` for round-trip
//!   serialization — this explicitly includes `interactive`, which is parsed into its own typed
//!   field AND still participates in `present_fields` (never silently dropped, func-SA §4.1).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use cyrup_core::ModelId;

use super::types::{AgentDefinition, AgentSource, OutputMode, OutputSpec, SystemPromptMode, ToolRef};
use crate::fork_context::ContextMode;

/// The two frontmatter keys whose absence causes the entire agent file to be silently skipped
/// (R-SA-005). Exposed as constants so both this module and any diagnostic/test surface reference
/// the exact same literal keys.
pub const REQUIRED_FIELD_NAME: &str = "name";
pub const REQUIRED_FIELD_DESCRIPTION: &str = "description";

/// Every frontmatter key this parser gives first-class typed treatment to — mirrors
/// `pi-subagents/src/agents/agent-serializer.ts`'s `KNOWN_FIELDS` set exactly. Any frontmatter key
/// **not** in this set is preserved verbatim into [`AgentDefinition::extra_fields`] rather than
/// silently dropped.
const KNOWN_FIELDS: &[&str] = &[
    "name",
    "package",
    "description",
    // Both alias spellings entered `KNOWN_FIELDS` with pi's agent-alias feature
    // (`agent-serializer.ts:9-10` @ v0.43.0). They MUST be here AND emitted by
    // `management::serialize_agent`: a key that is "known" but never written is silently deleted on
    // the first management rewrite, because the extra-fields round-trip loop skips known keys.
    "alias",
    "aliases",
    "tools",
    "model",
    "fallbackModels",
    "thinking",
    "systemPromptMode",
    "inheritProjectContext",
    "inheritSkills",
    "defaultContext",
    "skill",
    "skills",
    "extensions",
    "subagentOnlyExtensions",
    "output",
    "defaultReads",
    "defaultProgress",
    "interactive",
    "maxSubagentDepth",
    "completionGuard",
    // Both present in `KNOWN_FIELDS` at the ported v0.34.0 baseline
    // (`agent-serializer.ts:4-26` @ v0.34.0). Until they were parsed here, a `toolBudget:` or
    // `memory:` line in an agent file was demoted to `extra_fields` and did nothing at all.
    "toolBudget",
    "memory",
    // Agent-level LAUNCH DEFAULTS (`agent-serializer.ts:4-40` @ v0.43.0). Parsed into
    // `default_async`/`default_timeout_ms` and applied by `route_single` only when the call site
    // omitted the corresponding parameter.
    "async",
    "timeoutMs",
];

/// True iff `key` is one of the crate's first-class typed frontmatter fields (pi's `KNOWN_FIELDS`,
/// `agent-serializer.ts:4-26`). Exposed `pub(crate)` so `management.rs`'s agent serializer can apply
/// the same "extra_fields loop skips known keys" guard pi's `serializeAgent` applies
/// (`agent-serializer.ts:91-104`), keeping the two modules' notion of "known" in exact lockstep.
pub(crate) fn is_known_field(key: &str) -> bool {
    KNOWN_FIELDS.contains(&key)
}

// ---------------------------------------------------------------------------------------------
// Layer 1: the line-oriented YAML-subset block parser (arch-SA §6.2.3)
// ---------------------------------------------------------------------------------------------

/// The parsed result of one frontmatter block: an ordered `key -> raw value` map (insertion order
/// preserved so callers needing deterministic `present_fields` iteration or diagnostics can rely
/// on it) plus the trimmed markdown body that follows the closing `---`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedFrontmatter {
    /// Ordered key/value pairs exactly as they appeared in the frontmatter block. Block (nested,
    /// one-level-indent) values are stored as a single string with embedded newlines, common
    /// leading whitespace stripped relative to the block's own minimum indent — never split into
    /// a further nested map (this parser has no concept of two-level nesting, matching source).
    pub fields: Vec<(String, String)>,
    /// The markdown body following the closing `---` delimiter, trimmed of leading/trailing
    /// whitespace.
    pub body: String,
}

impl ParsedFrontmatter {
    /// Look up one frontmatter value by key (first match; frontmatter files are not expected to
    /// repeat a key, but if they do, first-occurrence-wins mirrors a plain JS object literal's
    /// last-write-wins... note this parser instead needs last-wins to match `frontmatter[key] =
    /// value` semantics — see [`Self::get`]'s doc for the precise rule).
    pub fn get(&self, key: &str) -> Option<&str> {
        // The TS source assigns into a plain object (`frontmatter[match[1]] = value`), so a
        // repeated key's LAST occurrence wins. Mirror that exactly rather than first-match.
        self.fields
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// True iff `key` appeared literally in the parsed frontmatter block (R-SA-010's
    /// `present_fields` bookkeeping).
    pub fn contains_key(&self, key: &str) -> bool {
        self.fields.iter().any(|(k, _)| k == key)
    }

    /// All literal keys that appeared in the frontmatter block, in first-occurrence order,
    /// deduplicated. Used to populate [`AgentDefinition::present_fields`] (a `HashSet`, so
    /// iteration order here does not matter to that caller) and to build `extra_fields`.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        let mut seen = HashSet::new();
        self.fields.iter().filter_map(move |(k, _)| {
            if seen.insert(k.as_str()) {
                Some(k.as_str())
            } else {
                None
            }
        })
    }
}

/// Escape-free "does this line look like `key: value`" matcher, mirroring the source regex
/// `^([\w-]+):\s*(.*)$` — word characters (`[A-Za-z0-9_]`) and hyphens only in the key, a colon,
/// optional whitespace, then the rest of the line as the raw value. Returns `None` for lines that
/// don't match (comments, blank lines, malformed lines) — such lines are silently ignored, never
/// an error, matching source's own silent-ignore behavior for non-matching lines.
///
/// **Anchored at column 0.** Source runs this regex against the RAW (untrimmed) `line`
/// (`frontmatter.ts:61`), and `^([\w-]+)` therefore requires the key to begin at the very start of
/// the line — a leading space is not a `[\w-]` character, so an INDENTED `key: value` line does not
/// match and is ignored (unless it was already consumed as a block-value continuation earlier in the
/// loop). This function reproduces that by rejecting any key run containing a non-`[\w-]` byte (a
/// leading space lands in the key slice and fails the check), so callers MUST pass the raw line, not
/// a trimmed copy — passing a trimmed line would wrongly turn an indented orphan `key: value` into a
/// field (the bug this anchoring fixes).
fn match_key_value(line: &str) -> Option<(&str, &str)> {
    let colon_idx = line.find(':')?;
    let key = &line[..colon_idx];
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return None;
    }
    let rest = &line[colon_idx + 1..];
    Some((key, rest.trim_start()))
}

/// Strip a single layer of matching surrounding quotes (`"..."` or `'...'`), mirroring source's
/// `value.slice(1, -1)` guarded by a matching-pair check. A value that starts with a quote char
/// but does not end with the *same* quote char (or is too short to contain a matching pair) is
/// left untouched.
///
/// Returns `(was_quoted, value)`. The `was_quoted` flag matters because source gates its
/// folded-block-scalar detection on it (`frontmatter.ts:121`: `isFolded = !isQuoted && (rawValue
/// === ">" || rawValue === ">-")`) — a literally quoted `">"` is the one-character STRING `>`, not
/// a block indicator.
fn strip_matching_quotes(value: &str) -> (bool, &str) {
    let bytes = value.as_bytes();
    if let (Some(&first), Some(&last)) = (bytes.first(), bytes.last())
        && bytes.len() >= 2
        && ((first == b'"' && last == b'"') || (first == b'\'' && last == b'\''))
    {
        // Safe: both quote chars are single-byte ASCII, so slicing at these positions never
        // splits a multi-byte UTF-8 sequence. `get(1..value.len() - 1)` avoids any raw indexing.
        if let Some(inner) = value.get(1..value.len() - 1) {
            return (true, inner);
        }
    }
    (false, value)
}

/// The position (in `char` count, matching JS `String.prototype.search`'s semantics closely
/// enough for the ASCII-indentation-only inputs this grammar targets) of the first non-whitespace
/// character in `line`, or the line's full length if it is entirely whitespace (mirrors source's
/// `line.search(/\S|$/)`, which likewise returns the string length for an all-whitespace line).
fn first_non_whitespace_offset(line: &str) -> usize {
    line.chars()
        .position(|c| !c.is_whitespace())
        .unwrap_or_else(|| line.chars().count())
}

/// The block's common leading-whitespace prefix, mirroring source's
/// `rawBlock.match(/^[ \t]+(?=\S)/m)` (`frontmatter.ts:102`) EXACTLY: scan the joined block
/// line-by-line (the `m` flag makes `^` match after every `\n`) and return the leading `[ \t]+` run
/// of the FIRST line whose run is immediately followed by a NON-WHITESPACE character (the `(?=\S)`
/// lookahead). Lines that are entirely whitespace — and a leading BLANK line — are skipped rather
/// than yielding an empty/degenerate prefix.
///
/// **This is the indent-anchor fix.** The previous implementation took the prefix from the raw
/// block's very first characters (`raw_block.chars().take_while(..)`), which silently produced an
/// EMPTY prefix — hence no dedent at all — whenever the block's first captured line was blank. That
/// never happened before folded scalars existed (a blank line has `indent == 0`, so it could not be
/// captured as a continuation), but a folded block DOES capture blank lines
/// (`frontmatter.ts:91`), so `description: >` followed by a blank line and then indented text used
/// to keep its full source indentation in the parsed value. Anchoring on the first line that
/// actually has content reproduces the regex.
///
/// Greediness note: `[ \t]+` is greedy and the lookahead cannot be satisfied by backtracking (every
/// shorter prefix is still followed by ` ` or `\t`, which are not `\S`), so "maximal run, then
/// require a non-whitespace char" is the regex's exact behaviour.
fn block_indent_prefix(raw_block: &str) -> String {
    for line in raw_block.split('\n') {
        let ws: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        if ws.is_empty() {
            continue;
        }
        // `ws` is built from single-byte ASCII chars, so `ws.len()` is a valid char boundary.
        match line.get(ws.len()..).and_then(|rest| rest.chars().next()) {
            Some(c) if !c.is_whitespace() => return ws,
            _ => continue,
        }
    }
    String::new()
}

/// Strip the common leading whitespace prefix from a set of raw block-continuation lines, then
/// join with `\n`. Source strips that prefix from every line via a global multiline regex replace,
/// then trims one leading `\n` if the strip left one (`frontmatter.ts:104-106`).
fn dedent_block(raw_lines: &[String]) -> String {
    let raw_block = raw_lines.join("\n");
    let prefix = block_indent_prefix(&raw_block);
    if prefix.is_empty() {
        return raw_block;
    }
    // `split('\n')` (not `.lines()`): a trailing empty segment is a real line of the block and must
    // survive the round-trip. `.lines()` swallows it, which folded blocks (which capture blank
    // lines) would otherwise notice.
    let stripped: String = raw_block
        .split('\n')
        .map(|line| line.strip_prefix(prefix.as_str()).unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n");
    // Mirror source's final `.replace(/^\n/, "")`: drop exactly one leading newline left over
    // from the strip, if present.
    stripped.strip_prefix('\n').map_or(stripped.clone(), str::to_string)
}

/// Fold a YAML folded block scalar (`>` / `>-`), a line-for-line port of
/// `pi-subagents/src/agents/frontmatter.ts::foldBlock` (`frontmatter.ts:12-40`).
///
/// Folding rules, exactly as source implements them:
/// - each line is `trimEnd`ed; a line that is empty after trimming is a BLANK-line separator and is
///   counted, never emitted directly;
/// - a line that still has leading whitespace after the block-level dedent is "more indented" and
///   YAML preserves its line break rather than folding it into a space;
/// - between two consecutive content lines: a single space if neither is more-indented, else a
///   newline;
/// - across `n` blank lines: `n` newlines, plus one extra if either neighbour is more-indented;
/// - the whole result is finally `trim`ed, so `>` and `>-` behave identically here (source draws no
///   distinction between them either — both route through this same function).
fn fold_block(block: &str) -> String {
    let mut folded = String::new();
    let mut has_content = false;
    let mut previous_is_more_indented = false;
    let mut blank_lines: usize = 0;

    for line in block.split('\n') {
        let current = line.trim_end();
        if current.trim().is_empty() {
            if has_content {
                blank_lines += 1;
            }
            continue;
        }

        let current_is_more_indented = current.len() > current.trim_start().len();
        if has_content {
            if blank_lines > 0 {
                let extra = usize::from(previous_is_more_indented || current_is_more_indented);
                for _ in 0..(blank_lines + extra) {
                    folded.push('\n');
                }
            } else if previous_is_more_indented || current_is_more_indented {
                folded.push('\n');
            } else {
                folded.push(' ');
            }
        }
        folded.push_str(current);
        has_content = true;
        previous_is_more_indented = current_is_more_indented;
        blank_lines = 0;
    }

    folded.trim().to_string()
}

/// Normalize a simple-scalar frontmatter list from **either** comma-separated **or** YAML
/// block-list syntax — a direct port of `frontmatter.ts::parseFrontmatterList`
/// (`frontmatter.ts:46-57`).
///
/// Each physical line is trimmed, then the standard `- ` list marker is removed IF present
/// (source's `/^-\s+(.+)$/` — note the REQUIRED whitespace after the hyphen, which is what keeps an
/// ordinary hyphen-leading value like `-foo` intact), and the remainder is then split on `,`. Every
/// resulting entry is trimmed and empties are dropped.
///
/// `None` in, `None` out — the caller distinguishes "the key was absent" from "the key was present
/// but yielded no entries", which several fields depend on (source spreads `...(rawTools !==
/// undefined ? { tools } : {})`, so an explicitly EMPTY `tools:` means "no tools", not "default
/// tools").
pub(crate) fn parse_frontmatter_list(raw: Option<&str>) -> Option<Vec<String>> {
    let raw = raw?;
    Some(
        raw.split('\n')
            .flat_map(|line| {
                let value = line.trim();
                let item = value
                    .strip_prefix('-')
                    .filter(|rest| rest.starts_with(char::is_whitespace))
                    .map(str::trim_start)
                    .filter(|rest| !rest.is_empty())
                    .unwrap_or(value);
                item.split(',').collect::<Vec<_>>()
            })
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// Normalize a raw alias list against the agent's own runtime name — a direct port of pi's
/// `normalizeAgentAliases` (`agents.ts:495-499`):
///
/// ```text
/// [...new Set((rawAliases ?? []).map((alias) => alias.trim()).filter(Boolean))]
///     .filter((alias) => alias !== agentName)
/// ```
///
/// Order matters and is preserved exactly: **trim → drop empties → de-duplicate (first occurrence
/// wins, JS `Set` insertion order) → drop the agent's own name**. The self-name filter runs LAST, so
/// an alias equal to the agent name is removed even if it also appeared twice.
///
/// pi returns `undefined` for an empty result; this returns an empty `Vec`, which
/// [`AgentDefinition::aliases`] documents as the identical state.
#[must_use]
pub fn normalize_agent_aliases(raw_aliases: Vec<String>, agent_name: &str) -> Vec<String> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for alias in raw_aliases {
        let trimmed = alias.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !seen.insert(trimmed.to_string()) {
            continue;
        }
        if trimmed == agent_name {
            continue;
        }
        out.push(trimmed.to_string());
    }
    out
}

/// Parse a full agent/chain `.md` file's frontmatter block: flat `key: value` lines plus one level
/// of block-indent values, faithfully porting `pi-subagents/src/agents/frontmatter.ts`'s
/// `parseFrontmatter` line for line (arch-SA §6.2.3).
///
/// A file that does not start with `---` (after normalizing `\r\n` to `\n`), or has no closing
/// `\n---` delimiter, yields an **empty** frontmatter map and the entire (normalized) input as the
/// body — never an error. This exactly matches source: absent/malformed frontmatter is not this
/// layer's problem to reject; the required-field check (R-SA-005) one layer up is what turns "no
/// usable frontmatter" into a silent per-file skip.
pub fn parse_frontmatter_block(content: &str) -> ParsedFrontmatter {
    let normalized = content.replace("\r\n", "\n");

    if !normalized.starts_with("---") {
        return ParsedFrontmatter {
            fields: Vec::new(),
            body: normalized,
        };
    }

    // Mirror `normalized.indexOf("\n---", 3)`: search for the closing delimiter starting at byte
    // offset 3 (i.e. allowing the search to find a `\n---` sequence that begins as early as index
    // 3, matching a `---\n---` empty-block edge case exactly as source's `indexOf` would).
    let search_from = normalized.get(3..).unwrap_or("");
    let end_index = search_from.find("\n---").map(|rel| rel + 3);

    let Some(end_index) = end_index else {
        return ParsedFrontmatter {
            fields: Vec::new(),
            body: normalized,
        };
    };

    // `frontmatterBlock = normalized.slice(4, endIndex)`: skip the opening `---` plus the newline
    // right after it (byte offset 4), up to (not including) the `\n` that starts the closing
    // delimiter.
    let frontmatter_block = normalized.get(4..end_index).unwrap_or("");
    // `body = normalized.slice(endIndex + 4).trim()`: skip past `\n---` (4 chars) and trim.
    let body = normalized
        .get(end_index + 4..)
        .unwrap_or("")
        .trim()
        .to_string();

    let mut fields: Vec<(String, String)> = Vec::new();
    let mut current_key: Option<String> = None;
    let mut current_block_lines: Vec<String> = Vec::new();
    let mut current_indent: Option<usize> = None;
    // Whether the pending block was opened by a FOLDED scalar indicator (`>` / `>-`), which changes
    // both what the block captures (blank lines too) and how it is stored (folded, not verbatim).
    let mut current_folded = false;
    // SUBA-052 / pi `currentLiteral` (`agents/frontmatter.ts:86` @v0.47.1). A LITERAL scalar
    // indicator (`|` / `|-`) captures blank lines exactly as a folded one does, but stores the
    // dedented block VERBATIM — no folding. Landed upstream in `a4fc59a` ("fix: parse block scalar
    // skill descriptions", #952), released v0.46.0; `git show v0.43.0:.../frontmatter.ts | grep
    // isLiteral` is empty, so this is drift.
    //
    // Before this flag existed, `description: |` fell through BOTH the `value.is_empty()` and the
    // `is_folded` arms — `strip_matching_quotes` yields `(false, "|")`, which is neither — and was
    // stored as the one-character string `"|"`, after which the indented body lines failed the
    // `^([\w-]+):` match and were silently discarded. Silent wrong value, no warning.
    let mut current_literal = false;

    let flush_block = |fields: &mut Vec<(String, String)>,
                        current_key: &mut Option<String>,
                        current_block_lines: &mut Vec<String>,
                        current_indent: &mut Option<usize>,
                        current_folded: &mut bool,
                        current_literal: &mut bool| {
        if let Some(key) = current_key.take() {
            let stripped = dedent_block(current_block_lines);
            // pi `frontmatter[currentKey] = currentFolded ? foldBlock(stripped) : stripped`
            // (`:110`): a LITERAL block takes the same `stripped` branch an ordinary nested block
            // does, so nothing extra happens here — only the CAPTURE and the DEFER conditions
            // change.
            let value = if *current_folded {
                fold_block(&stripped)
            } else {
                stripped
            };
            fields.push((key, value));
        }
        current_block_lines.clear();
        *current_indent = None;
        *current_folded = false;
        *current_literal = false;
    };

    for line in frontmatter_block.split('\n') {
        let indent = first_non_whitespace_offset(line);

        // Source: `indent > (currentIndent ?? 0) || (currentFolded && trimmed === "")`
        // (`frontmatter.ts:91`). A blank line has `indent == 0` and so is NOT a continuation of an
        // ordinary block — but a FOLDED block keeps it, because blank lines are a folded scalar's
        // paragraph separator and `foldBlock` needs to see them.
        // SUBA-052 / pi `:91` @v0.47.1: the blank-line continuation test is
        // `(currentFolded || currentLiteral) && trimmed === ""` — a literal block keeps its blank
        // lines for the same reason a folded one does, and here they are actually load-bearing
        // (a literal scalar's blank line IS content).
        if current_key.is_some()
            && (indent > current_indent.unwrap_or(0)
                || ((current_folded || current_literal) && line.trim().is_empty()))
        {
            // Continuation of the current block value.
            current_block_lines.push(line.to_string());
            continue;
        }

        // Flush any pending block value before considering this line as a new key (or ignoring
        // it entirely).
        flush_block(
            &mut fields,
            &mut current_key,
            &mut current_block_lines,
            &mut current_indent,
            &mut current_folded,
            &mut current_literal,
        );

        // Match against the RAW line (source: `line.match(/^([\w-]+):.../)`, `frontmatter.ts:61`),
        // NOT a trimmed copy: the `^`-anchor means an indented orphan `key: value` line that was not
        // consumed as a block continuation above does not match here and is ignored, exactly as pi.
        let Some((key, raw_value)) = match_key_value(line) else {
            // Non-matching line (comment, blank, indented orphan, malformed): silently ignored.
            continue;
        };
        let raw_value = raw_value.trim();
        let (is_quoted, value) = strip_matching_quotes(raw_value);
        // A bare `>` or `>-` opens a YAML FOLDED block scalar. Source gates this on the value not
        // being quoted (`frontmatter.ts:121`), so `description: ">"` is still the literal string.
        let is_folded = !is_quoted && (raw_value == ">" || raw_value == ">-");
        // SUBA-052 / pi `:125` @v0.47.1:
        // `const isLiteral = !isQuoted && (rawValue === "|" || rawValue === "|-")`. Gated on the
        // value not being quoted for the same reason `is_folded` is, so `description: "|"` stays
        // the literal one-character string.
        let is_literal = !is_quoted && (raw_value == "|" || raw_value == "|-");

        if value.is_empty() || is_folded || is_literal {
            // Empty-valued key or block-scalar indicator: defer storing until we see the block body.
            current_key = Some(key.to_string());
            current_block_lines = Vec::new();
            current_indent = Some(indent);
            current_folded = is_folded;
            current_literal = is_literal;
        } else {
            fields.push((key.to_string(), value.to_string()));
        }
    }

    // Flush a final pending block value (a block that ran to the end of the frontmatter section
    // with no following flat key to trigger the mid-loop flush).
    flush_block(
        &mut fields,
        &mut current_key,
        &mut current_block_lines,
        &mut current_indent,
        &mut current_folded,
        &mut current_literal,
    );

    ParsedFrontmatter { fields, body }
}

// ---------------------------------------------------------------------------------------------
// Layer 2: agent-specific parsing (R-SA-005/006/018; func-SA §4.1)
// ---------------------------------------------------------------------------------------------

/// `key: value` -> `Option<bool>`, accepting only the literal strings `"true"`/`"false"`;
/// anything else (including absence) is `None`, matching source's ternary-chain pattern
/// (`frontmatter.x === "true" ? true : frontmatter.x === "false" ? false : <default>`) at the
/// point *before* that default is applied — this function returns the "did the frontmatter state
/// something" tri-state; callers apply their own default when it returns `None`.
fn parse_bool_field(value: Option<&str>) -> Option<bool> {
    match value {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    }
}

/// Package-identifier normalization + validation (arch-SA §6.2.3 / R-SA-006), a direct port of
/// `pi-subagents/src/agents/identity.ts::normalizePackageName` + `parsePackageName`:
/// lowercase, whitespace runs -> single `-`, strip any char outside `[a-z0-9.-]`, collapse
/// repeated `-`/`.` runs, then trim leading/trailing `-`/`.`. The normalized result MUST then
/// match `^[a-z0-9][a-z0-9-]*(?:\.[a-z0-9][a-z0-9-]*)*$` (lowercase alphanumeric/hyphen segments,
/// dot-separated) or validation fails.
///
/// Returns `Ok(None)` when `raw` is `None`, empty, or whitespace-only (source: `value ===
/// undefined || value === false || value === ""` — this parser only ever sees frontmatter string
/// values or absence, never a literal boolean `false`, so the `false` arm of source's check has no
/// analog here). Returns `Err(())` when a non-empty value fails to normalize to a valid
/// identifier — the caller (R-SA-006) turns that into a whole-file skip, not a field-only skip.
fn parse_package_name(raw: Option<&str>) -> Result<Option<String>, ()> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let lowered = trimmed.to_lowercase();
    // Whitespace runs -> single "-".
    let mut collapsed_ws = String::with_capacity(lowered.len());
    let mut last_was_ws = false;
    for ch in lowered.chars() {
        if ch.is_whitespace() {
            if !last_was_ws {
                collapsed_ws.push('-');
            }
            last_was_ws = true;
        } else {
            collapsed_ws.push(ch);
            last_was_ws = false;
        }
    }
    // Strip anything outside [a-z0-9.-].
    let filtered: String = collapsed_ws
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    // Collapse repeated "-" runs, then repeated "." runs.
    let collapsed_hyphen = collapse_repeated_char(&filtered, '-');
    let collapsed_dot = collapse_repeated_char(&collapsed_hyphen, '.');
    // Trim leading/trailing runs of "-"/".".
    let final_name = collapsed_dot
        .trim_start_matches(['-', '.'])
        .trim_end_matches(['-', '.'])
        .to_string();

    if final_name.is_empty() || !is_valid_package_identifier(&final_name) {
        return Err(());
    }
    Ok(Some(final_name))
}

fn collapse_repeated_char(s: &str, target: char) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_was_target = false;
    for ch in s.chars() {
        if ch == target {
            if !prev_was_target {
                out.push(ch);
            }
            prev_was_target = true;
        } else {
            out.push(ch);
            prev_was_target = false;
        }
    }
    out
}

/// `^[a-z0-9][a-z0-9-]*(?:\.[a-z0-9][a-z0-9-]*)*$` — lowercase alphanumeric/hyphen segments,
/// dot-separated, each segment starting with an alphanumeric character (R-SA-006).
fn is_valid_package_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    for segment in s.split('.') {
        let mut chars = segment.chars();
        match chars.next() {
            Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
            _ => return false,
        }
        if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return false;
        }
    }
    true
}

/// R-SA-018: discovery-time defaults for `systemPromptMode`/`inheritProjectContext`, computed from
/// the agent's **local (pre-packaging) name** — `Append`/`true` only when `local_name ==
/// "delegate"`, else `Replace`/`false`. Applies even when the agent is packaged (a packaged agent
/// literally locally named `delegate` gets the same defaults as the unpackaged builtin).
fn default_system_prompt_mode(local_name: &str) -> SystemPromptMode {
    if local_name == "delegate" {
        SystemPromptMode::Append
    } else {
        SystemPromptMode::Replace
    }
}

fn default_inherit_project_context(local_name: &str) -> bool {
    local_name == "delegate"
}

/// Split a raw `tools` frontmatter value into typed [`ToolRef`]s: an `mcp:`-prefixed entry becomes
/// [`ToolRef::Mcp`] (prefix preserved verbatim, per `ToolRef::Mcp`'s own doc); every other entry
/// becomes [`ToolRef::Builtin`] — this parser has no registry access to distinguish a genuine
/// builtin-tool identifier from an extension-path entry at parse time, exactly mirroring
/// `pi-subagents/src/agents/agents.ts`'s own two-way split (`mcpDirectTools` vs. a single `tools`
/// bucket covering everything else); any further Builtin-vs-ExtensionPath refinement is a later
/// resolution step against a known-tools registry, out of scope for this file.
fn parse_tool_refs(entries: Vec<String>) -> Vec<ToolRef> {
    entries
        .into_iter()
        .map(|entry| {
            if let Some(mcp_name) = entry.strip_prefix("mcp:") {
                ToolRef::Mcp(mcp_name.to_string())
            } else {
                ToolRef::Builtin(entry)
            }
        })
        .collect()
}

/// `thinking: <value>` -> `Option<String>`. pi's `AgentConfig.thinking` (`agents.ts:64,86,126` @v0.43.0) is an
/// OPEN string, so this preserves the raw frontmatter value verbatim rather than coercing it into a
/// closed on-only [`cyrup_core::ThinkingLevel`] enum:
///
/// - `Some("off")` — an EXPLICIT off, kept distinct from unset (`None`); the on-only enum could name
///   neither, so the old closed-enum parse conflated the two.
/// - `Some("high")` / etc. — a recognized on-level, preserved as its literal string.
/// - `Some("super-duper")` — any other (future or provider-specific) level, PRESERVED rather than
///   dropped — thinking is not one of R-SA-005's two required fields, so an unrecognized value never
///   aborts the file and never silently disappears.
/// - `None` — the frontmatter key was absent OR present-but-empty (pi treats an empty `thinking`
///   value as falsy/no-op, so an empty value collapses to "unset" here; the key's literal presence
///   is still recorded in `present_fields` for the serializer's preserve-frontmatter round-trip).
fn parse_thinking_value(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// `defaultContext: <value>` -> `Option<ContextMode>`. Accepts `"fork"`/`"fresh"` (case-sensitive,
/// matching source's `frontmatter.defaultContext === "fork" ? "fork" : frontmatter.defaultContext
/// === "fresh" ? "fresh" : undefined`); anything else (including absence) is `None`, meaning "the
/// agent stated no preference" — the crate-wide default (`ContextMode::Fresh`) is applied by a
/// later layer (`merge.rs`/`exec/`), not eagerly defaulted here, so `present_fields` can still
/// distinguish "agent declared `defaultContext`" from "agent said nothing" (see
/// `AgentDefinition::default_context`'s own doc).
fn parse_default_context(raw: &str) -> Option<ContextMode> {
    match raw {
        "fork" => Some(ContextMode::Fork),
        "fresh" => Some(ContextMode::Fresh),
        _ => None,
    }
}

/// `maxSubagentDepth: <value>` -> `Option<u32>`. Only a value that parses as a non-negative
/// integer is accepted; anything else (negative, non-numeric, fractional) is treated as absent —
/// mirrors source's `Number.isInteger(parsed) && parsed >= 0 ? parsed : undefined` guard exactly.
fn parse_max_subagent_depth(raw: &str) -> Option<u32> {
    raw.trim().parse::<u32>().ok()
}

/// `output: <path>` -> `Option<OutputSpec>`. Frontmatter's `output` field is a single path string
/// in source (`AgentConfig.output: string`, e.g. `output: context.md`) — this parser lifts it into
/// an [`OutputSpec`] with `path` populated and `mode: None` (no agent-level default output *mode*
/// is expressible in frontmatter; only a call-site `RunOptions::output_mode` or a later config
/// layer can supply one, per `OutputSpec`'s own doc).
fn parse_output_spec(raw: &str) -> Option<OutputSpec> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(OutputSpec {
        path: Some(PathBuf::from(trimmed)),
        mode: None::<OutputMode>,
    })
}

/// Parse one agent `.md` file's contents into an [`AgentDefinition`], or `None` if the file MUST
/// be silently skipped per R-SA-005 (missing `name`/`description`) or R-SA-006 (invalid `package`
/// identifier — the **entire** file is skipped, not merely the `package` field).
///
/// `source` and `file_path` are supplied by the caller (`discovery/mod.rs`'s directory walk) since
/// this function has no filesystem access of its own — it operates purely on already-read file
/// contents, keeping it trivially unit-testable against in-memory fixtures.
///
/// Never returns an `Err` for a malformed individual agent file — R-SA-009's three-way
/// throw/silent-skip/diagnostic distinction reserves silent-skip for exactly this case (malformed
/// *agent* frontmatter, as opposed to malformed *settings*, which aborts discovery, or malformed
/// *chain* files, which produce a non-fatal diagnostic elsewhere in `discovery/chains.rs`).
pub fn parse_agent_file(content: &str, source: AgentSource, file_path: &Path) -> Option<AgentDefinition> {
    let parsed = parse_frontmatter_block(content);

    let local_name = parsed.get(REQUIRED_FIELD_NAME)?.to_string();
    let description = parsed.get(REQUIRED_FIELD_DESCRIPTION)?.to_string();
    if local_name.is_empty() || description.is_empty() {
        // Source's `!frontmatter.name || !frontmatter.description` treats an empty string
        // identically to "absent" (both are JS-falsy) — mirror that exactly rather than only
        // checking for the key's literal presence.
        return None;
    }

    // R-SA-006: invalid `package` identifier skips the WHOLE file, not merely the field.
    let package_name = parse_package_name(parsed.get("package")).ok()?;

    let runtime_name = AgentDefinition::qualified_name(&local_name, package_name.as_deref());

    // G103 / pi `agents.ts:1610` @v0.43.0: `...(rawTools !== undefined ? { tools } : {})` — the
    // `tools` field is carried onto the agent exactly when the frontmatter KEY was present, and its
    // value is whatever `splitToolList` produced, INCLUDING the empty list. An explicitly-empty
    // `tools:` therefore means "this agent gets no tools" and stays distinct from an absent
    // `tools:`, which means "no allowlist restriction" (see [`AgentDefinition::tools`]'s own doc).
    // Collapsing the two — which this used to do with an `is_empty() -> None` fold — silently
    // handed a no-tools agent the full builtin set, because [`crate::exec::build_attempt_spawn_plan`]
    // emits `--no-tools` only for the explicit-but-empty case.
    let tools = parse_frontmatter_list(parsed.get("tools")).map(parse_tool_refs);

    let default_reads = parse_frontmatter_list(parsed.get("defaultReads"))
        .filter(|v| !v.is_empty())
        .map(|v| v.into_iter().map(PathBuf::from).collect::<Vec<_>>());

    // `aliases:` / `alias:` — pi `agents.ts:1516`:
    // `normalizeAgentAliases(parseFrontmatterList(frontmatter.aliases ?? frontmatter.alias), runtimeName)`.
    // The PLURAL key is tried first (`??` short-circuits on the plural being present at all), and the
    // normalization is measured against the RUNTIME name, not the local one.
    let aliases = normalize_agent_aliases(
        parse_frontmatter_list(parsed.get("aliases").or_else(|| parsed.get("alias")))
            .unwrap_or_default(),
        &runtime_name,
    );

    // `skill` and `skills` are aliases (source: `frontmatter.skill || frontmatter.skills`) — the
    // singular form is tried first, matching source's `||` short-circuit precedence exactly.
    let skill_raw = parsed.get("skill").or_else(|| parsed.get("skills"));
    let skills = parse_frontmatter_list(skill_raw).unwrap_or_default();

    let fallback_models: Vec<ModelId> = parse_frontmatter_list(parsed.get("fallbackModels"))
        .unwrap_or_default()
        .into_iter()
        .map(ModelId::from)
        .collect();

    let system_prompt_mode = match parsed.get("systemPromptMode") {
        Some("replace") => SystemPromptMode::Replace,
        Some("append") => SystemPromptMode::Append,
        _ => default_system_prompt_mode(&local_name),
    };

    let inherit_project_context = match parse_bool_field(parsed.get("inheritProjectContext")) {
        Some(v) => v,
        None => default_inherit_project_context(&local_name),
    };

    // `inheritSkills` has a fixed default of `false` regardless of local name (source:
    // `defaultInheritSkills()` always returns `false`).
    let inherit_skills = parse_bool_field(parsed.get("inheritSkills")).unwrap_or(false);

    let default_context = parsed.get("defaultContext").and_then(parse_default_context);

    let extensions = parse_frontmatter_list(parsed.get("extensions"));

    let subagent_only_extensions =
        parse_frontmatter_list(parsed.get("subagentOnlyExtensions")).unwrap_or_default();

    let model = parsed.get("model").map(ModelId::from);
    let thinking = parsed.get("thinking").and_then(parse_thinking_value);
    let output = parsed.get("output").and_then(parse_output_spec);
    let default_progress = parse_bool_field(parsed.get("defaultProgress"));
    let interactive = parse_bool_field(parsed.get("interactive"));
    let max_subagent_depth = parsed
        .get("maxSubagentDepth")
        .and_then(parse_max_subagent_depth);
    let completion_guard = parse_bool_field(parsed.get("completionGuard"));

    // `toolBudget:` — pi `agents.ts:1163-1195` @ v0.34.0: a present, non-blank value is
    // `JSON.parse`d and must be a JSON OBJECT; `tool-budget.ts::validateToolBudgetConfig` then
    // normalizes it. Until this landed, `toolBudget` was demoted to `extra_fields` and no budget
    // ever reached a child.
    //
    // `[CYRUP-DELTA]` on the FAILURE path only: pi THROWS out of `loadAgentsFromDir`, so one
    // malformed `toolBudget:` anywhere aborts agent discovery entirely and the user loses every
    // agent. This function's contract is R-SA-005/006's per-file silent skip (it returns
    // `Option`, not `Result`), so a malformed budget skips THIS FILE — the same treatment an
    // invalid `package` gets — and logs the reason at `warn` rather than taking the whole
    // directory down with it. The valid path is byte-identical to pi.
    let tool_budget = match parsed.get("toolBudget").filter(|v| !v.trim().is_empty()) {
        None => None,
        Some(raw) => {
            let parsed_json = serde_json::from_str::<serde_json::Value>(raw).map_err(|err| {
                format!("Agent '{local_name}' has invalid toolBudget frontmatter; expected a JSON object. ({err})")
            });
            match parsed_json.and_then(|value| {
                crate::exec::tool_budget::validate_tool_budget_config(
                    Some(&value),
                    &format!("Agent '{local_name}' toolBudget"),
                )
            }) {
                Ok(budget) => budget,
                Err(message) => {
                    tracing::warn!(
                        agent = %local_name,
                        path = %file_path.display(),
                        "{message} — skipping this agent file"
                    );
                    return None;
                }
            }
        }
    };

    // `async:` — pi `agents.ts:1541-1546`: strictly `"true"`/`"false"`; anything else is an ERROR
    // upstream. Same `[CYRUP-DELTA]` as `toolBudget` above: a per-file skip + warn instead of
    // aborting the whole directory scan.
    let default_async = match parsed.get("async") {
        None => None,
        Some("true") => Some(true),
        Some("false") => Some(false),
        Some(_) => {
            tracing::warn!(
                agent = %local_name,
                path = %file_path.display(),
                "Agent '{local_name}' has invalid async frontmatter; expected true or false. — skipping this agent file"
            );
            return None;
        }
    };

    // `timeoutMs:` — pi `agents.ts:1547-1554`: `Number.isInteger(parsed) && parsed > 0`, else error.
    let default_timeout_ms = match parsed.get("timeoutMs") {
        None => None,
        Some(raw) => match raw.trim().parse::<u64>() {
            Ok(ms) if ms > 0 => Some(ms),
            _ => {
                tracing::warn!(
                    agent = %local_name,
                    path = %file_path.display(),
                    "Agent '{local_name}' has invalid timeoutMs frontmatter; expected a positive integer. — skipping this agent file"
                );
                return None;
            }
        },
    };

    // `memory:` — pi `agents.ts:1290` @ v0.43.0 / the same call at v0.34.0. `parseMemoryFrontmatter`
    // never errors: an illegal scope or a missing path simply declines the config.
    let memory = crate::discovery::agent_memory::parse_memory_frontmatter(parsed.get("memory"));
    // `disabled` is NOT an honored agent-FILE frontmatter field. pi's `loadAgentsFromDir`
    // (`agents.ts:1482-1644`) never reads `frontmatter.disabled` into `AgentConfig.disabled`, and
    // `disabled` is absent from `KNOWN_FIELDS` (`agent-serializer.ts:4-26`) — so a `disabled:` line in
    // an agent file is just an unknown extra field (round-tripped verbatim into `extra_fields`), NOT a
    // flag. An agent is disabled ONLY by a settings override (`subagents.disableBuiltins` /
    // `agentOverrides.<name>.disabled`), applied later by `merge.rs`. Parsing it here would let a
    // handcrafted file disable itself, which pi does not permit; leave it `None` at parse time.
    let disabled: Option<bool> = None;

    let present_fields: HashSet<String> = parsed.keys().map(str::to_string).collect();
    let extra_fields: BTreeMap<String, String> = parsed
        .fields
        .iter()
        .filter(|(k, _)| !is_known_field(k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    Some(AgentDefinition {
        name: runtime_name,
        local_name,
        package_name,
        description,
        aliases,
        tools,
        extensions,
        extensions_from_default: false,
        subagent_only_extensions,
        model,
        fallback_models,
        thinking,
        system_prompt_mode,
        inherit_project_context,
        inherit_skills,
        skills,
        default_reads,
        default_progress,
        output,
        completion_guard,
        interactive,
        max_subagent_depth,
        default_context,
        default_async,
        default_timeout_ms,
        memory,
        tool_budget,
        disabled,
        system_prompt_body: parsed.body,
        source,
        file_path: file_path.to_path_buf(),
        present_fields,
        extra_fields,
        override_info: None,
        model_source: None,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    // -----------------------------------------------------------------------------------------
    // Layer 1: parse_frontmatter_block direct ports of frontmatter.ts's own implicit contract
    // -----------------------------------------------------------------------------------------

    #[test]
    fn no_frontmatter_delimiter_returns_empty_map_and_whole_body() {
        let parsed = parse_frontmatter_block("Just a plain markdown file.\nNo frontmatter here.");
        assert!(parsed.fields.is_empty());
        assert_eq!(parsed.body, "Just a plain markdown file.\nNo frontmatter here.");
    }

    #[test]
    fn unclosed_frontmatter_block_returns_empty_map_and_whole_body() {
        let parsed = parse_frontmatter_block("---\nname: worker\ndescription: Worker\n\nNo closing delimiter.");
        assert!(parsed.fields.is_empty());
    }

    #[test]
    fn flat_key_value_pairs_parse() {
        let parsed = parse_frontmatter_block(
            "---\nname: worker\ndescription: Worker\ntools: read, grep, find\n---\n\nDo work\n",
        );
        assert_eq!(parsed.get("name"), Some("worker"));
        assert_eq!(parsed.get("description"), Some("Worker"));
        assert_eq!(parsed.get("tools"), Some("read, grep, find"));
        assert_eq!(parsed.body, "Do work");
    }

    #[test]
    fn quoted_values_are_unwrapped() {
        let parsed = parse_frontmatter_block("---\nname: worker\ndescription: \"A worker agent\"\n---\n\nBody\n");
        assert_eq!(parsed.get("description"), Some("A worker agent"));
    }

    #[test]
    fn single_quoted_values_are_unwrapped() {
        let parsed = parse_frontmatter_block("---\nname: worker\ndescription: 'A worker agent'\n---\n\nBody\n");
        assert_eq!(parsed.get("description"), Some("A worker agent"));
    }

    #[test]
    fn crlf_line_endings_are_normalized() {
        let parsed = parse_frontmatter_block("---\r\nname: worker\r\ndescription: Worker\r\n---\r\n\r\nBody\r\n");
        assert_eq!(parsed.get("name"), Some("worker"));
        assert_eq!(parsed.body, "Body");
    }

    #[test]
    fn one_level_block_indent_value_is_preserved_with_embedded_newlines() {
        // The pi-subagents source fixture (agent-frontmatter.test.ts, "preserves nested
        // permission YAML blocks through discovery and serialization").
        let content = "---\nname: worker\ndescription: Worker\ntools: bash,read,write\npermission:\n  \"*\": ask\n  read: allow\n  bash:\n    \"*\": ask\n    \"git *\": allow\n---\n\nDo work\n";
        let parsed = parse_frontmatter_block(content);
        assert_eq!(
            parsed.get("permission"),
            Some("\"*\": ask\nread: allow\nbash:\n  \"*\": ask\n  \"git *\": allow")
        );
    }

    #[test]
    fn comment_and_blank_lines_inside_frontmatter_are_ignored() {
        let content = "---\nname: worker\n\ndescription: Worker\n---\n\nBody\n";
        let parsed = parse_frontmatter_block(content);
        assert_eq!(parsed.get("name"), Some("worker"));
        assert_eq!(parsed.get("description"), Some("Worker"));
    }

    #[test]
    fn repeated_key_last_occurrence_wins() {
        let content = "---\nname: first\nname: second\ndescription: Worker\n---\n\nBody\n";
        let parsed = parse_frontmatter_block(content);
        assert_eq!(parsed.get("name"), Some("second"));
    }

    #[test]
    fn block_value_at_end_of_frontmatter_with_no_trailing_flat_key_is_flushed() {
        let content = "---\nname: worker\ndescription: Worker\npermission:\n  read: allow\n---\n\nBody\n";
        let parsed = parse_frontmatter_block(content);
        assert_eq!(parsed.get("permission"), Some("read: allow"));
    }

    #[test]
    fn indented_orphan_key_value_line_is_not_parsed_as_a_field() {
        // BUG fix: pi matches `^([\w-]+):` against the RAW line (`frontmatter.ts:61`), so an INDENTED
        // `key: value` line that is not a block-value continuation is ignored — it never becomes a
        // field. (Matching against a trimmed copy, as the old code did, wrongly promoted it.) Here
        // `description: Worker` has a non-empty value so it does NOT open a block, and the following
        // indented `  orphan: value` line therefore fails the column-0 anchor and is dropped.
        let content = "---\nname: worker\ndescription: Worker\n  orphan: value\n---\n\nBody\n";
        let parsed = parse_frontmatter_block(content);
        assert_eq!(parsed.get("name"), Some("worker"));
        assert_eq!(parsed.get("description"), Some("Worker"));
        assert_eq!(parsed.get("orphan"), None, "an indented orphan line must not become a field");
        assert!(!parsed.fields.iter().any(|(k, _)| k == "orphan"));
    }

    #[test]
    fn deeply_indented_orphan_line_after_a_flat_key_is_ignored_not_captured_as_block() {
        // A flat key with a NON-EMPTY value never opens a block, so a subsequent indented line is a
        // true orphan (ignored), not a continuation. Contrast with the `permission:` (empty-value)
        // case, which DOES open a block.
        let content = "---\nname: worker\ndescription: Worker\nmodel: anthropic/claude\n      stray: 1\n---\n\nBody\n";
        let parsed = parse_frontmatter_block(content);
        assert_eq!(parsed.get("model"), Some("anthropic/claude"));
        assert!(!parsed.fields.iter().any(|(k, _)| k == "stray"));
    }

    // -----------------------------------------------------------------------------------------
    // Layer 2: parse_agent_file — valid full frontmatter (pi-subagents scout.md/worker.md shape)
    // -----------------------------------------------------------------------------------------

    #[test]
    fn valid_full_frontmatter_parses_every_field() {
        let content = "---\nname: scout\ndescription: Fast codebase recon\ntools: read, grep, find, ls, bash, write, intercom\nthinking: low\nsystemPromptMode: replace\ninheritProjectContext: true\ninheritSkills: false\noutput: context.md\ndefaultProgress: true\nmaxSubagentDepth: 2\ncompletionGuard: false\nfallbackModels: openai/gpt-5-mini, anthropic/claude-sonnet-4\ndefaultContext: fork\n---\n\nYou are a scouting subagent.\n";
        let def = parse_agent_file(content, AgentSource::Builtin, Path::new("/agents/scout.md"))
            .expect("valid frontmatter must parse");

        assert_eq!(def.name, "scout");
        assert_eq!(def.local_name, "scout");
        assert_eq!(def.package_name, None);
        assert_eq!(def.description, "Fast codebase recon");
        assert_eq!(
            def.tools,
            Some(vec![
                ToolRef::Builtin("read".to_string()),
                ToolRef::Builtin("grep".to_string()),
                ToolRef::Builtin("find".to_string()),
                ToolRef::Builtin("ls".to_string()),
                ToolRef::Builtin("bash".to_string()),
                ToolRef::Builtin("write".to_string()),
                ToolRef::Builtin("intercom".to_string()),
            ])
        );
        assert_eq!(def.thinking, Some("low".to_string()));
        assert_eq!(def.system_prompt_mode, SystemPromptMode::Replace);
        assert!(def.inherit_project_context);
        assert!(!def.inherit_skills);
        assert_eq!(
            def.output,
            Some(OutputSpec {
                path: Some(PathBuf::from("context.md")),
                mode: None,
            })
        );
        assert_eq!(def.default_progress, Some(true));
        assert_eq!(def.max_subagent_depth, Some(2));
        assert_eq!(def.completion_guard, Some(false));
        assert_eq!(
            def.fallback_models,
            vec![ModelId::from("openai/gpt-5-mini"), ModelId::from("anthropic/claude-sonnet-4")]
        );
        assert_eq!(def.default_context, Some(ContextMode::Fork));
        assert_eq!(def.system_prompt_body, "You are a scouting subagent.");
        assert_eq!(def.source, AgentSource::Builtin);
        assert_eq!(def.file_path, PathBuf::from("/agents/scout.md"));
        assert!(def.present_fields.contains("thinking"));
        assert!(def.present_fields.contains("name"));
        assert!(def.extra_fields.is_empty());
    }

    #[test]
    fn mcp_prefixed_tools_split_into_mcp_variant() {
        let content = "---\nname: worker\ndescription: Worker\ntools: read, mcp:filesystem.list, edit\n---\n\nBody\n";
        let def = parse_agent_file(content, AgentSource::Project, Path::new("/a.md")).expect("parses");
        assert_eq!(
            def.tools,
            Some(vec![
                ToolRef::Builtin("read".to_string()),
                ToolRef::Mcp("filesystem.list".to_string()),
                ToolRef::Builtin("edit".to_string()),
            ])
        );
    }

    #[test]
    fn packaged_agent_gets_qualified_runtime_name() {
        let content = "---\nname: scout\npackage: code-analysis\ndescription: Fast recon\n---\n\nInspect code\n";
        let def = parse_agent_file(content, AgentSource::Package, Path::new("/pkg/scout.md")).expect("parses");
        assert_eq!(def.name, "code-analysis.scout");
        assert_eq!(def.local_name, "scout");
        assert_eq!(def.package_name, Some("code-analysis".to_string()));
    }

    #[test]
    fn package_name_is_normalized_before_validation() {
        // pi-subagents' own "normalizes package frontmatter consistently" fixture.
        let content = "---\nname: scout\npackage: Code Analysis!\ndescription: Fast recon\n---\n\nInspect\n";
        let def = parse_agent_file(content, AgentSource::Project, Path::new("/a.md")).expect("parses");
        assert_eq!(def.package_name, Some("code-analysis".to_string()));
        assert_eq!(def.name, "code-analysis.scout");
    }

    #[test]
    fn permission_style_nested_block_round_trips_into_extra_fields() {
        let content = "---\nname: worker\ndescription: Worker\ntools: bash,read,write\npermission:\n  \"*\": ask\n  read: allow\n  bash:\n    \"*\": ask\n    \"git *\": allow\n---\n\nDo work\n";
        let def = parse_agent_file(content, AgentSource::Project, Path::new("/w.md")).expect("parses");
        assert_eq!(
            def.extra_fields.get("permission").map(String::as_str),
            Some("\"*\": ask\nread: allow\nbash:\n  \"*\": ask\n  \"git *\": allow")
        );
        assert!(def.present_fields.contains("permission"));
    }

    // -----------------------------------------------------------------------------------------
    // R-SA-005: missing-required-field silent skip
    // -----------------------------------------------------------------------------------------

    #[test]
    fn missing_name_is_silently_skipped() {
        let content = "---\ndescription: No name here\n---\n\nBody\n";
        assert!(parse_agent_file(content, AgentSource::Project, Path::new("/x.md")).is_none());
    }

    #[test]
    fn missing_description_is_silently_skipped() {
        let content = "---\nname: worker\n---\n\nBody\n";
        assert!(parse_agent_file(content, AgentSource::Project, Path::new("/x.md")).is_none());
    }

    #[test]
    fn empty_name_value_is_treated_as_missing() {
        let content = "---\nname:\ndescription: Worker\n---\n\nBody\n";
        // An empty-valued `name:` line starts a block-value capture per the grammar (layer 1);
        // with no following indented continuation line it flushes to an empty string, which the
        // required-field check (JS-falsy semantics) treats identically to "absent".
        assert!(parse_agent_file(content, AgentSource::Project, Path::new("/x.md")).is_none());
    }

    #[test]
    fn no_frontmatter_at_all_is_silently_skipped() {
        let content = "# Just a heading\n\nSome prose, no frontmatter block.\n";
        assert!(parse_agent_file(content, AgentSource::Project, Path::new("/x.md")).is_none());
    }

    #[test]
    fn missing_required_field_does_not_panic_and_other_files_are_unaffected() {
        // Directly exercises R-SA-005's "discovery of other files MUST continue unaffected" via
        // two independent parse calls — this module has no discovery-loop state, so "continuing
        // unaffected" reduces to "this call returns None without panicking or otherwise
        // poisoning shared state," which the type signature (`Option`, no shared mutable state)
        // already guarantees; this test pins that behavior for a bad-then-good pair.
        let bad = "---\ndescription: No name\n---\n\nBody\n";
        let good = "---\nname: worker\ndescription: Worker\n---\n\nBody\n";
        assert!(parse_agent_file(bad, AgentSource::Project, Path::new("/bad.md")).is_none());
        let ok = parse_agent_file(good, AgentSource::Project, Path::new("/good.md"));
        assert!(ok.is_some());
    }

    // -----------------------------------------------------------------------------------------
    // R-SA-006: invalid package identifier skips the WHOLE file
    // -----------------------------------------------------------------------------------------

    #[test]
    fn invalid_package_identifier_skips_whole_file() {
        // pi-subagents' own "skips invalid package frontmatter that cannot be normalized"
        // fixture: `package: !!!` normalizes to an empty string, which fails validation.
        let content = "---\nname: scout\npackage: !!!\ndescription: Fast recon\n---\n\nInspect\n";
        assert!(parse_agent_file(content, AgentSource::Project, Path::new("/x.md")).is_none());
    }

    #[test]
    fn package_name_that_normalizes_to_empty_skips_whole_file() {
        let content = "---\nname: scout\npackage: \"   ---   \"\ndescription: Fast recon\n---\n\nInspect\n";
        assert!(parse_agent_file(content, AgentSource::Project, Path::new("/x.md")).is_none());
    }

    #[test]
    fn absent_package_field_is_fine_and_parses_unqualified() {
        let content = "---\nname: scout\ndescription: Fast recon\n---\n\nInspect\n";
        let def = parse_agent_file(content, AgentSource::Project, Path::new("/x.md")).expect("parses");
        assert_eq!(def.package_name, None);
        assert_eq!(def.name, "scout");
    }

    #[test]
    fn empty_package_field_is_treated_as_absent_not_invalid() {
        // Source: `value === "" -> { packageName: undefined }` (not an error path).
        let content = "---\nname: scout\npackage:\ndescription: Fast recon\n---\n\nInspect\n";
        let def = parse_agent_file(content, AgentSource::Project, Path::new("/x.md")).expect("parses");
        assert_eq!(def.package_name, None);
    }

    // -----------------------------------------------------------------------------------------
    // R-SA-018: name-sensitive systemPromptMode/inheritProjectContext defaults
    // -----------------------------------------------------------------------------------------

    #[test]
    fn ordinary_agent_defaults_to_replace_mode_with_no_inherited_context() {
        let content = "---\nname: worker\ndescription: Worker\n---\n\nDo work\n";
        let def = parse_agent_file(content, AgentSource::Project, Path::new("/w.md")).expect("parses");
        assert_eq!(def.system_prompt_mode, SystemPromptMode::Replace);
        assert!(!def.inherit_project_context);
        assert!(!def.inherit_skills);
    }

    #[test]
    fn delegate_local_name_defaults_to_append_mode_with_inherited_context() {
        let content = "---\nname: delegate\ndescription: Delegate\n---\n\nDo work\n";
        let def = parse_agent_file(content, AgentSource::Builtin, Path::new("/delegate.md")).expect("parses");
        assert_eq!(def.system_prompt_mode, SystemPromptMode::Append);
        assert!(def.inherit_project_context);
        assert!(!def.inherit_skills);
    }

    #[test]
    fn packaged_delegate_still_gets_delegate_defaults_via_local_name() {
        // R-SA-018: "This default MUST still apply even when the agent is packaged — a packaged
        // agent literally locally named `delegate` gets the same defaults as the unpackaged
        // builtin."
        let content = "---\nname: delegate\npackage: acme\ndescription: Delegate\n---\n\nDo work\n";
        let def = parse_agent_file(content, AgentSource::Package, Path::new("/pkg/delegate.md")).expect("parses");
        assert_eq!(def.name, "acme.delegate");
        assert_eq!(def.system_prompt_mode, SystemPromptMode::Append);
        assert!(def.inherit_project_context);
    }

    #[test]
    fn explicit_system_prompt_mode_overrides_name_sensitive_default() {
        let content = "---\nname: delegate\ndescription: Delegate\nsystemPromptMode: replace\n---\n\nDo work\n";
        let def = parse_agent_file(content, AgentSource::Project, Path::new("/d.md")).expect("parses");
        assert_eq!(def.system_prompt_mode, SystemPromptMode::Replace);
    }

    #[test]
    fn explicit_inherit_project_context_overrides_name_sensitive_default() {
        let content = "---\nname: delegate\ndescription: Delegate\ninheritProjectContext: false\n---\n\nDo work\n";
        let def = parse_agent_file(content, AgentSource::Project, Path::new("/d.md")).expect("parses");
        assert!(!def.inherit_project_context);
    }

    // -----------------------------------------------------------------------------------------
    // Malformed-value handling: individual bad field values never abort the whole file
    // -----------------------------------------------------------------------------------------

    #[test]
    fn unknown_thinking_string_survives_and_does_not_skip_the_file() {
        // pi `thinking` is an OPEN string: an arbitrary (future/provider-specific) value is preserved
        // verbatim, never dropped, and never causes the file to be skipped.
        let content = "---\nname: worker\ndescription: Worker\nthinking: super-duper\n---\n\nBody\n";
        let def = parse_agent_file(content, AgentSource::Project, Path::new("/w.md")).expect("parses");
        assert_eq!(def.thinking, Some("super-duper".to_string()));
        assert!(def.present_fields.contains("thinking"));
        // It is a KNOWN field, so it does NOT leak into extra_fields even though it is unrecognized.
        assert!(!def.extra_fields.contains_key("thinking"));
    }

    #[test]
    fn thinking_off_is_preserved_as_explicit_off_distinct_from_unset() {
        // Explicit `off` must survive as `Some("off")` — distinct from an agent that says nothing
        // (`None`). The old closed 5-level enum conflated both to `None`.
        let off = parse_agent_file(
            "---\nname: worker\ndescription: Worker\nthinking: off\n---\n\nBody\n",
            AgentSource::Project,
            Path::new("/w.md"),
        )
        .expect("parses");
        assert_eq!(off.thinking, Some("off".to_string()));

        let unset = parse_agent_file(
            "---\nname: worker\ndescription: Worker\n---\n\nBody\n",
            AgentSource::Project,
            Path::new("/w.md"),
        )
        .expect("parses");
        assert_eq!(unset.thinking, None);
        assert_ne!(off.thinking, unset.thinking, "off must not be conflated with unset");
    }

    #[test]
    fn empty_thinking_value_collapses_to_unset() {
        // pi treats an empty `thinking` value as falsy/no-op; it collapses to `None` (unset), while
        // the key's literal presence is still tracked for the serializer's preserve round-trip.
        let content = "---\nname: worker\ndescription: Worker\nthinking:\n---\n\nBody\n";
        let def = parse_agent_file(content, AgentSource::Project, Path::new("/w.md")).expect("parses");
        assert_eq!(def.thinking, None);
        assert!(def.present_fields.contains("thinking"));
    }

    #[test]
    fn disabled_in_an_agent_file_is_an_unknown_extra_field_not_an_honored_flag() {
        // pi never reads `frontmatter.disabled` into `AgentConfig.disabled` (only settings disable an
        // agent); `disabled` is not a KNOWN field, so a `disabled:` line round-trips into extra_fields
        // and leaves the typed `disabled` flag untouched (`None`) — a handcrafted file cannot disable
        // itself.
        let content = "---\nname: worker\ndescription: Worker\ndisabled: true\n---\n\nBody\n";
        let def = parse_agent_file(content, AgentSource::Project, Path::new("/w.md")).expect("parses");
        assert_eq!(def.disabled, None, "disabled: in a file must NOT set the honored flag");
        assert_eq!(
            def.extra_fields.get("disabled").map(String::as_str),
            Some("true"),
            "disabled: is round-tripped as an unknown extra field"
        );
        assert!(def.present_fields.contains("disabled"));
    }

    #[test]
    fn negative_max_subagent_depth_is_dropped_not_fatal() {
        let content = "---\nname: worker\ndescription: Worker\nmaxSubagentDepth: -1\n---\n\nBody\n";
        let def = parse_agent_file(content, AgentSource::Project, Path::new("/w.md")).expect("parses");
        assert_eq!(def.max_subagent_depth, None);
    }

    #[test]
    fn non_numeric_max_subagent_depth_is_dropped_not_fatal() {
        let content = "---\nname: worker\ndescription: Worker\nmaxSubagentDepth: soon\n---\n\nBody\n";
        let def = parse_agent_file(content, AgentSource::Project, Path::new("/w.md")).expect("parses");
        assert_eq!(def.max_subagent_depth, None);
    }

    #[test]
    fn unrecognized_boolean_value_falls_through_to_default_not_fatal() {
        let content = "---\nname: worker\ndescription: Worker\ninheritSkills: maybe\n---\n\nBody\n";
        let def = parse_agent_file(content, AgentSource::Project, Path::new("/w.md")).expect("parses");
        assert!(!def.inherit_skills);
    }

    #[test]
    fn unrecognized_system_prompt_mode_falls_back_to_name_sensitive_default() {
        let content = "---\nname: worker\ndescription: Worker\nsystemPromptMode: sideways\n---\n\nBody\n";
        let def = parse_agent_file(content, AgentSource::Project, Path::new("/w.md")).expect("parses");
        assert_eq!(def.system_prompt_mode, SystemPromptMode::Replace);
    }

    #[test]
    fn unrecognized_default_context_falls_back_to_none() {
        let content = "---\nname: worker\ndescription: Worker\ndefaultContext: sideways\n---\n\nBody\n";
        let def = parse_agent_file(content, AgentSource::Project, Path::new("/w.md")).expect("parses");
        assert_eq!(def.default_context, None);
    }

    // -----------------------------------------------------------------------------------------
    // Comma-separated list fields
    // -----------------------------------------------------------------------------------------

    #[test]
    fn skill_singular_and_skills_plural_are_aliases_singular_wins_when_both_present() {
        let content = "---\nname: worker\ndescription: Worker\nskill: one, two\nskills: three, four\n---\n\nBody\n";
        let def = parse_agent_file(content, AgentSource::Project, Path::new("/w.md")).expect("parses");
        assert_eq!(def.skills, vec!["one".to_string(), "two".to_string()]);
    }

    #[test]
    fn skills_plural_used_when_singular_absent() {
        let content = "---\nname: worker\ndescription: Worker\nskills: alpha, beta\n---\n\nBody\n";
        let def = parse_agent_file(content, AgentSource::Project, Path::new("/w.md")).expect("parses");
        assert_eq!(def.skills, vec!["alpha".to_string(), "beta".to_string()]);
    }

    #[test]
    fn subagent_only_extensions_parses_paths_with_slashes() {
        let content = "---\nname: worker\ndescription: Worker\nsubagentOnlyExtensions: ./tools/child-search.ts, /opt/pi/child-only.ts\n---\n\nBody\n";
        let def = parse_agent_file(content, AgentSource::Project, Path::new("/w.md")).expect("parses");
        assert_eq!(
            def.subagent_only_extensions,
            vec!["./tools/child-search.ts".to_string(), "/opt/pi/child-only.ts".to_string()]
        );
    }

    #[test]
    fn extensions_none_vs_empty_vs_populated_tri_state() {
        let no_field = parse_agent_file(
            "---\nname: w\ndescription: W\n---\n\nB\n",
            AgentSource::Project,
            Path::new("/w.md"),
        )
        .expect("parses");
        assert_eq!(no_field.extensions, None);

        let empty_field = parse_agent_file(
            "---\nname: w\ndescription: W\nextensions:\n---\n\nB\n",
            AgentSource::Project,
            Path::new("/w.md"),
        )
        .expect("parses");
        assert_eq!(empty_field.extensions, Some(Vec::new()));

        let populated = parse_agent_file(
            "---\nname: w\ndescription: W\nextensions: foo, bar\n---\n\nB\n",
            AgentSource::Project,
            Path::new("/w.md"),
        )
        .expect("parses");
        assert_eq!(populated.extensions, Some(vec!["foo".to_string(), "bar".to_string()]));
    }

    // -----------------------------------------------------------------------------------------
    // G103: an explicitly-empty `tools:` is NOT the same as an absent one (pi `agents.ts:1610`)
    // -----------------------------------------------------------------------------------------

    #[test]
    fn an_explicitly_empty_tools_key_parses_to_an_empty_allowlist_not_to_none() {
        // pi: `parseFrontmatterList("")` is `[]`, `splitToolList([])` is `{ tools: [] }`, and
        // `...(rawTools !== undefined ? { tools } : {})` therefore CARRIES that empty list onto the
        // agent. `None` means "no allowlist restriction"; `Some(vec![])` means "no tools".
        for content in [
            "---\nname: scribe\ndescription: Scribe\ntools:\n---\n\nBody\n",
            "---\nname: scribe\ndescription: Scribe\ntools: \"\"\n---\n\nBody\n",
        ] {
            let def = parse_agent_file(content, AgentSource::Project, Path::new("/s.md"))
                .expect("parses");
            assert_eq!(
                def.tools,
                Some(Vec::new()),
                "an explicitly-empty `tools:` must be an EMPTY allowlist, not `None`; input was \
                 {content:?}"
            );
            assert!(def.present_fields.contains("tools"));
        }
    }

    #[test]
    fn an_absent_tools_key_parses_to_none() {
        // The MIRROR of the case above — the distinction only exists if BOTH sides hold.
        let content = "---\nname: scribe\ndescription: Scribe\n---\n\nBody\n";
        let def = parse_agent_file(content, AgentSource::Project, Path::new("/s.md")).expect("parses");
        assert_eq!(def.tools, None);
        assert!(!def.present_fields.contains("tools"));
    }

    // -----------------------------------------------------------------------------------------
    // present_fields / extra_fields bookkeeping (R-SA-010's fill-unset-only precondition)
    // -----------------------------------------------------------------------------------------

    #[test]
    fn present_fields_tracks_exactly_the_literal_frontmatter_keys() {
        let content = "---\nname: worker\ndescription: Worker\nthinking: high\n---\n\nBody\n";
        let def = parse_agent_file(content, AgentSource::Project, Path::new("/w.md")).expect("parses");
        assert!(def.present_fields.contains("name"));
        assert!(def.present_fields.contains("description"));
        assert!(def.present_fields.contains("thinking"));
        assert!(!def.present_fields.contains("model"));
        assert!(!def.present_fields.contains("tools"));
    }

    #[test]
    fn unknown_keys_round_trip_into_extra_fields_and_are_also_present() {
        let content = "---\nname: worker\ndescription: Worker\ncustomVendorField: some-value\n---\n\nBody\n";
        let def = parse_agent_file(content, AgentSource::Project, Path::new("/w.md")).expect("parses");
        assert_eq!(
            def.extra_fields.get("customVendorField").map(String::as_str),
            Some("some-value")
        );
        assert!(def.present_fields.contains("customVendorField"));
    }

    #[test]
    fn interactive_is_parsed_into_typed_field_and_never_dropped_from_extra_fields_expectation() {
        // func-SA §4.1: `interactive` is parsed but unenforced in v1 — it MUST still be typed and
        // round-trippable, never silently dropped. Since it IS a known field, it is captured in
        // the typed `interactive` slot (not `extra_fields`, which is reserved for genuinely
        // unknown keys) — this test pins that it is not lost either way.
        let content = "---\nname: worker\ndescription: Worker\ninteractive: true\n---\n\nBody\n";
        let def = parse_agent_file(content, AgentSource::Project, Path::new("/w.md")).expect("parses");
        assert_eq!(def.interactive, Some(true));
        assert!(!def.extra_fields.contains_key("interactive"));
        assert!(def.present_fields.contains("interactive"));
    }

    #[test]
    fn known_fields_never_leak_into_extra_fields() {
        let content = "---\nname: worker\ndescription: Worker\nmodel: anthropic/claude-sonnet-4\nthinking: high\ncompletionGuard: true\n---\n\nBody\n";
        let def = parse_agent_file(content, AgentSource::Project, Path::new("/w.md")).expect("parses");
        for key in KNOWN_FIELDS {
            assert!(
                !def.extra_fields.contains_key(*key),
                "known field {key} must not appear in extra_fields"
            );
        }
    }

    // -----------------------------------------------------------------------------------------
    // Real pi-subagents builtin fixtures, inlined verbatim (byte-for-byte content, arch-SA §12
    // item 2 compatibility intent) rather than `include_str!`-ed from outside the crate/workspace
    // boundary, so this test module has no dependency on a sibling `pi-subagents` checkout being
    // present at a fixed relative path in every build/CI environment.
    // -----------------------------------------------------------------------------------------

    const SCOUT_MD: &str = "---\nname: scout\ndescription: Fast codebase recon that returns compressed context for handoff\ntools: read, grep, find, ls, bash, write, intercom\nthinking: low\nsystemPromptMode: replace\ninheritProjectContext: true\ninheritSkills: false\noutput: context.md\ndefaultProgress: true\n---\n\nYou are a scouting subagent running inside pi.\n";

    const WORKER_MD: &str = "---\nname: worker\ndescription: Implementation agent for normal tasks and approved oracle handoffs\nthinking: high\nsystemPromptMode: replace\ninheritProjectContext: true\ninheritSkills: false\ntools: read, grep, find, ls, bash, edit, write, contact_supervisor\ndefaultContext: fork\ndefaultReads: context.md, plan.md\ndefaultProgress: true\n---\n\nYou are `worker`: the implementation subagent.\n";

    const DELEGATE_MD: &str = "---\nname: delegate\ndescription: Lightweight subagent that inherits the parent model with no default reads\nsystemPromptMode: append\ninheritProjectContext: true\ntools: read, grep, find, ls, bash, edit, write, contact_supervisor\ninheritSkills: false\n---\n\nYou are a delegated agent. Execute the assigned task using the provided tools.\n";

    #[test]
    fn real_scout_md_fixture_parses_correctly() {
        let def = parse_agent_file(SCOUT_MD, AgentSource::Builtin, Path::new("scout.md"))
            .expect("real scout.md must parse to Some");
        assert_eq!(def.name, "scout");
        assert_eq!(def.thinking, Some("low".to_string()));
        assert!(def.inherit_project_context);
        assert_eq!(def.default_progress, Some(true));
        assert_eq!(
            def.output,
            Some(OutputSpec {
                path: Some(PathBuf::from("context.md")),
                mode: None,
            })
        );
    }

    #[test]
    fn real_worker_md_fixture_parses_with_fork_default_context() {
        let def = parse_agent_file(WORKER_MD, AgentSource::Builtin, Path::new("worker.md")).expect("parses");
        assert_eq!(def.name, "worker");
        assert_eq!(def.default_context, Some(ContextMode::Fork));
        assert_eq!(
            def.default_reads,
            Some(vec![PathBuf::from("context.md"), PathBuf::from("plan.md")])
        );
    }

    #[test]
    fn real_delegate_md_fixture_gets_name_sensitive_defaults() {
        let def = parse_agent_file(DELEGATE_MD, AgentSource::Builtin, Path::new("delegate.md")).expect("parses");
        assert_eq!(def.system_prompt_mode, SystemPromptMode::Append);
        assert!(def.inherit_project_context);
    }

    // -----------------------------------------------------------------------------------------
    // Folded block scalars (`>` / `>-`) — `frontmatter.ts::foldBlock` (v0.43.0 `frontmatter.ts:12-40`).
    //
    // Every expected value below was produced by executing a transliteration of that exact
    // upstream file (Python's `re` reproduces the `^`+MULTILINE and greedy-`[ \t]+`+`(?=\S)`
    // semantics the TS relies on) against the same fixture strings.
    // -----------------------------------------------------------------------------------------

    #[test]
    fn folded_scalar_folds_lines_into_one_paragraph_instead_of_storing_the_indicator() {
        // Before folded-scalar support this stored the LITERAL string ">" as the description,
        // which is what the delegate tool schema and `/agents` then showed the user.
        let parsed = parse_frontmatter_block(
            "---\nname: worker\ndescription: >\n  Reviews Rust changes for\n  correctness and style.\ntools: read\n---\n\nBody\n",
        );
        assert_eq!(
            parsed.get("description"),
            Some("Reviews Rust changes for correctness and style.")
        );
        assert_eq!(parsed.get("tools"), Some("read"));
        assert_eq!(parsed.body, "Body");
    }

    #[test]
    fn folded_scalar_strip_chomp_indicator_is_accepted_too() {
        let parsed =
            parse_frontmatter_block("---\nname: worker\ndescription: >-\n  one\n  two\n---\n\nBody\n");
        assert_eq!(parsed.get("description"), Some("one two"));
    }

    #[test]
    fn folded_scalar_blank_line_separates_paragraphs() {
        let parsed = parse_frontmatter_block(
            "---\nname: worker\ndescription: >\n  para one line a\n  para one line b\n\n  para two\n---\n\nBody\n",
        );
        assert_eq!(
            parsed.get("description"),
            Some("para one line a para one line b\npara two")
        );
    }

    #[test]
    fn folded_scalar_preserves_line_breaks_around_more_indented_lines() {
        let parsed = parse_frontmatter_block(
            "---\nname: worker\ndescription: >\n  normal line\n    more indented\n  back to normal\n---\n\nBody\n",
        );
        assert_eq!(
            parsed.get("description"),
            Some("normal line\n  more indented\nback to normal")
        );
    }

    #[test]
    fn folded_scalar_with_a_leading_blank_line_still_dedents_the_block() {
        // The indent-anchor bug: the old prefix logic read the block's FIRST characters, which for
        // a block starting with a blank line is `\n` — an empty prefix, so nothing was dedented and
        // every content line then looked "more indented" to the folder, yielding
        // "first content line\n  second content line" instead of a single folded paragraph.
        let parsed = parse_frontmatter_block(
            "---\nname: worker\ndescription: >\n\n  first content line\n  second content line\n---\n\nBody\n",
        );
        assert_eq!(
            parsed.get("description"),
            Some("first content line second content line")
        );
    }

    #[test]
    fn a_quoted_gt_is_a_literal_string_not_a_folded_block_indicator() {
        let parsed = parse_frontmatter_block("---\nname: worker\ndescription: \">\"\n---\n\nBody\n");
        assert_eq!(parsed.get("description"), Some(">"));
    }

    #[test]
    fn a_plain_empty_valued_block_is_still_stored_verbatim_not_folded() {
        let parsed = parse_frontmatter_block(
            "---\nname: worker\ndescription: Worker\npermission:\n  \"*\": ask\n  bash:\n    \"*\": ask\n---\n\nBody\n",
        );
        assert_eq!(parsed.get("permission"), Some("\"*\": ask\nbash:\n  \"*\": ask"));
    }

    // -----------------------------------------------------------------------------------------
    // SUBA-052 — LITERAL block scalars (`|` / `|-`), pi `agents/frontmatter.ts:86,91,125,126`
    // @v0.47.1 (`a4fc59a`, released v0.46.0).
    // -----------------------------------------------------------------------------------------

    /// THE USER ACTION: an author writes the single most common YAML idiom for a multi-line
    /// description. Before the fix `strip_matching_quotes` yielded `(false, "|")` — neither empty
    /// nor folded — so the parser stored the one-character string `"|"` and then silently discarded
    /// every indented body line (they fail the `^([\w-]+):` key match). The agent listed with a
    /// description of `|`, matched nothing in proactive-skill selection, and any multi-line key that
    /// feeds behaviour ran with a one-character value.
    ///
    /// One table over `|`, `|-`, `>`, `>-` and a plain scalar, exactly as the item's Verify asks.
    #[test]
    fn block_scalar_indicators_are_parsed_per_pis_folded_vs_literal_split() {
        // (indicator, expected description)
        let cases: &[(&str, &str)] = &[
            // LITERAL: stored verbatim, newlines preserved. Red before the fix — was `"|"`.
            ("|", "line one\nline two\nline three"),
            ("|-", "line one\nline two\nline three"),
            // FOLDED: unchanged behaviour, lines joined into one paragraph.
            (">", "line one line two line three"),
            (">-", "line one line two line three"),
        ];
        for (indicator, expected) in cases {
            let parsed = parse_frontmatter_block(&format!(
                "---\nname: worker\ndescription: {indicator}\n  line one\n  line two\n  line three\ntools: read\n---\n\nBody\n"
            ));
            assert_eq!(
                parsed.get("description"),
                Some(*expected),
                "indicator {indicator:?}"
            );
            // The block must not swallow the following flat key.
            assert_eq!(parsed.get("tools"), Some("read"), "indicator {indicator:?}");
            assert_eq!(parsed.body, "Body", "indicator {indicator:?}");
        }

        // A PLAIN scalar is untouched by either arm.
        let plain = parse_frontmatter_block("---\nname: worker\ndescription: just text\n---\n\nBody\n");
        assert_eq!(plain.get("description"), Some("just text"));
    }

    /// pi gates BOTH indicators on `!isQuoted` (`:124-125`), so a quoted pipe is the literal
    /// one-character string — the mirror of the existing `">"` test.
    #[test]
    fn a_quoted_pipe_is_a_literal_string_not_a_literal_block_indicator() {
        let parsed = parse_frontmatter_block("---\nname: worker\ndescription: \"|\"\n---\n\nBody\n");
        assert_eq!(parsed.get("description"), Some("|"));
    }

    /// pi `:91` folds `currentLiteral` into the blank-line continuation test alongside
    /// `currentFolded`, so a literal block keeps its interior blank lines — which for a literal
    /// scalar are content, not a paragraph separator.
    #[test]
    fn a_literal_block_keeps_its_interior_blank_lines() {
        let parsed = parse_frontmatter_block(
            "---\nname: worker\ndescription: |\n  para one\n\n  para two\ntools: read\n---\n\nBody\n",
        );
        assert_eq!(parsed.get("description"), Some("para one\n\npara two"));
        assert_eq!(parsed.get("tools"), Some("read"));
    }

    // -----------------------------------------------------------------------------------------
    // Block lists — `frontmatter.ts::parseFrontmatterList` (v0.43.0 `frontmatter.ts:46-57`).
    // -----------------------------------------------------------------------------------------

    #[test]
    fn parse_frontmatter_list_accepts_comma_and_block_syntax() {
        assert_eq!(
            parse_frontmatter_list(Some("read, grep, find")),
            Some(vec!["read".into(), "grep".into(), "find".into()])
        );
        assert_eq!(
            parse_frontmatter_list(Some("- read\n- grep")),
            Some(vec!["read".into(), "grep".into()])
        );
        assert_eq!(
            parse_frontmatter_list(Some("- read\n  - grep")),
            Some(vec!["read".into(), "grep".into()])
        );
        assert_eq!(
            parse_frontmatter_list(Some("- read, grep\n- find")),
            Some(vec!["read".into(), "grep".into(), "find".into()])
        );
    }

    #[test]
    fn parse_frontmatter_list_leaves_ordinary_hyphenated_values_intact() {
        // Only the standard `- ` marker (hyphen + whitespace) is a list marker.
        assert_eq!(
            parse_frontmatter_list(Some("-foo, bar-baz")),
            Some(vec!["-foo".into(), "bar-baz".into()])
        );
        assert_eq!(parse_frontmatter_list(Some("-")), Some(vec!["-".into()]));
    }

    #[test]
    fn parse_frontmatter_list_distinguishes_absent_from_empty() {
        assert_eq!(parse_frontmatter_list(None), None);
        assert_eq!(parse_frontmatter_list(Some("")), Some(Vec::new()));
    }

    #[test]
    fn agent_file_block_list_tools_become_real_tool_refs() {
        // The user-visible defect this fixes: a block-list `tools:` used to yield tool names
        // literally prefixed with "- ", so every tool in the list failed to resolve.
        let def = parse_agent_file(
            "---\nname: worker\ndescription: Worker\ntools:\n  - read\n  - grep\n  - mcp:github/search\n---\n\nBody\n",
            AgentSource::User,
            Path::new("worker.md"),
        )
        .expect("parses");
        assert_eq!(
            def.tools,
            Some(vec![
                ToolRef::Builtin("read".into()),
                ToolRef::Builtin("grep".into()),
                ToolRef::Mcp("github/search".into()),
            ])
        );
    }

    #[test]
    fn agent_file_block_list_skills_and_reads_are_parsed() {
        let def = parse_agent_file(
            "---\nname: worker\ndescription: Worker\nskills:\n  - alpha\n  - beta\ndefaultReads:\n  - context.md\n  - plan.md\nfallbackModels:\n  - a/one\n  - b/two\nextensions:\n  - ext-a\n---\n\nBody\n",
            AgentSource::User,
            Path::new("worker.md"),
        )
        .expect("parses");
        assert_eq!(def.skills, vec!["alpha".to_string(), "beta".to_string()]);
        assert_eq!(
            def.default_reads,
            Some(vec![PathBuf::from("context.md"), PathBuf::from("plan.md")])
        );
        assert_eq!(
            def.fallback_models,
            vec![ModelId::from("a/one"), ModelId::from("b/two")]
        );
        assert_eq!(def.extensions, Some(vec!["ext-a".to_string()]));
    }

    #[test]
    fn agent_file_folded_description_reaches_the_agent_definition() {
        let def = parse_agent_file(
            "---\nname: worker\ndescription: >\n  Reviews Rust changes for\n  correctness and style.\n---\n\nBody\n",
            AgentSource::User,
            Path::new("worker.md"),
        )
        .expect("parses");
        assert_eq!(def.description, "Reviews Rust changes for correctness and style.");
    }
}
