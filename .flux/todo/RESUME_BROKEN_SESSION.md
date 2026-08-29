---
stage: aug
status: done
updated: 2026-08-29 01:47
---

# SEAM-112: /resume Produces A Broken Session

## Objective

`/resume` produces a broken session: **nothing renders, and bash tool calls repeat
endlessly.** Filed 2026-08-15 from live use, rated `critical`.

The render half is closed. **The open question is narrowly this: why do the bash calls
repeat?** Everything below exists to stop the next implementer spending the session
re-deriving what is already settled.

---

## Audit results — settled, do not re-investigate

### The ledger's three candidates: two are dead, one is narrowed

The row lists three candidates. Two are now disproved by reading the code at HEAD.

**Candidate (2) — "the generation bump fires before the new session is fully installed, so
the TUI subscribes to a stream that is then replaced." FALSE, structurally impossible.**
In [`runtime.rs`](../../crates/cyrup-session-svc/src/runtime.rs) `install_inner`, the
session and the generation are assigned under the **same write lock** (`:445` `g.session =
next.clone()`, `:446` `g.generation = new_gen`), and the watch notify (`:449`
`gen_tx.send(new_gen)`) happens only after that lock is dropped. A watcher cannot observe a
bumped generation before the session behind it is installed. There is no window.

**Candidate (3) — "`rebind_session` resets the transcript but nothing drives the new
subscription." FALSE.** `on_session_swapped`
([`run_arms.rs`](../../crates/cyrup-tui/src/app/run_arms.rs) `:143-315`) is thorough: it
re-subscribes (`:163` `*events = new_session.subscribe()`), repoints the loop's session
handle (`:164` `ctx.session = new_session`), and re-seeds the view from the resumed
conversation (`:285` `let restored = ctx.session.replay_items().await`, `:290`
`replay_items_with_extensions`). It also re-installs the ui sinks, the overlay sink, the
extension read-backs, the fault listener, the shortcut set, the auth snapshot, the context
usage and the terminal title. This is why "nothing renders" is closed at HEAD.

**Candidate (1) — "the rebuilt session's tool-result path is not re-wired." NARROWED: the
resume SEED is fine; only the LIVE path remains.** The seed is
[`builder.rs`](../../crates/cyrup-session-svc/src/builder.rs) step 7 (`:1499`), which maps
`existing_raw` through `raw_message_to_agent`
([`event.rs:418`](../../crates/cyrup-session-svc/src/event.rs)). That function's `Raw::Core`
arm delegates to `core_message_to_agent`, which carries an explicit
`Message::ToolResult → AgentMessage::ToolResult` arm preserving `tool_call_id`,
`tool_name`, `content` and `is_error`. Every pi role is pinned by
`agent_transcript_raw_seed.rs:454`. **Tool results survive the resume seed.** Do not spend
time there.

### The row's own citations have drifted — navigate by symbol, not line

The row says *"Wiring verified at HEAD — do not begin by re-deriving it"* and then gives line
numbers. The modules have been split since it was filed, so several no longer resolve. An
implementer trusting them lands on comment lines or missing files:

| Row cites | Reality at HEAD |
| --- | --- |
| `run.rs:344` (`Some(ev) = events.next()`) | **`:397`** — `:344` is a `bash_running()` guard |
| `run.rs:293` (swap arm hoisted) | **`:331`** — `swapped = session_swapped` |
| `run_arms.rs:158` (re-subscribe) | **`:163`/`:164`** |
| `agent.rs:1985-2026` (`continue_run`) | **file no longer exists** — split into [`agent/lifecycle.rs:209`](../../crates/cyrup-agent/src/agent/lifecycle.rs) and [`loop_fn.rs:200`,`:280`](../../crates/cyrup-agent/src/loop_fn.rs) |
| `session.rs:5550-5562` (`record_bash_result`) | **file no longer exists** at that path |
| `subscriber.rs:89-93` (`Fanout::invalidate`) | correct |
| `runtime.rs:513` (`switch_session_with`) | correct |
| `session_bind.rs:4` (`rebind_session`) | correct |

The four named test files all exist:
[`runtime_swap.rs`](../../crates/cyrup-tui/src/tests/runtime_swap.rs) (161),
[`extension_ui_reset_on_swap.rs`](../../crates/cyrup-tui/src/tests/extension_ui_reset_on_swap.rs) (74),
[`session_start_lifecycle.rs`](../../crates/cyrup-session-svc/src/tests/session_start_lifecycle.rs) (176),
[`run_loop_swap_arm_reachable.rs`](../../crates/cyrup-tui/src/tests/run_loop_swap_arm_reachable.rs) (124).

### Where the in-source SEAM-112 markers actually are

This is the finding that reframes the row. The markers are **not** on the swap path. Every
one is on **compaction / re-seed / retry**:

| Marker | Says |
| --- | --- |
| [`auto_compaction.rs:307`](../../crates/cyrup-session-svc/src/session/auto_compaction.rs) | the re-seed is on the SUCCESS PATH ONLY |
| `auto_compaction.rs:375` | after a compaction that will retry, re-drop the retriable trailing assistant |
| [`compaction.rs:216`](../../crates/cyrup-session-svc/src/session/compaction.rs) | same success-path-only ordering |
| [`round8_postrun.rs:177`](../../crates/cyrup-session-svc/src/tests/round8_postrun.rs) | **"after a successful OVERFLOW compaction the interrupted turn must actually be RETRIED"** |
| [`compact_refusals.rs:469`](../../crates/cyrup-session-svc/src/tests/compact_refusals.rs) | a FAILED compaction must leave `agent.state.messages` exactly as it found it |
| `builder.rs:824`, `:1499` | the raw projection and the resume seed |

