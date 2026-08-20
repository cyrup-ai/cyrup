# 02 — cyrup-agent (the turn loop)

> **Re-audited 2026-08-12, cyrup HEAD `a9000b1` (docs-only; last code commit `04c1ba2`), against pi
> `v0.84.1`.** Five commits past the `1806375` / `9219dcd` baselines the two tables below were last
> measured at; `crates/cyrup-agent/src/agent.rs` has grown from ~1700 to 2011 lines, so **every line
> reference in the previous revision of this file was stale by 50–350 lines** — all citations below
> were re-read at this HEAD on both sides.
>
> **This pass: 3 closed (AGENT-006, AGENT-007, AGENT-014), 2 moved open → partially-closed
> (AGENT-S01, AGENT-S04), 14 newly filed (AGENT-020 … AGENT-033), 0 reopened, 0 refuted.**
> Four items were **severity-corrected downward** by the refuter and are written at the corrected
> rating with the caveat stated inline: AGENT-012 and AGENT-015 medium → low, AGENT-023 and
> AGENT-024 medium → low.
>
> **REPAIR PASS 2026-08-12 (completeness critique, findings 3 / 9 / 14).** Three changes, no item
> renumbered, merged or deleted. (1) **AGENT-020 raised high → critical.** Its own text describes a
> silently destroyed user-typed steering message; `README.md:106-107` defines `critical` as data
> loss, silent wrong output, a permission bypass, or a crash on a normal path, and typing while a
> turn is in flight is the normal path. The definition was applied, not amended. (2) **Every
> upstream line citation in this file was re-resolved at v0.83.0** — the tag that governs
> classification — after AGENT-020 was found citing `agent.ts:361-388` / `:362-364`, which are the
> **v0.84.1** offsets. `packages/agent/src/agent.ts` shifted +7 to +15 lines and
> `agent-loop.ts` +4 lines after `:641` between the two tags; twelve items carried the later
> offsets. All are corrected below, with the v0.84.1 offset noted alongside. The **bytes** were
> identical in every case — no finding changed. (3) **AGENT-028 reclassified as a `tracker`**: its
> Fix is "do not port speculatively, first decide whether cyrup models pi's harness at all", i.e. it
> proposes a decision rather than work, so it keeps its ID and body but leaves the severity tally.
>
> Open set is now **26 items — 1 critical, 1 high, 6 medium, 18 low** — plus one `tracker`
> (AGENT-028) and two `partially-closed` entries whose residuals are filed as their own items, none
> of the three counted. The version-lag items in this area were
> measured against **pi v0.83.0 → v0.84.1** (the ported baseline vs latest). The whole
> `packages/agent` **source** diff outside `src/harness/**` is 29 lines across four files and all of
> it is filed (AGENT-022, AGENT-023, AGENT-024, AGENT-026); `packages/agent/src/harness/**` — ~11.4k
> insertions / ~10.9k deletions including a new session subtree and a new telemetry layer — was
> **not audited** and is owned by no area file today (see AGENT-028 and `## Coverage`).

This area covers `cyrup/crates/cyrup-agent` — the run loop, tool-call preparation / execution /
finalization, the state reducer, hooks and the proxy — measured against
`pi/packages/agent/src/{agent-loop,agent,proxy,types}.ts`, with spill into `cyrup-core`'s
message/tool model, `cyrup-session-svc`'s per-turn refresh hook and submission gate, and
`crates/cyrup/src/timings.rs`, where the loop-side half is the thing under test.

Headline for this pass: the three highest-value fixes since the last baseline all landed and all
survive an adversarial re-read — hook errors now carry their real text (AGENT-006), post-turn hook
failures now emit pi's full four-event closing quartet (AGENT-007), and `StopReason::Pending` exists
end to end (AGENT-014). What replaced them at the top is **AGENT-020**, a silent destruction of a
user-typed steering message caused by `continue_run` draining its queues *before* claiming the run
latch, and **AGENT-030**, the session-level submission gate that reads a per-run flag where pi reads
a whole-driver-loop flag. Both live on the same seam and must be reasoned about together. The
second theme is that **two fixes left residuals on their sibling paths**: AGENT-007's fix corrected
one of two error-assistant emitters (AGENT-025 is the other), and AGENT-S01's fix moved the header
read onto live state without seeding it on the low-level API (AGENT-021) or recomputing it on a
per-turn model override (AGENT-029). AGENT-029 and AGENT-017 are coupled: shipping AGENT-017 alone
turns AGENT-029 from latent into live.

> ### Reconciliation 2026-08-14 — sweeps 1 and 2 applied, counts re-derived
>
> **cyrup HEAD `380c713`** (this file was written against `04c1ba2`), tree clean. Two whole-backlog
> parity sweeps have landed since this file was last edited: **sweep 1 — 232 items across 11 crates**,
> and **sweep 2**, run under the same rules. Area agents were forbidden from editing documentation so
> that a single writer could reconcile all sixteen files in one pass; this block, and the dispositions
> written into the `## Open items` rows below, are that reconciliation. **Every status in this file
> that predates this block is stale — including the header notes above it and the
> `## Status of every item…` table.**
>
> **No ID was renumbered, merged or deleted.** A refuted item keeps its ID with the refutation
> recorded in its row, so nobody re-derives it. Refutations are corrections to *this analysis*, not
> failures of the sweep — see `00-residual-ledger.md`, which now publishes the measured error rate.
>
> **The test architecture changed underneath every path citation in this file.** The integration
> tests were relocated into their crates as unit tests (`63d729a` / `c3982b5` / `d973906`), taking the
> suite from **310 integration binaries to 6 + 8 gated** behind a new **`cyrup-it`** harness crate.
> The gate is now **6440 tests / 6440 passed / 8 skipped in 16.4 s**. Any citation of the form
> `crates/<crate>/tests/<x>.rs` in this file is stale unless it names `cyrup-it`, and note that
> `cyrup-it` is `required-features = ["it"]`, so **the gate does not build or run it**.
>
> **Still a static analysis.** Neither sweep executed the suite: area agents were restricted to
> `cargo check -p <crate> [--all-targets]` and the orchestrator ran the gate once over the combined
> work. Every red-before/green-after claim below is a reasoned argument plus a type-check, and every
> `Verify` line in this file remains a design, not an observation.
>
> **Area 02 — recount: 29 rows → 2 open (0 critical · 0 high · 0 medium · 2 low) + 1 tracker
> (`AGENT-028`), and two new items filed and closed on arrival (`AGENT-034`, `AGENT-035`).** The
> area's only high (`AGENT-030`) and 24 of its 26 counted items are closed. `AGENT-S01` promotes from
> partially-closed to closed; `AGENT-S04` owes nothing here — its residual is area 01's.
>
> **The two remaining items are both blocked outside this crate**, and neither was declined on effort:
> `AGENT-026` waits on `cyrup_provider::StreamOptions.sampling_params` (area 01, re-verified absent at
> HEAD), and every fix site for `AGENT-027` is `crates/cyrup` or `crates/cyrup-ext` — two consecutive
> sweeps reported it not-reached for that reason, and area 08 has now landed most of it.
>
> **The highest-yield finding here is a method, not a defect.** Sweep 2 exhausted the assigned backlog
> before writing a line and then ran the verification this file's own Coverage section names as blind
> spot 3 — reading pi's `packages/agent/test/agent-loop.test.ts` and `agent.test.ts`, which **no pass
> had ever opened**. It produced `AGENT-034` immediately, because pi's suite asserts the exact throw
> strings cyrup had rewritten. A second sweep comparing every literal error string in
> `packages/agent/src/{agent,agent-loop,proxy}.ts` against the workspace produced `AGENT-035`.
> **Blind spot 3 should be promoted: pi's own test suite is the highest-yield unexploited oracle in
> this area.**
>
> **STRUCTURAL:** every path of the form `crates/cyrup-agent/tests/<x>.rs` in this file is stale —
> there is no `crates/cyrup-agent/tests/` directory at all. Affects the citations in `AGENT-006`,
> `-009`, `-016`, `-018`, `-019`, `-020`, `-021`, `-023`, `-024`, `-025`, `-029`, `DRIFT-039`'s body
> and the Coverage "read first-hand" list. Add `crates/cyrup-agent/src/tests/area02_backlog.rs` to the
> Coverage test list — it is where both sweeps' regressions live.
>
> **CROSS-CRATE, for whoever hits it:** `AgentError`'s `RunActive` and `NoMessages` variants now carry
> payloads. The only consumer outside the crate is `SessionServiceError::Agent(#[from] …)`, which is
> unaffected — but any `matches!(.., AgentError::RunActive)` written after 2026-08-14 needs
> `RunActive(_)`.


## Status since the `1806375` / `9219dcd` baselines

