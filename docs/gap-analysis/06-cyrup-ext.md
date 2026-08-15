# 06 — cyrup-ext (the extension host)

This area covers the extension host itself: the event catalog and dispatch reduction (`cyrup-ext/src/{event,dispatch,facade,registry}.rs`), the WIT world and its wasmtime runtime (`cyrup-ext/wit/world.wit`, `cyrup-ext/src/host/`), the native built-in extension path (`cyrup-ext/src/native.rs`), the guest SDK (`cyrup-ext-sdk/`), and the `cyrup-session-svc` and `cyrup-tui` wiring that is the only production consumer of any of it. It is measured against `pi/packages/coding-agent/src/core/extensions/` at the ported baseline **pi v0.83.0** — `types.ts`, `runner.ts`, `loader.ts` — with post-baseline drift measured against **pi v0.84.1**. The standing caveat is that cyrup's WASM Component Model host is a deliberate *mechanism* divergence from pi's jiti/TypeScript loader; the *semantics* of the event, registration and context surfaces are fully in scope.

> **Re-audited 2026-08-12, cyrup HEAD `a9000b1`** (branch `david/cyrup`, working tree clean; last **code** commit `04c1ba2`), upstream **pi v0.83.0** (ported baseline) with the version-lag sweep run against **v0.84.1**.
>
> **6 items closed this pass** — EXT-008, EXT-012, EXT-S01, EXT-S02, **EXT-S03**, EXT-S06. That includes both of the area's audited highs (EXT-S01, EXT-S02), so **06 now has no open high inherited from a prior pass**; `00-residual-ledger.md:60`/`:161`/`:256` still list EXT-S02 as open on the strength of `commands.rs:31` typing `SlashCommand::name` as `&'static str`, which is stale — it is `Cow<'static, str>` at `commands.rs:36`.
>
> **1 closure reopened in part** — EXT-005. Its closure was conditioned on two `CYRUP-DELTA` provenance notes being written into `crates/cyrup-ext/src/lib.rs`; `grep -n 'CYRUP-DELTA\|scopedModels\|ctx.signal' crates/cyrup-ext/src/lib.rs` returns zero at HEAD and both gaps are still live. Residual filed as **EXT-045**. Four further items are now **partially closed** with named residuals: EXT-006, EXT-028, EXT-033, EXT-S04, EXT-S05.
>
> **21 items newly filed** — EXT-037…EXT-057. Four are post-baseline **upstream drift** measured on `v0.83.0..v0.84.1` (EXT-049 `ToolCallEventResult.terminate`, EXT-050 the guarded `api.events` wrapper, EXT-051 `refreshToken(signal)`/`isSubscription`, EXT-052 the `streamSimple` `onPayload`/`onResponse` contract); the evidence re-scoping for EXT-019 and EXT-022 came from the same sweep and is folded into those items rather than filed twice. **One is the area's only open critical** — EXT-054, `ExtensionManifest.capabilities` is parsed and never read by any code path, so cyrup's own documented per-extension WASM sandbox grant model is entirely inert. It was found by walking the crate's own security claims rather than pi's surface, which is exactly the blind spot a pi-anchored item-driven sweep cannot cover.
>
> Open now: **1 critical · 0 high · 28 medium · 21 low** (50 items). Treat that as a floor, not a total — see `## Coverage`.
>
> ### Update 2026-08-13 (sandbox-and-extension-gating pass)
>
> **EXT-054 and EXT-055 are FIXED**, so the area has **no open critical**. Recount:
> **0 critical · 0 high · 27 medium · 22 low** (49 open). The deltas: `EXT-054` (crit) and `EXT-055`
> (medium) close; `EXT-058` is re-rated medium → low with its DEFECT classification refuted (pi's
> `--offline` is documented "startup network operations" verbatim, `args.ts:277` @v0.83.0, and pi has
> no extension network gate at all — see the item); `EXT-059` is filed new (medium,
> `AgentSession::load_wasm_extension` is still a full-authority manifest-less load). Blind spot 4 /
> repair-pass note 9 — "nobody has verified the `is_trusted = origin.is_pre_trust() ||
> project_trusted` gate" — is **still open**: this pass threaded the manifest grant through and
> enforced it, but did not audit `caps/{http,proc}.rs` for a hole in the trust gate itself.
>
> ### Repair pass 2026-08-12 (post-critique)
>
> Applied after the completeness critique of the twelve finished area files. Three changes, no new
> items and no renumbering:
>
> 1. **EXT-054 raised `high` → `critical`** (critique finding 3). README:106-107 defines `critical` as
>    "data loss, silent wrong output, **a permission bypass**, or a crash on a normal path", with no
>    reachability qualifier. The prior rating held it at high on the argument that zero WASM guests
>    ship today. Re-verified at HEAD before raising: `load_discovered` (`facade.rs:1166-1184`) calls
>    `self.load_wasm(id.clone(), &bytes, services)` and `load_wasm` (`facade.rs:1063-1070`) takes
>    `(id, bytes, services)` — the manifest is provably not in the signature, so the declared grant
>    cannot be applied. That is a permission bypass on the definition as written. The zero-guest
>    reachability argument is retained **inside the item** as blast-radius context for the planner; it
>    is not a reason to file the item below its class.
> 2. **Status rows added for EXT-037…EXT-057** (critique finding 17b). The collapsed
>    `EXT-037 … EXT-057 | new this pass` row hid twenty-one items from the status table that the
>    README declares must cover "every item from every prior pass". Each now has its own row.
> 3. **The sweep axis this pass actually used is now recorded in `## Coverage`**, and the sweep it did
>    **not** run is recorded as an explicit blind spot (critique finding 12). Area 06's new items came
>    from walking cyrup's own security and capability claims; that is a legitimate axis, but it was
>    written up as a happy accident rather than a method, and no enumeration of
>    `core/extensions/`'s exported surface was performed.

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
> **Area 06 — recount: 53 rows → 24 open (0 critical · 0 high · 11 medium · 13 low).** The header's
> "0 critical · 0 high · 27 medium · 22 low (49 open)" is superseded. `EXT-N01` is closed (it was
> fixed and the row's title already said so), so the area has no open high either.
>
> **SECOND WIT BUMP: `HOST_WORLD` moved `cyrup:ext@0.5` → `@0.6`** as sweep 2's single batched change,
> under the same ADR-0002 one-bump-per-pass rule sweep 1 used for 0.4 → 0.5. Members: the **import
> re-signings** `ui.set-widget` (`EXT-047`) and `ui.theme-list` (`EXT-021`) — both the fails-to-LINK
> kind, which is what forces the bump; the **export addition** `transform-markdown` (`EXT-019`) plus
> its declaring import `registration.register-markdown-transformer`; and the additive imports
> `ui.set-working-message` / `set-working-visible` / `set-working-indicator` /
> `set-hidden-thinking-label` / `theme-get-by-name`, which would not have needed a bump alone. A guest
> built against 0.5 is now correctly refused by `check_world`. The full history block is written into
> `manifest.rs:152-176`. **The bump is NOT yet proven against a real guest**: host bindgen accepts the
> new shapes and `cargo check -p cyrup-ext-sdk --target wasm32-wasip2` expands `export_extension!`
> cleanly, so host and guest agree at the type level, but the Tier-1 fixture component in `cyrup-it`
> is built by a separate path and was never run. **The verification phase's first move for this area
> should be to build and instantiate that fixture** — a failure will present as an opaque wasmtime
> link error, which is exactly what `check_world` exists to pre-empt, and the ABI fingerprint in
> `build.rs` should have invalidated the artifact cache for both WIT copies, but that invalidation is
> itself untested.
>
> **The `EXT-034` invariant is now recorded in its row, because it is not in the item and a future
> editor will otherwise re-break it:** the bus drain MUST be skipped when a dispatch carries an
> `exclude`. Removing that argument reintroduces a **HANG, not a failure** — see the mechanism-gap
> note below and in `00-residual-ledger.md`.
>
> **A SECOND fabricated-citation instance was found, of exactly the class `EXT-036` was filed for, in
> a place `EXT-036`'s sweep did not look** — `interface ui`, not the event catalog.
> `working-start: func(label: string)` / `working-stop: func()` are documented "(Pi
> startWorking/stopWorking, types.ts:265-275)". Neither function exists upstream:
> `git grep -n 'startWorking\|stopWorking' v0.83.0 -- packages/coding-agent/src` returns nothing, and
> types.ts:265-275 is getAllThemes/getTheme/setTheme/getToolsExpanded. The pair is **cyrup-original**
> and its invented shape is strictly weaker than pi's — it welds the message to the visibility, so
> `setWorkingVisible(false)` with the message intact was unexpressible. Both now carry a
> `[CYRUP-DELTA]` in `world.wit` and `cyrup-ext-sdk/src/ctx.rs`. **The `EXT-036` sweep should be
> re-run over the import surfaces, not only the event catalog.**
>
> **THREE ITEMS ARE STRUCTURALLY UNCLOSEABLE FROM THIS AREA AND SHOULD BE MOVED, NOT RETRIED.**
> `EXT-025` (both fix options land outside area 06 — the live paths are session-svc, the only callers
> of the dead methods are tests in `cyrup-it`), `EXT-003` and `EXT-059` (session-svc), and
> `EXT-013`/`041`/`053` plus the residuals of `EXT-039`/`040`/`019` (cyrup-tui). Re-filing them costs
> nothing and stops a fourth pass re-deriving the same blocked plan.
>
> **Blind spot 3 advanced again; blind spot 4 is now the area's largest unexamined surface.** The
> whole `HostServices` default-impl block was read end to end while adding `all_tools`, `commands`,
> the four working/thinking verbs, `theme_by_name` and the re-signed `set_widget`, and `GuestState`'s
> widget bookkeeping was rewritten. `caps/{http,fs,proc}.rs` and the `is_trusted` gate — roughly 1000
> lines below `GuestState` — have never been read by any pass and are now **the only part of
> `host/services.rs` nobody has walked**. Given `EXT-054` found the entire manifest capability model
> inert, that is where to look next.
>
> **PROCESS NOTE for the orchestrator:** four sibling crates (cyrup-agent, cyrup-session, cyrup-config,
> cyrup-provider) were transiently uncompilable at different points during sweep 2 from concurrent
> agents' in-flight edits, and each outage blocks every `cargo check -p cyrup-ext*` through the
> dependency graph. "Check after each edit" is not achievable under parallel execution in one tree.


## Status of every item from prior analyses

