//! SUBA-021 (first half) — the subagent CAPABILITY CEILING: the Rust port of
//! `pi-subagents/src/runs/shared/capability-ceiling.ts` (in-baseline — `git cat-file -e` succeeds at
//! both v0.43.0 and v0.47.1, which is what killed this item's original "post-baseline, out of
//! scope" framing).
//!
//! # What a ceiling is, and why it is not the tool allowlist
//!
//! A ceiling is a MONOTONICALLY TIGHTENING upper bound on what a subagent subtree may do:
//!
//! * `allowedTools` — the child may use no tool outside this set;
//! * `allowedAgents` — the child may delegate to no agent outside this set;
//! * `denyExtensions` — the child gets no extensions at all.
//!
//! It is not the per-agent `tools:` allowlist, which is a *request* the agent makes about itself
//! and which a child can widen for its own children. A ceiling can only ever narrow:
//! [`intersect_capability_ceilings`] intersects the lists and ORs `denyExtensions`, so
//! composing an inherited ceiling with a locally registered one can never produce a wider result
//! than either input. That is the whole security property, and without it — the state cyrup was in
//! — a child could be granted a capability set wider than its parent's simply by asking.
//!
//! # Where it lives
//!
//! Two places, exactly as upstream:
//!
//! 1. **A process-local registry** keyed by session id (`registry()`, `capability-ceiling.ts:48-56`,
//!    upstream a `globalThis` `Symbol.for` map so two copies of the module share one store). A host
//!    registers a ceiling for a session, gets a [`CapabilityCeilingHandle`], and the ceiling applies
//!    to every run launched under that session until the handle is dropped.
//! 2. **One env var across the process boundary** ([`CAPABILITY_CEILING_ENV`], base64url JSON,
//!    `:192-209`). A child re-reads it as its INHERITED ceiling and intersects it with anything
//!    registered locally, so the bound survives the re-exec that is this crate's whole mechanism.
//!    (PARITY-GAPS VL-S1 tracks the same var on the env-surface census.)
//!
//! # Rust deltas
//!
//! * `[CYRUP-DELTA]` — upstream's registration token is a JS `Symbol(source)`; symbols do not exist
//!   in Rust, so [`CapabilityCeilingHandle`] carries a process-monotonic `u64` token from
//!   [`NEXT_TOKEN`]. Identity semantics are the same (two registrations from the same `source`
//!   remain distinct entries).
//! * `[CYRUP-DELTA]` — upstream's `dispose()` is explicit and leaks the entry if the caller forgets.
//!   Here [`CapabilityCeilingHandle`] also disposes on `Drop`, because a Rust future can be dropped
//!   at any `.await` and an un-disposed entry is a ceiling that silently keeps applying to a session
//!   whose owner is gone. `dispose()` stays public and idempotent, so the explicit upstream call
//!   still compiles and still means the same thing.
//! * Every `throw new Error(...)` is an `Err(String)` carrying upstream's byte-identical text.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// pi `SUBAGENT_CAPABILITY_CEILING_VERSION` (`capability-ceiling.ts:3`).
pub const CAPABILITY_CEILING_VERSION: u32 = 1;

/// pi `SUBAGENT_CAPABILITY_CEILING_ENV = "PI_SUBAGENT_CAPABILITY_CEILING_V1"`
/// (`capability-ceiling.ts:5`), in this crate's `CYRUP_` naming family — the same rename convention
/// `exec/spawn_budget.rs`'s [`crate::exec::spawn_budget::MAX_SPAWNS_PER_SESSION_ENV`] documents.
pub const CAPABILITY_CEILING_ENV: &str = "CYRUP_SUBAGENT_CAPABILITY_CEILING_V1";

/// The upstream spelling of [`CAPABILITY_CEILING_ENV`], honoured as a read-side compatibility alias
/// so a pi user's existing environment keeps bounding cyrup children.
pub const CAPABILITY_CEILING_ENV_PI_ALIAS: &str = "PI_SUBAGENT_CAPABILITY_CEILING_V1";

/// pi's `values.length > 256` cap (`capability-ceiling.ts:76`).
const MAX_LIST_ENTRIES: usize = 256;
/// pi's `Buffer.byteLength(value.trim()) > 256` cap in `validateText` (`:59`).
const MAX_TEXT_BYTES: usize = 256;
/// pi's `Buffer.byteLength(name) > 128` per-entry cap (`:80`).
const MAX_ENTRY_BYTES: usize = 128;

