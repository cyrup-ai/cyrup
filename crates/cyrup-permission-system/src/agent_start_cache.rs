//! The `before_agent_start` result cache (port of pi `before-agent-start-cache.ts`, whole file, at
//! v0.8.0).
//!
//! Two independent keys, both plain strings compared with `!=`:
//!
//! 1. **Active-tools key** — the exposed tool-name list. When it is unchanged, pi skips the
//!    `setActiveTools` call entirely (`index.ts:1894-1898`).
//! 2. **Prompt-state key** — agent name + cwd + the manager's policy stamp + the active-tools key +
//!    the line-ending-normalized system prompt. When it is unchanged AND a previous result was
//!    recorded, pi returns that cached `{systemPrompt, entries}` and never re-runs
//!    `sanitizeAvailableToolsSection` / `resolveSkillPromptEntries` (`index.ts:1900-1913`).
//!
//! Both are invalidated together by `invalidateAgentStartCache` (`index.ts:1326-1331`), which pi
//! calls from `session_start`, the `resources_discover` reload branch and `session_shutdown` — and
//! deliberately NOT from `before_agent_start` itself.
//!
//! **The stamp is the correctness hinge.** `getPolicyCacheStamp` is `public` on pi's
//! `PermissionManager` (`permission-manager.ts:781` @v0.8.0) for exactly this consumer: it is the
//! mtime tuple of the four policy files, so a mid-session policy edit changes the key and the
//! cached result is discarded. Keying on anything cheaper (a turn counter, the prompt alone) would
//! serve a stale tool set after a policy edit — a permission-visible defect, not a perf one.

use crate::skill::SkillPromptEntry;

/// pi `createCacheKey(parts)` (`before-agent-start-cache.ts:11-13`): `JSON.stringify(parts)`.
///
/// The key is only ever compared to another key produced by this same function, so what matters is
/// that it is injective over the parts — which `serde_json`'s array encoding is, for the same
/// reason `JSON.stringify` is: every element is quoted and internally escaped, so no part can
/// impersonate a delimiter.
fn create_cache_key(parts: &[&str]) -> String {
    serde_json::to_string(parts).unwrap_or_default()
}

/// pi `createActiveToolsCacheKey` (`before-agent-start-cache.ts:15-17`).
#[must_use]
pub fn create_active_tools_cache_key(allowed_tool_names: &[String]) -> String {
    let refs: Vec<&str> = allowed_tool_names.iter().map(String::as_str).collect();
    create_cache_key(&refs)
}

/// pi `normalizeLineEndings` (`common.ts:163-165`): `\r\n` → `\n`.
fn normalize_line_endings(prompt: &str) -> String {
    prompt.replace("\r\n", "\n")
}

/// pi `BeforeAgentStartPromptStateInput` (`before-agent-start-cache.ts:3-9`).
pub struct PromptStateKeyInput<'a> {
    pub agent_name: Option<&'a str>,
    pub cwd: &'a str,
    pub permission_stamp: &'a str,
    pub system_prompt: &'a str,
    pub allowed_tool_names: &'a [String],
}

/// pi `createBeforeAgentStartPromptStateKey` (`before-agent-start-cache.ts:19-27`). Part order is
/// upstream's and must not be permuted: `normalizeAgentName(agentName) ?? ""`, `cwd`,
/// `permissionStamp`, the nested active-tools key, `normalizeLineEndings(systemPrompt)`.
#[must_use]
pub fn create_prompt_state_key(input: &PromptStateKeyInput<'_>) -> String {
    // pi `normalizeAgentName(input.agentName) ?? ""` — `normalizeAgentName` IS `getNonEmptyString`
    // (`common.ts:29-31`), i.e. trim then drop-if-empty.
    let agent = input
        .agent_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("");
    let tools_key = create_active_tools_cache_key(input.allowed_tool_names);
    let prompt = normalize_line_endings(input.system_prompt);
    create_cache_key(&[
        agent,
        input.cwd,
        input.permission_stamp,
        &tools_key,
        &prompt,
    ])
}

/// pi `shouldApplyCachedAgentStartState(previousKey, nextKey)`
/// (`before-agent-start-cache.ts:29-31`): `previousKey !== nextKey`.
///
/// Read it as "the state must be (re)applied", not "a cached value may be used" — upstream's two
/// call sites use it in opposite senses (`index.ts:1895` applies when true; `:1908` serves the
/// cache when false), which is why the name is kept verbatim rather than inverted.
#[must_use]
pub fn should_apply_cached_agent_start_state(previous_key: Option<&str>, next_key: &str) -> bool {
    previous_key != Some(next_key)
}

