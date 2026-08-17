---
stage: qa
status: completed
updated: 2026-08-17 13:34
---

# TUI-092-F4 — `context_usage`: reverse branch scan (rework, augmented)

## Aug pass 2026-08-17 03:40 — R1–R4 are LANDED; every DoD item verified in-tree

This pass found the remaining work **already implemented**. Nothing below is outstanding; the file is
ready for `/flux/qa`. Evidence, re-derived from the current tree rather than from the prior pass:

| DoD item | verified | evidence |
|---|---|---|
| `state_view` delegates | ✅ | `session.rs:4190` is `let context_usage = self.context_usage().await;`; no `last`/`window` locals, no `from_last_assistant` in the body |
| `from_last_assistant` call sites in crate | ✅ = **1** | `grep -c` → `1` (`session.rs:4172`, inside `context_usage`). Only other tree-wide hit is `cyrup-tui/src/tests/footer_chrome_fidelity.rs:240`, a test constructing the over-limit case — out of scope per *Do not touch* |
| R2 verifier 1 (`messages()\|build_context` in accessor) | ✅ = **0** | comment-stripped form, as specified |
| R2 verifier 2 (`entries()` in either scan) | ✅ = **0** | comment-stripped form |
| single producer by construction | ✅ | all four consumers delegate: `command.rs:190` (`GetContextUsage`), `session.rs:4045` (`stats_context_usage`), `session.rs:4190` (`state_view`), `session.rs:4001` (`session_stats` ← via `stats_context_usage`). `host_services.rs:1908` is the unrelated receiver |
| R3 header cites `:3181-3193` | ✅ | `session.rs:4075` now reads `agent-session.ts:3181-3193`, agreeing with the inline comment at `:4086` |
| R4 docstring | ✅ | `session.rs:4174-4178` cites `rpc-types.ts:95-108` / `rpc-mode.ts:446-461` and `agent-session.ts:863-865`; the bogus `:753` is gone |

**Upstream citations independently re-confirmed against `../pi` @ v0.84.1** (not carried over from the
prior pass): `agent-session.ts:3170` is `contextUsage: this.getContextUsage(),` inside the
`getSessionStats` return literal; `:3174` is `getContextUsage(): ContextUsage | undefined {`;
`rpc-mode.ts:446` is `case "get_state": {` with the twelve-scalar literal closing at `:460` and
`return success(...)` at `:461`; `rpc-types.ts:95-108` is `interface RpcSessionState`, and it declares
neither `contextUsage` nor `stats`. The delegation claim and the cyrup-original claim both hold.

**Gates run this pass** (disk checked first: `/` 40%, `/tmp` 10%):

* `cargo check -p cyrup-session-svc` → clean.
* `cargo clippy -p cyrup-session-svc --all-targets` → **exit=0**; **zero diagnostics on `session.rs`**.
  The 4 warnings emitted are pre-existing and elsewhere: `bash.rs:138` (`too_many_arguments`),
  `bash.rs:219` (`drop_non_drop`), plus two lib-test dupes/`type_complexity`. Out of scope.
* `cargo test -p cyrup-session-svc integration --no-fail-fast` → **11 passed / 0 failed** (294 filtered).

One note for `qa`: the workspace is **not** at CLAUDE.md's documented zero-warning clippy steady state —
`cyrup-provider` (5), `cyrup-config` (3) and `cyrup-agent` (2) also warn. All are outside this task's
file and predate it; flagging so qa does not attribute them here.

---

> **Part of** [`TUI-092-progressive-lockup.md`](TUI-092-progressive-lockup.md).
> **Kind** port bug · **Severity** medium · **Effort** XS — **one function, ~8 lines, one file**:
> [`crates/cyrup-session-svc/src/session.rs`](../../../crates/cyrup-session-svc/src/session.rs).
>
> The F4 accessor rewrite is **done and accepted**. What remains is the duplicated copy of the old
> algorithm that the rewrite left behind in `state_view`, plus two citation defects in the same
> neighbourhood. Research this pass upgraded the remaining work from "consistency nit" to **port bug**:
> upstream has exactly one occupancy producer and every consumer delegates to it.

## Accepted — already production quality, do not redo

* **`context_usage`** (`session.rs:4123-4172`) — `messages()`/`build_context()`/`build_context_messages()`
  gone; one `branch_path(None)` reverse reference walk; zero message clones; `StopReason::Deferred`
  filtered via `filter_map(..).find(..)`. Signature unchanged.
* **`has_post_compaction_usage`** (`session.rs:4085-4106`) — repointed from the flat store to
  `branch_path(None)`, matching Pi's `getBranch()`; lock, direction, `Aborted`/`Error` exclusion and
  four-field sum all byte-identical.
