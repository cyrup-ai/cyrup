//! Completion-mutation guard (func-SA §5.2 R-SA-034; arch-SA §6.3.4/§9).
//!
//! On process close, after structured-output validation (`exec/output.rs`, R-SA-030) and before
//! acceptance-gate evaluation (`exec/acceptance.rs`, R-SA-033), the orchestrator runs this guard:
//! classify the agent+task as implementation-expecting or not via a fixed heuristic-pattern
//! classification, and — for implementation-expecting agents with mutation-capable tools declared
//! — check whether the observed transcript contains at least one mutating `edit`/`write`/
//! mutating-`bash` tool call. If expected but absent, the run MUST be marked failed with a
//! distinguishing error message. The guard is skipped entirely (never even classifies the task)
//! when the agent declares `completion_guard: false` or declares only read-only tools.
//!
//! # Faithfulness notes (verified against `pi-subagents/src/runs/shared/completion-guard.ts` and
//! `pi-subagents/src/runs/shared/long-running-guard.ts::isMutatingBashCommand`)
//!
//! This is a line-for-line port of the source's pattern-list classification (arch-SA §12 item 8:
//! "verbatim pattern porting is the default unless explicitly waived" — no waiver has been
//! granted for this file). The workspace has no `regex` dependency for this crate (`Cargo.toml`
//! deliberately does not list one — mirrors `discovery/frontmatter.rs`'s own hand-rolled-parser
//! precedent rather than pulling in a new dependency for a handful of fixed patterns), so every
//! source `RegExp` is reproduced as a small, purpose-built matcher over the lowercased text
//! instead: [`word_boundary_contains`] replicates a plain `\bword\b`-style regex test, and each
//! source pattern with additional structure (negative lookahead, alternation-of-phrases) gets its
//! own narrow helper rather than a generic regex-subset engine. Every individual source pattern is
//! called out by comment at its Rust call site so the mapping stays auditable.
//!
//! This module has ZERO dependency on `cyrup-agent`/`cyrup-session-svc` — it only reads
//! [`crate::exec::ndjson::SubagentEvent`], the dependency-free tagged union `exec/ndjson.rs`
//! already exposes, and [`crate::discovery::types::AgentDefinition`]/[`crate::discovery::types::ToolRef`].

use crate::discovery::types::{AgentDefinition, ToolRef};
use crate::exec::ndjson::SubagentEvent;

// -------------------------------------------------------------------------------------------
// Read-only builtin tool set (mirrors `READ_ONLY_BUILTIN_TOOLS` in completion-guard.ts)
// -------------------------------------------------------------------------------------------

/// Builtin tool names considered read-only for the purpose of this guard's "declares only
/// read-only tools" short-circuit (source `READ_ONLY_BUILTIN_TOOLS`). An agent whose *entire*
/// resolved builtin-tool allowlist is a subset of this set — with no MCP-direct tools declared —
/// has no mutation capability at all, so classifying its task text is pointless and the guard is
/// skipped outright regardless of what the task prose says.
const READ_ONLY_BUILTIN_TOOLS: &[&str] = &[
    "read",
    "grep",
    "find",
    "ls",
    "web_search",
    "fetch_content",
    "get_search_content",
    "intercom",
    "contact_supervisor",
];

// -------------------------------------------------------------------------------------------
// Word-boundary text matching primitives (regex-free port of the source's `\b...\b` patterns)
// -------------------------------------------------------------------------------------------

/// True if `ch` participates in a "word" for the purposes of a `\b` boundary test — mirrors
/// JavaScript `RegExp`'s `\w` class closely enough for this module's fixed English-prose pattern
/// set (`[A-Za-z0-9_]`). Every source pattern this module ports only ever brackets plain ASCII
/// alphabetic phrases with `\b`, so this narrower definition (vs. full Unicode word-break rules)
/// is faithful for this exact pattern set.
pub(crate) fn is_word_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

/// Case-insensitive `\bneedle\b` substring test: `needle` (already lowercase, may itself contain
/// internal spaces, e.g. `"write to"`) must appear in `haystack_lower` (already lowercased) at a
/// position where the character immediately before the match (if any) and the character
/// immediately after the match (if any) are both non-word characters. This is the single building
/// block every source `/\bphrase\b/i` pattern in this module reduces to.
pub(crate) fn word_boundary_contains(haystack_lower: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let bytes = haystack_lower.as_bytes();
    let needle_bytes = needle.as_bytes();
    let mut start = 0usize;
    while let Some(rel) = haystack_lower.get(start..).and_then(|s| s.find(needle)) {
        let match_start = start + rel;
        let match_end = match_start + needle_bytes.len();

        let before_ok = haystack_lower[..match_start]
            .chars()
            .next_back()
            .is_none_or(|c| !is_word_char(c));
        let after_ok = bytes
            .get(match_end)
            .is_none()
            || haystack_lower[match_end..]
                .chars()
                .next()
                .is_none_or(|c| !is_word_char(c));

        if before_ok && after_ok {
            return true;
        }
        // Advance by one byte past this occurrence's start to find the next (possibly
        // overlapping) candidate — patterns here are short phrases, never used at a scale where
        // this matters for performance.
        start = match_start + 1;
        if start > haystack_lower.len() {
            break;
        }
    }
    false
}

/// True if any of `needles` matches `haystack_lower` per [`word_boundary_contains`].
pub(crate) fn any_word_boundary(haystack_lower: &str, needles: &[&str]) -> bool {
    needles
        .iter()
        .any(|needle| word_boundary_contains(haystack_lower, needle))
}

// -------------------------------------------------------------------------------------------
// Pattern groups (mirrors the source's REVIEW_ONLY_PATTERNS / REVIEWER_REQUIRED_EDIT_PATTERNS /
// EXPLICIT_NO_EDIT_PATTERNS / SCOPED_NO_EDIT_CONSTRAINT_PATTERNS / RESEARCH_AGENT_PATTERNS /
// WORKER_IMPLEMENTATION_PATTERNS / GENERAL_IMPLEMENTATION_PATTERNS verbatim, one function per
// source array)
// -------------------------------------------------------------------------------------------

/// Source: `REVIEW_ONLY_PATTERNS` — `/\breview only\b/i`, `/\bsuggest fixes only\b/i`,
/// `/\bonly return findings\b/i`, `/\breturn findings only\b/i`.
fn matches_review_only(text_lower: &str) -> bool {
    any_word_boundary(
        text_lower,
        &[
            "review only",
            "suggest fixes only",
            "only return findings",
            "return findings only",
        ],
    )
}

/// Source: `REVIEWER_REQUIRED_EDIT_PATTERNS`. Two entries use a `(?:...)` verb alternation
/// (`must\s+(?:edit|modify|change|fix|patch|apply)` and its `required to`/`always` siblings) —
/// ported as one boundary check per (prefix, verb) pair rather than a generic alternation engine,
/// since the verb set is small and fixed.
fn matches_reviewer_required_edit(text_lower: &str) -> bool {
    const VERBS: &[&str] = &["edit", "modify", "change", "fix", "patch", "apply"];
    const PREFIXES: &[&str] = &["must", "required to", "always"];
    for prefix in PREFIXES {
        for verb in VERBS {
            // `\bmust\s+(?:edit|...)\b` — the source pattern allows arbitrary whitespace between
            // prefix and verb (`\s+`), collapsed here to a single-space phrase test since prose
            // task text realistically uses single spaces; `word_boundary_contains` still requires
            // a non-word boundary on each side of the whole phrase.
            let phrase = format!("{prefix} {verb}");
            if word_boundary_contains(text_lower, &phrase) {
                return true;
            }
        }
    }
    any_word_boundary(
        text_lower,
        &[
            "regardless of findings",
            "apply the fix directly",
            "apply fix directly",
            "apply the fixes directly",
            "apply fixes directly",
            "make the code changes",
            "make code changes",
        ],
    )
}

