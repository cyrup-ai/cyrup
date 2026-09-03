---
stage: qa
status: completed
updated: 2026-09-03 23:20
aug_against: branch 88b7bae (main 2cfff0f + CLTR_1 + CLTR_2) — uncapped sweep; every site is in cyrup-agent; 0 cross-crate, 0 test constructors
---

# CLTR_3A — `RunEntry` + `ResumePoint`: continuation as a parsed value, agent side (F1, part A)

OBJECTIVE: Give the "may this transcript be resumed?" precondition one home (`ResumePoint::check`),
make `Continue` unconstructible without that proof, derive `skip_initial_steering_poll` from the
entry kind instead of a separate `bool`, and — the headline — close the race between the snapshot
`continue_run` validates and the snapshot `start_run` runs. Group `RunCtx::new`'s 17 arguments
into `RunShared` + `RunBaseline` in the same step. Agent-side only; the public `Agent` surface is
unchanged here, consumers are CLTR_3B. Source: research §3 F1, §4 sketch, §6 step 3.

> **READ §0 FIRST.** The sweep found the change is perfectly contained, and reading `start_run`
> found that the task's original "one lock" framing was imprecise in a way that would have
> reintroduced a race. §1 is the corrected design; it is the whole point of this task.

---

## 0. What the sweep found

**0.1 Containment is total.** Every `EntryStart` site (17), both `RunCtx::new` callers, and every
`skip_initial_steering_poll` read/write live in `cyrup-agent/src/{agent/lifecycle.rs, agent/run/mod.rs,
agent/run/turn.rs, loop_fn.rs, agent/mod.rs}`. **Zero** sites in tests, zero in other crates.
`RunCtx::new` has exactly two callers: [`lifecycle.rs:282`](../../crates/cyrup-agent/src/agent/lifecycle.rs)
and [`loop_fn.rs:145`](../../crates/cyrup-agent/src/loop_fn.rs) (inside `build_run_ctx`).
`skip_initial_steering_poll` is read in exactly one place, [`turn.rs:17-18`](../../crates/cyrup-agent/src/agent/run/turn.rs).

**0.2 No name collisions.** `PromptSource` exists as a `pub trait` in `cyrup-mcp` (a different
crate — no clash with a `pub(crate)` enum here); `RunEntry` appears only in a doc comment in
`cyrup-ext-subagents`; `ResumePoint`/`RunShared`/`RunBaseline` are unused everywhere.

**0.3 "One lock" was the wrong target — the race has TWO halves.** `start_run`'s second lock
([`lifecycle.rs:258-273`](../../crates/cyrup-agent/src/agent/lifecycle.rs)) is not only a
re-snapshot: it also performs two state **writes** — `st.error_message = None` and
`st.is_streaming = true` (`:260-261`) — which must happen after the latch is claimed at `:235`, or a
rejected concurrent caller would clear state it does not own. So the correct order is not
"snapshot in `continue_run`, then claim": that merely swaps today's race (the run uses an
*unvalidated* re-snapshot) for a new one (the run uses a validated snapshot that a `set_messages`
between snapshot and claim has made *stale*). The design that closes both is **claim first, then
snapshot + validate + the two writes under one lock, and release the latch if validation fails**.
§1 prescribes it.

**0.4 Both `loop_fn` continuations must validate independently — correct the old DoD.**
`agent_loop_continue` (`loop_fn.rs:267`) validates and then spawns `run_agent_loop_continue`
(`:184`), which validates again — and `run_agent_loop_continue` is `pub` and callable on its own.
So the target is "both inline `is_empty` / `last is Assistant` blocks become one call to
`ResumePoint::check` each", not "one call in the file".

---

## 1. The design — claim, then snapshot-validate-write under one lock, release on failure

`SettlementGuard` ([`lifecycle.rs:66-100`](../../crates/cyrup-agent/src/agent/lifecycle.rs)) is
already the RAII release: its `drop` sets `is_streaming = false`, clears `pending_tool_calls` /
`streaming_message`, clears `cancel_slot`, and `running_tx.send(false)` (`:80-100`). Today it is
built at `:315` *inside* the spawned task. **Build it immediately after the claim instead**, so an
early `return Err(..)` between claim and spawn drops it and releases the latch exactly as a
finished run would. No new helper; no new release code path.

