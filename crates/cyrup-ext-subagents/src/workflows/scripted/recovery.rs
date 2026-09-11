//! The acceptance-recovery review classifier — pi `isExplicitReadOnlyRecoveryReview` and its
//! pattern table (`scripted-workflow.ts:1150-1320`): after a child returns REJECTED acceptance
//! recovery (`recovery.status === "available-for-review"` with reason
//! `acceptance-metadata-rejected`), only an explicitly read-only review child may launch, and this
//! module is the whole decision.
//!
//! # `fancy-regex`, and why
//!
//! Ten patterns use lookaround and three put a negative lookahead **inside a bounded repetition**
//! (a tempered greedy token, e.g. `:1185`'s
//! `(?:(?!\b(?:and|but|...)\b)[^,.;!?\n—–-]){0,80}`) — which has no "position after the match" to
//! check and therefore cannot be split for the `regex` crate. `fancy-regex` handles them verbatim,
//! costs zero new transitive crates (already resolved via `syntect`'s `default-fancy`), and
//! auto-delegates the lookaround-free majority to the linear-time `regex` engine
//! (`RegexImpl::Wrap`). [`build`] sets `backtrack_limit` explicitly; every match/replace fails
//! CLOSED (see the helpers) because the input is model-authored prompt text and the `{0,80}`
//! tempered tokens are exactly the shape that backtracks.
//!
//! # The three scrub pipelines — measured from source, in source order
//!
//! The decision is a scrubbing pipeline, not a list of tests: three separately scrubbed copies of
//! `task`, each `.replace(pattern, " ")` substituting a SINGLE space, later patterns seeing
//! earlier substitutions. Measured at pi HEAD `7fe9dee1` the pipelines are **12 / 24 / 30**
//! patterns (`:1195-1207` / `:1208-1229` / `:1230-1255`) — SCOPE_3f §4.4's "12 / 21 / 25" predates
//! the last upstream growth of the two larger pipelines; the SOURCE is the authority ("port the
//! bytes"), so the source counts are what this module ships and tests.
//!
//! # Flags
//!
//! Every `.test()` pattern upstream is `/i`, never `/g` (`:1155-1156`, `:1187-1190`), so JS's
//! `lastIndex` statefulness never applies and `is_match` is a faithful port. `/g` on the scrub
//! patterns is the CALL FORM here (`replace_all`), not pattern text. The one runtime-built global
//! variant — `new RegExp(RECOVERY_REVIEW_READ_ONLY_PATTERN.source, "gi")` (`:1255`) — is the same
//! compiled [`static@READ_ONLY_PATTERN`] used both ways: `is_match` for the positive conjunct,
//! `replace_all` for the final mutation-text scrub.

use std::sync::LazyLock;

use fancy_regex::{Regex, RegexBuilder};
use serde_json::Value;

use crate::exec::task_intent::{TaskMutationIntent, classify_task_mutation_intent};

/// Compile one upstream pattern with an EXPLICIT backtracking budget. 1,000,000 is
/// `fancy-regex` 0.18's own default (`src/lib.rs:453`) — set here so the bound is a visible,
/// deliberate choice rather than an inherited one; exceeding it fails closed at every call site.
fn build(pattern: &str) -> Regex {
    RegexBuilder::new(pattern)
        .backtrack_limit(1_000_000)
        .build()
        .unwrap_or_else(|error| unreachable!("upstream classifier pattern must compile: {error}"))
}

/// `pattern.test(text)` for a POSITIVE conjunct: the classifier requires a match, so a
/// `BacktrackLimitExceeded` (or any engine error) fails CLOSED by reporting *no match* — the
/// candidate is then NOT an explicit read-only review.
fn matches_or_fail_closed_false(pattern: &Regex, text: &str) -> bool {
    pattern.is_match(text).unwrap_or(false)
}

/// `!pattern.test(text)` for a NEGATIVE conjunct: the classifier requires NO match, so an engine
/// error fails CLOSED by reporting a match — the candidate is then NOT an explicit read-only
/// review. The two helpers exist so neither polarity can silently fail OPEN.
fn matches_or_fail_closed_true(pattern: &Regex, text: &str) -> bool {
    pattern.is_match(text).unwrap_or(true)
}

/// One scrub pipeline: sequential `replace_all(pattern, " ")` in slice order — order matters,
/// later patterns see earlier substitutions (`:1195-1255`).
///
/// # Errors
///
/// Any engine error (backtrack budget) aborts the scrub; the caller fails closed.
fn scrub(task: &str, patterns: &[&LazyLock<Regex>]) -> Result<String, fancy_regex::Error> {
    let mut text = task.to_string();
    for pattern in patterns {
        // `try_replacen(.., 0, ..)` is `replace_all` with the error surfaced instead of swallowed.
        text = pattern.try_replacen(&text, 0, " ")?.into_owned();
    }
    Ok(text)
}
/// pi `RECOVERY_REVIEW_MUTATION_VERB_PATTERN` — flags `/i`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static MUTATION_VERB_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\b(?:add(?:ing)?|append(?:ing)?|apply(?:ing)?|cherry[ -]pick(?:ing)?|change|changing|clean(?:ing)?|commit(?:ting)?|cop(?:y|ying)|create|creating|delete|deleting|edit(?:ing)?|fix(?:ing)?|implement(?:ing)?|insert(?:ing)?|make|making|merge|merging|mov(?:e|ing)|modify(?:ing)?|mutate|mutating|open(?:ing)?|patch(?:ing)?|prepend(?:ing)?|push(?:ing)?|rebase|rebasing|refactor(?:ing)?|remove|removing|rename|renaming|replace|replacing|revert(?:ing)?|revise|revising|rewrite|rewriting|sav(?:e|ing)|stag(?:e|ing)|stash(?:ing)?|tag(?:ging)?|touch(?:ing)?|update|updating|write|writing)\b"##,
    )
});