/// Source: `EXPLICIT_NO_EDIT_PATTERNS` — `/\bdo not edit\b/i`, `/\bdon't edit\b/i`,
/// `/\bdo not modify\b/i`, `/\bdo not change files\b/i`.
fn matches_explicit_no_edit(text_lower: &str) -> bool {
    any_word_boundary(
        text_lower,
        &[
            "do not edit",
            "don't edit",
            "do not modify",
            "do not change files",
        ],
    )
}

/// Source: `SCOPED_NO_EDIT_CONSTRAINT_PATTERNS` — matched (not merely tested) so
/// [`strip_scoped_no_edit_constraints`] can excise every occurrence, mirroring the source's
/// `stripped.replace(pattern, " ")` loop. Returns the byte ranges of every non-overlapping match
/// of any of the five source phrases, in left-to-right order.
///
/// Source patterns: `/\bdo not edit files?\s+outside\b/i`, `/\bdo not edit\s+outside\b/i`,
/// `/\bdo not edit\s+unrelated files?\b/i`, `/\bdo not change\s+unrelated files?\b/i`,
/// `/\bdo not modify\s+unrelated files?\b/i`.
fn scoped_no_edit_constraint_ranges(text_lower: &str) -> Vec<(usize, usize)> {
    // Each entry: the fixed phrase(s) this one source pattern can match, longest-first within a
    // group so `find_longest_at` below prefers `"do not edit files outside"` over the shorter
    // `"do not edit file outside"` when both are literally present is moot (they're mutually
    // exclusive singular/plural forms) — order here only matters for readability.
    const PHRASES: &[&str] = &[
        "do not edit files outside",
        "do not edit file outside",
        "do not edit outside",
        "do not edit unrelated files",
        "do not edit unrelated file",
        "do not change unrelated files",
        "do not change unrelated file",
        "do not modify unrelated files",
        "do not modify unrelated file",
    ];

    let mut ranges = Vec::new();
    for phrase in PHRASES {
        let mut start = 0usize;
        while let Some(rel) = text_lower.get(start..).and_then(|s| s.find(phrase)) {
            let match_start = start + rel;
            let match_end = match_start + phrase.len();
            let before_ok = text_lower[..match_start]
                .chars()
                .next_back()
                .is_none_or(|c| !is_word_char(c));
            let after_ok = text_lower[match_end..]
                .chars()
                .next()
                .is_none_or(|c| !is_word_char(c));
            if before_ok && after_ok {
                ranges.push((match_start, match_end));
            }
            start = match_start + 1;
            if start > text_lower.len() {
                break;
            }
        }
    }
    ranges.sort_unstable();
    ranges
}

/// Source: `RESEARCH_AGENT_PATTERNS` — `/\binvestigate\b/i`, `/\bscout\b/i`,
/// `/\bresearch(?:er)?\b/i` (matches both `research` and `researcher`; the latter is a strict
/// extension of the former so a single `word_boundary_contains(.., "research")` covers both —
/// `\bresearch\b` alone would NOT match inside `"researcher"` since `er` is a word character
/// continuing past the boundary, so `researcher` is tested as its own literal phrase too).
fn matches_research_agent(agent_lower: &str) -> bool {
    any_word_boundary(agent_lower, &["investigate", "scout", "research", "researcher"])
}

/// Source: `WORKER_IMPLEMENTATION_PATTERNS`. The second source entry,
/// `/\b(?:update|add|remove|replace|create)\b(?!\s+(?:(?:a|an|the)\s+)?(?:report|summary|findings?)(?:\b|$))/i`,
/// is a verb match with a **negative lookahead** excluding `"<verb> [a/an/the] report|summary|
/// finding(s)"` — ported as [`matches_verb_not_followed_by_report_like_noun`] rather than folded
/// into the plain word-boundary helper, since a negative lookahead has no direct
/// `word_boundary_contains` equivalent.
fn matches_worker_implementation(task_lower: &str) -> bool {
    if any_word_boundary(
        task_lower,
        &["implement", "fix", "edit", "modify", "patch", "refactor", "delete"],
    ) {
        return true;
    }
    if matches_verb_not_followed_by_report_like_noun(
        task_lower,
        &["update", "add", "remove", "replace", "create"],
    ) {
        return true;
    }
    any_word_boundary(
        task_lower,
        &[
            "apply the changes",
            "apply changes",
            "apply the change",
            "apply change",
            "apply the fix",
            "apply fix",
            "apply the fixes",
            "apply fixes",
            "apply the patch",
            "apply patch",
            "make the changes",
            "make changes",
            "do those fixes",
        ],
    )
}

/// Source: `GENERAL_IMPLEMENTATION_PATTERNS`. The last entry is a verb-then-required-noun
/// alternation (`/\b(?:update|add|remove|replace|delete|create)\s+(?:the\s+)?(?:file|files|...)\b/i`)
/// — ported as [`matches_verb_then_target_noun`] rather than the negative-lookahead style above,
/// since this one is a POSITIVE required-noun-after-verb pattern, structurally different from
/// [`matches_verb_not_followed_by_report_like_noun`].
fn matches_general_implementation(task_lower: &str) -> bool {
    if any_word_boundary(
        task_lower,
        &["implement", "fix", "edit", "modify", "patch", "refactor"],
    ) {
        return true;
    }
    if any_word_boundary(
        task_lower,
        &[
            "apply the changes",
            "apply changes",
            "apply the change",
            "apply change",
            "apply the fix",
            "apply fix",
            "apply the fixes",
            "apply fixes",
            "apply the patch",
            "apply patch",
            "make the changes",
            "make changes",
            "do those fixes",
        ],
    ) {
        return true;
    }
    const VERBS: &[&str] = &["update", "add", "remove", "replace", "delete", "create"];
    const NOUNS: &[&str] = &[
        "file",
        "files",
        "code",
        "source",
        "implementation",
        "test",
        "tests",
        "component",
        "function",
        "module",
        "class",
        "method",
        "logic",
        "import",
        "imports",
        "readme",
        "docs",
        "doc",
        "changelog",
        "package.json",
        "config",
        "manifest",
        "extension",
        "prompt",
        "command",
    ];
    matches_verb_then_target_noun(task_lower, VERBS, NOUNS)
}