| ID | Status | Note |
|---|---|---|
| EXT-001 | **closed** | Re-derived at HEAD. `EventKind::fails_closed` is `matches!(self, EventKind::ToolCall)` (`event.rs:155-157`); the `Err(e)` arm of `dispatch_block_mutate` reports then returns `Blocked{reason:"Extension failed, blocking execution: …"}` only when `kind.fails_closed()`, else `continue` (`dispatch.rs:248-257`). Upstream asymmetry re-read verbatim: `emitToolCall` has no try/catch while `emitUserBash`/`emitContext` immediately below it both wrap the handler and call `emitError` (`runner.ts` @v0.83.0). Do **not** weaken `fails_closed` to fix EXT-029. |
| EXT-002 | **closed** | Both `from_agent` fns return `None` for `MessageEnd` (`event.rs:175`, `:437`); sole production emit `cyrup-session-svc/src/subscriber.rs:156`, guarded by a `no_subscribers` check at `:151-154`; same-role guard is the `discriminant` comparison in `emit_message_end` (`facade.rs:571-581`). upstream `MessageEndEventResult.message` "must keep the original message role", `types.ts:1091-1094` @v0.83.0. |
| EXT-003 | **still open** (medium) | Both halves survive at HEAD. `saved: None` still passed literally (`builder.rs:516`); `remember` branch still a bare `tracing::warn!` (`:501-511`); `pre_trust_extension_verdict` still opens `ExtensionHost::with_wasm(host_config).ok()?` (`:1648`) *before* the native vote loop (`:1652`). |
| EXT-004 | **closed** | Chain re-walked: `mark/take_tools_dirty` (`registry.rs:266-274`) → `refresh_tools` (`facade.rs:403-408`) → `materialize_guest_tools` (`:413-435`), called by `load_wasm` (`:1102`) and `active_tools` (`:384`). upstream: `registerTool` ends with `runtime.refreshTools()` on every registration (`loader.ts:245-252`). The EXT-030 residual lives inside the same function (`facade.rs:431-433`). |
| EXT-005 | **partially closed — reopened in part** | WIT half closed and improved: `ctx-state` now carries `get-mode`/`has-ui` (`world.wit:528,531`) beside is-idle/has-pending-messages/is-project-trusted/get-system-prompt (`:533-539`); `control.abort`/`shutdown` ungated (`:568,:572`). But the two `CYRUP-DELTA` notes the closure required were never written, and both gaps are real — `scopedModels` (`types.ts:326`) and `signal` (`:334`) have no WIT counterpart. Residual → **EXT-045**. |
| EXT-006 | **partially closed — still open** (medium) | Replay half for **custom messages** closed: `replay_session_with_extensions` (`cyrup-tui/src/app.rs:1305-1327`), two production callers (`cyrup/src/main.rs:1685`, `app.rs:6988`). Three residuals remain: display options + theme on `render-call`, the one-shot render at ingest, and replayed **tool** rows → filed separately as **EXT-041**. |
| EXT-007 | **closed 2026-08-14 — REFUTED** | **REFUTED by sweep 6; the row above was stale.** The `prompt_guidelines` override on `impl Tool for WasmTool` is at `crates/cyrup-ext/src/host/live.rs:1808-1810` (`fn prompt_guidelines(&self) -> Vec<&str>`), with the provenance note at `:1801-1807` recording that the blocker was `Tool::prompt_guidelines` returning `&[&str]` and that the signature was changed to an owned slice. vs pi `extensions/types.ts:459` @v0.83.0. Closed by a sweep between 3 and 5 |
| EXT-008 | **closed** | `register_tool` rejects a foreign owner and records an `ExtensionConflict` (`registry.rs:217-224`) before the insert; `register_guest_tool` likewise (`:288-295`); `tool_owner_in` (`:238-245`) unifies the executable and descriptor tables. The `tool_renderer_owner.remove` at `:303` is now reachable only for the same owner. upstream first-wins on both sides (`runner.ts:450-471`). Conflict-diagnostic surfacing for **shortcuts** is a separate gap → EXT-039. |
| EXT-009 | **closed 2026-08-14 — REFUTED** | **REFUTED by sweep 6.** `crates/cyrup-ext/src/event.rs:59` is `BeforeProviderHeaders = 31`, `COUNT: u8 = 33` at `:75`, the `from_u8` arm at `:112`, the name arm at `:154`. The recorded evidence (`AgentSettled = 30`, `COUNT = 31`, grep returning only `world.wit` header lines) is stale on every clause. vs pi `extensions/types.ts:686-689`, subscribed `:1212` @v0.83.0 |
| EXT-010 | **closed** | `AgentSettled = 30` (`event.rs:53`), `on-agent-settled: func();` (`world.wit:218`), dispatch precedes the session fan-out (`subscriber.rs:226` then `:228`), run-tail synthesis (`session.rs:751-757`). `diff` of the two `world.wit` copies is empty. upstream subscribed at `types.ts:1217`. |
| EXT-011 | **closed 2026-08-14 — REFUTED** | **REFUTED by sweep 6.** `SessionInfoChanged = 32` at `crates/cyrup-ext/src/event.rs:62`, routed at `:113` and `:155`. vs pi `extensions/types.ts:571-575`, subscribed `:1203` @v0.83.0 |
| EXT-012 | **closed** | `register-entry-renderer` (`world.wit:290`), host import (`host/live.rs:137-142`), first-wins `entry_renderer_owner` table (`registry.rs:192`, `:353-361`, `:364-366`), `render_entry` + three-state `RenderOutcome` (`facade.rs:808-813`, `:1381-1394`), native `InitApi::register_entry_renderer` (`native.rs:295-297`), TUI consumer (`app.rs:4376`), test `tests/entry_renderer.rs`. upstream `registerEntryRenderer` `types.ts:1279` @v0.83.0 (the auditor's `:1291` is the v0.84.1 line). Replay of custom **entries** is still unwired → EXT-041. |
| EXT-013 | **still open** (medium) | Unchanged. `command_completions` (`facade.rs:1207-1214`) has one caller, `tests/discover_load.rs:100`; `argument_completions` (`host/live.rs:1178-1195`) one non-SDK caller, `tests/wasm_component.rs:169`. Now sharper: since EXT-S02 closed, the TUI *does* list extension commands but hardcodes `has_arg_completion: false` (`cyrup-tui/src/commands.rs:348`). |
| EXT-014 | **still open** (medium) | Unchanged, and understated: `ToolExecutionUpdateEvent` carries `args` as well as `toolName` (`types.ts:769-775`), so cyrup drops **two** fields there, not one. |
| EXT-015 | **still open** (medium) | Unchanged; all four upstream refs re-counted exact at v0.83.0 (`types.ts:562`, `:578`, `:585`, `:616`). cyrup also drops `reason` from `session_before_switch`. |
| EXT-016 | **closed 2026-08-14 — REFUTED** | **REFUTED by sweep 6.** `world.wit:380` is `on-resources-discover: func(cwd: string, reason: string) -> hook-outcome`, with the `{type, cwd, reason}` shape documented at `:379`. vs pi `extensions/types.ts:544` (`ResourcesDiscoverEvent`) @v0.83.0 |
| EXT-017 | **still open** (medium) | Unchanged in code, **worse in consequence**: since EXT-S02 closed, `command_descriptions`' unordered `HashMap` walk (`registry.rs:662-669`) feeds `slash_command_catalog` (`session.rs:2277,:2281`) which feeds the interactive `/` menu (`app.rs:4103,:6340,:6937`). `resolved_commands`/`resolved_command_owner` remain production-dead. |
| EXT-018 | **closed 2026-08-14 — REFUTED** | **REFUTED by sweep 6 on the load-bearing half.** `grep -c bus crates/cyrup-ext/src/native.rs` = **15** at HEAD (the row asserts 0), and no `cfg`-gated `bus` line survives in `facade.rs`. vs pi `extensions/loader.ts:389` @v0.83.0 |
| EXT-019 | **still open** (low) | Still zero hits at HEAD. **Re-scoped to v0.84.1**: upstream now has `MarkdownTransformContext {messageType, isStreaming, availableWidth}`, `registerMarkdownTransformer`, `markdownTransformer?` on `Extension`, and `ExtensionRunner.getMarkdownTransformers()`. Port the v0.84.1 shape, not the v0.83.0 one. |
| EXT-020 | **closed** | `usage` on `HostEvent::ToolResult` (`event.rs:271-274`), `on-tool-result(…, usage-json: option<string>)` (`world.wit:180`), `EventPatch::ToolResult` (`contract.rs:36-41`). EXT-028's live-coverage residual also closed by `tests/wasm_tool_result_usage.rs` (117 lines). |
| EXT-021 | **still open** (medium) | Unchanged; all eight upstream cites re-verified line-exactly at v0.83.0. A **ninth** partial found: `setWorkingMessage(message?)` (`types.ts:151`) is only half-covered by `working-start(label)`. `setWidget`'s dropped key/placement is filed separately as **EXT-047**. |
| EXT-022 | **still open** (low) | Unchanged in cyrup. **Contract changed upstream**: `refreshModels?` moved `types.ts:1448` @v0.83.0 → `:1469` @v0.84.1 and its doc now says `Use context.publish({ persist: entry })` where it said `context.store`. Re-scope the fix. |
| EXT-023 | **closed 2026-08-14 — REFUTED** | **REFUTED by sweep 6.** `prepare-arguments` occurs 4 times in `world.wit`, declared as a flag on `tool-descriptor` at `:82` with the closure-cannot-cross-a-component-boundary rationale at `:77-81`, and `tool-descriptor` now carries **12** fields including `render-shell` and `constrained-sampling` — not the 8 the row asserts. Closed by EXT-023 + EXT-024/PROV-011 work in sweeps 3-5. vs pi `extensions/types.ts:468` @v0.83.0 |
| EXT-024 | **still open** (medium) | Unchanged. **Evidence correction**: the auditor's `grep 'constrained_sampling'` = 0 is false — `cyrup-provider/src/api/bedrock_converse_stream.rs:44` matches, but the hit is a comment reading "cyrup's `ToolDef` has no such field", which confirms rather than refutes. `ToolRenderKind` still has zero consumers. |
| EXT-025 | **still open** (low) | Unchanged; per-symbol greps re-run. Drift confirmed on both sides — the live production input path is the inline `AgentSession::emit_input_event` (`session.rs:907`, called from `:872`), and the live `emit_user_bash_event` (`session.rs:4590-4614`) threads `exclude_from_context` and the **session** cwd while the facade copy hardcodes `false` and the **process** cwd (`facade.rs:617-627`). |
| EXT-026 | **still open** (low) | Unchanged; the never-compiled `not(wasm-host)` arms have multiplied (`facade.rs:441-443`, `:899-908`, `:1261-1264`). Static only — no `cargo check --no-default-features` was run this pass either. |
| EXT-027 | **still open** (low) | Unchanged; no bundled-extension tier exists. upstream kept moving (`llama/index.ts` +11, `llama/provider.ts` +28 over `v0.83.0..v0.84.1`). |
| EXT-028 | **partially closed — still open** (low) | All four filed halves are genuinely closed: `HOST_WORLD = "cyrup:ext@0.4"` with the restated "added, removed, or **re-signed**" rule and a bump history naming `f777e44` (`manifest.rs:41-69`); `package cyrup:ext@0.4.0;` in both byte-identical copies (`world.wit:18`); two tie tests (`tests/wit_world_sync.rs:71-89`, `:96-129`); a compile-time `ABI_FINGERPRINT` folded into `cache_key` (`build.rs`, `build/abi.rs`, `build/mod.rs:31-33,:60-66`, pinned at `wit_world_sync.rs:135-160`). **Residual**: the file's own version marker rotted through two bumps — see the item below. |
| EXT-029 | **still open** (medium) | Unchanged, and **worse in practice**: since EXT-S03 closed, the spurious `onError` is now rendered into the interactive transcript as `Extension "<id>" error: cancelled` (`app.rs:3161-3165`, drained at `:6807`). |
| EXT-030 | **closed 2026-08-14 — REFUTED** | **REFUTED by sweep 6.** The wholesale clear is repaired in place: `crates/cyrup-ext/src/facade.rs:551` carries an explicit note that it 'also swallowed the deliberate `mark_tools_dirty()`'. The row's line numbers (`:419-424`, `:431-433`) are stale. vs pi `extensions/loader.ts:245-252` @v0.83.0 |
| EXT-031 | **still open** (medium) | Unchanged. `prepare_next_turn` still sets only `update.tools` (`cyrup-session-svc/src/hooks.rs:179`) with the divergence rationale at `:158-160`. A documented divergence is still a gap — there is no accepted-divergence category. |
| EXT-032 | **closed 2026-08-14 — REFUTED** | **REFUTED by sweep 6.** `crates/cyrup-ext/src/tests/native_dispatch.rs:872` now reads 'EXT-032: the wall-clock assertion that used to sit here' — the flaky bound was removed and the deterministic outcome assertion kept |
| EXT-033 | **closed 2026-08-14 — REFUTED** | **REFUTED by sweep 6 on the half this row keeps open.** The DIAGNOSTIC branch for a CONFIGURED path is `crates/cyrup-ext/src/loader.rs:171`, with coverage at `tests/payload_and_seam_parity.rs:896-899`. Fully closed |
| EXT-034 | **closed 2026-08-14 — REFUTED** | **REFUTED by sweep 6 as stated** — `deliver_bus_events` is at `facade.rs:1319` with production call sites at `:1598` and `:1752` (the row's `:1201`/`:1255` are stale). **KEEP the row's load-bearing invariant:** the bus drain MUST be skipped when a dispatch carries an `exclude`, or the result is a HANG, not a failure — it is preserved in-source at `facade.rs:189` and `:1108`. vs pi `extensions/event-bus.ts:18-27` @v0.83.0 |
| EXT-035 | **still open** (medium) | Unchanged in kind, **worse in ratio**: `InitApi` gained exactly one surface (`register_entry_renderer`, `native.rs:295-297`) while `interface registration` now offers **eleven** verbs plus `subscribe`, so a native reaches 5 of 11. `run_shortcut` still resolves owners out of `self.live` only and is still cfg-gated. |
| EXT-036 | **closed 2026-08-14 — REFUTED** | **CLOSED by sweep 6 — this row's 'four stale sites survive' correction was right and has now been discharged, and the sweep was extended past `interface ui` as this file asked.** See the EXT-036 row in `## Open items` for the four NEW citation clusters and three fabrications the extended sweep found |
| EXT-S01 | **closed** | The native load loop contains each failure per-extension — `if let Err(e) = host.load_native_with_services(...)` collects an `ExtensionLoadDiagnostic{path: id, error, fatal: true}` and continues (`builder.rs:775-786`), with the rationale at `:761-774` correctly noting the bin still exits 1, so a fail-closed permission gate does **not** become a fail-open session. `release_id` (`facade.rs:1345-1349`, called `:251-253`) stops a failed native poisoning `loaded_ids()`. upstream `loader.ts:414-440`, `:481-500`. |
| EXT-S02 | **closed** | `SlashCommand::name` is `Cow<'static, str>` (`cyrup-tui/src/commands.rs:36`); `dynamic_commands_from_catalog_gated` (`:308-350`) maps catalog rows to `CommandSource::{Extension,Prompt,Skill}` honouring `enableSkillCommands`; `CommandRegistry::with_dynamic` (`:167-178`) merges behind the builtins; installed at `app.rs:4103`, `:6340`, `:6937`. upstream `interactive-mode.ts:601-628`, skill gate `:610`. Three behaviours inside this seam remain open separately: EXT-017, EXT-013, EXT-053. |
| EXT-S03 | **closed** | `App::install_error_listener` (`app.rs:3140-3149`) forwards contained `ExtensionError`s onto an unbounded channel, drained at `:6807` into `show_extension_error` (`:3161-3165`), installed at boot (`:6328`) and re-installed on session swap (`:6927`). upstream copy is byte-identical (`interactive-mode.ts:2545-2546`, bound at `:1701`). Documented residual: cyrup's `ExtensionError` has no `stack`, so pi's dimmed stack line is absent (stated at `app.rs:3155-3158`). |
| EXT-S04 | **partially closed — still open** (low) | `customInstructions` ported end to end (`world.wit:558`, `host/live.rs:829`, SDK `ctx.rs:1087-1090`, consumer `session.rs:2862`); this is why `HOST_WORLD` moved 0.3→0.4. The callback substitution is incomplete — see the item below. |
| EXT-S05 | **partially closed** | `mode` + `hasUI` closed (`world.wit:528,531`, `ext-mode` enum `:25`, copied from the same `HostConfig` the native path uses at `facade.rs:1081`, coverage `tests/guest_host_mode.rs`). The third member of the same upstream sentence — `cwd` (`types.ts:315`) — is still absent; residual → **EXT-044**. |
| EXT-S06 | **closed** | `scan_dir` handles the direct-file rule (`loader.rs:172-180`), entries sorted for determinism (`:171`), `push_file` de-dupes on the canonicalized **artifact** path (`:199-201`), and `is_component_file` uses `is_file()` which follows symlinks, matching pi's `entry.isFile() || entry.isSymbolicLink()` (stated at `loader.rs:152-154`). upstream `discoverExtensionsInDir` rule 1. |
| EXT-037 | **new this pass** — open (low) | `ext-tools.get-commands` returns bare extension command names only. Severity lowered medium → low against the auditor: guest-only introspection API, zero shipping guests. EXT-017 is the same defect on the user-visible path and carries the medium. |
| EXT-038 | **new this pass** — open (medium) | `ext-tools.get-all-tools` returns only extension tools and drops `promptGuidelines` + `sourceInfo`. The `promptGuidelines` half is blocked on the same `Tool::prompt_guidelines` signature change as EXT-007. |
| EXT-039 | **new this pass** — open (medium) | Extension shortcuts bypass the reserved-keybinding refusal, emit no conflict diagnostics, and lose to built-ins. Precedence half is inferred, not read — see `## Coverage` §Inferred. Read side is TUI-N05 in area 07. |
| EXT-040 | **new this pass** — open (low) | `register-shortcut`'s description is discarded, so `/hotkeys` shows a raw key-id. |
| EXT-041 | **new this pass** — open (medium) | Replayed tool calls and results lose their extension renderer. Split out of EXT-006's third residual so the custom-message replay closure is not re-opened. |
| EXT-042 | **new this pass** — open (low) | `model_select` buries `previousModel`/`source` in the blob; `thinking_level_select` drops `previousLevel`. |
| EXT-043 | **new this pass** — open (medium) | The `project_trust` event carries no `cwd`, so the deciding extension cannot key its verdict. Pairs with EXT-003's `remember` persistence. |
| EXT-044 | **closed 2026-08-14 — REFUTED** | **REFUTED by sweep 6.** `cwd` reaches a WASM guest at `world.wit:312` (on-user-bash), `:380` (on-resources-discover) and `:385` (on-project-trust); the crate-level CYRUP-DELTA register at `crates/cyrup-ext/src/lib.rs:22` records the closure. vs pi `extensions/types.ts:315` @v0.83.0 |
| EXT-045 | **closed 2026-08-14 — REFUTED** | **REFUTED by sweep 6 on BOTH halves.** `scoped-models: func() -> string` is `world.wit:655`, and the two CYRUP-DELTA notes this item exists to punish are now a full register at `crates/cyrup-ext/src/lib.rs:18-44` — 'what interface ctx-state deliberately does NOT mirror' — naming `signal` (ported as the `is-run-cancelled` poll, `world.wit:870`, because a Component Model value cannot be a callback target) and `sessionManager`/`modelRegistry`. Two further registers exist at `:46-76` (`interface ui`) and `:78-110` (`UserBashEventResult.operations`). vs pi `extensions/types.ts:326`, `:334` @v0.83.0 |
| EXT-046 | **closed 2026-08-14 — REFUTED** | **REFUTED by sweep 6.** `world.wit:646` is `set-label: func(entry-id: string, label: option<string>)` — `none` is upstream's `undefined`. vs pi `extensions/types.ts:1314` @v0.83.0 |
| EXT-047 | **new this pass** — open (medium) | `ui.set-widget` drops pi's widget key and placement. Read side is TUI-014 (area 07); the RPC split is area 08. |
| EXT-048 | **new this pass** — open (low) | The dialog timeout key is `timeoutMs`; pi's field is `timeout`, and the in-tree citations for it are wrong. |
| EXT-049 | **closed 2026-08-14 — REFUTED** | **REFUTED by sweep 6.** `world.wit:51-54` declares `record block-result { reason: option<string>, terminate: bool }` with the upstream doc text quoted verbatim and the correct **@v0.84.1** tag (the field is ABSENT at the v0.83.0 baseline), and the note that only `on-tool-call` reads it. vs pi `extensions/types.ts:1072-1079` @v0.84.1 |
| EXT-050 | **new this pass** — open (low) | `pi.events` gained stale-context guarding and tracked unsubscribe; cyrup's bus has neither. Post-baseline drift. One function with EXT-018, EXT-034 and EXT-057. |
| EXT-051 | **new this pass** — open (low) | Extension provider OAuth drifted: `refreshToken` gained a signal, `oauth` gained `isSubscription`. Auditor's "grep returns nothing" evidence was **corrected** — the transport exists; the typed field and a consumer do not. |
| EXT-052 | **closed 2026-08-14 — REFUTED** | **REFUTED by sweep 6.** `world.wit:808-809` declares `on-payload: func(stream-id, payload-json) -> option<string>` and `on-response: func(stream-id, status: u16, headers-json)` on `interface provider-stream`, implemented at `host/live.rs:917` and `:942` with the must-invoke contract quoted, wired through `facade.rs:1431` and `services.rs:1242`/`:1609`/`:1618`. vs pi `extensions/types.ts:1452-1457` @v0.84.1 |
| EXT-053 | **new this pass** — open (low) | An extension command shadowing a built-in is dropped from autocomplete with no diagnostic. Surfaced by EXT-S02's closure. |
| EXT-054 | **FIXED 2026-08-13** (was **critical**) | `ExtensionManifest.capabilities` is parsed and never read by any code path — the declared per-extension WASM sandbox grant model is entirely inert. Re-verified at HEAD: `load_wasm` (`facade.rs:1063-1070`) never receives the manifest `load_discovered` (`:1166-1184`) holds. Permission bypass per README:106-107. |
| EXT-055 | **FIXED 2026-08-13** (was medium; fixed with EXT-054, and its `with_fs_root` symbol name was wrong — the mutator is `GuestState::with_fs`) | `FsCaps::with_fs_root` has zero callers, so `ext-fs` is permanently denied for every guest. Fail-closed, and the mirror of EXT-054 — same root cause, opposite failure direction. Fix in the same change. |
| EXT-058 | **open** (re-rated medium → low 2026-08-13; DEFECT classification refuted — pi's `--offline` is documented "startup network operations" verbatim and pi has no extension network gate) | Guest WASM `http-client` is not gated by `--offline`. |
| EXT-059 | **new 2026-08-13** — open (medium) | `AgentSession::load_wasm_extension` is a full-authority manifest-less load reachable as public API. Filed from the EXT-054 fix; `session.rs` was outside that pass's ownership. |
| EXT-056 | **closed 2026-08-14 — REFUTED** | **REFUTED by sweep 6.** `crates/cyrup-ext/src/registry.rs:409` states 'FIRST registration wins (EXT-056), like every sibling table', with regression coverage at `tests/payload_and_seam_parity.rs:415-418` whose doc names the last-wins `insert` it replaced. vs pi `extensions/loader.ts:314-318` @v0.83.0 |
| EXT-057 | **closed 2026-08-14 — REFUTED** | **REFUTED by sweep 6 on both halves.** EXT-057a (silent drop at the round bound) is named and closed at `facade.rs:1305`; EXT-057b (a faulting listener never reaching the `onError` channel) at `facade.rs:1309`, with the onError routing point recorded at `dispatch.rs:186`. vs pi `extensions/event-bus.ts:18-27` @v0.83.0 |

## Open items

> Both the original items and the `-S` surface-sweep items are now in **one** table. The
> 2026-08-03 split (which is how `SEAM-S01` escaped a full pass) is gone; do not re-split it.

> **RECOUNTED 2026-08-15 (area-06 pass, fifth edition) — counted set: 0 critical, 0 high, 8 medium, 11 low = 19 open.** The table carries **71 rows: 52 fully closed, 19 open (4 of them partially closed).** This pass closed **sixteen** rows — including `EXT-021`, `-037` and `-038`, whose RESIDUALS (all three routed to "area 08") were re-checked at HEAD and are already implemented in `cyrup-session-svc/src/host_services.rs`: the live theme backend (SEAM-T01), `LiveHostServices::commands` and `LiveHostServices::all_tools`. Three more rows that would have been re-routed to another area on their paper state. The other thirteen: `EXT-050`, `-061`, `-062`, `-063`, `-065`, `-066`, `-067`, `-068`, `-069`, `-070`, `-072`, `-073` and the newly filed `EXT-074`; `EXT-064` moved to partially closed. **The single most important fact for the next pass: NINE of the sixteen were ALREADY DONE at HEAD and this file did not know it.** `EXT-065`/`-066`/`-067`/`-068`/`-069` landed with the 0.7 → 0.8 world bump in `350bdb5`, and half of `EXT-062`/`-070` with them; the last edit to this file (`c06bb0c`) predates that commit, so every row filed on 2026-08-14 describes a tree four commits old. Verify at HEAD before estimating — the ledger's own framing was the stale artefact, not the code.
>
> **Two findings this pass that were not in the backlog.** (1) `EXT-074`, the mirror of this area's characteristic defect: a tier RESTRICTION advertised in `world.wit` that the host stopped enforcing at GAP-11. (2) The `HOST_WORLD` bump rule was about to be applied backwards on `EXT-061`: `check_world` passes when the GUEST minor is `>=` the host's, so the gate defends only old-guest-on-new-host, an ADDED import cannot fail that direction, and the direction it could fail is accepted whatever the numbers are. A bump for an additive import refuses every already-built guest and prevents nothing. `world.wit`'s header claimed the opposite for `ui.theme-get-json` while `manifest.rs`'s history entry for the SAME import said the correct thing; both now agree, in `manifest.rs`'s direction.
>
> **SUPERSEDED — RECOUNTED 2026-08-14 (ext-rpc surface enumeration, fourth edition) — counted set: 0 critical, 0 high, 18 medium, 17 low = 35.** The table carries **70 rows: 35 fully closed, 35 open (11 of them partially closed).** Thirteen items were filed this pass — **`EXT-061`…`EXT-073`** — from a MECHANICAL two-sided enumeration of the extension API surface (pi `core/extensions/types.ts` @v0.83.0: all 33 `on(event:)` overloads, all 28 `ExtensionUIContext` members, all 18 `ExtensionContext` members, all 7 `ExtensionCommandContext` additions, all 24 non-`on` `ExtensionAPI` members, all 13 `ToolDefinition` fields, all 5 `ToolInfo` keys, all 4 `PiManifest` keys, against all 157 WIT functions across 16 interfaces in both copies), NOT from reading the backlog. **`EXT-071` closes on arrival** (fixed this pass) and does not move the open counts; **`EXT-073` is partially fixed** and stays open on its residual. `EXT-061` is the single unaccounted-for MEMBER the enumeration found — everything else it produced is a shape, placement or provenance divergence on a member that is present, which is the class an item-driven sweep structurally cannot see. *(Previous edition: 0 / 0 / 11 / 12 = 23, 34 closed, 57 rows.)*
>
> **SUPERSEDED — RECOUNTED 2026-08-14 (sweeps 7-8 reconciliation, third edition) — counted set: 0 critical, 0 high, 11 medium, 12 low = 23.** The table carries **57 rows: 34 fully closed, 23 open (10 of them partially closed)** — 57 because this reconciliation **filed the missing `EXT-M03` row**: the ID is cited five times in `crates/cyrup-ext/src/host/live.rs` and had no row in this file at all, so a reader following those citations found nothing. The work landed in sweep 6; only the bookkeeping was absent. It is closed on arrival and does not move the open counts. Sweep 8 closed **`EXT-060`** (low) as already-done, resolved by deletion. Two rows stay open with **corrected fix sites that were wrong in a way that would have produced no-op fixes**: `EXT-013` ("NOW A ONE-LINER" — it is not; the catalog carries no arg-completion signal) and `EXT-024` (needs a `cyrup-session-svc` producer before any TUI branch has anything to read). Both corrections are recorded in their rows. *(Previous edition: 0 / 0 / 11 / 13 = 24, 32 closed.)*

> **SUPERSEDED — RECOUNTED 2026-08-14 (sweeps 3-6 reconciliation) — counted set: 0 critical, 0 high, 11 medium, 13 low = 24, UNCHANGED.** 32 rows are now marked CLOSED. `EXT-060` was filed by the sweeps 1-2 reconciliation; **`EXT-M01` (medium) and `EXT-M02` (low) were filed AND closed by sweep 6** *(and `EXT-M03` was fixed by sweep 6 but its row was never written — filed retroactively by the sweeps 7-8 reconciliation; see the third-edition note above)* — both are mechanism gaps found by hunting rather than from the backlog, so they add to the closed set without moving the open counts. **Sweep 6 also refuted eighteen rows in `## Status of every item from prior analyses` that still read "still open"** (`EXT-007`, `-009`, `-011`, `-016`, `-018`, `-023`, `-030`, `-032`, `-033`, `-034`, `-036`, `-044`, `-045`, `-046`, `-049`, `-052`, `-056`, `-057`) — every one had already been closed in this table by a sweep between 3 and 5, so that table, not this one, was the stale artefact. *(Counted set previously: 0 / 0 / 11 / 13 = 24, 29 closed.)*
>
> **ROUTING — `TOOL-024` belongs to this area's code but keeps its id in `04-cyrup-tools.md` (ids are never renumbered or moved between files, and duplicating the row would double-count it).** Both its fix sites are `crates/cyrup-ext/src/wrapper.rs` — the `Fixed` double at `:229-230` and `every_surface_method_delegates` at `:379-380`. Two sweeps have now routed a *tools* agent at this cyrup-ext defect; whoever next owns cyrup-ext should take it. See standing rule 2 in `## Coverage`.
>
> **STILL UNDISCHARGED, and it is the first thing the verification phase should do for this area:** the 0.6 and 0.7 `HOST_WORLD` bumps are NOT proven against a real guest. Host bindgen accepts the shapes and `cargo check -p cyrup-ext-sdk --target wasm32-wasip2` expands the guest side cleanly, so host and guest agree at the TYPE level — but the Tier-1 fixture component lives in `crates/cyrup-it`, which is `required-features = ["it"]` and is therefore never built or run by the 16-second gate, and it has still never been instantiated at 0.7. A failure will present as an opaque wasmtime LINK error, which is exactly what `check_world` exists to pre-empt.
>
> **SECOND STANDING RISK, unchanged since sweep 2:** `crates/cyrup-ext/src/caps/{http,fs,proc}.rs` and the `is_trusted = origin.is_pre_trust() || project_trusted` gate — roughly 1000 lines — have never been read by any pass. Sweep 6 read only the ~80 lines of `caps/proc.rs` around its `select!` while hunting; that is not an audit. Given `EXT-054` found the entire manifest capability model inert, that is where the next security-shaped sweep should look.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| ~~EXT-054~~ | ~~**critical**~~ | cyrup-original | M | **FIXED 2026-08-13** — `capabilities` now reaches instantiation via `load_wasm_with_caps`; enforced host-side; `tests/manifest_capabilities.rs` (9 tests, 5 RED before) |
| EXT-059 | medium | cyrup-original | S | `AgentSession::load_wasm_extension` is a full-authority manifest-less load reachable as public API — **new, filed 2026-08-13 from the EXT-054 fix** — **2026-08-14, still open**: sweeps 2 and 6 — not reached from this area; **FIX SITE: `crates/cyrup-session-svc/src/session.rs`.** Needs an owner whose feature includes cyrup-session-svc. |
| EXT-058 | low | product-decision | S | Guest WASM `http-client` is not gated by `--offline` — **classification refuted 2026-08-13**: pi's flag is documented "startup network operations" verbatim (`args.ts:277` @v0.83.0) and pi has no extension network gate at all, so this is parity, not a defect. The control that was missing was EXT-054's `"net": false`, now enforced. Open as a decision — **2026-08-14, still open**: sweep 2 — correctly untouched: an open product decision whose defect classification was already refuted. |
| EXT-003 | medium | not-ported | M | Project-trust store is unwired at the extension seam — **2026-08-14, still open**: sweeps 2 and 6 — not reached; **FIX SITE: wholly `crates/cyrup-session-svc/src/builder.rs`** (`saved: None`, the `remember` warn-only branch, and `pre_trust_extension_verdict`'s ordering). Re-verified still open at HEAD by sweep 6, which could not touch it. |
| EXT-006 | medium | parity-bug | L | Renderers run without display options or theme, and only once — **2026-08-14, still open**: sweep 2 — not reached; the WIT half must land WITH the cyrup-tui draw-path move or it is an ABI break with no behaviour change (area 07). |
| ~~EXT-007~~ | ~~medium~~ **CLOSED 2026-08-14 — REFUTED** | parity-bug | M | The first system prompt is built from built-ins only; guest promptGuidelines are dropped — **REFUTED, CLOSED 2026-08-14**: sweep 2 — **REFUTED: the claimed blocker is stale, and three separate places in the record were planning around it.** `Tool::prompt_guidelines` already returns `Vec<&str>` (`crates/cyrup-core/src/tool.rs:130`), `ToolDescriptor.prompt_guidelines: Vec<String>` exists (`registry.rs:27`), and `impl Tool for WasmTool` already overrides it (`host/live.rs:1690`) with an in-source note saying TOOL-021/EXT-007 unblocked it. **RE-SCOPED, not closed:** the live residual is one producer in cyrup-session-svc — the FIRST system prompt is built from built-ins only, so guest guidelines are dropped at that one point. The cross-area blocker is dropped from this item, from EXT-038's Fix text and from TOOL-021 (now closed). |
| ~~EXT-009~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | M | before_provider_headers event is missing entirely — **CLOSED 2026-08-14**: sweep 1. |
| ~~EXT-011~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | S | session_info_changed is emitted internally but never crosses the extension boundary — **CLOSED 2026-08-14**: sweep 1. |
| EXT-013 | medium | parity-bug | M | Slash-command argument completions and autocomplete providers are dead — **2026-08-14, STILL OPEN, and the "NOW A ONE-LINER" routing is WRONG — it has now cost two sweeps.** Sweep 8 re-verified at HEAD. The cyrup-ext side does exist and work, and the built-in half **has landed** (`arg_cmd` sets the flag true for `/model` and `/login`, `cyrup-tui/src/commands.rs:105-118`, pinned at `tests/commands.rs:236-238`). But the dynamic half **cannot be set from `cyrup-tui` at all**: the line has drifted to **`commands.rs:358`**, and `dynamic_commands_from_catalog_gated` builds those rows from `AgentSession::slash_command_catalog()`, which emits **only** `name`/`description`/`source`/`sourceInfo` (`crates/cyrup-session-svc/src/session.rs:2504-2514`) and carries **no arg-completion signal**. The data never arrives, so "one line" would be a no-op. **CORRECTED FIX SITE: `crates/cyrup-session-svc` (widen the catalog row with the arg-completion signal, sourced from `ResolvedCommand`/`CommandDescriptor`) PLUS `crates/cyrup-tui` — one owner, one commit.** pi carries it by mapping the registered command straight onto the `SlashCommand` (`modes/interactive/interactive-mode.ts:601-608` @v0.83.0; the field is `RegisteredCommand.getArgumentCompletions?` at `core/extensions/types.ts:1162-1168`; the consumer is `packages/tui/src/autocomplete.ts:346-350`). |
| ~~EXT-014~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | M | tool_execution_update / tool_execution_end drop toolName (and args) — **CLOSED 2026-08-14**: sweep 1. |
| ~~EXT-015~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | M | Session-lifecycle events lose their discriminating fields at the extension boundary — **CLOSED 2026-08-14**: sweep 1 — the extension-boundary halves are area 06's; the session-svc PRODUCERS (session.rs:2691/:2545/:2956/:3337, runtime.rs:480/:526/:568/:706) landed concurrently in area 08 with matching shapes, derived independently from pi. |
| EXT-017 | medium — **PARTIALLY CLOSED 2026-08-14** | parity-bug | S | Command listing is non-deterministic, drops name:N, and a colliding command is unexecutable — **PARTIALLY CLOSED 2026-08-14**: sweep 1 — the catalog + wasm command dispatcher use `resolved_commands`/`resolved_command_owner`. **RESIDUAL: `cyrup-session-svc/src/session.rs:2277` still emits the bare name (area 08).** EXT-037 fixed the same defect on the guest-facing path, which is the half inside area 06. |
| ~~EXT-018~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | M | The inter-extension event bus is wasm-only — natives have no pi.events — **CLOSED 2026-08-14**: sweep 1 — `bus` is no longer `#[cfg(feature = "wasm-host")]`-gated; upstream ref corrected to loader.ts:389 @v0.83.0. |
| ~~EXT-021~~ | ~~medium~~ **CLOSED 2026-08-15 — residual verified gone** | not-ported | L | ctx.ui capabilities with no WIT representation (at least eight) — **PARTIALLY CLOSED 2026-08-14**: sweep 2 — the cheap (import-only) half landed: `interface ui` gained `set-working-message`, `set-working-visible`, `set-working-indicator`, `set-hidden-thinking-label`, `theme-get-by-name`, and `theme-list` widened from a json array of NAMES to `{name, path}` rows, with matching `HostServices` methods, `ui::Host` impls and SDK wrappers. **ITEM CORRECTED:** it says `working-start`/`working-stop` "collapse pi's working-indicator controls". They collapse nothing — **pi has no `startWorking`/`stopWorking` at v0.83.0 at all** (`git grep -n 'startWorking\|stopWorking' v0.83.0 -- packages/coding-agent/src` is EMPTY) and the types.ts:265-275 the pair cites is getAllThemes/getTheme/setTheme/getToolsExpanded. They are cyrup-ORIGINAL with a fabricated citation, and the invented shape is strictly WEAKER than pi's — it welds the message to the visibility, so `setWorkingVisible(false)` with the message intact was unexpressible. Both now carry a CYRUP-DELTA in `world.wit` and `cyrup-ext-sdk/src/ctx.rs`. **RESIDUAL, NARROWED 2026-08-14 (sweep 6):** `onTerminalInput` (types.ts:145) is **CLOSED** — the `on-terminal-input` export plus `ui.subscribe-terminal-input`/`unsubscribe-terminal-input` imports landed with the 0.6 → 0.7 bump, and `crates/cyrup-ext/src/lib.rs:70-76` records it with the bullet deliberately kept and struck through rather than deleted. What survives is (a) `setEditorComponent` (:260) / `getEditorComponent` (:263), which now carry an explicit CYRUP-DELTA at `lib.rs:53-67`, and (b) the other half — **no live backend fills `theme_list`/`theme_by_name` anywhere in the workspace, which is AREA 08 and was not addressed by sweep 6.** **2026-08-15: RESIDUAL CLOSED.** Both halves of the narrowed residual are done at HEAD: (a) the `setEditorComponent`/`getEditorComponent` CYRUP-DELTA is written out in `crates/cyrup-ext/src/lib.rs:53-65` with the Component-Model reason, and (b) the backend the row said was missing now exists — `LiveHostServices` overrides `theme`/`theme_list`/`theme_by_name`/`set_theme` off `ThemeAccess` (`cyrup-session-svc/src/host_services.rs:1139-1156`, SEAM-T01), so `theme_list`/`theme_by_name` are no longer filled by nobody. |
| ~~EXT-023~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | M | prepareArguments is unreachable for WASM guest tools, and the SDK drops the field silently — **CLOSED 2026-08-14**: sweep 1 — the WIT gained `prepare`; the SDK's `prepare_arguments` now crosses the ABI. |
| EXT-024 | medium | parity-bug | M | renderShell/constrainedSampling unexpressible, and render_kind has zero consumers — **2026-08-14, still open**: sweep 2 — not reached; `render_kind` still has no TUI consumer (area 07) and `constrainedSampling` is area 01 (PROV-011). **FIX SITE CORRECTED 2026-08-14 (sweep 8), same correction as `TOOL-022`/`TOOL-015`: "cyrup-tui (+ cyrup-core)" is INCOMPLETE and landing only that half is a no-op.** `grep -rn 'render_kind\|ToolRenderKind\|SelfRendered' crates/cyrup-tui/src/` is **zero**, but the reason is that **`cyrup-tui` has no tool-metadata channel at all** — `ToolRun` (`transcript.rs:701-720`) holds only `name: String`, there is no `tool_info`/`tool_catalog`/`set_tools` accessor on `App`, and the extension-renderer siblings `rendered_call`/`rendered_result` are supplied per-call by the caller rather than looked up. A producer must publish a `name → ToolRenderKind` map from **`crates/cyrup-session-svc`** first. **And pi's actual mechanism, which none of the three rows states:** `renderShell: "self"` makes `ToolExecutionComponent` add a bare `selfRenderContainer` instead of the framed `contentBox` **at construction** (`modes/interactive/components/tool-execution.ts:65-76` @v0.83.0), with three-way precedence `toolDefinition.renderShell ?? builtInToolDefinition.renderShell ?? "default"` at `:105-113`. See `04-cyrup-tools.md` under `TOOL-022`. |
| ~~EXT-029~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | An abort landing during a gated tool-call dispatch reports as an extension failure — **CLOSED 2026-08-14**: sweep 1. |
| ~~EXT-030~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | materialize_guest_tools unconditionally clears the tools-dirty flag, swallowing its own re-arm — **CLOSED 2026-08-14**: sweep 1. |
| EXT-031 | medium | parity-bug | L | Turn-boundary refresh propagates tools but not the rebuilt system prompt — **2026-08-14, still open**: sweep 2 — not reached; the turn-boundary system-prompt rebuild lives in cyrup-session-svc. Note DRIFT-033 landed the two-prompt-slot model there, which is this item's prerequisite. |
| ~~EXT-033~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | A configured extension path that is unloadable or nonexistent produces no diagnostic — **CLOSED 2026-08-14**: sweep 1. |
| ~~EXT-034~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | M | Bus events emitted from an event handler are never delivered — **CLOSED 2026-08-14**: sweep 2 — the bus fan-out is extracted into `BusFanout` (`facade.rs`), a standalone object sharing `Arc<NativeMap>`/`Arc<LiveMap>`/`Arc<SharedBus>` with the host and holding `Arc<Dispatcher>` for `report_external`; `Dispatcher` holds it back as `Weak<dyn crate::bus::BusDrain>` — **weak is load-bearing, not an optimization**: the fan-out owns the dispatcher, so a strong edge would leak the entire host, registry and every loaded guest for the process lifetime. Every dispatch entry point drains after its subscriber loop, including BOTH arms of `dispatch_first_handled` and all four early returns of the new `block_mutate_chain` helper. **The re-scope note remaining.json asked for is obsolete** — the fix did NOT need `ExtensionHost::live`/`::native` to become `Arc<RwLock<..>>`; they became Arc-SHARED, a smaller change than the item's proposed design, and `bus_deliver_one` stays in one place. **NEW CONSTRAINT that replaced the old one, recorded because a future editor will otherwise re-break it: the drain MUST be skipped when a dispatch carries an `exclude`** — see the mechanism-gap register below; removing that argument reintroduces a HANG, not a failure. |
| ~~EXT-035~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | M | NativeExtension can register only 5 of the 11 WIT registration surfaces — **CLOSED 2026-08-14**: sweep 1 + 2 — the InitApi surfaces landed in sweep 1; sweep 2 added the missing `NativeExtension::execute_shortcut(&self, key, ctx)` (native.rs:326 had been referencing a method that did not exist) and rewrote `ExtensionHost::run_shortcut` as ONE ungated function resolving the owner from the registry and trying `self.native` before `self.live`, dropping the `#[cfg(feature="wasm-host")]` gate and its unconditionally-failing `#[cfg(not(...))]` twin. **Strike the item's "5 of 11" framing — it is 11 of 11 with a firing path.** |
| ~~EXT-038~~ | ~~medium~~ **CLOSED 2026-08-15 — residual verified gone** | parity-bug | S | ext-tools.get-all-tools returns only extension tools and drops promptGuidelines and sourceInfo — **PARTIALLY CLOSED 2026-08-14**: sweep 2 — `HostServices::all_tools() -> Option<Vec<Value>>` added; `ext_tools::get_all_tools` prefers it and keeps `registry.tool_info()` as the no-session fallback. `tool_info()` itself gained `promptGuidelines` (readable now that the EXT-007 blocker is refuted) and `sourceInfo` in pi's `{path, source, scope, origin}` shape. **RESIDUAL (area 08): the `LiveHostServices::all_tools` impl off the dynamic tool registry, so built-ins appear.** Mechanism note filed below: pi's `ToolInfo` has NO `source` field, and cyrup's `registry.tool_info()` emits a cyrup-invented `source: "extension"\|"guest"` discriminator — the WASM-vs-native tier leaking into a guest-facing introspection API, which pi's one-extension-kind model has no word for. Left in place rather than removing a key an unknown consumer might read, but it is an unlabelled cyrup-original on a parity surface and should either get a CYRUP-DELTA or go. **2026-08-15: RESIDUAL CLOSED.** `LiveHostServices::all_tools` exists (`cyrup-session-svc/src/host_services.rs:1734+`) off the dynamic tool registry, so built-ins appear, in pi's five-key `ToolInfo` shape with a CYRUP-DELTA recording the one remaining divergence (name-sorted rows vs pi's registration order, because the registry is a `BTreeMap`). Pinned by `all_tools_reports_the_whole_merged_registry_in_pis_toolinfo_shape`. The mechanism note about the invented `source` key was resolved separately by EXT-060 (deleted). |
| EXT-039 | medium — **PARTIALLY CLOSED 2026-08-14** | not-ported | M | Extension shortcuts bypass the reserved-keybinding refusal, emit no conflict diagnostics, and lose to built-ins — **PARTIALLY CLOSED 2026-08-14**: sweep 1 — the registry half is done (`resolve_shortcuts`, `RESERVED_KEYBINDINGS_FOR_EXTENSION_CONFLICTS`, `shortcut_diagnostics`), re-verified at HEAD in sweep 2. **PRECEDENCE CLAIM REWRITTEN:** the Fix said two extensions on the same key should stop "silently collapsing" — but pi is LAST-wins there (`extensionShortcuts.set(normalizedKey, shortcut)` runs unconditionally after the warning, runner.ts:530-536, whose text is literally "Using ${shortcut.extensionPath}"). Only the DIAGNOSTIC was missing. And pi gates in `getShortcuts` against the resolved keybindings, not in `registerShortcut`. **RESIDUAL (area 07): call `resolve_shortcuts`, invert app.rs:1691-1703, thread `shortcut_diagnostics` into `startup_diagnostics.extensions`.** |
| EXT-041 | medium | parity-bug | M | Replayed tool calls and results lose their extension renderer — **2026-08-14, still open**: sweep 2 — not reached (cyrup-tui). |
| ~~EXT-043~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | S | The project_trust event carries no cwd — **CLOSED 2026-08-14**: sweep 1 — the extension-boundary halves are area 06's; the session-svc PRODUCERS (session.rs:2691/:2545/:2956/:3337, runtime.rs:480/:526/:568/:706) landed concurrently in area 08 with matching shapes, derived independently from pi. |
| ~~EXT-044~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | S | ctx.cwd is unreachable from a WASM guest — **CLOSED 2026-08-14**: sweep 1. |
| ~~EXT-047~~ | ~~medium~~ **CLOSED 2026-08-14** | parity-bug | M | ui.set-widget drops pi's widget key and placement — **CLOSED 2026-08-14**: sweep 2 — CLOSED AT BOTH ENDS, derived independently in two areas in one pass. Area 06 re-signed the WIT import to `set-widget: func(key, content-json: option<string>, opts-json: string)` in both `world.wit` copies, added `WidgetPlacement`/`WidgetEffect` to `host/services.rs`, and gave the SDK `Ui::set_widget(key, lines, placement)` plus `Ui::clear_widget(key)` for pi's `content: undefined`. Area 08 landed the `UiEffect::SetWidget` carrier, the `extension_ui_effect_json` projection onto pi's `widgetKey`/`widgetLines`/`widgetPlacement`, and the TUI keyed map — see SEAM-011/SEAM-028. `set_widget_carries_pis_three_fields_and_no_widget_blob` (cyrup-modes) is no longer `#[ignore]`d. **This is the second time (after EXT-015/042/043/046) two areas fitted the two ends of one seam without coordination — evidence the pi-derivation discipline is working.** |
| ~~EXT-049~~ | ~~medium~~ **CLOSED 2026-08-14** | upstream-drift | S | ToolCallEventResult.terminate is unrepresentable — **CLOSED 2026-08-14**: sweep 1 — closed, and CROSS-REFERENCED: the agent half landed concurrently as AGENT-022 (`cyrup-agent/src/hooks.rs:53-62`, `BeforeOutcome::Block{reason, terminate}`). Neither area should be reopened for the other's half. |
| ~~EXT-052~~ | ~~medium~~ **CLOSED 2026-08-14** | not-ported | M | An extension-supplied streamSimple provider fires neither before_provider_request nor after_provider_response — **CLOSED 2026-08-14**: sweep 1. |
| ~~EXT-055~~ | ~~medium~~ | cyrup-original | S | **FIXED 2026-08-13** with EXT-054 — `ext-fs` now has manifest-derived roots and mode-aware read/write; the SDK gained the `read_file`/`write_file` wrappers it had never had. The title's `FsCaps::with_fs_root` is a symbol that does not exist; the mutator was `GuestState::with_fs` |
| ~~EXT-016~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | resources_discover carries neither cwd nor reason — **CLOSED 2026-08-14**: sweep 1 — closed properly (payload, not just the accessor), so the "degrades to nice-to-have once EXT-044 lands" note is STRUCK rather than carried. |
| EXT-019 | low — **PARTIALLY CLOSED 2026-08-14** | not-ported | M | registerMarkdownTransformer has no counterpart (re-scoped to v0.84.1) — **PARTIALLY CLOSED 2026-08-14**: sweep 2 — the v0.84.1 shape is ported end to end on the host side: WIT `registration.register-markdown-transformer` (declaration only — the closure stays guest-side) and the guest export `transform-markdown`; `markdown_transformers: Vec<ExtensionId>` in LOAD order, idempotent per owner (upstream ASSIGNS the field, so an extension has at most one fold step); `ExtensionHost::transform_markdown(markdown, message_type, is_streaming, available_width)` folding every owner in load order and containing a faulting/panicking transformer by passing its input through; `InitApi::register_markdown_transformer` + `NativeExtension::transform_markdown`; and the full SDK surface including the `export_extension!` arm. **RESIDUAL, re-filed under area 07 so it is not lost as an area-06 item nobody in 06 can close: the cyrup-tui markdown render path must call `ExtensionHost::transform_markdown`.** |
| EXT-022 | low | not-ported | M | ProviderConfig.refreshModels is not represented (re-scoped to the publish/persist contract) — **2026-08-14, still open**: sweep 2 — **RE-SCOPED from "add a marker + export" to the real v0.84.1 contract, which the item understates.** `RefreshModelsContext` is defined in packages/ai/src/models.ts:46-62 (NOT extensions/types.ts) and carries `credential`, a read-only `stored` snapshot, a generation-checked `publish(ModelsPublication): Promise<boolean>`, `allowNetwork`, `force` and an always-present `signal`; `ModelsPublication` is `{persist?: ModelsStoreEntry \| null, update?: () => void}` at :39-44. A faithful port therefore needs a `models.publish` host import and the persist/generation machinery, not just a `provider-refresh-models` export. **Consciously not started in sweep 2, and that was right** — landing the export alone would be another declared-surface-with-no-reader of exactly the EXT-023 kind. |
| EXT-025 | low | cyrup-original | S | reload() and four emit_* facade methods are dead code that has drifted — **2026-08-14, still open — BLOCKED, NOT UNREACHED, and the block is STRUCTURAL. RE-FILED with widened ownership: it needs ONE agent owning `crates/cyrup-ext` + `crates/cyrup-session-svc` + `crates/cyrup-it`, deleting and repointing in a SINGLE commit.** Sweeps 2 and 6 both confirmed: the preferred fix (make the live session-svc paths call the facade so there is one implementation) is entirely in cyrup-session-svc; the alternative (delete the four dead `emit_*` and `reload()`) reds tests in `crates/cyrup-it`, which are their only callers. **Sweep 6 added the fact that makes splitting it unsafe: `cyrup-it` is `required-features = ["it"]`, so the 16-second workspace gate does NOT build it — the breakage would land silently and be found by whoever next runs the gated suite.** Splitting this across three feature-agents cannot work. |
| EXT-026 | low — **PARTIALLY CLOSED 2026-08-14** | cyrup-original | M | A wasmtime-free cyrup-session-svc build cannot be produced — **PARTIALLY CLOSED 2026-08-14**: sweep 2 — **CONFIDENCE UPGRADED and the item re-scoped.** The item was "medium (static analysis only — no compile was run this pass either)". A compile WAS run and the cyrup-ext half was a HARD BUILD ERROR, not a warning: `LoadError` was imported under `#[cfg(feature = "wasm-host")]` while `discover_with_diagnostics` returns `Vec<LoadError>` in every build, so `cargo check -p cyrup-ext --no-default-features` failed outright — the feature had been broken for some time with nothing detecting it. Fixed; `--no-default-features --all-targets` is clean. **RESIDUAL: the item's actual target, a wasmtime-free cyrup-session-svc build (area 08).** Standing note: the no-default-features build needs a check in whatever gate the orchestrator runs, or it will rot again. |
| EXT-027 | low | not-ported | L | pi's bundled llama.cpp router extension has no counterpart — **2026-08-14, still open**: sweep 2 — not reached; pi's bundled llama.cpp router extension, a whole extension, effort L, no work started. |
| ~~EXT-028~~ | ~~low~~ **CLOSED 2026-08-14** | stale-port | S | Both world.wit copies still declare `cyrup:ext@0.3.0` on line 1 while the package line is 0.4.0 — **CLOSED 2026-08-14**: sweep 1 — `HOST_WORLD` moved to `cyrup:ext@0.5` and the header marker rots no longer. **Sweep 2 note:** the extension to `wit_world_sync.rs` that parses the HEADER comment as well as the package line was NOT added; file it as a small residual if the discipline is wanted. |
| ~~EXT-032~~ | ~~low~~ **CLOSED 2026-08-14** | test-defect | S | p3_no_human_wait_is_still_budget_contained asserts an uncontrollable wall-clock bound — **CLOSED 2026-08-14**: sweep 1. |
| ~~EXT-036~~ | ~~low~~ **CLOSED 2026-08-14** | stale-port | S | The event-catalog provenance comments are stale in four places and contradict each other — **CLOSED 2026-08-14**: sweep 1 rewrote the four event-catalog sites (re-derived range `types.ts:1190-1231` @v0.83.0; the 1:1 claim is now true, 33/33). **Sweep 6 CLOSED THE REST and extended the sweep past `interface ui` to the whole import surface, as this file's own reconciliation asked.** The five WIT sites the `cyrup-ext-sdk/src/ctx.rs` register named are fixed in BOTH copies (`world.wit:467-484`, `:506-508`, `:565-566`, `:576-584`). **FOUR NEW CITATION CLUSTERS, recorded so a seventh pass does not re-derive them:** (1) `modes/rpc/rpc-types.ts`, uniform **+8** (`confirm` `:232` is a blank line → `:240`; `input` `:233-240` is a banner comment → `:241-248`) across both WIT copies, `host/services.rs`, `ctx.rs`, `example.rs`; (2) `host/services.rs`'s fire-and-forget block, uniform **~+6** (notify `:136`, setStatus `:141-142`, setHeader `:184`, setFooter `:174-177`, setTitle `:187`, setEditorText `:210`, setToolsExpanded `:275`; `rpc-mode.ts:149`,`:163`,`:196`); (3) the `pi.*` region — **NOT an offset but the WRONG SURFACE** (getFlag `:1218` is `on("turn_start")`; unregisterProvider `:1361` is an `@example` line; the Models view `:1273-1279` is registerMessageRenderer/registerEntryRenderer; registerMessageRenderer `:1284` is blank; agent_settled `:1225` is tool_execution_end; setThinkingLevel `:1288`; getActiveTools/getCommands `:1257-1266`); (4) the `ToolDefinition` method region, uniform **−17** (execute `:464`, signal `:466`, onUpdate `:467`, renderCall `:472`, renderResult `:481`/`:475-481`, defineTool `:493`). **THREE FABRICATIONS:** `pasteEditorText` names a function pi has at NO version (upstream is `pasteToEditor`, `types.ts:213`) and hid inside a `:200-230` range — `world.wit`'s `paste-editor-text` spelling now carries an explicit `[CYRUP-DELTA]` against `:213`, kept because renaming the import would re-sign it and force a `HOST_WORLD` bump for a comment fix; `ui.custom` cited `:175`, the closing paren of `setWidget`'s component-factory overload (custom is `:196`); and `extensions/types.ts:1043` was used as a CATCH-ALL for three unrelated types (`AgentToolResult` — which is in a **different package**, `packages/agent/src/types.ts:355-369` — `UserBashEventResult` `:1078-1083`, `ToolResultEventResult` `:1085-1090`), `:1043` being `| AgentStartEvent`. **ONE APPARENT FABRICATION REFUTED:** `registerEntryRenderer :1295` / `entryRenderers :1703-1704` look invented at the baseline (`types.ts` is 1701 lines at v0.83.0) but are correct **v0.84.1** numbers carried without a version tag — version lag, not invention; both tags are now written out in `world.wit`, `api.rs`, `registry.rs`, `native.rs`, `guest.rs` and `example.rs`. Every sweep-6 edit is comment-only in both WIT copies (`git diff \| grep '^[-+]' \| grep -v '//'` is EMPTY), so no ABI change and no bump: package line, header marker `// cyrup:ext@0.7.0`, `HOST_WORLD` `cyrup:ext@0.7`, the 34-declared/34-claimed `on-*` export count and the byte-identity of the two copies are all unchanged and re-verified. |
| ~~EXT-037~~ | ~~low~~ **CLOSED 2026-08-15 — residual verified gone** | parity-bug | S | ext-tools.get-commands returns bare extension command names only — **PARTIALLY CLOSED 2026-08-14**: sweep 2 — `HostServices::commands() -> Option<Vec<Value>>` added beside `active_tools`; `ext_tools::get_commands` now prefers the live catalog and falls back to `registry.resolved_commands()`, emitting `{name: invocation_name, description, source: "extension", sourceInfo}` in LOAD order instead of a `HashMap::keys()` walk emitting `{name}` with raw names — a colliding second `deploy` is now reachable by a guest as `deploy:2`. **RESIDUAL (area 08): the `LiveHostServices::commands` impl delegating to `AgentSession::slash_command_catalog`, which is the only source carrying prompt templates and skills.** Without it the fallback is honest but extension-only. **2026-08-15: RESIDUAL CLOSED.** `LiveHostServices::commands` exists (`cyrup-session-svc/src/host_services.rs:1787-1815`), delegating to the attached `SessionCatalog` — i.e. `AgentSession::slash_command_catalog`, the only source carrying prompt templates and skills — with `None` (and thus the registry fallback) only when no catalog is attached. Pinned by `commands_passes_the_live_catalog_through_unchanged`. |
| EXT-040 | low — **PARTIALLY CLOSED 2026-08-14** | parity-bug | S | register-shortcut's description is discarded, so /hotkeys shows a raw key-id — **PARTIALLY CLOSED 2026-08-14**: sweep 1 + 2 — the registry/facade half is done (`shortcut_specs` exists, and EXT-035's new test pins the description round-trip). **RESIDUAL (area 07): `cyrup-tui/main.rs` consuming `shortcut_specs`.** |
| ~~EXT-042~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | model_select buries previousModel/source, and thinking_level_select drops previousLevel — **CLOSED 2026-08-14**: sweep 1 — the extension-boundary halves are area 06's; the session-svc PRODUCERS (session.rs:2691/:2545/:2956/:3337, runtime.rs:480/:526/:568/:706) landed concurrently in area 08 with matching shapes, derived independently from pi. |
| ~~EXT-045~~ | ~~low~~ **CLOSED 2026-08-14** | not-ported | S | ctx.scopedModels and ctx.signal are unreachable, and EXT-005's CYRUP-DELTA notes were never written — **CLOSED 2026-08-14**: sweep 1 — including the part EXT-005's closure promised and never delivered: a CYRUP-DELTA register now exists in `crates/cyrup-ext/src/lib.rs` covering `ctx.signal` (ported as a poll, with the component-model reason) and `sessionManager`/`modelRegistry`. EXT-005 can stop being listed as reopened-in-part. |
| ~~EXT-046~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | session.set-label cannot clear a label — **CLOSED 2026-08-14**: sweep 1 — area 08 widened `LiveHostServices::set_label` to `Option<&str>` in the same pass and the signatures agree. |
| ~~EXT-048~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | The dialog timeout key is `timeoutMs`; pi's field is `timeout`, and the in-tree citations are wrong — **CLOSED 2026-08-14**: sweep 1. |
| ~~EXT-050~~ | ~~low~~ **CLOSED 2026-08-15 — verified already done** | upstream-drift | M | pi.events gained stale-context guarding and tracked unsubscribe — **CLOSED 2026-08-15**: re-verified at HEAD, BOTH halves are live and the row's "residual unchanged" was stale. `bus.unsubscribe` is in the world (`world.wit`, `interface bus`), backed by `GuestState::bus_unsubscribe` → `SharedBus::unsubscribe` (`host/services.rs:1719`); the `assertActive` analog is `GuestState::assert_active` (`:1767`), consulted by BOTH `bus_subscribe` (`:1711`) and `bus_emit` (`:1964`), with a CYRUP-DELTA on the failure MODE (upstream throws; a result-less WIT import cannot, so it drops with a `tracing::warn!`); and `invalidate` (`:1735`) sets the one-shot stale reason AND runs `SharedBus::unsubscribe_all` for the owner, pi's `loader.ts:206-215` @v0.84.1. **The item's own blocker is gone too:** teardown is no longer tied to the production-dead `reload` — `facade.rs::invalidate_live` is called from the live `/new`,`/resume`,`/fork`,`/switch` dispose path in `cyrup-session-svc/src/session.rs` (the `beforeSessionInvalidate` position, pinned by `tests/dispose_invalidates.rs`). |
| EXT-051 | low — **PARTIALLY CLOSED 2026-08-14** | upstream-drift | S | Extension provider OAuth drifted: refreshToken gained a signal, oauth gained isSubscription — **PARTIALLY CLOSED 2026-08-14**: sweep 2 — narrowed and handed off. The signal half needs NO work (the item's own preferred fix — poll `ctx-state.is-run-cancelled` — landed with EXT-045 in sweep 1); the typed field now exists as `ProviderConfig::oauth_is_subscription()` / `ProviderRegistration::oauth_is_subscription()`. **RESIDUAL is the CONSUMER in `crates/cyrup-provider/src/auth/`, beside the existing `AuthProvider::is_subscription` overrides — re-filed under area 01.** |
| EXT-053 | low | parity-bug | S | An extension command shadowing a built-in is dropped from autocomplete with no diagnostic — **2026-08-14, still open**: sweep 2 — not reached (cyrup-tui). |
| ~~EXT-056~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | register_tool_renderer is last-wins while every sibling table is first-wins — **CLOSED 2026-08-14**: sweep 1. |
| ~~EXT-057~~ | ~~low~~ **CLOSED 2026-08-14** | parity-bug | S | deliver_bus_events silently drops queued events at the round bound, and listener faults never surface — **CLOSED 2026-08-14**: sweep 1. |
| ~~EXT-N01~~ | ~~high~~ **CLOSED 2026-08-14** | test-defect | S | The proc-cap buffer test asserted a byte-exact cap the pump never held — scheduling-dependent red — **fixed this pass** — **CLOSED 2026-08-14**: closed pre-sweep — the proc-cap buffer test asserted a byte-exact cap the pump never held. |
| EXT-S04 | low | not-ported | M | ctx.compact's onError path has no observable counterpart — **2026-08-14, still open**: sweep 2 — not reached: the assertActive-on-emit refusal stays unimplemented because `ExtensionHost::reload` is still production-dead (EXT-025). |
| ~~EXT-060~~ | ~~low~~ **CLOSED 2026-08-14 — resolved by DELETION** | cyrup-original | S | `registry.tool_info()` emits a cyrup-invented `source` discriminator on a guest-facing introspection API — **CLOSED 2026-08-14**: sweep 8 verified at HEAD that the discriminator is **gone**. `crates/cyrup-ext/src/registry.rs:560-602` now emits exactly pi's five `ToolInfo` keys — `name`, `description`, `parameters`, `promptGuidelines`, `sourceInfo` — on **both** the native and the guest arm. The item's "decide in one change: `CYRUP-DELTA` or delete" was resolved by **deleting**. Target shape verified independently rather than taken from the code's own comment: pi's `ToolInfo` is `Pick<ToolDefinition, "name"\|"description"\|"parameters"\|"promptGuidelines"> & { sourceInfo: SourceInfo }` — five keys, no `source` — at `core/extensions/types.ts:1552-1554` @v0.83.0. **Citation corrected in passing:** the in-source note at `registry.rs:573-578` already amends an earlier `:1551-1553` to `:1552-1554` on the ground that `:1551` is the doc comment; sweep 8's independent read **agrees with the correction**, so the `:1551-1553` in this row's original text below is the stale one. **Superseded original text follows.** — ~~**FILED 2026-08-14**~~ from sweep 2's mechanism-gap register. pi's `ToolInfo` has NO `source` field: it is `Pick<ToolDefinition, "name"\|"description"\|"parameters"\|"promptGuidelines"> & {sourceInfo}` (extensions/types.ts:1551-1553). cyrup's `tool_info()` adds `source: "extension"\|"guest"`, which is **the WASM-vs-native tier leaking into a parity surface** — a distinction pi's one-extension-kind model has no word for, and one a guest can now read and branch on. EXT-038 added the two genuinely missing pi fields and deliberately left `source` in place rather than removing a key an unknown consumer might read. **Decide in one change: either stamp a `[CYRUP-DELTA]` on it naming types.ts:1551-1553 and the reason, or delete it.** Half of that is not an option — an unlabelled cyrup-original on a guest-facing surface is the exact class EXT-036 and EXT-021 were filed for. |
| ~~EXT-M03~~ | ~~low~~ **FILED (retroactively) AND CLOSED 2026-08-14** | parity-bug | S | `impl Tool for WasmTool` had no `label` override, so a guest's declared label crossed the entire ABI **write-only** — **ROW FILED 2026-08-14 BY THE SWEEP-8 DOC RECONCILIATION, which found `EXT-M03` cited FIVE times in `crates/cyrup-ext/src/host/live.rs` (`:1773`, `:1803`, `:2258`, `:2307`, `:2357`) with NO row in this file.** The work landed in sweep 6; only the row was missing, so the ID was an orphan — a citation pointing at nothing, which `README.md` blind spot 4 names as *more dangerous than no entry at all*. **The defect:** `<WasmTool as Tool>::label` fell through to the trait default `None` (`cyrup-core/src/tool.rs`) although the SDK builder sets the field (`cyrup-ext-sdk/src/descriptor.rs:132`, defaulted to the name at `:113`, set explicitly by all three `tool_factory` presets), `lower_tool_descriptor` lowers it (`cyrup-ext-sdk/src/guest.rs:58`), the WIT declares it as a bare `string` (upstream's `ToolDefinition.label` is REQUIRED, `extensions/types.ts:452-453` @v0.83.0), and `register_tool` lifts it into `ToolDescriptor.label` — **where it stopped.** The asymmetry is the real consequence: a NATIVE tool can express a label because it writes its own `impl Tool`; **a WASM guest could not, whatever it declared.** Same shape as `EXT-007`'s `prompt_guidelines`. **FIXED:** `fn label` added at `host/live.rs:1818`, mapping an EMPTY label to `None` rather than `Some("")` — the trait documents `None` as "fall back to the tool name", which is what upstream's always-populated label degenerates to, and a non-SDK guest sending `""` must not blank the UI row. **PINNED AT THE SOURCE LEVEL, and the reason is itself the finding:** `descriptor_label` is a free function, so a test of it *passes whether or not the impl calls it* — deleting the override again would leave that test green. Constructing a real `WasmTool` needs an `Arc<LiveExtension>`, a compiled component and the `wasm32-wasip2` toolchain, **which is precisely why the omission survived**; so `the_wasm_tool_impl_delegates_every_descriptor_backed_accessor` (`host/live.rs:2323`) scans the `impl Tool for WasmTool` block itself, the way `tests/wit_world_sync.rs` pins the two `world.wit` copies, with a non-vacuity check on the slice. `render_call`/`render_result` are deliberately NOT listed — a guest renderer is routed by `has_renderer` through the registry's tool-renderer table, not off this impl. **This is instance 2 of the dropped-delegation class**, between `TOOL-024` (`RegisteredTool`) and `PROV-M01` (`Provider` — the first on a non-`Tool` trait, and the first with a live user-visible consequence). |
| ~~EXT-M02~~ | ~~low~~ **FILED AND CLOSED 2026-08-14** | cyrup-original | S | `EpochDriver::spawn`'s `tokio::select!` was unbiased, so a driver spawned with an already-cancelled token exited on a coin flip — **FILED AND CLOSED 2026-08-14**: sweep 6, found by hunting. **Unbiased-select class.** `crates/cyrup-ext/src/host/epoch.rs:28` raced `token.cancelled()` against `iv.tick()` with no `biased;`. A freshly built `tokio::interval` fires its FIRST tick immediately, so on the very first loop iteration both arms are ready whenever the token was already cancelled at spawn — and an unbiased `select!` picks at RANDOM, which is precisely what a JS race cannot do. The driver then incremented the epoch of an engine that is shutting down and looped, exiting only on a later coin flip (expected ~2 iterations, so a flake rather than a hang). Fixed with `biased;`. **AUDIT RESULT, recorded so the next pass does not redo it: this was the ONLY unbiased `select!` in cyrup-ext / cyrup-ext-sdk / cyrup-sdk.** The other eleven (`native.rs:748`, `dispatch.rs:474`, `facade.rs:469`, `host/instance.rs:132`, `host/live.rs` ×5, …) already carry `biased;`, and **`caps/proc.rs:297`'s unbiased one is CORRECT** — its loop re-evaluates the authoritative condition at the top of every iteration and both arms are no-ops. **Do not "fix" it.** No upstream counterpart: pi has no epoch driver (arch-08 §5.3, R-ARCH-EXT-012). |
| ~~EXT-M01~~ | ~~medium~~ **FILED AND CLOSED 2026-08-14** | parity-bug | S | `LiveExtension::execute_tool` leaked the bound tool `CancelToken` whenever its future was dropped mid-await, so `host-tool.is-cancelled` answered `true` for every later poll from every guest handler — **FILED AND CLOSED 2026-08-14**: sweep 6, found by HUNTING, not from the backlog. **JS→Rust guarantee gap.** Upstream `signal` is a PARAMETER of `ToolDefinition.execute` (`extensions/types.ts:480-486`, `:483` @v0.83.0), so it is call-scoped by the language and a started `async` function always settles. cyrup cannot send a `CancelToken` through the Component Model, so `execute_tool` binds it on `GuestState` for the call's duration and the guest polls it via the `host-tool.is-cancelled` import — turning a **call-scoped parameter into INSTANCE-scoped mutable state**. The clear was hand-written on both arms of the `tokio::select!`, covering completion and token-cancellation but NOT the third exit a Rust future has: the whole `execute_tool` future being dropped at its await point by an outer `select!`, a `tokio::time::timeout`, or an aborted `JoinHandle`. On that path the cancelled token stayed bound forever, and **`is-cancelled` is NOT gated to tool calls** (`world.wit:773`, impl `host/live.rs:852`) — a guest may poll it from any handler — so a guest that checks it to decide whether to keep working would silently stop working until the next `execute_tool` happened to rebind. Fixed with a `ToolCancelBinding` RAII guard whose `Drop` clears the binding, declared AFTER the instance mutex guard so it drops BEFORE it (the token is cleared while the lock is still held, so no other call can observe the gap). The four sibling entry points were audited (`execute_command`, the with-session callback, `bus_deliver`, `execute_shortcut`) — none installs per-call state, and `set_tier` is re-set unconditionally at the top of every entry point, so a tier leaked by a dropped future is corrected by the next call and is NOT a second instance. Tests: `crates/cyrup-ext/src/tests/wasm_host.rs::a_bound_cancelled_token_is_what_the_guest_poll_reads` (presence, so the two absence assertions cannot pass vacuously), `::the_binding_is_cleared_when_the_guard_leaves_scope`, `::the_binding_is_cleared_when_the_call_future_is_dropped_mid_await` (the regression; carries an `AtomicBool` proving the future was polled past the binding before being dropped). **STANDING RULE this generalises to — see `## Coverage`:** every host-side per-call state bound before an await and cleared on the `select!` arms is the same bug, and the WIT seam FORCES that pattern every time a pi parameter cannot cross the Component Model boundary. |
| ~~EXT-061~~ | ~~medium~~ **CLOSED 2026-08-15** | not-ported | S | `ctx.getSystemPromptOptions()` had no counterpart — **CLOSED 2026-08-15**, ported end to end and NOT as a declared-surface-with-no-reader: WIT `ctx-state.get-system-prompt-options: func() -> result<string, string>` in both copies; `HostServices::system_prompt_options() -> Option<Value>` (`host/services.rs`); the host import (`host/live.rs`) which `require_command_tier()`s FIRST — pi puts `getSystemPrompt()` on the base ctx (`types.ts:346`) and this on `ExtensionCommandContext` (`:355`), so the tier gate is upstream's placement, not a cyrup restriction; the NATIVE half `HostCtx::system_prompt_options()` reading the same backend accessor (`native.rs`, so cyrup's two tiers cannot disagree — EXT-044's lesson); the SDK `CommandCtx::system_prompt_options()`; and the LIVE backend `LiveHostServices::system_prompt_options` off `PromptRebuilder`, the same structure the next prompt rebuild consumes, so the bag cannot drift from the string. **No-backend answer is pi's OWN default `{cwd}`** (`core/extensions/runner.ts:287`, re-bound `:350` @v0.83.0), never `{}` and never an error. **NO `HOST_WORLD` bump, and the reasoning is recorded in `manifest.rs` because it was worked out backwards first:** `check_world` passes when the GUEST minor is `>=` the host's, so the gate defends only old-guest-on-new-host; an added import cannot fail that direction, and the direction it could fail is accepted by the gate at any numbers. Tests: 3 in `src/tests/native_ctx_state.rs` + 1 in `cyrup-session-svc/src/host_services.rs`, all labelled COVERAGE not proof (new API — cannot go red pre-fix). |
| ~~EXT-062~~ | ~~low~~ **CLOSED 2026-08-15** | cyrup-original | S | `registration.add-autocomplete` carried no pi citation — **CLOSED 2026-08-15**: found HALF DONE at HEAD (the `[CYRUP-DELTA]` was already in both `world.wit` copies and `registry.rs`/`api.rs` from the 0.8 batch, which this file never recorded); this pass added the two missing sites, `native.rs::InitApi::add_autocomplete` and `host/live.rs::add_autocomplete`. `types.ts:1166` re-derived at v0.83.0 = `getArgumentCompletions?` on `RegisteredCommand` ✓. |
| ~~EXT-063~~ | ~~low~~ **CLOSED 2026-08-15** | cyrup-original | S | `ManifestResources.agents` + the `cyrup` package.json key — **CLOSED 2026-08-15**: the disclosure the item asked for is written on `ManifestResources` (`cyrup-resources/src/package/manifest.rs`), naming `loader.ts:561-566` @v0.83.0 as the four ported keys, `agents` as the cyrup-original fifth, and `pi`-wins-on-collision as an INVARIANT ("do not reorder the `.or()`") rather than an artefact of `parsed.pi.or(parsed.cyrup)` at `:108`. Two tests added beside it, labelled COVERAGE (the behaviour was already correct; they go red on the future `.or()` reorder). Comment-and-test only, so area 05's crate is untouched behaviourally. |
| EXT-064 | medium — **PARTIALLY CLOSED 2026-08-15** | cyrup-original | M | `ui.set-header`/`set-footer` take a `string` where pi takes a component factory — **PARTIALLY CLOSED 2026-08-15 (verified at HEAD, the row was stale)**: the `[CYRUP-DELTA]` the item asked for IS at the signature in both `world.wit` copies, naming BOTH collapses (factory→string, and `undefined`→`""` for "restore the built-in") against `types.ts:183-187`/`:190` @v0.83.0. **RESIDUAL, unchanged and stated in that note: `ReadonlyFooterDataProvider` (`types.ts:179-181`) still has no analog**, so an extension footer cannot render the git branch or other extensions' `setStatus` segments. Needs a data channel (a `ui.footer-data` import or extra `set-footer` parameters) and should batch with EXT-006's render-path widening — an import addition, which per EXT-061's finding needs no `HOST_WORLD` bump. |
| ~~EXT-065~~ | ~~medium~~ **CLOSED 2026-08-15 — verified already done** | cyrup-original | S | `add-autocomplete-provider` was ungated by the manifest `ui` grant — **CLOSED 2026-08-15**: verified at HEAD, landed with the 0.7→0.8 world bump and this file never recorded it. The import is on `interface ui` in both copies (`world.wit`), and the impl goes through `ui_guest_of` (`host/live.rs:460-469`) — the gated helper — with an explicit "do not relax it back to `guest_of`" note. The capability consequence EXT-054 built the control for is therefore closed, not merely documented. |
| ~~EXT-066~~ | ~~medium~~ **CLOSED 2026-08-15 — verified LIVE at both ends** | parity-bug | S | The live theme's colours were unreadable — **CLOSED 2026-08-15**: `ui.theme-get-json` is in both `world.wit` copies and `host/live.rs::theme_get_json` composes it from `theme()` + `theme_by_name()` (one round trip, so a theme switch cannot race two reads). **Checked specifically for this area's characteristic defect — a capability declared in the world against a dead backend — and it is NOT one at HEAD:** `LiveHostServices` now overrides all four theme accessors off `ThemeAccess` (`cyrup-session-svc/src/host_services.rs:1139-1156`, `cyrup-tui/src/theme_access.rs`), landed as SEAM-T01, which is exactly the hole this item's earlier state had. Unattached, the four keep pi's `noOpUIContext` values (`runner.ts:261-263` @v0.83.0), which is the correct headless answer, not a dead one. |
| ~~EXT-067~~ | ~~medium~~ **CLOSED 2026-08-15 — verified already done** | parity-bug | S | `session_before_compact`/`session_before_tree` drop pi's `signal` — **CLOSED 2026-08-15**: verified at HEAD. Both signatures now carry a `[CYRUP-DELTA] EXT-067` naming `signal: AbortSignal` at `types.ts:601` / `:642` @v0.83.0 and stating that it is ported as the `ctx-state.is-run-cancelled` poll rather than dropped — which is the substitution the item said was never named. Payload citations re-derived this pass: `SessionBeforeCompactEvent` `:592-602` ✓, `SessionBeforeTreeEvent` `:639-643` ✓ (their SIBLING comments cited `:577-587`/`:623-628`, which EXT-072 fixed in the same edit). |
| ~~EXT-068~~ | ~~medium~~ **CLOSED 2026-08-15 — verified already done** | parity-bug | S | `on-session-tree`'s opaque blob — **CLOSED 2026-08-15**: verified at HEAD. The parameter was renamed `tree-json` → `event-json` (part of the 0.7→0.8 re-signing) and the export now carries the four-field `SessionTreeEvent` documentation with `extensions/types.ts:646-652` @v0.83.0 re-derived ✓, naming `fromExtension` as the re-entrancy guard and the session-svc serialization site. |
| ~~EXT-069~~ | ~~low~~ **CLOSED 2026-08-15 — verified already done** | cyrup-original | S | `on-tool-exec-*` abbreviations — **CLOSED 2026-08-15**: verified at HEAD. All three are `on-tool-execution-{start,update,end}`, the rename re-signed the world 0.7→0.8, and the WIT carries a do-not-re-abbreviate note ("every such exception is a place a real gap can hide"). Overload lines re-derived: `:1223`/`:1224`/`:1225` @v0.83.0 ✓. |
| ~~EXT-070~~ | ~~low~~ **CLOSED 2026-08-15** | stale-port | S | The extension-manifest surface split across two crates — **CLOSED 2026-08-15**: found HALF DONE — `cyrup-ext/src/manifest.rs` already carried the rewritten note (disjoint key sets, `loader.ts:561-566` declaration + `:572-573` field read, and `:596` named as the path join it actually is; all three re-derived at v0.83.0 ✓), but the RECIPROCAL pointer the item also asked for was missing. Added on `ManifestResources` (`cyrup-resources/src/package/manifest.rs`), in the same block as EXT-063's disclosure. |
| ~~EXT-071~~ | ~~low~~ **FILED AND CLOSED 2026-08-14** | stale-port | S | `interface ext-tools`' shape comments advertised a field the code no longer emits and under-documented another — **FIXED 2026-08-14 (ext-rpc surface enumeration)**: `world.wit:828` documented `ToolInfo {name, source, parameters, …}` where `source` is exactly the cyrup-invented discriminator **EXT-060 removed** from the emitted object — the `CYRUP_SHARE_VIEWER_URL` failure mode (an advertised surface that does not exist); `:830` documented `SlashCommandInfo {name, description}` where pi's type is `{name, description?, source, sourceInfo}` and cyrup's own EXT-037 implementation already emits all four. Both comments rewritten in **both** `world.wit` copies against `types.ts:1552-1554` and `core/slash-commands.ts:6-11` @v0.83.0, with a do-not-restore note. Comment-only; no ABI change; copies re-verified byte-identical. |
| ~~EXT-072~~ | ~~low~~ **CLOSED 2026-08-15** | stale-port | M | ~50 stale pi citations in five clusters — **CLOSED 2026-08-15**. Every "actual" line was RE-DERIVED against `v0.83.0` this pass rather than carried from the row (rule 7), by resolving each citation against the tag's `types.ts` mechanically; the row's values all held. Rewritten in **both** `world.wit` copies plus `cyrup-ext-sdk/src/{ctx,api}.rs`, `cyrup-ext/src/{event,native,registry}.rs` and `host/{services,live}.rs`: cluster A (`addAutocompleteProvider` `:218`→`:225`, incl. two sites in `registry.rs` the row did not list), cluster B (`isIdle` `:333`→`:330`, `isProjectTrusted` `:335`→`:332`, `hasPendingMessages` `:341`→`:338`, and the base-ctx `abort`/`shutdown`/`compact` range), cluster C (`ExtensionCommandContext` `:353-387`, `ReplacedSessionContext` `:394-404`, `newSession` `:361-365`, `fork` `:368-371`, `switchSession` `:380-383`, opts bag `:361-383`), cluster E (all nine payload interfaces), all seven singletons (incl. `createAssistantMessageEventStream` → `packages/ai/src/utils/event-stream.ts:86`, `CompactionPreparation` → `core/compaction/compaction.ts:692`, `TreePreparation` → `extensions/types.ts:624-636`, `setThinkingLevel` wiring → `runner.ts:336`, `sendMessage` → `:1286`) and all three self-undermining corrective notes. **AND THE GUARD THE ITEM ASKED FOR, offline so it cannot be skipped:** `no_struck_pi_citation_is_restored_as_a_live_citation` (`src/tests/wit_world_sync.rs`) — 17 struck values × 9 files, each allowed ONLY on a line that also names the item that struck it. **Measured RED before: 34 live sites.** **RESIDUAL, stated rather than implied:** the lint pins struck VALUES, not every citation, and `cyrup-ext-sdk/src/{example,guest,descriptor}.rs` were not audited this pass. |
| ~~EXT-073~~ | ~~medium~~ **CLOSED 2026-08-15** | stale-port | S | Two fabrications in the WIT — **CLOSED 2026-08-15 on the residual.** All nine `:1135-1161` citations rewritten to the re-derived overload lines (`tool_call :1228`, `tool_result :1229`, `context :1207`, `message_end :1222`, `before_agent_start :1214`, `input :1231`, `user_bash :1230`, `before_provider_request :1209`, `after_provider_response :1213`), and both header occurrences of `session_info_changed … :1203` → `:1193`, in both copies. **A TENTH instance the item did not have, found by the guard:** `agent_settled … subscribed at :1225` — `:1225` is `tool_execution_end`, eight events away — live in FOUR files (both `world.wit` copies, `cyrup-ext-sdk/src/api.rs:51`, `cyrup-ext/src/event.rs:50`), and `session_info_changed … :1203` was in those same two `.rs` files too. **GUARD:** `every_subscribed_at_citation_names_the_event_pi_subscribes_on_that_line` pins the full 33-entry overload map and checks the cited line against the event the comment is about — a range check would have missed all eight of these, since every one is INSIDE `:1190-1231`. **Measured RED before: 8 sites.** |
| ~~EXT-074~~ | ~~medium~~ **FILED AND CLOSED 2026-08-15** | stale-port | S | `models.set-model`/`set-thinking-level` advertised a COMMAND-only tier gate the host stopped enforcing — **FILED AND CLOSED 2026-08-15**, found while porting EXT-061's tier gate, not from the backlog. `world.wit` said `set-model: … // COMMAND-only at the host` and, for `set-thinking-level`, "an event-tier call is REJECTED with an observable error (like every `control.*` op) — never a silent no-op". Neither is true at HEAD: GAP-11 removed both gates (`host/live.rs:561-562` and `:580-581` take the UNGATED `guest_of`, each with an in-source note explaining why the deferral through the `control` mpsc dissolves the R-08-008 deadlock), and the comments never moved. `cyrup-ext-sdk/src/ctx.rs:68` carried the same claim in the file a guest author reads. **This is the MIRROR of this area's characteristic defect** — EXT-066 was a capability declared in the world with a dead backend; this is a RESTRICTION declared in the world that the backend does not apply — and it is the more dangerous direction on a parity surface, because a guest author who believes it writes their own tier plumbing to work around a gate that is not there. pi is ungated too (`setModel` `core/extensions/loader.ts:359-362`, `setThinkingLevel` `:369-372` @v0.83.0, both bound with only `assertActive`), so the CODE was the parity-correct half and the comments were the stale one — fixed in that direction at all three sites. |

## EXT-054 — `ExtensionManifest.capabilities` is never read by any code path — the declared per-extension WASM sandbox grant model is entirely inert

**Kind** cyrup-original · **Severity** critical · **Effort** M · **Confidence** **confirmed — the mis-grant reproduced end to end with a real WASM guest** · **observed 2026-08-13** (live-terminal; [`REPRO-LOG.md`](REPRO-LOG.md))

> **FIXED 2026-08-13.** `load_discovered` (`facade.rs:1223`) now calls
> `self.load_wasm_with_caps(id.clone(), &bytes, services, &disc.manifest.capabilities)` — the new
> capped entry point (`facade.rs:1094-1116`) — which parses the `fs` grants and seeds the guest via
> `GuestState::with_capabilities` (`host/services.rs:1288-1305`) BEFORE `init` runs. `load_wasm`
> keeps its old three-argument signature and delegates with `Capabilities::host_granted()`
> (`manifest.rs:63-72`): it is the manifest-LESS host-internal entry, so it is not narrowed by a
> manifest it was never given, and none of the ~30 existing `load_wasm` call sites changed.
> Enforcement is host-side at the import boundary — `exec_guest_of` / `net_guest_of` /
> `ui_guest_of` (`host/live.rs:53-88`) gate `exec.run`, all six `proc.*`, all four
> `http-client.*` and all 22 `ui.*` imports; `ext-fs` is gated by `FsCaps` having no root
> (EXT-055). Grants cross as DATA and the guest gets no import that reads or changes them, per
> ADR-0002's batch-17 instruction. A malformed grant now FAILS the load (`ExtError::Capability`)
> rather than being dropped. Deny-by-default held: both `loader.rs` synthesis sites are now the
> explicit `Capabilities::none()` (`:213`, `:264`).
>
> **Evidence** — new `crates/cyrup-ext/tests/manifest_capabilities.rs`, 9 tests, driven through the
> real `discover_and_load` path with the real `wasm32-wasip2` SDK component and asserted at the
> `HostServices` BOUNDARY (a denied guest produces zero `exec_calls`/`http_requests`, not merely an
> error string it printed itself). RED before / GREEN after was measured by reverting the single
> `load_discovered` line to `self.load_wasm(id.clone(), &bytes, services)`:
> `test result: FAILED. 4 passed; 5 failed` → `test result: ok. 9 passed; 0 failed`. The five that
> flip are `exec_is_refused_when_the_manifest_denies_it`,
> `net_is_refused_when_the_manifest_denies_it`, `ui_effects_are_dropped_when_the_manifest_denies_ui`,
> `fs_grants_are_scoped_by_mode_and_by_subtree` and `a_malformed_fs_grant_fails_the_load`.
>
> Whole-crate gate after the change, pasted verbatim from the run:
> `cargo test -p cyrup-ext -p cyrup-ext-sdk --all-features` → **33 targets, 271 passed, 0 failed,
> 1 ignored, exit 0**.
>
> **Wording corrected on this item** (as the assignment instructed): `FsCaps::with_fs_root` does not
> exist and never did — `rg with_fs_root crates` returns zero. The mutator was `GuestState::with_fs`
> (`host/services.rs:1263`), which had zero callers. Both the Fix text below and EXT-055 said
> `with_fs_root`; batch 3's lint spec inherits the same wrong name and should be corrected there too.

> **Reproduced 2026-08-13, and the result is stronger than this item claims.** `wasm32-wasip2` was
> already installed; `cargo build -p cyrup-ext-sdk --target wasm32-wasip2` finished in **0.10 s** and
> produced a 4.1 MB component. A guest whose `extension.json` declared
> `{"fs": [], "exec": false, "net": false, "ui": false}` — every bit off, the strictest declaration
> the schema can express — was loaded via `-e <dir>` and got the **full host surface**:
>
> * `/execdemo` ran `echo hi` as a real host process and printed `exec stdout: hi`.
> * `/httpdemo https://api.together.xyz/v1/models` opened a real TLS connection and returned a live
>   `401 Missing API key` from Together's server.
> * Both results were surfaced through `ctx.ui().notify()`, so the `ui` bit is inert too.
>
> The consumer-side grep was re-run at HEAD and is unchanged: producers only, no consumer.
>
> **Two corrections to the Impact below.** (1) *"Blast radius today: zero WASM guests ship … so
> nothing is currently mis-granted"* **overstates the safety margin**. The SDK's own reference guest
> (`crates/cyrup-ext-sdk/src/example.rs`) is a complete, loadable component that ships `/execdemo`
> and `/httpdemo` as ready-made proofs, the target builds it in under a second, and `-e <dir>` loads
> it pre-trusted — the mis-grant is reproducible **today, with no third-party code**. Read that
> sentence as "no third-party guest ships, but the in-tree SDK example is a loadable guest that
> demonstrates the mis-grant end to end." (2) **`--offline` does not gate the guest `http-client`
> import either** — the `net` bypass above was measured with `--offline` set on the host, so neither
> the manifest grant nor the offline flag stands between an installed guest and the network. Filed
> separately as **EXT-058**; the Verify block should add the offline case.

> **Raised `high` → `critical` in the 2026-08-12 repair pass.** README:106-107 defines `critical` as
> "data loss, silent wrong output, **a permission bypass**, or a crash on a normal path" and attaches
> no reachability qualifier. The signature evidence was re-verified at HEAD before raising:
> `load_discovered` (`facade.rs:1166-1184`) has the manifest in hand and calls
> `self.load_wasm(id.clone(), &bytes, services)`; `load_wasm`'s signature (`facade.rs:1063-1070`) is
> `(id, bytes, services)` — the manifest provably cannot reach instantiation. The zero-shipping-guest
> argument that previously held this at high is real and is retained below as **blast radius**, but
> blast radius today is not the class of the defect: the grant model is the control a reviewer reads
> to decide an extension is safe to install, and it is inert.

**cyrup** — `grep -rn capabilities crates/cyrup-ext/src --include='*.rs'` at HEAD returns only *producers* and prose: the struct field (`crates/cyrup-ext/src/manifest.rs:20`), the `Capabilities { fs: Vec<String>, exec: bool, net: bool, ui: bool }` definition (`:23-35`), two `capabilities: Default::default()` synthesis sites in the loader (`crates/cyrup-ext/src/loader.rs:213`, `:259`), and doc comments. **There is no consumer anywhere.** `ExtensionHost::load_discovered` (`crates/cyrup-ext/src/facade.rs:1166-1182`) reads `disc.manifest.check_world(HOST_WORLD)?` and `disc.is_trusted(project_trusted)` and then calls `self.load_wasm(id.clone(), &bytes, services)`; `load_wasm` (`facade.rs:1063-1103`) takes only `(id, bytes, services)` and never sees the manifest, so the capability declaration is dropped on the floor at the one place it could be applied. `capabilities.{exec,net,ui}` therefore narrow nothing: a guest whose `extension.json` declares `{"exec": false, "net": false, "ui": false}` still reaches `exec.run`, `http-client`, `proc` and every `ui.*` import, gated solely by the coarse load-time trust check `origin.is_pre_trust() || project_trusted` (`crates/cyrup-ext/src/loader.rs:56-59`). This contradicts the crate's own documented control in three places: `manifest.rs:2` ("Declares the capabilities a guest requests (granted subject to trust, arch-07/12)"), `crates/cyrup-ext/src/host/store_state.rs:1-3` and `:20-22` ("capability-scoped — NO ambient fs/net unless granted, R-ARCH-EXT-011" / "only explicitly preopened dirs (granted via the manifest, arch-12) are visible"). **The third citation, ADR-0002, is withdrawn 2026-08-13**: that ADR is about encoding (values, not references) and makes no claim about capability scoping — its own Consequences section says so and asks for this re-pointing (`ADR-0002-extension-io-is-serde.md:252-255`, new-work item 3). The `store_state.rs:20-22` claim was ALSO wrong on its own terms and has been corrected in the source: there are no WASI preopens at all, and `capabilities.fs` feeds `FsCaps`/`ext-fs`, not preopens.

**upstream** — none, and that is the point: pi has no capability model at all (every TypeScript extension runs with the whole process's authority, `pi/packages/coding-agent/src/core/extensions/loader.ts` @v0.83.0). This is a divergence from **cyrup's own** security design rather than a pi parity gap, which is precisely why a pi-anchored, item-driven sweep cannot see it (README structural blind spot 1).

**Impact** — the sandbox cyrup advertises does not exist. Every loaded WASM guest gets the full host surface regardless of what it declared, and the declaration is the thing a reviewer would read to decide whether an extension is safe to install. The trust gate that *is* enforced is all-or-nothing and directory-scoped, so "trusted enough to run" silently means "trusted with process execution, network and the filesystem". **Blast radius today (corrected 2026-08-13 from a live run):** no *third-party* guest ships, **but the in-tree SDK example is itself a loadable guest that demonstrates the mis-grant end to end** — `crates/cyrup-ext-sdk/src/example.rs` ships `/execdemo` and `/httpdemo`, `cargo build -p cyrup-ext-sdk --target wasm32-wasip2` produces the component in 0.10 s, and `-e <dir>` loads it pre-trusted. Measured: an all-false manifest still reached a real host process and a real TLS round trip. `wasm-host` is default-on and `load_discovered` is a live path, so the first installed guest is mis-granted on arrival and the mis-grant is invisible because the manifest says otherwise. Schedule it **before** the first third-party component, not after. **`--offline` is not a second line of defence** — the `net` bypass reproduces with `--offline` set on the host (**EXT-058**).

**Fix** — thread the manifest into instantiation: change `load_wasm` (`facade.rs:1063-1103`) to take `&ExtensionManifest` (or a resolved `Capabilities`) and have `load_discovered` (`facade.rs:1166-1182`) pass `disc.manifest`. In `GuestState` construction (`crates/cyrup-ext/src/host/services.rs:1181` region) seed `ProcCaps`/`HttpCaps`/`FsCaps` from the grant rather than from `Default`, and make the `exec`/`net`/`ui` host imports in `crates/cyrup-ext/src/host/live.rs` return a typed denial when the corresponding bit is false. The `fs` grant strings (`manifest.rs:26-28`, e.g. `"read:."`, `"write:.cyrup/todo"`) need a parser and must feed the `FsCaps` mutator — **`GuestState::with_fs` (`services.rs:1263`), NOT `FsCaps::with_fs_root`, which does not exist** — which is EXT-055 and should be done in the same change. Deny-by-default: an absent `capabilities` block grants nothing, and the loader's two `Default::default()` synthesis sites (`loader.rs:213`, `:259`) must therefore stay the empty grant, not a permissive one.

**Verify** — load two fixture components in one process, one declaring `{"exec": true}` and one declaring `{"exec": false}`; assert the first's `exec.run` succeeds and the second's returns a capability-denied error rather than running. Repeat for `net` against `http-client` and for a `fs` grant that permits one directory and refuses its sibling. **Added 2026-08-13:** include the `--offline` case — a guest declaring `{"net": false}` on a host launched `--offline` must not reach the network, which today it does (**EXT-058**). The regression fixture already exists: build `cyrup-ext-sdk` for `wasm32-wasip2` and load it with `-e`. Add to `crates/cyrup-ext/tests/wasm_component.rs` and a new `tests/manifest_capabilities.rs`.

## EXT-003 — Project-trust store is unwired at the extension seam

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — the vote is live (`pre_trust_extension_verdict`, `crates/cyrup-session-svc/src/builder.rs:1640-1671`) and its verdict is fed to `decide_trust_with_extension`, but the store is absent: `builder.rs:516` still passes `saved: None` literally, and the `remember` branch at `:501-511` is still a bare `tracing::warn!` whose own text admits "no trust store is wired into the session builder". Second half unchanged: `pre_trust_extension_verdict` opens with `let host = ExtensionHost::with_wasm(host_config).ok()?;` at `:1648`, *before* the native vote loop at `:1652`, so a wasm-runtime construction failure returns `None` and discards every native vote.

**upstream** — `pi/packages/coding-agent/src/core/project-trust.ts:63-65` persists (`options.trustStore.set(options.cwd, trusted)` guarded by `result.remember === true`) and `:71-74` reads back (`const decision = options.trustStore.get(options.cwd); if (decision !== null) return decision;`) @v0.83.0.

**Impact** — a directory the user already trusted re-prompts on every launch; "remember" is accepted and discarded. Fail-closed (a re-prompt, not a bypass), which is why this is medium. The `with_wasm` ordering compounds it: on a machine where the wasm runtime cannot be constructed, the native trust deciders silently do not vote.

**Fix** — persist `remember` at `builder.rs:501-511` and read the store back, passing the hit as `saved:` at `:516`. Change `builder.rs:1648` to fall back to `ExtensionHost::new(host_config)` when `with_wasm` fails. Pair with EXT-043, which gives the deciding extension the cwd it is voting on — without that, "remember" has no well-defined key from the extension's point of view.

**Verify** — trust a directory with remember set, restart, assert no prompt and that `decide_trust_with_extension` receives `saved: Some(true)`. Force `with_wasm` to fail and assert native votes still aggregate.

## EXT-006 — Renderers run without display options or theme, and only once

**Kind** parity-bug · **Severity** medium · **Effort** L · **Confidence** high

**cyrup** — the replay half for custom messages is now closed (`replay_session_with_extensions`, `crates/cyrup-tui/src/app.rs:1305-1327`, two production callers). Two residuals remain. (1) **Signature**: `render-call: func(custom-type: string, call-json: string) -> option<string>` (`crates/cyrup-ext/wit/world.wit:169`) carries neither display options nor a theme, and `render-result` matches. (2) **One-shot**: the render is computed once at ingest — `let rendered = extension_render(ext_host, ev).await;` at `app.rs:4368`, fed to `ingest_event_rendered` at `:4380` — so the text is baked and an expand/collapse or a theme change never re-invokes the renderer. The third residual, replayed **tool** rows, is filed separately as EXT-041.

**upstream** — `MessageRenderer = (message, options, theme) => Component | undefined` at `pi/packages/coding-agent/src/core/extensions/types.ts:1146`, with `EntryRenderOptions {expanded}` at `:1142` @v0.83.0; pi re-invokes the renderer from the draw path, so options and theme are live inputs rather than ingest-time constants.

**Impact** — an extension renderer cannot respond to expand/collapse or to the active theme. Toggling either leaves the extension's output frozen in the state it was first drawn in, while every built-in row around it updates.

**Fix** — widen `render-call`/`render-result` in **both** `world.wit` copies to carry display options + theme (an export re-signing ⇒ bump `HOST_WORLD`, batch with the C5 ABI move); move the render out of ingest into the draw path, or cache it keyed by `(entry_id, expanded, theme)` and invalidate on either.

**Verify** — toggle expansion and then the theme on a rendered entry and assert the renderer is re-invoked with the new options each time.

## EXT-007 — The first system prompt is built from built-ins only; guest promptGuidelines are dropped

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `crates/cyrup-session-svc/src/builder.rs:1045-1048` derives `selected_tools`/`tool_contributions` from `base_tools`; the prompt is built at `:1062` (`SystemPromptBuilder::new().build(&prompt_inputs)`); `ext_host.active_tools(&base_tools)` still runs six lines later at `:1068`. `builder.rs:1054` passes `prompt_guidelines: Vec::new()` outright. Root cause for the guidelines half is unchanged: `fn prompt_guidelines(&self) -> &[&str]` at `crates/cyrup-core/src/tool.rs:120` cannot be implemented against a descriptor owning `Vec<String>`, and `impl Tool for WasmTool` (`crates/cyrup-ext/src/host/live.rs:1386-1420`) overrides name/parameters/execution_mode/description/`prompt_snippet` (`:1406-1408`)/execute and nothing else, so the empty trait default stands.

**upstream** — pi builds `_toolPromptGuidelines` from the **merged** definition registry (built-ins + MCP + extension tools) inside `_refreshToolRegistry`, `pi/packages/coding-agent/src/core/agent-session.ts:2497-2504` @v0.83.0.

**Impact** — extension tools are callable but undescribed in the system prompt of the session's first turn, and a guest's `promptGuidelines` never reach the model at all. Compounds EXT-031, which loses the same content for the rest of any run a tool joins mid-flight.

**Fix** — move `ext_host.active_tools(&base_tools)` above `builder.rs:1045` and derive `selected_tools`/`tool_contributions` from the merged set. Widen `Tool::prompt_guidelines` (`cyrup-core/src/tool.rs:120`) to an owned or `Cow` slice and implement the override on `WasmTool` (`host/live.rs:1386-1420`). EXT-038's `promptGuidelines` half is blocked on the same signature change — do both together.

**Verify** — an init-registered guest tool with a snippet and guidelines: assert both appear in the first assembled system prompt.

## EXT-009 — before_provider_headers event is missing entirely

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `crates/cyrup-ext/src/event.rs:13-53` lists 31 `EventKind` variants ending at `AgentSettled = 30`, with `COUNT: u8 = 31` at `:59`; the provider pair is `BeforeProviderRequest`/`AfterProviderResponse` and nothing else. `grep -rn 'before_provider_headers\|before-provider-headers' crates/` returns only the two `world.wit` header lines that now *admit* the omission (`crates/cyrup-ext/wit/world.wit:9-10`, both copies).

**upstream** — `BeforeProviderHeadersEvent` at `pi/packages/coding-agent/src/core/extensions/types.ts:686-689` @v0.83.0, subscribed at `:1212`, reduced by `emitBeforeProviderHeaders` at `runner.ts:1045`; the doc at `:681-685` specifies that a `null` value **deletes** that header.

**Impact** — extensions cannot add, override or delete provider request headers. Any auth-shim, proxy-tagging or telemetry extension that works under pi is impossible under cyrup. Compounds EXT-052: neither the header hook nor the payload hook covers extension-supplied providers.

**Fix** — add `EventKind::BeforeProviderHeaders = 31` (bump `COUNT` at `event.rs:59`, add the `from_u8`/`name()` arms), a guest export in `interface events` in both `world.wit` copies with `option<string>` values so `none` means delete, an `EventPatch::Headers` in `contract.rs`, SDK hooks, and a dispatch point in provider request assembly (area 01 code). Export addition ⇒ bump `HOST_WORLD`.

**Verify** — a guest that adds one header and nulls another; assert the outbound request carries the addition and lacks the deletion.

## EXT-011 — session_info_changed is emitted internally but never crosses the extension boundary

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — there is no `SessionInfoChanged` among the 31 `EventKind` variants (`crates/cyrup-ext/src/event.rs:13-53`), and the WIT session block (`crates/cyrup-ext/wit/world.wit:234-237`) has only `on-session-start`/`on-session-shutdown`/`on-resources-discover`/`on-project-trust`. **Correction carried from the refutation pass**: the signal itself already exists and fires — `AgentSessionEvent::SessionInfoChanged { name }` is produced by `set_session_name` and asserted at `crates/cyrup-session-svc/tests/round8_postrun.rs:240-257`. The gap is purely extension-host *routing*, not a missing event source, so the fix is a `HostEvent` variant plus a WIT export plus one dispatch call beside the existing fan-out — effort S, not M.

**upstream** — `SessionInfoChangedEvent { type, name: string | undefined }` at `pi/packages/coding-agent/src/core/extensions/types.ts:571-575` @v0.83.0, subscribed at `:1193`.

**Impact** — extensions cannot react to a session being renamed or its metadata changing; a status-line or external-sync extension goes stale while the session layer below it already knows.

**Fix** — add the kind (bump `COUNT`/`from_u8`/`name()`), an `on-session-info-changed: func(name: option<string>)` export in both `world.wit` copies, an SDK hook, and dispatch it beside the existing `AgentSessionEvent::SessionInfoChanged` emit in `crates/cyrup-session-svc/src/session.rs`. Export addition ⇒ bump `HOST_WORLD`; batch with the other ABI items.

**Verify** — rename a session; assert a subscribed guest receives the event with the new name, and that the existing `round8_postrun.rs:240-257` assertion still holds.

## EXT-013 — Slash-command argument completions and autocomplete providers are dead

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `ExtensionHost::command_completions` (`crates/cyrup-ext/src/facade.rs:1207-1214`) has exactly one caller workspace-wide, `crates/cyrup-ext/tests/discover_load.rs:100`. `LiveExtension::argument_completions` (`crates/cyrup-ext/src/host/live.rs:1178-1195`) has one non-SDK caller, `crates/cyrup-ext/tests/wasm_component.rs:169`. Nothing under `crates/cyrup-tui/` calls either. Since EXT-S02 closed, the TUI *does* surface extension commands — and hardcodes `has_arg_completion: false` for every dynamic command at `crates/cyrup-tui/src/commands.rs:348`, so the popup never asks.

**upstream** — pi threads the live callback straight through: `getArgumentCompletions: cmd.getArgumentCompletions,` at `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:607` @v0.83.0, in the same object literal as `name: cmd.invocationName` at `:605`; declared on `RegisteredCommand` at `extensions/types.ts:1166`.

**Impact** — an extension can declare argument completions that never appear. The registration API succeeds and the feature is inert; the author's only symptom is that `<tab>` does nothing after their command name.

**Fix** — carry the real flag into `dynamic_commands_from_catalog_gated` (`commands.rs:308-350`) instead of `false`, and call `command_completions` from the TUI completion path for the argument position, folding `argument_completions` results in load order. Reuse the off-thread pattern already used by `extension_render` (`crates/cyrup-tui/src/app.rs`): sync pre-check, spawn, timeout, `abort()`. Route resolution through `resolved_command_owner` (`registry.rs:506-512`) so `deploy:2` is reachable — that is EXT-017's edit, and this item should land immediately after it.

**Verify** — type `/deploy <tab>` with a guest declaring argument completions; assert its items appear, and that a registered autocomplete provider contributes to plain-text completion.

## EXT-014 — tool_execution_update / tool_execution_end drop toolName (and args)

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `crates/cyrup-ext/src/event.rs:304-305`: `ToolExecUpdate { call_id, chunk }` and `ToolExecEnd { call_id, result, is_error }`. `HostEvent::from_agent` discards the rest with `..` at `event.rs:445` and `:451`, while `ToolExecStart` immediately above at `:438-444` destructures and keeps `tool_name` and `args` — the loss is visible in three adjacent arms. WIT signatures match the loss at `crates/cyrup-ext/wit/world.wit:230-231`.

**upstream** — `ToolExecutionUpdateEvent { type, toolCallId, toolName, args, partialResult }` at `pi/packages/coding-agent/src/core/extensions/types.ts:769-775` and `ToolExecutionEndEvent { type, toolCallId, toolName, result, isError }` at `:778-784` @v0.83.0. Note the update event carries **`args`** as well, so cyrup drops two fields there, not one.

**Impact** — an extension observing tool execution must maintain its own `callId → toolName` map from `tool_execution_start`, and cannot filter by tool at all if it missed the start (late registration, reload, or a run that began before it subscribed).

**Fix** — add `tool_name` to both variants and `args` to `ToolExecUpdate` (`event.rs:304-305`), stop discarding at `event.rs:445`/`:451`, widen the two WIT exports in both copies. Export re-signing ⇒ bump `HOST_WORLD`; batch with the other ABI items.

**Verify** — a guest subscribed only to `tool_execution_end` asserts a non-empty `tool_name`; a guest subscribed only to `tool_execution_update` asserts non-empty `args`.

## EXT-015 — Session-lifecycle events lose their discriminating fields at the extension boundary

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `SessionStart { reason }` / `SessionShutdown { reason }` (`crates/cyrup-ext/src/event.rs:307-308`) and `SessionBeforeSwitch { target_id }` / `SessionBeforeFork { entry_id }` (`:332-333`); WIT at `crates/cyrup-ext/wit/world.wit:234-235` and `:240-241`. The fields exist one layer down — `AgentSessionEvent::SessionStart` carries `previous_session_file` and is fanned out intact before the extension dispatch drops it.

**upstream** @v0.83.0, all four re-counted exact — `SessionStartEvent` (`reason` **and** `previousSessionFile?`) at `types.ts:562`; `SessionBeforeSwitchEvent` (`reason: "new"|"resume"` **and** `targetSessionFile?`) at `:578`; `SessionBeforeForkEvent` (`entryId` **and** `position: "before"|"at"`) at `:585`; `SessionShutdownEvent` (`reason`, `targetSessionFile?`) at `:616`.

**Impact** — an extension cannot tell a fresh session from a resume, cannot find the previous session file, and cannot tell a fork *before* an entry from a fork *at* it. Session-lifecycle extensions are limited to a coarse "something happened". Note cyrup drops `reason` from `session_before_switch` specifically, which is the field that distinguishes the two cases it most needs.

**Fix** — widen the four `HostEvent` variants and their WIT exports in both copies; pass `previous_session_file` from the existing `emit_session_start` fan-out in `crates/cyrup-session-svc/src/session.rs`; add pi's `reason` alongside `target_id` in `crates/cyrup-session-svc/src/runtime.rs` (the new-session case is papered over with `target_id: String::new()`). Export re-signing ⇒ bump `HOST_WORLD`.

**Verify** — resume a session and assert the guest sees the previous session file and `reason == "resume"`; fork before and at an entry and assert the two are distinguishable.

## EXT-017 — Command listing is non-deterministic, drops name:N, and a colliding command is unexecutable

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `commands: HashMap<String, (ExtensionId, CommandDescriptor)>` at `crates/cyrup-ext/src/registry.rs:139`, and `command_descriptions` (`:662-669`) iterates `.commands.iter()` directly — random order, one entry per name, bare `name`. The correct implementations exist and are **production-dead**: `resolved_commands` (`:462-501`) and `resolved_command_owner` (`:506-512`) are reached only from `crates/cyrup-ext/tests/aggregation.rs:227-238` and `crates/cyrup-ext-subagents/tests/watchdog_wiring.rs:90`. Execution is still raw-name: `live_for_command` → `command_owner` (`facade.rs:1217-1227`, `registry.rs:650-652`), a last-wins map lookup. **Consequence escalated this pass**: since EXT-S02 closed, `command_descriptions` feeds `slash_command_catalog` (`crates/cyrup-session-svc/src/session.rs:2281`, emitting the bare `name` at `:2277`) which feeds the interactive `/` autocomplete (`crates/cyrup-tui/src/app.rs:4103`, `:6340`, `:6937`), so the nondeterministic ordering is now directly user-visible.

**upstream** — `resolveRegisteredCommands` assigns `name:N` in load order with a `takenInvocationNames` bump loop (`pi/packages/coding-agent/src/core/extensions/runner.ts:598-631`), and `name: cmd.invocationName` is what reaches autocomplete (`modes/interactive/interactive-mode.ts:605`) @v0.83.0.

**Impact** — the slash-command palette reorders between runs, and when two extensions register `deploy` only one is listed and the **last** registrant executes; the other is silently unreachable. The reordering claim is derived from Rust's per-process randomly-seeded `HashMap` hasher rather than from an observed reorder — see `## Coverage`.

**Fix** — swap `session.rs:2277` to emit `r.invocation_name` from `resolved_commands()`, and change `live_for_command` (`facade.rs:1217-1227`) to consult `resolved_command_owner` so `deploy:2` dispatches to its own owner. `tests/aggregation.rs:227-238` already proves the correct behaviour — it is simply unreachable from production. EXT-013 and EXT-053 both depend on this edit.

**Verify** — two extensions registering `deploy`: assert stable load-order listing showing `deploy:1`/`deploy:2` and that invoking each reaches its own owner.

## EXT-018 — The inter-extension event bus is wasm-only — natives have no pi.events

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `bus: Arc<crate::host::SharedBus>` is `#[cfg(feature = "wasm-host")]` at `crates/cyrup-ext/src/facade.rs:137-138` (constructed `:171-172`); `deliver_bus_events` is cfg-gated and resolves subscribers only out of `self.live` (`facade.rs:1014`), the wasm instance map. `grep -c bus crates/cyrup-ext/src/native.rs` is zero, and `InitApi` (`native.rs:230-238`) has six fields — subs, tools, commands, tool_renderers, message_renderers, entry_renderers — none a bus. All three cyrup-shipped extensions (permission-system, intercom, subagents) are `NativeExtension`s, so the documented coordination channel reaches nothing that ships.

**upstream** — pi attaches the one shared bus to every extension regardless of kind: `events: eventBus,` on the returned `ExtensionAPI` at `pi/packages/coding-agent/src/core/extensions/loader.ts:389` @v0.83.0 (it becomes a guarded wrapper at v0.84.1 — see EXT-050), bus impl at `core/event-bus.ts:12-32`.

**Impact** — the permission system, intercom and subagents extensions cannot signal each other through the channel built for exactly that. Any cross-extension coordination has to be re-invented out of band.

**Fix** — move `bus` out of the cfg gate, give `NativeExtension` an `on_bus_event` entry point mirroring the renderer-registration path, and extend `deliver_bus_events` to fan out to `self.native` as well as `self.live`. Fold with EXT-034 (the drain placement), EXT-050 (unsubscribe + lifecycle teardown) and EXT-057 (the round-bound drop) — all four touch the same function and should land as one change.

**Verify** — a native emitting on `demo:bus` and a second native subscribed to it; assert delivery in a build with **and** without `wasm-host`.

## EXT-021 — ctx.ui capabilities with no WIT representation (at least eight)

**Kind** not-ported · **Severity** medium · **Effort** L · **Confidence** high on the WIT surface; medium on completeness (see caveat)

**cyrup** — `interface ui` (`crates/cyrup-ext/wit/world.wit:302-348`) was enumerated function by function at HEAD: 22 functions — notify, set-status, abort-signal, confirm, input, select, editor, set-widget, set-header, set-footer, set-title, custom, get/set/paste-editor-text, theme-get/list/set, working-start/stop, get/set-tools-expanded. It still collapses pi's working-indicator controls into `working-start(label)`/`working-stop()` (`:343-344`) and still returns bare theme **names** from `theme-list` (`:340`).

**upstream** — absent from the WIT, all eight verified line-exactly at v0.83.0: `onTerminalInput` (`types.ts:145`), `setWorkingVisible` (`:154`), `setWorkingIndicator` (`:164`), `setHiddenThinkingLabel` (`:167`), `setEditorComponent` (`:260`), `getEditorComponent` (`:263`), `getAllThemes(): {name, path}[]` (`:269`), `getTheme(name)` (`:272`). A **ninth** is partially covered: `setWorkingMessage(message?)` (`:151`) is not the same thing as `working-start(label)`. A tenth — `setWidget`'s dropped key and placement — is filed separately as **EXT-047**.

**Impact** — extensions cannot observe raw terminal input, cannot replace or read the editor component, cannot control working-indicator visibility independently of the label, and cannot enumerate themes with their paths or inspect a theme without switching to it.

**Fix** — the cheap half is all *imports* and needs no `HOST_WORLD` bump (`manifest.rs:51-54`): add `set-hidden-thinking-label`, `set-working-indicator`, `set-working-visible`, `set-working-message`, `theme-get-by-name`, and widen `theme-list` (`world.wit:340`) to a `{name, path}` record. The expensive half (`onTerminalInput`, `setEditorComponent`/`getEditorComponent`) needs guest exports and a component-safe renderable-editor representation; if genuinely out of scope, write the `CYRUP-DELTA` into `crates/cyrup-ext/src/lib.rs` naming the pi lines and the reason — the omission EXT-045 exists to punish.

**Verify** — a guest calling each new import and asserting the observable TUI effect; for any delta, assert the `lib.rs` note exists so it is not re-found as a gap.

*Caveat*: the WIT surface was verified exhaustively and the eight pi declarations line-exactly, but `crates/cyrup-ext/src/host/services.rs` (~1944 lines) has still not been audited method by method, so a `ctx.ui` capability could be implemented host-side while missing from the WIT.

## EXT-023 — prepareArguments is unreachable for WASM guest tools, and the SDK drops the field silently

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — the host mechanism exists and works for natives (`Tool::prepare_arguments`, `crates/cyrup-core/src/tool.rs:134`, invoked before validation in `crates/cyrup-agent/src/agent.rs`). The WASM boundary cannot express it: `grep -n prepare crates/cyrup-ext/wit/world.wit` returns **nothing** at HEAD — the string does not occur in the world at all; the WIT `tool-descriptor` (`world.wit:39-48`) has 8 fields and no prepare flag, `ToolDescriptor` (`crates/cyrup-ext/src/registry.rs:16-30`) likewise, and `impl Tool for WasmTool` (`crates/cyrup-ext/src/host/live.rs:1386-1420`) has no override so the identity default stands. The SDK accepts and discards it: `pub prepare_arguments: bool` at `crates/cyrup-ext-sdk/src/descriptor.rs:47-49`, documented as "the host coerces args before validation when set", and `lower_tool_descriptor` (`crates/cyrup-ext-sdk/src/guest.rs:55-69`) copies 8 of 10 fields. Struct-literal construction of a different type means no compile error and no warning.

**upstream** — `prepareArguments?: (args: unknown) => Static<TParams>;` on `ToolDefinition` at `pi/packages/coding-agent/src/core/extensions/types.ts:468` @v0.83.0, run before `validateToolArguments` in `packages/agent/src/agent-loop.ts`.

**Impact** — a guest tool that needs argument coercion (the most common reason a tool call fails validation) sets a documented SDK field that does nothing, with no diagnostic anywhere.

**Fix** — add `prepare-arguments: bool` to the WIT `tool-descriptor` and a `prepare-arguments: func(args-json: string) -> option<string>` guest export in both copies; carry the flag on `registry.rs:16-30`; implement `Tool::prepare_arguments` on `WasmTool`. Copy the field in `lower_tool_descriptor` (`guest.rs:55-69`) and add a compile-time exhaustiveness guard there so future fields cannot be dropped silently — the same guard closes EXT-024's half. Export addition ⇒ bump `HOST_WORLD`.

**Verify** — a guest tool declaring `prepare_arguments` that coerces a string to a number; assert the call validates and executes.

## EXT-024 — renderShell/constrainedSampling unexpressible, and render_kind has zero consumers

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `grep -n render-shell crates/cyrup-ext/wit/world.wit` is empty and the `tool-descriptor` record carries no such field, while cyrup-core models it host-side as `ToolRenderKind` (`crates/cyrup-core/src/tool.rs:67`) / `Tool::render_kind` (`:127-128`). Every occurrence workspace-wide is a definition, default, re-export, passthrough or unit test (`cyrup-core/src/tool.rs:67,127-128,198`; `cyrup-core/src/lib.rs:33`; `cyrup-ext/src/wrapper.rs:24,110-111,287`) — **zero consumers** in `cyrup-tui` or `cyrup-agent`, so even a native tool declaring `SelfRendered` is ignored by the TUI. The SDK drops its half too: `render_shell: RenderShell` at `crates/cyrup-ext-sdk/src/descriptor.rs:43-45`, never copied by `lower_tool_descriptor` (`guest.rs:55-69`). *Evidence correction*: the auditor reported `grep -rn constrained_sampling crates/` as empty; it is not — `crates/cyrup-provider/src/api/bedrock_converse_stream.rs:44` matches, but the hit is a comment reading "cyrup's `ToolDef` has no such field", which confirms the gap rather than refuting it.

**upstream** — `constrainedSampling?: false | ConstrainedSamplingConfig;` at `pi/packages/coding-agent/src/core/extensions/types.ts:463` and `renderShell?: "default" | "self";` at `:465` @v0.83.0.

**Impact** — a self-rendering tool still draws the default row chrome; the model's structured-output constraint hook does not exist at any layer.

**Fix** — three pieces for `renderShell`: the WIT/`registry.rs` field, the copy in `lower_tool_descriptor`, and — the part that matters most — a TUI consumer of `render_kind` in the tool-row draw path, now feasible since renderers are live end to end. `constrainedSampling` needs provider-side request assembly (area 01).

**Verify** — a native tool declaring `SelfRendered`: assert the TUI omits the default shell.

## EXT-029 — An abort landing during a gated tool-call dispatch reports as an extension failure

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — the pre-check is at `crates/cyrup-agent/src/agent.rs:967-968` and the re-check at `:999-1000`, so every not-yet-dispatched call in a batch reports correctly. The defect is the race window: the token handed to the hook is a **child** of the run's — `self.hooks.before_tool_call(ctx, self.cancel.child()).await` at `agent.rs:984` — so a run abort cancels it mid-flight and the dispatch returns `Err(ExtError::Cancelled)`. That error is not excluded by the `Err(e)` arm of `dispatch_block_mutate` (`crates/cyrup-ext/src/dispatch.rs:249-257`), which reports it to every `onError` listener and, because `tool_call` is fail-closed, synthesizes `Blocked{reason: Some("Extension failed, blocking execution: cancelled")}`. **Worse since EXT-S03 closed**: that spurious `onError` is now rendered into the interactive transcript as `Extension "<id>" error: cancelled` (`crates/cyrup-tui/src/app.rs:3161-3165`, drained at `:6807`).

**upstream** — pi has no path by which an abort becomes an extension fault: `emitToolCall` (`pi/packages/coding-agent/src/core/extensions/runner.ts:927-948` @v0.83.0) takes no signal and has no cancellation race, and the abort path returns `createErrorToolResult("Operation aborted")` before the block branch (`packages/agent/src/agent-loop.ts:629-635` @v0.84.1).

**Impact** — pressing Esc during a tool call writes a transcript entry blaming a healthy extension **and** now shows a spurious `[Extension issues]`-style error line. Reachable whenever an extension subscribes to `tool_call` — the normal state once `cyrup-permissions.jsonc` exists.

**Fix** — in `dispatch_block_mutate` (`dispatch.rs:249-257`), special-case `ExtError::Cancelled`: do not `report` it and do not synthesize a block reason — return `Blocked { reason: None }` so the agent's own "Operation aborted" text wins; or short-circuit to `Proceed` in `crates/cyrup-ext/src/hooks.rs` when the run token is already cancelled. **Do not weaken `fails_closed`** — that reopens EXT-001.

**Verify** — abort mid-`before_tool_call` with a subscribed extension; assert the result text is "Operation aborted", that no `onError` fired and no transcript error line appeared, and that `tests/ext_fail_closed.rs` still passes unchanged.

## EXT-030 — materialize_guest_tools unconditionally clears the tools-dirty flag, swallowing its own re-arm

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — the not-yet-live branch deliberately re-arms at `crates/cyrup-ext/src/facade.rs:419-424` (`self.registry.mark_tools_dirty(); continue;`), but the tail at `:431-433` is `if changed { self.registry.take_tools_dirty(); }`, and `take_tools_dirty` is a wholesale `swap(false, AcqRel)` (`crates/cyrup-ext/src/registry.rs:272-274`) that clears that re-arm along with any mark raised meanwhile. Trigger: at least one tool materialized (`changed == true`) **and** another was skipped or arrived concurrently. `refresh_tools` short-circuits on the same flag (`facade.rs:404-406`), and `load_wasm` calls `materialize_guest_tools` directly (`facade.rs:1102`), so the clearing happens on every extension load.

**upstream** — pi has no dirty flag: `registerTool` ends with `runtime.refreshTools()` on every registration (`pi/packages/coding-agent/src/core/extensions/loader.ts:245-252`) and `_refreshToolRegistry` rebuilds the whole registry each time (`core/agent-session.ts:2452-2546`) @v0.83.0, so no signal can be lost.

**Impact** — a tool registered concurrently with a materialization pass, or one whose owner was not yet live, is dropped permanently for the session. Nondeterministic and load-order dependent.

**Fix** — drop the `take_tools_dirty()` at `facade.rs:431-433` and instead stop the materializer's own re-registrations from raising the flag — a `register_tool_quiet` on `ExtensionRegistry`, or a `mark_dirty: bool` parameter on `register_tool` (`registry.rs:217-229`). `refresh_tools` already takes the flag once at entry (`facade.rs:404`), which is the correct scoping.

**Verify** — two guests where one registers a tool while the other's descriptors materialize; assert both tools appear after a single `refresh_tools`, and that a descriptor whose owner is not yet live is retried on the next refresh.

## EXT-031 — Turn-boundary refresh propagates tools but not the rebuilt system prompt

**Kind** parity-bug · **Severity** medium · **Effort** L · **Confidence** high

**cyrup** — `PolicyHooks::prepare_next_turn` sets only the tool array — `update.tools = Some(session.next_turn_tools().await);` at `crates/cyrup-session-svc/src/hooks.rs:179` — and `TurnUpdate.system_prompt` remains an unset slot. The divergence is stated in the doc block immediately above at `:158-160` ("Pi also re-pushes `context.systemPrompt` here. cyrup does not"). The stated reason is real: cyrup has one prompt slot into which `assemble_run_messages` already wrote a `before_agent_start` handler's sanitized prompt, and overwriting it mid-run would undo the permission companion's tool-exposure shaping. Under this project's rules a documented divergence is still work — there is no accepted-divergence category.

**upstream** — pi keeps the override and the base in separate slots: `_installAgentNextTurnRefresh` returns `{...previousContext, systemPrompt: this._systemPromptOverride ?? this._baseSystemPrompt, tools: …}` at `pi/packages/coding-agent/src/core/agent-session.ts:519-540` @v0.83.0.

**Impact** — a tool registered mid-run becomes callable but undescribed for the remainder of that run. Compounds EXT-007: between them an extension tool's snippet is missing from the first prompt of a session and from the rest of any run it joins mid-flight.

**Fix** — split the prompt into `base_system_prompt` and `system_prompt_override` as pi does, then have `next_turn_tools` return `(tools, resolved_prompt)` and `prepare_next_turn` (`hooks.rs:179`) populate `TurnUpdate.system_prompt`. Until the split lands, keep **both** existing guards — removing either without the split silently undoes permission-system prompt sanitization.

**Verify** — register a tool mid-run and assert its snippet appears in the next turn's prompt while a `before_agent_start` sanitization applied earlier in the same run survives. The tool-array half is already covered by `crates/cyrup-agent/tests/turn_tool_refresh.rs`.

## EXT-033 — A configured extension path that is unloadable or nonexistent produces no diagnostic

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — the FILE half of the original item is **closed**: `discover` now branches on `is_component_file(p)` first and calls `push_file` (`crates/cyrup-ext/src/loader.rs:124-130`), with `is_component_file` at `:155-157` and `push_file` synthesizing the minimal manifest at `:193-221`; new coverage at `crates/cyrup-ext/tests/loader_direct_file.rs`. The DIAGNOSTIC half survives: a configured path that exists but is neither an extension dir nor a `.wasm`, **and a nonexistent path**, both fall through to `scan_dir`, whose first statement is still `let Ok(rd) = std::fs::read_dir(dir) else { return };` (`loader.rs:169`) — a silent return. `discover_and_load` (`facade.rs:1138-1149`) only pushes to `errors` from `load_discovered`, so such a path yields neither `loaded` nor `errors`.

**upstream** — pi guards on `fs.existsSync(resolved) && fs.statSync(resolved).isDirectory()` and falls through to `addPaths([resolved])` for anything else (`pi/packages/coding-agent/src/core/extensions/loader.ts:704-717` @v0.83.0), which then surfaces the failure as a per-path `LoadExtensionsResult.errors` entry.

**Impact** — a typo'd `-e` path is indistinguishable from a correct one: no load, no diagnostic, empty `errors`. The author's only symptom is that their extension's tools and commands are absent. This is also the documented escape hatch under `--no-extensions`, so it is the path a user is most likely to reach for.

**Fix** — give `discover` a `(Vec<DiscoveredExtension>, Vec<LoadError>)` return, or pre-validate `roots.configured` in `facade::discover_and_load` (`facade.rs:1138-1149`), so a path that resolves to nothing produces exactly one `LoadExtensionsResult.errors` entry naming it. Surface it through the same `startup_diagnostics.extensions` channel `builder.rs:775-786` already uses (EXT-S01's mechanism).

**Verify** — point `DiscoveryRoots.configured` at (a) a nonexistent path and (b) an existing non-extension file; assert each produces exactly one `errors` entry naming the path, and that the already-closed `.wasm` case still loads. Extend `crates/cyrup-ext/tests/loader_direct_file.rs`.

## EXT-034 — Bus events emitted from an event handler are never delivered

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `ExtensionHost::deliver_bus_events` (`crates/cyrup-ext/src/facade.rs:1003-1028`) has exactly **two** production call sites, both at the tail of a command-tier call: `facade.rs:1201` (`run_command`) and `:1255` (`run_shortcut`); the only other hits workspace-wide are doc links and `crates/cyrup-ext/tests/wasm_bus_flag.rs:82`. Nothing drains after `dispatch_notify` / `dispatch_block_mutate` / `dispatch_collect_handled` / `dispatch_first_handled` (`crates/cyrup-ext/src/dispatch.rs`), after `LiveExtension::invoke_event`, or anywhere in `cyrup-session-svc` — including the live inline `emit_user_bash_event` (`crates/cyrup-session-svc/src/session.rs:4608-4612`), which dispatches and returns with no drain. The deferral itself is deliberate and correct (wasm single-instance reentrancy forbids re-entering the emitting guest inside its own `bus.emit` import); the defect is that the drain was wired only into the command tier.

**upstream** — `createEventBus()` returns `emit: (channel, data) => { emitter.emit(channel, data); }` over a node `EventEmitter` (`pi/packages/coding-agent/src/core/event-bus.ts:12-32` @v0.83.0), so every listener runs synchronously at the emit call. There is no queue, no drain point, and no entry point from which an emit can go undelivered.

**Impact** — `pi.events` silently works from a slash-command handler and silently does not work from an event handler, which is where cross-extension coordination actually happens (a permission decision, a tool result, a session start). The author sees `bus.emit` succeed with no error. Combined with EXT-018, the bus is usable today only for guest→guest signalling initiated by a slash command.

**Fix** — drain at every seam that can have re-entered a guest. The cheapest correct placement is inside `Dispatcher`'s public entry points, after the subscriber loop completes, which is outside every guest store guard; that requires the dispatcher to hold an `Arc<SharedBus>`. Land together with EXT-018 (so natives get the same drain), EXT-050 (lifecycle teardown) and EXT-057 (the round-bound drop).

**Verify** — two guests from the fixture component, B subscribed to `demo:bus`. Drive an **event** (not a command) into A whose handler emits, and assert B's `bus-deliver` ran before the dispatch call returned, with no manual `deliver_bus_events`. Add beside the two existing shapes in `tests/wasm_bus_flag.rs`.

## EXT-035 — NativeExtension can register only 5 of the 11 WIT registration surfaces

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `InitApi` (`crates/cyrup-ext/src/native.rs:230-238`) gained exactly one surface since this item was filed — `register_entry_renderer` (`:295-297`) — so its six fields are subs/tools/commands/tool_renderers/message_renderers/entry_renderers, and `InitParts` (`:219-226`) is the matching 6-tuple. Meanwhile `interface registration` (`crates/cyrup-ext/wit/world.wit:270-297`) now offers **eleven** registration verbs plus `subscribe`: `register-tool` `:272`, `register-command` `:273`, `register-shortcut` `:274`, `register-flag` `:275`, `get-flag` `:276`, `register-provider` `:277`, `unregister-provider` `:278`, `register-message-renderer` `:279`, `register-entry-renderer` `:290`, `add-autocomplete` `:291`, `add-autocomplete-provider` `:294`. A native reaches 5 of 11 — the ratio got **worse**, not better. The asymmetry is load-bearing even given a back door: `run_shortcut` (`facade.rs:1241-1252`) is still cfg-gated and resolves owners out of `self.live` only, so a registry entry owned by a native could never fire.

**upstream** — pi has one extension kind and one API object: `loader.ts:274-410` @v0.83.0 builds a single `ExtensionAPI` carrying registerTool/registerCommand/registerShortcut/registerFlag/getFlag/registerProvider/unregisterProvider/registerMessageRenderer/registerEntryRenderer/addAutocompleteProvider/events and hands it to every extension it loads. There is no upstream notion of an extension that can register tools but not shortcuts, flags or providers.

**Impact** — with zero WASM guests shipping, six of the eleven documented registration capabilities are unreachable by every extension that actually exists. A first-party extension cannot bind a keyboard shortcut, cannot declare a CLI flag (so it cannot be configured the way pi extensions are), and cannot contribute a provider — which is how `CYRUP_*` env-var sprawl grows.

**Fix** — add `register_shortcut(key, desc)`, `register_flag(name, spec)`, `register_provider(id, config)`, `unregister_provider` and the two autocomplete verbs to `InitApi` (`native.rs:230-297`), extend `InitParts` and the `load_native` fold to push each into the existing registry methods. Then make `run_shortcut` (`facade.rs:1241-1252`) try `self.native` before `self.live` and drop its `wasm-host` cfg gate. Do the shortcut half together with EXT-039 (reserved-key refusal) and EXT-040 (the dropped description), which rewrite the same registrar.

**Verify** — a native registering a shortcut, a flag and a provider in `init`: assert `shortcut_keys()` lists the key, that `run_shortcut` reaches the native's handler, that a CLI flag value round-trips through a native `get_flag`, and that `registry().provider_ids()` contains the id. Add to `crates/cyrup-ext/tests/native_dispatch.rs`.

## EXT-038 — ext-tools.get-all-tools returns only extension tools and drops promptGuidelines and sourceInfo

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-ext/src/host/live.rs:746-750` serializes `guest.registry.tool_info()`. That function (`crates/cyrup-ext/src/registry.rs:389-420`) walks only `tool_order` and `guest_tool_order` — the two **extension** tables — and emits `{name, source: "extension"|"guest", description, parameters}`. Built-in tools (read/write/edit/bash/grep/find/ls) are absent entirely, and `promptGuidelines` and `sourceInfo` are not emitted at all. The sibling `get_active_tools` immediately above (`live.rs:732-745`) does it correctly, preferring the live session's real set via `guest.services.active_tools()`, and `set_active_tools` (`:751-759`) also routes to the live backend.

**upstream** — `getAllTools()` maps `this._toolDefinitions` — the merged registry including built-ins, MCP and extension tools — to `{name, description, parameters, promptGuidelines, sourceInfo}` at `pi/packages/coding-agent/src/core/agent-session.ts:906-914` @v0.83.0; the type is `ToolInfo` at `extensions/types.ts:1552`, and the API doc at `:1323` reads "Get all configured tools with parameter schema, prompt guidelines, and source metadata."

**Impact** — this is the introspection API a plan-mode or tool-restriction extension uses to decide what to pass to `setActiveTools`. Under cyrup it reports that the session has only extension tools, so such an extension computes a restriction set that silently omits every built-in; `set_active_tools` then applies it and the agent loses read/write/edit/bash. The read is wrong and the write is honoured, which is what makes this functional rather than cosmetic.

**Fix** — add an `all_tools() -> Option<Vec<Value>>` accessor to `HostServices` (`crates/cyrup-ext/src/host/services.rs`) mirroring `active_tools`, implemented in `LiveHostServices` (`crates/cyrup-session-svc/src/host_services.rs`) off the dynamic tool registry so it covers built-ins + custom + extension tools; have `get_all_tools` (`host/live.rs:746`) prefer it and keep `registry.tool_info()` as the no-session fallback. Add `promptGuidelines` and a `sourceInfo` object to both producers. The guidelines half is blocked on the same `Tool::prompt_guidelines(&self) -> &[&str]` signature EXT-007 must widen — do them together.

**Verify** — with a session carrying built-ins plus one extension tool, assert a guest's `get_all_tools()` contains the built-in names and that a tool declaring prompt guidelines reports them. Extend `crates/cyrup-ext/tests/wasm_component.rs`.

## EXT-039 — Extension shortcuts bypass the reserved-keybinding refusal, emit no conflict diagnostics, and lose to built-ins

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high on the refusal and diagnostics halves; medium on the precedence half (see caveat)

**cyrup** — registration has no gate at all: `ExtensionRegistry::register_shortcut` (`crates/cyrup-ext/src/registry.rs:515-523`) is a bare `g.shortcuts.insert(key.into(), owner)` — no lowercase normalization, no comparison against the keymap, no diagnostic, and two extensions claiming the same key silently collapse to whichever registered last. `ExtensionHost::shortcut_keys()` (`crates/cyrup-ext/src/facade.rs:1233-1235`) hands the raw keys to the TUI, which parses and stores them (`set_extension_shortcuts`, `crates/cyrup-tui/src/app.rs:544-550`). Dispatch consults the **built-in** action table first (`return self.apply_action(action)` at `app.rs:1691`) and extension shortcuts only afterwards (`:1697-1703`), with an in-tree comment stating that as the intent. There is no counterpart to `getShortcutDiagnostics` anywhere in `crates/`.

**upstream** — `getShortcuts(resolvedKeybindings)` at `pi/packages/coding-agent/src/core/extensions/runner.ts:492-534` @v0.83.0 lowercases every key and, against the built-in keymap from `buildBuiltinKeybindings` (`:92-111`), (a) **skips** the extension shortcut with a warning when the colliding built-in is reserved (`restrictOverride === true`), (b) warns but **lets the extension win** when it is not reserved, then `extensionShortcuts.set(normalizedKey, shortcut)`, and (c) warns on extension-vs-extension collisions. The reserved list is `RESERVED_KEYBINDINGS_FOR_EXTENSION_CONFLICTS` (`runner.ts:70-89`: app.interrupt, app.clear, app.exit, app.suspend, app.model.\*, tui.input.submit, tui.select.confirm/cancel, …). `getShortcutDiagnostics()` (`:538-540`) is folded into the `[Extension issues]` startup panel at `modes/interactive/interactive-mode.ts:1612-1618`.

**Impact** — two opposite failures from one missing layer. An extension binding a **non-reserved** built-in key works upstream and is silently dead in cyrup. An extension binding a **reserved** key (Ctrl+C, Enter) is refused with a visible warning upstream; in cyrup it is accepted into the registry, listed by `/hotkeys` as if live, and never fires — an advertised-but-dead binding with no diagnostic. Two extensions claiming the same key silently collapse.

**Fix** — give `register_shortcut` (`registry.rs:515`) the upstream shape: lowercase the key, take the resolved keybinding config, port `RESERVED_KEYBINDINGS_FOR_EXTENSION_CONFLICTS` and `buildBuiltinKeybindings`, reject a reserved collision recording an `ExtensionConflict` (the mechanism exists at `registry.rs:251-256` and already flows into `LoadExtensionsResult.errors` via `facade.rs:1158-1162`), and warn-but-accept a non-reserved one. Then invert the TUI precedence at `crates/cyrup-tui/src/app.rs:1691-1703`, which is safe once the reserved list is enforced at registration. Surface the warnings through the same `startup_diagnostics.extensions` channel `builder.rs:775-786` uses. Land with EXT-035 and EXT-040, which rewrite the same registrar.

**Verify** — register three shortcuts: one on a reserved key (assert refused, one diagnostic, absent from `shortcut_keys()`), one on a non-reserved built-in key (assert it fires instead of the built-in, one warning), and two extensions on the same key (one diagnostic naming both).

*Caveat*: the precedence-**inversion** half is inferred from the reserved-list design and pi's "Using ${shortcut.extensionPath}" warning text, not read off pi's editor dispatch site. The reserved-refusal and missing-diagnostics halves are read directly and are not in doubt.

## EXT-041 — Replayed tool calls and results lose their extension renderer

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `replay_session_with_extensions` (`crates/cyrup-tui/src/app.rs:1305-1327`) resolves a renderer **only** for custom messages: `let AgentMessage::Custom(c) = message else { continue };` at `:1315`. The tool arms of the walk it delegates to pass no rendered payload at all — `push_tool_start_rendered(…, None)` at `app.rs:1370-1375` and `push_tool_end_rendered(…, None)` at `:1398-1405`. The plumbing exists and is used on the live path (`extension_render` routes tool calls and results through `ExtensionHost::render_tool_call`/`render_tool_result`, `crates/cyrup-ext/src/facade.rs:701-731`); the replay walk simply never asks. Both production callers hit it (`crates/cyrup/src/main.rs:1685`, `app.rs:6988`).

**upstream** — `populateHistory`'s assistant arm constructs a `ToolExecutionComponent` for **every** replayed `toolCall` and passes `this.getRegisteredToolDefinition(content.name)` as its definition (`pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:3374-3389` @v0.83.0); that definition carries `renderCall`/`renderResult`, which the component prefers over the built-in framing. The custom-message arm cyrup did port is the sibling of this one, not a superset.

**Impact** — a resumed session draws every extension-rendered tool row with the built-in framing while the live session drew the extension's output — exactly the asymmetry EXT-006's replay half was closed to remove, surviving on the tool surface. It bites the shipped population directly: `cyrup-ext-subagents` registers `render_call`/`render_result` for its subagent tool (`crates/cyrup-ext-subagents/src/extension.rs:9616`, `:9646`), so `/resume` on a session containing subagent runs loses their rendering.

**Fix** — extend `replay_session_with_extensions` (`app.rs:1305`) to build a second index keyed by tool-call id: for each replayed tool call whose name satisfies `ext_host.has_tool_renderer(name)` (`facade.rs:913-915`) call `extension_render` for the call, and the result side for each tool result; hand both maps into the `None` slots at `app.rs:1374` and `:1404`. The async-first-then-sync-walk pattern the custom-message half already uses applies unchanged, including the `EXTENSION_RENDER_TIMEOUT` race. Do the same for replayed custom **entries** via `has_entry_renderer`/`render_entry` (`facade.rs:926-928`, `:808`), which EXT-012's closure left unwired on this path.

**Verify** — run a session producing a tool call for a tool with a registered renderer, resume it, and assert the transcript shows the extension's rendered rows rather than the default shell — the assertion `crates/cyrup-tui/tests/extension_renderers.rs:420-520` already makes for custom messages, extended to the tool and entry surfaces.

## EXT-043 — The project_trust event carries no cwd

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `HostEvent::ProjectTrust` is a payload-less unit variant (`crates/cyrup-ext/src/event.rs:310`, kind mapping `:398`) and the WIT export is `on-project-trust: func() -> hook-outcome;` (`crates/cyrup-ext/wit/world.wit:237`). It is dispatched with no payload by `aggregate_project_trust` (`crates/cyrup-ext/src/facade.rs:466-478`), reached from `pre_trust_extension_verdict` (`crates/cyrup-session-svc/src/builder.rs:1640-1671`) — which demonstrably **has** the value in hand, `let host_config = HostConfig { mode, has_ui, cwd: cwd.to_path_buf() };` at `:1646`, four lines before the dispatch — and never passes it. There is no cwd accessor anywhere in the WIT to fall back on (EXT-044).

**upstream** — `ProjectTrustEvent { type: "project_trust"; cwd: string; }` at `pi/packages/coding-agent/src/core/extensions/types.ts:519-522` and `ProjectTrustContext { cwd; mode; hasUI; ui }` at `:531-536` @v0.83.0. The whole decision is per-directory: the store is keyed by cwd (`options.trustStore.set(options.cwd, trusted)`, `core/project-trust.ts:63-65`).

**Impact** — an extension implementing a trust policy — the documented use of this hook, and the one security-relevant hook in the catalog — cannot key its verdict on the directory. Any allowlist-based policy either trusts everything or nothing, and cannot honour `remember` meaningfully because it does not know what it is remembering. This is the seam whose result decides whether project-local extensions are loaded at all (`discover_and_load(&roots, project_trusted, …)`, `facade.rs:1132-1137`).

**Fix** — change the variant to `ProjectTrust { cwd: String }` (`event.rs:310`), the export to `on-project-trust: func(cwd: string) -> hook-outcome;` in both `world.wit` copies (`:237`), the marshaller in `crates/cyrup-ext/src/host/live.rs`, and pass `self.config.cwd` from `aggregate_project_trust` (`facade.rs:466-478`) — the facade already holds it at `HostConfig.cwd` (`facade.rs:106`). Export re-signing ⇒ bump `HOST_WORLD`. EXT-044 is the complementary import-only change that serves every other handler and needs no bump. Pair with EXT-003, which is the store this verdict should key.

**Verify** — load two projects in one process with a native that decides trust; assert its handler receives each project's own cwd and that a per-directory allowlist yields different verdicts. Extend the trust assertions in `crates/cyrup-ext/tests/aggregation.rs`.

## EXT-044 — ctx.cwd is unreachable from a WASM guest

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `grep -n cwd crates/cyrup-ext/wit/world.wit` at HEAD returns exactly three hits, **none of them an accessor**: a comment at `:194`, the `on-user-bash` event parameter at `:198`, and the `proc.spawn` argument at `:395`. `interface ctx-state` (`world.wit:523-540`) offers get-mode / has-ui / is-idle / has-pending-messages / is-project-trusted / get-system-prompt and nothing path-shaped, even though its own header comment claims to mirror pi's base `ExtensionContext`. The host holds the value on every path (`HostConfig.cwd`, `crates/cyrup-ext/src/facade.rs:106`, threaded into `HostCtx::event`/`::command` for natives at `:307`/`:337`); only the WIT verb and the copy into `GuestState` are missing. The native tier **does** expose it (`ctx.cwd`, read e.g. at `crates/cyrup-ext-subagents/src/extension.rs:9500`), which is why the gap is invisible from the shipped population.

**upstream** — `cwd: string` sits on the **base** `ExtensionContext` at `pi/packages/coding-agent/src/core/extensions/types.ts:315` @v0.83.0 (same block as `mode` `:311` and `hasUI` `:313`), so it is available to every handler and every tool execute, not just command handlers; interactive mode supplies `cwd: this.sessionManager.getCwd()`.

**Impact** — a guest cannot resolve any relative path: it cannot tell which project it is running in, cannot scope a cache, cannot interpret a path in a tool argument, and cannot compose a path for its `ext-fs` or `exec` capability without guessing. That is a divergence between cyrup's own two extension tiers as well as against pi. It is also the cheapest fix for two other items — EXT-016 and EXT-043 both degrade to nice-to-have once a guest can simply ask.

**Fix** — add `get-cwd: func() -> string;` to `interface ctx-state` (`world.wit:523-540`) in **both** copies — an **import**, so per `crates/cyrup-ext/src/manifest.rs:51-54` it is additive and needs no `HOST_WORLD` bump — implement it on `HostState` beside the other ctx-state getters in `crates/cyrup-ext/src/host/live.rs`, sourcing `GuestState`'s copy of `HostConfig.cwd` (extend `with_host_mode`, `facade.rs:1077-1081`, which today copies only mode and has-ui), and expose it as `Ctx::cwd()` in `crates/cyrup-ext-sdk/src/ctx.rs` beside `mode()`/`has_ui()`.

**Verify** — a guest asserting `ctx.cwd()` equals the session cwd from an event handler, a tool `execute`, and a command handler alike. Add to `crates/cyrup-ext/tests/guest_host_mode.rs`, which already drives the mode/has-ui pair the same way.

## EXT-047 — ui.set-widget drops pi's widget key and placement

**Kind** parity-bug · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `set-widget: func(widget-json: string);` (`crates/cyrup-ext/wit/world.wit:326`) — one opaque payload where pi takes three arguments. The host forwards it verbatim (`crates/cyrup-ext/src/host/live.rs:245-250`) into `UiEffect::SetWidget { widget: Value }` (`crates/cyrup-session-svc/src/host_services.rs:154`), whose own doc at `:148-153` **admits** the collapse and says there is "no cyrup-side convention to re-derive them from". The TUI then stores the whole blob in one slot (`AppState::extension_widget`, assigned at `crates/cyrup-tui/src/app.rs:3228`). A de-facto convention already exists and is unenforced: the shipped subagents extension hand-rolls `{"key": FLEET_STATUS_WIDGET_KEY, "content": null}` to mean *clear* (`crates/cyrup-ext-subagents/src/extension.rs:9489-9492`, `:9979-9982`, the latter commented "pi `ctx.ui.setWidget(FLEET_STATUS_WIDGET_KEY, undefined)`") — and that clear still leaves `extension_widget = Some({key, content: null})`, not `None`.

**upstream** — `setWidget(key: string, content: string[] | undefined, options?: ExtensionWidgetOptions)` at `pi/packages/coding-agent/src/core/extensions/types.ts:170-175` @v0.83.0 (plus a component-factory overload), with `ExtensionWidgetOptions {placement?}` at `:107-110` and `WidgetPlacement = "aboveEditor" | "belowEditor"` at `:104`. The key is what makes it a **map**: one widget per key, `undefined` content removes that key's widget, placement decides which chrome slot it lands in.

**Impact** — three concrete losses. (1) Two extensions that both set a widget silently clobber each other. (2) A widget cannot be removed — the shipped subagents extension already tries and the slot stays occupied with a null-content payload. (3) `belowEditor` placement is unexpressible. The read side is area 07's territory (TUI-S01 / TUI-014); the WIT signature is what makes the host structurally unable to do the right thing.

**Fix** — re-sign the import as `set-widget: func(key: string, content-json: option<string>, opts-json: string);` in both `world.wit` copies — an import re-signing, so bump `HOST_WORLD` and batch with the other ABI items. Change `HostServices::set_widget` (`crates/cyrup-ext/src/host/services.rs:260`) and `UiEffect::SetWidget` (`crates/cyrup-session-svc/src/host_services.rs:154`) to `{key, content: Option<Value>, placement}`, and turn `AppState::extension_widget` into a keyed map whose `None` content removes the entry. Update the two shipped call sites in `crates/cyrup-ext-subagents/src/extension.rs` to the typed form they are already emulating by hand.

**Verify** — two extensions each setting a widget under its own key: assert both are retained; then have one clear its key and assert only the other survives. Assert a `belowEditor` widget is distinguishable from an `aboveEditor` one at the host boundary.

## EXT-049 — ToolCallEventResult.terminate is unrepresentable

**Kind** upstream-drift · **Severity** medium · **Effort** S · **Confidence** high

**cyrup** — `HookOutcome::Block { reason: Option<String> }` (`crates/cyrup-ext/src/contract.rs:12-21`) has no terminate channel, and neither does the WIT `hook-outcome` arm `block(option<string>)` (`crates/cyrup-ext/wit/world.wit:30-35`). A block therefore reaches the agent as a plain error result: `dispatch_block_mutate` returns `Reduced::Blocked{reason}` (`crates/cyrup-ext/src/dispatch.rs:249-262`), which the agent turns into `immediate_error` (`crates/cyrup-agent/src/agent.rs:1008-1030`), hardcoding `terminate: false` at `:1030`. The batch machinery it would feed already exists and works — `has_more_tools = !batch.terminate;` at `agent.rs:534`, with the `every()`-equivalent at `:1306` — so only the seam is missing.

**upstream** — `ToolCallEventResult` gained `terminate?: boolean` at `pi/packages/coding-agent/src/core/extensions/types.ts:1072-1079` @v0.84.1 (**absent at v0.83.0**, so this landed after the ported baseline), documented "Hint that the agent should stop after the current tool batch when this call is blocked. Early termination only happens when every finalized tool result in the batch sets this to true." Consumed at `packages/agent/src/agent-loop.ts:636-646` and folded by `shouldTerminateToolBatch` at `:583` into `hasMoreToolCalls = !executedToolBatch.terminate` at `:216`.

**Impact** — a permission gate that denies a call hard — the motivating case, and cyrup's `tool_call` subscriber **is** the permission system — cannot end the run. Upstream, a gate that blocks every call in a batch with `terminate` stops the loop; under cyrup the model gets the error results back and keeps retrying against a gate that will deny every one, burning turns and tokens. It is also silently unavailable: an author writes `terminate: true` and nothing reads it.

**Fix** — add `terminate: bool` to `HookOutcome::Block` (`contract.rs:16`) and turn the WIT `block(option<string>)` arm into a `block-result` record `{reason: option<string>, terminate: bool}` (`world.wit:30-35`, both copies); carry it through `Reduced::Blocked` (`dispatch.rs:249-262`) and the `Hooks` adapter (`crates/cyrup-ext/src/hooks.rs`); set it on the finalized result where `immediate_error` is built for the **block** path only (`agent.rs:1008-1030`) so the existing every()-rule at `:1306` picks it up. `hook-outcome` re-signing touches every export ⇒ bump `HOST_WORLD`. Add the field to the native `HookOutcome` too, so the permission system can use it without a WASM boundary.

**Verify** — an extension blocking **every** call in a two-call batch with terminate: assert the agent loop ends after the batch. Assert that blocking only one of two does **not** terminate (upstream's `every()` rule). Add beside the fail-closed assertions in `crates/cyrup-ext/tests/ext_fail_closed.rs`.

## EXT-052 — An extension-supplied streamSimple provider fires neither before_provider_request nor after_provider_response

**Kind** not-ported · **Severity** medium · **Effort** M · **Confidence** high

**cyrup** — `grep -rn 'onPayload\|on_payload\|onResponse\|on_response' crates/cyrup-ext-sdk/src crates/cyrup-ext/src` returns nothing but Undici `onResponseStart` comments in `caps/http.rs`. A guest's custom stream is driven by `provider-stream-simple: func(id, stream-id, model-json, context-json, options-json) -> result<_, string>` (`crates/cyrup-ext/wit/world.wit:105`), invoked at `crates/cyrup-ext/src/host/live.rs:1296-1320`, which stringifies `options` (`:1306`) and hands it over as an opaque blob; the only callback back is `provider-stream.emit-event` (`world.wit:506`), which carries assistant-message stream events and nothing about the request payload or the HTTP response. So a guest provider has no way to invoke the two hooks, and cyrup's own `emit_before_provider_request` (`crates/cyrup-ext/src/facade.rs:592-606`) and the `AfterProviderResponse` dispatch are never reached on that route.

**upstream** — pi made this a hard contract in the `ProviderConfig.streamSimple` doc at `pi/packages/coding-agent/src/core/extensions/types.ts:1452-1457` @v0.84.1: "Implementations must invoke `options.onPayload` before sending the provider request and use any returned replacement payload. They must invoke `options.onResponse` after receiving the response and before consuming its body, matching built-in providers." The callbacks are declared on `StreamOptions` in `packages/ai/src/types.ts`, and they are how `before_provider_request` (`extensions/types.ts:676-679`) and `after_provider_response` (`:692-696`) reach requests a provider actually issues.

**Impact** — every request an extension-registered provider makes is invisible to every other extension. A proxy-tagging, auditing, redaction or cost-accounting extension that works against built-in providers silently stops working the moment the user switches to an extension-supplied one, with no diagnostic — a silent behaviour change keyed on model choice. It also means `before_provider_request`'s payload **replacement** (the one mutating provider seam cyrup does have) cannot be applied on that route, so a redaction extension leaks. Compounds EXT-009: neither the header hook nor the payload hook covers extension providers.

**Fix** — add two host imports on `interface provider-stream` (`world.wit:505-507`), both additive so no `HOST_WORLD` bump: `on-payload: func(stream-id: string, payload-json: string) -> option<string>;` returning a replacement, and `on-response: func(stream-id: string, status: u16, headers-json: string);`. Implement them on `HostState` beside `emit_event` in `crates/cyrup-ext/src/host/live.rs`, routing the first through `ExtensionHost::emit_before_provider_request` (`facade.rs:592`) and the second through an `AfterProviderResponse` dispatch — the same two reductions the built-in provider path already uses. Expose them from `crates/cyrup-ext-sdk/src/provider.rs` and document the must-invoke contract there verbatim from `types.ts:1452-1457`.

**Verify** — a guest provider whose `stream_simple` calls `on_payload` and `on_response`; with a second extension subscribed to `before_provider_request` and `after_provider_response`, assert the second sees both and that a payload replacement returned by `on_payload` is what the guest provider actually sends.

## EXT-055 — FsCaps::with_fs_root has zero callers, so ext-fs is permanently denied for every guest

**Kind** cyrup-original · **Severity** medium · **Effort** S · **Confidence** high

> **FIXED 2026-08-13, and the title's symbol name is wrong** — `FsCaps::with_fs_root` has never
> existed (`rg with_fs_root crates` → zero). The zero-caller mutator was `GuestState::with_fs`
> (`host/services.rs:1263`). Title kept verbatim per the no-renaming rule; read it as `with_fs`.
>
> Fixed in the same change as EXT-054, and the gap was WIDER than filed: `ext-fs` had no host root
> *and* no guest-side surface at all — `crates/cyrup-ext-sdk` had no `read_file`/`write_file`
> wrapper, so no guest could have called the interface even with a root. Both halves closed.
> `FsCaps` is now multi-root and mode-aware (`host/services.rs:957-1035`): `from_grants(base,
> grants)` resolves each `read:`/`write:` grant under `HostConfig.cwd`, `resolve`/`resolve_write`
> split the two modes so a `read:` grant no longer authorizes a write, and the empty-grant refusal
> names the manifest key (`"declares no capabilities.fs entry (e.g. \"read:.\" or
> \"write:.cyrup/todo\")"`). SDK: `Ctx::read_file`/`Ctx::write_file` (`cyrup-ext-sdk/src/ctx.rs:288-320`)
> plus `/fsread` + `/fswrite` demo commands in the reference guest (`example.rs:458-494`), the fs
> analog of the existing `/execdemo` + `/httpdemo`.
>
> **Evidence** — `crates/cyrup-ext/tests/manifest_capabilities.rs`:
> `fs_is_refused_when_no_grant_is_declared`, `fs_grants_are_scoped_by_mode_and_by_subtree` (a
> `["read:.", "write:.cyrup/todo"]` guest reads a project file, writes into `.cyrup/todo`, is
> REFUSED a write to a `read:`-only path, and is refused a `..` escape) and
> `a_malformed_fs_grant_fails_the_load`. `fs_grants_are_scoped_by_mode_and_by_subtree` is RED before
> the fix (measured) and GREEN after.

**cyrup** — `GuestState.fs` is initialized `fs: FsCaps::default()` (`crates/cyrup-ext/src/host/services.rs:1181`), and `FsCaps { root: Option<PathBuf> }` documents `None => all fs access denied` (`:958-962`); `FsCaps::resolve` opens with `let root = self.root.as_ref().ok_or("filesystem capability not granted")?;` (`:967-969`). The only mutator — `self.fs = FsCaps { root: Some(root) };` (`services.rs:1211`) — has **zero callers** workspace-wide; the only grep hit is its own definition. Therefore `ext-fs.read-file` and `ext-fs.write-file` (`crates/cyrup-ext/wit/world.wit:462-466`, host impls `crates/cyrup-ext/src/host/live.rs:625-635`, both of which call `guest.fs.resolve(&path)?` first) return `Err("filesystem capability not granted")` for every extension ever loaded.

**upstream** — none; pi has no capability model. Like EXT-054 this is a cyrup-original invariant that does not hold, and shares its root cause: the manifest's `fs: ["read:.", "write:.cyrup/todo"]` grant syntax (`crates/cyrup-ext/src/manifest.rs:26-28`) has no code that could ever honour it.

**Impact** — fail-closed, so not a security hole, but `interface ext-fs` is exported in the world's import list and documented as a real capability that can never succeed. A guest author gets an opaque runtime error string with no diagnostic pointing at the missing wiring, and no configuration that would fix it.

**Fix** — parse the manifest `fs` grants and call `with_fs_root` (or a multi-root successor) during `load_wasm`, which is exactly the thread EXT-054 opens; until the grant parser exists, at minimum seed the root from `HostConfig.cwd` for pre-trusted origins so the interface is reachable, and make the denial error name the manifest key that would grant it. Do not ship `ext-fs` as an unreachable export.

**Verify** — a guest with a manifest granting `read:.` reads a file under the session cwd and is refused a read outside it; a guest with no `fs` grant is refused both, with an error naming the manifest key.

## EXT-016 — resources_discover carries neither cwd nor reason

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `HostEvent::ResourcesDiscover` is a payload-less unit variant (`crates/cyrup-ext/src/event.rs:309`, kind mapping `:397`); WIT `on-resources-discover: func() -> hook-outcome;` (`crates/cyrup-ext/wit/world.wit:236`), dispatched from `aggregate_resources` (`crates/cyrup-ext/src/facade.rs:482-487`). There is no cwd accessor anywhere in the WIT to fall back on (EXT-044).

**upstream** — `ResourcesDiscoverEvent { type, cwd, reason: "startup" | "reload" }` at `pi/packages/coding-agent/src/core/extensions/types.ts:544-548` @v0.83.0.

**Impact** — a resource-contributing extension cannot tell which directory it is discovering for, nor distinguish startup from a `/reload`, so it cannot cache or scope its contribution. Low rather than medium because the handler returns paths rather than making a security decision — unlike EXT-043, which is the same shape on the trust hook.

**Fix** — add `cwd: String` and `reason: String` to the variant, pass them from the discovery call site and the reload path, widen `world.wit:236` in both copies. Export re-signing ⇒ bump `HOST_WORLD`. EXT-044 (`ctx-state.get-cwd`) is the cheaper partial that needs no bump and covers the cwd half for every handler at once.

**Verify** — a guest asserting a non-empty cwd and `reason == "startup"` at launch, and `"reload"` after `/reload`.

## EXT-019 — registerMarkdownTransformer has no counterpart

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** high

**cyrup** — `grep -rni 'markdown_transformer\|markdowntransform\|transform-markdown' crates/` returns zero hits at HEAD, and nothing in `crates/cyrup-ext/wit/world.wit`.

**upstream** — **re-scoped this pass**: at v0.83.0 this was a bare `MarkdownTransformer` type; the `v0.83.0..v0.84.1` diff adds `MarkdownTransformContext { messageType, isStreaming, availableWidth }` at `pi/packages/coding-agent/src/core/extensions/types.ts:1147-1153`, `registerMarkdownTransformer` on `ExtensionAPI` at `:1292` (impl `loader.ts:309-312`), `markdownTransformer?` on `Extension` at `:1703`, and `ExtensionRunner.getMarkdownTransformers()` at `runner.ts:589-591`.

**Impact** — extensions cannot post-process transcript Markdown (link rewriting, redaction, custom syntax). Post-baseline upstream addition — expected lag, not a regression.

**Fix** — port the **v0.84.1** shape, not the v0.83.0 one: a registration import in `world.wit`'s `interface registration`, an owner list in `registry.rs` preserving load order, a `transform_markdown` fold on the facade that passes the three context fields, and a call site in the TUI markdown render path. Guest export ⇒ bump `HOST_WORLD`.

**Verify** — two guests transforming the same message; assert both applied in load order and that `is_streaming` differs between a streaming and a settled render.

## EXT-022 — ProviderConfig.refreshModels is not represented

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** high

**cyrup** — `ProviderConfig` (`crates/cyrup-ext/src/provider.rs:17-42`) has no refresh hook — `has_stream_simple` at `:41` is the last field; the WIT provider block (`crates/cyrup-ext/wit/world.wit:101-105`) has login / refresh-token / get-api-key / modify-models / stream-simple and no `provider-refresh-models`.

**upstream** — `refreshModels?(context: RefreshModelsContext)` at `pi/packages/coding-agent/src/core/extensions/types.ts:1448` @v0.83.0, moved to `:1469` @v0.84.1 — and **the contract changed since this item was filed**: the doc now reads `Use context.publish({ persist: entry })` where v0.83.0 said `context.store`.

**Impact** — an extension provider's model list is fixed at registration; a provider that gains models at runtime cannot refresh them.

**Fix** — add a `provider-refresh-models` guest export in both `world.wit` copies plus a marker on `ProviderConfig`, shaped to the **publish/persist** contract rather than the old store one, and reuse the collapse-concurrent-calls machinery at `crates/cyrup-provider/src/utils/refresh.rs`. Guest export ⇒ bump `HOST_WORLD`.

**Verify** — a guest provider whose `refresh_models` publishes a changed list; assert the model picker reflects it, that a `persist` entry survives a restart, and that concurrent refreshes collapse.

## EXT-025 — reload() and four emit_* facade methods are dead code that has drifted

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — per-symbol greps at HEAD: `emit_before_agent_start` (`crates/cyrup-ext/src/facade.rs:495`) — callers are `tests/wasm_dispatch.rs:68,86` and `crates/cyrup-ext-subagents/tests/child_prompt_runtime_integration.rs:121`, all tests; `emit_input` (`:532`) — `wasm_dispatch.rs` only; `emit_user_bash` (`:612`) — `wasm_dispatch.rs` only; `command_completions` (`:1207`) — `tests/discover_load.rs:100` only; `ExtensionHost::reload` (`:1271`) — `discover_load.rs:118` only. `emit_message_end` (`:562`) is the live exception (`crates/cyrup-session-svc/src/subscriber.rs:156`). The drift is confirmed on both sides: the live production input path is the **separate inline copy** `AgentSession::emit_input_event` (`crates/cyrup-session-svc/src/session.rs:907`, called from `:872`), and the live `emit_user_bash_event` (`session.rs:4590-4614`) threads `exclude_from_context` (`:4599`) and the **session** cwd (`:4600`) while the facade copy hardcodes `exclude_from_context: false` and the **process** cwd (`facade.rs:617-627`).

**upstream** — one emitter per event on `ExtensionRunner` and no parallel unused copy (`pi/packages/coding-agent/src/core/extensions/runner.ts:950-977` for `emitUserBash`) @v0.83.0.

**Impact** — two implementations of the same seam, one exercised only by tests and already behind in signature. A contributor editing the facade copy changes nothing in production; one editing only the inline copy leaves the tests asserting stale behaviour. `reload()` being dead is also what makes EXT-050's only teardown path unreachable.

**Fix** — either delete the four dead `emit_*` methods and `reload()` and repoint the tests at the live paths, or make the live paths call the facade so there is one implementation — the latter is what `emit_message_end` already demonstrates. Prefer the latter; EXT-034 and EXT-050 both need a single seam to hook.

**Verify** — grep confirms each `emit_*` has at least one production caller or no definition; `tests/wasm_dispatch.rs` and `tests/discover_load.rs` exercise the live paths.

## EXT-026 — A wasmtime-free cyrup-session-svc build cannot be produced

**Kind** cyrup-original · **Severity** low · **Effort** M · **Confidence** medium (static analysis only — no compile was run this pass either)

**cyrup** — `mod host_services;` is still unconditional at `crates/cyrup-session-svc/src/lib.rs:29`, and `crates/cyrup-session-svc/src/host_services.rs:17-19` still imports `cyrup_ext::caps::http::HttpCaps`, `cyrup_ext::caps::proc::ProcCaps` and `cyrup_ext::host::{…}` with no cfg guard. `Cargo.toml:73` declares `cyrup-ext = { path = "crates/cyrup-ext", version = "0.0.0" }` with no `default-features = false`, while `crates/cyrup-session-svc/Cargo.toml:33` inherits it `{ workspace = true }` and declares its own forwarding feature at `:23`, so cyrup-ext's `default = ["wasm-host"]` stays on regardless. The never-compiled `not(wasm-host)` arms have **multiplied** since this item was filed: `facade.rs:441-443` (`materialize_guest_tools`), `:899-908` (`render_via_guest`), `:1261-1264` (`run_shortcut`).

**upstream** — no analog; pi has no compile-time feature tiers. This is a cyrup-original invariant (`wasm-host` is documented as an opt-*out* slimming switch) that does not hold.

**Impact** — the advertised slim build is unbuildable, and the `not(wasm-host)` code paths accumulate without ever being type-checked, so they will not work when the gate is finally fixed.

**Fix** — gate `mod host_services;` and its `cyrup_ext::caps`/`cyrup_ext::host` imports on `wasm-host`; declare `cyrup-ext` in the workspace with `default-features = false` and re-enable `wasm-host` from the crates that want it; then compile the `not(wasm-host)` arms at least once and add them to CI.

**Verify** — `cargo check -p cyrup-session-svc --no-default-features` succeeds and the resulting dependency graph contains no `wasmtime`.

## EXT-027 — pi's bundled llama.cpp router extension has no counterpart

**Kind** not-ported · **Severity** low · **Effort** L · **Confidence** high

**cyrup** — `grep -rli llama crates/` returns only incidental hits in `cyrup-provider`/`cyrup-config` (`auth/helpers.rs`, `utils/overflow.rs`, `providers/together.rs`, `api/openai_completions.rs`, `tests/catalog_data.rs`, `cyrup-config/src/login.rs`). There is no bundled-extension tier in cyrup at all.

**upstream** — `pi/packages/coding-agent/src/extensions/` holds `index.ts` plus `llama/{client,huggingface,index,provider,ui}.ts` at v0.83.0, and it kept moving — `git diff --stat v0.83.0..v0.84.1 -- packages/coding-agent/src/extensions/` shows `llama/index.ts` +11 and `llama/provider.ts` +28.

**Impact** — no local llama.cpp routing out of the box. Lowest value in this area.

**Fix** — only worth doing if a bundled-extension tier is wanted; it would be the first. Needs a shipped-with-binary extension registration path alongside `NativeExtension`.

**Verify** — n/a until the tier exists. *Carried forward on prior evidence*: the llama source itself has still not been read on either pass.

## EXT-028 — Both world.wit copies still declare `cyrup:ext@0.3.0` on line 1 while the package line is 0.4.0

**Kind** stale-port · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — the four halves this item was filed for are **closed** (see the status table). What survives is the item's own subject applied to itself: `crates/cyrup-ext/wit/world.wit:1` and `crates/cyrup-ext-sdk/wit/world.wit:1` both read `// cyrup:ext@0.3.0 — the versioned WIT world (arch-08 §4.1).` while line 18 of the same file reads `package cyrup:ext@0.4.0;` and `crates/cyrup-ext/src/manifest.rs:69` reads `HOST_WORLD = "cyrup:ext@0.4"`. `manifest.rs:14` likewise still says "WIT world compatibility, e.g. `cyrup:ext@0.3`". The tie test `host_world_matches_the_wit_package_version_in_both_copies` (`crates/cyrup-ext/tests/wit_world_sync.rs:71-89`) parses only the `package` line, so nothing guards the header marker — and since the file also went 0.2→0.3, that marker has now rotted through **two** bumps.

**upstream** — no analog; pi has no compiled ABI. The invariant being violated is cyrup's own, stated at `manifest.rs:41-54`.

**Impact** — doc-only, but on the one file whose versioning discipline is this item's entire subject. A reader checking "which world does this file declare?" gets 0.3.0 from line 1 and 0.4.0 from line 18, and the guard that exists cannot see the disagreement. Exactly the rot class EXT-028 was filed against.

**Fix** — correct line 1 in **both** copies and `manifest.rs:14`, then extend `wit_world_sync.rs:71-89` to parse the header comment as well as the `package` line and assert all three agree with `HOST_WORLD`. Any version string in the file that the test does not read will rot again.

**Verify** — `grep -n '0\.3' crates/cyrup-ext/wit/world.wit crates/cyrup-ext-sdk/wit/world.wit crates/cyrup-ext/src/manifest.rs` returns nothing but the intentional bump-history lines, and the extended test fails if either copy's header is edited alone.

## EXT-032 — p3_no_human_wait_is_still_budget_contained asserts an uncontrollable wall-clock bound

**Kind** test-defect · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `assert!(elapsed < Duration::from_millis(300), "budget-contained near 80ms, took {elapsed:?}");` at `crates/cyrup-ext/tests/native_dispatch.rs:877`, on a `Dispatcher::with_budget(Duration::from_millis(80))` (`:863`) against a 400ms sleep (`:858`) — 220ms of scheduling headroom under a full parallel `cargo test`. It is strictly redundant with the deterministic outcome assertion at `:878-884`, which already rejects the not-budget-fired case by requiring the reason to contain "Extension failed, blocking execution". The timing assertion can therefore only fail spuriously.

**upstream** — no analog; a cyrup test-suite defect of the class the project keeps finding.

**Impact** — a flaky failure under load that blames the dispatcher budget, eroding trust in the suite. It proves nothing the surrounding assertions do not already prove deterministically.

**Fix** — delete the assertion at `native_dispatch.rs:877`. If a timing bound is wanted for documentation, use the defensive 2s form already used at `:710`.

**Verify** — the test still **fails** if `fails_closed` or the budget watchdog is reverted, and passes under `cargo test -- --test-threads=1` and under a loaded parallel run alike.

## EXT-036 — The event-catalog provenance comments are stale in four places and contradict each other

**Kind** stale-port · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — **corrected this pass**: the prior audit declared the `world.wit` half closed. It is not; four stale sites survive, two of them inside the header the count test *does* guard. (1) `crates/cyrup-ext/src/event.rs:57-58` still reads "1:1 with Pi's 31-event catalog (extensions/types.ts:1133-1171 + `agent_settled` at :1225)" — the 1:1 claim is false and the range is wrong; `event.rs:100` repeats the same range. (2) `crates/cyrup-ext/wit/world.wit:89`, inside `interface events`, still reads "All 30 Pi events (extensions/types.ts:1133-1171) are represented here" — contradicting the file's own header 81 lines above, in **both** copies. (3) The header itself (`world.wit:8-11`) cites "extensions/types.ts:1198-1239, 33 overloads": the real overload block is `types.ts:1190-1231` @v0.83.0 and `:1203-1244` @v0.84.1, so the cited range corresponds to **no version**. (4) The version marker on line 1 is EXT-028's residual, tracked there. `the_header_event_count_matches_the_declared_event_exports` (`crates/cyrup-ext/tests/wit_world_sync.rs:96-129`) ties the *number* and nothing else, which is why a fabricated citation passed straight through an audit that quoted it as evidence.

**upstream** — the `on(event: "…")` overload block is `pi/packages/coding-agent/src/core/extensions/types.ts:1190-1231` @v0.83.0, 33 overloads (re-derived by hand this pass and confirmed). Enumerating them against `EventKind::name()` (`event.rs:104-134`) leaves exactly `before_provider_headers` (EXT-009) and `session_info_changed` (EXT-011) — no third gap and no cyrup-invented event.

**Impact** — these comments are the primary provenance marker a reader consults to decide whether the event catalog is complete, and they assert a completeness that does not hold, in places that disagree with each other on the number and cite a range that matches no upstream version. Under this project's rules an in-tree pi citation *is* the guarantee that a port matches upstream; a fabricated range is worth as much as no range.

**Fix** — rewrite `event.rs:57-58` and `:100` to state the real position (31 of pi's 33, `extensions/types.ts:1190-1231` at the ported tag), naming the two outstanding events and cross-referencing EXT-009 / EXT-011; fix `world.wit:89` and the header range at `:8-11` in **both** copies. Then extend `wit_world_sync.rs:96-129` to validate the cited **range** against the tag as well as the count — otherwise the next citation rots the same way.

**Verify** — grep the crate for `1133-1171`, `1198-1239` and `30-event`; confirm no occurrences remain in either `world.wit` copy or in `event.rs`, and that the stated missing-event list matches a diff of pi's `on`-event names against `EventKind::name()`.

## EXT-037 — ext-tools.get-commands returns bare extension command names only

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-ext/src/host/live.rs:760-766` builds `guest.registry.command_names()` (`crates/cyrup-ext/src/registry.rs:656-658`, a `HashMap::keys()` walk) and maps each to `json!({"name": n})`. A guest therefore gets: extension commands only, **raw** names (never `name:N`), no `description`, no `source`, no `sourceInfo`, and a nondeterministic order. cyrup already builds the correct shape elsewhere — `AgentSession::slash_command_catalog` (`crates/cyrup-session-svc/src/session.rs:2275-2310`) emits `{name, description, source, sourceInfo}` for extension commands, prompt templates **and** skills — but `ext-tools.get-commands` (`crates/cyrup-ext/wit/world.wit:516`) does not route through it and `GuestServices` exposes no accessor for it.

**upstream** — `getCommands()` returns `[...extensionCommands, ...templates, ...skills]`, where extension commands map to `{name: command.invocationName, description, source: "extension", sourceInfo}` (`pi/packages/coding-agent/src/core/agent-session.ts:2332-2354` @v0.83.0); the type is `SlashCommandInfo` at `core/slash-commands.ts:6-11`, declared on the API at `extensions/types.ts:1329`.

**Impact** — a guest calling `pi.getCommands()` cannot render a command palette, cannot show descriptions, cannot see prompt templates or skills at all, and cannot invoke a colliding second `deploy` because it is handed the raw name. Rated **low**, not medium: this is pure information loss on a guest-only introspection API, no WASM guest ships, and cyrup's natives use a different path entirely. EXT-017 is the same defect on a path a user sees today, which is where the medium sits.

**Fix** — add a `commands()` accessor to `HostServices` (`crates/cyrup-ext/src/host/services.rs`, beside `active_tools`) returning the `slash_command_catalog()` rows; implement it in `LiveHostServices` (`crates/cyrup-session-svc/src/host_services.rs`) by delegating to `AgentSession::slash_command_catalog`; have `get_commands` (`host/live.rs:760`) prefer the live backend and fall back to `registry.resolved_commands()` (`registry.rs:462`) emitting `{name: r.invocation_name, description, source: "extension"}` when no session is attached. This is the live-source-with-registry-fallback pattern `get_active_tools` already uses at `host/live.rs:736-743`.

**Verify** — register two extension commands both named `deploy`, one prompt template and one skill; assert a guest's `get_commands()` returns four rows in load order with `deploy:1`/`deploy:2`, non-empty descriptions, and `source` values covering extension/prompt/skill. Add to `crates/cyrup-ext/tests/aggregation.rs` beside the existing `resolved_commands` assertions at `:227-238`.

## EXT-040 — register-shortcut's description is discarded, so /hotkeys shows a raw key-id

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `async fn register_shortcut(&mut self, key: String, _desc: String)` at `crates/cyrup-ext/src/host/live.rs:98-101` explicitly discards the description and calls `guest.registry.register_shortcut(guest.owner.clone(), key)`; the registry method (`crates/cyrup-ext/src/registry.rs:515-523`) takes only `(owner, key)` and `shortcuts` is `HashMap<String, ExtensionId>` (`registry.rs:144`), so there is nowhere to put it. `ExtensionHost::shortcut_keys()` returns bare ids (`facade.rs:1233-1235`) and the `/hotkeys` table falls back to printing the key id as its own label — `let label = spec.description.as_deref().unwrap_or(spec.id.as_str());` at `crates/cyrup-tui/src/app.rs:2158`, whose surrounding comment (`:2151-2157`) names this exact drop at `live.rs:98`. The WIT already carries the field: `register-shortcut: func(key: string, desc: string);` (`crates/cyrup-ext/wit/world.wit:274`).

**upstream** — `ExtensionShortcut { shortcut: KeyId; description?: string; handler; extensionPath: string }` at `pi/packages/coding-agent/src/core/extensions/types.ts:1524-1529` @v0.83.0, registered via `registerShortcut(shortcut, {description?, handler})` at `:1250`. The `/hotkeys` Extensions table renders `const description = shortcut.description ?? shortcut.extensionPath;` at `modes/interactive/interactive-mode.ts:5856`.

**Impact** — `/hotkeys` lists extension bindings as `ctrl+alt+f | ctrl+alt+f` instead of `Ctrl+Alt+F | Show the subagent fleet`. The value crosses the WIT boundary and is thrown away one line inside the host, and there is no fallback to the extension id either, which pi has.

**Fix** — widen `shortcuts` to `HashMap<String, (ExtensionId, Option<String>)>` (`registry.rs:144`), thread `desc` through `register_shortcut` (`registry.rs:515`) and the host import (`host/live.rs:98`), and add `shortcut_specs() -> Vec<(String, Option<String>)>` on the facade beside `shortcut_keys()` (`facade.rs:1233`). The TUI already accepts the richer form — `ShortcutSpec`'s `From<(String, Option<String>)>` impl at `crates/cyrup-tui/src/app.rs:2189-2193` exists for precisely this — so the only other edit is `crates/cyrup/src/main.rs:1653`. Fall back to the extension **id**, not the key, when `desc` is empty. Land with EXT-039 and EXT-035, which rewrite the same registrar.

**Verify** — a guest registering `ctrl+t` with description "Toggle X": assert `/hotkeys` renders `Toggle X` in the Action cell. Extend `crates/cyrup-tui/tests/extension_shortcut.rs`.

## EXT-042 — model_select buries previousModel/source, and thinking_level_select drops previousLevel

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `HostEvent::ModelSelect { model: Value }` (`crates/cyrup-ext/src/event.rs:329`) and `ThinkingLevelSelect { level: String }` (`:330`); WIT `on-model-select: func(model-json: string);` and `on-thinking-level-select: func(level: string);` (`crates/cyrup-ext/wit/world.wit:206-207`). The producer nests two of pi's three top-level model fields **inside** `model`: `crates/cyrup-session-svc/src/session.rs:2757-2761` builds `{"provider", "id", "previousModel": {…}, "source": source}` and passes the whole object as the `model` payload at `:2766`. `previousLevel` has no producer at all — `session.rs:3119` dispatches `ThinkingLevelSelect { level: level_str }` and nothing else, even though the previous level is in scope a few lines earlier at the no-op guard, which makes the fix trivial. The SDK's decoded types confirm neither is reachable (`crates/cyrup-ext-sdk/src/events.rs:115-117`, `:121-123`).

**upstream** — `ModelSelectEvent { type, model, previousModel, source }` at `pi/packages/coding-agent/src/core/extensions/types.ts:793-798` and `ThinkingLevelSelectEvent { type, level, previousLevel }` at `:801-805` @v0.83.0 — three and three sibling fields respectively.

**Impact** — a pi extension ported to cyrup reads `event.previousModel` / `event.source` and gets `undefined`; they exist one level down inside `event.model`, which is also where `provider`/`id` live, so `model` is not a `Model` shape either. `previousLevel` cannot be recovered at all, so a handler that wants to react only to an *increase* in thinking level must keep its own shadow copy and will be wrong on the first event of a session.

**Fix** — widen both variants (`event.rs:329-330`) to `ModelSelect { model, previous_model: Option<Value>, source: String }` and `ThinkingLevelSelect { level, previous_level: Option<String> }`, widen the two WIT exports in both copies, un-nest at `session.rs:2757-2766`, and capture the pre-change level before it is overwritten so it can be passed at `:3119`. Add the fields to the SDK event structs (`events.rs:115`, `:121`). Export re-signing ⇒ bump `HOST_WORLD`; batch with the other ABI items.

**Verify** — switch model twice and thinking level twice; assert a subscribed guest sees `previous_model`/`source` as top-level fields and a `previous_level` that differs from `level` on the second change.

## EXT-045 — ctx.scopedModels and ctx.signal are unreachable, and EXT-005's CYRUP-DELTA notes were never written

**Kind** not-ported · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `grep -n 'CYRUP-DELTA\|scopedModels\|scoped_models\|ctx.signal' crates/cyrup-ext/src/lib.rs` returns **zero** at HEAD: the two provenance notes EXT-005's closure explicitly required ("Two CYRUP-DELTAs should be recorded in `cyrup-ext/src/lib.rs`") do not exist. Both gaps are still live. `scopedModels`: `interface models` (`crates/cyrup-ext/wit/world.wit:362-372`) has list-models / current / set-model / context-usage / thinking-level / set-thinking-level and no scoped accessor, while the host-side value exists and is maintained (`AgentSession::scoped_models`, `crates/cyrup-session-svc/src/session.rs:3771-3773`). `signal`: the only cancellation a guest can observe is per-call — `host-tool.is-cancelled: func(call-id: string) -> bool` (`world.wit:486`) — so a non-tool handler cannot ask whether a run is in flight or be woken when it aborts.

**upstream** — `scopedModels: readonly ScopedModel[]` at `pi/packages/coding-agent/src/core/extensions/types.ts:326` ("Same set the `/scoped-models` command shows") and `signal: AbortSignal | undefined` at `:334` ("The current abort signal, or undefined when the agent is not streaming") @v0.83.0 — both on the **base** `ExtensionContext`, supplied on every context pi builds.

**Impact** — small on its own: a guest cannot offer a model picker restricted to the session's scoped set, and cannot cooperatively bail out of long non-tool work when the run aborts. The larger cost is the missing note — EXT-005 was closed on the promise that these would be written into the source, and they were not, so the next reader of `crates/cyrup-ext/src/lib.rs` sees a `ctx-state` block claiming to mirror pi's `ExtensionContext` with nothing marking what it omits. That is exactly the mechanism (README structural blind spot 1) by which a gap becomes invisible.

**Fix** — two import-only WIT additions, no `HOST_WORLD` bump (`manifest.rs:51-54`): `scoped-models: func() -> string;` on `interface models` (`world.wit:362`) fed by `AgentSession::scoped_models` through a new `HostServices` accessor, and `is-run-cancelled: func() -> bool;` on `interface ctx-state` (`world.wit:523`) fed by the session cancel token — the run-scoped analog of the existing per-call `host-tool.is-cancelled`. If `signal` is genuinely deferred, **write the CYRUP-DELTA this time**, naming `types.ts:334` and the reason (a live `AbortSignal` is not expressible as a component value).

**Verify** — a guest asserting `models.scoped_models()` matches `/scoped-models`, and that a long-running non-tool handler observes `is-run-cancelled()` flipping after an abort. Grep `lib.rs` and assert a delta note exists for anything left unported.

## EXT-046 — session.set-label cannot clear a label

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `set-label: func(entry-id: string, label: string);` (`crates/cyrup-ext/wit/world.wit:359`), with no `option<>`. The host import is `async fn set_label(&mut self, entry_id: String, label: String)` (`crates/cyrup-ext/src/host/live.rs:355-361`), forwarding to `HostServices::set_label(&self, entry_id: &str, label: &str)` (`crates/cyrup-ext/src/host/services.rs:478`), whose live impl always sets: `mgr.append_label(&EntryId::from(entry_id), Some(label))` at `crates/cyrup-session-svc/src/host_services.rs:1071`. The clearing primitive already exists one layer down — `append_label` takes `Option<&str>` — so only the two signatures block it.

**upstream** — `setLabel(entryId: string, label: string | undefined): void;` at `pi/packages/coding-agent/src/core/extensions/types.ts:1314` @v0.83.0, documented "Set or clear a label on an entry. Labels are user-defined markers for bookmarking/navigation."

**Impact** — an extension that labels entries (bookmarking or review markers, the documented use) can never remove one. An empty string does not clear it either — it writes an empty label — so the entry keeps a marker the user cannot get rid of through the extension that created it.

**Fix** — change the WIT to `set-label: func(entry-id: string, label: option<string>);` in both copies (`:359`), widen the host import (`host/live.rs:355`) and the `HostServices::set_label` signature to `Option<&str>`, and pass it straight through to `append_label`, which already accepts it. Mirror in the SDK's `Ctx::set_label`. This is an **import re-signing**, which per `manifest.rs:51-54` **does** require a `HOST_WORLD` bump (a 0.4 guest imports the old two-string shape) — batch with the other ABI items.

**Verify** — a guest setting then clearing a label on an entry; assert the tree shows the label and then no label. Extend `crates/cyrup-ext/tests/wasm_component.rs`.

## EXT-048 — The dialog timeout key is `timeoutMs`; pi's field is `timeout`, and the in-tree citations are wrong

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — cyrup reads `timeoutMs`: `DialogOptions.timeout_ms` with serde camelCase (`crates/cyrup-ext-sdk/src/ctx.rs:472`, builder `:495-497`, lowered `:507`), consumed as `opts.timeout_ms` at `crates/cyrup-session-svc/src/host_services.rs:515`. The provenance comments assert this **is** pi's shape and are wrong twice over: `crates/cyrup-ext/wit/world.wit:312-313` (both copies) says "`opts-json` is the Pi `ExtensionUIDialogOptions` bag (types.ts:89): `{timeoutMs, signal}`", and `crates/cyrup-session-svc/src/host_services.rs:104` repeats it — and `types.ts:89` at v0.83.0 is a blank line before the UI Context banner.

**upstream** — `export interface ExtensionUIDialogOptions { signal?: AbortSignal; timeout?: number; }` at `pi/packages/coding-agent/src/core/extensions/types.ts:95-100` @v0.83.0, with `timeout?: number` on `:100`, documented "Timeout in milliseconds. Dialog auto-dismisses with live countdown display." `git grep -n timeoutMs v0.83.0 -- packages/coding-agent/src` returns only unrelated startup-ui / http-dispatcher / package-manager hits, so there is no wire variant using `timeoutMs` either.

**Impact** — small today because cyrup's own SDK is the only producer, so both halves agree. It becomes real the moment anything else writes the bag — a hand-written guest, a `custom()` overlay spec, or an RPC-side dialog forwarder — because `{timeout: 5000}` is silently ignored and the dialog gets the fallback ceiling instead of the author's bound, with no error. The immediate harm is the false provenance: two comments cite a pi line for a field name that does not exist there, and this project treats in-tree pi citations as the guarantee that a port matches upstream.

**Fix** — accept both keys behind `crates/cyrup-session-svc/src/host_services.rs:515` (serde `alias = "timeout"` on `DialogOptions::timeout_ms`, `crates/cyrup-ext-sdk/src/ctx.rs:472`, with `timeout` as the canonical name), and correct the two citations at `world.wit:312-313` (both copies) and `host_services.rs:104` to name pi's real field (`{timeout, signal}`, `types.ts:95-100`) plus a one-line note that `timeoutMs` is accepted for back-compat. Comment-only in `world.wit`, so no `HOST_WORLD` bump.

**Verify** — deserialize `{"timeout": 5000}` and `{"timeoutMs": 5000}` into `DialogOptions` and assert both yield 5000; grep the crate for `timeoutMs` cited as a pi field name and confirm none remain.

## EXT-050 — pi.events gained stale-context guarding and tracked unsubscribe; cyrup's bus has neither

**CLOSED 2026-08-15 — verified already done at HEAD.** Both halves are live: `bus.unsubscribe` (WIT + `SharedBus::unsubscribe`), `GuestState::assert_active` consulted by `bus_subscribe` AND `bus_emit`, and `GuestState::invalidate` running `unsubscribe_all` for the owner. The item's blocker — that the only teardown trigger was the production-dead `reload` — is also gone: `facade.rs::invalidate_live` is driven from the live session dispose path in `cyrup-session-svc/src/session.rs`, pinned by `tests/dispose_invalidates.rs`.

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** high

**cyrup** — the WIT bus is emit + subscribe only, with no unsubscribe and no handle: `emit: func(topic: string, payload-json: string);` and `subscribe: func(topic: string);` (`crates/cyrup-ext/wit/world.wit:475-477`). `SharedBus` (`crates/cyrup-ext/src/host/services.rs`, `emit` at `:985-989`) keeps a subscriber table and a pending queue with a single blunt reset — `self.bus.clear()` inside `ExtensionHost::reload` (`crates/cyrup-ext/src/facade.rs:1288`) — which is all-or-nothing and, because `reload` is production-dead (EXT-025; sole caller `crates/cyrup-ext/tests/discover_load.rs:118`), never runs in production. There is no per-extension staleness concept anywhere in `crates/cyrup-ext/src/`.

**upstream** — `events` stopped being the bare bus at v0.84.1 and became a guarded wrapper: `events: { emit(channel, data) { runtime.assertActive(); eventBus.emit(channel, data); }, on(channel, handler) { runtime.assertActive(); return runtime.trackEventBusSubscription(eventBus.on(channel, handler)); } }` (`pi/packages/coding-agent/src/core/extensions/loader.ts:413-421`). The runtime side added `eventBusUnsubscribers` (`:179`) and `trackEventBusSubscription` (`:215-225`), and `invalidate()` now runs every tracked unsubscribe and clears the set (`:206-214`); the interface field is at `extensions/types.ts:1610`.

**Impact** — two behaviours cyrup cannot express. (1) A guest cannot stop listening on a topic — `bus.subscribe` is permanent for the instance's life, so an extension that listens only while a mode is active keeps receiving and must filter by hand. (2) A subscription registered by an extension whose context went stale (session replacement or reload) is not torn down and its emit is not refused; upstream now throws on both. Low today because the bus is wasm-only and cyrup ships zero WASM guests (EXT-018), but this is the shape the rest gets built on — fixing EXT-018/EXT-034 without it bakes in a leak.

**Fix** — add `unsubscribe: func(topic: string);` to `interface bus` (`world.wit:468-478`, both copies) — an additive **import**, no `HOST_WORLD` bump — backed by a removal on `SharedBus`. Then tie teardown to instance lifetime: drop an extension's subscriptions when its `LiveExtension` leaves `ExtensionHost::live` (it enters at `facade.rs:1094-1096`) rather than relying on the whole-bus `clear()`, and refuse an emit/subscribe from an extension no longer in the live map — cyrup's structural analog of `assertActive`. Do this in the same change as EXT-018's un-cfg-gating so natives get the same lifecycle.

**Verify** — a guest that subscribes, unsubscribes, and asserts it stops receiving; and a guest whose instance is dropped mid-session, asserting its `bus-deliver` is no longer invoked and a later emit from it is refused rather than queued. Extend `crates/cyrup-ext/tests/wasm_bus_flag.rs`.

## EXT-051 — Extension provider OAuth drifted: refreshToken gained a signal, oauth gained isSubscription

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — the export takes no cancellation argument: `provider-refresh-token: func(id: string, credentials-json: string) -> result<string, string>;` (`crates/cyrup-ext/wit/world.wit:102`), invoked from the provider block of `crates/cyrup-ext/src/host/live.rs`. For the second half, **the auditor's grep was wrong and the item is narrower than filed**: cyrup already has the concept — `fn is_subscription(&self) -> bool` on the auth trait (`crates/cyrup-provider/src/auth/mod.rs:101`, overridden in `anthropic.rs:662`, `github_copilot.rs:813`, `kimi_coding.rs:704`, `openai_codex.rs:1030`, `xai.rs:631`, all citing pi v0.84.1) — and `ProviderConfig.oauth` is `Option<Value>`, an opaque JSON blob (`crates/cyrup-ext/src/provider.rs:38-40`), so a guest's `isSubscription` key **already crosses the seam untyped**. What is missing is the typed field and a consumer, not the transport.

**upstream** — both changes are visible in `git diff v0.83.0..v0.84.1 -- .../extensions/types.ts`: `refreshToken(credentials: OAuthCredentials, signal: AbortSignal)` at `types.ts:1481` (one-argument at v0.83.0), and a new `isSubscription?: boolean` on the `oauth` block at `:1475` documented "Whether access through this auth method is backed by a provider subscription."

**Impact** — a guest provider's token refresh cannot be cancelled, so a refresh against a hung auth endpoint runs to whatever the epoch budget allows rather than aborting with the run; the failure mode is a wedged login-adjacent path during shutdown. `isSubscription` reaches the host as an untyped key that nothing reads, so an extension-supplied subscription provider is presented like a metered API-key one.

**Fix** — for the signal, prefer cyrup's existing poll idiom over re-signing: leave the export alone and let the guest poll the new `ctx-state.is-run-cancelled` import proposed in EXT-045 (additive import, no bump). For the metadata, add a typed `is_subscription: bool` to the `oauth` block of `ProviderConfig` (`crates/cyrup-ext/src/provider.rs:17-42`) and thread it into the auth surface that already models the concept in `crates/cyrup-provider/src/auth/`.

**Verify** — a guest provider whose refresh blocks: assert an aborted run stops it rather than waiting out the epoch budget. Assert an extension provider declaring `isSubscription` is described as subscription-backed in the auth listing, the same way the built-in providers are.

## EXT-053 — An extension command shadowing a built-in is dropped from autocomplete with no diagnostic

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `CommandRegistry::with_dynamic` (`crates/cyrup-tui/src/commands.rs:166-178`) builds a `HashSet` of the builtin names and pushes a dynamic command only `if !existing.contains(cmd.name.as_ref())`. The drop is silent: nothing is recorded, nothing reaches `startup_diagnostics.extensions` (the channel `crates/cyrup-session-svc/src/builder.rs:775-786` and `facade.rs:1158-1162` use for load and conflict diagnostics), and there is no second chance at a suffixed name because `slash_command_catalog` emits the bare `name` (`crates/cyrup-session-svc/src/session.rs:2277`) — EXT-017. The comment at `commands.rs:169-171` justifies the drop by citing pi's suffixing rule, but cyrup implements neither the suffix nor the warning.

**upstream** — `getBuiltInCommandConflictDiagnostics` (`pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:529-543` @v0.83.0) walks `getRegisteredCommands()`, filters those whose `name` is a built-in, and emits one warning each: `Extension command '/x' conflicts with built-in interactive command. Skipping in autocomplete.` when `invocationName === name`, or `… Available as '/x:2'.` when it was suffixed; those diagnostics are pushed into the `[Extension issues]` startup panel at `:1610`. The autocomplete filter itself (`:603`) is the same drop — but never without the message.

**Impact** — an author who names a command `/model` or `/settings` sees it vanish from the `/` menu with no explanation and, unlike upstream, has no `/model:2` fallback to reach it. The `[Extension issues]` panel cyrup already renders for load faults stays empty for the one conflict class a user is most likely to create.

**Fix** — have `with_dynamic` (`commands.rs:166-178`) return the dropped names alongside the registry and surface them through the same panel the load diagnostics use, with pi's two message forms verbatim. Once EXT-017 lands and the catalog emits `invocation_name`, a shadowed command naturally becomes reachable as `/x:2` and the second message form becomes accurate rather than aspirational — schedule this immediately after EXT-017 and reuse its edit.

**Verify** — register an extension command named `settings`; assert it is absent from `/` autocomplete **and** that exactly one `[Extension issues]` warning naming it is shown. After EXT-017, assert it is reachable as `/settings:2` and that the message says so. Extend `crates/cyrup-tui/tests/commands.rs`.

## EXT-056 — register_tool_renderer is last-wins while every sibling table is first-wins

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `ExtensionRegistry::register_tool_renderer` (`crates/cyrup-ext/src/registry.rs:318-324`) is a bare `self.lock_write()?.tool_renderer_owner.insert(tool_name.into(), owner)`, with a doc at `:310-317` stating that the **last** registration wins. Every neighbour disagrees: `register_message_renderer` (`:335-342`) and `register_entry_renderer` (`:353-361`) both use `.entry(…).or_insert(owner)` — first-wins, citing pi's load-order resolution — and `register_tool` (`:217-224`) now rejects a foreign owner outright since EXT-008 closed. Reached in production from `InitApi`'s `tool_renderers` list via `facade.rs:265-299`.

**upstream** — pi has no separate tool-renderer table at all: `renderCall`/`renderResult` ride on the tool's own `ToolDefinition` (`pi/packages/coding-agent/src/core/extensions/types.ts:472-481` region) and are resolved by `getToolDefinition`, which returns the **first** extension whose `ext.tools` map has the name (`runner.ts:463-471` @v0.83.0). Whoever wins the tool wins its renderer.

**Impact** — under cyrup a later extension calling `register_tool_renderer("bash")` re-points rendering to an extension that lost — or never made — the tool registration, so the tool executes as one extension's and draws as another's. The doc's own justification ("matching `register_guest_tool`'s descriptor path") no longer holds, because that path now early-returns on a foreign owner before it touches `tool_renderer_owner`.

**Fix** — make `register_tool_renderer` (`registry.rs:318-324`) first-wins with `.entry(…).or_insert(owner)` like its two siblings, and record an `ExtensionConflict` (`registry.rs:251-256`) when a second owner claims the same tool name so the drop is diagnosable. Update the doc at `:310-317`, whose stated rationale is now stale.

**Verify** — two extensions registering a renderer for the same tool name: assert the first-loaded one renders and that exactly one conflict diagnostic names both. Add beside the renderer assertions in `crates/cyrup-ext/tests/entry_renderer.rs`.

## EXT-057 — deliver_bus_events silently drops queued events at the round bound, and listener faults never surface

**Kind** parity-bug · **Severity** low · **Effort** S · **Confidence** high

**cyrup** — `crates/cyrup-ext/src/facade.rs:1003-1027`: `const MAX_ROUNDS: usize = 64; for _ in 0..MAX_ROUNDS { let batch = self.bus.take_pending(); if batch.is_empty() { return; } … }`. When the bound is reached the loop simply falls out with events still sitting in `SharedBus.pending` — no diagnostic, no error, no record that anything was dropped, so a chatty (not even pathological) A→B→A topic pattern loses messages silently. Second half: a contained `bus_deliver` fault is only `tracing::warn!(extension, topic, error, "inter-extension bus delivery contained (skipped)")` (`facade.rs:1017-1024`) — it never goes through `add_error_listener`'s channel, so it cannot reach `App::show_extension_error` / the `[Extension issues]` surface that EXT-S03 wired.

**upstream** — `createEventBus()` (`pi/packages/coding-agent/src/core/event-bus.ts` @v0.83.0) is a node `EventEmitter` whose `emit` runs every listener synchronously: no queue, no round bound, nothing can be dropped. Its `on` wrapper does surface handler faults — `catch (err) { console.error(\`Event handler error (${channel}):\`, err); }`.

**Impact** — low today because no WASM guest ships, but this is the shape EXT-018, EXT-034 and EXT-050 will build on: an extension author whose bus messages vanish past round 64 gets no signal at all, and an extension whose bus listener throws is invisible in the front end that EXT-S03 exists to make faults visible in.

**Fix** — when `MAX_ROUNDS` is exhausted with a non-empty queue, emit an `ExtensionError` through the same `add_error_listener` channel naming the topic and the round bound, and drop the remainder explicitly rather than by falling out of a loop. Route the contained `bus_deliver` fault at `facade.rs:1017-1024` through that channel too, keeping the `tracing::warn!`. Land with EXT-034, which is the same function.

**Verify** — drive a two-guest ping-pong past 64 rounds and assert exactly one error reaches the listener naming the bound; make a subscriber's `bus-deliver` trap and assert the fault appears in the TUI transcript the way a handler fault does.

## EXT-N01 — `unread_stdout_from_a_bursty_child_is_bounded_not_unbounded` asserted a byte-exact cap the pump was never designed to hold

**Kind** test-defect · **Severity** high · **Effort** S · **Confidence** confirmed · **Status** fixed this pass

**cyrup** — `crates/cyrup-ext/src/caps/proc.rs` asserted `first <= MAX_PIPE_BUFFER_BYTES` and `second <= MAX_PIPE_BUFFER_BYTES` against the live buffer of a `yes` child. The pump's real invariant is one chunk looser: `PipeBufState::wait_for_room` returns as soon as `len < MAX_PIPE_BUFFER_BYTES` (`:283`), and `spawn_pump` then appends a full 8 KiB `read()` before re-checking (`:834-840`). The buffer can therefore legitimately sit anywhere in `[cap, cap + 8191]`, and which value a 500 ms sampler sees is pure scheduling. Observed both ways in consecutive full-workspace runs on an otherwise unchanged tree: green in one, and in the next `buffered stdout (16781628 bytes) exceeded the cap (16777216)` — an overshoot of **4412 bytes**, i.e. a partial chunk, which is the bounded behaviour the test exists to prove rather than the unbounded growth it reported.

**upstream** — none, and that is the classification. `ProcCaps` is a cyrup-original host capability for WASM guests; pi has no WASM host and no counterpart to `MAX_PIPE_BUFFER_BYTES`. The constant's own doc at `proc.rs:63` states the intent outright — "the point is FINITE, not a specific number" — so a byte-exact assertion pins a property the design never claimed. Per the repo rule this is case (b): the test asserted something it cannot control (the scheduler's interleaving of `wait_for_room` and `read`), not a divergence from upstream.

**Impact** — a scheduling-dependent red in `-p cyrup-ext --lib`, one of the two defects that made `cargo test --workspace` non-deterministic across back-to-back runs. Left alone it would have trained readers to re-run the gate until it went green, which is how a real regression gets waved through.

**Fix** — *applied.* Named the pump's chunk size as `PIPE_CHUNK_BYTES` (`const PIPE_CHUNK_BYTES: usize = 8192;`, used by `spawn_pump` in place of the inline literal) so the bound is derived from the code rather than a magic number, and asserted `<= MAX_PIPE_BUFFER_BYTES + PIPE_CHUNK_BYTES`. **Strengthened, not merely relaxed:** a third assertion now requires `second.abs_diff(first) <= PIPE_CHUNK_BYTES` — 500 ms of `yes` moves far more than one chunk, so a pump that was not genuinely parked cannot land within one chunk of where it already was. The plateau claim is now checked directly instead of being inferred from an absolute ceiling. No behaviour changed: the only production edit is the literal `8192` becoming a named const of the same value.

**Verify** — done. 6/6 consecutive green in isolation (3.45-3.46 s each) and green in the full-workspace run. Mutation-checked so the widened bound is not vacuous: short-circuiting `wait_for_room`'s guard to `if true || …` (removing backpressure entirely) fails the test at **56004586 bytes** against the new `16785408` bound. Mutation reverted.

## EXT-S04 — ctx.compact's onError path has no observable counterpart

**Kind** not-ported · **Severity** low · **Effort** M · **Confidence** high

**cyrup** — `customInstructions` is now ported end to end: `compact: func(opts-json: string) -> result<_, string>` (`crates/cyrup-ext/wit/world.wit:558`), host import at `crates/cyrup-ext/src/host/live.rs:829` (`ControlOp::Compact { custom_instructions }`), SDK `compact_with(&CompactOptions)` at `crates/cyrup-ext-sdk/src/ctx.rs:1087-1090`, consumer at `crates/cyrup-session-svc/src/session.rs:2862`. This is why `HOST_WORLD` moved 0.3→0.4 (`crates/cyrup-ext/src/manifest.rs:63-68`). The callbacks remain unported with an explicit rationale at `world.wit:554-557` — subscribe to `session_compact` instead — but that substitution is **incomplete**: `session_compact` fires only when a compaction produces an entry, so a compaction that *threw* has no observable counterpart at all.

**upstream** — `compact: (options) => { void (async () => { try { const result = await this.session.compact(options?.customInstructions); options?.onComplete?.(result); } catch (error) { … options?.onError?.(err); } })(); }` at `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:1819-1829` @v0.83.0. The `onError` branch fires on a compaction that threw — which produces no compaction entry and therefore no `session_compact` event.

**Impact** — an extension that asks for a compaction cannot learn that it failed, so it cannot sequence work after it, retry, or tell the user. It can observe success and cannot observe failure, which is the worse half of the pair to be missing.

**Fix** — either give `control.compact` a richer return that distinguishes "vetoed" and "errored" from "produced no entry", or add a `session_compact_failed` event to the catalog beside `session_compact` and dispatch it from the error path in `crates/cyrup-session-svc/src/session.rs`. The return-value route is import-only (no bump); the event route is an export addition (bump). Prefer the return value — the caller already awaits a `result<_, string>` and only the error text is being thrown away.

**Verify** — a guest calling `compact_with` against a session where compaction fails; assert the guest observes the failure rather than waiting forever for a `session_compact` that never arrives.

## EXT-059 — `AgentSession::load_wasm_extension` is a full-authority, manifest-less load reachable as public API

**Kind** cyrup-original · **Severity** medium · **Effort** S · **Confidence** confirmed · **filed 2026-08-13 from the EXT-054 fix**

**cyrup** — EXT-054 closed the DISCOVERY path: `load_discovered` (`facade.rs:1223`) now passes `disc.manifest.capabilities` to `load_wasm_with_caps`, so every extension that arrives from disk is capped by its own `extension.json`. `ExtensionHost::load_wasm` (`facade.rs:1078-1092`) deliberately keeps its three-argument signature as the host's OWN manifest-less entry point and applies `Capabilities::host_granted()` (`exec`/`net`/`ui` on, `fs` empty) — correct for a caller that is the host and has already decided, and it is why ~30 existing call sites did not have to change. But that entry point is re-exported one level up as `AgentSession::load_wasm_extension(id, bytes)` (`crates/cyrup-session-svc/src/session.rs:3956-3962`, `Ok(self.services.ext_host.load_wasm(id, bytes, services).await?)`), which is `pub` on the session type and takes RAW BYTES with no manifest — so any embedder holding an `AgentSession` can instantiate a component at full interactive authority with nothing to declare and nothing to review. Today its only callers are tests (`grep` at HEAD: eight `crates/cyrup-session-svc/tests/wasm_*.rs` files, no production caller).

**upstream** — no analog: pi has no capability model at all, and no byte-level extension load — `loadExtensions` takes PATHS (`extensions/loader.ts` @v0.83.0). This is a cyrup-original surface, so the question is what cyrup's own invariant should be, exactly as EXT-054 was.

**Impact** — narrow today (no production caller), but it is the second half of the door EXT-054 closed. An embedder who reads `manifest.rs`'s "capabilities a guest requests (granted subject to trust)" reasonably believes every guest is capped; this path caps none. Fail-open by construction, and unlike EXT-054 it cannot be reproduced through the binary, so it will not resurface in a live pass.

**Fix** — give `AgentSession::load_wasm_extension` a manifest (or a `Capabilities`) parameter and route it to `ExtensionHost::load_wasm_with_caps`; the eight test call sites are the only churn. If a byte-level unrestricted load must remain, rename it to say so (`load_wasm_extension_unrestricted`) so the authority is in the name rather than in a doc comment. **Not done in the EXT-054 pass: `crates/cyrup-session-svc/src/session.rs` and `crates/cyrup-session-svc/tests/**` were outside that pass's file ownership.**

**Verify** — the EXT-054 fixture, loaded through `AgentSession::load_wasm_extension` with an all-false declaration, must refuse `exec`/`net` the same way `crates/cyrup-ext/tests/manifest_capabilities.rs::exec_is_refused_when_the_manifest_denies_it` does through the discovery path. `the_manifest_less_load_keeps_the_host_grant` in that same file pins the CURRENT behaviour and must be updated, not deleted, when this lands.

## EXT-058 — Guest WASM `http-client` is not gated by `--offline`

**Kind** cyrup-original · **Severity** medium · **Effort** S · **Confidence** **confirmed — reproduced with a real WASM guest on an `--offline` host** · **observed 2026-08-13** (live-terminal; [`REPRO-LOG.md`](REPRO-LOG.md))

> **INVESTIGATED 2026-08-13 — code claim HOLDS, defect classification REFUTED. Do not "fix" this by
> wiring `--offline` into the guest gate.** The observation is exact: no code path connects the
> offline flag to the `http-client` imports (`grep -rn offline crates/cyrup-ext/src` → zero hits at
> HEAD). But the item's premise — that this diverges from what the flag promises — does not survive
> reading upstream. pi's own flag is documented, verbatim and to the character, as
> **"Disable startup network operations (same as PI_OFFLINE=1)"** (`packages/coding-agent/src/cli/args.ts:277`
> @v0.83.0), and cyrup's help text is the identical sentence with `CYRUP_OFFLINE` substituted
> (`crates/cyrup/src/cli.rs:936`). pi has NO extension network gate of any kind — its extensions run
> in-process with the host's whole authority (`extensions/loader.ts` @v0.83.0) — so an extension
> reaching the network on a `pi --offline` host is upstream's behaviour, not cyrup's divergence.
> `cyrup-config/src/policy.rs:53-62` already records the same finding for the model-catalog fetch
> ("upstream's only control is the offline switch", DRIFT-007). The Fix's own second branch — "if it
> is genuinely startup-only, say so in the flag's help text" — is therefore already satisfied,
> verbatim, and has been since the flag was written.
>
> **The control the operator actually lacked was EXT-054's, and it now exists.** An operator who
> wants a guest kept off the network declares `"net": false` in its `extension.json`, which as of
> today's fix is enforced host-side at the `http-client` import boundary
> (`crates/cyrup-ext/tests/manifest_capabilities.rs::net_is_refused_when_the_manifest_denies_it`).
> That is a per-extension, reviewable, deny-by-default control, which is strictly better than a
> process-wide flag for this purpose. Adding an offline intersection on top would be a
> cyrup-original divergence from pi's documented flag scope, and would silently break a guest
> legitimately using a LAN/localhost endpoint on a host started `--offline` — so it is not being
> added without a written decision. **Re-rated `medium` → `low`, kind `cyrup-original` →
> documentation/hardening, and left OPEN as a product decision rather than a repair.** Whoever picks
> it up owns the decision, not the code: the code is one `ExtensionHost::set_offline` setter away in
> either direction.

> **Filed 2026-08-13 from the `EXT-054` repro run.** Adjacent to `EXT-054` but **not covered by it**:
> `EXT-054` is about the manifest grant being inert, this is about the *second* control — the host's
> own offline flag — also not applying. They are separately fixable and the combination is the point.

**cyrup** — `grep -rn "offline" crates/cyrup-ext/src` at HEAD returns **nothing**: the extension host has no notion of the offline flag at all, so the guest `http-client` import in `crates/cyrup-ext/src/host/live.rs` cannot consult it. `--offline` is documented in the shipped help as "Disable startup network operations", so this may be within the letter of the flag — which is exactly why it is worth stating rather than assuming.

**upstream** — none. pi has no WASM guest model and no capability surface to gate, so there is no parity question here; this is a coherence question about **cyrup's own** two controls.

**Impact** — Measured: the host was launched with `--offline` **and** `--model faux/faux-1`, loading a guest whose manifest declared `{"net": false}`, and the guest still performed a live outbound HTTPS request:

```
 /httpdemo https://api.together.xyz/v1/models
 http status: 401 body: Missing API key        <-- a response from Together's real servers
```

Taken with `EXT-054`, **neither of cyrup's two controls stands between an installed guest and the internet**: not the per-extension capability grant, and not the process-wide offline flag. An operator who runs `--offline` on an air-gapped or policy-restricted host, having also written a deny-everything manifest, gets neither guarantee. Rated medium rather than high only because it requires an installed guest, which today means the in-tree SDK example — the same reachability `EXT-054` records.

**Fix** — Decide what `--offline` means for guests and implement it, rather than leaving it undefined. If it is process-wide (the reading an operator will assume), thread the flag into the extension host's service construction and have the `http-client` import return a typed offline error when it is set. If it is genuinely startup-only, say so in the flag's help text — "Disable startup network operations (does not restrict extensions)" — so the guarantee is not over-read. Land the enforcement route with `EXT-054`, which is threading the manifest through the same seam.

**Verify** — Load a guest with `{"net": true}` on a host launched `--offline` and assert its `http-client` call returns an offline error rather than reaching the network; the same guest on a host without `--offline` must succeed. The fixture already exists: `cargo build -p cyrup-ext-sdk --target wasm32-wasip2` and `-e <dir>`, then `/httpdemo`.

## EXT-061 — `ctx.getSystemPromptOptions()` has no counterpart, so a command can read the resolved system prompt but not the bag that built it

**CLOSED 2026-08-15 — ported end to end, with a live backend.** WIT `ctx-state.get-system-prompt-options -> result<string, string>` (command-tier, matching pi's placement on `ExtensionCommandContext`), `HostServices::system_prompt_options`, the host import, the NATIVE `HostCtx::system_prompt_options`, the SDK `CommandCtx::system_prompt_options`, and `LiveHostServices::system_prompt_options` off `PromptRebuilder` — pi's `_baseSystemPromptOptions` analog, the same structure the next rebuild consumes. No-backend answer is pi's own `{cwd}` default. **No `HOST_WORLD` bump** — see the `manifest.rs` history entry for why an additive import must not move it.

**Kind** not-ported · **Severity** medium · **Effort** S · **Confidence** confirmed · **filed 2026-08-14 (ext-rpc surface enumeration)**

**cyrup** — `grep -rn 'system_prompt_options\|getSystemPromptOptions\|system-prompt-options' crates/` returns **nothing** across the entire workspace at HEAD: no import in either `world.wit` copy, no method on `Ctx`/`CommandCtx` in `crates/cyrup-ext-sdk/src/ctx.rs`, no `CYRUP-DELTA` note, and no gap-analysis ID anywhere in `docs/gap-analysis/`. What cyrup does expose is `ctx-state.get-system-prompt` — the RESOLVED string, pi's `ctx.getSystemPrompt()` (`types.ts:346` @v0.83.0) — and nothing else. Near-miss worth recording because it is why this survived: `crates/cyrup-ext-sdk/src/ctx.rs:1336` cites `types.ts:355` for `fork_with_callback`, so the one line in the file that IS `getSystemPromptOptions` is already cited, for something else (that mis-citation is EXT-072 cluster C).

**upstream** — `pi/packages/coding-agent/src/core/extensions/types.ts:355 @v0.83.0` — `getSystemPromptOptions(): BuildSystemPromptOptions;`, documented at `:354` as *"Get the current base system-prompt construction options."*, declared on `ExtensionCommandContext` (`:353-387`). Also present as an action hook on the runtime surface: `getSystemPromptOptions?: () => BuildSystemPromptOptions;` at `:1634`. Upstream deliberately exposes the options bag NEXT TO the resolved string — `getSystemPrompt()` is `:346`, three members earlier on the base context.

**Impact** — this is the only genuinely unaccounted-for member of the whole 134-member extension API surface after the two `EditorFactory` members already owned by EXT-021/TUI-030. A command that wants to rebuild the system prompt with one option changed — pi's whole reason for putting the bag beside the string — has to reconstruct the bag by guesswork from the resolved text, which is lossy and silently wrong the moment cyrup's own prompt builder gains an option.

**Fix** — add `get-system-prompt-options: func() -> string` (a serialized `BuildSystemPromptOptions`) to `interface ctx-state` in **both** `world.wit` copies beside `get-system-prompt`, back it with the options value the session's prompt builder already holds, and surface it on `CommandCtx` in `cyrup-ext-sdk/src/ctx.rs` at the tier pi puts it (command-only, `ExtensionCommandContext`). New export ⇒ bump `HOST_WORLD`; batch it with any other WIT-widening item in this file.

**Verify** — a command-tier guest reads the options bag, flips one field, and asserts the value it read matches the one the host used to build the prompt `get-system-prompt` returns; assert an event-tier guest is refused, matching pi's placement on the command context.

## EXT-062 — `registration.add-autocomplete` is a cyrup-original registration call carrying no pi citation at any of its five sites

**CLOSED 2026-08-15.** Half of it was already done at HEAD (both `world.wit` copies, `registry.rs`, `api.rs`); this pass added the two remaining sites, `native.rs` and `host/live.rs`. `types.ts:1166` re-derived at v0.83.0.

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed · **filed 2026-08-14 (ext-rpc surface enumeration)**

**cyrup** — `add-autocomplete: func(command: string)` at `crates/cyrup-ext/wit/world.wit:461` (and the byte-identical `crates/cyrup-ext-sdk/wit/world.wit:461`), with implementations at `crates/cyrup-ext/src/registry.rs:887-895`, `crates/cyrup-ext/src/native.rs:378`, `crates/cyrup-ext/src/host/live.rs:223` and `crates/cyrup-ext-sdk/src/api.rs:683-686`. Every one of the five sites labels it `EXT-035 / R-08-021` — an internal requirement id — and **none names a pi line**.

**upstream** — pi's `ExtensionAPI` has no `addAutocomplete`. The nearest upstream thing is an OPTIONAL FIELD, not a call: `getArgumentCompletions?: (argumentPrefix: string) => AutocompleteItem[] | null | Promise<AutocompleteItem[] | null>;` on `RegisteredCommand` at `pi/packages/coding-agent/src/core/extensions/types.ts:1166 @v0.83.0`, which a command declares inline in its `registerCommand` options bag rather than through a second registration call.

**Impact** — not a defect on the merits: a WIT record cannot carry a closure, so the "this command HAS argument completions" flag must be declared separately, which is the identical argument already written down for `prepare-arguments` and `has-renderer`. The problem is that it is **undisclosed**. It reads as invented to the next auditor, and this is exactly the shape `working-start`/`working-stop` had before EXT-021 caught them carrying a fabricated citation. An undocumented invented surface is how divergence enters while everyone is looking at parity.

**Fix** — add a `CYRUP-DELTA` note at `world.wit:459-461` in **both** copies naming `types.ts:1166` as the field this call declares, stating the closure-to-export inversion in one sentence, and cross-referencing the `prepare-arguments` / `has-renderer` precedent. Mirror the note at `registry.rs:887` and `api.rs:683`. Comment-only; no ABI change.

**Verify** — `grep -n 'add-autocomplete' crates/cyrup-ext/wit/world.wit crates/cyrup-ext-sdk/wit/world.wit crates/cyrup-ext/src/registry.rs crates/cyrup-ext-sdk/src/api.rs` — every site names `types.ts:1166`; the two `world.wit` copies stay byte-identical.

## EXT-063 — `ManifestResources.agents` and the `cyrup` package.json key are cyrup-original additions to pi's four-key resource manifest

**CLOSED 2026-08-15.** The `[CYRUP-DELTA]` block now sits on `ManifestResources` naming the four ported pi keys, the cyrup-original fifth, and `pi`-wins-on-collision as an invariant; two tests pin the collision behaviour (labelled coverage — the behaviour was already correct).

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed · **filed 2026-08-14 (ext-rpc surface enumeration)**

**cyrup** — pi's `PiManifest` has four keys and all four are ported (see EXT-070). Bolted onto them are two cyrup-originals: `ManifestResources.agents` at `crates/cyrup-resources/src/package/manifest.rs:33-35`, and acceptance of a `cyrup` package.json key alongside `pi` at `:52`, resolved with `parsed.pi.or(parsed.cyrup)` — so `pi` wins on collision. Both are labelled in-code.

**upstream** — `pi/packages/coding-agent/src/core/extensions/loader.ts:561-566 @v0.83.0` declares `interface PiManifest` with exactly `extensions` / `skills` / `prompts` / `themes`; the field is read at `:572-573` (`if (pkg.pi && typeof pkg.pi === "object") return pkg.pi as PiManifest;`). There is no fifth resource family and no alias key.

**Impact** — low. `agents` is arch-SA §4.1 / R-SA-020 (subagent personas — pi has no equivalent resource family) and the `cyrup` key is a cross-harness alias with a deterministic precedence. Filed so that the invented half of the manifest surface is KNOWN rather than discovered by the next auditor diffing key counts, which is the whole point of tracking the `cyrup-original` class.

**Fix** — none required as a repair. What is owed is disclosure: a `CYRUP-DELTA` block on `manifest.rs` naming `loader.ts:561-566` as the ported four, `agents` as the cyrup-original fifth with its requirement id, and the `pi`-wins-on-collision rule as a stated invariant rather than an artefact of `.or()` ordering. **FIX SITE: `crates/cyrup-resources/src/package/manifest.rs` — area 05's crate, not area 06's.** Filed here because the finding came from the extension-manifest surface enumeration.

**Verify** — a package declaring both `pi` and `cyrup` keys with conflicting values resolves to the `pi` one, asserted rather than assumed; the doc block names all five keys and marks the fifth.

## EXT-064 — `ui.set-header`/`set-footer` take a `string` where pi takes a component factory, and `ReadonlyFooterDataProvider` has no analog

**PARTIALLY CLOSED 2026-08-15 — the row was stale.** The `[CYRUP-DELTA]` at the signature exists at HEAD in both copies and names both collapses. The live residual is only `ReadonlyFooterDataProvider`, which still has no analog.

**Kind** cyrup-original · **Severity** medium · **Effort** M · **Confidence** confirmed · **filed 2026-08-14 (ext-rpc surface enumeration)**

**cyrup** — `set-header: func(content: string)` and `set-footer: func(content: string)` at `crates/cyrup-ext/wit/world.wit:568-569` (both copies). Two collapses in one: the component factory becomes a flat string, and the empty string is overloaded to mean "restore the built-in".

**upstream** — `pi/packages/coding-agent/src/core/extensions/types.ts:183-187 @v0.83.0` — `setFooter(factory: ((tui: TUI, theme: Theme, footerData: ReadonlyFooterDataProvider) => Component) | undefined): void;` — and `:190` for `setHeader(factory: ((tui: TUI, theme: Theme) => Component) | undefined): void;`. Both are FACTORIES, re-invoked by the draw path; `undefined` (not an empty string) restores the built-in; and the footer factory is handed a `ReadonlyFooterDataProvider` carrying the git branch and the `setStatus` segments.

**Impact** — the string collapse is a recorded decision (TUI-033, closed as a `CYRUP-DELTA` in the gap-analysis doc) and is not what this item disputes. What is open is (a) the WIT itself carries **no** delta note at `:566-570` — only a citation-correction comment — so a reader of `world.wit` alone sees a signature that silently contradicts the pi line it cites, which is precisely the condition EXT-021 was filed for; and (b) `ReadonlyFooterDataProvider` has **no cyrup analog at all**, so an extension footer cannot show the git branch or read the status segments other extensions set, and there is nowhere in the tree that says so.

**Fix** — two parts, separable. Comment half (S): write the `CYRUP-DELTA` at `world.wit:566-570` in **both** copies, stating the factory→string collapse, the empty-string-restores overload, and the dropped `footerData` argument, citing `types.ts:183-187` and `:190`. Surface half (M): give the footer content a data channel — either a `ui.footer-data` import returning the branch + status segments as json, or extra `set-footer` parameters — so a guest can render what pi's factory is handed. The second half is an export re-signing ⇒ bump `HOST_WORLD`, and should batch with EXT-006's render-path widening.

**Verify** — a guest sets a footer and reads the branch/status data back, asserting it matches what the host's own footer renders; setting the empty string restores the built-in footer, asserted rather than assumed.

## EXT-065 — `add-autocomplete-provider` sits on `interface registration`, not `interface ui`, so the manifest `ui` grant does not gate it

**CLOSED 2026-08-15 — verified already done at HEAD** (landed with the 0.7 → 0.8 bump). The import is on `interface ui` and its impl goes through the GATED `ui_guest_of`, so the manifest grant reaches it.

**Kind** cyrup-original · **Severity** medium · **Effort** S · **Confidence** confirmed · **filed 2026-08-14 (ext-rpc surface enumeration)**

**cyrup** — `add-autocomplete-provider: func()` is declared on `interface registration` at `crates/cyrup-ext/wit/world.wit:464` (both copies), alongside `add-autocomplete` and the other `register-*` calls. The gate is verified absent at HEAD, not inferred: the `capabilities.ui` check is `ui_guest_of` (`crates/cyrup-ext/src/host/live.rs:86-90`, calling `GuestState::require_ui` at `host/services.rs:1520-1526`), and `grep -n require_ui crates/cyrup-ext/src/host/live.rs` returns **exactly one** hit — inside `ui_guest_of`, which is used only by `impl bindings::cyrup::ext::ui::Host for HostState` (`live.rs:244`). `add_autocomplete_provider` sits at `live.rs:228`, in the `registration::Host` impl, and calls the ungated `guest_of`.

**upstream** — `pi/packages/coding-agent/src/core/extensions/types.ts:225 @v0.83.0` — `addAutocompleteProvider(factory: AutocompleteProviderFactory): void;` — declared **inside `export interface ExtensionUIContext`** (`:131-282`), i.e. reached as `ctx.ui.addAutocompleteProvider(...)`, not off the `pi.*` registration object.

**Impact** — the placement is not cosmetic, because cyrup attached a capability model to the interface boundary that pi does not have. `capabilities.ui` in `extension.json` gates `interface ui`; `add-autocomplete-provider` is not in `interface ui`, so **an extension with no `ui` grant can still stack an autocomplete provider onto the core input editor.** Upstream's placement puts it squarely inside the UI surface. This is the same class of hole as EXT-054 and EXT-055 — a declared grant that does not reach a call the operator would reasonably expect it to cover — differing only in blast radius.

**Fix** — move the declaration into `interface ui` in **both** `world.wit` copies, next to the other editor-facing calls, so the existing `ui` grant check applies with no new enforcement code; update `cyrup-ext-sdk/src/api.rs` and the host impl to match. Moving an import between interfaces re-signs the world ⇒ bump `HOST_WORLD`. If it must stay on `registration` for ordering reasons, add an explicit `ui`-grant check at the host impl and say so in the WIT, so the gate exists somewhere.

**Verify** — extend `crates/cyrup-ext/tests/manifest_capabilities.rs` with a guest declaring `{"ui": false}` that calls `add-autocomplete-provider` and asserts it is refused, beside the existing `net`/`exec` refusal cases. RED before the move.

## EXT-066 — The live theme is the one theme a guest cannot read the colours of

**CLOSED 2026-08-15 — verified LIVE at both ends.** `ui.theme-get-json` exists AND `LiveHostServices` overrides the four theme accessors off `ThemeAccess` (SEAM-T01), so this is no longer a capability against a backend that answers `None` forever.

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed · **filed 2026-08-14 (ext-rpc surface enumeration)**

**cyrup** — `theme-get` returns `option<string>` — the theme NAME — documented at `crates/cyrup-ext/wit/world.wit:589-590` as *"the only part that survives the serde seam"*, citing `extensions/types.ts:266` (verified line-exact). But the sibling `theme-get-by-name` **does** return a serialized theme as json, so the seam evidently survives a whole `Theme` in the other direction.

**upstream** — `pi/packages/coding-agent/src/core/extensions/types.ts:266 @v0.83.0` — `readonly theme: Theme;`, a property holding the whole theme object, documented at `:265` as *"Get the current theme for styling."* An upstream renderer reads colours straight off it.

**Impact** — the asymmetry is undocumented and is the defect: a guest can serialize any theme it can name, except the one that is actually active. A renderer that wants to match the host's palette has to call `theme-get` for the name and then `theme-get-by-name` for the colours — two round trips, and racy if the theme changes between them — or, more likely, hard-code a palette. Pairs with EXT-006, whose fix must hand a theme to the render path anyway.

**Fix** — either add `theme-get-json: func() -> option<string>` returning the live theme in the same serialized shape `theme-get-by-name` already produces (new export ⇒ bump `HOST_WORLD`, batch with EXT-006), or — if the name-only return is to stand — rewrite the note at `world.wit:589-590` in **both** copies to say why `theme-get-by-name` may serialize a theme and `theme-get` may not, because as written the two lines contradict each other.

**Verify** — a guest reads the live theme's colours and asserts they equal `theme-get-by-name(theme-get())`, with the theme switched between two palettes mid-test.

## EXT-067 — `session_before_compact` and `session_before_tree` drop pi's `signal` payload field

**CLOSED 2026-08-15 — verified already done at HEAD.** Both signatures carry a `[CYRUP-DELTA] EXT-067` naming `signal` at `types.ts:601`/`:642` and the `ctx-state.is-run-cancelled` substitution.

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed · **filed 2026-08-14 (ext-rpc surface enumeration)**

**cyrup** — neither `on-session-before-compact` nor `on-session-before-tree` passes a cancellation channel, and neither WIT comment (`crates/cyrup-ext/wit/world.wit:397-403` and `:408-412`, both copies) mentions the omission — both enumerate the OTHER payload fields exhaustively, which is what makes the drop read as an oversight rather than a decision.

**upstream** — `pi/packages/coding-agent/src/core/extensions/types.ts:592-602 @v0.83.0` — `SessionBeforeCompactEvent`, with `signal: AbortSignal;` at `:601` — and `:639-643` — `SessionBeforeTreeEvent`, with `signal: AbortSignal;` at `:642`. Both are the cancellable-preparation events, and the signal is how a handler learns the operation it is preparing for was abandoned.

**Impact** — a handler doing real work in either event (summarizing, calling a model, walking the tree) cannot learn the operation was cancelled and keeps working against a run that is gone. `ctx-state.is-run-cancelled` (`world.wit:870-879`) is the plausible substitute — it is the poll-shaped port of `ctx.signal` — but nothing anywhere names it as the substitute for THESE two fields, so a guest author has no way to know it is the intended answer. Sibling of EXT-015, which covers the same class on the other four lifecycle events.

**Fix** — smallest correct change is the disclosure: extend both WIT comments in **both** copies to state that pi's `signal` field is deliberately expressed as the `ctx-state.is-run-cancelled` poll (the same `CYRUP-DELTA` already written at `world.wit:870-879`), citing `types.ts:601` and `:642`. If the poll is NOT sufficient — it is instance-scoped, not event-scoped — the alternative is an event-scoped cancellation token, which is a payload re-signing ⇒ bump `HOST_WORLD`, and should batch with EXT-015.

**Verify** — cancel a compaction while a guest's `session_before_compact` handler is mid-await and assert the guest observes the cancellation by whichever mechanism the fix chooses; same for `session_before_tree`.

## EXT-068 — `on-session-tree` carries one opaque `tree-json` blob and is the only event export with no comment and no pi citation

**CLOSED 2026-08-15 — verified already done at HEAD.** Parameter renamed `tree-json` → `event-json` (0.7 → 0.8) and the four-field payload documented against `types.ts:646-652` @v0.83.0.

**Kind** parity-bug · **Severity** medium · **Effort** S · **Confidence** confirmed · **filed 2026-08-14 (ext-rpc surface enumeration)**

**cyrup** — `on-session-tree: func(tree-json: string);` at `crates/cyrup-ext/wit/world.wit:413` (both copies). Every other event export in the file carries a comment naming its pi payload interface; this one carries none, and no `types.ts` citation. The parameter name suggests a session TREE, which is not what pi's event of this name carries.

**upstream** — `pi/packages/coding-agent/src/core/extensions/types.ts:646-652 @v0.83.0` — `SessionTreeEvent` has four fields: `newLeafId: string | null`, `oldLeafId: string | null`, `summaryEntry?: BranchSummaryEntry`, `fromExtension?: boolean`. It is a **leaf-transition** notification, not a tree dump.

**Impact** — nothing in the workspace states which of the two payloads the blob actually carries, so a guest author cannot write a handler against it without reading the host implementation, and a future reader cannot tell whether the port is correct. The `fromExtension` flag matters specifically: without it a tree-watching extension cannot distinguish a navigation it caused from one the user made, which is the standard re-entrancy guard.

**Fix** — decide and document. If the blob is the leaf transition, rename the parameter (`event-json`) and write the comment against `types.ts:646-652` naming all four fields; if any of the four is missing from what the host serializes, add it. If the blob really is a tree dump, that is a cyrup-original and needs a `CYRUP-DELTA` plus a separate answer for pi's four fields. A rename of a WIT parameter re-signs the world ⇒ bump `HOST_WORLD`.

**Verify** — navigate the session tree from the host and from an extension; assert the guest receives both leaf ids and can tell the two origins apart.

## EXT-069 — The three `on-tool-exec-*` exports abbreviate pi's `tool_execution_*` event names

**CLOSED 2026-08-15 — verified already done at HEAD.** All three are `on-tool-execution-*`, with a do-not-re-abbreviate note.

**Kind** cyrup-original · **Severity** low · **Effort** S · **Confidence** confirmed · **filed 2026-08-14 (ext-rpc surface enumeration)**

**cyrup** — `on-tool-exec-start` / `on-tool-exec-update` / `on-tool-exec-end` in `interface events`, both `world.wit` copies.

**upstream** — `pi/packages/coding-agent/src/core/extensions/types.ts:1223`, `:1224`, `:1225 @v0.83.0` — `tool_execution_start`, `tool_execution_update`, `tool_execution_end`. (The events themselves are mapped correctly and are not in question; EXT-014 owns their payload gap.)

**Impact** — harmless at runtime, but it is the **only** place in the 33-event mapping where the export name is not a mechanical kebab-case of pi's, so any name-based cross-check — the exact kind of script this surface enumeration and EXT-036's verify line both call for — reports three false gaps and has to be special-cased. Every such special case is a place a real gap can hide.

**Fix** — rename to `on-tool-execution-start` / `-update` / `-end` in **both** `world.wit` copies, `crates/cyrup-ext/src/event.rs`, `native.rs`, `host/live.rs` and the SDK's `export_extension!` arms. Export renames re-sign the world ⇒ bump `HOST_WORLD`; batch with any other WIT item here. If the abbreviation is kept instead, record it as a named exception in the header comment so a checker can encode it once.

**Verify** — a script mapping pi's 33 `on(event: "…")` names to kebab-case finds a matching `on-*` export for all 33 with no exception list.

## EXT-070 — The extension-manifest surface is split across two crates and `manifest.rs`'s doc comment conflates the two halves

**CLOSED 2026-08-15.** `cyrup-ext/src/manifest.rs`'s half was already done; this pass added the reciprocal pointer in `cyrup-resources/src/package/manifest.rs`.

**Kind** stale-port · **Severity** low · **Effort** S · **Confidence** confirmed · **filed 2026-08-14 (ext-rpc surface enumeration)**

**cyrup** — `crates/cyrup-ext/src/manifest.rs:8-10` presents `extension.json` as the analog of pi's `pi` package.json field. The two share **zero keys**: cyrup's is `{id, version, world, entry, capabilities{fs, exec, net, ui}}` (`manifest.rs:15-25`, `:40-51`) — identity, WIT version and sandbox grant; pi's is `{extensions, skills, prompts, themes}` — resource paths. pi's four keys **are** fully ported, in a different crate — `crates/cyrup-resources/src/package/manifest.rs:24-35` — which `manifest.rs` never mentions. Same comment mis-cites the field read: it names `loader.ts:596`, which is `const packageJsonPath = path.join(dir, "package.json");`.

**upstream** — `pi/packages/coding-agent/src/core/extensions/loader.ts:561-566 @v0.83.0` declares `interface PiManifest`; the `pi` field is read at `:572-573`. pi has no per-extension manifest and no capability model at all — a pi extension is a `.ts` file with ambient Node authority — so `extension.json` is 100% cyrup-original as a schema, justified by the WASM sandbox and ADR-0002, and `manifest.rs:8-10` already says so.

**Impact** — a reader of either file concludes the other half is missing. That is not hypothetical: this surface enumeration initially scored `PiManifest` as unported until the second crate turned up. The mis-citation compounds it by pointing a checker at a line that has nothing to do with the claim.

**Fix** — rewrite `manifest.rs:8-10` to say `extension.json` is cyrup-original and **disjoint** from pi's `pi` key, name `crates/cyrup-resources/src/package/manifest.rs` as where pi's four keys live, and correct the citation to `loader.ts:561-566` (declaration) and `:572-573` (field read). Add the reciprocal pointer in `cyrup-resources`. Comment-only.

**Verify** — `grep -n 'loader.ts:596' crates/` returns nothing; each of the two manifest files names the other.

## EXT-071 — `interface ext-tools`' shape comments advertised a removed field and under-documented another

**Kind** stale-port · **Severity** low · **Effort** S · **Confidence** confirmed · **FILED AND CLOSED 2026-08-14 (ext-rpc surface enumeration)**

**cyrup (as found)** — `crates/cyrup-ext/wit/world.wit:828` (now `:834`) documented `get-all-tools` as returning *"json array of ToolInfo {name, source, parameters, …}"*. `source` is precisely the cyrup-invented tier discriminator **EXT-060 removed** from the emitted object: `registry.rs:544-586` emits pi's five keys and `crates/cyrup-ext/src/tests/aggregation.rs:211-235` pins it. The WIT comment still advertised the field the code no longer emits — the `CYRUP_SHARE_VIEWER_URL` failure mode exactly, an advertised surface that nothing produces. `:830` (now `:839`) had the mirror problem in the other direction: it documented `get-commands` as *"SlashCommandInfo {name, description}"*, two keys, while cyrup's own EXT-037 implementation (`host/live.rs:1000-1035`) correctly emits all four of pi's.

**upstream** — `pi/packages/coding-agent/src/core/extensions/types.ts:1552-1554 @v0.83.0` — `export type ToolInfo = Pick<ToolDefinition, "name" | "description" | "parameters" | "promptGuidelines"> & { sourceInfo: SourceInfo };`, five keys, no `source`. `pi/packages/coding-agent/src/core/slash-commands.ts:6-11 @v0.83.0` — `SlashCommandInfo { name: string; description?: string; source: SlashCommandSource; sourceInfo: SourceInfo }`, four keys.

**Impact** — a guest author reading the WIT (which is the guest's only contract) writes a deserializer for a `source` field that will never arrive, and does not write one for `source`/`sourceInfo` on commands that always do. Both are comment-only; neither touches the ABI.

**Fix — DONE 2026-08-14** — both comments rewritten in **both** `world.wit` copies to pi's real key sets, each with an explicit do-not-restore note naming EXT-060 and EXT-037 and citing `types.ts:1552-1554` / `slash-commands.ts:6-11` @v0.83.0. The `interface ext-tools` header citation was corrected in the same edit (EXT-072). Copies re-verified byte-identical by `diff`.

**Verify** — `grep -n 'ToolInfo {' crates/cyrup-ext/wit/world.wit crates/cyrup-ext-sdk/wit/world.wit` shows the five-key list and no `source`; `the_host_and_guest_wit_world_copies_are_identical` (`crates/cyrup-ext/src/tests/wit_world_sync.rs:40`) stays green, as do the package-line and event-count pins.

## EXT-072 — ~50 in-tree pi citations across both `world.wit` copies and `ctx.rs` are stale by a uniform offset, in five clusters

**CLOSED 2026-08-15**, with every "actual" line re-derived at `v0.83.0` this pass rather than carried from this row. Rewritten across both `world.wit` copies and seven `.rs` files, and guarded by `no_struck_pi_citation_is_restored_as_a_live_citation` (34 sites RED before). Residual: the lint pins struck values, not every citation, and the SDK's `example.rs`/`guest.rs`/`descriptor.rs` were not audited.

**Kind** stale-port · **Severity** low · **Effort** M · **Confidence** confirmed · **filed 2026-08-14 (ext-rpc surface enumeration)**

**cyrup** — every `types.ts:N` citation in both `world.wit` copies (109 occurrences) and in `cyrup-ext-sdk/src/ctx.rs` (57) was resolved mechanically against `v0.83.0`. Five clusters came back wrong with a **uniform** offset each, which is the signature of a pi revision the region moved in — the same class as EXT-036's `+8` and `~+7` clusters, in regions that sweep did not walk.

- **Cluster A (uniform −7)** — `addAutocompleteProvider` cited `types.ts:218` at `world.wit:180` and `:462` (both copies) and `cyrup-ext-sdk/src/api.rs:688`. `:218` is `/** Get the current text from the core input editor. */`; actual **`:225`**. Same offset EXT-036 fixed *inside* `interface ui`; these three sites are in `interface events` / `interface registration` / `api.rs`.
- **Cluster B (uniform +3, `shutdown` +4)** — the `ExtensionContext` method block, cited at each member's NEIGHBOUR's doc line, in both `world.wit` copies AND `ctx.rs`: `isIdle` cited `:333` (actual **`:330`**), `isProjectTrusted` `:335` (actual **`:332`**), `hasPendingMessages` `:341` (actual **`:338`**), `abort` `:339` (actual **`:336`**), `shutdown` `:344` (actual **`:340`**). Sites, re-derived at HEAD after this pass's edits: `world.wit:855`, `:857`, `:859`, `:885`; `ctx.rs:148`, `:159`, `:169`. *(The `abort`/`shutdown` sites — `world.wit:904`, `:914`, `ctx.rs:225`, `:235`, `host/services.rs:119`, `:122` — were corrected in the EXT-073 fix.)* The bracketing citations are CORRECT (`cwd :315`, `scopedModels :326`, `signal :334`, `getSystemPrompt :346`), which is what makes it a stale cluster rather than invention. *(The `abort`/`shutdown` sites were corrected in the EXT-073 fix; the other three remain.)*
- **Cluster C (uniform −12…−20)** — the command-tier context, in both copies and `ctx.rs`: `ExtensionCommandContext` cited `:339-373` (actual **`:353-387`**, `ctx.rs:1267`); `ReplacedSessionContext` `:346-390` (`world.wit:187`) and `:374-390` (`ctx.rs:1446`, `:1493`) — actual **`:394-404`**; `newSession{withSession}` `:346` (actual **`:361-365`**, `ctx.rs:1323`); `fork{withSession}` `:355` (actual **`:368-371`**, `ctx.rs:1336` — and `:355` is `getSystemPromptOptions`, the one member cyrup does not port, EXT-061); `switchSession{withSession}` `:368` (actual **`:380-383`**, `ctx.rs:1348` — and `:368` is `fork(`); the control opts bag `:346-368` (actual **`:361-383`**, `world.wit:888`).
- **Cluster E (uniform −32 / −31 / −15)** — the event PAYLOAD interfaces in the notify block: `TurnEndEvent` cited `:703-709` → actual **`:735-740`**; `MessageStartEvent` `:711-715` → **`:743-746`**; `MessageUpdateEvent` `:717-722` → **`:749-753`** (`world.wit:346`, `:349`, `:352`); `UserBashEvent` `:782-790` → **`:813-821`**; `InputEvent` `:800-810` → **`:831-841`** (`:304`, `:307`); `SessionBeforeCompactEvent` `:577-587` → **`:592-602`**; `SessionCompactEvent` `:589-598` → **`:605-613`**; `SessionBeforeTreeEvent` `:623-628` → **`:639-643`** (`:397`, `:404`, `:408` — and `:623-628` lands on `TreePreparation`, a different type); `ToolResultEventBase` `:883-929` → **`:914-922`**, its `usage` field `:919-921` → **`:920-921`** (`:288`, `:290`). Fifteen other citations in the same block are CORRECT and are **not** part of the cluster — do not "fix" them: `BeforeProviderHeadersEvent :686-689`, the doc at `:681-685`, `BeforeProviderRequestEvent :676-679`, `AfterProviderResponseEvent :692-696`, `ModelSelectEvent :794-799`, `ToolExecutionUpdateEvent :770-776`, `ToolExecutionEndEvent :779-785`, `SessionStartEvent :562-569`, `SessionShutdownEvent :616-621`, `SessionInfoChangedEvent :571-575`, `ResourcesDiscoverEvent :544-548`, `ProjectTrustEvent :519-522`, `SessionBeforeSwitchEvent :578-582`, `SessionBeforeForkEvent :585-589`, `AgentSettledEvent :721-725`.
- **Singletons (six)** — (1) `world.wit:167` cites `ProviderConfig.oauth/streamSimple, types.ts:1373-1392`; that range is inside `registerProvider`'s `@example` JSDoc — actual `ProviderConfig` **`:1427-1464`**, `streamSimple` **`:1437`**, `oauth` **`:1450-1463`**. (2) `world.wit:800` cites `createAssistantMessageEventStream, types.ts:1373` — same JSDoc line; the symbol is not in this file. (3) `world.wit:213` cites `RegisteredCommand.handler` at `:1109` (actual **`:1167`**; `:1109` is `skipConversationRestore`) and `:215` cites `getArgumentCompletions` at `:1108` (actual **`:1166`**; `:1108` is `cancel?: boolean`) — uniform −58. (4) `world.wit:219` cites `registerShortcut handler, types.ts:1199-1205` (actual **`:1250-1256`**; `:1199-1205` is inside the overload block) — −51. (5) `world.wit:900` cites `send-message` at `:1223` (actual `sendMessage` **`:1286`**; `:1223` is the `tool_execution_start` overload). (6) `world.wit:660-663`'s `set-thinking-level` **tier delta is correctly disclosed** — cyrup makes it COMMAND-tier and rejects an event-tier call where pi allows it from any handler — but its supporting citation is wrong: it says `runner.ts:330`, and at `v0.83.0` `:330` is `this.runtime.getAllTools = actions.getAllTools;`; the `setThinkingLevel` wiring is **`runner.ts:336`**. The delta itself is not disputed and must not be "fixed"; only the line. (7) `world.wit:398` cites `CompactionPreparation` as `compaction.ts:634` — wrong line AND wrong path; actual `packages/coding-agent/src/core/compaction/compaction.ts:692` — and `world.wit:409` cites `TreePreparation` as `compaction.ts` when it is in `extensions/types.ts:624`, the very file being cited everywhere else.
- **The `:1257-1266` instance EXT-036 named by name** — `world.wit:823-824` (both copies) read *"Pi getActiveTools/getAllTools/setActiveTools/getCommands, types.ts:1257-1266"*; actual `getActiveTools` **`:1320`**, `getAllTools` **`:1323`**, `setActiveTools` **`:1326`**, `getCommands` **`:1329`**. EXT-036's write-up lists this exact value in its wrong-surface cluster and the sweep fixed the `.rs` sites (`host/services.rs:623-625` correctly cites `:1552`/`:1323`; `:645-646` correctly cites `:1329`) — but not the WIT. A closed item with a live instance of the defect it closed. **Corrected 2026-08-14** in the EXT-071 edit; recorded here because it is the strongest argument for the guard test below.
- **Three self-undermining corrective notes** — notes written specifically to stop a future auditor "restoring" a bad value, each itself inaccurate: `world.wit:524` asserts *"`:89` is a blank line before the UI Context banner at v0.83.0"* — `:89` is `export type { AppKeybinding, KeybindingsManager } from "../keybindings.ts";`, the blank is `:90` and the banner `:91-93`. `world.wit:621-622` asserts `:265-275` is *"getAllThemes/getTheme/setTheme/getToolsExpanded"* — `getToolsExpanded` is `:278`, outside the range (`:265-275` is theme/getAllThemes/getTheme/setTheme). `world.wit:842` cites `ExtensionContext` as `:305-347`; the interface is `:307-347` (`:305` is `ExtensionMode`).

**upstream** — every "actual" line above was re-derived at `v0.83.0` this pass and is quoted from the tag, not carried over from the finding.

**Impact** — under this project's rules an in-tree pi citation IS the evidence that a port matches upstream; a citation that resolves to an unrelated line is worth as much as none, and worse than none when it resolves to a *plausible* line, as cluster B's neighbour-doc offsets all do. Cluster C's `fork` citation lands on `getSystemPromptOptions`, which is exactly why EXT-061's gap went unnoticed for nine sweeps.

**Fix** — rewrite each site to the verified line, in **both** `world.wit` copies and `ctx.rs`/`api.rs`, tagging every one `@v0.83.0`. Then add the guard EXT-036's own Verify line asked for and never got: a test that, given a checked-out pi tree at the ported tag, resolves every `types.ts:N` citation in the WIT and asserts the cited line contains the cited symbol — gated on the pi checkout being present so it skips in CI without it. Without that, this rots again on the next upstream bump; it has now rotted twice.

**Verify** — the citation-lint test is green over all 109 WIT occurrences and all 57 in `ctx.rs`; `the_host_and_guest_wit_world_copies_are_identical` stays green.

## EXT-073 — Two fabrications survive in the WIT: a spliced `ctx.abort()` doc quote, and nine citations naming a band that exists in no version

**CLOSED 2026-08-15 on the residual**, plus a TENTH instance the item did not have (`agent_settled … :1225`, live in four files) that the new `every_subscribed_at_citation_names_the_event_pi_subscribes_on_that_line` guard found. 8 sites RED before.

**Kind** stale-port · **Severity** medium · **Effort** S · **Confidence** confirmed · **PARTIALLY FIXED 2026-08-14 (ext-rpc surface enumeration)**

**cyrup** — distinct from EXT-072's stale offsets: these two do not resolve to *anything* upstream, at any tag. This area has now produced fabricated pi citations three times (EXT-021's `types.ts:265-275` for `working-start`/`working-stop`; EXT-036's `types.ts:1198-1239` for the overload block; these).

1. **A spliced doc quote — FIXED.** `world.wit:895-896` (now `:904-905`, both copies), `cyrup-ext-sdk/src/ctx.rs:225`, and `crates/cyrup-ext/src/host/services.rs:119` all quoted `ctx.abort()` as *"Abort the current agent run. Available in all contexts."* Upstream's doc for `abort` is *"Abort the current agent operation"* (`types.ts:335`, method at `:336`); *"Available in all contexts."* is **`shutdown`'s** sentence (`:339`, method at `:340`); and upstream never writes "agent run" on either. The two docs were spliced into one quotation attributed to a line that carries neither half. **Corrected at all four sites this pass**, each now citing the method line and its own doc line with an explicit do-not-restore note.
2. **Nine citations into a band that exists in no version — STILL OPEN.** Seven `subscribed at` comments in `interface events` cite lines in the `:1135-:1161` band, which lies entirely OUTSIDE the overload block (`:1190-1231` @v0.83.0, `:1203-1244` @v0.84.1) — `:1135` is blank, `:1143-1146` is `MessageRenderer`, `:1158-1160` is the *Command Registration* banner. Re-derived actuals: `tool_call` cited `:1145` → **`:1228`**; `tool_result` `:1146` → **`:1229`** (`world.wit:287`); `context` `:1144` → **`:1207`**; `message_end` `:1143` → **`:1222`** (`:297`); `before_agent_start` `:1135` → **`:1214`** (`:298`); `input` `:1158` → **`:1231`**; `user_bash` `:1159` → **`:1230`** (`:303`); `before_provider_request` `:1160` → **`:1209`**; `after_provider_response` `:1161` → **`:1213`** (`:315`). PLUS, in the **file header** the event-count test does guard, `session_info_changed` is cited as *"upstream `:1203`"* at `world.wit:12` and again at `:374` — `:1203` is `session_compact`; `session_info_changed` is **`:1193`**. This is the identical not-any-version signature EXT-036 filed for `:1198-1239`, one band over.

**upstream** — the full verified overload map at `v0.83.0`, re-derived this pass and recorded here so the next reader does not have to: `project_trust 1190`, `resources_discover 1191`, `session_start 1192`, `session_info_changed 1193`, `session_before_switch 1195`, `session_before_fork 1198`, `session_before_compact 1200`, `session_compact 1203`, `session_shutdown 1204`, `session_before_tree 1205`, `session_tree 1206`, `context 1207`, `before_provider_request 1209`, `before_provider_headers 1212`, `after_provider_response 1213`, `before_agent_start 1214`, `agent_start 1215`, `agent_end 1216`, `agent_settled 1217`, `turn_start 1218`, `turn_end 1219`, `message_start 1220`, `message_update 1221`, `message_end 1222`, `tool_execution_start 1223`, `tool_execution_update 1224`, `tool_execution_end 1225`, `model_select 1226`, `thinking_level_select 1227`, `tool_call 1228`, `tool_result 1229`, `user_bash 1230`, `input 1231`. The header's OTHER claims are CORRECT and must not be touched: `before_provider_headers :1212`, `model_select :1226`, `thinking_level_select :1227`.

**Impact** — rated medium rather than low, unlike EXT-072, because a fabricated citation is not a rotted one: it was never true, so no upstream bump produced it and no upstream bump will reveal it. The spliced quote was the worse of the two — a quotation is the strongest evidence form this project uses, and this one attributed invented text to a specific line, in the SDK a guest author reads.

**Fix — residual** — rewrite the nine `subscribed at` citations and both header occurrences of `:1203` to the verified map above, in **both** `world.wit` copies, tagged `@v0.83.0`. Then extend `the_header_event_count_matches_the_declared_event_exports` (`crates/cyrup-ext/src/tests/wit_world_sync.rs:128`) to validate the cited RANGE as well as the count, which is EXT-036's own unmet Verify line, and fold it into the citation-lint test EXT-072 asks for.

**Verify** — `grep -n '11[34][0-9]\|1161\|:1203' crates/cyrup-ext/wit/world.wit crates/cyrup-ext-sdk/wit/world.wit` returns no `types.ts` citation in the dead band; the two copies stay byte-identical; the event-count and package-line pins stay green.

## Coverage

### Standing rules and detection methods added 2026-08-14 (sweep 6)

**1. Instance-scoped state substituted for a call-scoped pi parameter must be torn down in `Drop`, not on the success path.** Every pi surface where a PARAMETER cannot cross the Component Model boundary gets re-expressed as host-side instance-scoped state plus a guest poll import (`signal` → `is-cancelled`, `AbortSignal` → `is-run-cancelled`, the `onUpdate` callback → `emit-update` + `take_tool_updates`). A JS `async function` that has begun always settles, so upstream never has to unwind; a Rust future can be dropped at the await by an outer `select!` / `timeout` / `abort`, and neither `select!` arm runs on that path. This is a **seam-shaped bug generator, not a one-off** — it produced `EXT-M01`.

**2. Hand-written trait delegation is the Rust cost of a JS object spread, and its failures are INVISIBLE.** pi's `wrapRegisteredTool` is `return { ...tool, execute }` (`core/extensions/wrapper.ts:21-22` @v0.83.0) — every field the inner tool carries survives BY CONSTRUCTION, including fields added years later. `impl Tool for RegisteredTool` in `crates/cyrup-ext/src/wrapper.rs` must name each method, and its hand-written list of eleven silently omitted `constrained_sampling` (closed under `PROV-011`). Because **a dropped delegation returns exactly the trait default**, the drop cannot be caught by any test whose fixture does not override that method with a DISTINCT NON-DEFAULT value. The file's own doc comment ("Every `Tool` surface method delegates verbatim") was already false when it was opened. Short of a derive/macro that generates the impl from the trait, the mitigation is the `Fixed` fixture's stated discipline — **every surface method carries a distinct non-default value — and it must be extended in the SAME COMMIT as any new trait method.** `TOOL-024` fixed nine instances of this shape; `constrained_sampling` was added afterwards and made a tenth. Fixing the fixture once does not immunise it.

**3. Doc-comment-as-truth is not evidence.** `crates/cyrup-ext/src/registry.rs:49` documented the stored field as being there "so the agent loop can forward it", and `host/live.rs:1788-1796` documented `WasmTool`'s accessor as closing `PROV-011` at BOTH ends — while the agent loop hard-coded `None` and the wrapper between them dropped the value. Three separate doc comments described a working end-to-end path that did not exist. **A prose claim that a value "reaches" somewhere is not evidence; the only evidence is a test that reads it at the far end.**

**4. A field whose absence is defined to behave like its default cannot fail loudly.** `constrainedSampling` absent and `constrainedSampling: false` are defined by pi to be identical (`packages/ai/README.md:483`), so erasing a real declaration produces no error, no warning and a perfectly well-formed request — just an unconstrained one. Any future field with a "null behaves like the default" contract needs an explicit far-end test, because it has no failure mode of its own.

**5. Citation forensics — the two failure modes have DIFFERENT fixes and must not be conflated.** A **uniform offset** across a cluster of citations means **version lag** (a real pi revision, just not the ported tag) and is fixed by re-tagging. Citations **converging on one number across unrelated symbols** (here `extensions/types.ts:1043` doing duty for `AgentToolResult`, `UserBashEventResult` and `ToolResultEventResult`) means **invention**. Corroborating tell found in `host/services.rs`: every citation ADDED by the recent EXT-021/EXT-047 work (`:151`, `:154`, `:164`, `:167`, `:170-175`) is exact while every citation from the block's original import is ~6 low — so rot is confined to the import and is not systematic misreading by one author.

**6. A citation is only checkable when it names ONE line, or a range whose ENDPOINTS ARE BOTH MEANINGFUL.** All three fabrications found in area 06 to date hid inside a multi-line RANGE (`types.ts:200-230` hid `pasteEditorText`, a function pi has at no version; `:130-150` hid the chrome trio; `:1043-1048` hid the `AgentToolResult` package error). **Treat ranges as unverified until narrowed.**

**7. Cross-package citation hazard.** `extensions/types.ts` RE-EXPORTS types it does not define (`AgentToolResult`, `ExecResult`, `KeybindingsManager`, … at `:85-89`). A bare `types.ts:N` for a re-exported type looks unfalsifiable but is wrong — `AgentToolResult` is `packages/agent/src/types.ts:355-369`, a different package entirely. **Any citation of a re-exported symbol must name the package.**

**8. Ownership partitions the WRITE set; it says nothing about the BUILD set.** Under feature-partitioning, "you own crate X" is not "you can build crate X" — the build set is the dependency closure. During sweep 6, `crates/cyrup-ext` was uncompilable for ~20 minutes because an `interface ui` WIT change landed ahead of its host impl, which blocked every agent downstream of it.

### Read first-hand at cyrup HEAD `a9000b1` (tree clean; last code commit `04c1ba2`)

`crates/cyrup-ext/src/{event,registry,facade,loader,manifest,dispatch,contract,native}.rs` in full or across every region the chains required; `crates/cyrup-ext/src/host/live.rs` targeted (the `register_*` imports `:88-142`, ext-tools `:731-766`, control `:779-840`, `set_label` `:355`, ui `:245-250`, `WasmTool` `:1386-1420`, `provider_stream_simple` `:1296-1320`, `get_flag` `:1301-1337`); `crates/cyrup-ext/src/build/mod.rs:1-80`; **both copies of `wit/world.wit` in full, interface by interface, and `diff`ed byte-for-byte (identical)**; `crates/cyrup-ext/tests/wit_world_sync.rs:1-160` and `native_dispatch.rs:848-885`; `crates/cyrup-ext-sdk/src/{descriptor,guest,ctx,events,widget}.rs` targeted. Consumer seams followed where a chain demanded it: `crates/cyrup-session-svc/src/{builder,session,hooks,subscriber,host_services,lib}.rs`, `crates/cyrup-agent/src/agent.rs`, `crates/cyrup-core/src/tool.rs`, `crates/cyrup-tui/src/{app,commands}.rs`, plus `Cargo.toml` at the workspace root and in `cyrup-session-svc`.

### Read first-hand upstream at the ported tag pi v0.83.0

Extracted with `git show v0.83.0:<path>`, so the line numbers in this file are the tag's, not `main`'s. `packages/coding-agent/src/core/extensions/types.ts` in full — the `ExtensionUIContext` block `:95-282`, base `ExtensionContext` `:305-347`, `ToolDefinition` `:449-498`, every event interface `:519-806`, `RegisteredCommand`/`ResolvedCommand` `:1162-1172`, the 33 `on(event:"…")` overloads `:1190-1231`, the `ExtensionAPI` surface `:1236-1410`, `ExtensionShortcut` `:1524-1529`, `ToolInfo` `:1552`. Also `extensions/runner.ts` (reserved keybindings + `buildBuiltinKeybindings` `:70-111`, `getShortcuts` `:492-540`, `resolveRegisteredCommands` `:598-655`, the `emit*` reducers), `extensions/loader.ts`, `core/agent-session.ts` (`getAllTools` `:906-914`, `getCommands` `:2332-2354`, `_refreshToolRegistry`), `core/slash-commands.ts:4-11`, `core/event-bus.ts` **in full**, `core/project-trust.ts`, `modes/interactive/interactive-mode.ts` (conflict diagnostics `:529-543`, autocomplete `:598-628`, diagnostics panel `:1605-1618`, compact handler `:1819-1829`, shortcut install `:1833-1846`, `showExtensionError` `:2545`, `populateHistory` `:3244-3412`, `/hotkeys` `:5856`), and `packages/agent/src/agent-loop.ts:580-660` @v0.84.1.

### Version-lag sweep

`git diff --stat v0.83.0..v0.84.1 -- packages/coding-agent/src/core/extensions/ packages/coding-agent/src/extensions/ packages/coding-agent/src/core/sdk.ts` = 6 files, +110/−38 (`sdk.ts` unchanged). Every hunk of the `types.ts` / `index.ts` / `runner.ts` / `loader.ts` diff was read line by line. It produced four drift items — EXT-049, EXT-050, EXT-051, EXT-052 — plus evidence re-scoping folded into EXT-019 (`MarkdownTransformContext` + `getMarkdownTransformers`) and EXT-022 (`context.publish({persist})` replacing `context.store`) rather than filed twice. The `llama/` diff was counted but not read (blind spot 6).

### Surface-driven sweep — the axis actually used, stated as a method

Recorded in the repair pass because the critique was right that this file never used the term and
never said where its new items came from. This area ran **two** enumerations and **not** the one
README blind spot 1 prescribes.

**Axis A — the event catalog, upstream-anchored and exhaustive.** pi's 33 `on(event:"…")` overloads
were enumerated against cyrup's 31 `EventKind` variants and the 31 WIT `on-*` exports, machine-checked
by `tests/wit_world_sync.rs:96-129`. The name diff is exactly two (EXT-009, EXT-011). The **payload**
diff was then done by hand event by event, and that is where the yield was: EXT-014, EXT-015, EXT-016,
EXT-042, EXT-043.

**Axis B — cyrup's own claims, inverted.** Rather than asking "what does pi export that cyrup lacks",
this pass walked the **invariants cyrup asserts about itself** — every doc comment, module header and
ADR reference in `crates/cyrup-ext` that promises a control — and asked "what code enforces this?".
`manifest.rs:2`, `host/store_state.rs:1-3` and `:20-22`, and the `world.wit:384-386`/`:416-418`
comments each promise a capability grant; `grep -rn capabilities crates/cyrup-ext/src` returns only
producers. That produced **EXT-054** and **EXT-055**, the two highest-consequence items in the area,
and neither is visible from pi at all — pi has no capability model, so a pi-anchored sweep is
structurally blind to this whole class. **This axis generalises**: any cyrup-original invariant
(fail-closed gates, first-wins tables, ABI fingerprints, deny-by-default defaults) is a claim with a
testable consumer, and the sweep is "grep the claim, then grep for a reader".

**The sweep that was NOT run** — see blind spot 8.

### Event catalog enumerated exhaustively

pi = **33** `on(event:"…")` overloads (`types.ts:1190-1231` @v0.83.0; `:1203-1244` @v0.84.1). cyrup `EventKind` = **31** (`event.rs:13-53`, names `:104-134`). WIT `interface events` = **31** `on-*` exports, matching `EventKind` 1:1 and machine-checked against the header claim (`tests/wit_world_sync.rs:96-129`). The set difference is exactly two — `before_provider_headers` (EXT-009) and `session_info_changed` (EXT-011); no third gap and no cyrup-invented event. **But name parity is not payload parity**, and the payload diff is where this pass found work: `project_trust` loses `cwd` (EXT-043), `model_select` nests `previousModel`/`source` and `thinking_level_select` loses `previousLevel` (EXT-042), `resources_discover` loses `cwd`+`reason` (EXT-016), the four session-lifecycle events lose their discriminating fields (EXT-015), and `tool_execution_update`/`_end` lose `toolName` (and `args`, EXT-014). `session_tree` was checked and is **fine** — all four upstream fields ride inside the blob (`session.rs:1970-1975`).

### Inferred rather than read

- **EXT-017 / EXT-037 non-determinism.** Both rest on `HashMap` iteration order being observably unstable between runs. Rust's default hasher is randomly seeded per process, so this is sound in principle, but no reorder was observed — the claim is derived from `command_descriptions` iterating `.commands.iter()` (`registry.rs:662`) and that value reaching `crates/cyrup-tui/src/app.rs:4103` unsorted.
- **EXT-039's precedence inversion.** pi's `getShortcuts` (which populates the map, refusing reserved keys) and `setupExtensionShortcuts` (which installs `onExtensionShortcut`) were both read, as was cyrup's ordering at `app.rs:1691-1703`. pi's **editor** was not read, so "upstream lets a non-reserved extension binding win" is inferred from the reserved-list design and the warning text, not from the dispatch site. The refusal and diagnostics halves are read directly.
- **EXT-026.** Asserted from `mod host_services;` being unconditional and the workspace `cyrup-ext` dependency lacking `default-features = false`; the exact `default-features` text was not confirmed and `cargo check --no-default-features` was not run.

### Rejected, corrected, or folded — do not re-derive these

- **Nothing was refuted outright this pass.** Every auditor finding survived, but six were **corrected** and the corrections are load-bearing:
  - **EXT-037 severity lowered medium → low.** It is real (`get_commands` returns bare names), but it is information loss on a guest-only introspection API with zero WASM guests shipping. EXT-017 is the same defect on a user-visible path and carries the medium. Do not re-rate EXT-037 upward without a shipping guest.
  - **EXT-011 mechanism corrected.** cyrup already *emits* `AgentSessionEvent::SessionInfoChanged` (`cyrup-session-svc/tests/round8_postrun.rs:240-257`); only the extension-host routing is missing, so effort is S, not M. Do not re-file this as "the event does not exist".
  - **EXT-051 half corrected.** The auditor's `grep -rn 'is_subscription\|isSubscription' crates/` = nothing is **false** — the concept exists across `crates/cyrup-provider/src/auth/` — and `ProviderConfig.oauth` is an opaque `Value`, so the key already crosses the seam. What is missing is the typed field and a consumer, not the transport.
  - **EXT-024 evidence corrected.** `grep constrained_sampling crates/` is **not** empty: `cyrup-provider/src/api/bedrock_converse_stream.rs:44` matches, and the hit is a comment confirming the gap.
  - **EXT-028 and EXT-036 both had their closures partially refused.** EXT-028's four filed halves are genuinely closed but its own version marker rotted; EXT-036's `world.wit` half was declared closed on evidence that itself contains a fabricated citation range.
- **Two of the six blind-spot findings were folded into existing items rather than given new IDs**, to avoid double-filing: the stale `// cyrup:ext@0.3.0` marker on line 1 of both `world.wit` copies is **EXT-028's residual** (it is that item's own subject), and the header citation `types.ts:1198-1239` matching no upstream version is **EXT-036 site (3)**.
- **Ledger cross-check.** `00-residual-ledger.md:60` justifies listing EXT-S02 open by citing `commands.rs:31` typing `SlashCommand::name` as `&'static str`. That is stale — it is `Cow<'static, str>` at `commands.rs:36` and the catalog is wired into the TUI at three sites. The ledger's "1 open (EXT-S02)" row at `:256` and suggested-order item 4 at `:161` should both be struck. The ledger's structural defect D also applied to this file: the self-contradicting `~~EXT-028~~ **CLOSED** | Open |` status row is gone, replaced by an explicit `partially closed` row.

### Handoffs to other areas

- **Area 07 (TUI).** The *read* side of EXT-047's keyed widget map is TUI-S01 / TUI-014 territory; only the WIT/`HostServices` half is filed here. EXT-041's fix edits `crates/cyrup-tui/src/app.rs` but the defect is the extension-render seam, so it stays in 06. EXT-039's precedence inversion also lands in `app.rs:1691-1703`.
- **Area 08 (seam/modes).** pi's RPC `setWidget` splits into `widgetKey`/`widgetLines`/`widgetPlacement`, which EXT-047 unblocks but does not cover.
- **Area 01 (provider).** EXT-052's `on_payload` reduction reuses cyrup's existing `before_provider_request` path, and EXT-009's header hook needs a dispatch point in provider request assembly — both area 01 code.

### Blind spots for the next pass

1. **Static only, and the hardest half of this area is dynamic.** Nothing was compiled or run. Three `not(wasm-host)` arms (`facade.rs:441-443`, `:899-908`, `:1261-1264`) have almost certainly never been type-checked, and that cannot be proven from reading.
2. **Zero WASM guests ship, so the guest half of every finding is unexercised.** `crates/cyrup-ext-sdk/src/example.rs` is the only component author in the tree. Every finding whose impact reads "a guest cannot X" (EXT-037/038/044/045/046/050/052) is read off the WIT and the host imports; none was observed failing against a real component. Severity for guest-only items is a judgement about the seam's shape, not an observed report — which is why none is rated above medium.
3. **`crates/cyrup-ext/src/host/services.rs` (1944 lines) has still never been audited method by method** — only `SharedBus`, `GuestState`'s flag/command/renderer/autocomplete tables, the `HostServices` trait defaults, `set_label` and `set_widget`. EXT-021's caveat stands: a `ctx.ui` capability could be implemented host-side while missing from the WIT and this pass would not have seen it.
4. **The capability sandbox is still only half-audited.** EXT-054 and EXT-055 came out of asking what *reads* `Capabilities`, and the answer was "nothing". What was **not** done is reading `crates/cyrup-ext/src/caps/{http,fs,proc}.rs` and verifying the `is_trusted = origin.is_pre_trust() || project_trusted` gate the `world.wit` comments at `:384-386` and `:416-418` promise. A trust-gate hole there would be a **critical** that this pass could not see. **This is the highest-value target for the next pass in area 06.**
5. **The Tier-1 build path is unverified end to end.** EXT-028's four halves closed on directly-read evidence, but `build.rs`'s `CYRUP_EXT_ABI_FINGERPRINT` was never observed changing after a `world.wit` edit — the test at `wit_world_sync.rs:135-160` recomputes the hash rather than proving cache invalidation. `build/toolchain.rs` was not read.
6. **Upstream coverage is scoped to the extension paths.** `pi/packages/coding-agent/src/extensions/llama/` (~5 files) was diffstat-counted but not read, so EXT-027 is carried forward on old evidence; `wrapper.ts` was not re-read; pi-subagents / pi-permission-system / pi-intercom were out of scope, so extension-surface changes **they** depend on (the intercom `EXTENSION_BUS_FEATURE` work `PARITY-GAPS.md` records) were not cross-checked against the bus findings here — relevant to EXT-018, EXT-034, EXT-050 and EXT-057, which are all one function.
7. **Payload parity was enumerated for events but not for the other surfaces.** The 33-vs-31 event diff is exhaustive and machine-checked, and the per-event payload diff was done by hand. No equivalent field-by-field sweep was run over `interface ui`, `interface session`, `interface models` or `interface ctx-state` — EXT-046, EXT-047 and EXT-048 were each found incidentally while chasing something else, which strongly suggests more of the same class remains.
8. **NEW (repair pass) — the export-enumeration sweep README blind spot 1 prescribes was never run for this area, and its absence is the most likely source of missed items.** Axis A above enumerates one upstream surface (the event catalog) exhaustively and Axis B enumerates cyrup's own claims. Neither is the prescribed sweep: *walk every exported symbol of `pi/packages/coding-agent/src/core/extensions/{types,runner,loader,index}.ts` at v0.83.0 and ask, symbol by symbol, what in `crates/` consumes it.* `types.ts` alone is ~1550 lines and its `ExtensionAPI` (`:1236-1410`), `ExtensionUIContext` (`:95-282`) and `ToolDefinition` (`:449-498`) blocks were read as blocks and mined for known items rather than enumerated member by member with a consumer trace — which is exactly how EXT-046/047/048 came to be found "incidentally". `runner.ts`'s non-`emit*` exports and the whole of `loader.ts`'s export list were never enumerated at all. **Next pass: run it, symbol by symbol, and record the count of symbols traced.** Expect the same shape of yield the `packages/tui/src` sweep produced for area 07 (nine items, two critical, off six files nobody had read).
9. **NEW (repair pass) — the capability sandbox remains the highest-value target and is now the area's only critical.** Blind spot 4 already named `crates/cyrup-ext/src/caps/{http,fs,proc}.rs` as unread. EXT-054's promotion to critical raises the stakes: the item proves the *grant* is inert, but nobody has verified the `is_trusted = origin.is_pre_trust() || project_trusted` gate that is currently the only thing standing in front of the full host surface. A hole there would be a second critical, and it is one file-read away.


## EXT-074 — `models.set-model` / `set-thinking-level` advertised a COMMAND-only tier gate the host had stopped enforcing

**Kind** stale-port · **Severity** medium · **Effort** S · **Confidence** confirmed · **FILED AND CLOSED 2026-08-15 (area-06 pass, found while porting EXT-061's tier gate)**

**cyrup (as found)** — `world.wit` declared `set-model: func(model-json: string);  // COMMAND-only at the host` and, on its sibling, *"COMMAND-only at the host (deadlock rule): Pi allows setThinkingLevel from any handler …, but cyrup routes it through the command-tier `control` path, so an event-tier call is REJECTED with an observable error (like every `control.*` op) — never a silent no-op."* Neither statement was true at HEAD. `host/live.rs`'s `set_model` (`:561-562`) and `set_thinking_level` (`:580-581`) both take the UNGATED `guest_of`, each carrying an in-source GAP-11 note explaining that the tier gate was REMOVED because the op is queued through the `control` mpsc and applied at the store-free turn-boundary drain, which dissolves the R-08-008 deadlock the gate existed to prevent. `cyrup-ext-sdk/src/ctx.rs:68` repeated the claim — *"Model registry view (read; `set_model` is command-only)"* — in the file a guest author actually reads.

**upstream** — pi gates neither: `setModel` is bound with only `assertActive` at `pi/packages/coding-agent/src/core/extensions/loader.ts:359-362 @v0.83.0` and `setThinkingLevel` at `:369-372`, both reachable from any handler, and both take effect (`core/agent-session.ts:1476-1490` and `:1541-1572`).

**Impact** — the MIRROR of this area's characteristic defect. `EXT-066` was a capability declared in the world whose backend answered nothing; this is a RESTRICTION declared in the world that the backend does not apply. On a parity surface the second direction is the worse one: the WIT is the guest's ONLY contract, so an author who believes it either avoids a legal call from an event handler or writes their own deferral plumbing to route around a gate that is not there — and neither mistake produces a failure that points back at the comment. It also cost this pass real time: the comment was taken as the specification while designing `EXT-061`'s tier gate, and only reading `live.rs` showed the two disagreed.

**Fix — DONE 2026-08-15** — rewritten in the code's direction (the code was the parity-correct half) at all three sites: both `world.wit` copies and `cyrup-ext-sdk/src/ctx.rs`. The WIT now states that both are callable from ANY tier, cites pi's two `loader.ts` bindings, and explains that `set-thinking-level` keeps its `result` because it carries a real backend failure — no session attached — rather than a tier refusal. Comment-only; no ABI change; copies re-verified byte-identical.

**Verify** — `grep -n 'COMMAND-only' crates/cyrup-ext/wit/world.wit` returns nothing for the `models` interface; `the_host_and_guest_wit_world_copies_are_identical` stays green.

**Standing note this generalises to** — every tier/permission claim written as a COMMENT on a WIT signature is unenforced by construction. The gates that ARE enforced (`require_command_tier`, `require_ui`, `ui_guest_of`) are host-side calls, and a comment claiming one of them without a call is exactly as reliable as a capability with no backend. A future sweep wanting one more mechanical check here: for each WIT import whose comment claims a tier gate, assert the impl calls a gate helper.