* Reading `window` before acquiring the manager guard (matches Pi's statement order, removes lock nesting).
* `cargo check -p cyrup-session-svc` clean; zero clippy diagnostics on `session.rs`; 1675 tests green
  across `cyrup-session-svc` + `cyrup-tui` + `cyrup-session`.

---

## Research — upstream has ONE occupancy producer, and every consumer delegates

Read this pass from the local mirror [`../pi`](../../../../pi) (`v0.84.1-40-g936aff009`). Frozen excerpts:
[`tmp/pi-reference/agent-session-v0.83.0-getContextUsage.excerpt.ts`](../../../tmp/pi-reference/agent-session-v0.83.0-getContextUsage.excerpt.ts),
[`…-v0.84.1-…`](../../../tmp/pi-reference/agent-session-v0.84.1-getContextUsage.excerpt.ts),
[`session-manager-v0.84.1-getBranch.excerpt.ts`](../../../tmp/pi-reference/session-manager-v0.84.1-getBranch.excerpt.ts).

**Pi's `getSessionStats()` does not recompute occupancy — it delegates** (`agent-session.ts:3170`
@v0.84.1, inside the `getSessionStats` return literal at `:3154-3171`):

```ts
getSessionStats(): SessionStats {
    …
    return {
        …
        cost: usageTotals.cost,
        contextUsage: this.getContextUsage(),   // :3170 ← delegation, not a second scan
    };
}
```

`getContextUsage()` (`:3174`) is the **only** place upstream computes occupancy. Its consumers are the
footer (`footer.ts:108`) and that one delegation. There is no second implementation anywhere.

**Pi's `get_state` carries no occupancy at all.** The RPC handler builds `RpcSessionState` from twelve
scalar fields (`modes/rpc/rpc-mode.ts:446-461`) — `model`, `thinkingLevel`, `isStreaming`, `isCompacting`,
`steeringMode`, `followUpMode`, `sessionFile`, `sessionId`, `sessionName`, `autoCompactionEnabled`,
`messageCount`, `pendingMessageCount` — and the interface (`modes/rpc/rpc-types.ts:95-108`) declares no
`contextUsage` and no `stats`. Pi's `state` getter is a one-liner returning agent state
(`agent-session.ts:863-865`, `return this.agent.state`).

**Conclusion.** cyrup's `SessionStateView.context_usage` is a cyrup-original field, and populating it by
re-deriving the last assistant inline is unported behaviour: at every analogous upstream site the answer
comes from the single accessor. That is what R1 fixes.

---

## R1 — `state_view` still runs a private copy of the pre-F4 algorithm (must fix)