/// Ports `/\b(?:V1|V2|...)\b(?!\s+(?:(?:a|an|the)\s+)?(?:N1|N2|...)(?:\b|$))/i`: for every
/// word-boundary occurrence of a verb in `verbs`, the match is REJECTED (does not count) if,
/// immediately after the verb (skipping exactly one optional `a `/`an `/`the ` article and any
/// amount of whitespace, matching the source's `\s+(?:(?:a|an|the)\s+)?`), the following text
/// starts with `"report"`, `"summary"`, `"finding"`, or `"findings"` at a word boundary (or the
/// string ends there — the source's `(?:\b|$)`). Returns true iff at least one verb occurrence
/// survives (is NOT followed by one of those report-like nouns).
fn matches_verb_not_followed_by_report_like_noun(text_lower: &str, verbs: &[&str]) -> bool {
    const REPORT_LIKE_NOUNS: &[&str] = &["report", "summary", "finding", "findings"];
    for verb in verbs {
        let mut start = 0usize;
        while let Some(rel) = text_lower.get(start..).and_then(|s| s.find(verb)) {
            let match_start = start + rel;
            let match_end = match_start + verb.len();
            let before_ok = text_lower[..match_start]
                .chars()
                .next_back()
                .is_none_or(|c| !is_word_char(c));
            let after_word_ok = text_lower[match_end..]
                .chars()
                .next()
                .is_none_or(|c| !is_word_char(c));
            start = match_start + 1;
            if start > text_lower.len() {
                start = text_lower.len() + 1;
            }
            if !before_ok || !after_word_ok {
                continue;
            }

            let rest = &text_lower[match_end..];
            let after_ws = rest.trim_start_matches([' ', '\t']);
            if rest == after_ws {
                // Source requires `\s+` (at least one whitespace char) between the verb and the
                // optional article/noun for the lookahead to even apply; with no whitespace at
                // all following the verb (end of string, or punctuation immediately after), the
                // negative lookahead trivially does not exclude this occurrence.
                return true;
            }
            // Strip one optional `a `/`an `/`the ` article, mirroring `(?:(?:a|an|the)\s+)?`.
            let after_article = strip_optional_article(after_ws);
            let excluded = REPORT_LIKE_NOUNS.iter().any(|noun| {
                after_article.strip_prefix(noun).is_some_and(|tail| {
                    tail.chars().next().is_none_or(|c| !is_word_char(c))
                })
            });
            if !excluded {
                return true;
            }
        }
    }
    false
}

/// Strips at most one leading `"a "`, `"an "`, or `"the "` article token from `text` — the
/// `(?:(?:a|an|the)\s+)?` half of the source's negative-lookahead pattern.
fn strip_optional_article(text: &str) -> &str {
    for article in ["a ", "an ", "the "] {
        if let Some(rest) = text.strip_prefix(article) {
            return rest;
        }
    }
    text
}

/// Ports `/\b(?:V1|...)\s+(?:the\s+)?(?:N1|...)\b/i`: a verb, optional `"the "`, then one of the
/// required nouns, all at word boundaries.
fn matches_verb_then_target_noun(text_lower: &str, verbs: &[&str], nouns: &[&str]) -> bool {
    for verb in verbs {
        let mut start = 0usize;
        while let Some(rel) = text_lower.get(start..).and_then(|s| s.find(verb)) {
            let match_start = start + rel;
            let match_end = match_start + verb.len();
            let before_ok = text_lower[..match_start]
                .chars()
                .next_back()
                .is_none_or(|c| !is_word_char(c));
            start = match_start + 1;
            if start > text_lower.len() {
                start = text_lower.len() + 1;
            }
            if !before_ok {
                continue;
            }
            let rest = &text_lower[match_end..];
            let after_ws = rest.trim_start_matches([' ', '\t']);
            if rest == after_ws {
                // `\s+` is mandatory before the noun clause; no whitespace at all means this verb
                // occurrence cannot satisfy the rest of the pattern.
                continue;
            }
            let after_the = after_ws.strip_prefix("the ").unwrap_or(after_ws);
            let matched_noun = nouns.iter().any(|noun| {
                after_the.strip_prefix(noun).is_some_and(|tail| {
                    tail.chars().next().is_none_or(|c| !is_word_char(c))
                })
            });
            if matched_noun {
                return true;
            }
        }
    }
    false
}

// -------------------------------------------------------------------------------------------
// Task-text preprocessing (mirrors `stripFrameworkInstructions` / `stripScopedNoEditConstraints`)
// -------------------------------------------------------------------------------------------

