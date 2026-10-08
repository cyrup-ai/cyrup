//! Typed host aggregation for the discovery/startup events (gap-08 #4/#15/#16). The guest answers
//! `project_trust` / `resources_discover` via `hook-outcome::handled(json)`; the host folds those
//! raw [`crate::HandledValue`]s into the typed decisions Pi's runner consumes:
//! `project_trust` → a `{trusted, remember}` decision (Pi `ProjectTrustEvent`, types.ts:503-513;
//! runner.ts:197-227 — the FIRST extension that returns a DECIDED `"yes"|"no"` wins, `"undecided"`
//! falls through), and `resources_discover` → the CONCATENATION of every extension's skill/prompt/
//! theme paths, each attributed `{path, extensionPath}` (Pi `ResourcesEvent`, types.ts:528;
//! runner.ts:1046-1092 — no de-duplication, per-path attribution).

use crate::contract::HandledValue;
use cyrup_core::ExtensionId;

/// A project-trust decision folded from the `project_trust` handlers (Pi `{trusted, remember}`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectTrustDecision {
    /// Whether the project is trusted (project-local extensions may load). The resolved `"yes"`/`"no"`
    /// of Pi's tri-state — `"undecided"` never produces a decision (it falls through).
    pub trusted: bool,
    /// Whether to persist the decision (Pi `remember`).
    pub remember: bool,
    /// The extension whose decision was taken (the first to DECIDE, Pi semantics).
    pub by: ExtensionId,
}

/// Pi's tri-state `project_trust` decision (`ProjectTrustEventDecision`, types.ts:508): `"yes"` /
/// `"no"` are terminal; `"undecided"` falls through to the next handler. A legacy JSON boolean
/// `trusted` is tolerated (`true`→yes, `false`→no) so older payloads still decide.
pub fn parse_trust_decision(v: &serde_json::Value) -> Option<bool> {
    match v.get("trusted") {
        Some(serde_json::Value::String(s)) => match s.as_str() {
            "yes" => Some(true),
            "no" => Some(false),
            // "undecided" (or anything else) = no decision; fall through (Pi runner.ts:214).
            _ => None,
        },
        // Legacy boolean form: decide directly.
        Some(serde_json::Value::Bool(b)) => Some(*b),
        _ => None,
    }
}

/// Fold the collected `project_trust` handled values into a decision: the FIRST extension that
/// returns a DECIDED tri-state (`"yes"`/`"no"`) wins (Pi runner.ts:197-227); `"undecided"` handlers
/// fall through. `None` = no extension decided (the host falls back to its own trust prompt).
pub fn fold_project_trust(handled: &[(ExtensionId, HandledValue)]) -> Option<ProjectTrustDecision> {
    for (id, HandledValue(v)) in handled {
        if let Some(trusted) = parse_trust_decision(v) {
            let remember = v.get("remember").and_then(|r| r.as_bool()).unwrap_or(false);
            return Some(ProjectTrustDecision {
                trusted,
                remember,
                by: id.clone(),
            });
        }
    }
    None
}

/// One discovered resource path attributed back to the contributing extension (Pi
/// `{path, extensionPath}`, runner.ts:1064). `extension` is the cyrup `ExtensionId` (the analog of
/// Pi's `ext.path`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttributedPath {
    pub path: String,
    pub extension: ExtensionId,
}

/// The aggregated resources every extension provides (Pi `resources_discover`, runner.ts:1046-1092).
/// Each list is the CONCATENATION across extensions in load order — NO de-duplication — with every
/// path attributed to its contributing extension (gap-08 #15).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResourcesAggregate {
    pub skill_paths: Vec<AttributedPath>,
    pub prompt_paths: Vec<AttributedPath>,
    pub theme_paths: Vec<AttributedPath>,
}

impl ResourcesAggregate {
    /// Just the skill path strings, in concatenated order (convenience for callers that don't need
    /// attribution).
    pub fn skill_path_strs(&self) -> Vec<String> {
        self.skill_paths.iter().map(|p| p.path.clone()).collect()
    }
    pub fn prompt_path_strs(&self) -> Vec<String> {
        self.prompt_paths.iter().map(|p| p.path.clone()).collect()
    }
    pub fn theme_path_strs(&self) -> Vec<String> {
        self.theme_paths.iter().map(|p| p.path.clone()).collect()
    }
}

