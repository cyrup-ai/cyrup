//! The environment-variable contract this extension reads, and every probe that reads it.
//!
//! The four keys ([`CHILD_ENV_VAR`], [`SUBAGENT_ENV_HINT_KEYS`], [`INSTALL_ENV_VAR`],
//! [`POLICY_AGENT_DIR_ENV_KEY`]) plus the role/persona probes they answer: is this process a
//! subagent CHILD ([`is_subagent_child`]), and what persona was it spawned as
//! ([`resolve_agent_name_from_env`]).

/// The subagent-child env flag (value `"1"`) — literally the SAME const `cyrup-ext-subagents`
/// writes into every spawned child's env overlay
/// ([`cyrup_ext_subagents::spawn::nested_events::child_role_env`], driven from
/// `exec::build_attempt_spawn_plan`).
///
/// Aliased rather than re-typed as a literal: this crate ALREADY depends on `cyrup-ext-subagents`
/// (P-5, see `Cargo.toml`) and `ask.rs` already reads
/// `cyrup_ext_subagents::PARENT_SESSION_ENV_VAR` through that dependency, so the duplicate string
/// bought nothing and could silently drift out of agreement with the writer — which is exactly the
/// failure mode PERM-001 was: the gate read a name nothing on the spawn path ever wrote.
pub const CHILD_ENV_VAR: &str = cyrup_ext_subagents::spawn::nested_events::CHILD_ENV;

/// pi `SUBAGENT_ENV_HINT_KEYS` (`permission-forwarding.ts:9`) — the env keys whose presence means
/// "this process is running as a subagent child", ORed on any NON-EMPTY value by
/// [`is_subagent_child`] (pi `hasSubagentEnvHint`, `index.ts:93-103`).
///
/// The cyrup analogs, in upstream order, all three written into every child's spawn overlay by the
/// single chokepoint `cyrup_ext_subagents::exec::build_attempt_spawn_plan` (and aliased from that
/// crate rather than re-typed, for the same anti-drift reason as [`CHILD_ENV_VAR`]):
///
/// | pi | cyrup | what writes it |
/// |---|---|---|
/// | `PI_IS_SUBAGENT` | `CYRUP_SUBAGENT_CHILD` | `nested_events::child_role_env`, on EVERY spawn |
/// | `PI_SUBAGENT_SESSION_ID` | `CYRUP_SUBAGENT_RUN_ID` | the run-identity overlay, when the spawn belongs to a run |
/// | `PI_AGENT_ROUTER_SUBAGENT` | `CYRUP_SUBAGENT_AGENT_NAME` | the resolved persona name, when non-blank |
///
/// A ROOT orchestrator has none of them; the detached hop-2 `__subagent-runner` process has none
/// of them either (its hop-1 spawn overlays only the R-SA-P1 anchor), so it correctly keeps the
/// PARENT role and can host the forwarding watcher.
pub const SUBAGENT_ENV_HINT_KEYS: [&str; 3] = [
    CHILD_ENV_VAR,
    cyrup_ext_subagents::spawn::nested_events::RUN_ID_ENV,
    cyrup_ext_subagents::AGENT_NAME_ENV_VAR,
];

/// The explicit opt-in flag (DI-5): set truthy to force-install the gate even with no policy file.
pub const INSTALL_ENV_VAR: &str = "CYRUP_PERMISSION_SYSTEM";

/// pi `PERMISSION_POLICY_AGENT_DIR_ENV_KEY = "PI_PERMISSION_SYSTEM_POLICY_AGENT_DIR"`
/// (v0.8.0 `permission-manager.ts:29`), renamed to this crate's `CYRUP_` env-var convention (see
/// [`INSTALL_ENV_VAR`], [`crate::ext_config::CONFIG_PATH_ENV_KEY`],
/// `forwarding::FORWARDING_AGENT_DIR_ENV`).
///
/// Relocates the **global policy root** — the directory the four global policy artifacts live in
/// (`cyrup-permissions.jsonc`, `agents/`, `settings.json`, `mcp.json`). It does NOT move the
/// project-scoped `<cwd>/.cyrup/agent` tree, matching upstream: `createPermissionManagerForCwd`
/// (`index.ts:1287-1301`) supplies only `projectGlobalConfigPath` / `projectAgentsDir`, so every
/// GLOBAL path in a live session falls back to `defaultPolicyAgentDir()` (`:31-38`).
pub const POLICY_AGENT_DIR_ENV_KEY: &str = "CYRUP_PERMISSION_SYSTEM_POLICY_AGENT_DIR";

