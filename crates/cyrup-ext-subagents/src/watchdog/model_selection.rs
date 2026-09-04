//! Watchdog model selection — a 1:1 port of `pi-subagents/src/watchdog/model-selection.ts` (167
//! lines @v0.43.0).
//!
//! A watchdog that reviews with the same model that did the work is a rubber stamp. This module is
//! the machinery for picking a genuinely INDEPENDENT reviewer:
//!
//! * [`resolve_watchdog_model_input`] (`:76-97`) validates a user- or settings-supplied model string
//!   all the way down — resolve it against the registry, split any `:level` suffix, require it to
//!   name a `provider/model`, require the registry to know it, and require its provider to be
//!   AUTHENTICATED. Every failure is its own message, because "watchdog model didn't work" is
//!   useless to a user who mistyped a provider.
//! * [`recommend_strong_watchdog_model`] (`:158-166`) picks the strongest complementary model
//!   available. The complement rule ([`strong_family_order`], `:119-127`) is the interesting part:
//!   if the session is ALREADY on one of the two strong families, the recommendation is the OTHER
//!   one outright — never the same family, whatever else is installed. Only when the session is on
//!   neither does the provider of the session model break the tie, and even then it picks the
//!   opposite vendor first.
//!
//! Three guards make the recommendation trustworthy rather than aspirational (`resolveStrongCandidate`,
//! `:135-156`): a candidate must survive full validation INCLUDING the auth check, must still be
//! recognized as a member of the family it was queried for (so a provider aliasing an unrelated
//! model to a familiar name cannot masquerade), and must support `thinking: high` — the whole point
//! of the recommendation.
//!
//! [CYRUP-DELTA] `ctx.modelRegistry` is a live TypeScript object; here it is the
//! [`WatchdogModelRegistry`] trait, with [`BuiltinWatchdogModelRegistry`] as the production
//! implementation over `cyrup_provider::catalog::builtin_catalog` and
//! `cyrup_config::AuthStore::has_auth`. That trait is also what makes every rule above testable
//! against a fixed catalog rather than against whatever the developer happens to have credentials
//! for.
//!
//! [CYRUP-DELTA] `resolveModelCandidate`/`normalizeModelSegment` are ported HERE rather than reused
//! from [`crate::extension`], whose private copy predates v0.43.0's fuzzy resolution
//! (`model-fallback.ts:90-134`): that copy resolves only exact ids, so `anthropic/claude-opus-4.8`
//! would not match a catalog entry spelled `claude-opus-4-8` and the recommendation's whole
//! dotted-alias query list (`:20-24,27-31`) would silently never match. `model-fallback.ts` is not
//! in this batch's file set, so the shared copy is left alone rather than changed underneath its
//! own callers.

use std::collections::BTreeMap;

use crate::exec::split_known_thinking_suffix;

/// `THINKING_LEVELS` (`shared/model-info.ts:1`).
pub const THINKING_LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// `STRONG_WATCHDOG_THINKING` (`model-selection.ts:12`).
pub const STRONG_WATCHDOG_THINKING: &str = "high";

/// `ModelInfo` (`shared/model-info.ts:5-12`) — the registry projection every function here consults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchdogModelInfo {
    /// Provider id.
    pub provider: String,
    /// Model id within the provider.
    pub id: String,
    /// `provider/id`.
    pub full_id: String,
    /// The wire API family.
    pub api: Option<String>,
    /// Whether the model reasons at all.
    pub reasoning: Option<bool>,
    /// Per-level overrides; a `None` value marks the level UNSUPPORTED.
    pub thinking_level_map: Option<BTreeMap<String, Option<String>>>,
}

impl WatchdogModelInfo {
    /// `toModelInfo` (`shared/model-info.ts:22-30`) over a `provider`/`id` pair.
    #[must_use]
    pub fn new(provider: impl Into<String>, id: impl Into<String>) -> Self {
        let provider = provider.into();
        let id = id.into();
        Self {
            full_id: format!("{provider}/{id}"),
            provider,
            id,
            api: None,
            reasoning: None,
            thinking_level_map: None,
        }
    }
}

/// `ctx.modelRegistry` — the three methods `model-selection.ts` and `review.ts` call on it.
pub trait WatchdogModelRegistry: Send + Sync {
    /// `getAvailable()`.
    fn available(&self) -> Vec<WatchdogModelInfo>;
    /// `find(provider, id)`.
    fn find(&self, provider: &str, id: &str) -> Option<WatchdogModelInfo>;
    /// `hasConfiguredAuth(model)`.
    fn has_configured_auth(&self, model: &WatchdogModelInfo) -> bool;
}

/// The `ExtensionContext` slice this module reads: the registry plus `ctx.model`, the live session
/// model (absent when no model is bound).
pub struct WatchdogModelContext<'a> {
    /// The registry.
    pub registry: &'a dyn WatchdogModelRegistry,
    /// `ctx.model`.
    pub current_model: Option<WatchdogModelInfo>,
}