| ID | Status | Note |
|---|---|---|
| AGENT-001 | **closed** | 4091c86. Survives a third adversarial re-read at HEAD. `agent.rs:525-526` branches on `StopReason::Length` into `fail_truncated_tool_calls` (`agent.rs:906-938`), emitting pi's exact four-event order per call and returning `Batch{terminate:false}` at `:937`; the error string at `:917-922` is byte-identical to `agent-loop.ts:396`. Upstream `failToolCallsFromTruncatedMessage` (`agent-loop.ts:381-406`) is unchanged at v0.84.1. Residue: the `result` payload still carries AGENT-009's divergence because it routes through `immediate_error`. |
| AGENT-002 | **closed** | 8854601. Still genuinely two-phase at HEAD: the prep loop (`agent.rs:1182-1218`) contains zero `joinset.spawn` — prepared calls go into `deferred: Vec<Deferred>` (`:1207-1213`) and every body spawns afterwards at `:1225-1253`. Matches pi's lazy closure + `Promise.all` (`agent-loop.ts:522-534`, `:540-542`). |
| AGENT-003 | still-open | low. Both channels still `try_send`-and-discard (`agent.rs:1236-1239`, `:1355`). Unchanged upstream at v0.84.1. |
| AGENT-004 | **closed** | f777e44. `added_tool_names` on `cyrup_core::ToolResult` (`crates/cyrup-core/src/tool.rs:33-39`), carried through `finalize` (`agent.rs:1049-1051`, `:1127`) into `result_value_of` (`agent.rs:136-141`, omit-when-empty) and the transcript (`crates/cyrup-agent/src/event.rs:103-107`, `skip_serializing_if = "Vec::is_empty"`). Matches the conditional spread at `agent-loop.ts:787`. |
| AGENT-005 | **closed** | f777e44. `usage: Option<Usage>` on `ToolResult` (`tool.rs:28-32`), read by the after-hook (`crates/cyrup-agent/src/hooks.rs:64-67`), replace-not-merge patchable via `AfterOverride.usage` (`hooks.rs:89-92`) applied at `agent.rs:1095-1097`. Matches `agent-loop.ts:742`. |
| AGENT-006 | **closed** *(this pass)* | All four sites now bind and interpolate the error: `agent.rs:992` (before-hook), `agent.rs:1106-1118` (after-hook), `agent.rs:691` and `:696` (transform_context / convert_to_llm). Pinned by `crates/cyrup-agent/src/tests/hook_failure_text.rs`. A workspace grep for the four hardcoded placeholders returns exactly one hit — a comment at `hook_failure_text.rs:19` describing the old bug. |
| AGENT-007 | **closed** *(this pass)* | `RunCtx::emit_run_failure` (`agent.rs:415-435`) emits MessageStart/MessageEnd/TurnEnd/AgentEnd over an `errored_assistant` whose stop reason is `Aborted` iff cancelled (`:420-421`) and whose `new_messages` is replaced with the single failure (`:434`); called from both post-turn error arms (`agent.rs:616-619`, `:649-652`). Matches `handleRunFailure` (`agent.ts:496-512` @v0.83.0, `:511-527` @v0.84.1). The bare-`agent_end` arms are gone. The *other* caller of the old shape — the `transform_context`/`convert_to_llm` path — is a different call site this item never named and is filed as **AGENT-025**. |
| AGENT-008 | **closed** | 6d29542. `ThinkingLevel::Max` / `ModelThinkingLevel::Max` (`crates/cyrup-core/src/message.rs:38`, `:55`), mapped at `:68`/`:86`, serialized `"max"`; ladder `EXTENDED_THINKING_LEVELS: [ModelThinkingLevel; 7]` (`crates/cyrup-provider/src/collection.rs:410-418`). Matches pi's seven-value `ThinkingLevel`. |
| AGENT-009 | still-open | medium. Widened this pass: **four** producers pass `details: None`, not two, and the JSONL transcript *omits* `details` where pi writes `"details":{}`. |
| AGENT-010 | still-open | low. Both strings still differ (`agent.rs:952`, `:995`); the abort string does match. |
| AGENT-011 | still-open | low. `state.rs:160-166` still stop-reason-gated with the synthetic fallback. Unchanged upstream. |
| AGENT-012 | still-open | **severity corrected medium → low.** Both divergences confirmed at HEAD; the whole consequence is hook-invocation count and error-path text, the same class this file rates low for AGENT-010. |
| AGENT-013 | still-open | low. All five proxy failure paths route through `error_terminal` (`proxy.rs:529-541`) with the raw `ProviderError` Display. The entire v0.83.0→v0.84.1 `proxy.ts` diff is two `samplingParams` lines, so upstream is unchanged. |
| AGENT-014 | **closed** *(this pass)* | `StopReason::Pending` (`crates/cyrup-core/src/message.rs:163-166`, `is_settled()` at `:191-197`) seeded by both in-flight constructors (`agent.rs:103`, `proxy.rs:287`), normalized by providers (`crates/cyrup-provider/src/stream.rs:511-521`, rejected as a done-reason at `:404`), treated as non-terminal by both terminal matches (`agent.rs:508`, `:525`), abort stamps `Aborted` at `agent.rs:783`. Matches `proxy.ts:123`. |
| AGENT-015 | still-open | **severity corrected medium → low.** Divergence confirmed at HEAD, but the trigger window is narrow — see the caveat in the item. |
| AGENT-016 | still-open | medium. Confirmed at HEAD; the refuter added a consequence the item had missed (a lost slot means no tool-result message at all, so the next request carries an unpaired `tool_use`). |
| AGENT-017 | still-open | medium. `crates/cyrup-session-svc/src/hooks.rs:170-181` still sets only `update.tools`. Coupled to the new **AGENT-029** — ship together. |
| AGENT-018 | still-open | low. Reducer unchanged at `state.rs:142-170`; upstream unchanged at `agent.ts:546-548`, `:579-581`, `:529-535`. |
| AGENT-019 | still-open | low. `agent_loop.rs:327` still asserts a 115ms wall-clock bound; the collected `spans` are still never read. |
| AGENT-S01 | **partially-closed** *(this pass)* | The session-level model switch is fixed: `StateInner.headers` is live (`state.rs:80-85`), `Agent::set_headers` exists (`agent.rs:1503-1505`), `stream_assistant` reads it per turn (`agent.rs:733`), and both session model-change paths recompute (`crates/cyrup-session-svc/src/session.rs:2792`, `:3896`). Two residual paths remain, filed as **AGENT-021** (low-level API) and **AGENT-029** (per-turn model override). Not counted again in the open-severity tally. |
| AGENT-S02 | still-open | low. `subscribe` still returns `()` (`agent.rs:1485-1487`) and `on_event` takes no token. Rationale corrected: v0.84.1 **deleted** `_reconnectToAgent` and the compact-time disconnect, so the surviving upstream consumers are disposal (`agent-session.ts:395`, `:829-831`) and the rpc-mode backpressure listener (`modes/rpc/rpc-mode.ts:355-361`, `:732-733`). |
| AGENT-S03 | still-open | low. `GenerationConfig` still has no `metadata` field; `StreamOptions` is closed with `..Default::default()` at `agent.rs:744`. |
| AGENT-S04 | **partially-closed** *(this pass)* | Agent-side wiring is complete: `StateInner.transport` (`state.rs:86-95`), `Agent::set_transport` (`agent.rs:1510-1512`), run-start snapshot overlay (`agent.rs:1703`, `:1712`) matching `agent.ts:454`, reaching `StreamOptions.transport` at `agent.rs:734`, TUI row wired at `crates/cyrup-tui/src/app/execute_misc.rs:229-230` → `session.rs:3654-3658`. Still dead downstream — nothing in `crates/cyrup-provider` reads it — which is **an area-01 provider gap, not an agent gap**; handed off. Not counted again in the open-severity tally. |
| AGENT-020 | **new** | **low** *(raised to critical 2026-08-12, then LOWERED critical → low on 2026-08-13 when the raise's justification was refuted by measurement)*. `continue_run` drains the steering/follow-up queue before the run-active check. The code path is real and unchanged at HEAD, but the loss it predicts does not occur on the path the TUI uses: typing during a live stream queued and delivered the message 5/5 times, including four attempts timed at the settle boundary. Latent race, not data loss. |
| AGENT-021 | **new** | medium. `loop_fn::build_run_ctx` hardcodes `headers: None` — a regression introduced by AGENT-S01's own fix. |
| AGENT-022 | **new** | medium. `BeforeToolCallResult.terminate` (v0.84.1 drift) unrepresentable. |
| AGENT-023 | **new** | low *(corrected from medium)*. `Agent::reset()` under a live run; upstream now throws. |
| AGENT-024 | **new** | low *(corrected from medium, and restated)*. Post-turn hooks get no abort signal — at pi's Agent-options layer, not at the loop seam. |
| AGENT-025 | **new** | low. `transform_context` / `convert_to_llm` failure emits the wrong `agent_end.messages` and never reports `aborted`. |
| AGENT-026 | **new** | low. `samplingParams` (v0.84.1 drift) absent from the proxy body and from `StreamOptions`. |
| AGENT-027 | **new** | low. `timings.ts` ported as one namespace with 3 of pi's 12 marks and none of `extensions`. |
| AGENT-028 | **new** | **tracker** *(was low; reclassified in the 2026-08-12 repair pass)*. pi v0.84.x's typed telemetry contract has no counterpart — filed to force a scope decision on `packages/agent/src/harness/**`. Proposes a decision, not work; excluded from the severity tally. |
| AGENT-029 | **new** | medium. Per-turn model override does not recompute the attribution header overlay. Residual of AGENT-S01; coupled to AGENT-017. |
| AGENT-030 | **new** | high. `AgentSession::prompt` gates on the agent's per-run `is_streaming`, not on a session-level run-active flag. |
| AGENT-031 | **new** | low. `websocket_connect_timeout_ms` unreachable from the agent, and the parsed setting has no consumer at all. |
| AGENT-032 | **new** | low. Two JS-falsy `||` fallbacks ported as `Option`-only fallbacks. |
| AGENT-033 | **new** | low. A panicking event subscriber is swallowed where pi routes a throwing listener into `handleRunFailure`. |

## Open items

> **This table is now the COMPLETE open set for this area**, including the `-S` (surface-sweep)
> ids, which previously lived in a second table further down and caused this area to be undercounted
> by 4 — the miss recorded as structural defect A in `00-residual-ledger.md`. The
> `## Surface-sweep findings` section below is retained for provenance only; its items are listed
> here. `AGENT-S01` and `AGENT-S04` are `partially-closed`, their live residuals filed as
> `AGENT-021` / `AGENT-029` (S01) and handed to area 01 (S04); they are listed but not counted.
> `AGENT-028` is a **`tracker`** — it keeps its ID, its row and its full body, but proposes a scope
> decision rather than work, so it is listed and **not counted**. Counted total: **26**.

> **RE-DERIVED 2026-08-14 (sweeps 7-8 reconciliation, third edition) — counted set UNCHANGED at 0 critical, 0 high, 0 medium, 2 low = 2** (31 rows: 27 fully closed, 2 open — both partially — 1 `tracker`, and `AGENT-S04`, which carries no severity and is in neither total). No sweep-7 or sweep-8 agent touched this area and no row moved. Both remaining rows are blocked outside this crate, as recorded.

> **RECOUNTED 2026-08-14 — counted set: 0 critical, 0 high, 0 medium, 2 low = 2** (`AGENT-026`, `AGENT-027`, both blocked outside this crate), plus the `AGENT-028` tracker and the two provenance rows. `AGENT-034` and `AGENT-035` were filed and closed in the same pass. "Counted total: **26**" above is superseded.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| ~~AGENT-020~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `continue_run` drains the steering/follow-up queue before the run-active check — **REFUTED as filed 2026-08-13: the predicted loss does not occur on the normal path (5/5 delivered); critical → low** — **CLOSED 2026-08-14**: sweep 1 — the run-active guard is the first statement of `Agent::continue_run` and both drain sites restore via `PendingQueue::push_front` on `Err(RunActive)`; pinned by `src/tests/agent_loop.rs`. Re-verified by reading the code in sweep 2: the AGENT-020 comment cites agent.ts:351-353 @v0.83.0 and states plainly that it is a fast path only, because pi gets check-then-claim atomicity from single-threaded JS. |
| ~~AGENT-030~~ | ~~high~~ **CLOSED 2026-08-14** | parity-bug | M | `AgentSession::prompt` gates on the agent's per-run flag, so a prompt in the post-run gap starts a second run — **CLOSED 2026-08-14**: sweep 1 — `AgentSession::is_run_active()` exists (`cyrup-session-svc/src/session.rs:638`) as `!self.is_idle()` (driver_tx OR `agent.is_running()`), with an AGENT-030 citation block. |
| ~~AGENT-009~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | M | Error tool results diverge in `details` and in `tool_execution_end.result` shape — **CLOSED 2026-08-14**: sweep 1. |
| ~~AGENT-016~~ | ~~medium~~ **CLOSED 2026-08-14** | cyrup-original | S | Panicking tool in a parallel batch vanishes (unwind builds only) — **CLOSED 2026-08-14**: sweep 1 — both batch modes wrap `tool.execute` in `AssertUnwindSafe(..).catch_unwind()` and convert an unwind into `Err(ToolError)`; pinned by `agent016_panicking_tool_keeps_its_slot_in_a_{parallel,sequential}_batch`. |
| ~~AGENT-017~~ | ~~medium~~ **CLOSED 2026-08-14** | stale-port | S | Per-turn refresh re-pushes only `tools`; mid-run model / thinking-level change never reaches the loop — **CLOSED 2026-08-14**: sweep 1 — `cyrup-session-svc/src/hooks.rs:193-196` stamps `update.model` and `update.thinking_level` after the inner hook. The "Caveat from the refuter" about the hooks.rs:154-156 comment claiming the ordering "matches Pi exactly" was re-checked by area 08 in sweep 2 and is superseded by DRIFT-033's port. |
| ~~AGENT-021~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | `loop_fn::build_run_ctx` hardcodes `headers: None`, orphaning `AgentLoopConfig.gen_config.headers` — **CLOSED 2026-08-14**: sweep 1. |
| ~~AGENT-022~~ | ~~medium~~ **CLOSED 2026-08-14** | upstream-drift | M | `BeforeToolCallResult.terminate` — a blocking hook cannot stop the turn — **CLOSED 2026-08-14**: sweep 1 — the extension half landed concurrently as EXT-049 (`cyrup-agent/src/hooks.rs:53-62`, `BeforeOutcome::Block{reason, terminate}`). Two areas fixed the two ends of one seam in one pass. |
| ~~AGENT-029~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | A per-turn model override does not recompute the attribution header overlay — **CLOSED 2026-08-14**: sweep 1 — `HeaderFn`/`RunCtx::header_fn`/`Agent::set_header_fn` exist and `stream_assistant` resolves headers from the live per-turn model; guard `agent029_header_fn_is_keyed_on_the_dispatched_model`. |
| ~~AGENT-003~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `tool_execution_update` dropped when the bounded channel fills — **CLOSED 2026-08-14**: sweep 1. |
| ~~AGENT-010~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | Loop-generated tool-error strings do not match pi's — **CLOSED 2026-08-14**: sweep 1. |
| ~~AGENT-011~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `state.error_message` gated wrongly and invents a message — **CLOSED 2026-08-14**: sweep 1. |
| ~~AGENT-012~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | Abort checked before `before_tool_call`, and ordered below block — **CLOSED 2026-08-14**: sweep 1. |
| ~~AGENT-013~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | Proxy HTTP failures lack pi's `Proxy error: …` message — **CLOSED 2026-08-14**: sweep 1. |
| ~~AGENT-015~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | Aborted parallel batch: unprepared calls veto termination — **CLOSED 2026-08-14**: sweep 1. |
| ~~AGENT-018~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | Reducer diverges on non-assistant `message_start` and on when `pendingToolCalls` clears — **CLOSED 2026-08-14**: sweep 1. |
| ~~AGENT-019~~ | ~~low~~ **CLOSED 2026-08-14** | test-defect | S | `a_02_2_parallel_completion_vs_source_order` asserts wall-clock latency and a sleep-derived order — **CLOSED 2026-08-14**: sweep 1 — the 115 ms wall-clock bound is gone; the two tools rendezvous on a `tokio::sync::Barrier(2)` and `slow` parks on a oneshot released by a subscriber on `fast`'s `tool_execution_end`, so `ends == ["fast","slow"]` is a fact rather than a race. The only surviving `Duration` is a 10 s `timeout` documented in-source as a hang detector. Closes DRIFT-039 with it — one fix, two IDs. |
| ~~AGENT-023~~ | ~~low~~ **CLOSED 2026-08-14** | upstream-drift | S | `Agent::reset()` still wipes state under a live run; upstream now throws — **CLOSED 2026-08-14**: sweep 1 — reset is refused while a run is in flight; pinned by `agent023_reset_is_refused_while_a_run_is_in_flight`. |
| ~~AGENT-024~~ | ~~low~~ **CLOSED 2026-08-14** | not-ported | S | The post-turn hooks receive no abort signal — **CLOSED 2026-08-14**: sweep 1. |
| ~~AGENT-025~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | A `transform_context` / `convert_to_llm` failure emits the wrong `agent_end.messages` and never reports `aborted` — **CLOSED 2026-08-14**: sweep 1. |
| AGENT-026 | low — **PARTIALLY CLOSED 2026-08-14** | upstream-drift | S | `samplingParams` absent from the proxy request body and from `StreamOptions` — **PARTIALLY CLOSED 2026-08-14**: sweep 1 + 2 — the proxy struct/wire/mapping half is done. **RE-VERIFIED BLOCKED in sweep 2, not deferred by choice**: a workspace-wide grep for `sampling_params\|samplingParams` hits ONLY `crates/cyrup-agent/src/proxy.rs`, so area 01's half is still entirely absent after its own sweep-2 pass. **RESIDUAL is area 01**: `cyrup_provider::StreamOptions.sampling_params` plus the merge over `Model.sampling_params` in the OpenAI-compatible adapters; then the one-line `ProxyStreamFn::options_from` copy (proxy.rs:677-682, currently `None`). Strike this area's "then thread it through `GenerationConfig` … and `AgentBuilder`" — landing that half alone is a field documented as live with no path to the wire, which is AGENT-021 verbatim. |
| AGENT-027 | low — **PARTIALLY CLOSED 2026-08-14** | not-ported | S | `timings.ts` ported as one hardcoded namespace with 3 of 12 marks — **PARTIALLY CLOSED 2026-08-14**: sweep 2 (area 08) — `crates/cyrup/src/timings.rs` is now a namespaced port of `timings.ts` (process-global insertion-ordered table behind a `OnceLock<Mutex<..>>`, closed `TimingLabel { Main, Extensions }`, free `reset_timings`/`time`/`print_timings`, one titled group per namespace) and `main.rs` gained `createSessionManager`, `createRuntime`, `createAgentSessionRuntime`, `createAgentSession`, `resolveModelScope`, `readPipedStdin`, `prepareInitialMessage`. Two findings the item did not have: (a) `print_timings()` sat ABOVE the stdin-read/prompt-assembly block in the interactive and print/json arms, so any mark taken there was recorded and never printed — moved to pi's position (main.ts:899/:902); (b) `initTheme` has no cyrup counterpart at pi's position because cyrup's theme boot lives inside `run_interactive`, downstream of the print. **RESIDUAL: the `extensions` namespace producers in cyrup-ext's loader (`${extensionPath} module import` / `factory`) — area 06.** OWNERSHIP: nothing in crates/cyrup-agent marks or reads a timing; two consecutive sweeps reported it not-reached for that reason. |
| AGENT-028 | *(tracker)* | upstream-drift | L | pi v0.84.x's typed telemetry contract has no cyrup counterpart — scope decision, not counted |
| ~~AGENT-031~~ | ~~low~~ **CLOSED 2026-08-14** | not-ported | S | `websocket_connect_timeout_ms` unreachable from the agent; the parsed setting has no consumer — **CLOSED 2026-08-14**: sweep 1. |
| ~~AGENT-032~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | Two JS-falsy `\|\|` fallbacks ported as `Option`-only fallbacks — **CLOSED 2026-08-14**: sweep 1. |
| ~~AGENT-033~~ | ~~low~~ **CLOSED 2026-08-14** | cyrup-original | S | A panicking event subscriber is swallowed where pi fails the run — **CLOSED 2026-08-14**: sweep 1 — disposition taken: option (a), pi parity. Records the resulting tension with cyrup's own R-02-048 wording ("a subscriber failure MUST NOT halt the loop"), which `spec/` is absent to adjudicate; the no-deadlock half of R-02-048 is preserved by `SettlementGuard` and is still asserted by the rewritten test. |
| ~~AGENT-S02~~ | ~~low~~ **CLOSED 2026-08-14** | not-ported | M | `Agent::subscribe` returns no detach handle and `on_event` receives no abort signal — **CLOSED 2026-08-14**: sweep 1. |
| ~~AGENT-S03~~ | ~~low~~ **CLOSED 2026-08-14** | not-ported | S | `StreamOptions.metadata` is unreachable from the agent loop — **CLOSED 2026-08-14**: sweep 1. |
| ~~AGENT-S01~~ | ~~*(partially-closed)*~~ **CLOSED 2026-08-14** | not-ported | S | Attribution headers — residuals filed as AGENT-021 / AGENT-029 — **CLOSED 2026-08-14**: sweep 1 — both residuals (AGENT-021, AGENT-029) are closed, so this promotes from partially-closed to closed. |
| AGENT-S04 | *(partially-closed)* | not-ported | S | `transport` — agent side done; downstream handed to area 01 — **2026-08-14, still open**: sweep 2 — nothing owed by area 02. The agent-side wiring is present at HEAD (`StateInner.transport`, `Agent::set_transport`, the run-start snapshot overlay, and the read into `StreamOptions.transport` in `stream_assistant`); the residual is that nothing in crates/cyrup-provider consumes it — area 01. |
| AGENT-034 | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | **FILED AND CLOSED IN THE SAME PASS (sweep 2).** Six pi error strings collapsed into three generic Rust ones, and `prompt()` was missing pi's own run-active guard. pi keys FOUR distinct throws off `this.activeRun` (`prompt` agent.ts:341-343, `continue` :352, `reset` :335 @v0.84.1, `runWithLifecycle` :473) and TWO off the two `continue` surfaces' empty-transcript check (`Agent.continue` :357 = "No messages to continue from" vs agent-loop.ts:71/:128 = "Cannot continue: no messages in context"); `ContinueFromAssistant` is one string on all three pi sites and cyrup had a fourth. `AgentError::RunActive` became `RunActive(BusyEntry)` and `NoMessages` became `NoMessages(ContinueSurface)`. **The second half is CONTROL FLOW, not text: `Agent::prompt`/`prompt_with_images` had no guard at all and fell through to the latch, reporting the bare latch message instead of the one string in the family that tells the caller what to do.** User-visible because `SessionServiceError::Agent` re-emits `AgentError`'s Display verbatim (`"agent: {0}"`, cyrup-session-svc/src/error.rs:16-17), and pi's own suite asserts them. **Found by reading pi's `packages/agent/test/{agent,agent-loop}.test.ts` — an oracle NO pass had ever opened; blind spot 3 should be promoted accordingly.** |
| AGENT-035 | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | **FILED AND CLOSED IN THE SAME PASS (sweep 2).** An aborted proxy stream reported `"aborted"` where pi reports `Request aborted by user`, and a post-drain abort emitted NO terminal event at all. pi's `streamProxy` checks the abort signal by hand at two points (proxy.ts:186-190, :208-211) and its outer catch copies that literal into `partial.errorMessage` before pushing the terminal `error` event; cyrup surfaced `ProviderError::Aborted`'s bare Display, and pi's SECOND check — between the drained read loop and `stream.end()` — had no counterpart, so a cancel landing after the frame stream returned `None` closed SILENTLY. Both fixed in `crates/cyrup-agent/src/proxy.rs`. Sibling of AGENT-013 on the same seam and the same class. One `[CYRUP-DELTA]`: pi has a second abort string on this path (undici's AbortError, when the abort interrupts `fetch`/`reader.read()` mid-await) that cyrup cannot reproduce, because `open_sse`'s frame stream is itself cancel-aware (a `biased` select at cyrup-provider/src/stream/sse.rs:406-412), so both of pi's cases collapse onto one value. The string pi's own SOURCE contains is the one ported; the other is a JS-runtime artifact. |