/// pi `ResolvedSubagentCapabilityCeiling` (`capability-ceiling.ts:12-18`).
///
/// `allowed_tools`/`allowed_agents` are `Option` because upstream distinguishes ABSENT (no bound on
/// that axis) from an empty array (bound to nothing) — collapsing the two would silently turn
/// "unconstrained" into "denied", or the reverse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedCapabilityCeiling {
    /// pi `version` — always [`CAPABILITY_CEILING_VERSION`]; a decode of anything else is refused.
    pub version: u32,
    /// pi `allowedTools?` — sorted, de-duplicated. `None` = no tool bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    /// pi `allowedAgents?` — sorted, de-duplicated. `None` = no agent bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_agents: Option<Vec<String>>,
    /// pi `denyExtensions` — never optional on the RESOLVED shape (`ceiling.denyExtensions === true`
    /// at `:90` normalizes an absent value to `false`).
    pub deny_extensions: bool,
    /// pi `sources` — who imposed this ceiling, sorted and de-duplicated. Named in the refusal text
    /// so an operator can tell WHICH policy blocked a delegation.
    pub sources: Vec<String>,
}

/// pi `validateText` (`capability-ceiling.ts:58-63`): non-empty after trim, no C0/DEL control
/// characters, at most 256 UTF-8 bytes. Returns the TRIMMED value.
///
/// # Errors
///
/// Upstream's single verbatim sentence, with `field` interpolated.
fn validate_text(value: Option<&serde_json::Value>, field: &str) -> Result<String, String> {
    let invalid = || {
        format!(
            "Invalid capability ceiling {field}; expected a non-empty string without control \
             characters (max 256 UTF-8 bytes)."
        )
    };
    let Some(text) = value.and_then(serde_json::Value::as_str) else {
        return Err(invalid());
    };
    // pi tests the RAW value for control characters and the TRIMMED one for length.
    if text
        .chars()
        .any(|c| c.is_control() && (c as u32) < 0x20 || c as u32 == 0x7f)
    {
        return Err(invalid());
    }
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_TEXT_BYTES {
        return Err(invalid());
    }
    Ok(trimmed.to_string())
}

/// pi's `/^[A-Za-z0-9_.:-]+$/u` entry pattern (`capability-ceiling.ts:84-85`).
fn is_valid_entry_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-'))
}

