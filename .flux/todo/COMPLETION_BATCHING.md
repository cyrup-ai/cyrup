---
title: Completion Batching (SUBA-017)
priority: LOW
stage: augment
status: ready
updated: 2026-09-22 (research pass at HEAD 521beaa, upstream pinned v0.68.0)
---

# SUBA-017: completion batching

> Branch `claude/subagents-delegation`, HEAD `521beaa`.
> Upstream is pinned: `git -C tmp/pi-subagents show v0.68.0:<path>`. The batcher file is
> byte-identical at `v0.43.0` and `v0.68.0` (`git diff --stat v0.43.0 v0.68.0 --
> src/runs/background/completion-batcher.ts` is empty), so it is in-baseline and `not-ported` is
> still the right kind.
> Ledger row: `docs/gap-analysis/09-cyrup-ext-subagents.md:616`. Body: `:1153-1180`.
> Status table: `:435`.

## Verdict in one line

**Still open, but the premise is half wrong.** cyrup does not deliver N completions as N turns. It
has coalesced queued injections into one turn since `8de7460` (2026-09-08), in the session pump
(`crates/cyrup-session-svc/src/session/mod.rs:624-690`, `merge_injection_batch` at
`session/inject.rs:289`). Both re-reads (2026-09-14 and 2026-09-16) missed it because they only
grepped `cyrup-ext-subagents`. What cyrup lacks is upstream's **lead-in debounce**. Without it, the
first completion of a burst starts a turn by itself while its siblings are still being scanned, and
the pump has a re-drain gap that can add a third turn. The real difference is **1 turn vs 2–3**,
not 1 vs N. Two more pieces are also missing: upstream's grouped notice text
(`formatGroupedCompletion`) and the `completionBatch` config key.

---

## 1. Citation audit

| Row / body claim | At HEAD | Verdict |
|---|---|---|
| "cyrup delivers each completion separately", body Impact line: "Ten background runs … produce ten separate notices" | The session pump merges every queued `subagent-notify` into ONE `AgentMessage` and runs ONE turn over the batch (`session/mod.rs:643` `try_recv` drain, `:656` `merge_injection_batch`, `inject.rs:289-344`). The bodies are joined with `"\n\n"` and `display`/`trigger_turn` are OR'd (`inject.rs:250-258`). | **FALSE since `8de7460`.** Ten same-type notices become one or two messages, not ten turns. See §3 for how many turns a burst actually costs. |
| `watch/install.rs:304` "still hands the sink one message per result" | `:304` is now the `sink` **parameter** of `deliver_pending_completions`. The hand-off is `install.rs:416-418` (owned band) and `:454-456` (missing band). Each runs in a **spawned task** on the `DeliveryFleet` `JoinSet` (`:93-107`, ASYNC_NOTIFY F1), not inline. | **Stale line.** The one-message-per-run substance is still true at the sink boundary. |
| "config struct is now 34 fields (`registration/mod.rs:79`)" | 35 `pub` fields. The struct is at `registration/mod.rs:81` with `#[serde(rename_all = "camelCase", default)]` at `:80` and no `deny_unknown_fields`, so a user's `"completionBatch"` key parses today and is **silently ignored**. | **Stale count.** No `completion_batch` field. |
| `completion_batch\|batcher\|CompletionBatch` zero-hit | Re-run across `crates/` under every plausible name (`completion_batch`, `completionBatch`, `batcher`, `CompletionBatch`, `debounce`, `straggler`, `coalesc`, `grouped_completion`, `format_grouped`, `max_wait_ms`, `"Background tasks completed"`). The only debounce is the **control-notice** debounce (`tui/notices.rs:75` `DEBOUNCE_MS`, 1000 ms, foreground control notices only). The only coalescing on this path is the session pump above. | **True for this crate. Misleading for the workspace.** |
| Upstream `extension/index.ts:495` @v0.68.0, `:504` @v0.67.0, `:376` @v0.43.0 | All three verified with `git show <tag>:src/extension/index.ts`. | Correct. |
| `shared/types.ts:2663` key, `notify.ts:544/692/723/790` | `:2663` `completionBatch?: CompletionBatchConfig` ✓. The interface is at `:351-365`. `:544` `completionBatchKey` ✓, `:692` resolve ✓, `:723` key lookup inside `getBatcher` (`:722`) ✓, `:790` `batch_deferred` trace, with the push at `:791` ✓. | Correct. |
| Body Fix: "Port as `background/batcher.rs` between `CompletionWatcher` and `CompletionSink`" | Wrong seam. Putting it there would sit ABOVE `InlineAnsweredSink` and would lose enqueue ordering. See §4. | **Wrong. Replace it.** |
| Body Fix: "Same seam as SUBA-034 and SUBA-056 — do them together" | SUBA-034 closed 2026-08-15 (`:623`, the `CompletionBus` wake). SUBA-056 closed 2026-09-16 (`:613`). | **Obsolete.** Nothing left to do together. |
| Body: "its own test pins the one-notify-per-result contract" | `install.rs:807` `install_completion_watcher_fires_exactly_one_notify_and_deletes_the_result` ✓. It uses a direct `CapturingSink`, so a batcher placed in the host sink (§4) does not disturb it. | Correct. |
| cyrup doc `notices.rs:595` cites `wait-subscriptions.ts` `pi.events.on(…)` "`:285`" with no tag | At v0.68.0 the listener is `:287` (`wakeChannels.map(... pi.events.on(channel, reconcile))`). | Minor drift, unrelated to this row. |