/// Source: `stripFrameworkInstructions`. Drops any line that is purely orchestrator-injected
/// scaffolding (`[Write to: ...]`/`[Read from: ...]` framework markers, and the fixed set of
/// progress/output-path instruction lines the source enumerates) so those lines' own vocabulary
/// (`write`, `output`, `update progress`, …) never contributes false-positive implementation
/// signal. Line-oriented, case-insensitive, anchored at line start (`^\s*...`) per the source's
/// per-line regex tests.
fn strip_framework_instructions(task: &str) -> String {
    task.lines()
        .filter(|line| !is_write_read_marker_line(line))
        .filter(|line| !is_progress_or_output_instruction_line(line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `/^\s*\[(?:Write to|Read from):/i`
fn is_write_read_marker_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    let lower = trimmed.to_lowercase();
    lower.starts_with("[write to:") || lower.starts_with("[read from:")
}

/// `/^\s*(?:Create and maintain progress at:|Update progress at:|\*\*Output:\*\*|Write your
/// findings to(?: exactly this path)?:|This path is authoritative for this run\.|Ignore any other
/// output filename or output path mentioned elsewhere)/i`
fn is_progress_or_output_instruction_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    let lower = trimmed.to_lowercase();
    lower.starts_with("create and maintain progress at:")
        || lower.starts_with("update progress at:")
        || lower.starts_with("**output:**")
        || lower.starts_with("write your findings to exactly this path:")
        || lower.starts_with("write your findings to:")
        || lower.starts_with("this path is authoritative for this run.")
        || lower.starts_with("ignore any other output filename or output path mentioned elsewhere")
}

/// Source: `stripScopedNoEditConstraints` — replaces every match of
/// [`scoped_no_edit_constraint_ranges`] with a single space, mirroring `stripped.replace(pattern,
/// " ")` applied once per pattern in sequence (the source loop's net effect on non-overlapping
/// matches, which is the only case this pattern set can ever produce given how specific each
/// phrase is).
fn strip_scoped_no_edit_constraints(task_lower_source: &str) -> String {
    let ranges = scoped_no_edit_constraint_ranges(task_lower_source);
    if ranges.is_empty() {
        return task_lower_source.to_string();
    }
    let mut result = String::with_capacity(task_lower_source.len());
    let mut cursor = 0usize;
    for (start, end) in ranges {
        if start < cursor {
            // Overlapping match already covered by a prior replacement; skip.
            continue;
        }
        result.push_str(&task_lower_source[cursor..start]);
        result.push(' ');
        cursor = end;
    }
    result.push_str(&task_lower_source[cursor..]);
    result
}

// -------------------------------------------------------------------------------------------
// Public classification API (mirrors `expectsImplementationMutation` / `hasMutationToolCall` /
// `evaluateCompletionMutationGuard`)
// -------------------------------------------------------------------------------------------

/// Source: `expectsImplementationMutation(agent, task)`. Classifies whether `task`, given the
/// declared `agent` (persona) name, is expected to require a mutating tool call to be considered
/// genuinely complete — the "fixed heuristic-pattern classification" R-SA-034 requires.
///
/// This function does NOT consult declared tools at all (that short-circuit —
/// [`declares_only_read_only_tools`] — is applied by [`evaluate_completion_mutation_guard`],
/// exactly mirroring the source's own separation between `expectsImplementationMutation` as a
/// pure task/agent-name classifier and the read-only-tools carve-out living one level up in
/// `evaluateCompletionMutationGuard`).
#[must_use]
pub fn expects_implementation_mutation(agent: &str, task: &str) -> bool {
    let stripped = strip_framework_instructions(task);
    let stripped_lower = stripped.to_lowercase();
    let without_scoped_lower = strip_scoped_no_edit_constraints(&stripped_lower);

    if matches_review_only(&without_scoped_lower) {
        return false;
    }
    if matches_explicit_no_edit(&without_scoped_lower) {
        return false;
    }

    let agent_lower = agent.to_lowercase();
    if matches_research_agent(&agent_lower) {
        return false;
    }
    if word_boundary_contains(&agent_lower, "reviewer") {
        return matches_reviewer_required_edit(&stripped_lower);
    }

    let worker_intent =
        agent == "worker" && matches_worker_implementation(&stripped_lower);
    if worker_intent {
        return true;
    }

    matches_general_implementation(&stripped_lower)
}

/// Source: `hasMutationToolCall(messages)` (`completion-guard.ts:121-135`), re-scoped to this
/// crate's dependency-free [`SubagentEvent`] transcript instead of a rich `Message[]` array (this
/// crate has zero dependency on `cyrup-agent`'s message types, module docs above).
///
/// The source scans the assistant messages' `toolCall` **content parts** — the tool CALL, carrying
/// its `arguments` — NOT the tool result. This crate's wire analogue of "an assistant emitted a
/// tool call, with its requested arguments" is [`SubagentEvent::ToolExecutionStart`], which is the
/// only event on the wire carrying the call's `args` (`ToolExecutionEnd` echoes only
/// `result`/`is_error`, per `exec/ndjson.rs`'s wire-shape module doc). This function therefore
/// scans `ToolExecutionStart` events, matching the source exactly: a call is counted from the
/// moment it is REQUESTED, so a mutating call that started but never produced a
/// `ToolExecutionEnd` (the child was killed mid-tool-call, or the tool never finished) STILL counts
/// as an attempted mutation — precisely the "count never-completed calls" behavior the source's own
/// message-part walk exhibits (a `toolCall` part is present in the assistant message regardless of
/// whether a corresponding `toolResult` was ever appended).
///
/// Returns true on the first `edit`/`write` tool call observed, or the first `bash` call whose
/// `command` argument [`is_mutating_bash_command`] classifies as mutating (the source's
/// `part.arguments.command` read, applied here to the start event's `args.command`).
#[must_use]
pub fn has_mutation_tool_call(events: &[SubagentEvent]) -> bool {
    events.iter().any(|event| {
        let SubagentEvent::ToolExecutionStart {
            tool_name, args, ..
        } = event
        else {
            return false;
        };
        match tool_name.as_str() {
            "edit" | "write" => true,
            "bash" => args
                .get("command")
                .and_then(serde_json::Value::as_str)
                .is_some_and(is_mutating_bash_command),
            _ => false,
        }
    })
}

/// Source: `isMutatingBashCommand` (`long-running-guard.ts`). A bash command counts as mutating
/// if it contains an unquoted file-redirection operator (`>`/`>>` not immediately preceded by `-`
/// and not immediately followed by `&`/`|`/`;`/`(`/`)`, outside single/double quotes) OR matches
/// one of the fixed `MUTATING_BASH_PATTERNS`.
#[must_use]
pub fn is_mutating_bash_command(command: &str) -> bool {
    has_unquoted_file_redirection(command) || matches_mutating_bash_pattern(command)
}

/// Source: `hasUnquotedFileRedirection` — a hand-rolled quote-aware scanner (already
/// regex-free in the TypeScript source itself), ported verbatim character-by-character.
fn has_unquoted_file_redirection(command: &str) -> bool {
    let chars: Vec<char> = command.chars().collect();
    let mut in_single = false;
    let mut in_double = false;
    let mut i = 0usize;
    while let Some(&ch) = chars.get(i) {
        if ch == '\'' && !in_double {
            in_single = !in_single;
            i += 1;
            continue;
        }
        if ch == '"' && !in_single {
            in_double = !in_double;
            i += 1;
            continue;
        }
        if in_single || in_double {
            i += 1;
            continue;
        }
        if ch != '>' {
            i += 1;
            continue;
        }
        if i > 0 && chars.get(i - 1) == Some(&'-') {
            i += 1;
            continue;
        }
        let is_double_redirect = chars.get(i + 1) == Some(&'>');
        let mut cursor = i + usize::from(is_double_redirect) + 1;
        while chars.get(cursor).is_some_and(|c| c.is_whitespace()) {
            cursor += 1;
        }
        let Some(&target_start) = chars.get(cursor) else {
            i += 1;
            continue;
        };
        if matches!(target_start, '&' | '|' | ';' | '(' | ')') {
            i += 1;
            continue;
        }
        return true;
    }
    false
}

/// Source: `MUTATING_BASH_PATTERNS`, verbatim per-entry.
fn matches_mutating_bash_pattern(command: &str) -> bool {
    // `/(^|[;&|()\s])rm\s+/`, `mv`, `cp`, `mkdir`, `touch` — a shell-command-word occurrence of
    // the verb (start-of-string or preceded by a shell separator/whitespace) followed by
    // whitespace then at least one more character (`\s+` requires the verb not be the entire
    // remainder of the string).
    for verb in ["rm", "mv", "cp", "mkdir", "touch"] {
        if command_word_followed_by_whitespace(command, verb) {
            return true;
        }
    }
    // `/(^|[;&|()\s])git\s+apply\b/`
    if let Some(rest) = find_command_word(command, "git") {
        let after_ws = rest.trim_start_matches(char::is_whitespace);
        if after_ws != rest
            && let Some(tail) = after_ws.strip_prefix("apply")
            && tail.chars().next().is_none_or(|c| !is_word_char(c))
        {
            return true;
        }
    }
    // `/(^|[;&|()\s])patch\s+/`
    if command_word_followed_by_whitespace(command, "patch") {
        return true;
    }
    // `/(^|[;&|()\s])sed\s+[^\n;&|]*\s-i\b/` — `sed`, whitespace, then any run of characters
    // excluding `\n`/`;`/`&`/`|`, then whitespace then `-i` at a word boundary.
    if command_word_then_flag(command, "sed", "-i") {
        return true;
    }
    // `/(^|[;&|()\s])perl\s+[^\n;&|]*\s-pi\b/`
    if command_word_then_flag(command, "perl", "-pi") {
        return true;
    }
    // `/(^|[;&|()]|\n)\s*tee\s+[^|&;]+/` — `tee` (preceded by a shell separator/newline or
    // string-start, optionally with whitespace in between) followed by whitespace then at least
    // one non-`|`/`&`/`;` character.
    if matches_tee_invocation(command) {
        return true;
    }
    // `/\b(writeFile|writeFileSync|appendFile|appendFileSync)\b/`
    if any_word_boundary(
        command,
        &["writefile", "writefilesync", "appendfile", "appendfilesync"],
    ) {
        // Case-sensitive in the source (no `/i` flag) — but these are camelCase Node.js API
        // names that only ever appear in that exact casing in realistic command text; comparing
        // case-sensitively against the ORIGINAL command (not lowercased) preserves source
        // fidelity exactly, so this branch re-checks against `command` directly below instead of
        // relying on the lowercase-only `any_word_boundary` helper.
    }
    for needle in ["writeFile", "writeFileSync", "appendFile", "appendFileSync"] {
        if word_boundary_contains_case_sensitive(command, needle) {
            return true;
        }
    }
    // `/\bwrite_text\s*\(/`
    if let Some(idx) = find_word_boundary_case_sensitive(command, "write_text") {
        let rest = command.get(idx + "write_text".len()..).unwrap_or("");
        let after_ws = rest.trim_start_matches(char::is_whitespace);
        if after_ws.starts_with('(') {
            return true;
        }
    }
    // `/\bopen\s*\([^)]*,\s*["'][wa]/`
    if matches_python_open_write_mode(command) {
        return true;
    }
    false
}

/// True if `word` occurs in `command` at a position preceded by start-of-string or one of
/// `;&|()` or whitespace, and is immediately followed by at least one whitespace character plus
/// at least one more character after that whitespace run (the source's trailing `\s+` requiring
/// non-empty content after the verb). Case-insensitive, matching the source's `/i` flag on every
/// pattern this helper backs.
fn command_word_followed_by_whitespace(command: &str, word: &str) -> bool {
    let Some(rest) = find_command_word(command, word) else {
        return false;
    };
    let after_ws = rest.trim_start_matches(char::is_whitespace);
    after_ws != rest && !after_ws.is_empty()
}

/// Locates the first occurrence of `word` (case-insensitive) in `command` that is preceded by
/// start-of-string or a shell separator/whitespace character (`;`, `&`, `|`, `(`, `)`, or any
/// whitespace) — the source's `(^|[;&|()\s])` alternation — and returns the slice of `command`
/// immediately following that occurrence, or `None` if no such occurrence exists.
fn find_command_word<'a>(command: &'a str, word: &str) -> Option<&'a str> {
    let lower = command.to_lowercase();
    let word_lower = word.to_lowercase();
    let mut start = 0usize;
    while let Some(rel) = lower.get(start..).and_then(|s| s.find(&word_lower)) {
        let match_start = start + rel;
        let match_end = match_start + word_lower.len();
        let preceded_ok = match_start == 0
            || lower[..match_start]
                .chars()
                .next_back()
                .is_some_and(|c| matches!(c, ';' | '&' | '|' | '(' | ')') || c.is_whitespace());
        if preceded_ok {
            return command.get(match_end..);
        }
        start = match_start + 1;
        if start > lower.len() {
            break;
        }
    }
    None
}