impl<'a> WatchdogModelContext<'a> {
    /// Bind a context to a registry with no session model.
    #[must_use]
    pub fn new(registry: &'a dyn WatchdogModelRegistry) -> Self {
        Self {
            registry,
            current_model: None,
        }
    }

    /// Bind a context with a session model.
    #[must_use]
    pub fn with_current_model(mut self, model: Option<WatchdogModelInfo>) -> Self {
        self.current_model = model;
        self
    }

    /// `typeof ctx.model?.provider === "string" ? ctx.model.provider : undefined` (`:80`).
    fn preferred_provider(&self) -> Option<&str> {
        self.current_model
            .as_ref()
            .map(|model| model.provider.as_str())
    }
}

/// The production registry: `cyrup_provider`'s complete built-in catalog, with authentication
/// answered by `cyrup_config`'s real `auth.json`/env/runtime-override store.
///
/// [CYRUP-DELTA] `getAvailable()` upstream is already auth-filtered; this returns the whole catalog
/// and lets [`Self::has_configured_auth`] do the filtering at the point of use. That is a strictly
/// WIDER candidate list than pi's, never a narrower one, so no model pi would offer is hidden —
/// and every path that would actually USE a model runs the auth check first
/// ([`resolve_watchdog_model_input`], `:88-91`), which is where the narrowing has to happen anyway.
pub struct BuiltinWatchdogModelRegistry {
    /// `None` when the process could not resolve its config layout at all (no home directory);
    /// every model then reports UNAUTHENTICATED, which is the fail-closed direction — a watchdog
    /// model is refused rather than attempted without credentials.
    auth: Option<cyrup_config::AuthStore>,
}

impl BuiltinWatchdogModelRegistry {
    /// Open the registry against the given config directories' `auth.json`.
    #[must_use]
    pub fn new(dirs: Option<&cyrup_config::ConfigDirs>) -> Self {
        Self {
            auth: dirs.map(cyrup_config::AuthStore::open),
        }
    }
}

fn model_info_from_catalog(model: &cyrup_provider::Model) -> WatchdogModelInfo {
    WatchdogModelInfo {
        provider: model.provider.as_str().to_string(),
        id: model.id.as_str().to_string(),
        full_id: format!("{}/{}", model.provider.as_str(), model.id.as_str()),
        api: Some(model.api.as_str().to_string()),
        reasoning: Some(model.reasoning),
        thinking_level_map: model.thinking_level_map.clone(),
    }
}

impl WatchdogModelRegistry for BuiltinWatchdogModelRegistry {
    fn available(&self) -> Vec<WatchdogModelInfo> {
        cyrup_provider::catalog::builtin_catalog()
            .iter()
            .map(model_info_from_catalog)
            .collect()
    }

    fn find(&self, provider: &str, id: &str) -> Option<WatchdogModelInfo> {
        cyrup_provider::catalog::builtin_catalog()
            .iter()
            .find(|model| model.provider.as_str() == provider && model.id.as_str() == id)
            .map(model_info_from_catalog)
    }

    fn has_configured_auth(&self, model: &WatchdogModelInfo) -> bool {
        self.auth.as_ref().is_some_and(|auth| {
            auth.has_auth(&cyrup_core::ProviderId::from(model.provider.clone()), None)
        })
    }
}

// -------------------------------------------------------------------------------------------
// `shared/model-info.ts` + `runs/shared/model-fallback.ts` helpers (see the module-doc delta)
// -------------------------------------------------------------------------------------------

/// `normalizeModelSegment` (`model-fallback.ts:45-51`): case-fold, `.`/`_` runs become `-`, `-` runs
/// collapse, leading/trailing `-` dropped.
#[must_use]
pub fn normalize_model_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    let mut pending_dash = false;
    for ch in segment.chars().flat_map(char::to_lowercase) {
        if ch == '.' || ch == '_' || ch == '-' {
            pending_dash = true;
            continue;
        }
        if pending_dash && !out.is_empty() {
            out.push('-');
        }
        pending_dash = false;
        out.push(ch);
    }
    out
}

/// `isPlausibleDateStamp` (`model-fallback.ts:53-59`).
fn is_plausible_date_stamp(year: &str, month: &str, day: &str) -> bool {
    let (Ok(y), Ok(m), Ok(d)) = (
        year.parse::<u32>(),
        month.parse::<u32>(),
        day.parse::<u32>(),
    ) else {
        return false;
    };
    (1900..=2099).contains(&y) && (1..=12).contains(&m) && (1..=31).contains(&d)
}