## AGENT-020 — `continue_run` drains the steering/follow-up queue before the run-active check

**Kind** parity-bug · **Severity** **low** *(lowered from critical 2026-08-13 — see below)* · **Effort** S · **Confidence** **code path confirmed at HEAD; the filed Impact REFUTED by measurement** · **observed 2026-08-13** (live-terminal; [`REPRO-LOG.md`](REPRO-LOG.md))

> **REFUTED as stated, 2026-08-13, in a live terminal. Severity critical → low.**
>
> The cyrup-side code citation below is **accurate and unchanged at HEAD**: `continue_run` really
> does `drain()` before `start_run` claims the latch, and the drained `Vec<AgentMessage>` really
> would be dropped on `Err(RunActive)`. What does **not** happen is this item's Impact — the
> "unconditional silent destruction of a user-typed steering message" on "the normal path of typing
> while a turn is in flight", which was the sole justification for the `high → critical` raise.
>
> Measured under tmux against a real streaming Together turn: typing during an active stream and
> pressing Enter clears the editor, echoes the message into the transcript, lets the stream run to
> completion, and **then delivers it** — the model answered the canary (`"We need to obey stop.
> ACK."` / `"ACK"`). **Five for five**, across one deliberate mid-stream submission and four
> submissions timed 3.0 / 4.0 / 4.5 / 5.0 s into a ~10 s turn, deliberately aimed at the settle
> boundary. **No canary was lost.** The steering path the TUI actually uses queues and re-drives;
> `continue_run` is not entered while the latch is held on that path, so the drain-before-latch
> window is never opened by typing.
>
> **Restated Impact.** The drained `Vec<AgentMessage>` is dropped if `start_run` returns
> `Err(RunActive)`. That window is reachable only through the sub-millisecond `is_streaming`-cleared /
> oneshot-unresolved race described in **AGENT-030**, and was not observed. This is a **latent race**,
> not an unconditional loss.
>
> **Keep the Fix** — pushing the drained messages back on the error path is cheap and correct, and it
> is the right shape regardless of reachability. What does not survive is the severity and the claim
> that this is the highest-value item in the area.
>
> **Method note, recorded because it generalises.** The `README.md:106-107` "data loss on a normal
> path" criterion was applied to a *predicted* consequence that the binary does not exhibit. A
> severity raise must cite an observation or say plainly that it does not.

**cyrup** — `crates/cyrup-agent/src/agent.rs:1635-1657`: `continue_run` reads `state.messages` and,
when the last message is an assistant, calls `let steering = lock(&self.steering).drain();` at
`:1646` and otherwise `let follow = lock(&self.follow_up).drain();` at `:1650`. `PendingQueue::drain`
(`crates/cyrup-agent/src/queue.rs:51-56`) **removes** the items. Only then does it call `start_run`
(`:1648` / `:1654`), whose first act is the latch claim `self.running_tx.send_if_modified(…)` at
`agent.rs:1672-1682`, returning `Err(AgentError::RunActive)` at `:1681` when a run is already in
flight. The drained `Vec<AgentMessage>` is dropped on that early return — never pushed back, never
delivered. Reachable in-tree: `AgentSession::drive_run`
(`crates/cyrup-session-svc/src/session.rs:700-726`) calls `self.agent.continue_run()` at `:716` and
breaks on `Err` at `:722`, while the gate on a concurrent user prompt is `AgentSession::prompt` →
`is_streaming()` (`session.rs:627`, reading `agent.snapshot().is_streaming`, `session.rs:3202-3204`),
a flag `SettlementGuard::drop` clears at `agent.rs:1441` before the run handle's oneshot resolves.
See **AGENT-030** for that gate.

**upstream** — `pi/packages/agent/src/agent.ts:349-377` **at v0.83.0**, the ported tag that governs
this classification (**v0.84.1: `:360-388`**):
`async continue(): Promise<void> { if (this.activeRun) { throw new Error("Agent is already
processing. Wait for completion before continuing."); } … }` — the guard at `:351-353` (v0.84.1
`:362-364`) runs **before** `this.steeringQueue.drain()` at `:361` (v0.84.1 `:372`) and
`this.followUpQueue.drain()` at `:367` (v0.84.1 `:378`), so a rejected continuation leaves both
queues intact and the message is still delivered at the next drain point (`agent-loop.ts:259` /
`:263`, identical offsets at both tags). The **bodies** are byte-identical —
`diff <(git show v0.83.0:…agent.ts | sed -n '349,377p') <(git show v0.84.1:…agent.ts | sed -n '360,388p')`
is empty — but the **line numbers are not**, and the previous revision of this item asserted that
they were. Corrected in the 2026-08-12 repair pass (completeness critique finding 9).

**Impact** — *(Rewritten 2026-08-13 after the live measurement REFUTED the previous text; see the
block above.)* The drained `Vec<AgentMessage>` is dropped if `start_run` returns `Err(RunActive)`,
and because that happens inside the post-run driver loop it is invisible to the caller: `drive_run`
just `break`s at `session.rs:722` and the run ends looking normal. **Measured 2026-08-13 in the live
TUI: typing during an active stream does NOT reach this window** — the message is queued, echoed into
the transcript, and delivered when the run settles (5/5 attempts, including four timed at the settle
boundary). The loss is reachable only through the sub-millisecond `is_streaming`-cleared /
oneshot-unresolved window described in **AGENT-030**, and was not observed. The second branch would
lose a follow-up message the same way, and by the same narrow route.

*Previous text, retained so the correction is auditable:* "A user-typed steering message is silently
destroyed: the UI accepts it, the queue empties, and it never reaches the model or the transcript."

**Fix** — Hoist the busy check to the top of `Agent::continue_run` (`agent.rs:1635`) to mirror pi's
ordering: `if self.is_running() { return Err(AgentError::RunActive); }` before any `drain()`. That
alone is racy in Rust (pi gets atomicity from single-threaded JS), so keep it as a fast path **and**
make the two drain sites restore on failure: capture the drained vec and, on `Err` from `start_run`,
push each message back to the front of the queue (add `PendingQueue::push_front` in `queue.rs`)
before propagating. Prefer the restore form — it is correct even if a run starts between the check
and the claim.

**Verify** — Test in `crates/cyrup-agent/src/tests/agent_loop.rs`: build an agent whose transcript ends
with an assistant message, `agent.steer(user_text("keep-me"))`, start a long-running `prompt()` so
the latch is held, then call `continue_run()` and assert it returns `Err(AgentError::RunActive)`
**and** that `agent.has_queued_messages()` is still true. The second assertion fails today. Add the
session-level twin under `crates/cyrup-session-svc/tests/`: drive a two-iteration post-run loop,
submit a prompt during the gap, and assert the steering message still appears in a later turn's
transcript.

## AGENT-030 — `AgentSession::prompt` gates on the agent's per-run flag, so a prompt in the post-run gap starts a second run

**Kind** parity-bug · **Severity** high · **Effort** M · **Confidence** confirmed

**cyrup** — `AgentSession::prompt` (`crates/cyrup-session-svc/src/session.rs:627`) and `prepare`
(`session.rs:854`) both branch on `self.is_streaming().await`, which is
`self.agent.snapshot().await.is_streaming` (`session.rs:3202-3204`) — a flag `SettlementGuard::drop`
clears at `crates/cyrup-agent/src/agent.rs:1441` the moment each *individual* run's `agent_end`
settles. The session does own a wider latch — `driver_tx`, set true in `spawn_run`
(`session.rs:686`) and dropped only after the whole post-run loop finishes (`session.rs:739`) — but
it is consulted solely by `is_idle()` (`session.rs:601-603`) and by nothing on the submission path.

**upstream** — `pi/packages/coding-agent/src/core/agent-session.ts` **at v0.83.0** (v0.84.1 offsets in
parentheses; the code is byte-identical, only the line numbers move):
`_isAgentRunActive` is declared at `:313` (`:315`), set at the top of `_runAgentPrompt` at `:1062`
(`:1064`) and cleared only in the settle path at `:582` (`:597`), so it spans `_handlePostAgentRun()`
and every `agent.continue()`. `get isStreaming()` returns it at `:876-877` (`:878-879`) and
`prompt()` consults it at `:1159` (`:1167`) to route the submission through `_queueSteer`
(`:1168` / `:1176`) / `_queueFollowUp` (`:1166` / `:1174`) instead of starting a run.

**Impact** — During an auto-retry, an auto-compaction or a queued continuation, a user prompt that pi
would queue as steering is instead accepted as a **fresh run**. It races `drive_run`'s
`continue_run()` at `session.rs:716`, and whichever loses is silently discarded: the continuation's
queued message by AGENT-020, or `drive_run`'s own prompt because `session.rs:701` does
`if let Ok(handle) = self.agent.prompt(…)` and simply falls through on `Err`. Same seam as
AGENT-020; fixing one without the other leaves the loss on the other branch. **Severity note
(2026-08-12 repair pass):** the data-loss branch this item shares with AGENT-020 is what makes
AGENT-020 critical; this item's own primary defect is "starts a second run where pi queues", and the
loss here is race-conditional rather than unconditional, so it stays `high`. It must nevertheless
ship in the same change as AGENT-020.

**Fix** — Give the session a single run-active predicate spanning the whole driver loop and use it on
the submission path: make `AgentSession::is_streaming` read the `driver_tx` latch (or add
`is_run_active()` and switch `prompt` at `session.rs:627` and `prepare` at `session.rs:854` to it),
so a submission during the post-run gap routes to `queue_steer` / `queue_follow_up` as pi's
`prompt()` does at `agent-session.ts:1167`. Keep the agent-level `is_streaming` as the narrow
streaming indicator the UI wants. Fix site is area 08 — deduplicate there — but the behaviour under
test is the loop's run lifecycle, and it must land with AGENT-020.

**Verify** — Session-level test: start a prompt whose tool blocks on a barrier, release it so the run
settles into the post-run loop, submit a second prompt in that window, and assert (a) no second
`agent_start` is emitted and (b) the second prompt appears as steering in the continuation turn's
transcript. Today it starts a second run.

## AGENT-009 — Error tool results diverge in `details` and in `tool_execution_end.result` shape

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** confirmed

**cyrup** — `crates/cyrup-agent/src/agent.rs:123-144`: `result_value_of` unconditionally inserts
`"details"` as `Value::Null` when absent (`:132`) and unconditionally inserts `"terminate"` (`:142`),
while the sibling `update_value` (`agent.rs:150-158`) correctly omits `terminate` when `None`
(`:154-156`) — the two payloads are internally inconsistent. **Four** producers pass `details: None`
(the previous revision of this item named two): `immediate_error` (`agent.rs:1013`), `finalize`'s
tool-threw arm (`agent.rs:1061`), `finalize`'s after-hook-threw arm (`agent.rs:1113`), and
transitively `fail_truncated_tool_calls` via `agent.rs:915`. Second half, not previously stated:
`ToolResultMessage.details` carries `skip_serializing_if = "Option::is_none"`
(`crates/cyrup-agent/src/event.rs:96-98`), so the JSONL transcript **omits** the key where pi writes
`"details":{}`.

**upstream** — `pi/packages/agent/src/agent-loop.ts:756-761` **at v0.83.0** (v0.84.1 `:760-765`;
`agent-loop.ts` gained 4 lines at `:637` between the tags, so every citation past `:641` shifts):
`createErrorToolResult` returns `{ content: [{type:"text", text: message}], details: {} }` — an
object literal, and no `terminate`. `emitToolExecutionEnd` (`:763-771`, v0.84.1 `:767-775`) emits
`result: finalized.result` verbatim, so `JSON.stringify` drops absent keys. The transcript writer at
`:781` (v0.84.1 `:785`) therefore records `"details":{}`.

**Impact** — Any consumer that distinguishes "no details" from "empty details" — extensions, golden
snapshots, an SDK embedder, a differential fixture recorded from pi — sees a shape pi never emits:
`"details": null` and `"terminate": null` on every error result in the event stream, and a *missing*
`details` key in the JSONL.

**Fix** — In `immediate_error` (`agent.rs:1005-1035`) and both `finalize` error arms (`agent.rs:1061`,
`:1113`) emit an empty details map rather than `None`; in `result_value_of` (`agent.rs:123-144`)
insert `details` / `terminate` only when present. The `terminate` half requires
`cyrup_core::ToolResult.terminate` to become `Option<bool>`, as `ToolUpdate.terminate` already is.
Fix the transcript half in the same change so `details` serializes as `{}`.

**Verify** — Assert the serialized `tool_execution_end.result` for a not-found tool has
`details == {}` and no `terminate` key, and that the JSONL tool-result entry carries `"details":{}`.
`gap26_tool_execution_end_result_includes_terminate`
(`crates/cyrup-agent/src/tests/model_boundary.rs`) asserts `terminate == true` for a tool that genuinely
sets it — pi emits that too, so it survives the fix.

## AGENT-016 — Panicking tool in a parallel batch vanishes (unwind builds only)

**Kind** cyrup-original · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-agent/src/agent.rs:1232-1252`: the spawned body awaits
`tool.execute(cid.clone(), args, child, on_update)` at `:1242` with no `catch_unwind`. On unwind the
`ftx.send` at `:1244-1251` never runs, `remaining` (`:1224`, decremented only at `:1299`) never
reaches zero, the drain exits via `None => break` at `:1258`, and
`while joinset.join_next().await.is_some() {}` at `:1303` discards the `JoinError`. Consequences:
`finalized[idx]` stays `None`, so no `ToolExecutionEnd` (and `state.rs` is the only remover of
`pending_tool_calls`, so the id stays pending), **no tool-result message at all**, and `all_terminate`
forced false at `:1318`. The **sequential** path awaits inline at `agent.rs:1359-1377`, so an unwind
reaches the run-task `catch_unwind` at `agent.rs:1757` and closes cleanly — the two modes are
incomparable.

**upstream** — `pi/packages/agent/src/agent-loop.ts:666-707` **at v0.83.0** (v0.84.1 `:670-711`):
`executePreparedToolCall` wraps every execute in try/catch/finally and converts a throw into
`{ result: createErrorToolResult(…), isError: true }` at `:700-703` (v0.84.1 `:704-707`), identically
in both batch modes. pi cannot lose a tool result to a faulting tool.

**Impact** — Scoped by profile: `cyrup/Cargo.toml` sets `[profile.release] panic = "abort"`, so the
shipped binary aborts the process instead. Where it bites is `cargo test`, `cargo run`, and any
embedder building with `panic = "unwind"`. Added this pass: because the lost slot produces **no
tool-result message**, the next provider request carries an assistant `tool_use` with no matching
`tool_result` — a transcript violating the pairing invariant, which the provider rejects. So the
symptom is not merely a missing event; it is a broken conversation.

**Fix** — In the spawned body (`agent.rs:1232-1252`) wrap the await:
`match AssertUnwindSafe(tool.execute(…)).catch_unwind().await { Ok(r) => r, Err(p) => Err(ToolError { message: panic_message(p.as_ref()) }) }`,
reusing the existing `panic_message` helper; `FutureExt` is already in scope and `AssertUnwindSafe`
is sound for the same reason as at `agent.rs:387`. Do the same in the sequential path so both modes
match pi's single try/catch. Belt-and-braces: after the drain, synthesize an `immediate_error` for
any still-`None` slot in a batch that was not cancelled. Whether release should unwind is a separate
profile-policy question, out of scope here.

**Verify** — Test in `crates/cyrup-agent/src/tests/agent_loop.rs`: two parallel calls, tool A panics
`"boom-42"`, tool B returns normally. Assert (1) two `tool_execution_end`, A's with `is_error` and
content containing `boom-42`; (2) two tool-result `message_end` in source order; (3)
`turn_end.tool_results.len() == 2`; (4) `pending_tool_calls` empty before `agent_end`. Repeat under
`ToolExecution::Sequential` and assert an identical sequence. All four fail today in the parallel
case under the default (unwind) test profile.

## AGENT-017 — Per-turn refresh re-pushes only `tools`; mid-run model / thinking-level change never reaches the loop

**Kind** stale-port · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-session-svc/src/hooks.rs:170-181`: `prepare_next_turn` awaits the inner hook
and then sets exactly one field — `update.tools = Some(session.next_turn_tools().await);` at `:179` —
never `update.model` or `update.thinking_level`, although `TurnUpdate` carries both
(`crates/cyrup-agent/src/hooks.rs:121-122`) and the loop folds them stickily
(`agent.rs:582-587`). The mechanism exists and is honored; only the caller omits it.

