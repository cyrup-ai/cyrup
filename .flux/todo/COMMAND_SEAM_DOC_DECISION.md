---
stage: new
status: done
updated: 2026-08-22 19:32
---

# Settle src/command.rs And Its False Adapter-Seam Claim

**Crate:** `crates/cyrup-session-svc` · **Severity:** medium · **Effort:** medium

## Description

`src/command.rs` (196 lines) declares `pub enum SessionCommand` at :24 with 33 verbs, `pub enum SessionCommandOutput` at :66 with 15 variants, and `pub async fn execute` at :96, and its module doc at :1-4 asserts "the central invariant of the subsystem is that no mode reaches behaviour that does not flow through `SessionCommand`/`AgentSession`", repeated at :93-94 as "Every adapter routes here so behaviour cannot diverge per front-end." That is false: `rg -n '\.execute\(' crates/ --glob '*.rs'` returns zero hits anywhere in `crates/cyrup-modes/`, which declares its own independent `SessionCommand` at `crates/cyrup-modes/src/rpc.rs:84` and dispatches it to facade methods directly. `SessionCommandOutput` has no consumer outside this crate at all, and the only exercises are two in-crate tests (src/tests/integration.rs:519-568, src/tests/round5.rs:293-330). Meanwhile `crates/cyrup-sdk/src/lib.rs:102` re-exports `cyrup_modes::SessionCommand` while :114 makes the unrelated `cyrup_sdk::session_svc::SessionCommand` reachable in the same SDK. The load-bearing harm is the doc: a maintainer adding a verb reads it and keeps the 33-arm dispatch table in lockstep forever for zero non-test consumers. Fixing the doc is cheap and mandatory; deleting versus keeping the module is a decision the reviewer must record either way.

## Acceptance Criteria

- [ ] `rg -n 'every adapter|Every adapter|no mode reaches behaviour' crates/cyrup-session-svc/src/` returns nothing — the claims at command.rs:1-4, :93-94 and the aside at session/bash.rs:53 are struck or rewritten to match reality.
- [ ] One of two outcomes is landed and stated in the PR: (a) `src/command.rs`, `mod command;` (lib.rs:24), `pub use command::{SessionCommand, SessionCommandOutput};` (lib.rs:51) and the two tests at src/tests/integration.rs:519-568 and src/tests/round5.rs:293-330 are deleted; or (b) the module is kept and its doc now describes it as a deliberate embedder-facing command API, explicitly not the mandatory adapter seam.
- [ ] If kept, the doc names the collision hazard between `cyrup_sdk::SessionCommand` (from cyrup-modes) and `cyrup_sdk::session_svc::SessionCommand`.
- [ ] `rg -n '\.execute\(' crates/cyrup-modes` still returns 0 after the change (no new coupling was introduced as a workaround).
- [ ] `cargo check -p cyrup-session-svc`, `cargo clippy --all-targets` and `cargo test -p cyrup-session-svc` all pass (test count adjusts by exactly 2 if option (a) is taken).

## Findings

Each was produced by a survey agent and then adversarially checked by a separate verifier that tried to refute it. `OVERSTATED` means the finding is real but the verifier corrected its scope or severity — the corrected values are the ones below.

### `SessionCommand`/`AgentSession::execute` in src/command.rs has no in-tree consumer, and its module doc asserts an invariant the workspace contradicts

`OVERSTATED` · severity **medium** · effort **medium** · dimension `dead-surface`

**Evidence.** crates/cyrup-session-svc/src/command.rs is 196 lines: `pub enum SessionCommand` at :24 (33 variants), `pub enum SessionCommandOutput` at :66 (15 variants), `pub async fn execute` at :96. Module doc command.rs:1-4 states "the central invariant of the subsystem is that no mode reaches behaviour that does not flow through `SessionCommand`/`AgentSession`"; command.rs:93-94 repeats "Every adapter routes here so behaviour cannot diverge per front-end." Verified false: `rg -n '\.execute\(' crates/ --glob '*.rs'` returns zero hits anywhere in crates/cyrup-modes/ (only cyrup-tools' unrelated `Tool::execute`), so no adapter calls `AgentSession::execute`. crates/cyrup-modes/src/rpc.rs:84 declares its own independent `#[derive(serde::Deserialize)] pub enum SessionCommand` and dispatches it to facade methods directly (rpc.rs:1119-1510+). crates/cyrup-sdk/src/lib.rs:102 re-exports `cyrup_modes::SessionCommand`, while crates/cyrup-sdk/src/lib.rs:114 `pub use cyrup_session_svc as session_svc;` makes the second, unrelated `cyrup_sdk::session_svc::SessionCommand` reachable in the same SDK — the name collision is real. `rg -n 'SessionCommandOutput' crates/ --glob '*.rs'` outside crates/cyrup-session-svc/src returns zero hits.

**Why it matters.** 33 verbs and a dispatch table that must be kept in lockstep with every new `AgentSession` method to keep the module's own stated invariant true, for zero non-test consumers. The load-bearing problem is the doc: a maintainer adding a verb reads command.rs:1-4 and :93-94 and believes adapters route through here, so they will keep maintaining the table forever. The duplicate `SessionCommand` name across cyrup_sdk's two re-export paths is a smaller but real footgun.

**Fix.** Cheapest correct step regardless of the larger decision: strike the arch-11 §2.1 "single verb seam" / "every adapter routes here" claims from command.rs:1-4 and :93-94 (and the aside at session/bash.rs:53) so the doc matches reality. Then decide: (a) delete src/command.rs, `mod command;` (lib.rs:24) and `pub use command::{SessionCommand, SessionCommandOutput};` (lib.rs:51), plus the two tests that only exercise it (src/tests/integration.rs:519-568, src/tests/round5.rs:293-330); or (b) treat it as a deliberate embedder-facing API, keep it, and document it as such rather than as the mandatory adapter seam. Do not leave the false invariant text standing under either choice.

**Verifier correction.** Every factual claim holds, but the severity is inflated and two counts are wrong. Corrected scope: the enum has 33 verbs (not 35) and `SessionCommandOutput` has 15 variants (not 16); the two tests are at src/tests/integration.rs:519-568 and src/tests/round5.rs:293-330 (in-crate `#[cfg(test)] mod tests`, not a top-level tests/ dir). Severity lowered high -> medium: this is a compiling, tested, plausibly-embedder-facing public API, not rot. The highest-value and cheapest part of the fix is correcting the false doc claim; option (b) (re-pointing cyrup-modes at this enum) is an architectural decision, not a hygiene cleanup, and should not be presented as an equal-cost alternative.