/// pi `resolveAgentName` (`index.ts:2033-2047`) for cyrup's process-per-subagent model: the resolved
/// persona name this process was spawned as, read from the `CYRUP_SUBAGENT_AGENT_NAME` env var
/// (`cyrup_ext_subagents::AGENT_NAME_ENV_VAR`) — captured ONCE (the child IS its persona for its whole
/// lifetime), the exact equivalent of pi's in-process `active_agent` session entry / `<active_agent>`
/// prompt tag for a separate-process subagent. Trimmed; empty/absent → `None` (pi `normalizeAgentName`
/// null-normalization + the normalized-`""` top-level: a top-level process has no such var, so the
/// agent + projectAgent layers no-op and global + project still enforce). This is the SAME
/// `std::env::var` pattern the crate already uses for the sibling `CYRUP_SUBAGENT_*` anchors
/// (`ask.rs` `PARENT_SESSION_ENV_VAR`, `is_subagent_child`).
pub(super) fn resolve_agent_name_from_env() -> Option<String> {
    std::env::var(cyrup_ext_subagents::AGENT_NAME_ENV_VAR)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// pi `hasSubagentEnvHint` (`index.ts:93-103`, over
/// `permission-forwarding.ts:9`'s `SUBAGENT_ENV_HINT_KEYS`): this process is running AS a subagent
/// child if ANY of the hint keys is set to a non-empty (post-trim) value.
///
/// Three deliberate points of fidelity to upstream:
///
/// - **Any of three keys, not one.** pi ORs `PI_IS_SUBAGENT` / `PI_SUBAGENT_SESSION_ID` /
///   `PI_AGENT_ROUTER_SUBAGENT`; [`SUBAGENT_ENV_HINT_KEYS`] is the cyrup analog set, all three
///   written by the same spawn chokepoint (`exec::build_attempt_spawn_plan`).
/// - **Non-empty, not `== "1"`.** pi tests `entry.length > 0`. The old strict `== Some("1")`
///   silently classified a child spawned by any path that wrote a different truthy value (or by an
///   external router setting only the persona/run keys) as a ROOT — which selected the LOCAL ask
///   dialog in a process with no human attached, so its `ask` died instead of forwarding.
/// - **Trimmed.** pi's `process.env[key]?.trim() ?? ""`.
///
/// Not ported: pi's `subagent-sessions` session-directory containment fallback
/// (`index.ts:696-709`). That branch keys on pi's in-process subagent sessions living under a
/// dedicated directory of the agent dir; cyrup's subagent is always a separate OS process carrying
/// these env keys (`lib.rs`'s non-negotiable process-per-subagent mechanism), so there is no
/// same-process session-dir signal to test. Note also that pi's `isSubagentExecutionContext` is a
/// per-`ctx` RUNTIME predicate while this is consulted both at wiring time
/// ([`crate::permission_extension_for_env`]) and per call — the env keys are process-lifetime constants in
/// cyrup, so the two coincide.
pub(super) fn is_subagent_child() -> bool {
    has_subagent_env_hint(|key| std::env::var(key).ok())
}

/// The injectable core of [`is_subagent_child`] — pi `hasSubagentEnvHint`'s body
/// (`index.ts:100`, `values.some((entry) => entry.length > 0)` over the trimmed values).
///
/// Parameterized over the env reader so the predicate is directly testable without
/// `unsafe { std::env::set_var }` and the cross-test races a process-global mutation brings, the
/// same injectable-core convention `cyrup-ext-subagents`' `spawn::depth`/`spawn::mod` use.
///
/// Not ported: pi caches the answer keyed on a `\0`-joined signature of the values
/// (`index.ts:94-102`). That cache exists because pi re-evaluates this on every `ctx` predicate
/// call inside a hot per-tool-call path; cyrup consults it at wiring time plus once per ask, over
/// three `getenv`s, so a cache would be pure complexity — and a stale one is a correctness hazard.
pub(super) fn has_subagent_env_hint(get: impl Fn(&str) -> Option<String>) -> bool {
    SUBAGENT_ENV_HINT_KEYS
        .iter()
        .any(|key| get(key).is_some_and(|value| !value.trim().is_empty()))
}

pub(super) fn env_truthy(name: &str) -> bool {
    matches!(
        std::env::var(name).ok().as_deref().map(str::trim),
        Some("1") | Some("true") | Some("on") | Some("yes")
    )
}
