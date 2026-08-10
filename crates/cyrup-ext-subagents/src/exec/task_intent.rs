//! Shared task mutation-intent classifier — a faithful port of
//! `pi-subagents/src/runs/shared/task-intent.ts` @v0.43.0 (G83).
//!
//! Upstream's own module doc states the contract this file reproduces
//! (`task-intent.ts:1-18`):
//!
//! > Single authority for reading a task's wording, answering two different questions from one
//! > prohibition analysis:
//! >
//! > - `classifyTaskMutationIntent` / `expectsImplementationMutation`: does the task REQUIRE file
//! >   changes? Consumed by the completion mutation guard, which blocks completion, so its
//! >   vocabulary is deliberately narrow.
//! > - `taskMayMutate`: COULD the task plausibly change files? Consumed by acceptance level
//! >   inference, which only raises evidence gates, so its vocabulary is deliberately broad (any
//! >   bare write verb).
//! >
//! > Explicit read-only wording ("do not modify", "review only") is a task-level intent only when
//! > no write imperative survives outside those phrases. A task like "Do not modify tests;
//! > implement the fix" is an implementation task with a scoped constraint, not a read-only task.
//!
//! # Why this is a separate module
//!
//! Upstream split this classifier out of `completion-guard.ts` into its own file and re-exports
//! `expectsImplementationMutation` from the guard (`completion-guard.ts:3,5`). This module mirrors
//! that split exactly, and [`crate::exec::completion_guard`] re-exports
//! [`expects_implementation_mutation`] for the same reason: the guard is one of two consumers, the
//! other being acceptance-level inference (`exec/acceptance.rs`'s `infer_level`, upstream
//! `acceptance.ts:86-109`), which needs [`task_may_mutate`] and the three-valued
//! [`TaskMutationIntent`] the guard alone never sees.
//!
//! # Regex-free porting
//!
//! This crate has no `regex` dependency (see [`crate::exec::completion_guard`]'s module doc for the
//! standing rationale), so every source `RegExp` is reproduced as a small purpose-built matcher
//! over LOWERCASED text — every pattern in `task-intent.ts` carries the `/i` flag, so lowercasing
//! once up front is equivalent. Matchers follow one shape: `fn(text, at) -> Option<end>` answering
//! "does this pattern match starting exactly here", driven by [`find_all`], which reproduces a `/g`
//! regex's left-to-right non-overlapping scan. Each Rust matcher names its source pattern in a doc
//! comment so the mapping stays auditable.

use crate::exec::completion_guard::is_word_char;

// ===============================================================================================
// Low-level scanning primitives (the `\b`, literal, alternation and `\s+` atoms every source
// pattern below is built from)
// ===============================================================================================

/// `\b` immediately before byte offset `i`, for a pattern whose next character is a word
/// character: true when the preceding character is absent or is not a word character.
fn boundary_before(text: &str, i: usize) -> bool {
    text.get(..i)
        .and_then(|s| s.chars().next_back())
        .is_none_or(|c| !is_word_char(c))
}

/// `\b` immediately after byte offset `i`, for a pattern whose previous character is a word
/// character: true when the following character is absent or is not a word character.
fn boundary_after(text: &str, i: usize) -> bool {
    text.get(i..)
        .and_then(|s| s.chars().next())
        .is_none_or(|c| !is_word_char(c))
}

/// Literal match of `needle` at `i`; returns the end offset.
fn lit(text: &str, i: usize, needle: &str) -> Option<usize> {
    text.get(i..)
        .filter(|rest| rest.starts_with(needle))
        .map(|_| i + needle.len())
}

/// Regex alternation `(?:a|b|c)` at `i` — FIRST alternative that matches wins, exactly as a
/// JavaScript `RegExp` alternation does (order in the source pattern is therefore preserved
/// verbatim in every `&[&str]` below).
fn alt(text: &str, i: usize, options: &[&str]) -> Option<usize> {
    options.iter().find_map(|option| lit(text, i, option))
}

/// Alternation followed by `\b` — `(?:a|b)\b`.
fn alt_word(text: &str, i: usize, options: &[&str]) -> Option<usize> {
    options
        .iter()
        .find_map(|option| lit(text, i, option).filter(|end| boundary_after(text, *end)))
}

/// `\s*` — the end offset after skipping zero or more whitespace characters.
fn ws0(text: &str, i: usize) -> usize {
    let mut end = i;
    for ch in text.get(i..).unwrap_or("").chars() {
        if ch.is_whitespace() {
            end += ch.len_utf8();
        } else {
            break;
        }
    }
    end
}

/// `\s+` — `None` when no whitespace is present at `i`.
fn ws1(text: &str, i: usize) -> Option<usize> {
    let end = ws0(text, i);
    (end > i).then_some(end)
}

/// A pattern matcher: does this source pattern match `text` starting exactly at `at`, and if so
/// where does the whole match end?
type Matcher = fn(&str, usize) -> Option<usize>;

/// A `/g` regex's non-overlapping left-to-right scan: at every character boundary, try `matcher`;
/// on a match, record it and resume from the match end (or one character on, for a zero-width
/// match, exactly as `String.prototype.replace` advances `lastIndex`).
fn find_all(text: &str, matcher: Matcher) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i <= text.len() {
        if !text.is_char_boundary(i) {
            i += 1;
            continue;
        }
        if let Some(end) = matcher(text, i) {
            out.push((i, end));
            i = if end > i { end } else { i + 1 };
        } else {
            i += 1;
        }
    }
    out
}

/// True iff `matcher` matches anywhere in `text` — the `pattern.test(text)` half of the source.
fn any_match(text: &str, matcher: Matcher) -> bool {
    let mut i = 0usize;
    while i <= text.len() {
        if !text.is_char_boundary(i) {
            i += 1;
            continue;
        }
        if matcher(text, i).is_some() {
            return true;
        }
        i += 1;
    }
    false
}

/// Replace every range in `ranges` (already sorted, non-overlapping) with a single space.
fn replace_ranges(text: &str, ranges: &[(usize, usize)], with: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    for &(start, end) in ranges {
        if start < cursor {
            continue;
        }
        out.push_str(text.get(cursor..start).unwrap_or(""));
        out.push_str(with);
        cursor = end;
    }
    out.push_str(text.get(cursor..).unwrap_or(""));
    out
}

/// Source: `stripPatterns(task, patterns)` (`task-intent.ts:103-110`). Applies each pattern IN
/// ORDER, each as a `/g` replace-with-`" "` over the text produced by the previous pattern —
/// sequential application, not a single merged pass, because that is what `for (const pattern of
/// patterns) stripped = stripped.replace(...)` actually does and it is observable whenever two
/// patterns in the same group can overlap (e.g. `write a report` vs `produce findings only`).
fn strip_patterns(text: &str, matchers: &[Matcher]) -> String {
    let mut current = text.to_string();
    for matcher in matchers {
        let ranges = find_all(&current, *matcher);
        if ranges.is_empty() {
            continue;
        }
        current = replace_ranges(&current, &ranges, " ");
    }
    current
}

// ===============================================================================================
// Pattern groups — one Rust matcher per source array entry, in source order
// ===============================================================================================

/// Source: `REVIEW_ONLY_PATTERNS` (`task-intent.ts:20-25`) — `/\breview only\b/i`,
/// `/\bsuggest fixes only\b/i`, `/\bonly return findings\b/i`, `/\breturn findings only\b/i`.
fn m_review_only(text: &str, i: usize) -> Option<usize> {
    if !boundary_before(text, i) {
        return None;
    }
    alt_word(
        text,
        i,
        &[
            "review only",
            "suggest fixes only",
            "only return findings",
            "return findings only",
        ],
    )
}

/// Source: `NO_TOOL_INTENT_PATTERNS` (`task-intent.ts:53-59`) — `/\bno tools? needed\b/i`,
/// `/\bno tools? required\b/i`, `/\bwithout using tools\b/i`, `/\bdo not use tools\b/i`,
/// `/\bdon't use tools\b/i`. (`tools?` is greedy, so the plural form is listed first within each
/// pair.)
fn m_no_tool_intent(text: &str, i: usize) -> Option<usize> {
    if !boundary_before(text, i) {
        return None;
    }
    alt_word(
        text,
        i,
        &[
            "no tools needed",
            "no tool needed",
            "no tools required",
            "no tool required",
            "without using tools",
            "do not use tools",
            "don't use tools",
        ],
    )
}

