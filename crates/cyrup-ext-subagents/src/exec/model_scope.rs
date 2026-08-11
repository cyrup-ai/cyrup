//! Optional `subagents.modelScope` enforcement for subagent model resolution — a 1:1 port of
//! pi-subagents' `src/runs/shared/model-scope.ts` (present at the ported v0.33.x–v0.34.0 baseline,
//! added upstream by `6acfc59`, first shipped in v0.33.0).
//!
//! When `subagents.modelScope.enforce` is set in `settings.json`, a resolved subagent model that
//! matches none of the `allow` patterns is rejected. The severity depends on where the model came
//! from, exactly as upstream:
//!
//! - **explicit** (`--model`, the tool call's `model`, a chain step's `model`) → a hard **error**
//!   that aborts the run BEFORE any child process is spawned, surfaced to the caller as
//!   [`crate::error::SubagentError::ModelOutOfScope`] carrying pi's verbatim message. The run is
//!   REFUSED — never silently downgraded to some in-scope model, which would hide the policy
//!   violation from the caller and quietly change which model actually ran.
//! - **inherited** (persona frontmatter `model:`, `subagents.defaultModel`, the parent session's
//!   model, or a fallback-ladder entry after the primary) → a **warning** only, so existing
//!   configurations keep working. Upstream makes exactly this split
//!   (`model-scope.ts:59-78` `severity = source === "explicit" ? "error" : "warn"`), and a warn
//!   likewise never removes or substitutes the candidate.
//!
//! The decision logic ([`check_model_scope`]) is a pure function of its inputs, so it is unit
//! testable without touching the filesystem or config — matching the upstream module's own
//! stated design.
//!
//! # Where this is enforced
//!
//! | site | upstream | source |
//! |---|---|---|
//! | [`crate::exec::fallback::resolve_model_inheritance`] | `resolveSubagentModelOverride` (`model-fallback.ts:203-210`) | explicit → error, persona/inherited → warn |
//! | [`crate::exec::fallback::build_model_candidates_scoped`] | `buildModelCandidates` (`model-fallback.ts:253-276`) | fallback entries after the primary → warn |
//!
//! Unlike pi — whose async runs resolve their models parent-side in `async-execution.ts:457` — a
//! cyrup background run resolves each step's model INSIDE the detached hop-2 runner process, which
//! has no discovery/settings access by design. The scope therefore reaches that process the same way
//! every other orchestrator decision does: baked into the serialized
//! [`crate::background::runner_main::RunnerConfig`] handed over `--config`, so a background run
//! enforces the same policy the foreground path does.

use crate::exec::split_known_thinking_suffix;

/// The parsed `subagents.modelScope` settings block (pi `ModelScopeConfig`, `model-scope.ts:16-21`).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelScopeConfig {
    /// When `Some(true)`, an out-of-scope model is rejected/warned per [`ModelSource`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enforce: Option<bool>,
    /// Glob-style allow patterns (only `*` is special), matched against the full `provider/id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow: Option<Vec<String>>,
}

impl ModelScopeConfig {
    /// True iff enforcement is actually armed: `enforce: true` AND a non-empty `allow` list. A
    /// config that is enforcing with no patterns is a no-op (the settings parser rejects that
    /// combination, but this stays defensive for callers building configs programmatically — pi
    /// `checkModelScope`'s own `if (!allow || allow.length === 0) return undefined`).
    #[must_use]
    pub fn is_armed(&self) -> bool {
        self.enforce == Some(true) && self.allow.as_ref().is_some_and(|a| !a.is_empty())
    }
}

/// Where a resolved model originated, deciding enforcement severity (pi `ModelSource`,
/// `model-scope.ts:24`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelSource {
    /// A caller-supplied model: `--model`, the tool call's `model`, a chain step's `model`. A
    /// violation here is a hard error.
    Explicit,
    /// A model that came from agent frontmatter / `subagents.defaultModel` / the parent session /
    /// a fallback-ladder entry. A violation here only warns.
    Inherited,
}

/// Violation severity (pi `ModelScopeViolation["severity"]`, `model-scope.ts:28`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelScopeSeverity {
    /// Warn and continue — the model still runs.
    Warn,
    /// Refuse the run outright.
    Error,
}

/// One out-of-scope decision (pi `ModelScopeViolation`, `model-scope.ts:26-32`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelScopeViolation {
    /// The resolved model id, thinking suffix stripped, that fell outside the scope.
    pub model: String,
    /// Whether this refuses the run or merely warns.
    pub severity: ModelScopeSeverity,
    /// pi's verbatim user/LLM-facing message.
    pub message: String,
    /// The `allow` patterns that were in effect.
    pub allowed_patterns: Vec<String>,
}