[`state_view`](../../../crates/cyrup-session-svc/src/session.rs#L4177) computes occupancy itself at
`session.rs:4180-4185`, using the algorithm F4 just replaced:

```rust
let messages = self.messages().await;
let last = messages.iter().rev().find_map(|m| match m {   // ← old: scans the compaction-WINDOWED
    Message::Assistant(a) => Some(a),                     //   build, no Deferred filter
    _ => None,
});
let window = { Self::lock(&self.compaction_model).as_ref().map_or(0, |m| m.context_window) };
let context_usage = crate::state::ContextUsage::from_last_assistant(last, window);
```

Before F4 this agreed with `context_usage()` because it *was* the same code. It no longer does, and both
values ship through one seam — `C::GetState` ([`command.rs:188`](../../../crates/cyrup-session-svc/src/command.rs#L188))
and `C::GetContextUsage` ([`command.rs:190`](../../../crates/cyrup-session-svc/src/command.rs#L190)) — so
one client reads two different numbers for one session state.

**Divergent case, verified reachable.** A compaction is on the branch, its kept window holds no assistant,
and an assistant exists earlier on the branch → `context_usage()` returns that assistant's usage,
`state_view().context_usage` returns `used_tokens: 0`. Reachability is not hypothetical: when
`first_kept_entry_id` is `None` — the unresolvable v1 `firstKeptEntryIndex` that
[`context.rs:166-172`](../../../crates/cyrup-session/src/context.rs#L166) documents as a live migration
path — `build_context_messages` keeps **nothing** before the compaction, so the window is empty by
construction and every pre-compaction assistant falls outside it.

Note the same `state_view` call already carries the *correct* value one field away:
`self.session_stats().await` (`session.rs:4178`) delegates properly —
`let context_usage = self.stats_context_usage().await;` — so `stats.context_usage` is Pi-faithful and
already benefits from F4, while the sibling `context_usage` field does not. The struct
([`state.rs:334-347`](../../../crates/cyrup-session-svc/src/state.rs#L334)) carries occupancy twice, and
after F4 the two copies disagree.

### Required fix — delegate (the single implementation path)

Replace `session.rs:4180-4185` with one call, and correct the docstring above it (see R4). Everything else
in the function, including the `messages()` call, is unchanged:

```rust
    /// A serializable snapshot of the session for RPC `get_state`.
    ///
    /// cyrup-original in shape: Pi's `RpcSessionState` (`modes/rpc/rpc-types.ts:95-108`, built at
    /// `modes/rpc/rpc-mode.ts:446-461`) carries twelve scalars and NO occupancy or stats, and Pi's
    /// `state` getter is `return this.agent.state` (agent-session.ts:863-865). The extra `stats` /
    /// `context_usage` fields are cyrup's.
    pub async fn state_view(&self) -> crate::state::SessionStateView {
        let stats = self.session_stats().await;
        let messages = self.messages().await;
        // ONE producer for occupancy, as upstream has: Pi's `getSessionStats` does not re-derive it
        // either, it returns `contextUsage: this.getContextUsage()` (agent-session.ts:3170).
        // Deriving it inline here duplicated the pre-F4 windowed-build scan, so it disagreed with
        // `GetContextUsage` whenever a compaction's kept window held no assistant while an earlier
        // pre-compaction assistant existed — including every unresolvable-v1 `first_kept_entry_id`
        // session, whose kept window is empty by construction (`cyrup-session/src/context.rs:166-172`).
        let context_usage = self.context_usage().await;
        let model = Self::lock(&self.model).clone();
        // … struct literal unchanged; `message_count: messages.len()` still needs `messages`
```

**Why delegation and not the alternatives** — both were researched and are wrong:

* **Reusing `stats.context_usage` is impossible, not merely inelegant.** It is a *different type*:
  [`StatsContextUsage`](../../../crates/cyrup-session-svc/src/state.rs#L78) is
  `{tokens: Option<u64>, context_window: u64, percent: Option<f64>}` (Pi's spelling), whereas the field
  needs [`ContextUsage`](../../../crates/cyrup-session-svc/src/state.rs#L266) =
  `{used_tokens: u64, context_window: u64, fraction: f64}`. In the guard's `tokens: None` case there is no
  `used_tokens` or `fraction` to recover, so any conversion fabricates numbers.
* **Deleting the field is out of scope and contradicted by its own contract.**
  [`state.rs:69-75`](../../../crates/cyrup-session-svc/src/state.rs#L69) states the split is deliberate —
  `ContextUsage` "is what `get_state`, the TUI footer and the guest `ctx.getContextUsage()` capability
  read", and converging the two spellings "is a separate divergence". The field's *existence* is a known,
  documented parity question; its *value* being computed by a stale duplicate is this task's bug. Fix the
  value; leave the shape decision to the spelling-convergence task.
* **Borrow/lock safety.** `messages()` releases the manager guard before returning and
  `context_usage()` takes its own, so no guard is held across the new `await`. Keep the delegation on its
  own statement.
* **Cost.** Two O(branch-depth) reference walks per `state_view` (one inside `session_stats`, one here)
  against a function that already awaits `session_stats`, `messages` (a full context build),
  `session_name` and `is_streaming`. `state_view` is an on-demand RPC payload, never a per-event path.

`SessionStateView.context_usage` has **no in-tree reader** (`grep -rn '\.context_usage' crates/` finds only
`GetContextUsage` plus the unrelated `HostServices`/guest receiver), which is exactly why nothing caught
the divergence and why code reading is the verification method here.

## R2 — the shipped DoD verifiers cannot pass (must fix)

The one-liners this file previously carried match doc-comment **prose** as well as code, so they report `1`
on a correct implementation and can never reach the asserted `0`. Confirmed both this pass and last:
`sed -n '/pub async fn context_usage/,/^    }/p' … | grep -c 'messages()\|build_context'` → `1`, whose sole
hit is the explanatory comment `// messages().await.iter().rev().find_map(..) gave …`. The `entries()`
verifier fails identically against `// not getEntries(). entries() is the flat append-only store`.

Strip comments so the gate measures code:

```bash
# want 0 — no rebuild in the accessor
sed -n '/pub async fn context_usage/,/^    }/p' crates/cyrup-session-svc/src/session.rs \
  | grep -v '^\s*//' | grep -c 'messages()\|build_context'
# want 0 — neither scan reads the flat store
sed -n '/async fn has_post_compaction_usage/,/^    \/\/\/ A serializable snapshot/p' \
  crates/cyrup-session-svc/src/session.rs | grep -v '^\s*//' | grep -c 'entries()'
```

The same trap applies to the `from_last_assistant` count: the bare name appears in comments at
`session.rs:4114` and `:4143`, so count **call sites** —
`grep -c 'ContextUsage::from_last_assistant(' …` — which is `2` today and must be `1` after R1.

## R3 — citation ranges disagree inside `has_post_compaction_usage` (trivial)

Its header (`session.rs:4075`) cites `agent-session.ts:3178-3195`; the inline comment added by F4
(`session.rs:4086`) cites `:3174` and `:3181-3193`. Verified v0.83.0 numbering, which is the convention
this file already uses:

| upstream element | v0.83.0 line |
|---|---|
| `const branchEntries = this.sessionManager.getBranch()` | `:3174` |
| `getLatestCompactionEntry(branchEntries)` | `:3175` |
| `if (latestCompaction) { … }` whole guard | `:3177-3198` |
| the backward `for` scan itself | `:3181-3193` |
| `return { tokens: null, contextWindow, percent: null }` | `:3195-3197` |

The header describes the *scan*, so change `:3178-3195` → `:3181-3193`. Leave the inline comment as is —
it is already correct.

## R4 — `state_view`'s docstring cites the wrong upstream symbol (found this pass)

`session.rs:4175-4176` reads *"Pi `state` getter, agent-session.ts:753"*. Both halves are wrong:

* `:753` is inside `_handleAgentEvent`'s `message_start` arm (`message: event.message` forwarded to the
  extension runner) — unrelated to session state.
* Pi's `state` getter is `:863-865` and returns `AgentState` via `return this.agent.state` — agent state,
  not a session snapshot, so it is not this method's analog either.
* The real analog is the RPC `get_state` handler, `modes/rpc/rpc-mode.ts:446-461`, typed
  `RpcSessionState` (`modes/rpc/rpc-types.ts:95-108`) — which notably has neither `stats` nor
  `contextUsage`.

Fix it in the same edit as R1, using the replacement docstring shown there. This matters beyond tidiness:
the citation currently points a reader at a contract that does not contain the field they are reasoning
about.

---

## Definition of done

* `state_view` obtains occupancy solely by `self.context_usage().await`; the inline `last` / `window`
  locals and the `from_last_assistant` call are gone, and `messages()` remains only for
  `message_count: messages.len()`.
* `grep -c 'ContextUsage::from_last_assistant(' crates/cyrup-session-svc/src/session.rs` → **1**
  (only `context_usage`'s own, currently `session.rs:4172`).
* `GetState.context_usage` and `GetContextUsage` are equal for every session state **by construction** —
  one producer, no second scan in the crate.
* Both comment-stripped verifiers in R2 print `0`.
* `has_post_compaction_usage`'s header cites `:3181-3193`, agreeing with its inline comment.
* `state_view`'s docstring cites `modes/rpc/rpc-mode.ts:446-461` / `rpc-types.ts:95-108` and no longer
  claims `agent-session.ts:753`.
* Gates (light — the change is a delegation, not new logic):

  ```bash
  cargo check -p cyrup-session-svc
  cargo clippy -p cyrup-session-svc --all-targets; echo "exit=$?"   # no NEW diagnostic on session.rs
  cargo test -p cyrup-session-svc integration --no-fail-fast        # integration.rs:566 drives state_view
  ```

  Pre-existing and out of scope: `cyrup-provider --lib` flakes, `cyrup-ext --doc` (2), and the
  `cyrup-ext-subagents` fixture-bin `indexing_slicing` violation that makes a `--features test-fixtures`
  workspace clippy exit 101.

## Do not touch

* `messages()` / `raw_context_messages()` / `build_context` / `build_context_messages` — `message_count`,
  replay and the `/resume`|`/fork`|`/import` seed path need the full list.
* `context_usage`'s body and `has_post_compaction_usage`'s lock, direction, `Aborted`/`Error` exclusion and
  four-field sum — all accepted; R3 is comment-only.
* [`session_stats`](../../../crates/cyrup-session-svc/src/session.rs#L4000) /
  [`stats_context_usage`](../../../crates/cyrup-session-svc/src/session.rs#L4044) — already delegate
  correctly and are already Pi-faithful.
* The **existence** of `SessionStateView.context_usage` and the `ContextUsage` vs `StatsContextUsage`
  spelling split — documented as a separate divergence at
  [`state.rs:69-75`](../../../crates/cyrup-session-svc/src/state.rs#L69).
* The billed-usage semantic of
  [`from_last_assistant`](../../../crates/cyrup-session-svc/src/state.rs#L285); Pi's
  `estimateContextTokens` was considered and rejected.
* [`last_assistant_message`](../../../crates/cyrup-session-svc/src/session.rs#L1292) (`session.rs:1293`)
  and [`last_assistant_text`](../../../crates/cyrup-session-svc/src/session.rs#L3975) (`session.rs:3976`)
  — the same `messages()` rebuild-and-clone shape, and `last_assistant_message` sits on a per-**turn**
  path (the pre-send compaction check, `session.rs:1243-1247`). Real follow-up work; neither is on the
  per-**event** footer path F4 scopes.
* [`HostServices::context_usage`](../../../crates/cyrup-session-svc/src/host_services.rs#L1256) — same
  name, different receiver.
* Anything under `crates/cyrup-tui/`.