---

## 2. Upstream, pinned at v0.68.0 (load-bearing quotes)

### 2a. The batcher: `src/runs/background/completion-batcher.ts` (168 L)

Contract, `:4-13`:

> Holds successful async-completion notifications briefly so sibling jobs that finish within a
> short window arrive as a single grouped message. A hard max-wait cap (measured from the first
> item in a group) prevents holding notifications indefinitely. After a group is emitted,
> late-finishing siblings that arrive within the straggler window join a shorter "straggler" group
> with reduced debounce and max-wait timers.
>
> Failure and attention signals bypass this batcher entirely. Callers must flush() held items and
> emit those signals immediately so failures and needs-attention notices are never delayed.

Defaults, `:29-36`: `enabled: true, debounceMs: 150, maxWaitMs: 1000, stragglerDebounceMs: 75,
stragglerMaxWaitMs: 400, stragglerWindowMs: 2000`.

Lenient per-field resolution, `:38-59`. `parsePositiveInt` accepts only a finite integer ≥ 1. Any
other value falls through to the global value and then to the default **for that field alone**, and
`enabled` must be a real boolean. The upstream test `completion-batcher.test.ts:60` feeds `enabled:
"false"`, `debounceMs: 0`, `maxWaitMs: -5`, `stragglerDebounceMs: 1.5` and `NaN`, and every one of
them falls back.

