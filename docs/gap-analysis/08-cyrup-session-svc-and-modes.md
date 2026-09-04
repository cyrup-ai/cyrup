# 08 — cyrup-session-svc + cyrup-modes + bin + sdk

This area covers the single integration seam (`cyrup/crates/cyrup-session-svc/`), the non-interactive
front-ends (`cyrup/crates/cyrup-modes/` — print, json, rpc), the binary (`cyrup/crates/cyrup/`) and
the embedder SDK (`cyrup/crates/cyrup-sdk/`), measured against
`pi/packages/coding-agent/src/core/agent-session*.ts`, `.../modes/{rpc,print-mode,json-event}`,
`.../main.ts`, `.../cli/` and `.../utils/shell.ts`.

> **Re-audited 2026-08-12, cyrup HEAD `04c1ba2`** (tree `a9000b1`, docs-only on top of the last code
> commit `04c1ba2`; branch `david/cyrup`, working tree clean). Upstream re-read at the **named tags**
> with `git show <tag>:<path>` — never from a working tree: `pi` **v0.83.0** (the ported baseline)
> for every parity item, and `pi` **v0.84.1** (latest) for every version-lag item and for the
> re-verification of the RPC/print/signal hosts. `pi-subagents` v0.47.1, `pi-permission-system`
> v0.8.0 and `pi-intercom` v0.10.1 have no surface in this area and were not consulted.
>
> **What changed in this pass.** The whole `4935cc8` (Move 18/18b) closure batch was attacked rather
> than accepted and **held on nine of ten**: SEAM-001/002/009/021/022/023/024/026/031 are genuinely
> closed at HEAD, with both sides re-read. **SEAM-006's closure is refuted and the item is reopened**
> at a corrected severity — the runtime-host half really did land, but pi's `rebindSession` binds
> three keys (`mode`, `commandContextActions`, `onError`) and cyrup's `bind_extensions()` still takes
> no arguments, so an extension fault under `cyrup -p` / `--mode json` is still swallowed.
> **6 items closed this pass**: SEAM-018, SEAM-032, SEAM-S01, SEAM-S02, SEAM-S04, SEAM-S05.
> **1 item reopened**: SEAM-006 (`high` → `medium`, residual only).
> **1 item marked misdescribed and superseded**: SEAM-019 — its upstream premise (`--ui-mode`/`--alt`)
> is false at *both* tags; the real flag is `--tui-mode`, new at v0.84.1, re-filed as SEAM-051.
> **14 items newly filed**: SEAM-047…SEAM-058 from the auditor, plus SEAM-059 and SEAM-060 recovered
> by the refuter from surface the audit walked past. **1 new high** (SEAM-047: the first
> SIGTERM/SIGHUP neither tears down nor exits, so `cyrup --mode rpc` cannot be stopped by a
> supervisor).
>
> ---
>
> **REPAIR PASS, same day (2026-08-12), applying the completeness critique and the `cli/` subtree
> sweep.** No ID was renumbered, merged or deleted.
> - **`SEAM-051` medium → `high`.** Verified end to end: `grep -rn "tui_mode\|tui-mode" crates
>   --include='*.rs'` returns **nothing**, `--tui-mode` is absent from `KNOWN_LONG_FLAGS`
>   (`cli.rs:757-799`), so `partition_extension_flags` (`cli.rs:701-753`) captures it, nothing
>   registers it, `collect_diagnostics` rates it `error`, and all three modes exit 1
>   (`main.rs:514-517`, `:662-666`, `:770-774` — each re-read). **The DEFAULT value of a v0.84.1 flag
>   makes the binary refuse to start**, with a message claiming the option is unknown. Rated `high`
>   rather than `critical`: it is a deterministic, loud, first-second refusal with a printed
>   diagnostic and a one-token workaround, not a silent or unrecoverable failure — but it is a launch
>   failure for every pi-migrant command line and wrapper script, at effort S, and it was ranked below
>   188 mediums.
> - **`SEAM-020` low → `medium`**, and its body rewritten. The `cli/` sweep found the concrete defect
>   the item's ordering framing had abstracted away: `--list-models` prints the whole compiled catalog
>   because `all_available_models` (`provider.rs:237-240`) is pi's `getModels()`, not `getAvailable()`.
> - **`SEAM-058` is now a `tracker`**, excluded from the item count — its own Fix says "track, do not
>   build", and upstream has not wired the tree into `main()` either.
> - **10 items newly filed, `SEAM-061` … `SEAM-070`**, all from the `pi/packages/coding-agent/src/cli/`
>   startup-path sweep (`file-processor.ts`, `initial-message.ts`, `list-models.ts`,
>   `session-picker.ts`, `config-selector.ts`, `startup-ui.ts` at both tags, plus every symbol they
>   consume) — the ~1 000 lines of shipped Rust under `crates/cyrup/src/startup_ui.rs` +
>   `main.rs:1040-1270` that the previous edition's flag-name diff never opened. **5 new highs**, all
>   on the pre-launch surface a user meets before the TUI exists.
> - **Bookkeeping (critique finding 17a): `SEAM-035` … `SEAM-046` do not exist and never did.** The
>   gap is a **numbering artifact, not a deletion** — the 2026-08-12 pass began its new items at
>   `SEAM-047` rather than at `SEAM-035`. Confirmed against the committed edition
>   (`git show a9000b1:docs/gap-analysis/08-cyrup-session-svc-and-modes.md`), whose highest id is
>   `SEAM-034` plus the `-SNN` series; the ids are absent from every other file and from
>   `PARITY-GAPS.md` too. Caveat, stated because it cannot be closed here: `docs/gap-analysis/` has
>   exactly one commit in cyrup's history, so this check cannot see an id dropped before the directory
>   came under source control. `SEAM-061` onward continue from the real high-water mark.
>
> **Open set after the repair pass: 40 items — 0 critical, 7 high, 19 medium, 14 low — plus 1 tracker
> (`SEAM-058`) excluded from that count.**
>
> **Structural fix applied to this file.** The previous edition split the open set across two tables
> (`## Open items` and `## Surface-sweep findings`), which is exactly how the high-severity
> `SEAM-S01` escaped a full audit pass on 2026-08-07 — see structural defect A in
> `00-residual-ledger.md`. There is now **one** open-items table. The `-SNN` ids are retained
> unchanged; only their provenance is now a column note rather than a separate section.
>
> Static analysis only: nothing was built, run or tested, and no Rust or TypeScript source was
> modified.

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
> **Area 08 — recount: 45 rows → 8 open (0 critical · 1 high · 3 medium · 4 low).** The header's
> "43 items — 0 critical, 8 high" is stale: **every one of the area's nine high rows is closed except
> `SEAM-061`.** Four (`SEAM-047`, `SEAM-051`, `SEAM-064`, `SEAM-072`) were already fixed and only
> their severity cells were stale; `SEAM-065`, `SEAM-063`, `SEAM-062` closed in sweep 1; and
> **`SEAM-075` was REFUTED — the area's newest high never existed at HEAD.**
>
> **Three items closed by refutation, all of them things this file asserted about its own crate:**
> `SEAM-075` (`resolve_model` already returns `Option<Model>` + a fallback message, and `main.rs:643`
> carries an explicit "this arm deliberately has NO modelless hard stop" comment), `SEAM-059`
> (`spawn_abort_on_signal` already takes the runtime and dereferences the CURRENT session), and
> `SEAM-008` (143/129 already live on the FIRST delivery, pinned by a test). Area 08 also refuted five
> items *other areas* had handed it — `SESS-041`, `SESS-042`, `SESS-044`, `TOOL-031` and `CFG-035`
> were all already done in this crate.
>
> **`SEAM-061` is the backlog's only remaining high and it must not be split again.** The loader half
> is area 08's; the cyrup-tui half is still absent at HEAD (`keymap.rs:888-909` has no `ToggleScope`,
> so Tab is inert while `session_selector.rs:674` advertises it). Two sweeps have now split it across
> areas 07 and 08 and neither took it. **Assign it to one agent holding both crates.**
>
> **`SEAM-073`'s mechanism claim is CORRECTED, which is the whole reason it was rated worth an ID.**
> It asserts "direct evidence that some session-svc task still runs after the session and its whole
> agent directory are gone". There is no such task: the only models-store access in this crate is
> `SessionBuilder::load_persisted_catalog_overlay` (`builder.rs:528-545`), awaited inline by its
> single caller at `builder.rs:1021`, and none of the crate's six `tokio::spawn` sites touch the
> store. **Re-scope half (b) away from this crate before anyone hunts here.**
>
> **`SEAM-020` is re-rated medium → low** and narrowed to the `--help` half. That half is one line at
> `main.rs:215` blocked on a startup reorder: pi's extension flag set comes off
> `resourceLoader.getExtensions()` and pi's help exit is strictly AFTER `createAgentSessionRuntime`
> (main.ts:793-810) — which in pi also means `--help` resolves project trust first. That user-visible
> consequence is why two sweeps declined to do it blind under the no-run rule.
>
> **ONE CHANGE NOBODY ASKED FOR, disclosed rather than buried:** `print_timings()` was moved BELOW the
> stdin-read / prompt-assembly block in the interactive and print/json arms of `main.rs`. It is a
> stderr diagnostic and pi prints at exactly that position (main.ts:899/:902), but it is a behaviour
> change beyond the marks `AGENT-027` asked for — and without it, seven of the eight new marks would
> have been recorded and never printed.


## Status since prior analyses

| ID | Status | Note |
|---|---|---|
| SEAM-001 | **closed** | Verified at HEAD: `cyrup-sdk/src/client.rs:241-256` — `build_session` ends in `session.agent_session().bind_extensions().await`; `session.rs:2516-2518` `bind_extensions` → `emit_session_start("startup", None)`, latched at `:2526`. Upstream pi v0.83.0 `agent-session.ts:389`. The SDK residual the item was kept open on is gone. |
| SEAM-002 | **closed** | `handle.rs:101-102` `close(self)` → `dispose("quit")`; `session.rs:2405-2432` `dispose_with` = `abort_and_settle()` (`:2410`) → facade `SessionShutdown` → `dispatch_notify(HostEvent::SessionShutdown)` → `before_invalidate` → `session_cancel.cancel()`. Upstream `agent-session-runtime.ts:167-178`, `:398-405`. Both residuals closed. |
| SEAM-003 | **closed** | `runtime.rs:428-430` re-installs `self.actions` onto every replacement before announcement; sink minted once in `create_unannounced` (`:310-312`). Upstream `rpc-mode.ts:322-344`. |
| SEAM-004 | **closed** | `rpc.rs:1096-1100`, `:1135-1136`, `:1415-1419` all read `available_model_catalog()`; zero bare `model_catalog()`. v0.84.1 moved pi to the synchronous `getAvailableSnapshot()` (`rpc-mode.ts:469`/`:487`), which cyrup's already-synchronous accessor (`session.rs:2726`) matches. |
| SEAM-005 | **closed** | `rpc.rs:823-825` clears `in_flight` only on `AgentSettled`; `:832`/`:837-839` set the shutdown checkpoint on the same terminal. Upstream v0.84.1 `rpc-mode.ts:355-359`. |
| SEAM-006 | **reopened — partially closed** | Closure **refuted**. The runtime-host half landed (`print.rs:58-68`, `json.rs:47-51` take `&AgentSessionRuntime`; `main.rs:761` builds via `create_unannounced`). The `onError` sink and the `mode` label did not — see the item, now `medium`. |
| SEAM-007 | **closed** | `session.rs:1386-1389` `compact` returns `Result<CompactionResult, SessionServiceError>`; refusals surface as RPC errors at `rpc.rs:1174-1183`. Upstream `agent-session.ts:1800-1807`, `rpc-mode.ts:530`. |
| SEAM-008 | partially closed | Signal set and exit codes landed (`signals.rs:39-69`, `:26-32` → 130/143/129). They are used only on the **second** delivery (`:98-99`). Item stays open at `medium` on the bookkeeping residual; the "host never returns" consequence is SEAM-047's. |
| SEAM-009 | **closed** | `runtime.rs:539` hoists `fork_anchor_live` above the `match` at `:547`; the in-memory arm (`:574-579`) branches the live manager. `fork_anchor` (`session.rs:5128-5143`) raises `InvalidForkEntry` for a missing entry and a non-user-message `Before` anchor. Upstream `agent-session-runtime.ts:274-287`, `:335-340`. |
| SEAM-010 | **closed** | `cli.rs:47-57` `ThinkingArg::Max`; `diagnostics.rs:52-53` seven levels; pre-clap leniency at `:110-124`. Upstream `args.ts:57`, `:130-139`. |
| SEAM-011 | still open | Unchanged; root cause still both WIT copies at `world.wit:326`. Every OTHER `extension_ui_request` member re-diffed against `rpc-types.ts:238-273` and matches. |
| SEAM-012 | still open | Unchanged; emit sites still lossy at `runtime.rs:461` and `:525`. Batch with SEAM-025 (one WIT bump, both copies). |
| SEAM-013 | **closed** | `rpc.rs:844-849` samples `shutdown_requested()` every iteration; both pi checkpoints present (`:837-839`, `:754`/`:776`). Upstream v0.84.1 `rpc-mode.ts:357-358`, `:787`. |
| SEAM-014 | still open | Whole `handle` switch (`rpc.rs:1020-1381`) read: no `GetAvailableThinkingLevels` arm; `grep available_thinking_levels crates/cyrup-modes` is empty. |
| SEAM-015 | still open | Third argument still a literal `None` at `rpc.rs:1232`; omission recorded only in the source comment at `:1226-1227`. |
| SEAM-016 | still open | Unchanged, and the refuter folded in a second divergence in the same function: `StopReason::Pending => 1` (`run.rs:138`) where pi leaves the initialised `exitCode = 0`. |
| SEAM-017 | still open | Zero `RpcClient`/`rpc_client` hits workspace-wide. At v0.84.1 the client's listener type became `JsonAgentSessionEvent`. |
| SEAM-018 | **closed** | `crates/cyrup/src/credential_print.rs` (618 lines) implements both print verbs, dispatched pre-parse from `main.rs:105-107`. Against the ported baseline (pi v0.83.0 `cli/credential-print.ts`, 152 lines) this is complete. The v0.84.1 `auth check` extension is a separate item, SEAM-050. |
| SEAM-019 | **misdescribed — superseded by SEAM-051** | The item's upstream premise is false at BOTH tags: `git grep -nE 'uiMode\|"--alt"' v0.83.0 -- packages/coding-agent/src` is EMPTY, and so is the same grep at v0.84.1 but for `TuiMode` imports. pi has never had `--ui-mode` or `--alt`. Do not work this ID as written. |
| SEAM-020 | still open | `main.rs:140-143` and `:283-285` both run far above the runtime constructions at `:509`/`:652`/`:761`. |
| SEAM-021 | **closed** | `rpc.rs:1003-1007` `latch_if_running` sets `in_flight` only `if !session.is_idle()`; `Steer` (`:1039`) and `FollowUp` (`:1049`) call it inside `Ok(_)` only. EOF exit at `:851` is reachable. Was **critical**. |
| SEAM-022 | **closed** | `rpc.rs:680` `watch_generation()`, `:738-743` settle-before-service, `:778-785` dedicated `gen_rx.changed()` arm, `rebind_session` at `:505-518`. The name-based predicate and its false comment are gone. Producer bumps at `runtime.rs:439`. Was **critical**. |
| SEAM-023 | **closed** | `session.rs:1348-1351` `abort()` = `abort_retry(); agent.abort();`. The `waitForIdle` tail is carried by `abort_and_settle` (SEAM-024). |
| SEAM-024 | **closed** | `session.rs:1369-1372` `abort_and_settle` = `abort()` + bounded `timeout(ABORT_SETTLE_TIMEOUT, wait_for_idle())`. All three pi-mandated consumers converted (`dispose_with` `:2410`, `compact` `:1396`, RPC `abort` `rpc.rs:1062` on the concurrent path). Sync `abort()` retained for `signals.rs:94`. |
| SEAM-025 | partially closed | The FACADE event now carries `previous_session_file` (`session.rs:2525-2534`, supplied at `runtime.rs:446`). The EXTENSION event still drops it (`session.rs:2537-2540`, `event.rs:307-308`, both `world.wit:234-235`), and `dispose`/`dispose_with` still take no target. Stays open at `medium`. |
| SEAM-026 | **closed** | `client.rs:253-254` binds; `handle.rs:85-102` `close(self)` → `dispose("quit")`. The surviving residual of both SEAM-001 and SEAM-002 is gone. |
| SEAM-027 | partially closed | The SEAM-006 half landed (`json.rs:67` binds/announces once before the loop, `:73` re-reads the active session). The per-run subscription is unchanged (`json.rs:70-83`), so the between-prompt event gap is still open. |
| SEAM-028 | still open | `modes.rs:1014-1019` still asserts the collapsed `setWidget` shape as correct. |
| SEAM-029 | still open — **corrected** | The doc contradiction is real (`cli.rs:45-46` vs `diagnostics.rs:110-124`). The auditor's second claim is **false**: `cli.rs`'s `args.ts:57,130` / `args.ts:135` citations are ACCURATE at v0.83.0. Only `diagnostics.rs:51`'s `args.ts:59` is off by two. |
| SEAM-030 | still open | All three instances survive at `modes.rs:1277`/`:1289-1293`, `:1088`, `:1139`/`:1154-1158`. |
| SEAM-031 | **closed** | `state.rs:36-54` is pi's `SessionStats` field-for-field with the nested `tokens`/`contextUsage` objects; `from_entries` (`:93-120+`) walks ENTRIES and folds `Compaction`/`BranchSummary` usage back in at `:106-110`, so the post-compaction drop is gone. |
| SEAM-032 | **closed** | `modes.rs:745` asserts `totalMessages` and `:757-758` asserts the ABSENCE of `messageCount` under `get_session_stats`. `messageCount` legitimately survives in `get_state` only (`rpc.rs:1435` vs `rpc-types.ts:106`). |
| SEAM-033 | partially closed | Closed for print/json (`create_unannounced` at `runtime.rs:292-314`, used at `main.rs:761`, announced from the mode entry points). Still open for RPC and interactive, which both call `AgentSessionRuntime::create` (`main.rs:652`, `:509`). |
| SEAM-034 | still open | `state.rs:193-203` still has no `usage` and a non-optional `estimated_tokens_after`. |
| SEAM-S01 | **closed** | `builder.rs:946-967` → `apply_extension_flag_values` → `startup_diagnostics.flags`; `runtime.rs:113-119` rates each `error`; `report_runtime_diagnostics` (`main.rs:1846-1862`) exits 1 in every mode (`:514-517`, `:662-666`, `:770-774`). Single-dash half live at `diagnostics.rs:140-148`. Was **high**. |
| SEAM-S02 | **closed** | `signals.rs:92-99` awaits a SECOND `wait_for_signal()` then `std::process::exit(repeat.exit_code())`; `wait_for_signal` builds fresh streams per call so a repeat is observable. Codes pinned at `signals.rs:111-116`. |
| SEAM-S03 | still open | `grep -rn 'kill_tracked\|track_detached\|DETACHED_PIDS\|tracked_pids' crates/` returns ZERO. Detached children ARE created (`cyrup-tools/src/ops/local.rs:272` `setsid`, `:334` `killpg`) with no process-global registry. |
| SEAM-S04 | **closed** | `runtime.rs:36` `BeforeSessionInvalidate = Arc<dyn Fn() + Send + Sync>` (synchronous, matching pi's `() => void`), field `:239`, setter `:364-365`, fired at `session.rs:2427-2429` — after `dispatch_notify(SessionShutdown)`, before `session_cancel.cancel()` (`:2431`). |
| SEAM-S05 | **closed** | `rpc.rs:575-606` splits `rpc_driver` from `write_pump` (`:618-629`) and races them; every driver emission is a non-awaited `let _ = out.send(...)`. Upstream `output-guard.ts:85-103`. |
| SEAM-047 | **new** · **FIXED 2026-08-13** | First SIGTERM/SIGHUP neither tears down nor exits — `--mode rpc` runs forever. **high**. |
| SEAM-048 | **new** | `get_commands` enumerates the last-wins command `HashMap`; pi's `name:N` disambiguation tier is dead code. `medium` (mechanism corrected by the refuter). |
| SEAM-049 | **new** | Forking before the first message drops pi's `parentSession` link. |
| SEAM-050 | **new** | `cyrup auth check` unrecognized; the whole v0.84.1 auth-command surface unported. |
| SEAM-051 | **new** — **raised to `high`** · **FIXED 2026-08-13** | `--tui-mode <regular\|fullscreen>` rejected with exit 1 instead of parsed. Supersedes SEAM-019. The default value refuses to launch the binary. |
| SEAM-052 | **new** | `--version` prints `cyrup <version>` and pre-empts the parse-error diagnostics pi reports first. |
| SEAM-053 | **new** | Optional RPC wire fields emitted as explicit `null` where pi omits the key (second instance corrected by the refuter). |
| SEAM-054 | **new** | A blank stdin line is dropped instead of producing pi's `parse` error response. |
| SEAM-055 | **new** | Extension slash commands advertised with a synthesized empty-path `sourceInfo`. |
| SEAM-056 | **new** | pi's "session has not been saved yet" fork/clone guard is absent. |
| SEAM-057 | **new** | `--json`, `--rpc`, `--output-format` are cyrup-invented flags occupying the extension-flag namespace. |
| SEAM-058 | **new** — reclassified **`tracker`** in the repair pass | pi's experimental `server`/`client` command tree + harness have no counterpart (track, do not build). Excluded from the item count. |
| SEAM-059 | **new** | The signal watcher holds the startup session `Arc`, so after any replacement it aborts a disposed session. Recovered by the refuter. |
| SEAM-060 | **new** | `get_tree` drops pi's `labelTimestamp` from every node. Recovered by the refuter. |
| SEAM-061 | **closed 2026-08-14 — REFUTED** | `--resume` picker merges current-folder AND all-projects sessions into one list, labels it "Current Folder", and advertises a dead `tab scope` toggle. Filed **high**. **Closed by sweep 6 as REFUTED: both halves are live at HEAD** — `cyrup-tui/src/session_selector.rs:154`/`:276`/`:313`/`:1918`/`:1985` and `crates/cyrup/src/main.rs:1354` + `startup_ui.rs:191-201` (pi's two loaders, `cli/session-picker.ts:15-19` @v0.83.0). |
| SEAM-062 | **new** (repair pass) | Pre-launch `--resume` picker offers rename, shows the new name, and discards it. **high**. |
| SEAM-063 | **new** (repair pass) | Session delete permanently unlinks; pi tries the `trash` CLI first. The `io::Result` is discarded, so a failed delete still reports success. **high**. |
| SEAM-064 | **new** (repair pass) · **FIXED 2026-08-13** | Pre-launch trust prompt omits both "(this session only)" options, so every trust answer is persisted. **high**. |
| SEAM-065 | **new** (repair pass) | Trust resolved pre-launch, inverting pi's tier order — an extension `project_trust` verdict is skipped entirely. **high**. |
| SEAM-066 | **new** (repair pass) | Every pre-launch TUI surface hardwires the dark palette. |
| SEAM-067 | **new** (repair pass) | Pre-launch selectors never load `keybindings.json`; hint rows print the wrong keys. |
| SEAM-068 | **new** (repair pass) | `--list-models <search>` uses a lossy hand-rolled filter while a faithful port of `fuzzyFilter` sits unused. |
| SEAM-069 | **new** (repair pass) | Trust prompt's saved-decision line never says "inherited from". |
| SEAM-070 | **new** (repair pass) | `process.title` role suffix unported — rpc/subagent/broker children are indistinguishable in `ps`. |
| SEAM-071 | **FIXED 2026-08-13** | Gated in `builder.rs` (pi gates in its resource loader, not its CLI). THREE paths closed, not one: the native loop, the package tier re-admitted through `configured`, and the pre-trust vote. Gates only the AMBIENT tier — pi loads its inline `extensionFactories` regardless of `noExtensions` (`resource-loader.ts:579-581`), which ten test files here depend on. Subagent-child carve-out decided from `pi-args.ts:413-417` — permission-system/prompt-runtime/subagents survive in a CHILD only; intercom does not. **Live-verified 2026-08-13**: broker count 0→1 with extensions on, 0→0 with `-ne`. Residual SEAM-074 also now closed. |
| SEAM-074 | **FIXED 2026-08-13** (filed from the SEAM-071 fix, closed in the verification pass) | The four shipped built-ins now answer `NativeExtension::is_ambient()`; the `AMBIENT_NATIVE_IDS` list is deleted. It was **not** cosmetic: matching on the id also caught an INLINE extension sharing the name, which dropped `build_containment_and_flag_diagnostics`'s hand-injected `FailingExt { id: "subagents" }` out of the load and lost its init failure from both the panel and the fatal exit channel. |
| SEAM-072 | **new**, **closed** (fixed) · high | `build_inputs` read process stdin instead of taking Pi's `stdinContent` argument; any inherited open pipe hung the target forever. Read moved to `main.rs`, matching `main.ts:819-832`. |
| SEAM-073 | **new** (suite-verification pass) | 14 temp dirs leaked per `-p cyrup-session-svc` run; `FileLock` never removes its lock file and a models-store access lands after `TempDir` teardown. |

## Open items

This is the **complete** open set for area 08 — one table, deliberately. The `-SNN` ids came from the
2026-08-03 surface-driven sweep and are otherwise ordinary items; four of the five have now closed.
`SEAM-058` is **not** in this table: it proposes no work and is listed under `## Trackers` below.

**`SEAM-035` … `SEAM-046` are absent by construction, not by deletion** — a numbering artifact of the
2026-08-12 pass starting its new ids at `SEAM-047`. See the repair-pass note in the header block for
the check that establishes it. Do not "recover" them.

> ### RECOUNTED 2026-09-04, second pass (`SEAM-113` closed — authoritative over every block below)
>
> **Counted set: 0 critical, 0 high, 4 medium, 3 low = 7 open.** The table now carries **74 rows: 67
> closed, 7 open.** `SEAM-058` remains under `## Trackers` and is not counted. **This area has no
> above-medium row for the first time since `SEAM-113` was filed from live use on 2026-08-15.**
> `SEAM-113` closed as REFUTED-as-open: a stale finding under ADR-0006 — its contract was settled
> against v0.83.0, the target is v0.84.4, where the opt-in Ctrl+S persist that `82f40d3` landed IS the
> contract; the section's "rank 4 input is permanently empty" claim was corrected against
> `crates/cyrup/src/bootstrap.rs:247-275` and a live headless read-back; the supposed `--default`
> residual never shipped in any pi tag (`5133c9284`); and the matched `set_thinking_level` sibling is
> dispositioned in the same row. Nothing else moved; `git status` on the two crates is clean of this
> pass. See the row and the detail section.
>
> ### RECOUNTED 2026-09-04 (baseline sweep against `4fb5e40..2571969`, plus a v0.84.1→v0.84.4 diff-stat skim of `modes/rpc/` and `modes/json-event.ts`)
>
> **Counted set: 0 critical, 1 high, 4 medium, 3 low = 8 open.** The table now carries **74 rows: 66
> closed, 8 open.** `SEAM-058` remains under `## Trackers` and is not counted. `git log --oneline
> 4fb5e40..HEAD -- crates/cyrup-session-svc crates/cyrup-modes` was read in full (28 commits) and
> every commit's diff was checked against every open row's named symbol before deciding anything.
>
> **Nothing closed this pass.** `SEAM-015`, `SEAM-020`, `SEAM-057`, `SEAM-073` and `SEAM-115` were
> each re-read against HEAD (citations re-resolved through the `session.rs`→`session/` and
> `modes.rs`→`rpc/` decompositions where the crate was later split) and are **unchanged in substance**
> — none of the 28 commits since `4fb5e40` touches their fix sites. Left exactly as found.
>
> **`SEAM-113` stays open, and its evidence grows rather than shrinks.** `82f40d3` ("persist model and
> thinking level via pi's Ctrl+S set-as-default") landed a real feature — an opt-in `Ctrl+S` /
> `ConfirmSelectionAsDefault` persist path — but re-reading `apply_model_change`
> (`crates/cyrup-session-svc/src/session/model.rs:473-539` post-split) shows it is STILL the shared
> body behind ordinary `/model` use and STILL never writes `defaultProvider`/`defaultModel`. The new
> mechanism is upstream's contract **(b)** — the DRIFT-WINDOW behaviour this row's own 2026-08-19
> contract settlement named as explicitly NOT its fix — landed without revisiting that settlement in
> either this file or `docs/adr/`. See the row for the full re-check; the reported symptom (a model
> chosen in ordinary use not surviving to the next session) is unchanged because plain `Enter` and the
> typed `/model <pattern>` path both still bypass the new opt-in write.
>
> **Two rows filed, both from the diff-stat skim, neither from the item list.** `pi/` moved from
> `v0.84.1` (this file's recorded "latest" baseline) to `v0.84.4` on the clone under `tmp/pi`; a
> diff-stat skim of the RPC and json-event surfaces this area owns (`modes/rpc/*.ts`,
> `modes/json-event.ts` — the TUI-adjacent files in the same window, `interactive-mode.ts` and
> friends, were left to area 07) turned up two confirmed, evidenced gaps, both absent at the ported
> baseline `v0.83.0` too: **`SEAM-116`** — pi added an RPC `clear_queue` verb; cyrup has the exact
> session-layer capability (`AgentSession::drain_queue`) already ported and simply never wired to the
> RPC surface. **`SEAM-117`** — pi's `message_update` wire projection gained a top-level `usage` field
> and, for `toolcall_start`, `id`/`toolName`; cyrup's projector still emits the pre-drift two-key
> shape. Both filed medium/S; see their own sections for the full citations.
>
> **Left untouched, with reasons, per "publish what you excluded":** the `## Status since prior
> analyses` table above (a differently-named but functionally identical instance of the "prior
> analyses" table the operating rules say not to edit — it is superseded by every `## Open items`
> closure below it and was not touched). The remaining `agent-session.ts`/`settings-manager.ts` half
> of the v0.84.1→v0.84.4 diff (446 and 119 changed lines) was **not** walked this pass beyond what
> SEAM-113's own re-check already covers — it is large, mostly TUI-selector-shaped
> (`settings-selector.ts`, `settings-submenu.ts`, `thinking-selector.ts`, `model-selector.ts`, all
> area 07), and a confident area-08-scoped read of the remaining `agent-session.ts` core-logic delta
> did not fit this pass's budget; left for a dedicated follow-up rather than filed on a guess.
>
> ### RECOUNTED 2026-08-19 (post-`40821ed` citation repair — authoritative over every block below)
>
> **UPDATE 2026-08-29 — `SEAM-112` CLOSED, so this area's counted set is now 0 critical, 1 high, 2 medium, 3 low = 6 open.** The count below is left as measured.
>
> **Counted set: 1 critical, 1 high, 2 medium, 3 low = 7 open.** The table carries **72 rows: 65
> closed, 7 open.** `SEAM-058` remains under `## Trackers` and is not counted. Re-derived
> mechanically over the table. The sweep-9 block below reads "4 open / 68 rows" and was correct when
> written; `SEAM-112` and `SEAM-113` were filed from live use later the same day and never moved it,
> and this pass adds two.
>
> **Two rows filed, one of them retroactively.** **`SEAM-114` — FILED AND CLOSED in this pass:** the
> `context_usage` rewrite landed 2026-08-18 at `2086366` with no row in any area file, inside this
> area's own crate, carrying three CORRECTNESS fixes (an off-branch compaction boundary, two
> producers for one occupancy number, and an unfiltered `StopReason::Deferred`) that the TUI-092
> umbrella tracks only as a performance defect — and its per-defect task file was deleted with the
> round-2 batch, so a commit subject was the entire surviving record. **`SEAM-115` — FILED, open:**
> that commit added **no test**, and nothing else in the tree drives `AgentSession::context_usage`
> over a branched session, so all three correctness fixes are unpinned.
>
> **`SEAM-112`'s candidate (2) is disposed of and the row now carries a live-re-observation
> instruction** — `879eb4e` (2026-08-18) fixed a `tokio::select!` starvation that left the run loop
> bound to the DISPOSED session after every swap, which is this row's "nothing renders" verbatim. It
> does not explain the repeated bash calls, so the row stays open and stays critical. **SUPERSEDED 2026-08-29 — the row is CLOSED.** The repeated bash calls were a second, independent port divergence, found in source rather than by re-observation: pi guards the overflow-latch clear with `stopReason !== "error" && stopReason !== "length"` (`agent-session.ts:678`) and the retry-counter reset with `!== "error"` alone (`:684`); the port fused the two into one early `return` and kept only the shared arm, so every `Length` message cleared `overflow_recovery_attempted` on `message_end` immediately before `check_compaction` read it — the one-shot brake was unreachable and compact-and-retry re-drove the interrupted turn without bound. The live re-observation this block asked for was never needed.
>
> **Method note, and the reason two closed rows changed here.** `40821ed` split
> `crates/cyrup-tui/src/app.rs` into `crates/cyrup-tui/src/app/`; every `app.rs:NNNN` in this file
> was re-pointed by SYMBOL and verified by reading the target. Both automated remaps in this file had
> landed on the wrong module (`app/extension_ui.rs` for the `C::DeleteSession` arm, which is
> `app/execute_session.rs:15`; `app/mod.rs` for `load_keybindings_json`, which is
> `app/shell.rs:159`), and five occurrences carried a range whose END was still the pre-split number.
> Re-reading `SEAM-063`'s target also showed its FILING text is now false at HEAD — the fix landed —
> so that paragraph is marked rather than left reading as current.
>
> ### RECOUNTED 2026-08-15 (sweep 9 — `cyrup-session-svc` + `cyrup-modes`, area 08 in full)
>
> **Counted set: 0 critical, 0 high, 1 medium, 3 low = 4 open**, down from the fourth edition's 26.
> The table now carries **68 rows: 64 fully closed, 4 open.** `SEAM-058` remains under `## Trackers`
> and is not counted. Re-derived mechanically over the table, not by subtracting claims.
>
> **What the open 4 are, and why none of them is schedulable against this area today:**
> `SEAM-015` (medium — needs the area-06 `register-bash-operations` WIT round-trip; **its cyrup-tools
> half turned out to be DONE**, so the row's NEEDS is halved), `SEAM-020` (low — now **four** parts
> across `crates/cyrup` + `crates/cyrup-ext`, and still wants a live terminal), `SEAM-057` (low —
> owner decision, declined by a fourth sweep), `SEAM-073` (low — half (a) is area 05, half (b) is
> mis-scoped).
>
> **Nine rows were REFUTED as open — they were already closed at HEAD and nobody had marked them.**
> `SEAM-076`, `-077`, `-078`, `-079`, `-100`, `-101`, `-102`, `-103`, `-105`, `-106`, `-107`, `-110`,
> `-111` (the whole CLI-surface block bar none) plus `SEAM-S03`, whose routing note asserted a
> `crates/cyrup-tools` half was still needed when `TRACKED_DETACHED_CHILD_PIDS` has been sitting in
> `ops/local.rs` with a `Drop`-based un-enrol. **That is thirteen of the twenty-two rows this pass
> touched: the ledger's error rate on THIS block was not ~12% but well over half**, because a whole
> CLI-surface pass (`c06bb0c`) shipped without marking a single row. The lesson is the one the
> directory already states and this pass paid for again: **confirm at HEAD before estimating.**
>
> **Eight rows were FIXED**: `SEAM-080`, `-081`, `-082`, `-083`, `-084`, `-108`, `-109`, and
> `SEAM-048`'s residual. Two of those found a site the row did not name — `json.rs`'s `write_event`
> had no wire guard at all (SEAM-081), `bash_message_payload` carried the same two `null`s as the
> response (SEAM-083) — and one refuted its own routed residual: `SEAM-048`'s two `facade.rs`
> dispatch sites were already done, and the surviving last-wins reader was
> `ExtensionHost::native_command_names`, which no row had ever named.
>
> **Not done, named rather than implied:** nothing was run against a terminal or a real provider this
> pass, so `SEAM-075`'s and `SEAM-063`'s owed live runs are still owed, and `SEAM-020` is still
> declined for exactly that reason.

> **RECOUNTED 2026-08-14 (ext-rpc surface enumeration, fourth edition) — counted set: 0 critical, 0 high, 9 medium, 17 low = 26** (`SEAM-058` remains under `## Trackers` and is not counted). The table carries **68 rows: 42 fully closed, 26 open (4 of them partially closed).** These totals include a CONCURRENT pass's `SEAM-100`…`SEAM-111` (the CLI-flag surface enumeration), which landed in this table while this block was being written; no ID collides and nothing was renumbered. **This pass's own delta is +4 medium, +1 low** (`SEAM-080`, `-081`, `-083`, `-084` medium; `-082` low), with `SEAM-085` and `SEAM-086` closed on arrival. Seven items were filed this pass — **`SEAM-080`…`SEAM-086`** — from a MECHANICAL two-sided enumeration of the RPC protocol surface (commands, response envelopes and data shapes, event types, `extension_ui_request`/`response` shapes, and the whole `RpcClient` method surface) against pi `modes/rpc/` @v0.83.0. **`SEAM-085` and `SEAM-086` close on arrival** (both fixed this pass) and do not move the open counts, so the pass adds 4 medium + 1 low.
>
> **The third-edition counts below were stale and are superseded, not disputed:** they read `3 medium, 3 low = 6` over `45 rows` and predate `SEAM-076`…`SEAM-079`, which a later sweep filed into this table without recounting the block. Re-derived mechanically over the table this pass (excluding the blockquoted routing table, whose rows repeat ids): pre-existing open was **10** — `SEAM-015`, `SEAM-048`, `SEAM-S03`, `SEAM-078` (medium) and `SEAM-020`, `SEAM-057`, `SEAM-073`, `SEAM-076`, `SEAM-077`, `SEAM-079` (low) — plus this pass's 5 and the concurrent CLI pass's 11 = **26**. The `**43 items — 0 critical, 8 high, 20 medium, 15 low**` line below the table is older still and is a *cumulative filed* count, not an open count; do not read it as either.
>
> **SUPERSEDED — RECOUNTED 2026-08-14 (sweeps 7-8 reconciliation, third edition) — counted set: 0 critical, 0 high, 3 medium, 3 low = 6** (`SEAM-058` remains under `## Trackers` and is not counted). The table carries **45 rows: 39 fully closed, 6 open (3 of them partially closed)**. Sweep 8 closed **`SEAM-017`** as REFUTED/already-done — the port is 1262 lines at HEAD and the row said "not started". `SEAM-020`'s residual **size** is corrected (it is three parts, not one line, and one of them is a type mismatch); `SEAM-057` was declined by a third sweep as an owner decision; and `SEAM-015`, `SEAM-S03`, `SEAM-048` and `SEAM-073` had their out-of-area routings **re-confirmed rather than re-derived** — none of the four is schedulable against area 08. *(Previous edition: 0 / 0 / 3 / 4 = 7, 38 closed.)*

> **ROUTING, RE-CONFIRMED 2026-08-14 (sweep 8) — four of the six remaining rows have NO fix site in area 08's crates, and each was re-read rather than re-derived.** Scheduling any of them against a `cyrup-session-svc`/`cyrup-modes` agent produces a blocked pass — the exact failure the ledger's orchestration section names.
>
> | row | where it actually lands | what it needs first |
> |---|---|---|
> | `SEAM-015` | `crates/cyrup-tools` (area 04) **+** an extension-capability decision (area 06) | a `BashOperations` trait. `rpc.rs:1232` still passes a literal `None`. A WASM guest has no way to return a callable backend across the WIT boundary. **Its sibling `DRIFT-004` needs the same trait — send the two to one agent.** |
> | `SEAM-S03` | `crates/cyrup-tools` **only** (area 04) | pi's two tracking call sites are the spawn and its `finally` **inside the bash tool**; in cyrup that is `cyrup-tools`, a crate the `cyrup` bin depends on — so a registry living in the bin can never be written to. The blocker is documented in-source at `crates/cyrup/src/signals.rs:24-64`; the drain point already exists. |
> | `SEAM-048` | `crates/cyrup-ext/src/facade.rs` (area 06) — two dispatch sites | the area-08 half (catalog + wasm command dispatcher on `resolved_commands`/`resolved_command_owner`) **is done**. |
> | `SEAM-073` | half (a) `crates/cyrup-config/src/lock.rs` (area 05); half (b) **must be re-scoped away from `cyrup-session-svc` before anyone hunts there** | the row already records why: the crate's only models-store access is `SessionBuilder::load_persisted_catalog_overlay` (`builder.rs:528-545`), awaited inline by its single caller, and none of the crate's six `tokio::spawn` sites touch the store. |

> **SUPERSEDED — RECOUNTED 2026-08-14 (sweeps 3-6 reconciliation) — counted set: 0 critical, **0 high**, 3 medium, 4 low = 7** (`SEAM-058` remains under `## Trackers` and is not counted). 38 rows are now marked CLOSED. `SEAM-020` is re-rated medium → low with the reason in its row. **Sweep 6 closed the area's last high, `SEAM-061`, as REFUTED — it was already closed at HEAD in both crates.** *(Previous edition: 0 / 1 / 3 / 4 = 8, 35 closed.)*
>
> **ROUTING: neither of this area's two remaining mediums has its fix site in `cyrup-session-svc` or `cyrup-modes`.** `SEAM-015` needs `crates/cyrup-tools` + an area-06 capability decision; `SEAM-S03` needs the `crates/cyrup-tools` half alone. `SEAM-048`'s residual is `crates/cyrup-ext/src/facade.rs`. `SEAM-073`'s half (a) is `crates/cyrup-config/src/lock.rs`. `SEAM-020`'s residual (`render_help(&[])` at `crates/cyrup/src/main.rs:216`) and `SEAM-017` are in-area.

> **AMENDED 2026-08-14 (documentation audit) — four rows added, `SEAM-076`…`SEAM-079`; the area's counted set becomes 0 critical, 0 high, 4 medium, 6 low = 10.** All four are CLI-surface defects found by reading the shipped `--help` against the code while writing user documentation, and three were confirmed by running the binary. `SEAM-079` lands inside this area's own declared blind spot (c), which is the second time that blind spot has produced an item — consider driving the `config` body rather than re-declaring it.

> ### AMENDED 2026-08-14 (mechanical CLI-surface enumeration) — `SEAM-100` … `SEAM-111`
>
> **Twelve rows added; one of them (`SEAM-104`) was FIXED in the same pass and is already closed, so
> this block contributes +1 medium and +10 low to the open set** (was 0/0/4/6 = 10 before this pass
> and the concurrent one; **this block alone takes it to 0 critical, 0 high, 5 medium, 16 low = 21**).
> No id was renumbered, merged or deleted.
>
> **DO NOT quote 21 as the area total.** The concurrent `SEAM-080` … `SEAM-086` block landed in the
> same table during this pass and carries its own delta. The single reconciler must re-derive the
> area total from the table itself rather than adding the two blocks' claims together — this note
> states only what THIS block changed.
>
> **`SEAM-087` … `SEAM-099` are unallocated ON PURPOSE — do not "recover" them.** A second
> surface-enumeration agent was filing `SEAM-080` … `SEAM-086` (the ext-RPC surface) into this file
> **concurrently** with this pass. Rather than risk two agents minting the same id — the one
> unrecoverable error in this directory — this pass moved its own block to `SEAM-100` and left a
> deliberate gap. Same precedent, same rule as `SEAM-035` … `SEAM-046`.
>
> **Where these came from.** The CLI surface was enumerated MECHANICALLY on both sides — every flag,
> subcommand, flag-value enum and help-text line at `pi v0.83.0` (`cli/args.ts` `parseArgs` +
> `printHelp`, `main.ts` dispatch order, `package-manager-cli.ts`, `cli/credential-print.ts`) against
> every cyrup entry (79 upstream vs 86 cyrup) — and diffed in BOTH directions, rather than by
> re-reading the backlog. Eight upstream entries are missing in cyrup, nine cyrup entries have no
> upstream counterpart, and four differ in shape. **Six findings were confirmed by RUNNING the
> shipped binary** (`update --models`, `config --bogus`, `config zzz`, `--list-models @foo`, bare
> `-`, `auth`), which is the standard `REPRO-LOG.md` sets and is still rare for this directory. One
> probe had a side effect worth flagging: **`cyrup -` started a real agent turn and issued a provider
> request** — that is `SEAM-104`, now fixed.
>
> **`SEAM-100` is the highest-value single row on the surface**: `cyrup update --models` does not
> exist at all, *and area 05 already reasons about lock contention against a concurrent
> `cyrup update --models`.* A gap that other analysis has already built on top of will not be
> revealed by closing the items above it.
>
> **Findings from the same enumeration that got no new id, and why** — recorded so nobody re-derives
> them: `--json` / `--rpc` / `--output-format` → `SEAM-057` (declined by three sweeps as an owner
> decision; **one detail is added to its body by this pass** — `--output-format` is absent from
> `diagnostics.rs`'s `VALUE_LONG_FLAGS`, so unlike every other value-taking flag its value is not
> passed through arg-leniency verbatim); `--tui-mode` → `SEAM-051` / ADR-0005; `auth check` and its
> three flags → `SEAM-050`; `--help` being printed before extensions load, so the Extension CLI Flags
> block is structurally always empty → **`SEAM-020`'s residual**, which already names it as three
> parts and not one; the `--offline` help line naming `CYRUP_OFFLINE` and the env block advertising a
> dead `CYRUP_SHARE_VIEWER_URL` → `TUI-063` (the dropped `(default: …)` half is folded into
> `SEAM-102`).
>
> **Three carve-outs were CLOSED rather than left open.** (1) The extension-registered flag TIER is
> dynamic, but `git -C pi grep -n 'registerFlag' v0.83.0 -- packages/coding-agent/src/extensions/`
> returns zero hits — every `registerFlag` caller lives under `examples/` — so pi's shipped default
> flag surface has no extension entries and the tier is empty on both sides. (2) `rpc-entry.ts` and
> `bun/cli.ts` are separate binary ENTRY POINTS, not argv verbs, and contribute no flags. (3)
> `git -C pi grep -nE 'argv\[2\]|process\.argv' v0.83.0` confirms pi has no hidden argv verbs at all,
> which is what makes cyrup's two (`SEAM-109`) inventions rather than divergences.
>
> **NOT DONE, and named rather than implied:** not all 86 cyrup entries were exercised against the
> binary — six targeted probes plus `update -h` and `list`. The remaining rows rest on reading the
> code at the cited lines.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| ~~SEAM-075~~ | ~~**high**~~ **CLOSED 2026-08-14 — REFUTED** | parity-bug (regression) | M | **NEW 2026-08-13** — a credential-less INTERACTIVE launch hard-errors where pi opens the TUI so `/login` can be typed — **REFUTED, CLOSED 2026-08-14**: sweep 1 + 2 — **REFUTED at HEAD; the area's newest `high` no longer exists.** `resolve_model` returns `ResolvedModel = (Option<Model>, Option<ModelRef>, ModelThinkingLevel, Option<String>)` with the empty-catalog branch assigning `fallback = Some(format_no_models_available_message())` (builder.rs:1784) instead of `Err`, and the doc at builder.rs:1690-1706 cites SEAM-075 by name. main.rs:643 carries an explicit `SEAM-075: this arm deliberately has NO modelless hard stop` comment on the interactive arm, while the rpc (:847) and print/json (:973) arms both `return no_models_available()`. **The closure rests on the code path, not on a terminal — the item's Verify live-run is still UNPERFORMED.** |
| ~~SEAM-051~~ | ~~high~~ **CLOSED 2026-08-14** | upstream-drift | S | **FIXED 2026-08-13** — `--tui-mode` rejected with exit 1 — the DEFAULT value makes the binary refuse to start — **CLOSED 2026-08-14**: closed pre-sweep (2026-08-13) — `--tui-mode` is parsed instead of rejected with exit 1. Closes DRIFT-022's flag half. |
| ~~SEAM-047~~ | ~~high~~ **CLOSED 2026-08-14** | parity-bug | M | **FIXED 2026-08-13** — First SIGTERM/SIGHUP neither tears down nor exits 143/129 — `--mode rpc` never returns — **CLOSED 2026-08-14**: closed pre-sweep (2026-08-13) — first SIGTERM/SIGHUP now tears down and exits 143/129; SEAM-008 and SEAM-059 both landed on its back. |
| ~~SEAM-065~~ | ~~high~~ **CLOSED 2026-08-14** | parity-bug | M | Trust resolved pre-launch, inverting pi's tier order — the extension `project_trust` hook is skipped — **CLOSED 2026-08-14**: sweep 1 — the Fix's own prediction held: the builder's `saved: None` and its "no trust store is wired" warning are both retired, and the extension `remember` verdict now really persists. |
| ~~SEAM-064~~ | ~~high~~ **CLOSED 2026-08-14** | parity-bug | S | **FIXED 2026-08-13** — Pre-launch trust prompt omits both "(this session only)" options — every answer is persisted — **CLOSED 2026-08-14**: closed pre-sweep (2026-08-13) — the pre-launch trust prompt now carries both "(this session only)" options; the one-character `includeSessionOnly: true` fix. |
| ~~SEAM-063~~ | ~~high~~ **CLOSED 2026-08-14** | parity-bug | M | Session delete permanently unlinks where pi routes through `trash`; the failure is swallowed — **CLOSED 2026-08-14**: sweep 1 — with a stated residual: the pre-launch status lines print AFTER teardown, not in the picker header with pi's 2 s/3 s dwell, because `cyrup_tui::SessionSelector` has no status channel (area 07). The live run in the Verify block is still owed. |
| ~~SEAM-061~~ | ~~high~~ **CLOSED 2026-08-14 — REFUTED** | parity-bug | M | `--resume` picker lists every project's sessions under "Current Folder" with a dead `tab scope` toggle — **REFUTED, CLOSED 2026-08-14**: sweep 6 — **closed at HEAD in BOTH crates by a sweep between 3 and 5; this row and its "two sweeps have split this across areas 07 and 08 and neither took it" annotation were stale.** cyrup-tui carries the action and handler (`session_selector.rs:154`, `:276`, `:313`, `:1918`, `:1985`, all citing SEAM-061 by name — including upstream's un-wired `onToggleScope` semantics, where Tab is SWALLOWED rather than ignored). The area-08 loader half is also in: `crates/cyrup/src/main.rs:1354` takes pi's **two** loaders rather than one merged vector (`:1557` and `:2442` document the de-merge) and `crates/cyrup/src/startup_ui.rs:191-201` hands both to the picker citing `cli/session-picker.ts:15-19` @v0.83.0. **With this, area 08 has ZERO open highs, and areas 08/09/10 together have zero open criticals and zero open highs.** |
| ~~SEAM-062~~ | ~~high~~ **CLOSED 2026-08-14** | parity-bug | S | Pre-launch rename is accepted, echoed on screen, and silently discarded — **CLOSED 2026-08-14**: sweep 1 — via the item's "preferred full fix" route (rename now persists) rather than the parity route (disable rename), because `set_rename_enabled` is cyrup-tui's file. If strict parity is wanted instead, it is one setter in area 07 plus deleting the `Rename` arm here. |
| ~~SEAM-006~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | S | print/json `bind_extensions` passes no `onError` sink and no mode label — **CLOSED 2026-08-14**: sweep 1. |
| ~~SEAM-008~~ | ~~medium~~ **CLOSED 2026-08-14 — REFUTED** | not-ported | S | Signal identity and 143/129 are computed but used only on the second delivery — **REFUTED, CLOSED 2026-08-14**: sweep 1 + 2 — **REFUTED as tabled: nothing remains of the filed defect.** `first_delivery_exit_code` (signals.rs:149-154) hands 143/129 to the FIRST delivery in every non-interactive host and `spawn_abort_on_signal` exits with it at :214-217, pinned by `first_sigterm_and_sighup_exit_non_interactive_hosts`. The only residue is the SIGINT `CYRUP-DELTA` already documented at signals.rs:94-100 (pi registers no SIGINT handler at all; tokio's `ctrl_c()` future cannot decline to intercept) — a stated divergence with a stated cause, not an open item. |
| ~~SEAM-011~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | M | setWidget goes on the wire with a cyrup-invented `{widget}` blob — **CLOSED 2026-08-14**: sweep 2 — the consumer halves of EXT-047, landed against the same signature area 06 derived independently. (1) `cyrup-session-svc/src/host_services.rs`: `HostServices::set_widget` now takes `(&str, Option<&[String]>, WidgetPlacement)` and builds the front-end `UiEffect::SetWidget` carrier under pi's own key names, with `lines: null` preserved as pi's remove-this-key. (2) `cyrup-modes/src/rpc.rs`: the wire emission is pi's union member — `widgetKey` always, `widgetLines` only when present (ABSENT, never null, for a removal), `widgetPlacement` only when `belowEditor`. One `[CYRUP-DELTA]`: `WidgetPlacement` has no unset state after the WIT resolves pi's `aboveEditor` default, so the default is emitted as an ABSENT key — which is what pi's `options?.placement` produces for every extension that does not set one. **FOR THE ORCHESTRATOR: `UiEffect::SetWidget { widget: Value }` was deliberately KEPT as the front-end carrier (populated under pi's key names) rather than split into typed fields, so `crates/cyrup-tui` keeps compiling while area 07 is editing it. Splitting it later is a scheduled cross-crate break, not an oversight.** |
| ~~SEAM-012~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | M | `session_before_switch` carries no reason, `session_before_fork` no position — **CLOSED 2026-08-14**: sweep 1 — the WIT half arrived from area 06 mid-pass and the emit half is populated; the body saying both hooks are lossy is superseded. |
| ~~SEAM-014~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | S | RPC verb `get_available_thinking_levels` not implemented — **CLOSED 2026-08-14**: sweep 1. |
| SEAM-015 | medium | not-ported | M | RPC bash ignores the `operations` backend override — **2026-08-15, still open; NEEDS re-measured (sweep 9)**: the routing note said this needs *"a `BashOperations` trait in `crates/cyrup-tools` (area 04) **PLUS** an extension-capability decision in area 06"*. **The cyrup-tools half is DONE** — `pub trait BashOperations` exists at `crates/cyrup-tools/src/ops/mod.rs:462`, `BashOptions::operations: Option<Arc<dyn BashOperations>>` exists and is consumed (`run_bash` takes it, and any IN-HOST caller can supply one today, which is upstream's `options?.operations ?? createLocalBashOperations({shellPath})`, `agent-session.ts:2782` @v0.83.0). **ONE half is left and it is area 06's alone:** the `operations: None` the RPC bash arm passes is upstream's ABSENT `operations`, not a dropped one, because a WASM guest still has no way to return a callable across the WIT boundary — closing it is the `register-bash-operations` import + keyed `bash-operations-exec` export round-trip, whose design is written out in `crates/cyrup-ext/src/lib.rs`'s CYRUP-DELTA register. The site is documented in-source in `crates/cyrup-modes/src/rpc.rs`'s bash arm. Sibling: DRIFT-004. **Still not schedulable against area 08.** |
| ~~SEAM-016~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | print-mode exit code derived by reverse-scanning the transcript — **CLOSED 2026-08-14**: sweep 1. |
| ~~SEAM-025~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | M | Extension `session_start`/`session_shutdown` drop pi's session-file fields — **CLOSED 2026-08-14**: sweep 1 — the WIT/event widening arrived from area 06 mid-pass and the host halves (`emit_session_start`, `dispose_with`'s new target parameter, `install_inner`) are done. |
| ~~SEAM-027~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | M | `--mode json` subscribes per-run, dropping between-prompt events — **CLOSED 2026-08-14**: sweep 1. |
| ~~SEAM-033~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | M | RPC/interactive `session_start` still precedes `--name` and `--models` — **CLOSED 2026-08-14**: sweep 1. |
| ~~SEAM-048~~ | ~~medium~~ **CLOSED 2026-08-15** | parity-bug | S | `get_commands` enumerates a last-wins map; pi's `name:N` disambiguation is dead code — **CLOSED 2026-08-15** (sweep 9). **The routed residual was mis-measured and is REFUTED as stated: both `facade.rs` dispatch sites were already done at HEAD** — `run_native_command` and `live_for_command` both go through `command_route`, which is `resolved_commands().find(invocation_name)` and returns `(owner, REGISTERED name)`. What was actually left was a THIRD site nobody had named: `ExtensionHost::native_command_names`, the diagnostics/completion enumerator, still walked the last-wins `command_names()` map + `command_owner`. With two natives both registering `deploy` it returned `["deploy"]` — the first registrant invisible, and the one surviving entry naming a command `execute_native_command` REFUSES (a collided bare name is not a command upstream either). So `deploy:2` was executable and unlistable while `deploy` was listable and unexecutable. It now maps `resolved_commands()` to `invocation_name`, filtered to native owners, which is pi's `name: cmd.invocationName` over `getRegisteredCommands()` (`modes/interactive/interactive-mode.ts:605` @v0.83.0). Test `native_command_names_lists_both_colliding_commands_at_their_suffixes` (`crates/cyrup-ext/src/tests/command_dispatch.rs`), RED before. |
| ~~SEAM-049~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | Fork before the first message drops pi's `parentSession` link — **CLOSED 2026-08-14**: sweep 1. |
| ~~SEAM-050~~ | ~~medium~~ **CLOSED 2026-08-14** | upstream-drift | M | `cyrup auth check` and the v0.84.1 auth-command surface unported — **CLOSED 2026-08-14**: sweep 1. |
| ~~SEAM-059~~ | ~~medium~~ **CLOSED 2026-08-14 — REFUTED** | parity-bug | S | Signal watcher holds the startup session `Arc` and aborts a disposed session after any replacement — **REFUTED, CLOSED 2026-08-14**: sweep 1 + 2 — **REFUTED / verified fixed at HEAD.** `spawn_abort_on_signal(runtime: Arc<AgentSessionRuntime>, …)` dereferences the CURRENT session (`runtime.session().await.abort()`, signals.rs:182-211) with an in-source SEAM-059 citation block; all three call sites pass the runtime. It landed with SEAM-047, exactly as the item predicted. |
| ~~SEAM-S03~~ | ~~medium~~ **CLOSED 2026-08-15 — REFUTED (already done at HEAD)** | not-ported | M | No detached-child registry — **REFUTED, CLOSED 2026-08-15** (sweep 9): **the `crates/cyrup-tools` half the row says is still needed is present at HEAD.** `TRACKED_DETACHED_CHILD_PIDS` lives in `crates/cyrup-tools/src/ops/local.rs` beside the `setsid`/`killpg` primitives; `LocalProc::exec` enrols its shell at spawn and — the JS→Rust half — un-enrols from `KillTreeOnDrop::drop` rather than from a post-`select!` statement, because an abandoned future never reaches such a statement (rule 10). `kill_tracked_detached_children` is the FIRST act of the signal handler and of the repeat watcher, mirroring pi's three handlers (`print-mode.ts:55`, `rpc/rpc-mode.ts:373`, `interactive-mode.ts:3663` @v0.83.0) over `utils/shell.ts:180-195`. The whole mechanism is written out at `crates/cyrup/src/signals.rs`'s module doc, including why the repeat watcher needs its own drain where pi's does not. **STATED RESIDUAL, and it is NOT area 08's:** pi also drains from `emergencyTerminalExit` (`interactive-mode.ts:3605`) and its `uncaughtException` handler (`:3631`); cyrup has no panic hook and no emergency terminal-restore path in this crate, so there is no site to add the call to. That is a `cyrup-tui`/`main.rs` concern and the drain it needs is already exported. |
| SEAM-020 | ~~medium~~ low — **PARTIALLY CLOSED 2026-08-14** | parity-bug | M | `--list-models` prints the whole compiled catalog, not the auth-configured one; `--help` omits extension flags — **PARTIALLY CLOSED 2026-08-14**: sweep 1 + 2 — the `--list-models` half is DONE (main.rs:398-407 builds the `has_configured_auth` closure and calls `cyrup::provider::available_models`, so the empty branch and its `formatNoModelsAvailableMessage()` guidance are reachable). **RESIDUAL: the `--help` half only, and it is RE-RATED medium → low.** ~~It is one line at main.rs:215 (`render_help(&[])`) blocked on a startup reorder~~ — **SIZE CORRECTED 2026-08-14 (sweep 8), and this is the point of the correction: fixing only the reorder would ship a half-width help block.** Sweep 8 opened the callee — the lesson the ledger's own Citation-hazard section draws — and found **two further defects under the "one line"**. **(a) TYPE MISMATCH.** `crates/cyrup/src/cli.rs:892` `render_help(extension_flags: &[ExtensionFlag])` takes `cli.rs:308-311`'s `ExtensionFlag { name, value: ExtFlagValue }` — a **parsed argv flag** — while pi's `printHelp(extensionFlags?: ExtensionFlag[])` (`packages/coding-agent/src/cli/args.ts:212-222` @v0.83.0) takes flag **DECLARATIONS** carrying `type`, `description` and `extensionPath`. Feeding the reorder's output into the current signature is a **type error**, not a one-liner. **(b) THE RENDERER IS ALREADY LOSSY, even if fed.** `cli.rs:900-910` emits `format!("  --{}{}", f.name, value)` where upstream emits `` `  --${flag.name}${value}` ``\ `.padEnd(30)` + `` description ?? `Registered by ${flag.extensionPath}` `` — **the whole description column is dropped**. The declarations DO exist to be read: `cyrup-ext/src/registry.rs:955-975` `register_flag(owner, name, spec: Value)` stores them in a `flags` map, so the accessor is a small **area-06** addition, not a missing subsystem. **THREE PARTS, not one:** (a) move the help exit at `crates/cyrup/src/main.rs:214-217` to after `createAgentSessionRuntime`'s cyrup equivalent (`main.ts:803-810` @v0.83.0); (b) add a flag-DECLARATION type and an accessor on the `cyrup-ext` registry; (c) fix `cli.rs:900-910` to emit pi's padded description column. **Needs ONE agent holding `crates/cyrup` AND `crates/cyrup-ext`, plus a live terminal** — the reorder makes `--help` resolve project trust first, a user-visible change three sweeps have now declined to land blind under the no-run rule. **2026-08-15 (sweep 9) — a FOURTH sweep declined it, and the size grew again for the same reason the last correction did: the callee was opened one level further.** Both cited hazards re-confirmed at HEAD (`main.rs`'s `render_help(&[])` help exit sits above the runtime build; `cli.rs`'s `render_help(extension_flags: &[ExtensionFlag])` still takes the PARSED-argv type and still emits `format!("  --{}{}", …)` with no description column). **NEW, part (d): the declaration store has no ORDER.** `cyrup-ext`'s `RegistryInner.flags` is a `HashMap<String, Value>` with a sibling `flag_owner: HashMap<String, ExtensionId>` and **no `flag_order: Vec<String>`** — while pi's block is `resourceLoader.getExtensions().extensions.flatMap(ext => Array.from(ext.flags.values()))` (`main.ts:804-806` @v0.83.0), i.e. extension LOAD order then per-extension declaration order. Feeding the accessor from the map alone would emit a help block whose row order changes between runs, so part (b) is an ordered vector plus an accessor, not an accessor. The *data* is all present — `register_flag(owner, name, spec)` stores a `FlagSpec` carrying `type`/`default`/`description` (`cyrup-ext-sdk/src/descriptor.rs:268-275`) and pi's declaration type is `{name, description?, type, default?, extensionPath}` (`core/extensions/types.ts:1516-1522`, re-derived) — so nothing is missing, but this is **four parts across two crates**, not three, and part (a) alone still ships a half-width block. |
| ~~SEAM-066~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | M | Every pre-launch TUI surface hardwires the dark palette, ignoring `settings.theme` — **CLOSED 2026-08-14**: sweep 1. |
| ~~SEAM-067~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | S | Pre-launch selectors never load `keybindings.json`, and their hint rows name the wrong keys — **CLOSED 2026-08-14**: sweep 1. |
| ~~SEAM-068~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | `--list-models <search>` uses a lossy hand-rolled fuzzy filter; the faithful port sits unused — **CLOSED 2026-08-14**: sweep 1. |
| ~~SEAM-071~~ | ~~medium~~ | parity-bug | S | **FIXED 2026-08-13** — `--no-extensions` now gates the AMBIENT natives, the package tier and the pre-trust vote; pi's inline-factory tier is correctly left alone; **live broker-count verification NOW DONE** (see the item) |
| ~~SEAM-074~~ | ~~low~~ → medium | cyrup-original | S | **FIXED 2026-08-13** — the four shipped built-ins now answer `NativeExtension::is_ambient()`; `AMBIENT_NATIVE_IDS` deleted. Not cosmetic after all: the id list was dropping a hand-injected `FailingExt { id: "subagents" }` and turned `build_containment_and_flag_diagnostics` RED |
| ~~SEAM-072~~ | ~~high~~ **CLOSED 2026-08-14** | parity-bug | S | `build_inputs` owned fd 0 instead of taking `stdinContent` — indefinite hang on an inherited pipe — **fixed this pass** — **CLOSED 2026-08-14**: closed pre-sweep — `build_inputs` no longer owns fd 0; the stdin read moved to `main.rs`, matching main.ts:819-832. |
| SEAM-073 | low | cyrup-original | S | **2026-08-15 (sweep 9): routing RE-CONFIRMED, not re-derived — half (a) `FileLock` still has no unlink (`crates/cyrup-config/src/lock.rs`'s `impl Drop` is `FileExt::unlock` and nothing else), which is area 05; half (b) remains mis-scoped to this crate.** 14 temp dirs leaked per session-svc run; `FileLock` never deletes its lock file — **2026-08-14, still open**: sweep 2 — **mechanism claim CORRECTED, which is the whole reason it was rated worth an ID.** It asserts "direct evidence that some session-svc task still runs after the session and its whole agent directory are gone". There is no such task: the ONLY models-store access in crates/cyrup-session-svc is `SessionBuilder::load_persisted_catalog_overlay` (builder.rs:528-545), awaited inline by its single caller at builder.rs:1021, and none of the crate's six `tokio::spawn` sites touch the store. **Re-scope half (b) away from this crate before anyone hunts here; half (a) is `crates/cyrup-config/src/lock.rs` — area 05.** |
| ~~SEAM-017~~ | ~~low~~ **CLOSED 2026-08-14 — REFUTED / already-done** | not-ported | M | No `RpcClient` counterpart — **CLOSED 2026-08-14**: sweep 8 read the crate instead of the row. The status line *"sweep 2 — not started"* was stale by at least three sweeps. **`crates/cyrup-modes/src/rpc_client.rs` is 1262 lines at HEAD** and implements every method upstream declares (`packages/coding-agent/src/modes/rpc/rpc-client.ts` @v0.83.0, 600 lines): `spawn` (`:505`), `attach` (`:471` — taking any reader + writer, so an in-process `tokio::io::duplex` pair can drive it), `stop` (`:602`), `on_event` (`:646`), `stderr` (`:666`), the **33 protocol verbs** `prompt` … `get_commands` (`:696`-`:950`), and the three helpers `wait_for_idle` (`:963`), `collect_events` (`:977`), `prompt_and_wait` (`:991`). Re-exported from `cyrup-modes/src/lib.rs:39-40`; `Cargo.toml:28` documents its SIGTERM-before-SIGKILL stop. The v0.84.1 `JsonAgentSessionEvent` listener-type note at line 162 above belongs to the drift window, not to this row. |
| ~~SEAM-028~~ | ~~low~~ **CLOSED 2026-08-14** | test-defect | S | `modes.rs` setWidget case pins SEAM-011's invented wire field — **CLOSED 2026-08-14**: sweep 1 + 2 — the `#[ignore]` on `set_widget_carries_pis_three_fields_and_no_widget_blob` is removed and the inline case in the effect sequence asserts pi's member on BOTH a set and a remove, including that a removal OMITS `widgetLines` rather than sending null (SEAM-053's rule) and that a non-default placement IS emitted. No assertion was weakened — the old case asserted only `method`. |
| ~~SEAM-029~~ | ~~low~~ **CLOSED 2026-08-14** | stale-port | S | `ThinkingArg` doc comment claims the leniency path is unreachable — **CLOSED 2026-08-14**: sweep 1. |
| ~~SEAM-030~~ | ~~low~~ **CLOSED 2026-08-14** | test-defect | S | RPC tests assert wall-clock/scheduling outcomes they cannot control — **CLOSED 2026-08-14**: sweep 1. |
| ~~SEAM-034~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `CompactionResult` drops pi's `usage` field — **CLOSED 2026-08-14**: sweep 1. |
| ~~SEAM-052~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `--version` prints a program name and pre-empts the diagnostics gate — **CLOSED 2026-08-14**: sweep 1. |
| ~~SEAM-053~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | Optional RPC fields emitted as explicit `null` where pi omits the key — **CLOSED 2026-08-14**: sweep 1. |
| ~~SEAM-054~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | Blank stdin line dropped instead of answered with pi's `parse` error — **CLOSED 2026-08-14**: sweep 1. |
| ~~SEAM-055~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | Extension commands advertised with an empty-path `sourceInfo` — **CLOSED 2026-08-14**: sweep 1. |
| ~~SEAM-056~~ | ~~low~~ **CLOSED 2026-08-14** | not-ported | S | pi's "session has not been saved yet" fork/clone guard absent — **CLOSED 2026-08-14**: sweep 1. |
| SEAM-057 | low — **PARTIALLY CLOSED 2026-08-14** | cyrup-original | S | `--json`/`--rpc`/`--output-format` occupy the extension-flag namespace — **PARTIALLY CLOSED 2026-08-14**: sweep 1 — documented in-tree and listed in `render_help`. **RESIDUAL: the reserved-namespace half needs an OWNER DECISION, not an implementation — deleting `--json`/`--rpc`/`--output-format` breaks `cyrup-it`'s `--rpc` fixture. Not taken unilaterally in either sweep.** **2026-08-14 (sweep 8): a THIRD sweep declined it, for the reason the row already states. No change to the row's substance — recorded so the next planner stops re-routing it to an agent.** This is a decision for the owner; an agent cannot take it. **2026-08-15 (sweep 9): a FOURTH sweep declined it, same reason, no new information. Stop routing it.** |
| ~~SEAM-112~~ | ~~**critical**~~ **FIXED 2026-08-29** | port-bug | M | **`/resume` produces a broken session: nothing renders, and bash tool calls repeat endlessly.** Owner report, live use 2026-08-15. **BOTH HALVES ARE NOW CLOSED.** **HALF 1 — "nothing renders" — CLOSED 2026-08-18 at `879eb4e`.** The `session_swapped` arm sat LAST under `biased;` while the events arm bound an IRREFUTABLE `maybe_ev = events.next()`; `Fanout::invalidate` (`cyrup-session-svc/src/subscriber.rs:89-93`) drops every sender the instant a replacement lands, so the disposed session's stream went permanently `Ready(None)`, won every poll, and starved the swap arm — no re-subscribe, no `rebind_session()`, the loop still bound to the OLD session. At HEAD the events arm is refutable (`app/run.rs:397`, `Some(ev) = events.next()`) so a closed stream DISABLES the branch, and the swap arm is hoisted directly below the input arm (`app/run.rs:331`); the rebind is extracted to `App::on_session_swapped` (`app/run_arms.rs:143`) with a generation guard and is ALSO run from the input arm's pre-dispatch reconcile, and `src/tests/run_loop_swap_arm_reachable.rs` pins both structurally. The in-source rationale at `app/run.rs:321-330` names this symptom verbatim — "the TUI up, dead". Underlying wiring, re-verified at HEAD: `AgentSessionRuntime::switch_session_with` (`cyrup-session-svc/src/runtime.rs:513`) emits `session_before_switch` with reason `"resume"`, builds a fresh session via `factory.build(SessionTarget::Resume(path), cwd)`, and calls `install` → `install_inner` (`:387`/`:398`) which bumps the runtime `generation`; the TUI watches that generation, re-subscribes via `*events = new_session.subscribe()` (`app/run_arms.rs:163`/`:164`) and re-binds at `App::rebind_session` (`app/session_bind.rs:4`). **HALF 2 — the repeated bash calls — ROOT-CAUSED IN SOURCE 2026-08-29 and FIXED. No live reproduction was needed or performed.** **cyrup's one-shot overflow-recovery brake was unarmable for `StopReason::Length`, because the very message that arms it cleared it first.** pi @v0.84.3 guards the latch clear with TWO predicates — `stopReason !== "error" && stopReason !== "length"` (`agent-session.ts:678`) — and the retry-counter reset with `stopReason !== "error"` alone (`:684`). The port fused the two independently-guarded statements into ONE early `return` and kept only the arm they share, losing `&& !== "length"`. Consequence, given that the subscriber runs `on_assistant_message_end` on `message_end` while `check_compaction` reads the latch later on `agent_end` (`session/run.rs:235`): for the one overflow shape a `Length` message triggers (`is_context_overflow` case 3 — `usage.output == 0` and `input*100 >= window*99`, `cyrup-provider/src/utils/overflow.rs:101-109`, `will_retry == true`), `session/run.rs:291` (pre-fix `:278`) set `overflow_recovery_attempted` to `false` immediately BEFORE `session/auto_compaction.rs:85` read it. **The read was `false` on every pass, so the brake at `:85-98` was unreachable code and `:100` re-set a latch the next `Length` message would clear again.** pi's cycle stops after exactly one attempt; cyrup's had NO termination condition at all — each pass ran `drop_trailing_assistant`, compacted, and `continue_run`-ed the interrupted turn, re-executing the identical bash command. That is the reported symptom exactly, and it was the only unbounded loop in the audited surface. **FIX (`session/run.rs:285-309`):** split the guard — keep the `Error` early `return`, then clear the latch only when `stop_reason != StopReason::Length`, leaving the retry-counter reset to run for `Length` as upstream does. Resulting truth table is pi's exactly: `Error` clears neither, `Length` clears the retry counter only, everything else clears both. **Two corroborations in-repo:** (a) `session/auto_compaction.rs:400-407` ALREADY pairs `StopReason::Error` with `StopReason::Length` as the retriable tail — "the retriable error or truncated-length response" — so the compaction side of the port carried both arms while the latch side carried one; the asymmetry was the bug. (b) `PROV-069`'s closure note (`01-cyrup-core-and-provider.md:330`) records that a port of `isRecoverableLength` was drafted and REVERTED because routing a truncated turn into `run_auto_compaction` produced "compaction spam" — the same defect seen from the other side. **ALL THREE CANDIDATES STRUCK AS DISPROVED.** ~~(1) the rebuilt session's tool-result path is not re-wired~~ — DEAD in both halves: `cyrup-agent/src/state.rs:173` pushes into `st.messages` on `MessageEnd` "while the state lock is held, BEFORE subscribers are awaited", all three tool-result emit sites `.await?` a `MessageStart`/`MessageEnd` pair (`cyrup-agent/src/agent/run/tools/exec.rs:249-250`, `:362-363`, `cyrup-agent/src/agent/run/tools/mod.rs:110-111`) and `subscriber.rs:171` appends to the session tree on `MessageEnd`, so there is no window; the resume seed is likewise intact (`builder.rs:1499` → `raw_message_to_agent`, `event.rs:418`, whose `Core` arm maps `Message::ToolResult` → `AgentMessage::ToolResult` preserving `tool_call_id`). ~~(2) the generation bump fires before the new session is fully installed~~ — structurally impossible: `install_inner` assigns session (`runtime.rs:445`) and generation (`:446`) under one write lock and notifies only after (`:449`). ~~(3) `rebind_session` resets the transcript but nothing drives the new subscription~~ — `on_session_swapped` (`app/run_arms.rs:143-315`) re-subscribes (`:163`), repoints the loop handle (`:164`) and replays the conversation (`:285`, `:290`). **ALSO STRUCK — two superseded plans, recorded so they are not retried.** (i) "Reproduce by logging at tool-result append and at TUI subscribe/rebind, run ONE resume, read the log": unnecessary, the divergence is readable in source. (ii) The intermediate hypothesis "widen the early return to cover `ToolUse`" is ACTIVELY WRONG — `is_context_overflow` never returns `true` for a `ToolUse` message, so a `ToolUse` turn can never be the turn that consults the latch; and pi DOES clear the latch on `ToolUse` (`"toolUse" !== "error" && !== "length"`), so suppressing it fixes nothing and introduces a fresh divergence into code that is correct. **OUT OF SCOPE, deliberately:** porting pi's `isRecoverableLength` (`ai/src/utils/overflow.ts:171-173`, joined into the Case-1 `if` at `agent-session.ts:2135`) is PARITY-GAPS **VL-P10** and was drafted-and-reverted under `PROV-069`; it widens the compact-and-retry trigger and belongs to that row, not this one. **CITATIONS REPOINTED AT HEAD** — the row's originals had drifted: `app/run.rs:344` → `:397`; `app/run.rs:293` → `:331`; `app/run_arms.rs:158` → `:163`/`:164`; `app/run_arms.rs:138` → `:143`; `app/run.rs:284-292` → `:321-330`; `agent.rs:1985-2026` (`continue_run`) → file split, `cyrup-agent/src/agent/lifecycle.rs:209` and `cyrup-agent/src/loop_fn.rs:200`/`:280`; `session.rs:5550-5562` / `session.rs:4868` / `:4775-4781` → no such file, the split is `session/run.rs`, `session/auto_compaction.rs:78` and `session/retry.rs:140` (the stale pre-split offsets inside `auto_compaction.rs`'s own SEAM-112 comments were repointed with this fix). `subscriber.rs:89-93`, `runtime.rs:513` and `session_bind.rs:4` were already correct. — **FILED 2026-08-15 (live use); FIXED 2026-08-29** |
| ~~SEAM-113~~ | ~~**high**~~ **CLOSED 2026-09-04 — REFUTED as an open bug: stale under ADR-0006, drift already chased** | port-bug | M | **CLOSED 2026-09-04 — REFUTED as an open bug: a stale finding under ADR-0006 (`docs/adr/ADR-0006-upstream-chase-cadence.md`: the parity target is each upstream's LATEST tag), whose drift window has already been chased.** The row settled contract (a) against v0.83.0; the target is now pi **v0.84.4**, where the contract is (b) — persist only on an explicit opt-in — and cyrup matches it path for path, each side re-read: `packages/coding-agent/src/core/agent-session.ts:1657-1677` `setModel(model, options)` persists only inside `if (options.persist)`; typed `/model <pattern>` → `setModel(model, { persist: false })` (`modes/interactive/interactive-mode.ts:4832`); picker Enter → `selectModel(model, false)`, Ctrl+S → `selectModel(model, true)` (`:4996`/`:5002`, hint at `:4974-4981`); RPC `set_model` never persists (`modes/rpc/rpc-mode.ts:472-478`). cyrup HEAD `a4805955`: only `C::ConfirmSelectionAsDefault { kind: Model }` (`crates/cyrup-tui/src/app/execute_misc.rs:416-458`, `persist_setting` of `defaultProvider`/`defaultModel` at `:444-448`) writes; plain Enter (`execute.rs` `ConfirmSelection{Model}` → `session.set_model`), typed `/model` and RPC `set_model` all go through `apply_model_change` (`crates/cyrup-session-svc/src/session/model.rs`) with no settings write — which IS (b). **The "rank 4 INPUT is permanently empty" claim below is WRONG at HEAD**: `resolve_default_launch_model` (`crates/cyrup/src/bootstrap.rs:247-275`) reads `eff.default_provider()`/`eff.default_model()` (`:269-270`) into `default_launch_model` (`crates/cyrup/src/provider.rs:413`) → `find_initial_model` (now `crates/cyrup-config/src/model/select.rs:237`; the `model.rs` path cited below no longer exists). **Live headless run** (debug build, isolated `CYRUP_HOME`, `--mode rpc`, transcript in `REPRO-LOG.md` §0e): (1) RPC `set_model` to a configured anthropic catalog model → OK, and no `settings.json` was written; (2) a fresh relaunch resolved `amazon-bedrock` — the reported symptom, reproduced, and exactly what (b) prescribes; (3) seeding `{"defaultProvider":"anthropic","defaultModel":"<that id>"}` into `<CYRUP_HOME>/.cyrup/agent/settings.json` made a fresh relaunch resolve that pair — the read-back is real, so the only thing that ever "dies with the session" is a selection the user did not ask to persist. **The `--default` flag residual is refuted too**: `git -C tmp/pi grep -n -e parseDefaultFlagArgs -e '\-\-default' v0.84.4 -- packages/coding-agent/src` → 0 hits. The flag was added 2026-08-19 (`496185f6e`, `2ff8ba622`; the author's feature commit `af70240d4` lives only on the untagged branch `fix/ephemeral-model-settings`) and deleted the next day by `5133c9284` *chore(settings-selector): get rid of --default and global model* ("ctrl+s is enough") — all three first contained in v0.84.3, so no tag ever shipped it; v0.84.2's hint was already `<provider/model>`. At v0.84.4 `core/slash-commands.ts:21,23` the hints are `<provider/model>` and `<level>`; cyrup `crates/cyrup-tui/src/commands.rs:147,165` carry the identical bytes, pinned by `crates/cyrup-tui/src/tests/commands.rs:22,31,352` (`cargo nextest run -p cyrup-tui -E 'test(/commands/)'` 24/24). Porting the flag would ADD a feature upstream deliberately removed. **The matched sibling `set_thinking_level`** (`crates/cyrup-session-svc/src/session/thinking.rs:48`) is in the identical (b) state and is dispositioned with this row, no separate id: typed `/thinking <level>` → `selectThinkingLevel(level, false)` (`interactive-mode.ts:4788`) ↔ cyrup `execute_misc.rs:504-557` (session-only, with pi's exact `Unknown thinking level "…". Available levels: …` error and `Thinking level: …` status); persist only via Ctrl+S `ConfirmSelectionAsDefault { kind: Thinking }` (`execute_misc.rs:465-498`, `defaultThinkingLevel` at `:487`, status `Default thinking level: …`) ↔ `selectLevel(level, true)` (`:4804-4812`). No code changed. **Falsification**: a plain-Enter or typed `/model` selection writing `defaultModel`, or a seeded `defaultProvider`/`defaultModel` pair NOT being honoured on a fresh launch, reopens it — reopen only against the v0.84.4 contract, never against v0.83.0's. **The original row follows unchanged, for the record; its citations into `session.rs` and `cyrup-config/src/model.rs` predate the `session/` and `model/` splits.** **A model chosen with `/model` does not survive into the next session — it reverts to the catalog/settings default.** Owner report, live use 2026-08-15: repeatedly selecting `moonshotai/Kimi-K3` and finding `zai-org/GLM-5.2` on the next session. **CONTRACT SETTLED 2026-08-19 — it is (a), and the fix is in `cyrup-session-svc`, not `cyrup-config` and not `cyrup-tui`.** At **v0.83.0, the tag cyrup ports**, pi persists the selection **unconditionally, from four sites**: `setModel` (`core/agent-session.ts:1578-1593` — `:1586` transcript **plus `:1587` `settingsManager.setDefaultModelAndProvider`**), `_cycleScopedModel` (`:1629` + **`:1630`**), `_cycleAvailableModel` (`:1657` + **`:1658`**), and the selector component's `handleSelect` (`modes/interactive/components/model-selector.ts:354-359`, whose `// Save as new default` write at **`:357`** runs BEFORE `onSelectCallback` at `:358`). `setDefaultModelAndProvider` (`core/settings-manager.ts:695-701`) sets `globalSettings.defaultProvider` + `.defaultModel` and `save()`s — **GLOBAL scope only, never project**. **cyrup ports the transcript half and drops the settings half.** `AgentSession::apply_model_change` (`crates/cyrup-session-svc/src/session.rs:4532-4577`) — the shared body behind `set_model` (`:2983`), `set_model_resolved` (`:2999`) and both cycle paths (`:4502`) — carries `append_model_change` at `:4563` and no settings write at all; its own doc comment at `:4529` says *"push to the agent, re-derive headers, **persist**, re-clamp…"*, where "persist" means the transcript entry, and that is what masked the gap for four days. **`cyrup-config` has no setter to call**: `EffectiveSettings::default_provider()` (`crates/cyrup-config/src/settings.rs:553`) and `default_model()` (`:557`) are the ONLY production references to those two keys anywhere in the workspace — every other hit is a test — and there is no `set_default_model_and_provider`. **The port error is legible in the source:** `crates/cyrup-tui/src/model_selector.rs:500-505` names `setDefaultModelAndProvider` in its own comment and keeps exactly half of what it does — the `provider/id` qualification at `:504`, not the write. **Both precedence chains were walked rank-for-rank and are otherwise identical** (pi `main.ts:420-445` → `:447-467` → `core/sdk.ts:196-203` → `:205-221` → `findInitialModel`, `core/model-resolver.ts:593-651`; cyrup `crates/cyrup/src/main.rs:1185-1204` → `crates/cyrup-session-svc/src/builder.rs:1850-1917` → `find_initial_model`, `crates/cyrup-config/src/model.rs:1420-1508`) — **including rank 4 itself, `model.rs:1477-1489`, which is a faithful port of `model-resolver.ts:621-631` down to the `has_configured_auth` gate. Its INPUT is permanently empty**, so a selection survives `--resume`/`--continue` (rank 3, the transcript `model_change`) and nothing else, and every genuinely NEW session falls through to rank 5 (`first_default_or_first`, `model.rs:1161-1172`) or rank 6 (`available.first()`, `:1171`) — both config-independent constants. That is precisely "reverts to the catalog default". **This is NOT version drift.** Upstream flipped to contract (b) on **2026-08-19 — AFTER the ported tag, and while this row was open**: `2ff8ba622` gates the write behind `setModel(model, options: ModelMutationOptions)` (`agent-session.ts:1598-1617`, the write now inside `if (options.persist)` at `:1607-1609`) and adds `--default` via `parseDefaultFlagArgs` (`modes/interactive/interactive-mode.ts:242`), and `9c8070fbe` binds Ctrl+S in the selector (`model-selector.ts:365`, hint at `:132`). cyrup is **right** to have no `--default` — v0.83.0's hint is `"<provider/model>"` (`core/slash-commands.ts:21`), matching `crates/cyrup-tui/src/commands.rs:70` byte for byte, where HEAD's is `"[--default] <provider/model>"`. It is **not** right to have no persist. **The `models.json` / `TUI-089` sub-hypothesis is MOOT** — nothing writes the key on ANY path, custom or built-in, so a user-declared model is not treated differently. **REMAINS: one edit plus three decisions, all in the detail section** — the `persist_nested` in-session read-back seam, whether Ctrl+P cycling persists (at v0.83.0 it does), and `set_model_id` (`session.rs:3269-3294`), which bypasses `apply_model_change` entirely. **A matched sibling was found in the same trace and is named in the detail section: `set_thinking_level` has the identical omission** and is not separately filed. — **FILED 2026-08-15 (live use); CONTRACT SETTLED 2026-08-19 — still open, now with a named fix site.** **RE-CHECKED 2026-09-04 — the named fix site is unchanged, and a DIFFERENT mechanism landed instead of it.** `apply_model_change` (post-`6fa853d` decomposition: `crates/cyrup-session-svc/src/session/model.rs:473-539`) still ends its write half at `append_model_change` (`:504`) with no settings call — re-read in full, nothing added; `set_model_id` (now `session/model.rs:313-338`) and the matched sibling `set_thinking_level` (now `session/thinking.rs:48-100`) are equally unchanged. **What landed instead, at `82f40d3` (2026-08-28, "persist model and thinking level via pi's Ctrl+S set-as-default"), is upstream's contract (b) — the DRIFT-WINDOW behaviour this row explicitly named as NOT its contract**: a new `Ctrl+S` / `SelectorOutcome::ConfirmDefault` opt-in path (`crates/cyrup-tui/src/app/execute_misc.rs:416-462`, `C::ConfirmSelectionAsDefault { kind: Model, .. }` → `persist_setting(scope, "defaultProvider"/"defaultModel", …)`) that writes the settings default ONLY when the user explicitly presses `Ctrl+S` in the picker. The commit's own record says so directly (`.flux/done/2026-08-27-23-33/PERSIST_MODEL_EFFORT.md`: *"The `Enter` path keeps cyrup's existing `model → {value}` status line"* rather than persisting). Plain `Enter` in the picker and the typed `/model <pattern>` command both still resolve through `set_model`/`set_model_resolved` → `apply_model_change` with no write — `grep -n defaultModel crates/cyrup-tui/src crates/cyrup-session-svc/src` finds the write ONLY at the `ConfirmSelectionAsDefault` handler — so the ORIGINALLY REPORTED symptom (an ordinary model selection not surviving to the next session) is UNCHANGED. Nothing in this file or in `docs/adr/` revisits "CONTRACT SETTLED … it is (a)"; the new mechanism was built without addressing this row's Fix. Stays open, **unreduced at high**. |
| ~~SEAM-114~~ | ~~medium~~ **FILED RETROACTIVELY AND CLOSED 2026-08-19** | port-bug | M | `context_usage` rebuilt the whole branch message list, and the occupancy it produced had three ways of being wrong — **the work landed 2026-08-18 at `2086366` with NO ROW anywhere**, and its per-defect task file was deleted with the round-2 batch, so a commit subject was the only record. One performance defect (a `build_context_messages()` deep clone of every branch message on **every** `MessageEnd`, awaited on the TUI run-loop task) and three correctness defects, all in `crates/cyrup-session-svc/src/session.rs`: `has_post_compaction_usage` scanned `entries()` rather than the active branch, so a `/fork` or `/tree` navigation could latch an OFF-BRANCH compaction as the boundary and print a stale pre-compaction occupancy as current (`:4093`, now `branch_path(None)` at `:4107`); `state_view` re-derived occupancy inline, so `GetState.context_usage` and `GetContextUsage` could disagree for one session state — always, for an unresolvable-v1 `first_kept_entry_id` session (`:4194`, now delegating at `:4203`); and `StopReason::Deferred` was not filtered (`:4184`, `filter_map(..).find(..)`, deliberately not `find_map`). **The durable half is three citation corrections**, including that `SessionStateView`'s `stats` / `context_usage` are **cyrup-original** — pi's `RpcSessionState` (`rpc-types.ts:95-108`) is twelve scalars carrying neither. Its Verify is NOT met: see `SEAM-115`. |
| ~~SEAM-115~~ | ~~medium~~ **CLOSED 2026-09-04** | test-defect | S | The three `context_usage` correctness fixes shipped with no test at all — **CLOSED 2026-09-04 at `c6142d01`: `crates/cyrup-session-svc/src/tests/context_usage_branch.rs` lands the three cases the item body named, each RED against the defect it pins (re-introduced in `session/stats.rs` one at a time, then restored) and GREEN at HEAD; see the item body.** ~~**FILED 2026-08-19, open.**~~ `git show --stat 2086366` touches `crates/cyrup-session-svc/src/session.rs` and one deleted markdown file; `grep -rn 'context_usage' crates/cyrup-session-svc/src/tests/` returns nothing, and the four test files elsewhere that mention `ContextUsage` cover the JSON shape, `from_last_assistant`'s clamping and the intercom projection — **none drives `AgentSession::context_usage` over a branched session.** All three bugs are wrong NUMBERS produced by a correct-looking call, so each regresses silently; the commit's own "305 passed" would not have gone red for any of them. Three cases named in the body, each RED at `2086366~1`. **RE-CHECKED 2026-09-04**: `grep -rln 'context_usage' crates/cyrup-session-svc/src/` still returns only `command.rs`, `host_services.rs`, `state.rs` and `session/stats.rs` — no test file — so the residual is unchanged; see the item body. |
| ~~SEAM-116~~ | ~~medium~~ **CLOSED 2026-09-04** | upstream-drift | S | **CLOSED 2026-09-04 at `4481e807`** (`feat(modes): SEAM-116 port pi's clear_queue RPC verb (v0.84.4)`): `SessionCommand::ClearQueue` (`crates/cyrup-modes/src/rpc/types.rs:81`), the `handle` arm (`crates/cyrup-modes/src/rpc/mod.rs:948-965`, `session.drain_queue().await` at `:956`, reply data serialized from the new shared wire struct `ClearedQueue { steering, follow_up }` at `types.rs:203`, `#[serde(rename_all = "camelCase")]` so the key is `followUp`), and `RpcClient::clear_queue() -> ClearedQueue` (`crates/cyrup-modes/src/rpc_client.rs:723`), exported from the crate root. Upstream re-read at v0.84.4: `rpc-types.ts:26`/`:124-128`, `rpc-mode.ts:433-435`, `rpc-client.ts:226-229`, `core/agent-session.ts:1588-1596`, `packages/coding-agent/docs/rpc.md:137-158`; the verb first appears in `a79b37334` (*feat(coding-agent): expose RPC queue clearing*), first tag v0.84.4 (`git -C tmp/pi tag --contains a79b37334`), absent at v0.84.1 and v0.83.0. Pinned by `crates/cyrup-modes/src/tests/modes/rpc_clear_queue.rs` (four cases: wire shape + emptied `queue_update`, `:37`; end-to-end through the client with `pendingMessageCount` 2→0 and an idempotent second drain, `:102`; the item's own concurrency case, `clear_queue` racing a `steer` over eight rounds with drained ∪ residue asserted exact, `:145`; parse, `:200`) and `crates/cyrup-modes/src/tests/rpc_client.rs:639` (scripted host; the client writes only `{type,id}` and the reply deserializes typed) — `cargo nextest run -p cyrup-modes -E 'test(/clear_queue/)'` 5/5, full crate 80/80. The wire-shape case was run alone against the unmodified crate and went RED with the host answering `{"command":"clear_queue","success":false,"error":"Unknown command: clear_queue"}`; the other four reference the new variant/method and could not compile before. **Residual (low, recorded not filed):** the `data` object's KEY ORDER is `followUp` before `steering`, not pi's literal `{steering, followUp}` — every `data` payload this host emits is a `serde_json::Value` whose object is a `BTreeMap` (the workspace keeps `preserve_order` OFF deliberately, root `Cargo.toml`), JSON object order carries no meaning, and pi's `getData` reads by name; not a defect. **Falsification:** a `clear_queue` line answering `success:false`, a reply whose `data` lacks either key or carries text other than what was queued, or a post-drain `get_state` with `pendingMessageCount > 0`, reopens it. **The original row follows unchanged.** **NEW 2026-09-04 (v0.84.1→v0.84.4 diff-stat skim of `modes/rpc/`).** pi added an RPC `clear_queue` verb after the ported baseline — `modes/rpc/rpc-types.ts:26`/`:124-128`, `modes/rpc/rpc-mode.ts:433-435`, `modes/rpc/rpc-client.ts:226-229` (all @v0.84.4), documented at `docs/rpc.md:137-158` @v0.84.4 ("send `clear_queue` before `abort`… restore the returned text in the client editor") — with no cyrup counterpart, though the capability it needs is already ported and simply unused: `AgentSession::drain_queue` (`crates/cyrup-session-svc/src/session/queue.rs:54-67`) atomically drains both queues and returns `(Vec<String>, Vec<String>)`, pi's exact `{steering, followUp}` shape (`agent-session.ts:1588-1596` @v0.84.4), and has zero callers anywhere in `crates/cyrup-modes`. `SessionCommand` (`crates/cyrup-modes/src/rpc/types.rs:53-`) has no `ClearQueue` variant; `grep -rn clear_queue crates/cyrup-modes` is empty. Confirmed absent at the ported tag too (`git -C tmp/pi show v0.83.0:packages/coding-agent/src/modes/rpc/rpc-types.ts` has no `clear_queue` member) — this is upstream-drift, not a port bug. |
| SEAM-117 | medium | upstream-drift | S | **NEW 2026-09-04 (v0.84.1→v0.84.4 diff-stat skim of `modes/json-event.ts`).** pi's projected `message_update` wire event (the json/rpc stdout shape) gained a top-level `usage` field and, for `toolcall_start`, `id`+`toolName` — `modes/json-event.ts:1,6-15,20-38,56-60` @v0.84.4, documented at `docs/rpc.md:945,971,977-994` @v0.84.4 ("Cumulative usage, tool-call ids, and tool names remain available because their size is constant"). cyrup's projector still emits the pre-v0.84.4 two-key shape: `JsonAgentSessionEvent`'s `Serialize` (`crates/cyrup-modes/src/json_event.rs:169-189`) writes only `type`+`assistantMessageEvent`, never `usage`; `DeltaOnly`'s `ToolCallStart` arm (`:216-218`) still strips to `{type, contentIndex}` alone via the shared `indexed()` helper, carrying neither `id` nor `toolName`. Confirmed absent at v0.84.1 by the diff itself, so this lands after the file's own recorded `pi/` delta window (`v0.83.0`→`v0.84.1`) closes — upstream-drift, not a port bug against the ported baseline. |
| ~~SEAM-060~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | `get_tree` drops pi's `labelTimestamp` — **CLOSED 2026-08-14**: sweep 1 — `cyrup_session::manager::TreeNode` gained `label_timestamp` from area 03 during the same pass, so the "the underlying TreeNode needs the field first" precondition is satisfied and `tree_json` now emits it. |
| ~~SEAM-069~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | Trust prompt's saved-decision line never distinguishes an inherited ancestor decision — **CLOSED 2026-08-14**: sweep 1. |
| ~~SEAM-070~~ | ~~low~~ **CLOSED 2026-08-14** | not-ported | S | `process.title` role suffix unported — rpc and subagent children are indistinguishable in `ps` — **CLOSED 2026-08-14**: sweep 1. |
| ~~SEAM-076~~ | ~~low~~ **CLOSED 2026-08-15 — REFUTED (already done at HEAD)** | cyrup-original | S | `install`/`remove` help claims the source is written to settings — **REFUTED, CLOSED 2026-08-15** (sweep 9): closed at HEAD by commit `c06bb0c` and never marked. `subcommands.rs`'s install arm reads `Install a package and record it in the package registry.` and the remove arm `Remove a package and its source from the package registry.`, each with an in-source SEAM-076 block naming `installAndPersist`→`addSourceToSettings` (`package-manager.ts:817-841` @v0.83.0) as pi's storage and `cyrup-resources/src/package/install.rs:152-158` as cyrup's. No occurrence of `settings` survives in either help body. |
| ~~SEAM-077~~ | ~~low~~ **CLOSED 2026-08-15 — REFUTED (already done at HEAD)** | cyrup-original | S | `cyrup remove --help` advertises an `npm:` example `install` rejects — **REFUTED, CLOSED 2026-08-15** (sweep 9): closed at HEAD and never marked. The remove arm's examples are now `cyrup remove git:github.com/user/repo` and `cyrup uninstall ./local/path` — both forms `PackageSource::parse` accepts — with a SEAM-077 comment recording that pi's two examples (`package-manager-cli.ts:145-146` @v0.83.0) are BOTH `npm:`. `grep -c 'npm:' crates/cyrup/src/subcommands.rs` finds none in either help body. |
| ~~SEAM-078~~ | ~~medium~~ **CLOSED 2026-08-15 — REFUTED (already done at HEAD)** | cyrup-original | M | `cyrup update` advertises four self-update flags over a stub — **REFUTED, CLOSED 2026-08-15** (sweep 9): closed at HEAD and never marked. `--self`, `--all`, `--force` and the bare/`pi` short forms are each marked `(UNAVAILABLE)` in `render_command_help`, the body ends with `Self-update: this build cannot replace its own binary. Update it with: cargo install --git <repo> cyrup` (the repo read off `env!("CARGO_PKG_REPOSITORY")` so it cannot drift), and `self_update_unavailable()` now prints to **stderr**, names that route, echoes `current_exe()` and returns **exit 1** — pi's `printSelfUpdateUnavailable` shape (`package-manager-cli.ts:424-436`, `:855` @v0.83.0). The old stdout/exit-0 `update cyrup via your package manager` line is gone. |
| ~~SEAM-079~~ | ~~low~~ **CLOSED 2026-08-15 — REFUTED (already done at HEAD)** | cyrup-original | S | `cyrup config --help` runs the picker — **REFUTED, CLOSED 2026-08-15** (sweep 9): closed at HEAD and never marked. `subcommands::dispatch`'s `config` arm tests `-h`/`--help` **first, before any flag scan** (pi `package-manager-cli.ts:612-615` @v0.83.0) and prints `render_config_help()`, which documents `-l`, both trust flags, the Tab scope switch and the `+pattern`/`-pattern` marker convention against `CONFIG_COMMAND_USAGE` (pi `:92`). Coverage blind spot (c) is therefore also closed. |
| ~~SEAM-100~~ | ~~medium~~ **CLOSED 2026-08-15 — REFUTED (already done at HEAD)** | not-ported | M | `cyrup update --models` does not exist — **REFUTED, CLOSED 2026-08-15** (sweep 9): closed at HEAD and never marked, and it landed WHOLE rather than as the parse half alone. `UpdateTargetSel::Models` exists; `parse_package_command` has the `--models` arm; BOTH conflict messages are pi's verbatim (`--all cannot be combined with --self, --extensions, --models, or --extension` and `--models cannot be combined with --self, --extensions, --all, or --extension`, `package-manager-cli.ts:322-323`/`:331-332`); the usage string carries `--models`; the help body carries `--models   Refresh model catalogs only` in BOTH the options list and the short forms; and `dispatch` routes it to `crate::provider::refresh_model_catalogs` (pi `refreshModelCatalogs`, `:397-423`) BEFORE the self-update body, with `models_target_never_reaches_the_self_update_body` pinning that ordering. |
| ~~SEAM-101~~ | ~~low~~ **CLOSED 2026-08-15 — REFUTED (already done at HEAD)** | parity-bug | S | `config` accepts unknown options and stray arguments silently — **REFUTED, CLOSED 2026-08-15** (sweep 9): closed at HEAD and never marked, in the same twenty lines as SEAM-079 exactly as the row predicted. The `config` arm now walks `argv[1..]` and answers an unknown option with `Unknown option <arg> for "config".` + the usage line and an unexpected positional with `Unexpected argument <arg>.` + the usage line, **exit 1** in both cases (pi `package-manager-cli.ts:626-636` @v0.83.0). |
| ~~SEAM-102~~ | ~~low~~ **CLOSED 2026-08-15 — REFUTED (already done at HEAD)** | parity-bug | S | `--help`'s env block and the read set differ in both directions — **REFUTED, CLOSED 2026-08-15** (sweep 9): closed at HEAD and never marked. All seven named rows are present in `render_help`'s env block — re-derived by grepping each literal: `ANTHROPIC_AUTH_TOKEN`, `QWEN_TOKEN_PLAN_API_KEY`, `QWEN_TOKEN_PLAN_CN_API_KEY`, `XIAOMI_API_KEY`, `XIAOMI_TOKEN_PLAN_CN_API_KEY`, `XIAOMI_TOKEN_PLAN_AMS_API_KEY`, `XIAOMI_TOKEN_PLAN_SGP_API_KEY`, one hit each — and the share-viewer row carries pi's parenthetical again (`- Base URL for /share command (default: https://pi.dev/session/)`, pi `args.ts:389`). The invariant is pinned by a test in `cli.rs` that walks the provider→env-name pairs rather than a literal list. |
| ~~SEAM-103~~ | ~~low~~ **CLOSED 2026-08-15 — REFUTED (already done at HEAD)** | parity-bug | S | `--list-models` swallows a following `@file` token — **REFUTED, CLOSED 2026-08-15** (sweep 9): closed at HEAD and never marked. `apply_arg_leniency` carries pi's TWO-part guard (`args.ts:171-177` @v0.83.0): when the next token starts with `-` **or** `@`, `--list-models` is rewritten to `--list-models=` so clap cannot reach past the flag for a value. Pinned unit-side by `list_models_does_not_swallow_a_following_file_arg` and end-to-end by `list_models_leaves_a_following_file_arg_alone`. |
| ~~SEAM-104~~ | ~~low~~ **CLOSED 2026-08-14 — FIXED THIS PASS** | parity-bug | S | A bare `-` fell through to the positionals and became the PROMPT, so `cyrup -` started a real agent turn and issued a provider request where pi exits 1 without contacting anything — **FIXED 2026-08-14**: the non-upstream `arg.len() > 1` guard is deleted from `crates/cyrup/src/diagnostics.rs` and a red-before test pins both `-` and `--`. |
| ~~SEAM-105~~ | ~~low~~ **CLOSED 2026-08-15 — REFUTED (already done at HEAD)** | parity-bug | S | Repeated `--models`/`--tools`/`--exclude-tools` append; pi replaces — **REFUTED, CLOSED 2026-08-15** (sweep 9): closed at HEAD and never marked, and by the row's own second route (pre-clap argv post-processing) rather than `ArgMatches::get_occurrences`. `diagnostics.rs` declares `ASSIGNING_FLAGS` (all three families, every accepted spelling including `-t` and the `--tools=read` `=` form), records the `[start, end)` span each occurrence writes into the cleaned argv, and deletes every span but the LAST before clap parses. The comma form is untouched. |
| ~~SEAM-106~~ | ~~low~~ **CLOSED 2026-08-15 — REFUTED (already done at HEAD)** | parity-bug | S | `--export` runs after four guards pi runs it before — **REFUTED, CLOSED 2026-08-15** (sweep 9): closed at HEAD and never marked. The export branch now sits in `main.rs` immediately after the `--version` exit and above session-flag validation, the RPC `@file` guard and the `--api-key requires a model` bail — pi's own position (`main.ts:578-590` @v0.83.0) — with an in-source SEAM-106 block recording that it used to sit ~130 lines lower. Its optional output path is taken from `messages.first()` (post-`@file` partitioning), not from raw positionals, so `cyrup --export s.jsonl @notes.md` no longer writes to a file named `@notes.md`. |
| ~~SEAM-107~~ | ~~low~~ **CLOSED 2026-08-15 — REFUTED (already done at HEAD)** | parity-bug | S | `-p`'s `---` escape hatch unported — **REFUTED, CLOSED 2026-08-15** (sweep 9): closed at HEAD and never marked. `apply_arg_leniency` carries pi's three-part `-p`/`--print` condition (`args.ts:140-146` @v0.83.0) and marks the consumed token with `ESCAPED_MESSAGE_PREFIX` — a NUL, the one byte a process argument provably cannot contain, so the marker cannot collide with user input — which `Cli::restore_escaped_positionals` strips back to the literal spelling before anything reads `positionals`. |
| ~~SEAM-108~~ | ~~low~~ **CLOSED 2026-08-15 — FIXED (documentation)** | upstream-drift | S | The `auth` surface is v0.84.1-shaped against a v0.83.0 port — **FIXED 2026-08-15** (sweep 9). The row asked for a `[CYRUP-DELTA]` at the argument-validation site, not a revert, and that is what landed: `validate_credential_print_args` now opens with **`[CYRUP-DELTA] (SEAM-108)`** naming the ported baseline (**v0.83.0**), the tag the surface came from (**v0.84.1**), all three divergences (required-argument rule, verb COUNT — three vs `printCredentialPrintHelp`'s two at `credential-print.ts:24-30` — and the two error sentences), the concrete consequence (`cyrup auth print-api-key --provider openai` succeeds where v0.83.0 rejects it with `Credential printing requires --model <model>`, `:67-68`), and an explicit *do not "restore" v0.83.0's shape without an owner decision that also reverts SEAM-050*. Test `the_v0_84_1_forward_port_is_declared_as_a_cyrup_delta` reads the module's own source and asserts the marker, both tags and what the delta must name — **RED before: no `[CYRUP-DELTA]` mentioning SEAM-108 existed in the file.** |
| ~~SEAM-109~~ | ~~low~~ **CLOSED 2026-08-15 — FIXED (documentation)** | cyrup-original | S | Two hidden argv verbs with no upstream counterpart — **FIXED 2026-08-15** (sweep 9). Each module now carries a `# [CYRUP-DELTA] (SEAM-109)` section naming the mechanism it replaces, **re-derived rather than carried**: `__subagent-runner` cites `pi-subagents` `src/runs/background/async-execution.ts:492` (`const runner = path.join(…, "subagent-runner.ts")`) and `:516` (`spawn(nodeCommand, [jitiCliPath, runner, cfgPath], { detached: true, … })`) at HEAD `30c6080`; `__intercom-broker` cites `pi-intercom` `broker/spawn.ts:157-163` (`node <tsx-cli> <brokerPath>`) and `getBrokerSpawnOptions` `:174-191` (`detached: true`, `stdio: "ignore"`, `PI_CODING_AGENT_DIR` in the child env) at HEAD `30dcbdd`. Both state that pi has **no argv verbs at all**, that upstream's selector is a SCRIPT PATH handed to an interpreter (which a single compiled binary cannot have), that the user-visible mechanism — detached process, own process group, stdio to files, config by path — is ported literally and only the selector differs, and that the verb is intentionally undocumented. Test `the_hidden_argv_verbs_are_invisible_and_declared_as_cyrup_deltas` asserts the invisibility half (neither token in `render_help` or the package `SUBCOMMANDS`) and the delta half — **RED before: neither module had a delta.** |
| ~~SEAM-110~~ | ~~low~~ **CLOSED 2026-08-15 — REFUTED (already done at HEAD)** | cyrup-original | S | `update <source>` accepts a third self alias and advertises the wrong one — **REFUTED, CLOSED 2026-08-15** (sweep 9): closed at HEAD and never marked. The `source_is_self` binding carries a `CYRUP-DELTA (SEAM-110)` citing `package-manager-cli.ts:348` @v0.83.0 for pi's exactly-two-alias check and stating why the superset is kept, and the short-forms line now reads `cyrup update cyrup   Update cyrup only (self and pi also work as aliases, UNAVAILABLE)` — the guessable spelling is the documented one. |
| ~~SEAM-111~~ | ~~low~~ **CLOSED 2026-08-15 — REFUTED (already done at HEAD)** | parity-bug | S | Top-level help's Commands block understates the shipped surface — **REFUTED, CLOSED 2026-08-15** (sweep 9): closed at HEAD and never marked. `render_help`'s Commands block reads `cyrup update [source|self|pi]   Update cyrup, extensions, or model catalogs` and `cyrup config [-l]               Open TUI to enable/disable package resources (Tab switches scope)` — pi `args.ts:232`/`:234` @v0.83.0 — and the model-catalog clause is honest because SEAM-100 landed the command, exactly the ordering this row required. Pinned by tests asserting both `cyrup config [-l]` and `(Tab switches scope)`. |
| ~~SEAM-080~~ | ~~medium~~ **CLOSED 2026-08-15 — FIXED** | cyrup-original | M | `model_changed` is a cyrup-invented line on the RPC stdout stream — **FIXED 2026-08-15** (sweep 9), by the row's option **(a): filter, do not document-and-keep.** The decision and its reasoning are written down at `crates/cyrup-modes/src/json_event.rs`'s new `is_upstream_wire_event`, which is the single predicate all THREE stdout write sites now go through (`rpc.rs`'s live event arm, `rpc.rs`'s EOF drain, and `json.rs`'s `write_event`). `ModelChanged` stays on the in-process fanout, which is where `cyrup-tui` reads it, so nothing in-process loses it. Landed together with SEAM-081, as that row asked. **Still owed by whoever next edits them: `SEAM-032`'s and `SEAM-033`'s Impact paragraphs must stop citing `model_changed` as an upstream event** — they are in this file and were not rewritten this pass. |
| ~~SEAM-081~~ | ~~medium~~ **CLOSED 2026-08-15 — FIXED** | cyrup-original | M | `session_start`/`session_shutdown` reach RPC stdout — **FIXED 2026-08-15** (sweep 9), same predicate and same decision as SEAM-080. Both stay on the fanout AND on the extension tier: `session.rs`'s `dispose_with` and `emit_session_start` each emit `fanout_emit(...)` **and** `dispatcher().dispatch_notify(HostEvent::…)`, two independent paths, so filtering the stdout copy leaves the tier upstream actually has completely intact (`cyrup-it/tests/bin/lifecycle.rs` observes it there, unchanged). **Two extras this pass found while landing it.** (1) `json.rs`'s `write_event` had NO guard of any kind, so it was emitting all FOUR cyrup-only members — including `session_replaced`, which the row records as "correctly contained" on the strength of the rpc guards alone; the json mode was never checked. It is now covered by the same rule. (2) The row's LOAD-BEARING warning stands and is now enforceable: an acceptance test for SEAM-047 must assert an extension observed `session_shutdown`, not a stdout line, and a stdout assertion would now fail rather than silently pin the invention. |
| ~~SEAM-082~~ | ~~low~~ **CLOSED 2026-08-15 — FIXED (documentation)** | cyrup-original | S | `RpcClient::attach` is a cyrup-original constructor path — **FIXED 2026-08-15** (sweep 9). The row asked for no repair, only that the existing note state in terms that `attach` has NO upstream counterpart rather than only naming what it decomposes. `rpc_client.rs`'s `attach` doc now opens with a `**[CYRUP-DELTA] (SEAM-082)**` saying exactly that, citing `rpc-client.ts:73-139` @v0.83.0 as pi's ONLY constructor path, stating why cyrup adds one (an in-process `tokio::io::duplex` pair), and recording that the delta adds **no wire surface**. **No red-before test:** this is a doc block over an unchanged code path, so there is nothing that could go red — the row's own Verify is a `grep`, and it now passes. |
| ~~SEAM-083~~ | ~~medium~~ **CLOSED 2026-08-15 — FIXED** | parity-bug | S | The `bash` response emits `fullOutputPath`/`exitCode` as explicit `null` — **FIXED 2026-08-15** (sweep 9). Both fields gained `skip_serializing_if = "Option::is_none"` beside their existing read-side `#[serde(default)]`, upstream re-derived at `core/bash-executor.ts:33`/`:39` @v0.83.0 and `docs/rpc.md:473-479` (normal, no key) vs `:482-495` (truncated, key present). **A SECOND site the row did not name was carrying the same two `null`s and is fixed with it:** `bash_message_payload` (`bash.rs`) is a hand-written `json!` — serde attributes do not reach it — and it builds the `bashExecution` custom message that rides `message_update` to the same clients (pi's object literal is `recordBashResult`, `agent-session.ts:2803-2814` @v0.83.0, re-derived; the row's `:2628-2640` was stale). The row's audit ask is discharged too: `output`/`cancelled`/`truncated` are required on both sides and stay unconditional. Tests `bash_response_omits_absent_optionals_rather_than_sending_null` and `bash_execution_message_omits_absent_optionals_too` (`bash.rs`), RED before on the first assertion of each half, and each asserts the truncated/exited branch still CARRIES the key. |
| ~~SEAM-084~~ | ~~medium~~ **CLOSED 2026-08-15 — FIXED** | parity-bug | S | `get_commands` extension entries carry a fabricated `sourceInfo` — **FIXED 2026-08-15** (sweep 9), all three parts. The missing piece was that cyrup's registry is FLAT and had nowhere to keep what pi derives once per extension in `createExtension` (`core/extensions/loader.ts:433-444` @v0.83.0), so the fix adds `ExtensionProvenance {source, base_dir}` plus `record_extension_provenance`/`extension_provenance` to `cyrup-ext/src/registry.rs`, populated by the two loaders that know the answer: `load_discovered` records `local(disc.dir)` (upstream's `else "local"` + `path.dirname(resolvedPath)`) and `load_native_body` records `inline()` — upstream's `loadExtensionFromFactory`, whose default `extensionPath` is the literal `"<inline>"` (`loader.ts:490`), which is exactly what a compiled-in native is: no path, no directory. `session.rs`'s emit site now builds `sourceInfo` from that, **omitting `baseDir` when absent** (pi's `baseDir?`) and **omitting `description` when empty** (pi's `description?: string`, `extensions/types.ts:1163-1168`). `scope`/`origin` stay `"temporary"`/`"top-level"` because those are `createSyntheticSourceInfo`'s defaults (`source-info.ts:35-36`), which `createExtension` never overrides; the sibling TOP-LEVEL `"source": "extension"` is pi's `SlashCommandSource` (`slash-commands.ts:4`) and is deliberately untouched. Tests in `crates/cyrup-session-svc/src/tests/get_commands_source_info.rs`, both RED before. |
| ~~SEAM-085~~ | ~~low~~ **FILED AND CLOSED 2026-08-14** | stale-port | S | The `message_update` v0.84.1 projection is disclosed, but four of its supporting citations were **v0.84.1 line numbers presented against v0.83.0 paths with no version tag** — **FIXED 2026-08-14 (ext-rpc surface enumeration)**: `rpc.rs:304-305` and `:328-329` asserted `Pi's output(toJsonEvent(event)) (rpc-mode.ts:356)` where v0.83.0's `:355` is a bare `output(event)`, `:356` is the `agent_settled` line, and `toJsonEvent` is not in the tree at all; `json_event.rs:56` cited `coding-agent/docs/rpc.md:952-956` for the omission contract where v0.83.0's `:952-956` is the streaming example that SHOWS `message` and `partial` — the exact opposite. All now carry `@v0.84.1` plus what the same line is at the ported tag. Comment-only. |
| ~~SEAM-086~~ | ~~low~~ **FILED AND CLOSED 2026-08-14** | parity-bug | S | An `extension_ui_response` with a missing or non-string `id` was answered with an `Unknown command: extension_ui_response` error response where pi writes nothing — **FIXED 2026-08-14 (ext-rpc surface enumeration)**: the intercept now keys on the `type` discriminant alone and always `continue`s, as pi's unconditional `return` does (`rpc-mode.ts:763-777` @v0.83.0). Test `rpc_malformed_extension_ui_response_is_swallowed_not_answered` (`crates/cyrup-modes/src/tests/modes.rs`), RED before. |

**43 items — 0 critical, 8 high, 20 medium, 15 low.** (SEAM-071, SEAM-072 and SEAM-073 were added by
the later suite-verification pass, SEAM-072 closed on arrival; the 40/7/19/14 counts below predate
them.) Per structural defect A in
`00-residual-ledger.md`, treat the count as a floor — and see this file's own *Not audited* paragraph,
which names the inner RPC payload shapes as the largest unswept surface remaining.

## Trackers (excluded from the item count)

Keeps its ID and its body; proposes no schedulable work today.

| ID | Kind | Note |
|---|---|---|
| SEAM-058 | tracking | pi's experimental `server`/`client` command tree, `create-harness.ts` and `remote-session.ts`. Its own Fix is "track, do not build, until upstream wires it into `main()`", and the reachability re-check confirms upstream has not: at v0.84.1 `git grep -n experimentalCli` matches only the file itself and its test. The **action** it owes is a re-diff at the next upstream tag (its Verify line), not an implementation. Escalate it back into the counted set the moment `main()` references `experimentalCli`. |

## SEAM-047 — First SIGTERM/SIGHUP neither tears down nor exits 143/129; `--mode rpc` keeps running forever and never emits session_shutdown

**Kind** parity-bug · **Severity** high · **Effort** M · **Confidence** **confirmed — reproduced in the shipped binary** · **observed 2026-08-13** (headless-binary; [`REPRO-LOG.md`](REPRO-LOG.md)) · **FIXED 2026-08-13**

> **FIXED 2026-08-13.** `crates/cyrup/src/signals.rs` is now a per-host port of pi's three
> `registerSignalHandlers` sites. `spawn_abort_on_signal(runtime, cancel, host)` takes the
> `Arc<AgentSessionRuntime>` and the resolved `AppMode`; on the FIRST SIGTERM/SIGHUP in a
> non-interactive host it aborts the CURRENT session, fires `cancel`, `await runtime.dispose()`
> (the `session_shutdown{quit}` emission — pi `runtimeHost.dispose()`, print-mode.ts:57 /
> rpc-mode.ts:733) and `process::exit(143|129)` (print-mode.ts:52-62, rpc-mode.ts:374). The repeat
> watcher is armed *concurrently* with that dispose, which is pi's re-entrancy guard
> `if (shuttingDown) process.exit(exitCode)` (rpc-mode.ts:723-726). Interactive keeps the cancel
> token as its whole handler because its exit is the run loop's (`main.rs` disposes and returns 0,
> matching interactive-mode.ts:3559-3580) — exiting from the watcher would race the terminal restore.
>
> **Test** `crates/cyrup/tests/signal_shutdown.rs` — drives the real binary in `--mode rpc`, waits
> for a `get_state` round-trip so the serving loop is provably up, delivers ONE signal, and requires
> exit 143 (SIGTERM) / 129 (SIGHUP) within 15 s. **RED before** (probe: first delivery restored to
> "no exit" → `still alive 15s after the FIRST -TERM — SEAM-047`, both cases), **GREEN after**
> (`2 passed`). `cargo test -p cyrup` → 19 targets, 0 failed.
>
> **Two riders closed with it, as their own entries instruct.** `SEAM-059` — the watcher no longer
> holds the startup session `Arc`; it does `runtime.session().await.abort()`, pi's always-current
> dereference. `DRIFT-049` (duplicate-of this item) is closed by the same change.
> **NOT closed:** `SEAM-S03` — no detached-child registry exists to drain (pi's
> `killTrackedDetachedChildren()` has no cyrup counterpart, and `cyrup-tools` is outside this
> group's file ownership), so the handler cannot yet do pi's synchronous first step. **`SEAM-008`'s
> Fix is now the wrong mechanism** — see its entry.

> **Reproduced 2026-08-13, exit codes as evidence.** A live `cyrup --mode rpc` (stdin held open on a
> fifo) absorbs the first SIGTERM **and** the first SIGHUP completely — still running 15 s later,
> requiring SIGKILL. A **second** SIGTERM exits **143**, which is precisely this item's claim that
> `ShutdownSignal::exit_code` is consulted only on the second delivery. stdout was byte-empty across
> every run, so no `session_shutdown` (nor anything else) is emitted on the way out under either
> delivery. Nothing in the item needed correcting.

**cyrup** — `cyrup/crates/cyrup/src/signals.rs:88-101` — `spawn_abort_on_signal`'s first-signal body is exactly `session.abort(); cancel.cancel();` (`:94-95`). The exit codes it does compute (`ShutdownSignal::exit_code`, `:26-32` → 130/143/129) are used ONLY on the second delivery at `:98-99`. The `CancelToken` it fires is created at `cyrup/crates/cyrup/src/main.rs:367` and is observed by the INTERACTIVE arm only (passed into the run loop at `main.rs:568`); the RPC arm hands a clone to the watcher at `main.rs:670` and the print/json arm at `main.rs:781`, and neither `run_rpc` (`cyrup/crates/cyrup-modes/src/rpc.rs:575-579`) nor `run_print`/`run_json` (`print.rs:58-68`, `json.rs:47-51`) takes a cancel token at all. `rpc_driver`'s `select!` (`rpc.rs:717-842`) has no cancellation arm: its only exits are the extension-driven `shutdown_requested` checkpoint (`:845-849`) and `!reader_open && !in_flight && dispatches.is_empty()` (`:851`), neither reachable from a signal. So `runtime.dispose()` at `cyrup/crates/cyrup/src/run.rs:113` never runs and no `session_shutdown` is dispatched. In print/json the send loop (`print.rs:83-96`) simply proceeds to the NEXT `--follow-up` message after the abort and the process exits with the transcript-derived 0/1/130 from `run.rs:133-148`.

**upstream** — `pi/packages/coding-agent/src/modes/rpc/rpc-mode.ts:366-380` — `registerSignalHandlers` installs SIGTERM (plus SIGHUP off Windows) handlers whose body is `killTrackedDetachedChildren(); void shutdown(signal === "SIGHUP" ? 129 : 143, signal);`; `shutdown` (`:724-741`) unsubscribes, `await runtimeHost.dispose()` (`:734` — which emits `session_shutdown{reason:"quit"}`, `agent-session-runtime.ts:398-405`), pauses stdin, conditionally flushes, then `process.exit(exitCode)` (`:740`). `pi/packages/coding-agent/src/modes/print-mode.ts:50-68` is the same shape: `killTrackedDetachedChildren(); void disposeRuntime().finally(() => process.exit(signal === "SIGHUP" ? 129 : 143));`. Both read at v0.84.1.

**Impact** — A supervisor, CI runner, container stop or `timeout(1)` cannot stop `cyrup --mode rpc`: the first SIGTERM is absorbed and the process lingers holding the session file and any child processes until it is signalled a second time or SIGKILLed. No `session_shutdown` reaches extensions on that path, so intercom deregistration, permission-store teardown and subagent background-run cleanup never run. In print/json a SIGTERM mid-turn silently promotes to "run the next follow-up", and the exit code reports the transcript, so a killed run is indistinguishable from a clean one.

**Fix** — Give `run_rpc`/`run_print`/`run_json` the shutdown signal. Change `spawn_abort_on_signal` (`signals.rs:88`) to publish the `ShutdownSignal` through a `watch`/`oneshot` alongside firing `cancel`; add a `cancel.cancelled()` select arm to `rpc_driver`'s loop (`rpc.rs:717-842`) that sets `reader_open = false` so the existing drain-and-break at `:851` runs, and a between-message check in `run_print`/`run_json`'s send loops; have `run_print_dispatch`/`run_json_dispatch`/`run_rpc_dispatch` (`run.rs:22`, `:50`, `:101`) return 143/129 when a signal was the cause, after `runtime.dispose()`. Pairs naturally with SEAM-S03 (drain the detached-child registry in the same handler, synchronously, as pi does) and with SEAM-059 (the handler must reach the CURRENT session, not the startup one).

**Verify** — Integration test spawning `cyrup --mode rpc`, sending SIGTERM, asserting the process exits 143 within a bounded wait and that a `session_shutdown` line was written to stdout first; repeat with SIGHUP asserting 129. Second test: `cyrup -p 'a' --follow-up 'b'`, SIGTERM during the first turn, assert `'b'` was never submitted and the exit code is 143.

## SEAM-065 — Trust is resolved pre-launch, inverting pi's tier order, so an extension `project_trust` verdict is skipped entirely

**Kind** parity-bug · **Severity** high · **Effort** M · **Confidence** confirmed (both sides re-read in the repair pass)

**cyrup** — `cyrup/crates/cyrup/src/main.rs:325-329` calls `resolve_startup_ui` **before** any runtime is built (the builder runs at `:509`/`:652`/`:761`). Inside it, `main.rs:1142-1162` resolves trust from the store plus the default policy only, runs `run_trust_prompt`, and on any chosen option sets `config.trust_override = Some(trusted)` (`:1159`). That override then short-circuits the extension pass in the builder: `cyrup/crates/cyrup-session-svc/src/builder.rs:495-499` is `let ext_trust = if cfg.trust_override.is_none() && has_resources { pre_trust_extension_verdict(...).await } else { None };` — read verbatim at HEAD. So whenever the user answers the prompt, `pre_trust_extension_verdict` never runs and `decide_trust_with_extension` (`:512-522`) is handed `None`. The extension hook only gets a say when the user **cancels** the prompt.

**upstream** — `pi/packages/coding-agent/src/core/project-trust.ts:46-95` @v0.83.0 (identical at v0.84.1), read in full this pass — `resolveProjectTrusted` orders the tiers explicitly: `trustOverride` (`:47`), no-trust-requiring-resources (`:50`), then **`emitProjectTrustEvent(extensionsResult, …)` at `:54-70`**, whose `if (result) { … return trusted; }` returns before anything else and persists when `result.remember === true`. Only if no extension answered does it read the store (`:72-75`), then the default policy (`:77-84`), then `hasUI` (`:86-88`), and only last `selectProjectTrustOption` → `ctx.ui.select` (`:90-94`). pi reaches the prompt from *inside* `createAgentSessionServices`' `resolveProjectTrust` callback (`main.ts:687-706`), i.e. after `extensionsResult` exists — which is what makes the extension-first order possible.

**Impact** — An extension implementing `on-project-trust` (declared in `crates/cyrup-ext-sdk/wit/world.wit:237`) is defeated on the interactive path: cyrup asks the human first and the human's answer wins, suppressing the hook. A policy extension that would have returned `trusted: no` for a folder cannot stop the user selecting "Trust"; one that would have auto-approved a known-good folder still forces a prompt; and the `remember` half never fires. This is the pre-trust extension seam the builder was written for, dead on the path that matters. Reachability caveat, stated because it bounds the severity: no extension in-tree implements the hook today, so the bypass is latent rather than live — it becomes live the first time anyone ships a trust-policy extension, and the failure is silent when it does.

**Fix** — Do not resolve trust pre-launch. Delete the trust block from `resolve_startup_ui` (`main.rs:1142-1162`) and give `SessionServiceBuilder` a prompt callback (`with_trust_prompt(impl Fn(&[TrustOption], &Option<TrustEntry>) -> Option<usize>)`) that `builder.rs` invokes only when `decide_trust_with_extension` yields `TrustOutcome::NeedsPrompt` — i.e. after `pre_trust_extension_verdict`. The bin supplies `run_trust_prompt` as that callback. This also retires the builder's `saved: None` (`builder.rs:516`) and the "no trust store is wired" warning at `:506-510`, because the bin's `TrustStore` then reaches the same site. **Lands with SEAM-064**, which changes the option set the same callback renders.

**Verify** — `cargo test -p cyrup-session-svc` with a native extension returning `{trusted:false, remember:true}`: assert the prompt callback is NEVER invoked and the store receives the entry. Then a live run in a folder with `.cyrup/` resources plus such an extension — cyrup must start untrusted with no prompt on screen. Per the standing rule, the live run is required, not optional: this is a TTY surface.

## SEAM-064 — The pre-launch trust prompt omits both "(this session only)" options, so every trust answer is written to `trust.json` permanently

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** confirmed (both sides re-read in the repair pass) · **observed 2026-08-13** (live-terminal; [`REPRO-LOG.md`](REPRO-LOG.md)) · **FIXED 2026-08-13**

> **FIXED 2026-08-13.** `crates/cyrup/src/main.rs` now calls
> `cyrup_config::trust::trust_options(&dirs.cwd, true)` at the pre-launch site, with a comment citing
> pi `project-trust.ts:32` (`getProjectTrustOptions(cwd, { includeSessionOnly: true })`) and warning
> the next reader NOT to make the same change at `cyrup-session-svc/src/session.rs:3255`, which is
> pi's in-app selector and genuinely passes the default `false` (trust-selector.ts:44). One literal;
> `trust.rs` was already correct on both sides of the flag.
>
> **Test** `startup_ui.rs` gains `pre_launch_trust_prompt_offers_pi_five_rows_and_session_only_writes_nothing`:
> asserts pi's five labels in pi's order, that rows 2 and 4 carry an EMPTY `updates` and no
> `saved_path` (so `set_many` writes nothing — pi's `if (result.updates.length > 0)` guard,
> project-trust.ts:40-44), that `interpret_trust` still maps those indices to `Chosen`, and — as the
> control that keeps it from being a blanket disarm — that rows 0/1/3 still persist.
>
> **Honest limit on the evidence.** That test pins the OPTION-SET contract; it cannot reach the
> `main.rs` argument, because the prompt is a `CrosstermBackend` surface with no unit-test seam. The
> production line is verified by inspection only. **A live pty run must still show:** five rows in a
> folder with `.cyrup/` resources; Enter on "Trust (this session only)" → the session runs trusted
> and `<agent_dir>/trust.json` is **not created**; Enter on "Do not trust (this session only)" →
> untrusted, still no file; Enter on "Trust" → the file appears, exactly as the 2026-08-13 repro
> measured for the three-row prompt.

> **Reproduced 2026-08-13 on a real pty.** The startup prompt renders exactly **three** rows — Trust
> / Trust parent folder (`<parent>`) / Do not trust — with no session-only variant of either answer.
> The consequence was measured too, not inferred: pressing Enter on "Trust" writes
> `<agent_dir>/trust.json` containing `{"<cwd>": true}`, while cancelling with ESC leaves no
> `trust.json` at all. Cancel is therefore the only non-persisting exit, and it does not grant trust.
> Nothing in the item needed correcting.

**cyrup** — `cyrup/crates/cyrup/src/main.rs:1155` — `let options = cyrup_config::trust::trust_options(&dirs.cwd, false);`, fed straight into `cyrup::run_trust_prompt(&theme, &dirs.cwd, &options, &saved, &trust_store)` at `:1157`. The second parameter is `include_session_only` (`cyrup/crates/cyrup-config/src/trust.rs:336`), and it is exactly what gates both ephemeral rows: `"Trust (this session only)"` with `updates: Vec::new()` (`:356-363`) and `"Do not trust (this session only)"` (`:370-377`) — verified verbatim. With `false` the prompt renders three rows (Trust / Trust parent folder / Do not trust), every one of which carries a non-empty `updates`, which `run_trust_prompt` persists unconditionally via `trust_store.set_many(&option.updates)` (`cyrup/crates/cyrup/src/startup_ui.rs:266-268`). The unit tests at `startup_ui.rs:505` and `:519` pin the wrong arity. `false` is passed at **every** call site in the workspace (`main.rs:1155`, `cyrup-session-svc/src/session.rs:3255`), so the two options are unreachable code.

**upstream** — `pi/packages/coding-agent/src/core/project-trust.ts:32` @v0.83.0 (identical at v0.84.1), read this pass: the pre-launch path is `selectProjectTrustOption`, and it calls `getProjectTrustOptions(cwd, { includeSessionOnly: true })`. `core/trust-manager.ts:82-84` and `:91-93` then append `{ label: "Trust (this session only)", trusted: true, updates: [] }` and its negative twin — options with an empty `updates`, which `saveProjectTrustPromptResult` skips writing (`project-trust.ts:40-44`, `if (result.updates.length > 0)`). The contrast that makes this precise: pi's **in-app** `TrustSelectorComponent` calls `getProjectTrustOptions(options.cwd)` with no flag (`modes/interactive/components/trust-selector.ts:44`), so cyrup's `session.rs:3255` passing `false` is **correct** — only the pre-launch site is wrong.

**Impact** — A user who wants to run once in an untrusted folder without recording a verdict has no way to say so. Every answer at the startup trust prompt writes a permanent entry to `<agent_dir>/trust.json` that silently governs all future runs in that folder — including "Do not trust", which then locks the folder out with no prompt offered to reverse it. This is a security prompt whose two least-committal choices were dropped, on the one surface where a user is most likely to be answering about someone else's repository.

**Fix** — Change `main.rs:1155` to `trust_options(&dirs.cwd, true)`. Update the two tests at `startup_ui.rs:504-537` to build with `true` and assert the five-option order (Trust, Trust parent, Trust session-only, Do not trust, Do not trust session-only) and that `interpret_trust` on a session-only index yields `Chosen` with an **empty** `updates`, so `set_many` writes nothing. Leave `session.rs:3255` at `false` — it matches pi's in-app selector. One line of production change; the test update is the work.

**Verify** — `cargo test -p cyrup startup_ui`, then a live run: in a folder with a `.cyrup/` resource and no saved decision, launch `cyrup`, confirm the prompt lists five rows, pick "Trust (this session only)", and confirm `<agent_dir>/trust.json` is unchanged while the session runs trusted.

## SEAM-063 — Session delete permanently unlinks the JSONL where pi routes through the `trash` CLI first, and the failure is swallowed

**Kind** parity-bug · **Severity** high · **Effort** M · **Confidence** **confirmed — reproduced in the shipped binary** · **observed 2026-08-13** (live-terminal; [`REPRO-LOG.md`](REPRO-LOG.md))

> **Both halves reproduced 2026-08-13 on a real pty.** With an executable stub `trash` first on
> `PATH`, cyrup **never invokes it** (no log line) and unlinks the JSONL permanently. With the
> sessions directory `chmod 555` so `remove_file` must fail, the row still vanishes from the list, no
> error is shown or printed, and **the file survives** — a failed delete is visually identical to a
> successful one.
>
> **One correction to the Impact below.** The item says cyrup "additionally reports success
> unconditionally". That is accurate for the **in-app** `/resume` path as it stood then (the `C::DeleteSession` arm,
> `app/execute_session.rs:15-34` at HEAD, printed "deleted session"), but the **pre-launch** `--resume` picker measured here prints *nothing at all* on
> delete — no "Session deleted", no "moved to trash", no failure text; the only feedback is the row
> disappearing. On that surface the defect is a **missing status line as well as** a swallowed error,
> so the Fix must give `startup_ui.rs`'s `on_apply` a status channel for pi's
> `result.method === "trash" ? "Session moved to trash" : "Session deleted"` (`session-selector.ts:846`)
> and `"Failed to delete: …"` (`:849`) to render into. The upstream half was re-read at v0.84.1 this
> pass and matches the item verbatim.

**cyrup** — Two sites, same defect. Pre-launch picker: `cyrup/crates/cyrup/src/startup_ui.rs:133-137` — `if let Some(SessionSelectorOutcome::Delete(path)) = … { let _ = std::fs::remove_file(&path); }`, verbatim at HEAD. Permanent, and the `let _` discards the `io::Result`, so a failed delete produces no message while the row has already vanished from the list (`cyrup-tui/src/session_selector.rs:780`). In-app `/resume`: the `C::DeleteSession` arm (`cyrup/crates/cyrup-tui/src/app/execute_session.rs:15-34` at HEAD) → `session.delete_session_file(path)` → `cyrup/crates/cyrup-session-svc/src/session.rs`, also a bare `std::fs::remove_file`, and the arm printed "deleted session" whether or not the file went. `rg -ni 'trash' crates` returned **zero** hits workspace-wide when this was filed. *(All three clauses are FILING text and are false at HEAD: `delete_session_file` returns a `DeleteMethod` (`session.rs:3855`; the enum and pi's own status strings at `:5876-5895`), and the arm renders `method.status_message()` at `execute_session.rs:25-28` and `Failed to delete: {e}` at `:29-32`.)*

**upstream** — `pi/packages/coding-agent/src/modes/interactive/components/session-selector.ts:645-680` @v0.83.0 (identical at v0.84.1), read in full this pass — `deleteSessionFile` runs `spawnSync("trash", trashArgs)` **first**, with a `["--", path]` guard for leading-dash paths (`:649`); treats `status === 0` **or** the file having disappeared as success with `method: "trash"` (`:666-668`); only then falls back to `await unlink(sessionPath)` with `method: "unlink"` (`:672-674`); and on failure returns `{ok:false, error}` carrying both the unlink message and a `trash: …` hint (`:675-679`). The caller reports which happened — `result.method === "trash" ? "Session moved to trash" : "Session deleted"` (`:846`), or `"Failed to delete: …"` (`:849`). This path is live in the **pre-launch** picker too: `onDeleteSession` is assigned unconditionally in the constructor (`:832`), unlike `onRenameSession`.

**Impact** — For every user who has `trash` installed (macOS `brew install trash`, Linux `trash-cli`), pi's session delete is recoverable from the OS trash and cyrup's is irreversible. One confirmed keypress in the `--resume` picker or in-app `/resume` destroys a whole conversation JSONL with no undo, on a surface whose sibling operation (rename, SEAM-062) is a no-op — so the destructive action is the one that works. cyrup additionally reports success unconditionally, so a delete that failed on a read-only volume looks identical to one that succeeded.

**Fix** — *(Scope note added 2026-08-13 from the live run: the pre-launch picker has **no status line
at all**, so this fix must add one, not merely correct one.)* Add a `delete_session_file(path) -> Result<DeleteMethod, String>` helper — `std::process::Command::new("trash")` with pi's `--` guard, success on exit-0 **or** `!path.exists()`, else `std::fs::remove_file` — and call it from **both** `crates/cyrup/src/startup_ui.rs:133-137` and `crates/cyrup-session-svc/src/session.rs:3343-3347`. Propagate the method into the status message so `app/extension_ui.rs:108` can say "Session moved to trash" vs "Session deleted", and stop discarding the error at `startup_ui.rs:136`.

**Verify** — Unit-test the helper with a stub `trash` on `PATH`: success → `Trash`; exit-1 with the file still present → falls back to unlink; unlink failure → `Err`. Then a live run — with `trash` installed, delete a session from `cyrup --resume`, confirm the status line says "moved to trash" and the file is in the OS trash, not gone.

## SEAM-061 — The `--resume` picker merges current-folder and all-projects sessions into one list, labels it "Current Folder", and advertises a `tab scope` toggle that does nothing

**Kind** parity-bug · **Severity** high · **Effort** M · **Confidence** **confirmed — reproduced in the shipped binary** · **observed 2026-08-13** (live-terminal; [`REPRO-LOG.md`](REPRO-LOG.md))

> **Reproduced 2026-08-13 on a real pty, every clause.** Two project dirs; from `projA` the picker
> headed "Resume Session (Current Folder)" lists `projB`'s session alongside `projA`'s with the cwd
> column off. Tab is **completely inert** — not merely unbound but producing *no redraw at all* (the
> raw pty log shows the redraw emitting only `ESC[39m ESC[49m ESC[59m ESC[0m ESC[?25l` and no cell
> changes) — while the hint row prints `tab scope` and the empty state prints "No sessions in current
> folder. Press Tab to view all." Selecting the foreign row **resumes `projB`'s session with the
> footer cwd still `projA`** and no warning, so the item's impact claim is now measured rather than
> inferred.
>
> **One screen element the paragraphs below omit, added because it makes the misreport worse and is
> the first thing to fix:** the header also renders a scope **radio** — `◉ Current Folder | ○ All` —
> to the right of the title. The UI does not merely *name* the wrong scope in prose, it draws a
> two-state control showing "Current Folder" selected, beside a `tab scope` hint, over a list that is
> already both scopes merged.



**cyrup** — `cyrup/crates/cyrup/src/main.rs:1259-1268` — `gather_session_infos` runs `list_in_dir(cwd-layout)` and then appends every row from `list_global_sessions(dirs)` (`:1244-1252`, the cross-project `list_all(SessionsRoot)`), de-duplicated by path, into **one** `Vec<SessionInfo>`; read verbatim at HEAD, including the doc comment that names both of pi's loaders as its source. `main.rs:1096` hands that single vector to `cyrup::run_resume_picker(&theme, &sessions, None)`, which builds `SessionSelector::new(rows)` (`cyrup/crates/cyrup/src/startup_ui.rs:126-127`). `SessionSelector` defaults to `scope: SessionScope::Current` (`cyrup/crates/cyrup-tui/src/session_selector.rs:204`), so the header renders `"Resume Session (Current Folder)"` (`:561-563`) over a list containing every project's sessions, with the cwd column off (`show_path: false`, `:199`; the cwd renders only `if self.show_path`, `:456`). There is no `SessionAction::ToggleScope` in the keymap at all (`:825-837` is ToggleSort/ToggleNamedFilter/TogglePath/Delete/Rename), so Tab falls through `handle` (`:770-843`) inert — while the hint row unconditionally prints `"tab scope"` (`:674`) and the empty state prints `"No sessions in current folder. Press Tab to view all."` (`:426`). The crate concedes it at `:49-52`: the Tab toggle "needs an all-sessions loader, which the cyrup chrome does not hand the selector yet". `SessionListProgress` exists and is unused by the bin (`cyrup-session/src/listing.rs:40-41,57`; `main.rs:1261` passes `None`).

**upstream** — `pi/packages/coding-agent/src/cli/session-picker.ts:15-19` @v0.83.0 (byte-identical at v0.84.1), read in full this pass — `selectSession(currentSessionsLoader, allSessionsLoader, settingsManager)` takes **two** loaders, each `(onProgress?: SessionListProgress) => Promise<SessionInfo[]>` (`:12`), and passes both to `SessionSelectorComponent` (`:26-28`). `main.ts:419-421` supplies `SessionManager.list(cwd, sessionDir, onProgress)` and `SessionManager.listAll(sessionDir, onProgress)`. The component starts at `scope: "current"` (`modes/interactive/components/session-selector.ts:704`), loads only the current set on construction (`:859`), and Tab calls `onToggleScope` (`:551-556`) → `toggleScope()`, which lazily loads the all-set. `showCwd` is strictly `this.scope === "all"` (`:844`), and the title switches to `"Resume Session (All)"` (`:131`).

**Impact** — On any machine with more than one cyrup project, `cyrup --resume` shows a picker headed "Current Folder" that actually lists every session on disk, with no cwd column to tell them apart and rows labelled only by their first message. Picking a foreign row resumes another project's session (`main.rs:1124` sets `SessionTarget::Resume(path)` with no cwd guard) — the user has no on-screen way to notice, and the one control the UI tells them to use is dead. The screen actively misreports its own contents, which is worse than either half alone. Secondary: every `--resume` pays a full cross-project scan synchronously before the terminal is even entered, with no progress feedback, where pi scans one directory and streams progress.

**Fix** — Give `run_resume_picker` both listings instead of a merged one: change the signature to `run_resume_picker(theme, current: &[SessionInfo], all: &dyn Fn() -> Vec<SessionInfo>, current_id)`, keep `gather_session_infos`' local half for the initial rows and defer `list_global_sessions` to the toggle. Add `SessionAction::ToggleScope` to the session keymap (bound to Tab, matching `tui.input.tab`), handle it in `SessionSelector::handle` by calling `set_scope` + `set_rows` and flipping `show_path` to `scope == All` (pi's `showCwd`), and make the `"tab scope"` hint conditional on the toggle actually being armed. **Added 2026-08-13 from the live run:** `ctrl+p path (off)` exists today as a *manual* cwd-column toggle, whereas pi derives `showCwd` **strictly** from `scope === "all"` (`session-selector.ts:844`) — so `show_path` must be made to *follow the scope*, not merely be armable alongside it, and the header's `◉ Current Folder | ○ All` radio must be driven by the same state. Thread `SessionListProgress` into the initial load so the header can render pi's `loaded/total`. **Handoff:** the `SessionSelector` half is area 07's file; the loader/plumbing half is this area's. Land them together — either half alone leaves the screen lying.

**Verify** — `cargo test -p cyrup-tui session_selector` for the scope/`show_path` unit assertions, then a **live run**: create sessions in two project dirs, `cd` into the first, run `cyrup --resume` in a real terminal, confirm only the first project's sessions appear under "Resume Session (Current Folder)", press Tab and confirm the header flips to "Resume Session (All)", the other project's sessions appear, and the cwd column turns on. TestBackend alone does not close this.

## SEAM-062 — The pre-launch `--resume` picker offers rename, shows the new name on screen, and silently discards it

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** **confirmed — reproduced in the shipped binary** · **observed 2026-08-13** (live-terminal; [`REPRO-LOG.md`](REPRO-LOG.md))

> **Reproduced 2026-08-13 on a real pty, exactly as filed.** The picker advertises `ctrl+r rename`,
> enters rename mode with its own hint row ("enter to save · escape/ctrl+c to cancel"), accepts the
> typed text, and on Enter repaints the row's label as `NEWNAME` — complete positive feedback. The
> session JSONL is untouched (`grep -c NEWNAME` = 0, no `session_info` line appended) and a relaunch
> of the same picker shows the original label. Upstream re-read at v0.84.1: `cli/session-picker.ts:48`
> constructs the component with `{ showRenameHint: false, keybindings }` and passes no
> `renameSession` callback, so pi's pre-launch picker cannot enter rename mode at all. Nothing in the
> item needed correcting.

**cyrup** — `cyrup/crates/cyrup-tui/src/session_selector.rs:214` — `show_rename_hint: true` is the constructor default (verified verbatim, with an in-tree comment citing pi's `showRenameHint ?? canRename`), and `cyrup/crates/cyrup/src/startup_ui.rs:126-127` never calls `set_show_rename_hint(false)`, so the pre-launch picker prints the `rename` hint (`session_selector.rs:689-692`). `SessionAction::Rename` is ungated (`:833-837`) and enters rename mode for the selected row. On Enter the row is mutated in place — `row.name = Some(name)`, `row.label = name` (`:798-801`) — and a `SelectorOutcome::Apply(rename_payload)` is returned (`:802`). `run_resume_picker`'s `on_apply` closure (`startup_ui.rs:129-138`) matches **only** `SessionSelectorOutcome::Delete`; the rename payload falls through and is dropped, which the doc comment at `:131-133` states outright ("Rename is deferred to the in-app `/resume` (no header-rewrite seam pre-launch)"). Nothing is written to the JSONL.

**upstream** — `pi/packages/coding-agent/src/cli/session-picker.ts:48` @v0.83.0 (byte-identical at v0.84.1), read in full this pass — `selectSession` constructs `SessionSelectorComponent` with `{ showRenameHint: false, keybindings }` and **no** `renameSession` callback. In the component, `this.canRename = !!renameSession` (`modes/interactive/components/session-selector.ts:771`) is therefore false, `setShowRenameHint` hides the hint (`:772`), and the handler bails before entering rename mode (`:807-808`). pi's pre-launch picker cannot enter rename mode at all.

**Impact** — A user renaming a session from `cyrup --resume` gets complete positive feedback — the hint invites it, the input accepts it, the row's label changes to what they typed — and the rename is never persisted. Relaunch and the name is gone. Same class as the already-filed TUI-027 (typed text accepted and thrown away), on a surface nobody had read. It also sits beside SEAM-063, where the *destructive* sibling operation on the same screen does work, so the picker's one reversible action is the broken one.

**Fix** — Stop-the-bleeding is two lines: call `selector.set_show_rename_hint(false)` in `run_resume_picker` and gate `SessionAction::Rename` on a new `SessionSelector::set_rename_enabled(bool)` (default true for the in-app `/resume`, false pre-launch), matching pi's `canRename`. Preferred full fix: handle `SessionSelectorOutcome::Rename` in `run_resume_picker`'s `on_apply` by opening the target with `cyrup_session::SessionManager::open(path)` and appending the `session_info` name — the exact sequence `cyrup-session-svc/src/session.rs:3355-3365 rename_session_file` already performs — so the pre-launch rename works like pi's in-app one. Either is acceptable; leaving the current shape is not.

**Verify** — Live run in a real terminal: `cyrup --resume`, press the rename key on a row, type a name, Enter, quit with Esc, then `cyrup --resume` again — either the hint and the mode must be absent (parity fix) or the name must survive the relaunch (full fix). A unit test asserting `handle` returns `Ignored` for the rename chord when rename is disabled pins the first half.

## SEAM-006 — print/json bind_extensions passes no onError sink and no mode label

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** high

> **Reopened this pass.** The `4935cc8` closure covered the host half only. The refuter re-read both
> sides and confirmed the residual, and re-rated the item from `high` to `medium`: control ops now
> work under print/json, so what remains is the error sink and the mode label, not the whole host.

**cyrup** — Closed half, verified at HEAD: `cyrup/crates/cyrup-modes/src/print.rs:58-68` and `json.rs:47-51` take `runtime: &AgentSessionRuntime`, and the send loops re-read `runtime.session().await` per message (`print.rs:86`, `json.rs:73`); the bin arm builds `AgentSessionRuntime::create_unannounced` at `cyrup/crates/cyrup/src/main.rs:761` and passes the runtime through at `:791-794` → `run.rs:22-26`, `:50-54`. **Residual**: `AgentSession::bind_extensions()` (`cyrup/crates/cyrup-session-svc/src/session.rs:2516`) takes no arguments — there is no place to hand it a mode label or an error callback — and `add_error_listener` has only two call sites workspace-wide, both RPC (`cyrup/crates/cyrup-modes/src/rpc.rs:670` and inside `rebind_session` at `:516`).

**upstream** — `rebindSession` passes THREE keys to `bindExtensions`: `mode: mode === "json" ? "json" : "print"`, `commandContextActions`, and `onError` (which does `console.error("Extension error (path): err")`). **Offsets differ between tags and the ported baseline governs**, so both are given, each read at its own tag rather than shifted: **v0.83.0** `pi/packages/coding-agent/src/modes/print-mode.ts:71-101` — `rebindSession` at `:71`, `bindExtensions` `:73-101`, `mode` `:74`, `commandContextActions` `:75-97`, `onError` `:98-100`. **v0.84.1** `:74-119` — `mode` `:77`, `commandContextActions` `:78-100`, `onError` `:101-103`. The code is the same; only the line numbers moved.

**Impact** — An extension that faults under `cyrup -p` or `cyrup --mode json` is contained and NEVER surfaced: nothing is written to stderr and nothing reaches the json event stream, so a broken extension looks like a silently degraded run. Separately, the extension context's mode label is unset, so an extension that branches on `ctx.mode` (to suppress interactive UI, for example) cannot tell print from json from interactive. `main.rs:678-794` is what a spawned subagent child re-execs into, so subagent runs inherit both.

**Fix** — Give `bind_extensions` the parameters pi's `bindExtensions` takes: add a `BindOptions { mode: &str, on_error: Option<ErrorListener> }`-shaped argument at `session.rs:2516`, thread the mode label into the extension context alongside the existing `commandContextActions` equivalent (`RuntimeActions`, already installed at `runtime.rs:428-430`), and have `run_print`/`run_json` install an error listener that writes pi's `Extension error (path): err` line to stderr before calling it. Keep the no-argument form as a thin default so the RPC and interactive call sites need no change.

**Verify** — A native extension whose `session_start` handler panics/returns `Err` under `cyrup --mode print`: assert the `Extension error (…)` line reaches stderr. A second extension that reports `ctx.mode`: assert it observes `"print"` under `-p` and `"json"` under `--mode json`.

## SEAM-008 — Signal identity and 143/129 are computed but used only on the second delivery

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** high

> **Corrected this pass.** The auditor raised this to `high`. The refuter kept it at `medium`
> because the "an RPC host that never returns" consequence is SEAM-047's and double-booking it
> distorts the plan. SEAM-008's remaining scope is the signal-identity/exit-code bookkeeping;
> SEAM-047 owns the teardown-and-exit path that consumes it.

**cyrup** — Closed half: `cyrup/crates/cyrup/src/signals.rs:39-69` `wait_for_signal` now selects on `tokio::signal::ctrl_c()`, `SignalKind::terminate()` and `SignalKind::hangup()`, and `ShutdownSignal::exit_code` (`:26-32`) returns 130/143/129. **Residual**: those codes are consumed only by the repeat-delivery arm at `signals.rs:98-99`. The first delivery (`:94-95`) is `session.abort(); cancel.cancel();` and discards the identity entirely — nothing downstream can tell SIGINT from SIGTERM from SIGHUP, and the process exit code still comes from the transcript-derived `exit_code` at `cyrup/crates/cyrup/src/run.rs:133-148` (0/1/130 only).

**upstream** — `pi/packages/coding-agent/src/modes/rpc/rpc-mode.ts:366-380` and `print-mode.ts:50-68` (v0.84.1) both compute `signal === "SIGHUP" ? 129 : 143` on the FIRST delivery and hand it to the teardown path, which exits with it.

**Impact** — A SIGTERM'd or hung-up cyrup reports the transcript's 0/1/130 rather than 143/129, so supervisors and CI cannot distinguish a killed run from a clean one on the first signal — only on a second one, which most supervisors never send before SIGKILL.

**Fix** — ~~Publish the `ShutdownSignal` from the first delivery: change `spawn_abort_on_signal` (`signals.rs:88-101`) to send it on a `watch`/`oneshot` before `cancel.cancel()`, and have the dispatchers in `run.rs:22`/`:50`/`:101` prefer that code over the transcript-derived one.~~

> **REWRITTEN 2026-08-13 — the old Fix is not pi's mechanism, do not implement it.** SEAM-047's fix
> landed and pi's own shape settles this: pi's signal handler **exits the process itself** with
> `signal === "SIGHUP" ? 129 : 143` (print-mode.ts:52-62, rpc-mode.ts:374/740) — the code never
> travels to a dispatcher, and neither `runPrintMode` nor `rpc-mode`'s normal return path ever sees
> it. `signals.rs` now does exactly that, so SIGTERM→143 and SIGHUP→129 are live on the first
> delivery in print/json/rpc and a `watch`/`oneshot` into `run.rs` would be an invented seam.
>
> **What actually remains** is one case: **SIGINT**. pi registers no `SIGINT` handler in any host
> (its Ctrl-C is a TUI key event, interactive-mode.ts:3539-3546), whereas cyrup's tokio `ctrl_c()`
> future necessarily intercepts the signal and keeps the graceful abort, so a `kill -INT` still
> exits with the transcript-derived code rather than 130. That is a stated `CYRUP-DELTA` in
> `signals.rs`, not the item as written. Re-scope or close this ID against the new code before
> scheduling it.

**Verify** — Unit test that `wait_for_signal` reports the right `ShutdownSignal` variant per signal (already partly covered at `signals.rs:111-116`), plus SEAM-047's end-to-end exit-code assertions.

## SEAM-011 — setWidget goes on the RPC wire with a cyrup-invented {widget} blob

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-modes/src/rpc.rs:423-428` emits `{"type":"extension_ui_request","id":…,"method":"setWidget","widget": widget}`. Root cause is the WIT, unchanged in BOTH copies: `cyrup/crates/cyrup-ext/wit/world.wit:326` and `cyrup/crates/cyrup-ext-sdk/wit/world.wit:326` are `set-widget: func(widget-json: string);`.

**upstream** — `pi/packages/coding-agent/src/modes/rpc/rpc-types.ts:264-271` pins `method:"setWidget"; widgetKey: string; widgetLines: string[] | undefined; widgetPlacement?: "aboveEditor"|"belowEditor"`. The whole `RpcExtensionUIRequest` union (`:238-273`) was re-read this pass: no member carries a `widget` key.

**Impact** — An RPC client written to pi's contract cannot render extension widgets at all: no `widgetKey` to key on, no `widgetLines` to draw, no placement. This remains the LAST divergent member of the union — `notify`, `setStatus` (with the omit-when-`None` `statusText` at `rpc.rs:404-416`), `setTitle` and `set_editor_text` all match field-for-field, and the three TUI-only effects correctly return `None`.

**Fix** — Widen `set-widget` in both WIT copies to `func(key: string, lines: option<list<string>>, placement: option<string>)`, thread the three fields through `cyrup-ext`'s effect type, and emit pi's field names at `rpc.rs:423-428`. This is a guest ABI break, same class as `f777e44`'s.

**Verify** — Invert `cyrup/crates/cyrup-modes/src/tests/modes.rs:1014-1019` (SEAM-028) to assert `widgetKey`/`widgetLines`/`widgetPlacement` and that no `widget` key is present.

## SEAM-012 — session_before_switch carries no reason, session_before_fork no position

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-ext/src/event.rs:332-333` declares `SessionBeforeSwitch { target_id: String }` / `SessionBeforeFork { entry_id: String }`; both WIT copies match at `wit/world.wit:240-241`. Emit sites are still lossy: `cyrup/crates/cyrup-session-svc/src/runtime.rs:461` passes `SessionBeforeSwitch { target_id: String::new() }` on the `new_session` path (empty sentinel, no reason), and `runtime.rs:525` passes only `entry_id` while the `position` parameter (`runtime.rs:521`) never reaches the event.

**upstream** — `pi/packages/coding-agent/src/core/agent-session-runtime.ts:133-148` (`reason: "new"|"resume"`, `targetSessionFile`) and `:150-161` (`entryId` plus the spread options, i.e. `position`); type declarations at `core/extensions/types.ts:577-589`.

**Impact** — A gate extension cannot distinguish "new session" from "resume" (both arrive with an empty target) and cannot tell a fork *before* an entry from a fork *at* it, so a policy permitting one and denying the other is unwritable.

**Fix** — Widen both hooks in both WIT copies and in `event.rs:332-333`, populate `reason`/`target_session_file` at `runtime.rs:461` and `position` at `runtime.rs:525`. Batch with SEAM-025 — all four session hooks are one WIT bump.

**Verify** — Recording native extension asserting `reason == "new"` on `--new`, `"resume"` on resume, and `position` matching the requested `ForkPosition`.

## SEAM-014 — RPC verb get_available_thinking_levels not implemented

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — The `SessionCommand` enum (`cyrup/crates/cyrup-modes/src/rpc.rs:84-212`) holds 31 real verbs plus `#[serde(other)] Unknown` (`:210-211`); the whole `handle` switch (`rpc.rs:1020-1381`) was read this pass and has no `GetAvailableThinkingLevels` arm. `grep -rn available_thinking_levels crates/cyrup-modes` returns nothing. The backing method is live and unused by RPC: `cyrup/crates/cyrup-session-svc/src/session.rs:3076`.

**upstream** — `pi/packages/coding-agent/src/modes/rpc/rpc-types.ts:39` declares the command; handler at `modes/rpc/rpc-mode.ts:507-510`; response shape `{levels}` at `rpc-types.ts:158-164`.

**Impact** — A client cannot enumerate which thinking levels the active model supports, so it must hard-code the list or offer levels the model will reject.

**Fix** — Add the variant to `rpc.rs:84-212` and a handler returning `{"levels": session.available_thinking_levels()}`.

**Verify** — Extend `modes.rs`'s command-surface test to assert the verb succeeds and `data.levels` is a non-empty array; re-run the verb-set diff against `rpc-types.ts:20-72` and expect it empty but for `unknown`.

## SEAM-015 — RPC bash ignores the operations backend override

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-modes/src/rpc.rs:1228-1234` calls `session.execute_bash_with_user_event(&command, BashOptions { exclude_from_context, id: bash_id }, None)`. The third argument is still a literal `None` at `:1232`, and the omission is recorded only in the source comment at `:1226-1227`.

**upstream** — `pi/packages/coding-agent/src/modes/rpc/rpc-mode.ts:558-578`: `emitUserBash` at `:559-564`, short-circuit on `eventResult?.result` at `:566-571`, else `session.executeBash(..., { excludeFromContext, id, operations: eventResult?.operations })` with `operations` at `:576`.

**Impact** — An extension cannot supply a remote-exec or sandbox backend for a single RPC bash call, so sandboxing extensions are inert on the RPC path while working elsewhere.

**Fix** — Add an optional per-call backend to `BashOptions` (an `Option<Arc<dyn BashOps>>`-shaped seam), populate it from the `user_bash` event result inside `execute_bash_with_user_event`, and pass it through at `rpc.rs:1232`. This is the seam `289c089`'s commit message deferred.

**Verify** — Native extension returning `operations` from `user_bash`; assert the RPC bash result came from the injected backend, not the local shell.

## SEAM-016 — print-mode exit code derived by reverse-scanning the transcript

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup/src/run.rs:133-148` — `exit_code` iterates `session.messages().await.iter().rev()` (`:135`) and returns on the FIRST `Message::Assistant`, while `cyrup/crates/cyrup-modes/src/print.rs:102-103` reads `transcript.last()` once. The two therefore disagree whenever the final message is not an assistant, which is reachable because `flush_pending_bash_messages` appends `Custom` bash messages after the assistant. Two mapping divergences in the same function: `StopReason::Aborted => 130` (`run.rs:139`) where pi uses 1, and — added this pass by the refuter — `StopReason::Pending => 1` (`run.rs:138`) where pi leaves the initialised `exitCode = 0`.

**upstream** — `pi/packages/coding-agent/src/modes/print-mode.ts:140-155` (v0.84.1) reads `state.messages[state.messages.length - 1]` ONCE (`:141`) for both output and exit code; `exitCode = 1` at `:147` covers error AND aborted; the `else` branch leaves the `exitCode = 0` initialised at `:35`.

**Impact** — `cyrup --mode print` can exit non-zero while printing nothing, or exit zero on a run whose visible output came from a stale assistant message. Scripts keying on the exit code misclassify runs that end with a bash message, and an aborted or still-pending run reports a code pi never emits.

**Fix** — Compute the last message once in `run.rs:133-148` (or have `print.rs` return the decision), matching `print-mode.ts:140-155`, and align the three `StopReason` arms with pi: error and aborted → 1, everything else → 0. The `Aborted ⇒ 130` mapping cites arch-11 §6.6 at `run.rs:132`; that spec is not in this workspace, so raise it before changing rather than assuming.

**Verify** — `grep -rn exit_code crates/cyrup/tests/*.rs` returns nothing today; add a case whose last message is a `Custom` bash message and assert exit 0 with no output, plus cases pinning aborted → 1 and pending → 0.

## SEAM-025 — Extension session_start/session_shutdown drop pi's session-file fields

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — Progress: the FACADE event now carries the field — `cyrup/crates/cyrup-session-svc/src/session.rs:2525` `emit_session_start(&self, reason, previous_session_file)` builds `AgentSessionEvent::SessionStart { reason, previous_session_file }` at `:2530-2534`, and every replacement path supplies it (`runtime.rs:446`). **Residual**: the EXTENSION event drops it — `session.rs:2537-2540` dispatches `HostEvent::SessionStart { reason: reason.to_string() }` only; `cyrup/crates/cyrup-ext/src/event.rs:307-308` is still `SessionStart { reason: String }` / `SessionShutdown { reason: String }`; both WIT copies are still `on-session-start: func(reason: string)` / `on-session-shutdown: func(reason: string)` at `wit/world.wit:234-235`; and `dispose`/`dispose_with` (`session.rs:2380`, `:2405`) still take no target parameter.

**upstream** — `pi/packages/coding-agent/src/core/extensions/types.ts:562-568` (`previousSessionFile?`) and `:616-621` (`targetSessionFile?`), populated at `agent-session-runtime.ts:171-174`, `:305`, `:328`, `:347`.

**Impact** — An extension observing a session replacement cannot tell WHICH session it came from or is going to, so transcript-linking, audit trails and intercom identity handoff across a switch/fork are impossible — even though the host now has the value in hand.

**Fix** — Widen both hooks in both WIT copies and `event.rs:307-308`, pass `previous_session_file` through the dispatch at `session.rs:2537-2540`, and add a target parameter to `dispose`/`dispose_with` (`session.rs:2380`, `:2405`) populated from the replacement caller. Batch with SEAM-012.

**Verify** — Recording extension asserting `previousSessionFile` on a switch-induced `session_start` and `targetSessionFile` on the paired `session_shutdown`.

## SEAM-027 — --mode json subscribes per-run, dropping between-prompt events

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-modes/src/json.rs:70-83` — the loop does `let session = runtime.session().await; let mut stream = session.prompt(input).await?;` per message and drains only that stream. `AgentSession::prompt` subscribes RUN-scoped (`cyrup/crates/cyrup-session-svc/src/subscriber.rs`, `subscribe_run`, cleared by `end_run`), so gaps between prompts are unobserved. The persistent `AgentSession::subscribe` is used only by `rpc.rs:644`. Partial progress: `run_json` now writes the header once (`json.rs:54-60`) and binds/announces once before the loop (`:67`), so the SEAM-006 host half is done — the event gap is not.

**upstream** — `pi/packages/coding-agent/src/modes/print-mode.ts:106-118` (v0.84.1) installs ONE session-wide `session.subscribe(...)` inside `rebindSession()`, which runs at `:129` before the initial prompt at `:132` and is held across the message loop at `:135-137`; torn down only in `disposeRuntime()`.

**Impact** — With `--follow-up`, any event emitted between runs (extension UI, `session_info_changed`, `model_changed`, background compaction progress) is silently dropped from the json stream, so a consumer sees an incomplete event log.

**Fix** — Install one `session.subscribe()` before the first prompt in `json.rs:67` and drain the persistent stream, terminating each message on `agent_settled`. Must re-subscribe on session replacement, so model it on `rebind_session` (`rpc.rs:505-518`) rather than subscribing once and holding.

**Verify** — A json-mode test with two `--follow-up` prompts and an extension emitting between them; assert the emitted event appears in the stream.

## SEAM-033 — RPC and interactive session_start still precedes --name and --models

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — Closed for print/json: `cyrup/crates/cyrup-session-svc/src/runtime.rs:292-314` `create_unannounced` is pi's `createAgentSessionRuntime` verbatim (it never binds); the bin uses it at `cyrup/crates/cyrup/src/main.rs:761` with `apply_post_build` at `:776`, and the announcement lives in the mode entry points (`print.rs:78`, `json.rs:67`). **Still open for RPC and interactive**: `main.rs:652` and `main.rs:509` both call `AgentSessionRuntime::create`, whose body (`runtime.rs:259-266`) is `create_unannounced(...)` then `this.session().await.bind_extensions().await` — so `session_start` fires BEFORE `apply_post_build` at `main.rs:668` (rpc) and `:519` (interactive), which is where `--name` and `--models` are applied (`apply_post_build`, `main.rs:816-862`).

**upstream** — `pi/packages/coding-agent/src/main.ts:650` (`appendSessionInfo(name)`) and `:742-750` (`scopedModels`) both precede `createAgentSessionRuntime` at `:793-798`, and pi's `createAgentSessionRuntime` (`agent-session-runtime.ts:414-432`) never emits `session_start` — the HOST does, from `rpc-mode.ts:319` / `print-mode.ts:76` / `interactive-mode.ts`.

**Impact** — Under `--mode rpc` or interactive, an extension's `session_start` handler observes a session with no display name (`get-session-name` returns `none`) and, when `--models` scoping applies to a fresh session, the pre-scope model and thinking level. An audit or intercom extension registering under the session name registers the empty name; a gate keying policy on the active model keys on the wrong one. The follow-on `session_info_changed`/`model_changed` events go to a fanout the RPC loop has not yet joined, so nothing on the wire corrects it.

**Fix** — Have the RPC and interactive arms take the same path print/json already does: call `create_unannounced` at `main.rs:652` and `:509`, run `apply_post_build` in the resulting window, and let the host announce afterwards via the idempotent `session.bind_extensions()` (latched at `session.rs:2526`). Delete `AgentSessionRuntime::create`'s announcing tail (`runtime.rs:259-266`) once no caller wants it, so the two shapes cannot drift apart again.

**Verify** — A recording native extension whose `SessionStart` handler captures `session_name()` and the active `ModelRef`; drive `cyrup --mode rpc --name X --models <pattern>` and assert the handler saw `X` and the scoped model. Today it sees `None` and the unscoped model.

## SEAM-048 — RPC get_commands enumerates the last-wins command HashMap; pi's name:N disambiguation tier is dead code — **CLOSED 2026-08-15 (FIXED — the routed residual was refuted; the real one was `native_command_names`)**

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

> **Corrected this pass.** The auditor filed this as "the catalog and the dispatcher use two
> different functions". The refuter checked and the mechanism is worse and simpler: `grep -rn
> resolved_command_owner crates/` returns ONLY its definition and three assertions in
> `crates/cyrup-ext/src/tests/aggregation.rs:236-238` — no production caller. Live dispatch uses the bare
> last-wins `command_owner` (`facade.rs:328`, `facade.rs:1220`, `session.rs:1036`, `session.rs:1274`).
> So a `name:2` spelling would not be accepted by the dispatcher either; pi's whole disambiguation
> tier (`registry.rs:462-512`) is dead code outside tests. The fix must wire it into both sides.

**cyrup** — `cyrup/crates/cyrup-modes/src/rpc.rs:1369-1373` (`GetCommands` → `session.slash_command_catalog()`) → `cyrup/crates/cyrup-session-svc/src/session.rs:2275-2295`, whose extension section iterates `self.services.ext_host.registry().command_descriptions()`. That method (`cyrup/crates/cyrup-ext/src/registry.rs:662-669`) reads `RegistryInner::commands`, a `HashMap<String,(ExtensionId,CommandDescriptor)>` (`registry.rs:139`) that `register_command` populates with `g.commands.insert(name.clone(), …)` (`:452`) — LAST WINS, and iteration order is nondeterministic. The faithful port of pi's disambiguation exists three functions above and has no production caller: `resolved_commands()` (`registry.rs:462-500`) walks the duplicate-preserving `command_order` vec (`:141-142`, pushed at `:453`) and assigns `name:1`/`name:2` with the `takenInvocationNames` bump loop. `command_names()` (`registry.rs:655-658`) carries the identical defect.

**upstream** — `pi/packages/coding-agent/src/modes/rpc/rpc-mode.ts:680-687` builds each `RpcSlashCommand` from `command.invocationName`, sourced from `getRegisteredCommands()` → `resolveRegisteredCommands()` (`core/extensions/runner.ts:598-641`), which counts duplicates, assigns `${command.name}:${occurrence}` when `count > 1`, bumps on collision, and returns commands in EXTENSION LOAD ORDER (`for (const ext of this.extensions) for (const command of ext.commands.values())`, `runner.ts:602-607`). `getCommand(name)` matches on `invocationName` (`runner.ts:648`).

**Impact** — When two loaded extensions register the same command name, the second one's command is unreachable and invisible: the RPC client is told the command exists once under its bare name, invoking it reaches whichever extension registered last, and no `name:2` spelling is either advertised or accepted. Independently, the `get_commands` list order shuffles between runs, so a client rendering a command palette shows a different order each launch and payload diffs are noise. `command_descriptions()` also backs the TUI command list, so the defect is not RPC-only.

**Fix** — Make `resolved_commands()` the single source of truth on both sides. Change `command_descriptions()` (`registry.rs:662-669`) and `command_names()` (`:655-658`) to delegate to it and return `(invocation_name, descriptor)` in `command_order` order, and switch the four dispatch sites (`facade.rs:328`, `facade.rs:1220`, `session.rs:1036`, `session.rs:1274`) from `command_owner` to `resolved_command_owner` so the advertised name is the accepted name.

**Verify** — Register two native extensions that each declare a command named `check`; drive `{"type":"get_commands"}` and assert the response contains BOTH `check:1` and `check:2` in load order, and that `{"type":"prompt","message":"/check:2"}` reaches the second extension. Add a determinism assertion that two consecutive `get_commands` calls return identical arrays.

## SEAM-049 — Forking before the first message drops pi's parentSession link on both persistence paths

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session-svc/src/runtime.rs:581` — the no-anchor arm of `fork` is `(None, _) => self.factory.build(SessionTarget::New, None).await?.into_shared()`. `SessionFactory::build` (`cyrup/crates/cyrup-session-svc/src/factory.rs:99-105`) is defined as `self.build_with_parent(target, cwd, None)`, and `build_with_parent` (`factory.rs:110-118`) is the ONLY path that sets `cfg.parent_session` (`:118`). The runtime already holds the outgoing file — it bound it into `previous` at `runtime.rs:529` for the `session_start` event at `:584` — and it already uses `build_with_parent` correctly on the `new_session` path (`runtime.rs:466-470`). The fork path has the value in hand and discards it.

**upstream** — `pi/packages/coding-agent/src/core/agent-session-runtime.ts:296-299` — the persisted no-leaf branch is `const sessionManager = SessionManager.create(this.cwd, sessionDir); sessionManager.newSession({ parentSession: currentSessionFile });`. The in-memory no-leaf branch does the same at `:336-337`. Both record the parent before `teardownCurrent`.

**Impact** — `/fork` (or the RPC `fork` verb, or `clone`) anchored before the first message produces a session whose header has no `parentSession`, so the session tree loses the edge back to the session it was forked from. Anything walking parentage — session listing/ancestry, `--fork` resumption chains, transcript-linking in audit extensions — sees an orphan where pi shows a child. Silent: the fork succeeds and reports `cancelled:false`.

**Fix** — In `runtime.rs:581` replace `self.factory.build(SessionTarget::New, None)` with `self.factory.build_with_parent(SessionTarget::New, None, previous.clone())`, using the `previous` already bound at `runtime.rs:529`. `build_with_parent` is `pub(crate)` and `runtime.rs` is in the same crate, so no visibility change is needed.

**Verify** — Runtime test: create a persisted session, prompt once, fork at the FIRST user entry (whose parent is `None`, so `fork_anchor` returns `(None, _)`), then read the new session file's header and assert `parentSession` equals the original session file path. Repeat with an in-memory session.

## SEAM-050 — cyrup auth check is unrecognized, and the whole v0.84.1 auth-command surface is unported

**Kind** upstream-drift · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `cyrup/crates/cyrup/src/credential_print.rs:148-158` — the verb table is exactly `Some("print-api-key") => ApiKey`, `Some("print-bearer-token") => BearerToken`, and anything else raises `Unknown auth command "{}". Use "cyrup auth print-api-key" or "cyrup auth print-bearer-token".`, with `dispatch` returning `Some(1)` at `:439-442`. So `cyrup auth check --provider openai` prints that error and exits 1. The help text at `:105-112` advertises only the two print verbs and requires `--model`.

**upstream** — `pi/packages/coding-agent/src/cli/auth-command.ts` is a NEW file at v0.84.1 (`git ls-tree v0.83.0 -- packages/coding-agent/src/cli/` lists neither it nor `auth-check.ts`): `AuthCommandKind = "check" | "api_key" | "bearer_token"` (`:4`), per-kind usage strings (`:17-21`), `--json`/`--credentials`/`--no-refresh` accepted only by `check` (`:82-88`), `Unknown option --X for "auth print-api-key".` (`:100-103`), and `Credential printing requires --provider <provider> or --model <model>` (`:113-115`) — reached for the PRINT kinds too, since v0.84.1 `credential-print.ts:24` calls `validateAuthCommandArgs(args, kind)`. v0.83.0 `credential-print.ts:67-68` required `--model`. New file `cli/auth-check.ts` supplies `checkProviderAuth`/`getProviderCredential`/`createAuthCheckModelRuntime`. Driver rewritten at `main.ts:139-215` (`runAuthCommand`) and moved EARLIER in `main` (`main.ts:578`, ahead of the package/config blocks, where v0.83.0 had it at `:557` after them); exit codes `ready ? 0 : not_ready ? 1 : 2` at `main.ts:208`.

**Impact** — An external tool following the current pi contract (`pi auth check --provider anthropic --json`, branching on exit 0/1/2) gets `Error: Unknown auth command "check"` and exit 1 from cyrup — indistinguishable from `not_ready`. `cyrup auth print-api-key --provider openai` (no `--model`) is rejected where pi v0.84.1 accepts it. An unknown option passed to an auth subcommand produces cyrup's generic handling rather than pi's `Unknown option --X for "auth print-api-key".` plus the usage hint.

**Fix** — Port `cli/auth-command.ts` and `cli/auth-check.ts` into `credential_print.rs` (or a sibling `auth_command.rs`): add a `Check` kind to `CredentialPrintKind` (`credential_print.rs:41-46`), the three check-only flags, provider-or-model validation replacing the `--model`-required check, the `Unknown option --X for "<name>"` rejection, and the 0/1/2 exit mapping in `dispatch` (`:431-443`). Relax the `--model` requirement for the two print verbs at the same time. Cross-references PARITY-GAPS VL-P14.

**Verify** — `cyrup auth check --provider <configured>` prints `ready` and exits 0; with no credential prints `not_ready` and exits 1; with a corrupt store exits 2. `--json` emits the object shape; `--credentials` adds the `credentials` key. `cyrup auth print-api-key --provider openai` (no `--model`) resolves. `cyrup auth check --bogus` prints `Unknown option --bogus for "auth check".`

## SEAM-051 — --tui-mode <regular|fullscreen> is rejected with exit 1 instead of parsed, so the flag's DEFAULT value refuses to launch the binary

**Kind** upstream-drift · **Severity** **high** *(raised from medium in the repair pass)* · **Effort** S · **Confidence** confirmed — every link re-read at HEAD · **observed 2026-08-13** (headless-binary; [`REPRO-LOG.md`](REPRO-LOG.md)) · **FIXED 2026-08-13**

> **FIXED 2026-08-13**, as ADR-0005 §Decision A.1-A.2 specifies. `crates/cyrup/src/cli.rs`: a
> `TuiMode { Regular, Fullscreen }` value-enum, a `tui_mode: Option<TuiMode>` field on `Cli`,
> `--tui-mode` in **both** `KNOWN_LONG_FLAGS` and `KNOWN_VALUE_LONG_FLAGS` (pi consumes
> `args[i + 1]`, args.ts:181 @v0.84.1), and the help row byte-identical to args.ts:291 at pi's
> position (between `--verbose` and `--approve`). `crates/cyrup/src/diagnostics.rs`: pi's two error
> diagnostics, branch for branch against args.ts:180-192 — `--tui-mode requires regular or
> fullscreen` when the value is missing or `-`-prefixed (and the next token is **not** consumed, as
> pi does not `i++` there), `Invalid TUI mode "<v>". Valid values: regular, fullscreen` otherwise
> (value consumed). `crates/cyrup/src/main.rs`: `fullscreen` parses and is declined at startup with
> ADR-0005's own interim string — *"--tui-mode fullscreen is not built yet in this release
> (ADR-0005); falling back to regular."* — a **warning, not an exit**, because pi accepts the value
> and exiting would refuse a launch pi performs. That string is the grep tripwire work unit B-13
> deletes with the renderer.
>
> **Test** `crates/cyrup/tests/tui_mode_flag.rs` (5 tests, real binary): `regular` accepted in space
> form, `=` form, `--mode json` and `--mode rpc`; `bogus` prints pi's exact text on **stderr** and
> exits 1 in both forms; no value / a flag as the value / `--tui-mode=` print pi's `requires` text
> and exit 1; `fullscreen` prints the ADR-0005 line and does NOT report an unknown option; `--help`
> carries the row between `--verbose` and `--approve`. **RED before** (probe: `--tui-mode` removed
> from `KNOWN_LONG_FLAGS` → `["--tui-mode", "regular"] must not be an unknown option; stderr was:
> Error: Unknown option: --tui-mode`), **GREEN after** (`5 passed`).
>
> **One CYRUP-DELTA, stated in `diagnostics.rs`:** the `=` form. pi's parser matches only
> `arg === "--tui-mode"`, so `--tui-mode=regular` lands in its `unknownFlags` map — as does
> `--model=x` and every other `=` form, since pi has no `=` handling at all. cyrup has always
> accepted `=` for every known long flag (`cli.rs`'s `split('=')`), and that pre-existing,
> workspace-wide divergence is **not** re-litigated here; it only forced the two diagnostics to cover
> the `=` spelling as well, or `--tui-mode=bogus` would have died with a clap usage error (exit 2)
> instead of pi's text. **No id owns the general `=`-form divergence** — see the note at the end of
> this entry.
>
> **Also settles `DRIFT-022`** (tracker, `duplicate-of: SEAM-051`): the flag half is done; the
> renderer half is `TUI-019` under ADR-0005 §B.

> **Reproduced 2026-08-13 in the shipped binary.** `cyrup --offline --no-session --no-extensions
> --tui-mode regular -p hi` prints `Error: Unknown option: --tui-mode` and exits 1; the control run
> without the flag reaches the provider. Reproduced with the value present, absent, bogus and in `=`
> form, with extensions enabled and disabled, and in print / `--mode json` / `--mode rpc`. `--help`
> exits 0 and the flag is absent from the shipped help text.
>
> **Two mechanism details in the paragraphs below were corrected against that measurement.** (1) The
> emitted text is `Unknown option:` — **singular**; the plural `Unknown option(s):` form is used only
> when more than one flag is unmatched (`crates/cyrup-ext/src/tests/extension_flag_diagnostics.rs:42,99`).
> (2) The failure does **not** require the extension subsystem: the reconciliation diagnostic runs
> identically under `--no-extensions`, so do not read the extension-flag partitioning paragraph as a
> reachability qualifier. The verdict is unchanged.

> Supersedes **SEAM-019**, whose upstream premise (`--ui-mode` / `--alt`) does not exist at any pi
> tag. Work this item; do not work SEAM-019 as written.
>
> **Why high.** This is not a missing feature, it is a launch failure: `cyrup --tui-mode regular` —
> the flag's own default, the value that asks for the renderer cyrup already has — exits 1 before any
> session is built, with a message claiming the option is unknown. Anyone carrying a pi v0.84.1
> command line, a wrapper script, a shell alias or a CI invocation cannot start cyrup at all until
> they find and delete a flag that upstream documents as valid. Rated `high` rather than `critical`
> because the failure is deterministic, immediate, printed, and one token from working — it destroys
> nothing and hides nothing. It was previously ranked below 188 mediums at effort S.

**cyrup** — `grep -rn 'tui_mode\|tui-mode' crates/` returns zero hits outside `cyrup-tui` doc comments. The flag is therefore absent from `KNOWN_LONG_FLAGS` (`cyrup/crates/cyrup/src/cli.rs:757-799`), so `partition_extension_flags` (`cli.rs:701-753`) captures `--tui-mode fullscreen` as an extension flag at `:731-738` (the value does not start with `-`/`@`); nothing registers it, so `apply_extension_flag_values` (`cyrup/crates/cyrup-session-svc/src/builder.rs:946-967`) records `Unknown option: --tui-mode` (singular — the plural form is reserved for two or more unmatched flags; **corrected against the 2026-08-13 measurement**), `collect_diagnostics` (`runtime.rs:113-119`) rates it `error`, and `report_runtime_diagnostics` (`main.rs:1846-1862`) exits 1 in every mode. The reported message is also misleading: the flag is not unknown to pi, it is unported.

**upstream** — `pi/packages/coding-agent/src/cli/args.ts:180-193` (v0.84.1; absent at v0.83.0) — a dedicated `--tui-mode` arm accepting `regular`/`fullscreen`, with `"--tui-mode requires regular or fullscreen"` when the value is missing or starts with `-` (`:186`) and `Invalid TUI mode "X". Valid values: regular, fullscreen` otherwise (`:189-191`); typed `tuiMode?: TuiMode` at `args.ts:49`, listed in the help at `:291`, threaded at `main.ts:935` into `InteractiveMode`, consumed at `modes/interactive/interactive-mode.ts:345` and `:530` with the settings fallback `settingsManager.getTuiMode()`.

**Impact** — `cyrup --tui-mode regular` — the DEFAULT value, harmless even without an alt-screen renderer — aborts the run with exit 1 and a message claiming the option is unknown. A user or script carrying pi v0.84.1 flags cannot launch cyrup at all until the flag is removed.

**Fix** — Add `--tui-mode` to `KNOWN_LONG_FLAGS` and `KNOWN_VALUE_LONG_FLAGS` (`cli.rs:757-799`, `:803+`), add a `TuiMode { Regular, Fullscreen }` value-enum field to `Cli`, and add pi's two error diagnostics to `apply_arg_leniency` (`diagnostics.rs:90-152`) so an invalid value is an ERROR-severity diagnostic rather than a swallowed extension flag. Add the help line to `render_help` (`cli.rs:828-930`). Accepting `regular` as a no-op and rejecting `fullscreen` with an explicit "not supported in this build" message is a legitimate interim; silently exiting 1 on `regular` is not. The rendering half is the alt-screen work (PARITY-GAPS VL-P19, area 07).

**Verify** — `cyrup --tui-mode regular` starts normally; `cyrup --tui-mode bogus` prints `Invalid TUI mode "bogus". Valid values: regular, fullscreen` and exits 1; `cyrup --tui-mode` (no value) prints `--tui-mode requires regular or fullscreen`. **All three are now covered by `crates/cyrup/tests/tui_mode_flag.rs` against the real binary.**

> **New gap found while fixing this, owned by no id — the `--flag=value` form, workspace-wide.**
> pi's hand-rolled parser has **no `=` handling for known flags at all**: `parseArgs` compares
> `arg === "--model"` etc., so `--model=gpt-5`, `--theme=x`, `--session-dir=/tmp` and every other
> `=` spelling falls through to `arg.startsWith("--")` and is captured into `unknownFlags`
> (args.ts:204-207 @v0.84.1), which reconciles to `Unknown option: --model` and exit 1. cyrup accepts
> all of them, because `partition_extension_flags` keys on `arg.split('=').next()`
> (`crates/cyrup/src/cli.rs`) and clap parses the `=` form natively. So cyrup silently accepts a
> whole spelling of the CLI that pi rejects. It is a **superset**, not a launch failure, which is why
> it is filed here rather than fixed inside SEAM-051 — closing it means deciding whether cyrup
> narrows to pi's exact rejection (and eats the exit-code change for every `=` user) or records a
> deliberate delta. Effort S once decided; scope is `cli.rs` + `diagnostics.rs` + one test.

## SEAM-059 — The signal watcher holds the startup session Arc, so after any replacement it aborts a disposed session

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

> Recovered by the refuter from surface the audit pass walked past.

**cyrup** — `cyrup/crates/cyrup/src/signals.rs:88-101` — `spawn_abort_on_signal(session: Arc<AgentSession>, cancel: CancelToken)` moves that one `Arc` into a task that lives for the whole run and calls `session.abort()` on it (`:94`). Every call site binds the session ONCE, before the mode entry point: `main.rs:533` (interactive, from `runtime.session().await` at `:518`), `main.rs:670` (rpc, from `:667`) and `main.rs:781` (print/json, from `:775`). But `AgentSessionRuntime::install_inner` REPLACES the active session on every switch path — it disposes the outgoing one (`runtime.rs:420` `current.dispose_with(...)`) and installs a brand-new `Arc<AgentSession>` at `runtime.rs:433-438`. So after an RPC `new_session`/`switch_session`/`fork`/`clone` (`rpc.rs:1073-1303`), or an extension's `ctx.newSession()`/`ctx.fork()`/`ctx.reload()` in ANY mode, the watcher's `Arc` points at a session whose run is already settled and whose `session_cancel` is already fired. The RPC loop itself gets this right — it rebinds off `runtime.watch_generation()` (`rpc.rs:680`, `:778-785`, `rebind_session` at `:505-518`) — so the signal watcher is the one holder that never rebinds.

**upstream** — pi never binds a session into its handlers. `pi/packages/coding-agent/src/modes/rpc/rpc-mode.ts:373-375` calls `shutdown(...)`, whose body dereferences `runtimeHost` (`await runtimeHost.dispose()` at `:734`); `print-mode.ts:57-61` calls `disposeRuntime()`, likewise `await runtimeHost.dispose()` (`:47`). `AgentSessionRuntime.dispose()` (`agent-session-runtime.ts:398-405`) always resolves `this.session`, i.e. the CURRENT one.

**Impact** — Ctrl-C or SIGINT after any session replacement does not stop the run that is actually executing: the abort lands on a disposed session and the live turn continues to completion, burning tokens and emitting output the user asked to stop. Combined with SEAM-047 (no teardown on the first signal at all in rpc/print/json) the first signal is a complete no-op on those paths.

**Fix** — Give `spawn_abort_on_signal` the `Arc<AgentSessionRuntime>` instead of the session and do `runtime.session().await.abort()` inside the handler, matching pi's always-current dereference. Update the three call sites (`main.rs:533`, `:670`, `:781`). Lands with SEAM-047, which rewrites the same function.

**Verify** — Runtime test: build a runtime, install the watcher, replace the session via `new_session`, start a long turn on the replacement, deliver the signal and assert the REPLACEMENT session went to aborted/idle (today the disposed one is aborted and the live one keeps running).

## SEAM-S03 — No detached-child registry: setsid-detached bash children survive teardown — **CLOSED 2026-08-15 (REFUTED — the cyrup-tools half is present at HEAD)**

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `grep -rn 'kill_tracked\|track_detached\|DETACHED_PIDS\|tracked_pids' crates/` returns ZERO hits at HEAD. Detached children ARE created: `cyrup/crates/cyrup-tools/src/ops/local.rs:272` calls `libc::setsid()` in a `pre_exec` (documented at `:19` as the `killpg` path) and `:334` is the per-call `libc::killpg`. Nothing registers those pids process-globally, and neither `cyrup/crates/cyrup/src/signals.rs` (117 lines, no pid handling) nor any teardown path drains such a registry.

**upstream** — `pi/packages/coding-agent/src/utils/shell.ts:180-195` (v0.84.1) — a process-global `trackedDetachedChildPids: Set<number>` with `trackDetachedChildPid` / `untrackDetachedChildPid` / `killTrackedDetachedChildren()`. Registered at `core/tools/bash.ts:108` right after the detached spawn and untracked in the `finally`. Drained SYNCHRONOUSLY inside the signal handler, before any async teardown, in all three hosts: `rpc-mode.ts:374`, `print-mode.ts:58`, `interactive-mode.ts`.

**Impact** — A `setsid`-detached bash child (a dev server, a watcher, a long build) outlives cyrup on every exit path except the per-run cancel race that happens to catch it. Killing cyrup leaves orphaned process groups holding ports and file locks; a CI job that stops cyrup does not stop what cyrup started.

**Fix** — Add a process-global registry beside `cyrup-tools/src/ops/local.rs` (an `OnceLock<Mutex<HashSet<i32>>>`), register the pid right after the `pre_exec` spawn at `local.rs:272` and remove it in the same `finally`-equivalent that already runs `killpg` at `:334`; drain it synchronously from the first-signal path in `signals.rs:88-101` before any async teardown, and again from `dispose`. Lands with SEAM-047, which is where the synchronous drain point comes into existence.

**Verify** — Spawn a `setsid` child from a bash tool call that outlives the turn, SIGTERM cyrup, and assert the child's process group is gone within the exit path rather than reparented to init.

## SEAM-066 — Every pre-launch TUI surface hardwires the dark palette, ignoring `settings.theme`, custom theme packages, terminal detection, `showHardwareCursor` and `clearOnShrink`

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — Four pre-launch surfaces, one hardwired palette: `cyrup/crates/cyrup/src/main.rs:1044` (missing-session-cwd prompt), `main.rs:1089` (resume picker **and** trust prompt — one `let theme = UiTheme::default();` serves both) and `cyrup/crates/cyrup/src/subcommands.rs:624` (`cyrup config`), where `UiTheme::default()` is literally `UiTheme::dark()` (`cyrup/crates/cyrup-tui/src/theme.rs:144-148`). No settings read, no `COLORFGBG`, no OSC-11 probe, no registered custom themes. The machinery exists and is used 500 lines away on the in-app path: `main.rs:1589-1605` builds `ThemeController::boot_from_env(theme_setting)` from `settings.effective().theme_setting()` and then `sync_with_terminal(&StdinTerminalProbe, 100ms, &colorfgbg)` — exactly pi's two-phase resolution. The pre-launch surfaces simply do not call it.

**upstream** — `pi/packages/coding-agent/src/cli/startup-ui.ts:77-85` @v0.83.0 — `createStartupTui` does four settings-derived things before mounting anything: `setRegisteredThemes(await loadStartupThemes(settingsManager))` (`:78`), `initTheme(resolveThemeSetting(settingsManager.getThemeSetting(), detectTerminalBackgroundFromEnv().theme) ?? terminalTheme)` (`:79-80`), `new TUI(new ProcessTerminal(), settingsManager.getShowHardwareCursor(), getAgentDir())` (`:82`) and `ui.setClearOnShrink(settingsManager.getClearOnShrink())` (`:83`). `startStartupTui` (`:87-90`) then fires `applyDetectedStartupTheme` (`:92-100`), which for an unset/`auto` setting queries the terminal with a 100 ms bound and re-themes live. `cli/config-selector.ts:22` does the same for `pi config`. At v0.84.1 only the class name changes (`TuiMainScreen`, `startup-ui.ts:82` / `config-selector.ts:25`) — that half is mechanism-N/A per ADR-0001, but the settings-derived **arguments** are not.

**Impact** — A user with `"theme": "light"` in settings.json — or any custom theme from a package — gets a dark-palette resume picker, trust prompt, missing-cwd prompt and `cyrup config` screen, very likely on a light terminal, and then a correctly-themed app one keystroke later. `showHardwareCursor` and `clearOnShrink` are ignored on these surfaces too.

**Fix** — Add `fn startup_theme(dirs: &ConfigDirs) -> UiTheme` to `crates/cyrup/src/startup_ui.rs`: load settings with `SettingsManager::load(file_settings_store(dirs), Settings::new(), false)` (pi's `projectTrusted: false` startup manager, `startup-ui.ts:65-67`), build `ThemeController::boot_from_env(settings.effective().theme_setting().as_deref())` and return `controller.theme()`. Call it at `main.rs:1044`, `main.rs:1089` and `subcommands.rs:624` in place of `UiTheme::default()`. Registered custom themes need `run_startup_selector` to accept the resolved theme set the way `loadStartupThemes` does — if the resource resolve is too heavy for the pre-launch path, file that half against this same item rather than silently dropping it.

**Verify** — Set `"theme": "light"` in `<agent_dir>/settings.json` and **live-run** `cyrup --resume` and `cyrup config` in a real terminal: both must render the light palette, matching what `cyrup` itself renders after boot. A TestBackend unit test cannot show this.

## SEAM-067 — Pre-launch selectors never load the user's `keybindings.json`, and their hint rows name the wrong keys

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup/src/startup_ui.rs:128`, `:259`, `:333` and `cyrup/crates/cyrup/src/subcommands.rs:625` all use `SelectKeymap::default()`, and `run_resume_picker` builds `SessionSelector::new(rows)` without `with_keymaps` (`startup_ui.rs:127`), so the session-specific chords (delete / sort / named-filter / path / rename) also fall back to defaults — the selector's own comment at `session_selector.rs:771-774` notes it "adopt[s] whatever `tui.select.*` table actually routed this key", which pre-launch is always the default table. The user's file is read exactly once, on the in-app path only: `main.rs:1624-1629` reads `<agent_dir>/keybindings.json` and calls `app.load_keybindings_json`, which fans out to `select_keymap`/`session_keymap`/`tree_keymap`/`models_keymap`/editor via `merge_json` (`cyrup-tui/src/app/shell.rs:159-172`).

**upstream** — `pi/packages/coding-agent/src/cli/startup-ui.ts:81` @v0.83.0 (identical at v0.84.1) — `createStartupTui` calls `setKeybindings(KeybindingsManager.create())`, installing the user's `<agentDir>/keybindings.json` globally for every startup selector. `cli/session-picker.ts:22-23` does it a second time and threads the same manager into the component (`{ showRenameHint: false, keybindings }`, `:48`), and the component resolves every chord through it — `kb.matches(keyData, "tui.select.confirm")`, `"tui.select.cancel"`, `"app.session.delete"`, `"app.session.rename"`, `"app.session.toggleSort"`, `"app.session.togglePath"` (`modes/interactive/components/session-selector.ts:532-582`).

**Impact** — A user who rebound `tui.select.confirm`/`tui.select.cancel` or the `app.session.*` chords finds their bindings work inside cyrup but not in the `--resume` picker, the trust prompt, the missing-cwd prompt or `cyrup config` — and, worse, the on-screen hint rows print the **default** labels (`session_selector.rs:681-692` resolves labels from the same unmerged table), so the screen actively misreports which key does what. Compounded by SEAM-061, whose dead `tab scope` hint is on the same row.

**Fix** — Hoist the load: add `fn startup_keymaps(dirs: &ConfigDirs) -> (SelectKeymap, SessionKeymap)` reading `<agent_dir>/keybindings.json` and applying `merge_json` to both (the same merge `app/shell.rs:159-172` performs), logging and continuing on a malformed document. Use it at `startup_ui.rs:128`/`:259`/`:333` and `subcommands.rs:625`, and pass the session keymap through `SessionSelector::with_keymaps` in `run_resume_picker`. **Cross-area note:** area 05's repair pass owns pi's `migrateKeybindingsConfigFile`, which rewrites legacy ids in that same file — this loader must run *after* whatever alias table that item lands, or a legacy `keybindings.json` will still read as empty here.

**Verify** — Write `{"tui.select.cancel": ["ctrl+q"]}` to `<agent_dir>/keybindings.json` and **live-run** `cyrup --resume`: ctrl+q must dismiss the picker and the hint row must name ctrl+q, not Esc.

## SEAM-068 — `--list-models <search>` uses a lossy hand-rolled fuzzy filter while a faithful port of pi's `fuzzyFilter` sits unused in cyrup-tui

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup/src/main.rs:1364-1373` — `fn fuzzy_match(haystack, search)` splits the query with `search.split_whitespace()` and requires each token to be an ASCII-lowercased subsequence; no scoring, no swap fallback. It is called at `main.rs:1392-1395` over `format!("{} {}", m.provider, m.id)`. Meanwhile `cyrup/crates/cyrup-tui/src/fuzzy.rs:139-171` is a complete, documented port of pi's `fuzzyFilter` — `query.split(|c: char| c.is_whitespace() || c == '/')` at `:140`, the alphanumeric-swap retry at `:129-131`, Unicode `char::to_lowercase` at `:122-123`, the score-ascending stable sort at `:170` — and the `cyrup` bin already depends on `cyrup-tui`.

**upstream** — `pi/packages/coding-agent/src/cli/list-models.ts:45` @v0.83.0 (`:49` at v0.84.1) — `filteredModels = fuzzyFilter(models, searchPattern, (m) => \`${m.provider} ${m.id}\`)`. `packages/tui/src/fuzzy.ts:104-107` (identical at both tags) splits the query on `/[\s/]+/` — whitespace **and slash** — and `:120-128` requires every token to match; `fuzzyMatch` at `:75-92` adds the alphanumeric-swap retry (`"o4"` retried as `"4o"` at a +5 penalty).

**Impact** — `cyrup --list-models anthropic/sonnet` prints `No models matching "anthropic/sonnet"` while `pi --list-models anthropic/sonnet` lists the Anthropic Sonnet models, because cyrup treats the whole string as one token and the haystack `"anthropic claude-sonnet-4-5"` contains no `/`. `provider/model` is the form cyrup's own `--model` flag documents (`cli.rs:905`), so it is the first thing a user types. The swap retry is lost too (`--list-models o4` finds `gpt-4o` in pi, nothing in cyrup).

**Fix** — Delete `main.rs:1362-1373` and call the existing port: `cyrup_tui::fuzzy::filter(&models, search, |m| key)` over the `"{provider} {id}"` key, keeping `main.rs:1405-1410`'s provider-then-id sort afterwards — pi re-sorts too (`list-models.ts:54-58`), so the score order is discarded on both sides and the behaviours coincide exactly. **Land with SEAM-020**, which changes the input set to the same function.

**Verify** — `cargo test -p cyrup` asserting the filter over `["anthropic claude-sonnet-4-5", "openai gpt-4o"]` returns the Anthropic row for `"anthropic/sonnet"` and the OpenAI row for `"o4"`. Then `cyrup --list-models anthropic/sonnet` must print rows.

## SEAM-017 — No RpcClient counterpart — **CLOSED 2026-08-14 (REFUTED / already-done)**

> **CLOSED 2026-08-14 (sweep 8). Everything below is the filing text and every fact in it is now
> false.** `crates/cyrup-modes/src/rpc_client.rs` is **1262 lines** at HEAD and carries all 41 methods
> upstream declares (`packages/coding-agent/src/modes/rpc/rpc-client.ts` @v0.83.0 — note the path:
> `packages/pi/src/…` does **not** exist at this tag): `spawn`/`attach`/`stop`/`on_event`/`stderr`,
> the 33 verbs `prompt` … `get_commands`, and `wait_for_idle`/`collect_events`/`prompt_and_wait`.
> Exported at `cyrup-modes/src/lib.rs:39-40`. **The `grep` evidence below is the stale artefact, and
> the row is a clean instance of the doc-staleness class: the fix landed, no writer reconciled it,
> and one sweep re-read the row instead of the crate.**

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** high

**cyrup** — ~~`grep -rni 'rpcclient\|rpc_client' crates/` returns ZERO hits workspace-wide at HEAD.~~ `cyrup/crates/cyrup-modes/src/` contains `error.rs`, `json.rs`, `json_event.rs`, `lib.rs`, `print.rs`, `rpc.rs` — no client module.

**upstream** — `pi/packages/coding-agent/src/modes/rpc/rpc-client.ts` exports `RpcClient`, re-exported from `modes/index.ts:7`; at v0.84.1 its listener type became `JsonAgentSessionEvent` (`rpc-client.ts:50`).

**Impact** — Embedders and cyrup's own tests must hand-roll NDJSON framing and request correlation (`cyrup/crates/cyrup-modes/src/tests/modes.rs`, `read_json_line`), which is exactly how wire-shape divergences like SEAM-011 and SEAM-053 go unnoticed.

**Fix** — Add `cyrup-modes/src/rpc_client.rs` porting `rpc-client.ts`: spawn/attach, id-correlated request/response, event stream typed on the `json_event.rs` projection. Retrofit `modes.rs` onto it.

**Verify** — `modes.rs` tests drive the client instead of raw lines and still pass.

## SEAM-020 — `--list-models` prints the entire compiled catalog instead of the auth-configured one, and `--help` omits extension flags

**Kind** parity-bug · **Severity** **medium** *(raised from low in the repair pass)* · **Effort** M · **Confidence** high

> **Rewritten in the repair pass.** The item was filed as an *ordering* observation ("both handlers
> run before the session exists") and rated low on that framing. The `cli/` sweep found the concrete
> defect the ordering was hiding, and it is not about ordering at all: `--list-models` calls the wrong
> function. The `--help` half is unchanged and is the smaller of the two.

**cyrup** — Two halves.
- **The list is unfiltered.** `cyrup/crates/cyrup/src/main.rs:283-284` — `return list_models(&cyrup::provider::all_available_models(&models_json), search);`. Despite the name, `all_available_models` applies **no auth filter**: `cyrup/crates/cyrup/src/provider.rs:237-240` is `composed_registry(...).0.get_models(None)`, i.e. pi's `getModels()`, not `getAvailable()`. The empty-list branch that would print pi's guidance (`main.rs:1380-1384` → `format_no_models_available_message()`) is therefore unreachable in practice. cyrup already has the predicate one function down: `default_launch_model` takes `has_configured_auth: &dyn Fn(&Model) -> bool` and its doc comment at `provider.rs:255-256` explicitly cites "Pi step 4, `getAvailable()` filtered to `hasConfiguredAuth`".
- **The help is unfiltered too.** `main.rs:140-143` — `if cli.help { print!("{}", render_help(&[])); return Ok(0); }` with an EMPTY extension-flag slice. Both run long before `AgentSessionRuntime::create`/`create_unannounced` at `main.rs:509` (interactive), `:652` (rpc) and `:761` (print/json).

**upstream** — `pi/packages/coding-agent/src/cli/list-models.ts:35` @v0.83.0 — `const models = [...(await modelRuntime.getAvailable())];` (at v0.84.1, `:39`, `getAvailable(undefined, { signal })`). `packages/ai/src/models.ts:394-405` @v0.83.0 (v0.84.1 `:522-538`) implements it as: read each provider's credential, `checkProviderAuth`, then `checks.flatMap(({provider, credential, auth}) => { if (!auth) return []; return provider.filterModels?.(models, credential) ?? models; })` — providers without complete auth contribute nothing, and configured providers can narrow further by credential. The interface comment is unambiguous (`models.ts:152-153`): "Return models whose providers have complete auth configuration." `list-models.ts:37-40` prints `formatNoModelsAvailableMessage()` when that set is empty. For the help half, `main.ts:804-810` flat-maps `resourceLoader.getExtensions()` flags into `printHelp` and `:812-816` lists models off the live `modelRuntime`, both strictly AFTER `createAgentSessionRuntime` at `:793-798`.

**Impact** — `cyrup --list-models` on a fresh install prints hundreds of rows for providers the user has no credential for, where `pi --list-models` prints the "no models available" guidance that tells them how to log in. With one provider configured, pi shows that provider's models and cyrup shows everyone's — so the listing is not a usable answer to "what can I run?", and every `--model` picked from it that belongs to an unconfigured provider fails at launch. Secondary: `cyrup --help` never lists extension-contributed flags.

**Fix** — Build the `has_configured_auth` closure `default_launch_model` already consumes (stored credential in `auth.json`, a known provider env var, or a runtime `--api-key`) and add `available_models(&models_json, &has_configured_auth)` beside `all_available_models` in `provider.rs:237`; call it at `main.rs:284`. The empty branch at `main.rs:1380-1384` then becomes reachable and correct. Separately, feed `render_help` the extension flag set from the resource loader, which does require the ordering move the item originally described. **Land with SEAM-068**, which replaces the search filter in the same function, and note the v0.84.1 `AbortSignal.timeout` on `getAvailable` is PARITY-GAPS VL-P6 (area 01), not this item.

**Verify** — Unit-test `available_models` with a stub auth predicate (zero configured → empty; one configured → only its models). Then run `cyrup --list-models` in a shell with every provider env var unset and no `auth.json`: it must print the no-models-available guidance, not a table. For the help half, a native extension declaring a CLI flag must appear in `cyrup --help`.

## SEAM-028 — modes.rs setWidget case pins SEAM-011's invented wire field

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-modes/src/tests/modes.rs:1014-1019`: the comment at `:1014-1015` concedes that cyrup's WIT collapsed pi's 3-arg `setWidget(key, content, options)` into one opaque JSON payload, and the test then asserts the collapse as CORRECT — `assert_eq!(req["method"], "setWidget")` (`:1018`) and `assert_eq!(req["widget"], serde_json::json!({"widget":"text","text":"hi"}))` (`:1019`). Producer under test: `cyrup/crates/cyrup-modes/src/rpc.rs:423-428`.

**upstream** — `pi/packages/coding-agent/src/modes/rpc/rpc-types.ts:264-271` pins `widgetKey`/`widgetLines`/`widgetPlacement`; no `widget` field exists in pi's union.

**Impact** — The suite certifies the divergence, so fixing SEAM-011 turns a green test red and invites a revert. The adjacent `setStatus` case IS a correct parity assertion (it checks `statusText` is omitted), which makes the wrong one easy to overlook.

**Fix** — Mark it `#[ignore = "SEAM-011: cyrup collapses setWidget into one blob"]` with the pi-shaped assertion written beneath, or invert it as part of SEAM-011.

**Verify** — After SEAM-011 the test asserts pi's three fields and no `widget` key.

## SEAM-029 — ThinkingArg doc comment claims the leniency path is unreachable

**Kind** stale-port · **Severity** low · **Effort** S · **Confidence** high

> **Corrected this pass.** The auditor also claimed `cli.rs`'s pi citations were off by two. The
> refuter checked at the tag: v0.83.0 `args.ts:57` IS `VALID_THINKING_LEVELS`, `:130` IS the
> `--thinking` arm and `:135` IS the warning push — `cli.rs`'s citations are ACCURATE. Only
> `diagnostics.rs:51`'s `args.ts:59` (which is `isValidThinkingLevel`) is off by two. Fix the item
> text before working it: the defect is the contradiction, not the line numbers in `cli.rs`.

**cyrup** — `cyrup/crates/cyrup/src/cli.rs:45-46`, verbatim at HEAD: "`--thinking <level>` (args.ts:57,130). Clap validates membership; the warning-on-invalid path Pi takes (args.ts:135) is unreachable here because clap rejects an unknown value with a usage error." Contradicted by `cyrup/crates/cyrup/src/diagnostics.rs:110-124`, which inspects the `--thinking` value BEFORE clap sees it (called from `main.rs:112`), keeps it when in `VALID_THINKING_LEVELS` (`diagnostics.rs:52-53`, seven entries including `max`) and otherwise drops both tokens with `Invalid thinking level "{value}". Valid values: {joined}`.

**upstream** — `pi/packages/coding-agent/src/cli/args.ts:57` (`VALID_THINKING_LEVELS`), `:130-139` (the `--thinking` arm with the warn-and-continue push at `:135`), and `:59` (`isValidThinkingLevel`).

**Impact** — Doc-only, but this comment is exactly what mis-set a previous edition of this document: a reader concludes the leniency path does not exist and files a false gap.

**Fix** — Rewrite `cli.rs:45-46` to say the leniency pass lives in `diagnostics.rs:110-124` and runs pre-clap, keeping its `args.ts:57,130,135` citations as-is; correct `diagnostics.rs:51`'s `args.ts:59` to `args.ts:57` (or retarget the comment at `isValidThinkingLevel`, which really is `:59`).

**Verify** — Read-through; the two files agree with each other and with the tag.

## SEAM-030 — RPC tests assert wall-clock/scheduling outcomes they cannot control

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — Three instances, all surviving at HEAD. (a) `cyrup/crates/cyrup-modes/src/tests/modes.rs:1277` records `std::time::Instant::now()`, `:1279` takes `elapsed`, and `:1289-1293` asserts `elapsed < Duration::from_secs(3)` "proving the command loop is serialized (G1)" — five lines above the deterministic `bash["data"]["cancelled"] == true` at `:1296-1298`, which proves the same thing without a clock. (c) `modes.rs:1139` takes `tokio::time::Instant::now()` and `:1154-1158` asserts `started.elapsed() < Duration::from_secs(2)` on top of an already-deterministic `timeout(5s)` + `assert!(!resolved)` at `:1149-1153` — pure wall-clock margin with no semantic content, the most flake-prone. (b) `modes.rs:1088` is a fixed `tokio::time::sleep(Duration::from_millis(50))` before a negative assertion; **a smell, not a defect**, because `extension_ui_effect_json` returns `None` for `SetHeader`/`SetFooter`/`SetToolsExpanded`, so no `extension_ui_request` can ever be written regardless of sleep length.

**upstream** — No counterpart: these test cyrup-original concurrency structure. pi's `void handleInputLine(line)` (`pi/packages/coding-agent/src/modes/rpc/rpc-mode.ts:806-807`, v0.84.1) has no equivalent.

**Impact** — Under CI load or a debug build, (a) and (c) fail for reasons unrelated to the behaviour under test, training contributors to re-run rather than investigate — which is how a suite silently stops being trustworthy.

**Fix** — Delete (a)'s and (c)'s duration assertions; the deterministic assertions beside them already prove the property. Replace (b)'s sleep with a positive synchronisation point.

**Verify** — Tests still pass with the timing assertions removed and stay green under `--test-threads=1` on a loaded machine.

## SEAM-034 — CompactionResult drops pi's usage field

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session-svc/src/state.rs:193-203`: `CompactionResult { summary, first_kept_entry_id, tokens_before, estimated_tokens_after, details }` — no `usage` field, and `estimated_tokens_after` is non-optional at `:200`. Serialized straight onto the wire by the RPC `compact` handler (`cyrup/crates/cyrup-modes/src/rpc.rs:1177-1181`). The data exists one layer down: `cyrup/crates/cyrup-session/src/entry.rs` carries `usage: Option<Usage>` on the compaction entry, which `SessionStats::from_entries` (`state.rs:106-110`) now reads — so the stats tier has the number and the compaction response does not.

**upstream** — `pi/packages/coding-agent/src/core/compaction/compaction.ts:88-97`: `interface CompactionResult<T = unknown> { summary; firstKeptEntryId; tokensBefore; estimatedTokensAfter?; usage?: Usage; details?: T }`, with `usage` documented at `:93` as "Usage from the LLM call(s) that generated this summary"; on a split turn pi records the SUM via `combineUsage`. Wire contract `modes/rpc/rpc-types.ts:171`.

**Impact** — An RPC client cannot see what the compaction itself cost, so a cost-tracking front-end under-reports every compaction even though the session totals (SEAM-031, now closed) include it.

**Fix** — Add `#[serde(default, skip_serializing_if = "Option::is_none")] pub usage: Option<Usage>` to `state.rs:193-203` (elided when absent so existing goldens stay byte-identical) and populate it at the construction sites from the value already written to the compaction entry. While there, make `estimated_tokens_after` an `Option<u64>` to match `estimatedTokensAfter?`.

**Verify** — Compact a faux-provider session over RPC and assert the `compact` response `data.usage` matches the persisted compaction entry's `usage`; re-run the JSONL round-trip test to confirm byte-identity when `usage` is absent.

## SEAM-052 — --version prints `cyrup <version>` and pre-empts the parse-error diagnostics pi reports first

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high (the ordering half is airtight; the rendered string rests on clap's documented behaviour, not on observed output)

**cyrup** — `cyrup/crates/cyrup/src/cli.rs:79-90` — the `Cli` derive sets `name = "cyrup"`, `version`, `disable_version_flag = true` (`:83`), and the field carries `#[arg(short = 'v', long = "version", action = clap::ArgAction::Version)]` (`:89`). clap's `ArgAction::Version` renders `{display_name} {version}` and exits from inside the parse, which happens at `cyrup/crates/cyrup/src/main.rs:119` (`Cli::parse_from(&clap_argv)`) — i.e. BEFORE `report_diagnostics(&parse_diagnostics)` at `main.rs:130` and its error exit-1 at `:131-136`. No site in `crates/cyrup` prints a bare version string. The `--help` path is correctly ordered by contrast: `if cli.help` is at `main.rs:140`, after the diagnostics gate.

**upstream** — `pi/packages/coding-agent/src/main.ts:562-570` reports every parse diagnostic and `process.exit(1)` on any error-severity one, and only THEN `:573-576` does `if (parsed.version) { console.log(VERSION); process.exit(0); }` — a bare semver, no program name.

**Impact** — Two script-visible differences. (1) `cyrup --version` emits `cyrup 0.1.0` where `pi --version` emits `0.84.1`, so a version-compare in a wrapper script has to strip a prefix pi never emits. (2) `cyrup -x --version` exits 0 printing the version, while `pi -x --version` prints `Error: Unknown option: -x` and exits 1 — a mistyped flag beside `--version` is silently accepted.

**Fix** — Drop `action = clap::ArgAction::Version` from `cli.rs:89` (make `version` a plain `bool` like `help`), then handle it explicitly in `main.rs` immediately after the diagnostics gate at `:136` and before the `--help` block at `:140`: `if cli.version { println!("{}", env!("CARGO_PKG_VERSION")); return Ok(0); }`. Keep `disable_version_flag = true`.

**Verify** — `cyrup --version` prints exactly the semver with no program name; `cyrup -v` matches; `cyrup -x --version` prints `Error: Unknown option: -x` and exits 1. Update the existing `version_short_is_v_not_verbose` test at `cli.rs:1094`, which currently asserts the clap Version action fires during parse.

## SEAM-053 — Optional RPC wire fields are emitted as explicit null where pi omits the key entirely

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

> **Corrected this pass.** The auditor's second instance is wrong: the `fork` verb passes
> `ForkPosition::Before` (`rpc.rs:1277`) and `fork_anchor` (`session.rs:5138-5141`) returns
> `Some(text)` on every successful `Before` fork; the `position:"at"` verb is `clone`, which emits no
> `text` key at all (`rpc.rs:1299`), matching pi `rpc-mode.ts:625`. The `"text": null` case arises
> only on the veto/cancelled path (`runtime.rs:527`). The `get_state` instance below is the real one.

**cyrup** — `cyrup/crates/cyrup-modes/src/rpc.rs:1424-1437` — `state_view` builds the `get_state` payload with `json!` and unconditionally includes `"sessionFile": session.session_file().await.map(...)` (`:1431`) and `"sessionName": session.session_name().await` (`:1433`); a `None` serializes as JSON `null`, not an absent key. Contrast the code that DOES get this right: `SessionStats::session_file`/`context_usage` carry `#[serde(default, skip_serializing_if = "Option::is_none")]` (`cyrup/crates/cyrup-session-svc/src/state.rs:39`, `:52`), and the `setStatus` effect inserts `statusText` only when present (`rpc.rs:414-416`) — so the wire is inconsistent with itself.

**upstream** — `pi/packages/coding-agent/src/modes/rpc/rpc-types.ts:102-104` types `RpcSessionState.sessionFile?: string`, `sessionName?: string` and `model?`, populated from an object literal at `rpc-mode.ts:446-459` — `JSON.stringify` drops an `undefined` property, so pi's line for an unnamed ephemeral session contains neither key.

**Impact** — A client written against pi's `RpcSessionState` and using `"sessionName" in state` or `state.sessionName === undefined` — the natural TypeScript idioms for an optional property — takes the wrong branch against cyrup, which always supplies the key with a `null` value. Same for `sessionFile` on an ephemeral (`--no-session`) run. Byte-level golden comparisons against a pi transcript also fail.

**Fix** — Build `state_view` (`rpc.rs:1410-1437`) from a `serde_json::Map` and insert `sessionFile`/`sessionName`/`model` only when `Some`, or define a `#[derive(Serialize)]` struct with `skip_serializing_if = "Option::is_none"` on the optional members and serialize that. Audit the remaining `json!` payloads in `handle` for the same shape while there, including the `fork` veto path at `rpc.rs:1276-1281`.

**Verify** — An RPC test that runs `get_state` on a `--no-session` unnamed session and asserts `resp["data"].get("sessionFile").is_none()` and `.get("sessionName").is_none()`.

## SEAM-054 — A blank stdin line is silently dropped instead of producing pi's parse error response

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-modes/src/rpc.rs:1452-1476` — `read_lines` strips the LF and any trailing CR, then `if buf.is_empty() { continue; }` (`:1465-1467`) drops the record before it is ever forwarded to the command loop; the function doc at `:1449-1451` states the filtering. No response is written, so a client that sent a line receives nothing at all for it.

**upstream** — `pi/packages/coding-agent/src/modes/rpc/jsonl.ts:25-41` (v0.84.1) — `emitLine` is invoked for EVERY newline-delimited slice including an empty one, with no emptiness filter (`:38`). It lands in `handleInputLine` (`rpc-mode.ts:748-762`), where `JSON.parse("")` throws and pi writes `error(undefined, "parse", \`Failed to parse command: ${…}\`)` on stdout (`:752-758`).

**Impact** — A client using a bare newline as a keepalive, or one that accidentally emits a trailing blank line, gets a reply from pi and silence from cyrup. Any correlation-by-count logic (n lines in, n responses out) desynchronizes, and a client waiting on a response for a line it believes it sent hangs. cyrup already produces pi's exact `parse` error for every other malformed line (`rpc.rs:939-950`), so this is one input class out of step with the rest of the surface.

**Fix** — Delete the `if buf.is_empty() { continue; }` guard at `rpc.rs:1465-1467` and update the doc at `:1449-1451`; `dispatch`'s existing `serde_json::from_str` failure arm (`rpc.rs:939-950`) then produces `{"type":"response","command":"parse","success":false,"error":"Failed to parse command: …"}` with no id, exactly as pi does. `is_inline_command` (`rpc.rs:881-890`) already treats an unparseable line as non-inline, so the empty line routes down the concurrent path with no other change.

**Verify** — An RPC test writing `"\n"` followed by a valid `get_state` line and asserting TWO responses come back, the first with `command == "parse"` and `success == false`.

## SEAM-055 — Extension slash commands are advertised over RPC with a synthesized empty-path sourceInfo

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup-session-svc/src/session.rs:2283-2293` — the extension branch of `slash_command_catalog` emits a hard-coded `"sourceInfo": {"path": "", "source": "extension", "scope": "temporary", "origin": "top-level"}` for every registered command. The doc immediately above (`:2277-2280`) claims the synthetic info is "anchored at the extension id", which the literal empty `path` contradicts. The prompt-template and skill branches DO carry real provenance (`t.origin.source_info_json(&t.path)` at `:2302`, `s.origin.source_info_json(&s.skill_md)` at `:2311`), so only the extension branch is blank. The owning `ExtensionId` is available at the source and discarded: `RegistryInner::commands` stores it (`cyrup/crates/cyrup-ext/src/registry.rs:139`, written at `:452`) and `command_descriptions()` (`:662-669`) maps it away.

**upstream** — `pi/packages/coding-agent/src/modes/rpc/rpc-mode.ts:681-686` passes `sourceInfo: command.sourceInfo` straight through from the `ResolvedCommand`; the type is required, not optional (`RpcSlashCommand.sourceInfo: SourceInfo`, `rpc-types.ts:79-88`), and `SourceInfo.path` is a non-optional `string` (`core/source-info.ts:6-12`) that `createSyntheticSourceInfo` (`:24-40`) takes as its first positional argument precisely so a synthetic entry still names something.

**Impact** — An RPC client rendering a command palette grouped or filtered by source path — the reason `sourceInfo` is on the wire at all — cannot tell which extension contributed which command; every extension command reports the same empty path. A client keying a trust or enable/disable UI off `sourceInfo.path` collapses all extension commands into one bucket.

**Fix** — Have `command_descriptions()` (`registry.rs:662-669`) return the owning `ExtensionId` it already holds — or switch the caller to `resolved_commands()` (`registry.rs:462`), which carries `owner` on every `ResolvedCommand` — and populate `"path"` at `session.rs:2288` from the extension's resolved path/id. Lands naturally with SEAM-048, which touches the same two functions.

**Verify** — Register a native extension with a known path that declares one command; assert the `get_commands` response entry has `sourceInfo.path` equal to that extension's path rather than `""`.

## SEAM-056 — pi's "session has not been saved yet" fork/clone guard is absent

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** high (which error text cyrup actually surfaces was not observed — only that pi's sentence has zero occurrences in `crates/`)

**cyrup** — `grep -rn 'has not been saved yet\|Wait for the first assistant' crates/` returns ZERO hits workspace-wide. The persisted fork arm is `cyrup/crates/cyrup-session-svc/src/runtime.rs:547-558`: it goes straight to `let mut mgr = SessionManager::open(&file)?;` (`:549`) with no existence pre-check, so a persisted session whose file has not been written yet (cyrup defers the first write until an assistant message exists — see the `create_branched_session` note at `runtime.rs:560-573`) surfaces whatever `SessionManager::open` returns rather than an actionable message. The RPC verbs relay it verbatim: `fork` at `cyrup/crates/cyrup-modes/src/rpc.rs:1283` and `clone` at `:1301`.

**upstream** — `pi/packages/coding-agent/src/core/agent-session-runtime.ts:312-316` — inside the persisted, has-target-leaf branch, `if (!existsSync(currentSessionFile)) { throw new Error("This session has not been saved yet. Wait for the first assistant response before cloning or forking it."); }`, sitting immediately above the `SessionManager.open` at `:317`.

**Impact** — `/fork` or `/clone` on a brand-new session before its first assistant response is a normal user mistake. pi tells the user exactly what to do; cyrup surfaces a filesystem error string that names an internal path and gives no remedy. Over RPC the same string is what a client shows.

**Fix** — Add the existence check ahead of `runtime.rs:549`, returning a dedicated `SessionServiceError` variant whose `Display` is pi's sentence verbatim. Place it inside the `(Some(leaf), Some(file))` arm only, matching pi's placement inside the has-target-leaf branch.

**Verify** — Runtime test: build a persisted session, do not prompt, call `clone`; assert the error message is pi's exact sentence rather than an IO error. Same over RPC via the `clone` verb's `error` field.

## SEAM-057 — --json, --rpc and --output-format are cyrup-invented flags occupying the extension-flag namespace

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup/src/cli.rs:103-111` declares `--output-format <text|json>`, `--json` and `--rpc`, all three documented in-tree as back-compat aliases; they are honoured in `resolve_app_mode` (`cli.rs:571-585`: `cli.rpc ||` at `:572`, `cli.json ||` at `:575`, `cli.output_format ==` at `:575` and `:578`). All three are listed in `KNOWN_LONG_FLAGS` (`cli.rs:762-764`) and `--output-format` also in `KNOWN_VALUE_LONG_FLAGS` (`:806`), so `partition_extension_flags` (`cli.rs:708-721`) consumes them before the extension-flag capture. None appears in `render_help`'s Options block (`cli.rs:828-930`), so they are undiscoverable.

**upstream** — `git grep -nE '"--output-format"\|"--json"\|"--rpc"' v0.84.1 -- packages/coding-agent/src` returns only `auth-command.ts:82-84` (an auth SUBCOMMAND flag) and three npm/ripgrep argv strings; pi's `parseArgs` has no such arm at either tag. Each would therefore fall through to the unknown-long-flag arm at `cli/args.ts:188-201`, be recorded in `unknownFlags`, and — with no extension registering it — produce `Unknown option(s): --json` from `core/agent-session-services.ts:119-124` followed by `process.exit(1)` at `main.ts:844-848`.

**Impact** — Two divergences on a normal path. An extension that legitimately registers a `--json` or `--rpc` flag can never receive it under cyrup: the binary consumes the flag before the extension-flag capture runs and silently changes the output mode instead. And `cyrup --json` succeeds where `pi --json` is a hard exit-1 error, so a script relying on pi rejecting a removed or typo'd flag gets a silently different output format from cyrup.

**Fix** — Decide and record: either delete the three aliases from `Cli` (`cli.rs:103-111`), `resolve_app_mode` (`:571-585`) and `KNOWN_LONG_FLAGS`/`KNOWN_VALUE_LONG_FLAGS` (`:762-764`, `:806`) so the extension-flag namespace matches pi's, or keep them and document them in `render_help` plus state the divergence in-tree. The reserved-namespace half is the behavioural one and should be closed regardless.

**Verify** — Register a native extension declaring a boolean flag named `json`; run `cyrup --json` and assert the extension observes the flag (or, if the aliases are removed, that `cyrup --json` with no such extension exits 1 with `Unknown option(s): --json`).

**AMENDMENT 2026-08-14 (mechanical CLI-surface enumeration) — one detail this item does not carry.** `--output-format` is in `KNOWN_VALUE_LONG_FLAGS` (`cli.rs:862` at HEAD) but is **absent from `diagnostics.rs`'s `VALUE_LONG_FLAGS`** (`:71-89`). Every other value-taking long flag is listed there, which is what makes arg-leniency pass the flag AND its next token through verbatim; without it, `cyrup --output-format -x` reports `Unknown option: -x` instead of treating `-x` as the flag's value. So the third invented flag is not merely undiscoverable — it behaves unlike every real value-taking flag in the same parser. **No new id: this is the same three flags and the same decision `SEAM-057` already carries.** Whichever way the owner decides, this line goes with it — if the aliases are kept, `--output-format` belongs in `VALUE_LONG_FLAGS`; if they are deleted, the omission disappears with them. Line numbers here are HEAD's; the body above cites the older `cli.rs:103-111` / `:762-764` offsets.

## SEAM-058 — pi's experimental server/client command tree, create-harness.ts and remote-session.ts have no counterpart

**`tracker`** — not counted in this area's 40 open items. **Kind** tracking *(was upstream-drift)* · **Severity** n/a *(was low)* · **Effort** n/a until triggered · **Confidence** high

> **Reclassified in the 2026-08-12 repair pass.** The item proposes no work by its own Fix line
> ("Track, do not build, until upstream wires it into `main()`"), and its Impact line says "Today:
> none user-visible, because upstream has not wired the tree into `main()` either". A backlog row
> that instructs the reader not to build it is bookkeeping, not backlog. It keeps its ID and body; the
> recurring action it owes is the re-diff in its Verify line, which the next version-lag sweep runs.
> **Escalate it back into the counted set the moment `main()` references `experimentalCli`.**

**cyrup** — `cyrup/crates/cyrup/src/subcommands.rs` declares only `PackageCommand` and `UpdateTargetSel`; there is no `server`/`client` verb, no `--listen`/`--connect` option, and no `TransportAddress` type anywhere in `crates/`. `grep -rn 'unix://' crates/` finds nothing resembling pi's transport-address grammar. There is no `cyrup-protocol` or `cyrup-client` crate among the 18 crates.

**upstream** — `packages/coding-agent/src/cli/experimental/` is NEW after v0.83.0 (first added 2026-08-02, `1ee411a28`): `cli.ts:7` composes `piCommand.command(serverCommand).command(clientCommand)`; `commands/server.ts:25-44` declares `--listen` plus `--auth-token`/`--auth-token-file`; `commands/client.ts` declares `--connect`; `command.ts` is a 205-line option/subcommand combinator; `transport-address.ts` parses `unix://…`. `packages/coding-agent/src/server/create-harness.ts` is NEW (2026-08-06, `6fb2d766a`). `packages/coding-agent/src/client/remote-session.ts` imports `@earendil-works/pi-client` and `@earendil-works/pi-protocol`, both packages entirely absent at v0.83.0 (`git ls-tree -r --name-only v0.83.0 -- packages/protocol packages/client` is empty). **Reachability re-checked**: at v0.84.1 `git grep -n experimentalCli v0.84.1 -- packages/` returns only `cli/experimental/cli.ts:7` and `test/experimental-cli-command.test.ts`, and `git grep -n 'create-harness\|createHarness' v0.84.1 -- packages/coding-agent/src` returns nothing — none of it is reachable from `main()` yet.

**Impact** — Today: none user-visible, because upstream has not wired the tree into `main()` either — which is exactly why this is `low` and not `medium`. It is recorded so the surface is tracked before it becomes reachable: once pi routes `pi server --listen unix://…` from `main`, cyrup will be missing an entire CLI verb, its transport-address grammar, its auth-token options and the framed wire behind them. The wire-format half is already tracked as PARITY-GAPS VL-P23; this entry is the CLI/harness half VL-P23 does not cover.

**Fix** — Track, do not build, until upstream wires it into `main()`. When it does: the CLI half is a `subcommands.rs` verb plus a `TransportAddress` parser, the harness half maps onto the existing `SessionFactory`/`AgentSessionRuntime` seam, and the wire half needs the `cyrup-protocol`/`cyrup-client` crates VL-P23 describes — which must NOT be conflated with `crates/cyrup-intercom`'s line/JSON framing.

**Verify** — Re-diff `packages/coding-agent/src/{cli/experimental,server,client}` at the next upstream tag and check whether `main()` references `experimentalCli`; escalate the severity when it does.

## SEAM-060 — get_tree drops pi's labelTimestamp from every node

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

> Recovered by the refuter. This sits squarely in the inner-payload class the audit's own blind-spot
> list declared unaudited (`tree_json()` nodes vs `SessionTreeNode`), so nothing in the item-driven
> pass would have caught it.

**cyrup** — `cyrup/crates/cyrup-session-svc/src/session.rs:2330-2347` — `tree_json`'s inner `node_to_json` inserts only `entry`, `children` and (when present) `label`; the omission is stated as deliberate in the doc immediately above at `:2327-2329` ("The optional Pi `labelTimestamp` is omitted"). It reaches the wire verbatim at `cyrup/crates/cyrup-modes/src/rpc.rs:1330-1338` (`get_tree` → `{tree, leafId}`). Per the no-accepted-divergence rule an in-tree comment declaring a field "omitted" is work, not a blessing; the underlying `cyrup_session::manager::TreeNode` needs the field before `tree_json` can carry it.

**upstream** — `pi/packages/coding-agent/src/core/session-manager.ts:159-166` — `interface SessionTreeNode { entry; children; label?: string; labelTimestamp?: string }`, genuinely populated rather than vestigial: `labelTimestampsById` is maintained at `:865`, `:970` and `:1247-1250` and read into the node at `:1318`. The wire contract names the type directly (`modes/rpc/rpc-types.ts:202-208`, `data: { tree: SessionTreeNode[]; leafId: string | null }`).

**Impact** — An RPC client cannot tell when a branch label was set, so it cannot sort or age branch labels, cannot show "renamed 2 days ago", and cannot detect a label that predates the entries beneath it. A client written to pi's `SessionTreeNode` reads `undefined` for a field pi always supplies on labelled nodes.

**Fix** — Add `label_timestamp: Option<String>` to `cyrup_session::manager::TreeNode` alongside the existing `label`, populate it wherever the label itself is populated, and emit it from `node_to_json` (`session.rs:2330-2347`) with an omit-when-`None` insert (matching the `label` treatment); delete the "omitted" note at `:2327-2329`.

**Verify** — Label a branch over RPC, then `get_tree`, and assert the labelled node carries a `labelTimestamp` whose value matches the label operation's timestamp, and that unlabelled nodes carry neither key.

## SEAM-069 — The trust prompt's saved-decision line never says "inherited from", so an ancestor-folder decision is indistinguishable from one made for this folder

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup/src/startup_ui.rs:174-186` — `format_saved_trust` renders `format!("{label} ({})", entry.path.display())` for every saved entry, with no comparison against the current folder's trust path. The entry it is handed comes from `trust_store.nearest(&dirs.cwd)` (`main.rs:1144`), which by construction may resolve to an **ancestor** directory.

**upstream** — `pi/packages/coding-agent/src/modes/interactive/components/trust-selector.ts:21-30` @v0.83.0 (identical at v0.84.1) — `formatDecision(trustPath, decision)` returns `"none"` for null, and otherwise branches: `if (trustPath !== undefined && decision.path !== trustPath) return \`${label} (inherited from ${decision.path})\`;` before falling through to `${label} (${decision.path})`. The caller passes `this.trustOptions[0]?.savedPath` — the current folder's normalized trust path — as `trustPath` (`:61`).

**Impact** — The "Saved decision:" line reads `trusted (/home/u/work)` whether that decision was made for the current folder or inherited from a parent two levels up. A user cannot tell which folder they are actually changing, on a prompt whose whole job is to make that explicit.

**Fix** — Give `format_saved_trust` the current folder's trust path (`options.first().and_then(|o| o.saved_path.as_deref())`, matching pi's `trustOptions[0].savedPath`) and emit `"{label} (inherited from {path})"` when it differs from `entry.path`. Update the `format_saved_trust_matches_pi` test at `startup_ui.rs:488-501` with the inherited case. Lands with SEAM-064, which rewrites the same option vector.

**Verify** — `cargo test -p cyrup format_saved_trust`, then a live run: save a trust decision for a parent directory, `cd` into a child with `.cyrup/` resources, launch `cyrup`, and confirm the header reads "inherited from &lt;parent&gt;".

## SEAM-070 — `process.title` is never set, so the rpc-mode, subagent-runner and intercom-broker children are indistinguishable from an interactive session in `ps`

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `cyrup/crates/cyrup/src/main.rs:53-57` explicitly declines pi's whole process-identity block: "`process.title` has no std API, and `std::env::set_var` is `unsafe` under edition 2024 … gated as a hard-language limit". Nothing anywhere sets a process name (`rg 'proctitle|prctl|set_process_title|setprogname' crates/` = 0); `cyrup-tui/src/terminal_title.rs` sets the *terminal's* OSC title, a different surface. The two hidden re-exec subcommands (`cyrup/crates/cyrup/src/subagent_runner_cmd.rs:33`, `intercom_broker_cmd.rs:15`) re-exec `current_exe()` and therefore inherit the bare `cyrup` name.

**upstream** — `pi/packages/coding-agent/src/bun/cli.ts:5` @v0.83.0 `process.title = APP_NAME;` (and `src/cli.ts:12`, so it is not Bun-only), while `packages/coding-agent/src/rpc-entry.ts:6` sets a **different** value: `process.title = \`${APP_NAME}-rpc\`;`. Documented behaviour since `packages/coding-agent/CHANGELOG.md:3249`. All three files byte-identical at v0.84.1.

**Impact** — The base title is already satisfied for cyrup by accident: a Rust binary's argv[0] is `cyrup`, whereas Node's is `node`, which is why pi needs the assignment at all. What is genuinely lost is the **role suffix** — pi advertises an RPC-mode process as `pi-rpc`, so an operator can `pkill pi-rpc`, spot a stuck RPC child in `ps`, or distinguish it in Activity Monitor without touching a user's interactive session. In cyrup an rpc-mode process, a `__subagent-runner` child and an `__intercom-broker` child all appear as plain `cyrup`, so recovering from a hung background process means killing by PID after reading `ps -f` command lines. Compounds SEAM-047, whose symptom is exactly a background process that will not stop.

**Fix** — Set the per-process name (not the environment) after mode resolution in `crates/cyrup/src/main.rs::run`, via `prctl(PR_SET_NAME)` on Linux and the macOS equivalent — a two-platform `cfg` block or a small crate such as `proctitle`. Use `cyrup` for interactive, `cyrup-rpc` when `--mode rpc` is resolved (mirroring `rpc-entry.ts:6`), and distinct names in `subagent_runner_cmd::dispatch` and `intercom_broker_cmd::dispatch`. **Correct the comment at `main.rs:53-57` in the same change**: its `unsafe`-free rationale covers only the `std::env::set_var` half, and process naming is a syscall on the current process rather than a mutation of the shared environment, so it does not carry that hazard — leaving the comment as written is how this stayed unfiled. The `PI_CODING_AGENT` env half of the same block is already owned by TOOL-031 / PARITY-GAPS PB-5 (area 04); do not re-file it here.

**Verify** — Launch `cyrup --mode rpc`, then `ps -o comm= -p <pid>` returns `cyrup-rpc`; launch an interactive session and it returns `cyrup`. Both return `cyrup` today.

## SEAM-075 — a credential-less INTERACTIVE launch hard-errors instead of opening the TUI, so first-run `/login` is unreachable

**Kind** parity-bug (**regression**, introduced by the `PROV-052` fix) · **Severity** high · **Effort** M · **Confidence** confirmed · **filed 2026-08-13 in the cross-group verification pass**

**cyrup** — `crates/cyrup/src/main.rs:550` builds the interactive runtime as
`AgentSessionRuntime::create(factory, target).await.context("building agent session runtime")?`, with
**no `Err(SessionServiceError::NoModels(_))` arm** — unlike the rpc arm (`:697-703`) and the
print/json arm (`:806-812`), which both `return no_models_available()`. Underneath,
`cyrup-session-svc/src/builder.rs::resolve_model` makes an empty catalog fatal for *every* mode: it
early-returns `Err(SessionServiceError::NoModels(..))` at `:1487` before any mode is consulted.

**upstream** — pi gates the hard stop on the mode: `if (appMode !== "interactive" && !session.model)
{ console.error(chalk.red(formatNoModelsAvailableMessage())); process.exit(1); }` (`main.ts:851-854`
@v0.83.0, **re-read at the tag in this pass**). Interactive is deliberately excluded. `findInitialModel`
returns `{ model: undefined }` as a normal outcome (`core/model-resolver.ts:649-650`), and `sdk.ts:216-218`
turns that into a *banner*, not an error: `model = result.model; if (!model) { modelFallbackMessage =
formatNoModelsAvailableMessage(); }`. A modelless interactive session is a supported state upstream —
it is pi's entire first-run onboarding path.

**Impact** — a new user with no credentials runs bare `cyrup` and gets
`Error: building agent session runtime` and exit 1, with no way to reach `/login`. Before the
`PROV-052` fix this never fired, because the faux test double always supplied a model — so the fix
traded a *misleading* first run (the footer advertising `faux/faux-1` as a live model) for a *blocked*
one. The non-interactive half of `PROV-052` is correct and verified; this is only the interactive half.
Disclosed honestly by that group as unresolved item #2 rather than left to be discovered.

**Fix** — two parts, and the second is why this is M not S. (1) `cyrup-session-svc` needs a modelless
session path: `resolve_model`'s empty-catalog branch must yield `None` plus
`format_no_models_available_message()` as the fallback banner instead of `Err`, which means an
`Option<Model>` on the built session and a decision at every use site about what a turn does with no
model (pi's answer: the send path is what refuses, not the build path). (2) `main.rs:550` then takes
the arm its two siblings already have, but inverted — it *proceeds* rather than exits, surfacing the
banner the way `modelFallbackMessage` is surfaced today. Land it with the first-time-setup wizard
(`crates/cyrup/src/startup_ui.rs`, `crates/cyrup/tests/first_time_setup.rs`), which is the other half
of the same first-run story.

**Verify** — cannot be closed by `cargo test`; this is a TUI surface and needs a real terminal (the
`cyrup-tui` live-render rule). A live run must show: fresh `CYRUP_AGENT_DIR`, no provider credentials
in the environment and no `auth.json`, bare `cyrup` → **the TUI opens** with the no-models banner and
`/login` is usable, NOT `Error: building agent session runtime`. The non-interactive control must keep
its current behaviour: `cyrup -p hi` under the same conditions prints
`No models available. Use /login to log into a provider via OAuth or API key.` on stderr and exits 1
— **measured in this pass** (scrubbed `env -i`, scratch agent dir): exit 1, that exact message on
stderr, empty stdout.

## SEAM-074 — the four shipped built-ins are identified as ambient by an id list in `builder.rs` instead of by `NativeExtension::is_ambient()`

**Kind** cyrup-original · **Severity** low → **medium** (it was not cosmetic) · **Effort** S · **Confidence** confirmed · **filed 2026-08-13 from the SEAM-071 fix**

> **FIXED 2026-08-13** in the cross-group verification pass. Claim re-verified at HEAD first:
> `AMBIENT_NATIVE_IDS` was present at `builder.rs:1755` and `native_survives_no_extensions` read
> `!ext.is_ambient() && !AMBIENT_NATIVE_IDS.contains(&id)`, with no crate overriding `is_ambient`.
>
> **The "cosmetic today" rating was wrong, and the full-workspace suite proved it.** Matching on the
> id also catches an INLINE extension that merely shares the name. `cyrup-session-svc/tests/build_containment_and_flag_diagnostics.rs`
> injects `FailingExt { id: "subagents" }` by hand under `cfg.no_extensions = true`, so the id list
> dropped it from the load entirely and its init failure reached neither the `[Extension issues]`
> panel nor the fatal exit channel: `the_failure_reaches_the_panel_and_the_exit_channel_together`
> was **RED** in the first full run after the SEAM-071 batch — `panel channel lost the failure: []`,
> `left: 0 / right: 1` at `build_containment_and_flag_diagnostics.rs:312`. That is a code-defect, not
> a test-defect: pi separates its tiers by ORIGIN and never by name, so a by-value extension is
> always loaded (`loadFinalExtensionSet` → `loadExtensionFactories` unconditionally,
> `resource-loader.ts:579-581` @v0.83.0, over `main.ts:523`; only `extensionPaths` is collapsed at
> `:451-453`, re-read at the tag in this pass). The SEAM-071 pass missed it because its targeted run
> covered `--lib --test added_tool_names_producer --test integration --test control_ops` only.
>
> **Change** — `fn is_ambient(&self) -> bool { true }` on `PermissionSystemExtension`
> (`cyrup-permission-system/src/extension.rs`), `IntercomExtension` (`cyrup-intercom/src/extension.rs`),
> `SubagentsExtension` and `SubagentPromptRuntime` (`cyrup-ext-subagents/src/{extension,prompt_runtime}.rs`),
> each carrying the pi citation for why it stands in for an installed package; `AMBIENT_NATIVE_IDS`
> deleted and `native_survives_no_extensions` reduced to `if !ext.is_ambient() { return true; }`.
> The `SUBAGENT_CHILD_RUNTIME_NATIVES` carve-out is unchanged and still keyed by id — correct, since
> it is a positive re-injection list for the real built-ins (pi-args.ts:413-417 @v0.47.1).
>
> **Tests** — `builder.rs`'s `StubNative` now carries a tier flag instead of relying on its name, so
> the five SEAM-071 tests pin the discriminator rather than the list; plus a new
> `an_inline_extension_that_shares_a_built_ins_id_is_still_inline`, RED before this change and GREEN
> after. The integration proof is the previously-red target going green:
> `build_containment_and_flag_diagnostics` 10 passed / 0 failed.

**cyrup** — SEAM-071 needed to tell pi's two extension tiers apart: the PATH tier that `noExtensions` reduces (`resource-loader.ts:451-452` @v0.83.0) and the INLINE tier it leaves alone (`loadExtensionFactories`, `:579-581`, over `main.ts:523`'s `[...builtInExtensions, ...options.extensionFactories]`). cyrup routes both through one seam, `SessionBuilder::with_native_extension`, so the tiers are indistinguishable by construction. The fix added the right hook — `NativeExtension::is_ambient()` (`crates/cyrup-ext/src/native.rs`), default `false` = inline tier — but **nothing overrides it**: the four shipped built-ins are recognised by a hardcoded `AMBIENT_NATIVE_IDS` list in `crates/cyrup-session-svc/src/builder.rs` instead (`cyrup-permission-system`, `cyrup-intercom`, `subagents`, `subagent-prompt-runtime`).

**upstream** — n/a; pi's two tiers are separate arrays reaching separate loaders, so it never has to ask an extension which one it is.

**Impact** — cosmetic today and correct in behaviour, but a coupling that rots silently: a fifth built-in, or a renamed extension id, is ambient in fact and inline by the list, and nothing fails. `--no-extensions` then quietly stops matching pi for that extension only.

**Fix** — override `fn is_ambient(&self) -> bool { true }` on `PermissionSystemExtension` (`crates/cyrup-permission-system/src/extension.rs`), `IntercomExtension` (`crates/cyrup-intercom/src/extension.rs`), `SubagentsExtension` and `PromptRuntimeExtension` (`crates/cyrup-ext-subagents/src/{extension,prompt_runtime}.rs`), then delete `AMBIENT_NATIVE_IDS` and its use in `native_survives_no_extensions`. The builder-side tests keep working unchanged if the stubs take the override instead of the ids. **Not done in the SEAM-071 pass: those four crates were outside its file ownership.**

**Verify** — with `AMBIENT_NATIVE_IDS` emptied, `native_survives_no_extensions` still drops all four in a root session under `--no-extensions`, and still keeps an embedder-supplied extension.

## SEAM-071 — `--no-extensions` gates only the WASM/disk discovery roots, so the three native built-ins load anyway and `-ne` cannot produce pi's bare session

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed

> **FIXED 2026-08-13.** Claim re-verified at HEAD before the change: `builder.rs`'s native loop was
> `for ext in self.native_extensions` with no reference to `cfg.no_extensions`, and
> `extension_discovery_roots` was the flag's only consumer. Both confirmed.
>
> **LIVE VERIFICATION DONE 2026-08-13** (cross-group verification pass) — the check this item's own
> Verify block asked for, which the fixing pass could not perform. It is the one that matters,
> because unit tests pin which natives the *builder selects* and only a live run proves no broker
> PROCESS is spawned. A/B on the flag alone: same binary, same scratch `CYRUP_AGENT_DIR`, same
> `--model together/openai/gpt-oss-20b -p "say ok"`, `CYRUP_INTERCOM=1`, `--no-session`.
> **CONTROL** (extensions enabled): turn succeeds, and `ps -axo pid,command | grep -cE '[/]cyrup
> __intercom-broker'` goes **0 → 1** — a real detached broker (`38135 …/cyrup __intercom-broker`)
> that OUTLIVES the parent's exit, i.e. the immortal-broker mechanism this item was filed about,
> observed directly. **TREATMENT** (`--no-extensions`, everything else byte-identical): turn
> succeeds identically (`ok`, exit 0), stderr contains **zero** mentions of intercom, and the broker
> count stays **0 → 0**. The extension is not merely deselected — it never initialises, so it never
> reaches its spawn.
>
> A first attempt at this A/B was VACUOUS and is recorded so nobody repeats it: under a scrubbed
> `env -i` with no credentials, `PROV-052`'s no-models guard exits before extension init, so both
> arms trivially show 0 brokers. A credential is required for the control to mean anything. The
> second attempt was vacuous for a different reason and turned up a real bug: with a long scratch
> agent-dir path the broker died at startup in the CONTROL too — which is **`ICOM-052`
> (missing `SUN_LEN` guard), independently reproduced** and raised low → medium in `REPRO-LOG.md`.
> Only with a short agent dir (`/tmp/cy1/a`) does the control spawn a surviving broker, which is
> what makes the A/B above decisive.
>
> **The item's mechanism is half right, and the missing half is what makes the fix safe.** pi does
> not have ONE extension set that `noExtensions` reduces — it has two tiers and gates only one. The
> PATH tier is reduced to the explicit `-e` paths (`const extensionPaths = this.noExtensions ?
> cliEnabledExtensions : this.mergePaths(cliEnabledExtensions, enabledExtensions)`,
> `resource-loader.ts:451-452`, and again at `:555-557` for the reload path @v0.83.0); that is where
> installed packages live, and the item is right that `@gotgenes/pi-permission-system`, pi-intercom
> and pi-subagents all live there. But the INLINE tier is untouched: `loadFinalExtensionSet` calls
> `loadExtensionFactories(...)` with **no `noExtensions` check** (`:579-581`) over
> `extensionFactories = [...builtInExtensions, ...(options?.extensionFactories ?? [])]`
> (`main.ts:523`) — pi's own `llama.cpp` provider extension plus anything an embedder passed
> programmatically. An extension the caller handed over by value is not something a flag about
> *discovery* can be about.
>
> cyrup routes BOTH tiers through the same `SessionBuilder::with_native_extension` seam, because it
> compiles in what pi installs. A blanket "skip `self.native_extensions` when `no_extensions`" —
> route (b) as the item words it — therefore gates pi's inline tier too, and that is not
> theoretical: **ten test files in this workspace set `cfg.no_extensions = true` *and* inject their
> own probe extension**, and the first attempt at this fix turned three of them red
> (`added_tool_names_producer.rs`, all three cases: `the extension tool is active: ["read", "bash",
> "edit", "write"]`). Those tests are correct and pi-faithful; the fix was not.
>
> **What shipped.** `natives_to_load` (`builder.rs`, called from the step-4b native loop) drops a
> native only when it is AMBIENT — `cyrup_ext::NativeExtension::is_ambient()` (new,
> `crates/cyrup-ext/src/native.rs`, default `false` = pi's inline tier) or a member of
> `AMBIENT_NATIVE_IDS` (`cyrup-permission-system`, `cyrup-intercom`, `subagents`,
> `subagent-prompt-runtime`). The id list is an explicit stopgap: the answer belongs on each of those
> four crates as an `is_ambient()` override, which this pass could not write because those crates
> were outside its file ownership — **filed as SEAM-074**, and the list deletes itself when that
> lands.
>
> **Route (b) was taken over the item's preferred (a), and the placement is right even though the
> first attempt at it was not.** pi does not gate at its CLI: `main.ts:720` passes `noExtensions`
> straight into the RESOURCE LOADER, which applies it at the single point where the extension set is
> computed. `builder.rs` is cyrup's resource loader. Gating at four `*_extension_for_env` call sites
> × three session-build sites in `main.rs` would be twelve places holding one rule. (Ownership also
> forced it: this pass owns the extension-loading functions in `builder.rs`, not `main.rs`.)
>
> **Three loading paths were closed, not one.** The item found the first; the other two are the same
> defect and were found while fixing it.
> 1. The native loop — now `natives_to_load(self.native_extensions, cfg.no_extensions, is_child)`.
> 2. **The package tier.** `ext_roots.configured.extend(report.registry.ext_crate_paths)` appended
>    every installed package's extension dir into the one tier `--no-extensions` is defined to KEEP.
>    That tier is pi's `enabledExtensions` — literally the operand `noExtensions` drops in
>    `mergePaths(cliEnabledExtensions, enabledExtensions)` — so the flag was re-admitting through the
>    back door exactly what it nulled at the front. Now gated.
> 3. **The pre-trust vote** (`pre_trust_extension_verdict`). A native that `--no-extensions` will not
>    load was still voting on the project-trust decision. pi's pre-trust pass reads the SAME reduced
>    `extensionPaths` its main pass does (`resource-loader.ts:451-455`), so a dropped extension
>    cannot vote. Now filtered by the same predicate.
>
> **The subagent-child carve-out, decided and documented as the item requires — and it DIFFERS from
> what the item assumed.** pi's subagent launcher pairs `--no-extensions` with an explicit
> re-injection of exactly three extensions as `--extension <path>` args: `runtimeExtensions =
> [PROMPT_RUNTIME_EXTENSION_PATH, FANOUT_CHILD_EXTENSION_PATH when fanout-authorized,
> resolvePermissionSystemExtension()]` (pi-subagents v0.47.1 `src/runs/shared/pi-args.ts:413-417`),
> emitted at `:556-560` immediately after the `--no-extensions` at `:557`. cyrup selects those same
> three by ENV rather than by path (`main.rs:480-506`), because its child-side runtime is compiled in
> rather than loadable — env selection IS cyrup's re-injection channel. So
> `SUBAGENT_CHILD_RUNTIME_NATIVES` exempts `cyrup-permission-system`, `subagent-prompt-runtime` and
> `subagents`, **and only in a child** (`CYRUP_SUBAGENT_CHILD`); a root session drops all three.
> Without that exemption the permission gate would fail OPEN in a child launched with a pinned
> `agent.extensions` allowlist — strictly worse than the over-loading this item is about.
>
> **`cyrup-intercom` is NOT exempt**, contradicting this item's Fix note ("a `__subagent-runner`
> child attaches intercom unconditionally so `contact_supervisor` exists … those two must survive
> `-ne`"). pi's `runtimeExtensions` list does not contain pi-intercom, so an upstream child launched
> under `disableAmbientExtensions` loses it and its supervisor channel too. That is parity, and it is
> the branch that was actually producing brokers. The cyrup mechanism the item cited
> (`cyrup-intercom/src/extension.rs:628-630`, `:670-673` at HEAD — a child with orchestrator metadata
> attaches regardless of `is_installed`) is real, but it is an INSTALL gate, not an extension-set
> gate; `--no-extensions` sits above it. Overturning this decision is one entry in
> `SUBAGENT_CHILD_RUNTIME_NATIVES`.
>
> **Evidence** — five unit tests in `builder.rs`'s existing `mod tests`:
> `every_native_loads_without_no_extensions`,
> `no_extensions_drops_every_ambient_native_in_a_root_session`,
> `an_extension_the_embedder_passed_by_hand_survives_no_extensions`,
> `a_subagent_child_keeps_exactly_the_natives_pi_re_injects`,
> `the_child_exemption_does_not_leak_into_a_root_session`. RED before / GREEN after was measured by
> forcing `natives_to_load` back to its pre-fix "return everything" behaviour:
> `test result: FAILED. 42 passed; 2 failed` → `test result: ok. 45 passed; 0 failed`.
>
> The three integration tests the FIRST (over-broad) attempt broke are green under the shipped fix:
> `cargo test -p cyrup-session-svc --all-features --lib --test added_tool_names_producer
> --test integration --test control_ops` → **45 + 3 + 7 + 11 = 66 passed, 0 failed, exit 0**.
>
> **NOT closed by this fix, and it is the item's own Verify step:** the live assertion
> `CYRUP_INTERCOM=1 cyrup --no-extensions --offline --no-session -p hi` → zero
> `[/]cyrup __intercom-broker` processes. The unit tests pin the SELECTION; only a live run proves no
> broker is spawned. `crates/cyrup/tests/*` and `crates/cyrup/src/main.rs` are outside this pass's
> file ownership, so that run is unperformed here.

**cyrup** — `crates/cyrup-session-svc/src/builder.rs:775` is `for ext in self.native_extensions { … host.load_native_with_services(ext, …) … }` — unconditional; `cfg.no_extensions` is never consulted on that path. The only place the flag acts is `extension_discovery_roots` (`:1677-1690`), which nulls `project_cwd` and `agent_dir` and so suppresses **disk** extensions only. The natives are pushed at `crates/cyrup/src/main.rs:481/:490/:493/:506` (and again at the `:630`/`:724` session-build sites) from `subagent_extension_for_env`, `prompt_runtime_extension_for_env`, `intercom_extension_for_env` and `permission_extension_for_env`; `rg 'no_extensions' crates/cyrup/src/main.rs` returns **nothing**, so none of the four gates sees the flag. A user with `CYRUP_INTERCOM=1` running `cyrup --no-extensions -p hi` therefore still attaches intercom and still spawns a broker.

**upstream** — `pi` v0.83.0 `packages/coding-agent/src/core/resource-loader.ts:451-452`: `const extensionPaths = this.noExtensions ? cliEnabledExtensions : this.mergePaths(cliEnabledExtensions, enabledExtensions);` — under `--no-extensions` the set collapses to the explicitly-passed `-e` paths and nothing else. pi-subagents, pi-intercom and pi-permission-system are ordinary *discovered* extensions upstream, not built-ins, so upstream's `-ne` run has none of the three. That is precisely the guarantee `-ne` exists to give.

**Impact** — `--no-extensions` is the escape hatch a user reaches for when an extension is suspected of breaking a session, and the bisect it is supposed to enable does not work: the three highest-surface extensions in the product — the permission gate, the subagent orchestrator and intercom — are exactly the ones it cannot switch off. Concretely it also means `-ne` cannot be used to get a broker-free one-shot run, which is what forced the four `crates/cyrup/tests/*.rs` fixtures to scrub `CYRUP_INTERCOM`/`CYRUP_SUBAGENTS`/`CYRUP_PERMISSION_SYSTEM` from the child environment instead (ICOM-051, area 11 — that item closes the *test* hole and explicitly does not close this one). Security-adjacent in one direction only: `-ne` currently *keeps* the permission gate on, so this is not a fail-open, but a user who believes `-ne` produced a bare session is wrong about what is loaded.

**Fix** — Consult `cfg.no_extensions` where the natives are selected, not where they are loaded. Either (a) gate the four `*_extension_for_env` calls in `crates/cyrup/src/main.rs` on the resolved `--no-extensions` flag at all three session-build sites, or (b) skip the `self.native_extensions` loop in `builder.rs:775` when `cfg.no_extensions` is set. Prefer (a): the flag is a CLI concern and (b) would also silently drop the subagent-child attachments that are not user-installed extensions at all. **Decide and document the subagent-child carve-out either way** — a `__subagent-runner` child attaches intercom unconditionally so `contact_supervisor` exists (`crates/cyrup-intercom/src/extension.rs:637-640`), and pi's child gets `subagent-prompt-runtime.ts` through `pi-args.ts:13` rather than through discovery, so those two must survive `-ne` to keep parity.

**Verify** — `CYRUP_INTERCOM=1 cyrup --no-extensions --offline --no-session -p hi`, then `ps -axo pid,command | grep -cE '[/]cyrup __intercom-broker'` returns 0 and the session's tool list contains no `intercom`; the same run without `-ne` still attaches it. Plus a `-p cyrup` fixture asserting that under `-ne` the startup extension set is empty even with all three opt-in vars exported.

## SEAM-072 — `build_inputs` read the process's own stdin instead of taking Pi's `stdinContent` argument, so any test that drives prompt assembly hangs forever on an inherited pipe

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** confirmed · **Status** fixed this pass

**cyrup** — `crates/cyrup/src/input.rs:597-614` (pre-fix) ended with `let piped = read_piped_stdin().await?;` *inside* `build_inputs`, and `read_piped_stdin` (`:576-586`) guards only on `std::io::stdin().is_terminal()` before `tokio::io::stdin().read_to_string(&mut buf).await`. A pipe is not a terminal, so on any non-TTY stdin the call blocks until **EOF** — not until any bounded event. `build_inputs` is `pub` and is the documented "exact fn `main.rs` calls" that two integration targets drive directly (`crates/cyrup/src/tests/image_auto_resize_file_args.rs:50`, `crates/cyrup/src/tests/image_bytecap.rs:62`), so those targets inherited whatever descriptor the test runner had on fd 0.

**upstream** — pi v0.83.0 `packages/coding-agent/src/main.ts:819-826` reads stdin in `main` itself — `let stdinContent; if (appMode !== "rpc") { stdinContent = await readPipedStdin(); … }` — and `:828-832` *passes* it in: `prepareInitialMessage(parsed, settingsManager.getImageAutoResize(), stdinContent)`. `stdinContent?: string` is `prepareInitialMessage`'s third parameter (`:169-172`) and flows on to `buildInitialMessage({ parsed, stdinContent })` (`:178`). The descriptor-owning step and the prompt-assembly step are separate functions upstream; cyrup had fused them. cyrup's own doc comment at `input.rs:591-593` already quoted the three-argument upstream call while implementing a two-argument one.

**Impact** — Measured, not theorised. `cargo test -p cyrup --test image_auto_resize_file_args` with stdin at `/dev/null`: **2 passed in 4.77 s**. The identical command with an open pipe on stdin (`sleep 300 | cargo test …`): **both tests still "running for over 60 seconds"**, sampled to `Runtime::block_on` → `Context::park` → `kevent`, and never terminating. It hung the second full-workspace verification run at target 8 of ~338 for 11 minutes until killed. Because `cargo test` has no per-test timeout, the failure mode is a **silent indefinite stall that names nothing** — strictly worse than a red, and the reason a `cargo test --workspace` gate could not be trusted: whether the suite finished at all depended on the CI runner's fd 0. This is very likely the same root cause as the previously-unexplained `one_shot_parity::an_unmatched_models_pattern_warns_on_stderr` "running for over 60 seconds" report, since the whole workspace run shares one inherited descriptor.

**Fix** — *applied.* Restored pi's split: `read_piped_stdin` is now `pub` and is called by `crates/cyrup/src/main.rs` at both prompt-building sites (the interactive arm and the Print/Json arm — RPC still never reads it, matching `main.ts:820`'s `appMode !== "rpc"` guard), and `build_inputs(cli, cwd, auto_resize, piped: Option<String>)` takes the content as its fourth argument, mirroring `prepareInitialMessage`'s `stdinContent`. Nothing about the runtime behaviour of the binary changes — the same read happens at the same point in the same order — only the ownership of fd 0 moves out of the reusable function.

**Verify** — done. `sleep 200 | cargo test -p cyrup --test image_auto_resize_file_args --test image_bytecap` → 2 passed / 1 passed, 4.88 s and 5.51 s, i.e. the previously-fatal condition is now inert. Whole package under the same open pipe: `cargo test -p cyrup` → 17 targets, all `ok`, 221 tests, no stall — including `piped_stdin_trim.rs`, which drives the real binary through a real pipe and therefore proves the production stdin path is unchanged. Audit of the remaining process-global stdin readers: `cyrup-tui/src/drain.rs:154` and `terminal_query.rs:386` both gate on `is_tty() && is_raw_mode_enabled()` and `main.rs:674` is RPC-only, so `input.rs` was the sole unguarded blocking reader.

## SEAM-073 — Every `-p cyrup-session-svc` run leaves 14 temp directories behind, because `FileLock` never deletes its lock file and something touches the models store after the fixture's `TempDir` teardown starts

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed (the leak and its contents are measured; the exact late writer is inferred)

**cyrup** — `crates/cyrup-config/src/lock.rs`: `FileLock::acquire` opens the lock path with `.create(true)` (`:25-34`) and `impl Drop` calls `FileExt::unlock` only (`:42-46`) — the file itself is never removed. Both `FileModelsStore::read` and `::write` take that lock (`crates/cyrup-config/src/models_store.rs:91` and `:100`), so *reading* the store is enough to create `<agent_dir>/models-store.json.lock`. Measured: `cargo test -p cyrup-session-svc` leaves exactly **14** `$TMPDIR/.tmpXXXXXX` directories per run, and every one of them has identical contents — an empty `project/` and `agent/models-store.json.lock`, nothing else. `TempDir::drop`'s `remove_dir_all` swallows its error, so a store access landing after the directory scan begins leaves the whole tree behind silently. `cargo test -p cyrup-tui --lib`, whose fixtures build sessions the same way, leaks **0**, which is what makes this specific to whatever session-svc keeps running past the fixture's drop rather than to the fixture shape.

**upstream** — no counterpart: pi has no lock file beside `models-store.json` (`packages/coding-agent/src/core/models-store.ts` writes it directly), so both the stale-lock artefact and this leak are cyrup-original.

**Impact** — Small but monotone: 14 directories per session-svc run and 15 per full-workspace run (the extra one is a `cyrup-bash-*.log` spool), each ~3 inodes, forever, in a directory macOS only sweeps on reboot. Three consecutive full-workspace runs measured +15, +15, +15. The user-facing residue is the never-deleted `~/.cyrup/models-store.json.lock`, which is cosmetic. What makes it worth an ID is the mechanism rather than the bytes: it is direct evidence that some session-svc task still runs after the session and its whole agent directory are gone, which is the same class of lifetime bug as SEAM-S03.

**Fix** — Two independent halves. (a) Give `FileLock::drop` a best-effort `std::fs::remove_file(&self.path)` after the unlock, or hold the lock on `models-store.json` itself rather than a sidecar. (b) Find and join (or cancel) whatever performs a models-store access after `AgentSession` drop — instrument `FileLock::acquire` with a `tracing::trace!` carrying a backtrace and re-run `-p cyrup-session-svc` to name it; that half is the real defect and (a) only hides it.

**Verify** — `ls $TMPDIR | wc -l` before and after `cargo test -p cyrup-session-svc` differs by 0. Today it differs by 14.

## SEAM-076 — `install` / `remove` help claims the source is written to settings — **CLOSED 2026-08-15 (REFUTED — already done at HEAD)**

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup/src/subcommands.rs:303` prints `Install a package and add it to settings.` and `:307` prints `Remove a package and its source from settings.` The `run()` path (`:425-465`) constructs only a `PackageStore` + `PackageManager`; the sole write is `lock::save(&reg_path, &reg)` into `packages.json` (`crates/cyrup-resources/src/package/install.rs:148-153`). `SettingsManager` is imported at `:18` but used only by `cyrup config` (`:591`). **Verified empirically:** installing a local package under an isolated `CYRUP_AGENT_DIR` created `<agent_dir>/packages/packages.json` and wrote no settings file.

**upstream** — pi's install *does* write the source into settings, which is where the string comes from. cyrup deliberately uses a separate file-backed registry, recorded at `crates/cyrup-session-svc/src/builder.rs:936-945`. The mechanism divergence is intended; only the help text is stale.

**Impact** — a user following the help inspects or edits `settings.json`, finds nothing, and may hand-add a `packages` entry — which is a **different, additive** channel (`crates/cyrup-config/src/settings.rs:343-373`), producing a duplicate rather than a correction.

**Fix** — reword both lines to name the registry, e.g. `Install a package and record it in the package registry.`

**Verify** — `cyrup install ./p` under an isolated agent dir; assert `settings.json` is absent or unchanged, `packages.json` gained the row, and the help text no longer says "settings".

## SEAM-077 — `cyrup remove --help` advertises an `npm:` example that `install` rejects — **CLOSED 2026-08-15 (REFUTED — already done at HEAD)**

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup/src/subcommands.rs:307` shows the remove examples `cyrup remove npm:@foo/bar` and `cyrup uninstall npm:@foo/bar`. `PackageSource::parse` hard-rejects any `npm:` prefix with `ResourceError::UnsupportedNpm` (`crates/cyrup-resources/src/package/source.rs:78-81`) — there is no JS runtime. The comment at `subcommands.rs:297-302` records that the npm example was **deliberately deleted from the install help for exactly this reason**; the remove help was not given the same treatment and carries it twice.

**upstream** — pi supports npm sources, hence the examples. `CFG-009` (closed) owns the error-message half; `PARITY-GAPS` PB-7 / OQ-1 own the channel decision. Neither touches help text.

**Impact** — the only two examples the remove help shows are for a source class that can never have been installed, so a user copying either gets a failure with no working example to fall back on.

**Fix** — replace both npm examples with the git and path forms the install help already uses.

**Verify** — assert `cyrup remove --help` contains no `npm:` and at least one example `PackageSource::parse` accepts.

**Note** — the comment at `subcommands.rs:301` cites `gap-analysis 13-cyrup §D`, a document that does not exist in this directory. Two other source files cite it as well (`crates/cyrup/src/tests/image_bytecap.rs:1`, `crates/cyrup-session-svc/src/tests/install_noop.rs:1`). Per `README.md`'s third-edition rule — *grep the SOURCE for `AREA-NNN` citations at every reconciliation* — those three citations need an owner or an explicit strike.

## SEAM-078 — `cyrup update` advertises four self-update flags over an unimplemented stub — **CLOSED 2026-08-15 (REFUTED — already done at HEAD)**

**Kind** cyrup-original · **Severity** medium · **Effort** M · **Confidence** confirmed

**cyrup** — `crates/cyrup/src/subcommands.rs:285-289` documents `--self`, `--all`, `--force` and the bare `cyrup update` / `cyrup update pi` forms as updating cyrup itself. Every one of those paths reaches `:573-575`, which prints `Self-update is not available in this build; update cyrup via your package manager.` and stops. **Observed:** `cyrup update` prints `Extensions are skipped. Run cyrup update --extensions to update extensions.` followed by that line.

**upstream** — pi implements self-update against a release feed. The release-feed **poll** was deliberately excluded with a written rationale (`crates/cyrup/src/update_check.rs:14-23` — no cyrup release endpoint exists) and is recorded as *rejected, flagged for a human* in `12-upstream-drift-pi-core.md`'s rejected list; `TUI-S11` records the same split. **Neither covers the `update` subcommand's own advertised surface**, which is what this item files.

**Impact** — the remedy is wrong for the only supported install path: there is no package manager, so a user who follows the message has no route at all. The correct instruction today is to re-run `cargo install --git https://github.com/cyrup-ai/cyrup cyrup`. Four advertised flags resolve to a stub.

**Fix** — smallest correct change is to reword the stub to name a route that works from a source install, and to mark the four flags as unavailable in `--help` rather than documenting them as functional. Implementing self-update is blocked on the same missing release endpoint as the rejected poll and should remain an owner decision, not agent work.

**Verify** — assert `cyrup update --self` exits with a message naming a working route, and that `--help` does not present the flags as functional.

## SEAM-079 — `cyrup config --help` runs the picker instead of printing help — **CLOSED 2026-08-15 (REFUTED — already done at HEAD)**

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — the top-level help advertises `cyrup <command> --help   Show help for install/remove/uninstall/update/list/config/auth`, but the `config` handler (`crates/cyrup/src/subcommands.rs:590-675`) has no `--help` branch; the flag falls through and the interactive picker runs. **Observed:** `cyrup config --help` printed `No configurable skills, prompts, or themes found.`

**upstream** — not re-read this pass. This area's own `## Coverage` already declares the blind spot: *"(c) `cyrup config` (`subcommands.rs:590-675`) was checked against `ConfigSelectorOptions` but its interactive body was not driven."*

**Impact** — the one subcommand whose flag set is least guessable — `-l`/`--local`, `--approve`/`--no-approve`, and the `+pattern`/`-pattern` marker semantics it writes into the `skills`/`prompts`/`themes` arrays — is the one that cannot be asked. In a terminal it silently enters a full-screen picker; in a pipeline it emits the no-resources line and exits 0, which reads as success.

**Fix** — add a `--help`/`-h` branch at the top of the `config` handler mirroring the other five subcommands, documenting `-l` and the marker convention.

**Verify** — `cyrup config --help` prints usage and exits without entering the picker.

## SEAM-080 — `model_changed` is a cyrup-invented line on the RPC stdout stream, and two backlog items already reason about it as upstream — **CLOSED 2026-08-15 (FIXED — filtered off the wire, option (a); see the table row)**

**Kind** cyrup-original · **Severity** medium · **Effort** M · **Confidence** confirmed · **filed 2026-08-14 (ext-rpc surface enumeration)**

**cyrup** — `AgentSessionEvent::ModelChanged` is declared at `crates/cyrup-session-svc/src/event.rs:236-239` with its wire tag `"model_changed"` at `:361`, and emitted at `crates/cyrup-session-svc/src/session.rs:4285-4289` via `fanout_emit` — the same fanout `crates/cyrup-modes/src/rpc.rs:888-910` subscribes to and writes out. So an RPC `set_model` or `cycle_model` writes `{"type":"model_changed","provider":…,"model":…}` to stdout **in addition to** its `response`.

**upstream** — pi has no `model_changed` `AgentSessionEvent`. The union is `packages/coding-agent/src/core/agent-session.ts:139-181 @v0.83.0`; it carries `thinking_level_changed` and `session_info_changed` but nothing for the model. `git grep -n 'model_changed\|modelChanged' v0.83.0 -- packages/` hits only `core/cache-stats.ts:22`/`:88` (a boolean field on a cache-miss record) and `interactive-mode.ts:3469`, which reads that field — nothing that is ever an event.

**Impact** — a wire line pi never writes, on the protocol surface whose whole value is that a client written against pi's docs works against cyrup. Worse than merely unknown: **this file's own reasoning already treats it as upstream** — `SEAM-032`'s Impact paragraph and `SEAM-033`'s Impact paragraph, both in this file, list `model_changed` alongside the genuine `session_info_changed` as an event whose loss matters. An invented surface has propagated into the backlog that is supposed to detect invented surfaces, which is the failure mode the `cyrup-original` class exists to catch.

**Fix** — decide, then write the decision down. Either (a) filter `ModelChanged` off the RPC/json wire the way `SessionReplaced` already is at `rpc.rs:908` and `:931`, keeping it as an internal fanout event the TUI consumes — the change is one match arm at each of the two write sites; or (b) keep it on the wire as a documented cyrup extension, with a `CYRUP-DELTA` at `event.rs:236-239` stating that pi has no such event and naming what a pi-written client does with an unknown `type`. Do **not** leave it undecided: whichever way it goes, `SEAM-032`'s and `SEAM-033`'s Impact paragraphs must stop citing it as upstream.

**Verify** — drive `set_model` over RPC and assert the exact set of stdout lines matches what pi emits for the same command; add the assertion beside the existing RPC wire tests in `crates/cyrup-modes/src/tests/modes.rs`.

## SEAM-081 — `session_start` and `session_shutdown` reach RPC stdout; upstream both are extension-runner events `session.subscribe` never sees — **CLOSED 2026-08-15 (FIXED — same predicate as SEAM-080; `json.rs` had no guard at all)**

**Kind** cyrup-original · **Severity** medium · **Effort** M · **Confidence** confirmed · **filed 2026-08-14 (ext-rpc surface enumeration)**

**cyrup** — `AgentSessionEvent::SessionStart` (`crates/cyrup-session-svc/src/event.rs:262-265`, tag at `:365`) is emitted at `crates/cyrup-session-svc/src/session.rs:2798` via `fanout_emit`; `SessionShutdown` (tag at `event.rs:266`) at `session.rs:2655`, likewise. Both therefore land on the stream `crates/cyrup-modes/src/rpc.rs:888-910` writes to stdout.

**upstream** — both names exist at `v0.83.0`, but as **extension events, not session events**. `session_start` is declared at `core/extensions/types.ts:563` and subscribed via `on(event: "session_start", …)` at `types.ts:1192`; every construction site is a `sessionStartEvent:` config field or an `extensionRunner.emit` (`agent-session-runtime.ts:218`/`:251`/`:305`/`:328`/`:347`/`:391`, `agent-session.ts:389`/`:2622`). `session_shutdown` is declared at `types.ts:617`, subscribed at `:1204`, and emitted through `emitSessionShutdownEvent(this._extensionRunner, …)` (`agent-session.ts:2604`) behind `runner.ts:195`'s `hasHandlers("session_shutdown")` gate. **Neither is in the `AgentSessionEvent` union** (`agent-session.ts:139-181`), so `session.subscribe(...)` never sees them and `rpc-mode.ts:355`'s `output(event)` can never write them.

**Impact** — two more stdout lines pi never writes, from the same cause: cyrup routes extension-tier lifecycle events onto the session fanout instead of keeping the two tiers separate. Distinct from SEAM-025, which is about the extension-tier events losing pi's session-file fields — that item assumes the events are correctly *scoped* and disputes only their payload.

**LOAD-BEARING — read before scheduling SEAM-047.** `SEAM-047`'s Verify line proposes asserting *"that a `session_shutdown` line was written to stdout first"* as the acceptance test for the SIGTERM fix. **That would pin a cyrup-invented wire line as required behaviour**, permanently. Whoever lands SEAM-047 must either resolve this item first or re-express the assertion against something pi actually writes. The correct form is already sitting one file over: `12-upstream-drift-pi-core.md:327` — the SAME defect, filed as upstream drift — words its (b) clause as *"a registered extension observed `session_shutdown`"*, which is exactly right, because upstream's `session_shutdown` **is** an extension-tier event. The stdout wording is area 08's alone and is the one to change.

**Also known, and NOT a defect: `session_replaced`.** `AgentSessionEvent::SessionReplaced` (`event.rs:271-273`, tag at `:367`, emitted at `crates/cyrup-session-svc/src/subscriber.rs:90`) is wholly cyrup-invented — its doc cites internal `R-11-021` / arch-11 §3.2, not a pi file — but it is explicitly filtered out at **both** RPC write sites (`rpc.rs:908` and the EOF drain at `:931`), so it never reaches stdout. It is an internal rebind signal, correctly contained. Recorded here so the third invented event on this enum is KNOWN and nobody files it as a fourth item.

**Fix** — same decision shape as SEAM-080, and it should be taken together with it: filter both off the wire at `rpc.rs:908`/`:931` (the `SessionReplaced` guard is the pattern to extend, and doing so keeps the extension-tier emission intact for guests), or keep them as documented cyrup extensions with a `CYRUP-DELTA` at their declarations naming `types.ts:563`/`:617` and stating explicitly that upstream's are extension-runner events.

**Verify** — start and cleanly shut down an RPC session and assert the stdout line set matches pi's for the same lifecycle; assert a loaded extension still receives `session_start`/`session_shutdown` at the extension tier either way.

## SEAM-082 — `RpcClient::attach(reader, writer)` is a cyrup-original constructor path — **CLOSED 2026-08-15 (FIXED — the `attach` doc now declares itself a `[CYRUP-DELTA]`)**

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed · **filed 2026-08-14 (ext-rpc surface enumeration)**

**cyrup** — `RpcClient::attach(reader, writer)` at `crates/cyrup-modes/src/rpc_client.rs:471-492`, factoring the transport half out of the spawn path so tests can drive an in-memory duplex pair. Self-disclosed at `:464-470`, which cites `rpc-client.ts:127-129` and `:580` as the pair it decomposes. `spawn()` (`:505`) is the faithful `start()` port alongside it.

**upstream** — pi's `RpcClient` has no `attach`. Its only constructor path is `start()` (`packages/coding-agent/src/modes/rpc/rpc-client.ts:73-139 @v0.83.0`), which always spawns `node <cliPath> --mode rpc`.

**Impact** — benign: it adds no wire surface, and the client's other 32 commands, 3 helpers, 4 lifecycle methods, constants (30_000 request / 60_000 idle / 100ms start settle / 1_000ms SIGTERM grace), `req_${n}` id format, `RpcClientOptions` (6/6), `ModelInfo` (4/4) and `ForkMessage` (2/2) were all enumerated this pass and match exactly. Filed so the one API delta is KNOWN rather than assumed to be 1:1 — the enumeration found it precisely because nobody had written it down.

**Fix** — none as a repair. Confirm the existing note at `:464-470` states in terms that `attach` has **no** upstream counterpart (rather than only naming what it decomposes), so the next auditor does not re-derive it.

**Verify** — `grep -n 'attach' crates/cyrup-modes/src/rpc_client.rs` — the doc block says cyrup-original explicitly.

## SEAM-083 — The `bash` response emits `fullOutputPath` and `exitCode` as explicit `null` where pi omits the keys — **CLOSED 2026-08-15 (FIXED — plus the `bashExecution` payload, a second site this row did not name)**

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed · **filed 2026-08-14 (ext-rpc surface enumeration)**

**cyrup** — `BashResult` (`crates/cyrup-session-svc/src/bash.rs:28-53`) carries `#[serde(default)]` on `exit_code: Option<i32>` (`:37-38`) and `full_output_path: Option<String>` (`:51-52`) but **no `skip_serializing_if`**, and the struct has no container-level skip. The handler arm (`crates/cyrup-modes/src/rpc.rs:1345-1349`) is a bare `serde_json::to_value(result)`, so the struct's serde attributes **are** the wire contract. Every `{"type":"response","command":"bash",…}` therefore carries `"fullOutputPath":null`, and a killed or signalled command carries `"exitCode":null`.

**upstream** — `pi/packages/coding-agent/src/core/bash-executor.ts:29-40 @v0.83.0` — `exitCode: number | undefined;` (`:33`, a required key whose `undefined` value `JSON.stringify` **drops**) and `fullOutputPath?: string;` (`:39`, optional). `packages/coding-agent/docs/rpc.md:473-479` shows the normal response with **no `fullOutputPath` key at all**, and `:482-495` shows it appearing only when the output was truncated.

**Impact** — a client written against pi's docs uses the natural `"fullOutputPath" in data` / `data.fullOutputPath !== undefined` test to decide whether the output was truncated, and under cyrup that test is **true on every single bash response**, so it takes the truncated branch every time and goes looking for a temp file that does not exist. This is exactly SEAM-053's class, in a payload no sweep has reached: this file's `## Coverage` *Not audited* paragraph lists `BashResult` by name among the inner RPC element shapes that *"remain largely unread on both sides"*, and SEAM-053's own method — comparing `rpc-types.ts` payloads against their handler arm — is blind to `bash`, whose arm is a bare `serde_json::to_value(result)`. Confirmed NEW: `grep` for `fullOutputPath`/`exitCode` across `docs/gap-analysis/` returns only unrelated process-exit-code items.

**Fix** — add `#[serde(skip_serializing_if = "Option::is_none")]` to both fields in `crates/cyrup-session-svc/src/bash.rs`, matching what SEAM-053 did for the other envelopes. Audit the remaining `BashResult` fields (`output`, `cancelled`, `truncated`) against `bash-executor.ts:29-40` in the same edit — they are required on both sides, but the enumeration reached only the two optionals. **FIX SITE: `crates/cyrup-session-svc/src/bash.rs`** — inside area 08's crates, schedulable here.

**Verify** — assert `"fullOutputPath"` is **absent** from a normal `bash` response and **present** when the output was truncated; assert `"exitCode"` is absent for a cancelled/killed command and present otherwise. RED on the first assertion today.

## SEAM-084 — `get_commands` extension entries carry a fabricated `sourceInfo.source`, drop `baseDir`, and always emit `description` — **CLOSED 2026-08-15 (FIXED — all three parts, via a new `ExtensionProvenance` in the registry)**

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed · **filed 2026-08-14 (ext-rpc surface enumeration)**

**cyrup** — the extension branch of `get_commands` hard-codes the whole `sourceInfo` object at `crates/cyrup-session-svc/src/session.rs:2508-2511`: `"source": "extension"`, `"scope": "temporary"`, `"origin": "top-level"`, with **no `baseDir`**. The prompt-template and skill branches (`:2519`, `:2527`) do carry real provenance via `origin.source_info_json(...)`, so only the extension branch is synthesized.

**upstream** — `pi/packages/coding-agent/src/core/extensions/loader.ts:434-443 @v0.83.0`:

```ts
const source =
    extensionPath.startsWith("<") && extensionPath.endsWith(">")
        ? extensionPath.slice(1, -1).split(":")[0] || "temporary"
        : "local";
const baseDir = extensionPath.startsWith("<") ? undefined : path.dirname(resolvedPath);
…
sourceInfo: createSyntheticSourceInfo(extensionPath, { source, baseDir }),
```

with `SourceInfo = {path, source, scope, origin, baseDir?}` at `core/source-info.ts:6-12` and the `scope`/`origin` defaults at `:24-40`. `rpc-mode.ts:681-686` passes `sourceInfo: command.sourceInfo` straight through (its sibling top-level `source: "extension"` at `:684` is a **different field** and is correct in cyrup). `RegisteredCommand.description` is optional — `description?: string`, `core/extensions/types.ts:1163-1168` — so an undescribed command omits the key.

**Impact** — three divergences survive `SEAM-055`, which closed 2026-08-14 having fixed only `path` (see its body in this file). (a) **`source`** — pi emits `"local"` for a filesystem-loaded extension or the `<prefix:…>` segment for a synthetic one, and **never** the literal `"extension"`, so cyrup reports inside `sourceInfo` a value that exists nowhere upstream, and a client grouping by `sourceInfo.source` cannot separate a local extension from a synthetic one. (b) **`baseDir`** — pi emits `path.dirname(resolvedPath)` for every filesystem extension; cyrup omits the key, so a client resolving a command's assets relative to its extension directory has nothing to resolve against. (c) **`description`** — `CommandDescriptor.description` is a non-optional `String` (`crates/cyrup-ext/src/registry.rs:94-98`) and always serializes, emitting `""` where pi omits the key.

**Fix** — carry the real provenance through `ResolvedCommand` the way the prompt/skill branches already do: derive `source` as pi does (the `<prefix:…>` split, else `"local"`), populate `baseDir` from the extension's resolved directory for filesystem-loaded extensions and omit it for synthetic ones, and make the emitted `description` omit-when-empty. The owner id is already threaded (SEAM-055's fix), so this is the same one change extended. **FIX SITE: `crates/cyrup-session-svc/src/session.rs:2502-2515`**, plus an optional-description signal from `crates/cyrup-ext/src/registry.rs` (area 06's crate) if the empty-vs-absent distinction is to be exact.

**Verify** — load a filesystem extension registering a described and an undescribed command; assert `sourceInfo.source == "local"`, `sourceInfo.baseDir` equals the extension's directory, and the undescribed command's entry has **no** `description` key.

## SEAM-085 — The `message_update` projection is disclosed, but four of its citations were v0.84.1 lines presented against v0.83.0 paths

**Kind** stale-port · **Severity** low · **Effort** S · **Confidence** confirmed · **FILED AND CLOSED 2026-08-14 (ext-rpc surface enumeration)**

**cyrup** — the wire shape itself is **not** in dispute and is not a gap: `crates/cyrup-modes/src/rpc.rs:337-347` routes `RpcOut::Event` through `crate::to_json_event`, which (`crates/cyrup-modes/src/json_event.rs:91-111`) rebuilds `message_update` as a fresh two-key object — dropping the outer `message` — and strips `partial` from the inner event (`:122-186`). That is v0.84.1 behaviour deliberately forward-ported; `json_event.rs:1-7` discloses it in terms (*"the file does NOT exist at v0.83.0 … VERSION LAG, not a port bug"*) and `PARITY-GAPS.md:870` records it as already ported. Recorded here for the two-sided table: measured against the ported tag, cyrup's `message_update` is missing two fields pi's is documented to carry (`docs/rpc.md:915-955` @v0.83.0 shows the wire line WITH the outer `message` and the inner `partial` on every delta).

**The defect — mis-citation, and it is FIXED.** Four supporting citations named a v0.83.0 path with a v0.84.1 line number and no version tag:

- `rpc.rs:304-305` asserted ``Pi's `output(toJsonEvent(event))` (rpc-mode.ts:356)`` and `:328-329` repeated `(rpc-mode.ts:356)`. At `v0.83.0`, `rpc-mode.ts:355` is a bare `output(event);` and `:356` is `if (event.type === "agent_settled") {` — neither says what was claimed — and `git grep -n toJsonEvent v0.83.0 -- packages/` is **empty**: `modes/json-event.ts` does not exist at the ported tag. At `v0.84.1`, `:356` **is** `output(toJsonEvent(event));`.
- `json_event.rs:56` cited `coding-agent/docs/rpc.md:952-956` for the omission contract. At `v0.83.0` that range is the *streaming example*, which shows `"message":{...}` and `"partial":{...}` on every delta — the exact opposite of the quoted contract. At `v0.84.1` it is the contract.
- (`json_event.rs:51-52` cites the same two call sites but names `v0.84.1` in the adjacent `git grep` invocation, so it resolves correctly and was left alone.)

**Impact** — the project's rule is that an in-tree pi citation is the evidence a port matches upstream. A v0.84.1 line presented as v0.83.0 is the more dangerous form of a bad citation than a stale one, because it resolves to *real text at some tag*, so a checker that does not pin the version reads as confirming a claim it contradicts at the tag actually named.

**Fix — DONE 2026-08-14** — all three sites now carry `@v0.84.1` **and** state what the same line is at the ported baseline, so the version cannot be dropped again without the sentence going obviously wrong. Comment-only; no behaviour change.

**Verify** — `grep -n 'rpc-mode.ts:356\|rpc.md:952-956' crates/cyrup-modes/src/` — every occurrence carries an explicit version tag. `cargo check -p cyrup-modes --all-targets` green.

## SEAM-086 — A malformed `extension_ui_response` was answered with an error response where pi writes nothing

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed · **FILED AND CLOSED 2026-08-14 (ext-rpc surface enumeration)**

**cyrup (as found)** — the stdin intercept was `extension_ui_response_id` (`crates/cyrup-modes/src/rpc.rs:1505-1511`), which returned `None` unless `id` was present **and** a JSON string (`value.get("id").and_then(Value::as_str)`). On `None` the line fell through the `if let Some(id)` at `rpc.rs:798-805` into the ordinary command path, where `dispatch` (`:1032-1037`) saw an unrecognized `type` and emitted `{"type":"response","command":"extension_ui_response","success":false,"error":"Unknown command: extension_ui_response"}`.

**upstream** — `pi/packages/coding-agent/src/modes/rpc/rpc-mode.ts:763-777 @v0.83.0`:

```ts
if (… parsed.type === "extension_ui_response") {
    const response = parsed as RpcExtensionUIResponse;
    const pending = pendingExtensionRequests.get(response.id);
    if (pending) { pendingExtensionRequests.delete(response.id); pending.resolve(response); }
    return;
}
```

The intercept tests **only** `type`, never `id`, and `return`s unconditionally — so a malformed or unmatched envelope produces **no output line**.

**Impact** — an extra stdout line a client can observe, on a protocol whose contract is that a client written against pi's docs works unmodified. Low severity because it requires a malformed envelope, but the matched and unmatched-but-string-id cases already behaved correctly (`rpc.rs:799-804` silently drops when `pending.remove` misses, as pi does), so only this one case diverged — and the rest of the RPC surface enumerated clean: commands 32/32 with every camelCase param spelling matching `rpc-types.ts:20-73`, event types with nothing missing, `extension_ui_request` methods 9/9 including the `set_editor_text` snake_case outlier, `extension_ui_response` shapes 3/3, `RpcSessionState` 12/12 in pi's declaration order, the `assistantMessageEvent` tag set 12/12, and the `extension_error` envelope identical.

**Fix — DONE 2026-08-14** — `extension_ui_response_id` now returns `Option<Option<String>>`: the outer `Some` means "this is an `extension_ui_response`, intercept it" and is decided by the `type` tag alone; the inner `Option` is the correlation id. The call site always `continue`s and only looks up `pending` when the id is a string, so all three malformed/unmatched cases are swallowed exactly as pi's unconditional `return` swallows them.

**Verify** — `rpc_malformed_extension_ui_response_is_swallowed_not_answered` (`crates/cyrup-modes/src/tests/modes.rs`) writes three envelopes pi swallows — no `id`, a numeric `id`, and a string `id` matching no pending dialog — followed by a `get_state` sentinel, and asserts the first line the client reads back is the sentinel's response. RED before the fix on the first case.

## SEAM-100 — `cyrup update --models` does not exist, and the backlog already assumes it does — **CLOSED 2026-08-15 (REFUTED — already done at HEAD, whole, not just the parse half)**

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** confirmed — **verified by running the shipped binary**

**upstream** — `pi v0.83.0 packages/coding-agent/src/package-manager-cli.ts:250-257` (the `--models` arm), `:329-337` (target selection + its two conflict messages), `:397-423` (`refreshModelCatalogs`), `:726-735` (the dispatch branch in `handlePackageCommand`). Supporting text at `:86` (the usage string), `:154`, `:159`, `:169` (the help body), `:322-323` and `:331-332` (the two conflict messages).

**cyrup** — `parse_package_command` (`crates/cyrup/src/subcommands.rs:129-206`) has no `--models` arm at all, and `UpdateTargetSel` (`subcommands.rs:78-82`) has only `All` / `SelfUpdate` / `Extensions`. The token therefore falls into the catch-all at `subcommands.rs:194-196`. **Observed:** `cyrup update --models` prints `Unknown option --models for "update".`

**Impact** — **there is no CLI route to refresh model catalogs.** That is the item; everything below is downstream of it, and is folded into this row rather than filed separately so the fix lands as one piece:

- **Conflict messages.** pi's joined list is `--all cannot be combined with --self, --extensions, --models, or --extension` (`:322-323`); cyrup emits `--all cannot be combined with --self, --extensions, or --extension` (`subcommands.rs:213`). pi's second, models-specific message — `--models cannot be combined with --self, --extensions, --all, or --extension` (`:331-332`) — has no cyrup counterpart at all.
- **Usage string.** pi's (`:86`) is `… [--self|--extensions|--models|--all] …`; cyrup's (`subcommands.rs:286`) drops `--models`.
- **Help body.** pi lists `--models   Refresh model catalogs only` (`:159`) and the short form `pi update --models   Refresh model catalogs only` (`:169`); cyrup's (`subcommands.rs:311`) has neither, and its summary line reads `Update cyrup and installed packages.` against pi's `Update pi, installed packages, or model catalogs.` (`:154`).

**Compounding, and the reason this is the highest-value single item on the CLI surface:** *the existing backlog already reasons as though the command ships.* `docs/gap-analysis/05-cyrup-config-and-resources.md` argues about lock contention "against any concurrent `cyrup update --models`" in `CFG-042`'s region. A gap that other analysis has already built on top of is worse than an ordinary gap, because closing the items above it will not reveal it.

**Fix** — add the `--models` arm to `parse_package_command`, a `Models` variant to `UpdateTargetSel`, the two conflict messages, the usage/help text, and a `refresh_model_catalogs` port of `package-manager-cli.ts:397-423` wired into the dispatch. The catalog-refresh half is the real work; the parse half is small and **must not land alone** — an accepted flag over a stub is `SEAM-078` again.

**Verify** — `cyrup update --models` refreshes and exits 0; `cyrup update --models --all` prints pi's exact second conflict message and exits 1; `cyrup update --help` names `--models` in both the options list and the short forms; and a test asserting the summary line matches pi's.

## SEAM-101 — The `config` subcommand accepts unknown options and stray arguments silently — **CLOSED 2026-08-15 (REFUTED — already done at HEAD)**

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed — **verified by running the shipped binary**

**upstream** — `pi v0.83.0 packages/coding-agent/src/package-manager-cli.ts:626-636` — `Unknown option ${arg} for "config".` / `Unexpected argument ${arg}.`, both setting `process.exitCode = 1`; usage string `CONFIG_COMMAND_USAGE` at `:92`.

**cyrup** — the `config` path (`crates/cyrup/src/subcommands.rs:349-366`) scans argv for `-l`/`--local` and the trust flags and then unconditionally runs the picker; there is no rejection arm. **Observed:** `cyrup config --bogus` and `cyrup config zzz` both open the picker and exit 0.

**Impact** — a typo in a config invocation silently does something other than what was asked, and exits 0 while doing it. Every other cyrup subcommand rejects unknown options; `config` is the exception.

**Fix** — add the two rejection arms at the top of the `config` handler, using pi's exact two sentences and exit 1. **Distinct from `SEAM-079`**, which covers only the `--help` half of the same handler — but both land in the same twenty lines and should go to one agent.

**Verify** — `cyrup config --bogus` prints `Unknown option --bogus for "config".` and exits 1; `cyrup config zzz` prints `Unexpected argument zzz.` and exits 1; neither enters the picker.

## SEAM-102 — `--help`'s environment block and the read set are not the same set, in BOTH directions — **CLOSED 2026-08-15 (REFUTED — already done at HEAD, all seven names re-grepped)**

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — `pi v0.83.0 packages/coding-agent/src/cli/args.ts:343` (`ANTHROPIC_AUTH_TOKEN`), `:373` (`QWEN_TOKEN_PLAN_API_KEY`), `:374` (`QWEN_TOKEN_PLAN_CN_API_KEY`), `:375` (`XIAOMI_API_KEY`), `:376` (`XIAOMI_TOKEN_PLAN_CN_API_KEY`), `:377` (`XIAOMI_TOKEN_PLAN_AMS_API_KEY`), `:378` (`XIAOMI_TOKEN_PLAN_SGP_API_KEY`) — seven rows of a 45-row block (`:342-389`). Also `:389`, whose row reads `PI_SHARE_VIEWER_URL              - Base URL for /share command (default: https://pi.dev/session/)`, with the default itself at `packages/coding-agent/src/config.ts:502` (`DEFAULT_SHARE_VIEWER_URL = "https://pi.dev/session/"`).

**cyrup** — `render_help`'s env block (`crates/cyrup/src/cli.rs:1037-1077`) lists 38 names against pi's 45; after accounting for the four `PI_*`→`CYRUP_*` rebrands, exactly those seven are absent (`grep -c ANTHROPIC_AUTH_TOKEN crates/cyrup/src/cli.rs` → 0, likewise each). **All seven are genuinely implemented** — `crates/cyrup-provider/src/env_api_keys.rs:44` (`ANTHROPIC_AUTH_TOKEN`), `:53-54` (both `QWEN_TOKEN_PLAN` keys), `:84-86` and the adjacent Xiaomi arms. Separately, `cli.rs:1077` is `CYRUP_SHARE_VIEWER_URL           - Base URL for /share command`, dropping pi's `(default: …)` parenthetical (pi `args.ts:389`).

**Impact** — a user with a working credential is told by `--help` that cyrup does not read it. **This is the exact INVERSE of `TUI-063`**, where `CYRUP_SHARE_VIEWER_URL` is advertised and read by nothing. Both are failures of one invariant — *the help block and the read set must be the same set* — and the documentation audit that produced `TUI-063` caught only the direction that leaves a dead row. The dropped default matters for the same reason: restoring `TUI-063`'s read without restoring the documented default would leave the help text unable to say what happens when the var is unset.

**Fix** — generate the env block from the `env_api_keys` table rather than maintaining a parallel literal list, and restore the `(default: …)` parenthetical on the share-viewer row alongside `TUI-063`'s read.

**Verify** — the whole two-sided check is one `comm(1)`: `git -C pi show v0.83.0:packages/coding-agent/src/cli/args.ts | sed -n '342,390p' | grep -oE '^  [A-Za-z_{][A-Za-z0-9_{}:<]*'` against the same grep over `cli.rs`, name-mapped. Land it as a test so the invariant cannot rot again.

## SEAM-103 — `--list-models` swallows a following `@file` token as its search pattern — **CLOSED 2026-08-15 (REFUTED — already done at HEAD)**

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed — **verified by running the shipped binary**

**upstream** — `pi v0.83.0 packages/coding-agent/src/cli/args.ts:171-177` — `if (i + 1 < args.length && !args[i + 1].startsWith("-") && !args[i + 1].startsWith("@"))`.

**cyrup** — `#[arg(long = "list-models", num_args = 0..=1, default_missing_value = "")]` (`crates/cyrup/src/cli.rs:268`); clap's optional-value consumption has no `@`-guard. **Observed:** `cyrup --list-models @foo` prints `No models matching "@foo"`, where pi lists the whole configured catalog and routes `@foo` to `fileArgs`.

**Impact** — `pi --list-models @notes.md` and `cyrup --list-models @notes.md` give different output: upstream shows the catalog and keeps the file attachment, cyrup shows an empty result and loses it.

**Fix** — pre-filter in `apply_arg_leniency` (or a dedicated arm) so an `@`-prefixed token following `--list-models` is not offered to clap as the optional value, matching pi's two-part guard exactly (`-` and `@`).

**Verify** — `cyrup --list-models @foo` lists the configured catalog and leaves `@foo` in the file args; `cyrup --list-models gpt` still filters.

## SEAM-104 — A bare `-` became the prompt instead of `Unknown option: -` — **CLOSED 2026-08-14 (FIXED THIS PASS)**

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed — **verified by running the shipped binary**

**upstream** — `pi v0.83.0 packages/coding-agent/src/cli/args.ts:202-203` — `arg.startsWith("-") && !arg.startsWith("--")` matches the one-character token `-` and pushes an error diagnostic, which `main.ts:567-568` turns into exit 1. Its message arm is the *next* branch, `else if (!arg.startsWith("-"))` (`:204`), so `-` can never reach `result.messages`.

**cyrup (as filed)** — the unknown-short detector required `arg.len() > 1` (`crates/cyrup/src/diagnostics.rs:193-197`), so a bare `-` fell through to the positionals and became the PROMPT. **Observed on the shipped binary: `cyrup -` did not error — it started a real agent turn and issued a provider request.**

**Impact** — a spend-money-on-a-typo divergence: pi exits 1 without contacting anything.

**Fix — LANDED 2026-08-14.** The `arg.len() > 1` guard is deleted; the predicate is now pi's, verbatim. The `--`-prefixed case is still excluded by the existing `!arg.starts_with("--")`, so the extension-flag capture is untouched, and `KNOWN_SHORT_FLAGS` cannot contain `-`. A comment at the site records that there is deliberately NO length guard and why, so it is not "tidied" back in.

**Verify** — `bare_single_dash_is_an_unknown_option_not_a_prompt` in `crates/cyrup/src/diagnostics.rs` asserts `-` yields exactly one error diagnostic reading `Unknown option: -`, that it does NOT survive into the cleaned argv, and — presence before absence — that `--` still passes through untouched with no diagnostic. RED before the change. `cargo check -p cyrup --all-targets` clean.

## SEAM-105 — Repeated `--models` / `--tools` / `--exclude-tools` append; pi replaces — **CLOSED 2026-08-15 (REFUTED — already done at HEAD)**

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — `pi v0.83.0 packages/coding-agent/src/cli/args.ts:114` (`result.models = …`), `:121-124` (`result.tools = …`), `:125-129` (`result.excludeTools = …`) — all three ASSIGN, so a repeated flag replaces the earlier value.

**cyrup** — all three are declared as `Vec<String>` with `value_delimiter = ','` (`crates/cyrup/src/cli.rs:176`, `:195`, `:198`), so clap APPENDS across repeats.

**Impact** — `--tools read --tools bash` yields `{read}` under pi and `{read,bash}` under cyrup; `--models a --models b` yields `[b]` vs `[a,b]`. Only the repeated-flag form diverges — the single comma-separated form is identical, which is why every existing test passes.

**Fix** — keep the `Vec` for the comma form but take only the LAST occurrence group, e.g. by reading clap's occurrence structure (`ArgMatches::get_occurrences`) or by post-processing argv before clap sees it, alongside the other leniency rules in `diagnostics.rs`.

**Verify** — three tests, one per flag, asserting `--tools read --tools bash` resolves to exactly `{bash}` while `--tools read,bash` still resolves to both.

## SEAM-106 — `--export` runs after four guards pi runs it before — **CLOSED 2026-08-15 (REFUTED — already done at HEAD)**

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — `pi v0.83.0 packages/coding-agent/src/main.ts:578-590` — the export branch sits immediately after the `--version` exit and BEFORE `resolveAppMode` (`:592`), the RPC `@file` guard (`:598-601`), `validateForkFlags` / `validateSessionIdFlags` (`:603-604`) and the `--api-key requires a model` check (`:757-761`).

**cyrup** — export runs at `crates/cyrup/src/main.rs:350-352`, downstream of all four: session-flag validation at `:221`, the RPC `@file` guard at `:247`, and the `--api-key requires a model` bail at `:344-346`.

**Impact** — `cyrup --export s.jsonl --api-key K` and `cyrup --export s.jsonl --fork X --continue` both error where pi performs the export and exits 0. Export is the operation a user reaches for when the session is already in a bad state, so the guards fire on exactly the invocations that need it most. Separately, `--export` takes its optional output path from `cli.positionals.first()` (`:351`), which — unlike pi's `parsed.messages[0]` — still contains `@file` tokens, so `cyrup --export @notes.md` writes to a file named `@notes.md`.

**Fix** — move the export branch to immediately after the `--version` exit, matching pi's dispatch order, and take the path from the message list after `@file` partitioning rather than from raw positionals.

**Verify** — `--export out.jsonl --api-key K` exits 0 with the file written; `--export out.jsonl --fork X --continue` likewise; `--export @notes.md` does not create `@notes.md`.

## SEAM-107 — `-p`'s `---` escape hatch is unported — **CLOSED 2026-08-15 (REFUTED — already done at HEAD)**

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — `pi v0.83.0 packages/coding-agent/src/cli/args.ts:140-146` — after `--print`/`-p`, `if (next !== undefined && !next.startsWith("@") && (!next.startsWith("-") || next.startsWith("---")))` the token is pushed as a MESSAGE and consumed. The `next.startsWith("---")` clause is the escape hatch that lets a prompt legitimately begin with dashes.

**cyrup** — `--print` is a plain clap bool (`crates/cyrup/src/cli.rs:133-134`), so `---weird` after `-p` reaches `partition_extension_flags` (`cli.rs:762-802`) and is captured as an extension flag named `-weird` instead of becoming the prompt.

**Impact** — the narrowest item on this surface: it only fires for a prompt whose first token begins with three or more dashes. Recorded for completeness of the enumeration rather than as a scheduling candidate — but it is a real path by which a prompt is silently reinterpreted as a flag.

**Fix** — port the three-part condition into the `-p`/`--print` handling in `diagnostics.rs` so a `---`-prefixed following token is consumed as the message.

**Verify** — `cyrup -p ---weird` treats `---weird` as the prompt and registers no extension flag.

## SEAM-108 — The `auth` surface is v0.84.1-shaped against a v0.83.0 port — **CLOSED 2026-08-15 (FIXED — `[CYRUP-DELTA]` landed at the validation site; no revert)**

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — `pi v0.83.0 packages/coding-agent/src/cli/credential-print.ts:24-30` (`printCredentialPrintHelp` — TWO verbs, usage spelled `pi auth print-api-key --model <model> [--provider <provider>]`, i.e. model REQUIRED and provider optional), `:38-40` (the unknown-verb sentence, naming two verbs), `:66-76` (`validateCredentialPrintArgs`, which rejects a missing `--model` with `Credential printing requires --model <model>` and whose unknown-flag error is `Credential printing only accepts --provider and --model`).

**cyrup** — `crates/cyrup/src/credential_print.rs` shows three verbs (`:46-47`, `:176-183`), spells the usage `cyrup auth print-api-key [--provider <provider>] [--model <model>]` (`:79`) and requires provider OR model (`:314-321`); the unknown-verb sentence names three verbs (`:226-230`) and the unknown-flag error is `Unknown option --X for "auth print-api-key".`

**Impact** — `cyrup auth print-api-key --provider openai` SUCCEEDS where `pi v0.83.0 auth print-api-key --provider openai` errors. `SEAM-050` (closed) filed the v0.84.1 `auth` surface as unported and closed by landing it; this row records what landing it diverged FROM, so the divergence is known rather than implied by a closed row. The forward-port itself is deliberate and documented in-file — **this is not a request to revert it.**

**Fix** — none unless the owner wants strict v0.83.0 behaviour. What is owed is a `[CYRUP-DELTA]` at the argument-validation site stating that the required-argument rule, the verb count and the two error sentences are v0.84.1's and not the ported tag's, so a later fidelity pass against `credential-print.ts` does not read them as defects.

**Verify** — assert the delta line exists and names `@v0.84.1`; assert the three-verb help and the provider-OR-model rule are covered by tests that cite it.

## SEAM-109 — Two hidden argv verbs with no upstream counterpart — **CLOSED 2026-08-15 (FIXED — a `[CYRUP-DELTA]` at each verb, upstream re-derived)**

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `__subagent-runner --config <path>` (`crates/cyrup/src/main.rs:124-129`; `crates/cyrup/src/subagent_runner_cmd.rs:50-52`, `is_selected` matching `argv[1]`) and `__intercom-broker` (`crates/cyrup/src/main.rs:136-141`; `crates/cyrup/src/intercom_broker_cmd.rs:22-24`).

**upstream** — pi has **no argv verbs at all**: `git -C pi grep -nE 'argv\[2\]|process\.argv' v0.83.0` finds none. `pi-subagents` spawns a SEPARATE script (`src/runs/background/async-execution.ts:492`, `:516` — node + jiti on `subagent-runner.ts` with a config path), never a hidden verb on the pi binary. `packages/coding-agent/src/rpc-entry.ts` and `bun/cli.ts` are separate binary ENTRY POINTS, not argv verbs — `rpc-entry.ts:12` just prepends `--mode rpc` and `bun/cli.ts:1-15` sets `process.title` and re-imports `../cli.ts`, contributing no flags.

**Impact** — the single-binary re-exec is a defensible Rust analog of a mechanism that cannot be ported literally (there is no `node` to hand a script to). But it is an invented, user-reachable command surface: it is deliberately absent from `--help` (`main.rs:117-118`) and from `SUBCOMMANDS` (`subcommands.rs:31`), which makes it undiscoverable rather than absent. Not a defect — filed so it is enumerated rather than assumed to be parity, per this sweep's rule that an invented surface must be KNOWN.

**Fix** — none proposed. What is owed is a `[CYRUP-DELTA]` at each verb naming `async-execution.ts:492`/`:516` as the mechanism it replaces and stating that the verb is intentionally undocumented.

**Verify** — assert both handlers reject when `argv[1]` does not match exactly, and that neither name appears in `--help` or `SUBCOMMANDS`.

## SEAM-110 — `update <source>` accepts a third self alias that pi does not, and advertises the wrong one — **CLOSED 2026-08-15 (REFUTED — already done at HEAD)**

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed

**cyrup** — `crates/cyrup/src/subcommands.rs:235` — `let source_is_self = src == "self" || src == "pi" || src == "cyrup";`

**upstream** — `pi v0.83.0 packages/coding-agent/src/package-manager-cli.ts:348` — `source === "self" || source === "pi"`. Exactly two.

**Impact** — the rebranded third alias is the obviously right call for a fork named cyrup, and the superset is harmless. What is not harmless is the help text: `subcommands.rs:311` still reads `cyrup update pi   Update cyrup only (self works as alias to pi)` — it advertises the `pi` alias and never mentions the `cyrup` one, so the alias a user would actually guess is the one undocumented.

**Fix** — reword the short-forms line to name `cyrup` (keeping `self`/`pi` as accepted legacy spellings), and record the superset in a `[CYRUP-DELTA]` citing `:348`.

**Verify** — `cyrup update cyrup` behaves as `--self`; `cyrup update --help` names the `cyrup` alias.

## SEAM-111 — The top-level help's Commands block understates the shipped surface — **CLOSED 2026-08-15 (REFUTED — already done at HEAD)**

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** confirmed

**upstream** — `pi v0.83.0 packages/coding-agent/src/cli/args.ts:232` and `:234`: `pi update [source|self|pi]   Update pi, extensions, or model catalogs` and `pi config [-l]   Open TUI to enable/disable package resources (Tab switches scope)`.

**cyrup** — `crates/cyrup/src/cli.rs:923` and `:925`: `cyrup update [source|self|pi]   Update cyrup (use --all for cyrup and extensions)` and `cyrup config   Open TUI to enable/disable package resources`.

**Impact** — three omissions, and **two of them understate what actually ships**: `-l` IS supported (`subcommands.rs:360`) and Tab DOES switch scope, yet neither is advertised at the top level — so the two least guessable parts of `config` are invisible from both the top-level help and (per `SEAM-079`) `config --help`. The third omission, the model-catalog clause, is honest today only because `SEAM-100` is open.

**Fix** — restore `[-l]` and the Tab hint on the `config` line; restore the model-catalog clause on the `update` line **together with `SEAM-100`**, not before it.

**Verify** — assert `cyrup --help` contains `[-l]` and `Tab switches scope`; assert the `update` line names model catalogs only once the command exists.

## SEAM-114 — `context_usage` rebuilt the whole branch message list, and the occupancy it produced had three ways of being wrong — **FILED RETROACTIVELY AND CLOSED 2026-08-19 (landed at `2086366`)**

**Kind** port-bug · **Severity** medium · **Effort** M · **Confidence** confirmed

> **FILED RETROACTIVELY 2026-08-19.** The work landed on 2026-08-18 at `2086366`
> ("one occupancy producer — `context_usage` walks the active branch, TUI-092 F4") and had no row in
> any area file, although it is entirely inside this area's crate and carries three correctness
> fixes plus a `cyrup-original` disclosure. Its per-defect task file
> (`bugs/TUI-092-F4-context-usage-reverse-scan.md`) was deleted when the round-2 batch landed, so the
> only surviving record was a commit subject. The TUI-092 umbrella tracks F4 as a *performance*
> defect; the three correctness fixes below are not performance and belong here.

**upstream** — `pi/packages/coding-agent/src/core/agent-session.ts:3164-3208` @v0.83.0 `getContextUsage`: the model read and the `contextWindow <= 0` bail come FIRST (`:3165-3169`), the occupancy is answered from `this.sessionManager.getBranch()` (`:3174`) — the ACTIVE BRANCH, never `getEntries()` — and the post-compaction guard is a backward scan from the branch tail to the compaction index (`:3181-3193`). `getSessionStats` does not re-derive occupancy; it returns `contextUsage: this.getContextUsage()` (`:3160`). pi's RPC session state (`modes/rpc/rpc-types.ts:95-108`, built at `modes/rpc/rpc-mode.ts:446-461`) is twelve scalars carrying **neither** `stats` nor `contextUsage`, and pi's `state` getter (`agent-session.ts:863-865`) returns `AgentState`, not a session snapshot.

**cyrup** — one performance defect and three correctness defects, all in `crates/cyrup-session-svc/src/session.rs`, all fixed at HEAD. **(1) The walk.** `context_usage` (`:4146`) called `Self::messages` → `build_context()` → `build_context_messages()`, which deep-clones every message on the branch, tool payloads included, purely so the function could reverse the vector, take the first assistant and drop the rest — O(session history) of allocation on **every** `MessageEnd`, awaited on the TUI run-loop task (`cyrup-tui/src/app/events.rs:102` reaches it through `stats_context_usage`, `session.rs:4057`). It now walks `guard.branch_path(None)` in reverse and clones nothing (`session.rs:4173-4184`). **(2) Off-branch compaction boundary.** `has_post_compaction_usage` (`:4093`) scanned `entries()`, the flat append-only store, not the branch: after a `/fork` or a `/tree` navigation that store also holds the abandoned branch, so `rposition` could latch an OFF-BRANCH compaction as the boundary and then count an off-branch assistant as post-compaction usage — printing a stale pre-compaction occupancy as current, which is the exact failure the guard exists to prevent. Now `branch_path(None)` (`:4107`), and O(branch-depth) besides. **(3) Two producers for one number.** `state_view` (`:4194`) re-derived occupancy inline with a copy of the pre-rewrite algorithm, so `GetState.context_usage` and `GetContextUsage` — both shipping through one seam (`command.rs:190`) — could report different numbers for one session state: divergent whenever a compaction's kept window held no assistant while an earlier pre-compaction assistant existed, which includes **every** unresolvable-v1 `first_kept_entry_id` session, whose kept window is empty by construction (`cyrup-session/src/context.rs:166-172`). It now delegates (`:4203`), as upstream does. **(4) `StopReason::Deferred`.** The old path could not return a deferred assistant (`push_as_message` drops it, `cyrup-session/src/context.rs:62`), and a deferred turn is a durable provider handle with empty content, not a settled measurement, so its `usage` must not drive the footer. It is filtered with `filter_map(..).find(..)` — **not** `find_map` — so a deferred tail does not stop the scan (`:4184`). cyrup cannot produce one yet; a pi-written session carrying one must still read identically.

**Impact** — (1) is the TUI-092 F4 half and is measured there. (2) and (3) are silently wrong NUMBERS on a surface the user reads every turn — the footer occupancy — and (3) makes two RPC calls on one session state disagree, which is the class an RPC client cannot work around. (2) fires on the two commonest navigation commands, `/fork` and `/tree`.

**Fix** — landed at `2086366`; nothing outstanding in the code. **Three citation corrections ride along and are the durable part**: `has_post_compaction_usage` cited `agent-session.ts:3178-3195`, which straddles the backward scan's boundary — the scan is `:3181-3193`; `state_view` claimed to port pi's state getter at `:753`, which is inside `_handleAgentEvent`'s `message_start` arm, and its real analog is the RPC `get_state` handler; and the `stats` / `context_usage` fields on `SessionStateView` are **cyrup-original**, not parity, now stated as such at `session.rs:4190-4193`. **One residual citation to correct in-source, found this pass:** that same doc comment cites `agent-session.ts:3170` for "`getSessionStats` returns `contextUsage: this.getContextUsage()`" — at v0.83.0 that statement is `:3160`; `:3170` is a comment line inside `getContextUsage`.

**Verify** — ~~**NOT DONE — see `SEAM-115`.**~~ **DONE 2026-09-04 via `SEAM-115` (`c6142d01`).** `git show --stat 2086366` touches exactly two files, `crates/cyrup-session-svc/src/session.rs` and the deleted task file: **no test was added for any of the three correctness fixes**, and `grep -rn 'context_usage' crates/cyrup-session-svc/src/tests/` returns nothing.

## ~~SEAM-115~~ — ~~medium~~ **CLOSED 2026-09-04 (`c6142d01`)** — The three `context_usage` correctness fixes shipped with no test at all

**Kind** test-defect · **Severity** ~~medium~~ closed · **Effort** S · **Confidence** confirmed · **filed 2026-08-19, CLOSED 2026-09-04**

> **CLOSED 2026-09-04 at `c6142d01`** (`test(session-svc): SEAM-115 pin the three context_usage
> correctness fixes over a branched session`). One new file,
> `crates/cyrup-session-svc/src/tests/context_usage_branch.rs` (registered in `tests/mod.rs`), three
> cases — the (a)/(b)/(c) this section's Fix paragraph specified — each asserting its own
> precondition so a fixture drift cannot turn it into a vacuous pass:
>
> **(a) Branch isolation** — `post_compaction_guard_ignores_an_off_branch_compaction_and_its_assistant`
> (`:103`). Builds `u1→a1→u2→a2→C1`, `branch(u1)`, then `u3→a3→C2→u4→a4` on the side, then
> `branch(C1)`; asserts `stats_context_usage().tokens == None` (`:173`) and that `context_usage()`
> is a2's occupancy, not a4's (`:186`). RED against `has_post_compaction_usage` scanning
> `entries()` (`session/stats.rs:148` → `guard.entries()`): the flat store ends `…C2,u4,a4`,
> `rposition` latches C2, and the guard reported `Some(1738)` for a branch with no post-compaction
> assistant — the SEAM-114 defect-2 symptom verbatim.
>
> **(b) One producer** — `state_view_and_context_usage_are_one_producer_after_an_empty_kept_window`
> (`:207`). Persists a two-turn compaction, strips `firstKeptEntryId` from the compaction line of
> the JSONL — the pi `migrateV1ToV2` output for an unresolvable v1 `firstKeptEntryIndex`
> (`cyrup-session/src/entry.rs:82-96`; `build_context_messages` then keeps nothing before the
> compaction, `cyrup-session/src/context.rs:187-192`) — resumes via `SessionTarget::Resume`, proves
> the rebuilt context holds no assistant (`:263`), and asserts `state_view().context_usage ==
> context_usage()` with `used_tokens > 0` (`:276`). RED against an inline `messages()`-based
> derivation in `state_view` (`session/stats.rs:249`): the inline copy reported 0, the branch walk
> the pre-compaction a2. **Fixture note for the next reader:** a live `keepRecentTokens: 0`
> compaction does NOT produce the divergent shape — cyrup's cut point keeps the split tail of the
> last turn, assistant included (the summary reads `Turn Context (split turn)`), so the file edit is
> what makes the kept window empty, exactly the "unresolvable-v1" session SEAM-114 defect 3 named.
>
> **(c) Deferred tail** — `a_deferred_tail_does_not_stop_the_scan_nor_drive_the_occupancy` (`:302`).
> A settled turn, then a faux `StopReason::Deferred` receipt (`faux_assistant_message_with(…,
> FauxMessageOptions { deferred: Some(handle) })`, which the agent loop persists as an ordinary
> assistant entry with its own usage estimate); asserts `context_usage()` equals the settled turn's
> occupancy (`:360`) and `stats_context_usage().tokens == Some(that)`. RED against BOTH
> `find_map(..).filter(..)` (stops at the receipt → 0) and an unfiltered `.next()` (reports the
> receipt's own estimate), pinning the `filter_map(..).find(..)` choice at `session/stats.rs:229`.
>
> **RED/GREEN method.** All three pass at HEAD (`cargo nextest run -p cyrup-session-svc`: 326
> passed). RED was established by re-introducing each defect in `session/stats.rs` — four
> mutations, one per regression above — running the matching case (each failed at the assertion
> named), and restoring the file byte-for-byte. `2086366~1` was not checked out: branch switching
> is off-limits in this shared tree, and the cases use `branch()` / `stats_context_usage()` /
> `session_dag()`, which the pre-fix tree need not expose.
>
> **Upstream, re-read at the parity target (ADR-0006).** pi **v0.84.4**
> `packages/coding-agent/src/core/agent-session.ts:3375-3413` `getContextUsage` — `getBranch()` at
> `:3384`, the backward scan to the latest on-branch compaction at `:3390-3403`, `{tokens: null,
> contextWindow, percent: null}` at `:3406-3408` — **byte-identical** to v0.83.0 `:3164-3208`;
> `getSessionStats` returns `contextUsage: this.getContextUsage()` at `:3371` (v0.83.0 `:3160`).
> No tag-to-tag behaviour to port. cyrup at HEAD: `crates/cyrup-session-svc/src/session/stats.rs`
> `has_post_compaction_usage` (`:134`, `branch_path(None)` at `:148`), `context_usage` (`:187`,
> `branch_path(None)` at `:219`, the deferred filter at `:229`), `state_view` (`:239`, delegating
> at `:249`).
>
> **Also landed in `c6142d01`:** the one stale in-source citation SEAM-114 left open — `state_view`
> cited `agent-session.ts:3170` for the `getSessionStats` statement (a comment line inside
> `getContextUsage` at v0.83.0); now `:3160 @v0.83.0` / `:3371 @v0.84.4`. The `context_usage` and
> `has_post_compaction_usage` doc comments carry the v0.84.4 line refs beside their v0.83.0 ones.
>
> **Residuals, stated, not this row's:** (1) **low, parity** — pi's occupancy is
> `estimateContextTokens(this.messages)` (`:3410`; `core/compaction/compaction.ts:202-230`
> @v0.84.4): `calculateContextTokens(lastUsage)` — which prefers `usage.totalTokens` when non-zero
> (`:146-148`) — PLUS an `estimateTokens` sum over the messages AFTER that assistant
> (`trailingTokens`, `:219-222`), falling back to a whole-context estimate when no assistant usage
> exists. cyrup's `ContextUsage::from_last_assistant` (`crates/cyrup-session-svc/src/state.rs:302`)
> is the four-field sum alone, with no trailing term and no `totalTokens` preference; unfiled in
> this area (area 03's `SESS-028` covers the compaction-threshold estimate, not the footer). (2)
> **low, doc** — `session/stats.rs:212-214`'s "cyrup cannot produce one yet" about a deferred
> assistant is true of real providers only; the faux provider streams one and the agent loop
> persists it, which is what (c) relies on.
>
> **Falsification** — any of the three cases going green under the mutation it names, or a fourth
> way for `context_usage` to be wrong over a branched session that none of them catches.

**cyrup** — `SEAM-114`'s three correctness fixes are unpinned. `git show --stat 2086366` changed `crates/cyrup-session-svc/src/session.rs` and one markdown file, and nothing else in the tree covers them: `grep -rln 'context_usage\|ContextUsage' crates/*/src/tests/ crates/*/tests/` finds `cyrup-ext` (the `ctx.context_usage()` JSON shape), `cyrup-tui/src/tests/footer_chrome_fidelity.rs` (`from_last_assistant` clamping only), `cyrup-it/tests/intercom/presence_context_usage.rs` (the intercom projection) and `cyrup-session-svc/src/tests/session_stats_shape.rs` (whose two cases are `a_compaction_does_not_erase_the_tokens_it_already_billed` and `stats_carry_pi_s_identity_fields_and_derived_total`) — **not one of them drives `AgentSession::context_usage` over a branched session.** Every one of the three bugs is a wrong number produced by a correct-looking call, which is the class that regresses invisibly.

**upstream** — n/a; this is coverage over a cyrup fix, measured against pi's semantics at `agent-session.ts:3164-3208` @v0.83.0.

**Impact** — the off-branch boundary bug (`SEAM-114` defect 2) and the two-producer divergence (defect 3) both re-introduce themselves the moment someone "simplifies" `has_post_compaction_usage` back to `entries()` or re-inlines the `state_view` derivation, and both fail silently: the footer shows *a* number, just the wrong one. `SEAM-114`'s own commit message names 305 passing tests in this crate, none of which would have gone red.

**Fix** — three cases in `crates/cyrup-session-svc/src/tests/`, beside `session_stats_shape.rs`. (a) **Branch isolation:** build a session with a compaction, fork off it, and assert `context_usage()` on the fork ignores the abandoned branch's compaction and its post-compaction assistant — RED against an `entries()` scan. (b) **One producer:** on a session whose compaction kept window holds no assistant while an earlier pre-compaction assistant exists, assert `state_view().await.context_usage == context_usage().await` — RED against any inline re-derivation. (c) **Deferred:** a branch whose tail assistant is `StopReason::Deferred` over an earlier settled assistant must report the SETTLED one's occupancy, which pins both the filter and the `filter_map(..).find(..)` choice over `find_map`.

**Verify** — all three fail at `2086366~1` and pass at HEAD.

## ~~SEAM-113~~ — ~~high~~ **CLOSED 2026-09-04 — REFUTED as an open bug (stale under ADR-0006)** — `/model` never writes the settings default, so the choice dies with the session — ~~**CONTRACT SETTLED 2026-08-19: it is (a), a real port bug, in `cyrup-session-svc`**~~

**Kind** port-bug · **Severity** ~~high~~ closed · **Effort** M · **Confidence** confirmed (the report) · **filed 2026-08-15 (live use), contract settled 2026-08-19, CLOSED 2026-09-04**

> **CLOSED 2026-09-04 — the contract this section settled on was settled against the wrong tag.**
> ADR-0006 makes each upstream's LATEST tag the parity target; at pi **v0.84.4** the contract is
> (b), and this section's own "drift window" paragraph below already describes (b) accurately —
> `2ff8ba622` gating the write behind `options.persist`, `9c8070fbe` binding Ctrl+S. What it did not
> know is that the `--default` flag it warned about was **deleted one day later** by `5133c9284`
> (*chore(settings-selector): get rid of --default and global model*, 2026-08-20; first tag
> v0.84.3), so v0.84.4 carries the Ctrl+S opt-in and nothing else, and that is what cyrup ships:
> `82f40d3` (2026-08-28) landed exactly (b). Re-read this pass on both sides — pi
> `core/agent-session.ts:1657-1677`, `modes/interactive/interactive-mode.ts:4788,4832,4974-4981,4804-4812,4996,5002`,
> `modes/rpc/rpc-mode.ts:472-478`, `core/slash-commands.ts:21,23`; cyrup
> `crates/cyrup-tui/src/app/execute_misc.rs:416-458,465-498,504-557`,
> `crates/cyrup-session-svc/src/session/model.rs`, `session/thinking.rs:48`,
> `crates/cyrup-tui/src/commands.rs:147,165` — and every path matches: only Ctrl+S persists, Enter /
> typed `/model` / typed `/thinking` / RPC `set_model` never do.
>
> **Two claims in this section are corrected, not merely superseded.** (1) *"rank 4 … its INPUT is
> permanently empty"* / *"cyrup-config has no setter to call"* — false at HEAD `a4805955`: the
> writer is `persist_setting(scope, "defaultProvider"/"defaultModel", …)` (`execute_misc.rs:444-448`)
> and the reader is `resolve_default_launch_model` (`crates/cyrup/src/bootstrap.rs:247-275`, keys
> read at `:269-270` into `default_launch_model`, `crates/cyrup/src/provider.rs:413`, then
> `find_initial_model`, `crates/cyrup-config/src/model/select.rs:237`). Proven by a live headless
> run (`REPRO-LOG.md` §0e): RPC `set_model` wrote nothing and a fresh relaunch fell to
> `amazon-bedrock` — the reported symptom — and then seeding the two keys into
> `<CYRUP_HOME>/.cyrup/agent/settings.json` made a fresh relaunch resolve that pair. (2) The
> *"whoever takes the drift window must move both halves together or cyrup ends up with (b)'s gate
> and no flag to open it"* warning — moot: upstream removed the flag, and cyrup's opener is the
> same Ctrl+S upstream kept (`grep -rn -- '--default' crates/cyrup-tui/src` = 0, correctly).
>
> **The matched sibling `set_thinking_level`** is dispositioned here, not filed: typed
> `/thinking <level>` is session-scoped on both sides (`selectThinkingLevel(level, false)`,
> `:4788` ↔ `execute_misc.rs:504-557`, which also reproduces pi's `Unknown thinking level` error
> text), and `ConfirmSelectionAsDefault { kind: Thinking }` (`:465-498`, `defaultThinkingLevel` at
> `:487`) is the Ctrl+S persist ↔ `selectLevel(level, true)` (`:4804-4812`). **The "SECOND
> RESIDUAL"** (`builder.rs`'s step-3 reading only `default_model()`) was filed on the premise that
> the interactive binary hides it by pre-resolving through `default_launch_model` — still true, and
> still latent on the embedder path only; it is not reopened here and is not this row's symptom.
>
> **Falsification** — reopen only against v0.84.4 (or a later tag), never against v0.83.0: a
> plain-Enter or typed `/model` selection that writes `defaultModel`, or a seeded
> `defaultProvider`/`defaultModel` pair that a fresh launch does not honour. Nothing below was
> rewritten; read it as the v0.83.0 analysis it is.

> **The row filed two candidate contracts and said to establish which one holds before touching
> anything, because the fixes live in different crates. It is (a).** pi's `/model` persists, by
> design, at the tag cyrup ports; cyrup performs the transcript half of that write and not the
> settings half. Nothing in `cyrup-config`'s resolution is wrong, so the `defaultThinkingLevel`
> shape the row suspected (`CFG-056`, a getter folding "unset" into a type's zero) is **not** what
> this is — cyrup's getter is correct and its reader is correct; there is simply never anything to
> read.

**upstream** — at **v0.83.0** the selection is written from **four unconditional call sites**, and the write is always the same one function. (1) `setModel` (`pi/packages/coding-agent/src/core/agent-session.ts:1578-1593`): `:1586` `this.sessionManager.appendModelChange(model.provider, model.id)` **and `:1587` `this.settingsManager.setDefaultModelAndProvider(model.provider, model.id)`**, back to back. (2) `_cycleScopedModel`: `:1629` + **`:1630`**. (3) `_cycleAvailableModel`: `:1657` + **`:1658`**. (4) the selector component's `handleSelect` (`modes/interactive/components/model-selector.ts:354-359`), whose literal `// Save as new default` comment sits above **`:357`**, which runs BEFORE `:358 this.onSelectCallback(model)` — so the component persists even if the session layer somehow did not. `setDefaultModelAndProvider` (`core/settings-manager.ts:695-701`) assigns `this.globalSettings.defaultProvider` and `.defaultModel`, `markModified`s both, and `save()`s → `<agentDir>/settings.json`, **GLOBAL scope only**. The same shape governs thinking (`agent-session.ts:1687-1698`: `:1688` `appendThinkingLevelChange` + `:1689-1692` `setDefaultThinkingLevel`, gated on `this.supportsThinking() || effectiveLevel !== "off"`).

**upstream has since flipped to (b), and that is a DRIFT WINDOW, not this row's contract.** Two commits on the pi working tree (`v0.84.2-72-gb7bb00b93`), both dated **2026-08-19 — twenty days after `v0.83.0` (2026-07-30), five after `v0.84.2` (2026-08-14), and while this row was open**: `2ff8ba622` *"fix(coding-agent): keep model and thinking level changes session scoped (#8356)"* re-signs `setModel(model, options: ModelMutationOptions = {})` (`agent-session.ts:1598-1617`) and moves the settings write inside `if (options.persist)` (`:1607-1609`), adding `parseDefaultFlagArgs` (`modes/interactive/interactive-mode.ts:242`) and the `--default` flag; `9c8070fbe` *"feat(settings-selector): ctrl + s persists /model"* binds `ctrl+s` in the selector (`model-selector.ts:365`) and re-words the hint row to `"Enter to select · Ctrl+S to set as default · Esc to cancel"` (`:132`). **cyrup is CORRECT to have no `--default`**: v0.83.0's `argumentHint` is `"<provider/model>"` (`core/slash-commands.ts:21`), which `crates/cyrup-tui/src/commands.rs:70` matches exactly, where HEAD's is `"[--default] <provider/model>"` plus a whole new `thinking` entry. It is **not** correct to have no persist. Whoever eventually takes the drift window must move both halves together or cyrup ends up with (b)'s gate and no flag to open it.

**cyrup** — the write is missing at **`AgentSession::apply_model_change`, `crates/cyrup-session-svc/src/session.rs:4532-4577`**: the shared body behind `set_model` (`:2983`), `set_model_resolved` (`:2999`), `cycle_scoped_model` and `cycle_available_model` (`:4502`). It ports pi's transcript write (`:4563` `append_model_change`) and stops there. **Its own doc comment at `:4529` is why nobody saw it** — *"push to the agent, re-derive headers, **persist**, re-clamp/restore the thinking level, emit `model_changed` + `model_select`"* — where "persist" names the transcript entry and reads as if it covered the settings write too. **There is no setter to call, either:** `EffectiveSettings::default_provider()` (`crates/cyrup-config/src/settings.rs:553`) and `default_model()` (`:557`) are the **only** production references to `defaultProvider`/`defaultModel` in the whole workspace — `grep` finds nothing else outside tests — and no `set_default_model_and_provider` exists. **The half-port is visible in the source at the fourth upstream site:** `crates/cyrup-tui/src/model_selector.rs:500-505` cites `handleSelect → setDefaultModelAndProvider(model.provider, model.id)` by name and keeps one of that call's two effects — the fully-qualified `provider/id` at `:504` — dropping the write. (That comment's `model-selector.ts:330` is stale besides; the call is `:357` at v0.83.0.) Downstream of it, `crates/cyrup-tui/src/app/execute.rs:281-286` just calls `session.set_model(&value)`, and `handle_model_command` (`crates/cyrup-tui/src/app/selectors.rs:122-142`) does the same on the exact-match path at `:134`.

**Both precedence chains were walked rank for rank, and rank 4 is where it lands.** They are structurally identical; the only difference is that cyrup's step-3 input is permanently empty.

| rank | pi v0.83.0 | cyrup | same? |
|---|---|---|---|
| 1 | `--provider` + `--model` → `resolveCliModel` (`main.ts:420-445`; `model-resolver.ts:596-610`) | `cli.model`/`cli.provider` → `config.model_pattern` → `builder.rs:1850-1870` | yes |
| 2 | `--models` scope: saved default if in scope, else first scoped (`main.ts:447-467`) | `main.rs:1185-1204` `pick_scoped_active_model`, fresh sessions only | yes |
| 3 | resumed session's `model_change` (`core/sdk.ts:196-203`) | `builder.rs:1872-1892` | yes |
| **4** | **`settings.defaultProvider` + `.defaultModel`, exact `getModel()` on the FULL registry, gated on `hasConfiguredAuth` (`model-resolver.ts:621-631`)** | **`find_initial_model` step 3, `crates/cyrup-config/src/model.rs:1477-1489` — the same logic, the same gate** | **code yes / INPUT NEVER WRITTEN** |
| 5 | curated `defaultModelPerProvider` scan in `Object.keys` order (`model-resolver.ts:633-644`) | `first_default_or_first` over `KNOWN_PROVIDERS` (`model.rs:1161-1172`, table at `:1059-1108`) | yes |
| 6 | `availableModels[0]` (`model-resolver.ts:646-647`) | `available.first().cloned()` (`model.rs:1171`) | yes |
| 7 | `model: undefined` + `formatNoModelsAvailableMessage` (`core/sdk.ts:216-218`) | `builder.rs:1912-1915` | yes |

The settings-layer precedence underneath is identical too — runtime overrides (`apply_overrides`, `settings.rs:1410-1414` ≙ `settings-manager.ts:508-510`), then project over global (`recompute` → `deep_merge`, `settings.rs:1357-1368` ≙ `settings-manager.ts:305`), then global, **which is the layer `setDefaultModelAndProvider` writes and the layer nothing in cyrup writes**.

**Impact** — the selection lands in the session transcript only, so it survives `--resume`/`--continue` (rank 3) and dies on every genuinely new session, which then resolves at rank 5 or 6 — a config-independent constant. That is the reported symptom verbatim. It also makes `/model` the only settings-writing surface in the TUI that silently is not one: `/settings` rows DO persist, through `persist_setting` (`crates/cyrup-tui/src/app/execute_misc.rs:232`).

**The `models.json` / `TUI-089` sub-hypothesis the row asked about is MOOT** — nothing writes `defaultModel` on ANY path, so a user-declared model and a built-in are treated identically at the broken step. The reporter's exact landing could not be pinned without their `auth.json`/`models.json`, and one detail is worth recording so nobody re-derives it: **cyrup has no `baseten` provider at all** (no `catalog/baseten.json`, no `baseten.rs`) even though `default_model_per_provider("baseten") = "zai-org/GLM-5.2"` (`crates/cyrup-config/src/model.rs:1088`), so rank 5 cannot reach `GLM-5.2` through that entry. The id exists in `together` (`crates/cyrup-provider/src/providers/together.rs:363`, last of 21) and `huggingface` (`crates/cyrup-provider/src/providers/catalog/huggingface.json:1074`), and `moonshotai/Kimi-K3` exists only in `together` (`together.rs:286`, the `PROV-070` cyrup-only addition). Both of those providers carry their curated default in-catalog (`moonshotai/Kimi-K2.6`, `model.rs:1085`/`:1087`), so a single-provider user would land there — making the reporter's landing most likely **rank 6**, `available.first()`, over a multi-provider or live-refreshed catalog. It does not change the verdict: rank 4 is unreachable, so ranks 5/6 decide every fresh session.

**Fix** — **one edit, in `cyrup-session-svc`.** Put the settings write in `apply_model_change` (`session.rs:4532`) immediately after the `append_model_change` at `:4563`, mirroring v0.83.0 `agent-session.ts:1586-1587`. That single site covers `set_model`, `set_model_resolved` and both cycle paths, which is exactly upstream's shape — all three `AgentSession` methods carry the pair. Write **both** `defaultProvider` and `defaultModel`, **`SettingsScope::Global` only** (pi writes `this.globalSettings`, never project). **The write seam already exists and needs no new API**: `AgentSession::persist_setting(SettingsScope::Global, key, value)` (`session.rs:3823`) → `SettingsManager::persist_nested` (`crates/cyrup-config/src/settings.rs:1571`), which takes `&self` — so it is callable from `apply_model_change(&self, …)` — and is already the `/settings` grid's persistence path (`crates/cyrup-tui/src/app/execute_misc.rs:232`). **No change is needed in `cyrup-tui`**: upstream's redundant fourth write (`model-selector.ts:357`) is subsumed by the session-layer write, and every TUI selection already routes through `session.set_model`.

**Three decisions to take BEFORE writing the code, none of them obvious.** **(1) `persist_nested` does not refresh the in-memory merged view** — its own doc says so (`settings.rs:1559-1564`: visible in `effective()` only after the next `reload`), whereas pi's `setDefaultModelAndProvider` mutates `globalSettings` in place *and* saves. Next-session behaviour is identical; same-session `default_model()` read-back diverges. Either use the `&mut self` `set_nested` (`settings.rs:1525`) behind a new `&self` seam, or accept the divergence and say so in-source — cyrup's `/settings` rows already have exactly this shape. **(2) Cycling (Ctrl+P) persists at v0.83.0** (`agent-session.ts:1630`, `:1658`), and landing the write in the shared `apply_model_change` gets that for free, which is the faithful port. Upstream reversed it a day ago; the decision should be explicit rather than incidental. **(3) `set_model_id` (`session.rs:3269-3294`) bypasses `apply_model_change` entirely** — it does its own `append_model_change` at `:3292` — and would still not persist. Upstream's RPC `set_model` arm resolves the model and calls `await session.setModel(model)` (`modes/rpc/rpc-mode.ts:468-474`, the call at `:473`), so it *did* persist at v0.83.0; either route this through the shared body or add the write here as well.

**MATCHED SIBLING, found in the same trace, deliberately NOT given its own id in this pass — do not fix SEAM-113 without it.** `set_thinking_level` (`session.rs:3597-3653`) ports pi v0.83.0 `agent-session.ts:1687-1698` and its own comment at `:3641-3644` cites that exact block — yet it implements only the emit half (`:3623` `append_thinking_level_change`, `:3633` `fanout_emit`, `:3638-3651` the ext event) and drops `:1689-1692` `settingsManager.setDefaultThinkingLevel(effectiveLevel)` together with its `if (this.supportsThinking() || effectiveLevel !== "off")` guard. **Shift+Tab and `/settings → Thinking level` therefore never persist either.** Same family as `CFG-056`, which fixed the *getter*; the *writer* has been absent the whole time. The two are one edit's worth of work in one function's neighbourhood, and shipping one without the other leaves a matched pair half-done.

**SECOND RESIDUAL, also unfiled, and it becomes LIVE the moment the write lands.** `builder.rs:1894-1917` is labelled step 3 (`// 3. Settings default → first catalog entry (Pi findInitialModel, sdk.ts:205-221)`) but is not `findInitialModel` step 3: it reads only `default_model()` (`:1896`), **ignores `default_provider()` and the `hasConfiguredAuth` gate**, and resolves the value as a *pattern* against `provider.models()` — the single installed provider's catalog — where upstream (`model-resolver.ts:621-631`) requires **both** keys, does an exact lookup on the **full** registry, and gates on auth. The interactive binary hides this today: `crates/cyrup/src/main.rs:595-613` pre-resolves through `default_launch_model` (`crates/cyrup/src/provider.rs:413`) → `find_initial_model`, which *is* faithful, and hands the builder a fully-qualified `config.model_pattern` that step 1 consumes. It is latent on the SDK/embedder path only — until `defaultModel` is actually written, at which point the same persisted value resolves differently between the two entry points. Fix it in the same pass.

**Verify** — (a) drive `set_model` on a session and assert `<agentDir>/settings.json` carries **both** `defaultProvider` and `defaultModel` for the chosen model, and that the **project** settings file is untouched; (b) build a fresh session (no `--model`, no `--models`, not a resume) against a multi-provider catalog and assert `find_initial_model` returns the persisted pair via rank 4 rather than `first_default_or_first`'s constant — RED at HEAD, and the assertion that actually reproduces the report; (c) the same for Ctrl+P cycling and for `set_model_id`, per decisions (2) and (3). Cases belong beside `crates/cyrup-session-svc/src/tests/settings_resolve.rs`.

## ~~SEAM-116~~ — ~~medium~~ **CLOSED 2026-09-04 (`4481e807`)** — pi's `clear_queue` RPC verb (v0.84.4) has no cyrup counterpart, though the session-layer capability already exists unused

**Kind** upstream-drift · **Severity** ~~medium~~ closed · **Effort** S · **Confidence** confirmed · **filed 2026-09-04 (diff-stat skim, `pi/` v0.84.1 → v0.84.4), CLOSED 2026-09-04**

> **CLOSED 2026-09-04 at `4481e807`** (`feat(modes): SEAM-116 port pi's clear_queue RPC verb
> (v0.84.4)`). The Fix paragraph below was applied as written, plus one shared type:
>
> **cyrup at HEAD.** `SessionCommand::ClearQueue` (`crates/cyrup-modes/src/rpc/types.rs:81`, a unit
> variant after `Abort`, exactly where `rpc-types.ts:26` puts it). The `handle` arm
> (`crates/cyrup-modes/src/rpc/mod.rs:948-965`) calls `session.drain_queue().await` (`:956`) and
> answers `RpcResponse::ok("clear_queue", id, Some(data))` where `data` is the new
> `ClearedQueue { steering, follow_up }` (`types.rs:203`, `Serialize + Deserialize`,
> `rename_all = "camelCase"` so the wire key is `followUp`). It dispatches concurrently, like `abort`
> — it owns no `in_flight` bookkeeping, and pi's `clearQueue()` is synchronous. The client half is
> `RpcClient::clear_queue() -> ClearedQueue` (`crates/cyrup-modes/src/rpc_client.rs:723`), pi's
> `rpc-client.ts:226-229` wrapper; `ClearedQueue` is exported from the crate root (`lib.rs:45`) and
> reaches embedders as `cyrup_sdk::modes::ClearedQueue` alongside `RpcClient` (the SDK re-exports
> neither by name, consistently).
>
> **Design decision (recorded in the commit body).** One `ClearedQueue` struct in `rpc/types.rs`
> for BOTH directions, following the crate's own `RpcResponse` precedent (SEAM-017: "Pi shares ONE
> `RpcResponse` type between its host and its client … so cyrup shares one too"). Rejected: a
> `(Vec<String>, Vec<String>)` return (same-typed halves — a caller restoring them into the editor
> in the wrong order gets no compile error); a raw `Value` (contradicts the typed
> `ModelInfo`/`ForkMessage` client precedent); a client-only struct (host `json!` keys and client
> renames can drift apart).
>
> **Upstream, re-read at v0.84.4 for the closure**: `modes/rpc/rpc-types.ts:26` and `:124-128`,
> `modes/rpc/rpc-mode.ts:433-435`, `modes/rpc/rpc-client.ts:226-229`,
> `core/agent-session.ts:1588-1596`, `packages/coding-agent/docs/rpc.md:137-158`. Tag-to-tag:
> `git -C tmp/pi diff v0.84.1..v0.84.4 -- packages/coding-agent/src/modes/rpc/` shows the verb
> ADDED in this window; `git -C tmp/pi log -S clear_queue v0.84.4 -- …/rpc-types.ts` names
> `a79b37334` (*feat(coding-agent): expose RPC queue clearing*) and `git tag --contains a79b37334`
> → v0.84.4 only. Nothing else in the window touches this verb, so there is no intermediate
> behaviour to reconcile.
>
> **Tests.** `crates/cyrup-modes/src/tests/modes/rpc_clear_queue.rs` (registered in
> `tests/modes/mod.rs`) — `:37` the raw-stream wire shape (`data` is exactly
> `{steering:["Change direction"], followUp:["Summarize when finished"]}` — the docs' own example —
> and the LAST `queue_update` on the wire carries two empty arrays, with a two-message
> `queue_update` asserted to precede it so the empty one is the drain's doing); `:102` end to end
> through `RpcClient::clear_queue` over a duplex pair against `run_rpc` (`get_state`
> `pendingMessageCount` 2 → 0; a second drain answers empty arrays, not an error); `:145` the
> Verify paragraph's concurrency case — one message queued, then `clear_queue` and `steer` raced
> from two tasks, eight rounds, drained ∪ residue asserted equal to exactly the two messages;
> `:200` `{"type":"clear_queue"}` parses to `ClearQueue`. `crates/cyrup-modes/src/tests/rpc_client.rs:639`
> drives the client against the scripted host: the command line carries only `type` + `id`, and the
> reply lands typed. `cargo nextest run -p cyrup-modes -E 'test(/clear_queue/)'` 5/5; the full crate
> 80/80; clippy `--all-targets -D warnings`, `cargo doc` with `-D warnings`, and `cargo check` of
> `cyrup-sdk`/`cyrup-mcp`/`cyrup` all clean. **RED established**: the wire-shape case was run alone
> against the unmodified crate and failed at its `success == true` assertion with the host's
> `{"command":"clear_queue","error":"Unknown command: clear_queue","id":"cq","success":false}` — the
> `#[serde(other)]` path this section's cyrup paragraph predicted; the other four name the new
> variant/method and did not compile before the change.
>
> **Residual, low, recorded here rather than filed.** The `data` object's key order on the wire is
> `followUp`, `steering` — alphabetical — not pi's literal `{ steering, followUp }`. Every `data`
> payload this host emits goes through `serde_json::Value`, whose object is a `BTreeMap` because the
> workspace keeps `preserve_order` OFF on purpose (root `Cargo.toml`, the `mermansi` rejection note:
> flipping it "silently chang[es] map ordering in config persistence, provider request bodies, MCP
> payloads and session records"). JSON object order carries no meaning and pi's `getData` reads by
> name, so no client can observe it; it is the same property every other multi-key `data` payload
> (`{cancelled}`, `{levels}`, `{models}`) already has.
>
> **Falsification** — a `clear_queue` line answering `success:false`; a reply whose `data` lacks
> `steering` or `followUp`, or carries text other than what was queued; or a `get_state` after the
> drain reporting `pendingMessageCount > 0`, reopens it. Nothing below was rewritten.

**upstream** — `pi/packages/coding-agent/src/modes/rpc/rpc-types.ts:26` adds `{ id?: string; type: "clear_queue" }` to `RpcCommand` and `:124-128` adds the matching `{ command: "clear_queue"; success: true; data: { steering: string[]; followUp: string[] } }` to `RpcResponse`. `modes/rpc/rpc-mode.ts:433-435`: `case "clear_queue": { return success(id, "clear_queue", session.clearQueue()); }`. `modes/rpc/rpc-client.ts:226-229` gives `RpcClient` an `async clearQueue()` wrapper. `core/agent-session.ts:1588-1596` `clearQueue()` snapshots both queues, clears them (facade mirrors **and** `this.agent.clearAllQueues()`), emits `_emitQueueUpdate()`, and returns `{ steering, followUp }`. Shipped-docs confirmation, `docs/rpc.md:137-158` @v0.84.4: *"To implement interactive Esc behavior, send `clear_queue` before `abort`, then restore the returned text in the client editor."* None of this exists at the ported baseline `v0.83.0` — `git -C tmp/pi show v0.83.0:packages/coding-agent/src/modes/rpc/rpc-types.ts` has no `clear_queue` member of either union — so this is a verb pi added in the `v0.83.0`→`v0.84.4` drift window, not something the port ever had a chance to carry.

**cyrup** — `crates/cyrup-modes/src/rpc/types.rs:53-` (`SessionCommand`, the RPC request enum) has no `ClearQueue` variant; `crates/cyrup-modes/src/rpc/mod.rs`'s `handle` switch has no matching arm; `grep -rn 'clear_queue\|ClearQueue' crates/cyrup-modes` returns nothing. The session-layer half is already done and unconsumed: `AgentSession::drain_queue` (`crates/cyrup-session-svc/src/session/queue.rs:54-67`) takes both mirrors under their guards, discards the agent's duplicate copies (`self.agent.drain_queues_for_restore()`), emits `queue_update`, and returns `(Vec<String>, Vec<String>)` in `(steering, followUp)` order — its own doc comment cites `agent-session.ts:1416` and names exactly this pi method. Its sibling `clear_queue(&self)` (`:38-43`) is the fire-and-forget half already wired to `SessionCommand::ClearQueue` (`crates/cyrup-session-svc/src/command.rs:30`/`:111-112`) for the **in-process** SDK caller — that path is unrelated to the RPC wire and does not return the drained text, so it cannot stand in for the new verb.

**Impact** — an RPC client implementing pi's documented Esc behavior (drain the queue, restore the text into the editor, then abort) has no verb to call against cyrup; the closest available sequence — read `state.pendingSteering`/`pendingFollowUp` off `get_state`, then `abort` — does not clear atomically and races a concurrent `steer`/`follow_up`, exactly the race `drain_queue`'s own doc comment says the atomic form exists to avoid. Low severity ceiling: no data loss and no wrong output on any EXISTING verb, only an absent one.

**Fix** — add `ClearQueue` to `SessionCommand` (`crates/cyrup-modes/src/rpc/types.rs`), a `case` arm in `handle` (`crates/cyrup-modes/src/rpc/mod.rs`) calling `session.drain_queue().await` and building `RpcResponse::ok("clear_queue", raw_id, Some(json!({"steering": steering, "followUp": follow_up})))`, and (for symmetry with the rest of `RpcClient`'s surface, SEAM-017's territory) a `clear_queue()` method on `crates/cyrup-modes/src/rpc_client.rs`'s `RpcClient`.

**Verify** — drive `{"type":"clear_queue"}` over RPC against a session with queued steering and follow-up text; assert the response carries both arrays verbatim and that a subsequent `get_state` shows empty queues. Add a concurrency case: queue a message, call `clear_queue` and `steer` from two tasks racing, and assert the drained snapshot and the post-clear queue are mutually consistent (no lost update either way).

## SEAM-117 — pi's `message_update` wire projection (v0.84.4) gained `usage` and `toolcall_start.{id,toolName}`; cyrup's projector still emits the pre-drift two-key shape

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** confirmed · **filed 2026-09-04 (diff-stat skim, `pi/` v0.84.1 → v0.84.4)**

**upstream** — `pi/packages/coding-agent/src/modes/json-event.ts` @v0.84.4: `JsonMessageUpdateEvent` (`:11-15`) is now `{ type: "message_update"; usage: Usage; assistantMessageEvent: ToJsonAssistantMessageEvent<…> }` — a THIRD key, `usage`, alongside the two the ported baseline has. `ToJsonAssistantMessageEvent` (`:6-8`) additionally widens the `toolcall_start` member to carry `id: string; toolName: string` on top of its stripped `WithoutPartial` shape. `toJsonAssistantMessageEvent` (`:20-38`) resolves those two fields by indexing the cumulative `partial.content[contentIndex]` for the tool call BEFORE dropping `partial`. `toJsonEvent` (`:48-60`) builds the result as `{ type: "message_update", usage: event.message.usage, assistantMessageEvent: toJsonAssistantMessageEvent(event.assistantMessageEvent) }`. Shipped-docs confirmation, `docs/rpc.md` @v0.84.4: `:945` shows a `"usage": {...}` block inside a `message_update` example, `:971` documents `toolcall_start` as *"Tool call started (includes `id` and `toolName`)"*, `:977-980`/`:988` show `usage` on every streamed line, and `:983-984` states *"The top-level `usage` field contains the latest cumulative provider-reported usage."* Confirmed absent at `v0.84.1` (and at the ported baseline `v0.83.0`) by `git -C tmp/pi diff v0.84.1..v0.84.4 -- packages/coding-agent/src/modes/json-event.ts`, which shows the file growing from a bare two-key identity projection to this shape — so this is drift landing after the ported tag, not a port bug.

**cyrup** — `crates/cyrup-modes/src/json_event.rs`. `JsonAgentSessionEvent`'s `Serialize` impl (`:169-189`) builds `serializer.serialize_map(Some(2))` and writes exactly `"type"` and `"assistantMessageEvent"` — no `usage` key exists anywhere in the function, and `AgentSessionEvent::MessageUpdate`'s destructure (`:172-178`) discards everything but `assistant_message_event` via `..`. `DeltaOnly`'s `StreamEvent::ToolCallStart` arm (`:216-218`) calls `indexed(serializer, "toolcall_start", *content_index)`, the same two-key (`type`+`contentIndex`) helper used for `text_start`/`thinking_start`, carrying neither `id` nor `toolName`.

**Impact** — an RPC/json-mode client built against pi's CURRENT (`docs/rpc.md` @v0.84.4) documentation and expecting per-chunk cumulative `usage` on `message_update`, or expecting to read a tool call's `id`/`toolName` off `toolcall_start` (rather than waiting for `toolcall_end`), gets neither field from cyrup — a silently incomplete wire shape rather than a wrong one, since every field cyrup DOES emit still matches. No crash, no data loss on cyrup's own consumers (the in-process fanout carries the full cumulative message already), so this is capped below critical.

**Fix** — thread the cumulative message's `usage` through to the projection: `AgentSessionEvent::MessageUpdate`'s destructure needs the outer message's usage (or a dedicated field carrying it, mirroring how pi reads `event.message.usage`) rather than discarding it via `..`; add a third `map.serialize_entry("usage", …)` in `JsonAgentSessionEvent::serialize`. For `toolcall_start`, `DeltaOnly` needs the resolved tool call (id + name) at that content index — the cumulative `partial` snapshot the event already carries before it is dropped — and a bespoke 4-key branch (`type`, `contentIndex`, `id`, `toolName`) in place of the shared `indexed()` call.

**Verify** — drive a prompt through `--mode json` or RPC against a tool-using turn; assert every `message_update` line on the wire carries a `usage` object, and that the `toolcall_start` line carries `id` and `toolName` matching the tool call `toolcall_end` later reports for the same `contentIndex`. Byte-diff a `text_delta` line against the pre-fix shape plus the new `usage` key to confirm no other field regressed.

## Coverage

**Read first-hand at cyrup HEAD `04c1ba2`** (tree `a9000b1`, clean). cyrup: `cyrup-modes/src/{rpc.rs — all 1535 lines, across the wire types, the ui-request/effect shapers, `run_rpc`/`rpc_driver`/`write_pump`, `dispatch`, the whole `handle` switch, `state_view`, `read_lines`; print.rs; json.rs; json_event.rs}`; `cyrup-modes/tests/modes.rs` (the `get_session_stats`, setWidget/setStatus, dialog-timeout and `abort_bash` cases); `cyrup/src/{main.rs (arg pipeline `:100-370`, interactive arm `:470-590`, rpc arm `:593-676`, print/json arm `:678-800`, `apply_post_build` `:816-862`, `report_runtime_diagnostics` `:1846-1862`), run.rs (all 188 lines), cli.rs (`:20-260`, `:571-607`, `:690-815`, `:828-930`), diagnostics.rs (all 263 lines), signals.rs (all 117 lines), output_guard.rs, credential_print.rs (`:1-180`, `:431-536`), subcommands.rs}`; `cyrup-session-svc/src/{session.rs (abort/abort_and_settle `:1330-1400`, dispose/dispose_with `:2370-2436`, fork/branch helpers `:2438-2500`, bind_extensions/emit_session_start `:2510-2545`, slash_command_catalog/entries_json/tree_json `:2275-2347`, fork_anchor `:5128-5143`), runtime.rs (`:95-140`, `:200-330`, `:385-465`, `:448-612`), state.rs (all 220 lines), factory.rs `:92-130`, builder.rs `:946-967`}`; `cyrup-sdk/src/{client.rs, handle.rs, lib.rs}`; `cyrup-ext/src/{event.rs, registry.rs `:440-520`, `:640-680`}`; `cyrup-ext/src/facade.rs` command-dispatch sites; `cyrup-tools/src/ops/local.rs` (`setsid`/`killpg`); both `wit/world.wit` copies.

**Added by the repair pass, cyrup side:** `cyrup/src/startup_ui.rs` (all of it — `run_resume_picker` `:121-140`, the trust prompt `:238-274`, `format_saved_trust` `:174-186`, `run_missing_cwd_prompt` `:281-336`, and the tests at `:488-537`), `cyrup/src/main.rs` (`:1040-1170` the startup-UI orchestration, `:1240-1270` the session listings, `:1342-1460` the `--list-models` renderer and filter, `:53-57` the process-identity comment), `cyrup/src/startup.rs`, `cyrup/src/input.rs`, `cyrup/src/provider.rs:230-260`, `cyrup/src/subcommands.rs:590-675`, `cyrup-tui/src/session_selector.rs` (constructor defaults, `handle` `:770-843`, the header/hint rows `:420-470`, `:550-700`), `cyrup-tui/src/startup_selector.rs`, `cyrup-tui/src/theme.rs:144-148`, `cyrup-tui/src/fuzzy.rs:119-175`, `cyrup-config/src/trust.rs:330-390`, `cyrup-session-svc/src/builder.rs:483-524`, `cyrup-session-svc/src/session.rs:3335-3370`, `cyrup-tui/src/app/execute_session.rs:15-34`.

**Upstream read at the NAMED TAG** with `git show <tag>:<path>`, never from a working tree. **v0.83.0** (the ported baseline): `cli/args.ts` (all 400 lines), `cli/credential-print.ts`, `main.ts` (`:100-215`, `:470-620`, `:780-910`), `core/agent-session-runtime.ts` (all 440 lines), `core/agent-session-services.ts` (`:80-200`), `core/source-info.ts`, `core/session-manager.ts` (`:150-170`, `:860-980`, `:1240-1330`), `core/extensions/runner.ts` (`:595-650`), `core/extensions/types.ts`, `core/compaction/compaction.ts`, `modes/rpc/rpc-mode.ts` (the whole command switch `:385-715` plus `shutdown`/`handleInputLine` `:717-810`), `modes/rpc/rpc-types.ts` (all 289 lines). **v0.84.1** (latest): `modes/print-mode.ts`, `modes/rpc/rpc-mode.ts` (`:1-120`, `:280-420`, `:700-810`), `modes/json-event.ts`, `modes/rpc/jsonl.ts`, `core/output-guard.ts`, `utils/shell.ts`, `cli/args.ts` diff, `cli/auth-command.ts`, `cli/experimental/{cli.ts,command.ts,commands/server.ts}`, `server/create-harness.ts`, `main.ts` diff.

**Version-lag sweep method.** `git diff --stat v0.83.0..v0.84.1` scoped to `packages/{server,protocol,client}` and `packages/coding-agent/src/{cli.ts,cli/,main.ts,rpc-entry.ts,server/,modes/}` (81 + 39 files), then `--diff-filter=A` plus `git ls-tree v0.83.0` on each new path to separate "new at v0.84.1" from "existed and changed". Findings: `modes/json-event.ts` is new at v0.84.1 and is **already ported** (`cyrup-modes/src/json_event.rs`, wired at `json.rs:79` and `rpc.rs:300`) — PARITY-GAPS line 392 still lists "JSON/RPC message_update delta projection" as deferred and is **stale on that point**. `rpc-mode.ts`'s other v0.84.1 change is `getAvailable()` → `getAvailableSnapshot()`, which cyrup's already-synchronous `available_model_catalog()` matches (SEAM-004 re-verified against it). `print-mode.ts`'s is `waitForRawStdoutBackpressure` on the agent, which `cyrup-modes/src/json.rs:9-21` argues away with a specific, checkable reason (the seam's bounded-1024 awaited fanout). The residue is SEAM-050 / SEAM-051 / SEAM-058.

**Repair-pass sweep (2026-08-12): the `pi/packages/coding-agent/src/cli/` startup path.** The previous
edition's CLI coverage named `args.ts`, `credential-print.ts`, `auth-command.ts` and `experimental/`
only, and its sweep was a flag-NAME diff by its own admission — so six shipped `cli/` files and the
~1 000 lines of Rust that answer them had been audited by nobody. This pass read
`file-processor.ts`, `initial-message.ts`, `list-models.ts`, `session-picker.ts`,
`config-selector.ts` and `startup-ui.ts` **at both v0.83.0 and v0.84.1**, plus every symbol they
consume (`session-selector.ts`, `core/project-trust.ts`, `core/trust-manager.ts`,
`packages/tui/src/fuzzy.ts`, `packages/ai/src/models.ts`), and walked each into cyrup by ripgrep over
`crates/`. Inter-tag drift in the six: `list-models.ts` gains a third `signal?: AbortSignal` param
(+4 lines, shifting `:29+`); `config-selector.ts:25` and `startup-ui.ts:82` swap `new TUI(...)` for
`new TuiMainScreen(...)`; the other three are byte-identical, as are the four consumed files — so
every line number cited in `SEAM-061` … `SEAM-070` holds at either tag. Result: **10 items, 5 of them
high**, all on the pre-launch surface. The shape is consistent enough to be worth naming: these
screens were built as a thin shell around `cyrup-tui` selectors, and every settings-derived *input*
to that shell (theme, keybindings, which loader, which option set) was dropped on the way in, while
the drawing was ported faithfully. The ADR-0001 substrate carve-out covers `new TUI(...)`; it does
not cover the arguments.

**Confirmed covered by the same sweep, so nobody re-derives it.** `file-processor.ts` → `crates/cyrup/src/input.rs` is good (file-not-found bail, empty-file skip, the 4.5 MB base64 cap, the 80/85/70/55/40 quality ladder, and `autoResizeImages` genuinely threaded from settings at `input.rs:597`, not hardcoded). `buildInitialMessage` → `input.rs:528-551` `compose_inputs` is exact: pi's part order (stdin, fileText, messages[0]), the EMPTY `parts.concat()` separator (`initial-message.ts:40`), follow-ups as `messages[1..]`, images only when non-empty, and the load-bearing `data.trim() || undefined` on piped stdin (`main.ts:80` → `input.rs:564-571`). `formatTokenCount` and the whole `--list-models` table renderer match field for field (`main.rs:1342-1360`, `:1437-1453`). `shouldRunFirstTimeSetup`/`isOfficialDistribution` → `startup.rs:30-117`, all four gate conditions in pi's order. `showStartupSelector`'s missing-session-cwd use → `startup_ui.rs:281-336` + `main.rs:1043-1058`, including pi's `if (!selectedCwd) process.exit(0)`. `ConfigSelectorOptions` → `subcommands.rs:590-675`, including the `globalResolvedPaths` inherited-keys second resolve. `showFirstTimeSetup` is implemented and callerless — already PARITY-GAPS UW-2 / OQ-6, deliberately not re-filed. `session-picker.ts`'s third callback `onExit` (`:43-46`) is **dead upstream** — declared at `session-selector.ts:301`, assigned at `:800-802`, called nowhere — so cyrup collapsing Confirm/Cancel into `ResumeChoice` loses nothing; do not file it.

**Surface sweep method and results.** (1) Mechanical 32-vs-32 RPC verb-set diff from `rpc-types.ts:20-72` against the `SessionCommand` variants at `rpc.rs:84-212` — one difference, `get_available_thinking_levels` (SEAM-014, re-derived rather than taken on trust). (2) Every response payload in `rpc-types.ts:110-231` compared field-by-field against its `handle` arm — produced SEAM-053 and confirmed the `get_state` field set. (3) Every `RpcExtensionUIRequest` union member (`rpc-types.ts:238-273`) against `extension_ui_request_json` (`rpc.rs:319-373`) and `extension_ui_effect_json` (`:394-449`) — only `setWidget` diverges, so SEAM-011 is the single survivor. (4) Full CLI flag-set diff of `args.ts:63-210` against `Cli` + `KNOWN_LONG_FLAGS` — pi has one flag cyrup lacks (SEAM-051), cyrup has three pi lacks (SEAM-057). (5) Framing surface: `jsonl.ts` vs `read_lines`/`write_out` — produced SEAM-054; the trailing-unterminated-line-at-EOF and CRLF cases match exactly. (6) Signal surface: pi's per-host `registerSignalHandlers` vs `signals.rs` plus a full `CancelToken` consumer trace — produced SEAM-047 and, on re-check, SEAM-059. (7) `AgentSessionRuntime` method surface against `runtime.rs` — produced SEAM-049 and SEAM-056. (8) `SessionTreeNode` vs `tree_json` — produced SEAM-060.

**Severity re-derivation and mechanism carve-outs (repair pass).** Two ratings changed and are argued
in their item bodies (`SEAM-051` → high, `SEAM-020` → medium). Two further candidates were examined
under `README.md:106-107` and **stay where they are**, recorded so the next pass does not re-open
them: `SEAM-047` stays `high` (an unstoppable `--mode rpc` host is a hang and a resource leak, not
data loss or a bypass — though it becomes the worst item in the file the moment anyone runs cyrup
under a supervisor), and `SEAM-059` stays `medium` (Ctrl-C after a session replacement fails to stop
the live turn, which wastes tokens and ignores an explicit user instruction, but destroys nothing).
Neither is a `critical` by the definition; both are ranked above every other medium in practice by
their coupling to `SEAM-047`. Separately, the mechanism carve-outs that the `cli/` sweep declined to
extend are recorded with the sweep above: `new TUI(...)` / `TuiMainScreen` widget-tree plumbing,
`clearStartupTui`'s 25 ms settle (unnecessary because cyrup enters and leaves the alternate screen,
`startup_selector.rs:44,62`), the `new Promise` + double-resolve latch idiom, and `loadStartupThemes`'
`packageManager.resolve(async () => "skip")` install-prompt suppression are all genuinely
mechanism-N/A — the settings-derived arguments they carry are not, and are filed as `SEAM-066`.

**Rejected with reason, so no future pass re-derives them.** *SEAM-019 as written* — `--ui-mode`/`--alt` do not exist at v0.83.0 or v0.84.1 (`git grep -nE 'uiMode|"--alt"' v0.83.0 -- packages/coding-agent/src` is empty); the ID is retained as misdescribed and superseded by SEAM-051, not deleted. *SEAM-048's "two different functions" mechanism* — `resolved_command_owner` has **no production caller** (`grep -rn resolved_command_owner crates/` returns its definition plus three assertions in `cyrup-ext/tests/aggregation.rs:236-238`); live dispatch uses the last-wins `command_owner` at `facade.rs:328`, `facade.rs:1220`, `session.rs:1036` and `session.rs:1274`, so the item is written against the real mechanism. *SEAM-053's `fork` instance* — the `fork` verb passes `ForkPosition::Before` and always yields `Some(text)` on success; `clone` (the `position:"at"` verb) emits no `text` key at all, matching pi `rpc-mode.ts:625`. *SEAM-029's "cyrup cites pi wrongly" half* — `cli.rs`'s `args.ts:57,130,135` citations are accurate at the tag; only `diagnostics.rs:51` is off by two. *SEAM-008 raised to high* — the severity belongs to SEAM-047; double-booking one open half across two items distorts the plan. *`AgentSessionRuntime::dispose` awaiting `abort_and_settle`* where pi's `dispose()` does not abort at all — cyrup is strictly safer and this is the documented SEAM-024 fix, not a gap.

**Checked and deliberately NOT filed**, so nobody redoes it. `--export` (`Exported to: {path}`, `main.rs:1330`, positional output path from `cli.positionals.first()`) matches `main.ts:578-590`. `resolve_app_mode` (`cli.rs:571-585`) matches `resolveAppMode` (`main.ts:109-120`) including both TTY tests, and `should_take_over_stdout` (`cli.rs:690-692`) matches `isPlainRuntimeMetadataCommand` (`main.ts:126-128`). `PI_STARTUP_BENCHMARK only supports interactive mode` is present at `main.rs:588-590` (pi `main.ts:857-861`). The RPC-only background catalog refresh is present and correctly mode-gated at `main.rs:349-364` (pi `main.ts:863-866`). The non-interactive no-models guard is present at `main.rs:656`/`:764` → `no_models_available()` (`main.rs:1893`). `--min-expiry` unit parsing matches `credential-print.ts:50-61`. `get_entries`' `since` filter and `Entry not found:` message, `get_tree`'s `{tree,leafId}` envelope, `export_html`'s `{path}`, `cycle_model`'s `{model,thinkingLevel,isScoped}|null` and `cycle_thinking_level`'s `{level}|null` all match their `rpc-types.ts` lines. `read_lines`' CRLF and trailing-partial-line handling matches `jsonl.ts` exactly.

**Blind spots and things taken on trust.** Static only — nothing was built, run or tested. (a) SEAM-047's "`--mode rpc` never returns after SIGTERM" is traced end-to-end through the code (no cancel consumer, no signal-reachable loop exit) but not reproduced; the reasoning depends on the standard tokio behaviour that a `ReceiverStream` whose senders are still held pends forever, which is taken on trust. (b) SEAM-052's `cyrup <version>` rendering rests on clap's documented `ArgAction::Version` output, not on observed output — the ORDERING half is airtight from `main.rs:119` vs `:130` and does not depend on it. (c) SEAM-056's severity assumes a persisted-but-unwritten session yields an IO error from `SessionManager::open`; only the absence of pi's actionable message in `crates/` was verified, not the text cyrup actually produces.

**Still blind after the repair pass.** The pre-launch surface is now read on the *bin* side, but three
things next to it are not. (a) `cyrup-tui/src/session_selector.rs` was read for the members
`SEAM-061`/`SEAM-062` turn on — the scope/`show_path`/rename paths, the header and hint rows, and
`handle` — and **not** line by line against `modes/interactive/components/session-selector.ts` (1 000+
lines upstream); the sort modes, the named-filter, the threaded/flat rendering and the delete
confirmation are unaudited on both sides, and this pass found two defects in the parts it did open,
so the prior for more is high. (b) `run_startup_selector` (`cyrup-tui/src/startup_selector.rs`) and
the `Selector` it mounts were read for their theme/keymap inputs only. (c) `cyrup config`
(`subcommands.rs:590-675`) was checked against `ConfigSelectorOptions` but its interactive body was
not driven. All three are TTY surfaces, so per the standing rule nothing here closes without a live
run regardless of how the tests read.

**Not audited — a divergence in any of these would not have been caught here.** The INNER element shapes on the RPC wire remain largely unread on both sides: `entries_json()` elements vs pi's `SessionEntry`, `BashResult`, the `Model` serialization that `set_model`/`get_available_models`/`get_state` all embed, and `AgentMessage` in `get_messages`. These are the largest payloads on the wire and the previous edition flagged them as unaudited too; this pass closed only the `tree_json` corner of the class (SEAM-060), which immediately produced a finding — expect more in the rest of it. The `AgentSessionEvent` union against pi's event union (the bulk of the RPC stdout stream) belongs to areas 03/06 and was not re-derived; only the `message_update` projection was verified. `packages/server`'s v0.83.0→v0.84.1 rewrite (81 files, +8588/−1985 — a complete replacement of `cli.ts`/ipc/`supervisor.ts` by a unix-transport session server) was enumerated but not read; PARITY-GAPS asserts the package is outside the port's dependency closure and that was not independently re-verified against v0.84.1's `package.json`. `pi/packages/protocol` and `pi/packages/client` were listed but not read — VL-P23 owns them.

**Method limits.** *(Partly superseded by the repair pass: the `cli/` startup path is no longer a
name-only diff — six files were read end to end at both tags. What follows still holds for
`args.ts`'s per-flag semantics, which the repair pass did not touch.)* The CLI sweep compared flag NAMES and their leniency treatment exhaustively; it did not compare per-flag SEMANTICS end to end (e.g. whether `--print`'s next-token capture at `args.ts:140-146` is reproduced by clap's positional handling, or whether `--name` with no following value produces pi's `--name requires a value` at `args.ts:102` rather than a clap exit-2 usage error — `--name` is in `VALUE_LONG_FLAGS` at `diagnostics.rs:76` and falls through when it is the last token, but the resulting message was not traced). The RPC handler sweep read every arm of `handle`, but the blocking/concurrent dispatch classification (`is_inline_command`, `rpc.rs:881-890`) was checked for `prompt`/`steer`/`follow_up` only. The workspace has **no `CLAUDE.md`** — the file this README and PARITY-GAPS repeatedly cite for the deliberate-divergence list and the out-of-scope pi package list does not exist here — so "is this package in scope?" could only be answered from PARITY-GAPS' own assertion, which is why SEAM-058 is filed as tracking rather than work.

**Handoffs (repair pass additions).** To **area 07** (TUI): `SEAM-061`'s and `SEAM-062`'s selector
halves live in `crates/cyrup-tui/src/session_selector.rs` — the missing `SessionAction::ToggleScope`
arm, the `show_path`/`scope` coupling, and a `set_rename_enabled` gate; the loader and `on_apply`
halves are this area's, and neither item closes on one half alone. `SEAM-063`'s in-app `/resume` call
site is `cyrup-tui/src/app/execute_session.rs:15-34`, sharing the helper with the pre-launch site. To **area 05**:
`SEAM-067` must land *after* whatever keybinding-name alias table that area's repair pass produces
for pi's `migrateKeybindingsConfigFile`, or a legacy `keybindings.json` will still read as empty in
the pre-launch selectors; and `SEAM-066`'s theme resolution reads the same startup `SettingsManager`
area 05 owns. To **area 01**: `SEAM-020`'s auth predicate is a provider-tier concern
(`provider.rs:237-256`), and the v0.84.1 `AbortSignal.timeout` on `getAvailable` remains PARITY-GAPS
VL-P6 there, not here. To **area 04**: `SEAM-070` is the process-*name* half only — the
`PI_CODING_AGENT` env half of the same `main.rs:53-57` block is TOOL-031 / PB-5.

**Handoffs.** To **area 06** (`cyrup-ext`): SEAM-048's and SEAM-055's fixes both land in `cyrup-ext/src/registry.rs:655-669`, `command_names()` carries the identical last-wins defect for whatever else consumes it, and the four `command_owner` dispatch sites named in SEAM-048 are that area's call. To **area 07** (TUI): SEAM-051's rendering half is the alt-screen work (PARITY-GAPS VL-P19); only the arg-parsing half is filed here. To **area 05**: `--tui-mode`'s settings tier (`tuiMode` in `settings-manager.ts:135`/`:1128-1134`) is a settings key not audited here. To **area 01**: the v0.84.1 15-second `AbortSignal.timeout` threaded through `ModelRuntime.create`, `resolveModelScope`, `listModels` and `modelRuntime.refresh` (`main.ts:738`/`:792`/`:864`/`:915`) is PARITY-GAPS VL-P6, a provider-tier concern — only that cyrup's call sites pass no deadline was confirmed here.

**Citation hygiene.** Every cyrup line number in this edition was re-resolved by grep/read at HEAD in this pass, not carried over from the previous edition — several had moved by 100+ lines (`abort()` 1242→1348, `dispose` 2165→2380, `emit_session_start` 2206→2525, the setWidget emitter 395→423, `state_view` 1226→1410, the WIT `set-widget` 307→326). Where the auditor and the refuter disagreed on a line by one or two, the refuter's number is the one written.
