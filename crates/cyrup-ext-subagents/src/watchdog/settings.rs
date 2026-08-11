//! Watchdog configuration — a 1:1 port of `pi-subagents/src/watchdog/settings.ts` (568 lines
//! @v0.43.0).
//!
//! Three layers, merged lowest-first (`resolveWatchdogConfig`, `:537-568`):
//!
//! 1. **user** — `<agentDir>/settings.json` (`:422-424`), i.e. `~/.cyrup/agent/settings.json`;
//! 2. **project** — the nearest ancestor of `cwd` that has a `.cyrup` (upstream `.pi`) or
//!    `.agents` directory, then `<thatDir>/.cyrup/settings.json` (`:430-440`);
//! 3. **session** — an in-memory override the runtime supplies for `/subagents-watchdog session …`
//!    (`:466-469`), accepted either wrapped in `{subagents:{watchdog:…}}` or bare.
//!
//! Everything lives under the `subagents.watchdog` key (`:384-389`); a settings file with no such
//! key contributes the empty patch and never errors.
//!
//! ## Strict by construction
//!
//! Upstream's parser is *closed*: every object level has an allow-list and an unknown key THROWS
//! (`assertKnownFields`, `:188-192`), as does a value of the wrong type or out of range. The two
//! entry points differ only in what they do with that throw — [`resolve_watchdog_config_strict`]
//! propagates it (`:471-482`), while [`resolve_watchdog_config`] catches per layer, records a
//! [`WatchdogSettingsError`], and returns `ok: false` **with the pristine defaults**
//! (`:562-567`). That last part is the safety property: a typo in `subagents.watchdog` disables
//! the watchdog rather than half-configuring it, and `register-main.ts:135-137` renders the errors
//! with "Watchdog is disabled until the config is fixed."
//!
//! ## Patches are JSON, not structs
//!
//! Upstream builds `WatchdogConfigPatch` — a deep `Partial<>` — and folds the layers with a generic
//! `deepMerge` over plain objects (`:446-453`), only materializing the typed
//! `ResolvedWatchdogConfig` at the very end. This port does the same on
//! [`serde_json::Value`]: validation NORMALIZES as it goes (trimming the strings upstream's
//! `parseNonEmptyString` trims, `:204-207`) and emits a validated patch object, the layers are
//! deep-merged, and the result is deserialized once. A parallel tower of Rust `Option<Option<…>>`
//! patch structs would have to re-encode the same three-state "absent / null / present" the JSON
//! already carries.
//!
//! ## Writes
//!
//! [`write_user_watchdog_enabled`] and [`write_watchdog_model_settings`] (`:514-535`) are
//! read-modify-write against the SAME strict reader, so they refuse to touch a file they cannot
//! parse rather than overwriting a user's settings with a fresh object. One cosmetic difference:
//! `JSON.stringify(settings, null, 2)` preserves key insertion order, while `serde_json`'s `Map` is
//! a `BTreeMap` (the `preserve_order` feature is not enabled workspace-wide), so a rewritten file
//! comes back with its keys sorted. The bytes differ; the settings do not.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::types::{
    ResolvedWatchdogConfig, ThinkingSetting, WatchdogDeliveryMode, WatchdogLateWarningPolicy,
    WatchdogSettingsError, WatchdogSettingsResult, WatchdogSettingsScope, WatchdogSettingsSource,
    WatchdogSeverity,
};

/// The reasoning levels `parseThinking` accepts (`shared/model-info.ts:1`'s `THINKING_LEVELS`,
/// rendered into the error text at `settings.ts:212`). Same seven-element tuple, same order, as
/// `crate::exec`'s own private copy — which is where a model id's `:<level>` suffix is matched.
const THINKING_LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

// =================================================================================================
// Defaults (`DEFAULT_WATCHDOG_CONFIG`, settings.ts:71-121)
// =================================================================================================

/// `DEFAULT_WATCHDOG_CONFIG` (`settings.ts:71-121`) — tier 0 of the merge and, on a parse failure,
/// the whole answer.
///
/// A function rather than a `static`: upstream's `cloneDefaultConfig` (`:154-170`) hands out a deep
/// COPY precisely so a caller cannot mutate the shared default (its `children.overrides` is a fresh
/// `{}` every time), and a Rust `const` cannot hold the `BTreeMap`.
#[must_use]
pub fn default_watchdog_config() -> ResolvedWatchdogConfig {
    use super::types::{
        WatchdogAsyncCompletionConfig, WatchdogAutoFollowConfig, WatchdogCadenceConfig,
        WatchdogChildrenConfig, WatchdogEndpointConfig, WatchdogGuidanceConfig, WatchdogLspConfig,
        WatchdogScopeConfig, WatchdogSyncBacklog,
    };
    ResolvedWatchdogConfig {
        enabled: false,
        delivery: WatchdogDeliveryMode::Held,
        show_during_run: false,
        sync_backlog: WatchdogSyncBacklog::Off,
        agent_end_timeout_ms: 30_000,
        late_warning_policy: WatchdogLateWarningPolicy::ShowStaleNoAutofollow,
        severity_threshold: WatchdogSeverity::Concern,
        max_warnings: None,
        guidance: WatchdogGuidanceConfig {
            watchdog_md: true,
            system_prompt_path: None,
        },
        auto_follow: WatchdogAutoFollowConfig {
            blockers: true,
            max_attempts: Some(3),
            stalemate_repeats: 3,
        },
        scope: WatchdogScopeConfig { enabled: true },
        cadence: WatchdogCadenceConfig { every_n_tools: None },
        main: WatchdogEndpointConfig {
            enabled: false,
            model: None,
            thinking: None,
        },
        children: WatchdogChildrenConfig {
            enabled: false,
            model: None,
            thinking: None,
            watchdog_tail_timeout_ms: 120_000,
            auto_follow: WatchdogAutoFollowConfig {
                blockers: true,
                max_attempts: Some(3),
                stalemate_repeats: 3,
            },
            overrides: std::collections::BTreeMap::new(),
        },
        async_completion: WatchdogAsyncCompletionConfig {
            enabled: false,
            auto_follow_blockers: false,
        },
        lsp: WatchdogLspConfig {
            enabled: true,
            timeout_ms: 3_000,
            max_files: 20,
            max_diagnostics: 50,
        },
        compact_at_percent: 80,
        review_retry_delay_ms: 1_000,
        max_review_failures: 3,
    }
}

// =================================================================================================
// Field allow-lists (`settings.ts:123-152`)
// =================================================================================================

const WATCHDOG_FIELDS: &[&str] = &[
    "enabled",
    "delivery",
    "showDuringRun",
    "syncBacklog",
    "agentEndTimeoutMs",
    "lateWarningPolicy",
    "severityThreshold",
    "maxWarnings",
    "guidance",
    "autoFollow",
    "scope",
    "cadence",
    "main",
    "children",
    "asyncCompletion",
    "lsp",
    "compactAtPercent",
    "reviewRetryDelayMs",
    "maxReviewFailures",
];
const GUIDANCE_FIELDS: &[&str] = &["watchdogMd", "systemPromptPath"];
const AUTO_FOLLOW_FIELDS: &[&str] = &["blockers", "maxAttempts", "stalemateRepeats"];
const SCOPE_FIELDS: &[&str] = &["enabled"];
const CADENCE_FIELDS: &[&str] = &["everyNTools"];
const ENDPOINT_FIELDS: &[&str] = &["enabled", "model", "thinking"];
const CHILDREN_FIELDS: &[&str] = &[
    "enabled",
    "model",
    "thinking",
    "watchdogTailTimeoutMs",
    "autoFollow",
    "overrides",
];
const CHILD_OVERRIDE_FIELDS: &[&str] = &["enabled", "model", "thinking"];
const ASYNC_COMPLETION_FIELDS: &[&str] = &["enabled", "autoFollowBlockers"];
const LSP_FIELDS: &[&str] = &["enabled", "timeoutMs", "maxFiles", "maxDiagnostics"];

// =================================================================================================
// Parse plumbing (`settings.ts:66-69, 172-233`)
// =================================================================================================

