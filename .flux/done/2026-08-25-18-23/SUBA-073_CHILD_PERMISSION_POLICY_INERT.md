---
stage: qa
status: completed
updated: 2026-08-27 05:30
severity: high
effort: medium
subsystem: config / permissions / discovery frontmatter
source: docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md
item: SUBA-073
---

# SUBA-073 — Subagent permission policy never reaches a spawned child: `config.permissions` and agent `permission:`/`permissions:` frontmatter are accepted and inert

**Kind** not-ported · **Severity** `high` (corrected DOWN from filed `critical`; see the note below — `critical` is not defensible, `high` is) · **Effort** M · **Confidence** confirmed
**Subsystem** config / permissions / discovery frontmatter
**Window** in-baseline (≤ v0.43.0) — `v0.43.0:src/runs/shared/permissions.ts` and `v0.43.0:src/shared/types.ts` both carry it.

**upstream** — `git show v0.57.0:src/shared/types.ts` **`:2268`** declares
`permissions?: PermissionConfig` on `ExtensionConfig`, documented at `:2267` as *"Opt-in native tool
permissions. Bash remains outside this policy."* `git show v0.57.0:src/runs/shared/permissions.ts`
(99 lines) defines `PERMISSION_POLICY_ENV = "PI_SUBAGENT_PERMISSION_POLICY"` (**`:8`**),
`validatePermissionRules` (**`:21`**), `validatePermissionConfig` (**`:35`**), `resolvePermissionRules`
(**`:44`**), `permissionDecision` (**`:50`**) and `encodePermissionRules` (**`:55`**).
`src/extension/config.ts` runs `validatePermissionConfig(config.permissions)` on every config read.
`git show v0.57.0:src/agents/agents.ts` **`:2033`** throws
``Agent '${localName}' cannot declare both permission and permissions frontmatter.`` and then parses
`frontmatter.permissions ?? frontmatter.permission` through `validatePermissionRules`;
`agent-serializer.ts` carries both spellings in `KNOWN_FIELDS`. `async-execution.ts`,
`api/preflight.ts` call `resolvePermissionRules(ctx.permissions, agentConfig.permissions)` and
`pi-args.ts` writes the encoded policy into the child env.

