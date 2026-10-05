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
//!
//! # Reserved allow tokens resolve in TWO phases
//!
//! Upstream's `expandReservedPatterns` takes the parent model AND the parent session's scoped-model
//! snapshot together, because `resolveModelScopesForAgent` runs in-process at every launch with
//! `ctx.scopedModels` one property access away (`src/runs/shared/model-scope.ts:105-118`,
//! `model-resolution.ts:66-76` @v0.75.0). cyrup's launches cross a process boundary — a background
//! run resolves its models inside the detached hop-2 runner, which has no host at all — so the
//! snapshot has to be CAPTURED at launch rather than read at resolution. It is captured by
//! substituting it into the policy itself:
//!
//! 1. **shell**, once per launch, where the host is reachable:
//!    [`ModelScopeConfig::with_scoped_snapshot`] replaces every `scoped` token — in the global
//!    `allow` and in each `agents.<name>.allow` — with the ids
//!    [`scoped_model_ids_from_host`] read off [`cyrup_ext::host::HostServices::scoped_models`].
//!    A snapshot that is EMPTY substitutes nothing, which leaves the token for phase 2.
//! 2. **core**, at resolution: [`expand_reserved_patterns`] turns a surviving `inherit` — or a
//!    `scoped` that phase 1 found no snapshot for — into the parent session's `provider/id`, and
//!    leaves it literal when there is no parent model at all (upstream's fail-closed rule).
//!
//! The composition is upstream's function, case for case: a non-empty snapshot yields the
//! snapshot; an empty one degrades `scoped` to `inherit` semantics; neither leaves the token
//! unexpanded. What it additionally buys is that a background run enforces the snapshot its parent
//! held when the run was LAUNCHED, which is upstream's own rule for background runs, without a
//! second field on [`crate::exec::RunOptions`] and
//! [`crate::background::runner_main::RunnerConfig`] that could drift out of step with the parent
//! model beside it.

use crate::exec::split_known_thinking_suffix;

/// SUBA-155 — one `enforce`/`strict`/`allow` triple (pi `ModelScopeRule`,
/// `src/runs/shared/model-scope.ts:18-25` @v0.74.0). Upstream split this out of
/// `ModelScopeConfig` when it added per-agent restrictions, so the same three keys mean the same
/// thing at the top level and under `modelScope.agents.<name>`.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelScopeRule {
    /// When `Some(true)`, an out-of-scope model is rejected/warned per [`ModelSource`]. Under
    /// `agents.<name>` an absent value FALLS BACK to the global block's (pi
    /// `enforce: agentScope.enforce ?? config.enforce`, `model-scope.ts:174`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enforce: Option<bool>,
    /// Reject inherited/fallback models instead of warning. Same `?? config.strict` fallback under
    /// `agents.<name>` (`model-scope.ts:175`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    /// Glob-style allow patterns (only `*` is special), matched against the full `provider/id`,
    /// after [`expand_reserved_patterns`] has replaced any `inherit`/`scoped` token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow: Option<Vec<String>>,
}

/// The parsed `subagents.modelScope` settings block (pi `ModelScopeConfig`, `model-scope.ts:27-30`
/// @v0.74.0 — `ModelScopeConfig extends ModelScopeRule` with `agents`).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelScopeConfig {
    /// When `Some(true)`, an out-of-scope model is rejected/warned per [`ModelSource`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enforce: Option<bool>,
    /// SUBA-050 / pi `model-scope.ts:20` @v0.47.1 — *"Reject inherited and fallback models outside
    /// the allowlist instead of warning."* Landed upstream in `94b0cb1` ("feat: enforce strict
    /// subagent model scope", closes #995), released v0.47.0; `git show v0.43.0:.../model-scope.ts
    /// | grep strict` is empty, so this is drift rather than a stale port.
    ///
    /// Without it an operator's allowlist is advisory for exactly the sources that are hardest to
    /// audit: an agent whose frontmatter names an out-of-scope model, or whose fallback ladder walks
    /// onto one, warns and then runs on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    /// Glob-style allow patterns (only `*` is special), matched against the full `provider/id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow: Option<Vec<String>>,
    /// SUBA-155 — pi `agents?: Record<string, ModelScopeRule>`, *"Additional restrictions keyed by
    /// canonical agent name"* (`model-scope.ts:29` @v0.74.0).
    ///
    /// An ADDITIONAL, independent check: [`resolve_model_scopes_for_agent`] returns the global
    /// scope AND the matching agent scope, and a model must satisfy both. Before this existed
    /// serde accepted the key and dropped it, so a per-agent restriction an operator believed was
    /// in force was not — the failure direction that matters for a policy knob.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<std::collections::BTreeMap<String, ModelScopeRule>>,
}

impl ModelScopeConfig {
    /// SUBA-155 — phase 1 of the reserved-token resolution (see the module header): substitute the
    /// parent session's scoped-model snapshot for every `scoped` token, in the global `allow` and
    /// in every `agents.<name>.allow`.
    ///
    /// An EMPTY snapshot substitutes nothing and returns the policy unchanged, so the token
    /// survives into [`expand_reserved_patterns`] and degrades to `inherit` semantics there —
    /// upstream's `if (scopedModelIds?.length) … return parentModel ? … : [pattern]`
    /// (`model-scope.ts:113-116` @v0.75.0). That is also what keeps an unresolvable `scoped` naming
    /// ITSELF in the refusal rather than naming `inherit`.
    ///
    /// Applied at each launch (so a mid-session re-scope is picked up) and before a background
    /// run's policy is serialized (so that run enforces the snapshot its parent held at launch).
    #[must_use]
    pub fn with_scoped_snapshot(&self, scoped_model_ids: &[String]) -> Self {
        if scoped_model_ids.is_empty() {
            return self.clone();
        }
        let substitute = |allow: Option<&Vec<String>>| -> Option<Vec<String>> {
            allow.map(|patterns| {
                patterns
                    .iter()
                    .flat_map(|pattern| {
                        if pattern == SCOPED_PATTERN {
                            scoped_model_ids.to_vec()
                        } else {
                            vec![pattern.clone()]
                        }
                    })
                    .collect()
            })
        };
        Self {
            enforce: self.enforce,
            strict: self.strict,
            allow: substitute(self.allow.as_ref()),
            agents: self.agents.as_ref().map(|agents| {
                agents
                    .iter()
                    .map(|(name, rule)| {
                        (
                            name.clone(),
                            ModelScopeRule {
                                enforce: rule.enforce,
                                strict: rule.strict,
                                allow: substitute(rule.allow.as_ref()),
                            },
                        )
                    })
                    .collect()
            }),
        }
    }

    /// True iff enforcement is actually armed: `enforce: true` AND a non-empty `allow` list. A
    /// config that is enforcing with no patterns is a no-op (the settings parser rejects that
    /// combination, but this stays defensive for callers building configs programmatically — pi
    /// `checkModelScope`'s own `if (!allow || allow.length === 0) return undefined`).
    #[must_use]
    pub fn is_armed(&self) -> bool {
        self.enforce == Some(true) && self.allow.as_ref().is_some_and(|a| !a.is_empty())
    }
}

/// SUBA-035 — the one-line summary of the policy actually in force, for the surfaces an operator
/// consults when a model choice "did not apply".
///
/// The models report and `/subagents-doctor` are two different surfaces (upstream has the same
/// split: `checkModelScope` reports violations, the settings surface validates the config), so this
/// renders the compact form and the doctor keeps its own remedy-bearing one. What both must agree
/// on is the DECISION — armed vs. present-but-inert vs. absent — which is why this reads
/// [`ModelScopeConfig::is_armed`] rather than re-deriving the condition.
///
/// `strict` is named because it changes what an inherited/fallback violation DOES (SUBA-050): with
/// it, the model an operator did not choose explicitly is a hard error rather than a warning.
#[must_use]
pub fn model_scope_summary_line(scope: Option<&ModelScopeConfig>) -> String {
    let Some(scope) = scope else {
        return "  (none configured — every resolved model is in scope)".to_string();
    };
    let patterns = scope.allow.as_deref().unwrap_or(&[]);
    if !scope.is_armed() {
        return format!(
            "  (not enforcing — {} allow pattern(s) are inert)",
            patterns.len()
        );
    }
    format!(
        "  enforcing ({}): allow {}",
        if scope.strict == Some(true) {
            "strict"
        } else {
            "non-strict"
        },
        patterns.join(", ")
    )
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
    /// SUBA-155 — which block refused (pi `ModelScopeViolation.origin`, `model-scope.ts:47`):
    /// `modelScope`, or `modelScope.agents.<name>`. It is interpolated into [`Self::message`], so
    /// an operator reading a refusal can tell a global policy from a per-agent one.
    pub origin: String,
}