The state machine, `:116-167`:
- `push` (`:143-159`):
  - The group's straggler flag is decided when the group opens (`pending.length === 0`): `straggler
    = lastEmitAt !== null && now - lastEmitAt < stragglerWindowMs`.
  - Every push **resets** the debounce timer.
  - The max-wait timer is armed **once per group**.
- `emitGroup` (`:133-140`) clears both timers and **swaps `pending` out synchronously** before it
  calls `emit`, then stamps `lastEmitAt`.
- `flush = emitGroup`.
- `dispose` (`:161-166`) clears the timers and **returns the unemitted items without emitting
  them**.
- Disabled mode (`:106-114`) calls `emit([item])` immediately.

### 2b. Who calls it: `src/runs/background/notify.ts`

- `:692-693`: one resolved config and a `Map<string, CompletionBatcher>`, **one batcher per key**.
- `:544-549`: the key is `session:<sessionId>`, else `cwd:<cwd>`, else `"unknown"`.
- `:780-791`, the routing rules:
  ```ts
  if (details.source === "foreground") { emit([item]); return completion; }
  const batcher = getBatcher(result);
  if (details.status !== "completed") { batcher.flush(); emit([item]); return completion; }
  if (batchConfig.enabled) traceNotification("batch_deferred", item.trace);
  batcher.push(item);
  ```
  Only `completed` async successes are batched. Anything else flushes **its own key's** held group
  first and is then emitted separately. The test at `notify.test.ts:493` confirms that another
  session's failure does not flush this session's group.
- `:708-721` `emit`: **ownership is re-checked at emit time**, because a delayed batch can outlive a
  session switch (test `notify.test.ts:222`, "rechecks session ownership before emitting a delayed
  batch"). A rejected item settles `false`.
- `:524-542` `sendCompletion`: one `pi.sendMessage` per group. `content` is `formatSingleCompletion`
  for 1 item and `formatGroupedCompletion` for more. `display` is OR'd over `source ===
  "foreground" || status !== "completed" || scheduleOrigin` (so a group of plain successes is
  **hidden**). `triggerTurn` is OR'd over the items.
- `:472-489` `formatGroupedCompletion`: the header `Background tasks completed (N): **a**, **b**`,
  then per item `i. agent[taskInfo][ — scheduled run from NAME (schedule ID)]`, an optional
  `Workflow receipt:`, the preview, watchdog blockers, the handoff, correlation lines, the session
  line, and a blank line.
- `:802-817` `dispose`: every batcher's `dispose()` items are settled `false` with reason
  `dispose_pending`, and **nothing is emitted**.

### 2c. Durability upstream: the result file is the source of truth

`result-watcher.ts:553-577`: `const accepted = await notifier.deliver({...})`. If it is not
accepted, `scheduleResult(file, …, RETRY_DELAY_MS)` runs and the file stays. The file is marked
delivered (`:580`) and removed (`:620`) only after acceptance. So a batch disposed at shutdown
**loses nothing**: its items resolve `false` and the files are redelivered by the next watcher.

### 2d. Interactions asked about

- **`bg_wait`: no interaction.** `wait-completions.ts:180-184` says the wait's read is
  "deliberately read-only — the watcher owns notification and cleanup". Upstream therefore both
  answers inline in `bg_wait` **and** later sends the (batched) notice. It has no suppression.
- **Wait subscriptions: separate channel, never batched.** `wait-subscriptions.ts:182-205` sends
  its own `customType: "subagent-wait-subscription"` message with `triggerTurn: true`. It reconciles
  on `SUBAGENT_ASYNC_COMPLETE_EVENT` among others (`:279-287`). **That event is emitted at
  `result-watcher.ts:589`, AFTER `await notifier.deliver` (`:553`)**, so upstream's batch window
  (≤ `maxWaitMs`) delays every async-complete listener, the subscription reconcile included.
- **Fleet widget: no interaction.** It is driven by the job tracker and fleet status
  (`extension/index.ts:497-511`, `:566-570`), not by the notifier. The only other consumer of
  batcher state is `hasPendingDelivery()` feeding pi-web session liveness (`index.ts:1174`).
- **Foreground (detached) completions bypass the batcher** (`notify.ts:780-782`).

---

## 3. What cyrup has at HEAD (grepped, with the path traced)

### 3a. From result file to the parent

1. **Scan.** A `notify::PollWatcher` at `RESULTS_DIR_POLL_INTERVAL = 500ms`
   (`watch/results_watcher.rs:33`, `:349-352`) plus a 500 ms tick (`install.rs:228-229`). **Result
   arrivals are therefore quantised to 500 ms buckets.** Upstream uses `fs.watch` plus a 50 ms file
   coalescer (`result-watcher.ts:632-636`).
2. **Phase 1, observe, synchronously per completion** (`install.rs:373-386`). The
   `CompositeCompletionObserver` (`extension/executor/notices.rs:537-608`) runs the
   `WaitCompletionStore` (durable replay write), mission sync, the **`CompletionBus`** (the edge
   `bg_wait` selects on), the bus announcer, the process-terminal announcer, and last the
   wait-subscription reconciler.
3. **Phase 2, deliver concurrently.** A `JoinSet` task per run (`install.rs:388-427`), guarded by
   the `in_flight` set against double-spawn.
4. **Sink chain** (`notices.rs:528-531`): `InlineAnsweredSink(effective_completion_sink())`.
   - `InlineAnsweredSink` (`watch/sink.rs:331-364`) holds a `suppressible` delivery while a live
     `wait` claims that run (`claim_released`, bounded by `INLINE_CLAIM_MAX_WAIT`). If the wait
     answered it inline, the decorator mints a receipt **without injecting**.
     **[CYRUP-DELTA] over upstream**, which double-delivers.
   - The inner sink is `HostServicesCompletionSink` whenever host services are bound
     (`notices.rs:211-227`). Its `deliver` makes a **synchronous** `inject_message_ack(...)` call
     (`sink.rs:102-108`) and then awaits the ack (`:112-117`). `Accepted` →
     `CompletionDelivery::delivered(run_id)`. Anything else → `Deferred`.
5. **Settle.** `settle_delivery` (`install.rs:164-200`): consume the payload **only** against the
   `DeliveryReceipt` (`delivery/receipt.rs:27-48`, one mint, not `Clone`). `Deferred` → retry in
   place with the attempt bound.
6. **Session pump** (`cyrup-session-svc/src/session/mod.rs:624-690`, single consumer):
   ```rust
   if inbox.is_empty() { match rx.recv().await { … } }   // :631
   while let Ok(next) = rx.try_recv() { inbox.push(next); } // :643
   …
   session.wait_for_idle().await;                           // :655
   let plan = merge_injection_batch(&inbox);                // :656
   ```
   `merge_injection_batch` (`inject.rs:275-344`) groups by `custom_type` in insertion order, joins
   the bodies with `"\n\n"`, ORs `display`/`trigger_turn`, and every `trigger_turn` group goes into
   **one** turn (`deliver_injection_inbox`, `inject.rs:125-142`). The acks resolve `Accepted` on
   `Taken` (`mod.rs:661-667`).

### 3b. Turn count for a burst of N sibling successes (reasoned from the code; not measured)

- **Parent idle, all N in one 500 ms bucket.** Task #1 enqueues while the scan is still doing Phase
  1 disk writes for #2..N (the replay store writes to `<results_dir>/completion-replay/`, and
  `remove_mission_observer_index` touches disk). The pump wakes on `recv`, `try_recv` finds nothing
  yet, `wait_for_idle` returns at once, and **turn 1 carries c1 alone**. c2..cN queue during turn 1
  and become **turn 2**. Result: **2 turns**, and the first one reacts to 1/N of the fleet.
- **Parent busy.** The pump `recv`s c2, drains what is queued at that instant, then parks in
  `wait_for_idle` (`:655`) **without re-draining**. c3..cN arriving during the rest of the turn stay
  in `rx` and become a separate turn after c2's. Result: **up to 3 turns** (c1 | c2 | c3..cN).
- **Upstream**, for a burst inside 150 ms (max 1 s): **1 message**.

So the delta is real and user-facing, but it is **2–3 → 1**, not **N → 1**.

### 3c. Grep for absence (all run at `521beaa`)

`git grep -n -i -E "completion_batch|completionBatch|batcher|CompletionBatch|debounce|straggler|coalesc|grouped_completion|format_grouped|GroupedCompletion|max_wait_ms|Background tasks completed" -- crates/`
finds no completion batcher and no grouped notice. The hits are the control-notice debounce
(`tui/notices.rs`, `extension/executor/notices.rs:259-264,489-494`), model-exclusions persist
debounce, herdr reporter lanes, the ACP ledger, and `notify`-crate event-coalescing comments.
`merge_injection_batch` has **zero tests**: `git grep -n merge_injection_batch -- crates/` returns
only the definition and the one call. The coalescing that prevents N turns today is untested.

---

## 4. The right seam

### 4a. Where the batcher goes

**Inside the sink chain, BELOW `InlineAnsweredSink` and AT the injector:**

```
InlineAnsweredSink( BatchingHostServicesCompletionSink(services, cfg, ownership) )
```

Why each alternative fails:

- **Above `InlineAnsweredSink`, or "between watcher and sink" as the old row said.** A group
  containing a run a live `wait` has claimed would either (a) hold every sibling hostage for up to
  `INLINE_CLAIM_MAX_WAIT` (a whole wait timeout), or (b) flush the claimed run into the grouped
  text and **duplicate** a value the wait already put in the transcript. Below the decorator, a
  claimed run simply enters the batcher late, or never if it was answered, and its siblings flush
  on their own clock.
- **Wrapping the generic `Arc<dyn CompletionSink>`.** `CompletionSink::deliver` fuses *enqueue*
  and *await ack* into one future (`sink.rs:102-117`). A failure that must go out "immediately,
  after flushing held successes" (`notify.ts:785-788`) cannot be **ordered** behind the group's
  enqueue without either awaiting the group's ack (up to a whole parent turn, which delays the
  failure) or racing it. Only the injector can guarantee order, because
  `HostServices::inject_message_ack` enqueues **synchronously**. So the batcher calls
  `inject_message_ack` itself, under its own `std::sync::Mutex`: flush the group, then enqueue the
  bypass item, in call order. The pump's merge is insertion-ordered (`inject.rs:292-293`), so the
  order survives into the turn.
- **In the session pump (a debounce before `merge_injection_batch`).** The pump is generic. It
  also carries steers, watchdog warnings and intercom messages, and a lead-in delay there taxes
  all of them. It also lacks the status/suppressible information that the bypass rule needs, and
  it cannot read `SubagentExtensionConfig`.
- **Before Phase 1 observation.** Never. cyrup observes BEFORE delivering (`install.rs:373-386`),
  so `bg_wait`'s bus wake and the wait-subscription reconcile are **not delayed by the batch
  window**. Upstream's are (§2d). **Keep this. It is a place where cyrup is already better.**

Every delivery path goes through exactly once, because the watcher's `in_flight` set
(`install.rs:392-400`) still guarantees one live `deliver` per run and the batcher only defers
**when** that one call enqueues. Wait subscriptions (`extension/executor/wait_subscriptions.rs:179`)
keep their own `inject_message_ack`, as upstream keeps its own `sendMessage`. The pump already puts
them in the same turn when they are queued together.

### 4b. A completion that `bg_wait` is also awaiting

It is handled by the decorator above the batcher and needs no new code:
- The wait claims the run before its first `done()` check (`background/wait.rs:1527-1534`) or
  before the payload read (`:2082-2092`, `:2124-2130`).
- The delivery parks in `InlineAnsweredSink` (`sink.rs:357-358`).
- On release, `take_answered` suppresses it with a receipt (`:359-361`), so it never reaches the
  batcher, or it lets it through un-answered into whatever group is open at that moment.
- `bg_wait`'s own wake is Phase 1 and does not depend on the batcher.

### 4c. Rust shape (proposed; real anchor lines)

New `crates/cyrup-ext-subagents/src/background/watch/batch.rs`, exported next to the other sinks at
`watch/mod.rs:149-150`.

- **Functional core, no tokio:**
  - `CompletionBatchConfig { enabled, debounce, max_wait, straggler_debounce, straggler_max_wait,
    straggler_window }`, with `DEFAULT` equal to upstream `:29-36`.
  - `resolve_completion_batch_config(Option<&serde_json::Value>) -> CompletionBatchConfig`, with
    `parsePositiveInt` semantics per field and `enabled` only from a real bool.
  - `BatchState<T>`: `push(item, now) -> Deadlines`, `due(now) -> Option<Vec<T>>`,
    `flush() -> Vec<T>`, `dispose() -> Vec<T>`. It keeps upstream's straggler rule verbatim and
    swaps `pending` out atomically on emit.
  - `format_grouped_completion(&[GroupPart]) -> String`, a port of `notify.ts:472-489` over the
    fields cyrup has (agent, schedule origin, preview, session line; `taskInfo` is already folded
    into cyrup's summary by `child_position`, `message.rs:136-`).
- **Imperative shell:** `BatchingHostServicesCompletionSink { services, state:
  Mutex<HashMap<SessionId, BatchState<Pending>>>, cfg, ownership: ResultDeliveryOwnership }`,
  `impl CompletionSink`.
  - **Bypass:** when `!message.suppressible` or the outcome is not `Completed`: lock, drain the
    item's session group and `inject_message_ack` it, `inject_message_ack` this item, unlock, and
    await its own ack. When `cfg.enabled == false`: `inject_message_ack` this item alone, like
    today's `HostServicesCompletionSink`, with no flush.
  - **Held:** push `Pending { run_id, session_id, owner_id, part, trigger_turn, display, tx:
    oneshot::Sender<CompletionDelivery> }`, arm or refresh a flusher task (holding a
    **`Weak`** to the sink), and await `rx`. A dropped sender maps to `Deferred`.
  - **Flush:**
    1. Take the group.
    2. Drop members whose `tx.is_closed()` (the caller was aborted, e.g. the watcher handle was
       dropped).
    3. Re-check `ownership.snapshot().owns(session, owner)` per member, as upstream `:708-721`
       does, and send `Deferred` to the losers.
    4. Build one message: `display = any`, `trigger_turn = any`, content single-or-grouped.
    5. Call `inject_message_ack` once.
    6. Spawn one ack-waiter that answers **each** member with `CompletionDelivery::delivered(its
       own run_id)` on `Accepted`, else `Deferred`.
  - Minting per-member receipts through the public `CompletionDelivery::delivered`
    (`receipt.rs:70-72`) follows the precedent in the doc at `sink.rs:327-329`: sinks are the
    receipt authority.
- **`CompletionMessage`** (`message.rs:69-94`) gains `group_part: Option<GroupPart>`, set only by
  `format_completion_message` (`:288-345`). The grouped renderer must never re-parse `content`.
  The two loss formatters (`:366`, `:431`) set `None`. Struct literals exist only at `:333`,
  `:414`, `:438` and the two test helpers at `sink.rs:415`, `:432`.
- **Config:** `registration/mod.rs:81` gains `pub completion_batch: Option<serde_json::Value>`,
  **not** a typed struct. Sibling fields already use `Value` for leniency (`turn_budget`,
  `timeout_ms`, `permissions`). A typed `u64` would make `"debounceMs": -5` fail the **whole**
  `config.json` parse, where upstream falls back per field.
- **Wiring:** `install_completion_watcher` (`notices.rs:504-535`) already reads
  `config_snapshot()` at `:505`. Resolve the batch config there and pass it to
  `effective_completion_sink` (`:211-227`). Only the **host arm** (`:221-224`) gets the batcher.
  The override arm (tests) and the `LoggingCompletionSink` arm (no turn to save) stay as they are,
  so every existing per-run-counting test keeps its meaning. "Changes apply on the next session
  start" (upstream `docs/configuration.md` `completionBatch`) falls out of reinstall-per-
  `SessionStart` (`extension/host/native_impl.rs:489`).

### 4d. Session-svc: an independent S fix, recommended FIRST

`session/mod.rs:655-656`: add `while let Ok(next) = rx.try_recv() { inbox.push(next); }` between
`wait_for_idle().await` and `merge_injection_batch`. This closes the busy-parent case (3 → 2 turns)
for **every** producer, costs no latency, and needs no config. Add unit tests for
`merge_injection_batch`, which has none. Upstream has no equivalent seam to copy.

---

## 5. Correctness hazards

1. **Ordering.**
   - Within a group, keep arrival order. The pump's merge is insertion-ordered and the grouped
     numbering must match it.
   - Across group and bypass, flush-then-bypass must both happen under one lock, as two
     synchronous `inject_message_ack` calls (§4a).
   - Across keys, another session's failure must not flush this session's group (upstream test
     `notify.test.ts:493`).
2. **A completion that arrives during a flush.** `emitGroup` swaps `pending` out before emitting
   (`completion-batcher.ts:136-137`), so the new push opens a fresh group, which will be a
   straggler group. The Rust shell must `std::mem::take` under the lock **before** the ack await,
   and must never hold the lock across `.await`. The ack can take a whole parent turn, because the
   pump answers only on `Taken`.
3. **Shutdown or teardown with a pending batch: nothing may be lost.** It isn't, structurally.
   Payloads are consumed only against a receipt (`install.rs:164-181`), and no receipt exists for
   an un-injected member. `CompletionWatcherHandle::drop` aborts the drain task
   (`install.rs:37-41`). That drops the `JoinSet`, which aborts every parked `deliver` future, so
   their `oneshot` receivers close. The flusher must then **skip closed members** and must not
   keep the sink alive on its own (hold a `Weak`). If it did either, a stale flush after a
   `SessionStart` reinstall would inject old-session completions through the **late-bound**
   host-services slot into the **new** session and still leave the payloads on disk, so they would
   be announced twice. The un-injected payloads stay on disk, and the next watcher's prime pass
   (`install.rs:217`) delivers them. This matches upstream's `dispose` → `false` → file stays
   (§2c).
   - Residual at-least-once: if a group is injected and accepted and the process dies before
     `consume`, the next session re-announces it. That is already true per item today; the batcher
     widens the window by ≤ `max_wait`.
   - cyrup has no `hasPendingDelivery` liveness hook (`git grep -i has_pending_delivery` → 0). In
     headless `-p` the auto-drain `wait` (`native_impl.rs:625-650`) answers inline and the
     decorator suppresses, so a pending batch at exit costs a notice, not a result.
4. **Straddling a turn boundary.** A batch never splits a turn. The group is one inject, and the
   pump waits for idle. The cases:
   - The window opens while the parent is idle and the user submits during it: the user's turn
     wins and the group becomes the next turn. That is today's behaviour, plus ≤ 1 s.
   - The window opens mid-turn: no cost, because the pump would have waited anyway.
   - A **session switch** inside the window: re-check ownership at flush (hazard 3 plus upstream
     `:708-721`). A rejected member gets `Deferred`. Note that `Deferred` feeds
     `record_processing_failure` (`install.rs:183-197`) and so burns one of the
     `MAX_PROCESSING_ATTEMPTS`. That is acceptable only because a switch also reinstalls the
     watcher and aborts the old callers first. The test in §6 #16 must pin this.
5. **The 500 ms poll quantum versus the 150 ms debounce.** This one is cyrup-specific. Siblings
   that finish 0–500 ms apart but land in **different** poll buckets reach the batcher about
   500 ms apart, after upstream's 150 ms debounce has already fired. With an idle parent that means
   two turns again. Upstream's defaults assume `fs.watch` latency. Options, which the implementer
   must pick and record as `[CYRUP-DELTA]` if not upstream's:
   - (a) keep 150/1000 for parity and accept that only same-bucket siblings group;
   - (b) default `debounce` to `RESULTS_DIR_POLL_INTERVAL + margin` (≈ 650 ms) with `max_wait`
     1000, so any two adjacent buckets group, while a user-set `debounceMs` still wins.

   (b) is the better product. (a) is the parity reading. Do not decide silently.
6. **The loss notices must bypass.** `format_undeliverable_message` / `format_missing_payload_message`
   are `suppressible: false` (`message.rs:420`, `:446`). They are failure-class and must never wait
   in a group. Route on `!suppressible || outcome != Completed`, not on outcome alone.
7. **A quiet schedule.** `trigger_turn = false` for a quiet schedule's success (`message.rs:54-60`,
   `:340`). A group ORs it with its siblings (upstream `:536`), so a quiet success batched with a
   loud one wakes the turn. That is upstream's behaviour; keep it.

---

## 6. Tests, each with the mutation that must kill it

The pure core runs on injected `Instant`s, with no tokio. The shell uses `#[tokio::test(start_paused
= true)]`: `tokio` `test-util` is already a dev-dep (`Cargo.toml:233-243`). The recording
`HostServices` is shaped like `cyrup-it/tests/subagents/companions_hostservices_proof.rs:219-233`,
with a controllable ack.

| # | Test | Killing mutation |
|---|---|---|
| 1 | `a_burst_inside_the_debounce_emits_one_group` | emit on every push / debounce not reset on push |
| 2 | `the_max_wait_cap_emits_while_items_keep_arriving` (port of `completion-batcher.test.ts:114`) | re-arm max-wait on every push |
| 3 | `a_sibling_inside_the_straggler_window_uses_straggler_timers` (`:182`) | `straggler` hard-wired `false` |
| 4 | `after_the_straggler_window_a_fresh_normal_group_starts` (`:207`) | straggler flag never recomputed when a group opens |
| 5 | `disabled_config_emits_each_item_immediately` (`:80`) | `enabled` ignored |
| 6 | `resolve_falls_back_per_field_on_invalid_values` (`:60`, `:69`) | accept `0` / fractional / a string `"false"` |
| 7 | `a_malformed_completion_batch_key_does_not_fail_the_config_parse` (`SubagentExtensionConfig` from JSON with `"completionBatch":{"debounceMs":-5}`) | typed `u64` field instead of `Value` |
| 8 | `grouped_notice_matches_upstream_layout` (`notify.test.ts:457`, `:602`: header `Background tasks completed (3): **alpha**, **beta**, **gamma**`, `1. alpha\nalpha done`) | render as `"\n\n"`-joined singles |
| 9 | `a_group_of_plain_successes_is_hidden_and_triggers_a_turn` | `display` AND instead of OR / `trigger_turn` AND |
| 10 | `a_scheduled_run_keeps_its_attribution_in_a_group` (`notify.test.ts:970`) | drop the `— scheduled run from` suffix |
| 11 | `five_completions_within_the_window_inject_exactly_one_message` (the row's Verify, at the shell) | batcher bypassed in the host arm |
| 12 | `a_failure_flushes_held_successes_first_and_is_not_delayed` (`notify.test.ts:439`: 2 injects before any time advance, success first) | failure pushed into the group / bypass enqueued before flush |
| 13 | `a_loss_notice_bypasses_the_group` (`undeliverable` / `missing`) | route on outcome only, ignoring `suppressible` |
| 14 | `members_get_their_own_receipts_only_after_the_group_ack` (ack held → no member resolves; `Accepted` → each `Delivered(own run_id)`; `SessionUnavailable` → all `Deferred`) | mint at push or at enqueue; one receipt reused for all |
| 15 | `tearing_down_the_watcher_mid_window_injects_nothing_and_keeps_every_payload` (install-level, real results dir: publish 2, drop handle, advance past `max_wait`) | flusher holds a strong `Arc` / closed members not skipped |
| 16 | `a_session_switch_inside_the_window_defers_the_held_items` (`notify.test.ts:222`) | no ownership re-check at flush |
| 17 | `an_inline_answered_run_never_enters_a_group_and_does_not_hold_siblings` (wired `InlineAnswered(Batching(rec))`; claim A, deliver A and B; B injects at debounce; A suppressed on answer) | wiring order swapped (batcher outside the decorator) |
| 18 | `observation_is_not_delayed_by_the_batch` (the `CompletionBus` publish is seen before the debounce elapses) | batching moved ahead of Phase 1 |
| 19 | `session-svc: merge_injection_batch_merges_same_type_in_order` (pure, no fixture) | hash-ordered grouping / no merge |
| 20 | `session-svc: injections_queued_while_the_pump_waits_for_idle_ride_one_turn` | remove the post-`wait_for_idle` re-drain (§4d) |

---

## 7. Honest size

**M overall**, as the row says, but it splits:

- **§4d session-svc re-drain plus pump tests: S.** About 5 lines plus 2 tests. It is independent,
  takes no config, helps every producer, and should land first.
- **The batcher: M.**
  - About 150 LOC of pure core, config resolution and grouped format.
  - About 180 LOC of shell: the lock, the flusher, ack fan-out, closed-member skip, ownership
    re-check.
  - About 40 LOC across `CompletionMessage`, config and wiring.
  - Tests 1–18.

**Hardest part:** the shell's four guarantees at once, (1) enqueue order across group and bypass,
(2) per-member receipts only after the one group ack, (3) no stale flush after teardown or session
switch, and (4) never holding the lock across the ack await, plus the §5 #5 default decision.
Everything else is transcription.

## 8. Blockers

**None real.**
- `CompletionSink`'s single-`run_id` signature is not a blocker, because the batcher implements it
  per member and talks to `HostServices` directly.
- Receipt minting is public (`receipt.rs:70`).
- `tokio` `test-util` is present.
- The config struct has no `deny_unknown_fields` to fight.

---

## 9. Corrected row text (replacement for `09-cyrup-ext-subagents.md:616`)

> | SUBA-017 | low | not-ported | M | Completion batching unported (**in-baseline**, not drift) —
> **RE-READ 2026-09-22 at `521beaa` / upstream v0.68.0: open, PREMISE CORRECTED.** cyrup does NOT
> deliver N completions as N turns: since `8de7460` the session pump coalesces every queued
> injection into one turn (`cyrup-session-svc/src/session/mod.rs:643,656`, `merge_injection_batch`
> `session/inject.rs:289`, same-`custom_type` bodies joined, `display`/`trigger_turn` OR'd). Prior
> re-reads missed it by grepping only this crate. What is missing is upstream's LEAD-IN debounce
> (`completion-batcher.ts`, 168 L, byte-identical v0.43.0→v0.68.0; defaults 150/1000,
> straggler 75/400/2000), so for an idle parent the first completion of a burst takes a turn alone
> (2 turns, not 1). The pump also does not re-drain after `wait_for_idle` (`mod.rs:655`), so a busy
> parent can see 3. The grouped `formatGroupedCompletion` text (`notify.ts:472-489`) and the
> `completionBatch` key (`shared/types.ts:2663`) are absent. `SubagentExtensionConfig`
> (`registration/mod.rs:81`, 35 fields) silently ignores the key. The sink hand-off is now
> `watch/install.rs:416-418` (spawned per run), not `:304`. **Seam:** below `InlineAnsweredSink`,
> at the injector (`HostServicesCompletionSink`, `watch/sink.rs:88-118`), because only a
> synchronous `inject_message_ack` can order a failure behind a flushed group. Never ahead of Phase-1
> observation, which cyrup runs before delivery (`install.rs:373-386`), so `bg_wait` and
> wait-subscription wakes are not delayed. Upstream's are (`result-watcher.ts:553`→`:589`).
> `merge_injection_batch` has zero tests. Plan: `.flux/todo/COMPLETION_BATCHING.md`. |

The body's **Impact** line should read: *"A burst of N background successes costs an idle parent 2
turns (the first result alone, then the rest), and a busy parent up to 3, where upstream spends 1.
The model reacts to 1/N of the fleet first. The notice is N singular blocks rather than
upstream's numbered group."* The body's **Fix** line should point at §4 of this file and drop the
obsolete "same seam as SUBA-034 and SUBA-056".