/// Backs the `sed`/`perl` patterns: `word`, whitespace, then a run of characters excluding
/// `\n`/`;`/`&`/`|`, then whitespace, then `flag` at a word boundary.
fn command_word_then_flag(command: &str, word: &str, flag: &str) -> bool {
    let Some(rest) = find_command_word(command, word) else {
        return false;
    };
    let after_ws = rest.trim_start_matches(char::is_whitespace);
    if after_ws == rest {
        return false;
    }
    // `[^\n;&|]*` — consume as far as possible without crossing a newline/`;`/`&`/`|`, then look
    // for `\s-flag\b` starting anywhere within that consumed span (the source's `.*\s-i\b` is
    // itself greedy-then-backtrack, which reduces to "does `\s<flag>\b` occur anywhere before the
    // first `\n`/`;`/`&`/`|`").
    let scan_limit = after_ws
        .find(['\n', ';', '&', '|'])
        .unwrap_or(after_ws.len());
    let Some(scan_region) = after_ws.get(..scan_limit) else {
        return false;
    };
    let needle = format!(" {flag}");
    let mut start = 0usize;
    while let Some(rel) = scan_region.get(start..).and_then(|s| s.find(&needle)) {
        let match_start = start + rel;
        let flag_start = match_start + 1; // past the leading space
        let flag_end = flag_start + flag.len();
        let after_ok = scan_region
            .get(flag_end..)
            .and_then(|s| s.chars().next())
            .is_none_or(|c| !is_word_char(c));
        if after_ok {
            return true;
        }
        start = match_start + 1;
        if start > scan_region.len() {
            break;
        }
    }
    false
}

/// Backs `/(^|[;&|()]|\n)\s*tee\s+[^|&;]+/`.
fn matches_tee_invocation(command: &str) -> bool {
    let lower = command.to_lowercase();
    let mut start = 0usize;
    while let Some(rel) = lower.get(start..).and_then(|s| s.find("tee")) {
        let match_start = start + rel;
        let match_end = match_start + 3;
        // Preceded by start-of-string, `;`/`&`/`|`/`(`/`)`/`\n`, possibly with additional
        // whitespace in between (`\s*` before `tee` in the source's own capture group ordering:
        // the separator/newline is matched, THEN `\s*`, THEN `tee` — so whitespace may sit
        // between the separator and `tee`, or `tee` may be at absolute start-of-string with only
        // leading whitespace).
        let prefix = &lower[..match_start];
        let trimmed_prefix = prefix.trim_end_matches([' ', '\t']);
        let preceded_ok = trimmed_prefix.is_empty()
            || trimmed_prefix
                .chars()
                .next_back()
                .is_some_and(|c| matches!(c, ';' | '&' | '|' | '(' | ')' | '\n'));
        if preceded_ok {
            let after_tee = &lower[match_end..];
            let after_ws = after_tee.trim_start_matches([' ', '\t']);
            let has_target = after_ws != after_tee
                && after_ws
                    .chars()
                    .next()
                    .is_some_and(|c| !matches!(c, '|' | '&' | ';'));
            if has_target {
                return true;
            }
        }
        start = match_start + 1;
        if start > lower.len() {
            break;
        }
    }
    false
}

/// Backs `/\bopen\s*\([^)]*,\s*["'][wa]/` — Python-style `open(path, "w"...)`/`open(path, 'a'...)`.
fn matches_python_open_write_mode(command: &str) -> bool {
    let Some(idx) = find_word_boundary_case_sensitive_lower(command, "open") else {
        return false;
    };
    let rest = command.get(idx + 4..).unwrap_or("");
    let after_ws = rest.trim_start_matches(char::is_whitespace);
    let Some(after_paren) = after_ws.strip_prefix('(') else {
        return false;
    };
    let Some(comma_idx) = after_paren.find(',') else {
        return false;
    };
    let Some(args_head) = after_paren.get(..comma_idx) else {
        return false;
    };
    if args_head.contains(')') {
        return false;
    }
    let after_comma = after_paren.get(comma_idx + 1..).unwrap_or("");
    let after_comma_ws = after_comma.trim_start_matches(char::is_whitespace);
    let mut chars = after_comma_ws.chars();
    match chars.next() {
        Some('"') | Some('\'') => {}
        _ => return false,
    }
    matches!(chars.next(), Some('w') | Some('a'))
}

/// Case-sensitive `\bneedle\b` test (used only for the two source patterns that omit the `/i`
/// flag: `writeFile`/... and `write_text`).
fn word_boundary_contains_case_sensitive(haystack: &str, needle: &str) -> bool {
    find_word_boundary_case_sensitive(haystack, needle).is_some()
}