```rust
// agent/lifecycle.rs — the shape. `prompt`, `prompt_with_images` and `continue_run` all go here.
impl Agent {
    /// Claim the run latch, then under ONE state lock: take the run-start baseline, perform the
    /// two run-start writes, and let the caller validate against the very snapshot the run will
    /// use. The returned guard releases the latch on drop, so a validation failure after the claim
    /// unwinds cleanly — there is no window in which the transcript can change between validation
    /// and use, and no window in which a rejected caller has mutated state it does not own.
    fn claim_and_snapshot(&self, busy: BusyEntry) -> Result<(RunBaseline, SettlementGuard, RunCancel), AgentError> {
        let claimed = self.running_tx.send_if_modified(|running| { if *running { false } else { *running = true; true } });
        if !claimed { return Err(AgentError::RunActive(busy)); }
        let cancel = RunCancel::new();
        *lock(&self.cancel_slot) = Some(cancel.clone());
        let guard = SettlementGuard { state: self.state.clone(), cancel_slot: self.cancel_slot.clone(),
                                      running_tx: self.running_tx.clone(), result_tx: None, new_messages: Vec::new() };
        let baseline = {
            let mut st = lock(&self.state);
            st.error_message = None;            // the two run-start writes, under the same lock as the read
            st.is_streaming = true;
            RunBaseline { system_prompt: st.system_prompt.clone(), model: st.model.clone(), thinking_level: st.thinking_level,
                          gen_config: GenerationConfig { transport: st.transport, ..self.gen_config.clone() },
                          tools: st.tools.clone(), messages: st.messages.clone() }
        };
        Ok((baseline, guard, cancel))
    }

    pub async fn continue_run(&self) -> Result<RunHandle, AgentError> {
        if self.is_running() { return Err(AgentError::RunActive(BusyEntry::Continue)); }   // fast path, unchanged
        let (baseline, guard, cancel) = self.claim_and_snapshot(BusyEntry::Continue)?;
        if baseline.messages.is_empty() { return Err(AgentError::NoMessages(ContinueSurface::Agent)); }  // guard drops → latch released
        let last_is_assistant = baseline.messages.last().is_some_and(|m| m.is_assistant());
        if last_is_assistant {
            let steering = lock(&self.steering).drain();
            if !steering.is_empty() {
                return self.spawn_run(RunEntry::Prompt { messages: steering.clone(), source: PromptSource::SteeringDrain }, baseline, guard, cancel)
                    .map_err(|e| { lock(&self.steering).push_front(steering); e });       // same requeue-on-failure as today (:199-205)
            }
            let follow = lock(&self.follow_up).drain();
            if follow.is_empty() { return Err(AgentError::ContinueFromAssistant); }         // guard drops → latch released
            return self.spawn_run(RunEntry::Prompt { messages: follow.clone(), source: PromptSource::FollowUpDrain }, baseline, guard, cancel)
                .map_err(|e| { lock(&self.follow_up).push_front(follow); e });
        }
        let proof = ResumePoint::check(&baseline.messages, ContinueSurface::Agent)?;         // one home for the rule
        self.spawn_run(RunEntry::Continue(proof), baseline, guard, cancel)
    }
}
```
`spawn_run(entry, baseline, guard, cancel)` is today's `start_run` from `:282` onward: build
`RunCtx::new(self.shared(), baseline, cancel).with_header_fn(..)`, set `guard.result_tx = Some(tx)`,
move `guard` into the `tokio::spawn`, run `rc.run(entry)`, `guard.complete(new)` / the
`catch_unwind` twin (`:326-379`), return `Ok(RunHandle { new_messages: rx })`. It is infallible
(the `Result` only exists so the requeue `map_err` reads cleanly). `prompt` (`:143`) and
`prompt_with_images` (`:156`) become `claim_and_snapshot(BusyEntry::Prompt)?` then
`spawn_run(RunEntry::Prompt { messages, source: PromptSource::Fresh }, ..)`.