/// pi `RECOVERY_REVIEW_READ_ONLY_PATTERN` — flags `/i`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static READ_ONLY_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\b(?:read[- ]only|review only|only return findings|return findings only|suggest fixes only|without\s+(?:editing|modifying|changing|writing|touching)|do not\s+(?:edit|modify|change|write|touch)|don't\s+(?:edit|modify|change|write|touch)|must not\s+(?:edit|modify|change|write|touch))\b"##,
    )
});

/// pi `RECOVERY_REVIEW_NO_MUTATION_CLAUSE_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static NO_MUTATION_CLAUSE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\b(?:do not|don't|must not)\s+(?:edit|modify|change|write|touch)(?:\s+files?)?(?:\s*,\s*(?:commit|push|comment|merge|launch(?:\s+subagents?)?)(?=\s*(?:,|\bor\b|[.;!?\n)]|$)))*(?:\s*,?\s*or\s+(?:commit|push|comment|merge|launch(?:\s+subagents?)?)(?=\s*(?:[.;!?\n)]|$)))?"##,
    )
});

/// pi `RECOVERY_REVIEW_DELIVERABLE_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static DELIVERABLE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\b(?:compose|create|draft|prepare|produce|write)\s+(?:(?:a|an|the|your)\s+)?(?:findings?|review|report|summary|analysis|recommendations?)(?:\s+(?:to|at|in)\s+\S+)?"##,
    )
});

/// pi `RECOVERY_REVIEW_CONTEXT_OBJECT_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static CONTEXT_OBJECT_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\breview\s+(?:(?:the|this|that|saved)\s+)?(?:patch|diff|changes?|implementation|report)\b"##,
    )
});

/// pi `RECOVERY_REVIEW_MUTATION_NOUN_CONTEXT_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static MUTATION_NOUN_CONTEXT_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| build(r##"(?i)\b(?:later\s+real\s+)?update\s+imperatives?\b"##));

/// pi `RECOVERY_REVIEW_PRIOR_FIX_CONTEXT_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static PRIOR_FIX_CONTEXT_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| build(r##"(?i)\bthe\s+prior\s+fix\s+keeps\b"##));

/// pi `RECOVERY_REVIEW_DETECTION_CONTEXT_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static DETECTION_CONTEXT_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\b(?:mutation\s+detection\s+now\s+includes\s+move\/rename\/copy\s+file\s+mutation\s+imperatives|delegation\s+detection\s+now\s+blocks\s+get\/let\/have\/tell\/ask\s+follow-up\s+forms)(?=[,.;!?\n]|\b(?:and|but|then|however|nevertheless|nonetheless|yet)\b|$)"##,
    )
});

/// pi `RECOVERY_REVIEW_PATTERN_CHANGE_CONTEXT_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static PATTERN_CHANGE_CONTEXT_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\bRECOVERY_REVIEW_MUTATION_VERB_PATTERN\s+now\s+includes\s+append,\s+prepend,\s+and\s+sav(?:e|ing)\b(?=\s*(?:[,.;!?\n)]|$))"##,
    )
});

/// pi `RECOVERY_REVIEW_GIT_PATTERN_CHANGE_CONTEXT_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static GIT_PATTERN_CHANGE_CONTEXT_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\bfixed\s+mutating\s+git\s+follow-up\s+bypasses\b|\b(?:added|adding)\s+[a-z][a-z-]*(?:\/[a-z][a-z-]*)*(?:(?:\s*,\s*(?:and\s+)?|\s+and\s+)[a-z][a-z-]*(?:\/[a-z][a-z-]*)*)*\s+to\s+the\s+(?:mutation\s+imperative|mutating\s+git\s+command)\s+pattern\b|\band\s+[a-z][a-z-]*(?:\/[a-z][a-z-]*)*(?:(?:\s*,\s*(?:and\s+)?|\s+and\s+)[a-z][a-z-]*(?:\/[a-z][a-z-]*)*)*\s+to\s+the\s+mutating\s+git\s+command\s+pattern\b"##,
    )
});

/// pi `RECOVERY_REVIEW_VALIDATION_EVIDENCE_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static VALIDATION_EVIDENCE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| build(r##"(?i)\bvalidation(?:\s+after\s+fix)?\s*:"##));

/// pi `RECOVERY_REVIEW_CONTRACT_PROHIBITION_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static CONTRACT_PROHIBITION_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\b(?:do not|don't|must not)\s+(?:mutate\s+durable\s+state|launch\s+(?:mutating|destructive|mutating\/destructive)\s+work)(?:\s+or\s+(?:mutate\s+durable\s+state|launch\s+(?:mutating|destructive|mutating\/destructive)\s+work))*"##,
    )
});

/// pi `RECOVERY_REVIEW_ONLY_REVIEW_CONTRACT_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static ONLY_REVIEW_CONTRACT_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\bmay\s+only\s+launch\s+explicit\s+read-only\s+review\s+children\s+with\s+acceptance:false\b"##,
    )
});

/// pi `RECOVERY_REVIEW_BLOCKED_CONTEXT_FRAGMENT_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static BLOCKED_CONTEXT_FRAGMENT_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\bstate\.set, runs\.host, runs\.steer, ordinary\/mutating children, and destructive command wording are blocked(?=[,.;!?\n)\]}]|$)"##,
    )
});

/// pi `RECOVERY_REVIEW_REGRESSION_EVIDENCE_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static REGRESSION_EVIDENCE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\b(?:existing regressions cover plain rm and git clean\/reset\/restore|prior regressions covering rm\/git clean as evidence only)(?=[,.;!?\n)\]}]|$)"##,
    )
});

