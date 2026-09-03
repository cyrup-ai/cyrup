---
stage: qa
status: completed
updated: 2026-09-03 23:20
aug_against: branch 19ef318 (CLTR_1..3A landed) — uncapped sweep; pi settings-manager.ts read for the QueueMode parity question
---

# CLTR_3B — `Agent::edit_transcript`: the session's three pop/set triplets become locked edits (F1, part B)

OBJECTIVE: Add two public `Agent` methods — an atomic, latch-guarded transcript edit and the named
"pop the trailing assistant iff P" operation built on it — and replace the session's three
unlocked `snapshot → mutate → set_messages` triplets with them. Stop `drive_run` swallowing a
`continue_run` error: with the session no longer able to hand the agent an unresumable transcript,
that `Err` is a bug and must be observable. Incidental: `QueueMode: FromStr`. Requires CLTR_3A
(landed at `19ef318`). Source: research §3 F1 (tendrils), §4 sketch, §6 step 3.

> **READ §0 FIRST.** Three of the four subtasks change shape against the previous version: the
> swallow lives in a spawned task with no `Result` to return into; the `prompt` two lines above
> it is swallowed the same way; and pi is *lenient* on queue-mode text, so a hard `Err` there
> would be a parity break, not a fix.

---

## 0. What the sweep found — corrections

**0.1 Exactly the three triplets, and they are the only ones.** Uncapped, every
`snapshot().await.messages` in non-test source: [`retry.rs:142`](../../crates/cyrup-session-svc/src/session/retry.rs),
[`auto_compaction.rs:399`](../../crates/cyrup-session-svc/src/session/auto_compaction.rs),
[`bash.rs:288`](../../crates/cyrup-session-svc/src/session/bash.rs) — and `accessors.rs:316`, which
is the read-only `agent_messages()` getter, not a triplet. Every other `set_messages` caller is a
wholesale re-seed that stays: `auto_compaction.rs:341`, `compaction.rs:256`, `forking.rs:329`
(builds the whole list from `build_context_raw`, no snapshot). `edit_transcript` /
`pop_trailing_assistant_if` do not exist anywhere yet; no collision.

**0.2 `continue_run` has ONE real caller** — [`run.rs:164`](../../crates/cyrup-session-svc/src/session/run.rs)
inside `drive_run`; the other sweep hits are comments. But `drive_run` (`:149`) is
`async fn drive_run(self: Arc<Self>, messages)` — a **spawned driver task with no return type**
(`spawn_run` at `:129-136` sends `driver_tx(true)` and `tokio::spawn`s it). There is no `Result` to
return an `Err` into, so the previous version's "`return Err(SessionServiceError::Agent(e))`" is
impossible as written. The crate's own channel for a background failure is `tracing::warn!`
([`control.rs:257`](../../crates/cyrup-session-svc/src/session/control.rs), `tools.rs:77,86`;
`tracing` is already a dependency). **And `:150` swallows `prompt` the same way** —
`if let Ok(handle) = self.agent.prompt(messages).await` — the identical pattern two lines above
the one the task listed. Both get the same treatment (§4).

**0.3 `ContinueFromAssistant` after `handle_post_agent_run() == true` is a genuine bug — the task's
premise holds.** [`run.rs:218-236`](../../crates/cyrup-session-svc/src/session/run.rs) returns
`true` only after `prepare_retry` (`:221-222`, which calls `drop_trailing_assistant`) or
`check_compaction` (`:235-236`, the narrow pop at `auto_compaction.rs:398-411`). Both pop the
trailing assistant, so after this task a continuation that still fails `ResumePoint::check` means
a pop that should have happened did not.

**0.4 pi is LENIENT on queue-mode text — do not make `parse_queue_mode` a hard error.**
[`settings-manager.ts:745-747`](../../tmp/pi/packages/coding-agent/src/core/settings-manager.ts)
is `getSteeringMode(): "all" | "one-at-a-time" { return this.settings.steeringMode || "one-at-a-time"; }`
— the TS union is erased at runtime and `||` only substitutes for a falsy value, so an
unrecognised string passes through and behaves as one-at-a-time downstream. cyrup's
`parse_queue_mode` ("any non-`all` ⇒ one-at-a-time",
[`builder.rs:1864-1868`](../../crates/cyrup-session-svc/src/builder.rs)) is therefore
**pi-faithful**; the settings getters
([`effective.rs:187-198`](../../crates/cyrup-config/src/settings/effective.rs)) return an
unvalidated `String` defaulting to `"one-at-a-time"`. A hard `Err` would refuse to build a session
that runs fine today and in pi. `ConfigError::SettingsParse` wraps a `serde_json::Error` and is
not a home for it either. §5 keeps the leniency but makes it **observable**.