/// `ParseMeta` (`settings.ts:66-69`) — where the settings being parsed live, when they live
/// anywhere.
///
/// Upstream's record also carries `scope`, which nothing reads: every message routes through
/// `sourceName(meta)` (`:176-178`), which branches on `path` alone, and the `scope` that lands in a
/// [`WatchdogSettingsError`] comes from the CALLER's own loop variable (`:551,559`), never from
/// here. The field is dropped rather than carried unread.
#[derive(Debug, Clone)]
struct ParseMeta {
    path: Option<String>,
}

impl ParseMeta {
    /// `sourceName(meta)` (`settings.ts:176-178`).
    fn source_name(&self) -> String {
        match &self.path {
            Some(path) => format!("'{path}'"),
            None => "session override".to_string(),
        }
    }

    /// `invalid(meta, field, expected)` (`settings.ts:180-182`).
    fn invalid(&self, field: &str, expected: &str) -> String {
        format!(
            "Watchdog settings in {} have invalid '{field}'; expected {expected}.",
            self.source_name()
        )
    }

    /// `unknown(meta, field)` (`settings.ts:184-186`).
    fn unknown(&self, field: &str) -> String {
        format!(
            "Watchdog settings in {} have unknown field '{field}'.",
            self.source_name()
        )
    }
}

/// `isPlainObject` (`settings.ts:172-174`) — an object that is not an array. (`null` is
/// [`Value::Null`] here, which is not an object at all.)
fn as_plain_object(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object()
}

/// `assertKnownFields` (`settings.ts:188-192`).
fn assert_known_fields(
    input: &Map<String, Value>,
    allowed: &[&str],
    field_prefix: &str,
    meta: &ParseMeta,
) -> Result<(), String> {
    for key in input.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(meta.unknown(&format!("{field_prefix}.{key}")));
        }
    }
    Ok(())
}

/// `parseObject` (`settings.ts:194-197`).
fn parse_object<'a>(
    value: &'a Value,
    field: &str,
    meta: &ParseMeta,
) -> Result<&'a Map<String, Value>, String> {
    as_plain_object(value).ok_or_else(|| meta.invalid(field, "an object"))
}

/// `parseBoolean` (`settings.ts:199-202`).
fn parse_boolean(value: &Value, field: &str, meta: &ParseMeta) -> Result<bool, String> {
    value
        .as_bool()
        .ok_or_else(|| meta.invalid(field, "a boolean"))
}

/// `parseNonEmptyString` (`settings.ts:204-207`) — returns the TRIMMED value, which is what lands
/// in the patch and therefore in the resolved config.
fn parse_non_empty_string(value: &Value, field: &str, meta: &ParseMeta) -> Result<String, String> {
    match value.as_str() {
        Some(s) if !s.trim().is_empty() => Ok(s.trim().to_string()),
        _ => Err(meta.invalid(field, "a non-empty string")),
    }
}

/// `parseThinking` (`settings.ts:209-213`) — a recognized level string or the JSON literal `false`.
/// The *string* `"false"` is NOT accepted here (only `model-selection.ts`'s separate command-line
/// parser maps that spelling).
fn parse_thinking(value: &Value, field: &str, meta: &ParseMeta) -> Result<ThinkingSetting, String> {
    let expected = {
        let levels: Vec<String> = THINKING_LEVELS.iter().map(|l| format!("'{l}'")).collect();
        format!("{} or false", levels.join(" or "))
    };
    if value == &Value::Bool(false) {
        return Ok(ThinkingSetting::Off);
    }
    match value.as_str() {
        Some(s) if THINKING_LEVELS.contains(&s) => Ok(ThinkingSetting::Level(s.to_string())),
        _ => Err(meta.invalid(field, &expected)),
    }
}

/// `parseInteger` (`settings.ts:215-218`) — `Number.isInteger` plus the caller's range check.
fn parse_integer(
    value: &Value,
    field: &str,
    meta: &ParseMeta,
    expected: &str,
    check: impl Fn(i64) -> bool,
) -> Result<i64, String> {
    match value.as_i64() {
        Some(n) if check(n) => Ok(n),
        _ => Err(meta.invalid(field, expected)),
    }
}

/// `parseNullableInteger` (`settings.ts:220-223`).
fn parse_nullable_integer(
    value: &Value,
    field: &str,
    meta: &ParseMeta,
    expected: &str,
    check: impl Fn(i64) -> bool,
) -> Result<Option<i64>, String> {
    if value.is_null() {
        return Ok(None);
    }
    parse_integer(value, field, meta, expected, check).map(Some)
}

/// `parseEnum` (`settings.ts:225-228`), specialized per union so the error text renders the exact
/// upstream tuple order.
fn parse_enum(
    value: &Value,
    field: &str,
    meta: &ParseMeta,
    values: &[&str],
) -> Result<String, String> {
    let expected: Vec<String> = values.iter().map(|v| format!("'{v}'")).collect();
    match value.as_str() {
        Some(s) if values.contains(&s) => Ok(s.to_string()),
        _ => Err(meta.invalid(field, &expected.join(" or "))),
    }
}

/// `parseSyncBacklog` (`settings.ts:230-233`).
fn parse_sync_backlog(value: &Value, field: &str, meta: &ParseMeta) -> Result<Value, String> {
    if value.as_str() == Some("off") {
        return Ok(Value::String("off".to_string()));
    }
    let n = parse_integer(
        value,
        field,
        meta,
        "'off' or a positive integer",
        |candidate| candidate >= 1,
    )?;
    Ok(Value::from(n))
}

// =================================================================================================
// The patch parsers (`settings.ts:235-389`)
// =================================================================================================

/// Copy a key from `input` into `patch` through `f`, when the key is PRESENT (`"x" in input`),
/// which is upstream's three-state test — an explicit `null` is present.
fn take<F>(
    input: &Map<String, Value>,
    patch: &mut Map<String, Value>,
    key: &str,
    f: F,
) -> Result<(), String>
where
    F: FnOnce(&Value) -> Result<Value, String>,
{
    if let Some(value) = input.get(key) {
        patch.insert(key.to_string(), f(value)?);
    }
    Ok(())
}

/// `parseGuidancePatch` (`settings.ts:235-246`).
fn parse_guidance_patch(value: &Value, field: &str, meta: &ParseMeta) -> Result<Value, String> {
    let input = parse_object(value, field, meta)?;
    assert_known_fields(input, GUIDANCE_FIELDS, field, meta)?;
    let mut patch = Map::new();
    take(input, &mut patch, "watchdogMd", |v| {
        parse_boolean(v, &format!("{field}.watchdogMd"), meta).map(Value::from)
    })?;
    take(input, &mut patch, "systemPromptPath", |v| {
        if v.is_null() {
            return Ok(Value::Null);
        }
        parse_non_empty_string(v, &format!("{field}.systemPromptPath"), meta).map(Value::from)
    })?;
    Ok(Value::Object(patch))
}

/// `parseAutoFollowPatch` (`settings.ts:248-260`).
fn parse_auto_follow_patch(value: &Value, field: &str, meta: &ParseMeta) -> Result<Value, String> {
    let input = parse_object(value, field, meta)?;
    assert_known_fields(input, AUTO_FOLLOW_FIELDS, field, meta)?;
    let mut patch = Map::new();
    take(input, &mut patch, "blockers", |v| {
        parse_boolean(v, &format!("{field}.blockers"), meta).map(Value::from)
    })?;
    take(input, &mut patch, "maxAttempts", |v| {
        parse_nullable_integer(
            v,
            &format!("{field}.maxAttempts"),
            meta,
            "null or a positive integer",
            |c| c >= 1,
        )
        .map(|n| n.map_or(Value::Null, Value::from))
    })?;
    take(input, &mut patch, "stalemateRepeats", |v| {
        parse_integer(
            v,
            &format!("{field}.stalemateRepeats"),
            meta,
            "a positive integer",
            |c| c >= 1,
        )
        .map(Value::from)
    })?;
    Ok(Value::Object(patch))
}