fn normalize_list(
    ceiling: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<Option<Vec<String>>, String> {
    let Some(values) = ceiling.get(field) else {
        return Ok(None);
    };
    let Some(values) = values.as_array() else {
        return Err(format!(
            "Invalid capability ceiling {field}; expected an array."
        ));
    };
    if values.len() > MAX_LIST_ENTRIES {
        return Err(format!(
            "Invalid capability ceiling {field}; expected at most 256 names."
        ));
    }
    let mut seen = BTreeSet::new();
    for entry in values {
        let name = validate_text(Some(entry), &format!("{field} entry"))?;
        if !is_valid_entry_name(&name) {
            return Err(format!(
                "Invalid capability ceiling {field} entry '{name}'."
            ));
        }
        if name.len() > MAX_ENTRY_BYTES {
            return Err(format!(
                "Invalid capability ceiling {field} entry '{name}'; max 128 UTF-8 bytes."
            ));
        }
        seen.insert(name);
    }
    // pi `[...new Set(...)].sort()` — a `BTreeSet` is both in one step.
    Ok(Some(seen.into_iter().collect()))
}

/// pi `normalizeCeiling` (`capability-ceiling.ts:65-93`).
///
/// # Errors
///
/// Upstream's four structural refusals plus everything [`normalize_list`] raises.
pub fn normalize_ceiling(value: &serde_json::Value) -> Result<ResolvedCapabilityCeiling, String> {
    let Some(ceiling) = value.as_object() else {
        return Err("Invalid capability ceiling; expected an object.".to_string());
    };
    let has_allowed_tools = ceiling.contains_key("allowedTools");
    let has_allowed_agents = ceiling.contains_key("allowedAgents");
    let has_deny_extensions = ceiling.contains_key("denyExtensions");
    if !has_allowed_tools && !has_allowed_agents && !has_deny_extensions {
        return Err(
            "Invalid capability ceiling; expected allowedTools, allowedAgents, or denyExtensions."
                .to_string(),
        );
    }
    if has_deny_extensions
        && !ceiling
            .get("denyExtensions")
            .is_some_and(serde_json::Value::is_boolean)
    {
        return Err("Invalid capability ceiling denyExtensions; expected a boolean.".to_string());
    }
    Ok(ResolvedCapabilityCeiling {
        version: CAPABILITY_CEILING_VERSION,
        allowed_tools: normalize_list(ceiling, "allowedTools")?,
        allowed_agents: normalize_list(ceiling, "allowedAgents")?,
        // pi `ceiling.denyExtensions === true` — only the literal `true`.
        deny_extensions: ceiling.get("denyExtensions") == Some(&serde_json::Value::Bool(true)),
        sources: Vec::new(),
    })
}

/// pi `parseSubagentCapabilityCeiling` (`capability-ceiling.ts:95-104`) — normalize a value that
/// already claims to be RESOLVED, so it must carry both a `version` and a `sources` array.
///
/// # Errors
///
/// Upstream's three refusals plus everything [`normalize_ceiling`] raises.
pub fn parse_capability_ceiling(
    value: &serde_json::Value,
    field: &str,
) -> Result<ResolvedCapabilityCeiling, String> {
    let Some(record) = value.as_object() else {
        return Err(format!("Invalid {field}; expected an object."));
    };
    if record.get("version").and_then(serde_json::Value::as_u64)
        != Some(u64::from(CAPABILITY_CEILING_VERSION))
    {
        return Err(format!("Invalid {field} version."));
    }
    let mut normalized = normalize_ceiling(value)?;
    let Some(sources) = record.get("sources").and_then(serde_json::Value::as_array) else {
        return Err(format!(
            "Invalid {field} sources; expected an array of strings."
        ));
    };
    if sources.iter().any(|s| !s.is_string()) {
        return Err(format!(
            "Invalid {field} sources; expected an array of strings."
        ));
    }
    let mut seen = BTreeSet::new();
    for source in sources {
        seen.insert(validate_text(Some(source), &format!("{field} source"))?);
    }
    normalized.sources = seen.into_iter().collect();
    Ok(normalized)
}

// -------------------------------------------------------------------------------------------
// The per-session registry (pi `registry()`, `capability-ceiling.ts:45-56`)
// -------------------------------------------------------------------------------------------

/// pi's `Registration = { source, ceiling }` (`capability-ceiling.ts:45`) collapses to just the
/// ceiling here: upstream sets `normalized.sources = [source]` on BOTH the register and the update
/// path (`:119`, `:128`), so `registration.source` is `ceiling.sources[0]` by construction and
/// storing it twice is two things that can disagree.
type Registration = ResolvedCapabilityCeiling;

type Registry = HashMap<String, HashMap<u64, Registration>>;

/// `[CYRUP-DELTA]` — upstream keys registrations by a JS `Symbol(source)`. Rust has no symbols, so
/// each registration takes a process-monotonic token with the same identity semantics.
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(0);

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// pi `SubagentCapabilityCeilingHandle` (`capability-ceiling.ts:40-43`).
///
/// Dropping it removes the registration (see the module doc's second `CYRUP-DELTA`).
#[derive(Debug)]
pub struct CapabilityCeilingHandle {
    session_id: String,
    source: String,
    token: u64,
    disposed: bool,
}

impl CapabilityCeilingHandle {
    /// pi `handle.update(ceiling)` (`capability-ceiling.ts:125-130`) — replace this registration's
    /// ceiling in place, re-validating it.
    ///
    /// # Errors
    ///
    /// `Cannot update a disposed capability ceiling handle.` verbatim, plus anything
    /// [`normalize_ceiling`] raises.
    pub fn update(&mut self, ceiling: &serde_json::Value) -> Result<(), String> {
        if self.disposed {
            return Err("Cannot update a disposed capability ceiling handle.".to_string());
        }
        let mut normalized = normalize_ceiling(ceiling)?;
        normalized.sources = vec![self.source.clone()];
        if let Ok(mut store) = registry().lock()
            && let Some(session) = store.get_mut(&self.session_id)
        {
            session.insert(self.token, normalized);
        }
        Ok(())
    }

    /// pi `handle.dispose()` (`capability-ceiling.ts:131-136`) — idempotent, and it drops the whole
    /// session entry once its last registration is gone.
    pub fn dispose(&mut self) {
        if self.disposed {
            return;
        }
        self.disposed = true;
        if let Ok(mut store) = registry().lock() {
            let empty = if let Some(session) = store.get_mut(&self.session_id) {
                session.remove(&self.token);
                session.is_empty()
            } else {
                false
            };
            if empty {
                store.remove(&self.session_id);
            }
        }
    }

    /// The `source` this registration was made under — named in
    /// [`capability_ceiling_agent_restriction_message`]'s refusal text.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }
}

impl Drop for CapabilityCeilingHandle {
    fn drop(&mut self) {
        self.dispose();
    }
}

/// pi `registerSubagentCapabilityCeiling` (`capability-ceiling.ts:106-138`).
///
/// # Errors
///
/// The `sessionId`/`source` text validations and everything [`normalize_ceiling`] raises.
pub fn register_capability_ceiling(
    session_id: &str,
    source: &str,
    ceiling: &serde_json::Value,
) -> Result<CapabilityCeilingHandle, String> {
    let session_id = validate_text(
        Some(&serde_json::Value::String(session_id.to_string())),
        "sessionId",
    )?;
    let source = validate_text(
        Some(&serde_json::Value::String(source.to_string())),
        "source",
    )?;
    let mut normalized = normalize_ceiling(ceiling)?;
    // pi `normalized.sources = [source]` — a registration always attributes itself, overwriting any
    // `sources` the caller supplied.
    normalized.sources = vec![source.clone()];
    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut store) = registry().lock() {
        store
            .entry(session_id.clone())
            .or_default()
            .insert(token, normalized);
    }
    Ok(CapabilityCeilingHandle {
        session_id,
        source,
        token,
        disposed: false,
    })
}

