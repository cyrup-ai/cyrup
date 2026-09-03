---
stage: qa
status: completed
updated: 2026-09-03 23:20
aug_against: branch 3ca59c3 (CLTR_1..7 landed) — uncapped sweep of is_streaming / is_run_active / is_idle / StateInner::snapshot across all 23 crates incl. tests (182 hits, tmp/cltr8_sweep.txt); pi agent.ts + agent-session.ts read for the predicate audit; second pass: the public accessor's ordering hazard analysed against pi's `_emitAgentSettled` and cyrup's run-scoped streams
---

# CLTR_8 — One run-in-flight fact, two flags: collapse `is_streaming` onto the latch (F8)

OBJECTIVE: Delete `StateInner.is_streaming`, which is written at the same sites as the
`running_tx` latch and means the same thing, and source `AgentStateSnapshot.is_streaming` from
the latch instead. Then audit the session sites that read `is_streaming()` rather than
`is_run_active()` against AGENT-030 and switch them or document why the narrower predicate is
right. Net-negative code; the audit is the real work. Source: research §3 F8
([`.flux/research/CORE_LOOP_TYPE_REVIEW.md`](../research/CORE_LOOP_TYPE_REVIEW.md)).

## Aug findings that change the plan (read first)

**Finding 1 — the write sites moved since the research; there are now three, not two, and the
reducer never touches the flag.** After CLTR_3A/6 the field is set at
[`lifecycle.rs:298`](../../crates/cyrup-agent/src/agent/lifecycle.rs) (`claim_and_snapshot`, under
the lock right after the latch claim), cleared at `:84` (`SettlementGuard::drop`, one statement
before `running_tx.send(false)` at `:98`) and at `:120` (`reset`, which already refuses while
`is_running()`). `reduce` (`state.rs`) has no `is_streaming` arm. Three other literals name it:
`agent/builder.rs:258` (`false`), `loop_fn.rs:127` (`true`), and the test-local `StateInner`
literal at `tests/area02_backlog.rs:439` (`false`) — the last is a test edit the type change
forces, as CLTR_6's was.

**Finding 2 — `StateInner::snapshot` cannot read the latch by itself; the facade hands it in.**
The latch lives on `Agent` (`running_rx`), not on `StateInner`; the ONLY caller of
`StateInner::snapshot` is [`facade.rs:67-69`](../../crates/cyrup-agent/src/agent/facade.rs)
(`lock(&self.state).snapshot()`; `loop_fn` never snapshots). So `snapshot` takes the fact as a
parameter: `pub(crate) fn snapshot(&self, is_streaming: bool)`, and the facade passes
`self.is_running()`. Observable timing shifts by at most the two statements the CLTR_3A guard
already documents: `true` from the claim (before the model check) instead of from the write after
it; `false` from `running_tx.send(false)` instead of from the state write one statement earlier —
both strictly closer to the truth the latch defines.

**Finding 3 — the audit's answer is decided by pi, at every site: the session latch.** pi's
`AgentSession` getter is `get isStreaming(): boolean { return this._isAgentRunActive; }`
([`agent-session.ts:900-901`](../../tmp/pi/packages/coding-agent/src/core/agent-session.ts)) —
the SESSION latch set at the top of `_runAgentPrompt` (`:1086`) and cleared in
`_emitAgentSettled` (`:610`), spanning retries, auto-compaction and queued continuations. Every
routing decision consults that getter: `prompt` → steer/followUp (`:1190`), `sendMessage` →
steer/followUp vs `_runAgentPrompt` (`:1477`, `:1485`), bash deferral (`:3007`), tree navigation
(`:3089`). cyrup's `is_run_active()` ([`session/mod.rs:483-498`](../../crates/cyrup-session-svc/src/session/mod.rs))
IS `_isAgentRunActive` (`!is_idle()` = `driver_tx || agent.is_running()`), and AGENT-030
(`run.rs:72-77`) already moved `prompt` onto it. The sweep finds FIVE session sites still on the
agent's flag, not four: [`inject.rs:41`](../../crates/cyrup-session-svc/src/session/inject.rs)
(`send_user_message`, pi `:1190`), `inject.rs:76` (`send_custom_message`, pi `:1477`),
`inject.rs:127` (`inject_message`, pi `:1477/:1485`), [`control.rs:236`](../../crates/cyrup-session-svc/src/session/control.rs)
(the extension `send_message` control op, pi `:1477`), and [`bash.rs:233`](../../crates/cyrup-session-svc/src/session/bash.rs)
(bash-message deferral, pi `:3007`). All five switch to `self.is_run_active()` (sync — the
`.await` goes). The concrete behaviour this fixes: a message or bash result landing in the
post-`agent_end` gap (auto-retry, auto-compaction, queued continuation) was appended or started a
SECOND run under the agent flag; under the session latch it queues onto the active loop exactly
as pi's does, and `drive_run`'s `continue_run` drains it (steering first) — `flush_pending_bash_messages`
already runs at the end of the whole loop (`run.rs:193`), which is pi's `finally`.