/// `parseScopePatch` (`settings.ts:262-268`).
fn parse_scope_patch(value: &Value, field: &str, meta: &ParseMeta) -> Result<Value, String> {
    let input = parse_object(value, field, meta)?;
    assert_known_fields(input, SCOPE_FIELDS, field, meta)?;
    let mut patch = Map::new();
    take(input, &mut patch, "enabled", |v| {
        parse_boolean(v, &format!("{field}.enabled"), meta).map(Value::from)
    })?;
    Ok(Value::Object(patch))
}

/// `parseCadencePatch` (`settings.ts:270-280`) — note the floor of **5**, not 1.
fn parse_cadence_patch(value: &Value, field: &str, meta: &ParseMeta) -> Result<Value, String> {
    let input = parse_object(value, field, meta)?;
    assert_known_fields(input, CADENCE_FIELDS, field, meta)?;
    let mut patch = Map::new();
    take(input, &mut patch, "everyNTools", |v| {
        if v.is_null() {
            return Ok(Value::Null);
        }
        parse_integer(
            v,
            &format!("{field}.everyNTools"),
            meta,
            "null or an integer >= 5",
            |c| c >= 5,
        )
        .map(Value::from)
    })?;
    Ok(Value::Object(patch))
}

/// The shared body of `parseEndpointPatch` (`settings.ts:282-290`) and
/// `parseChildOverridePatch` (`:292-300`) — upstream duplicates it against two identical
/// allow-lists.
fn parse_endpoint_like_patch(
    value: &Value,
    field: &str,
    meta: &ParseMeta,
    allowed: &[&str],
) -> Result<Value, String> {
    let input = parse_object(value, field, meta)?;
    assert_known_fields(input, allowed, field, meta)?;
    let mut patch = Map::new();
    take(input, &mut patch, "enabled", |v| {
        parse_boolean(v, &format!("{field}.enabled"), meta).map(Value::from)
    })?;
    take(input, &mut patch, "model", |v| {
        parse_non_empty_string(v, &format!("{field}.model"), meta).map(Value::from)
    })?;
    take(input, &mut patch, "thinking", |v| {
        let thinking = parse_thinking(v, &format!("{field}.thinking"), meta)?;
        serde_json::to_value(thinking).map_err(|e| meta.invalid(&format!("{field}.thinking"), &e.to_string()))
    })?;
    Ok(Value::Object(patch))
}

/// `parseChildrenPatch` (`settings.ts:302-322`).
fn parse_children_patch(value: &Value, field: &str, meta: &ParseMeta) -> Result<Value, String> {
    let input = parse_object(value, field, meta)?;
    assert_known_fields(input, CHILDREN_FIELDS, field, meta)?;
    let mut patch = Map::new();
    take(input, &mut patch, "enabled", |v| {
        parse_boolean(v, &format!("{field}.enabled"), meta).map(Value::from)
    })?;
    take(input, &mut patch, "model", |v| {
        parse_non_empty_string(v, &format!("{field}.model"), meta).map(Value::from)
    })?;
    take(input, &mut patch, "thinking", |v| {
        let thinking = parse_thinking(v, &format!("{field}.thinking"), meta)?;
        serde_json::to_value(thinking).map_err(|e| meta.invalid(&format!("{field}.thinking"), &e.to_string()))
    })?;
    take(input, &mut patch, "watchdogTailTimeoutMs", |v| {
        parse_integer(
            v,
            &format!("{field}.watchdogTailTimeoutMs"),
            meta,
            "a positive integer",
            |c| c >= 1,
        )
        .map(Value::from)
    })?;
    take(input, &mut patch, "autoFollow", |v| {
        parse_auto_follow_patch(v, &format!("{field}.autoFollow"), meta)
    })?;
    if let Some(raw_overrides) = input.get("overrides") {
        let overrides = parse_object(raw_overrides, &format!("{field}.overrides"), meta)?;
        let mut parsed = Map::new();
        for (agent, override_value) in overrides {
            if agent.trim().is_empty() {
                return Err(meta.invalid(
                    &format!("{field}.overrides"),
                    "agent names to be non-empty",
                ));
            }
            parsed.insert(
                agent.clone(),
                parse_endpoint_like_patch(
                    override_value,
                    &format!("{field}.overrides.{agent}"),
                    meta,
                    CHILD_OVERRIDE_FIELDS,
                )?,
            );
        }
        patch.insert("overrides".to_string(), Value::Object(parsed));
    }
    Ok(Value::Object(patch))
}

/// `parseAsyncCompletionPatch` (`settings.ts:324-331`).
fn parse_async_completion_patch(
    value: &Value,
    field: &str,
    meta: &ParseMeta,
) -> Result<Value, String> {
    let input = parse_object(value, field, meta)?;
    assert_known_fields(input, ASYNC_COMPLETION_FIELDS, field, meta)?;
    let mut patch = Map::new();
    take(input, &mut patch, "enabled", |v| {
        parse_boolean(v, &format!("{field}.enabled"), meta).map(Value::from)
    })?;
    take(input, &mut patch, "autoFollowBlockers", |v| {
        parse_boolean(v, &format!("{field}.autoFollowBlockers"), meta).map(Value::from)
    })?;
    Ok(Value::Object(patch))
}

/// `parseLspPatch` (`settings.ts:333-342`) — `maxDiagnostics` alone allows 0.
fn parse_lsp_patch(value: &Value, field: &str, meta: &ParseMeta) -> Result<Value, String> {
    let input = parse_object(value, field, meta)?;
    assert_known_fields(input, LSP_FIELDS, field, meta)?;
    let mut patch = Map::new();
    take(input, &mut patch, "enabled", |v| {
        parse_boolean(v, &format!("{field}.enabled"), meta).map(Value::from)
    })?;
    take(input, &mut patch, "timeoutMs", |v| {
        parse_integer(
            v,
            &format!("{field}.timeoutMs"),
            meta,
            "a positive integer",
            |c| c >= 1,
        )
        .map(Value::from)
    })?;
    take(input, &mut patch, "maxFiles", |v| {
        parse_integer(
            v,
            &format!("{field}.maxFiles"),
            meta,
            "a positive integer",
            |c| c >= 1,
        )
        .map(Value::from)
    })?;
    take(input, &mut patch, "maxDiagnostics", |v| {
        parse_integer(
            v,
            &format!("{field}.maxDiagnostics"),
            meta,
            "a non-negative integer",
            |c| c >= 0,
        )
        .map(Value::from)
    })?;
    Ok(Value::Object(patch))
}