**upstream** — `pi/packages/coding-agent/src/core/agent-session.ts:520-541` **at v0.83.0**, the tag
that governs this `stale-port` classification (v0.84.1 `:535-556`; the block is byte-identical, the
whole file having gained 15 lines above it — so the `model` / `thinkingLevel` lines cited below as
`:552-553` are `:537-538` at v0.83.0): `_installAgentNextTurnRefresh` returns `{ ...previousSnapshot, context: {…}, model: this.agent.state.model, thinkingLevel: this.agent.state.thinkingLevel }`
— `model` at `:537` and `thinkingLevel` at `:538` (v0.84.1 `:552` / `:553`), both **after** the
spread, so the session deliberately out-votes any extension override. `agent-loop.ts:233-245`
(identical offsets at both tags) folds them into the running config.

**Impact** — Switching model or cycling the thinking level while a run is in flight has no effect
until the next prompt: an agentic tool loop keeps streaming against the old model, the old reasoning
tier and the old price, where pi switches at the next turn boundary — while the session still
persists the change to the JSONL and emits the change events, so transcript and UI both claim a
switch that did not happen.

**Fix** — In `crates/cyrup-session-svc/src/hooks.rs:178-180`, alongside `update.tools`, set
`update.model` and `update.thinking_level` from the live agent snapshot, **after** the inner hook's
result so session precedence matches `agent-session.ts:537-538` @v0.83.0. Ship this **together with
AGENT-029** — this item is precisely what makes AGENT-029's header staleness fire. Leave the
deliberate, documented `systemPrompt` exception at `hooks.rs:158-160` alone.

**Verify** — Test in `crates/cyrup-session-svc/tests/` with a recording `StreamFn` capturing
`ModelRef` and `StreamOptions.reasoning` per request; drive a two-turn run and change model /
thinking level from a subscriber on the first `tool_execution_end`; assert request #2 carries the new
values. `crates/cyrup-agent/src/tests/turn_tool_refresh.rs` is the working template for the tools half.