/// Source: `SCOPED_NO_EDIT_CONSTRAINT_PATTERNS` (`task-intent.ts:45-51`) —
/// `/\bdo not edit files?\s+outside\b/i`, `/\bdo not edit\s+outside\b/i`,
/// `/\bdo not edit\s+unrelated files?\b/i`, `/\bdo not change\s+unrelated files?\b/i`,
/// `/\bdo not modify\s+unrelated files?\b/i`.
///
/// These are CONSTRAINTS on an implementation task's blast radius, never a task-level read-only
/// intent, so `classifyTaskMutationIntent`/`taskMayMutate` excise them before any prohibition
/// analysis runs.
fn m_scoped_no_edit_constraint(text: &str, i: usize) -> Option<usize> {
    if !boundary_before(text, i) {
        return None;
    }
    // `\bdo not edit files?\s+outside\b` / `\bdo not edit\s+outside\b`
    if let Some(after_verb) = lit(text, i, "do not edit") {
        let after_files = alt(text, after_verb, &[" files", " file"]).unwrap_or(after_verb);
        if let Some(after_ws) = ws1(text, after_files)
            && let Some(end) = lit(text, after_ws, "outside")
            && boundary_after(text, end)
        {
            return Some(end);
        }
        if let Some(after_ws) = ws1(text, after_verb)
            && let Some(end) = lit(text, after_ws, "outside")
            && boundary_after(text, end)
        {
            return Some(end);
        }
    }
    // `\bdo not (edit|change|modify)\s+unrelated files?\b`
    for prefix in ["do not edit", "do not change", "do not modify"] {
        if let Some(after_verb) = lit(text, i, prefix)
            && let Some(after_ws) = ws1(text, after_verb)
            && let Some(end) = alt_word(text, after_ws, &["unrelated files", "unrelated file"])
        {
            return Some(end);
        }
    }
    None
}

/// Source: `READ_ONLY_DELIVERABLE_PATTERNS` first entry (`task-intent.ts:62`):
/// `/\b(?:draft|write|compose|prepare|produce)\s+(?:(?:a|an|the)\s+)?(?:github\s+)?(?:issue|bug report|issue draft|issue body|proposal|plan|report|summary|findings?|analysis|recommendations?)\b/i`
fn m_read_only_deliverable_verb_noun(text: &str, i: usize) -> Option<usize> {
    const NOUNS: &[&str] = &[
        "issue",
        "bug report",
        "issue draft",
        "issue body",
        "proposal",
        "plan",
        "report",
        "summary",
        "findings",
        "finding",
        "analysis",
        "recommendations",
        "recommendation",
    ];
    if !boundary_before(text, i) {
        return None;
    }
    let after_verb = alt_word(text, i, &["draft", "write", "compose", "prepare", "produce"])?;
    let after_verb_ws = ws1(text, after_verb)?;
    // `(?:(?:a|an|the)\s+)?` then `(?:github\s+)?`, each optional and each requiring its own `\s+`.
    let mut cursor = after_verb_ws;
    if let Some(after_article) = alt_word(text, cursor, &["a", "an", "the"])
        && let Some(after_ws) = ws1(text, after_article)
    {
        cursor = after_ws;
    }
    if let Some(after_github) = lit(text, cursor, "github")
        && boundary_after(text, after_github)
        && let Some(after_ws) = ws1(text, after_github)
    {
        cursor = after_ws;
    }
    alt_word(text, cursor, NOUNS)
}

/// Source: `READ_ONLY_DELIVERABLE_PATTERNS` second entry (`task-intent.ts:63`):
/// `/\b(?:issue|bug report)\s+(?:draft|body|template)\b/i`
fn m_read_only_deliverable_issue_form(text: &str, i: usize) -> Option<usize> {
    if !boundary_before(text, i) {
        return None;
    }
    let after_head = alt_word(text, i, &["issue", "bug report"])?;
    let after_ws = ws1(text, after_head)?;
    alt_word(text, after_ws, &["draft", "body", "template"])
}

/// Source: `READ_ONLY_DELIVERABLE_PATTERNS` third entry (`task-intent.ts:64`):
/// `/\b(?:return|provide|produce)\s+(?:text|markdown|answer|findings?|recommendations?)\s+only\b/i`
fn m_read_only_deliverable_text_only(text: &str, i: usize) -> Option<usize> {
    if !boundary_before(text, i) {
        return None;
    }
    let after_verb = alt_word(text, i, &["return", "provide", "produce"])?;
    let after_verb_ws = ws1(text, after_verb)?;
    let after_noun = alt_word(
        text,
        after_verb_ws,
        &[
            "text",
            "markdown",
            "answer",
            "findings",
            "finding",
            "recommendations",
            "recommendation",
        ],
    )?;
    let after_noun_ws = ws1(text, after_noun)?;
    let end = lit(text, after_noun_ws, "only")?;
    boundary_after(text, end).then_some(end)
}

const READ_ONLY_DELIVERABLE_PATTERNS: &[Matcher] = &[
    m_read_only_deliverable_verb_noun,
    m_read_only_deliverable_issue_form,
    m_read_only_deliverable_text_only,
];

/// Source: `taskHasReadOnlyDeliverable(taskText)` (`task-intent.ts:134-136`).
fn task_has_read_only_deliverable(text: &str) -> bool {
    READ_ONLY_DELIVERABLE_PATTERNS
        .iter()
        .any(|matcher| any_match(text, *matcher))
}

/// Source: `RESEARCH_AGENT_PATTERNS` (`task-intent.ts:67-71`) — `/\binvestigate\b/i`,
/// `/\bscout\b/i`, `/\bresearch(?:er)?\b/i`. Tested against the AGENT name, never the task text.
/// `research(?:er)?` is greedy, so `researcher` is listed first.
fn is_research_agent(agent_lower: &str) -> bool {
    fn matcher(text: &str, i: usize) -> Option<usize> {
        if !boundary_before(text, i) {
            return None;
        }
        alt_word(text, i, &["investigate", "scout", "researcher", "research"])
    }
    any_match(agent_lower, matcher)
}

/// Source: `isReviewerStyleAgent(agent)` (`task-intent.ts:138-140`) —
/// `/\b(?:advisor|reviewer|oracle)\b/i`.
fn is_reviewer_style_agent(agent_lower: &str) -> bool {
    fn matcher(text: &str, i: usize) -> Option<usize> {
        if !boundary_before(text, i) {
            return None;
        }
        alt_word(text, i, &["advisor", "reviewer", "oracle"])
    }
    any_match(agent_lower, matcher)
}

// -----------------------------------------------------------------------------------------------
// FIX_OR_PATCH_IMPLEMENTATION_PATTERN (`task-intent.ts:73`)
// -----------------------------------------------------------------------------------------------

/// The pronoun branch's alternatives: `(?:it|this|that|them|each|any|all|these|those)\b`.
const FIX_TARGET_PRONOUNS: &[&str] = &[
    "it", "this", "that", "them", "each", "any", "all", "these", "those",
];

/// `(?:(?:a|an|the|any|all)\s+)?` — the optional article before the adjective run.
const FIX_TARGET_ARTICLES: &[&str] = &["a", "an", "the", "any", "all"];

/// `(?:(?:failing|failed|…|compiler)\s+)*` — the repeatable adjective run. `type-?script` and
/// `type-?check` are expanded to both spellings, longest first.
const FIX_TARGET_ADJECTIVES: &[&str] = &[
    "failing",
    "failed",
    "broken",
    "flaky",
    "red",
    "cold",
    "start",
    "current",
    "existing",
    "reported",
    "approved",
    "known",
    "regression",
    "unit",
    "integration",
    "e2e",
    "source",
    "typescript",
    "type-script",
    "ts",
    "type-check",
    "typecheck",
    "compiler",
];