/// pi `intersectSubagentCapabilityCeilings` (`capability-ceiling.ts:140-157`) — the TIGHTENING
/// compose. `None` inputs are skipped; an all-`None` input yields `None` (unbounded).
///
/// A list axis is bounded only if at least ONE input bounds it, and then the result is the
/// intersection of every input that bounds it — so adding a ceiling can never widen the result.
/// `denyExtensions` is an OR for the same reason.
#[must_use]
pub fn intersect_capability_ceilings(
    ceilings: &[Option<ResolvedCapabilityCeiling>],
) -> Option<ResolvedCapabilityCeiling> {
    let active: Vec<&ResolvedCapabilityCeiling> = ceilings.iter().flatten().collect();
    if active.is_empty() {
        return None;
    }
    let intersect = |pick: fn(&ResolvedCapabilityCeiling) -> Option<&Vec<String>>| {
        let defined: Vec<&Vec<String>> = active.iter().filter_map(|c| pick(c)).collect();
        let first = defined.first()?;
        let mut kept: Vec<String> = first
            .iter()
            .filter(|entry| defined.iter().all(|list| list.contains(entry)))
            .cloned()
            .collect();
        kept.sort();
        kept.dedup();
        Some(kept)
    };
    let mut sources: Vec<String> = active
        .iter()
        .flat_map(|c| c.sources.iter().cloned())
        .collect();
    sources.sort();
    sources.dedup();
    Some(ResolvedCapabilityCeiling {
        version: CAPABILITY_CEILING_VERSION,
        allowed_tools: intersect(|c| c.allowed_tools.as_ref()),
        allowed_agents: intersect(|c| c.allowed_agents.as_ref()),
        deny_extensions: active.iter().any(|c| c.deny_extensions),
        sources,
    })
}

/// pi `resolveSubagentCapabilityCeiling` (`capability-ceiling.ts:159-166`) — the INHERITED ceiling
/// intersected with everything registered for this session.
#[must_use]
pub fn resolve_capability_ceiling(
    session_id: Option<&str>,
    inherited: Option<ResolvedCapabilityCeiling>,
) -> Option<ResolvedCapabilityCeiling> {
    let mut all = vec![inherited];
    if let Some(session_id) = session_id
        && let Ok(store) = registry().lock()
        && let Some(session) = store.get(session_id)
    {
        all.extend(session.values().map(|ceiling| Some(ceiling.clone())));
    }
    intersect_capability_ceilings(&all)
}