/// Fold the collected `resources_discover` handled values into the typed [`ResourcesAggregate`].
/// Concatenates (no dedup) and attributes each path to its extension (Pi runner.ts:1064-1072).
pub fn fold_resources(handled: &[(ExtensionId, HandledValue)]) -> ResourcesAggregate {
    let mut agg = ResourcesAggregate::default();
    for (id, HandledValue(v)) in handled {
        append_attributed(&mut agg.skill_paths, v.get("skillPaths"), id);
        append_attributed(&mut agg.prompt_paths, v.get("promptPaths"), id);
        append_attributed(&mut agg.theme_paths, v.get("themePaths"), id);
    }
    agg
}

/// Append the string entries of a JSON array `field` into `dst`, each attributed to `id`. A
/// non-array / absent field contributes nothing.
fn append_attributed(
    dst: &mut Vec<AttributedPath>,
    field: Option<&serde_json::Value>,
    id: &ExtensionId,
) {
    if let Some(arr) = field.and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(s) = item.as_str() {
                dst.push(AttributedPath {
                    path: s.to_string(),
                    extension: id.clone(),
                });
            }
        }
    }
}

/// pi `CacheWarmingAction` (`core/cache-warmer.ts:88` @v1.0.4): the only two values that exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheWarmingAction {
    Warm,
    Stop,
}

impl CacheWarmingAction {
    /// The wire spelling pi uses on both legs of the hook.
    pub fn as_str(self) -> &'static str {
        match self {
            CacheWarmingAction::Warm => "warm",
            CacheWarmingAction::Stop => "stop",
        }
    }
}

/// Parse a `cache_warming_decision` handled value's `{action}` (pi
/// `CacheWarmingDecisionEventResult`, `core/cache-warmer.ts:117-120` @v1.0.4). `None` = this
/// handler expressed NO opinion, which is upstream's `result?.action !== undefined` guard
/// (`runner.ts:1131`) and leaves the action as it stood.
///
/// An action that is present but is neither `"warm"` nor `"stop"` is treated as no opinion rather
/// than forwarded. **This is a DELIBERATE divergence, and the direction matters.** Upstream's
/// untyped `action = result.action` (`runner.ts:1131`) propagates a third string straight through,
/// and its only consumer tests `if (action === "stop")` (`cache-warmer.ts:320`) — so an
/// unrecognised action reads as **warm**: pi SENDS the refresh and, because
/// `extensionOverride = action !== decision.action` (`:319`) is true, persists it as
/// `Cache warmed (extension override): $…`. In other words upstream spends money under an action
/// name its own union does not have. cyrup refuses instead: the fold falls back to the previous
/// readable opinion, or to the host's own economics verdict, so "last override wins" is strictly
/// "last PARSEABLE override wins". That costs strict parity on an input no correct extension
/// produces and buys back the case where a sloppy guest cannot make the host pay.
///
/// Do not "fix" this back toward parity on the strength of the old comment here, which claimed the
/// opposite — that a third string read as `"stop"` upstream. It does not; it reads as a warm.
/// (Both sides re-read at v1.0.4 and at this HEAD.) If strict parity is ever wanted, the change is
/// to map an unrecognised-but-present action to [`CacheWarmingAction::Warm`] with
/// `extension_override = true`, not to drop the guard.
pub fn parse_cache_warming_action(v: &serde_json::Value) -> Option<CacheWarmingAction> {
    match v.get("action")?.as_str()? {
        "warm" => Some(CacheWarmingAction::Warm),
        "stop" => Some(CacheWarmingAction::Stop),
        _ => None,
    }
}

/// Fold the collected `cache_warming_decision` handled values into the final action override: the
/// LAST handler that returned a readable `{action}` wins (pi `emitCacheWarmingDecision`,
/// `core/extensions/runner.ts:1121-1142` @v1.0.4 — `if (result?.action !== undefined) action =
/// result.action`, assigned inside the loop and never returned early). `None` = nobody overrode,
/// so the caller keeps the host's own `event.action`.
///
/// **This is the OPPOSITE rule to [`fold_project_trust`] next door, and the asymmetry is
/// upstream's.** `project_trust` returns the instant a handler decides, so a later handler must
/// not run at all; `cache_warming_decision` runs every handler of every extension for its side
/// effects (a telemetry handler that also wants to observe the economics) and only then takes the
/// last opinion. Dispatch it with
/// [`crate::dispatch::Dispatcher::dispatch_collect_handled`], never `dispatch_first_handled` — a
/// reader who assumes symmetry with this file's other fold gets first-wins and a silently
/// skipped handler.
pub fn fold_cache_warming_decision(
    handled: &[(ExtensionId, HandledValue)],
) -> Option<(ExtensionId, CacheWarmingAction)> {
    handled
        .iter()
        .rev()
        .find_map(|(id, HandledValue(v))| parse_cache_warming_action(v).map(|a| (id.clone(), a)))
}