**Finding 4 — the PUBLIC accessor `AgentSession::is_streaming()` keeps the agent latch; that is
structurally forced, not a shortcut (second-pass analysis).** Its readers are the RPC
`get_state.isStreaming` (`cyrup-modes/src/rpc/mod.rs:1333`), the state view (`session/stats.rs:246`),
the SDK (`cyrup-sdk/src/handle.rs:372`), the TUI (`tree_nav.rs:118,144`, `run_action.rs:127,149`),
and seven tests, two of which assert `!is_streaming()` the instant the SDK's `run()` returns
(`cyrup-it/tests/bin/embedding.rs:141`; `:180` and `integration.rs:434` after `wait_for_idle`).
pi's getter is the session latch, and pi clears it BEFORE it emits `agent_settled`
(`_emitAgentSettled`, [`agent-session.ts:609-613`](../../tmp/pi/packages/coding-agent/src/core/agent-session.ts):
`this._isAgentRunActive = false;` then the emit, then `_resolveIdleWaitIfIdle()` in its `finally`) —
pi has TWO signals: the flag, and the idle-wait promise resolved after delivery. cyrup has ONE:
`driver_tx` is both the run-active latch and the idle latch, and it drops LAST in `drive_run`
(`run.rs:197-206`: `emit_agent_settled()` → `fanout.end_run()` → `driver_tx.send(false)`).
Redefining the accessor as `is_run_active()` would therefore race `embedding.rs:141` (the
run-scoped stream ends at `end_run()`, one statement before the latch drops). And the obvious
fix — dropping `driver_tx` before `end_run()` — is UNSAFE for a different reason: `wait_for_idle`
(`run.rs:632-640`) releases on `driver_tx`; a caller that then issues `prompt()` passes the
AGENT-030 gate and registers `fanout.subscribe_run()` (`run.rs:79`) BEFORE the previous driver's
`end_run()` runs, which clears `run_scoped` wholesale ([`subscriber.rs:79-84`](../../crates/cyrup-session-svc/src/subscriber.rs))
— the new prompt's stream would end with no events. The only pi-exact design is pi's own: a
second run-active signal cleared before the settle emit, with `driver_tx` kept as the idle latch
that drops last; that touches `is_idle`/`wait_for_idle`/AGENT-030 and is recorded below as the
follow-up, not done here. So: the accessor keeps the agent latch (its body is unchanged; only
`snapshot().is_streaming`'s SOURCE changes underneath it), gains a doc stating the delta and the
reason, and the five routing sites — the decisions pi makes on its getter, none of which touch a
run-scoped stream — use the session predicate directly. The `session/mod.rs:490-494` comment
that describes the accessor as "which `SettlementGuard::drop` clears" must be rewritten: nothing
clears a flag any more, the latch releases. One comment in a third crate describes the old routing
([`cyrup-intercom/src/inbound.rs:241`](../../crates/cyrup-intercom/src/inbound.rs): "routes to
`agent.steer(msg)` whenever `is_streaming()`") and is corrected to name `is_run_active()` — a
comment-only edit; `cyrup-tui/src/app/run_action.rs:105-107` ("`is_streaming` reads the agent
snapshot") stays true and stays.

**Finding 5 — `claim_and_snapshot`'s comments count the writes.** The CLTR_6 comment at
`lifecycle.rs:289-290` says "before the two run-start writes below"; with `is_streaming` gone
there is ONE (`error_message = None`). The `:278-279` comment "resets `is_streaming` through its
drop" becomes "releases the latch through its drop".

## SUBTASK1 — delete the field (`cyrup-agent`)

**[`state.rs`](../../crates/cyrup-agent/src/state.rs)** — delete `:97` `pub is_streaming: bool,`
from `StateInner`. `snapshot` (`:120-133`) becomes:

```rust
    /// `is_streaming` is the run latch — the ONE run-in-flight fact (pi `AgentState.isStreaming`,
    /// set/cleared around `runWithLifecycle`, agent.ts:498/:530) — read by the caller from
    /// `running_rx`, because the latch lives on `Agent`, not here.
    pub(crate) fn snapshot(&self, is_streaming: bool) -> AgentStateSnapshot {
        AgentStateSnapshot {
            system_prompt: self.system_prompt.clone(),
            model: self.model.clone(),
            thinking_level: self.thinking_level,
            messages: self.messages.clone(),
            tool_count: self.tools.len(),
            is_streaming,
            streaming_message: self.streaming_message.clone(),
            pending_tool_calls: self.pending_tool_calls.iter().cloned().collect(),
            error_message: self.error_message.clone(),
            headers: self.headers.clone(),
        }
    }
```

`AgentStateSnapshot.is_streaming: bool` (`:145`) stays, with a doc line:
`/// Whether a run is in flight — sourced from the agent's run latch, never a second flag.`

**[`agent/facade.rs:67-69`](../../crates/cyrup-agent/src/agent/facade.rs)** —
`pub async fn snapshot(&self) -> AgentStateSnapshot { let running = self.is_running(); lock(&self.state).snapshot(running) }`
(read the latch, then take the lock — never hold the lock while touching the channel).

**[`agent/lifecycle.rs`](../../crates/cyrup-agent/src/agent/lifecycle.rs)** — delete the three
lines `st.is_streaming = false;` (`:84`, `:120`) and `st.is_streaming = true;` (`:298`). Comment
`:278-279`: "clears the cancel slot and resets `is_streaming` through its drop." → "clears the
cancel slot and releases the latch through its drop." Comment `:289`: "before the two run-start
writes below" → "before the run-start write below". The `SettlementGuard` doc (`:58-62`) is now
literally true and stays.

**[`agent/builder.rs:258`](../../crates/cyrup-agent/src/agent/builder.rs)** — delete
`is_streaming: false,`. **[`loop_fn.rs:127`](../../crates/cyrup-agent/src/loop_fn.rs)** — delete
`is_streaming: true,`. **[`tests/area02_backlog.rs:439`](../../crates/cyrup-agent/src/tests/area02_backlog.rs)**
— delete `is_streaming: false,` from the test-local `StateInner` literal (the only test edit; no
assertion changes).

## SUBTASK2 — the audit: five session sites onto the session predicate (`cyrup-session-svc`)

Each site gets the same one-line comment (adapt the pi citation per site) and drops the `.await`:

- [`session/inject.rs:41`](../../crates/cyrup-session-svc/src/session/inject.rs)
  `if self.is_streaming().await {` →
  ```rust
        // AGENT-030 — pi routes on `this.isStreaming`, which IS the session latch
        // `_isAgentRunActive` (agent-session.ts:900-901, consulted at :1190): a submission landing
        // in the post-`agent_end` gap queues onto the active loop instead of starting a second run.
        if self.is_run_active() {
  ```
- `inject.rs:76` `_ if self.is_streaming().await => match deliver_as {` →
  `_ if self.is_run_active() => match deliver_as {` with the comment above it citing `:1477`.
- `inject.rs:127` `if self.is_streaming().await {` → `if self.is_run_active() {` citing
  `:1477/:1485`; the existing "Pi: while streaming, queue onto the active run (steer)." line stays.
- [`session/control.rs:236`](../../crates/cyrup-session-svc/src/session/control.rs)
  `if trigger_turn && deliver_as.is_none() && !self.is_streaming().await {` →
  `if trigger_turn && deliver_as.is_none() && !self.is_run_active() {` citing `:1477-1483`.
- [`session/bash.rs:233`](../../crates/cyrup-session-svc/src/session/bash.rs)
  `if self.is_streaming().await {` → `if self.is_run_active() {` citing `:3007` ("If agent is
  streaming, defer adding to avoid breaking tool_use/tool_result ordering" — deferred until the
  WHOLE loop's `flush_pending_bash_messages`, `run.rs:193`, pi's `finally`).

**[`cyrup-intercom/src/inbound.rs:241`](../../crates/cyrup-intercom/src/inbound.rs)** — comment
only: "routes to `agent.steer(msg)` whenever `is_streaming()`" → "routes to `agent.steer(msg)`
whenever `is_run_active()`" (the cited `session.rs:3926-3928` location is stale too; point it at
`session/inject.rs`).

**[`session/accessors.rs:44-47`](../../crates/cyrup-session-svc/src/session/accessors.rs)** —
body unchanged (`self.agent.snapshot().await.is_streaming`, now the agent latch); replace the doc
`/// Whether a run is currently streaming.` with:

```rust
    /// Whether the AGENT's run is in flight — its run latch, released the moment each individual
    /// run settles. [CYRUP-DELTA] pi's `get isStreaming()` (agent-session.ts:900-901) returns the
    /// SESSION latch `_isAgentRunActive`, which also spans the post-run driver loop; that predicate
    /// is [`Self::is_run_active`], and every routing decision pi makes on its getter uses it. This
    /// accessor keeps the narrower agent latch because cyrup has ONE session signal where pi has
    /// two: `driver_tx` is both the run-active latch and the idle latch, so it must drop AFTER
    /// `fanout.end_run()` (or a `prompt` issued from `wait_for_idle` would register a run-scoped
    /// stream the previous run's `end_run` then clears) — and SDK / embedding callers assert
    /// idleness the instant the run-scoped stream ends (`session/run.rs`).
```

**[`session/mod.rs:490-494`](../../crates/cyrup-session-svc/src/session/mod.rs)** — the sentence
"cyrup's [`Self::is_streaming`] reads `agent.snapshot().is_streaming`, which `SettlementGuard::drop`
clears the moment each INDIVIDUAL run settles, so a prompt landing in the post-run gap (…) started
a SECOND run that raced `drive_run`'s `continue_run()`." → "cyrup's [`Self::is_streaming`] reads
the AGENT's run latch, which releases the moment each INDIVIDUAL run settles, so a submission
gated on it in the post-run gap (an auto-retry, an auto-compaction, a queued continuation) would
start a SECOND run that races `drive_run`'s `continue_run()` — every routing site therefore
reads this predicate."

`stats.rs:246`, the RPC state, the SDK and the TUI readers: unchanged (Finding 4).

## Definition of done

- `grep -rn "is_streaming" crates/cyrup-agent/src` matches only `state.rs` (the `snapshot`
  parameter and the `AgentStateSnapshot` field) and `facade.rs` — no `StateInner` field, no
  `st.is_streaming` write anywhere, no literal in `builder.rs`/`loop_fn.rs`/`area02_backlog.rs`.
- `StateInner::snapshot(&self, is_streaming: bool)`; `Agent::snapshot` passes `self.is_running()`.
- `grep -rn "is_streaming().await" crates/cyrup-session-svc/src/session` matches ONLY
  `accessors.rs`'s definition and `stats.rs:246`; the five routing sites read `is_run_active()`
  and each carries the AGENT-030 comment; the accessor carries the CYRUP-DELTA doc; the
  `session/mod.rs` paragraph no longer says a flag is "cleared"; the `cyrup-intercom/src/inbound.rs`
  comment names `is_run_active()`.
- No file outside `cyrup-agent`, `cyrup-session-svc` and the one `cyrup-intercom` comment changes;
  no test file changes except the one `StateInner` literal; `settlement_latch.rs` (3,
  multi-thread), `pending_containment.rs`, the seven session/it tests reading `is_streaming()`,
  and `inject_message_details.rs` pass unedited.
- `cargo check --workspace --all-targets --features test-fixtures` clean;
  `cargo test --workspace --features test-fixtures --no-fail-fast` all green (8466 baseline);
  `cargo clippy --workspace --all-targets --features test-fixtures` adds no warning (the one
  pre-existing `cyrup-tui` `question_mark` warning is not this task's);
  `cargo doc -p cyrup-agent -p cyrup-session-svc --no-deps` exits 0 (`[`Self::is_run_active`]`
  resolves from `accessors.rs` — same `impl AgentSession`).

## Research notes

Research §3 F8, §2 last row. `run.rs:75,325` already use `is_run_active()`; `is_run_active()` is
`!self.is_idle()` (`session/mod.rs:496-498`), `is_idle()` is `!driver_tx && !agent.is_running()`
(`:479-481`). pi anchors: [`agent.ts:498/:530`](../../tmp/pi/packages/agent/src/agent.ts)
(`isStreaming` set/cleared around the run), [`agent-session.ts:320`](../../tmp/pi/packages/coding-agent/src/core/agent-session.ts)
`_isAgentRunActive`, `:900-906` the `isStreaming`/`isIdle` getters, `:1086`/`:610` set/clear,
`:1190`, `:1477-1485`, `:3007`, `:3089` the routing sites, `rpc-mode.ts:454` the RPC state read.
Follow-up (out of scope, designed): making the public accessor pi's getter needs pi's two
signals — a session `run_active: watch<bool>` set where `driver_tx` is set (`run.rs:134`) and
cleared at the top of `emit_agent_settled` (pi `:609`), read by `is_run_active()` and the
accessor; `driver_tx` stays the idle latch that drops last (`:206`) so `wait_for_idle` and the
run-scoped stream keep their order. It changes the AGENT-030 `prompt` gate's window (a prompt
during the settled emit would be accepted, as pi's is) and needs its own review of
`subscribe_run` against `end_run`.

No tests to be written — another team owns tests. No benchmarks to be written.