**0.5 `parse_queue_mode` has FOUR callers, not two:** `builder.rs:1611,1612` and
[`session/mod.rs:327,328`](../../crates/cyrup-session-svc/src/session/mod.rs) (the queue-mode
mirrors). All four keep calling the one boundary function.

**0.6 `set_messages` has no latch check** ([`facade.rs:119-121`](../../crates/cyrup-agent/src/agent/facade.rs)
is a bare `lock(&self.state).messages = msgs`). `edit_transcript` adds one, mirroring
`is_running()` (`:184-186`, `*self.running_rx.borrow()`). `facade.rs` does not import
`AssistantMessage`; `retry.rs` and `auto_compaction.rs` do.

---

## 1. The two public methods (SUBTASK1 — `cyrup-agent`)

What: in [`agent/facade.rs`](../../crates/cyrup-agent/src/agent/facade.rs), beside `set_messages`:
```rust
/// Atomic transcript edit under the state lock — the replacement for every
/// `snapshot → mutate → set_messages` triplet, which spanned two awaits with no lock and could
/// interleave with the reducer. Refused while a run is in flight (the same latch `reset`
/// observes), so it can never race the run's own appends.
///
/// The AGENT-030 post-run gap — after `agent_end` releases this latch but before the session's
/// driver decides whether to continue — is the SESSION's to gate: `is_run_active()` reads
/// `driver_tx`, which the agent cannot see. This method is the second line, not the first.
pub fn edit_transcript<R>(&self, f: impl FnOnce(&mut Vec<AgentMessage>) -> R) -> Result<R, AgentError> {
    if self.is_running() {
        return Err(AgentError::RunActive(BusyEntry::Edit));
    }
    let mut st = lock(&self.state);
    Ok(f(&mut st.messages))
}

/// Pop the trailing assistant message iff `pred` holds for it, returning it. The one operation
/// both session retry predicates need — "any trailing assistant" and "a trailing
/// `Error`/`Length` assistant" — expressed as a predicate rather than as two copies of the pop.
pub fn pop_trailing_assistant_if(
    &self,
    pred: impl FnOnce(&AssistantMessage) -> bool,
) -> Result<Option<Arc<AssistantMessage>>, AgentError> {
    self.edit_transcript(|m| match m.last() {
        Some(AgentMessage::Assistant(a)) if pred(a) => {
            let a = Arc::clone(a);
            m.pop();
            Some(a)
        }
        _ => None,
    })
}
```
Add `use cyrup_core::AssistantMessage;` to `facade.rs`. In [`error.rs:28-41`](../../crates/cyrup-agent/src/error.rs)
add `BusyEntry::Edit` with, in the `Display` at `:43-59`:
```rust
// [CYRUP-DELTA] no pi counterpart — pi edits `this.agent.state.messages` directly from the
// session; cyrup's atomic edit is refused mid-run in the same family of texts as the others.
Self::Edit => "Agent is already processing. Wait for completion before editing the transcript.",
```
Why: the three session sites currently interleave with the reducer; `set_messages` itself stays
for the wholesale re-seeds (§0.1) and is untouched.

## 2. Replace the three triplets (SUBTASK2 — `cyrup-session-svc`)