fn find_word_boundary_case_sensitive(haystack: &str, needle: &str) -> Option<usize> {
    let mut start = 0usize;
    while let Some(rel) = haystack.get(start..).and_then(|s| s.find(needle)) {
        let match_start = start + rel;
        let match_end = match_start + needle.len();
        let before_ok = haystack[..match_start]
            .chars()
            .next_back()
            .is_none_or(|c| !is_word_char(c));
        let after_ok = haystack[match_end..]
            .chars()
            .next()
            .is_none_or(|c| !is_word_char(c));
        if before_ok && after_ok {
            return Some(match_start);
        }
        start = match_start + 1;
        if start > haystack.len() {
            break;
        }
    }
    None
}

/// `find_word_boundary_case_sensitive`, but case-insensitive (`open` in `/\bopen\s*\(/` DOES
/// carry the `/i` flag in the source) — returns the byte offset of the match in the ORIGINAL
/// (not-lowercased) `haystack` so callers can slice the real string afterward.
fn find_word_boundary_case_sensitive_lower(haystack: &str, needle_lower: &str) -> Option<usize> {
    let lower = haystack.to_lowercase();
    let mut start = 0usize;
    while let Some(rel) = lower.get(start..).and_then(|s| s.find(needle_lower)) {
        let match_start = start + rel;
        let match_end = match_start + needle_lower.len();
        let before_ok = lower[..match_start]
            .chars()
            .next_back()
            .is_none_or(|c| !is_word_char(c));
        let after_ok = lower[match_end..]
            .chars()
            .next()
            .is_none_or(|c| !is_word_char(c));
        if before_ok && after_ok {
            return Some(match_start);
        }
        start = match_start + 1;
        if start > lower.len() {
            break;
        }
    }
    None
}

// -------------------------------------------------------------------------------------------
// Read-only-tools short-circuit (mirrors `declaresOnlyReadOnlyTools`)
// -------------------------------------------------------------------------------------------

/// True iff `agent`'s resolved `tools` allowlist is non-empty, contains no MCP-direct entries,
/// and every entry is one of [`READ_ONLY_BUILTIN_TOOLS`] — i.e. the agent has no mutation
/// capability declared at all. Mirrors source `declaresOnlyReadOnlyTools(tools, mcpDirectTools)`,
/// re-scoped to this crate's [`AgentDefinition::tools`]/[`ToolRef`] shapes: a `ToolRef::Mcp` entry
/// is this crate's equivalent of a non-empty `mcpDirectTools`, and `ToolRef::ExtensionPath` counts
/// as a non-read-only (mutation-capable-or-unknown) entry, exactly as an unrecognized builtin name
/// would fail the source's `tools.every(...)` check.
///
/// `tools == None` ("no allowlist restriction, all builtins available", per
/// [`AgentDefinition::tools`]'s own doc comment) is NOT "only read-only tools" — an unrestricted
/// agent implicitly has access to every mutating builtin too, so this returns `false` for `None`,
/// matching the source's own `tools !== undefined` guard (an agent that declared no `tools` field
/// at all is conservatively treated as NOT read-only-only).
fn declares_only_read_only_tools(agent: &AgentDefinition) -> bool {
    let Some(tools) = &agent.tools else {
        return false;
    };
    if tools.is_empty() {
        return false;
    }
    tools.iter().all(|tool_ref| match tool_ref {
        ToolRef::Builtin(name) => READ_ONLY_BUILTIN_TOOLS.contains(&name.as_str()),
        ToolRef::Mcp(_) | ToolRef::ExtensionPath(_) => false,
    })
}

// -------------------------------------------------------------------------------------------
// Top-level evaluation (mirrors `evaluateCompletionMutationGuard`)
// -------------------------------------------------------------------------------------------

/// The result of evaluating the completion-mutation guard for one run (R-SA-034), mirroring
/// source `CompletionMutationGuardResult` field-for-field.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CompletionMutationGuardResult {
    /// Whether the classifier determined this agent+task pair was expected to require a
    /// mutation. Always `false` when the agent declares `completion_guard: false` or declares
    /// only read-only tools (the guard's two exemptions, R-SA-034) — see
    /// [`evaluate_completion_mutation_guard`]'s doc for exactly where those short-circuits apply.
    pub expected_mutation: bool,
    /// Whether at least one mutating tool call was actually observed in the transcript.
    pub attempted_mutation: bool,
    /// `expected_mutation && !attempted_mutation` — the guard's actual trigger condition. When
    /// `true`, the orchestrator (a later phase's `exec/mod.rs`/`exec/fallback.rs` completion path,
    /// R-SA-033's ordering) MUST set the run's exit code to failure and append
    /// [`COMPLETION_GUARD_ERROR_MESSAGE`] (or an equivalent distinguishing message) to the
    /// delivered error.
    pub triggered: bool,
}

/// A distinguishing error-message fragment for a triggered guard, for callers (a later phase's
/// completion path) that want a fixed, greppable string rather than composing their own — R-SA-034
/// requires "a distinguishing error message" but does not fix its exact wording; this constant is
/// this crate's canonical phrasing so every trigger site produces consistent text.
pub const COMPLETION_GUARD_ERROR_MESSAGE: &str =
    "completion-mutation guard: task appeared to require an implementation change, but no \
     mutating edit/write/bash tool call was observed before the run completed";