/// SUBA-155 — the reserved `scoped` allow token (pi `SCOPED_PATTERN`, `model-scope.ts:51`
/// @v0.74.0, added by `c305d4bb`/#2538): expands to the parent session's scoped-model snapshot.
pub const SCOPED_PATTERN: &str = "scoped";

/// SUBA-155 — the reserved `inherit` allow token (pi `INHERIT_MODEL`, expanded by
/// `expandReservedPatterns`, `model-scope.ts:109`): expands to the parent session's
/// `provider/id`.
pub const INHERIT_PATTERN: &str = "inherit";

/// SUBA-155 — the two `allow` patterns that are NOT patterns: they name a model set the policy
/// cannot know by itself and that has to be substituted before any matching happens.
///
/// An explicit enum rather than the `&'static str` the refusal used to carry, because the two
/// differ in what they resolve AGAINST (the parent model vs. the parent's scoped snapshot) and in
/// which phase resolves them, and a caller that has to compare strings to tell them apart cannot
/// be made exhaustive by the compiler when a third token is added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReservedScopeToken {
    /// `inherit` — the parent session's `provider/id`.
    Inherit,
    /// `scoped` — the parent session's scoped-model snapshot (pi's `/scoped-models`), degrading to
    /// [`Self::Inherit`] when the parent is unscoped.
    Scoped,
}

impl ReservedScopeToken {
    /// The literal spelling an operator writes in `allow`, and the one a refusal names.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inherit => INHERIT_PATTERN,
            Self::Scoped => SCOPED_PATTERN,
        }
    }

    /// `Some` iff this `allow` entry is a reserved token rather than a glob.
    #[must_use]
    pub fn parse(pattern: &str) -> Option<Self> {
        match pattern {
            INHERIT_PATTERN => Some(Self::Inherit),
            SCOPED_PATTERN => Some(Self::Scoped),
            _ => None,
        }
    }
}

/// SUBA-155 — pi `MAX_RENDERED_PATTERNS` (`model-scope.ts:53`): a violation message renders at
/// most this many patterns and then says how many there are in total, so a hundred-pattern
/// allowlist does not become a hundred-pattern error string.
const MAX_RENDERED_PATTERNS: usize = 8;

/// SUBA-155 — one launch-time check: an `allow` list with every reserved token already expanded,
/// plus the block it came from (pi `ResolvedModelScope`, `model-scope.ts:35-37`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResolvedModelScope {
    /// Upstream's `enforce ?? config.enforce` for an agent scope; the block's own value globally.
    pub enforce: Option<bool>,
    /// Upstream's `strict ?? config.strict` for an agent scope; the block's own value globally.
    pub strict: Option<bool>,
    /// The EXPANDED patterns. A reserved token that could not be expanded is kept VERBATIM, which
    /// is what makes an enforced `inherit`/`scoped` with no parent model fail CLOSED (pi's own
    /// comment: *"absent inputs stay literal so enforced resolution fails closed"*,
    /// `model-scope.ts:105`).
    pub allow: Vec<String>,
    /// `modelScope` or `modelScope.agents.<name>`.
    pub origin: String,
}

impl ResolvedModelScope {
    /// True iff this scope actually refuses anything: `enforce: true` AND a non-empty `allow`.
    #[must_use]
    pub fn is_armed(&self) -> bool {
        self.enforce == Some(true) && !self.allow.is_empty()
    }

    /// Whether this scope still holds an UNEXPANDED reserved token — i.e. the parent session had
    /// no model (and, for `scoped`, no snapshot) to expand it against.
    ///
    /// `inherit` wins when both are present, matching upstream's
    /// `allow?.includes(INHERIT_MODEL) ? INHERIT_MODEL : SCOPED_PATTERN`
    /// (`model-resolution.ts:302` @v0.75.0).
    #[must_use]
    fn holds_unexpanded_reserved_token(&self) -> Option<ReservedScopeToken> {
        let mut found = None;
        for pattern in &self.allow {
            match ReservedScopeToken::parse(pattern) {
                Some(ReservedScopeToken::Inherit) => return Some(ReservedScopeToken::Inherit),
                Some(token) => found = Some(token),
                None => {}
            }
        }
        found
    }
}

/// SUBA-155 — phase 2 of the reserved-token resolution: pi `expandReservedPatterns`
/// (`model-scope.ts:108-119` @v0.75.0) with the snapshot arm already applied by
/// [`ModelScopeConfig::with_scoped_snapshot`] (see the module header).
///
/// Both surviving tokens become the parent session's `provider/id`: that is `inherit`'s whole
/// meaning, and it is what `scoped` degrades to once phase 1 has established that the parent holds
/// no snapshot (upstream's `return parentModel ? [...] : [pattern]`). A token that cannot be
/// expanded is returned UNEXPANDED, so an enforced scope made only of reserved tokens matches
/// nothing and the run is refused rather than admitted — upstream's stated fail-closed rule. Every
/// other pattern passes through untouched.
#[must_use]
pub fn expand_reserved_patterns(pattern: &str, parent_model: Option<&str>) -> Vec<String> {
    match (ReservedScopeToken::parse(pattern), parent_model) {
        (Some(_), Some(model)) => vec![model.to_string()],
        (Some(_) | None, _) => vec![pattern.to_string()],
    }
}

/// SUBA-155 — pi `scopedModelIdsFromContext` (`model-resolution.ts:66-76` @v0.75.0) over the JSON
/// [`cyrup_ext::host::HostServices::scoped_models`] reports (pi's `ctx.scopedModels`, the set
/// `/scoped-models` shows — EXT-045 wired the live reader).
///
/// Each row's `model` must carry BOTH a non-empty `provider` and a non-empty `id` to become a
/// `provider/id` entry — upstream's `normalizeParentModel` gate (`model-resolution.ts:55-58`),
/// which rejects `""`, a bare provider and a bare id. Anything else in the array is skipped
/// rather than poisoning the allowlist with a half-formed pattern. A missing or non-array value
/// (no host bound, headless) is the empty snapshot, which is upstream's documented "empty when no
/// scoping is configured" and the input `with_scoped_snapshot` treats as "nothing to substitute".
#[must_use]
pub fn scoped_model_ids_from_host(scoped_models: Option<&serde_json::Value>) -> Vec<String> {
    let Some(rows) = scoped_models.and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            let model = row.get("model")?;
            let provider = model.get("provider")?.as_str()?;
            let id = model.get("id")?.as_str()?;
            (!provider.is_empty() && !id.is_empty()).then(|| format!("{provider}/{id}"))
        })
        .collect()
}

/// SUBA-155 — the three inputs a launch-time scope resolution needs, as one value (pi passes them
/// as `resolveModelScopesForAgent`'s parameters, `model-scope.ts:161` @v0.75.0; its fourth, the
/// scoped-model snapshot, is phase 1's and is already folded into `config` — see the module
/// header).
///
/// Bundled rather than spread because they travel TOGETHER through every launch path and are
/// meaningless apart: a config without the agent name silently drops per-agent rules, and a config
/// without the parent model silently fails an `inherit` scope closed. One value makes a caller
/// that has only some of them impossible to write.
#[derive(Clone, Copy, Debug, Default)]
pub struct ModelScopeContext<'a> {
    /// The `subagents.modelScope` block in force — with the parent's scoped-model snapshot already
    /// substituted by [`ModelScopeConfig::with_scoped_snapshot`] — or `None` for no policy at all.
    pub config: Option<&'a ModelScopeConfig>,
    /// The canonical agent name, which selects a `modelScope.agents.<name>` rule.
    pub agent_name: &'a str,
    /// The parent session's model as `provider/id`, which a surviving `inherit`/`scoped` token
    /// expands to.
    pub parent_model: Option<&'a str>,
}

impl ModelScopeContext<'_> {
    /// [`resolve_model_scopes_for_agent`] over this context's own fields.
    #[must_use]
    pub fn resolve(&self) -> Vec<ResolvedModelScope> {
        resolve_model_scopes_for_agent(self.config, self.agent_name, self.parent_model)
    }
}

