---
stage: qa
status: completed
updated: 2026-08-29 22:00
---

# Fix The Fixture Env-Var Race — one false clause left

## QA verdict: 9/10 — needs-rework

**Accepted, not to be re-opened.** The doc now names both honouring paths and states the
detached/background boundary with its remedy. The edit is documentation-only — the sole non-doc
lines in that region are `#[serde(skip)]` and the field declaration, both unchanged — and the
`serde(skip)` rationale is intact. `cargo doc` clean. Independently re-audited: the eleven
converted files are at **combined residue 0**, statics at **20**, and
`subagent_persona_and_depth_integration` mutates env in exactly four functions, each holding the
lock.

## The defect — the doc asserts a mechanism that does not exist

[`registration/mod.rs:143-144`](../../crates/cyrup-ext-subagents/src/registration/mod.rs):

```rust
/// - foreground chain and parallel steps, through
///   `ExecSingleStepExecutor::foreground`, which that prologue also feeds.
```

The foreground prologue does **not** feed it. There are two independent snapshots:

- [`foreground.rs:240`](../../crates/cyrup-ext-subagents/src/extension/executor/foreground.rs) —
  `let cfg = self.config_snapshot().await;` for the single-run path;
- [`chain.rs:114`](../../crates/cyrup-ext-subagents/src/extension/executor/chain.rs) — its own
  `let cfg = self.config_snapshot().await;`, which is what passes `cfg.spawn_command.clone()` into
  `ExecSingleStepExecutor::foreground`.

The substance is right — both paths honour the field, the boundary is correct — so nobody is misled
about *reach*. But this doc exists because vagueness about mechanism produced a real bug three
commits ago, and **a confident false clause is worse than the silence it replaced**: silence invites
checking, an assertion does not.

### Required fix

Replace that clause with what actually happens: the foreground chain/parallel walk takes its **own**
config snapshot and passes the field to `ExecSingleStepExecutor::foreground`. Do not imply either
path feeds the other. Everything else in the doc stands.

## Definition of done

- [ ] The chain/parallel bullet describes the real mechanism — its own snapshot, passed to
      `ExecSingleStepExecutor::foreground` — with no claim that the foreground prologue feeds it.
- [ ] Still documentation-only; the `serde(skip)` rationale and the detached-boundary paragraph are
      untouched.
- [ ] `cargo doc -p cyrup-ext-subagents --no-deps` clean.
- [ ] `cargo test -p cyrup-it --features it --test subagents -- --test-threads=1` reports
      **196/196** and `cargo test -p cyrup-ext-subagents` **2587**, run AFTER this edit. The
      previous pair predates it; confirm, do not carry forward.

## Out of scope, recorded for the `CYRUP_HOME` follow-up

Five files mutate process-global env with **no lock at all**:
`native_supervisor_channel_integration` (6 `set_var`), `companions_hostservices_proof`,
`subagents_optin_gate_integration`, `verify_redaction_inherited_env`, `fleet_inspector_integration`.
Pre-existing and outside this task's fixture-var scope, but they are unguarded global mutation in
the same test binary and belong in that task's picture.