/// Case-insensitive glob match where only `*` is special (pi `globToRegExp` + `matchesScopePattern`,
/// `model-scope.ts:35-50`), anchored at both ends, against the model with its **known** thinking
/// suffix stripped.
///
/// Implemented without the `regex` crate (this crate has no such dependency — see
/// `fallback.rs`'s `RetryPattern` for the same dependency-free posture). pi escapes every RegExp
/// metacharacter except `*` and then maps `*` → `.*`, i.e. every other character — including `.`,
/// `+`, `(`, `[` — is matched literally; that is exactly a literal-segment split on `*` with
/// wildcard gaps, which is what the greedy matcher below implements.
#[must_use]
pub fn matches_scope_pattern(model: &str, pattern: &str) -> bool {
    let base = split_known_thinking_suffix(model).0.to_lowercase();
    let pattern = pattern.to_lowercase();
    glob_matches(&base, &pattern)
}

/// Anchored `*`-only glob match over already-lowercased inputs.
///
/// The pattern is split on `*` into literal segments; the first must be a prefix, the last a
/// suffix, and each interior segment must occur (in order) somewhere between them. An empty
/// segment (from `**` or a leading/trailing `*`) is vacuously satisfied. This is exactly the
/// language of pi's `^<escaped-with-*→.*>$` RegExp, which cannot backtrack-fail here because every
/// interior segment is a plain literal and a leftmost-first scan is optimal for that shape.
fn glob_matches(text: &str, pattern: &str) -> bool {
    let mut segments = pattern.split('*');
    // `split` on a non-empty separator always yields at least one element, so this is not an
    // "empty iterator" case; the `else` arm is unreachable in practice but keeps the code
    // panic-free without an `expect`.
    let Some(first) = segments.next() else {
        return text.is_empty();
    };
    let Some(mut rest) = text.strip_prefix(first) else {
        return false;
    };
    let tail: Vec<&str> = segments.collect();
    let Some((last, middle)) = tail.split_last() else {
        // No `*` at all: the whole pattern was one literal segment, so it must have consumed the
        // entire text.
        return rest.is_empty();
    };
    for segment in middle {
        if segment.is_empty() {
            continue;
        }
        let Some(idx) = rest.find(segment) else {
            return false;
        };
        rest = rest.get(idx + segment.len()..).unwrap_or("");
    }
    // The final segment must match at the END of what remains (the `$` anchor), and must not
    // overlap anything already consumed — `ends_with` on the remainder gives exactly that.
    rest.len() >= last.len() && rest.ends_with(last)
}

/// Pure scope decision (pi `checkModelScope`, `model-scope.ts:59-78`).
///
/// Returns `Some(violation)` when the model is out of scope AND enforcement is armed, else `None`.
/// Enforcement with no `allow` list is a no-op.
#[must_use]
pub fn check_model_scope(
    model: Option<&str>,
    scope: Option<&ModelScopeConfig>,
    source: ModelSource,
) -> Option<ModelScopeViolation> {
    let model = model.filter(|m| !m.is_empty())?;
    let scope = scope?;
    if scope.enforce != Some(true) {
        return None;
    }
    let allow = scope.allow.as_ref()?;
    if allow.is_empty() {
        return None;
    }
    if allow.iter().any(|pattern| matches_scope_pattern(model, pattern)) {
        return None;
    }

    let base_model = split_known_thinking_suffix(model).0.to_string();
    let severity = match source {
        ModelSource::Explicit => ModelScopeSeverity::Error,
        ModelSource::Inherited => ModelScopeSeverity::Warn,
    };
    let message = format!(
        "Model '{base_model}' is outside the configured subagent model scope. Allowed patterns: {}.",
        allow.join(", ")
    );
    Some(ModelScopeViolation {
        model: base_model,
        severity,
        message,
        allowed_patterns: allow.clone(),
    })
}

/// Emit a warn-severity violation the way pi's `defaultScopeWarn` does
/// (`model-fallback.ts:175-195`, `console.warn("[pi-subagents] " + message)`). Error-severity
/// violations are never routed here — they are returned to the caller and refuse the run.
pub(crate) fn warn_violation(violation: &ModelScopeViolation) {
    if violation.severity == ModelScopeSeverity::Warn {
        tracing::warn!(model = %violation.model, "[cyrup-ext-subagents] {}", violation.message);
    }
}