/// SUBA-155 — pi `resolveModelScopesForAgent` (`model-scope.ts:161-188` @v0.75.0): the global
/// policy and, when one is configured for this agent, the per-agent policy, as INDEPENDENT checks.
///
/// The agent scope inherits `enforce`/`strict` from the global block when it does not state them
/// (`agentScope.enforce ?? config.enforce`), and a block with no `allow` list contributes no scope
/// at all — upstream's `if (config.allow)` / `if (agentScope?.allow)` gates.
#[must_use]
pub fn resolve_model_scopes_for_agent(
    config: Option<&ModelScopeConfig>,
    agent_name: &str,
    parent_model: Option<&str>,
) -> Vec<ResolvedModelScope> {
    let Some(config) = config else {
        return Vec::new();
    };
    let expand = |patterns: &[String]| -> Vec<String> {
        patterns
            .iter()
            .flat_map(|pattern| expand_reserved_patterns(pattern, parent_model))
            .collect()
    };
    let mut scopes = Vec::new();
    if let Some(allow) = config.allow.as_deref() {
        scopes.push(ResolvedModelScope {
            enforce: config.enforce,
            strict: config.strict,
            allow: expand(allow),
            origin: "modelScope".to_string(),
        });
    }
    if let Some(agent_scope) = config.agents.as_ref().and_then(|map| map.get(agent_name))
        && let Some(allow) = agent_scope.allow.as_deref()
    {
        scopes.push(ResolvedModelScope {
            enforce: agent_scope.enforce.or(config.enforce),
            strict: agent_scope.strict.or(config.strict),
            allow: expand(allow),
            origin: format!("modelScope.agents.{agent_name}"),
        });
    }
    scopes
}

/// SUBA-155 — why a launch was refused by `subagents.modelScope`, as the two cases upstream
/// genuinely has rather than one struct that has to pretend.
///
/// pi throws two different `Error`s on this path: `checkModelScope`'s violation message
/// (`model-scope.ts:101-103` @v0.75.0) and `throwForUnresolvedEnforcedReservedScope`'s
/// (`model-resolution.ts:303`). The second has no model and no allowlist to report — it fires
/// BEFORE any model is resolved, precisely because the policy could not be made concrete — so
/// expressing it as a [`ModelScopeViolation`] would mean inventing a model and an `allowedPatterns`
/// list the refusal never had. Each variant carries exactly what its own case knows, and
/// [`Self::message`] renders pi's verbatim sentence for either.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelScopeRefusal {
    /// A resolved model fell outside an armed scope at error severity.
    OutOfScope(ModelScopeViolation),
    /// An armed scope's reserved token could not be made concrete: the host reports no current
    /// parent session model, so there is nothing for `inherit`/`scoped` to name.
    UnresolvableReservedToken {
        /// `modelScope`, or `modelScope.agents.<name>` — which block cannot be enforced.
        origin: String,
        /// Which token is unresolvable.
        token: ReservedScopeToken,
    },
}

impl ModelScopeRefusal {
    /// pi's verbatim refusal sentence, which is what reaches the caller as
    /// [`crate::error::SubagentError::ModelOutOfScope`] / a step failure.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::OutOfScope(violation) => violation.message.clone(),
            Self::UnresolvableReservedToken { origin, token } => format!(
                "Cannot enforce subagent model scope ({origin}): '{}' requires a current parent \
                 session model.",
                token.as_str()
            ),
        }
    }

    /// The out-of-scope decision, for a caller that wants the severity/patterns rather than the
    /// sentence. `None` for the unresolvable-token case, which has neither.
    #[must_use]
    pub fn violation(&self) -> Option<&ModelScopeViolation> {
        match self {
            Self::OutOfScope(violation) => Some(violation),
            Self::UnresolvableReservedToken { .. } => None,
        }
    }
}

impl From<ModelScopeViolation> for ModelScopeRefusal {
    fn from(violation: ModelScopeViolation) -> Self {
        Self::OutOfScope(violation)
    }
}