**Why the order is exactly this.** Today's `continue_run` drains steering only when the last message
is an assistant (`:188-206`), then follow-up (`:207-217`), and only then rejects with
`ContinueFromAssistant` (`:209`) — the sketch keeps that order so `gap19_continue_from_assistant_skips_initial_steering_poll`
(`tests/model_boundary.rs:484`) and the requeue-on-failure behaviour are byte-identical. What
changes is that every branch now runs against **the same baseline the run will use**, and every
`Err` after the claim releases the latch through the guard's `drop`.

---

## 2. `RunEntry`, `PromptSource`, `ResumePoint` (SUBTASK1 — `agent/run/mod.rs`)

What: replace `EntryStart` ([`run/mod.rs:21-24`](../../crates/cyrup-agent/src/agent/run/mod.rs)) with:
```rust
pub(crate) enum PromptSource { Fresh, SteeringDrain, FollowUpDrain }
pub(crate) enum RunEntry {
    Prompt { messages: Vec<AgentMessage>, source: PromptSource },
    Continue(ResumePoint),
}
impl RunEntry {
    /// pi `skipInitialSteeringPoll` (agent.ts:351,440-446): only a steering-drain prompt skips
    /// the first poll, so the second queued steering message is not jammed into the same turn.
    pub(crate) fn skip_initial_steering_poll(&self) -> bool {
        matches!(self, RunEntry::Prompt { source: PromptSource::SteeringDrain, .. })
    }
}
/// Proof that a transcript may be resumed without a new message. Zero-sized; private field;
/// ONE constructor — the single home of the precondition that used to be written out three times.
pub(crate) struct ResumePoint(());
impl ResumePoint {
    pub(crate) fn check(messages: &[AgentMessage], surface: ContinueSurface) -> Result<Self, AgentError> {
        if messages.is_empty() { return Err(AgentError::NoMessages(surface)); }
        if messages.last().is_some_and(|m| m.is_assistant()) { return Err(AgentError::ContinueFromAssistant); }
        Ok(ResumePoint(()))
    }
}
```
Update `agent/mod.rs:41`'s re-export (`pub(crate) use run::{EntryStart, RunCtx};` →
`{RunEntry, RunCtx}`) and the two `use` lines at `lifecycle.rs:7` and `loop_fn.rs:16`.
`run_entry` (`run/mod.rs:262-284`) matches `RunEntry::Prompt { messages, .. }` and
`RunEntry::Continue(_)`; its bodies are unchanged. `run` (`:250`) sets
`self.skip_initial_steering_poll = entry.skip_initial_steering_poll()` **before** `run_entry`;
the field on `RunCtx` (`:80`) stays — `turn.rs:17-18` reads and clears it per turn — but it
is no longer a constructor argument and is initialized `false` in `new`.

## 3. `RunShared` + `RunBaseline`; `RunCtx::new` loses the `bool` (SUBTASK2 — `agent/run/mod.rs`)

What: the 17 parameters of `RunCtx::new` (`:90-108`) are two groups, and they are exactly the
two groups `start_run` already assembles separately (`:283-291` vs `:292-297`):
```rust
/// Handles that live for the whole run — cloned out of `Agent` (or built by `loop_fn`) once.
pub(crate) struct RunShared {
    pub state: Arc<Mutex<StateInner>>, pub subscribers: Arc<Mutex<Vec<Arc<dyn EventSubscriber>>>>,
    pub steering: Arc<Mutex<PendingQueue>>, pub follow_up: Arc<Mutex<PendingQueue>>,
    pub hooks: Arc<dyn Hooks>, pub stream_fn: Arc<dyn StreamFn>, pub key_resolver: Option<Arc<dyn ApiKeyResolver>>,
    pub tool_execution: ToolExecution, pub session_id: Option<SessionId>,
}
/// The run-start `.slice()` baseline — pi `createContextSnapshot` — taken under the state lock.
pub(crate) struct RunBaseline {
    pub system_prompt: String, pub model: ModelRef, pub thinking_level: ModelThinkingLevel,
    pub gen_config: GenerationConfig, pub tools: Vec<Arc<dyn Tool>>, pub messages: Vec<AgentMessage>,
}
impl RunCtx {
    pub(crate) fn new(shared: RunShared, baseline: RunBaseline, cancel: RunCancel) -> Self { /* … skip_initial_steering_poll: false … */ }
}
```
`#[allow(clippy::too_many_arguments)]` at `:89` is deleted — the lint no longer fires.
`Agent` gains `fn shared(&self) -> RunShared` (nine `clone()`s, replacing `lifecycle.rs:283-291`).
`loop_fn::build_run_ctx` (`:109-164`) builds `RunShared` from its fresh `state`/`subscribers` and
the `AgentLoopConfig` fields, and `RunBaseline` from `AgentContext` + config — the public
`AgentContext` / `AgentLoopConfig` shapes are **unchanged** (they mirror pi and the SDK re-exports
the module wholesale, research §5). `working_messages` (`:120`) becomes `baseline.messages`.