/// pi `CachedPromptStateResult` (`index.ts:1303-1306`): the skill enforcement entries plus the
/// sanitized system prompt — `None` when the sanitizers left the prompt unchanged, which is what
/// upstream's `systemPrompt?: string` (absent, not null) encodes and what decides whether the
/// handler returns `{ systemPrompt }` or `{}`.
#[derive(Clone, Default)]
pub struct CachedPromptState {
    pub entries: Vec<SkillPromptEntry>,
    pub system_prompt: Option<String>,
}

/// The three module-scope cache slots pi holds (`index.ts:1312-1314`), grouped so
/// `invalidateAgentStartCache` (`:1326-1331`) is one assignment.
#[derive(Default)]
pub struct AgentStartCache {
    /// pi `lastActiveToolsCacheKey` (`:1312`).
    pub last_active_tools_key: Option<String>,
    /// pi `lastPromptStateCacheKey` (`:1313`).
    pub last_prompt_state_key: Option<String>,
    /// pi `lastPromptStateCacheResult` (`:1314`).
    pub last_prompt_state_result: Option<CachedPromptState>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_tools_key_is_order_sensitive_and_injective() {
        let a = create_active_tools_cache_key(&["read".into(), "write".into()]);
        let b = create_active_tools_cache_key(&["write".into(), "read".into()]);
        assert_ne!(
            a, b,
            "pi keys on the ARRAY, so order is part of the identity"
        );
        assert_eq!(
            a, r#"["read","write"]"#,
            "pi `JSON.stringify(allowedToolNames)`"
        );
        // No element can impersonate the delimiter: `["a,b"]` must not equal `["a","b"]`.
        assert_ne!(
            create_active_tools_cache_key(&["read,write".into()]),
            create_active_tools_cache_key(&["read".into(), "write".into()])
        );
    }

    #[test]
    fn prompt_state_key_changes_when_only_the_policy_stamp_changes() {
        // The whole reason `getPolicyCacheStamp` is public upstream: a mid-session policy edit
        // must invalidate the cached prompt state even though prompt/tools/cwd are identical.
        let tools = vec!["read".to_string()];
        let base = PromptStateKeyInput {
            agent_name: Some("coder"),
            cwd: "/w",
            permission_stamp: "1",
            system_prompt: "p",
            allowed_tool_names: &tools,
        };
        let bumped = PromptStateKeyInput {
            permission_stamp: "2",
            ..base_clone(&base)
        };
        assert_ne!(
            create_prompt_state_key(&base),
            create_prompt_state_key(&bumped)
        );
    }

    fn base_clone<'a>(i: &PromptStateKeyInput<'a>) -> PromptStateKeyInput<'a> {
        PromptStateKeyInput {
            agent_name: i.agent_name,
            cwd: i.cwd,
            permission_stamp: i.permission_stamp,
            system_prompt: i.system_prompt,
            allowed_tool_names: i.allowed_tool_names,
        }
    }

    #[test]
    fn prompt_state_key_normalizes_line_endings_and_trims_the_agent_name() {
        let tools: Vec<String> = vec![];
        let crlf = PromptStateKeyInput {
            agent_name: Some("  coder  "),
            cwd: "/w",
            permission_stamp: "1",
            system_prompt: "a\r\nb",
            allowed_tool_names: &tools,
        };
        let lf = PromptStateKeyInput {
            agent_name: Some("coder"),
            cwd: "/w",
            permission_stamp: "1",
            system_prompt: "a\nb",
            allowed_tool_names: &tools,
        };
        // pi `normalizeLineEndings` + `normalizeAgentName` both run INSIDE the key builder, so a
        // CRLF prompt and a padded agent name are the same cache identity as their normal forms.
        assert_eq!(create_prompt_state_key(&crlf), create_prompt_state_key(&lf));

        // A blank agent name is `""`, not the literal spaces (pi `?? ""` over `getNonEmptyString`).
        let blank = PromptStateKeyInput {
            agent_name: Some("   "),
            ..base_clone(&lf)
        };
        let none = PromptStateKeyInput {
            agent_name: None,
            ..base_clone(&lf)
        };
        assert_eq!(
            create_prompt_state_key(&blank),
            create_prompt_state_key(&none)
        );
    }

    #[test]
    fn should_apply_is_a_plain_inequality() {
        assert!(should_apply_cached_agent_start_state(None, "k"));
        assert!(should_apply_cached_agent_start_state(Some("j"), "k"));
        assert!(!should_apply_cached_agent_start_state(Some("k"), "k"));
    }
}