/// SUBA-155 — pi `throwForUnresolvedEnforcedReservedScope` (`model-resolution.ts:295-304`
/// @v0.75.0): the refusal for an ENFORCED scope whose reserved token could not be expanded,
/// because the host has no current parent session model.
///
/// `include_mixed` is upstream's own second mode: with it, ANY unexpanded reserved token in the
/// list is refused (the no-explicit-model and inherited-source paths); without it, only a list
/// that is nothing BUT one reserved token — so an `allow: ["inherit", "openai/*"]` still admits an
/// explicitly requested `openai/gpt-5` on a headless host instead of refusing it outright.
///
/// This must be consulted BEFORE resolution: without it the refusal still happens (an unexpanded
/// token matches no model), but only for the model that happens to be resolved, at warn severity
/// on an inherited source — so an enforced policy the host cannot make concrete would print a
/// warning and run the model anyway. Upstream fails CLOSED and says why.
#[must_use]
pub fn unresolvable_enforced_reserved_scope(
    scopes: &[ResolvedModelScope],
    include_mixed: bool,
) -> Option<ModelScopeRefusal> {
    let found = scopes.iter().find(|scope| {
        scope.enforce == Some(true)
            && if include_mixed {
                scope.holds_unexpanded_reserved_token().is_some()
            } else {
                scope.allow.len() == 1 && scope.holds_unexpanded_reserved_token().is_some()
            }
    })?;
    Some(ModelScopeRefusal::UnresolvableReservedToken {
        origin: found.origin.clone(),
        token: found.holds_unexpanded_reserved_token()?,
    })
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

/// Pure scope decision (pi `checkModelScope`, `model-scope.ts:77-104` @v0.74.0).
///
/// Returns `Some(violation)` when the model is out of scope AND enforcement is armed, else `None`.
/// Enforcement with no `allow` list is a no-op.
///
/// SUBA-155 — the parameter is one already-[`resolve_model_scopes_for_agent`]-resolved scope, not
/// the raw settings block: the reserved `inherit`/`scoped` tokens must be expanded BEFORE a
/// pattern is matched, and `origin` has to be on the scope so the message can name which block
/// refused. A caller holding several scopes goes through [`enforce_model_scopes`].
#[must_use]
pub fn check_model_scope(
    model: Option<&str>,
    scope: &ResolvedModelScope,
    source: ModelSource,
) -> Option<ModelScopeViolation> {
    let model = model.filter(|m| !m.is_empty())?;
    if scope.enforce != Some(true) {
        return None;
    }
    let allow = &scope.allow;
    if allow.is_empty() {
        return None;
    }
    if allow
        .iter()
        .any(|pattern| matches_scope_pattern(model, pattern))
    {
        return None;
    }

    let base_model = split_known_thinking_suffix(model).0.to_string();
    // SUBA-050 / pi `model-scope.ts:94` @v0.74.0:
    // `source === "explicit" || scope.strict === true ? "error" : "warn"`. The `=== true` is
    // load-bearing on both sides — an absent `strict` and an explicit `strict: false` behave
    // identically, and only the literal boolean `true` promotes an inherited/fallback violation to
    // a hard error.
    let severity = match source {
        ModelSource::Explicit => ModelScopeSeverity::Error,
        ModelSource::Inherited if scope.strict == Some(true) => ModelScopeSeverity::Error,
        ModelSource::Inherited => ModelScopeSeverity::Warn,
    };
    let message = format!(
        "Model '{base_model}' is outside the configured subagent model scope ({}). Allowed patterns: {}.",
        scope.origin,
        render_allow_patterns(allow)
    );
    Some(ModelScopeViolation {
        model: base_model,
        severity,
        message,
        allowed_patterns: allow.clone(),
        origin: scope.origin.clone(),
    })
}

/// SUBA-155 — pi's render cap (`model-scope.ts:96-98` @v0.74.0):
/// `a, b, …, h, … (N patterns total)` past [`MAX_RENDERED_PATTERNS`]. The full list still travels
/// on [`ModelScopeViolation::allowed_patterns`]; only the human-facing sentence is capped.
fn render_allow_patterns(allow: &[String]) -> String {
    if allow.len() <= MAX_RENDERED_PATTERNS {
        return allow.join(", ");
    }
    format!(
        "{}, … ({} patterns total)",
        allow
            .iter()
            .take(MAX_RENDERED_PATTERNS)
            .cloned()
            .collect::<Vec<_>>()
            .join(", "),
        allow.len()
    )
}

/// SUBA-155 — pi `enforceModelScopes` (`model-resolution.ts:298-310` @v0.74.0): check `model`
/// against EVERY resolved scope, refuse on the first error-severity violation, and hand the
/// warn-severity ones back to the caller.
///
/// Upstream collects all violations, throws the first `error` one and warns the rest; the same
/// order is reproduced here, because a global `modelScope` and a `modelScope.agents.<name>` are
/// independent checks and a model must satisfy both.
///
/// # Errors
///
/// Returns the first error-severity [`ModelScopeViolation`], which refuses the run.
pub fn enforce_model_scopes(
    model: Option<&str>,
    scopes: &[ResolvedModelScope],
    source: ModelSource,
) -> Result<Vec<ModelScopeViolation>, ModelScopeViolation> {
    let violations: Vec<ModelScopeViolation> = scopes
        .iter()
        .filter_map(|scope| check_model_scope(model, scope, source))
        .collect();
    if let Some(error) = violations
        .iter()
        .find(|violation| violation.severity == ModelScopeSeverity::Error)
    {
        return Err(error.clone());
    }
    for violation in &violations {
        warn_violation(violation);
    }
    Ok(violations)
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

    let (rule, saw_field) = parse_scope_rule(input, "modelScope")?;
    let mut config = ModelScopeConfig {
        enforce: rule.enforce,
        strict: rule.strict,
        allow: rule.allow,
        agents: None,
    };
    let mut saw_field = saw_field;

    // SUBA-155 — pi `parseModelScopeConfig`'s `agents` arm (`model-scope.ts:196-210` @v0.74.0),
    // including its refusal of a NESTED `agents` key inside an agent scope.
    if let Some(raw) = input.get("agents") {
        let Some(entries) = raw.as_object() else {
            return Err(
                "invalid 'modelScope.agents'; expected an object keyed by agent name".to_string(),
            );
        };
        let mut agents: std::collections::BTreeMap<String, ModelScopeRule> =
            std::collections::BTreeMap::new();
        for (raw_name, raw_scope) in entries {
            let name = raw_name.trim();
            if name.is_empty() {
                return Err(
                    "invalid 'modelScope.agents' key; expected a non-empty agent name".to_string(),
                );
            }
            let field = format!("modelScope.agents.{name}");
            let Some(agent_input) = raw_scope.as_object() else {
                return Err(format!("invalid '{field}'; expected an object"));
            };
            if agent_input.contains_key("agents") {
                return Err(format!(
                    "invalid '{field}.agents'; nested agent scopes are not supported"
                ));
            }
            let (agent_rule, _) = parse_scope_rule(agent_input, &field)?;
            agents.insert(name.to_string(), agent_rule);
        }
        config.agents = Some(agents);
        saw_field = true;
    }

    // SUBA-155 — pi `hasAnyAllow` (`model-scope.ts:212` @v0.74.0): a global `enforce: true` is
    // satisfied by a per-agent `allow` list too, so an operator who restricts only named agents
    // does not have to repeat a global allowlist.
    let has_any_allow = config.allow.as_ref().is_some_and(|a| !a.is_empty())
        || config.agents.as_ref().is_some_and(|map| {
            map.values()
                .any(|rule| rule.allow.as_ref().is_some_and(|a| !a.is_empty()))
        });
    if config.enforce == Some(true) && !has_any_allow {
        return Err(
            "modelScope.enforce is set without a non-empty 'allow' list; supply allowed model \
             patterns or disable enforcement"
                .to_string(),
        );
    }

    Ok(if saw_field { Some(config) } else { None })
}

/// SUBA-155 — pi `parseScopeRule` (`model-scope.ts:144-155` @v0.74.0): the three shared keys,
/// validated identically wherever they appear. `field` prefixes every message, so an error under
/// `modelScope.agents.reviewer` names that path rather than the bare key.
///
/// Returns the rule and whether the object said anything at all (pi's
/// `Object.keys(config).length > 0` gate, applied by the caller).
///
/// # Errors
///
/// A non-boolean `enforce`/`strict`, or an `allow` that is not an array of strings or is
/// effectively empty.
fn parse_scope_rule(
    input: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<(ModelScopeRule, bool), String> {
    let mut rule = ModelScopeRule::default();
    let mut saw_field = false;

    if let Some(raw) = input.get("enforce") {
        let Some(flag) = raw.as_bool() else {
            return Err(format!("invalid '{field}.enforce'; expected a boolean"));
        };
        rule.enforce = Some(flag);
        saw_field = true;
    }

    // SUBA-050 / pi `parseScopeRule` validates `strict` between `enforce` and `allow`, with the
    // same typed-error shape as `enforce`.
    if let Some(raw) = input.get("strict") {
        let Some(flag) = raw.as_bool() else {
            return Err(format!("invalid '{field}.strict'; expected a boolean"));
        };
        rule.strict = Some(flag);
        saw_field = true;
    }

    if let Some(raw) = input.get("allow") {
        let invalid = format!("invalid '{field}.allow'; expected an array of strings");
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
            return Err(format!(
                "invalid '{field}.allow'; expected a non-empty array of patterns"
            ));
        }
        rule.allow = Some(allow);
        saw_field = true;
    }

    Ok((rule, saw_field))
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

    fn scope(patterns: &[&str]) -> ModelScopeConfig {
        ModelScopeConfig {
            enforce: Some(true),
            strict: None,
            allow: Some(patterns.iter().map(|p| (*p).to_string()).collect()),
            agents: None,
        }
    }

    /// SUBA-155 — the GLOBAL scope a config resolves to with no agent rule and no parent session,
    /// so the rows below that are about `checkModelScope` alone keep reading as before. Rows about
    /// the resolution itself call [`resolve_model_scopes_for_agent`] directly.
    fn resolved(config: &ModelScopeConfig) -> ResolvedModelScope {
        resolve_model_scopes_for_agent(Some(config), "worker", None)
            .into_iter()
            .next()
            .unwrap_or_else(|| ResolvedModelScope {
                enforce: config.enforce,
                strict: config.strict,
                allow: Vec::new(),
                origin: "modelScope".to_string(),
            })
    }

    /// SUBA-155 — `allow: ["inherit"]` is the shape pi's own documentation shows, and before this
    /// row it REJECTED EVERY EXPLICIT MODEL.
    ///
    /// THE USER ACTION: an operator wants subagents pinned to whatever model the parent session is
    /// on, copies `"modelScope": {"enforce": true, "allow": ["inherit"]}` out of pi's reference,
    /// and every `subagent({model: "..."})` call is refused with
    /// `SubagentError::ModelOutOfScope` naming an allowlist of one meaningless word — because
    /// `inherit` was matched LITERALLY against `provider/id`. Upstream expands it first
    /// (`expandReservedPatterns`, `src/runs/shared/model-scope.ts:105-114` @v0.74.0).
    #[test]
    fn the_inherit_token_expands_to_the_parent_session_model() {
        let config = scope(&["inherit"]);
        let scopes = resolve_model_scopes_for_agent(Some(&config), "worker", Some("openai/gpt-5"));
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].allow, vec!["openai/gpt-5".to_string()]);
        assert_eq!(scopes[0].origin, "modelScope");

        // The parent's own model is admitted...
        assert!(
            check_model_scope(Some("openai/gpt-5"), &scopes[0], ModelSource::Explicit).is_none(),
            "`inherit` must admit the model the parent session is actually on"
        );
        // ...and another model is still refused, so the policy still polices.
        let refused = check_model_scope(Some("anthropic/opus"), &scopes[0], ModelSource::Explicit)
            .expect("a different model is out of scope");
        assert_eq!(refused.severity, ModelScopeSeverity::Error);
    }

    /// SUBA-155 — the fail-closed rule: a reserved token with nothing to expand against stays
    /// LITERAL, so an enforced scope made of it matches nothing and refuses the run rather than
    /// admitting everything (pi's own comment at `model-scope.ts:105`). Upstream additionally
    /// explains itself, which is what `unresolved_enforced_reserved_scope_message` ports.
    #[test]
    fn an_unexpandable_reserved_token_fails_closed_and_says_why() {
        let config = scope(&["inherit"]);
        let scopes = resolve_model_scopes_for_agent(Some(&config), "worker", None);
        assert_eq!(
            scopes[0].allow,
            vec!["inherit".to_string()],
            "an unexpanded token must stay literal, never be dropped (which would admit all)"
        );
        assert!(
            check_model_scope(Some("openai/gpt-5"), &scopes[0], ModelSource::Explicit).is_some(),
            "with no parent model an enforced `inherit` admits nothing"
        );
        assert_eq!(
            unresolvable_enforced_reserved_scope(&scopes, false)
                .as_ref()
                .map(ModelScopeRefusal::message),
            Some(
                "Cannot enforce subagent model scope (modelScope): 'inherit' requires a current \
                 parent session model."
                    .to_string()
            )
        );
        // A resolvable scope says nothing.
        let resolvable =
            resolve_model_scopes_for_agent(Some(&config), "worker", Some("openai/gpt-5"));
        assert_eq!(
            unresolvable_enforced_reserved_scope(&resolvable, true),
            None
        );
    }

    /// SUBA-155 — `scoped` resolves to the parent's scoped-model snapshot (pi's `/scoped-models`),
    /// and degrades to `inherit` semantics when that snapshot is empty
    /// (`model-scope.ts:113-116` @v0.75.0).
    ///
    /// Asserted over the COMPOSITION of the two phases cyrup splits upstream's one function into
    /// (module header), case for case against upstream's own four: a non-empty snapshot yields the
    /// snapshot; an empty snapshot plus a parent model yields that model; neither leaves the token
    /// literal, which is fail-closed and names `scoped` — not `inherit` — in the refusal.
    #[test]
    fn the_scoped_token_resolves_to_the_snapshot_and_degrades_to_inherit() {
        let config = scope(&[SCOPED_PATTERN]);
        let snapshot = vec!["openai/gpt-5".to_string(), "anthropic/opus".to_string()];

        let scopes = resolve_model_scopes_for_agent(
            Some(&config.with_scoped_snapshot(&snapshot)),
            "worker",
            Some("openai/gpt-4"),
        );
        assert_eq!(scopes[0].allow, snapshot, "the whole snapshot is admitted");
        assert!(
            check_model_scope(Some("anthropic/opus"), &scopes[0], ModelSource::Explicit).is_none(),
            "a model in the snapshot is admitted even though it is not the parent's own model"
        );
        assert!(
            check_model_scope(Some("openai/gpt-4"), &scopes[0], ModelSource::Explicit).is_some(),
            "the control: the parent's OWN model is refused when it is outside the snapshot, so \
             `scoped` is not silently behaving as `inherit`"
        );

        // Empty snapshot → nothing substituted, so phase 2 degrades the token to the parent model.
        let degraded = resolve_model_scopes_for_agent(
            Some(&config.with_scoped_snapshot(&[])),
            "worker",
            Some("openai/gpt-4"),
        );
        assert_eq!(degraded[0].allow, vec!["openai/gpt-4".to_string()]);

        // Neither → literal, i.e. fail closed, and the refusal names `scoped`.
        let closed =
            resolve_model_scopes_for_agent(Some(&config.with_scoped_snapshot(&[])), "worker", None);
        assert_eq!(closed[0].allow, vec![SCOPED_PATTERN.to_string()]);
        assert_eq!(
            unresolvable_enforced_reserved_scope(&closed, false)
                .as_ref()
                .map(ModelScopeRefusal::message),
            Some(
                "Cannot enforce subagent model scope (modelScope): 'scoped' requires a current \
                 parent session model."
                    .to_string()
            )
        );
    }

    /// SUBA-155 — phase 1 substitutes the snapshot into the PER-AGENT lists too, which is the only
    /// way `modelScope.agents.<name>: {allow: ["scoped"]}` can mean anything.
    #[test]
    fn the_snapshot_is_substituted_into_per_agent_allow_lists() {
        let mut config = scope(&[SCOPED_PATTERN]);
        config.agents = Some(std::collections::BTreeMap::from([(
            "reviewer".to_string(),
            ModelScopeRule {
                enforce: None,
                strict: None,
                allow: Some(vec![SCOPED_PATTERN.to_string(), "anthropic/*".to_string()]),
            },
        )]));
        let snapshot = vec!["openai/gpt-5".to_string()];
        let substituted = config.with_scoped_snapshot(&snapshot);
        let agent_allow = substituted
            .agents
            .as_ref()
            .and_then(|map| map.get("reviewer"))
            .and_then(|rule| rule.allow.as_deref())
            .expect("the per-agent rule survived substitution");
        assert_eq!(
            agent_allow,
            ["openai/gpt-5".to_string(), "anthropic/*".to_string()],
            "`scoped` is replaced in place and every other pattern is left alone"
        );
    }

    /// SUBA-155 — pi `scopedModelIdsFromContext` (`model-resolution.ts:66-76` @v0.75.0) over the
    /// JSON shape `HostServices::scoped_models` reports (`{model: {provider, id}, thinkingLevel?}`).
    ///
    /// The `normalizeParentModel` gate is the load-bearing part: a half-formed row must be SKIPPED,
    /// never turned into a pattern like `anthropic/` that would silently widen or narrow the
    /// allowlist.
    #[test]
    fn the_host_snapshot_reader_keeps_only_well_formed_provider_id_rows() {
        assert!(scoped_model_ids_from_host(None).is_empty());
        assert!(scoped_model_ids_from_host(Some(&serde_json::json!({}))).is_empty());
        assert_eq!(
            scoped_model_ids_from_host(Some(&serde_json::json!([
                {"model": {"provider": "openai", "id": "gpt-5"}, "thinkingLevel": "high"},
                {"model": {"provider": "anthropic", "id": "opus"}},
                {"model": {"provider": "", "id": "x"}},
                {"model": {"provider": "y", "id": ""}},
                {"model": {"provider": "z"}},
                {"model": "openai/gpt-5"},
                {}
            ]))),
            vec!["openai/gpt-5".to_string(), "anthropic/opus".to_string()]
        );
    }

    /// SUBA-155 — `modelScope.agents.<name>` is an ADDITIONAL, independent restriction, and it
    /// inherits `enforce`/`strict` from the global block (pi `resolveModelScopesForAgent`,
    /// `model-scope.ts:160-181` @v0.74.0). Before this row serde accepted the key and dropped it,
    /// so a per-agent restriction an operator believed was in force was not — the failure
    /// direction that matters for a policy knob.
    #[test]
    fn a_per_agent_rule_adds_a_restriction_and_inherits_enforce_and_strict() {
        let mut config = scope(&["anthropic/*", "openai/*"]);
        config.strict = Some(true);
        config.agents = Some(std::collections::BTreeMap::from([(
            "reviewer".to_string(),
            ModelScopeRule {
                enforce: None,
                strict: None,
                allow: Some(vec!["anthropic/*".to_string()]),
            },
        )]));

        // An agent with no rule of its own sees only the global scope.
        let worker = resolve_model_scopes_for_agent(Some(&config), "worker", None);
        assert_eq!(worker.len(), 1);
        assert_eq!(worker[0].origin, "modelScope");

        // The named agent gets BOTH checks, and the second one carries the global flags.
        let reviewer = resolve_model_scopes_for_agent(Some(&config), "reviewer", None);
        assert_eq!(reviewer.len(), 2);
        assert_eq!(reviewer[1].origin, "modelScope.agents.reviewer");
        assert_eq!(reviewer[1].enforce, Some(true), "enforce ?? config.enforce");
        assert_eq!(reviewer[1].strict, Some(true), "strict ?? config.strict");

        // `openai/*` passes the global scope and is refused by the per-agent one.
        let refused = enforce_model_scopes(
            Some("openai/gpt-5"),
            &reviewer,
            crate::exec::model_scope::ModelSource::Explicit,
        )
        .expect_err("the per-agent rule must refuse a model the global rule allows");
        assert_eq!(refused.origin, "modelScope.agents.reviewer");
        assert!(
            refused.message.contains("(modelScope.agents.reviewer)"),
            "the message must name WHICH block refused: {}",
            refused.message
        );

        // And a model inside both is admitted.
        assert!(
            enforce_model_scopes(
                Some("anthropic/opus"),
                &reviewer,
                crate::exec::model_scope::ModelSource::Explicit
            )
            .is_ok()
        );
    }

    /// SUBA-155 — the parser's `agents` arm: an object keyed by agent name, no nested `agents`,
    /// and a global `enforce: true` satisfied by a per-agent `allow` alone (pi `hasAnyAllow`,
    /// `model-scope.ts:212` @v0.74.0).
    #[test]
    fn the_parser_reads_per_agent_rules_and_refuses_the_shapes_upstream_refuses() {
        let value = serde_json::json!({
            "enforce": true,
            "agents": { " reviewer ": { "allow": ["anthropic/*"], "strict": true } }
        });
        let parsed = parse_model_scope_config(Some(&value))
            .expect("a per-agent allow satisfies a global enforce")
            .expect("the block says something");
        let agents = parsed.agents.as_ref().expect("agents parsed");
        let rule = agents.get("reviewer").expect("the key is trimmed");
        assert_eq!(
            rule.allow.as_deref(),
            Some(&["anthropic/*".to_string()][..])
        );
        assert_eq!(rule.strict, Some(true));

        // A nested `agents` is refused by name.
        let nested = serde_json::json!({
            "allow": ["anthropic/*"],
            "agents": { "reviewer": { "agents": {} } }
        });
        assert_eq!(
            parse_model_scope_config(Some(&nested)),
            Err(
                "invalid 'modelScope.agents.reviewer.agents'; nested agent scopes are not \
                 supported"
                    .to_string()
            )
        );

        // A non-object `agents`, and an empty key.
        assert_eq!(
            parse_model_scope_config(Some(&serde_json::json!({"agents": []}))),
            Err("invalid 'modelScope.agents'; expected an object keyed by agent name".to_string())
        );
        assert_eq!(
            parse_model_scope_config(Some(
                &serde_json::json!({"agents": {"  ": {"allow": ["x"]}}})
            )),
            Err("invalid 'modelScope.agents' key; expected a non-empty agent name".to_string())
        );
        // An agent rule's own fields are validated under the agent's path.
        assert_eq!(
            parse_model_scope_config(Some(
                &serde_json::json!({"agents": {"reviewer": {"enforce": "yes"}}})
            )),
            Err("invalid 'modelScope.agents.reviewer.enforce'; expected a boolean".to_string())
        );
    }

    /// SUBA-155 — pi's render cap (`MAX_RENDERED_PATTERNS = 8`, `model-scope.ts:53,96-98`): a
    /// nine-pattern allowlist renders eight plus `… (9 patterns total)`, while the full list still
    /// travels on the violation for a caller that wants it.
    #[test]
    fn a_long_allowlist_is_rendered_capped_at_eight_patterns() {
        let patterns: Vec<String> = (1..=9).map(|n| format!("vendor{n}/*")).collect();
        let config = ModelScopeConfig {
            enforce: Some(true),
            strict: None,
            allow: Some(patterns.clone()),
            agents: None,
        };
        let violation = check_model_scope(
            Some("openai/gpt-5"),
            &resolved(&config),
            ModelSource::Explicit,
        )
        .expect("out of scope");
        assert!(
            violation.message.ends_with(
                "vendor1/*, vendor2/*, vendor3/*, vendor4/*, vendor5/*, vendor6/*, vendor7/*, \
                 vendor8/*, … (9 patterns total)."
            ),
            "{}",
            violation.message
        );
        assert_eq!(
            violation.allowed_patterns, patterns,
            "the cap is cosmetic; the full list still travels"
        );

        // Exactly eight is rendered in full.
        let eight = ModelScopeConfig {
            enforce: Some(true),
            strict: None,
            allow: Some(patterns[..8].to_vec()),
            agents: None,
        };
        let v8 = check_model_scope(
            Some("openai/gpt-5"),
            &resolved(&eight),
            ModelSource::Explicit,
        )
        .expect("out of scope");
        assert!(v8.message.ends_with("vendor8/*."), "{}", v8.message);
    }

    /// SUBA-035 — the summary must distinguish the three states an operator can be in, and in
    /// particular must NOT call a present-but-inert policy "enforcing": the whole reason the item
    /// exists is that a policy nobody can see gets blamed for the wrong things.
    #[test]
    fn the_scope_summary_separates_absent_inert_and_armed() {
        assert_eq!(
            model_scope_summary_line(None),
            "  (none configured — every resolved model is in scope)"
        );

        // enforce: true with NO patterns — armed-looking, enforces nothing (upstream's
        // `if (!allow || allow.length === 0) return undefined`).
        let empty = ModelScopeConfig {
            enforce: Some(true),
            strict: None,
            allow: Some(Vec::new()),
            agents: None,
        };
        assert_eq!(
            model_scope_summary_line(Some(&empty)),
            "  (not enforcing — 0 allow pattern(s) are inert)"
        );

        // Patterns but no `enforce` — also inert, and the count is still reported so the operator
        // can see the list exists.
        let unarmed = ModelScopeConfig {
            enforce: None,
            strict: None,
            allow: Some(vec!["anthropic/*".to_string()]),
            agents: None,
        };
        assert_eq!(
            model_scope_summary_line(Some(&unarmed)),
            "  (not enforcing — 1 allow pattern(s) are inert)"
        );

        assert_eq!(
            model_scope_summary_line(Some(&scope(&["anthropic/*", "openai/gpt-5"]))),
            "  enforcing (non-strict): allow anthropic/*, openai/gpt-5"
        );

        let mut strict = scope(&["anthropic/*"]);
        strict.strict = Some(true);
        assert_eq!(
            model_scope_summary_line(Some(&strict)),
            "  enforcing (strict): allow anthropic/*",
            "SUBA-050's strict flag changes what an inherited/fallback violation DOES, so it has \
             to be visible here"
        );
    }

    #[test]
    fn glob_only_treats_star_as_special_and_is_case_insensitive() {
        assert!(matches_scope_pattern(
            "anthropic/claude-opus-4",
            "anthropic/*"
        ));
        assert!(matches_scope_pattern(
            "ANTHROPIC/Claude-Opus-4",
            "anthropic/*"
        ));
        assert!(matches_scope_pattern("anthropic/claude-opus-4", "*opus*"));
        assert!(matches_scope_pattern(
            "anthropic/claude-opus-4",
            "anthropic/claude-opus-4"
        ));
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
        assert!(matches_scope_pattern(
            "anthropic/claude-opus-4:max",
            "anthropic/claude-opus-4"
        ));
        assert!(matches_scope_pattern(
            "anthropic/claude-opus-4:high",
            "anthropic/claude-opus-4"
        ));
        // An UNKNOWN colon suffix is part of the id, not a thinking level, and must not be stripped.
        assert!(!matches_scope_pattern(
            "anthropic/claude-opus-4:preview",
            "anthropic/claude-opus-4"
        ));
    }

    #[test]
    fn an_explicit_out_of_scope_model_is_an_error_and_an_inherited_one_is_a_warning() {
        let s = scope(&["anthropic/*"]);
        let explicit =
            check_model_scope(Some("openai/gpt-5"), &resolved(&s), ModelSource::Explicit)
                .expect("out-of-scope explicit model must violate");
        assert_eq!(explicit.severity, ModelScopeSeverity::Error);
        assert_eq!(
            explicit.message,
            "Model 'openai/gpt-5' is outside the configured subagent model scope (modelScope). Allowed \
             patterns: anthropic/*."
        );
        let inherited =
            check_model_scope(Some("openai/gpt-5"), &resolved(&s), ModelSource::Inherited)
                .expect("out-of-scope inherited model must violate");
        assert_eq!(inherited.severity, ModelScopeSeverity::Warn);
        assert_eq!(inherited.message, explicit.message);
    }

    /// SUBA-050 — pi `model-scope.ts:73` @v0.47.1:
    /// `source === "explicit" || scope.strict === true ? "error" : "warn"`.
    ///
    /// THE USER ACTION: an operator sets a model allowlist to keep subagents off expensive or
    /// non-compliant models and wants it BINDING. Without `strict` the policy is advisory for
    /// exactly the sources that are hardest to audit — an agent whose frontmatter names an
    /// out-of-scope model, or a fallback ladder that walks onto one, warned and then ran on it.
    ///
    /// Red before the fix in the most literal way available: `ModelScopeConfig` had no `strict`
    /// field at all, so this test would not compile; once the field existed but the severity match
    /// stayed the unconditional `Inherited => Warn`, the first assertion would fail.
    #[test]
    fn strict_promotes_an_inherited_out_of_scope_model_from_a_warning_to_an_error() {
        let mut strict = scope(&["anthropic/*"]);
        strict.strict = Some(true);

        let inherited = check_model_scope(
            Some("openai/gpt-5"),
            &resolved(&strict),
            ModelSource::Inherited,
        )
        .expect("out-of-scope inherited model must still violate");
        assert_eq!(
            inherited.severity,
            ModelScopeSeverity::Error,
            "strict:true must make an inherited violation a hard error"
        );
        // The message is unchanged — upstream only moves the severity.
        assert_eq!(
            inherited.message,
            "Model 'openai/gpt-5' is outside the configured subagent model scope (modelScope). Allowed \
             patterns: anthropic/*."
        );

        // Explicit stays an error (it always was), so the strict arm cannot mask a regression there.
        let explicit = check_model_scope(
            Some("openai/gpt-5"),
            &resolved(&strict),
            ModelSource::Explicit,
        )
        .expect("violates");
        assert_eq!(explicit.severity, ModelScopeSeverity::Error);

        // pi's `scope.strict === true` is strict equality: an EXPLICIT `false` behaves exactly like
        // an absent key, so neither may promote.
        let mut lax = scope(&["anthropic/*"]);
        lax.strict = Some(false);
        assert_eq!(
            check_model_scope(
                Some("openai/gpt-5"),
                &resolved(&lax),
                ModelSource::Inherited
            )
            .expect("violates")
            .severity,
            ModelScopeSeverity::Warn
        );

        // And `strict` alone never ARMS enforcement — pi's `!scope?.enforce` short-circuit runs
        // first (`model-scope.ts:67`).
        let mut strict_but_off = strict.clone();
        strict_but_off.enforce = Some(false);
        assert!(
            check_model_scope(
                Some("openai/gpt-5"),
                &resolved(&strict_but_off),
                ModelSource::Inherited
            )
            .is_none()
        );
    }

    /// SUBA-050's settings half — pi `parseModelScopeConfig` (`model-scope.ts:108-113` @v0.47.1)
    /// validates `strict` with the same typed error `enforce` gets. Before the fix the key was
    /// silently dropped by serde, which is the exact "accepted, unvalidated and inert" shape the
    /// item names.
    #[test]
    fn a_strict_key_round_trips_and_a_non_boolean_strict_is_rejected() {
        let parsed = parse_model_scope_config(Some(&serde_json::json!({
            "enforce": true,
            "strict": true,
            "allow": ["anthropic/*"]
        })))
        .expect("valid config parses");
        assert_eq!(parsed.as_ref().and_then(|c| c.strict), Some(true));

        let err = parse_model_scope_config(Some(&serde_json::json!({
            "enforce": true,
            "strict": "yes",
            "allow": ["anthropic/*"]
        })))
        .expect_err("a non-boolean strict must abort discovery, not be dropped");
        assert_eq!(err, "invalid 'modelScope.strict'; expected a boolean");
    }

    #[test]
    fn enforcement_is_a_no_op_without_enforce_or_without_patterns() {
        let off = ModelScopeConfig {
            enforce: Some(false),
            strict: None,
            allow: Some(vec!["anthropic/*".into()]),
            agents: None,
        };
        assert!(
            check_model_scope(Some("openai/gpt-5"), &resolved(&off), ModelSource::Explicit)
                .is_none()
        );
        let empty = ModelScopeConfig {
            enforce: Some(true),
            strict: None,
            allow: Some(Vec::new()),
            agents: None,
        };
        assert!(
            check_model_scope(
                Some("openai/gpt-5"),
                &resolved(&empty),
                ModelSource::Explicit
            )
            .is_none()
        );
        assert!(
            enforce_model_scopes(Some("openai/gpt-5"), &[], ModelSource::Explicit)
                .is_ok_and(|warns| warns.is_empty())
        );
        assert!(
            check_model_scope(
                None,
                &resolved(&scope(&["anthropic/*"])),
                ModelSource::Explicit
            )
            .is_none()
        );
    }

    #[test]
    fn the_violation_message_strips_the_thinking_suffix_from_the_reported_model() {
        let v = check_model_scope(
            Some("openai/gpt-5:high"),
            &resolved(&scope(&["anthropic/*", "together/*"])),
            ModelSource::Explicit,
        )
        .expect("violates");
        assert_eq!(v.model, "openai/gpt-5");
        assert_eq!(
            v.message,
            "Model 'openai/gpt-5' is outside the configured subagent model scope (modelScope). Allowed \
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
            Err(
                "modelScope.enforce is set without a non-empty 'allow' list; supply allowed model \
                 patterns or disable enforcement"
                    .to_string()
            )
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
                strict: None,
                allow: Some(vec!["anthropic/*".to_string(), "openai/gpt-5".to_string()]),
                agents: None,
            })
        );
        assert!(parsed.as_ref().is_some_and(ModelScopeConfig::is_armed));
    }

    #[test]
    fn an_empty_object_says_nothing_and_yields_no_config() {
        assert_eq!(
            parse_model_scope_config(Some(&serde_json::json!({}))),
            Ok(None)
        );
    }

    // ===================================================================================
    // SUBA-155 — the policy as an OPERATOR writes it, through the real launch resolver
    // ===================================================================================

    /// Pi's documented policy block, verbatim, for the tests below to parse rather than hand-build
    /// — a hand-built `ModelScopeConfig` cannot catch a parser that drops a key.
    fn parsed(json: serde_json::Value) -> ModelScopeConfig {
        parse_model_scope_config(Some(&json))
            .expect("the documented block is valid")
            .expect("the block says something")
    }

    /// SUBA-155's headline scenario, end to end through the launch resolver rather than the pure
    /// core: a policy written the way pi DOCUMENTS it must resolve the way pi resolves it.
    ///
    /// THE USER ACTION: an operator wants subagents pinned to whatever model the parent session is
    /// on, copies `"modelScope": {"enforce": true, "allow": ["inherit"]}` out of pi's reference
    /// into `settings.json`, and launches a subagent with an explicit `model`. Matched literally,
    /// `inherit` matches no `provider/id` at all, so EVERY explicit model was refused with
    /// `SubagentError::ModelOutOfScope` naming an allowlist of one meaningless word.
    ///
    /// The control is the second half: a genuinely out-of-scope model must STILL be refused, so
    /// the fix cannot degrade into "accept everything".
    #[test]
    fn the_documented_inherit_policy_admits_the_parent_model_and_still_refuses_another() {
        let config = parsed(serde_json::json!({"enforce": true, "allow": ["inherit"]}));
        let parent = cyrup_core::ModelId::from("openai/gpt-5");

        // The parent's own model, asked for EXPLICITLY, resolves.
        let asked = parent.clone();
        let mut available = vec![asked.clone()];
        assert_eq!(
            crate::exec::fallback::resolve_model_inheritance(
                Some(&asked),
                None,
                Some(&parent),
                &mut available,
                Some(&config),
                "worker",
            )
            .map_err(|refusal| refusal.message()),
            Ok(crate::exec::fallback::ModelOverride::Explicit(asked)),
            "`allow: [\"inherit\"]` must admit the model the parent session is actually on"
        );

        // CONTROL: another model is still refused, with pi's verbatim message.
        let other = cyrup_core::ModelId::from("anthropic/opus");
        let mut available = vec![other.clone()];
        let refusal = crate::exec::fallback::resolve_model_inheritance(
            Some(&other),
            None,
            Some(&parent),
            &mut available,
            Some(&config),
            "worker",
        )
        .expect_err("an out-of-scope explicit model must still be refused");
        assert_eq!(
            refusal.message(),
            "Model 'anthropic/opus' is outside the configured subagent model scope (modelScope). \
             Allowed patterns: openai/gpt-5.",
            "the refusal must name the EXPANDED allowlist, not the literal word `inherit`"
        );
    }

    /// SUBA-155 — an armed reserved token with no parent session model to expand against must
    /// REFUSE the launch and say why, which is pi's
    /// `throwForUnresolvedEnforcedReservedScope(options?.scope, explicit === undefined ||
    /// options?.source === "inherited")` at `model-resolution.ts:347` @v0.75.0.
    ///
    /// THE USER ACTION: the same operator's policy, on a host with no live session model
    /// (headless, an SDK embedder, or a session whose model is momentarily unbound). The token
    /// stays literal and matches nothing — but the only model that then gets checked is the
    /// INHERITED one, at warn severity, so the launch printed a warning and ran the persona's own
    /// model anyway. An armed policy that cannot be made concrete must fail CLOSED.
    #[test]
    fn an_armed_reserved_token_with_no_parent_session_refuses_the_launch_instead_of_warning() {
        let config = parsed(serde_json::json!({"enforce": true, "allow": ["inherit"]}));
        let persona = cyrup_core::ModelId::from("openai/gpt-5");
        let mut available = vec![persona.clone()];

        let refusal = crate::exec::fallback::resolve_model_inheritance(
            None,
            Some(&persona),
            None, // no parent session model
            &mut available,
            Some(&config),
            "worker",
        )
        .expect_err("an armed `inherit` with no parent session model must refuse the launch");
        assert_eq!(
            refusal.message(),
            "Cannot enforce subagent model scope (modelScope): 'inherit' requires a current parent \
             session model.",
            "the refusal must say WHY the policy cannot be enforced, not name a one-word allowlist"
        );
        assert_eq!(
            refusal.violation(),
            None,
            "there is no out-of-scope model here: the refusal fires before any model is resolved"
        );
        assert_eq!(
            available,
            vec![persona.clone()],
            "the refusal happens before resolution, so the availability set is untouched"
        );

        // `scoped` is the same refusal, naming itself.
        let scoped_only = parsed(serde_json::json!({"enforce": true, "allow": ["scoped"]}));
        assert_eq!(
            crate::exec::fallback::resolve_model_inheritance(
                None,
                Some(&persona),
                None,
                &mut available,
                Some(&scoped_only),
                "worker",
            )
            .err()
            .map(|refusal| refusal.message()),
            Some(
                "Cannot enforce subagent model scope (modelScope): 'scoped' requires a current \
                 parent session model."
                    .to_string()
            )
        );

        // CONTROL — upstream's `includeMixed` distinction: a list that is not ONLY a reserved
        // token still admits an explicitly requested model that matches one of its real patterns,
        // so a headless host does not become "refuse every launch".
        let mixed = parsed(serde_json::json!({"enforce": true, "allow": ["inherit", "openai/*"]}));
        let asked = cyrup_core::ModelId::from("openai/gpt-5");
        let mut available = vec![asked.clone()];
        assert_eq!(
            crate::exec::fallback::resolve_model_inheritance(
                Some(&asked),
                None,
                None,
                &mut available,
                Some(&mixed),
                "worker",
            )
            .map_err(|refusal| refusal.message()),
            Ok(crate::exec::fallback::ModelOverride::Explicit(asked)),
            "a mixed allowlist must still admit a model one of its real patterns covers"
        );
    }

    /// SUBA-155 — `modelScope.agents.<name>` must be HONOURED by the launch resolver, not accepted
    /// and dropped. The failure direction that matters for a policy knob is the silent one: an
    /// operator believes a named agent is restricted and it is not.
    #[test]
    fn a_per_agent_rule_refuses_at_launch_a_model_the_global_block_admits() {
        let config = parsed(serde_json::json!({
            "enforce": true,
            "allow": ["anthropic/*", "openai/*"],
            "agents": {"reviewer": {"allow": ["anthropic/*"]}}
        }));
        let asked = cyrup_core::ModelId::from("openai/gpt-5");

        // The unnamed agent sees the global list only, so `openai/*` resolves.
        let mut available = vec![asked.clone()];
        assert!(
            crate::exec::fallback::resolve_model_inheritance(
                Some(&asked),
                None,
                None,
                &mut available,
                Some(&config),
                "worker",
            )
            .is_ok(),
            "the global block admits openai/*, so an agent with no rule of its own runs"
        );

        // The named agent is refused BY NAME, and the message says which block refused.
        let mut available = vec![asked.clone()];
        let refusal = crate::exec::fallback::resolve_model_inheritance(
            Some(&asked),
            None,
            None,
            &mut available,
            Some(&config),
            "reviewer",
        )
        .expect_err("the per-agent rule must refuse a model the global rule allows");
        assert_eq!(
            refusal.message(),
            "Model 'openai/gpt-5' is outside the configured subagent model scope \
             (modelScope.agents.reviewer). Allowed patterns: anthropic/*.",
        );
    }

    /// A [`cyrup_ext::host::HostServices`] double reporting only a canned scoped-model snapshot,
    /// in the `{model: {provider, id}, thinkingLevel?}` shape the live backend emits
    /// (`cyrup-session-svc`'s `host_services.rs::scoped_models`, EXT-045).
    struct FixedScopedModelsHost(serde_json::Value);

    impl cyrup_ext::host::HostServices for FixedScopedModelsHost {
        fn scoped_models(&self) -> serde_json::Value {
            self.0.clone()
        }
    }

    /// SUBA-155 — `allow: ["scoped"]` must admit the parent session's scoped-model set, which
    /// means the snapshot has to be CAPTURED at launch: the resolver that enforces the policy runs
    /// (for a background run) in a process with no host at all.
    ///
    /// THE USER ACTION: an operator has narrowed this session with `/scoped-models` and wants
    /// subagents held to the same set, so writes `"allow": ["scoped"]`. With the snapshot never
    /// read, `scoped` could only ever degrade to `inherit` — admitting the parent's ONE current
    /// model and refusing every other model the operator deliberately kept in scope.
    #[test]
    fn the_launch_time_policy_captures_this_sessions_scoped_model_snapshot() {
        let dir = tempfile::tempdir().expect("real tempdir");
        crate::extension::testsupport::seed_scope_fixture(
            dir.path(),
            "worker",
            Some(r#"{"subagents":{"modelScope":{"enforce":true,"allow":["scoped"]}}}"#),
        );
        let roots = crate::paths::Roots::from_env();
        let executor = crate::extension::SubagentExecutor::new();

        // No host bound: nothing to capture, so the token survives for phase 2 to degrade.
        assert_eq!(
            executor
                .resolve_model_scope_for_launch(dir.path(), &roots)
                .expect("settings parse")
                .and_then(|policy| policy.allow),
            Some(vec![SCOPED_PATTERN.to_string()]),
            "with no host there is no snapshot, which must leave `scoped` to degrade to `inherit`"
        );

        // A scoped session: the whole set is captured into the policy.
        executor.set_host_services(std::sync::Arc::new(FixedScopedModelsHost(
            serde_json::json!([
                {"model": {"provider": "anthropic", "id": "opus"}, "thinkingLevel": "high"},
                {"model": {"provider": "openai", "id": "gpt-5"}}
            ]),
        )));
        let captured = executor
            .resolve_model_scope_for_launch(dir.path(), &roots)
            .expect("settings parse")
            .expect("a modelScope block is configured");
        assert_eq!(
            captured.allow.as_deref(),
            Some(&["anthropic/opus".to_string(), "openai/gpt-5".to_string()][..]),
            "`scoped` must resolve to the session's scoped-model set"
        );

        // And the captured policy polices: every scoped model resolves, an unscoped one does not,
        // even though the parent session's model is a THIRD model entirely.
        let parent = cyrup_core::ModelId::from("together/glm");
        for admitted in ["anthropic/opus", "openai/gpt-5"] {
            let asked = cyrup_core::ModelId::from(admitted);
            let mut available = vec![asked.clone()];
            assert!(
                crate::exec::fallback::resolve_model_inheritance(
                    Some(&asked),
                    None,
                    Some(&parent),
                    &mut available,
                    Some(&captured),
                    "worker",
                )
                .is_ok(),
                "{admitted} is in the scoped set and must resolve"
            );
        }
        let unscoped = cyrup_core::ModelId::from("openai/gpt-4");
        let mut available = vec![unscoped.clone()];
        assert_eq!(
            crate::exec::fallback::resolve_model_inheritance(
                Some(&unscoped),
                None,
                Some(&parent),
                &mut available,
                Some(&captured),
                "worker",
            )
            .err()
            .map(|refusal| refusal.message()),
            Some(
                "Model 'openai/gpt-4' is outside the configured subagent model scope (modelScope). \
                 Allowed patterns: anthropic/opus, openai/gpt-5."
                    .to_string()
            ),
            "CONTROL: a model outside the scoped set is still refused"
        );
    }
}