## 4. `loop_fn` collapses its duplicates (SUBTASK3 — `loop_fn.rs`)

What (§0.4): `run_agent_loop_continue` (`:191-201`) and `agent_loop_continue` (`:273-283`) each
replace their two inline `if`s with
`let proof = ResumePoint::check(&context.messages, ContinueSurface::Loop)?;` and build
`RunEntry::Continue(proof)` (the first) / keep spawning the first (the second — its `unwrap_or_default`
comment "Validation already passed" at `:287` stays true). `run_agent_loop` (`:178`) and
`agent_loop` (`:254`) build `RunEntry::Prompt { messages: prompts, source: PromptSource::Fresh }`.
`build_run_ctx` drops its trailing `false` (`:162`).

## 5. `lifecycle.rs` (SUBTASK4) — §1 verbatim

`claim_and_snapshot` replaces `start_run`'s `:235-280`; `spawn_run` is `:282-384` with the
guard passed in (`SettlementGuard` construction at `:315-321` moves to `claim_and_snapshot`;
`result_tx` is assigned in `spawn_run` before the spawn). `continue_run` is §1's body. The
`catch_unwind` failure twin (`:326-379`) is untouched. `RunHandle` and `emit_standalone` untouched.

## 6. Definition of done

- `EntryStart` is gone; `RunEntry` / `PromptSource` / `ResumePoint` exist as in §2; `ResumePoint`
  has exactly one constructor and a private field; no `bool` parameter anywhere on the run entry path.
- `RunCtx::new` takes `(RunShared, RunBaseline, RunCancel)`; the `too_many_arguments` allow is gone.
- `continue_run` and both `prompt`s go through `claim_and_snapshot`; **`spawn_run` contains no
  `st.messages.clone()` and no state write** — the only read of `state.messages` on the run entry
  path is inside `claim_and_snapshot`, after the latch claim, under the same lock as the two
  run-start writes.
- Every `Err` returned after the claim releases the latch via `SettlementGuard::drop`: verify by
  reading that `claim_and_snapshot` builds the guard before returning and that no early return
  in `continue_run` bypasses it.
- `loop_fn.rs` contains two calls to `ResumePoint::check` and no inline `is_assistant()` check.
- Steering / follow-up drain order and requeue-on-failure are unchanged (`:188-217` semantics).
- `gap19_continue_from_assistant_skips_initial_steering_poll` (`tests/model_boundary.rs:484`),
  the `NoMessages` / `ContinueFromAssistant` assertions in `agent_loop.rs`, `round2_parity.rs`,
  `area02_backlog.rs`, `pending_containment.rs` and `settlement_latch.rs` (3, multi-thread) pass
  **unedited** — no test names `EntryStart` or `RunCtx::new`, so no test plumbing is expected.
- `cargo check --workspace --all-targets --features test-fixtures` green; `cargo test -p cyrup-agent`
  green; clippy exits 0 with no warning from a changed file.

## Research notes

Research §3 F1 (all sub-fields), §4 before/after sketch (superseded on the lock ordering by §1
here), §2 rows 1–3. Resolved question 1 stands: the AGENT-030 post-run gap is the session's to
gate (`is_run_active()` = `!is_idle()`, `session/mod.rs:499`); nothing here touches it. The
`RunCtx::messages` element type stays `Vec<Arc<AgentMessage>>` (PERF-002); `RunBaseline.messages`
is the owned `Vec<AgentMessage>` that `RunCtx::new` wraps once (`run/mod.rs:129`). `AgentError`
(`error.rs:86-106`) and `ContinueSurface` (`:68-74`) need no new variants for 3A.

No tests to be written — another team owns tests. No benchmarks to be written.