**cyrup** — `grep -rn 'PERMISSION_POLICY_ENV' crates/cyrup-ext-subagents/src/exec/ crates/cyrup-ext-subagents/src/spawn/`
→ **0 hits**; there is no writer anywhere in the workspace. Every hit crate-wide is a READ site: the
child-side gate `src/watchdog/permission_arbiter.rs:355` (cyrup's `CYRUP_SUBAGENT_*` spelling) and
`src/prompt_runtime.rs:1399,1442,2225-2227,2446,2467`. The crate states it in-tree at
`src/watchdog/permission_arbiter.rs:60-63`: *"The parent-side half (`validatePermissionConfig`,
`resolvePermissionRules`, `encodePermissionRules`, and `pi-args.ts:713-758`'s env writes) is still
unported, so a policy reaches a child today only if something outside this crate sets
`PERMISSION_POLICY_ENV`; that is the remaining work, and it lives in `exec/`, not here."* On the
frontmatter side, `src/discovery/frontmatter.rs:72-116 KNOWN_FIELDS` contains **neither** `permission`
nor `permissions` (grep for `permission` in that range: 0 hits), and the crate's own tests PIN the
demotion — `frontmatter.rs:1213-1216` asserts a `permission:` block lands in `extra_fields` and
`present_fields`. `SubagentExtensionConfig` (`src/registration/mod.rs:79-245`) has no `permissions`
key.

**Impact** — An operator who writes `{"permissions": {"rules": {"write": "deny"}}}` in subagent
config, or an agent author who writes `permission: {"*": ask, bash: {"*": ask, "git *": allow}}` in
an agent file, gets the value accepted with no error and silently not enforced: the child spawns with
no policy env var, `permission_arbiter`'s gate is never armed, and the denied tool runs. Upstream's
mutual-exclusion error for declaring both spellings is also absent. The child-side enforcement
machinery is fully ported and permanently unreachable.

**Severity note (correction applied).** Filed `critical`; corrected to `high` (medium is defensible
too, but the frontmatter half alone earns `high`) on three grounds read literally against
`README.md:510`. (1) This is not a bypass of an *enforcing* system: a cyrup subagent child is still
gated by `cyrup-permission-system`, wired into every spawn, with the child→parent ask-forwarding spool
live at `spawn/nested_events.rs:781`; upstream itself documents `permissions` as **opt-in** and leaves
bash to pi-guard. (2) Upstream's own normal state is "no policy, no gate" —
`resolvePermissionRules` returns `undefined` on an empty merged map and no handler is installed —
which is exactly the state cyrup is permanently in; the divergence is that cyrup cannot *leave* it.
(3) No data loss, no crash, no silent wrong output. `high` is defensible on the frontmatter half
alone: an agent file that literally reads `permission: {...}` is accepted, round-tripped through
`extra_fields`, re-serialized on rewrite and never enforced, with no diagnostic — and
`registration/authority.rs:22` states the crate's own principle that *"a config key that is parsed
and ignored is a permission bypass."* `critical` is not defensible given (1).

---

## Scope

In scope: the behaviour described above, in `crates/cyrup-ext-subagents/`.

Out of scope: the other SUBA items in this batch (each has its own file); any refactor beyond what
the fix needs; the ledger corrections in `SUBA-CORPUS-HEALTH.md`; and — newly identified during this
research pass — the separate, pre-existing gap that `SubagentExtensionConfig::validate_authority_policy`
/`validate_artifact_dir`/`validate_artifact_config` are defined but **never called** by the real
production loader (`crates/cyrup/src/subagent_config.rs`'s `load_subagent_extension_config` calls
only `validate_missions`). That loader lives in the `cyrup` crate, outside this task's declared scope,
and fixing it is a separate item. `validate_permission_config` (below) is written to the same,
already-established pattern as its three siblings for consistency, on the understanding that it will
be equally unwired in production until that separate item is done — this is a **pre-existing,
sibling-consistent characteristic**, not a regression this task introduces. It does not block this
task's own acceptance criteria, which route the effective policy through the LIVE two-rung merge at
run time (see the design below), not through config-load validation.

Full finding, with the complete evidence chain: [`09a-cyrup-ext-subagents-v0.57-drift.md` § SUBA-073](../../docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md)

---

## What already exists — REUSE, do not re-port

Confirmed by reading `crates/cyrup-ext-subagents/src/watchdog/permission_arbiter.rs` directly: the
**child-side half** of `permissions.ts` is already a faithful, tested port, living in that file (its
own module doc explains why: *"the policy decode/decision half of `runs/shared/permissions.ts` live
here, that file having no other cyrup port"*):

- `PERMISSION_POLICY_ENV: &str = "CYRUP_SUBAGENT_PERMISSION_POLICY"` (`:358`, `pub`)
- `PERMISSION_AUDIT_PATH_ENV: &str = "CYRUP_SUBAGENT_PERMISSION_AUDIT_PATH"` (`:361`, `pub`)
- `PermissionRuleDecision` enum (`Allow`/`Ask`/`Deny`, `:373-403`, `pub`, with `as_str()`/`parse()`)
- `PermissionRules = BTreeMap<String, PermissionRuleDecision>` type alias (`:406`, `pub`) — **no
  `Serialize`/`Deserialize` derive on `PermissionRuleDecision`**, so this is NOT a serde-driven type;
  every function in this family hand-walks `serde_json::Value` directly, matching upstream's own
  untyped-JS-object style. Any new code (config field, encoder) must do the same rather than
  reaching for `#[derive(Serialize)]`.
- `validate_permission_rules(value: Option<&Value>, label: &str) -> Result<Option<PermissionRules>, String>` (`:414-447`, `pub`) — **this is upstream's `validatePermissionRules`, already fully ported**, including all five refusals (non-object, empty tool name, `bash`, reserved internal tool, bad decision). **The task's own Fix text names this for porting into `exec/permissions.rs` — that is now WRONG; it already exists here and must be REUSED via `use crate::watchdog::permission_arbiter::validate_permission_rules;`, not redefined.**
- `decode_permission_rules(encoded: Option<&str>) -> Result<Option<PermissionRules>, String>` (`:455-462`, `pub`) — the CHILD's read side of `PERMISSION_POLICY_ENV`. Already wired to `prompt_runtime.rs`.
- `permission_decision(rules: Option<&PermissionRules>, tool_name: &str) -> PermissionRuleDecision` (`:471-481`, `pub`) — the enforcement lookup. Already wired.
- `redact_secret_values`, `permission_args_preview`, `append_permission_audit` — audit/logging helpers, already ported and wired.

Both `watchdog` and `permission_arbiter` are `pub mod` all the way from `lib.rs`
(`lib.rs:49 pub mod watchdog;`, `watchdog/mod.rs:52 pub mod permission_arbiter;`), so every item above
is reachable from anywhere in the crate as `crate::watchdog::permission_arbiter::...`.

**Genuinely missing** (confirmed absent by `grep -n 'fn validate_permission_config\|fn resolve_permission_rules\|fn encode_permission_rules' crates/cyrup-ext-subagents/src/watchdog/permission_arbiter.rs` → 0 hits): exactly **three** functions, all PARENT-side:

- `validatePermissionConfig` (`permissions.ts:35-41`) — validates the `{rules?}` WRAPPER shape (config-level, not agent-level) and rejects unknown top-level keys.
- `resolvePermissionRules` (`permissions.ts:44-47`) — merges global + agent rules (agent wins), strips `"allow"` entries (the default; no need to transmit it).
- `encodePermissionRules` (`permissions.ts:55-60`) — JSON-encodes the merged map for the env var, with upstream's 16 KiB cap.

---

## Design (traced against real cyrup call sites — this is the actual wiring, not a guess)

Upstream's own precedent for "how does a resolved-value-with-a-config-rung reach `pi-args.ts`" is
**exactly** what `SUBA-008`'s tool-budget port already does in this crate, confirmed by reading both
real call sites below. Follow that wiring precisely; do not invent a new mechanism.

### 1. New file: `crates/cyrup-ext-subagents/src/exec/permissions.rs`

```rust
//! SUBA-073 — the PARENT half of `pi-subagents/src/runs/shared/permissions.ts` (99 lines
//! @v0.43.0): resolving `config.permissions` + an agent's own frontmatter rules into the policy a
//! spawned child receives. The CHILD half (decode, per-tool decision, audit) is already ported at
//! [`crate::watchdog::permission_arbiter`] — this file reuses it rather than re-deriving it; see
//! that module's own doc for why the split exists.

use serde_json::Value;

use crate::watchdog::permission_arbiter::{PermissionRuleDecision, PermissionRules, validate_permission_rules};

/// `validatePermissionConfig(value, label)` (`permissions.ts:35-41`) — validates the CONFIG-LEVEL
/// `{rules?}` wrapper (`config.permissions`), distinct from [`validate_permission_rules`], which
/// validates a bare rules map (agent frontmatter has no wrapper). Returns the inner rules map
/// directly since `{rules?}` carries nothing else.
///
/// # Errors
///
/// `{label} must be an object.` for a non-object/array/null value; `{label} has unsupported
/// fields: <sorted, comma-joined>.` for any key besides `rules`; everything
/// [`validate_permission_rules`] itself raises for `.rules`.
///
/// [CYRUP-DELTA] the unsupported-fields list is SORTED here for deterministic error text; upstream
/// preserves JS object key insertion order. Cosmetic only — the set of named keys is identical.
pub fn validate_permission_config(
    value: Option<&Value>,
    label: &str,
) -> Result<Option<PermissionRules>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let Some(object) = value.as_object() else {
        return Err(format!("{label} must be an object."));
    };
    let mut unknown: Vec<&str> = object.keys().map(String::as_str).filter(|k| *k != "rules").collect();
    unknown.sort_unstable();
    if !unknown.is_empty() {
        return Err(format!("{label} has unsupported fields: {}.", unknown.join(", ")));
    }
    validate_permission_rules(object.get("rules"), &format!("{label}.rules"))
}

/// `resolvePermissionRules(globalConfig?, agentRules?)` (`permissions.ts:44-47`) — merge global and
/// agent-level rules, agent wins on conflict, then strip any `Allow` entries (the enforcement
/// default anyway — [`crate::watchdog::permission_arbiter::permission_decision`] falls back to
/// `Allow` for anything absent, so keeping an explicit `Allow` on the wire is pure overhead and
/// upstream never does).
#[must_use]
pub fn resolve_permission_rules(
    global: Option<&PermissionRules>,
    agent: Option<&PermissionRules>,
) -> Option<PermissionRules> {
    let mut merged: PermissionRules = global.cloned().unwrap_or_default();
    if let Some(agent) = agent {
        merged.extend(agent.iter().map(|(k, v)| (k.clone(), *v)));
    }
    merged.retain(|_, decision| *decision != PermissionRuleDecision::Allow);
    (!merged.is_empty()).then_some(merged)
}

/// pi's 16 KiB cap (`permissions.ts:12`, `MAX_POLICY_BYTES`).
const MAX_POLICY_BYTES: usize = 16 * 1024;

/// `encodePermissionRules(rules)` (`permissions.ts:55-60`) — JSON-encode the resolved rules for
/// [`crate::watchdog::permission_arbiter::PERMISSION_POLICY_ENV`]. `None`/empty encodes to `None`
/// (upstream: no env var written at all, not an empty-object value — see the `spawn_plan.rs` call
/// site, which only inserts the env key when this returns `Some`).
///
/// # Errors
///
/// `Resolved permission policy is too large.` when the encoded JSON exceeds 16 KiB — upstream's
/// verbatim text.
pub fn encode_permission_rules(rules: Option<&PermissionRules>) -> Result<Option<String>, String> {
    let Some(rules) = rules.filter(|r| !r.is_empty()) else {
        return Ok(None);
    };
    let object: serde_json::Map<String, Value> = rules
        .iter()
        .map(|(tool, decision)| (tool.clone(), Value::String(decision.as_str().to_string())))
        .collect();
    let encoded = serde_json::to_string(&Value::Object(object))
        .map_err(|e| format!("failed to encode permission policy: {e}"))?;
    if encoded.len() > MAX_POLICY_BYTES {
        return Err("Resolved permission policy is too large.".to_string());
    }
    Ok(Some(encoded))
}
```

Declare it in `exec/mod.rs` beside the other single-concern submodules: `pub mod permissions;`
(matches `pub mod turn_budget;`, `pub mod capability_ceiling;`'s own declaration style — check
`exec/mod.rs`'s existing `mod`/`pub mod` list for the exact neighboring lines and match their
visibility).

### 2. `SubagentExtensionConfig` — one new field (`registration/mod.rs`)

Add, next to `turn_budget`/`tool_description_mode` (`registration/mod.rs:225-230,240-246` — the
"carried RAW, validated at USE time" pair, chosen deliberately over the missions/authority_policy/
artifact_config "typed + validated at load time" pair because that second pattern's load-time wiring
is **already dead in production** for all three existing members — see the Scope section above; do
not add a fourth field to a validation pathway that provably never runs):

```rust
/// pi `ExtensionConfig.permissions?: PermissionConfig` (`shared/types.ts:2268` @v0.57.0, *"Opt-in
/// native tool permissions. Bash remains outside this policy."*). Carried RAW, exactly like
/// [`Self::turn_budget`] and for the same reason: validated at the point of use
/// ([`crate::exec::permissions::validate_permission_config`]) rather than at config load, so a
/// malformed block degrades that one resolution rather than discarding the whole config file.
///
/// `None` (the key omitted) means no global policy rung — the effective policy is then whatever
/// the agent's own frontmatter declares, or no policy at all.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub permissions: Option<serde_json::Value>,
```

Add `permissions: None,` to `SubagentExtensionConfig::default()`'s literal (or wherever the
`#[derive(Default)]`/manual `Default` impl lives — check whether the struct derives `Default` or
implements it by hand before writing this line).