/// pi `RECOVERY_REVIEW_BLOCKED_QUOTED_EXAMPLE_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static BLOCKED_QUOTED_EXAMPLE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)(?:`[^`\n]+`|'[^'\n]+'|"[^"\n]+")(?:(?:\s*,\s*(?:and\s+)?|\s+and\s+)(?:`[^`\n]+`|'[^'\n]+'|"[^"\n]+"))*\s+(?:is|are|remains?)\s+(?:blocked|(?:an?\s+)?blocked\s+examples?)(?=[,.;!?\n)\]}]|$)"##,
    )
});

/// pi `RECOVERY_REVIEW_CONTEXT_QUOTED_EXAMPLE_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static CONTEXT_QUOTED_EXAMPLE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\b(?:examples?|phrasing|forms|variants):\s*(?:`[^`\n]+`|'[^'\n]+'|"[^"\n]+")(?:(?:\s*,\s*(?:and\s+)?|\s+and\s+)(?:`[^`\n]+`|'[^'\n]+'|"[^"\n]+"))*"##,
    )
});

/// pi `RECOVERY_REVIEW_LISTED_BLOCKED_EXAMPLE_PATTERN` — flags `/gim`, ported as `(?im)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static LISTED_BLOCKED_EXAMPLE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?im)(?:^|[.;!?\n]\s*)blocked\s+examples:\s*(?:\r?\n[ \t]*(?:[-*]|\d+[.)])\s+[^\r\n]+)+"##,
    )
});

/// pi `RECOVERY_REVIEW_BLOCKS_QUOTED_EXAMPLE_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static BLOCKS_QUOTED_EXAMPLE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\b(?:this\s+)?blocks?\s+examples?\s+like\s+(?:`[^`\n]+`|'[^'\n]+'|"[^"\n]+")(?:(?:\s*,\s*(?:and\s+)?|\s+and\s+)(?:`[^`\n]+`|'[^'\n]+'|"[^"\n]+"))*"##,
    )
});

/// pi `RECOVERY_REVIEW_EXAMPLES_LIKE_BLOCKED_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static EXAMPLES_LIKE_BLOCKED_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\bexamples?\s+like\s+(?:`[^`\n]+`|'[^'\n]+'|"[^"\n]+")(?:(?:\s*,\s*(?:and\s+)?|\s+and\s+)(?:`[^`\n]+`|'[^'\n]+'|"[^"\n]+"))*\s+(?:is|are|remains?)\s+blocked\b"##,
    )
});

/// pi `RECOVERY_REVIEW_QUOTED_VISIBLE_BLOCKED_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static QUOTED_VISIBLE_BLOCKED_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)(?:\b(?:(?:the\s+)?examples?|commands?\s+hidden\s+in)\s+|(?:^|[\s(\[{]))(?:`[^`\n]+`|'[^'\n]+'|"[^"\n]+")(?:(?:\s*,\s*(?:and\s+|or\s+)?|\s+(?:and|or)\s+)(?:`[^`\n]+`|'[^'\n]+'|"[^"\n]+"))*\s+(?:(?:is|are|remains?)\s+)?visible\s+and\s+blocked\b"##,
    )
});

/// pi `RECOVERY_REVIEW_PROMPTS_LIKE_BLOCKED_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static PROMPTS_LIKE_BLOCKED_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\bdescribed\s+prompts?\s+like\s+Run\s+git\s+rebase\s+main,\s+git\s+rebase\s+main,\s+Run\s+git\s+cherry-pick\s+abc123,\s+cherry-pick\s+abc123,\s+and\s+Stage\s+the\s+changed\s+files\s+as\s+blocked\b"##,
    )
});

/// pi `RECOVERY_REVIEW_GIT_MUTATIONS_BROADER_CONTEXT_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static GIT_MUTATIONS_BROADER_CONTEXT_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\bpositive\s+git\s+mutations\s+are\s+broader:\s*git\s+branch\s+-D\s+old,\s+git\s+tag\s+-d\s+v1\.0,\s+git\s+stash,\s+git\s+revert\s+abc123,\s+and\s+natural\s+cherry\s+pick\s+abc123\s+now\s+trips?\s+the\s+recovery\s+barrier\b"##,
    )
});

/// pi `RECOVERY_REVIEW_GIT_COVERAGE_CONTEXT_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static GIT_COVERAGE_CONTEXT_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\bbroadened\s+positive\s+git\s+mutation\s+coverage\s+for\s+natural\s+`cherry\s+pick`,\s+`revert`,\s+`stash`,\s+`tag`,\s+plus\s+git\s+`branch\|revert\|stash\|tag`"##,
    )
});

/// pi `RECOVERY_REVIEW_REGRESSION_QUOTED_EXAMPLE_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static REGRESSION_QUOTED_EXAMPLE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\b(?:added\s+)?exact\s+regressions?\s+for\s+(?:`[^`\n]+`|'[^'\n]+'|"[^"\n]+")(?:(?:\s*,\s*(?:and\s+)?|\s+and\s+)(?:`[^`\n]+`|'[^'\n]+'|"[^"\n]+"))*"##,
    )
});

/// pi `RECOVERY_REVIEW_REGRESSION_ANAPHORIC_EXAMPLE_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static REGRESSION_ANAPHORIC_EXAMPLE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\b(?:added\s+)?exact\s+regressions?\s+for\s+(?:do|run|execute|perform|apply)\s+(?:it|that|this|(?:the\s+)?(?:(?:previous(?:ly)?|prior|above|quoted|blocked)\s+){0,4}(?:command|example|operation|action|phrase|instruction|request))(?:[\s,]+(?:now|still|again|really|actually|immediately)){0,3}[\s,]+anyway(?:(?:\s*,\s*(?:and\s+)?|\s+and\s+)(?:do|run|execute|perform|apply)\s+(?:it|that|this|(?:the\s+)?(?:(?:previous(?:ly)?|prior|above|quoted|blocked)\s+){0,4}(?:command|example|operation|action|phrase|instruction|request))(?:[\s,]+(?:now|still|again|really|actually|immediately)){0,3}[\s,]+anyway)*"##,
    )
});