/// The required head noun. `issues?`/`problems?`/… are expanded plural-first (`s?` is greedy);
/// `docs?` → `docs`/`doc`; `lint(?:ing)?` → `linting`/`lint`; `type-?check` → both spellings; and
/// `type\s+checking` is handled separately in [`fix_target_noun`] because it contains whitespace.
const FIX_TARGET_NOUNS: &[&str] = &[
    "bug",
    "defect",
    "issues",
    "issue",
    "problems",
    "problem",
    "failures",
    "failure",
    "regressions",
    "regression",
    "tests",
    "test",
    "errors",
    "error",
    "items",
    "item",
    "typos",
    "typo",
    "code",
    "source",
    "implementation",
    "component",
    "function",
    "module",
    "class",
    "method",
    "logic",
    "file",
    "files",
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
    "linting",
    "lint",
    "build",
    "ci",
    "type-check",
    "typecheck",
];

/// The head-noun alternation, including the whitespace-bearing `type\s+checking` alternative.
fn fix_target_noun(text: &str, i: usize) -> Option<usize> {
    if let Some(end) = alt_word(text, i, FIX_TARGET_NOUNS) {
        return Some(end);
    }
    let after_type = lit(text, i, "type")?;
    let after_ws = ws1(text, after_type)?;
    let end = lit(text, after_ws, "checking")?;
    boundary_after(text, end).then_some(end)
}

/// `(?:adjective\s+)*noun\b`, with the backtracking a greedy regex quantifier performs: the
/// adjective and noun vocabularies OVERLAP (`source`, `regression`, `type-check`, …), so
/// `fix source` must be able to consume `source` as the NOUN after the adjective run matched zero
/// times.
fn fix_target_adjectives_then_noun(text: &str, i: usize) -> Option<usize> {
    if let Some(end) = fix_target_noun(text, i) {
        return Some(end);
    }
    for adjective in FIX_TARGET_ADJECTIVES {
        if let Some(after_adjective) = lit(text, i, adjective)
            && boundary_after(text, after_adjective)
            && let Some(after_ws) = ws1(text, after_adjective)
            && let Some(end) = fix_target_adjectives_then_noun(text, after_ws)
        {
            return Some(end);
        }
    }
    None
}

/// The whole object of a `fix`/`patch`: the pronoun branch, or the
/// optional-article + adjective-run + noun branch.
fn fix_target(text: &str, i: usize) -> Option<usize> {
    if let Some(end) = alt_word(text, i, FIX_TARGET_PRONOUNS) {
        return Some(end);
    }
    if let Some(end) = fix_target_adjectives_then_noun(text, i) {
        return Some(end);
    }
    for article in FIX_TARGET_ARTICLES {
        if let Some(after_article) = lit(text, i, article)
            && boundary_after(text, after_article)
            && let Some(after_ws) = ws1(text, after_article)
            && let Some(end) = fix_target_adjectives_then_noun(text, after_ws)
        {
            return Some(end);
        }
    }
    None
}

/// Source: `FIX_OR_PATCH_IMPLEMENTATION_PATTERN` (`task-intent.ts:73`). A bare `fix`/`patch` is
/// NOT implementation intent on its own — upstream deliberately requires a recognizable OBJECT, so
/// `Patch src/auth.ts` classifies `unknown` (`test/unit/task-intent.test.ts:11-13`) while
/// `patch the bug` classifies `implementation`.
fn m_fix_or_patch_implementation(text: &str, i: usize) -> Option<usize> {
    if !boundary_before(text, i) {
        return None;
    }
    let after_verb = alt_word(text, i, &["fix", "patch"])?;
    let after_ws = ws1(text, after_verb)?;
    fix_target(text, after_ws)
}

// -----------------------------------------------------------------------------------------------
// Shared implementation-verb sub-patterns
// -----------------------------------------------------------------------------------------------

/// `\bapply\s+(?:the\s+)?(?:(?:suggested|proposed|recommended)\s+)?(?:changes?|fix(?:es)?|patch)\b`
/// (`task-intent.ts:79`, `:87`).
fn m_apply_changes(text: &str, i: usize) -> Option<usize> {
    if !boundary_before(text, i) {
        return None;
    }
    let after_apply = lit(text, i, "apply").filter(|end| boundary_after(text, *end))?;
    let mut cursor = ws1(text, after_apply)?;
    if let Some(after_the) = lit(text, cursor, "the")
        && boundary_after(text, after_the)
        && let Some(after_ws) = ws1(text, after_the)
    {
        cursor = after_ws;
    }
    if let Some(after_adjective) = alt_word(text, cursor, &["suggested", "proposed", "recommended"])
        && let Some(after_ws) = ws1(text, after_adjective)
    {
        cursor = after_ws;
    }
    alt_word(
        text,
        cursor,
        &["changes", "change", "fixes", "fix", "patch"],
    )
}

/// `\bmake\s+(?:the\s+)?changes\b` (`task-intent.ts:80`, `:88`).
fn m_make_changes(text: &str, i: usize) -> Option<usize> {
    if !boundary_before(text, i) {
        return None;
    }
    let after_make = lit(text, i, "make").filter(|end| boundary_after(text, *end))?;
    let mut cursor = ws1(text, after_make)?;
    if let Some(after_the) = lit(text, cursor, "the")
        && boundary_after(text, after_the)
        && let Some(after_ws) = ws1(text, after_the)
    {
        cursor = after_ws;
    }
    alt_word(text, cursor, &["changes"])
}

/// `\bdo those fixes\b` (`task-intent.ts:81`, `:89`).
fn m_do_those_fixes(text: &str, i: usize) -> Option<usize> {
    if !boundary_before(text, i) {
        return None;
    }
    alt_word(text, i, &["do those fixes"])
}

/// `\b(?:update|add|remove|replace|create)\b(?!\s+(?:(?:a|an|the)\s+)?(?:report|summary|findings?)(?:\b|$))`
/// (`task-intent.ts:78`) — a bare write verb, REJECTED when what follows it is a report-like
/// deliverable (`Create a summary`, `Add a report`).
fn m_worker_bare_write_verb(text: &str, i: usize) -> Option<usize> {
    if !boundary_before(text, i) {
        return None;
    }
    let end = alt_word(text, i, &["update", "add", "remove", "replace", "create"])?;
    // The negative lookahead needs `\s+` to even apply; with no whitespace after the verb it
    // cannot exclude this occurrence.
    let Some(after_ws) = ws1(text, end) else {
        return Some(end);
    };
    let mut cursor = after_ws;
    if let Some(after_article) = alt_word(text, cursor, &["a", "an", "the"])
        && let Some(after_article_ws) = ws1(text, after_article)
    {
        cursor = after_article_ws;
    }
    // `(?:report|summary|findings?)(?:\b|$)` — `(?:\b|$)` is satisfied by `\b` alone here, since
    // every alternative ends in a word character and end-of-string is itself a `\b` there.
    let excluded = alt_word(text, cursor, &["report", "summary", "findings", "finding"]).is_some();
    (!excluded).then_some(end)
}

/// `\b(?:update|add|remove|replace|delete|create)\s+(?:the\s+)?(?:file|files|code|…|command)\b`
/// (`task-intent.ts:90`) — the GENERAL list's positive required-noun form (structurally the
/// opposite of the worker list's negative lookahead above).
fn m_general_write_verb_then_target(text: &str, i: usize) -> Option<usize> {
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
    if !boundary_before(text, i) {
        return None;
    }
    let after_verb = alt_word(
        text,
        i,
        &["update", "add", "remove", "replace", "delete", "create"],
    )?;
    let mut cursor = ws1(text, after_verb)?;
    if let Some(after_the) = lit(text, cursor, "the")
        && boundary_after(text, after_the)
        && let Some(after_ws) = ws1(text, after_the)
    {
        cursor = after_ws;
    }
    alt_word(text, cursor, NOUNS)
}

/// Source: `WORKER_IMPLEMENTATION_PATTERNS` (`task-intent.ts:75-82`).
fn m_worker_bare_verb(text: &str, i: usize) -> Option<usize> {
    if !boundary_before(text, i) {
        return None;
    }
    alt_word(
        text,
        i,
        &["implement", "edit", "modify", "refactor", "delete"],
    )
}

const WORKER_IMPLEMENTATION_PATTERNS: &[Matcher] = &[
    m_worker_bare_verb,
    m_fix_or_patch_implementation,
    m_worker_bare_write_verb,
    m_apply_changes,
    m_make_changes,
    m_do_those_fixes,
];