### The mechanism this suggests — the hypothesis to test first

Every re-seed site replaces `agent.state.messages` **wholesale from the persisted
projection** (`self.agent.set_messages(compacted_messages)`, `auto_compaction.rs` ~`:341`).
The live tool-result path appends to agent state at
[`agent/run/turn.rs:76-77`](../../crates/cyrup-agent/src/agent/run/turn.rs) — into **both**
`self.messages` and `self.new_messages` — with the tool sites at
[`tools/exec.rs:248`,`:361`](../../crates/cyrup-agent/src/agent/run/tools/exec.rs) and
[`tools/mod.rs:109`](../../crates/cyrup-agent/src/agent/run/tools/mod.rs).

**So: a tool result that is in agent state but not yet persisted as a session entry is
erased by any wholesale re-seed.** The model then sees its own `tool_use` unanswered and
re-issues the identical call. If the condition that triggered the re-seed still holds, the
cycle repeats — endlessly, with the same command, which is exactly the reported symptom.

A resumed session is the natural way to enter that state: it starts at or near the context
limit, so the first turn can overflow immediately and drive compaction on every attempt.

**This is a hypothesis with a specific check, not a conclusion. It is unproven and must not
be treated as the cause until the live run below confirms it.** It is recorded because it
explains BOTH symptoms from one cause and the ledger's three candidates do not.

---

## The work

### 1. Reproduce ONCE, with instrumentation — before changing anything

The ledger is explicit and it is right: **do not characterise this by re-running it.** One
observation, read the log.

Add temporary `tracing` at exactly these four points:

- **tool result appended** — [`agent/run/turn.rs:76`](../../crates/cyrup-agent/src/agent/run/turn.rs):
  log `tool_call_id`, `tool_name`, and `self.messages.len()` after the push.
- **agent state re-seeded** — `auto_compaction.rs` at the `set_messages` call (~`:341`) and
  every other `set_messages` site: log the incoming length and whether the last message is
  an `Assistant` with an unanswered `ToolUse`.
- **TUI re-subscribe / rebind** — [`run_arms.rs:163-164`](../../crates/cyrup-tui/src/app/run_arms.rs):
  log the generation being bound and `restored.len()` at `:285`.
- **run continuation refused** — [`agent/lifecycle.rs:209`](../../crates/cyrup-agent/src/agent/lifecycle.rs)
  and [`loop_fn.rs:200`,`:280`](../../crates/cyrup-agent/src/loop_fn.rs): log each
  `ContinueFromAssistant`.

Then run **one** `/resume` on a session large enough to overflow, let the bash call repeat
two or three times, and stop.

### 2. Read the log against this decision tree

- **A tool result is appended, then a re-seed drops the list back to a length that excludes
  it** → the hypothesis above is confirmed. Fix at the re-seed: the re-seed must not discard
  agent-state messages that have no persisted entry yet. Follow the ordering already
  established at `auto_compaction.rs:307` (success path only) and `:375` (re-drop the
  retriable tail) rather than inventing a new sequencing.
- **No tool result is ever appended** → the failure is upstream in the tool execution path
  (`tools/exec.rs:248`/`:361`); the result never reaches `turn.rs:76`.
- **The result is appended and survives, but the model still repeats** → the request
  projection is dropping it at the LLM boundary; look at `convertToLlm`
  ([`hooks.rs:185`](../../crates/cyrup-agent/src/hooks.rs)), where `AgentMessage::ToolResult`
  maps back to `Message::ToolResult`, and check `tool_call_id` correlation.
- **`ContinueFromAssistant` fires each cycle** → the interrupted-turn retry is the driver;
  the `will_retry` branch at `auto_compaction.rs:375` is the site.

### 3. Fix at the root cause

Apply the fix at whichever of the four the log names. Do not fix the symptom — suppressing a
repeated call without explaining the erased result leaves the session silently lossy.

### 4. Correct the row's stale citations

While in the file, repoint the drifted line references in the SEAM-112 rows
([`08-cyrup-session-svc-and-modes.md:410`](../../docs/gap-analysis/08-cyrup-session-svc-and-modes.md),
[`00-residual-ledger.md:24`](../../docs/gap-analysis/00-residual-ledger.md)) to the symbols
in the drift table above. The row's value is that it tells the next reader not to
re-derive; broken citations destroy exactly that value.

---

## Out of scope

- The swap path itself (`on_session_swapped`, `install_inner`, `Fanout::invalidate`). Two of
  three ledger candidates were disproved there; it is correct at HEAD.
- The resume seed's role/tool-result fidelity — verified, and pinned by
  `agent_transcript_raw_seed.rs:454`.
- The "nothing renders" half, closed by `879eb4e`.
- Any change that makes the repeat *quieter* rather than explaining the erased result.

---

## Definition of done

1. One instrumented `/resume` captured, and the log names which of the four decision-tree
   branches fired.
2. The mechanism by which a tool result fails to reach the model is stated in one sentence,
   grounded in that log — not inferred from reading.
3. The fix is applied at that mechanism, and a second `/resume` under the same conditions no
   longer repeats the call.
4. The temporary instrumentation is removed.
5. `cargo check --workspace --all-targets` is clean.
6. The SEAM-112 rows carry the corrected citations.