/// pi `RECOVERY_REVIEW_REGRESSION_FOLLOWED_BY_ANAPHORIC_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static REGRESSION_FOLLOWED_BY_ANAPHORIC_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\badded\s+(?:exact\s+)?regressions?\s+for\s+quoted\s+rm\s+remaining\s+blocked\s+followed\s+by\s+execute\s+the\s+previous\s+command\b(?=\s*(?:[,.;!?\n)]|\bwhile\b|$))"##,
    )
});

/// pi `RECOVERY_REVIEW_REGRESSION_FOLLOWED_BY_NAMED_RM_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static REGRESSION_FOLLOWED_BY_NAMED_RM_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\b(?:added\s+)?(?:exact\s+)?regressions?\s+for\s+quoted\s+rm\s+remaining\s+blocked\s+followed\s+by\s+(?:do|run|execute|perform|apply)\s+(?:it|that|this|(?:the\s+)?(?:(?:previous(?:ly)?|prior|above|quoted|blocked)\s+){0,4}(?:command|example|operation|action|phrase|instruction|request))(?:[\s,]+(?:now|still|again|really|actually|immediately)){0,3}(?:[\s,]+anyway)?(?=\s*(?:[,.;!?\n)]|\bwhile\b|$))"##,
    )
});

/// pi `RECOVERY_REVIEW_BLOCKED_LIVE_VARIANT_CONTEXT_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static BLOCKED_LIVE_VARIANT_CONTEXT_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\b(?:while\s+)?keeping\s+live-command\s+variants\s+such\s+as\s+followed\s+by\s+execute\s+the\s+previous\s+command\s+then\s+update\s+tests\s+blocked\b(?=\s*(?:[,.;!?\n)]|$))"##,
    )
});

/// pi `RECOVERY_REVIEW_ANAPHORIC_REFERENCES_CONTEXT_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static ANAPHORIC_REFERENCES_CONTEXT_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\bdirect\s+anaphoric\s+references\s+now\s+include\s+numeric\s+and\s+word\s+ordinals\s+through\s+tenth\s+plus\s+one,\s+so\s+(?:do|run|execute|perform|apply)\s+(?:it|that|this|(?:the\s+)?(?:(?:\d+(?:st|nd|rd|th)|first|second|third|fourth|fifth|sixth|seventh|eighth|ninth|tenth|last|next|previous(?:ly)?|prior|above|quoted|blocked)\s+){0,4}(?:command|example|operation|action|phrase|instruction|request|one))(?:[\s,]+(?:right|now|still|again|really|actually|immediately)){0,4}[\s,]+anyway(?:(?:\s*,\s*(?:and\s+)?|\s+and\s+)(?:do|run|execute|perform|apply)\s+(?:it|that|this|(?:the\s+)?(?:(?:\d+(?:st|nd|rd|th)|first|second|third|fourth|fifth|sixth|seventh|eighth|ninth|tenth|last|next|previous(?:ly)?|prior|above|quoted|blocked)\s+){0,4}(?:command|example|operation|action|phrase|instruction|request|one))(?:[\s,]+(?:right|now|still|again|really|actually|immediately)){0,4}[\s,]+anyway)*\s+(?:is|are|remains?)\s+blocked\b(?:\s+after\s+quoted\s+destructive\s+examples\s+are\s+scrubbed)?"##,
    )
});

/// pi `RECOVERY_REVIEW_NO_ANAPHORIC_MUTATION_CLAUSE_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static NO_ANAPHORIC_MUTATION_CLAUSE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\b(?:do not|don't|must not)\s+(?:do|run|execute|perform|apply)\s+(?:it|that|this|(?:the\s+)?(?:(?:\d+(?:st|nd|rd|th)|first|second|third|fourth|fifth|sixth|seventh|eighth|ninth|tenth|last|next|previous(?:ly)?|prior|above|quoted|blocked)\s+){0,4}(?:command|example|operation|action|phrase|instruction|request|one))(?:(?!\b(?:and|but|then|however|nevertheless|nonetheless|yet)\b)[^,.;!?\n—–-]){0,80}"##,
    )
});

/// pi `RECOVERY_REVIEW_NO_DELEGATION_CLAUSE_PATTERN` — flags `/gi`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static NO_DELEGATION_CLAUSE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\b(?:do not|don't|must not)\s+(?:(?:launch|start|spawn|run)\b(?:(?!\b(?:and|but|then|however|nevertheless|nonetheless|yet)\b)[^,.;!?\n])*(?:workers?|reviewers?|agents?|subagents?|children|child|runs?)|(?:get|let|request|hand\s+off|assign|use|have|tell)\b(?:(?!\b(?:and|but|then|however|nevertheless|nonetheless|yet)\b)[^,.;!?\n])*(?:workers?|reviewers?|agents?|subagents?|children|child)|(?:get|let|have|tell|request)\b(?:(?!\b(?:and|but|then|however|nevertheless|nonetheless|yet)\b)[^,.;!?\n])*(?:review|implementation|fix(?:es)?|changes?|follow-up)\b(?:(?!\b(?:and|but|then|however|nevertheless|nonetheless|yet)\b)[^,.;!?\n])*\b(?:from|with|via|by)\b(?:(?!\b(?:and|but|then|however|nevertheless|nonetheless|yet)\b)[^,.;!?\n])*(?:workers?|reviewers?|agents?|subagents?|children|child)|ask\b(?:(?!\b(?:and|but|then|however|nevertheless|nonetheless|yet)\b)[^,.;!?\n])*(?:(?:workers?|reviewers?|agents?|subagents?|children|child)\b(?:(?!\b(?:and|but|then|however|nevertheless|nonetheless|yet)\b)[^,.;!?\n])*\b(?:continue|implement|review|fix|edit|write|modify|change|patch|update|delete|remove|create|follow-up)|(?:review|implementation|fix(?:es)?|changes?|follow-up)\b(?:(?!\b(?:and|but|then|however|nevertheless|nonetheless|yet)\b)[^,.;!?\n])*\b(?:from|via)\b(?:(?!\b(?:and|but|then|however|nevertheless|nonetheless|yet)\b)[^,.;!?\n])*(?:workers?|reviewers?|agents?|subagents?|children|child)))\b"##,
    )
});