/// `stripTrailingDateStamp` (`model-fallback.ts:61-67`): drop a `-20251001` or `-2025-10-01` tail so
/// dated and undated ids compare equal.
#[must_use]
pub fn strip_trailing_date_stamp(segment: &str) -> String {
    let parts: Vec<&str> = segment.rsplitn(4, '-').collect();
    // `rsplitn` yields reversed: [day, month, year, head] for the dashed form.
    if let [day, month, year, head] = parts.as_slice()
        && day.len() == 2
        && month.len() == 2
        && year.len() == 4
        && is_plausible_date_stamp(year, month, day)
    {
        return (*head).to_string();
    }
    if let Some((head, compact)) = segment.rsplit_once('-')
        && compact.len() == 8
        && compact.chars().all(|c| c.is_ascii_digit())
        && is_plausible_date_stamp(
            compact.get(..4).unwrap_or(""),
            compact.get(4..6).unwrap_or(""),
            compact.get(6..8).unwrap_or(""),
        )
    {
        return head.to_string();
    }
    segment.to_string()
}

/// `fuzzyResolveModel` (`model-fallback.ts:90-134`).
///
/// A qualified query only ever matches within the named provider — this must never silently switch
/// providers for a cost- or security-sensitive config — and an ambiguous match with no preferred
/// provider resolves to nothing rather than to an arbitrary winner.
#[must_use]
pub fn fuzzy_resolve_model(
    base_model: &str,
    available: &[WatchdogModelInfo],
    preferred_provider: Option<&str>,
) -> Option<String> {
    let mut query_provider: Option<String> = None;
    let mut query_id_raw = base_model.to_string();
    if let Some(slash) = base_model.find('/') {
        query_provider = Some(normalize_model_segment(
            base_model.get(..slash).unwrap_or(""),
        ));
        query_id_raw = base_model.get(slash + 1..).unwrap_or("").to_string();
    } else {
        for separator in [':', '.'] {
            let Some(idx) = base_model.find(separator) else {
                continue;
            };
            if idx == 0 {
                continue;
            }
            let provider_part = normalize_model_segment(base_model.get(..idx).unwrap_or(""));
            if !available
                .iter()
                .any(|entry| normalize_model_segment(&entry.provider) == provider_part)
            {
                continue;
            }
            query_provider = Some(provider_part);
            query_id_raw = base_model.get(idx + 1..).unwrap_or("").to_string();
            break;
        }
    }
    let query_id = normalize_model_segment(&query_id_raw);
    let query_id_no_date = strip_trailing_date_stamp(&query_id);
    let candidates: Vec<&WatchdogModelInfo> = available
        .iter()
        .filter(|entry| {
            let entry_id = normalize_model_segment(&entry.id);
            if entry_id != query_id && strip_trailing_date_stamp(&entry_id) != query_id_no_date {
                return false;
            }
            match query_provider.as_deref() {
                Some(provider) => normalize_model_segment(&entry.provider) == provider,
                None => true,
            }
        })
        .collect();
    if candidates.is_empty() {
        return None;
    }
    if let Some(preferred) = preferred_provider {
        let preferred = normalize_model_segment(preferred);
        if let Some(entry) = candidates
            .iter()
            .find(|entry| normalize_model_segment(&entry.provider) == preferred)
        {
            return Some(entry.full_id.clone());
        }
    }
    if candidates.len() == 1 {
        return candidates.first().map(|entry| entry.full_id.clone());
    }
    None
}

/// `splitThinkingSuffix` (`model-fallback.ts:14-21`) — the UNCONDITIONAL split on the last `:`,
/// distinct from [`crate::exec::split_known_thinking_suffix`].
fn split_thinking_suffix(model: &str) -> (&str, &str) {
    match model.rfind(':') {
        Some(idx) => (
            model.get(..idx).unwrap_or(model),
            model.get(idx..).unwrap_or(""),
        ),
        None => (model, ""),
    }
}

/// `resolveBaseModelCandidate` (`model-fallback.ts:71-88`): exact match first, then fuzzy.
fn resolve_base_model_candidate(
    base_model: &str,
    available: &[WatchdogModelInfo],
    preferred_provider: Option<&str>,
) -> Option<String> {
    if base_model.contains('/') {
        if let Some(exact) = available.iter().find(|entry| entry.full_id == base_model) {
            return Some(exact.full_id.clone());
        }
    } else {
        let exact_matches: Vec<&WatchdogModelInfo> = available
            .iter()
            .filter(|entry| entry.id == base_model)
            .collect();
        if let Some(preferred) = preferred_provider
            && let Some(entry) = exact_matches
                .iter()
                .find(|entry| entry.provider == preferred)
        {
            return Some(entry.full_id.clone());
        }
        if exact_matches.len() == 1 {
            return exact_matches.first().map(|entry| entry.full_id.clone());
        }
    }
    fuzzy_resolve_model(base_model, available, preferred_provider)
}

/// `resolveModelCandidate` (`model-fallback.ts:147-163`): resolve the whole string first, then retry
/// with any trailing `:suffix` split off and re-attached.
#[must_use]
pub fn resolve_model_candidate(
    model: &str,
    available: &[WatchdogModelInfo],
    preferred_provider: Option<&str>,
) -> Option<String> {
    if model.is_empty() {
        return None;
    }
    if available.is_empty() {
        return Some(model.to_string());
    }
    if let Some(resolved) = resolve_base_model_candidate(model, available, preferred_provider) {
        return Some(resolved);
    }
    let (base_model, thinking_suffix) = split_thinking_suffix(model);
    if thinking_suffix.is_empty() {
        return Some(model.to_string());
    }
    match resolve_base_model_candidate(base_model, available, preferred_provider) {
        Some(resolved) => Some(format!("{resolved}{thinking_suffix}")),
        None => Some(model.to_string()),
    }
}