/// Source: `GENERAL_IMPLEMENTATION_PATTERNS` first entry (`task-intent.ts:85`) —
/// `/\b(?:implement|edit|modify|refactor)\b/i`. Note the general list, unlike the worker list, has
/// NO bare `delete`.
fn m_general_bare_verb(text: &str, i: usize) -> Option<usize> {
    if !boundary_before(text, i) {
        return None;
    }
    alt_word(text, i, &["implement", "edit", "modify", "refactor"])
}

const GENERAL_IMPLEMENTATION_PATTERNS: &[Matcher] = &[
    m_general_bare_verb,
    m_fix_or_patch_implementation,
    m_apply_changes,
    m_make_changes,
    m_do_those_fixes,
    m_general_write_verb_then_target,
];

// -----------------------------------------------------------------------------------------------
// REVIEWER_REQUIRED_EDIT_PATTERNS (`task-intent.ts:27-35`)
// -----------------------------------------------------------------------------------------------

/// The verb alternation shared by `must\s+…`, `required\s+to\s+…` and `always\s+…`.
const REVIEWER_EDIT_VERBS: &[&str] = &[
    "edit",
    "modify",
    "change",
    "fix",
    "patch",
    "apply",
    "implement",
];

/// `\bmust\s+(?:edit|modify|change|fix|patch|apply|implement)\b` (`task-intent.ts:28`).
fn m_reviewer_must_verb(text: &str, i: usize) -> Option<usize> {
    if !boundary_before(text, i) {
        return None;
    }
    let after = lit(text, i, "must").filter(|end| boundary_after(text, *end))?;
    let after_ws = ws1(text, after)?;
    alt_word(text, after_ws, REVIEWER_EDIT_VERBS)
}

/// `\brequired\s+to\s+(?:edit|…|implement)\b` (`task-intent.ts:29`).
fn m_reviewer_required_to_verb(text: &str, i: usize) -> Option<usize> {
    if !boundary_before(text, i) {
        return None;
    }
    let after = lit(text, i, "required").filter(|end| boundary_after(text, *end))?;
    let after_ws = ws1(text, after)?;
    let after_to = lit(text, after_ws, "to").filter(|end| boundary_after(text, *end))?;
    let after_to_ws = ws1(text, after_to)?;
    alt_word(text, after_to_ws, REVIEWER_EDIT_VERBS)
}

/// `\balways\s+(?:edit|…|implement)\b` (`task-intent.ts:32`).
fn m_reviewer_always_verb(text: &str, i: usize) -> Option<usize> {
    if !boundary_before(text, i) {
        return None;
    }
    let after = lit(text, i, "always").filter(|end| boundary_after(text, *end))?;
    let after_ws = ws1(text, after)?;
    alt_word(text, after_ws, REVIEWER_EDIT_VERBS)
}

/// `(?:^|[.!?\n]\s*)implement\s+(?:the\s+)?(?:approved|requested|specified|file|code|source|fix(?:es)?|changes?)\b`
/// (`task-intent.ts:30`) — a SENTENCE-INITIAL `implement …` directive. The leading alternation is
/// part of the match, so the matcher anchors on it rather than on the verb.
///
/// BOTH alternatives are tried at offset 0, in source order (`^` first, then `[.!?\n]\s*`): a JS
/// alternation backtracks into its second branch when the first fails, so a task text that BEGINS
/// with `.`/`!`/`?`/newline still matches through the punctuation branch. That shape is not
/// hypothetical here — `stripFrameworkInstructions` deletes a leading `[Read from: …]` /
/// `[Write to: …]` marker line (the prefix `spawn/chain_graph.rs::build_chain_instructions` and
/// `registration/slash_commands.rs` both prepend as `"…]\n\n{task}"`), which leaves the surviving
/// text starting with the newline that separated them.
fn m_reviewer_sentence_initial_implement(text: &str, i: usize) -> Option<usize> {
    // `^` — only at offset 0, and only when the flags have no `m` (they do not).
    if i == 0
        && let Some(end) = implement_mandate_object_at(text, i)
    {
        return Some(end);
    }
    // `[.!?\n]\s*`
    let after_punctuation = alt(text, i, &[".", "!", "?", "\n"])?;
    implement_mandate_object_at(text, ws0(text, after_punctuation))
}

/// `implement\s+(?:the\s+)?(?:approved|requested|specified|file|code|source|fix(?:es)?|changes?)\b`
/// — the part of `task-intent.ts:30` that follows its leading `(?:^|[.!?\n]\s*)` alternation.
fn implement_mandate_object_at(text: &str, at: usize) -> Option<usize> {
    let after_verb = lit(text, at, "implement").filter(|end| boundary_after(text, *end))?;
    let mut cursor = ws1(text, after_verb)?;
    if let Some(after_the) = lit(text, cursor, "the")
        && boundary_after(text, after_the)
        && let Some(after_ws) = ws1(text, after_the)
    {
        cursor = after_ws;
    }
    alt_word(
        text,
        cursor,
        &[
            "approved",
            "requested",
            "specified",
            "file",
            "code",
            "source",
            "fixes",
            "fix",
            "changes",
            "change",
        ],
    )
}

/// `\bregardless\s+of\s+findings\b` (`task-intent.ts:31`).
fn m_reviewer_regardless_of_findings(text: &str, i: usize) -> Option<usize> {
    if !boundary_before(text, i) {
        return None;
    }
    let after = lit(text, i, "regardless").filter(|end| boundary_after(text, *end))?;
    let after_ws = ws1(text, after)?;
    let after_of = lit(text, after_ws, "of").filter(|end| boundary_after(text, *end))?;
    let after_of_ws = ws1(text, after_of)?;
    alt_word(text, after_of_ws, &["findings"])
}

/// `\bapply\s+(?:the\s+)?fix(?:es)?\s+directly\b` (`task-intent.ts:33`).
fn m_reviewer_apply_fix_directly(text: &str, i: usize) -> Option<usize> {
    if !boundary_before(text, i) {
        return None;
    }
    let after_apply = lit(text, i, "apply").filter(|end| boundary_after(text, *end))?;
    let mut cursor = ws1(text, after_apply)?;
    if let Some(after_the) = lit(text, cursor, "the")
        && boundary_after(text, after_the)
        && let Some(after_ws) = ws1(text, after_the)
    {
        cursor = after_ws;
    }
    let after_fix = alt_word(text, cursor, &["fixes", "fix"])?;
    let after_fix_ws = ws1(text, after_fix)?;
    alt_word(text, after_fix_ws, &["directly"])
}

/// `\bmake\s+(?:the\s+)?code\s+changes\b` (`task-intent.ts:34`).
fn m_reviewer_make_code_changes(text: &str, i: usize) -> Option<usize> {
    if !boundary_before(text, i) {
        return None;
    }
    let after_make = lit(text, i, "make").filter(|end| boundary_after(text, *end))?;
    let mut cursor = ws1(text, after_make)?;
    if let Some(after_the) = lit(text, cursor, "the")
        && boundary_after(text, after_the)
        && let Some(after_ws) = ws1(text, after_the)
    {
        cursor = after_ws;
    }
    let after_code = lit(text, cursor, "code").filter(|end| boundary_after(text, *end))?;
    let after_code_ws = ws1(text, after_code)?;
    alt_word(text, after_code_ws, &["changes"])
}

const REVIEWER_REQUIRED_EDIT_PATTERNS: &[Matcher] = &[
    m_reviewer_must_verb,
    m_reviewer_required_to_verb,
    m_reviewer_sentence_initial_implement,
    m_reviewer_regardless_of_findings,
    m_reviewer_always_verb,
    m_reviewer_apply_fix_directly,
    m_reviewer_make_code_changes,
];

/// Source: `MAY_MUTATE_VERB_PATTERN` (`task-intent.ts:168`) —
/// `/\b(?:fix|implement|update|write|edit|modify|migrate|delete|remove|refactor|commit)\b/i`.
/// Deliberately BROADER than the implementation vocabulary above: it feeds acceptance-level
/// inference, which only ever raises evidence gates.
fn m_may_mutate_verb(text: &str, i: usize) -> Option<usize> {
    if !boundary_before(text, i) {
        return None;
    }
    alt_word(
        text,
        i,
        &[
            "fix",
            "implement",
            "update",
            "write",
            "edit",
            "modify",
            "migrate",
            "delete",
            "remove",
            "refactor",
            "commit",
        ],
    )
}

