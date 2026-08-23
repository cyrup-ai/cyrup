---
stage: exec
status: done
updated: 2026-08-23 00:00
---

# Test-Only Builder with_native_supervisor_channel Has Zero Callers, Leaving Its Branch Untested

> Source: `intercom-hygiene-audit` workflow. Severity **low**, effort **medium**.
> Every claim below was produced by a finder agent and then reproduced by an independent
> adversarial verifier; findings that did not reproduce were dropped.

## Scope

- `crates/cyrup-intercom/src/extension.rs`

## Description

`crates/cyrup-intercom/src/extension.rs:143` defines `pub fn with_native_supervisor_channel(mut
self, available: bool) -> Self`, documented at :141-142 as existing to "Override the
`nativeSupervisorChannelAvailable` probe (`v0.10.1 index.ts:1504`) instead of reading the process
environment — for tests, which must not mutate process-global env state." It has zero callers
anywhere in the workspace. I enumerated every `pub fn` name declared in the crate's src (200
distinct names) and counted workspace-wide `grep -rw` references for each; exactly three came back
with a single reference, i.e. their own definition: `is_broker_running`
(src/transport/spawn.rs:278), `list_sessions_with_timeout` (src/transport/client.rs:532), and this
one. The first two are legitimate 1:1 port surface, each carrying an upstream citation for an
exported upstream function (`spawn.ts:243-259` at spawn.rs:277, `v0.10.1 broker/client.ts:581` at
client.rs:527) — fidelity, not debt. This third is different: it is not a port of anything
upstream, it is a Rust-side test seam, and the tests it was built for were never written. The
field it sets, `native_supervisor_channel` (declared :97), is otherwise assigned only at :133 from
`crate::identity::native_supervisor_channel_available()`, which reads process env
(identity.rs:73-74 over `ENV_SUPERVISOR_CHANNEL_DIR = "CYRUP_SUBAGENT_SUPERVISOR_CHANNEL_DIR"`,
identity.rs:62). It is read at exactly one place, extension.rs:452 (`&&
!self.native_supervisor_channel`), gating whether `ContactSupervisorTool` is registered — the
guard whose comment at :445-449 explains that a child on the native channel must not also get the
legacy broker-routed tool, "so the same decision can be requested through two mechanisms while the
parent polls only one of them."

## Why it matters

The doc comment asserts a test harness that does not exist, so a reader takes the extension.rs:452
gate to be covered when neither arm is: no test in the workspace constructs an `IntercomExtension`
with `native_supervisor_channel == true`. The failure this guards is silent and expensive — a
child handed both the native channel and the legacy `ContactSupervisorTool` can ask the same
decision through two mechanisms while the parent polls only one, i.e. a hang, not an error.

## Evidence

- /home/user/cyrup/crates/cyrup-intercom/src/extension.rs:141-145 — doc "for tests, which must not mutate process-global env state"; `#[must_use] pub fn with_native_supervisor_channel(mut self, available: bool) -> Self`
- `grep -rwn 'with_native_supervisor_channel' --include=*.rs .` (target/ excluded) → 1 hit, extension.rs:143 (the definition)
- Enumeration I re-ran: `grep -rhoP '^\s*pub (?:async )?(?:const )?fn \K\w+' crates/cyrup-intercom/src --include=*.rs | sort -u` → 200 distinct names; looping `grep -rw <name> crates/ | grep -v target | wc -l` over all 200 yielded exactly three names with count <= 1: is_broker_running, list_sessions_with_timeout, with_native_supervisor_channel
- /home/user/cyrup/crates/cyrup-intercom/src/transport/spawn.rs:277-278 — `/// \`isBrokerRunning\` (\`spawn.ts:243-259\`) ...` above `pub async fn is_broker_running` (port surface, correctly excluded)
- /home/user/cyrup/crates/cyrup-intercom/src/transport/client.rs:527-532 — `v0.10.1 broker/client.ts:581` citation above `pub async fn list_sessions_with_timeout` (port surface, correctly excluded)
- `grep -n 'native_supervisor_channel' crates/cyrup-intercom/src/extension.rs` → 97 (field decl), 133 (only other assignment, from identity::native_supervisor_channel_available()), 143-144 (the builder), 452 (the sole read)
- /home/user/cyrup/crates/cyrup-intercom/src/identity.rs:62,67-68,73-74 — `ENV_SUPERVISOR_CHANNEL_DIR = "CYRUP_SUBAGENT_SUPERVISOR_CHANNEL_DIR"`, and `native_supervisor_channel_available()` delegating to the `_from` closure over `std::env::var`
- Coverage stops at the probe, not the branch: identity.rs:607-611 tests `native_supervisor_channel_available_from` on closures only. `grep -rln 'CYRUP_SUBAGENT_SUPERVISOR_CHANNEL_DIR' --include=*.rs` (target/ excluded) → 4 files (cyrup-it/tests/subagents/native_supervisor_channel_integration.rs, cyrup-ext-subagents/src/spawn/intercom_target.rs, cyrup-ext-subagents/src/native_supervisor.rs, cyrup-intercom/src/identity.rs); grepping native_supervisor_channel_integration.rs for `IntercomExtension` returns nothing, and none of the nine `IntercomExtension::new` call sites in crates/cyrup-it/tests/intercom/ sets the flag

## Required fix

Collect on the seam rather than delete it — its stated purpose is sound, it just has no consumer.
Add a `#[cfg(test)]` test in extension.rs that builds an `IntercomExtension` with
`Some(ChildOrchestratorMetadata)`, applies `.with_native_supervisor_channel(true)` and then
`(false)`, drives `init(&mut InitApi)` for each, and asserts `ContactSupervisorTool` is registered
in the `false` case and absent in the `true` case — pinning both arms of extension.rs:452 without
any `std::env::set_var`. If instead the decision is that the branch does not warrant a test,
delete extension.rs:141-145 and fold the rationale into the existing comment at :445-449; do not
leave a builder whose doc promises a test harness that does not exist. Leave `is_broker_running`
and `list_sessions_with_timeout` alone — they are upstream-cited port surface.

## Acceptance Criteria

- [ ] The fix above applied as written; no scope beyond it.
- [ ] Port fidelity preserved — no `broker.ts`/`client.ts`/`paths.ts` citation dropped, no
      `[CYRUP-DELTA]` note removed, no ported branch collapsed.
- [ ] Baseline recorded before the change and matched after:
      `cargo clippy -p cyrup-intercom --all-targets`, `cargo test -p cyrup-intercom --lib`.
- [ ] `cargo build -p cyrup` still succeeds.
