# 08 — cyrup-session-svc + cyrup-modes + bin + sdk

This area covers the single integration seam (`cyrup/crates/cyrup-session-svc/`), the non-interactive front-ends (`cyrup/crates/cyrup-modes/` — print, json, rpc), the binary (`cyrup/crates/cyrup/`) and the embedder SDK (`cyrup/crates/cyrup-sdk/`), measured against `pi/packages/coding-agent/src/core/agent-session*.ts`, `.../modes/{rpc,print-mode}`, `.../main.ts` and `.../cli/args.ts` at pi v0.83.0. Headline finding: the extension seam and the session lifecycle events are now genuinely live — six items close outright and three close in half — but the RPC command loop still carries two critical defects (an `in_flight` latch that hangs `run_rpc` at stdin EOF, and no rebind when an extension control op replaces the session), `abort()` still diverges from pi in two ways that teardown now depends on, and `get_session_stats` turns out to be an entirely cyrup-invented payload. Re-baselined against HEAD `1806375` on 2026-08-03 by reading only; no build, test or network was run.

## Status since the c8bd2ab baseline

| ID | Status | Note |
|---|---|---|
| SEAM-001 | partially closed | Runtime + print/json hosts announce `session_start{reason:"startup"}` (`session.rs:2197-2199`, latched `:2207`); commit 5e1cb89. SDK path still never announces — item kept OPEN on that residual, fix tracked in SEAM-026. |
| SEAM-002 | partially closed | `dispose` on every host teardown path emits `SessionShutdown` (`session.rs:2165-2176`); commit 694af90. Two residuals keep it open: no SDK teardown (SEAM-026) and sync-`abort()` ordering (SEAM-024). |
| SEAM-003 | **closed** | `apply_pending_control` (`session.rs:2485-2569`) is a real sink; `RuntimeHostActions` installed before `bind_extensions` (`runtime.rs:227-230`), every error routed through `report_control_failure`. Commit 1d87913. |
| SEAM-004 | **closed** | All three model verbs read `available_model_catalog` (`rpc.rs:913/952/1232`); zero bare `model_catalog()` remain. Commit 694af90. |
| SEAM-005 | **closed** | `agent_settled` emitted on both bound (`session.rs:679-692`) and unbound (`subscriber.rs:222-230`) paths, extensions before subscribers, `end_run` last. Commit 1d87913. |
| SEAM-006 | still open | Unchanged, and the consequence grew: control ops are live but print/json has no runtime host. |
| SEAM-007 | **closed** | `compact()` returns `Result<CompactionResult, _>`; refusals surface as RPC errors (`rpc.rs:990-1000`). Commit 694af90. Payload gap split out as SEAM-034. |
| SEAM-008 | still open | SIGHUP still unhandled; exit codes still 0/1/130 only. Cheaper now that `dispose` is wired. |
| SEAM-009 | still open | Unchanged. Upstream line refs refreshed (`:290`, `:337`). |
| SEAM-010 | **closed** | `Max` in `ThinkingArg` (`cli.rs:50`) and the pre-clap leniency pass is real (`diagnostics.rs:51-53`, `:109-124`). Commit 6d29542. Doc residual is SEAM-029. |
| SEAM-011 | still open | Unchanged; every OTHER `extension_ui_request` member re-verified as matching pi. |
| SEAM-012 | still open | Unchanged. Batch with SEAM-025 (one WIT bump, both copies). |
| SEAM-013 | **closed** | `ControlOp::Shutdown` → `shutdown_requested` (`session.rs:2681-2684`) sampled every loop iteration (`rpc.rs:676`); both pi checkpoints present. Commit 1d87913. |
| SEAM-014 | still open | Verb-set diff mechanically re-derived: cyrup implements 31 of pi's 32; `get_available_thinking_levels` is the exact difference. |
| SEAM-015 | partially closed | RPC bash now goes through `execute_bash_with_user_event` (`rpc.rs:1045-1051`); commit 289c089. The `operations` backend override is still unported (literal `None` at `rpc.rs:1050`) — item stays open on that. |
| SEAM-016 | still open | Unchanged; cyrup site corrected to `print.rs:75`. |
| SEAM-017 | still open | Zero `RpcClient` hits workspace-wide. |
| SEAM-018 | still open | Expected version lag (upstream added 2026-07-27). |
| SEAM-019 | still open | `--ui-mode`/`--alt` still absorbed silently. |
| SEAM-020 | still open | 289c089 changed which models are listed, not the ordering vs runtime construction. |
| SEAM-021 | still open | Line-exact at HEAD; **critical**. |
| SEAM-022 | still open | **Critical**, and worse since 1d87913 made control ops able to replace the session. |
| SEAM-023 | still open | `abort()` is still a one-line body omitting `abort_retry()`. |
| SEAM-024 | still open | Exposure grew: `dispose` is now on every teardown path and every replacement. |
| SEAM-025 | still open | Unchanged. |
| SEAM-026 | still open | Unchanged; the surviving residual of both SEAM-001 and SEAM-002. |
| SEAM-027 | still open | Unchanged; `json.rs:23` doc is now also stale after SEAM-005. |
| SEAM-028 | still open | Unchanged. |
| SEAM-029 | still open | Unchanged; doc-only. |
| SEAM-030 | still open | Correctly three instances; instance (b) downgraded to a smell inside the item. |
| SEAM-031 | **new** | `get_session_stats` payload is cyrup-invented and post-compaction. |
| SEAM-032 | **new** | Test pins SEAM-031's invented `messageCount` field. |
| SEAM-033 | **new** | Initial `session_start` precedes `--name`/`--models` application (interactive + RPC). |
| SEAM-034 | **new** | `CompactionResult` drops pi's `usage`. |

## Open items

> **⚠ THIS TABLE IS NOT THE COMPLETE OPEN SET.** 5 further items from the 2026-08-03
> surface-driven sweep live in their own table under `## Surface-sweep findings` (line ~484), with
> `-S` ids — **including 1 rated critical/high**. Enumerating only this table undercounts the
> area by 5 items, which is exactly how `SEAM-S01` (high) escaped a full audit pass on
> 2026-08-07. Count BOTH tables. See structural defect A in `00-residual-ledger.md`.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| SEAM-021 | closed | closed by `4935cc8` (Move 18/18b) — was critical; re-audit against that commit |
| SEAM-022 | closed | closed by `4935cc8` (Move 18/18b) — was critical; re-audit against that commit |
| SEAM-001 | closed | closed by `4935cc8` (Move 18/18b) — was high; re-audit against that commit |
| SEAM-002 | closed | closed by `4935cc8` (Move 18/18b) — was high; re-audit against that commit |
| SEAM-006 | closed | closed by `4935cc8` (Move 18/18b) — was high; re-audit against that commit |
| SEAM-009 | closed | closed by `4935cc8` (Move 18/18b) — was high; re-audit against that commit |
| SEAM-023 | closed | closed by `4935cc8` (Move 18/18b) — was high; re-audit against that commit |
| SEAM-024 | closed | closed by `4935cc8` (Move 18/18b) — was high; re-audit against that commit |
| SEAM-026 | closed | closed by `4935cc8` (Move 18/18b) — was high; re-audit against that commit |
| SEAM-031 | closed | closed by `4935cc8` (Move 18/18b) — was high; re-audit against that commit |
| SEAM-008 | medium | not-ported | S | SIGHUP ignored; no 143/129 signal exit codes |
| SEAM-011 | medium | parity-bug | M | setWidget goes on the wire with a cyrup-invented {widget} blob |
| SEAM-012 | medium | not-ported | M | session_before_switch carries no reason, session_before_fork no position |
| SEAM-014 | medium | not-ported | S | RPC verb get_available_thinking_levels not implemented |
| SEAM-015 | medium | not-ported | M | RPC bash ignores the operations backend override |
| SEAM-016 | medium | parity-bug | S | print-mode exit code derived by reverse-scanning the transcript |
| SEAM-025 | medium | not-ported | M | session_start/session_shutdown drop pi's session-file fields |
| SEAM-027 | medium | parity-bug | M | --mode json subscribes per-run, dropping between-prompt events |
| SEAM-033 | medium | parity-bug | M | Initial session_start emitted before --name and --models are applied |
| SEAM-017 | low | not-ported | M | No RpcClient counterpart |
| SEAM-018 | low | not-ported | M | cyrup auth print-api-key / print-bearer-token missing |
| SEAM-019 | low | not-ported | S | --ui-mode / --alt absent and silently swallowed |
| SEAM-020 | low | parity-bug | M | --help and --list-models handled before the session exists |
| SEAM-028 | low | test-defect | S | modes.rs setWidget case pins SEAM-011's invented wire field |
| SEAM-029 | low | stale-port | S | ThinkingArg doc comment claims the leniency path is unreachable |
| SEAM-030 | low | test-defect | S | RPC tests assert wall-clock/scheduling outcomes they cannot control |
| SEAM-032 | low | test-defect | S | rpc_extended_command_surface pins the invented messageCount stats field |
| SEAM-034 | low | parity-bug | S | CompactionResult drops pi's usage field |