// ===============================================================================================
// The no-edit prohibition analysis (`task-intent.ts:37-43`, `:121-132`)
// ===============================================================================================

/// Characters the prohibition's captured object may NOT contain: `[^.;,:!?\n–—-]`. Hyphen and both
/// dashes end the object so `Do not modify tests — implement the fix` leaves the follow-on clause
/// intact for write-intent testing.
fn is_prohibition_object_terminator(ch: char) -> bool {
    matches!(ch, '.' | ';' | ',' | ':' | '!' | '?' | '\n' | '–' | '—' | '-')
}

/// The `(?!\b(?:but|and|then)\b)` half of the object capture: true when a coordinating word starts
/// exactly at `i` at word boundaries, which ENDS the object.
fn at_coordinating_word(text: &str, i: usize) -> bool {
    boundary_before(text, i) && alt_word(text, i, &["but", "and", "then"]).is_some()
}

/// One `NO_EDIT_PROHIBITION_PATTERN` match: the whole match's byte range plus the byte range of
/// its captured object (regex capture group 1).
#[derive(Debug, Clone, Copy)]
struct ProhibitionMatch {
    start: usize,
    end: usize,
    object_start: usize,
    object_end: usize,
}

/// Source: `NO_EDIT_PROHIBITION_PATTERN` (`task-intent.ts:40`):
/// ```text
/// /\b(?:do not|don't|must not)\s+(?:edit|modify|write(?:\s+to)?|touch|change)\b((?:(?!\b(?:but|and|then)\b)[^.;,:!?\n–—-])*)/gi
/// ```
/// Upstream's own comment on it: *"The prohibition's object ends at punctuation or at a
/// coordinating word (but/and/then), so a follow-on clause like 'but implement the fix' stays in
/// the text for write-intent testing instead of being swallowed as the object."*
fn prohibition_match_at(text: &str, i: usize) -> Option<ProhibitionMatch> {
    if !boundary_before(text, i) {
        return None;
    }
    let after_prefix = alt(text, i, &["do not", "don't", "must not"])?;
    let after_ws = ws1(text, after_prefix)?;
    // `(?:edit|modify|write(?:\s+to)?|touch|change)\b` — alternation order preserved; the greedy
    // `(?:\s+to)?` prefers `write to` and falls back to bare `write`.
    let after_verb = if let Some(end) = alt_word(text, after_ws, &["edit", "modify"]) {
        end
    } else if let Some(after_write) = lit(text, after_ws, "write") {
        if !boundary_after(text, after_write) {
            return None;
        }
        ws1(text, after_write)
            .and_then(|ws| lit(text, ws, "to"))
            .filter(|end| boundary_after(text, *end))
            .unwrap_or(after_write)
    } else {
        alt_word(text, after_ws, &["touch", "change"])?
    };

    let object_start = after_verb;
    let mut cursor = after_verb;
    while let Some(ch) = text.get(cursor..).and_then(|s| s.chars().next()) {
        if is_prohibition_object_terminator(ch) || at_coordinating_word(text, cursor) {
            break;
        }
        cursor += ch.len_utf8();
    }
    Some(ProhibitionMatch {
        start: i,
        end: cursor,
        object_start,
        object_end: cursor,
    })
}

/// Objects of a no-edit prohibition that mean "the codebase in general" rather than a named scope.
/// Source: `GENERIC_PROHIBITION_OBJECT` (`task-intent.ts:43`):
/// ```text
/// /^\s*(?:(?:any|all|the|these|those|your|our|existing|project|product|source|sources|config|configs|repo|repository)[\s/,-]*)*(?:files?|code|codebase|sources?|anything|repo(?:sitory)?)?\s*$/i
/// ```
/// A generic object makes the prohibition BLANKET, which wins over any write verb elsewhere in the
/// task; a named scope (`tests`, `vendor/`, `the production database`) leaves it a mere constraint.
fn is_generic_prohibition_object(object: &str) -> bool {
    const QUALIFIERS: &[&str] = &[
        "any",
        "all",
        "the",
        "these",
        "those",
        "your",
        "our",
        "existing",
        "project",
        "product",
        "sources",
        "source",
        "configs",
        "config",
        "repository",
        "repo",
    ];
    const HEAD_NOUNS: &[&str] = &[
        "files",
        "file",
        "codebase",
        "code",
        "sources",
        "source",
        "anything",
        "repository",
        "repo",
    ];

    /// `(?:files?|code|codebase|sources?|anything|repo(?:sitory)?)?\s*$` — the optional head noun
    /// plus end-of-string.
    fn tail_matches(rest: &str) -> bool {
        if rest.chars().all(char::is_whitespace) {
            return true;
        }
        HEAD_NOUNS.iter().any(|noun| {
            rest.strip_prefix(noun)
                .is_some_and(|tail| tail.chars().all(char::is_whitespace))
        })
    }

    /// `(?:(?:qualifier)[\s/,-]*)*` followed by the tail, with regex backtracking (each qualifier
    /// consumes at least one character, so the recursion always terminates).
    fn loop_matches(rest: &str, qualifiers: &[&str]) -> bool {
        if tail_matches(rest) {
            return true;
        }
        qualifiers.iter().any(|qualifier| {
            rest.strip_prefix(qualifier).is_some_and(|tail| {
                let after_separators =
                    tail.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '/' | ',' | '-'));
                loop_matches(after_separators, qualifiers)
            })
        })
    }

    let lowered = object.to_lowercase();
    loop_matches(lowered.trim_start(), QUALIFIERS)
}

/// Source: `NoEditProhibitionAnalysis` (`task-intent.ts:112-119`).
struct NoEditProhibitionAnalysis {
    /// True when any prohibition (or review-only/no-tool wording) is present.
    present: bool,
    /// True when a prohibition covers files/code in general; such wording wins over write verbs.
    blanket: bool,
    /// Task text with prohibition phrases and their objects removed, for write-intent testing.
    stripped_text: String,
}

/// Source: `analyzeNoEditProhibitions(taskText)` (`task-intent.ts:121-132`).
fn analyze_no_edit_prohibitions(task_text_lower: &str) -> NoEditProhibitionAnalysis {
    let mut present = any_match(task_text_lower, m_review_only)
        || any_match(task_text_lower, m_no_tool_intent);
    let mut blanket = present;
    let stripped = strip_patterns(task_text_lower, &[m_review_only, m_no_tool_intent]);

    // The prohibition replace runs with a CALLBACK upstream, so it both records `present`/`blanket`
    // and performs the replacement in one `/g` pass.
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut i = 0usize;
    while i <= stripped.len() {
        if !stripped.is_char_boundary(i) {
            i += 1;
            continue;
        }
        if let Some(found) = prohibition_match_at(&stripped, i) {
            present = true;
            if let Some(object) = stripped.get(found.object_start..found.object_end)
                && is_generic_prohibition_object(object)
            {
                blanket = true;
            }
            ranges.push((found.start, found.end));
            i = if found.end > found.start {
                found.end
            } else {
                found.start + 1
            };
        } else {
            i += 1;
        }
    }
    let stripped_text = if ranges.is_empty() {
        stripped
    } else {
        replace_ranges(&stripped, &ranges, " ")
    };

    NoEditProhibitionAnalysis {
        present,
        blanket,
        stripped_text,
    }
}

// ===============================================================================================
// Framework-instruction stripping (`task-intent.ts:95-101`)
// ===============================================================================================

/// `/^\s*\[(?:Write to|Read from):/i`
fn is_write_read_marker_line(line: &str) -> bool {
    let lower = line.trim_start().to_lowercase();
    lower.starts_with("[write to:") || lower.starts_with("[read from:")
}

