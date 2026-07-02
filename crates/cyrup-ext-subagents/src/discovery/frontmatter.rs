//! Hand-rolled YAML-subset frontmatter parser (func-SA §5.1 R-SA-005/006/018; arch-SA §6.2.3).
//!
//! This is a deliberate **line-oriented parser for the exact permissive grammar
//! `pi-subagents`' own `src/agents/frontmatter.ts::parseFrontmatter` implements** — flat
//! `key: value` pairs plus **one level** of block-indent values (an empty-valued `key:` line
//! followed by more-indented continuation lines, common-leading-whitespace-stripped and stored as
//! a single newline-joined string). It is **not** a general YAML parser: no arrays, no anchors,
//! no multiline scalar indicators (`|`/`>`), no nested-block-of-blocks. Per arch-SA §6.2.3 / §12
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
//! - Comma-separated list fields (`tools`, `defaultReads`, `skill`/`skills`, `fallbackModels`,
//!   `extensions`, `subagentOnlyExtensions`) split on `,`, trim each entry, drop empties.
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

use cyrup_core::{ModelId, ThinkingLevel};

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
];

fn is_known_field(key: &str) -> bool {
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
fn strip_matching_quotes(value: &str) -> &str {
    let bytes = value.as_bytes();
    if let (Some(&first), Some(&last)) = (bytes.first(), bytes.last())
        && bytes.len() >= 2
        && ((first == b'"' && last == b'"') || (first == b'\'' && last == b'\''))
    {
        // Safe: both quote chars are single-byte ASCII, so slicing at these positions never
        // splits a multi-byte UTF-8 sequence. `get(1..value.len() - 1)` avoids any raw indexing.
        if let Some(inner) = value.get(1..value.len() - 1) {
            return inner;
        }
    }
    value
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

/// Strip the common leading whitespace prefix from a set of raw block-continuation lines, then
/// join with `\n`. The "prefix" is taken from the **first** captured line's leading whitespace run
/// (mirrors source's `rawBlock.match(/^([ \t]+)/m)` taking the first line in the joined block that
/// has any leading whitespace, then stripping that exact prefix from every line via a global
/// multiline regex replace, and finally trimming one leading `\n` if the strip left one).
fn dedent_block(raw_lines: &[String]) -> String {
    let raw_block = raw_lines.join("\n");
    let prefix: String = raw_block
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    if prefix.is_empty() {
        return raw_block;
    }
    let stripped: String = raw_block
        .lines()
        .map(|line| line.strip_prefix(prefix.as_str()).unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n");
    // Mirror source's final `.replace(/^\n/, "")`: drop exactly one leading newline left over
    // from the strip, if present.
    stripped.strip_prefix('\n').map_or(stripped.clone(), str::to_string)
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

    let flush_block = |fields: &mut Vec<(String, String)>,
                        current_key: &mut Option<String>,
                        current_block_lines: &mut Vec<String>,
                        current_indent: &mut Option<usize>| {
        if let Some(key) = current_key.take() {
            let stripped = dedent_block(current_block_lines);
            fields.push((key, stripped));
        }
        current_block_lines.clear();
        *current_indent = None;
    };

    for line in frontmatter_block.split('\n') {
        let indent = first_non_whitespace_offset(line);
        let trimmed = line.trim();

        if current_key.is_some() && indent > current_indent.unwrap_or(0) {
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
        );

        let Some((key, raw_value)) = match_key_value(trimmed) else {
            // Non-matching line (comment, blank, malformed): silently ignored, matching source.
            continue;
        };
        let value = strip_matching_quotes(raw_value.trim());

        if value.is_empty() {
            // Empty-valued key: might start a block value; defer storing until we see indent.
            current_key = Some(key.to_string());
            current_block_lines = Vec::new();
            current_indent = Some(indent);
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
    );

    ParsedFrontmatter { fields, body }
}

// ---------------------------------------------------------------------------------------------
// Layer 2: agent-specific parsing (R-SA-005/006/018; func-SA §4.1)
// ---------------------------------------------------------------------------------------------

/// Split a comma-separated frontmatter list value into trimmed, non-empty entries — mirrors every
/// `frontmatter.<field>?.split(",").map((t) => t.trim()).filter(Boolean)` call site in source
/// (`tools`, `defaultReads`, `skill`/`skills`, `fallbackModels`, `extensions`,
/// `subagentOnlyExtensions`).
fn split_comma_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

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
fn parse_tool_refs(raw: &str) -> Vec<ToolRef> {
    split_comma_list(raw)
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

/// `thinking: <value>` -> `Option<ThinkingLevel>`. Accepts the five on-level strings
/// (case-sensitive, matching `ThinkingLevel`'s `rename_all = "camelCase"` wire form: `minimal`,
/// `low`, `medium`, `high`, `xhigh`) plus the literal `off`, which maps to `None` (no reasoning) —
/// mirroring `ModelThinkingLevel`'s off-inclusive value space at the frontmatter-string level even
/// though `AgentDefinition::thinking` itself is typed as `Option<ThinkingLevel>` (the on-level
/// subset only). An unrecognized string is treated as "not stated" (`None`) rather than aborting
/// the whole file — thinking is not one of R-SA-005's two required fields, and a malformed
/// individual field value elsewhere in an otherwise-valid frontmatter block MUST NOT cause a
/// whole-file skip (only `name`/`description` absence and invalid `package` do that, R-SA-005/006).
fn parse_thinking_level(raw: &str) -> Option<ThinkingLevel> {
    match raw {
        "off" => None,
        "minimal" => Some(ThinkingLevel::Minimal),
        "low" => Some(ThinkingLevel::Low),
        "medium" => Some(ThinkingLevel::Medium),
        "high" => Some(ThinkingLevel::High),
        "xhigh" => Some(ThinkingLevel::Xhigh),
        _ => None,
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

    let tools = parsed.get("tools").map(parse_tool_refs).unwrap_or_default();
    let tools = if tools.is_empty() { None } else { Some(tools) };

    let default_reads = parsed
        .get("defaultReads")
        .map(split_comma_list)
        .filter(|v| !v.is_empty())
        .map(|v| v.into_iter().map(PathBuf::from).collect::<Vec<_>>());

    // `skill` and `skills` are aliases (source: `frontmatter.skill || frontmatter.skills`) — the
    // singular form is tried first, matching source's `||` short-circuit precedence exactly.
    let skill_raw = parsed.get("skill").or_else(|| parsed.get("skills"));
    let skills = skill_raw
        .map(split_comma_list)
        .unwrap_or_default();

    let fallback_models: Vec<ModelId> = parsed
        .get("fallbackModels")
        .map(split_comma_list)
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

    let extensions = parsed.get("extensions").map(split_comma_list);

    let subagent_only_extensions = parsed
        .get("subagentOnlyExtensions")
        .map(split_comma_list)
        .unwrap_or_default();

    let model = parsed.get("model").map(ModelId::from);
    let thinking = parsed.get("thinking").and_then(parse_thinking_level);
    let output = parsed.get("output").and_then(parse_output_spec);
    let default_progress = parse_bool_field(parsed.get("defaultProgress"));
    let interactive = parse_bool_field(parsed.get("interactive"));
    let max_subagent_depth = parsed
        .get("maxSubagentDepth")
        .and_then(parse_max_subagent_depth);
    let completion_guard = parse_bool_field(parsed.get("completionGuard"));
    let disabled = parse_bool_field(parsed.get("disabled"));

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
        tools,
        extensions,
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
        assert_eq!(def.thinking, Some(ThinkingLevel::Low));
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
    fn unrecognized_thinking_value_falls_back_to_none_without_skipping_file() {
        let content = "---\nname: worker\ndescription: Worker\nthinking: super-duper\n---\n\nBody\n";
        let def = parse_agent_file(content, AgentSource::Project, Path::new("/w.md")).expect("parses");
        assert_eq!(def.thinking, None);
    }

    #[test]
    fn thinking_off_maps_to_none() {
        let content = "---\nname: worker\ndescription: Worker\nthinking: off\n---\n\nBody\n";
        let def = parse_agent_file(content, AgentSource::Project, Path::new("/w.md")).expect("parses");
        assert_eq!(def.thinking, None);
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
        assert_eq!(def.thinking, Some(ThinkingLevel::Low));
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
}