## SEAM-021 — RPC steer/follow_up latch in_flight, hanging run_rpc at stdin EOF

**Kind** parity-bug · **Severity** critical · **Effort** S · **Confidence** high (traced end-to-end; hang not reproduced — no execution this round)

**cyrup** — `cyrup/crates/cyrup-modes/src/rpc.rs:863` (`Steer` arm `:861-868`) and `rpc.rs:871` (`FollowUp` arm `:869-876`) set `*in_flight = true;` before the call and regardless of outcome, unlike the `Prompt` arm (`rpc.rs:847-859`), which sets it at `:854` only inside `Ok(accepted)` and only when `!matches!(accepted, PromptAccepted::Handled)`. `in_flight` is declared at `rpc.rs:532` and cleared in exactly two places: `rpc.rs:656` on `AgentSessionEvent::AgentSettled` and `rpc.rs:615` in the rebind block. Neither call can start a run: `session.rs:1140-1151` (`steer`) and `:1156-1171` (`follow_up`) push onto the facade mirror, delegate to the agent, emit `queue_update` and return `Ok(PromptAccepted::Queued(..))` — no `spawn_run`. Both can also return `Err` from `throw_if_extension_command` (`session.rs:1175-1183`) with the flag already latched.

**upstream** — `pi/packages/coding-agent/src/modes/rpc/rpc-mode.ts:417-425` has no in-flight gate at all for steer/follow-up, and `process.stdin.on("end", onInputEnd)` shuts the host down unconditionally.

**Impact** — After stdin EOF the loop's only exit (`rpc.rs:696`, inside the `!reader_open && !in_flight && dispatches.is_empty()` block at `:683`) is unreachable: the `cmd_rx` arm is disabled, `dispatches` is empty, and `events.next()` pends on a live persistent stream whose senders the loop still holds. `run_rpc` blocks forever, `runtime.dispose()` at `cyrup/crates/cyrup/src/run.rs:111` never runs, no `session_shutdown` is emitted, and the process lingers. Any client that steers and then closes stdin leaks a cyrup process.

**Fix** — In `rpc.rs:861-876` mirror the `Prompt` arm: set `*in_flight` only on `Ok(accepted)` and only when the accepted variant actually starts a run (`!matches!(accepted, PromptAccepted::Handled | PromptAccepted::Queued(..))`). Cheapest safe form is to hoist the `Prompt` arm's latch logic into a helper all three arms call.

**Verify** — An RPC test that sends `{"type":"steer",…}`, closes stdin, and asserts `run_rpc` returns inside a bounded `tokio::time::timeout`; today it never returns. Repeat for `follow_up` and for the `Err` path through `throw_if_extension_command`.

## SEAM-022 — RPC host does not rebind when an extension control op replaces the session