/// `getSupportedThinkingLevels` (`shared/model-info.ts:69-82`).
///
/// Note the two special cases: an absent model or an absent `thinkingLevelMap` supports every level
/// EXCEPT `max`, and `xhigh`/`max` require an explicit map entry where every other level is
/// supported unless explicitly nulled.
#[must_use]
pub fn get_supported_thinking_levels(model: Option<&WatchdogModelInfo>) -> Vec<&'static str> {
    let all_but_max = || {
        THINKING_LEVELS
            .iter()
            .copied()
            .filter(|level| *level != "max")
            .collect::<Vec<_>>()
    };
    let Some(model) = model else {
        return all_but_max();
    };
    if model.reasoning == Some(false) {
        return vec!["off"];
    }
    let Some(map) = model.thinking_level_map.as_ref() else {
        return all_but_max();
    };
    THINKING_LEVELS
        .iter()
        .copied()
        .filter(|level| match map.get(*level) {
            Some(None) => false,
            Some(Some(_)) => true,
            None => *level != "xhigh" && *level != "max",
        })
        .collect()
}

// -------------------------------------------------------------------------------------------
// `model-selection.ts` proper
// -------------------------------------------------------------------------------------------

/// The two strong families and their alias queries (`STRONG_WATCHDOG_MODELS`, `:14-33`).
///
/// Each family lists four spellings because a provider may register the model under a dotted or a
/// dashed id and under either an official or a short provider name; the fuzzy resolver collapses
/// most of that, but the explicit list is what makes the FIRST query win deterministically.
const STRONG_WATCHDOG_MODELS: &[(StrongWatchdogFamily, &str, &[&str])] = &[
    (
        StrongWatchdogFamily::Opus48,
        "Opus 4.8",
        &[
            "anthropic/claude-opus-4-8",
            "anthropic/claude-opus-4.8",
            "anthropic/opus-4-8",
            "anthropic/opus-4.8",
        ],
    ),
    (
        StrongWatchdogFamily::Gpt55,
        "GPT 5.5",
        &[
            "openai-codex/gpt-5.5",
            "openai-codex/gpt-5-5",
            "openai/gpt-5.5",
            "openai/gpt-5-5",
        ],
    ),
];

/// `StrongWatchdogFamily` (`model-selection.ts:35`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrongWatchdogFamily {
    /// Anthropic's Opus 4.8.
    Opus48,
    /// OpenAI's GPT 5.5.
    Gpt55,
}

fn family_entry(family: StrongWatchdogFamily) -> Option<(&'static str, &'static [&'static str])> {
    STRONG_WATCHDOG_MODELS
        .iter()
        .find(|(candidate, _, _)| *candidate == family)
        .map(|(_, label, queries)| (*label, *queries))
}

/// `ResolvedWatchdogModelInput` (`model-selection.ts:39-43`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedWatchdogModelInput {
    /// The canonical `provider/id`.
    pub model: String,
    /// The level a `:suffix` on the input pinned, if any.
    pub thinking: Option<String>,
    /// The registry entry it resolved to.
    pub registry_model: WatchdogModelInfo,
}

/// `WatchdogModelRecommendation` (`model-selection.ts:45-51`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchdogModelRecommendation {
    /// The canonical `provider/id`.
    pub model: String,
    /// Always [`STRONG_WATCHDOG_THINKING`].
    pub thinking: String,
    /// The human family label.
    pub label: String,
    /// The one-sentence justification shown to the user.
    pub reason: String,
    /// The registry entry.
    pub registry_model: WatchdogModelInfo,
}

/// `splitProviderModel` (`model-selection.ts:61-65`): a `/` that is neither first nor last.
fn split_provider_model(value: &str) -> Option<(&str, &str)> {
    let slash = value.find('/')?;
    if slash == 0 || slash == value.len() - 1 {
        return None;
    }
    Some((value.get(..slash)?, value.get(slash + 1..)?))
}

/// `assertSupportedThinking` (`model-selection.ts:67-70`).
fn assert_supported_thinking(value: &str, source: &str) -> Result<String, String> {
    if THINKING_LEVELS.contains(&value) {
        return Ok(value.to_string());
    }
    Err(format!(
        "Unsupported watchdog thinking '{value}' from {source}; expected {}, false, or inherit.",
        THINKING_LEVELS.join(", ")
    ))
}

/// The `string | false | undefined` a thinking input carries, as
/// `parseWatchdogThinkingInput` returns it (`model-selection.ts:72-78`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchdogThinkingInput {
    /// Reasoning off (the literal `false`, or the STRING `"false"` — this parser accepts both).
    Off,
    /// A validated level.
    Level(String),
}