/// pi `resolveCurrentSubagentCapabilityCeiling` (`capability-ceiling.ts:168-170`) — the same, with
/// the inherited half read from [`CAPABILITY_CEILING_ENV`].
///
/// # Errors
///
/// Whatever [`decode_capability_ceiling`] raises for a malformed inherited value; a MALFORMED
/// ceiling must fail loudly rather than degrade to "unbounded", which would invert the guarantee.
pub fn resolve_current_capability_ceiling(
    session_id: Option<&str>,
) -> Result<Option<ResolvedCapabilityCeiling>, String> {
    let raw = std::env::var(CAPABILITY_CEILING_ENV)
        .or_else(|_| std::env::var(CAPABILITY_CEILING_ENV_PI_ALIAS))
        .ok();
    let inherited = decode_capability_ceiling(raw.as_deref())?;
    Ok(resolve_capability_ceiling(session_id, inherited))
}

/// pi `isAgentAllowedByCapabilityCeiling` (`capability-ceiling.ts:172-174`).
#[must_use]
pub fn is_agent_allowed(agent_name: &str, ceiling: Option<&ResolvedCapabilityCeiling>) -> bool {
    match ceiling.and_then(|c| c.allowed_agents.as_ref()) {
        None => true,
        Some(allowed) => allowed.iter().any(|a| a == agent_name),
    }
}

/// pi `capabilityCeilingAgentRestrictionMessage` (`capability-ceiling.ts:176-181`) — the refusal
/// text, verbatim, including both `(none)`/`unknown source` fallbacks.
#[must_use]
pub fn capability_ceiling_agent_restriction_message(
    agent_name: &str,
    ceiling: Option<&ResolvedCapabilityCeiling>,
) -> Option<String> {
    if is_agent_allowed(agent_name, ceiling) {
        return None;
    }
    let sources = ceiling
        .filter(|c| !c.sources.is_empty())
        .map_or_else(|| "unknown source".to_string(), |c| c.sources.join(", "));
    let allowed = ceiling
        .and_then(|c| c.allowed_agents.as_ref())
        .filter(|a| !a.is_empty())
        .map_or_else(|| "(none)".to_string(), |a| a.join(", "));
    Some(format!(
        "Capability ceiling from {sources} does not allow agent '{agent_name}'. Allowed agents: \
         {allowed}."
    ))
}

/// pi `assertAgentAllowedByCapabilityCeiling` (`capability-ceiling.ts:183-186`).
///
/// # Errors
///
/// [`capability_ceiling_agent_restriction_message`]'s text when the agent is outside the ceiling.
pub fn assert_agent_allowed(
    agent_name: &str,
    ceiling: Option<&ResolvedCapabilityCeiling>,
) -> Result<(), String> {
    match capability_ceiling_agent_restriction_message(agent_name, ceiling) {
        Some(message) => Err(message),
        None => Ok(()),
    }
}

/// pi `capabilityCeilingAgentRestrictionSources` (`capability-ceiling.ts:188-190`) — `None` when the
/// agent axis is unbounded, so a caller can tell "no restriction" from "restricted by nobody named".
#[must_use]
pub fn capability_ceiling_agent_restriction_sources(
    ceiling: Option<&ResolvedCapabilityCeiling>,
) -> Option<Vec<String>> {
    let ceiling = ceiling?;
    ceiling.allowed_agents.as_ref()?;
    Some(ceiling.sources.clone())
}

/// pi `encodeSubagentCapabilityCeiling` (`capability-ceiling.ts:192-195`) — base64url JSON, the
/// form that crosses the spawn boundary in [`CAPABILITY_CEILING_ENV`].
#[must_use]
pub fn encode_capability_ceiling(ceiling: Option<&ResolvedCapabilityCeiling>) -> Option<String> {
    use base64::Engine as _;
    let ceiling = ceiling?;
    let json = serde_json::to_vec(ceiling).ok()?;
    Some(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json))
}