/// pi `RECOVERY_REVIEW_DELEGATION_PATTERN` — flags `/i`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static DELEGATION_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\b(?:launch|start|spawn|run)\b[^,.;!?\n]*(?:workers?|reviewers?|agents?|subagents?|children|child|runs?)\b|\b(?:delegate|hand\s+off|assign)\s+(?:remediation|implementation(?:\s+follow-up)?|changes?|fix(?:es)?|follow-up)\s+to\s+(?:(?:a|an|the)\s+)?(?:workers?|reviewers?|agents?|subagents?|children|child)\b|\b(?:get|let|have|tell|request)\b[^,.;!?\n]*(?:workers?|reviewers?|agents?|subagents?|children|child)\b[^,.;!?\n]*\b(?:continue|implement|review|fix|edit|write|modify|change|patch|update|delete|remove|create|follow-up)\b|\b(?:get|let|have|tell|request)\b[^,.;!?\n]*(?:review|implementation|fix(?:es)?|changes?|follow-up)\b[^,.;!?\n]*\b(?:from|with|via|by)\b[^,.;!?\n]*(?:workers?|reviewers?|agents?|subagents?|children|child)\b|\bask\b[^,.;!?\n]*(?:(?:workers?|reviewers?|agents?|subagents?|children|child)\b[^,.;!?\n]*\b(?:continue|implement|review|fix|edit|write|modify|change|patch|update|delete|remove|create|follow-up)|(?:review|implementation|fix(?:es)?|changes?|follow-up)\b[^,.;!?\n]*\b(?:from|via)\b[^,.;!?\n]*(?:workers?|reviewers?|agents?|subagents?|children|child))\b|\buse\b[^,.;!?\n]*(?:workers?|reviewers?|agents?|subagents?|children|child|runs?)\s+(?:for|to)\s+(?:implementation|follow-up|remediation|fix|edit|write|modify|change|patch|update|delete|remove|create)\b"##,
    )
});

/// pi `RECOVERY_REVIEW_ANAPHORIC_MUTATION_PATTERN` — flags `/i`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static ANAPHORIC_MUTATION_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)\b(?:do|run|execute|perform|apply)\s+(?:it|that|this|(?:the\s+)?(?:(?:\d+(?:st|nd|rd|th)|first|second|third|fourth|fifth|sixth|seventh|eighth|ninth|tenth|last|next|previous(?:ly)?|prior|above|quoted|blocked)\s+){0,4}(?:command|example|operation|action|phrase|instruction|request|one))(?!(?:\s+(?:(?:now|still|also|already)\s+)*(?:is|are|remains?)\s+blocked\b))(?:(?:[\s,]+\w+){0,4}[\s,]+anyway|(?:(?!\b(?:and|but|then|however|nevertheless|nonetheless|yet)\b)[^,.;!?\n]){0,80})(?=\s*(?:[,.;!?\n)]|\b(?:and|but|then|however|nevertheless|nonetheless|yet)\b|$))"##,
    )
});

/// pi `RECOVERY_REVIEW_DESTRUCTIVE_COMMAND_PATTERN` — flags `/i`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static DESTRUCTIVE_COMMAND_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)(?:^|[\s;,.`'"(\[{])(?:\S*\/)?(?:rm|rmdir|unlink|truncate|mv|cp|chmod|chown)\b|\bgit\b(?:\s+(?:-[A-Za-z](?:\s+(?:"[^"\n]*"|'[^'\n]*'|\S+))?|--(?:git-dir|work-tree|namespace|exec-path|config-env)(?:=(?:"[^"\n]*"|'[^'\n]*'|\S+)|\s+(?:"[^"\n]*"|'[^'\n]*'|\S+))|--[A-Za-z0-9-]+(?:=(?:"[^"\n]*"|'[^'\n]*'|\S+))?))*\s+(?:add|branch|cherry-pick|clean|commit|merge|rebase|reset|restore|revert|stash|tag|checkout|switch)\b"##,
    )
});

/// pi `RECOVERY_REVIEW_DASH_LIVE_ACTION_PATTERN` — flags `/i`, ported as `(?i)`; `g` is expressed by
/// the call form (`replace_all`), never by pattern text. Literal `[` inside character classes
/// is `\\[`-escaped for the Rust engine's class syntax ([CYRUP-DELTA, syntax], same class).
static DASH_LIVE_ACTION_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    build(
        r##"(?i)(?:[—–]|--|\s-\s|:|\s\/\s)\s*(?:then\s+)?(?:(?:add|append|apply|change|cherry[ -]pick|clean|commit|copy|create|delete|edit|fix|implement|insert|make|merge|move|modify|mutate|open|patch|prepend|push|rebase|refactor|remove|rename|replace|revert|revise|rewrite|save|stage|stash|tag|touch|update|write)\b|(?:launch|start|spawn|run)\b[^,.;!?\n]*(?:workers?|reviewers?|agents?|subagents?|children|child|runs?)\b|(?:\S*\/)?(?:rm|rmdir|unlink|truncate|mv|cp|chmod|chown)\b|git\b[^,.;!?\n]*\b(?:add|branch|cherry-pick|clean|commit|merge|rebase|reset|restore|revert|stash|tag|checkout|switch)\b)"##,
    )
});

