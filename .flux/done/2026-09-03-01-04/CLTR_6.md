---
stage: qa
status: completed
updated: 2026-09-03 23:20
aug_against: branch 4bd296d (CLTR_1..5 landed) — uncapped sweep of Agent::builder/AgentBuilder::new/set_model/snapshot().model/ModelRef sentinels across all 23 crates incl. tests (310 hits, tmp/cltr6_sweep.txt)
---

# CLTR_6 — Modelless agent as a state, not a sentinel: `Option<ModelRef>` (F5)

OBJECTIVE: Replace the `ModelRef { provider: "", api: None, model: "" }` sentinel that
`cyrup-session-svc` seeds for a credential-less session — guarded today only by a comment reading
"unreachable" — with `Option<ModelRef>` on the agent's state, resolved to a `ModelRef` or
`AgentError::NoModelSelected` at the run boundary. `RunCtx.model` stays non-optional. Source:
research §3 F5 ([`.flux/research/CORE_LOOP_TYPE_REVIEW.md`](../research/CORE_LOOP_TYPE_REVIEW.md)).

## Aug findings that change the plan (read first)

**Finding 1 — `Agent::builder(model, stream_fn)` keeps its signature; only `AgentBuilder::new`
loses the model.** The sweep counts 99 `Agent::builder(model, sf)` call sites in
`cyrup-agent/src/tests/*.rs`, one in `cyrup-ext/src/tests/ext_fail_closed.rs`, and one in
`cyrup-session-svc/src/builder.rs:1603`. `AgentBuilder::new(..)` has exactly ONE caller:
[`agent/facade.rs:47`](../../crates/cyrup-agent/src/agent/facade.rs). So `AgentBuilder::new(stream_fn)`
takes only the stream fn (as the task says), `AgentBuilder::model(m)` sets `Some`, and
`Agent::builder(model, sf)` becomes `AgentBuilder::new(sf).model(model)` — every test compiles
unedited and keeps meaning "an agent WITH this model". Both `Agent` and `AgentBuilder` are
re-exported at the crate root ([`lib.rs:23`](../../crates/cyrup-agent/src/lib.rs)), so the session
builder can name `cyrup_agent::AgentBuilder` directly for the modelless path.

**Finding 2 — the check goes AFTER the claim, under the lock, and relies on the CLTR_3A guard.**
The task (and the research) say "before the latch is claimed". Since CLTR_3A, `claim_and_snapshot`
([`lifecycle.rs:259-308`](../../crates/cyrup-agent/src/agent/lifecycle.rs)) is deliberately
claim-FIRST: reading state before the claim reopens the check-then-claim gap (a `set_model(None)`
landing between the check and the claim would start a run with no model — the exact race 3A
closed for `set_messages`). The `SettlementGuard` is built before any state is touched and its
docstring already promises "every early `Err` below releases the latch, clears the cancel slot and
resets `is_streaming` through its drop". So: claim → build guard → lock → **`model` check first,
before the two run-start writes** → writes → baseline. A modelless agent never holds the latch
after the call returns, which is the guarantee the task wants; the DoD below is worded to that.

**Finding 3 — two failure paths read `state.model` at failure time and must not unwrap.**
`emit_run_failure` ([`run/mod.rs:267-269`](../../crates/cyrup-agent/src/agent/run/mod.rs)) and the
panic twin in `spawn_run` ([`lifecycle.rs:342`](../../crates/cyrup-agent/src/agent/lifecycle.rs))
both do `lock(&state).model.clone()` because pi reads `this._state.model` (agent.ts:500-502), not
the loop's running baseline. Both run only while a run exists (so a model was `Some` at claim
time), but `set_model(None)` mid-run is now expressible. Keep pi's read and fall back to the run's
own baseline model: `run/mod.rs` has `self.model` (the sticky `RunCtx` baseline); `spawn_run` must
capture `let fail_model = baseline.model.clone();` before `RunCtx::new` consumes `baseline`.
Neither path can ever stamp an empty provider onto a message.

**Finding 4 — `loop_fn::build_run_ctx` seeds `StateInner` directly.**
[`loop_fn.rs:122-123`](../../crates/cyrup-agent/src/loop_fn.rs) builds a `StateInner { model:
config.model.clone(), .. }` literal — it becomes `Some(config.model.clone())`. `AgentLoopConfig.model`
stays `ModelRef` (an embedder always has one) and `RunBaseline.model` stays `ModelRef`.