/// `parseWatchdogThinkingInput` (`model-selection.ts:72-78`).
///
/// `undefined` and `""` both yield `None`; `false` and the string `"false"` both yield
/// [`WatchdogThinkingInput::Off`] — the string form matters because this parser reads TOOL
/// arguments and slash-command text, where a boolean cannot survive.
///
/// # Errors
///
/// Returns the message [`assert_supported_thinking`] builds for an unrecognized level.
pub fn parse_watchdog_thinking_input(
    value: Option<&str>,
    source: &str,
) -> Result<Option<WatchdogThinkingInput>, String> {
    match value {
        None | Some("") => Ok(None),
        Some("false") => Ok(Some(WatchdogThinkingInput::Off)),
        Some(level) => Ok(Some(WatchdogThinkingInput::Level(
            assert_supported_thinking(level, source)?,
        ))),
    }
}

/// `resolveWatchdogModelInput` (`model-selection.ts:80-97`) — full validation of a model string.
///
/// # Errors
///
/// One message per failure mode, verbatim from upstream: empty input, unresolvable to
/// `provider/model`, unknown to the registry, or unauthenticated.
pub fn resolve_watchdog_model_input(
    ctx: &WatchdogModelContext<'_>,
    raw_model: &str,
) -> Result<ResolvedWatchdogModelInput, String> {
    let trimmed = raw_model.trim();
    if trimmed.is_empty() {
        return Err("Watchdog model must be a non-empty provider/model value.".to_string());
    }
    let available = ctx.registry.available();
    let resolved = resolve_model_candidate(trimmed, &available, ctx.preferred_provider())
        .unwrap_or_else(|| trimmed.to_string());
    let (base_model, thinking_suffix) = split_known_thinking_suffix(&resolved);
    let Some((provider, id)) = split_provider_model(base_model) else {
        return Err(format!(
            "Watchdog model '{raw_model}' did not resolve to provider/model. Use a \
             provider-qualified model such as openai-codex/gpt-5.5:high or \
             anthropic/claude-opus-4-8:high."
        ));
    };
    let Some(registry_model) = ctx.registry.find(provider, id) else {
        return Err(format!(
            "Watchdog model '{raw_model}' was not found as '{base_model}'."
        ));
    };
    if !ctx.registry.has_configured_auth(&registry_model) {
        return Err(format!(
            "Watchdog model '{base_model}' is not authenticated. Configure credentials for \
             provider '{provider}' or choose an authenticated model."
        ));
    }
    let thinking = match thinking_suffix.strip_prefix(':') {
        Some(level) => Some(assert_supported_thinking(level, "watchdog model suffix")?),
        None => None,
    };
    Ok(ResolvedWatchdogModelInput {
        model: format!("{provider}/{id}"),
        thinking,
        registry_model,
    })
}

/// `familyForModel` (`model-selection.ts:99-106`) — the id patterns that define membership, with an
/// optional trailing date stamp in either spelling.
#[must_use]
pub fn family_for_model(model: Option<&WatchdogModelInfo>) -> Option<StrongWatchdogFamily> {
    let model = model?;
    let provider = normalize_model_segment(&model.provider);
    let id = normalize_model_segment(&model.id);
    let base = strip_trailing_date_stamp(&id);
    if provider.contains("openai") && base == "gpt-5-5" {
        return Some(StrongWatchdogFamily::Gpt55);
    }
    if provider.contains("anthropic") && (base == "claude-opus-4-8" || base == "opus-4-8") {
        return Some(StrongWatchdogFamily::Opus48);
    }
    None
}