/// The 12-pattern `taskDestructiveCommandText` scrub (`:1195-1207`), in source order.
static DESTRUCTIVE_COMMAND_SCRUB: &[&LazyLock<Regex>] = &[
    &BLOCKED_CONTEXT_FRAGMENT_PATTERN,
    &REGRESSION_EVIDENCE_PATTERN,
    &BLOCKED_QUOTED_EXAMPLE_PATTERN,
    &CONTEXT_QUOTED_EXAMPLE_PATTERN,
    &LISTED_BLOCKED_EXAMPLE_PATTERN,
    &BLOCKS_QUOTED_EXAMPLE_PATTERN,
    &EXAMPLES_LIKE_BLOCKED_PATTERN,
    &QUOTED_VISIBLE_BLOCKED_PATTERN,
    &PROMPTS_LIKE_BLOCKED_PATTERN,
    &GIT_MUTATIONS_BROADER_CONTEXT_PATTERN,
    &GIT_COVERAGE_CONTEXT_PATTERN,
    &REGRESSION_FOLLOWED_BY_NAMED_RM_PATTERN,
];

/// The 24-pattern `taskDashLiveActionText` scrub (`:1208-1229`), in source order.
static DASH_LIVE_ACTION_SCRUB: &[&LazyLock<Regex>] = &[
    &DELIVERABLE_PATTERN,
    &CONTEXT_OBJECT_PATTERN,
    &MUTATION_NOUN_CONTEXT_PATTERN,
    &PRIOR_FIX_CONTEXT_PATTERN,
    &DETECTION_CONTEXT_PATTERN,
    &PATTERN_CHANGE_CONTEXT_PATTERN,
    &GIT_PATTERN_CHANGE_CONTEXT_PATTERN,
    &VALIDATION_EVIDENCE_PATTERN,
    &CONTRACT_PROHIBITION_PATTERN,
    &ONLY_REVIEW_CONTRACT_PATTERN,
    &BLOCKED_QUOTED_EXAMPLE_PATTERN,
    &CONTEXT_QUOTED_EXAMPLE_PATTERN,
    &LISTED_BLOCKED_EXAMPLE_PATTERN,
    &BLOCKS_QUOTED_EXAMPLE_PATTERN,
    &EXAMPLES_LIKE_BLOCKED_PATTERN,
    &QUOTED_VISIBLE_BLOCKED_PATTERN,
    &PROMPTS_LIKE_BLOCKED_PATTERN,
    &GIT_MUTATIONS_BROADER_CONTEXT_PATTERN,
    &GIT_COVERAGE_CONTEXT_PATTERN,
    &REGRESSION_QUOTED_EXAMPLE_PATTERN,
    &REGRESSION_ANAPHORIC_EXAMPLE_PATTERN,
    &REGRESSION_FOLLOWED_BY_ANAPHORIC_PATTERN,
    &BLOCKED_LIVE_VARIANT_CONTEXT_PATTERN,
    &ANAPHORIC_REFERENCES_CONTEXT_PATTERN,
];

/// The 30-pattern `taskMutationText` scrub (`:1230-1255`), in source order — the last entry is
/// the runtime-built global READ_ONLY variant (`:1255`), the same compiled pattern used as the
/// positive conjunct ("compile once, use both ways").
static MUTATION_SCRUB: &[&LazyLock<Regex>] = &[
    &DELIVERABLE_PATTERN,
    &CONTEXT_OBJECT_PATTERN,
    &MUTATION_NOUN_CONTEXT_PATTERN,
    &PRIOR_FIX_CONTEXT_PATTERN,
    &DETECTION_CONTEXT_PATTERN,
    &PATTERN_CHANGE_CONTEXT_PATTERN,
    &GIT_PATTERN_CHANGE_CONTEXT_PATTERN,
    &VALIDATION_EVIDENCE_PATTERN,
    &CONTRACT_PROHIBITION_PATTERN,
    &ONLY_REVIEW_CONTRACT_PATTERN,
    &BLOCKED_CONTEXT_FRAGMENT_PATTERN,
    &REGRESSION_EVIDENCE_PATTERN,
    &BLOCKED_QUOTED_EXAMPLE_PATTERN,
    &CONTEXT_QUOTED_EXAMPLE_PATTERN,
    &LISTED_BLOCKED_EXAMPLE_PATTERN,
    &BLOCKS_QUOTED_EXAMPLE_PATTERN,
    &EXAMPLES_LIKE_BLOCKED_PATTERN,
    &QUOTED_VISIBLE_BLOCKED_PATTERN,
    &PROMPTS_LIKE_BLOCKED_PATTERN,
    &GIT_MUTATIONS_BROADER_CONTEXT_PATTERN,
    &GIT_COVERAGE_CONTEXT_PATTERN,
    &REGRESSION_QUOTED_EXAMPLE_PATTERN,
    &REGRESSION_ANAPHORIC_EXAMPLE_PATTERN,
    &REGRESSION_FOLLOWED_BY_ANAPHORIC_PATTERN,
    &BLOCKED_LIVE_VARIANT_CONTEXT_PATTERN,
    &ANAPHORIC_REFERENCES_CONTEXT_PATTERN,
    &NO_ANAPHORIC_MUTATION_CLAUSE_PATTERN,
    &NO_DELEGATION_CLAUSE_PATTERN,
    &NO_MUTATION_CLAUSE_PATTERN,
    &READ_ONLY_PATTERN,
];