- [`retry.rs:140-147`](../../crates/cyrup-session-svc/src/session/retry.rs) `drop_trailing_assistant`
  — keep the name and signature (callers unchanged); body becomes
  `let _ = self.agent.pop_trailing_assistant_if(|_| true);` — the `Err` is `RunActive`, which
  cannot occur here (every caller runs inside the session's `is_run_active()` gate); discard it.
- [`auto_compaction.rs:398-411`](../../crates/cyrup-session-svc/src/session/auto_compaction.rs) —
  the narrow pop becomes
  `let _ = self.agent.pop_trailing_assistant_if(|a| matches!(a.stop_reason, cyrup_core::StopReason::Error | cyrup_core::StopReason::Length));`
  The comment at `:395-397` explaining WHY it is narrower than `drop_trailing_assistant` stays;
  its "Do not reuse that helper here" sentence is now moot (the predicate IS the narrowness) —
  reword to say so.
- [`bash.rs:286-296`](../../crates/cyrup-session-svc/src/session/bash.rs) `append_bash_message` —
  `:288-290` becomes `let _ = self.agent.edit_transcript(|m| m.push(msg));`. The persist call at
  `:291-295` is unchanged. Note this runs from `flush_pending_bash_messages` after the run has
  settled and from `record_bash_result` when not streaming (`bash.rs:228-232`), so `RunActive`
  cannot occur; discard it.
Why: each becomes one locked edit under the same lock the reducer uses.

## 3. `drive_run` stops swallowing (SUBTASK3 — `run.rs`)

What: [`run.rs:149-172`](../../crates/cyrup-session-svc/src/session/run.rs). `:170` `Err(_) => break`
becomes
```rust
Err(e) => {
    // Cannot happen once the session's own pops go through `pop_trailing_assistant_if`:
    // `handle_post_agent_run` returned `true` only after `prepare_retry` / `check_compaction`
    // popped the trailing assistant, so a refused continuation here is a bug, not a state.
    tracing::warn!(error = %e, "continue_run refused after a post-run step said to continue");
    break;
}
```
and `:150` `if let Ok(handle) = self.agent.prompt(messages).await {` becomes a `match` whose `Err`
arm does the same `tracing::warn!(error = %e, "prompt refused inside the run driver")` before the
fall-through to the `finally` at `:178` (the `prompt` guard is `RunActive`, which `spawn_run`'s
caller already checked via `is_run_active()`; a refusal here is the check-then-claim race and must
be visible, not silent). Why: `drive_run` is a spawned task — a `Result` cannot surface, and
silence is the exact failure this task exists to remove; `tracing::warn!` is the crate's
convention for it (§0.2).

## 4. `QueueMode: FromStr`, leniency kept and made observable (SUBTASK4)

What: in [`queue.rs`](../../crates/cyrup-agent/src/queue.rs) (derives `Clone, Copy, Debug,
PartialEq, Eq, Default`, no serde — `:7-12`):
```rust
/// The two strings pi accepts (`settings-manager.ts:101-102`, `:745-757`; the RPC arm in
/// `cyrup-modes/src/rpc/types.rs:33-36` emits the same). Strict: anything else is `Err`.
impl std::str::FromStr for QueueMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "all" => Ok(Self::All),
            "one-at-a-time" => Ok(Self::OneAtATime),
            other => Err(format!("unrecognised queue mode {other:?}; expected \"all\" or \"one-at-a-time\"")),
        }
    }
}
```
[`builder.rs:1864-1868`](../../crates/cyrup-session-svc/src/builder.rs) `parse_queue_mode` **stays as
the one settings boundary** (four callers, §0.5) but is rewritten on the parser and keeps pi's
leniency out loud:
```rust
/// The settings `steeringMode`/`followUpMode` string → [`cyrup_agent::QueueMode`]. pi reads the
/// setting with `|| "one-at-a-time"` and never validates it (`settings-manager.ts:745-757`), so
/// an unrecognised value silently runs one-at-a-time. cyrup keeps that behaviour — refusing to
/// build the session over a typo would be a parity break — but says so.
pub(crate) fn parse_queue_mode(s: &str) -> cyrup_agent::QueueMode {
    s.parse().unwrap_or_else(|e| {
        tracing::warn!(value = %s, error = %e, "queue mode setting not recognised; using one-at-a-time as pi does");
        cyrup_agent::QueueMode::OneAtATime
    })
}
```
Why: the typed parser is the door for any caller that wants strictness (the RPC layer already
has `QueueModeArg`); the settings boundary stays pi-faithful and no longer swallows silently.

## 5. Definition of done

- `Agent::edit_transcript` and `Agent::pop_trailing_assistant_if` exist as in §1; `BusyEntry::Edit`
  exists with its `Display` text and delta note.
- Zero occurrences of the `snapshot().await.messages` → mutate → `set_messages` pattern remain in
  non-test session-svc source (only `accessors.rs:316`'s read-only getter reads that field).
  `set_messages` still has its four wholesale re-seed callers (§0.1) and is otherwise untouched.
- `run.rs` has no `Err(_) => break` and no `if let Ok(handle) = self.agent.prompt`; both arms
  `tracing::warn!` with the error.
- `QueueMode: FromStr` is strict; `parse_queue_mode` parses through it, warns on `Err`, and still
  returns `OneAtATime`; all four callers unchanged.
- `round2.rs`, `round3.rs`, `mid_run_tool_anchoring.rs`, `round8_postrun.rs` (session-svc) and
  `settlement_latch.rs` (agent) pass **unedited**. No test names the removed pattern, so no
  plumbing is expected.
- `cargo check --workspace --all-targets --features test-fixtures` green;
  `cargo test -p cyrup-agent -p cyrup-session-svc` green; clippy exits 0 with no warning from a
  changed file; `cargo doc -p cyrup-agent --no-deps` exits 0 (new doc comments).

## Research notes

Research §3 F1 tendrils and boundary; §4 sketch (the `facade.rs` and session blocks — the
`run.rs` line there predates reading `drive_run`'s signature). Resolved question 1 stands and is
now stated on `edit_transcript` itself. pi reference vendored at `tmp/pi` (gitignored;
re-copy per PERF-006 §9.1 if absent).

No tests to be written — another team owns tests. No benchmarks to be written.