/// The second `stripFrameworkInstructions` filter (`task-intent.ts:99`): every alternative of
/// ```text
/// /^\s*(?:Create and maintain progress at:|Update progress at:|\*\*Output:\*\*|Write your findings to(?: exactly this path)?:|Return the complete artifact in your final response\.|The runtime will persist it to exactly this path:|Do not call contact_supervisor merely because no write-capable tool is available\.|This path is authoritative for this run\.|Ignore any other output filename or output path mentioned elsewhere)/i
/// ```
/// The last three alternatives are the READ-ONLY delivery instruction
/// `exec/output.rs::format_output_path_instruction` emits (upstream `formatOutputPathInstruction`,
/// `single-output.ts:84-97`); without them a read-only child's own injected output instruction
/// would contribute `write`/`persist` vocabulary to its task classification.
fn is_progress_or_output_instruction_line(line: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "create and maintain progress at:",
        "update progress at:",
        "**output:**",
        "write your findings to exactly this path:",
        "write your findings to:",
        "return the complete artifact in your final response.",
        "the runtime will persist it to exactly this path:",
        "do not call contact_supervisor merely because no write-capable tool is available.",
        "this path is authoritative for this run.",
        "ignore any other output filename or output path mentioned elsewhere",
    ];
    let lower = line.trim_start().to_lowercase();
    PREFIXES.iter().any(|prefix| lower.starts_with(prefix))
}

/// Source: `stripFrameworkInstructions(task)` (`task-intent.ts:95-101`) — drops orchestrator-
/// injected scaffolding lines so their own vocabulary never contributes classification signal.
fn strip_framework_instructions(task: &str) -> String {
    task.lines()
        .filter(|line| !is_write_read_marker_line(line))
        .filter(|line| !is_progress_or_output_instruction_line(line))
        .collect::<Vec<_>>()
        .join("\n")
}

// ===============================================================================================
// Public API
// ===============================================================================================

/// Source: `TaskMutationIntent` (`task-intent.ts:93`) — `{ kind: "implementation" } | { kind:
/// "read-only" } | { kind: "unknown" }`.
///
/// The three-valued shape is load-bearing for acceptance inference, which treats `Unknown`
/// differently from `ReadOnly`: only `Unknown` falls through to the keyword read-only probe
/// (`acceptance.ts:91-92`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskMutationIntent {
    /// The task REQUIRES file changes.
    Implementation,
    /// The task is explicitly read-only, or reads as a findings-only deliverable.
    ReadOnly,
    /// Neither established.
    Unknown,
}

/// Source: `hasImplementationIntent(agent, taskText)` (`task-intent.ts:142-146`).
///
/// `agent === "worker"` is an EXACT, case-sensitive comparison upstream, so it is one here too;
/// the reviewer-style and research probes are `/i` and therefore take the lowercased name.
fn has_implementation_intent(agent: &str, agent_lower: &str, task_text_lower: &str) -> bool {
    if is_reviewer_style_agent(agent_lower) {
        return REVIEWER_REQUIRED_EDIT_PATTERNS
            .iter()
            .any(|matcher| any_match(task_text_lower, *matcher));
    }
    if agent == "worker" {
        return WORKER_IMPLEMENTATION_PATTERNS
            .iter()
            .any(|matcher| any_match(task_text_lower, *matcher));
    }
    GENERAL_IMPLEMENTATION_PATTERNS
        .iter()
        .any(|matcher| any_match(task_text_lower, *matcher))
}

/// Source: `classifyTaskMutationIntent(agent, task)` (`task-intent.ts:148-161`).
///
/// The ORDER is the classifier's substance and is reproduced exactly:
///
/// 1. Strip framework instructions, then strip scoped no-edit constraints.
/// 2. Analyze no-edit prohibitions. If ANY is present:
///    - a BLANKET prohibition (its object names files/code in general) → `ReadOnly`, and write
///      verbs elsewhere do not rescue it;
///    - otherwise re-test implementation intent against the text with the prohibition phrases and
///      their objects removed → `Implementation` when a write imperative survives, else `ReadOnly`.
/// 3. A research-style AGENT NAME → `ReadOnly`.
/// 4. Implementation intent in the (scoped-constraint-stripped) task → `Implementation`.
/// 5. A reviewer-style AGENT NAME → `ReadOnly`.
/// 6. A read-only DELIVERABLE ("write a report") → `ReadOnly`, else `Unknown`.
///
/// Note step 2 runs BEFORE step 3: a researcher handed `Do not modify tests; implement the fix`
/// classifies `Implementation`, because the surviving write imperative is evaluated before the
/// agent name is ever consulted.
#[must_use]
pub fn classify_task_mutation_intent(agent: &str, task: &str) -> TaskMutationIntent {
    let agent_lower = agent.to_lowercase();
    let task_text = strip_framework_instructions(task).to_lowercase();
    let task_text_without_scoped_constraints =
        strip_patterns(&task_text, &[m_scoped_no_edit_constraint]);
    let prohibitions = analyze_no_edit_prohibitions(&task_text_without_scoped_constraints);
    if prohibitions.present {
        if prohibitions.blanket {
            return TaskMutationIntent::ReadOnly;
        }
        return if has_implementation_intent(agent, &agent_lower, &prohibitions.stripped_text) {
            TaskMutationIntent::Implementation
        } else {
            TaskMutationIntent::ReadOnly
        };
    }

    if is_research_agent(&agent_lower) {
        return TaskMutationIntent::ReadOnly;
    }
    if has_implementation_intent(agent, &agent_lower, &task_text) {
        return TaskMutationIntent::Implementation;
    }
    if is_reviewer_style_agent(&agent_lower) {
        return TaskMutationIntent::ReadOnly;
    }
    if task_has_read_only_deliverable(&task_text_without_scoped_constraints) {
        TaskMutationIntent::ReadOnly
    } else {
        TaskMutationIntent::Unknown
    }
}

/// Source: `expectsImplementationMutation(agent, task)` (`task-intent.ts:163-165`) —
/// `classifyTaskMutationIntent(...).kind === "implementation"`.
#[must_use]
pub fn expects_implementation_mutation(agent: &str, task: &str) -> bool {
    classify_task_mutation_intent(agent, task) == TaskMutationIntent::Implementation
}