/// Evaluate the full R-SA-034 completion-mutation guard for one finished run: `agent` is the
/// resolved [`AgentDefinition`] (its `local_name`/`name` supplies the source's `agent` string
/// classification input, and its `tools`/`completion_guard` fields supply both exemptions);
/// `task` is the literal task prompt text handed to the child; `events` is the full parsed
/// transcript of [`SubagentEvent`]s observed on the child's stdout for this attempt (R-SA-057).
///
/// # The two exemptions (R-SA-034)
///
/// Both are checked BEFORE classification even runs, matching the source's own short-circuit
/// ordering (`declaresOnlyReadOnlyTools(...) ? false : expectsImplementationMutation(...)`,
/// extended here with the explicit-`false` config flag the source's own equivalent config
/// resolution layer — outside this file's scope in the TS port — applies upstream of
/// `evaluateCompletionMutationGuard` itself):
///
/// 1. `agent.completion_guard == Some(false)` — the agent explicitly opted out. The guard is
///    skipped entirely: this function returns `expected_mutation: false` (task text is never
///    classified at all) without needing to inspect `tools`.
/// 2. [`declares_only_read_only_tools`] — the agent's resolved tools contain no mutation
///    capability at all, so even a task that reads as "implementation-expecting" cannot possibly
///    be satisfied by any declared tool; classification is skipped for the same reason.
///
/// Neither exemption ever makes `triggered` true by omission — both force `expected_mutation` to
/// `false`, which makes `triggered` false regardless of `attempted_mutation`.
#[must_use]
pub fn evaluate_completion_mutation_guard(
    agent: &AgentDefinition,
    task: &str,
    events: &[SubagentEvent],
) -> CompletionMutationGuardResult {
    let guard_disabled = agent.completion_guard == Some(false);
    let expected_mutation = if guard_disabled || declares_only_read_only_tools(agent) {
        false
    } else {
        expects_implementation_mutation(&agent.local_name, task)
    };
    let attempted_mutation = has_mutation_tool_call(events);
    CompletionMutationGuardResult {
        expected_mutation,
        attempted_mutation,
        triggered: expected_mutation && !attempted_mutation,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    use crate::discovery::types::{AgentSource, SystemPromptMode};

    fn agent(local_name: &str, tools: Option<Vec<ToolRef>>, completion_guard: Option<bool>) -> AgentDefinition {
        AgentDefinition {
            name: local_name.to_string(),
            local_name: local_name.to_string(),
            package_name: None,
            description: "test agent".to_string(),
            tools,
            extensions: None,
            subagent_only_extensions: Vec::new(),
            model: None,
            fallback_models: Vec::new(),
            thinking: None,
            system_prompt_mode: SystemPromptMode::Replace,
            inherit_project_context: false,
            inherit_skills: false,
            skills: Vec::new(),
            default_reads: None,
            default_progress: None,
            output: None,
            completion_guard,
            interactive: None,
            max_subagent_depth: None,
            default_context: None,
            disabled: None,
            system_prompt_body: String::new(),
            source: AgentSource::User,
            file_path: PathBuf::from("/tmp/agent.md"),
            present_fields: HashSet::new(),
            extra_fields: std::collections::BTreeMap::new(),
            override_info: None,
            model_source: None,
        }
    }

    /// Build a tool CALL event (`ToolExecutionStart`) carrying the call's `args` — the wire event
    /// [`has_mutation_tool_call`] scans (the source scans assistant `toolCall` content parts, i.e.
    /// the CALL with its arguments, never the tool result).
    fn tool_start(name: &str, args: serde_json::Value) -> SubagentEvent {
        SubagentEvent::ToolExecutionStart {
            tool_call_id: "c1".into(),
            tool_name: name.to_string(),
            args,
        }
    }

    /// A finished tool call (`ToolExecutionEnd`) — used only to prove that the guard scans the
    /// CALL, not the RESULT: a mutating `command` echoed back only in a `ToolExecutionEnd.result`
    /// (with no corresponding start) must NOT be counted.
    fn tool_end(name: &str, result: serde_json::Value) -> SubagentEvent {
        SubagentEvent::ToolExecutionEnd {
            tool_call_id: "c1".into(),
            tool_name: name.to_string(),
            result,
            is_error: false,
        }
    }

    // ---- word_boundary_contains ----

    #[test]
    fn word_boundary_contains_requires_boundaries_on_both_sides() {
        assert!(word_boundary_contains("please review only this", "review only"));
        assert!(!word_boundary_contains("prereview only this", "review only"));
        assert!(!word_boundary_contains("review onlyish", "review only"));
    }

    // ---- expects_implementation_mutation: direct ports of the source's own test assertions ----

    #[test]
    fn review_research_and_framework_instructions_do_not_expect_mutation() {
        assert!(!expects_implementation_mutation(
            "worker",
            "Review only: return findings, do not edit"
        ));
        assert!(!expects_implementation_mutation(
            "worker",
            "Do not edit files. Tell me how to fix the bug."
        ));
        assert!(!expects_implementation_mutation(
            "worker",
            "Review the diff and suggest fixes only. Do not edit files."
        ));
        assert!(!expects_implementation_mutation(
            "worker",
            "Implement this. Do not edit files outside this repo. Do not edit files."
        ));
        assert!(!expects_implementation_mutation("worker", "Investigate why this failed"));
        assert!(!expects_implementation_mutation("researcher", "Research the API behavior"));
        assert!(!expects_implementation_mutation("researcher", "Research this and patch the bug"));
        assert!(!expects_implementation_mutation("reviewer", "Review this and fix any real issues"));
        assert!(expects_implementation_mutation(
            "reviewer",
            "Review this and fix any real issues; regardless of findings, apply changes directly"
        ));
        assert!(!expects_implementation_mutation(
            "worker",
            "[Write to: /tmp/result.md]\n\nSummarize findings"
        ));
        assert!(!expects_implementation_mutation("worker", "Write report"));
        assert!(!expects_implementation_mutation("worker", "Create a report"));
        assert!(!expects_implementation_mutation("worker", "Create a summary"));
        assert!(!expects_implementation_mutation("worker", "Add a report"));
        assert!(!expects_implementation_mutation("worker", "Update a summary"));
        assert!(!expects_implementation_mutation("worker", "Write to {chain_dir}"));
        assert!(!expects_implementation_mutation(
            "worker",
            "Do async work\nUpdate progress at: /tmp/progress.md\n**Output:**\nWrite your findings to exactly this path: /tmp/out.md\nThis path is authoritative for this run.\nIgnore any other output filename or output path mentioned elsewhere."
        ));
    }

    #[test]
    fn worker_implementation_verbs_win_over_investigative_wording() {
        assert!(expects_implementation_mutation(
            "worker",
            "Investigate why the worker did not edit files and fix it"
        ));
        assert!(expects_implementation_mutation(
            "worker",
            "Research the current code path and patch the bug"
        ));
        assert!(expects_implementation_mutation(
            "worker",
            "Fix the bug where no edits were made"
        ));
        assert!(expects_implementation_mutation("worker", "Implement the fix and return findings."));
    }

    #[test]
    fn worker_edit_intent_covers_common_docs_config_and_source_tasks() {
        assert!(expects_implementation_mutation("worker", "Update README to mention the native tool"));
        assert!(expects_implementation_mutation(
            "worker",
            "Remove share functionality and all Vercel references"
        ));
        assert!(expects_implementation_mutation(
            "worker",
            "Replace the registered command with a render tool"
        ));
        assert!(expects_implementation_mutation("worker", "Create completion-guard.ts"));
        assert!(expects_implementation_mutation("worker", "Add tests for the completion guard"));
        assert!(expects_implementation_mutation(
            "worker",
            "Implement the approved fixes. Do not edit files outside this repo."
        ));
        assert!(expects_implementation_mutation(
            "worker",
            "Implement the fix. Do not edit unrelated files."
        ));
    }

    // ---- has_mutation_tool_call / is_mutating_bash_command ----

    #[test]
    fn edit_and_write_tool_calls_count_as_mutation_attempts() {
        assert!(has_mutation_tool_call(&[tool_start("edit", serde_json::json!({"path": "a.ts"}))]));
        assert!(has_mutation_tool_call(&[tool_start("write", serde_json::json!({"path": "a.ts"}))]));
    }

    #[test]
    fn a_never_completed_mutating_call_still_counts_as_an_attempt() {
        // The source scans the assistant `toolCall` part, present the moment the model requests
        // the call — a mutating call that started but produced no `ToolExecutionEnd` (child killed
        // mid-tool-call, or the tool never finished) must STILL count. Only the start event is
        // present here; no matching end.
        let start_only = vec![tool_start("edit", serde_json::json!({"path": "a.ts"}))];
        assert!(has_mutation_tool_call(&start_only));

        let bash_start_only = vec![tool_start(
            "bash",
            serde_json::json!({"command": "rm -rf build"}),
        )];
        assert!(has_mutation_tool_call(&bash_start_only));
    }

    #[test]
    fn a_mutating_command_only_in_the_result_payload_is_not_counted() {
        // Regression for the ported bug: the guard must read the tool CALL's args, never the tool
        // RESULT. A `ToolExecutionEnd` whose `result` merely echoes a mutating command — with no
        // corresponding start event — must NOT be counted as an attempted mutation.
        let end_only = vec![tool_end(
            "bash",
            serde_json::json!({"command": "rm -rf build", "stdout": ""}),
        )];
        assert!(!has_mutation_tool_call(&end_only));
    }

    #[test]
    fn obvious_mutating_bash_commands_count_as_mutation_attempts() {
        assert!(is_mutating_bash_command(
            "mkdir -p src && cat > src/file.ts <<'EOF'\nhi\nEOF"
        ));
        assert!(is_mutating_bash_command("cat <<'EOF' > src/file.ts\nhi\nEOF"));
        assert!(is_mutating_bash_command(
            "python3 -c \"from pathlib import Path; Path('x').write_text('hi')\""
        ));
        assert!(is_mutating_bash_command("node script.js > generated.txt"));
        assert!(!is_mutating_bash_command("echo 'a > b'"));
        assert!(!is_mutating_bash_command("node -e \"console.log(a > b)\""));
        assert!(!is_mutating_bash_command("python3 <<'PY'\nprint('inspect only')\nPY"));
        assert!(!is_mutating_bash_command("echo 'rm file'"));
        assert!(!is_mutating_bash_command("printf \"mkdir x\""));
        assert!(is_mutating_bash_command("git apply patch.diff"));
        assert!(is_mutating_bash_command("patch -p0 < fix.patch"));
    }

    #[test]
    fn has_mutation_tool_call_reads_bash_command_from_call_args() {
        let events = vec![tool_start(
            "bash",
            serde_json::json!({"command": "rm -rf build"}),
        )];
        assert!(has_mutation_tool_call(&events));

        let non_mutating = vec![tool_start("bash", serde_json::json!({"command": "ls -la"}))];
        assert!(!has_mutation_tool_call(&non_mutating));
    }

    // ---- evaluate_completion_mutation_guard: the three required scenarios ----

    #[test]
    fn triggers_on_mutation_expected_task_with_zero_observed_mutating_calls() {
        let a = agent("worker", None, None);
        let events = vec![SubagentEvent::MessageEnd {
            message: serde_json::json!({"role": "assistant", "content": []}),
        }];
        let result = evaluate_completion_mutation_guard(&a, "Implement the approved fix", &events);
        assert_eq!(
            result,
            CompletionMutationGuardResult {
                expected_mutation: true,
                attempted_mutation: false,
                triggered: true,
            }
        );
    }

    #[test]
    fn exempts_a_read_only_tools_agent_even_with_implementation_wording() {
        let a = agent(
            "architect",
            Some(vec![
                ToolRef::Builtin("read".to_string()),
                ToolRef::Builtin("grep".to_string()),
                ToolRef::Builtin("find".to_string()),
                ToolRef::Builtin("ls".to_string()),
            ]),
            None,
        );
        let result = evaluate_completion_mutation_guard(
            &a,
            "Produce a proposal that implements the approved fix",
            &[],
        );
        assert_eq!(
            result,
            CompletionMutationGuardResult {
                expected_mutation: false,
                attempted_mutation: false,
                triggered: false,
            }
        );
    }

    #[test]
    fn exempts_an_explicit_completion_guard_false_agent() {
        let a = agent("worker", None, Some(false));
        let result = evaluate_completion_mutation_guard(&a, "Implement the approved fix", &[]);
        assert_eq!(
            result,
            CompletionMutationGuardResult {
                expected_mutation: false,
                attempted_mutation: false,
                triggered: false,
            }
        );
    }

    // ---- additional coverage mirroring the source's "stay conservative" and "does not trigger
    // when mutation observed" cases ----

    #[test]
    fn omitted_empty_bash_unknown_write_and_mcp_tool_capabilities_stay_conservative() {
        let task = "Implement the approved source fix";

        assert!(evaluate_completion_mutation_guard(&agent("architect", None, None), task, &[]).triggered);
        assert!(
            evaluate_completion_mutation_guard(&agent("architect", Some(vec![]), None), task, &[])
                .triggered
        );
        assert!(evaluate_completion_mutation_guard(
            &agent(
                "architect",
                Some(vec![
                    ToolRef::Builtin("read".to_string()),
                    ToolRef::Builtin("bash".to_string()),
                    ToolRef::Builtin("ls".to_string()),
                ]),
                None
            ),
            task,
            &[]
        )
        .triggered);
        assert!(evaluate_completion_mutation_guard(
            &agent(
                "architect",
                Some(vec![
                    ToolRef::Builtin("read".to_string()),
                    ToolRef::Builtin("custom_lookup".to_string()),
                ]),
                None
            ),
            task,
            &[]
        )
        .triggered);
        assert!(evaluate_completion_mutation_guard(
            &agent(
                "architect",
                Some(vec![
                    ToolRef::Builtin("read".to_string()),
                    ToolRef::Builtin("write".to_string()),
                ]),
                None
            ),
            task,
            &[]
        )
        .triggered);
        assert!(evaluate_completion_mutation_guard(
            &agent(
                "architect",
                Some(vec![
                    ToolRef::Builtin("read".to_string()),
                    ToolRef::Builtin("grep".to_string()),
                    ToolRef::Mcp("mcp:github.search".to_string()),
                ]),
                None
            ),
            task,
            &[]
        )
        .triggered);
    }

    #[test]
    fn worker_with_mutating_capable_tools_still_triggers_when_no_mutation_observed() {
        let a = agent(
            "worker",
            Some(vec![
                ToolRef::Builtin("read".to_string()),
                ToolRef::Builtin("edit".to_string()),
            ]),
            None,
        );
        let result = evaluate_completion_mutation_guard(&a, "Fix the test implementation", &[]);
        assert_eq!(
            result,
            CompletionMutationGuardResult {
                expected_mutation: true,
                attempted_mutation: false,
                triggered: true,
            }
        );
    }

    #[test]
    fn implementation_task_with_mutation_attempt_does_not_trigger() {
        let a = agent("worker", None, None);
        let events = vec![tool_start("edit", serde_json::json!({"path": "test.ts"}))];
        let result = evaluate_completion_mutation_guard(&a, "Fix the failing test", &events);
        assert!(!result.triggered);
    }

    #[test]
    fn scoped_no_edit_constraints_are_stripped_before_explicit_no_edit_check() {
        // "Do not edit files outside this repo" must NOT suppress mutation-expectation on its
        // own (R-SA-034 exemption is for a blanket "do not edit", not a scoped constraint) — this
        // exercises stripScopedNoEditConstraints running before matches_explicit_no_edit.
        assert!(expects_implementation_mutation(
            "worker",
            "Implement the fix. Do not edit files outside this repo."
        ));
        assert!(expects_implementation_mutation(
            "worker",
            "Implement the fix. Do not edit unrelated files."
        ));
    }

    #[test]
    fn declares_only_read_only_tools_matches_source_semantics() {
        assert!(!declares_only_read_only_tools(&agent("a", None, None)));
        assert!(!declares_only_read_only_tools(&agent("a", Some(vec![]), None)));
        assert!(declares_only_read_only_tools(&agent(
            "a",
            Some(vec![ToolRef::Builtin("read".to_string())]),
            None
        )));
        assert!(!declares_only_read_only_tools(&agent(
            "a",
            Some(vec![
                ToolRef::Builtin("read".to_string()),
                ToolRef::Builtin("edit".to_string())
            ]),
            None
        )));
        assert!(!declares_only_read_only_tools(&agent(
            "a",
            Some(vec![
                ToolRef::Builtin("read".to_string()),
                ToolRef::Mcp("mcp:foo".to_string())
            ]),
            None
        )));
    }
}