**Finding 5 — session consumers are exactly four sites, and `thinking.rs` needs nothing.**
`next_turn_model_baseline` ([`session/tools.rs:140-145`](../../crates/cyrup-session-svc/src/session/tools.rs))
is the only reader of `snapshot().model` outside `cyrup-agent` (no test reads it — verified);
`hooks.rs:278-279` consumes it into `TurnUpdate.model`, which is ALREADY `Option<ModelRef>`
([`hooks.rs:143`](../../crates/cyrup-agent/src/hooks.rs)) with `None` = "keep the loop's baseline",
so `update.model = model;` is the whole change. `session/model.rs:319` and `:485` are the two
`agent.set_model` callers (both wrap in `Some`). `session/thinking.rs` reads the session's own
`compaction_model` mirror, not the agent — no change. No path in the session ever clears the model
(`model: None` / `set_model(None)` has zero non-test hits), so `set_model(Option)` gains a caller
only in principle; it is still the honest signature.

**Finding 6 — the agent-level error text is a CYRUP-DELTA.** pi's `Agent`/loop never runs
modelless (`AgentLoopConfig.model: Model` is required, types.ts:141); the modelless guard lives in
`AgentSession.prompt` with `formatNoModelSelectedMessage()`, which cyrup already carries verbatim
as `SessionServiceError::NoModelSelected` ([`error.rs:64`](../../crates/cyrup-session-svc/src/error.rs)).
The new `AgentError::NoModelSelected` is the second line for paths that bypass the session
(`loop_fn` embedders, `continue_run` from an extension control op, tests) and carries a plain
diagnostic, not the `/login` → `/model` guidance — that guidance is the session's.

## SUBTASK1 — `cyrup-agent`: state, builder, facade, error, run boundary

**[`state.rs`](../../crates/cyrup-agent/src/state.rs)** — `:90` `pub model: ModelRef,` →
`pub model: Option<ModelRef>,` with a doc line: `/// `None` is pi's `Model | undefined` — a
credential-less session's agent has NO model until `/model` selects one (agent-session.ts:890-892).
Resolved to a `ModelRef` or `AgentError::NoModelSelected` at run start; never a sentinel address.`
`:138` `pub model: ModelRef,` → `pub model: Option<ModelRef>,`. `:120` `model: self.model.clone()`
is unchanged (an `Option` clones).

**[`agent/builder.rs`](../../crates/cyrup-agent/src/agent/builder.rs)** — `:18` `model: ModelRef,`
→ `model: Option<ModelRef>,`. `:33-35` becomes:

```rust
    #[must_use]
    pub fn new(stream_fn: Arc<dyn StreamFn>) -> Self {
        Self {
            system_prompt: String::new(),
            model: None,
```

(the rest of the literal unchanged). Add, after `system_prompt` (`:57-61`):

```rust
    /// The agent's model. Absent = modelless (pi `Model | undefined`, agent-session.ts:890-892):
    /// the agent builds and accepts state edits, and `prompt`/`continue_run` return
    /// [`crate::AgentError::NoModelSelected`] until one is set.
    #[must_use]
    pub fn model(mut self, m: ModelRef) -> Self {
        self.model = Some(m);
        self
    }
```

`build()` `:126` `model: self.model,` is unchanged.

**[`agent/facade.rs`](../../crates/cyrup-agent/src/agent/facade.rs)** — `:45-48`:

```rust
    /// An agent WITH a model. For a modelless agent use [`AgentBuilder::new`] and skip
    /// [`AgentBuilder::model`].
    #[must_use]
    pub fn builder(model: ModelRef, stream_fn: Arc<dyn StreamFn>) -> AgentBuilder {
        AgentBuilder::new(stream_fn).model(model)
    }
```

`:74-76` → `pub async fn set_model(&self, m: Option<ModelRef>) { lock(&self.state).model = m; }`
with a doc: `/// `None` makes the agent modelless: the next `prompt`/`continue_run` returns
[`AgentError::NoModelSelected`]. A run already in flight keeps its own baseline (pi
`agent.state.model = next` is likewise a between-turns write, agent-session.ts:1643).` (`AgentError`
and `BusyEntry` are already imported in facade.rs since CLTR_3B.)