/// Source: `taskMayMutate(task)` (`task-intent.ts:176-181`).
///
/// Upstream's own doc: *"Whether the task could plausibly change files. Blanket prohibitions win;
/// write verbs inside a scoped prohibition's object or a read-only deliverable phrase ('write a
/// summary report') do not count; any other bare write verb does."*
///
/// Note this consults NO agent name at all — it is a pure property of the task wording.
#[must_use]
pub fn task_may_mutate(task: &str) -> bool {
    let task_text = strip_patterns(
        &strip_framework_instructions(task).to_lowercase(),
        &[m_scoped_no_edit_constraint],
    );
    let prohibitions = analyze_no_edit_prohibitions(&task_text);
    if prohibitions.blanket {
        return false;
    }
    let without_deliverables = strip_patterns(
        &prohibitions.stripped_text,
        READ_ONLY_DELIVERABLE_PATTERNS,
    );
    any_match(&without_deliverables, m_may_mutate_verb)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    // ===========================================================================================
    // Table tests transcribed from upstream's OWN cases,
    // `pi-subagents:v0.43.0:test/unit/task-intent.test.ts`
    // ===========================================================================================

    /// `test/unit/task-intent.test.ts:6-9` — "keeps write imperatives despite investigative
    /// wording".
    #[test]
    fn keeps_write_imperatives_despite_investigative_wording() {
        for (agent, task) in [
            ("worker", "Inspect the failure and implement the fix"),
            ("worker", "Research the current code path and patch the bug"),
        ] {
            assert_eq!(
                classify_task_mutation_intent(agent, task),
                TaskMutationIntent::Implementation,
                "{task:?}"
            );
        }
    }

    /// `test/unit/task-intent.test.ts:11-13` — "does not broaden the shared completion-guard
    /// classifier for role-only path patches".
    #[test]
    fn a_bare_path_patch_is_unknown_not_implementation() {
        assert_eq!(
            classify_task_mutation_intent("worker", "Patch src/auth.ts"),
            TaskMutationIntent::Unknown
        );
    }

    /// `test/unit/task-intent.test.ts:15-19` — "treats scoped no-edit constraints as constraints,
    /// not task intent".
    #[test]
    fn scoped_no_edit_constraints_are_constraints_not_task_intent() {
        for task in [
            "Do not modify tests; implement the fix",
            "Fix the bug. Do not edit files outside src/.",
            "Must not touch the production database; implement the fix locally",
        ] {
            assert_eq!(
                classify_task_mutation_intent("worker", task),
                TaskMutationIntent::Implementation,
                "{task:?}"
            );
        }
    }

    /// `test/unit/task-intent.test.ts:21-34` — "stops the prohibition object before a following
    /// implementation clause". Includes the en dash and em dash cases verbatim.
    #[test]
    fn the_prohibition_object_stops_before_a_following_implementation_clause() {
        for task in [
            "Do not modify tests but implement the fix",
            "Do not modify tests and implement the fix",
            "Do not modify tests: implement the fix",
            "Do not modify tests? Implement the fix",
            "Do not modify tests - implement the fix",
            "Do not modify tests – implement the fix",
            "Do not modify tests — implement the fix",
        ] {
            assert_eq!(
                classify_task_mutation_intent("worker", task),
                TaskMutationIntent::Implementation,
                "{task:?}"
            );
        }
        assert_eq!(
            classify_task_mutation_intent("worker", "Do not modify tests and fixtures"),
            TaskMutationIntent::ReadOnly
        );
    }

    /// `test/unit/task-intent.test.ts:36-42` — "lets blanket no-edit prohibitions win over write
    /// verbs".
    #[test]
    fn blanket_no_edit_prohibitions_win_over_write_verbs() {
        for (agent, task) in [
            ("worker", "Implement this. Do not edit files."),
            ("worker", "Do not edit files. Tell me how to fix the bug."),
            (
                "worker",
                "Report on the extraction pipeline. Do not modify project/source files.",
            ),
            (
                "reviewer",
                "Final correctness review after prior fixes. Inspect all changed files and tests. Do not modify project/source files. Report findings.",
            ),
            (
                "worker",
                "Verification-only task. Do not edit product/source/config files.\n   Run a disposable check, delete its temporary harness, and retain only\n   a sanitized report at an explicitly named artifact path.",
            ),
        ] {
            assert_eq!(
                classify_task_mutation_intent(agent, task),
                TaskMutationIntent::ReadOnly,
                "{task:?}"
            );
        }
    }

    /// `test/unit/task-intent.test.ts:44-47` — "strips repeated prohibition phrases before testing
    /// write intent".
    #[test]
    fn repeated_prohibition_phrases_are_all_stripped_before_write_intent_testing() {
        assert_eq!(
            classify_task_mutation_intent(
                "worker",
                "Do not modify vendor/. Do not modify generated/. Summarize the build."
            ),
            TaskMutationIntent::ReadOnly
        );
        assert_eq!(
            classify_task_mutation_intent(
                "worker",
                "Do not modify vendor/. Do not modify generated/. Implement the fix in src/."
            ),
            TaskMutationIntent::Implementation
        );
    }

    /// `test/unit/task-intent.test.ts:49-57` — "classifies research agents and reviewer-style tasks
    /// as read-only". Note `advisor` and `oracle` are reviewer-style agent names upstream; before
    /// this port only `reviewer` was.
    #[test]
    fn research_and_reviewer_style_agents_classify_read_only_unless_edits_are_required() {
        for (agent, task) in [
            ("researcher", "Research this and patch the bug"),
            ("reviewer", "Review this and fix any real issues"),
            (
                "oracle",
                "Review findings and determine what to implement with playbooks instead of before",
            ),
            (
                "advisor",
                "Review findings and determine what to implement with playbooks instead of before",
            ),
        ] {
            assert_eq!(
                classify_task_mutation_intent(agent, task),
                TaskMutationIntent::ReadOnly,
                "{agent:?} / {task:?}"
            );
        }
        for (agent, task) in [
            (
                "reviewer",
                "Review this; regardless of findings, apply changes directly",
            ),
            ("oracle", "Implement the approved file changes"),
            ("advisor", "Implement the approved file changes"),
        ] {
            assert_eq!(
                classify_task_mutation_intent(agent, task),
                TaskMutationIntent::Implementation,
                "{agent:?} / {task:?}"
            );
        }
    }

    /// `test/unit/task-intent.test.ts:59-62` — "keeps report-writing deliverables read-only". Note
    /// the two cases differ: `Write a report …` is `read-only` (a READ_ONLY_DELIVERABLE match)
    /// while `Create a summary` is `unknown` (excluded from implementation by the negative
    /// lookahead, but matching no deliverable pattern either).
    #[test]
    fn report_writing_deliverables_stay_read_only() {
        assert_eq!(
            classify_task_mutation_intent("worker", "Write a report on the API"),
            TaskMutationIntent::ReadOnly
        );
        assert_eq!(
            classify_task_mutation_intent("worker", "Create a summary"),
            TaskMutationIntent::Unknown
        );
    }

    /// `test/unit/task-intent.test.ts:64-67` — "expectsImplementationMutation mirrors the
    /// classifier".
    #[test]
    fn expects_implementation_mutation_mirrors_the_classifier() {
        assert!(expects_implementation_mutation(
            "worker",
            "Do not modify tests; implement the fix"
        ));
        assert!(!expects_implementation_mutation(
            "worker",
            "Review the diff and suggest fixes only. Do not edit files."
        ));
    }

    /// `test/unit/task-intent.test.ts:71-75` — "treats any bare write verb as write-capable".
    #[test]
    fn task_may_mutate_treats_any_bare_write_verb_as_write_capable() {
        for task in [
            "Write the code",
            "Commit the changes",
            "Delete temporary data",
            "Remove obsolete assets",
            "Update dependencies",
        ] {
            assert!(task_may_mutate(task), "{task:?}");
        }
    }

    /// `test/unit/task-intent.test.ts:77-81` — "does not count verbs inside prohibitions or
    /// read-only deliverables".
    #[test]
    fn task_may_mutate_ignores_verbs_inside_prohibitions_and_read_only_deliverables() {
        assert!(!task_may_mutate(
            "Do not modify project/source files. Report findings."
        ));
        assert!(!task_may_mutate("Write a report on the API"));
        assert!(!task_may_mutate("Summarize the build output"));
    }

    /// `test/unit/task-intent.test.ts:83-86` — "keeps verbs that survive outside a scoped
    /// prohibition".
    #[test]
    fn task_may_mutate_keeps_verbs_surviving_outside_a_scoped_prohibition() {
        assert!(task_may_mutate("Do not modify tests but implement the fix"));
        assert!(task_may_mutate("Do not modify tests; update the parser"));
    }

    // ===========================================================================================
    // Component-level coverage for the pieces upstream exercises only indirectly
    // ===========================================================================================

    #[test]
    fn generic_prohibition_objects_are_recognized_by_shape() {
        // Blanket: the object names files/code in general.
        for object in [
            " files",
            " all files",
            " the code",
            " project/source files",
            " product/source/config files",
            " any existing sources",
            " the repo",
            " the repository",
            " anything",
            "",
            "   ",
        ] {
            assert!(is_generic_prohibition_object(object), "{object:?}");
        }
        // Scoped: the object names a specific target.
        for object in [
            " tests",
            " vendor/",
            " generated/",
            " the production database",
            " unrelated files in vendor",
            " tests and fixtures",
        ] {
            assert!(!is_generic_prohibition_object(object), "{object:?}");
        }
    }

    #[test]
    fn framework_instruction_lines_are_stripped_including_the_read_only_delivery_form() {
        // The read-only delivery instruction (`formatOutputPathInstruction`'s no-write-tool
        // branch) must not leak `write`/`persist` vocabulary into the classification.
        let task = "Analyze this\n\n---\n**Output:**\nReturn the complete artifact in your final response.\nThe runtime will persist it to exactly this path: /tmp/report.md\nDo not call contact_supervisor merely because no write-capable tool is available.\nThis path is authoritative for this run.\nIgnore any other output filename or output path mentioned elsewhere, including output destinations in the base agent prompt, system prompt, or task instructions.";
        let stripped = strip_framework_instructions(task);
        assert!(!stripped.contains("persist"), "{stripped:?}");
        assert!(!stripped.contains("**Output:**"), "{stripped:?}");
        assert!(!stripped.contains("contact_supervisor"), "{stripped:?}");
        assert_eq!(
            classify_task_mutation_intent("worker", task),
            TaskMutationIntent::Unknown
        );
        assert!(!task_may_mutate(task));
    }

    #[test]
    fn no_tool_intent_wording_is_a_blanket_read_only_signal() {
        for task in [
            "Answer from memory. No tools needed.",
            "Answer from memory. No tool required.",
            "Explain the design without using tools.",
            "Do not use tools; implement the fix",
        ] {
            assert_eq!(
                classify_task_mutation_intent("worker", task),
                TaskMutationIntent::ReadOnly,
                "{task:?}"
            );
            assert!(!task_may_mutate(task), "{task:?}");
        }
    }

    #[test]
    fn fix_or_patch_requires_a_recognizable_object() {
        // Objects that DO establish implementation intent.
        for task in [
            "fix it",
            "fix this",
            "Fix the failing test",
            "Fix the bug where no edits were made",
            "patch the bug",
            "fix the cold start regression",
            "fix source",
            "fix the type-check errors",
            "fix type checking",
            "fix linting",
            "fix package.json",
        ] {
            assert_eq!(
                classify_task_mutation_intent("architect", task),
                TaskMutationIntent::Implementation,
                "{task:?}"
            );
        }
        // Objects that do NOT.
        for task in ["Patch src/auth.ts", "fix everything downstream"] {
            assert_ne!(
                classify_task_mutation_intent("architect", task),
                TaskMutationIntent::Implementation,
                "{task:?}"
            );
        }
    }

    #[test]
    fn the_general_list_has_no_bare_delete_but_the_worker_list_does() {
        // `delete` is bare in WORKER_IMPLEMENTATION_PATTERNS (`task-intent.ts:76`) but only
        // verb+noun in GENERAL_IMPLEMENTATION_PATTERNS (`task-intent.ts:90`).
        assert_eq!(
            classify_task_mutation_intent("worker", "Delete the stale entries"),
            TaskMutationIntent::Implementation
        );
        assert_eq!(
            classify_task_mutation_intent("architect", "Delete the stale entries"),
            TaskMutationIntent::Unknown
        );
        assert_eq!(
            classify_task_mutation_intent("architect", "Delete the module"),
            TaskMutationIntent::Implementation
        );
    }

    #[test]
    fn apply_changes_accepts_the_suggested_proposed_recommended_qualifiers() {
        for task in [
            "Apply the suggested changes",
            "Apply proposed fixes",
            "Apply the recommended patch",
            "Apply the fix",
        ] {
            assert_eq!(
                classify_task_mutation_intent("architect", task),
                TaskMutationIntent::Implementation,
                "{task:?}"
            );
        }
    }

    #[test]
    fn reviewer_style_agents_need_an_explicit_edit_mandate() {
        for task in [
            "You must edit the affected files",
            "You are required to implement the fix",
            "Always modify the source when you find a defect",
            "Apply the fixes directly",
            "Make the code changes",
        ] {
            assert_eq!(
                classify_task_mutation_intent("advisor", task),
                TaskMutationIntent::Implementation,
                "{task:?}"
            );
        }
        // A mere write verb is NOT a mandate for a reviewer-style agent.
        assert_eq!(
            classify_task_mutation_intent("advisor", "Refactor the parser"),
            TaskMutationIntent::ReadOnly
        );
    }

    #[test]
    fn the_sentence_initial_implement_mandate_is_anchored() {
        assert_eq!(
            classify_task_mutation_intent("oracle", "Implement the approved file changes"),
            TaskMutationIntent::Implementation
        );
        assert_eq!(
            classify_task_mutation_intent(
                "oracle",
                "Review the plan. Implement the requested changes."
            ),
            TaskMutationIntent::Implementation
        );
        // Mid-sentence `implement …` is not a mandate.
        assert_eq!(
            classify_task_mutation_intent(
                "oracle",
                "Decide whether we should implement the approved changes"
            ),
            TaskMutationIntent::ReadOnly
        );
    }

    /// `(?:^|[.!?\n]\s*)` is an ALTERNATION, and a JS engine backtracks into its second branch at
    /// offset 0 when `^` leads nowhere. The port originally tried only `^` there, so a task text
    /// whose first character is one of `.!?\n` never matched at all.
    ///
    /// That shape is produced by this crate's own composers: `stripFrameworkInstructions` deletes a
    /// leading `[Read from: …]` / `[Write to: …]` marker line, and both
    /// `spawn/chain_graph.rs::build_chain_instructions` (`"[Read from: …]\n[Write to: …]\n\n"`,
    /// asserted at `chain_graph.rs:2516`) and `registration/slash_commands.rs:1252`
    /// (`"[Read from: …]\n\n{task}"`) prepend exactly that marker, leaving the surviving text
    /// beginning with the blank separator line.
    #[test]
    fn the_implement_mandate_matches_at_offset_zero_through_the_punctuation_branch() {
        for task in [
            "\nImplement the approved changes",
            ". Implement the approved changes",
            "! Implement the approved changes",
            "?Implement the requested fix",
        ] {
            assert_eq!(
                classify_task_mutation_intent("oracle", task),
                TaskMutationIntent::Implementation,
                "{task:?}"
            );
        }
        // The live composer shape: a framework marker line, a blank line, then the mandate.
        assert_eq!(
            classify_task_mutation_intent(
                "advisor",
                "[Read from: notes.md]\n[Write to: out.md]\n\nImplement the approved changes"
            ),
            TaskMutationIntent::Implementation
        );
    }

    #[test]
    fn a_prohibition_present_but_scoped_is_evaluated_before_the_agent_name() {
        // Step 2 runs BEFORE step 3, so a research-style agent handed a surviving write imperative
        // classifies `implementation` — the agent name never gets consulted.
        assert_eq!(
            classify_task_mutation_intent("researcher", "Do not modify tests; implement the fix"),
            TaskMutationIntent::Implementation
        );
        // ...but with no prohibition at all, the agent name wins.
        assert_eq!(
            classify_task_mutation_intent("researcher", "Implement the fix"),
            TaskMutationIntent::ReadOnly
        );
    }

    #[test]
    fn strip_patterns_applies_each_pattern_in_sequence() {
        // Two READ_ONLY_DELIVERABLE patterns can overlap ("produce findings only"); sequential
        // application means the FIRST pattern's match wins, exactly as the source's loop does.
        let stripped = strip_patterns("please produce findings only", READ_ONLY_DELIVERABLE_PATTERNS);
        assert!(!stripped.contains("findings"), "{stripped:?}");
        // ORDER-DISCRIMINATING: pattern 1 (`produce findings`) consumes less than pattern 3
        // (`produce findings only`), so applying them in source order leaves the trailing `only`
        // behind. Reversing the loop would consume it too — which is why the assertion above alone
        // could not tell a sequential pass from a reordered one.
        assert!(
            stripped.contains("only"),
            "pattern 1 must run before pattern 3: {stripped:?}"
        );
    }

    /// `NO_EDIT_PROHIBITION_PATTERN`'s object stops at `\b(?:but|and|then)\b`
    /// (`task-intent.ts:40`). Upstream's own cases cover `but` and `and` only, so `then` — the
    /// third alternative — needs its own case or the alternation is half-unverified.
    #[test]
    fn then_ends_the_prohibition_object_like_but_and_and_do() {
        assert_eq!(
            classify_task_mutation_intent("worker", "Do not modify tests then implement the fix"),
            TaskMutationIntent::Implementation
        );
        assert!(task_may_mutate("Do not modify tests then implement the fix"));
    }

    /// `RESEARCH_AGENT_PATTERNS` is `investigate|scout|research(?:er)?` (`task-intent.ts:67-71`),
    /// matched against the AGENT NAME. Upstream's own cases only ever use `researcher`.
    #[test]
    fn every_research_agent_name_alternative_forces_read_only() {
        for agent in ["investigate", "investigate-bot", "scout", "deep-scout", "research"] {
            assert_eq!(
                classify_task_mutation_intent(agent, "Implement the fix"),
                TaskMutationIntent::ReadOnly,
                "{agent:?}"
            );
        }
        // `\binvestigate\b` does NOT reach `investigator` — the trailing `\b` fails on the `o`.
        assert_eq!(
            classify_task_mutation_intent("investigator-bot", "Implement the fix"),
            TaskMutationIntent::Implementation
        );
    }
}