/// `parseWatchdogPatch` (`settings.ts:344-382`).
fn parse_watchdog_patch(value: &Value, field: &str, meta: &ParseMeta) -> Result<Value, String> {
    let input = parse_object(value, field, meta)?;
    assert_known_fields(input, WATCHDOG_FIELDS, field, meta)?;
    let mut patch = Map::new();
    take(input, &mut patch, "enabled", |v| {
        parse_boolean(v, &format!("{field}.enabled"), meta).map(Value::from)
    })?;
    take(input, &mut patch, "delivery", |v| {
        let values: Vec<&str> = WatchdogDeliveryMode::ALL.iter().map(|d| d.as_str()).collect();
        parse_enum(v, &format!("{field}.delivery"), meta, &values).map(Value::from)
    })?;
    take(input, &mut patch, "showDuringRun", |v| {
        parse_boolean(v, &format!("{field}.showDuringRun"), meta).map(Value::from)
    })?;
    take(input, &mut patch, "syncBacklog", |v| {
        parse_sync_backlog(v, &format!("{field}.syncBacklog"), meta)
    })?;
    take(input, &mut patch, "agentEndTimeoutMs", |v| {
        parse_integer(
            v,
            &format!("{field}.agentEndTimeoutMs"),
            meta,
            "a positive integer",
            |c| c >= 1,
        )
        .map(Value::from)
    })?;
    take(input, &mut patch, "lateWarningPolicy", |v| {
        let values: Vec<&str> = WatchdogLateWarningPolicy::ALL
            .iter()
            .map(|p| p.as_str())
            .collect();
        parse_enum(v, &format!("{field}.lateWarningPolicy"), meta, &values).map(Value::from)
    })?;
    take(input, &mut patch, "severityThreshold", |v| {
        let values: Vec<&str> = WatchdogSeverity::ALL.iter().map(|s| s.as_str()).collect();
        parse_enum(v, &format!("{field}.severityThreshold"), meta, &values).map(Value::from)
    })?;
    take(input, &mut patch, "maxWarnings", |v| {
        parse_nullable_integer(
            v,
            &format!("{field}.maxWarnings"),
            meta,
            "null or a non-negative integer",
            |c| c >= 0,
        )
        .map(|n| n.map_or(Value::Null, Value::from))
    })?;
    take(input, &mut patch, "guidance", |v| {
        parse_guidance_patch(v, &format!("{field}.guidance"), meta)
    })?;
    take(input, &mut patch, "autoFollow", |v| {
        parse_auto_follow_patch(v, &format!("{field}.autoFollow"), meta)
    })?;
    take(input, &mut patch, "scope", |v| {
        parse_scope_patch(v, &format!("{field}.scope"), meta)
    })?;
    take(input, &mut patch, "cadence", |v| {
        parse_cadence_patch(v, &format!("{field}.cadence"), meta)
    })?;
    take(input, &mut patch, "main", |v| {
        parse_endpoint_like_patch(v, &format!("{field}.main"), meta, ENDPOINT_FIELDS)
    })?;
    take(input, &mut patch, "children", |v| {
        parse_children_patch(v, &format!("{field}.children"), meta)
    })?;
    take(input, &mut patch, "asyncCompletion", |v| {
        parse_async_completion_patch(v, &format!("{field}.asyncCompletion"), meta)
    })?;
    take(input, &mut patch, "lsp", |v| {
        parse_lsp_patch(v, &format!("{field}.lsp"), meta)
    })?;
    take(input, &mut patch, "compactAtPercent", |v| {
        parse_integer(
            v,
            &format!("{field}.compactAtPercent"),
            meta,
            "an integer from 50 through 95",
            |c| (50..=95).contains(&c),
        )
        .map(Value::from)
    })?;
    take(input, &mut patch, "reviewRetryDelayMs", |v| {
        parse_integer(
            v,
            &format!("{field}.reviewRetryDelayMs"),
            meta,
            "a positive integer",
            |c| c >= 1,
        )
        .map(Value::from)
    })?;
    take(input, &mut patch, "maxReviewFailures", |v| {
        parse_integer(
            v,
            &format!("{field}.maxReviewFailures"),
            meta,
            "a positive integer",
            |c| c >= 1,
        )
        .map(Value::from)
    })?;
    Ok(Value::Object(patch))
}

/// `parseSettingsObject` (`settings.ts:384-389`) — dig `subagents.watchdog` out of a whole
/// settings file; anything else in the file is none of this parser's business.
fn parse_settings_object(settings: &Value, meta: &ParseMeta) -> Result<Value, String> {
    let Some(root) = as_plain_object(settings) else {
        return Ok(Value::Object(Map::new()));
    };
    let Some(subagents_value) = root.get("subagents") else {
        return Ok(Value::Object(Map::new()));
    };
    let subagents = parse_object(subagents_value, "subagents", meta)?;
    let Some(watchdog) = subagents.get("watchdog") else {
        return Ok(Value::Object(Map::new()));
    };
    parse_watchdog_patch(watchdog, "subagents.watchdog", meta)
}

/// `parseSessionOverride` (`settings.ts:466-469`) — the session layer accepts either the wrapped
/// or the bare shape.
fn parse_session_override(value: &Value, meta: &ParseMeta) -> Result<Value, String> {
    if value.get("subagents").is_some() {
        return parse_settings_object(value, meta);
    }
    parse_watchdog_patch(value, "subagents.watchdog", meta)
}

// =================================================================================================
// Files (`settings.ts:391-444, 508-512`)
// =================================================================================================

/// `readSettingsFileStrict` (`settings.ts:391-412`) — a missing file is the empty object; an
/// unreadable, unparseable or non-object one is an error.
fn read_settings_file_strict(file_path: &Path) -> Result<Value, String> {
    if !file_path.exists() {
        return Ok(Value::Object(Map::new()));
    }
    let raw = std::fs::read_to_string(file_path).map_err(|e| {
        format!(
            "Failed to read settings file '{}': {e}",
            file_path.display()
        )
    })?;
    let parsed: Value = serde_json::from_str(&raw).map_err(|e| {
        format!(
            "Failed to parse settings file '{}': {e}",
            file_path.display()
        )
    })?;
    if as_plain_object(&parsed).is_none() {
        return Err(format!(
            "Settings file '{}' must contain a JSON object.",
            file_path.display()
        ));
    }
    Ok(parsed)
}

/// `isDirectory` (`settings.ts:414-420`).
fn is_directory(dir: &Path) -> bool {
    std::fs::metadata(dir).map(|m| m.is_dir()).unwrap_or(false)
}

/// The user home dir, following this crate's existing `CYRUP_HOME` -> `HOME` -> tempdir convention
/// (`exec/mcp_direct_tools.rs:831-836`, itself mirroring `extension.rs::dirs_home`).
fn home_dir() -> PathBuf {
    std::env::var_os("CYRUP_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir)
}

/// `getAgentDir()` (`shared/utils.ts:95-100`) — `$CYRUP_AGENT_DIR`/`$PI_CODING_AGENT_DIR` with `~`
/// expansion, else `<home>/.cyrup/agent`. Byte-identical to
/// `exec/mcp_direct_tools.rs`'s `resolve_agent_dir`, which is the crate's existing port of the same
/// upstream function.
fn agent_dir() -> PathBuf {
    let home = home_dir();
    let configured = std::env::var("CYRUP_AGENT_DIR")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| {
            std::env::var("PI_CODING_AGENT_DIR")
                .ok()
                .filter(|v| !v.is_empty())
        });
    match configured {
        Some(v) if v == "~" => home,
        Some(v) if v.starts_with("~/") => home.join(v.get(2..).unwrap_or("")),
        Some(v) => PathBuf::from(v),
        None => home.join(".cyrup").join("agent"),
    }
}

/// `getProjectConfigDir(projectRoot)` (`shared/utils.ts:91-93`) — `<root>/.cyrup` (upstream
/// `<root>/.pi`), the same directory `cyrup_config::ConfigDirs::project_config_dir` names.
fn project_config_dir(project_root: &Path) -> PathBuf {
    project_root.join(".cyrup")
}

/// `getUserSettingsPath` (`settings.ts:422-424`).
fn user_settings_path() -> PathBuf {
    agent_dir().join("settings.json")
}

/// `getWatchdogUserSettingsPath()` (`settings.ts:426-428`) — the path `/subagents-watchdog on|off`
/// names in its confirmation and in its failure message.
#[must_use]
pub fn get_watchdog_user_settings_path() -> PathBuf {
    user_settings_path()
}

/// `getProjectSettingsPath(cwd)` (`settings.ts:430-440`) — walk UP from `cwd` for the first
/// directory that has a `.cyrup` or a `.agents` directory, and take that directory's
/// `.cyrup/settings.json`. `None` when the walk reaches the filesystem root without finding one, in
/// which case there is no project layer at all.
fn find_project_settings_path(cwd: &Path) -> Option<PathBuf> {
    let mut current = cwd.to_path_buf();
    loop {
        if is_directory(&project_config_dir(&current)) || is_directory(&current.join(".agents")) {
            return Some(project_config_dir(&current).join("settings.json"));
        }
        let parent = current.parent()?.to_path_buf();
        if parent == current {
            return None;
        }
        current = parent;
    }
}

/// `getWatchdogProjectSettingsPath(cwd)` (`settings.ts:442-444`) — the WRITE target, which is
/// always `<cwd>/.cyrup/settings.json` with no upward walk at all.
#[must_use]
pub fn get_watchdog_project_settings_path(cwd: &Path) -> PathBuf {
    project_config_dir(cwd).join("settings.json")
}