/// The reviewer-agent gate — upstream's INLINE `/\b(?:advisor|oracle|review|reviewer)\b/i`
/// (`:1271`), **four** alternatives. This is deliberately NOT
/// `crate::exec`'s `is_reviewer_style_agent` (a port of `task-intent.ts:138-140` with THREE
/// alternatives): since `\breview\b` requires a word boundary after `review`, the three-alternative
/// version does not match `"reviewer"` via `review` — it matches it via its own `reviewer`
/// alternative — but it REJECTS an agent literally named `review`. The four-alternative check is
/// strictly wider; "simplifying" this back onto the shared helper reintroduces that bug.
static RECOVERY_REVIEWER_AGENT_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| build(r"(?i)\b(?:advisor|oracle|review|reviewer)\b"));

/// pi `isAcceptanceMetadataRecovery` (`scripted-workflow.ts:1146-1149`): a failed child whose
/// recovery metadata is `available-for-review` / `acceptance-metadata-rejected` — the ONE result
/// shape that arms the recovery barrier.
#[must_use]
pub fn is_acceptance_metadata_recovery(
    result: &crate::workflows::WorkflowScriptChildResult,
) -> bool {
    !result.ok
        && result.recovery.as_ref().is_some_and(|recovery| {
            recovery.status == "available-for-review"
                && recovery.reason == "acceptance-metadata-rejected"
        })
}

/// pi `isExplicitReadOnlyRecoveryReview` (`scripted-workflow.ts:1192-1276`): may this launch
/// follow a rejected acceptance recovery? The nine-name conjunction (eleven `&&` terms, `:1264-
/// 1274`) over the three scrubbed texts, in source order, ending in the ALREADY-PORTED
/// [`classify_task_mutation_intent`] (`exec/task_intent.rs`; upstream imports it at `:7` and
/// applies it at `:1274` — called here, never re-derived).
///
/// Any scrub failure (backtracking budget) fails CLOSED: the launch is treated as NOT an explicit
/// read-only review, so the recovery barrier holds.
#[must_use]
pub fn is_explicit_read_only_recovery_review(params: &serde_json::Map<String, Value>) -> bool {
    let agent = params
        .get("agent")
        .and_then(Value::as_str)
        .map_or("", str::trim);
    let task = params
        .get("task")
        .and_then(Value::as_str)
        .map_or("", str::trim);
    // `params.acceptance === false` — strict boolean false, not absence.
    if params.get("acceptance") != Some(&Value::Bool(false)) || agent.is_empty() {
        return false;
    }
    // Fail closed on any scrub error: a text we could not scrub is a text we must not clear.
    let Ok(task_destructive_command_text) = scrub(task, DESTRUCTIVE_COMMAND_SCRUB) else {
        return false;
    };
    let Ok(task_dash_live_action_text) = scrub(task, DASH_LIVE_ACTION_SCRUB) else {
        return false;
    };
    let Ok(task_mutation_text) = scrub(task, MUTATION_SCRUB) else {
        return false;
    };
    matches_or_fail_closed_false(&RECOVERY_REVIEWER_AGENT_PATTERN, agent)
        && matches_or_fail_closed_false(&READ_ONLY_PATTERN, task)
        && !matches_or_fail_closed_true(
            &DESTRUCTIVE_COMMAND_PATTERN,
            &task_destructive_command_text,
        )
        && !matches_or_fail_closed_true(&MUTATION_VERB_PATTERN, &task_mutation_text)
        && !matches_or_fail_closed_true(&DELEGATION_PATTERN, &task_mutation_text)
        && !matches_or_fail_closed_true(&ANAPHORIC_MUTATION_PATTERN, &task_mutation_text)
        && !matches_or_fail_closed_true(&DESTRUCTIVE_COMMAND_PATTERN, &task_mutation_text)
        && !matches_or_fail_closed_true(&DASH_LIVE_ACTION_PATTERN, &task_dash_live_action_text)
        && classify_task_mutation_intent(agent, task) == TaskMutationIntent::ReadOnly
}