/// Validate and normalize a raw `subagents.modelScope` value read from `settings.json` — pi
/// `parseModelScopeConfig` (`model-scope.ts:85-127`), including its "enforce without a non-empty
/// allow list" rejection and its whitespace trimming of each pattern.
///
/// Returns `Ok(None)` when the field is absent or says nothing at all (pi's
/// `Object.keys(config).length > 0` gate).
///
/// Message shape follows this crate's own settings-validation convention (see the `defaultModel`
/// check in [`crate::discovery::parse_subagent_settings`]): the fragment names the offending field,
/// and [`crate::discovery::read_subagent_settings_file`] prefixes the originating file path —
/// composing the same `<path>` + `<field problem>` information pi puts in one sentence, from one
/// place instead of two.
///
/// # Errors
///
/// Returns a [`crate::error::SubagentError::MalformedSettings`]-bound message for a non-object
/// `modelScope`, a non-boolean `enforce`, a non-array / non-string-element / effectively-empty
/// `allow`, or `enforce: true` without patterns. Per R-SA-009 a malformed value MUST abort
/// discovery rather than being silently dropped — which is precisely what happened before this
/// field existed: `SubagentSettings` does not deny unknown keys, so a whole `modelScope` block was
/// discarded by serde without a word.
pub fn parse_model_scope_config(
    value: Option<&serde_json::Value>,
) -> Result<Option<ModelScopeConfig>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let Some(input) = value.as_object() else {
        return Err("invalid 'modelScope'; expected an object".to_string());
    };

    let mut config = ModelScopeConfig::default();
    let mut saw_field = false;

    if let Some(raw) = input.get("enforce") {
        let Some(flag) = raw.as_bool() else {
            return Err("invalid 'modelScope.enforce'; expected a boolean".to_string());
        };
        config.enforce = Some(flag);
        saw_field = true;
    }

    if let Some(raw) = input.get("allow") {
        let invalid = "invalid 'modelScope.allow'; expected an array of strings".to_string();
        let Some(entries) = raw.as_array() else {
            return Err(invalid);
        };
        let mut allow: Vec<String> = Vec::with_capacity(entries.len());
        for entry in entries {
            let Some(text) = entry.as_str() else {
                return Err(invalid);
            };
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                allow.push(trimmed.to_string());
            }
        }
        if allow.is_empty() {
            return Err(
                "invalid 'modelScope.allow'; expected a non-empty array of patterns".to_string()
            );
        }
        config.allow = Some(allow);
        saw_field = true;
    }

    if config.enforce == Some(true) && !config.allow.as_ref().is_some_and(|a| !a.is_empty()) {
        return Err(
            "modelScope.enforce is set without a non-empty 'allow' list; supply allowed model \
             patterns or disable enforcement"
                .to_string(),
        );
    }

    Ok(if saw_field { Some(config) } else { None })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn scope(patterns: &[&str]) -> ModelScopeConfig {
        ModelScopeConfig {
            enforce: Some(true),
            allow: Some(patterns.iter().map(|p| (*p).to_string()).collect()),
        }
    }

    #[test]
    fn glob_only_treats_star_as_special_and_is_case_insensitive() {
        assert!(matches_scope_pattern("anthropic/claude-opus-4", "anthropic/*"));
        assert!(matches_scope_pattern("ANTHROPIC/Claude-Opus-4", "anthropic/*"));
        assert!(matches_scope_pattern("anthropic/claude-opus-4", "*opus*"));
        assert!(matches_scope_pattern("anthropic/claude-opus-4", "anthropic/claude-opus-4"));
        assert!(!matches_scope_pattern("openai/gpt-5", "anthropic/*"));
        // A `.` in the pattern is a LITERAL dot upstream (escaped before `*` -> `.*`), so it must
        // NOT behave like a regex any-char.
        assert!(!matches_scope_pattern("openai/gpt-5", "openai/gpt-5x"));
        assert!(!matches_scope_pattern("openai/gpt5", "openai/gpt.5"));
        assert!(matches_scope_pattern("openai/gpt.5", "openai/gpt.5"));
        // Anchored at both ends.
        assert!(!matches_scope_pattern("xanthropic/claude", "anthropic/*"));
        assert!(!matches_scope_pattern("anthropic/claude-x", "*claude"));
    }

    #[test]
    fn the_known_thinking_suffix_is_stripped_before_matching_including_max() {
        // `:max` is the 7th level (cyrup commit 6d29542 extended THINKING_LEVELS to 7).
        assert!(matches_scope_pattern("anthropic/claude-opus-4:max", "anthropic/claude-opus-4"));
        assert!(matches_scope_pattern("anthropic/claude-opus-4:high", "anthropic/claude-opus-4"));
        // An UNKNOWN colon suffix is part of the id, not a thinking level, and must not be stripped.
        assert!(!matches_scope_pattern("anthropic/claude-opus-4:preview", "anthropic/claude-opus-4"));
    }

    #[test]
    fn an_explicit_out_of_scope_model_is_an_error_and_an_inherited_one_is_a_warning() {
        let s = scope(&["anthropic/*"]);
        let explicit = check_model_scope(Some("openai/gpt-5"), Some(&s), ModelSource::Explicit)
            .expect("out-of-scope explicit model must violate");
        assert_eq!(explicit.severity, ModelScopeSeverity::Error);
        assert_eq!(
            explicit.message,
            "Model 'openai/gpt-5' is outside the configured subagent model scope. Allowed \
             patterns: anthropic/*."
        );
        let inherited = check_model_scope(Some("openai/gpt-5"), Some(&s), ModelSource::Inherited)
            .expect("out-of-scope inherited model must violate");
        assert_eq!(inherited.severity, ModelScopeSeverity::Warn);
        assert_eq!(inherited.message, explicit.message);
    }

    #[test]
    fn enforcement_is_a_no_op_without_enforce_or_without_patterns() {
        let off = ModelScopeConfig { enforce: Some(false), allow: Some(vec!["anthropic/*".into()]) };
        assert!(check_model_scope(Some("openai/gpt-5"), Some(&off), ModelSource::Explicit).is_none());
        let empty = ModelScopeConfig { enforce: Some(true), allow: Some(Vec::new()) };
        assert!(check_model_scope(Some("openai/gpt-5"), Some(&empty), ModelSource::Explicit).is_none());
        assert!(check_model_scope(Some("openai/gpt-5"), None, ModelSource::Explicit).is_none());
        assert!(check_model_scope(None, Some(&scope(&["anthropic/*"])), ModelSource::Explicit).is_none());
    }

    #[test]
    fn the_violation_message_strips_the_thinking_suffix_from_the_reported_model() {
        let v = check_model_scope(
            Some("openai/gpt-5:high"),
            Some(&scope(&["anthropic/*", "together/*"])),
            ModelSource::Explicit,
        )
        .expect("violates");
        assert_eq!(v.model, "openai/gpt-5");
        assert_eq!(
            v.message,
            "Model 'openai/gpt-5' is outside the configured subagent model scope. Allowed \
             patterns: anthropic/*, together/*."
        );
    }

    #[test]
    fn parse_rejects_every_malformed_shape_r_sa_009_style() {
        assert_eq!(parse_model_scope_config(None), Ok(None));
        assert_eq!(
            parse_model_scope_config(Some(&serde_json::json!([]))),
            Err("invalid 'modelScope'; expected an object".to_string())
        );
        assert_eq!(
            parse_model_scope_config(Some(&serde_json::json!({"enforce": "yes"}))),
            Err("invalid 'modelScope.enforce'; expected a boolean".to_string())
        );
        assert_eq!(
            parse_model_scope_config(Some(&serde_json::json!({"allow": "anthropic/*"}))),
            Err("invalid 'modelScope.allow'; expected an array of strings".to_string())
        );
        assert_eq!(
            parse_model_scope_config(Some(&serde_json::json!({"allow": [1]}))),
            Err("invalid 'modelScope.allow'; expected an array of strings".to_string())
        );
        assert_eq!(
            parse_model_scope_config(Some(&serde_json::json!({"allow": ["", "  "]}))),
            Err("invalid 'modelScope.allow'; expected a non-empty array of patterns".to_string())
        );
        assert_eq!(
            parse_model_scope_config(Some(&serde_json::json!({"enforce": true}))),
            Err("modelScope.enforce is set without a non-empty 'allow' list; supply allowed model \
                 patterns or disable enforcement"
                .to_string())
        );
    }

    #[test]
    fn parse_trims_patterns_and_accepts_a_well_formed_block() {
        let parsed = parse_model_scope_config(Some(
            &serde_json::json!({"enforce": true, "allow": ["  anthropic/*  ", "openai/gpt-5"]}),
        ))
        .expect("valid block parses");
        assert_eq!(
            parsed,
            Some(ModelScopeConfig {
                enforce: Some(true),
                allow: Some(vec!["anthropic/*".to_string(), "openai/gpt-5".to_string()]),
            })
        );
        assert!(parsed.as_ref().is_some_and(ModelScopeConfig::is_armed));
    }

    #[test]
    fn an_empty_object_says_nothing_and_yields_no_config() {
        assert_eq!(parse_model_scope_config(Some(&serde_json::json!({}))), Ok(None));
    }
}