// =================================================================================================
// Merge + resolve (`settings.ts:446-482`)
// =================================================================================================

/// `deepMerge(base, patch)` (`settings.ts:446-453`) — recursive for two plain objects, replacing
/// otherwise. An explicit `null` REPLACES (it is not a plain object), which is how
/// `maxAttempts: null` clears a default.
fn deep_merge(base: &Value, patch: &Value) -> Value {
    let (Some(base_obj), Some(patch_obj)) = (as_plain_object(base), as_plain_object(patch)) else {
        return patch.clone();
    };
    let mut next = base_obj.clone();
    for (key, value) in patch_obj {
        let merged = match next.get(key) {
            Some(current) => deep_merge(current, value),
            None => value.clone(),
        };
        next.insert(key.clone(), merged);
    }
    Value::Object(next)
}

/// `resolvePatch(patch)` (`settings.ts:455-460`): merge onto a fresh default, then apply the two
/// rules that are NOT plain merges —
///
/// * `config.enabled` comes from the patch or the DEFAULT (never from a nested merge), and
/// * `config.main.enabled` defaults to `config.enabled`, so turning the watchdog on turns the main
///   endpoint on without naming it.
fn resolve_patch(patch: &Value) -> ResolvedWatchdogConfig {
    let default = default_watchdog_config();
    let Ok(default_value) = serde_json::to_value(&default) else {
        return default;
    };
    let mut merged = deep_merge(&default_value, patch);
    let patch_enabled = patch.get("enabled").and_then(Value::as_bool);
    let enabled = patch_enabled.unwrap_or(false);
    let main_enabled = patch
        .get("main")
        .and_then(|m| m.get("enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(enabled);
    if let Some(obj) = merged.as_object_mut() {
        obj.insert("enabled".to_string(), Value::Bool(enabled));
        if let Some(main) = obj.get_mut("main").and_then(Value::as_object_mut) {
            main.insert("enabled".to_string(), Value::Bool(main_enabled));
        }
    }
    // Validation has already rejected every shape this could trip on, so the deserialize is
    // total in practice; the defaults are the no-panic fallback for the unreachable case.
    serde_json::from_value(merged).unwrap_or(default)
}

/// `parseSourceFile(filePath, scope)` (`settings.ts:462-464`). The `scope` argument only ever
/// reached [`ParseMeta`], which does not carry it — see that type's doc — so it is dropped here.
fn parse_source_file(file_path: &Path) -> Result<Value, String> {
    let meta = ParseMeta {
        path: Some(file_path.display().to_string()),
    };
    let settings = read_settings_file_strict(file_path)?;
    parse_settings_object(&settings, &meta)
}

/// `resolveWatchdogConfigStrict(cwd, { session })` (`settings.ts:471-482`) — the throwing variant,
/// used where a caller wants the failure rather than a silent fallback.
///
/// # Errors
/// The first layer's parse failure, verbatim.
pub fn resolve_watchdog_config_strict(
    cwd: &Path,
    session: Option<&Value>,
) -> Result<ResolvedWatchdogConfig, String> {
    let mut patch = Value::Object(Map::new());
    patch = deep_merge(
        &patch,
        &parse_source_file(&user_settings_path())?,
    );
    if let Some(project_path) = find_project_settings_path(cwd) {
        patch = deep_merge(
            &patch,
            &parse_source_file(&project_path)?,
        );
    }
    if let Some(session) = session {
        let meta = ParseMeta {
            path: None,
        };
        patch = deep_merge(&patch, &parse_session_override(session, &meta)?);
    }
    Ok(resolve_patch(&patch))
}

/// `resolveWatchdogConfig(cwd, { session })` (`settings.ts:537-568`) — the NON-throwing variant
/// every runtime path uses: each layer's failure is recorded and the whole result falls back to the
/// pristine defaults, so a broken config disables the watchdog rather than half-configuring it.
#[must_use]
pub fn resolve_watchdog_config(cwd: &Path, session: Option<&Value>) -> WatchdogSettingsResult {
    let mut sources: Vec<WatchdogSettingsSource> = Vec::new();
    let mut errors: Vec<WatchdogSettingsError> = Vec::new();
    let mut patch = Value::Object(Map::new());

    let source_specs: [(WatchdogSettingsScope, Option<PathBuf>); 2] = [
        (WatchdogSettingsScope::User, Some(user_settings_path())),
        (
            WatchdogSettingsScope::Project,
            find_project_settings_path(cwd),
        ),
    ];
    for (scope, path) in source_specs {
        let Some(path) = path else { continue };
        sources.push(WatchdogSettingsSource {
            scope,
            path: Some(path.display().to_string()),
            exists: path.exists(),
        });
        match parse_source_file(&path) {
            Ok(layer) => patch = deep_merge(&patch, &layer),
            Err(message) => errors.push(WatchdogSettingsError {
                scope,
                path: Some(path.display().to_string()),
                message,
            }),
        }
    }
    if let Some(session) = session {
        sources.push(WatchdogSettingsSource {
            scope: WatchdogSettingsScope::Session,
            path: None,
            exists: true,
        });
        let meta = ParseMeta {
            path: None,
        };
        match parse_session_override(session, &meta) {
            Ok(layer) => patch = deep_merge(&patch, &layer),
            Err(message) => errors.push(WatchdogSettingsError {
                scope: WatchdogSettingsScope::Session,
                path: None,
                message,
            }),
        }
    }

    let ok = errors.is_empty();
    WatchdogSettingsResult {
        ok,
        config: if ok {
            resolve_patch(&patch)
        } else {
            default_watchdog_config()
        },
        errors,
        sources,
    }
}

// =================================================================================================
// Writes (`settings.ts:41-53, 484-535`)
// =================================================================================================

/// `WatchdogSettingsWriteScope` (`settings.ts:41`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogSettingsWriteScope {
    /// `<agentDir>/settings.json`.
    User,
    /// `<cwd>/.cyrup/settings.json`.
    Project,
}

/// `WatchdogModelSettingsTarget` (`settings.ts:42-45`) — which endpoint block the write lands in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchdogModelSettingsTarget {
    /// `subagents.watchdog.main`.
    Main,
    /// `subagents.watchdog.children`.
    Children,
    /// `subagents.watchdog.children.overrides.<agent>`.
    Child(String),
}

/// `WatchdogModelSettingsWrite` (`settings.ts:47-53`).
///
/// `model`/`thinking` carry upstream's THREE states: `None` is `undefined` (leave the key alone),
/// `Some(None)` is `null` (DELETE the key), `Some(Some(v))` sets it.
#[derive(Debug, Clone)]
pub struct WatchdogModelSettingsWrite {
    /// Which file.
    pub scope: WatchdogSettingsWriteScope,
    /// The project root, for [`WatchdogSettingsWriteScope::Project`]; defaults to the process cwd.
    pub cwd: Option<PathBuf>,
    /// Which endpoint block.
    pub target: WatchdogModelSettingsTarget,
    /// The model id.
    pub model: Option<Option<String>>,
    /// The reasoning level.
    pub thinking: Option<Option<ThinkingSetting>>,
}

/// `ensureObjectField` (`settings.ts:484-488`) — create the key if absent, error if it holds a
/// non-object.
fn ensure_object_field<'a>(
    parent: &'a mut Map<String, Value>,
    key: &str,
    field: &str,
    meta: &ParseMeta,
) -> Result<&'a mut Map<String, Value>, String> {
    if !parent.contains_key(key) {
        parent.insert(key.to_string(), Value::Object(Map::new()));
    }
    parent
        .get_mut(key)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| meta.invalid(field, "an object"))
}

/// `settingsPathForWrite` (`settings.ts:495-497`).
fn settings_path_for_write(scope: WatchdogSettingsWriteScope, cwd: Option<&Path>) -> PathBuf {
    match scope {
        WatchdogSettingsWriteScope::User => user_settings_path(),
        WatchdogSettingsWriteScope::Project => {
            let cwd = cwd
                .map(Path::to_path_buf)
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| PathBuf::from("."));
            get_watchdog_project_settings_path(&cwd)
        }
    }
}