### 3. Agent-level frontmatter — `discovery/frontmatter.rs`

Add both spellings to `KNOWN_FIELDS` (`:72-116`), next to the existing `alias`/`aliases` pair with
the SAME "why both must be here" comment style already established there:

```rust
"permission",
"permissions",
```

Parse them where `turnBudget` is parsed (`:878-915` — the exact idiom: `parsed.get(KEY)`, a per-file
WARN + `return None` on any validation failure, never a hard `Result::Err` bubble-up, matching this
function's own `[CYRUP-DELTA]` convention explained inline at the `toolBudget`/`turnBudget` sites).
Both spellings are FLAT rules maps (`{tool: decision}`), not the `{rules: {...}}` wrapper
`config.permissions` uses — so this calls
[`crate::watchdog::permission_arbiter::validate_permission_rules`] DIRECTLY, never
`validate_permission_config` (that wrapper-validator is for the extension-config side only):

```rust
// SUBA-073 `permission:`/`permissions:` — pi `agents.ts:2033` @v0.57.0: mutual exclusion is a
// hard refusal (both spellings present is always an authoring mistake, never "prefer one"),
// then whichever ONE is present is parsed through validate_permission_rules exactly like an
// agent-level `toolBudget:`/`turnBudget:` block above. Same [CYRUP-DELTA] as those: a per-file
// skip + warn instead of aborting the whole directory scan.
let has_permission = parsed.get("permission").filter(|v| !v.trim().is_empty()).is_some();
let has_permissions = parsed.get("permissions").filter(|v| !v.trim().is_empty()).is_some();
if has_permission && has_permissions {
    tracing::warn!(
        agent = %local_name,
        path = %file_path.display(),
        "Agent '{local_name}' cannot declare both permission and permissions frontmatter. — skipping this agent file"
    );
    return None;
}
let permission_rules = match parsed.get("permissions").or_else(|| parsed.get("permission")) {
    None => None,
    Some(raw) => {
        let parsed_json = serde_json::from_str::<serde_json::Value>(raw).map_err(|err| {
            format!("Agent '{local_name}' permission frontmatter must be a JSON object mapping tool names to allow, ask, or deny. ({err})")
        });
        match parsed_json.and_then(|value| {
            crate::watchdog::permission_arbiter::validate_permission_rules(
                Some(&value),
                &format!("Agent '{local_name}' permission frontmatter"),
            )
            .map_err(|e| e)
        }) {
            Ok(rules) => rules,
            Err(message) => {
                tracing::warn!(
                    agent = %local_name,
                    path = %file_path.display(),
                    "{message} — skipping this agent file"
                );
                return None;
            }
        }
    }
};
```

(The exact error-message wording for a non-JSON value is this task's own choice — no upstream
citation constrains it beyond the JSON-parse failure existing at all; the pattern above mirrors
`turnBudget`'s own phrasing for consistency.)

### 4. `AgentDefinition` — one new field (`discovery/types.rs`)

Add next to `default_turn_budget` (`:817`, same section/style):

```rust
/// SUBA-073 — this agent's own `permission:`/`permissions:` frontmatter, already validated
/// (`discovery/frontmatter.rs`). Merged with the global `config.permissions` rung at run time via
/// [`crate::exec::permissions::resolve_permission_rules`] — this field alone is NOT the effective
/// policy, only this agent's contribution to it.
pub permission_rules: Option<crate::watchdog::permission_arbiter::PermissionRules>,
```

Add `permission_rules: None,` to `AgentDefinition`'s `Default` impl (`:1080`-area) and to the
`discovery/frontmatter.rs` parse function's final struct literal (wherever `default_turn_budget:
default_turn_budget,` is assigned, add `permission_rules,` beside it).

`cargo check` will then name every other `AgentDefinition { ... }` struct literal in the crate that
needs the new field — most already use `..Default::default()`/`..existing` spread and need no edit;
a handful of hand-written test fixtures will need `permission_rules: None,` added. This is expected,
mechanical fallout of adding a struct field, not a design decision.

**No changes needed in `discovery/merge.rs`**: confirmed by reading it — the tiered agent merge picks
one whole winning `AgentDefinition` per name across tiers, it does not field-merge across tiers, and
`default_turn_budget`'s own zero special-casing there (`grep -n tool_budget discovery/merge.rs` →
one `None` in a test fixture, nothing else) is the precedent this field follows identically.

### 5. Round-trip write-back — `discovery/management/frontmatter_write.rs`

**Required**, not optional: this crate's own established rule (stated inline at both the
`toolBudget` and `turnBudget` sites, `:227-230,240-243`) is that adding a key to `KNOWN_FIELDS`
without also emitting it from the serializer SILENTLY DELETES it from an agent file on the first
management-tool rewrite (`update_agent`/etc.), because the "extra fields" round-trip loop skips
every key `is_known_field` recognizes. Add, mirroring the `turnBudget` block exactly (`:240-255`):

```rust
// SUBA-073 permission/permissions (`agent-serializer.ts` @v0.57.0): emitted as compact JSON
// under the canonical `permissions:` key — matching `aliases`'s own precedent of always writing
// the newer/plural spelling regardless of which the original file used — or as an empty value
// under preserve. Same silent-deletion trap as `toolBudget`/`turnBudget` above, now that
// `permission`/`permissions` are both in `KNOWN_FIELDS`.
if def.permission_rules.is_some() || preserve(&["permission", "permissions"]) {
    let value = def
        .permission_rules
        .as_ref()
        .map(|rules| {
            let object: serde_json::Map<String, serde_json::Value> = rules
                .iter()
                .map(|(tool, decision)| (tool.clone(), serde_json::Value::String(decision.as_str().to_string())))
                .collect();
            serde_json::to_string(&serde_json::Value::Object(object)).unwrap_or_default()
        })
        .unwrap_or_default();
    lines.push(format!("permissions: {value}"));
}
```

Verify `preserve(&[...])`'s exact signature first (`frontmatter_write.rs`'s existing calls all pass
a single-element slice, e.g. `preserve(&["toolBudget"])` — confirm it accepts a multi-element slice
for the two-spelling case before assuming the call above compiles as written; if it only accepts one
key, call it twice with `||` instead: `preserve(&["permission"]) || preserve(&["permissions"])`).

### 6. `AgentConfig` — one new field (`exec/agent_config.rs`)

**Not `RunOptions`.** Traced both existing dual-rung ported values (`tool_budget`, `turn_budget`) to
their real call sites: `turn_budget` lands on `RunOptions` because pi resolves it once, at the tool
boundary, layering CALLER-override on top; `tool_budget` lands on `AgentConfig`
(`exec/agent_config.rs:98,126`) because it has no caller-override tier — it is purely the resolved
persona value, read directly off `AgentConfig` at the spawn site
(`spawn_plan.rs:999 agent.tool_budget.as_ref()`). Permissions has NO caller-override tier either
(`resolvePermissionRules(globalConfig?, agentRules?)` takes exactly two inputs, neither of them a
per-call param — confirmed: no `permissions` key exists anywhere in the subagent tool's own param
schema). It is structurally `tool_budget`'s twin, not `turn_budget`'s — put it on `AgentConfig`:

```rust
/// SUBA-073 — the FULLY MERGED policy (global `config.permissions` + this agent's own frontmatter,
/// via [`crate::exec::permissions::resolve_permission_rules`]) this attempt's child receives.
/// Resolved once, by the caller of [`AgentConfig::from_agent_definition`]
/// (`extension/executor/foreground.rs`/`background.rs`), the same seam that resolves `tool_budget`
/// — see [`Self::tool_budget`]'s own doc for why this lands here and not on `RunOptions`.
pub permission_rules: Option<crate::watchdog::permission_arbiter::PermissionRules>,
```

`AgentConfig::from_agent_definition` (`:126`-area) seeds it from the agent's OWN value alone:
`permission_rules: agent.permission_rules.clone(),` — matching `tool_budget: agent.tool_budget.clone(),`
on the same line. The GLOBAL rung is layered on immediately after construction, at both call sites
below (mirroring `agent_config.tool_budget = Some(budget);`'s override-after-construction shape at
`foreground.rs:169-171`, except this is a MERGE, not a replace).

### 7. The two resolution call sites (confirmed by reading both directly)

**`extension/executor/foreground.rs`**, right after `let mut agent_config =
AgentConfig::from_agent_definition(&agent, depth);` (`:162`) and before or after the `tool_budget`
override block (`:163-171`) — `cfg` (`self.config_snapshot().await`, `:139`) and `agent` (the
`AgentDefinition`, `:151-152`) are both already in scope here:

```rust
// SUBA-073 / pi `resolvePermissionRules(ctx.config?.permissions, agentConfig.permissions)`
// (`async-execution.ts`, `api/preflight.ts`): merge the extension config's global rung with this
// agent's own frontmatter rung — agent wins on conflict, `allow` entries stripped. A malformed
// `config.permissions` refuses THIS call with upstream's own message, exactly like the
// `turnBudget` config rung three lines below handles a malformed `subagents.turnBudget`.
let global_permission_rules = crate::exec::permissions::validate_permission_config(
    cfg.permissions.as_ref(),
    "config.permissions",
)
.map_err(SubagentError::Management)?;
agent_config.permission_rules = crate::exec::permissions::resolve_permission_rules(
    global_permission_rules.as_ref(),
    agent.permission_rules.as_ref(),
);
```

**`extension/executor/background.rs`**, right after the equivalent `AgentConfig::from_agent_definition`
call on that path (search for it — this file's `turn_budget` merge at `:241-246` is the sibling
seam; `agent`/`cfg` are both in scope there too since `turn_budget: match turn_budget.or(agent.default_turn_budget) { ... crate::exec::turn_budget::resolve_turn_budget_config(cfg.turn_budget.as_ref(), ...) ... }` already reads both from this exact scope):

```rust
let global_permission_rules = crate::exec::permissions::validate_permission_config(
    cfg.permissions.as_ref(),
    "config.permissions",
)
.map_err(SubagentError::Management)?;
agent_config.permission_rules = crate::exec::permissions::resolve_permission_rules(
    global_permission_rules.as_ref(),
    agent.permission_rules.as_ref(),
);
```

Confirm the exact variable names (`agent_config`/`agent`/`cfg`) at this second site before writing —
`background.rs`'s construction path may name them slightly differently than `foreground.rs`'s; read
the surrounding ~40 lines first the way this research pass did for `foreground.rs:123-230`.

If a THIRD run-mode entry point independently constructs its own `AgentConfig` without routing
through either of these two functions (chain-mode dynamic fan-out, nested background steps that
build a fresh `AgentConfig` per step rather than propagating one already-built one) — check for this
before declaring done; upstream's own citation only names `async-execution.ts` and
`api/preflight.ts`, which is the direct precedent for "exactly these two, and no others," but verify
against cyrup's actual call graph rather than assuming the counts match 1:1.

### 8. The env write — `exec/spawn_plan.rs`, beside the tool-budget encoder

Insert immediately after the existing tool-budget block (`:992-1005`, confirmed the literal location
the task's Fix text means by "beside the existing tool-budget encoder"):

```rust
// SUBA-073 — pi ships the resolved permission policy to the child in `PERMISSION_POLICY_ENV`
// (`pi-args.ts:938`); the child-side `watchdog::permission_arbiter`/`prompt_runtime` gate already
// decodes and enforces it. Same hand-off shape as the tool budget immediately above. Absent
// policy => no var, so a child cannot silently inherit a STALE policy from the parent's own
// environment (the overlay only ever adds — same rule as every other member of this family).
//
// pi ALSO writes `PERMISSION_AUDIT_PATH_ENV` whenever a policy is present (`pi-args.ts:905-906`),
// defaulting to `<tempDir>/permission-audit.jsonl` when the caller supplied no explicit path —
// this crate has no per-call override for the audit path today, so it always takes that default,
// using the SAME `temp_dir` this function already receives for the persona-body/task spill files.
match crate::exec::permissions::encode_permission_rules(agent.permission_rules.as_ref())
    .map_err(SubagentError::CapabilityCeilingViolation)?
{
    Some(encoded) => {
        env_overlay.insert(
            crate::watchdog::permission_arbiter::PERMISSION_AUDIT_PATH_ENV.to_string(),
            temp_dir.join("permission-audit.jsonl").display().to_string(),
        );
        env_overlay.insert(
            crate::watchdog::permission_arbiter::PERMISSION_POLICY_ENV.to_string(),
            encoded,
        );
    }
    None => {}
}
```

**Error-variant note**: `SubagentError::CapabilityCeilingViolation` above is almost certainly the
WRONG variant name for a permission-encoding failure — it is reused here only because this research
pass did not enumerate every `SubagentError` variant. Check `crate::error::SubagentError`'s
definition before writing this; use whichever existing variant is the generic "a resolved value
could not be prepared for spawn, and the launch must fail rather than silently drop the
restriction" case (the task's own Fix text: *"Treat a declared restriction that cannot be expressed
to the child as a hard launch error rather than a silent widening"* — this IS that hard-error path,
for the one case `encode_permission_rules` can fail: a policy over 16 KiB). If no existing variant
fits cleanly, `SubagentError::Management(String)` (seen used elsewhere in this same file for
config-resolution failures, e.g. the `turnBudget` config-rung error at `foreground.rs:191`) is the
safe, precedented fallback.

---

## Verify

- A child launched under `{"permissions":{"rules":{"write":"deny"}}}` (no per-agent rules) must have
  `CYRUP_SUBAGENT_PERMISSION_POLICY` set in its spawn env overlay (assert via
  `plan.spec.env_overlay.get(...)`, decoded with
  `crate::watchdog::permission_arbiter::decode_permission_rules`) and the decoded map must contain
  `"write" -> Deny`.
- An agent declaring `permission: {"bash": "ask"}` in frontmatter must FAIL TO LOAD at discovery time
  with `Agent '<name>' permission frontmatter.bash is unsupported; pi-subagents leaves bash policy to
  pi-guard.` (verbatim from the already-ported `validate_permission_rules`) — this is a real,
  already-implemented refusal path; write a discovery-level test pinning it, not just a spawn-level
  one.
- An agent declaring BOTH `permission:` and `permissions:` frontmatter keys must fail to load with
  `Agent '<name>' cannot declare both permission and permissions frontmatter.`
- An agent-level rule must override the same tool's global rule (agent `"write": "allow"` over global
  `"write": "deny"` must resolve to NO entry for `write` at all, since `resolve_permission_rules`
  strips `Allow` — assert the encoded env var, if written at all for other rules, does not mention
  `write`).
- `cargo test -p cyrup-ext-subagents` passes.

## Acceptance Criteria

- [ ] A `config.permissions` restriction demonstrably applies to a spawned child (env var present, decodes to the expected rules)
- [ ] Agent frontmatter `permission:` / `permissions:` reaches the child (merged into the same env var; agent overrides global on conflict)
- [ ] Declaring both `permission:` and `permissions:` on one agent fails discovery with upstream's exact message
- [ ] A declared restriction that cannot be honoured (the 16 KiB cap) fails the launch instead of running unrestricted
- [ ] `permission`/`permissions` round-trip through `discovery/management`'s agent update path without being silently dropped (the `KNOWN_FIELDS`-without-a-serializer-arm trap)
- [ ] `cargo test -p cyrup-ext-subagents` passes