**[`error.rs`](../../crates/cyrup-agent/src/error.rs)** — add to `AgentError`, after
`ContinueFromAssistant`:

```rust
    /// A run was requested while the agent has no model (`StateInner.model == None`).
    /// [CYRUP-DELTA] pi's loop config requires a `Model` (types.ts:141) and its modelless guard
    /// lives in `AgentSession.prompt` with `formatNoModelSelectedMessage()` — which cyrup carries
    /// verbatim as `cyrup_session_svc::SessionServiceError::NoModelSelected`, fired BEFORE this.
    /// This variant is the second line, for paths that bypass the session (a `loop_fn` embedder,
    /// `continue_run` from an extension control op, a test), so no run can ever stream against
    /// an empty provider/model address.
    #[error("No model selected. Select a model before starting a run.")]
    NoModelSelected,
```

**[`agent/lifecycle.rs`](../../crates/cyrup-agent/src/agent/lifecycle.rs)** — in
`claim_and_snapshot`, replace `:287-306` (`let baseline = { let mut st = …; … };`) with:

```rust
        let baseline = {
            let mut st = lock(&self.state);
            // Resolved FIRST, before the two run-start writes below, so a modelless agent
            // performs no state write and — through `guard`'s drop — never holds the latch
            // after this returns. Checked under the same lock as the snapshot (not before the
            // claim) for the reason the claim itself comes first: a `set_model(None)` in a
            // check-then-claim gap would otherwise start a run with no model.
            let Some(model) = st.model.clone() else {
                return Err(AgentError::NoModelSelected);
            };
            st.error_message = None;
            st.is_streaming = true;
            // (existing Pi `createContextSnapshot` / `transport` comments, verbatim)
            RunBaseline {
                system_prompt: st.system_prompt.clone(),
                model,
                thinking_level: st.thinking_level,
                gen_config: GenerationConfig { transport: st.transport, ..self.gen_config.clone() },
                tools: st.tools.clone(),
                messages: st.messages.clone(),
            }
        };
```

In `spawn_run` (`:322-333`): add `let fail_model = baseline.model.clone();` immediately before
`let mut rc = RunCtx::new(self.shared(), baseline, cancel)`, and `:342` becomes:

```rust
                    // Pi reads `this._state.model` (agent.ts:500-502); with `Option` the run's own
                    // baseline is the fallback for a model cleared mid-run — never an empty address.
                    let model = { lock(&fail_state).model.clone() }.unwrap_or(fail_model);
```

**[`agent/run/mod.rs`](../../crates/cyrup-agent/src/agent/run/mod.rs)** — `:269` becomes
`let model = { lock(&self.state).model.clone() }.unwrap_or_else(|| self.model.clone());` (keep the
comment at `:267-268`, append "; `self.model` is the fallback for a model cleared mid-run").

**[`loop_fn.rs:123`](../../crates/cyrup-agent/src/loop_fn.rs)** — `model: config.model.clone(),`
→ `model: Some(config.model.clone()),`. `AgentLoopConfig.model` (`:56`) and `RunBaseline.model`
stay `ModelRef`.

## SUBTASK2 — `cyrup-session-svc` consumers

**[`builder.rs:1587-1603`](../../crates/cyrup-session-svc/src/builder.rs)** — delete the SEAM-075
comment block and the `let agent_model = …unwrap_or_else(|| ModelRef { "" … })` literal
(`:1587-1602`). `:1603` `let mut agent_builder = Agent::builder(agent_model, agent_stream_fn)` →
`let mut agent_builder = cyrup_agent::AgentBuilder::new(agent_stream_fn)` (the chain
`.system_prompt(..)…​.transport(..)` is unchanged). Directly after the chain's `;` and before
`if let Some(h) = attribution_headers` (`:1620`), add:

```rust
        // SEAM-075 — pi's agent holds `Model | undefined` (`AgentSession.model` is a straight read
        // of `this.agent.state.model`, agent-session.ts:890-892): a modelless session builds an
        // agent with NO model, and the first `/model` sets it through `agent.set_model`. Every
        // reader — the `/model` picker, the footer, `state_view`, the attribution headers, the
        // `CYRUP_*` env — reads the session's `Option<ModelRef>`, which stays `None` until then.
        if let Some(m) = model_ref.clone() {
            agent_builder = agent_builder.model(m);
        }
```

`use cyrup_agent::Agent;` at `:12` stays only if `Agent` is still named elsewhere in the file
(`:1791` `Arc::new(agent)` does not name the type) — let the unused-import warning decide; the DoD
is zero warnings.

**[`session/tools.rs:140-145`](../../crates/cyrup-session-svc/src/session/tools.rs)** — return type
`-> (Option<ModelRef>, cyrup_core::ModelThinkingLevel)`; body unchanged (`snap.model` is now the
`Option`). Append to the doc: `/// `None` (a modelless agent) leaves `TurnUpdate.model` unset, so
the running loop keeps its own baseline — a run cannot be in flight without one anyway.`

**[`hooks.rs:279`](../../crates/cyrup-session-svc/src/hooks.rs)** — `update.model = Some(model);`
→ `update.model = model;`.

**[`session/model.rs:319`, `:485`](../../crates/cyrup-session-svc/src/session/model.rs)** —
`self.agent.set_model(model_ref.clone()).await;` → `self.agent.set_model(Some(model_ref.clone())).await;`
at both sites.

`session/thinking.rs`, `session/accessors.rs:34` and `cyrup-sdk/src/handle.rs:357` — no change
(they read the session's own `Option<ModelRef>` mirror, which was already `Option`).

## Definition of done

- `grep -rn 'ProviderId::from("")\|ModelId::from("")\|provider: ""' crates/cyrup-session-svc/src`
  matches nothing; the SEAM-075 "unreachable" comment is gone and the new one above is in place.
- `AgentError::NoModelSelected` exists; `claim_and_snapshot` returns it from inside the state
  lock, before `error_message`/`is_streaming` are written, and the `SettlementGuard` drop releases
  the latch — i.e. after `prompt`/`continue_run` return this error, `is_running()` is `false`.
- `StateInner.model` and `AgentStateSnapshot.model` are `Option<ModelRef>`; `AgentBuilder::new`
  takes only the stream fn; `AgentBuilder::model` exists; `Agent::builder(model, sf)` is
  unchanged in signature; `Agent::set_model` takes `Option<ModelRef>`.
- `RunCtx.model`, `RunBaseline.model` and `AgentLoopConfig.model` remain `ModelRef`; no
  `.unwrap()`/`.expect()` on the model anywhere in the diff (the two failure paths use the
  baseline fallback).
- The session's own `SessionServiceError::NoModelSelected` preflight (`error.rs:64`, pi's verbatim
  guidance text) is untouched.
- No test file changes; the 99 `Agent::builder(..)` test sites compile unedited.
- `cargo check --workspace --all-targets --features test-fixtures` clean;
  `cargo test --workspace --features test-fixtures --no-fail-fast` all green (8466 baseline;
  `cyrup-session-svc/src/tests/modelless_launch.rs` in particular);
  `cargo clippy --workspace --all-targets --features test-fixtures` adds no warning (the one
  pre-existing `cyrup-tui` `question_mark` warning is not this task's);
  `cargo doc -p cyrup-agent -p cyrup-session-svc --no-deps` exits 0 (the workspace denies broken
  intra-doc links — `[`crate::AgentError::NoModelSelected`]`, `[`AgentBuilder::new`]`,
  `[`AgentBuilder::model`]`, `[`AgentError::NoModelSelected`]` resolve as written).

## Research notes

Research §3 F5, §2 row 8, §5 "`ModelRef.api: Option<ApiId>` → required" (deliberately NOT in
scope — a `cyrup-core`/`cyrup-provider` change; `empty_assistant` keeps its `UNRESOLVED_API`
fallback). `cyrup-agent/src/proxy/mod.rs:38` builds a real faux `ModelRef` for the proxy — not a
sentinel, untouched. `cyrup-session-svc/src/session/forking.rs:404` and `builder.rs:2057` build
`ModelRef`s from resolved catalog entries — not sentinels, untouched.

No tests to be written — another team owns tests. No benchmarks to be written.