/// pi `decodeSubagentCapabilityCeiling` (`capability-ceiling.ts:197-209`).
///
/// # Errors
///
/// Upstream's two verbatim refusals — a value that is not decodable JSON, and one whose `version` is
/// not [`CAPABILITY_CEILING_VERSION`] — plus everything [`parse_capability_ceiling`] raises.
pub fn decode_capability_ceiling(
    value: Option<&str>,
) -> Result<Option<ResolvedCapabilityCeiling>, String> {
    use base64::Engine as _;
    let Some(value) = value.filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let bytes = engine
        .decode(value)
        // Node's `Buffer.from(v, "base64url")` accepts a padded value too.
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(value))
        .map_err(|error| format!("Invalid inherited capability ceiling: {error}"))?;
    let parsed: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Invalid inherited capability ceiling: {error}"))?;
    if parsed.get("version").and_then(serde_json::Value::as_u64)
        != Some(u64::from(CAPABILITY_CEILING_VERSION))
        || !parsed.is_object()
    {
        return Err("Invalid inherited capability ceiling version.".to_string());
    }
    parse_capability_ceiling(&parsed, "inherited capability ceiling").map(Some)
}

#[cfg(test)]
fn ceiling_for_test_tools_only() -> ResolvedCapabilityCeiling {
    ResolvedCapabilityCeiling {
        version: CAPABILITY_CEILING_VERSION,
        allowed_tools: Some(vec!["read".to_string()]),
        allowed_agents: None,
        deny_extensions: false,
        sources: vec!["t".to_string()],
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn ceiling(json: serde_json::Value) -> ResolvedCapabilityCeiling {
        normalize_ceiling(&json).expect("normalizes")
    }

    /// THE security property SUBA-021 exists for: composing ceilings can only ever NARROW. Before
    /// this module there was no ceiling concept at all, so a child could be granted a capability set
    /// wider than its parent's. pi `intersectSubagentCapabilityCeilings` (`:140-157`).
    #[test]
    fn intersecting_ceilings_can_only_narrow_never_widen() {
        let parent = ceiling(serde_json::json!({ "allowedTools": ["bash", "read", "write"] }));
        let child = ceiling(serde_json::json!({ "allowedTools": ["read", "write", "edit"] }));
        let composed = intersect_capability_ceilings(&[Some(parent), Some(child)])
            .expect("two ceilings compose");
        assert_eq!(
            composed.allowed_tools,
            Some(vec!["read".to_string(), "write".to_string()]),
            "`edit` was NOT in the parent's set, so asking for it cannot grant it"
        );

        // An axis nobody bounds stays unbounded; `denyExtensions` is an OR, so one `true` wins.
        let composed = intersect_capability_ceilings(&[
            Some(ceiling(serde_json::json!({ "allowedTools": ["read"] }))),
            Some(ceiling(serde_json::json!({ "denyExtensions": true }))),
        ])
        .expect("composes");
        assert_eq!(composed.allowed_agents, None);
        assert!(composed.deny_extensions);
        // An empty list is BOUND TO NOTHING, not unbounded — the distinction `Option` preserves.
        let none_allowed = ceiling(serde_json::json!({ "allowedTools": [] }));
        assert_eq!(none_allowed.allowed_tools, Some(Vec::new()));
        assert_eq!(intersect_capability_ceilings(&[None, None]), None);
    }

    /// pi `assertAgentAllowedByCapabilityCeiling` / `capabilityCeilingAgentRestrictionMessage`
    /// (`:176-186`) — the refusal names the imposing sources and the allowed set, verbatim.
    #[test]
    fn an_agent_outside_the_ceiling_is_refused_with_pis_verbatim_text() {
        let mut ceiling = ceiling(serde_json::json!({ "allowedAgents": ["reviewer"] }));
        ceiling.sources = vec!["org-policy".to_string(), "repo-policy".to_string()];

        assert!(is_agent_allowed("reviewer", Some(&ceiling)));
        assert!(assert_agent_allowed("reviewer", Some(&ceiling)).is_ok());
        assert_eq!(
            assert_agent_allowed("writer", Some(&ceiling)).expect_err("refused"),
            "Capability ceiling from org-policy, repo-policy does not allow agent 'writer'. \
             Allowed agents: reviewer."
        );

        // No ceiling at all, and a ceiling that bounds only tools, both allow every agent.
        assert!(is_agent_allowed("writer", None));
        assert!(is_agent_allowed(
            "writer",
            Some(&ceiling_for_test_tools_only())
        ));
        assert_eq!(capability_ceiling_agent_restriction_sources(None), None);

        // Upstream's two fallbacks: no sources → "unknown source", empty list → "(none)".
        let anonymous = ResolvedCapabilityCeiling {
            version: CAPABILITY_CEILING_VERSION,
            allowed_tools: None,
            allowed_agents: Some(Vec::new()),
            deny_extensions: false,
            sources: Vec::new(),
        };
        assert_eq!(
            assert_agent_allowed("writer", Some(&anonymous)).expect_err("refused"),
            "Capability ceiling from unknown source does not allow agent 'writer'. Allowed \
             agents: (none)."
        );
    }

    /// pi `normalizeCeiling`'s refusals (`:65-93`), byte-for-byte — a ceiling is a security bound,
    /// so a malformed one must be rejected rather than silently narrowed or widened.
    #[test]
    fn a_malformed_ceiling_is_refused_with_pis_verbatim_texts() {
        for (input, expected) in [
            (
                serde_json::json!([]),
                "Invalid capability ceiling; expected an object.",
            ),
            (
                serde_json::json!({}),
                "Invalid capability ceiling; expected allowedTools, allowedAgents, or \
                 denyExtensions.",
            ),
            (
                serde_json::json!({ "denyExtensions": "yes" }),
                "Invalid capability ceiling denyExtensions; expected a boolean.",
            ),
            (
                serde_json::json!({ "allowedTools": "bash" }),
                "Invalid capability ceiling allowedTools; expected an array.",
            ),
            (
                serde_json::json!({ "allowedAgents": ["has space"] }),
                "Invalid capability ceiling allowedAgents entry 'has space'.",
            ),
            (
                serde_json::json!({ "allowedTools": [""] }),
                "Invalid capability ceiling allowedTools entry; expected a non-empty string \
                 without control characters (max 256 UTF-8 bytes).",
            ),
        ] {
            assert_eq!(
                normalize_ceiling(&input).expect_err("refused"),
                expected,
                "for {input}"
            );
        }
        assert_eq!(
            normalize_ceiling(&serde_json::json!({
                "allowedTools": (0..257).map(|n| format!("t{n}")).collect::<Vec<_>>()
            }))
            .expect_err("refused"),
            "Invalid capability ceiling allowedTools; expected at most 256 names."
        );
    }

    /// The env round-trip that makes the bound survive this crate's re-exec mechanism.
    /// pi `encodeSubagentCapabilityCeiling`/`decodeSubagentCapabilityCeiling` (`:192-209`).
    #[test]
    fn a_ceiling_round_trips_through_the_env_var_and_a_bad_one_is_refused() {
        let mut original = ceiling(serde_json::json!({
            "allowedTools": ["read", "bash"],
            "allowedAgents": ["reviewer"],
            "denyExtensions": true
        }));
        original.sources = vec!["org-policy".to_string()];

        let encoded = encode_capability_ceiling(Some(&original)).expect("encodes");
        assert!(
            !encoded.contains('+') && !encoded.contains('/') && !encoded.contains('='),
            "base64URL, unpadded: {encoded}"
        );
        assert_eq!(
            decode_capability_ceiling(Some(&encoded)).expect("decodes"),
            Some(original)
        );

        assert_eq!(decode_capability_ceiling(None).expect("absent"), None);
        assert_eq!(decode_capability_ceiling(Some("")).expect("empty"), None);
        assert!(
            decode_capability_ceiling(Some("!!!not base64!!!"))
                .expect_err("refused")
                .starts_with("Invalid inherited capability ceiling: ")
        );

        use base64::Engine as _;
        let wrong_version = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"version":2,"denyExtensions":true,"sources":[]}"#);
        assert_eq!(
            decode_capability_ceiling(Some(&wrong_version)).expect_err("refused"),
            "Invalid inherited capability ceiling version."
        );
    }

    /// pi's per-session registry (`:106-166`), plus the `Drop`-disposal `CYRUP-DELTA`: a handle that
    /// goes out of scope at an `.await` must not leave a ceiling applying to the session forever.
    #[test]
    fn a_registered_ceiling_applies_to_its_session_and_is_gone_once_the_handle_drops() {
        let session = format!("ceiling-session-{}", NEXT_TOKEN.load(Ordering::Relaxed));
        assert_eq!(resolve_capability_ceiling(Some(&session), None), None);

        {
            let handle = register_capability_ceiling(
                &session,
                "org-policy",
                &serde_json::json!({ "allowedAgents": ["reviewer"] }),
            )
            .expect("registers");
            assert_eq!(handle.source(), "org-policy");

            let resolved =
                resolve_capability_ceiling(Some(&session), None).expect("the ceiling applies");
            assert_eq!(resolved.allowed_agents, Some(vec!["reviewer".to_string()]));
            assert_eq!(resolved.sources, vec!["org-policy".to_string()]);
            assert_eq!(
                assert_agent_allowed("writer", Some(&resolved)).expect_err("refused"),
                "Capability ceiling from org-policy does not allow agent 'writer'. Allowed \
                 agents: reviewer."
            );
            // A DIFFERENT session is untouched.
            assert_eq!(
                resolve_capability_ceiling(Some("other-session"), None),
                None
            );
        }

        assert_eq!(
            resolve_capability_ceiling(Some(&session), None),
            None,
            "dropping the handle disposed the registration"
        );
    }

    /// pi `handle.update` (`:125-130`) and its disposed-handle refusal.
    #[test]
    fn updating_a_handle_replaces_its_ceiling_and_a_disposed_one_refuses() {
        let session = format!("ceiling-update-{}", NEXT_TOKEN.load(Ordering::Relaxed));
        let mut handle = register_capability_ceiling(
            &session,
            "org-policy",
            &serde_json::json!({ "allowedAgents": ["reviewer"] }),
        )
        .expect("registers");

        handle
            .update(&serde_json::json!({ "allowedAgents": ["reviewer", "planner"] }))
            .expect("updates");
        assert_eq!(
            resolve_capability_ceiling(Some(&session), None)
                .expect("still applies")
                .allowed_agents,
            Some(vec!["planner".to_string(), "reviewer".to_string()])
        );

        handle.dispose();
        handle.dispose(); // idempotent
        assert_eq!(
            handle
                .update(&serde_json::json!({ "allowedAgents": ["reviewer"] }))
                .expect_err("refused"),
            "Cannot update a disposed capability ceiling handle."
        );
    }

    /// The INHERITED half composes with the registered half, which is what makes a nested subtree
    /// tighten monotonically across the re-exec boundary. pi `resolveSubagentCapabilityCeiling`
    /// (`:159-166`).
    #[test]
    fn an_inherited_ceiling_intersects_with_the_locally_registered_one() {
        let session = format!("ceiling-inherit-{}", NEXT_TOKEN.load(Ordering::Relaxed));
        let _handle = register_capability_ceiling(
            &session,
            "repo-policy",
            &serde_json::json!({ "allowedAgents": ["reviewer", "planner"] }),
        )
        .expect("registers");

        let mut inherited = ceiling(serde_json::json!({ "allowedAgents": ["reviewer", "writer"] }));
        inherited.sources = vec!["org-policy".to_string()];

        let resolved =
            resolve_capability_ceiling(Some(&session), Some(inherited)).expect("composes");
        assert_eq!(
            resolved.allowed_agents,
            Some(vec!["reviewer".to_string()]),
            "only the agent BOTH levels allow survives"
        );
        assert_eq!(
            resolved.sources,
            vec!["org-policy".to_string(), "repo-policy".to_string()]
        );
    }

    /// pi `parseSubagentCapabilityCeiling`'s own three refusals (`:95-104`).
    #[test]
    fn parsing_a_resolved_ceiling_requires_a_version_and_a_sources_array() {
        assert_eq!(
            parse_capability_ceiling(&serde_json::json!("x"), "inherited capability ceiling")
                .expect_err("refused"),
            "Invalid inherited capability ceiling; expected an object."
        );
        assert_eq!(
            parse_capability_ceiling(
                &serde_json::json!({ "denyExtensions": true, "sources": [] }),
                "inherited capability ceiling"
            )
            .expect_err("refused"),
            "Invalid inherited capability ceiling version."
        );
        assert_eq!(
            parse_capability_ceiling(
                &serde_json::json!({ "version": 1, "denyExtensions": true }),
                "inherited capability ceiling"
            )
            .expect_err("refused"),
            "Invalid inherited capability ceiling sources; expected an array of strings."
        );
    }
}