/// `writeSettingsFile` (`settings.ts:508-512`) — 2-space JSON plus a trailing newline.
///
/// # Errors
/// Any filesystem failure, as the message the slash command renders.
fn write_settings_file(settings_path: &Path, settings: &Value) -> Result<PathBuf, String> {
    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!(
                "Failed to create settings directory '{}': {e}",
                parent.display()
            )
        })?;
    }
    let body = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("Failed to serialize settings: {e}"))?;
    std::fs::write(settings_path, format!("{body}\n")).map_err(|e| {
        format!(
            "Failed to write settings file '{}': {e}",
            settings_path.display()
        )
    })?;
    Ok(settings_path.to_path_buf())
}

/// The read-modify-write body shared by both writers: read strictly, dig/create
/// `subagents.watchdog` (`ensureWatchdogSettings`, `settings.ts:490-493`), let `edit` mutate it,
/// write back.
fn edit_watchdog_settings<F>(settings_path: &Path, meta: &ParseMeta, edit: F) -> Result<PathBuf, String>
where
    F: FnOnce(&mut Map<String, Value>, &ParseMeta) -> Result<(), String>,
{
    let mut settings = read_settings_file_strict(settings_path)?;
    let root = settings
        .as_object_mut()
        .ok_or_else(|| meta.invalid("subagents", "an object"))?;
    {
        let subagents = ensure_object_field(root, "subagents", "subagents", meta)?;
        let watchdog = ensure_object_field(subagents, "watchdog", "subagents.watchdog", meta)?;
        edit(watchdog, meta)?;
    }
    write_settings_file(settings_path, &settings)
}

/// `targetSettingsObject` (`settings.ts:499-506`).
fn target_settings_object<'a>(
    watchdog: &'a mut Map<String, Value>,
    target: &WatchdogModelSettingsTarget,
    meta: &ParseMeta,
) -> Result<&'a mut Map<String, Value>, String> {
    match target {
        WatchdogModelSettingsTarget::Main => {
            ensure_object_field(watchdog, "main", "subagents.watchdog.main", meta)
        }
        WatchdogModelSettingsTarget::Children => {
            ensure_object_field(watchdog, "children", "subagents.watchdog.children", meta)
        }
        WatchdogModelSettingsTarget::Child(agent) => {
            let agent = agent.trim().to_string();
            if agent.is_empty() {
                return Err(meta.invalid(
                    "subagents.watchdog.children.overrides.<agent>",
                    "a non-empty agent name",
                ));
            }
            let children =
                ensure_object_field(watchdog, "children", "subagents.watchdog.children", meta)?;
            let overrides = ensure_object_field(
                children,
                "overrides",
                "subagents.watchdog.children.overrides",
                meta,
            )?;
            let field = format!("subagents.watchdog.children.overrides.{agent}");
            ensure_object_field(overrides, &agent, &field, meta)
        }
    }
}

/// `writeUserWatchdogEnabled(enabled)` (`settings.ts:514-522`) — sets BOTH
/// `subagents.watchdog.enabled` and `subagents.watchdog.main.enabled`, so `/subagents-watchdog on`
/// cannot leave the master switch on with the main endpoint pinned off by an earlier explicit
/// `main.enabled: false`.
///
/// # Errors
/// An unreadable/unparseable settings file, or a write failure.
pub fn write_user_watchdog_enabled(enabled: bool) -> Result<PathBuf, String> {
    let settings_path = user_settings_path();
    let meta = ParseMeta {
        path: Some(settings_path.display().to_string()),
    };
    edit_watchdog_settings(&settings_path, &meta, |watchdog, meta| {
        watchdog.insert("enabled".to_string(), Value::Bool(enabled));
        target_settings_object(watchdog, &WatchdogModelSettingsTarget::Main, meta)?
            .insert("enabled".to_string(), Value::Bool(enabled));
        Ok(())
    })
}