/// pi `recoveryBarrierMessage` (`scripted-workflow.ts:1277-1279`), verbatim.
#[must_use]
pub fn recovery_barrier_message(source_key: &str, target: &str) -> String {
    format!(
        "Run '{target}' cannot launch after run '{source_key}' returned rejected acceptance recovery; only explicit read-only review children with acceptance:false may follow."
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use serde_json::{Map, Value, json};

    use super::*;

    fn params(agent: &str, task: &str, acceptance_false: bool) -> Map<String, Value> {
        let mut map = Map::new();
        map.insert("agent".into(), json!(agent));
        map.insert("task".into(), json!(task));
        if acceptance_false {
            map.insert("acceptance".into(), json!(false));
        }
        map
    }

    #[test]
    fn all_thirty_six_patterns_compile() {
        // Touch every LazyLock so a pattern-transcription error fails HERE, not first-use.
        let all: &[&LazyLock<Regex>] = &[
            &MUTATION_VERB_PATTERN,
            &READ_ONLY_PATTERN,
            &NO_MUTATION_CLAUSE_PATTERN,
            &DELIVERABLE_PATTERN,
            &CONTEXT_OBJECT_PATTERN,
            &MUTATION_NOUN_CONTEXT_PATTERN,
            &PRIOR_FIX_CONTEXT_PATTERN,
            &DETECTION_CONTEXT_PATTERN,
            &PATTERN_CHANGE_CONTEXT_PATTERN,
            &GIT_PATTERN_CHANGE_CONTEXT_PATTERN,
            &VALIDATION_EVIDENCE_PATTERN,
            &CONTRACT_PROHIBITION_PATTERN,
            &ONLY_REVIEW_CONTRACT_PATTERN,
            &BLOCKED_CONTEXT_FRAGMENT_PATTERN,
            &REGRESSION_EVIDENCE_PATTERN,
            &BLOCKED_QUOTED_EXAMPLE_PATTERN,
            &CONTEXT_QUOTED_EXAMPLE_PATTERN,
            &LISTED_BLOCKED_EXAMPLE_PATTERN,
            &BLOCKS_QUOTED_EXAMPLE_PATTERN,
            &EXAMPLES_LIKE_BLOCKED_PATTERN,
            &QUOTED_VISIBLE_BLOCKED_PATTERN,
            &PROMPTS_LIKE_BLOCKED_PATTERN,
            &GIT_MUTATIONS_BROADER_CONTEXT_PATTERN,
            &GIT_COVERAGE_CONTEXT_PATTERN,
            &REGRESSION_QUOTED_EXAMPLE_PATTERN,
            &REGRESSION_ANAPHORIC_EXAMPLE_PATTERN,
            &REGRESSION_FOLLOWED_BY_ANAPHORIC_PATTERN,
            &REGRESSION_FOLLOWED_BY_NAMED_RM_PATTERN,
            &BLOCKED_LIVE_VARIANT_CONTEXT_PATTERN,
            &ANAPHORIC_REFERENCES_CONTEXT_PATTERN,
            &NO_ANAPHORIC_MUTATION_CLAUSE_PATTERN,
            &NO_DELEGATION_CLAUSE_PATTERN,
            &DELEGATION_PATTERN,
            &ANAPHORIC_MUTATION_PATTERN,
            &DESTRUCTIVE_COMMAND_PATTERN,
            &DASH_LIVE_ACTION_PATTERN,
        ];
        assert_eq!(all.len(), 36);
        for pattern in all {
            assert!(pattern.as_str().len() > 5);
        }
        let _ = &*RECOVERY_REVIEWER_AGENT_PATTERN;
    }

    /// Expected values below were verified against UPSTREAM by evaluating the real
    /// `isExplicitReadOnlyRecoveryReview` pattern chain in Node on these exact inputs — note the
    /// counter-intuitive negative: `"do not edit, modify, or write files"` FAILS upstream too,
    /// because the no-mutation-clause scrub removes only `do not edit` and the surviving bare
    /// `modify`/`write` trip `MUTATION_VERB`.
    #[test]
    fn accepts_a_plain_read_only_review() {
        assert!(is_explicit_read_only_recovery_review(&params(
            "reviewer",
            "Review the saved report. Read-only: return findings only.",
            true,
        )));
    }

    #[test]
    fn requires_acceptance_false_and_a_reviewer_agent() {
        let task = "Review the saved report. Read-only: return findings only.";
        assert!(!is_explicit_read_only_recovery_review(&params(
            "reviewer", task, false
        )));
        assert!(!is_explicit_read_only_recovery_review(&params(
            "worker", task, true
        )));
        // The FOUR-alternative agent gate: a bare `review` agent qualifies (the shared
        // three-alternative helper would reject it — the trap §4.4 names).
        assert!(is_explicit_read_only_recovery_review(&params(
            "review", task, true
        )));
    }

    #[test]
    fn rejects_surviving_bare_mutation_verbs() {
        // Upstream-verified FALSE: the clause scrub leaves `modify`/`write` behind.
        assert!(!is_explicit_read_only_recovery_review(&params(
            "reviewer",
            "Review the saved report. Read-only: do not edit, modify, or write files. Return findings only.",
            true,
        )));
    }

    #[test]
    fn rejects_mutation_verbs_and_destructive_commands() {
        assert!(!is_explicit_read_only_recovery_review(&params(
            "reviewer",
            "Read-only review, then fix the bug. Do not edit anything else.",
            true,
        )));
        assert!(!is_explicit_read_only_recovery_review(&params(
            "reviewer",
            "Read-only: do not edit files. Then run rm -rf target/.",
            true,
        )));
        assert!(!is_explicit_read_only_recovery_review(&params(
            "reviewer",
            "Read-only: do not edit files. git rebase main afterwards.",
            true,
        )));
    }

    #[test]
    fn scrubbing_clears_quoted_blocked_examples() {
        // A quoted destructive example that is DESCRIBED as blocked is scrubbed before the
        // destructive-command test — the whole point of the pipeline. Upstream-verified TRUE.
        assert!(is_explicit_read_only_recovery_review(&params(
            "reviewer",
            "Review the diff. Read-only. `rm -rf /tmp/x` is blocked. Return findings only.",
            true,
        )));
    }

    #[test]
    fn rejects_delegation_and_anaphoric_mutation() {
        assert!(!is_explicit_read_only_recovery_review(&params(
            "reviewer",
            "Read-only: do not edit files. Launch a worker to fix the issues.",
            true,
        )));
        assert!(!is_explicit_read_only_recovery_review(&params(
            "reviewer",
            "Read-only: do not edit files. `rm -rf /` is blocked. Now do it anyway.",
            true,
        )));
    }

    #[test]
    fn rejects_dash_live_actions() {
        assert!(!is_explicit_read_only_recovery_review(&params(
            "reviewer",
            "Review the report, read-only, do not edit files — then update the tests.",
            true,
        )));
    }

    #[test]
    fn barrier_message_is_verbatim() {
        assert_eq!(
            recovery_barrier_message("gate", "runs.host('build')"),
            "Run 'runs.host('build')' cannot launch after run 'gate' returned rejected acceptance recovery; only explicit read-only review children with acceptance:false may follow."
        );
    }

    #[test]
    fn acceptance_metadata_recovery_requires_the_exact_shape() {
        use crate::workflows::{AcceptanceRecoveryMetadata, WorkflowScriptChildResult};
        let mut child = WorkflowScriptChildResult {
            key: "k".into(),
            ok: false,
            ..Default::default()
        };
        assert!(!is_acceptance_metadata_recovery(&child));
        child.recovery = Some(AcceptanceRecoveryMetadata {
            status: "available-for-review".into(),
            reason: "acceptance-metadata-rejected".into(),
            report_path: "r.md".into(),
            report_hash: "h".into(),
        });
        assert!(is_acceptance_metadata_recovery(&child));
        child.ok = true;
        assert!(!is_acceptance_metadata_recovery(&child));
    }
}