**Caveat from the refuter** — the doc at `hooks.rs:154-156` is *accurate* about cyrup's own behaviour
(an extension's model / thinking_level do survive the spread there). What is false is the preceding
claim that the ordering "matches Pi exactly", since pi stamps the session's values **over** the
extension's. Correct that sentence specifically, not the whole comment.

## AGENT-021 — `loop_fn::build_run_ctx` hardcodes `headers: None`, orphaning `AgentLoopConfig.gen_config.headers`

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-agent/src/loop_fn.rs:120-134` builds `StateInner` for the low-level loop with
`headers: None` at `:130`, ignoring `config.gen_config.headers` entirely (the `gen_config` is moved
into `RunCtx` at `loop_fn.rs:150` but its `headers` field is never read again) — while `transport` at
`loop_fn.rs:133` **is** seeded correctly from `config.gen_config.transport`, which is the asymmetry.
AGENT-S01's fix moved the header read off `GenerationConfig` and onto live state
(`agent.rs:733` reads `lock(&self.state).headers`), and the only writers of `StateInner.headers` are
`AgentBuilder::build` (`agent.rs:1989`) and `Agent::set_headers` (`agent.rs:1503-1505`) — neither on
the low-level path. Net: `AgentLoopConfig { gen_config: GenerationConfig { headers: Some(h), .. }, .. }`
compiles, is accepted, and sends nothing. `GenerationConfig.headers` is still documented as live at
`crates/cyrup-agent/src/state.rs:21-22`.

**upstream** — `pi/packages/agent/src/types.ts:271` `export interface AgentLoopConfig extends
SimpleStreamOptions` — `headers` is a `SimpleStreamOptions` field, and `agent-loop.ts:308-312` spreads
the whole config into the provider call
(`streamFunction(config.model, llmContext, { ...config, apiKey: resolvedApiKey, signal })`). A
low-level `runAgentLoop` / `agentLoop` caller that sets `headers` therefore has them on the wire by
construction; there is no second storage location that can shadow them.

**Impact** — An SDK embedder driving `run_agent_loop` / `agent_loop` / `run_agent_loop_continue` /
`agent_loop_continue` with custom request headers — auth, provider attribution, a gateway routing
key, an org id — silently sends none of them. The request still goes out, so the failure surfaces as
an unexplained 401/403 or as silently mis-routed traffic rather than as a rejected config. Rated
above AGENT-S03's `low` deliberately: S03's field does not exist, so an embedder gets a compile
error, whereas this field exists, is documented as live, is accepted, and silently drops
potentially auth-bearing values.

**Fix** — `crates/cyrup-agent/src/loop_fn.rs:130` → `headers: config.gen_config.headers.clone(),`, the
same seeding `AgentBuilder::build` does at `agent.rs:1989`. Nothing else changes — `stream_assistant`
already reads it per turn.

**Verify** — Test in `crates/cyrup-agent/tests/` using the recording `StreamFn` template from
`crates/cyrup-agent/src/tests/model_boundary.rs:679-720`
(`set_headers_repoints_the_next_requests_header_overlay`): drive `run_agent_loop` with
`gen_config.headers = Some(map!["x-probe" => "42"])` and assert the captured `StreamOptions.headers`
on request #1 contains `x-probe`. It is `None` today.

## AGENT-022 — `BeforeToolCallResult.terminate` — a blocking hook cannot stop the turn

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** confirmed

**cyrup** — `crates/cyrup-agent/src/hooks.rs:49-52`:
`pub enum BeforeOutcome { Proceed, Block { reason: Option<String> } }` — no `terminate`. The block
path routes through `immediate_error` (`agent.rs:993-996`), which hardcodes `terminate: false` at
`agent.rs:1030`, so a blocked call can never satisfy `all_terminate` in either batch mode
(`agent.rs:1306-1319` parallel, `:1390-1392` sequential). The live producer is the permission gate:
`crates/cyrup-session-svc/src/hooks.rs:128-133` returns `BeforeOutcome::Block { reason: Some(reason) }`
with no way to express termination. The extension seam has the same hole:
`crates/cyrup-ext/wit/world.wit` declares the hook outcome as `block(option<string>)` with no
`terminate` (the only `terminate` in the file is on `tool-output`), and
`crates/cyrup-ext/src/hooks.rs:44` maps `Reduced::Blocked { reason }` straight through.

**upstream** — `pi/packages/agent/src/types.ts:61-69` at v0.84.1 adds `terminate?: boolean` to
`BeforeToolCallResult` ("Hint that the agent should stop after the current tool batch when this call
is blocked"); consumed at `agent-loop.ts:636-646`, which builds the error result then
`if (beforeResult.terminate === true) { result.terminate = true; }` before returning it, so the
blocked result participates in `shouldTerminateToolBatch` (`agent-loop.ts:582-584`). The
extension-facing twin landed in the same release:
`pi/packages/coding-agent/src/core/extensions/types.ts:1072-1081` at v0.84.1 has `terminate?: boolean`
on `ToolCallEventResult` where v0.83.0 has only `block` / `reason`; it reaches the loop through
`agent.beforeToolCall` at `agent-session.ts:479-499`, which returns the extension result verbatim.
Confirmed absent at the ported baseline: `git show v0.83.0:packages/agent/src/agent-loop.ts` has the
plain `return { kind: "immediate", result: createErrorToolResult(…), isError: true }`.

**Impact** — A gate or extension that denies every tool call in a batch cannot end the turn: cyrup
feeds the block errors back to the model and starts another provider request, so a hard-deny policy
costs an extra turn (and its tokens) per batch and can loop. Upstream, an extension that sets
`terminate` on the block stops the run cleanly at the batch boundary. Also an
advertised-but-unrepresentable capability once the WIT world is regenerated against v0.84.x — a
pi-v0.84 extension's documented field would be silently half-honoured.

**Fix** — Change `BeforeOutcome::Block` to `Block { reason: Option<String>, terminate: bool }`
(`crates/cyrup-agent/src/hooks.rs:49-52`); thread it through the block arm at `agent.rs:993-996` into
`immediate_error` (give it a `terminate: bool` parameter and set `Finalized.terminate` at
`agent.rs:1030` and `result_value_of`'s last argument at `:1028` from it); leave every other
`immediate_error` call site passing `false`. Then carry it on the extension seam: add `terminate` to
the tool-call result record in both copies of `world.wit` (`crates/cyrup-ext/wit/world.wit` and
`crates/cyrup-ext-sdk/wit/world.wit`, tied by `tests/wit_world_sync.rs`) and map it in
`crates/cyrup-ext/src/hooks.rs`. Batch the WIT half into the pending `cyrup:ext@0.4.0` bump described
as cluster C5 in `00-residual-ledger.md` rather than shipping a standalone minor.

**Verify** — Two-call parallel batch where a `before_tool_call` hook returns
`Block { reason: Some("denied"), terminate: true }` for **both** calls; assert `agent_end` follows
with no second `turn_start`, and that a batch where only one call blocks with `terminate` still runs
another turn. Repeat under `ToolExecution::Sequential`.

## AGENT-029 — A per-turn model override does not recompute the attribution header overlay

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-agent/src/agent.rs:677` `let model = self.model.clone();` reads the loop's own
running baseline, which `run_loop` rewrites stickily from a `TurnUpdate::model` at
`agent.rs:582-584`. Four lines later, `agent.rs:733` reads
`headers: lock(&self.state).headers.clone()` — a snapshot whose only writers are
`AgentBuilder::build` (`agent.rs:1989`) and `Agent::set_headers` (`agent.rs:1503-1505`), the latter
called from exactly two places, both *session-level* model changes
(`crates/cyrup-session-svc/src/session.rs:2792` `set_model_id`, `:3896` `apply_model_change`).
Nothing recomputes headers when the loop's own model changes mid-run. The public `Agent::set_model`
(`agent.rs:1497-1499`) writes `state.model` only and has the same hole for any embedder.

**upstream** — `pi/packages/coding-agent/src/core/sdk.ts:312-328` — **verified byte-identical AND
offset-identical at v0.83.0 and v0.84.1** (`git diff v0.83.0 v0.84.1 -- …/sdk.ts` is empty), so this
is one of the few citations in this file that needs no per-tag qualification: the `streamFn` closure
returns
`modelRuntime.streamSimple(model, context, { ...options, transformHeaders: async (requestHeaders) => { const headers = mergeProviderAttributionHeaders(model, settingsManager, options?.sessionId, requestHeaders); … } })`
— the merge is a per-request callback closed over the `model` argument of *that* invocation, i.e. the
model the loop chose for that turn (`agent-loop.ts:308`, where `config.model` is the possibly
overridden `nextTurnSnapshot.model ?? config.model` from `:237`). A model change of any origin gets
the right headers by construction. `getSessionHeaders` / `getDefaultAttributionHeaders`
(`pi/packages/coding-agent/src/core/provider-attribution.ts`) are keyed on the model's provider id
and base-url host.

**Impact** — The same silent cross-vendor leak AGENT-S01 was filed for, on the path AGENT-S01's fix
does not cover. Once **AGENT-017** lands — the session pushing `update.model` on every turn is the
whole point of that item — every mid-run model switch will send the *previous* provider's
attribution: an opencode `x-opencode-session` UUID to `api.anthropic.com`, or an OpenRouter
`HTTP-Referer` / `X-OpenRouter-Title` to whoever is next. Silent in both directions — no error, just
a stale header and, on the reverse switch, degraded prompt-cache routing. Rated medium despite being
latent today (no in-tree producer of `TurnUpdate::model`) because the rating is carried by the
coupling: shipping AGENT-017 apart from this creates the leak.

**Fix** — Make the overlay a function of the model rather than a latched snapshot. Minimal form: give
`RunCtx` an optional `Arc<dyn Fn(&ModelRef) -> Option<HeaderMap> + Send + Sync>` (pi's
`transformHeaders`), seeded by the session builder from `AgentSession::attribution_headers`
(`crates/cyrup-session-svc/src/session.rs:2734`, which already computes this correctly per model),
and call it in `stream_assistant` at `agent.rs:733` with the live `model` instead of reading
`state.headers`. Keep `Agent::set_headers` as the static-overlay input the resolver merges over, so
the existing two call sites and `crates/cyrup-agent/src/tests/model_boundary.rs:691-720` keep passing.
Do this in the same change as AGENT-017.

**Verify** — Recording `StreamFn` capturing `(ModelRef, StreamOptions.headers)` per request; a
`prepare_next_turn` hook returning `TurnUpdate { model: Some(other_provider_model), .. }` after turn
1; assert request #2's headers are the ones `attribution_headers(other)` computes, and specifically
that no `x-opencode-session` from the first provider survives. Today request #2 carries request #1's
overlay.

## AGENT-003 — `tool_execution_update` dropped when the bounded channel fills

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — Parallel path: `crates/cyrup-agent/src/agent.rs:1169`
`mpsc::channel::<ToolRuntimeMsg>(64)` with the sink at `:1236-1239` doing `let _ = utx.try_send(…)`,
discarding the `Result`. Sequential path: `agent.rs:1350` `mpsc::channel::<ToolUpdate>(64)` and
`:1355` `let _ = utx.try_send(u);`. The `accepting` AtomicBool correctly mirrors pi's
`acceptingUpdates`.

**upstream** — `pi/packages/agent/src/agent-loop.ts:671` **at v0.83.0** (v0.84.1 `:675`)
`const updateEvents: Promise<void>[] = []`; `:681-691` (v0.84.1 `:685-695`) pushes every emission;
`await Promise.all(updateEvents)` at `:695` / `:699` (v0.84.1 `:699` / `:703`) on both the success
and throw paths. The only upstream drop rule is `acceptingUpdates` (`:672`, `:680`, `:694`, `:698`,
`:705` — v0.84.1 `:676`, `:684`, `:698`, `:702`, `:709`).

**Impact** — Progress output from a chatty tool can be silently truncated in the UI and in the
transcript. Bounded: after 8854601's two-phase rewrite the receiver drains within microseconds of
the first spawn, so only a synchronous burst of >64 updates outruns it, and the built-in bash tool
throttles at 100ms leading+trailing. Caveat: third-party and extension tools have no such throttle,
so `low` is a statement about the built-ins.

**Fix** — Switch both channels to `mpsc::unbounded_channel` and replace `try_send` with `send` in both
paths, keeping the `accepting` gate as the sole drop rule.

**Verify** — Tool emitting 500 updates synchronously with no await between them; assert 500
`tool_execution_update` events reach a subscriber.

## AGENT-010 — Loop-generated tool-error strings do not match pi's

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-agent/src/agent.rs:952` `format!("Tool '{}' not found", call.name)` (single
quotes); `agent.rs:995`
`reason.unwrap_or_else(|| "Tool call blocked by beforeToolCall".to_string())`.

**upstream** — `pi/packages/agent/src/agent-loop.ts:611` (identical offset at both tags — the
inter-tag insertion starts at `:637`)
`` createErrorToolResult(`Tool ${toolCall.name} not found`) `` (no quotes); and **at v0.83.0**
`agent-loop.ts:639` `result: createErrorToolResult(beforeResult.reason || "Tool execution was
blocked")` — at v0.84.1 the same expression moved up to `:637` and was hoisted into a `const result`
so `terminate` could be stamped on it (that hoist is AGENT-022). The abort string already matches on
both sides (`agent.rs:968` / `:1000` vs `agent-loop.ts:632` / `:647`; v0.84.1 `:632` / `:651`).

**Impact** — These strings go into the transcript sent back to the model, so cyrup and pi feed
different text on identical inputs — a divergence in what the model conditions on, and a mismatch for
any golden/differential fixture recorded from pi.

**Fix** — `agent.rs:952` → `format!("Tool {} not found", call.name)`; `agent.rs:995` →
`"Tool execution was blocked"`. Fix **AGENT-032**'s falsy-fallback half in the same edit — the same
line is the site.

**Verify** — Assert the exact strings in the emitted tool result.
`crates/cyrup-session-svc/src/tests/mid_run_tool_anchoring.rs:235` is a **comment** quoting the current
wrong string; the assertion below it does not match on text, so only the comment needs updating.

## AGENT-011 — `state.error_message` gated wrongly and invents a message

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-agent/src/state.rs:160-166`: the `TurnEnd` arm gates on
`matches!(a.stop_reason, StopReason::Error | StopReason::Aborted)` and falls back to
`Some("turn ended with error".to_string())` when the message carries no `error_message`.

**upstream** — `pi/packages/agent/src/agent.ts:558-562` **at v0.83.0** (v0.84.1 `:573-577`; bytes
identical, offsets shifted by the +15-line `agent.ts` drift):
`case "turn_end": if (event.message.role === "assistant" && event.message.errorMessage) { this._state.errorMessage = event.message.errorMessage; } break;`
— gated purely on presence, no stop-reason gate, no synthetic fallback.

**Impact** — Two divergences: an aborted turn with no `errorMessage` gets a fabricated string in cyrup
and nothing in pi (a user-visible "turn ended with error" on a deliberate cancel); and a `turn_end`
carrying an `errorMessage` with a non-error stop reason updates pi's state but not cyrup's, so a
recoverable-error annotation is lost.

**Fix** — Rewrite the arm to drop both the stop-reason gate and the fallback.

**Verify** — Reducer unit tests for both directions. No test asserts `"turn ended with error"`
anywhere in the workspace.

## AGENT-012 — Abort checked before `before_tool_call`, and ordered below block

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

> **Severity corrected medium → low this pass.** Both divergences are real and were re-read on both
> sides, but the entire observable consequence is (a) the hook is not invoked on an already-cancelled
> run where pi invokes it, and (b) on an abort landing during the hook, the tool result carries the
> block reason instead of `"Operation aborted"`. Both are error-path text / invocation-count
> divergences with no data loss and no wrong output on a normal path — the same class as AGENT-010,
> which this file rates low.

**cyrup** — `crates/cyrup-agent/src/agent.rs:967-969` keeps a pre-hook abort check
(`if self.cancel.is_cancelled() { return Prep::Immediate(… "Operation aborted") }`) **before**
`before_tool_call` at `:984`. Second divergence at `agent.rs:986-1005`: the arms are ordered `Err`
(`:992`), `Block` (`:993-996`), then `Proceed` (`:998-1004`) with `is_cancelled()` nested inside the
`Proceed` arm at `:999`.

**upstream** — `pi/packages/agent/src/agent-loop.ts:616-656` **at v0.83.0** (v0.84.1 `:616-660`):
`prepareToolCall` has **no** pre-hook abort check. The only checks are `if (signal?.aborted)` at
`:629` — identical offset at both tags, immediately after the hook returns and **before** the block
branch at `:636` — and a second at `:644` (v0.84.1 `:648`). pi therefore always invokes
`beforeToolCall`, and abort out-votes a block.

**Impact** — On abort, extensions relying on `beforeToolCall` firing for every call (audit logs,
permission bookkeeping, ref-counted resources) silently miss calls. And a call the hook blocked
during an aborted run reports the block reason where pi reports `"Operation aborted"`, so the
transcript attributes the stop to policy instead of to the user.

**Fix** — Delete `agent.rs:967-969`; hoist the `is_cancelled()` check out of the `Proceed` arm so it
runs on any `Ok(_)` before the `Block` branch is considered.

**Verify** — Counting `before_tool_call` hook plus a token cancelled before prep: assert the hook
count equals the call count, and that a hook returning `Block{reason}` under a cancelled token yields
`"Operation aborted"`.

## AGENT-013 — Proxy HTTP failures lack pi's `Proxy error: …` message

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-agent/src/proxy.rs:458`, `:479`, `:489`, `:502`, `:516` — all five failure
paths (build client, open SSE, mid-stream frame, JSON parse, builder mismatch) call
`error_terminal(&builder, &cancel, e.to_string())`; `error_terminal` (`proxy.rs:529-541`) assigns
`error.error_message = Some(message)` at `:539` — the raw `ProviderError` `Display`, never pi's
literal, and the server's JSON `error` field is never extracted.

**upstream** — `pi/packages/agent/src/proxy.ts:167` **at v0.83.0** (v0.84.1 `:169`)
`` let errorMessage = `Proxy error: ${response.status} ${response.statusText}`; `` upgraded at `:171`
(v0.84.1 `:173`) to `` errorMessage = `Proxy error: ${errorData.error}`; `` when the body parses as
`{error?: string}`. Byte-unchanged at v0.84.1 — the entire v0.83.0→v0.84.1 `proxy.ts` diff is the two
`samplingParams` insertions of AGENT-026, which is also why these two offsets shift by exactly 2.

**Impact** — Proxy-mode failures surface an unstructured raw body instead of pi's two-tier message, so
a proxy's own JSON error string is buried and the failure is not attributable to the proxy at a
glance.

**Fix** — Match `ProviderError::Http { status, message }` specifically in `run_proxy` and reproduce
pi's two-tier construction (status + statusText, upgraded to `errorData.error` when the body parses);
fall back to `e.to_string()` for non-HTTP variants.

**Verify** — Stub proxy returning 502 with `{"error":"upstream down"}`; assert the terminal
`errorMessage == "Proxy error: upstream down"`, and `Proxy error: 502 Bad Gateway` when the body is
not JSON.

## AGENT-015 — Aborted parallel batch: unprepared calls veto termination

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

> **Severity corrected medium → low this pass.** The divergence is real, but the trigger was traced:
> an abort landing *during* `prepare()` still fills the slot with an Immediate "Operation aborted"
> (`terminate: false`) on both sides, so both agree. The divergence needs the cancel to land in the
> narrow window at `agent.rs:1215` **between** two `prepare()` calls **and** every already-prepared
> entry to have set `terminate` — a hint only `crates/cyrup-ext-subagents` produces
> (`prompt_runtime.rs:991`, `extension.rs:17634` / `:17677`). Outside that window the only other
> unfilled-slot path is a panicking tool, which is AGENT-016.

**cyrup** — `crates/cyrup-agent/src/agent.rs:1168` pre-sizes `finalized: Vec<Option<Finalized>>` to
`n`; the prep loop breaks on cancel at `:1215-1217`; the fold starts
`let mut all_terminate = !finalized.is_empty();` at `:1306` — true whenever `calls` is non-empty,
because the vec was pre-sized — and `:1318` `None => all_terminate = false` lets every never-prepared
slot veto termination. The sequential path already has the right shape via the `produced` counter
(`agent.rs:1333`, `:1397`, `:1403-1405`), so the two batch modes disagree with each other as well as
with pi.

**upstream** — `pi/packages/agent/src/agent-loop.ts:497`
`const finalizedCalls: FinalizedToolCallEntry[] = []` holds only entries actually pushed;
`orderedFinalizedCalls` (`:540-542`) is that shortened list; `shouldTerminateToolBatch`
(`agent-loop.ts:582-584`) is `finalizedCalls.length > 0 && finalizedCalls.every(f => f.result.terminate === true)`
over it.

**Impact** — Abort a run mid-parallel-batch where the prepared tools all set `terminate` and cyrup
runs another turn instead of ending — a further provider request (cost, latency, and a turn the user
did not ask for) where pi terminates. Switching the same workload to sequential execution changes the
outcome.

**Fix** — Fold over present slots only:
`let present: Vec<_> = finalized.into_iter().flatten().collect(); let all_terminate = !present.is_empty() && present.iter().all(|f| f.terminate);`,
keeping the message-emission loop over `present`.

**Verify** — Two-call parallel batch, both tools `terminate: true`, token cancelled after the first
prep; assert `agent_end` follows with no second `turn_start`, and assert the same sequence under
`ToolExecution::Sequential`.

## AGENT-018 — Reducer diverges on non-assistant `message_start` and on when `pendingToolCalls` clears

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** medium

**cyrup** — `crates/cyrup-agent/src/state.rs:142-146`: `MessageStart` sets `streaming_message` only
`if message.is_assistant()`, while the matching `MessageEnd` (`:150-153`) unconditionally clears it.
`state.rs:167-170`: the `AgentEnd` arm does
`st.streaming_message = None; st.pending_tool_calls.clear();`, and `RunCtx::emit`
(`agent.rs:381-396`) reduces at `:383-385` **before** awaiting subscribers at `:387-395`, so a
subscriber handling `agent_end` already sees an empty pending set.

**upstream** — `pi/packages/agent/src/agent.ts` **at v0.83.0** (v0.84.1 offsets in parentheses; bytes
identical). `:531-533` (`:546-548`):
`case "message_start": this._state.streamingMessage = event.message; break;` — no role check.
`:564-566` (`:579-581`): the `agent_end` case clears only `streamingMessage`; `pendingToolCalls` is
reset in `finishRun()` (`:514-520`, the clear at `:517` — v0.84.1 `:529-535` / `:532`), called from
the `finally` at `:491-493` (v0.84.1 `:506-508`) — i.e. **after** all listeners settle.

**Impact** — Two observability divergences for anything reading `AgentStateSnapshot` from inside a
subscriber: (a) a front-end or extension rendering `streaming_message` shows nothing for user and
tool-result messages where pi shows them; (b) a subscriber reading `pending_tool_calls` on
`agent_end` to detect calls abandoned by an aborted run sees an empty set under cyrup and the real
set under pi — exactly the diagnostic that would surface AGENT-016. No effect on transcript or
provider payloads.

**Fix** — Drop the `is_assistant()` guard at `state.rs:142`. Move `pending_tool_calls.clear()` out of
the `AgentEnd` arm into `SettlementGuard::drop` (`agent.rs:1437-1452`), mirroring pi's `finishRun`.

**Verify** — Reducer unit test: `reduce(MessageStart{ AgentMessage::user_text("hi") })` →
`streaming_message.is_some()`. Integration test in `crates/cyrup-agent/src/tests/agent_loop.rs` with a
subscriber snapshotting `pending_tool_calls` inside its `agent_end` handler on a run aborted
mid-batch, asserting the set is non-empty.

**Caveat** — The assistant-only rule cites R-02-040 in its own doc comment. `spec/` is not in this
workspace, so whether that requirement sanctions the divergence cannot be adjudicated here; filed at
`low` with the uncertainty explicit. The `pending_tool_calls` half carries no such citation and is
unambiguous.

## AGENT-019 — `a_02_2_parallel_completion_vs_source_order` asserts wall-clock latency and a sleep-derived order

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-agent/src/tests/agent_loop.rs:327`:
`assert!(elapsed < Duration::from_millis(115), …)` where `elapsed` spans `prompt` →
`handle.finished()` → `wait_for_idle()` (`:276-281`) — faux-provider streaming and every subscriber
await, not just the tool bodies — while the two `SpanTool`s sleep 80ms and 50ms (`:264-265`), leaving
~35ms for everything else. `agent_loop.rs:301-302`: `assert_eq!(ends, vec!["fast","slow"], …)` plus
`assert_ne!(ends, starts, …)` derive the expected completion order purely from the 80-vs-50ms gap,
i.e. from the scheduler. The real `(name, start, end)` intervals collected into `spans` at `:263` are
never read; the sibling `a_02_3_one_sequential_forces_batch_sequential` does exactly that at
`:352-354` (`assert!(s[0].2 <= s[1].1)`).

**upstream** — The property under test is `pi/packages/agent/src/agent-loop.ts:540-542`
(`await Promise.all(...)`), i.e. "the batch is concurrent" — a structural claim about overlapping
intervals. pi's suite is not an oracle for a wall-clock bound.

**Impact** — Under load or a debug-profile runner `elapsed` crosses 115ms and the suite fails for
reasons unrelated to the code; the reflex remedy is to raise the constant until the assertion proves
nothing. The completion-order assertion fails the same way in the opposite direction — one 30ms
hiccup on `fast` inverts it. These are the only assertions covering the concurrency half of
AGENT-002's fix, so loosening them silently stops guarding what 8854601 landed.

**Fix** — Replace `agent_loop.rs:327` with an interval-overlap assertion over the `spans` already
collected: sort by start and assert `s[0].2 > s[1].1` — the dual of `a_02_3`'s non-overlap check.
For the completion-order half, release the two tools with a test-driven `Notify`/oneshot the way
`agent_002_parallel_defers_execution_until_whole_batch_is_prepared` uses a `Barrier`, so `ends` vs
`starts` becomes a fact rather than a race.

**Verify** — After the rewrite, run `cargo test -p cyrup-agent a_02_2` under artificial load
(`taskset -c 0` alongside a busy loop) and confirm it still passes; today the wall-clock assertion
fails under that condition while the code is correct.

## AGENT-023 — `Agent::reset()` still wipes state under a live run; upstream now throws

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed

> **Severity corrected medium → low this pass.** The drift is real, but `grep -rn '\.reset()' crates`
> finds **no production caller** of `Agent::reset` anywhere in the tree — the only call site is
> `crates/cyrup-agent/src/tests/model_boundary.rs:649`. This is an SDK-surface-only hazard, the class
> this file rates low for AGENT-S02/S03. The fix is still a two-line guard.

**cyrup** — `crates/cyrup-agent/src/agent.rs:1601-1616`: the doc at `:1601-1603` says "Clear
transcript, runtime state, and queued messages — unconditionally, even mid-run (Pi `reset`,
agent.ts:313-322)" and the body clears `st.messages`, `is_streaming`, `streaming_message`,
`pending_tool_calls`, `error_message` (`:1606-1612`) plus both queues (`:1613-1614`) with no check of
`running_rx` / `cancel_slot`. `Agent::is_running()` (`agent.rs:1585-1587`) is right there and unused
by this path. The cited line range no longer exists upstream.

**upstream** — `pi/packages/agent/src/agent.ts:332-345` at v0.84.1:
`reset(): void { if (this.activeRun) { throw new Error("Agent is already processing. Wait for completion before resetting."); } … }`
— the guard at `:334-336` is new; `git show v0.83.0:packages/agent/src/agent.ts` has no guard at the
corresponding `reset()`, so this is drift, not an old miss.

**Impact** — A `reset()` racing an in-flight run empties `state.messages` while the loop keeps
reducing `message_end` into it, so the run resumes writing into a cleared transcript: the JSONL and
the UI end up with a turn whose prior context is gone, and `pending_tool_calls` is cleared while
tools are still executing, so `tool_execution_end` removes ids from an already-empty set. Upstream
refuses the call outright.

**Fix** — Add the guard at the top of `agent.rs:1604`:
`if self.is_running() { return Err(AgentError::RunActive); }` — the `Result<(), AgentError>` return
type already exists and is documented as "always `Ok`", so this is a behaviour change with no
signature change. Update the doc comment at `agent.rs:1601-1603` to cite `agent.ts:332-345` and drop
the "unconditionally, even mid-run" claim. Audit callers for the new `Err` (there are none in
production today, so this is a one-file change).

**Verify** — Start a long-running prompt against a faux provider that blocks on a barrier, call
`reset()`, assert `Err(AgentError::RunActive)` and that `snapshot().messages` is unchanged; release
the barrier and assert the run still completes with a well-formed transcript.

## AGENT-024 — The post-turn hooks receive no abort signal

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

> **Corrected this pass — both the severity and the mechanism.** The auditor filed this as "cyrup's
> post-turn hook signatures diverge from upstream's". They do **not**: pi's *loop-level*
> `AgentLoopConfig.shouldStopAfterTurn` (`types.ts:222` at v0.84.1, `:217` at v0.83.0) and
> `prepareNextTurn` (`types.ts:229-231`) take **no** signal, exactly like cyrup's `Hooks`
> (`hooks.rs:228-238`), while `transformContext` (`:200`), `beforeToolCall` (`:275`) and
> `afterToolCall` (`:291`) do — the same split cyrup has. The real residue is narrower and lives one
> layer up, at pi's Agent-options layer, which cyrup does not model separately.

**cyrup** — `crates/cyrup-agent/src/hooks.rs:228-238`:
`async fn prepare_next_turn(&self, _ctx: PostTurn<'_>) -> Result<Option<TurnUpdate>, HookError>` and
`async fn should_stop_after_turn(&self, _ctx: PostTurn<'_>) -> Result<bool, HookError>` take no
`CancelToken`, and the call sites at `agent.rs:574` and `agent.rs:638` pass only the context even
though `self.cancel.child()` is available and used four lines away at `agent.rs:685` and `:984`.
Because cyrup has no equivalent of pi's Agent-options wrapper layer, there is nowhere else for the
run's token to enter. The real consumer is the session's per-turn refresh
(`crates/cyrup-session-svc/src/hooks.rs:170-181`), which awaits `session.next_turn_tools().await` at
`:179` with no way to bail out on abort.

**upstream** — `pi/packages/agent/src/agent.ts:463-471` at v0.84.1 — `createLoopConfig` wraps the user
hook as
`async (context) => { if (this.prepareNextTurnWithContext) { return await this.prepareNextTurnWithContext(context, this.signal); } return await this.prepareNextTurn?.(this.signal); }`,
binding the run's `AbortSignal` into the closure it hands the loop. The `prepareNextTurn` half is not
drift — `git show v0.83.0:packages/agent/src/agent.ts` has the identical `this.signal` argument — so
that gap predates the ported baseline. The `shouldStopAfterTurn` half **is** v0.84.1 drift:
`agent.ts:460-462` plus the `AgentOptions.shouldStopAfterTurn` field at `:108` and the public field at
`:193-196` all landed in this window. Contexts: `types.ts:126-135`
(`ShouldStopAfterTurnContext`), `:147` (`PrepareNextTurnContext extends` it).

**Impact** — A post-turn hook doing real work runs to completion after the user has pressed Ctrl-C, so
abort latency is bounded by the slowest post-turn hook instead of by the loop; cyrup checks
cancellation only at `agent.rs:967` / `:999` / `:1215` / `:1399` and inside `stream_assistant`'s
select, never between `turn_end` and the next turn. Small in practice today because the only in-tree
post-turn work is `session.next_turn_tools().await` — a local tool-set rebuild, not a network call —
which is why this is low. Second half: an embedder porting a pi extension that reads the signal in
`shouldStopAfterTurn` has no cyrup surface to bind it to.

**Fix** — Add `_cancel: CancelToken` as the second parameter to both trait methods
(`crates/cyrup-agent/src/hooks.rs:228-238`), defaulting the bodies as today; pass `self.cancel.child()`
at the two call sites (`agent.rs:574`, `:638`), matching the existing pattern at `agent.rs:685` /
`:984` / `:1082`. Update the production impls in `crates/cyrup-session-svc/src/hooks.rs:170-186` —
have `prepare_next_turn` return early when the token is already cancelled, so a cancelled run does
not pay for `next_turn_tools()` — plus any test doubles under `crates/cyrup-agent/tests/`.

**Verify** — Hook whose `prepare_next_turn` awaits `cancel.cancelled()` and records that it observed
the signal; abort the run from a subscriber on `turn_end` and assert the hook returned promptly and
recorded the observation. Also assert the total run wall time does not include the hook's would-be
sleep.

## AGENT-025 — A `transform_context` / `convert_to_llm` failure emits the wrong `agent_end.messages` and never reports `aborted`

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-agent/src/agent.rs:859-877`: `emit_error_assistant` builds the failure with
`StopReason::Error` **hardcoded** at `:870` and emits only `MessageStart` / `MessageEnd`; control
returns to `run_loop`, which pushes the failure onto `self.messages` and `self.new_messages` at
`:505-506` and then emits `TurnEnd` at `:509-513` and
`AgentEnd { messages: self.new_messages.clone() }` at `:514` — the whole run accumulator (prompt +
failure), not the failure alone. The sibling path added by AGENT-007's fix gets this right:
`emit_run_failure` (`agent.rs:415-435`) picks `Aborted` vs `Error` from `self.cancel.is_cancelled()`
at `:420-421`, emits `AgentEnd { messages: vec![fm.clone()] }` at `:433`, and sets
`self.new_messages = vec![fm]` at `:434`. Two paths, one upstream function, two different payloads.

**upstream** — `pi/packages/agent/src/agent-loop.ts:288-295` (identical offsets at both tags) awaits
`config.transformContext` (`:291`) and `config.convertToLlm` (`:295`) bare, so a rejection unwinds
out of `streamAssistantResponse` → `runLoop` (`:193`) → `runAgentLoop` (`:116`) →
`runWithLifecycle`'s catch at `pi/packages/agent/src/agent.ts:489-490` **at v0.83.0** (v0.84.1
`:504-505`) → `handleRunFailure(error, abortController.signal.aborted)`. `handleRunFailure`
(`agent.ts:496-512`, v0.84.1 `:511-527`) sets `stopReason: aborted ? "aborted" : "error"` at `:504`
(v0.84.1 `:519`) and emits `{ type: "agent_end", messages: [failureMessage] }` at `:511` (v0.84.1
`:526`) — the single synthetic message and nothing else. The throw at `agent-loop.ts:193` also means
`newMessages.push(message)` at `:194` never runs, so upstream's accumulator never receives the
failure either.

**Impact** — Two observable divergences on the same path. (a) `agent_end.messages` differs in length
and content from pi's, and it is a contract surface:
`crates/cyrup-session-svc/src/subscriber.rs:193-196` and
`crates/cyrup-session-svc/src/event.rs:328-330` map it straight to `AgentSessionEvent::AgentEnd`,
which reaches the RPC/JSON wire and every extension guest via `crates/cyrup-ext/src/event.rs:463` →
`crates/cyrup-ext/src/host/live.rs:1550`; `RunHandle::finished()` returns the same wrong vector.
(b) Cancelling a run while a slow `transform_context` (the compaction / context-budget hook is
exactly this) is in flight yields `stopReason: "error"` and a red error state where pi reports a
clean `"aborted"`. Neither is pinned by a test:
`crates/cyrup-agent/src/tests/hook_failure_text.rs:260-311` asserts only `error_message` and
`StopReason::Error`.

**Fix** — Delete `emit_error_assistant` (`agent.rs:859-877`) and route both `Err` arms in
`stream_assistant` (`agent.rs:691`, `:696`) through the existing `emit_run_failure`
(`agent.rs:415-435`) — it already emits the identical four-event quartet with the correct stop reason
and payload. That requires `stream_assistant` to take `&mut self` (it is `&self` at `agent.rs:670`)
and `run_loop` to return immediately rather than falling through to the `Error|Aborted` branch at
`agent.rs:508-516`. Alternative if that refactor is unwanted: keep the split, make
`emit_error_assistant` choose the aborted stop reason, and have `run_loop` replace `new_messages`
with the single failure before emitting `AgentEnd`. Update the two tests in `hook_failure_text.rs`
to also assert the `agent_end` payload length.

**Verify** — Hook whose `transform_context` returns `Err`; assert `agent_end.messages.len() == 1` and
that the single message is the failure. Second case: a hook that awaits the cancel token and returns
`Err` after abort; assert the emitted `stop_reason` is `aborted` and that `snapshot().error_message`
matches pi's.

## AGENT-026 — `samplingParams` absent from the proxy request body and from `StreamOptions`

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-agent/src/proxy.rs:305-322`: `ProxyStreamOptions` carries
temperature / max_tokens / reasoning / cache_retention / session_id / headers / metadata / transport /
thinking_budgets / max_retry_delay_ms and no `sampling_params`; `build_proxy_request_options`
(`proxy.rs:377-393`) forwards exactly that set into `ProxyRequestOptions` (`proxy.rs:327-352`).
Workspace-wide `grep -rn 'sampling_params\|samplingParams' crates/` returns zero hits, so
`cyrup_provider::StreamOptions` has no such field either and nothing upstream of the proxy could
populate it.

**upstream** — `pi/packages/agent/src/proxy.ts` at v0.84.1 adds `"samplingParams"` to the
`ProxySerializableStreamOptions` Pick at `:59-71` and `samplingParams: options.samplingParams,` to
`buildProxyRequestOptions` at `:102-114` — the entire v0.83.0→v0.84.1 diff of that file. The field is
declared at `pi/packages/ai/src/types.ts:183-189`
(`samplingParams?: Record<string, unknown>` — "Arbitrary sampling parameters merged into the request
body as-is, after the named request fields, so keys here override them … e.g. top_p, top_k, min_p,
repetition_penalty. Merged over `Model.samplingParams` per key. Only applied by OpenAI-compatible
adapters") with the per-model default at `types.ts:810-811`, introduced by commit `25a2c8dcf`.

**Impact** — Scoped by what consumes it. Upstream it is honoured only by the OpenAI-compatible
adapters, so the user-visible loss is: a custom llama.cpp / vLLM / SGLang endpoint configured with
`top_p` / `top_k` / `min_p` / `repetition_penalty` gets none of them, silently, and generates with the
server's defaults instead of the user's. The proxy half is narrower: a cyrup client talking to a pi
proxy server omits the key so the server falls back to its own defaults. No error either way.

**Fix** — Two halves. **Area 01 owns the primary**: add
`sampling_params: Option<serde_json::Map<String, Value>>` to `cyrup_provider::StreamOptions`
(`crates/cyrup-provider/src/stream.rs`, beside `metadata` at `:188`) and merge it over
`Model.sampling_params` into the request body after the named fields in the OpenAI-compatible
adapters. This area's half: add the matching field to `ProxyStreamOptions`
(`crates/cyrup-agent/src/proxy.rs:305-322`), the `ProxyRequestOptions` wire struct (`:327-352`) with
`skip_serializing_if = "Option::is_none"`, the mapping in `build_proxy_request_options`
(`:377-393`), and the copy in `ProxyStreamFn::options_from`; then thread it through
`GenerationConfig` (`crates/cyrup-agent/src/state.rs`) and `AgentBuilder` so the loop can set it at
all.

**Verify** — Proxy: assert the serialized request body contains `"samplingParams":{"top_p":0.9}` when
set and omits the key when unset. Provider: a recording adapter asserting the OpenAI-completions body
carries the merged keys with per-request keys overriding `Model.samplingParams`.

**Caveat** — The exact merge order and which of the three OpenAI-compatible adapters apply it is
asserted from upstream's type docs, not from reading the adapter bodies; area 01 should confirm
before scheduling.

## AGENT-027 — `timings.ts` ported as one hardcoded namespace with 3 of 12 marks

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup/src/timings.rs:31-81`: `Timings` is a single flat struct with
`entries: Vec<(String, u128)>` and `print()` hardcoding `let title = "Startup Timings: main";` at
`:71`. There is no namespace concept, so a second instance would also print "main". Production use is
three marks total: `crates/cyrup/src/main.rs:59` (`Timings::new()`), `:127` (`parseArgs`), `:186`
(`runMigrations`), `:538` (`interactiveMode.init`). Nothing in `crates/cyrup-ext` — the extension
loader — marks anything.

**upstream** — `pi/packages/coding-agent/src/core/timings.ts:12-49` keys a
`Map<TimingLabel, TimingNamespace>` on `type TimingLabel = "main" | "extensions"` (`:12`), and
`printTimings` (`:45-49`) iterates every namespace printing one titled group each. The `main`
namespace carries **twelve** marks in `pi/packages/coding-agent/src/main.ts` — `parseArgs`,
`runMigrations`, `firstTimeSetup`, `createSessionManager`, `createRuntime`,
`createAgentSessionRuntime`, `readPipedStdin`, `prepareInitialMessage`, `initTheme`,
`resolveModelScope`, `createAgentSession`, `interactiveMode.init` (≈ `:619`, `:656`, `:665`, `:700`,
`:842`, `:848`, `:876`, `:883`, `:885`, `:892`, `:900`, `:939`). The `extensions` namespace is reset
at `pi/packages/coding-agent/src/core/resource-loader.ts:388` and filled per extension at
`pi/packages/coding-agent/src/core/extensions/loader.ts:501` (`${extensionPath} module import`),
`:509` and `:532` (`${extensionPath} factory`).

**Impact** — `CYRUP_TIMING=1` / `PI_TIMING=1` is a supported diagnostic (documented in the module
header at `timings.rs:1-8`) that answers a different question in cyrup than in pi: the whole
extension-loading phase — the most common cause of a slow start, and the only phase attributable to a
specific user-installed extension — is invisible, as are session-manager creation, runtime creation,
piped-stdin read, theme init and model-scope resolution. A maintainer profiling startup gets a
three-row table with an unexplained gap where nine rows and a per-extension breakdown should be.

**Fix** — Add a namespace key to `Timings` (an enum `TimingNamespace { Main, Extensions }` or a
`&'static str` label) and make `print()` emit one titled group per namespace rather than hardcoding
the title at `timings.rs:71`; the simplest faithful shape is a process-global
`Mutex<HashMap<&'static str, Vec<(String,u128)>>>` mirroring pi's module-level Map, since the
extension loader has no handle on `main`'s local `Timings`. Then add the nine missing `mark()` calls
in `crates/cyrup/src/main.rs` at the sites corresponding to pi's, and mark per-extension module-load
and instantiate in the `crates/cyrup-ext` loader under the `extensions` namespace.

**Verify** — Run with `CYRUP_TIMING=1` and assert the output contains both
`--- Startup Timings: main ---` and `--- Startup Timings: extensions ---`, that the main group has 12
labelled rows matching pi's labels, and that the extensions group has one `<path> module import` and
one `<path> factory` row per loaded extension. A unit test can assert the label set on the collected
entries without executing a real startup.

## AGENT-028 — pi v0.84.x's typed telemetry contract has no cyrup counterpart

**Kind** upstream-drift · **Severity** *(tracker — not counted)* · **Effort** L · **Confidence** confirmed

> **Reclassified `low` → `tracker`, 2026-08-12 repair pass** (completeness critique finding 14).
> This item proposes **no work**: its Fix is "do not port speculatively — first decide whether cyrup
> models pi's harness at all", and its own body already labels itself "**scope-defining**, not loop
> debt". An item that proposes a decision rather than a change is bookkeeping, so it keeps its ID,
> its row and this body but leaves the severity tally, which is what makes the tally a backlog.
> **It is not deferred and it is not closed** — the scope decision on `packages/agent/src/harness/**`
> is genuinely outstanding, is owned by nobody (blind spot 2 below), and the moment it is answered
> "in scope" this becomes a real, large, `L` item and returns to the count. Whoever answers it must
> also settle AGENT-S02's and blind spot 2's dependence on the same call.

**cyrup** — `grep -rln "telemetry" crates/cyrup-agent/` matches only
`crates/cyrup-agent/src/agent.rs` and `crates/cyrup-agent/src/loop_fn.rs`, and in both the word
appears solely in doc comments describing `on_payload` / `on_response` (`agent.rs:719`, `:1963`,
`:1969`; `state.rs:41-45`). There is no span, schema, attribute or exporter concept anywhere in the
crate — `GenerationConfig` (`crates/cyrup-agent/src/state.rs:13-66`) exposes exactly two telemetry
hooks and nothing else.

**upstream** — `git diff --stat v0.83.0..v0.84.1 -- packages/agent` shows `src/harness/telemetry.ts`
+615 (new), `docs/telemetry-schema.md` +381 (new), `scripts/generate-telemetry-docs.ts` +117 (new),
`test/harness/telemetry.test.ts` +188 (new), and `package.json` picking up
`@earendil-works/pi-telemetry` (extracted by `6b461b75b`, typed contracts by `04d6447f7`).
`pi/packages/agent/src/index.ts` at v0.84.1 re-exports the whole surface:
`AGENT_TELEMETRY_SCHEMAS`, `AI_TELEMETRY_SCHEMA`, `HARNESS_TELEMETRY_SCHEMA`, `startAiSpan`,
`startHarnessSpan`, plus ~35 telemetry types and `createTypedSpanStarter` /
`defineTelemetrySchema` / `InMemoryTelemetryContext` / `NOOP_TELEMETRY_CONTEXT`.

**Impact** — An observability surface pi embedders can bind to (OpenTelemetry-shaped spans over the
agent's request and harness phases, with a machine-checked attribute schema) has no cyrup
equivalent, so an operator instrumenting a fleet gets nothing above the two payload/response
callbacks. **Scope caveat, stated so a verifier can weigh it**: every producer lives under
`packages/agent/src/harness/**`, pi's agent-harness v2 — a subsystem cyrup does not model at all
(cyrup's equivalents live in `cyrup-session` / `cyrup-tools`). `agent-loop.ts` and `agent.ts` emit
no spans at v0.84.1 (a grep for `span`/`telemetry` over both files at that tag returns nothing), so
the turn loop itself is unaffected. This is filed as **scope-defining**, not as loop debt.

**Fix** — Do not port speculatively. First decide whether cyrup models pi's harness at all. If not,
the correct disposition is a scope note in `docs/gap-analysis/README.md` recording
`packages/agent/src/harness/**` (≈11k changed lines across v0.83.0..v0.84.1, including a full
session/storage/reducer rewrite) as deliberately out of scope — which **no current area file
states**. If cyrup does want the observability, the portable slice is the schema + span-starter shape
(`defineTelemetrySchema`, `createTypedSpanStarter`, `NOOP_TELEMETRY_CONTEXT`) over
`tracing`/`opentelemetry` spans wrapping the provider request in `stream_assistant` (`agent.rs:752`)
and each tool execution (`agent.rs:1242`, `:1359`), which is where pi's `AI_TELEMETRY_SCHEMA` spans
sit.

**Verify** — Scope-note disposition: assert `README.md`'s contents table or a new blind-spots section
names `packages/agent/src/harness/**` and the packages it maps to. Port disposition: assert a `NOOP`
context produces zero allocations and that an in-memory context records one `ai` span per provider
request with the attribute set `AI_TELEMETRY_SCHEMA` declares.

## AGENT-031 — `websocket_connect_timeout_ms` unreachable from the agent; the parsed setting has no consumer

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-provider/src/stream.rs:177-179` declares
`pub websocket_connect_timeout_ms: Option<u64>` citing pi `types.ts:159`, and
`crates/cyrup-provider/src/utils/simple_options.rs:84` threads it onward — but `GenerationConfig`
(`crates/cyrup-agent/src/state.rs:13-66`) has no field for it and `stream_assistant` closes its
`StreamOptions` literal with `..Default::default()` (`agent.rs:744`), so it is permanently `None`.
Separately `crates/cyrup-config/src/settings.rs:705-713` parses and validates the
`websocketConnectTimeoutMs` setting (citing pi's `getWebSocketConnectTimeoutMs`), and
`grep -rn 'websocket_connect_timeout_ms()' crates` returns **no caller** — a fully dead setting.

**upstream** — pi's session `streamFn` sets
`websocketConnectTimeoutMs: options?.websocketConnectTimeoutMs ?? settingsManager.getWebSocketConnectTimeoutMs()`
in the `modelRuntime.streamSimple` options (`pi/packages/coding-agent/src/core/sdk.ts`, identical at
both tags), and `AgentLoopConfig extends SimpleStreamOptions` (`pi/packages/agent/src/types.ts:271`)
so a low-level caller can set it too.

**Impact** — The third sibling of the AGENT-S03 / AGENT-026 surface: a declared `StreamOptions` field
with no path from the agent, plus a user-facing setting that is parsed, validated and then read by
nothing. Bounded because cyrup has no WebSocket transport at all
(`crates/cyrup-provider/src/api/openai_codex_responses.rs:39-46` says so), which is why this is low
rather than medium — but the agent-side hole is independent of that and identical in shape to
AGENT-S03.

**Fix** — Add `websocket_connect_timeout_ms` to `GenerationConfig`
(`crates/cyrup-agent/src/state.rs:13-66`) and to `AgentBuilder`, populate `StreamOptions` at
`agent.rs:721-745` instead of relying on `..Default::default()`, and have the session builder seed it
from `Settings::websocket_connect_timeout_ms()` so the parsed setting acquires a consumer. Do it in
the same change as AGENT-S03 and AGENT-026's agent-side half — one `GenerationConfig` edit covers all
three.

**Verify** — Recording `StreamFn`: set the setting, drive one turn, assert the captured
`StreamOptions.websocket_connect_timeout_ms` matches; and assert the low-level `run_agent_loop` path
carries a caller-supplied value.

## AGENT-032 — Two JS-falsy `||` fallbacks ported as `Option`-only fallbacks

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — (a) `crates/cyrup-agent/src/agent.rs:993-996`
`reason.unwrap_or_else(|| "Tool call blocked by beforeToolCall".to_string())` falls back only on
`None`, so `Some("")` becomes `Content::text("")` on the tool result at `agent.rs:1012` — an empty
text content block. Reachable through the extension seam: `crates/cyrup-ext/wit/world.wit` declares
`block(option<string>)` and `crates/cyrup-ext/src/hooks.rs:44` maps `Reduced::Blocked { reason }`
straight into `BeforeOutcome::Block { reason }` with no emptiness check, so a guest returning
`block(some(""))` gets there. (b) `agent.rs:701-705`
`.or_else(|| self.gen_config.api_key.clone())` likewise does not fall back on an empty string, so a
resolver returning `Some("")` sends an empty key where pi would use the static one.

**upstream** — (a) `pi/packages/agent/src/agent-loop.ts:639` **at v0.83.0** (v0.84.1 `:637`)
`createErrorToolResult(beforeResult.reason || "Tool execution was blocked")` — an empty-string reason
is falsy and yields the **default** text. (b) `agent-loop.ts:306` (identical at both tags)
`(config.getApiKey ? await config.getApiKey(...) : undefined) || config.apiKey` — an empty resolved
key falls through to the static one.

**Impact** — (a) An empty text content block is not merely cosmetic: Anthropic's Messages API rejects
a request containing one with a 400, so an extension returning `block(some(""))` breaks the next
provider request rather than producing pi's default message. (b) is SDK-only today — no production
`ApiKeyResolver` impl exists (only `crates/cyrup-agent/src/tests/agent_loop.rs:880` and
`crates/cyrup-sdk/tests/embedder_seams.rs:208`) — which is why the pair is low overall.

**Fix** — Port the falsiness, not just the `Option`: at `agent.rs:995` use
`reason.filter(|s| !s.is_empty()).unwrap_or_else(|| "Tool execution was blocked".to_string())`
(landing AGENT-010's text change in the same edit), and at `agent.rs:701-705` filter the resolved key
with `.filter(|k| !k.is_empty())` before the `or_else`. Sweep the loop for any other `||` port while
in there.

**Verify** — Extension/hook returning `Block { reason: Some(String::new()) }`; assert the emitted tool
result text is `"Tool execution was blocked"` and that no empty content block reaches the transcript.
Resolver returning `Some("")` with a static `api_key` set; assert the static key is sent.

## AGENT-033 — A panicking event subscriber is swallowed where pi fails the run

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `RunCtx::emit` (`crates/cyrup-agent/src/agent.rs:387-395`) wraps every
`s.on_event(&ev)` in `std::panic::AssertUnwindSafe(...).catch_unwind().await` and **discards the
result**; the comment at `:388-393` states the intent (a subscriber failure must not halt the loop).

**upstream** — `processEvents` (`pi/packages/agent/src/agent.ts:573-575` **at v0.83.0**, v0.84.1
`:588-590`) awaits each listener bare inside `runWithLifecycle`'s try (`:487-490`, v0.84.1
`:502-505`), so a throwing listener aborts the run and produces the full `handleRunFailure` quartet
with the listener's message as `errorMessage`.

**Impact** — The two runtimes disagree on what a broken observer does: pi surfaces it as a run failure
the user can see, cyrup hides it entirely. Bounded twice over — `EventSubscriber::on_event` returns
`()`, so the only way to fail is a panic, and `[profile.release] panic = "abort"` means release
builds abort the process rather than swallowing — which is why this is low rather than medium. Filed
because there is no "accepted divergence" category in this analysis: it is a deliberate-looking
mechanism difference that costs behaviour, so it stays on the list until someone signs off.

**Fix** — Decide and document. Either (a) match pi: on `Err(p)` from the `catch_unwind`, route
`panic_message(p.as_ref())` into `emit_run_failure` (`agent.rs:415-435`) so the run closes with the
listener's message as `error_message`; or (b) keep the containment and make it observable — log at
`error` with the subscriber's type name and set `state.error_message` — recording the divergence and
its reason in the doc comment at `agent.rs:388-393` rather than leaving it as an unstated policy.

**Verify** — Subscriber that panics `"observer-boom"` on `turn_end`; assert (a) the run ends with
`stop_reason: "error"` and `error_message` containing `observer-boom`, or under disposition (b) that
the panic is logged and reflected in state. Today neither happens and the run completes as if
nothing occurred.

---

## Surface-sweep findings (provenance: 2026-08-03 sweep, HEAD `9219dcd`; re-audited 2026-08-12)

Found by a **surface-driven** sweep that walked pi asking what has no cyrup counterpart at all,
rather than checking a list of known items. That inversion exists because the item-driven method
cannot see behaviour nobody wrote an item for. IDs keep the `-SNN` suffix to mark their provenance;
they are listed in the single open-items table above, not in a second table.

## AGENT-S01 — Provider attribution + opencode session-affinity headers are computed once and never recomputed

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed ·
**Status** *partially-closed 2026-08-12*

**cyrup** — **Closed for the session-level model switch.** `StateInner.headers` is live
(`crates/cyrup-agent/src/state.rs:80-85`), `Agent::set_headers` exists (`agent.rs:1503-1505`),
`stream_assistant` reads it **per turn** at `agent.rs:733`, and the session facade recomputes on both
model-change paths — `crates/cyrup-session-svc/src/session.rs:3896` (`apply_model_change`) and
`:2792` (`set_model_id`), each via `attribution_headers` (`session.rs:2734`). **Not closed for two
residual paths**, filed as their own items: `loop_fn.rs:130` hardcodes `headers: None` so the
low-level API can never send any (**AGENT-021**), and a per-turn `TurnUpdate::model` override
(`agent.rs:582-584`) retargets `self.model` (read at `agent.rs:677`) without recomputing
`state.headers` (**AGENT-029**).

**upstream** — `pi/packages/coding-agent/src/core/sdk.ts:312-328` (byte- **and** offset-identical at
v0.83.0 and v0.84.1 — re-verified in the 2026-08-12 repair pass) merges attribution inside the
`transformHeaders` callback of the `streamFn` closure, so it is
dispatched on the model of *that* request by construction. `getSessionHeaders` /
`getDefaultAttributionHeaders` (`pi/packages/coding-agent/src/core/provider-attribution.ts`) are
keyed on the model's provider id and base-url host.

**Impact** — As originally filed (an opaque `x-opencode-session` UUID following the user to an
unrelated vendor, OpenRouter attribution leaking both directions, degraded prompt-cache routing) —
now confined to the two residual paths above.

**Fix** — See AGENT-021 and AGENT-029. Nothing further is needed on the session path.

**Verify** — `crates/cyrup-agent/src/tests/model_boundary.rs:679-720`
(`set_headers_repoints_the_next_requests_header_overlay`) pins the closed half; the residual halves
have their own verification sketches in AGENT-021 / AGENT-029.

## AGENT-S02 — `Agent::subscribe` returns no detach handle and `on_event` receives no abort signal

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed

**cyrup** — `crates/cyrup-agent/src/agent.rs:1485-1487`
`pub fn subscribe(&self, s: Arc<dyn EventSubscriber>) { lock(&self.subscribers).push(s); }` —
returns `()`. `EventSubscriber::on_event` (`crates/cyrup-agent/src/subscriber.rs`) takes only
`&AgentEvent`, no cancel token.

**upstream** — `pi/packages/agent/src/agent.ts:243-246` **at v0.83.0** (v0.84.1 `:250-253`)
`subscribe(listener): () => void { this.listeners.add(listener); return () => this.listeners.delete(listener); }`,
with the signal passed at `:574` (v0.84.1 `:589`) `await listener(event, signal)`.

**Impact** — Two capabilities absent rather than degraded. (a) Nothing can detach from an agent's
event stream; an embedder attaching a per-operation observer leaks it for the process lifetime.
(b) A subscriber doing expensive work (streaming to a remote client, rendering, persisting) cannot
observe that the run it is servicing was aborted, so it runs to completion and the abort's latency
benefit is lost for exactly the listeners that make abort worth having. Both in-tree cyrup
subscribers are permanent, so this is an embedder/SDK-surface gap.

**Rationale correction (2026-08-12)** — the previous revision's `compact()` citation is stale.
v0.84.1 **deleted** `_reconnectToAgent` and the `_disconnectFromAgent()` call at the top of
`compact()`. The surviving upstream consumers of the detach handle are disposal
(`agent-session.ts:395`, `:829-831`) and the rpc-mode stdout-backpressure listener
(`modes/rpc/rpc-mode.ts:355-361`, `:732-733`), which is unsubscribed on every rebind and at
shutdown.

**Fix** — Have `subscribe` return an opaque `SubscriptionHandle` whose drop (or explicit
`unsubscribe()`) removes the entry from `self.subscribers`; add a `CancelToken` parameter to
`EventSubscriber::on_event`, defaulted in the trait so existing impls compile, and pass
`self.cancel.child()` from `RunCtx::emit` (`agent.rs:387-395`).

**Verify** — Subscribe, receive one event, unsubscribe, drive another run, assert no further events;
and a subscriber that awaits the token and records observing an abort mid-run.

## AGENT-S03 — `StreamOptions.metadata` is unreachable from the agent loop

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup-agent/src/state.rs:13-66`: `GenerationConfig` has no `metadata` field, and
`stream_assistant` builds `StreamOptions` at `agent.rs:721-745` closing with `..Default::default()`,
so `crates/cyrup-provider/src/stream.rs:188 pub metadata: Option<serde_json::Map<String, Value>>` is
never populated by either `Agent` or `loop_fn::AgentLoopConfig`. (`ProxyStreamOptions.metadata`
exists at `proxy.rs:312` and is forwarded at `proxy.rs:386`, but is always `None` for the same
reason.)

**upstream** — `pi/packages/ai/src/types.ts` declares `metadata?: Record<string, unknown>` on
`StreamOptions` ("For example, Anthropic uses `user_id` for abuse tracking and rate limiting"), and
`AgentLoopConfig extends SimpleStreamOptions` (`pi/packages/agent/src/types.ts:271`), spread into the
provider call at `agent-loop.ts:308-312`.

**Impact** — An SDK embedder cannot set Anthropic `metadata.user_id` or any other provider metadata
through either `Agent` or the low-level `loop_fn` API, where pi's low-level caller can set it by
construction. No effect on the shipped binary — no pi built-in populates it either — so this is an
API-surface hole, not a runtime bug.

**Fix** — Add `metadata: Option<serde_json::Map<String, Value>>` to `GenerationConfig` and
`AgentBuilder`, and populate `StreamOptions.metadata` at `agent.rs:721-745`. Land with AGENT-031 and
AGENT-026's agent-side half — one struct, three fields.

**Verify** — Recording `StreamFn`; set `metadata` through both `AgentBuilder` and
`AgentLoopConfig.gen_config`, drive one turn each, and assert the captured `StreamOptions.metadata`
matches.

## AGENT-S04 — The `transport` setting is wired into the agent but consumed by nothing

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** confirmed ·
**Status** *partially-closed 2026-08-12*

**cyrup** — **The agent-side wiring the item called dead is now complete**: `StateInner.transport` is
live (`crates/cyrup-agent/src/state.rs:86-95`), `Agent::set_transport` exists
(`agent.rs:1510-1512`), `start_run` overlays it into the run's `GenerationConfig` at `agent.rs:1703`
/ `:1712` (pi's run-start snapshot, `agent.ts:454`), and it reaches `StreamOptions.transport` at
`agent.rs:734`; the TUI row is wired through `crates/cyrup-tui/src/app/execute_misc.rs:229-230` →
`crates/cyrup-session-svc/src/session.rs:3654-3658`. **Still open downstream**:
`grep -rn '\.transport' crates/cyrup-provider/src/api/*.rs crates/cyrup-provider/src/stream/*.rs`
returns nothing, so no cyrup wire API acts on the value and every request resolves to SSE.

**upstream** — the only consumers are `pi/packages/ai/src/api/openai-codex-responses.ts` (`:300`,
`:307`, `:1480`) and `simple-options.ts:39`. (Correction to the previous revision:
`bedrock-converse-stream.ts`'s single `transport` hit is a comment, not a consumer.) Both are
documented-unported wire APIs in cyrup.

**Impact** — The TUI settings dialog presents a `Transport: auto/websocket/sse` choice that cannot
affect anything, and the `websockets` migration writes a key nothing consumes. It becomes a genuine
behavioural gap only when `openai-codex-responses` is ported.

**Fix** — **Reclassified to area 01 as a provider gap.** Nothing further is owed by this area; the
remaining work is consuming `StreamOptions.transport` in the wire APIs, which is scheduled with the
`openai-codex-responses` port. Deduplicate against area 01 before scheduling.

**Verify** — Area 01: a recording adapter asserting the codex-responses API selects the WebSocket path
when `transport == WebSocket` and SSE otherwise.

## Coverage

**Method and constraints.** Static analysis only. Nothing was executed; no Rust or TypeScript source
was modified; the only file written is this one. Every `closed` verdict from a prior pass was treated
as a claim to refute, and every new finding was independently re-read on both sides by a second pass
before being written here. A commit message asserting a fix was never accepted as evidence.

**Repair pass 2026-08-12 — what it covered, what it rejected, what is still blind.**

*Covered — the cross-tag citation sweep (completeness critique finding 9).* The critique caught
AGENT-020 citing `agent.ts:361-388` / `:362-364` as "identical at v0.83.0 and v0.84.1". The bytes are
identical; the offsets are not. Rather than fix that one parenthetical, **every upstream citation in
this file was re-resolved by opening both tags**, using `git show <tag>:<path>` and anchor greps, and
diffing the function bodies to confirm byte-identity. Two shifts govern the whole file:

| upstream file | v0.83.0 → v0.84.1 | affected items |
|---|---|---|
| `packages/agent/src/agent.ts` | +7 lines before `:250`, +11 by `:350`, +15 from `:487` on | AGENT-011, 018, 020, 023, 025, 033, S02, and AGENT-007's status row |
| `packages/agent/src/agent-loop.ts` | 0 through `:636`; the block arm rewritten at `:636-641` → `:636-645`; **+4 from `:642` on** | AGENT-003, 009, 010, 012, 016, 022, 032 |
| `packages/agent/src/proxy.ts` | +2 from `:169` on (the two `samplingParams` lines) | AGENT-013, 026 |
| `packages/coding-agent/src/core/agent-session.ts` | +2 before `:400`, +15 from `:520` on, +8 from `:1158` on | AGENT-017, 030, S02 |
| `packages/coding-agent/src/core/sdk.ts` | **no change — byte-identical, same offsets** | AGENT-029, S01, 031 |

Every affected citation now leads with the **v0.83.0** offset (the tag that governs classification,
per `README.md:224-225`) with the v0.84.1 offset in parentheses. **No finding changed and no severity
moved as a result** — in all twelve cases the code was byte-identical and only the addressing was
wrong. AGENT-022, AGENT-023, AGENT-024 and AGENT-026 are `upstream-drift` items whose subject matter
*is* the v0.84.1 code, so they correctly continue to lead with v0.84.1 offsets; each was re-checked
to confirm it says so explicitly.

*Covered — severity, applying `README.md:106-107` rather than inheriting the prior rating.*
**AGENT-020 raised high → critical**: its Impact is unconditional silent destruction of a user-typed
message on the normal path. The rule used, stated here so it is auditable and not ad hoc: an item is
`critical` when it meets one of the four README conditions **and** the triggering path is one a user
takes in ordinary use. Three other items were tested against that rule and **deliberately left where
they are**: AGENT-030 (data loss is race-conditional, and its unconditional branch *is* AGENT-020 —
stays `high`, ships in the same change), AGENT-016 (broken conversation, but `panic = "abort"` in
`[profile.release]` means the shipped binary aborts instead — stays `medium`), and AGENT-033 (same
profile bound, plus `on_event` returns `()` so only a panic can trigger it — stays `low`).

*Covered — the tracker reclassification (finding 14).* **AGENT-028** proposes a scope decision, not
work, and is now marked `tracker`: ID kept, body kept, excluded from the count. Every other item in
this file was re-read against that test and all 25 propose a concrete change with a named fix site.
The nearest calls were AGENT-S04 (already `partially-closed` and handed to area 01, so already
uncounted) and AGENT-S01 (likewise); neither needed a second marker.

*Rejected in this repair pass, with reason.* (a) **Raising AGENT-030 to critical** — rejected above;
recorded so it is not re-litigated. (b) **Renumbering AGENT-028 or folding it into a README scope
note** — rejected: IDs are never deleted, and the scope question is real work-shaped debt the moment
it is answered. (c) **Adjusting the `agent-loop.ts` citations by uniform shift** — rejected on
`README.md:224-225`; every line was re-resolved by reading the file at the tag, which is what
revealed that the shift is 0 before `:636` and +4 after, not uniform.

*Still blind after this pass.* The sweep verified **upstream** citations at both tags. It did **not**
re-verify every cyrup-side line number — only those in the items it touched (AGENT-020's
`agent.rs:1635-1657` / `:1646` / `:1650` / `:1659` / `:1672-1682` / `:1681`, `queue.rs:51`,
`session.rs:627` / `:700-726` / `:716` / `:722` / `:3202-3204` / `:854`, `agent.rs:1441`), all of
which resolved exactly. The Rust side of the other twenty-odd items still rests on the 2026-08-12
audit pass. The five pre-existing blind spots below are unchanged by this repair.

**Read first-hand at cyrup HEAD, in full**: `crates/cyrup-agent/src/agent.rs` (all 2011 lines, three
passes), `state.rs`, `hooks.rs`, `event.rs`, `queue.rs`, `loop_fn.rs`, `subscriber.rs`;
`crates/cyrup/src/output_guard.rs`; `crates/cyrup/src/timings.rs`. **In part**:
`crates/cyrup-agent/src/proxy.rs` (1-140, 265-330, 375-400, 440-545); `crates/cyrup-core/src/message.rs`
(155-200) and `tool.rs` (25-60); `crates/cyrup-session-svc/src/hooks.rs` (128-215) and `session.rs`
(600-745, 2730-2800, 3140-3145, 3200-3205, 3880-3900); `crates/cyrup-ext/wit/world.wit`;
`crates/cyrup-ext/src/hooks.rs`; `crates/cyrup-provider/src/stream.rs` (170-200, 400-420, 500-525);
`crates/cyrup-config/src/settings.rs` (700-720); `crates/cyrup-agent/src/tests/agent_loop.rs` (250-360),
`hook_failure_text.rs` (1-80, 241-311), `model_boundary.rs` (640-720); plus `cyrup/Cargo.toml`
profiles (`[profile.release] panic = "abort"` at `:222`; the `:62` `panic = "deny"` is an unrelated
clippy lint). Targeted greps across all 18 crates for: `sampling_params`, `metadata`, `transport`,
`set_headers`, `StopReason::Pending`, `continue_run`, `.reset()`, `websocket_connect_timeout_ms`,
`telemetry`, `PI_TIMING`, `before_provider_headers`, `transformHeaders`, and each of the four
hardcoded hook placeholders AGENT-006 named.

**Read first-hand upstream**: `pi/packages/agent/src/{agent-loop,agent,types,proxy}.ts` dumped in full
at **v0.84.1** and read end to end (796 / 592 / 443 / 369 lines);
`pi/packages/coding-agent/src/core/{output-guard,timings,event-bus,exec,messages}.ts` in full;
`agent-session.ts` at v0.84.1 in the ranges 390-440, 470-560, 590-600, 820-840, 870-880, 1060-1075,
1120-1175, 1780-1935, 1950-2200; `sdk.ts:305-345` at **both** v0.84.1 and v0.83.0;
`extensions/types.ts:1065-1100` at both tags; `agent.ts:310-345` and `:440-480` at v0.83.0.

**Version-lag sweep.** `git diff v0.83.0..v0.84.1` scoped to `packages/agent` plus the eight assigned
`coding-agent/core` files. The agent package's **source** diff outside `harness/` is **29 lines
across four files, and all of it is filed** — `agent-loop.ts` (+6, AGENT-022), `agent.ts` (+15,
AGENT-022 / AGENT-023 / AGENT-024), `proxy.ts` (+2, AGENT-026), `types.ts` (+6, AGENT-022).
`agent-session-services.ts` (+2) is `modelRuntimeSignal` threaded into `ModelRuntime.create` — an
area-01/05 lifecycle concern, handed off, not filed here.

**Surface-driven sweep (the README's counter to blind spot 1).** Symbol-driven over the four small
agent-package files: every exported type, every `AgentLoopConfig` field, every `AgentEvent` variant,
every `AgentState` member, every public `Agent` method was checked for a cyrup consumer. That sweep
produced AGENT-021, AGENT-024, AGENT-026, AGENT-031 and AGENT-033. It was **not** symbol-driven over
the eight `coding-agent/core` files: the small ones were read whole, the two large ones
diff-guided — see blind spot 5.

**Rejected this pass, with reason** *(so a future pass does not re-derive them)*:

- **Nothing was refuted outright.** All 23 re-audited items and all 10 new findings survived; four
  were severity-corrected downward (AGENT-012, AGENT-015, AGENT-023, AGENT-024) and two had their
  mechanism restated (AGENT-024 at the Agent-options layer rather than the loop seam; AGENT-S02's
  `compact()` rationale replaced with the two surviving v0.84.1 consumers).
- **`Agent::start_run` does not clear `streaming_message` where pi's `runWithLifecycle` does**
  (`agent.ts:499`) — unreachable: the `AgentEnd` reducer arm (`state.rs:167-170`) clears it on every
  termination path, including the `catch_unwind` twin at `agent.rs:1799-1804`.
- **`processEvents`'s "Agent listener invoked outside active run" throw** (`agent.ts:585-587`) — a
  JS-lifetime assertion with no Rust analogue.
- **`output-guard.ts`'s backpressure half** (`writeRawStdout`'s ENOBUFS/EAGAIN retry,
  `waitForRawStdoutBackpressure`, `flushRawStdout`) — cyrup solves the same problem at the mode layer
  with an awaited pump plus per-line flush, documented against pi's `flushRawStdout` at
  `crates/cyrup-modes/src/rpc.rs:572-573`, `:613`, `:1440-1446`.
- **`PostTurn.turn_index` and `GenerationConfig.api_key`** — additive cyrup extras with no upstream
  counterpart and no behavioural cost.
- **`loop_fn.rs` as a facade** — the known trap; not re-reported. (AGENT-021 is about a specific
  wrong value inside it, not about the facade.)
- Carried forward from the 2026-08-03 pass and honoured, not re-derived: the `a_02_7`,
  `miss4_abort_carries_streamed_partial_content`, `proxy_live_turn.rs:122` and 5s-rendezvous
  test-defect candidates. **Caveat**: that rejection list predates five commits of code change, and
  the rejections were not re-tested this pass — an item rejected as "not reachable" then may be
  reachable now.

**Handoffs to other areas** — each verified far enough to route, not audited here:

1. `agent-session.ts`'s +150 lines are mostly area 03/08: `isRecoverableLength` joining the Case-1
   compaction condition (`:1985-1995`), `normalizeToolResultImages` now running unconditionally after
   the tool_result hook (`:500-530`), the compaction-in-progress submit guard (`:1130-1136`),
   `_getRequiredRequestAuth` / `_getSummarizationRequestAuth` returning a baseUrl-overridden
   `requestModel`, `getAvailableSnapshot()` replacing async `getAvailable()` in both model-cycle
   paths, and `oldRunner.invalidate()` on reload. The first three are already in
   `PARITY-GAPS.md:249/:255/:252`.
2. `transformHeaders` + the `before_provider_headers` extension event are unported and predate the
   baseline; `crates/cyrup-ext/wit/world.wit:10` already names `before_provider_headers` as absent —
   **area 06** owns it. The loop-relevant consequence is filed here as AGENT-029.
3. `AgentTool.label` (pi `types.ts:388`) — **area 04**.
4. `messages.ts` is ported to `crates/cyrup-session/src/agent_message.rs` + `context.rs` — **area 03**.
5. `event-bus.ts` and `exec.ts` are ported (`crates/cyrup-ext/src/host/services.rs:978-1085` and
   `:56` / `live.rs:416-441`) — **area 06**.
6. AGENT-030's fix site is `crates/cyrup-session-svc/src/session.rs` — **area 08** — but it is filed
   here because the behaviour under test is the run lifecycle and it must ship with AGENT-020.
7. AGENT-017's fix site is also area 08; AGENT-026's primary half and AGENT-S04's residual are
   **area 01**. Deduplicate all three before scheduling.

**Blind spots — what the next pass should attack first:**

1. **Nothing was executed.** Every verdict is from reading Rust at `a9000b1` and TypeScript at
   v0.83.0 / v0.84.1. AGENT-020's *in-tree* reachability in particular is a code-path argument (the
   `is_streaming` gate reads a flag `SettlementGuard::drop` has already cleared) and has not been
   reproduced; the API-level loss is unconditional and does not depend on that argument.
   AGENT-030 is the same argument stated as its own item.
2. **`pi/packages/agent/src/harness/**` was NOT audited** — the single largest change in this
   upstream, ~11,400 insertions / ~10,900 deletions across v0.83.0..v0.84.1, including a full rewrite
   of `agent-harness.ts`, a new 667-line `reducer.ts`, a new `session/` subtree (jsonl codec / repo /
   storage / state / types, memory, search, a 993-line conformance suite) and the telemetry layer.
   Only `index.ts`'s export list and the telemetry file names were sampled. **No area file declares
   the harness in or out of scope, so it is owned by nobody**; AGENT-028 exists partly to force that
   decision. If cyrup is meant to model it, the real gap in this area is an order of magnitude larger
   than what is filed.
3. **`pi/packages/agent/test/*.test.ts` was not read**, and it grew by +118 (`agent-loop`) and +78
   (`agent`) lines in this window. Those tests would independently pin the expected event sequences
   for AGENT-015, AGENT-022 and AGENT-025 — the claims here about `agent_end.messages` payloads and
   batch-termination semantics rest on reading the emitters, not on upstream's own oracle.
4. **`agent-session.ts` is 3344 lines and ~400 were read**, chosen by following the v0.83.0..v0.84.1
   diff hunks. Behaviour that did not change upstream and that no item covers is invisible to this
   pass — the README's structural blind spot 1, applied to the largest file in scope.
5. **`agent-session-runtime.ts` (441 lines) was only skimmed for its export list** —
   `AgentSessionRuntime`, `setRebindSession`, `setBeforeSessionInvalidate`,
   `SessionImportFileNotFoundError`, `createAgentSessionRuntime` — and what in cyrup consumes any of
   them was not checked.
6. **AGENT-026's provider half** (`StreamOptions.samplingParams` merged over `Model.samplingParams`
   in the OpenAI-compatible adapters) is asserted from upstream's type docs, not from reading the
   adapter bodies. Area 01 should confirm the merge order and which adapters apply it.
7. **The `spec/` tree is absent**, so where cyrup cites a requirement to justify a divergence it
   cannot be adjudicated. This bears on exactly one item — AGENT-018's first half (R-02-040) — kept
   at low with the uncertainty stated inline.