/// `writeWatchdogModelSettings(input)` (`settings.ts:524-535`).
///
/// # Errors
/// An unreadable/unparseable settings file, an empty agent name, or a write failure.
pub fn write_watchdog_model_settings(input: &WatchdogModelSettingsWrite) -> Result<PathBuf, String> {
    let settings_path = settings_path_for_write(input.scope, input.cwd.as_deref());
    let meta = ParseMeta {
        path: Some(settings_path.display().to_string()),
    };
    edit_watchdog_settings(&settings_path, &meta, |watchdog, meta| {
        let target = target_settings_object(watchdog, &input.target, meta)?;
        match &input.model {
            Some(None) => {
                target.remove("model");
            }
            Some(Some(model)) => {
                target.insert("model".to_string(), Value::String(model.clone()));
            }
            None => {}
        }
        match &input.thinking {
            Some(None) => {
                target.remove("thinking");
            }
            Some(Some(thinking)) => {
                let value = serde_json::to_value(thinking)
                    .map_err(|e| format!("Failed to serialize thinking: {e}"))?;
                target.insert("thinking".to_string(), value);
            }
            None => {}
        }
        Ok(())
    })
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
    use serde_json::json;

    fn meta() -> ParseMeta {
        ParseMeta {
            path: None,
        }
    }

    fn file_meta() -> ParseMeta {
        ParseMeta {
            path: Some("/tmp/settings.json".to_string()),
        }
    }

    #[test]
    fn the_defaults_are_upstreams_defaults() {
        let config = default_watchdog_config();
        assert!(!config.enabled, "default OFF");
        assert!(!config.main.enabled);
        assert_eq!(config.agent_end_timeout_ms, 30_000);
        assert_eq!(config.severity_threshold, WatchdogSeverity::Concern);
        assert_eq!(config.max_warnings, None);
        assert!(config.guidance.watchdog_md);
        assert!(config.auto_follow.blockers);
        assert_eq!(config.auto_follow.max_attempts, Some(3));
        assert_eq!(config.auto_follow.stalemate_repeats, 3);
        assert!(config.scope.enabled);
        assert_eq!(config.cadence.every_n_tools, None);
        assert_eq!(config.children.watchdog_tail_timeout_ms, 120_000);
        assert!(config.children.overrides.is_empty());
        assert!(config.lsp.enabled);
        assert_eq!(config.lsp.timeout_ms, 3_000);
        assert_eq!(config.lsp.max_files, 20);
        assert_eq!(config.lsp.max_diagnostics, 50);
        assert_eq!(config.compact_at_percent, 80);
        assert_eq!(config.review_retry_delay_ms, 1_000);
        assert_eq!(config.max_review_failures, 3);
    }

    #[test]
    fn enabling_the_watchdog_enables_the_main_endpoint_without_naming_it() {
        let config = resolve_patch(&json!({ "enabled": true }));
        assert!(config.enabled);
        assert!(config.main.enabled, "main.enabled inherits config.enabled");
    }

    #[test]
    fn an_explicit_main_enabled_false_survives_the_master_switch() {
        let config = resolve_patch(&json!({ "enabled": true, "main": { "enabled": false } }));
        assert!(config.enabled);
        assert!(!config.main.enabled);
    }

    #[test]
    fn an_unknown_field_is_rejected_with_its_full_path() {
        let error = parse_watchdog_patch(
            &json!({ "enabled": true, "nope": 1 }),
            "subagents.watchdog",
            &file_meta(),
        )
        .unwrap_err();
        assert_eq!(
            error,
            "Watchdog settings in '/tmp/settings.json' have unknown field 'subagents.watchdog.nope'."
        );
    }

    #[test]
    fn a_nested_unknown_field_names_the_nested_path() {
        let error = parse_watchdog_patch(
            &json!({ "lsp": { "timeoutMs": 10, "wat": true } }),
            "subagents.watchdog",
            &meta(),
        )
        .unwrap_err();
        assert_eq!(
            error,
            "Watchdog settings in session override have unknown field 'subagents.watchdog.lsp.wat'."
        );
    }

    #[test]
    fn a_wrong_typed_value_names_what_was_expected() {
        let error =
            parse_watchdog_patch(&json!({ "enabled": "yes" }), "subagents.watchdog", &meta())
                .unwrap_err();
        assert_eq!(
            error,
            "Watchdog settings in session override have invalid 'subagents.watchdog.enabled'; expected a boolean."
        );
    }

    #[test]
    fn the_cadence_floor_is_five_not_one() {
        assert!(parse_watchdog_patch(
            &json!({ "cadence": { "everyNTools": 4 } }),
            "subagents.watchdog",
            &meta()
        )
        .is_err());
        assert!(parse_watchdog_patch(
            &json!({ "cadence": { "everyNTools": 5 } }),
            "subagents.watchdog",
            &meta()
        )
        .is_ok());
        // `null` is the "boundary only" setting and is always allowed.
        let config = resolve_patch(
            &parse_watchdog_patch(
                &json!({ "cadence": { "everyNTools": null } }),
                "subagents.watchdog",
                &meta(),
            )
            .unwrap(),
        );
        assert_eq!(config.cadence.every_n_tools, None);
    }

    #[test]
    fn compact_at_percent_is_bounded_at_both_ends() {
        for bad in [49, 96] {
            assert!(
                parse_watchdog_patch(
                    &json!({ "compactAtPercent": bad }),
                    "subagents.watchdog",
                    &meta()
                )
                .is_err(),
                "{bad} should be rejected"
            );
        }
        for good in [50, 80, 95] {
            assert!(
                parse_watchdog_patch(
                    &json!({ "compactAtPercent": good }),
                    "subagents.watchdog",
                    &meta()
                )
                .is_ok(),
                "{good} should be accepted"
            );
        }
    }

    #[test]
    fn max_diagnostics_alone_accepts_zero() {
        assert!(parse_watchdog_patch(
            &json!({ "lsp": { "maxDiagnostics": 0 } }),
            "subagents.watchdog",
            &meta()
        )
        .is_ok());
        assert!(parse_watchdog_patch(
            &json!({ "lsp": { "maxFiles": 0 } }),
            "subagents.watchdog",
            &meta()
        )
        .is_err());
    }

    #[test]
    fn thinking_accepts_a_level_or_the_json_false_but_not_the_string() {
        let patch = parse_watchdog_patch(
            &json!({ "main": { "thinking": "high" } }),
            "subagents.watchdog",
            &meta(),
        )
        .unwrap();
        assert_eq!(
            resolve_patch(&patch).main.thinking,
            Some(ThinkingSetting::Level("high".into()))
        );
        let patch = parse_watchdog_patch(
            &json!({ "main": { "thinking": false } }),
            "subagents.watchdog",
            &meta(),
        )
        .unwrap();
        assert_eq!(resolve_patch(&patch).main.thinking, Some(ThinkingSetting::Off));
        let error = parse_watchdog_patch(
            &json!({ "main": { "thinking": "false" } }),
            "subagents.watchdog",
            &meta(),
        )
        .unwrap_err();
        assert!(error.contains("'off' or 'minimal' or 'low' or 'medium' or 'high' or 'xhigh' or 'max' or false"), "{error}");
    }

    #[test]
    fn a_model_string_is_trimmed_into_the_patch() {
        let patch = parse_watchdog_patch(
            &json!({ "main": { "model": "  anthropic/claude  " } }),
            "subagents.watchdog",
            &meta(),
        )
        .unwrap();
        assert_eq!(
            resolve_patch(&patch).main.model.as_deref(),
            Some("anthropic/claude")
        );
    }

    #[test]
    fn a_blank_model_string_is_rejected() {
        assert!(parse_watchdog_patch(
            &json!({ "main": { "model": "   " } }),
            "subagents.watchdog",
            &meta()
        )
        .is_err());
    }

    #[test]
    fn an_empty_agent_override_name_is_rejected() {
        let error = parse_watchdog_patch(
            &json!({ "children": { "overrides": { "  ": { "enabled": true } } } }),
            "subagents.watchdog",
            &meta(),
        )
        .unwrap_err();
        assert!(error.contains("agent names to be non-empty"), "{error}");
    }

    #[test]
    fn overrides_survive_into_the_resolved_config() {
        let patch = parse_watchdog_patch(
            &json!({ "children": { "overrides": { "reviewer": { "enabled": true, "model": "openai/gpt", "thinking": false } } } }),
            "subagents.watchdog",
            &meta(),
        )
        .unwrap();
        let config = resolve_patch(&patch);
        let entry = config.children.overrides.get("reviewer").unwrap();
        assert_eq!(entry.enabled, Some(true));
        assert_eq!(entry.model.as_deref(), Some("openai/gpt"));
        assert_eq!(entry.thinking, Some(ThinkingSetting::Off));
    }

    #[test]
    fn a_null_max_attempts_clears_the_default_three() {
        let patch = parse_watchdog_patch(
            &json!({ "autoFollow": { "maxAttempts": null } }),
            "subagents.watchdog",
            &meta(),
        )
        .unwrap();
        assert_eq!(resolve_patch(&patch).auto_follow.max_attempts, None);
    }

    #[test]
    fn sync_backlog_takes_off_or_a_positive_integer() {
        use super::super::types::WatchdogSyncBacklog;
        let patch =
            parse_watchdog_patch(&json!({ "syncBacklog": 4 }), "subagents.watchdog", &meta())
                .unwrap();
        assert_eq!(resolve_patch(&patch).sync_backlog, WatchdogSyncBacklog::Count(4));
        assert!(parse_watchdog_patch(
            &json!({ "syncBacklog": 0 }),
            "subagents.watchdog",
            &meta()
        )
        .is_err());
        assert!(parse_watchdog_patch(
            &json!({ "syncBacklog": "on" }),
            "subagents.watchdog",
            &meta()
        )
        .is_err());
    }

    #[test]
    fn deep_merge_replaces_scalars_and_recurses_objects() {
        let merged = deep_merge(
            &json!({ "a": 1, "nested": { "x": 1, "y": 2 } }),
            &json!({ "a": 2, "nested": { "y": 3, "z": 4 } }),
        );
        assert_eq!(merged, json!({ "a": 2, "nested": { "x": 1, "y": 3, "z": 4 } }));
    }

    #[test]
    fn a_settings_file_without_the_watchdog_key_contributes_nothing() {
        let patch = parse_settings_object(&json!({ "theme": "dark" }), &file_meta()).unwrap();
        assert_eq!(patch, json!({}));
        let patch = parse_settings_object(&json!({ "subagents": { "other": 1 } }), &file_meta())
            .unwrap();
        assert_eq!(patch, json!({}));
    }

    #[test]
    fn the_session_layer_accepts_both_the_wrapped_and_the_bare_shape() {
        let wrapped = parse_session_override(
            &json!({ "subagents": { "watchdog": { "enabled": true } } }),
            &meta(),
        )
        .unwrap();
        let bare = parse_session_override(&json!({ "enabled": true }), &meta()).unwrap();
        assert_eq!(wrapped, bare);
    }

    // ---- file-backed layering ------------------------------------------------------------------

    struct Fixture {
        _root: tempfile::TempDir,
        project: PathBuf,
    }

    /// A temp project root carrying the `.cyrup` marker directory the project-layer walk looks for.
    ///
    /// The USER layer's path comes from the process environment (`agent_dir()`), and this crate is
    /// `#![forbid(unsafe_code)]` while Rust 2024 requires `unsafe` for `std::env::set_var`, so a
    /// test cannot relocate it. These tests therefore drive the pure path resolution
    /// ([`find_project_settings_path`]) and the parse/merge/write core against explicit paths —
    /// which is where every decision this module makes lives — plus the layering behaviour that is
    /// observable without moving the user file (the user layer is always LISTED as a source, and an
    /// absent user file contributes the empty patch).
    fn fixture() -> Fixture {
        let root = tempfile::tempdir().expect("tempdir");
        let project = root.path().join("project");
        std::fs::create_dir_all(project.join(".cyrup")).expect("project dir");
        Fixture {
            _root: root,
            project,
        }
    }

    #[test]
    fn the_project_layer_walks_up_to_the_nearest_marked_directory() {
        let fx = fixture();
        let nested = fx.project.join("a").join("b");
        std::fs::create_dir_all(&nested).expect("nested");
        let found = find_project_settings_path(&nested).expect("found a project layer");
        assert_eq!(found, fx.project.join(".cyrup").join("settings.json"));
    }

    #[test]
    fn an_agents_directory_also_marks_the_project_root() {
        let root = tempfile::tempdir().expect("tempdir");
        let project = root.path().join("proj");
        std::fs::create_dir_all(project.join(".agents")).expect(".agents");
        let nested = project.join("deep").join("deeper");
        std::fs::create_dir_all(&nested).expect("nested");
        let found = find_project_settings_path(&nested).expect("found");
        assert_eq!(found, project.join(".cyrup").join("settings.json"));
    }

    #[test]
    fn a_strict_read_rejects_a_non_object_settings_file() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("settings.json");
        std::fs::write(&path, "[]").expect("write");
        let error = read_settings_file_strict(&path).unwrap_err();
        assert!(error.ends_with("must contain a JSON object."), "{error}");
    }

    #[test]
    fn a_strict_read_reports_a_syntax_error() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("settings.json");
        std::fs::write(&path, "{ not json").expect("write");
        let error = read_settings_file_strict(&path).unwrap_err();
        assert!(error.starts_with("Failed to parse settings file '"), "{error}");
    }

    #[test]
    fn a_missing_settings_file_is_the_empty_object() {
        let root = tempfile::tempdir().expect("tempdir");
        let value = read_settings_file_strict(&root.path().join("nope.json")).unwrap();
        assert_eq!(value, json!({}));
    }

    #[test]
    fn a_model_write_creates_the_nested_blocks_and_preserves_the_rest_of_the_file() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("settings.json");
        std::fs::write(&path, r#"{"theme":"dark","subagents":{"maxDepth":3}}"#).expect("write");
        let meta = ParseMeta {
            path: Some(path.display().to_string()),
        };
        edit_watchdog_settings(&path, &meta, |watchdog, meta| {
            let target = target_settings_object(watchdog, &WatchdogModelSettingsTarget::Main, meta)?;
            target.insert("model".to_string(), json!("anthropic/opus"));
            Ok(())
        })
        .expect("write");
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert_eq!(written["theme"], json!("dark"));
        assert_eq!(written["subagents"]["maxDepth"], json!(3));
        assert_eq!(
            written["subagents"]["watchdog"]["main"]["model"],
            json!("anthropic/opus")
        );
        assert!(
            std::fs::read_to_string(&path).expect("read").ends_with("\n"),
            "trailing newline"
        );
    }

    #[test]
    fn a_null_model_deletes_the_key_and_leaves_the_sibling() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"subagents":{"watchdog":{"main":{"model":"a/b","thinking":"high"}}}}"#,
        )
        .expect("write");
        let meta = ParseMeta {
            path: Some(path.display().to_string()),
        };
        edit_watchdog_settings(&path, &meta, |watchdog, meta| {
            let target = target_settings_object(watchdog, &WatchdogModelSettingsTarget::Main, meta)?;
            target.remove("model");
            Ok(())
        })
        .expect("write");
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert!(written["subagents"]["watchdog"]["main"]["model"].is_null());
        assert_eq!(
            written["subagents"]["watchdog"]["main"]["thinking"],
            json!("high")
        );
    }

    #[test]
    fn a_write_refuses_a_settings_file_it_cannot_parse() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("settings.json");
        std::fs::write(&path, "{oops").expect("write");
        let meta = ParseMeta {
            path: Some(path.display().to_string()),
        };
        let error = edit_watchdog_settings(&path, &meta, |_, _| Ok(())).unwrap_err();
        assert!(error.starts_with("Failed to parse settings file '"), "{error}");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "{oops",
            "the unparseable file is left untouched"
        );
    }

    #[test]
    fn a_non_object_watchdog_key_is_an_error_not_an_overwrite() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("settings.json");
        std::fs::write(&path, r#"{"subagents":{"watchdog":"on"}}"#).expect("write");
        let meta = ParseMeta {
            path: Some(path.display().to_string()),
        };
        let error = edit_watchdog_settings(&path, &meta, |_, _| Ok(())).unwrap_err();
        assert!(
            error.contains("invalid 'subagents.watchdog'; expected an object."),
            "{error}"
        );
    }

    #[test]
    fn an_agent_override_target_creates_the_overrides_chain() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("settings.json");
        let meta = ParseMeta {
            path: Some(path.display().to_string()),
        };
        edit_watchdog_settings(&path, &meta, |watchdog, meta| {
            let target = target_settings_object(
                watchdog,
                &WatchdogModelSettingsTarget::Child(" reviewer ".to_string()),
                meta,
            )?;
            target.insert("model".to_string(), json!("openai/gpt"));
            Ok(())
        })
        .expect("write");
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert_eq!(
            written["subagents"]["watchdog"]["children"]["overrides"]["reviewer"]["model"],
            json!("openai/gpt"),
            "the agent name is trimmed"
        );
    }

    #[test]
    fn an_empty_agent_override_target_is_rejected() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("settings.json");
        let meta = ParseMeta {
            path: Some(path.display().to_string()),
        };
        let error = edit_watchdog_settings(&path, &meta, |watchdog, meta| {
            target_settings_object(
                watchdog,
                &WatchdogModelSettingsTarget::Child("   ".to_string()),
                meta,
            )?;
            Ok(())
        })
        .unwrap_err();
        assert!(error.contains("a non-empty agent name"), "{error}");
        assert!(!path.exists(), "nothing was written");
    }

    #[test]
    fn a_broken_layer_falls_back_to_the_pristine_defaults() {
        // `resolve_watchdog_config` catches per layer; simulate the same shape directly since the
        // user layer's path comes from the process environment.
        let bad = parse_watchdog_patch(
            &json!({ "enabled": true, "bogus": 1 }),
            "subagents.watchdog",
            &meta(),
        );
        assert!(bad.is_err());
        let result = WatchdogSettingsResult {
            ok: false,
            config: default_watchdog_config(),
            errors: vec![WatchdogSettingsError {
                scope: WatchdogSettingsScope::Session,
                path: None,
                message: bad.unwrap_err(),
            }],
            sources: Vec::new(),
        };
        assert!(!result.ok);
        assert!(!result.config.enabled, "a broken config disables the watchdog");
    }

    #[test]
    fn resolving_against_a_directory_with_no_project_layer_still_yields_the_user_layer() {
        let fx = fixture();
        let result = resolve_watchdog_config(&fx.project, None);
        // The user layer is always consulted, whether or not the file exists.
        assert!(
            result
                .sources
                .iter()
                .any(|s| s.scope == WatchdogSettingsScope::User),
            "{:?}",
            result.sources
        );
        assert!(result.ok, "{:?}", result.errors);
    }

    #[test]
    fn a_session_override_layers_over_the_files() {
        let fx = fixture();
        let session = json!({ "enabled": true, "main": { "model": "anthropic/opus" } });
        let result = resolve_watchdog_config(&fx.project, Some(&session));
        assert!(result.ok, "{:?}", result.errors);
        assert!(result.config.enabled);
        assert!(result.config.main.enabled);
        assert_eq!(result.config.main.model.as_deref(), Some("anthropic/opus"));
        assert!(
            result
                .sources
                .iter()
                .any(|s| s.scope == WatchdogSettingsScope::Session && s.exists),
            "{:?}",
            result.sources
        );
    }

    #[test]
    fn a_broken_session_override_is_reported_and_disables_the_watchdog() {
        let fx = fixture();
        let session = json!({ "enabled": true, "typo": 1 });
        let result = resolve_watchdog_config(&fx.project, Some(&session));
        assert!(!result.ok);
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].scope, WatchdogSettingsScope::Session);
        assert!(!result.config.enabled);
    }
}