**Kind** parity-bug · **Severity** critical · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-modes/src/rpc.rs:800-811` derives `rebind` purely from the command NAME: `response.success && matches!(response.command.as_str(), "new_session" | "switch_session" | "fork" | "clone") && …cancelled != Some(true)`. Since 1d87913 a guest or native slash command can replace the session through `ControlOp::NewSession/Switch/Fork/Reload` (`cyrup/crates/cyrup-session-svc/src/session.rs:2546-2559` → `RuntimeHostActions`, `runtime.rs:140-178`). Such a command arrives as `{"type":"prompt","message":"/mycmd"}` and is routed INLINE — `is_inline_command` (`rpc.rs:713-720`) matches only `prompt|steer|follow_up` — and the inline arm's comment at `rpc.rs:576-578` still asserts "They never replace the active session, so no rebind handling is needed here", which 1d87913 made false. Meanwhile `install_inner` (`runtime.rs:284-325`) has already disposed the old session at `:298`, called `notify_replaced` at `:300` → `Fanout::invalidate` (`subscriber.rs:89`) and swapped `g.session` at `:311-316`. The `maybe_ev = events.next()` arm (`rpc.rs:646`) carries no `, if` guard, and `rpc.rs:666-667` merely SKIPS writing `SessionReplaced`. `watch_generation` exists (`runtime.rs:259-261`) and is consumed by `cyrup-tui` and `cyrup-session-svc/tests/integration.rs` — never by `rpc.rs`.

**upstream** — `pi/packages/coding-agent/src/core/agent-session-runtime.ts:117-119` (`setRebindSession`) + `:187-190` (`finishSessionReplacement`), which every replacement path calls (`:258`, `:306`, `:329`, `:349`, `:393`); registered by `pi/packages/coding-agent/src/modes/rpc/rpc-mode.ts:312-314` with `rebindSession` at `:316-360`.

**Impact** — After an extension-driven replacement every subsequent RPC command executes against the disposed session (`handle` takes `session: &AgentSession`, `rpc.rs:836-841`) while `runtime.session()` holds the live one. The ui sink, ui-effect sink and error listener stay bound to the dead session's `LiveHostServices`, so extension UI and errors vanish and prompts land in a session nobody reads.

**Fix** — Replace the name-based `rebind` predicate at `rpc.rs:800-811` with an observation of `runtime.watch_generation()`: add a `generation.changed()` select arm (or check it after every dispatch, inline arms included) and run the existing rebind block at `rpc.rs:605-620` whenever the generation advances. Delete the false comment at `rpc.rs:576-578`.

**Verify** — Register a native extension whose slash command calls `ctx.new_session()`; drive it over RPC as `{"type":"prompt","message":"/newsess"}`, then send `get_state` and assert the returned session identity/message count is the NEW session's, and that an `extension_ui_request` from the new session still reaches stdout.

## SEAM-001 — Initial session_start never emitted (SDK residual)

**Kind** not-ported · **Severity** high · **Effort** S · **Confidence** high

**cyrup** — Closed for both binary host paths: `bind_extensions` (`cyrup/crates/cyrup-session-svc/src/session.rs:2197-2199`) calls `emit_session_start("startup", None)`, latched by `self.start_announced.swap(true, Ordering::SeqCst)` at `session.rs:2207`, dispatching `HostEvent::SessionStart` at `session.rs:2219`; non-test callers are `runtime.rs:230` (inside `AgentSessionRuntime::create`, `runtime.rs:210-232`) and `cyrup/crates/cyrup/src/run.rs:81` (`announce_session_start`, called at `run.rs:27` and `:53`). Idempotence pinned by `cyrup/crates/cyrup-session-svc/tests/session_start_lifecycle.rs:134-153` and `:157-176`. **Residual**: `cyrup/crates/cyrup-sdk/src/client.rs:221-231` (`build_session`) is `SessionBuilder::new` → customizers → `Ok(Session::new(builder.build().await?))`, and `build_session_auto` (`client.rs:252-262`) delegates to it; grep for `bind_extensions` under `crates/cyrup-sdk` is empty.

**upstream** — `pi/packages/coding-agent/src/core/agent-session.ts:389` emits `session_start` from `bindExtensions`, which every pi host calls.

**Impact** — An embedder using the documented one-call SDK path gets extensions that never receive `session_start`, so anything initialising on that hook (audit loggers, intercom identity registration, permission policy load) silently no-ops.

**Fix** — Land SEAM-026; that is the whole remaining surface.

**Verify** — SDK-level test registering a recording native extension via `customize`, asserting a `SessionStart` was observed after `build_session().await`.

## SEAM-002 — session_shutdown never emitted on teardown (residuals)

**Kind** not-ported · **Severity** high · **Effort** S · **Confidence** high

**cyrup** — Closed for the binary hosts: `AgentSession::dispose` (`cyrup/crates/cyrup-session-svc/src/session.rs:2165-2176`) does `self.abort()` (`:2166`), facade `SessionShutdown` (`:2167`), `dispatch_notify(&HostEvent::SessionShutdown{..})` (`:2170-2174`), `self.session_cancel.cancel()` (`:2175`); non-test callers are `cyrup/crates/cyrup/src/main.rs:474`, `run.rs:92` (from `run.rs:36` and `:63`), `run.rs:111`, `runtime.rs:298` and `runtime.rs:529`. **Residual (a)**: no SDK teardown at all — `grep -n 'pub async fn close|pub fn close|dispose|impl Drop' crates/cyrup-sdk/src/*.rs` returns only doc-comment mentions at `lib.rs:41` and `lib.rs:87`. **Residual (b)**: `dispose` calls the SYNC `abort()` where pi awaits it first.

**upstream** — `pi/packages/coding-agent/src/core/agent-session-runtime.ts:398-404` (`dispose` emitting `session_shutdown{reason:"quit"}`) and `:167-177` (`teardownCurrent`, whose comment says the await-abort-first ordering is load-bearing).

**Impact** — SDK embedders leak sessions with no shutdown hook; the ordering half means an aborted turn's tool results may not be persisted before shutdown is announced.

**Fix** — (a) is SEAM-026, (b) is SEAM-024. Close this item when both land.

**Verify** — Recording-extension test asserting `SessionShutdown` after `Session::close()`, plus SEAM-024's settlement assertion.

## SEAM-006 — print/json mode runs on a bare AgentSession, not the AgentSessionRuntime host

**Kind** not-ported · **Severity** high · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup/src/main.rs:554-628` is the `AppMode::Print | AppMode::Json` arm: `SessionBuilder::new(provider, config)` at `main.rs:558`, `builder.build().await` at `main.rs:607`, `.into_shared()` — no `AgentSessionRuntime` anywhere (contrast the Rpc arm's `AgentSessionRuntime::create` at `main.rs:534` and the interactive one at `main.rs:440`). `run_print_dispatch`/`run_json_dispatch` take only `session: &AgentSession` (`cyrup/crates/cyrup/src/run.rs:20-24`, `:47-51`). Outside tests `add_error_listener` is called only from the two RPC sites `cyrup/crates/cyrup-modes/src/rpc.rs:523` and `:614`.

**upstream** — `pi/packages/coding-agent/src/modes/print-mode.ts:71-100` `rebindSession` binds three keys: `{mode: mode === "json" ? "json" : "print", commandContextActions{waitForIdle,newSession,fork,navigateTree,switchSession,reload}, onError}`. It does NOT bind `uiContext` — that key belongs to the RPC host only (`rpc-mode.ts:317`); a previous edition of this document said otherwise.

**Impact** — Bigger since 1d87913 made control ops live: a guest `ctx.newSession()/fork/switchSession/reload` under `--mode print` reaches `session.rs:2546-2559`, finds no `runtime_actions`, and is reported only through `report_control_failure` (`session.rs:2638`). No `onError` sink and no mode label reach extensions. `main.rs:554-628` is what a spawned subagent child re-execs into, so subagent runs inherit all of this.

**Fix** — Build print/json on `AgentSessionRuntime::create` like the Rpc arm (`main.rs:534`), pass the runtime into `run_print_dispatch`/`run_json_dispatch` (`run.rs:20-24`, `:47-51`), and install an error listener plus a mode label at bind time. Overlaps SEAM-027 — doing both lets print and json share one host with one session-wide subscription.

**Verify** — A native extension calling `ctx.new_session()` under `--mode print` succeeds and subsequent output comes from the new session; an extension error reaches the print-mode `onError` path.

## SEAM-009 — fork/clone on a non-persisted session discards the transcript

**Kind** parity-bug · **Severity** high · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session-svc/src/runtime.rs:396-444`. The `match session_file` is at `runtime.rs:416`; the `Some(file)` arm opens a throwaway manager at `:418`, calls `crate::session::fork_anchor(&mgr, &entry, position)` at `:419-420` and `mgr.create_branched_session(&leaf, &layout)` at `:431`. The in-memory arm at `runtime.rs:440` is `None => (self.factory.build(SessionTarget::New, None).await?.into_shared(), None)` — no anchor validation, no branch, `selected_text` hard-coded `None`. Because `fork_anchor` lives INSIDE the persisted arm, an invalid entry id on an unsaved session is silently accepted.

**upstream** — `pi/packages/coding-agent/src/core/agent-session-runtime.ts:262-350`: `getEntry(entryId)` + `throw new Error("Invalid entry ID for forking")` at `:275-276` and `:282-283`, i.e. BEFORE the `isPersisted()` split at `:290`; the non-persisted branch at `:333-350` reuses the LIVE `this.session.sessionManager` (`:333`) and calls `sessionManager.createBranchedSession(targetLeafId)` at `:337`.

**Impact** — Forking or cloning an unsaved session silently yields an empty session instead of a branch of the current transcript, and a bogus entry id reports success rather than an error. Data loss with no diagnostic.

**Fix** — Hoist `fork_anchor` above the `match session_file` at `runtime.rs:416` so validation runs on both paths, and give the `None` arm a live-manager `create_branched_session` + `build_from_manager` path modelled on `runtime.rs:431-432`.

**Verify** — Runtime test that prompts an in-memory session twice, forks at the first entry and asserts the child transcript contains the first exchange; a second asserting a bogus entry id errors on the in-memory path.

## SEAM-023 — AgentSession::abort() omits abort_retry()

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session-svc/src/session.rs:1242-1244` — `pub fn abort(&self) { self.agent.abort(); }` is the entire body. The retry backoff sleeps on a SEPARATE token: `session.rs:3544` stores `self.session_cancel.child_token()` into `retry_cancel` (field declared `:357`, initialised `:468`, cleared `:3549`) and sleeps under it; `agent.abort()` cancels the per-run token, not this one. `abort_retry` is at `session.rs:3480-3484`, and outside tests its only callers are `cyrup/crates/cyrup-session-svc/src/command.rs:134` and `cyrup/crates/cyrup-modes/src/rpc.rs:1012` — the explicit `abort_retry` verb only.

**upstream** — `pi/packages/coding-agent/src/core/agent-session.ts:1542-1546`: `async abort() { this.abortRetry(); this.agent.abort(); await this.waitForIdle(); }`; `abortRetry` at `:2731`.

**Impact** — Escape in the TUI, the RPC `abort` verb, or SIGINT during provider-retry backoff does not stop the backoff; the retry fires later against an "aborted" session, producing surprise output and delaying shutdown.

**Fix** — One line: call `self.abort_retry()` first in `session.rs:1242-1244`. Lands naturally with SEAM-024.

**Verify** — Faux provider forced into retry backoff; call `abort()` and assert no retry-attempt event follows and `wait_for_idle()` completes promptly.

## SEAM-024 — AgentSession::abort() does not await idle

**Kind** parity-bug · **Severity** high · **Effort** M · **Confidence** high

**cyrup** — `session.rs:1242-1244` is synchronous fire-and-forget; `wait_for_idle` exists and is async at `session.rs:1128-1136` (awaits the post-run driver watch then `self.agent.wait_for_idle()`). Three consumers assume settlement: `dispose` (`session.rs:2165-2176`) calls `self.abort()` at `:2166` then immediately emits `SessionShutdown` (`:2167`), dispatches `HostEvent::SessionShutdown` (`:2170-2174`) and cancels `session_cancel` (`:2175`); `compact` (`session.rs:1258`) aborts at `:1263` then installs the compaction cancel token at `:1265`; the RPC `abort` verb (`rpc.rs:877-888`) replies success immediately. `dispose` is now on EVERY teardown path (`run.rs:36/63/92/111`, `main.rs:474`) and every replacement (`runtime.rs:298`), so exposure grew with 694af90.

**upstream** — `pi/packages/coding-agent/src/core/agent-session-runtime.ts:167-177` `teardownCurrent` opens with "Settle any active response first so the aborted turn (including tool results) is persisted to the outgoing session before it is replaced" and does `await this.session.abort()` at `:169` BEFORE `emitSessionShutdownEvent` at `:170-174` and `this.session.dispose()` at `:176`.

**Impact** — Quitting or switching sessions mid-turn can drop the aborted turn's tool results from the outgoing session file, and `session_shutdown` is announced while the run is still writing. The RPC `abort` reply lies about completion, so a client that immediately prompts races the dying run.

**Fix** — Add `async fn abort_and_settle(&self)` = `abort_retry(); agent.abort(); wait_for_idle().await;` with a bounded timeout so a wedged run cannot hang teardown; call it from `dispose` (`session.rs:2166`), `compact` (`session.rs:1263`) and the RPC `abort` arm (`rpc.rs:877-888`). Keep the sync `abort()` for the signal handler (`signals.rs:48`).

**Verify** — Prompt a faux session with a slow tool, call `dispose` mid-turn, assert the persisted session file contains the tool result and that `SessionShutdown` is dispatched after the last run event.

## SEAM-026 — cyrup-sdk never binds extensions and has no close()

**Kind** not-ported · **Severity** high · **Effort** M · **Confidence** high

**cyrup** — `CyrupBuilder::build_session` (`cyrup/crates/cyrup-sdk/src/client.rs:221-231`) builds a `SessionBuilder`, applies customizers and returns `Ok(Session::new(builder.build().await?))`; `build_session_auto` (`client.rs:252-262`) delegates at `:261`. Neither calls `bind_extensions()`. `grep -n 'pub async fn close|pub fn close|dispose|impl Drop' crates/cyrup-sdk/src/*.rs` returns only two doc-comment mentions (`lib.rs:41`, `lib.rs:87`) — no teardown method exists. Extensions ARE reachable from this path: `CyrupBuilder::customize` hands out the `SessionBuilder`, which exposes `with_native_extension`. `AgentSessionRuntime` is re-exported from `cyrup-sdk/src/lib.rs`, but the documented one-call paths (doc examples at `client.rs:210-220` and `:241-251`) bypass it.

**upstream** — `pi/packages/coding-agent/src/core/sdk.ts` likewise does not bind, but re-exports `agent-session-runtime.ts` and every first-party consumer goes through `AgentSessionRuntime`, whose `dispose()` (`agent-session-runtime.ts:398-404`) emits `session_shutdown{reason:"quit"}`.

**Impact** — Embedders get sessions where extensions never see `session_start` or `session_shutdown`, guest control ops have no `RuntimeActions` sink (so `newSession`/`fork`/`switch`/`reload` fail with `NoRuntimeHost`), and there is no teardown call at all.

**Fix** — Either have `build_session` call `bind_extensions()` and add `Session::close(self)` → `inner.dispose("quit")`, or construct an `AgentSessionRuntime` inside the builder (which also supplies the runtime actions sink). Prefer the latter — it closes SEAM-001's and SEAM-002's residuals together.

**Verify** — SDK test with a recording native extension asserting `SessionStart` after build and `SessionShutdown` after `close()`, plus a guest `new_session` control op succeeding.

## SEAM-031 — RPC get_session_stats returns a cyrup-invented, post-compaction object

**Kind** parity-bug · **Severity** high · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session-svc/src/state.rs:9-24`: `#[serde(rename_all = "camelCase")] pub struct SessionStats { message_count (:13), user_message_count (:14), assistant_message_count (:15), tool_result_count (:16), input_tokens (:18), output_tokens (:20), cache_tokens (:23) }` — the wire object is `{messageCount, userMessageCount, assistantMessageCount, toolResultCount, inputTokens, outputTokens, cacheTokens}`. Computed by `SessionStats::from_messages` (`state.rs:26-54`) from `AgentSession::session_stats` (`cyrup/crates/cyrup-session-svc/src/session.rs:3062-3064`), whose input is `self.messages()` = `manager.build_context().messages` (`session.rs:3011-3013`) — the LLM-flattened current-branch, POST-compaction context. Emitted verbatim by the RPC handler at `cyrup/crates/cyrup-modes/src/rpc.rs:1066-1069`.

**upstream** — `pi/packages/coding-agent/src/core/agent-session.ts:259-277`: `interface SessionStats { sessionFile; sessionId; userMessages; assistantMessages; toolCalls; toolResults; totalMessages; tokens: {input, output, cacheRead, cacheWrite, total}; cost; contextUsage? }`. `agent-session.ts:3107-3161` `getSessionStats()` carries the docstring "Aggregates over ALL session entries (including history that was compacted away), so token/cost totals reflect what was actually billed across the session", iterates `this.sessionManager.getEntries()`, adds `entry.usage` for `branch_summary`/`compaction` entries, counts `toolCalls` off assistant content blocks, and returns `cost: usageTotals.cost` + `contextUsage: this.getContextUsage()`. Wire contract `pi/packages/coding-agent/src/modes/rpc/rpc-types.ts:183`.

**Impact** — A client written against pi's schema reads `undefined` for every single field; there is not one overlapping name. `sessionId`, `sessionFile`, `toolCalls`, `cost` and `contextUsage` are absent outright, `cacheRead`/`cacheWrite` are collapsed into one `cacheTokens`, and there is no `tokens.total`. Independently of naming the NUMBERS are wrong after any compaction: cyrup recomputes from the rebuilt post-compaction context, so a long session's reported token spend silently drops at each compaction. Absent `cost` means a client cannot show spend at all.

**Fix** — Rewrite `state.rs:9-54` to pi's shape and semantics, computed from `SessionManager::entries()` rather than `build_context().messages` so `Compaction`/`BranchSummary` entry usage is included — the data is already persisted (`cyrup/crates/cyrup-session/src/entry.rs:65-71` carries `usage: Option<Usage>`). Fold `context_usage()` (`session.rs:3068`) into the struct as pi does; `cost` needs the per-model pricing already used by the provider tier. Depends on SEAM-034 for the compaction usage plumbing. Note `state.rs:33-36` shows a prior partial alignment (tool-result usage, citing pi `agent-session.ts:3129-3132`) — the shape and entry-level aggregation were never done.

**Verify** — Extend `cyrup/crates/cyrup-modes/tests/modes.rs`'s `rpc_extended_command_surface` to assert `data.sessionId`, `data.totalMessages`, `data.tokens.cacheRead` and `data.cost` exist and that no `messageCount` key remains (see SEAM-032); add a session-svc test that prompts twice, compacts, prompts again and asserts `tokens.total` is monotonically non-decreasing across the compaction — today it drops.

## SEAM-008 — SIGHUP ignored; no 143/129 signal exit codes

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup/src/signals.rs:15-36` `wait_for_signal` selects on `tokio::signal::ctrl_c()` (`:23`) and `SignalKind::terminate()` (`:20`, `:24`) only; `SignalKind::hangup()` appears nowhere in the 52-line file. `signals.rs:42-51` `spawn_abort_on_signal` does `session.abort(); cancel.cancel();` (`:48-49`) and returns `JoinHandle<()>`, so no signal identity escapes; exit codes come from `cyrup/crates/cyrup/src/run.rs:118-130` (0/1/130 only). Installed at `main.rs:546` (rpc) and `:617` (print/json).

**upstream** — `pi/packages/coding-agent/src/modes/rpc/rpc-mode.ts:368` `signals.push("SIGHUP")` and `:374` `void shutdown(signal === "SIGHUP" ? 129 : 143, signal)`; `pi/packages/coding-agent/src/modes/print-mode.ts:50` and `:57` `process.exit(signal === "SIGHUP" ? 129 : 143)`.

**Impact** — A terminal hangup does not tear cyrup down; SIGTERM'd runs report 0/1/130 rather than 143, so supervisors and CI cannot distinguish a killed run from a clean one.

**Fix** — Add `SignalKind::hangup()` to the select in `signals.rs:15-36`, return the signal identity from `wait_for_signal`, thread it through `spawn_abort_on_signal` (`signals.rs:42-51`) into the exit-code computation at `run.rs:118-130` (129 for SIGHUP, 143 for SIGTERM). `dispose` is already on every teardown path, so nothing else is needed.

**Verify** — Integration test spawning `cyrup --mode print`, sending SIGHUP and SIGTERM, asserting exit 129 and 143 respectively plus a `session_shutdown` on the way out.

## SEAM-011 — setWidget goes on the RPC wire with a cyrup-invented {widget} blob

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-modes/src/rpc.rs:395-400` emits `{"type":"extension_ui_request","id":…,"method":"setWidget","widget": widget}`; the comment at `rpc.rs:391-394` admits the collapse. Root cause is the WIT: `cyrup/crates/cyrup-ext/wit/world.wit:307` `set-widget: func(widget-json: string);`, duplicated verbatim at `cyrup/crates/cyrup-ext-sdk/wit/world.wit:307`.

**upstream** — `pi/packages/coding-agent/src/modes/rpc/rpc-types.ts:265-271` pins `method:"setWidget"; widgetKey: string; widgetLines: string[] | undefined; widgetPlacement?: "aboveEditor"|"belowEditor"` — no `widget` field exists anywhere in pi's `RpcExtensionUIRequest` union (`rpc-types.ts:250-273`).

**Impact** — An RPC client written to pi's contract cannot render extension widgets at all: no `widgetKey` to key on, no `widgetLines` to draw, no placement. This is the LAST divergent member of the union — `notify` (`rpc.rs:369-375`), `setStatus` with the omit-when-None `statusText` (`:379-390`), `setTitle` (`:401-407`) and `set_editor_text` (`:412-418`) all match field-for-field, and the three TUI-only effects correctly return `None` (`:420-422`).

**Fix** — Widen `set-widget` in both WIT copies to `func(key: string, lines: option<list<string>>, placement: option<string>)`, thread the three fields through `cyrup-ext`'s effect type, and emit pi's field names at `rpc.rs:395-400`. This is a guest ABI break, same class as f777e44's.

**Verify** — Invert `cyrup/crates/cyrup-modes/tests/modes.rs:971-976` (SEAM-028) to assert `widgetKey`/`widgetLines`/`widgetPlacement` and that no `widget` key is present.

## SEAM-012 — session_before_switch carries no reason, session_before_fork no position

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-ext/src/event.rs:332-333` declares `SessionBeforeSwitch { target_id: String }` / `SessionBeforeFork { entry_id: String }`. Both WIT copies match: `cyrup/crates/cyrup-ext/wit/world.wit:232-233` and `cyrup/crates/cyrup-ext-sdk/wit/world.wit:232-233` are `func(target-id: string) -> hook-outcome` / `func(entry-id: string) -> hook-outcome`. Emit sites: `cyrup/crates/cyrup-session-svc/src/runtime.rs:339` passes `SessionBeforeSwitch { target_id: String::new() }` for `new_session` (empty sentinel, no reason); `runtime.rs:402-403` passes only `entry_id` while the `position: ForkPosition` parameter at `runtime.rs:399` never reaches the event.

**upstream** — `pi/packages/coding-agent/src/core/extensions/types.ts:577-582` (`reason: "new"|"resume"; targetSessionFile?`) and `:584-589` (`entryId; position: "before"|"at"`).

**Impact** — A gate extension cannot distinguish "new session" from "resume" (both arrive with an empty target) and cannot tell a fork *before* an entry from a fork *at* it, so a policy permitting one and denying the other is unwritable.

**Fix** — Widen both hooks in both WIT copies and in `event.rs:332-333`, populate `reason`/`target_session_file` at `runtime.rs:339` and `position` at `runtime.rs:402-403`. Batch with SEAM-025 — all four hooks are one WIT bump.

**Verify** — Recording native extension asserting `reason == "new"` on `--new`, `"resume"` on resume, and `position` matching the requested `ForkPosition`.

## SEAM-014 — RPC verb get_available_thinking_levels not implemented

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — Mechanically proven, not taken on trust: pi's 32 `RpcCommand` type tags (`pi/packages/coding-agent/src/modes/rpc/rpc-types.ts:20-73`) diffed against cyrup's 32 `SessionCommand` variants (`cyrup/crates/cyrup-modes/src/rpc.rs:84-215`, serde `rename_all = "snake_case"` at `:83`) yields exactly two lines: pi has `get_available_thinking_levels`, cyrup has `unknown`. The verb falls to `#[serde(other)] Unknown` (`rpc.rs:210-211`) and answers `Unknown command: get_available_thinking_levels` via `rpc.rs:793-797`. The backing method exists and is unused by RPC: `cyrup/crates/cyrup-session-svc/src/session.rs:2735` `available_thinking_levels()`.

**upstream** — Handler `pi/packages/coding-agent/src/modes/rpc/rpc-mode.ts:508` (`const levels = session.getAvailableThinkingLevels();`); response shape `{levels}` at `rpc-types.ts:156-162`.

**Impact** — A client cannot enumerate which thinking levels the active model supports, so it must hard-code the list or offer levels the model will reject.

**Fix** — Add the variant to `rpc.rs:84-215` and a handler returning `{"levels": session.available_thinking_levels()}`.

**Verify** — Extend `modes.rs`'s command-surface test to assert the verb succeeds and `data.levels` is a non-empty array; re-run the 32-vs-32 set diff and expect it empty but for `unknown`.

## SEAM-015 — RPC bash ignores the operations backend override

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — Closed half: `cyrup/crates/cyrup-modes/src/rpc.rs:1045-1051` calls `session.execute_bash_with_user_event(&command, BashOptions { exclude_from_context, id: bash_id }, None)` — the same wrapper the interactive `!` path uses — with a provenance comment at `rpc.rs:1034-1043` citing pi `5d548ae9` (#7214, 2026-07-28); coverage at `cyrup/crates/cyrup-session-svc/tests/round9_l5res.rs:415-439`. **Residual**: the third argument is a literal `None` at `rpc.rs:1050`, and the omission is recorded only in the source comment at `rpc.rs:1042-1043`.

**upstream** — `pi/packages/coding-agent/src/modes/rpc/rpc-mode.ts:558-579`: `emitUserBash` first, short-circuit on `eventResult?.result`, else `executeBash` with `operations: eventResult?.operations` at `:578`.

**Impact** — An extension cannot supply a remote-exec or sandbox backend for a single RPC bash call, so sandboxing extensions are inert on the RPC path while working elsewhere.

**Fix** — Add an optional per-call backend to `BashOptions` (an `Option<Arc<dyn BashOps>>`-shaped seam), populate it from the `user_bash` event result inside `execute_bash_with_user_event`, and pass it through at `rpc.rs:1050`. This is the seam 289c089's commit message deferred.

**Verify** — Native extension returning `operations` from `user_bash`; assert the RPC bash result came from the injected backend, not the local shell.

## SEAM-016 — print-mode exit code derived by reverse-scanning the transcript

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high (the `Aborted ⇒ 130` half is unadjudicable without `spec/`)

**cyrup** — `cyrup/crates/cyrup/src/run.rs:118-130` `exit_code` iterates `session.messages().await.iter().rev()` at `:120` and returns on the FIRST `Message::Assistant`. `cyrup/crates/cyrup-modes/src/print.rs:75` independently decides output from `if let Some(Message::Assistant(assistant)) = transcript.last()` (transcript read at `print.rs:74`). On a transcript whose last message is not an assistant, print emits nothing while `exit_code` still reports a stop reason from an older message. Reachable because `flush_pending_bash_messages` (`session.rs:679`) appends `Custom` bash messages after the assistant.

**upstream** — `pi/packages/coding-agent/src/modes/print-mode.ts:130-131` reads `state.messages[state.messages.length - 1]` ONCE for both output and exit code, leaving `exitCode` at its `0` initialisation when the last message is not an assistant; `exitCode = 1` at `:137` covers BOTH error and aborted (cyrup maps aborted to 130 at `run.rs:124`).

**Impact** — `cyrup --mode print` can exit non-zero while printing nothing, or exit zero on a run whose visible output came from a stale assistant message. Scripts keying on the exit code misclassify runs that end with a bash message.

**Fix** — Compute the last message once in `run.rs:118-130` (or have `print.rs` return the decision), matching `print-mode.ts:130-131`. Decide separately whether to keep 130 for aborted; that cites arch-11 §6.6 at `run.rs:117`, a spec not present in this workspace, so ask before changing it.

**Verify** — Add the missing tests: `grep -rn exit_code crates/cyrup/tests/*.rs` returns nothing today, and `crates/cyrup/tests/dispatch.rs:156-186` covers only 0-vs-1 via the dispatch return. Add a case whose last message is a `Custom` bash message and assert exit 0 with no output.

## SEAM-025 — session_start/session_shutdown drop pi's session-file fields

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-ext/src/event.rs:307-308` declares `SessionStart { reason: String }` / `SessionShutdown { reason: String }`. The loss is visible inside one function: `emit_session_start(&self, reason, previous_session_file)` (`session.rs:2206`) puts `previous_session_file` on the FACADE event at `:2209-2213` but dispatches `HostEvent::SessionStart { reason: reason.to_string() }` at `:2219`, discarding it. `AgentSession::dispose(&self, reason: &str)` (`session.rs:2165`) has no target parameter at all, and the replacement caller at `runtime.rs:298` passes only `reason`. Both WIT copies match the loss: `cyrup/crates/cyrup-ext/wit/world.wit:226-227` and `cyrup/crates/cyrup-ext-sdk/wit/world.wit:226-227` are `func(reason: string)`.

**upstream** — `pi/packages/coding-agent/src/core/extensions/types.ts:561-568` (`previousSessionFile?` at `:567`) and `:615-621` (`targetSessionFile?` at `:620`), populated on every replacement path in `agent-session-runtime.ts` (`:167-174`, `:288-296`, `:318-330`, `:390-392`).

**Impact** — An extension observing a session replacement cannot tell WHICH session it came from or is going to, so transcript-linking, audit trails and intercom identity handoff across a switch/fork are impossible.

**Fix** — Widen both hooks in both WIT copies and `event.rs:307-308`, thread `previous_session_file` through the dispatch at `session.rs:2219`, and add a target parameter to `dispose` (`session.rs:2165`) populated at `runtime.rs:298`. Batch with SEAM-012.

**Verify** — Recording extension asserting `previousSessionFile` on a switch-induced `session_start` and `targetSessionFile` on the paired `session_shutdown`.

## SEAM-027 — --mode json subscribes per-run, dropping between-prompt events

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-modes/src/json.rs:36-41` does `let mut stream = session.prompt(input).await?;` at `:36` and drains it. That stream is RUN-scoped: `AgentSession::prompt` (`session.rs:575`) calls `self.fanout.subscribe_run()`, and `subscribe_run` (`cyrup/crates/cyrup-session-svc/src/subscriber.rs:52-56`) registers in the `run_scoped` vector that `Fanout::end_run` (`subscriber.rs:82-86`) clears after `agent_settled`. `cyrup/crates/cyrup/src/run.rs:54-61` calls `run_json` once per prompt (initial plus each `--follow-up`), so gaps between prompts are unobserved. The persistent `AgentSession::subscribe` (`session.rs:565-567`) is never used by json mode.

**upstream** — `pi/packages/coding-agent/src/modes/print-mode.ts:104` installs ONE session-wide `session.subscribe(...)` inside `rebindSession()`, called at `:119` before the initial prompt at `:122` and held across the message loop at `:126`; torn down only in `disposeRuntime()` (`:40-45`).

**Impact** — With `--follow-up`, any event emitted between runs (extension UI, `session_info_changed`, `model_changed`, background compaction progress) is silently dropped from the json stream, so a consumer sees an incomplete event log.

**Fix** — Install one `session.subscribe()` before the first prompt in `run.rs:54-61` and have `run_json` drain the persistent stream, terminating on `agent_settled` per prompt. Overlaps SEAM-006 — doing both lets print/json share one host. Also fix the stale doc at `json.rs:23`, which still says the run stream terminates after `agent_end`; since SEAM-005 the terminator is `agent_settled` (`session.rs:684-688`).

**Verify** — A json-mode test with two `--follow-up` prompts and an extension emitting between them; assert the emitted event appears in the stream.

## SEAM-033 — Initial session_start is emitted before --name and --models are applied

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `AgentSessionRuntime::create` announces the session INSIDE itself: `cyrup/crates/cyrup-session-svc/src/runtime.rs:227-229` installs the actions sink then `session.bind_extensions().await` at `runtime.rs:230` → `emit_session_start("startup", None)` (`session.rs:2197-2199`). Both binary hosts that use the runtime call `apply_post_build` only AFTER create returns: RPC at `cyrup/crates/cyrup/src/main.rs:534` then `:544`, interactive at `main.rs:440` then `:444`. `apply_post_build` (`main.rs:646-686`) does `session.set_session_name(name)` at `:648` — which emits `SessionInfoChanged` (`session.rs:2000-2011`) — and, for a fresh session with `--models`, `set_model_resolved` at `:672`, `set_thinking_level` at `:678` and `set_scoped_models` at `:683`. print/json is correctly ordered: `announce_session_start` runs inside `run_print_dispatch`/`run_json_dispatch` (`run.rs:27`, `:53`), i.e. after `apply_post_build` at `main.rs:615`. A guest really can read the name at this point: `get-session-name: func() -> option<string>` is a host import at `cyrup/crates/cyrup-ext/wit/world.wit:339`.

**upstream** — `pi/packages/coding-agent/src/main.ts:650` calls `sessionManager.appendSessionInfo(name)` and `:742-750` resolves `scopedModels` into the session options — both strictly BEFORE `createAgentSessionRuntime` at `:793`. Critically, pi's `createAgentSessionRuntime` (`agent-session-runtime.ts:414-432`) does NOT emit `session_start`: it calls `createRuntime` and returns. The HOST emits it by calling `session.bindExtensions(...)` from `rebindSession` (`rpc-mode.ts:317`, `print-mode.ts:72`, `interactive-mode.ts:1698`), strictly after main.ts has finished configuring. Emit site `agent-session.ts:389` + `:2250`.

**Impact** — Under `--mode rpc` or interactive, an extension's `session_start` handler observes a session with no display name (`get-session-name` returns `none`) and, when `--models`/`enabledModels` scoping applies to a fresh session, the pre-scope model and thinking level. An audit or intercom extension registering itself under the session name registers the empty name; a gate keying policy on the active model keys on the wrong one. The follow-on `session_info_changed`/`model_changed` events go to an empty fanout (the RPC loop subscribes later, at `rpc.rs:497`), so nothing on the wire corrects it.

**Fix** — Split announcement out of construction, matching pi: give `AgentSessionRuntime::create` a non-announcing variant (or an optional `before_start` hook, as `install_inner` already has at `runtime.rs:289`/`:319-322`), have the bin run `apply_post_build` in that window, and let the host announce afterwards via the idempotent `session.bind_extensions()` (`session.rs:2197`, latched `:2207`). The existing `session_start_lifecycle.rs:157-176` regression still passes — it pins single-announcement only.

**Verify** — A recording native extension whose `SessionStart` handler captures `session_name()` and the active `ModelRef`; drive `cyrup --mode rpc --name X --models <pattern>` and assert the handler saw `X` and the scoped model. Today it sees `None` and the unscoped model.

## SEAM-017 — No RpcClient counterpart

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** high

**cyrup** — `grep -rn 'rpc_client\|RpcClient' crates/ --include=*.rs` returns ZERO hits workspace-wide at HEAD. `cyrup/crates/cyrup-modes/src/` contains `error.rs`, `json.rs`, `lib.rs`, `print.rs`, `rpc.rs` — no client module.

**upstream** — `pi/packages/coding-agent/src/modes/rpc/rpc-client.ts` exists and exports `RpcClient`.

**Impact** — Embedders and cyrup's own tests must hand-roll NDJSON framing and request correlation (`cyrup/crates/cyrup-modes/tests/modes.rs`, `read_json_line`), which is exactly how wire-shape divergences like SEAM-011 and SEAM-031 go unnoticed.

**Fix** — Add `cyrup-modes/src/rpc_client.rs` porting `rpc-client.ts`: spawn/attach, id-correlated request/response, event stream. Retrofit `modes.rs` onto it.

**Verify** — `modes.rs` tests drive the client instead of raw lines and still pass.

## SEAM-018 — cyrup auth print-api-key / print-bearer-token missing

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup/src/subcommands.rs` declares only `pub enum PackageCommand` (`:58`) and `pub enum UpdateTargetSel` (`:78`) — grep for `pub enum` returns exactly those two; no auth surface.

**upstream** — `pi/packages/coding-agent/src/cli/credential-print.ts` (added 2026-07-27, i.e. after cyrup's 2026-07-10 baseline).

**Impact** — Scripts cannot extract a stored credential for use by another tool. Expected version lag, not owed debt.

**Fix** — Port `credential-print.ts` as a `cyrup auth print-api-key|print-bearer-token` subcommand over `cyrup-config`'s auth storage.

**Verify** — Subcommand prints the stored key for a configured provider and exits non-zero with a clear message when absent.

## SEAM-019 — --ui-mode / --alt absent and silently swallowed

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `grep -rn 'ui_mode\|ui-mode\|fullscreen' crates/ --include=*.rs` returns ZERO hits workspace-wide at HEAD, so neither flag is in the known-flag sets in `cyrup/crates/cyrup/src/diagnostics.rs` and `--ui-mode fullscreen` is absorbed with no diagnostic.

**upstream** — `pi/packages/coding-agent/src/cli/args.ts` declares `uiMode?: UiMode` and pushes an ERROR diagnostic for a bad `--ui-mode` value, plus `--alt`.

**Impact** — A user passing pi's flags gets silence rather than a warning; the flag appears accepted and does nothing.

**Fix** — Cheap half: add both flags to the known set in `diagnostics.rs` so a bad value warns instead of vanishing. The full port depends on cyrup-tui gaining an alt-screen mode.

**Verify** — `cyrup --ui-mode bogus` emits a diagnostic naming the valid values.

## SEAM-020 — --help and --list-models handled before the session exists

**Kind** parity-bug · **Severity** low · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup/src/main.rs:130-133` — `if cli.help { print!("{}", render_help(&[])); return Ok(0); }` with an EMPTY extension-flag slice; the comment at `main.rs:128-129` concedes "the bin injects an empty set today (the injection point is preserved 1:1)". `main.rs:254-256` resolves `--list-models` from `cyrup::provider::all_available_models(&models_json)` (after the runtime overlay `restore_model_catalog` at `main.rs:249`), long before any session, runtime or extension exists.

**upstream** — `pi/packages/coding-agent/src/main.ts` prints help after `createAgentSessionRuntime` (`main.ts:793`), flat-mapping `resourceLoader.getExtensions()` flags, and resolves `--list-models` off the live `modelRuntime`.

**Impact** — `cyrup --help` never lists extension-contributed flags, and `--list-models` shows the static catalog rather than what the live model runtime would offer.

**Fix** — Move both handlers after runtime construction, feeding `render_help` the extension flag set from the resource loader and `--list-models` from the runtime's model registry. 289c089's runtime catalog overlay changed WHICH static models are listed but not the ordering relative to runtime construction.

**Verify** — A native extension declaring a CLI flag appears in `cyrup --help`; `--list-models` output matches `get_available_models` over RPC.

## SEAM-028 — modes.rs setWidget case pins SEAM-011's invented wire field

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-modes/tests/modes.rs:971-976`: the comment at `:971-972` concedes "cyrup's WIT collapsed Pi's 3-arg `setWidget(key, content, options)` into ONE opaque JSON payload, forwarded verbatim" and then asserts the collapsed shape as CORRECT — `assert_eq!(req["method"], "setWidget");` at `:975` and `assert_eq!(req["widget"], serde_json::json!({"widget":"text","text":"hi"}));` at `:976`. Producer under test: `cyrup/crates/cyrup-modes/src/rpc.rs:395-400`.

**upstream** — `pi/packages/coding-agent/src/modes/rpc/rpc-types.ts:265-271` pins `widgetKey`/`widgetLines`/`widgetPlacement`; no `widget` field exists in pi's union.

**Impact** — The suite certifies the divergence, so fixing SEAM-011 turns a green test red and invites a revert. Same shape as the three defects this project already found and fixed. The adjacent `setStatus` case at `modes.rs:965-969` IS a correct parity assertion (it checks `statusText` is omitted), which makes the wrong one easy to overlook.

**Fix** — Mark it `#[ignore = "SEAM-011: cyrup collapses setWidget into one blob"]` with the pi-shaped assertion written beneath, or invert it as part of SEAM-011.

**Verify** — After SEAM-011 the test asserts pi's three fields and no `widget` key.

## SEAM-029 — ThinkingArg doc comment claims the leniency path is unreachable

**Kind** stale-port · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup/src/cli.rs:40-41`, verbatim at HEAD: "`--thinking <level>` (args.ts:57,130). Clap validates membership; the warning-on-invalid path Pi takes (args.ts:135) is unreachable here because clap rejects an unknown value with a usage error." Contradicted by `cyrup/crates/cyrup/src/diagnostics.rs:109-124`, which inspects the `--thinking` value BEFORE clap sees it, keeps it when in `VALID_THINKING_LEVELS` (`diagnostics.rs:51-53`, seven entries including `max`) and otherwise drops both tokens with `Invalid thinking level "{value}". Valid values: {joined}`.

**upstream** — `pi/packages/coding-agent/src/cli/args.ts:59` is `VALID_THINKING_LEVELS` and the warn-and-continue is `args.ts:132-140`. The pi lines cited in `cli.rs` are off by two, and `diagnostics.rs:51` already cites `args.ts:59` correctly — so the two cyrup files disagree with each other.

**Impact** — Doc-only, but this comment is exactly what mis-set a previous edition of this document: a reader concludes the leniency path does not exist and files a false gap.

**Fix** — Rewrite `cli.rs:40-41` to say the leniency pass lives in `diagnostics.rs:109-124` and cite `args.ts:59` / `args.ts:132-140`.

**Verify** — Read-through; the two files agree on the pi line numbers.

## SEAM-030 — RPC tests assert wall-clock/scheduling outcomes they cannot control

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — Three instances, line refs re-derived by grepping `Instant::now|elapsed()|sleep(Duration` across all four crates' `tests/` dirs. (a) `cyrup/crates/cyrup-modes/tests/modes.rs:1234` records `std::time::Instant::now()`, `:1236` takes `elapsed`, and `:1246-1251` asserts `elapsed < Duration::from_secs(3)` "proving the command loop is serialized (G1)" — five lines above a deterministic assertion (`bash["data"]["cancelled"] == true`, `modes.rs:1254-1258`) that proves the same thing without a clock. (c) `modes.rs:1096` takes `tokio::time::Instant::now()` and `:1111-1115` asserts `started.elapsed() < Duration::from_secs(2)` on top of an already-deterministic `tokio::time::timeout(5s)` + `assert!(!resolved)` at `:1106-1110` — pure wall-clock margin with no semantic content, the most flake-prone. (b) `modes.rs:1045` is a fixed `tokio::time::sleep(Duration::from_millis(50))` before a negative assertion at `:1047-1054`; **downgraded to a smell**, because `extension_ui_effect_json` returns `None` for `SetHeader`/`SetFooter`/`SetToolsExpanded` (`cyrup/crates/cyrup-modes/src/rpc.rs:420-422`), so no `extension_ui_request` can ever be written regardless of sleep length.

**upstream** — No counterpart: these test cyrup-original concurrency structure. Pi's `void handleInputLine(line)` (`pi/packages/coding-agent/src/modes/rpc/rpc-mode.ts:782-784`) has no equivalent.

**Impact** — Under CI load or a debug build, (a) and (c) fail for reasons unrelated to the behaviour under test, training contributors to re-run rather than investigate — which is how a suite silently stops being trustworthy.

**Fix** — Delete (a)'s and (c)'s duration assertions; the deterministic assertions beside them already prove the property. Replace (b)'s sleep with a positive synchronisation point.

**Verify** — Tests still pass with the timing assertions removed and stay green under `--test-threads=1` on a loaded machine.

## SEAM-032 — rpc_extended_command_surface pins the invented messageCount stats field

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-modes/tests/modes.rs:723-726`: comment "get_session_stats carries the aggregate counters" at `:723`, then `let stats = resp("get_session_stats");` (`:724`), `assert_eq!(stats["success"], true);` (`:725`), `assert!(stats["data"]["messageCount"].is_number(), "stats missing messageCount: {stats}");` (`:726`). `messageCount` is a cyrup invention (`cyrup/crates/cyrup-session-svc/src/state.rs:13`).

**upstream** — `pi/packages/coding-agent/src/core/agent-session.ts:259-277` — the equivalent field is `totalMessages`, and the object also carries `sessionId`, `toolCalls`, `tokens.*` and `cost`. `pi/packages/coding-agent/src/modes/rpc/rpc-types.ts:183` types the response `data: SessionStats`.

**Impact** — This is the ONLY assertion covering the `get_session_stats` payload and it is green on the wrong shape, so SEAM-031's fix turns it red and invites a revert. Note `messageCount` DOES legitimately appear in the `get_state` payload (`rpc.rs:1251`, matching pi `rpc-mode.ts:457`), so the field name is wrong only here — easy to miss.

**Fix** — Replace with pi's `totalMessages`/`sessionId`/`tokens.total` checks as part of SEAM-031, or annotate explicitly as pinning a known divergence with the pi-shaped assertion written beneath.

**Verify** — After SEAM-031 the same test asserts pi's field names and that no `messageCount` key is present under `get_session_stats`.

## SEAM-034 — CompactionResult drops pi's usage field

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session-svc/src/state.rs:87-97`: `pub struct CompactionResult { summary (:90), first_kept_entry_id (:91), tokens_before (:92), estimated_tokens_after (:94), details (:96) }` — no `usage` field. Constructed at `cyrup/crates/cyrup-session-svc/src/session.rs:1387` and `:3804` (the only construction sites) and serialized straight onto the wire by the RPC `compact` handler (`cyrup/crates/cyrup-modes/src/rpc.rs:992-997`). The data exists one layer down: `cyrup/crates/cyrup-session/src/entry.rs:70-71` carries `usage: Option<Usage>` on the compaction entry, its doc comment naming it the port of pi's `CompactionEntry.usage`.

**upstream** — `pi/packages/coding-agent/src/core/compaction/compaction.ts:88-97`: `interface CompactionResult<T = unknown> { summary; firstKeptEntryId; tokensBefore; estimatedTokensAfter?; usage?: Usage; details?: T }`, with the comment at `:93` "Usage from the LLM call(s) that generated this summary, if available"; on a split turn pi records the SUM via `combineUsage` (defined `:99`, applied `:882`). Wire contract `pi/packages/coding-agent/src/modes/rpc/rpc-types.ts:171`.

**Impact** — An RPC client cannot see what the compaction itself cost, so a cost-tracking front-end under-reports every compaction. Also blocks correct `get_session_stats` totals (pi folds compaction-entry usage into session cost), making this a prerequisite for SEAM-031.

**Fix** — Add `#[serde(default, skip_serializing_if = "Option::is_none")] pub usage: Option<Usage>` to `state.rs:89-97` (elided when absent so existing goldens stay byte-identical), populate it at `session.rs:1387` and `:3804` from the value already written to the compaction entry. While there, note `estimatedTokensAfter` is `number|undefined` upstream while cyrup types `estimated_tokens_after` non-optional at `state.rs:94`.

**Verify** — Compact a faux-provider session over RPC and assert the `compact` response `data.usage` matches the persisted compaction entry's `usage`; re-run the JSONL round-trip test to confirm byte-identity when `usage` is absent.

## Coverage

**Read at HEAD `1806375` (tree clean).** cyrup: `cyrup-session-svc/src/{session.rs, runtime.rs, subscriber.rs, state.rs, command.rs}`; `cyrup-modes/src/{rpc.rs, print.rs, json.rs, lib.rs}`; `cyrup/src/{main.rs, run.rs, cli.rs, diagnostics.rs, signals.rs, subcommands.rs}`; `cyrup-sdk/src/{client.rs, lib.rs}`; both `wit/world.wit` copies; `cyrup-session/src/entry.rs`; the `tests/` dirs of all four crates. Upstream: `pi/packages/coding-agent/src/core/{agent-session.ts, agent-session-runtime.ts, sdk.ts, extensions/types.ts, compaction/compaction.ts}`, `.../modes/{rpc/rpc-mode.ts, rpc/rpc-types.ts, print-mode.ts}`, `.../main.ts`, `.../cli/args.ts`.

**Method for closures.** Every `closed` and `partially-closed` status was attacked, not accepted: code read on both sides, with an exhaustive grep chosen to FALSIFY the claim wherever one existed — a bare `model_catalog()` for SEAM-004; both bound and unbound `agent_settled` paths for SEAM-005; a full producer→consumer trace for SEAM-013; a mechanical 32-vs-32 verb-set diff for SEAM-014; the whole of `apply_pending_control` plus sink-install ordering for SEAM-003. Nothing was overturned: six items close outright (SEAM-003/004/005/007/010/013) and three close in half (SEAM-001/002/015), which stay OPEN on their residuals.

**Corrections applied to the previous edition** (evidence, not status): pi's print-mode binds three keys and does NOT bind `uiContext` (SEAM-006); upstream fork refs `:290`/`:337` (SEAM-009); `rpc-types.ts:265-271` for setWidget (SEAM-011/028); `rpc-mode.ts:508` (SEAM-014); `print.rs:75` (SEAM-016); `rpc-types.ts:183` (SEAM-031); `rpc-types.ts:171` (SEAM-034); SEAM-030 instance (b) downgraded to a smell.

**Checked and deliberately NOT filed**, so nobody redoes it. `get_state` is a genuine field-for-field match: cyrup's `state_view` (`rpc.rs:1226-1254`) emits pi's twelve `RpcSessionState` fields in pi's order (`rpc-mode.ts:446-459`). `SessionStateView` (`state.rs:103-114`) DOES diverge from `RpcSessionState` (missing thinkingLevel/isCompacting/steeringMode/followUpMode/sessionFile/autoCompactionEnabled; extra cwd/provider/stats/contextUsage; `model` as `String`), but it is not on the wire — it is an embedder-only type reached via `command.rs:76` `SessionCommandOutcome::State`, not a port of `RpcSessionState`; its doc citation `agent-session.ts:753` is stale, which CLAUDE.md says to expect. `fork`/`clone` response `{text, cancelled}` (`rpc.rs:1097`) matches `rpc-types.ts:186-187`; `get_entries`' `since` param exists (`rpc.rs:190-193`, filtered `:1137-1141`) matching `rpc-types.ts:64`; the `get_entries`/`get_tree`/`get_messages`/`get_commands` envelopes match `rpc-types.ts:195-227`. The timing sweep across all four `tests/` dirs found seven hits: three are SEAM-030's; `summarization_retry_events.rs:414-419`, `round9_l5res.rs:347` and `round4.rs:297` are bounded polling loops with real assertions afterwards (acceptable synchronisation); `summarization_retry_events.rs:102` (`settle()`) and `compact_refusals.rs:232` precede only positive assertions, so an under-length sleep yields a false FAILURE, not a false pass. `session_start_lifecycle.rs:157-176` looks like it pins the ordering SEAM-033 wants changed but pins only single-announcement, so it is not a test defect.

**Blind spots and things taken on trust.** Nothing was executed this round (no cargo build/check/test/clippy, per the rules), so SEAM-021's hang and SEAM-022's stale-session claim are traced end-to-end through the code but not reproduced; that a `ReceiverStream` pends forever while its senders are held is standard tokio-stream behaviour, taken on trust — SEAM-022's stale-session half does not depend on it. Still unaudited: the INNER element shapes — `entries_json()`/`tree_json()` elements vs pi's `SessionEntry`/`SessionTreeNode`, `BashResult`, `Model` serialization, and the synthesized `sourceInfo` bag at `session.rs:2083-2088`; a divergence in any of those would not have been caught here. `spec/` is absent from this workspace, so SEAM-016's `Aborted ⇒ 130` half (citing arch-11 §6.6 at `run.rs:117`) is unadjudicable — though pi uses `1` for both error and aborted, so a divergence exists whatever the spec says. Not re-audited here: the `AgentSessionEvent` union against pi's event union (areas 03/06), and the package/config subcommand set-difference vs `package-manager-cli.ts` (area 05).


---

## Surface-sweep findings (2026-08-03, HEAD `9219dcd`)

Found by a **surface-driven** sweep that walked pi asking what has NO cyrup counterpart at
all, rather than checking a list of known items. That inversion exists because the
item-driven method missed pi's stray-OSC-reply swallow (`pi/packages/tui/src/tui.ts:788-794`)
— a real, user-reported bug — and by construction cannot see behaviour nobody wrote an item
for. IDs use an `-SNN` suffix to mark their provenance.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| SEAM-S01 | high | not-ported | S | Unknown/mistyped `--flags` are silently swallowed — pi's `Unknown option` / `requires a value` errors (exit 1) are never produced, and `AgentSessionRuntime::diagnostics()` has no production consumer at all |
| SEAM-S02 | medium | not-ported | S | A second SIGINT/SIGTERM is swallowed — the signal watcher is one-shot and tokio never restores SIG_DFL, so after the first signal the process ignores both signals permanently and there is no force-exit path |
| SEAM-S03 | medium | not-ported | M | No detached-child registry: `setsid`-detached bash children are not killed from any signal/teardown path, only from the per-run cancel race |
| SEAM-S04 | low | not-ported | M | `AgentSessionRuntime` has no `beforeSessionInvalidate` hook — no lifecycle point exists between `session_shutdown` and session invalidation for host-owned extension-UI teardown |
| SEAM-S05 | low | not-ported | M | RPC stdout writes are inline-awaited inside the command `select!`, so a stalled client parks the whole loop and `abort`/`abort_bash`/`shutdown` cannot be serviced |

## SEAM-S01 — Unknown/mistyped `--flags` are silently swallowed — pi's `Unknown option` / `requires a value` errors (exit 1) are never produced, and `AgentSessionRuntime::diagnostics()` has no production consumer at all

**Kind** not-ported · **Severity** high · **Effort** S · **Confidence** confirmed

**upstream** — pi/packages/coding-agent/src/core/agent-session-services.ts:98-125 — `applyExtensionFlagValues` pushes `unknownFlags` for any name no loaded extension registered and emits `{type:"error", message:'Unknown option(s): --foo'}` (:120-124); a bare `--flag` on a string-typed flag emits `{type:"error", message:'Extension flag "--foo" requires a value'}` (:113-116). Merged into `services.diagnostics` at :182, surfaced at main.ts:843 `reportDiagnostics(runtime.diagnostics)` and main.ts:844-848 `process.exit(1)` on any error-severity diagnostic.

**cyrup** — ABSENT. 

**Impact** — 

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## SEAM-S02 — A second SIGINT/SIGTERM is swallowed — the signal watcher is one-shot and tokio never restores SIG_DFL, so after the first signal the process ignores both signals permanently and there is no force-exit path

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed

**upstream** — pi/packages/coding-agent/src/modes/rpc/rpc-mode.ts:722-724 — `shutdown()` opens `if (shuttingDown) { process.exit(exitCode); }`, hard-exiting on a repeat signal without waiting for `runtimeHost.dispose()` or `flushRawStdout()`. pi/packages/coding-agent/src/modes/print-mode.ts:52-62 — the handler stays registered for the process lifetime, `disposeRuntime()` short-circuits on its `disposed` flag (:40-42), and `.finally()` fires `process.exit(signal === "SIGHUP" ? 129 : 143)` on every re-entry.

**cyrup** — ABSENT. 

**Impact** — 

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## SEAM-S03 — No detached-child registry: `setsid`-detached bash children are not killed from any signal/teardown path, only from the per-run cancel race

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed

**upstream** — pi/packages/coding-agent/src/utils/shell.ts:175-194 — process-global `trackedDetachedChildPids: Set<number>` with `trackDetachedChildPid` / `untrackDetachedChildPid` / `killTrackedDetachedChildren()` (which `killProcessTree`s each survivor then clears). Registered at pi/packages/coding-agent/src/core/tools/bash.ts:108 (`if (child.pid) trackDetachedChildPid(child.pid)`, right after a spawn with `detached: process.platform !== "win32"` at :100) and untracked in the `finally` at :142. Drained SYNCHRONOUSLY inside the signal handler, before any async teardown, in all three hosts: rpc-mode.ts:373, print-mode.ts:55, interactive-mode.ts:3674/:3700/:3732.

**cyrup** — ABSENT. 

**Impact** — 

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## SEAM-S04 — `AgentSessionRuntime` has no `beforeSessionInvalidate` hook — no lifecycle point exists between `session_shutdown` and session invalidation for host-owned extension-UI teardown

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed

**upstream** — pi/packages/coding-agent/src/core/agent-session-runtime.ts:129-131 `setBeforeSessionInvalidate(cb)`, documented (:121-128) as "a synchronous callback that runs after `session_shutdown` handlers finish but before the current session is invalidated … for host-owned UI teardown that must not yield to the event loop, such as detaching extension-provided TUI components before the old extension context becomes stale." Invoked at the exact midpoint of `teardownCurrent` (:176, between `emitSessionShutdownEvent` :170-174 and `this.session.dispose()` :177) and again in `dispose()` (:403). Ordering is pinned by pi's own test: packages/coding-agent/test/agent-session-runtime-events.test.ts:201 asserts `["session_shutdown", "beforeSessionInvalidate", "rebindSession"]`. Sole production consumer interactive-mode.ts:481-483 → `resetExtensionUI()` (:2042-2070).

**cyrup** — ABSENT. 

**Impact** — 

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

## SEAM-S05 — RPC stdout writes are inline-awaited inside the command `select!`, so a stalled client parks the whole loop and `abort`/`abort_bash`/`shutdown` cannot be serviced

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** confirmed

**upstream** — pi/packages/coding-agent/src/core/output-guard.ts:85-93 `writeRawStdout(text)` appends onto a module-global promise chain `rawStdoutWriteTail` and returns immediately — no caller awaits a write. `waitForRawStdoutBackpressure()` (:95-103) is the separate drain, and `writeRawStdoutChunk` (:20-41) retries `ENOBUFS`/`EAGAIN`/`EWOULDBLOCK` after 10 ms. Backpressure is applied to the AGENT, not to the command reader: rpc-mode.ts:357-359 `session.agent.subscribe(async () => { await waitForRawStdoutBackpressure(); })`. `flushRawStdout()` (output-guard.ts:105-108) is the exit-path drain, deliberately skipped on SIGTERM (rpc-mode.ts:735-737).

**cyrup** — ABSENT. 

**Impact** — 

**Fix** — port the upstream behaviour named above; the pi reference gives the exact shape.

**Verify** — assert the behaviour end to end, not that a function exists.