/// `currentProviderFamily` (`model-selection.ts:108-113`).
fn current_provider_family(ctx: &WatchdogModelContext<'_>) -> Option<&'static str> {
    let provider = ctx
        .current_model
        .as_ref()
        .map(|model| normalize_model_segment(&model.provider))
        .unwrap_or_default();
    if provider.contains("openai") {
        return Some("openai");
    }
    if provider.contains("anthropic") {
        return Some("anthropic");
    }
    None
}

/// `strongFamilyOrder` (`model-selection.ts:115-123`) — the complement rule.
///
/// Already on a strong family? The order is the OTHER family ALONE — never a fallback to the same
/// one, which is exactly what makes the reviewer independent. Otherwise the session's provider
/// breaks the tie by preferring the opposite vendor, and with no session model at all the default is
/// `gpt55` then `opus48`.
#[must_use]
pub fn strong_family_order(ctx: &WatchdogModelContext<'_>) -> Vec<StrongWatchdogFamily> {
    match family_for_model(ctx.current_model.as_ref()) {
        Some(StrongWatchdogFamily::Gpt55) => return vec![StrongWatchdogFamily::Opus48],
        Some(StrongWatchdogFamily::Opus48) => return vec![StrongWatchdogFamily::Gpt55],
        None => {}
    }
    match current_provider_family(ctx) {
        Some("openai") => vec![StrongWatchdogFamily::Opus48, StrongWatchdogFamily::Gpt55],
        _ => vec![StrongWatchdogFamily::Gpt55, StrongWatchdogFamily::Opus48],
    }
}

/// `findFamilyMatch` (`model-selection.ts:125-129`): a registry-derived query, added only when
/// EXACTLY one available model belongs to the family — an ambiguous family contributes nothing.
fn find_family_match(
    family: StrongWatchdogFamily,
    available: &[WatchdogModelInfo],
) -> Option<String> {
    let matches: Vec<&WatchdogModelInfo> = available
        .iter()
        .filter(|entry| family_for_model(Some(entry)) == Some(family))
        .collect();
    if matches.len() == 1 {
        return matches.first().map(|entry| entry.full_id.clone());
    }
    None
}

/// `fullModelId` (`model-selection.ts:53-55`).
fn full_model_id(model: &WatchdogModelInfo) -> String {
    format!("{}/{}", model.provider, model.id)
}

/// `resolveStrongCandidate` (`model-selection.ts:131-156`).
fn resolve_strong_candidate(
    ctx: &WatchdogModelContext<'_>,
    family: StrongWatchdogFamily,
) -> Option<WatchdogModelRecommendation> {
    let available = ctx.registry.available();
    let (label, base_queries) = family_entry(family)?;
    let mut queries: Vec<String> = base_queries.iter().map(|q| (*q).to_string()).collect();
    if let Some(family_match) = find_family_match(family, &available) {
        queries.push(family_match);
    }
    for query in queries {
        // A query that fails validation (unknown, unauthenticated, unqualified) is simply the next
        // candidate's turn — upstream's bare `catch { continue; }`.
        let Ok(resolved) = resolve_watchdog_model_input(ctx, &query) else {
            continue;
        };
        // Re-check membership against what the registry actually returned, so an alias cannot
        // smuggle an unrelated model in under a familiar name.
        if family_for_model(Some(&resolved.registry_model)) != Some(family) {
            continue;
        }
        if !get_supported_thinking_levels(Some(&resolved.registry_model))
            .contains(&STRONG_WATCHDOG_THINKING)
        {
            continue;
        }
        let current = ctx
            .current_model
            .as_ref()
            .map_or_else(|| "no current session model".to_string(), full_model_id);
        return Some(WatchdogModelRecommendation {
            model: resolved.model,
            thinking: STRONG_WATCHDOG_THINKING.to_string(),
            label: label.to_string(),
            reason: format!(
                "Use {label} with thinking high as a strong independent watchdog for {current}."
            ),
            registry_model: resolved.registry_model,
        });
    }
    None
}

/// `recommendStrongWatchdogModel` (`model-selection.ts:158-166`).
///
/// # Errors
///
/// Upstream's verbatim message when neither family yields an authenticated, high-thinking candidate.
pub fn recommend_strong_watchdog_model(
    ctx: &WatchdogModelContext<'_>,
) -> Result<WatchdogModelRecommendation, String> {
    for family in strong_family_order(ctx) {
        if let Some(recommendation) = resolve_strong_candidate(ctx, family) {
            return Ok(recommendation);
        }
    }
    let current = ctx
        .current_model
        .as_ref()
        .map_or_else(|| "the current session".to_string(), full_model_id);
    Err(format!(
        "No authenticated strong complementary watchdog model was found for {current}. Configure \
         access to Opus 4.8 or GPT 5.5, then run the recommendation again."
    ))
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

    /// A fixed catalog, so every rule is tested against known models rather than against whatever
    /// the developer happens to have credentials for.
    struct FakeRegistry {
        models: Vec<WatchdogModelInfo>,
        unauthenticated: Vec<String>,
    }

    impl FakeRegistry {
        fn new(models: Vec<WatchdogModelInfo>) -> Self {
            Self {
                models,
                unauthenticated: Vec::new(),
            }
        }

        fn without_auth(mut self, provider: &str) -> Self {
            self.unauthenticated.push(provider.to_string());
            self
        }
    }

    impl WatchdogModelRegistry for FakeRegistry {
        fn available(&self) -> Vec<WatchdogModelInfo> {
            self.models.clone()
        }
        fn find(&self, provider: &str, id: &str) -> Option<WatchdogModelInfo> {
            self.models
                .iter()
                .find(|m| m.provider == provider && m.id == id)
                .cloned()
        }
        fn has_configured_auth(&self, model: &WatchdogModelInfo) -> bool {
            !self.unauthenticated.contains(&model.provider)
        }
    }

    fn reasoning(provider: &str, id: &str) -> WatchdogModelInfo {
        let mut model = WatchdogModelInfo::new(provider, id);
        model.reasoning = Some(true);
        model
    }

    fn both_families() -> FakeRegistry {
        FakeRegistry::new(vec![
            reasoning("anthropic", "claude-opus-4-8"),
            reasoning("openai-codex", "gpt-5-5"),
            reasoning("anthropic", "claude-sonnet-4-5"),
        ])
    }

    #[test]
    fn normalize_folds_case_dots_underscores_and_runs() {
        assert_eq!(normalize_model_segment("GPT_5.5"), "gpt-5-5");
        assert_eq!(normalize_model_segment("--a__b..c--"), "a-b-c");
        assert_eq!(normalize_model_segment("Anthropic"), "anthropic");
    }

    #[test]
    fn date_stamps_are_stripped_in_both_spellings_and_only_when_plausible() {
        assert_eq!(strip_trailing_date_stamp("gpt-5-5-20251001"), "gpt-5-5");
        assert_eq!(strip_trailing_date_stamp("gpt-5-5-2025-10-01"), "gpt-5-5");
        assert_eq!(
            strip_trailing_date_stamp("gpt-5-5-2025-99-01"),
            "gpt-5-5-2025-99-01"
        );
        assert_eq!(strip_trailing_date_stamp("gpt-5-5"), "gpt-5-5");
    }

    #[test]
    fn fuzzy_resolution_matches_a_dotted_alias_to_a_dashed_catalog_id() {
        let registry = both_families();
        let available = registry.available();
        assert_eq!(
            resolve_model_candidate("anthropic/claude-opus-4.8", &available, None),
            Some("anthropic/claude-opus-4-8".to_string())
        );
        // A qualified query never switches providers.
        assert_eq!(
            resolve_model_candidate("openai/claude-opus-4.8", &available, None),
            Some("openai/claude-opus-4.8".to_string())
        );
    }

    #[test]
    fn a_thinking_suffix_survives_resolution() {
        let registry = both_families();
        assert_eq!(
            resolve_model_candidate("claude-opus-4.8:high", &registry.available(), None),
            Some("anthropic/claude-opus-4-8:high".to_string())
        );
    }

    #[test]
    fn resolve_input_reports_each_failure_mode_distinctly() {
        let registry = both_families().without_auth("openai-codex");
        let ctx = WatchdogModelContext::new(&registry);
        assert_eq!(
            resolve_watchdog_model_input(&ctx, "  ").unwrap_err(),
            "Watchdog model must be a non-empty provider/model value."
        );
        assert!(
            resolve_watchdog_model_input(&ctx, "nonsense")
                .unwrap_err()
                .contains("did not resolve to provider/model")
        );
        assert_eq!(
            resolve_watchdog_model_input(&ctx, "anthropic/unknown-model").unwrap_err(),
            "Watchdog model 'anthropic/unknown-model' was not found as 'anthropic/unknown-model'."
        );
        assert!(
            resolve_watchdog_model_input(&ctx, "openai-codex/gpt-5-5")
                .unwrap_err()
                .contains("is not authenticated")
        );
    }

    #[test]
    fn resolve_input_returns_the_canonical_id_and_the_suffix_level() {
        let registry = both_families();
        let ctx = WatchdogModelContext::new(&registry);
        let resolved =
            resolve_watchdog_model_input(&ctx, "anthropic/claude-opus-4.8:high").unwrap();
        assert_eq!(resolved.model, "anthropic/claude-opus-4-8");
        assert_eq!(resolved.thinking.as_deref(), Some("high"));
        let bare = resolve_watchdog_model_input(&ctx, "anthropic/claude-opus-4-8").unwrap();
        assert_eq!(bare.thinking, None);
    }

    #[test]
    fn an_unrecognized_suffix_level_is_reported_not_silently_dropped() {
        let mut models = both_families().models;
        models.push(reasoning("anthropic", "claude-opus-4-8:wild"));
        let registry = FakeRegistry::new(models);
        let ctx = WatchdogModelContext::new(&registry);
        // `:wild` is not a THINKING_LEVEL, so `split_known_thinking_suffix` leaves it on the id and
        // the model simply is not found — which is the correct outcome, not a thinking error.
        assert!(
            resolve_watchdog_model_input(&ctx, "anthropic/claude-opus-4-8:wild")
                .unwrap()
                .thinking
                .is_none()
        );
    }

    #[test]
    fn the_complement_rule_never_recommends_the_family_already_in_use() {
        let registry = both_families();
        let on_gpt = WatchdogModelContext::new(&registry)
            .with_current_model(Some(reasoning("openai-codex", "gpt-5-5")));
        assert_eq!(
            strong_family_order(&on_gpt),
            vec![StrongWatchdogFamily::Opus48]
        );
        assert_eq!(
            recommend_strong_watchdog_model(&on_gpt).unwrap().model,
            "anthropic/claude-opus-4-8"
        );
        let on_opus = WatchdogModelContext::new(&registry)
            .with_current_model(Some(reasoning("anthropic", "claude-opus-4-8")));
        assert_eq!(
            strong_family_order(&on_opus),
            vec![StrongWatchdogFamily::Gpt55]
        );
        assert_eq!(
            recommend_strong_watchdog_model(&on_opus).unwrap().model,
            "openai-codex/gpt-5-5"
        );
    }

    #[test]
    fn a_non_family_session_model_breaks_the_tie_by_opposing_vendor() {
        let registry = both_families();
        let anthropic_session = WatchdogModelContext::new(&registry)
            .with_current_model(Some(reasoning("anthropic", "claude-sonnet-4-5")));
        assert_eq!(
            strong_family_order(&anthropic_session),
            vec![StrongWatchdogFamily::Gpt55, StrongWatchdogFamily::Opus48]
        );
        let openai_session = WatchdogModelContext::new(&registry)
            .with_current_model(Some(reasoning("openai", "gpt-4o")));
        assert_eq!(
            strong_family_order(&openai_session),
            vec![StrongWatchdogFamily::Opus48, StrongWatchdogFamily::Gpt55]
        );
        // No session model at all takes the default order.
        assert_eq!(
            strong_family_order(&WatchdogModelContext::new(&registry)),
            vec![StrongWatchdogFamily::Gpt55, StrongWatchdogFamily::Opus48]
        );
    }

    #[test]
    fn a_candidate_that_cannot_think_high_is_rejected() {
        let mut opus = reasoning("anthropic", "claude-opus-4-8");
        opus.thinking_level_map = Some(BTreeMap::from([("high".to_string(), None)]));
        let registry = FakeRegistry::new(vec![opus]);
        let ctx = WatchdogModelContext::new(&registry);
        let err = recommend_strong_watchdog_model(&ctx).unwrap_err();
        assert!(err.starts_with("No authenticated strong complementary watchdog model was found"));
    }

    #[test]
    fn an_unauthenticated_family_falls_through_to_the_other() {
        let registry = both_families().without_auth("openai-codex");
        let ctx = WatchdogModelContext::new(&registry);
        // Default order prefers gpt55, which has no auth, so opus48 wins.
        let recommendation = recommend_strong_watchdog_model(&ctx).unwrap();
        assert_eq!(recommendation.model, "anthropic/claude-opus-4-8");
        assert_eq!(recommendation.label, "Opus 4.8");
        assert_eq!(recommendation.thinking, STRONG_WATCHDOG_THINKING);
        assert!(recommendation.reason.contains("no current session model"));
    }

    #[test]
    fn family_membership_tolerates_a_date_stamp_but_not_a_different_model() {
        assert_eq!(
            family_for_model(Some(&WatchdogModelInfo::new(
                "anthropic",
                "claude-opus-4-8-20251001"
            ))),
            Some(StrongWatchdogFamily::Opus48)
        );
        assert_eq!(
            family_for_model(Some(&WatchdogModelInfo::new("openai", "gpt-5.5"))),
            Some(StrongWatchdogFamily::Gpt55)
        );
        assert_eq!(
            family_for_model(Some(&WatchdogModelInfo::new(
                "anthropic",
                "claude-opus-4-7"
            ))),
            None
        );
        assert_eq!(
            family_for_model(Some(&WatchdogModelInfo::new("mistral", "gpt-5-5"))),
            None,
            "the provider must match too"
        );
        assert_eq!(family_for_model(None), None);
    }

    #[test]
    fn supported_thinking_levels_follow_the_map_and_its_two_special_cases() {
        assert_eq!(
            get_supported_thinking_levels(None),
            vec!["off", "minimal", "low", "medium", "high", "xhigh"]
        );
        let mut non_reasoning = WatchdogModelInfo::new("p", "m");
        non_reasoning.reasoning = Some(false);
        assert_eq!(
            get_supported_thinking_levels(Some(&non_reasoning)),
            vec!["off"]
        );
        let mut mapped = WatchdogModelInfo::new("p", "m");
        mapped.thinking_level_map = Some(BTreeMap::from([
            ("low".to_string(), None),
            ("max".to_string(), Some("x".to_string())),
        ]));
        assert_eq!(
            get_supported_thinking_levels(Some(&mapped)),
            vec!["off", "minimal", "medium", "high", "max"]
        );
    }

    #[test]
    fn thinking_input_accepts_both_false_forms_and_rejects_an_unknown_level() {
        assert_eq!(parse_watchdog_thinking_input(None, "t").unwrap(), None);
        assert_eq!(parse_watchdog_thinking_input(Some(""), "t").unwrap(), None);
        assert_eq!(
            parse_watchdog_thinking_input(Some("false"), "t").unwrap(),
            Some(WatchdogThinkingInput::Off)
        );
        assert_eq!(
            parse_watchdog_thinking_input(Some("high"), "t").unwrap(),
            Some(WatchdogThinkingInput::Level("high".into()))
        );
        let err = parse_watchdog_thinking_input(Some("turbo"), "watchdog input").unwrap_err();
        assert_eq!(
            err,
            "Unsupported watchdog thinking 'turbo' from watchdog input; expected off, minimal, \
             low, medium, high, xhigh, max, false, or inherit."
        );
    }
}
