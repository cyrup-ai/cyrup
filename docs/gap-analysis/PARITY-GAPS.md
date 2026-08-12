# PARITY-GAPS

**Generated 2026-08-12. Supersedes the 2026-08-08 document entirely** — that one listed work batches
8–10 have since finished, missed work they uncovered, and (in its own predecessor) framed unported
work as "accepted divergence … don't fix without asking". **There is no such category.** The
project's goal is behavioural equivalence with the four upstreams. Every entry below is WORK.

Mechanism may differ where the language forces it — WASM Component Model guests where pi runs
TypeScript through `jiti`; ratatui where pi hand-rolls a renderer. Port the BEHAVIOUR and state the
mechanism difference with its reason. That is not an exemption; where a mechanism difference costs
behaviour, the entry says so and stays on the list.

| | |
|---|---|
| cyrup HEAD | **04c1ba2** (2026-08-12, clean) |
| `pi` | ported baseline **v0.83.0** → latest **v0.84.1** |
| `pi-subagents` | baseline **≈v0.43.0 with holes** (see note) → latest **v0.43.0** |
| `pi-permission-system` | ported baseline **v0.7.1** → latest **v0.8.0** |
| `pi-intercom` | ported baseline **v0.7.0** → latest **v0.9.2** (`lib.rs:2` still says v0.6.0 — stale) |

**Counts: 77 gaps — 21 port bugs, 10 unwired, 46 version lag.**
Plus 3 deletion candidates (§5) that are dead code with no behavioural difference, and 5 open
questions (§6) that need a human decision rather than more analysis.

**A note on the `pi-subagents` baseline.** The crate records no version string anywhere, and the
"v0.33.x–v0.34.0" figure CLAUDE.md carries is obsolete. It cites `v0.43.0` 461 times against 309 for
`v0.34.0`, and it ports files whose first tag is v0.35.0 (`watchdog/`), v0.36.0 (`tui/fleet-status.ts`),
v0.41.0 (`missions/`) and v0.43.0 (`agents/agent-refinements.ts`). Treat it as **targeting v0.43.0
with holes**. The five pre-v0.34.0 files it never ported are the port bugs PB-6…PB-10 below.

Read upstream with `git -C <repo> show <tag>:<path>`. Every line citation below was resolved by
reading the target file at the named tag — see §7 for exactly how much of that was first-hand.

---

## 1. Port bugs — upstream had it at the tag cyrup ported; cyrup does not

**These rank above everything else in this document.** They are not version lag: the behaviour was
available to be ported and was not.

### 1a. From `pi` v0.83.0

**PB-1 · `radius` provider is not registered** — *medium*
- upstream: `pi/packages/ai/src/providers/all.ts:117` @v0.83.0 (`radiusProvider()`; :121 @v0.84.1); definition `pi/packages/ai/src/providers/radius.ts:20`
- cyrup: `crates/cyrup-provider/src/providers/all.rs:141` (`builtin_providers_with`, body to :243) — no radius push
- observable: `--provider radius` resolves to no provider. The Radius OAuth flow is already ported (`crates/cyrup-provider/src/auth/oauth/radius.rs`, exported at `auth/oauth/mod.rs:61`) and the wire API it streams over exists (`crates/cyrup-provider/src/api/pi_messages.rs`), so a working credential can never be attached to a streamable provider. `crates/cyrup-provider/src/auth/builtin_oauth.rs:17` documents the hole in-tree.

**PB-2 · `qwen-token-plan` and `qwen-token-plan-cn` are not registered, but the resolver advertises them** — *medium*
- upstream: `pi/packages/ai/src/providers/all.ts:115-116` @v0.83.0; definition `pi/packages/ai/src/providers/qwen-token-plan.ts:6-15`
- cyrup: `crates/cyrup-provider/src/providers/all.rs:141-243` (no push; no `providers/catalog/qwen-token-plan*.json`) versus `crates/cyrup-config/src/model.rs:1022-1023` (both in `KNOWN_PROVIDERS`) and `model.rs:973-974` (both given the default model `qwen3.7-max`)
- observable: cyrup accepts `--provider qwen-token-plan` at argument validation and resolves a default model for it, then fails at stream time with no such provider. pi resolves and streams.

**PB-3 · `Models::refresh` accepts no options and returns no per-provider result** — *medium*
- upstream: `pi/packages/ai/src/models.ts:46-56` @v0.83.0 (`ModelsRefreshOptions{allowNetwork,force,signal}` + `ModelsRefreshResult{aborted,errors}`), refresh at `:276`. v0.84.1 additionally adds `providers?: readonly string[]` (`models.ts:67`) and generation-checked publication (`:320-361`)
- cyrup: `crates/cyrup-provider/src/collection.rs:317` (`pub async fn refresh(&self, provider: Option<&str>)`); the all-provider path is `futures::future::join_all(refreshes).await` at `:335` with every result discarded
- observable: no cache-only refresh, no force past a freshness check, no cancellation, and no report of which providers failed. Two concurrent refreshes of one provider both publish, last-writer-wins.

**PB-4 · Compact-read classification has no `docs` arm** — *small*
- upstream: `pi/packages/coding-agent/src/core/tools/read.ts:98` @v0.83.0 (`getPiDocsClassification`, resolving against `dirname(getReadmePath())`), called from `:130`. **Present at the ported tag** — the 2026-08-08 document and one survey both filed this as version lag; it is not
- cyrup: `crates/cyrup-tui/src/transcript.rs:2265` (`compact_read_classification` — `skill` and `resource` arms complete, no `docs` arm; the doc at `:2258-2263` states why)
- observable: reading cyrup's own shipped README/docs/examples renders as an ordinary file read, not the compact `docs` label. **Blocked on a decision, not on code** — see OQ-2.

**PB-5 · `PI_CODING_AGENT` is never stamped into the environment (and `AI_AGENT` is the v0.84.1 half)** — *small*
- upstream: `pi/packages/coding-agent/src/cli.ts:13` @v0.83.0 (`process.env.PI_CODING_AGENT = "true"`); `AI_AGENT = "pi"` is new at v0.84.1, `cli.ts:14` and `rpc-entry.ts:8`
- cyrup: `crates/cyrup/src/main.rs:53-57` — an explicit comment declines to replicate it because `std::env::set_var` is `unsafe` under edition 2024; `crates/cyrup-tools/src/tools/bash.rs:158-175` assembles the child env explicitly and adds neither key
- observable: a shell hook, npm script or MCP server that branches on `$AI_AGENT` / `$PI_CODING_AGENT` cannot tell it is inside cyrup. The unsafe-`set_var` rationale covers *process-global* mutation only — `bash.rs` already builds a per-child env vector, so both keys can be added there with no `unsafe`.

**PB-6 · Changelog-on-upgrade is absent; `lastChangelogVersion` is never read or written** — *medium*
- upstream: `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:997` @v0.83.0 (`getLastChangelogVersion`), `:998-999` (`getChangelogPath` / `parseChangelog`), `:1003` and `:1010` (`setLastChangelogVersion(VERSION)`); getter/setter at `core/settings-manager.ts:660` and `:664`
- cyrup: `crates/cyrup-config/src/settings.rs:994` (`last_changelog_version`) has zero callers workspace-wide and no setter exists; `/changelog` is hardcoded at `crates/cyrup-tui/src/app.rs:1824` to `push_block("What's New", "No changelog entries found.")`
- observable: after upgrading, pi shows the new entries once and records the version; cyrup shows nothing and `/changelog` always reports "No changelog entries found." The `collapseChangelog` settings row (`crates/cyrup-tui/src/app.rs:5667`) toggles a value nothing reads. (`enableInstallTelemetry`, the row beside it, **does** have live consumers — `crates/cyrup-config/src/policy.rs:25-27` and `crates/cyrup-session-svc/src/builder.rs:1145` — so do not include it in this claim.)

**PB-7 · The npm package channel is unported, and `npmCommand` is inert** — *large*
- upstream: `pi/packages/coding-agent/src/core/package-manager.ts:1720` (`getNpmCommand`) with install/update/list running through `:1740` (`runNpmCommand`), `:1745` (`getGitDependencyInstallArgs`), `:1753` (`runNpmCommandSync`); manifest kinds at `pi/packages/coding-agent/src/core/pi-manifest.ts:3-9` — a package may ship `extensions`, **`skills`, `prompts` and `themes`**
- cyrup: `crates/cyrup-resources/src/package/source.rs:81-83` returns `Err(Unsupported)` for any `npm:` spec; `crates/cyrup-config/src/settings.rs:742` (`npm_command`) has zero callers anywhere in the workspace
- observable: `cyrup install npm:<pkg>` fails outright, and setting `"npmCommand": ["pnpm","--silent"]` does nothing. The *extension* half of the channel is genuinely mechanism-forced (cyrup extensions are WASM components; there is no JS runtime to load a TS extension), but skills, prompts and themes are plain files that need no runtime and are unreachable purely because the channel is gone. See OQ-1.

### 1b. From `pi-subagents` (files whose first tag is at or before the ported baseline)

**PB-8 · Subagent RPC bridge is entirely absent** — *large*
- upstream: `pi-subagents/src/extension/rpc.ts:622` @v0.43.0 (`registerSubagentRpcBridge`, 653-line file; method list at `:29`; event names at `:25-27`), registered from `src/extension/index.ts:529`. First tag **v0.33.0**
- cyrup: `crates/cyrup-ext-subagents/src/extension.rs:9313-9352` (the whole `init` registration/subscription block) — no bridge; `grep -ri 'subagents:rpc' crates/` returns 0 (the only `rpc` tokens in the crate are doc citations at `extension.rs:7561` and `:9833`)
- observable: no host, embedder or sibling extension can drive subagents programmatically. Upstream answers `ping`/`status`/`spawn`/`steer`/`interrupt`/`stop`/`resume` over `subagents:rpc:v1:request` with a `subagents:rpc:v1:reply:<id>` envelope; cyrup emits no ready event and answers nothing.

**PB-9 · `clarify: true` is advertised but shows no preview/edit UI** — *large*
- upstream: `pi-subagents/src/runs/foreground/chain-clarify.ts:199` (`ChainClarifyComponent`, 1350-line file), dispatched at `src/runs/foreground/subagent-executor.ts:3190` and `:3572` and `src/runs/foreground/chain-execution.ts:692`, all three via `await ctx.ui.custom<ChainClarifyResult>(...)`. First tag **v0.21.2**
- cyrup: `crates/cyrup-ext-subagents/src/extension.rs:6634` declares the param with the description "Show TUI to preview/edit before execution."; the flag is read at `:5576-5578` (the async→foreground downgrade) and at `:9678` (suppressing the `[async]` badge) — **neither read produces a UI**
- observable: cyrup accepts `clarify: true`, forces the run foreground, and launches immediately with the model's unmodified prompt. The tool description promises a UI that does not exist. The seam it needs is live: `HostServices::open_overlay` (`crates/cyrup-ext/src/host/services.rs:224`) is already consumed in production by this same crate at `extension.rs:9908`.

**PB-10 · `turnBudget` — no soft assistant-turn budget for children** — *medium*
- upstream: `pi-subagents/src/runs/shared/turn-budget.ts:5` (`resolveTurnBudgetConfig`) and `:26` (`appendTurnBudgetSystemPrompt`); tool param at `src/extension/schemas.ts:328`. First tag **v0.33.0**
- cyrup: the tool schema built around `crates/cyrup-ext-subagents/src/extension.rs:6634` has 45 `props.insert` keys and none is `turnBudget`; `crates/cyrup-ext-subagents/src/tui/intercom.rs:348-352` hard-codes `turn_budget_exceeded: false` with a comment saying the flag has no source
- observable: no "## Turn budget" wrap-up block in the child's system prompt, no abort past `maxTurns+graceTurns`, and the result always reports `turnBudgetExceeded: false`, so an unexplained process signal is misattributed. (Frontmatter `toolBudget` **is** read, `discovery/frontmatter.rs:850` — this is the turn half only.)

**PB-11 · Scheduled subagent runs (`schedule.*`) are unported** — *large*
- upstream: `pi-subagents/src/runs/background/scheduled-runs.ts:14` (`SCHEDULED_RUN_ACTIONS`) and `:358` (`class ScheduledRunManager`), 753-line file. First tag **v0.33.0**
- cyrup: `crates/cyrup-ext-subagents/src/extension.rs:3909` states "The `schedule.*` family is unported"; the action enum at `extension.rs:6557` lists 27 verbs, none beginning `schedule.`; `extension.rs:12572` pins the enum as "pi's SUBAGENT_ACTIONS union minus the deferred schedule* four"
- observable: `subagent({action:"schedule.create", at:…})` is refused as an unknown action; no schedule store, no due-time firing, no `scheduledStopTargets` contribution to `/subagents-stop`.

**PB-12 · No live child transcript writer; the `transcriptPath` artifact is missing** — *medium*
- upstream: `pi-subagents/src/shared/child-transcript.ts:102` (`createChildTranscriptWriter`, appending per record with `fs.appendFileSync` at `:133`), created at `src/runs/background/subagent-runner.ts:1200-1201`; the field is the **fourth** `ArtifactPaths` member, `src/shared/types.ts:1048` (the interface opens at `:1044`); reported by `src/runs/background/run-status.ts:128`. First tag **v0.33.0**
- cyrup: `crates/cyrup-ext-subagents/src/artifacts.rs:61-70` — `ArtifactPaths` has four fields (input/output/jsonl/metadata) and `:58` says so; the substitute `.jsonl` is written only after the run settles (`extension.rs:4925-4928` foreground, `background/runner_main.rs:2611-2614` background). A live NDJSON stream does exist but goes elsewhere: `exec/mod.rs:2113-2118` writes `<cwd>/.cyrup-subagent-scratch/attempt-N.jsonl`
- observable: the FleetView transcript pane for a RUNNING foreground child points at `paths.jsonl_path` (`tui/fleet.rs:1041-1058`), a file that does not exist until the child finishes, so it renders empty where upstream's fills in real time; and `status`/`run-status` never print a `Transcript:` line.

**PB-13 · Chain-run artifacts default to the temp root, not the project** — *small*
- upstream: `pi-subagents/src/runs/foreground/subagent-executor.ts:2022` @v0.34.0 (`chainDir: params.chainDir ?? getProjectChainRunsDir(effectiveCwd)`), helper at `src/shared/artifacts.ts:16`. At v0.43.0 the same slot is `subagent-executor.ts:2623` via `getChainRunsDir`, whose "project" default still resolves to `getProjectChainRunsDir` (`shared/artifacts.ts:141-143`)
- cyrup: `crates/cyrup-ext-subagents/src/artifacts.rs:146` (`project_chain_runs_dir`) has **zero references of any kind**; the live resolver `resolve_chain_dir` at `crates/cyrup-ext-subagents/src/extension.rs:6539` falls back to `chain_runs_dir(cwd)` = `temp_root_dir()/chain-runs/<cwd_key>` (`artifacts.rs:164-166`)
- observable: a chain run's artifacts land under `$TMPDIR/.../chain-runs/<cwd_key>/<runId>` instead of `<cwd>/.cyrup-subagents/chain-runs/<runId>` — invisible to the project, not committable, swept by OS tmp cleanup. The `[CYRUP-DELTA]` comment at `extension.rs:6536-6538` documents only the added per-run subdirectory and is silent on the root change.

**PB-14 · The "skills not found" warning is unported on BOTH surfaces** — *small*
- upstream, run side: `pi-subagents/src/runs/foreground/execution.ts:1112` @v0.34.0 — `skillsWarning: missingSkills.length > 0 ? \`Skills not found: ${missingSkills.join(", ")}\` : undefined`, declared on the shared result shape at `:179` (v0.43.0: `execution.ts:1524`)
- upstream, management side: `src/agents/agent-management.ts:773` and `:823` @v0.34.0 call `skillsWarning(ctx.cwd, …)`, helper at `:190` (v0.43.0: `:971`, `:1023`, helper `:206`)
- cyrup: `crates/cyrup-ext-subagents/src/exec/mod.rs:3190-3193` keeps `resolution.resolved` and **discards `resolution.missing`**; `SingleResult` has no `skills_warning` field and `crates/cyrup-ext-subagents/src/artifacts.rs:427` documents omitting it. On the management side `crates/cyrup-ext-subagents/src/discovery/skills.rs:149` (`resolve_skills`) has zero callers, and the stale deferral note at `discovery/management.rs:1276-1277` still claims the skills subsystem is "entirely absent today"
- observable: `subagent({action:"create", config:{skills:"typo"}})` reports success with no warning, **and** a run with the same typo produces no warning either — the mistake surfaces nowhere. (The `Skills not found:` string at `exec/mod.rs:3180` is a different thing: the hard failure for a missing *orchestration* skill, exit 1.)

### 1c. From `pi-permission-system` v0.7.1

**PB-15 · Model-option compatibility guard (temperature stripping) is entirely unported** — *medium*
- upstream: `pi-permission-system/src/model-option-compatibility.ts:62` @v0.8.0 (`getUnsupportedTemperatureReason`), `:126` (`ensureModelOptionGuardForApi`), `:164` (`registerModelOptionCompatibilityGuard`); wired as the **first statement** of the `session_start` handler, `src/index.ts:1829` (handler opens `:1828`). `git diff v0.7.1..v0.8.0 -- src/model-option-compatibility.ts` is **empty** and v0.7.1 `index.ts:2088` makes the same call — this predates the ported baseline
- cyrup: `crates/cyrup-permission-system/src/extension.rs:1991` (the `HostEvent::SessionStart` arm) registers no guard; `crates/cyrup-provider/src/api/openai_responses.rs:359-361`, `openai_codex_responses.rs:707-708` and `azure_openai_responses.rs:386-387` insert `temperature` unconditionally; `grep -rn "does not support temperature" crates/` = 0
- observable: with the permission system installed, pi strips `temperature` for openai-codex-responses, the openai-codex provider, any `codex`-tokened model id, and any reasoning model on openai-/azure-openai-responses; cyrup sends it (its own test at `openai_codex_responses.rs:1515` asserts `body["temperature"] == 0.25`), so those requests are rejected or silently mis-parameterised. The fix is not blocked: `crates/cyrup-session-svc/src/guest_providers.rs` already exposes `register_provider`/`unregister_provider`.

**PB-16 · Permission-request events are never emitted — native extensions cannot reach the event bus** — *medium*
- upstream: `pi-permission-system/src/index.ts:150` @v0.8.0 (`PERMISSION_REQUEST_EVENT_CHANNEL = "pi-permission-system:permission-request"`), `:1518-1529` (`emitPermissionRequestEvent`), `:1531-1548` (`emitPermissionStateEvent`), fired at `:1606`, `:1612`, `:1626`. Present at v0.7.1 (`index.ts:137`, `:1753-1755`, fired `:1825`/`:1844`/`:1871`)
- cyrup: `grep -rn "events.emit\|emit_event" crates/cyrup-permission-system/src` = 0. **The bus itself exists** — `SharedBus` at `crates/cyrup-ext/src/host/services.rs:988`, with `subscribe` at `:1002` (cited to pi `event-bus.ts:18`), `emit` at `:1010`, `take_pending` at `:1018`, fanned out by `crates/cyrup-ext/src/facade.rs:1003-1026` — but it is wired to WASM guests only (`host/live.rs:642` `bus_emit`, `:650` `bus_subscribe`) and `crates/cyrup-ext/src/native.rs` contains **zero** `bus` references, so a native extension has no accessor
- observable: in pi any extension can subscribe to `pi-permission-system:permission-request` and observe every waiting/approved/denied transition with its requestId, tool, command, target and agent; in cyrup no such stream exists. The work is a native-extension bus accessor plus three emit sites — **not** a new subsystem, contrary to the older framing.

**PB-17 · The forwarding half of the security-review audit trail is unwritten (11 of 17 sites)** — *medium*
- upstream: `pi-permission-system/src/index.ts` @v0.8.0 has 18 `writeReviewEntry(` occurrences — the definition at `:200` plus **17 call sites**. Eight are the forwarding path: `:735` (`permission_forwarding.warning`/`.error`), `:1032`, `:1058`, `:1080`, `:1173`, `:1184`, `:1187`, `:1228`. All eight exist at v0.7.1 (`:1011`, `:1019`, `:1298`, `:1324`, `:1346`, `:1417`, `:1428`, `:1473`)
- cyrup: `crates/cyrup-permission-system/src/forwarding.rs` is 1125 lines and `grep -n "logger\|review\|tracing" forwarding.rs` returns **zero**; both entry points (`wait_for_forwarded_approval` `:398`, `process_forwarded_requests` `:528`) log nothing. `write_review_entry` is defined at `extension.rs:930` with 6 calls (`:977`, `:1142`, `:1179`, `:1239`, `:1278`, `:1314`)
- observable: a forwarded child ask that times out, expires, is auto-approved or is denied leaves no audit record, and every forwarding I/O failure is silent where pi writes `permission_forwarding.error`.

**PB-18 · `/permission-system` prints text instead of opening the settings modal** — *medium*
- upstream: `pi-permission-system/src/config-modal.ts:63-122` @v0.8.0 (`openPermissionSystemSettingsModal` — `ctx.ui.custom<void>` overlay at `:66` rendering the settings modal with a live `onChange`→`setConfig` loop), registered at `src/index.ts:1502-1512`. Same shape at v0.7.1
- cyrup: `crates/cyrup-permission-system/src/extension.rs:737` (`run_permission_system_command`) parses `<setting> <value>` and returns a `String`. **The in-tree rationale at `:699-706` ("HostServices exposes no custom-overlay seam") is STALE**: `HostServices::open_overlay` is `crates/cyrup-ext/src/host/services.rs:224`, implemented at `crates/cyrup-session-svc/src/host_services.rs:676`, driven by `crates/cyrup-tui/src/app.rs:6310`, and already consumed in production by `crates/cyrup-ext-subagents/src/extension.rs:9908` against `tui/fleet_overlay.rs:177` — with no `cyrup-tui` dependency in the consuming crate
- observable: `/permission-system` opens a live two-row modal in pi (arrow keys, live toggle, config-path footer); in cyrup it prints a status paragraph and the user must retype `/permission-system debug on`. This is a straight port onto an existing seam, not a blocked item.

### 1d. From `pi-intercom` v0.7.0

**PB-19 · The broker binds a Unix socket unconditionally and never consults its own listen-target resolver** — *large*
- upstream: `pi-intercom/broker/broker.ts:21` @v0.7.0 (`const LISTEN_TARGET = getBrokerListenTarget();`) with the two-branch listen at `:176-179`; helper `broker/paths.ts:107`, Windows named pipe at `paths.ts:65-74`, TCP predicate at `paths.ts:44-59`. At v0.9.2 the broker additionally publishes its endpoint (`broker/broker.ts:252-256`, `stateId: BROKER_STATE_ID`) and enforces it at `:408-409`
- cyrup: `crates/cyrup-intercom/src/broker/mod.rs:1243` — `let listener = UnixListener::bind(&socket_path)?;`, unconditional, after the stale-socket unlink at `:1242`; `:24` imports only `tokio::net::UnixListener`; `crates/cyrup-intercom/src/paths.rs:6-8` records the deferral. The ported resolver `broker_listen_target` (`crates/cyrup-intercom/src/transport/target.rs:278`) has **zero callers of any kind**. The CLIENT half is fully live: `broker_connect_target` (`target.rs:254`) is called from `transport/spawn.rs:226` and `:299`, and `transport/client.rs:202` handles all three transports
- observable: with `CYRUP_INTERCOM_TRANSPORT=tcp` (or on Windows) a cyrup client resolves a TCP/pipe endpoint while a cyrup broker only ever listens on a Unix socket and never writes `broker.port.json` — the two halves cannot meet. A cyrup client that does reach a pi broker sends a `stateId` (`transport/protocol.rs:643-651`) that a cyrup broker neither publishes nor validates.

**PB-20 · The bundled `pi-intercom` skill is not shipped or registered** — *medium*
- upstream: `pi-intercom/skills/pi-intercom/SKILL.md` @v0.7.0 — 513 lines (514 at v0.9.2), declared at `package.json:26-28` (`"pi": { "skills": ["./skills"] }`) and packed via `package.json:14`
- cyrup: `find crates/cyrup-intercom -type f ! -name '*.rs'` returns only `Cargo.toml`; `init` at `crates/cyrup-intercom/src/extension.rs:457-495` registers 2 tools and 2 commands, subscribes 8 event kinds, never subscribes `EventKind::ResourcesDiscover`, and never registers a skill
- observable: a pi session with intercom installed gets a 513-line coordination-protocol skill the model can load; a cyrup session has none. Not blocked: `EventKind::ResourcesDiscover` exists (`crates/cyrup-ext/src/event.rs:20`), is dispatched via `facade.rs:485`, and `cyrup-permission-system` already subscribes to it (`extension.rs:1900`).

**PB-21 · The session-name poll timer is unported; `CYRUP_INTERCOM_NAME_POLL_MS` is inert** — *small*
- upstream: `pi-intercom/index.ts:598-611` @v0.7.0 (`startNamePoll`, `setInterval` at `:601`, interval from `getNamePollMs()` at `:609`; helper `:421-429`). Same shape at v0.9.2 (`index.ts:461`, used `:829`)
- cyrup: `crates/cyrup-intercom/src/identity.rs:24` declares `ENV_INTERCOM_NAME_POLL_MS` and nothing reads it; `transport/client.rs:368` (`update_presence`) has no production caller; the only live presence path is `update_presence_with_context` from `sync_presence` (`extension.rs:205`), which fires on lifecycle transitions and passes no name. The label is sent once at connect (`connect.rs:444`)
- observable: renaming a session never updates its presence label for peers — other sessions' `/intercom` listings keep the old name until the client reconnects. Setting the env var does nothing.

---

## 2. Unwired — the code exists in cyrup and has no production caller

**This is the project's most common defect class and the cheapest to fix.** Batches 8–10 shipped
roughly forty such items — all with green tests, because the tests called the functions directly. A
green suite is not evidence that a subsystem runs. Every entry here is a wiring job, not a port.

**UW-1 · The native modifier probe has no production caller, so the Apple-Terminal Shift+Enter rescue never fires** — *medium*
- upstream: `pi/packages/tui/src/native-modifiers.ts:21-56` (`loadNativeModifiersHelper` loads the prebuilt darwin/win32 addon), consumed at `pi/packages/tui/src/terminal.ts:6` and used at `:324`
- cyrup: `crates/cyrup-tui/src/native_modifiers.rs:62` (`set_native_modifier_probe`) — the only call workspace-wide is `crates/cyrup-tui/tests/native_shift_enter.rs:138`. The consumer side IS wired: `app.rs:7174` calls `is_native_modifier_pressed` on the production `map_event` path
- observable: with no probe installed the predicate always answers false, so on macOS Apple Terminal Shift+Enter still submits instead of inserting a newline — the exact defect the ported code exists to fix. Mechanism note: pi `require`s a prebuilt `.node` addon; cyrup needs an OS query (`CGEventSourceKeyState`/`GetKeyState`), which is FFI and cannot live inside `#![forbid(unsafe_code)]` `cyrup-tui` — the injectable seam exists precisely for that and is fed by nothing.

**UW-2 · The first-run setup wizard is gated but never invoked — the `if` body is empty** — *small*
- upstream: `pi/packages/coding-agent/src/main.ts:615-616` @v0.83.0 (`if (appMode === "interactive" && … shouldRunFirstTimeSetup()) { await showFirstTimeSetup(startupSettingsManager); }`); unchanged at v0.84.1 `main.ts:663-664`
- cyrup: `crates/cyrup/src/main.rs:218-223` evaluates `should_run_first_time_setup` and the body (`:221-222`) is comment-only; `crates/cyrup/src/startup.rs:256` (`run_first_time_setup`) has zero callers
- observable: on a first run with `CYRUP_EXPERIMENTAL=1`, no `settings.json` and no agent-dir override, pi presents the theme + analytics wizard and persists the answers; cyrup does nothing. **The gate can fire**: `OFFICIAL_PACKAGE_NAME`/`APP_NAME`/`CONFIG_DIR_NAME` (`startup.rs:32-34`) name cyrup itself and match the live values at `:38-43`, so `is_official_distribution()` (`:71-73`) is **true** for this build. The comment at `main.rs:215-217` claiming the predicate is "faithfully false for the cyrup rebrand" is stale, and so is CLAUDE.md's "compile-time constant `false`".

**UW-3 · Child-watchdog NDJSON status events are never read by the parent** — *medium*
- upstream: `pi-subagents/src/runs/foreground/execution.ts:846` (`isChildWatchdogStatusEvent`), `:848` (`acceptChildWatchdogEvent`), `:857` and `:585` (`childWatchdogIsActive`); `src/runs/background/subagent-runner.ts:626`, `:628`, `:640`, `:831`, `:2711-2712`; definitions `src/watchdog/child-status.ts:167`, `:181`, `:186`
- cyrup: `crates/cyrup-ext-subagents/src/watchdog/child_status.rs:480`, `:497`, `:516` — production callers zero (its test module opens at `:546`). The two readers that should call them, `exec/ndjson.rs` and `background/`, contain no watchdog reference. The child EMIT side is wired (`prompt_runtime.rs:1701`; `exec/mod.rs:1727-1737` hands over `CHILD_WATCHDOG_CONFIG_ENV`)
- observable: a child mid-watchdog-review when its agent settles is killed by the ordinary final-drain timer instead of held open by the watchdog tail timer, so its blocker/concern warnings are lost. cyrup emits `subagent.watchdog.status` frames its own parent discards as an unknown event type. `child_status.rs:461-473` states this in-tree.

**UW-4 · Watchdog review never runs a model turn — every review is silently clean** — *medium*
- upstream: `pi-subagents/src/watchdog/review.ts:295` — `await agent.prompt(buildReviewPrompt(request, selection))` inside the closure created by `createMainWatchdogReview` (`:249`)
- cyrup: `crates/cyrup-ext-subagents/src/watchdog/review.rs:871` (`NoTurnReviewAgent`) whose `run` returns `Ok(Vec::new())` at `:876`; it is the agent bound in BOTH production paths — `watchdog/register_main.rs:169` (orchestrator) and `prompt_runtime.rs:1761` (child)
- observable: the whole `watchdog/` subtree is wired — nine subscriptions at `extension.rs:9338-9352`, `/subagents-watchdog` at `:9326`, four `watchdog.*` verbs at `:7567-7570` — and a review model is resolved on every agent-end boundary, but no warning can ever be emitted. `/subagents-watchdog status` reports "real model review" (`register_main.rs:191`) over a machine that cannot produce a finding.

**UW-5 · Watchdog permission arbiter never runs a model turn — every `ask` denies** — *medium*
- upstream: `pi-subagents/src/watchdog/permission-arbiter.ts:41` (`createWatchdogPermissionArbiter`) constructing `new Agent({… streamFunction })` at `:102`, exported as `requestWatchdogPermission` at `:145`
- cyrup: `crates/cyrup-ext-subagents/src/watchdog/permission_arbiter.rs:587` (`NoDecisionPermissionAgent`, returning `Ok(None)` at `:595`), bound at `prompt_runtime.rs:1734` — the sole production construction of the gate
- observable: a child tool whose policy tier is `ask` is denied with the `malformed` reason ("Watchdog permission arbiter returned no decision.", `permission_arbiter.rs:734`) instead of being adjudicated. Fail-closed is the right direction, but no `ask`-tier tool can ever succeed inside a subagent.

**UW-6 · Nothing ever ships a permission policy to a child — the child-side gate is inert** — *medium*
- upstream: `pi-subagents/src/runs/shared/permissions.ts:40` (`resolvePermissionRules`) and `:51` (`encodePermissionRules`), written into the child env at `src/runs/shared/pi-args.ts:730` (audit path) and `:758` (policy)
- cyrup: `crates/cyrup-ext-subagents/src/exec/mod.rs:1376` (`build_attempt_spawn_plan`) builds the child env overlay through `~:1874` — structured-output vars, `TOOL_BUDGET_ENV` (`:1840-1846`), steer inbox, supervisor channel, required child tools — but never `CYRUP_SUBAGENT_PERMISSION_POLICY` / `…_PERMISSION_AUDIT_PATH` (defined at `watchdog/permission_arbiter.rs:358`, `:361`). The only writer workspace-wide is a test stub at `prompt_runtime.rs:1949-1950`; the reader is `prompt_runtime.rs:1716-1717`
- observable: `with_permission_gate` always receives `None`, so no child tool is ever checked against agent/config permission rules and no `permission.request`/`permission.decision` audit record is ever appended. `permission_arbiter.rs:56-63` admits this in-tree.

**UW-7 · The fleet-status widget receives no keystrokes** — *medium*
- upstream: `pi-subagents/src/tui/fleet-status.ts:282-283` (`ui.onTerminalInput((data) => this.handleKey(data))`), handler at `:352` (down/left activates only when the editor is empty; Enter on a non-`main` row opens the inspector)
- cyrup: `crates/cyrup-ext-subagents/src/tui/fleet_status.rs:764` (`handle_key`) has zero production callers (13 test callers; the test module opens at `:1174`), as do `press` (`:1169`) and `is_widget_registered` (`:711`). The host has no seam to wire it to: `crates/cyrup-ext/src/host/services.rs` has `set_widget` (`:260`) and `open_overlay` (`:224`) but nothing resembling `on_terminal_input`. The PUBLISH half is live (`extension.rs:9489`, `:9889`, `:9978`)
- observable: while subagents run, pressing ↓ or ← on an empty editor expands pi's widget into a selectable roster whose Enter opens the fleet inspector; in cyrup the widget is display-only and those keys fall through to the editor.

**UW-8 · Mission workflow state is never written** — *small*
- upstream: `pi-subagents/src/missions/workflow-state.ts:17` (`missionStatePath`); its only consumer is the `workflowScript` runtime at `src/runs/foreground/subagent-executor.ts:4139`, which exposes `state.get`/`state.set`
- cyrup: `crates/cyrup-ext-subagents/src/missions/workflow_state.rs:209` (`get`) and `:222` (`set`) have no non-test caller. The **read** half of the path IS live — `mission_state_path` is called from `missions/goal_driver.rs:372` (deriving a mission's next ready action) and `missions/actions.rs:908` (rendering `State: <path>` for `mission.show`)
- observable: `<missionDir>/<missionId>/state.json` is never written, so `goal_driver`'s production read always finds a missing file and falls through to the decisions list, and `mission.show` advertises a path that never exists. Closes for free the moment `workflowScript` (VL-S2) lands.

**UW-9 · The yolo-mode runtime API has no publish seam and no caller** — *medium*
- upstream: `pi-permission-system/src/index.ts:1480-1484` @v0.8.0 (`registerPiPermissionSystemRuntimeApi({getYoloMode,setYoloMode,toggleYoloMode})`); `src/yolo-mode-api.ts:23-29` publishes it on `globalThis.__piPermissionSystem`, `:40-43` reads it back. `git diff v0.7.1..v0.8.0 -- src/yolo-mode-api.ts` is empty
- cyrup: `crates/cyrup-permission-system/src/extension.rs:608` (`yolo_mode`), `:628` (`set_yolo_mode`), `:693` (`toggle_yolo_mode`). `set_yolo_mode` has exactly one caller — `:694`, inside `toggle_yolo_mode` — and `toggle_yolo_mode` has none. The `/permission-system` yoloMode row deliberately routes through `save_extension_config` instead (`:783-790`, rationale `:721-728`). `crates/cyrup-ext/src/native.rs:318` (`trait NativeExtension`) exposes `id`/`init`/`on_event`/`execute_command` — no way to publish a callable API object
- observable: in pi another extension can read or flip yolo mode through the published API; in cyrup the three methods compile, are documented and tested, and cannot be invoked in production. Unlike PB-16 this one genuinely needs a new seam — `SharedBus` is an event bus, not a callable-API registry.

**UW-10 · The intercom compose and session-picker overlays are render-only** — *medium*
- upstream: `pi-intercom/index.ts:1857` @v0.7.0 (`new SessionListOverlay(...)`) and `:1874` (`new ComposeOverlay(...)`), both handed to `ctx.ui.custom`; classes at `ui/session-list.ts:44` and `ui/compose.ts:13`
- cyrup: `crates/cyrup-intercom/src/ui/compose.rs:86` (`handle_input`), `:74` (`set_sending`), `:79` (`set_error`) and `ui/session_list.rs:75` (`handle_input`) have zero production callers (test modules open at `compose.rs:222` / `session_list.rs:184`). The only production use is a one-shot `render` at `crates/cyrup-intercom/src/extension.rs:404-407` whose output ends "Type `/intercom {target} <message>` to send." The rationale at `ui/mod.rs:12-19` predates `open_overlay`
- observable: `/intercom <target>` prints a picture of a compose box and asks the user to retype the whole command with a body; pi opens a live overlay where you type, see "Sending…", and see delivery errors inline. Arrow-key selection in the picker is likewise unreachable. Caveat: `ui/mod.rs` also names the missing `register_shortcut` on `InitApi` — the `alt+m`-triggered path stays blocked by VL-S15; only the slash-command path is reachable today.

---

## 3. Version lag — upstream added it after cyrup's ported baseline

Still work. An item here is in scope for the next version bump, not out of scope.

### 3a. `pi` v0.83.0 → v0.84.1 (25)

**VL-P1 · `baseten` provider, `thinkingFormat: "baseten"` and `compat.chatTemplateArgs`** — *medium*
`pi/packages/ai/src/providers/baseten.ts:6-14`; wire encoding `api/openai-completions.ts:779-795`; types `ai/src/types.ts:565` and `:574-575` — vs `crates/cyrup-provider/src/api/compat.rs:28-39` (`ThinkingFormat`, 10 variants, no `Baseten`) and `compat.rs:73-168` (`ModelCompat`, no `chat_template_args`). `grep -rni baseten crates/` = 0. **Observable**: a Baseten model is unreachable, and any `models.json` carrying `thinkingFormat:"baseten"` fails to deserialize the compat block (the enum has no `#[serde(other)]`) rather than emitting `chat_template_args` + `reasoning_effort`.

**VL-P2 · `qwen-token-plan-individual` provider** — *small*
`pi/packages/ai/src/providers/all.ts:120` (file absent at v0.83.0) — vs `crates/cyrup-provider/src/providers/all.rs:141-243`. **Observable**: cyrup registers **35** built-in providers against v0.84.1's **40** (16 openai-completions fleet + 4 anthropic fleet + 15 individual pushes). A user on the Qwen individual plan has no provider. *(CLAUDE.md's "38 of 40" is wrong; three of the five missing — PB-1, PB-2 — are port bugs, not lag.)*

**VL-P3 · `samplingParams` are carried nowhere** — *medium*
`pi/packages/ai/src/api/simple-options.ts:27-33` (model merged under request), declared `types.ts:189` (`StreamOptions`) and `types.ts:802` (`Model`), applied `api/openai-completions.ts:885-886`, `openai-responses.ts:331-332`, `azure-openai-responses.ts:325-326`, composed from user config at `coding-agent/src/core/provider-composer.ts:123-125` — vs `crates/cyrup-provider/src/utils/simple_options.rs:61-98` (threads 20 fields, none of them sampling params); no such field on `stream.rs` `StreamOptions` or `model.rs` `Model`. **Observable**: `top_p`/`top_k`/`repetition_penalty` in a `models.json` entry or a per-request option are silently dropped and the provider is called with defaults.

**VL-P4 · vLLM `thinking_token_budget` via `compat.supportsThinkingTokenBudget`** — *medium*
`pi/packages/ai/src/types.ts:583`; applied `api/openai-completions.ts:851-866` (with a `MIN_ANSWER_TOKENS` floor) — vs `crates/cyrup-provider/src/api/compat.rs:73-168`. **Observable**: against a vLLM-backed endpoint an uncapped reasoning phase can consume the whole `max_tokens` and leave no answer and no tool call; pi caps it at the level budget minus a 1024-token answer floor.

**VL-P5 · `telemetryContext` on request options** — *small* (lowest-value entry here; embedder-facing only)
`pi/packages/ai/package.json:65` (telemetry became a runtime dep of `packages/ai` at v0.84.1), `ai/src/types.ts:122-123`, threaded `api/simple-options.ts:36` — vs `crates/cyrup-provider/src/stream.rs` `StreamOptions` (no telemetry field); zero `TelemetryContext` hits in `crates/`. **Observable**: an embedder cannot attach a parent span to a logical request, so provider calls emit no correlated telemetry. Note this puts `packages/telemetry` inside the port's dependency closure; `packages/{server,storage,evals}` remain outside it.

**VL-P6 · Auth operations take no cancellation signal, and OAuth refresh has no 15 s bound** — *large*
`pi/packages/ai/src/auth/types.ts:45-48` (`AuthOperationOptions`) threaded onto every `CredentialStore` method at `:70`, `:76`, `:86-90`, `:93`; `DEFAULT_OAUTH_REFRESH_TIMEOUT_MS = 15_000` at `auth/resolve.ts:120`, raced at `:149-153` — vs `crates/cyrup-provider/src/auth/store.rs:24-54` (no options argument) and `crates/cyrup-provider/src/auth/resolve.rs:157-231`, whose refresh at `:198` is unbounded. **Observable**: a hanging OAuth refresh holds the per-provider `modify` lock and blocks every other request for that provider. (`CredentialStore::list` is present at `store.rs:41` with production consumers — only the cancellation half is open.)

**VL-P7 · GitHub Copilot Individual: no policy-state fallback** — *small*
`pi/packages/ai/src/auth/oauth/github-copilot.ts:92-113` (`parseAvailableCopilotModelIds` builds `pickerIds` AND `policyEnabledIds`, returning the latter when the former is empty), endpoint gate `:115-133` — vs `crates/cyrup-provider/src/providers/github_copilot.rs:310-326` and `:331` (single list, no fallback, no gate). **Observable**: a Copilot Individual account whose `/models` rows all report `model_picker_enabled:false` but `policy.state:"enabled"` gets an empty model list.

**VL-P8 · A blocked tool call cannot request early termination of the batch** — *small*
`pi/packages/agent/src/types.ts:68` (`BeforeToolCallResult.terminate`), consumed `agent-loop.ts:636-646` — vs `crates/cyrup-agent/src/hooks.rs:49-52` (`BeforeOutcome::{Proceed,Block}`) and `crates/cyrup-agent/src/agent.rs:1030` (blocked result hardcodes `terminate: false`). **Observable**: a gate that denies every call in a batch cannot stop the turn; the loop feeds the errors back and the model gets another turn. The AFTER-hook half is already ported (`hooks.rs:69`, `:94`; honoured `agent.rs:534`).

**VL-P9 · `Agent::reset()` does not reject mid-run** — *small*
`pi/packages/agent/src/agent.ts:333-336` (throws when `activeRun`; no guard at v0.83.0 `agent.ts:326`) — vs `crates/cyrup-agent/src/agent.rs:1604-1616`, whose own doc at `:1601` says it clears "unconditionally, even mid-run". **Observable**: `reset()` during a stream wipes `messages`/`streaming_message`/`pending_tool_calls` under the live run, which keeps writing into a cleared transcript.

**VL-P10 · No compact-and-retry after a recoverable `length` stop** — *medium*
`pi/packages/coding-agent/src/core/agent-session.ts:1993-1994` (`recoverableLength` joins the Case-1 condition); predicate `packages/ai/src/utils/overflow.ts:171-173` — vs `crates/cyrup-session-svc/src/session.rs:4160`, where the term is absent though the recovery scaffold at `:4161-4182` is ported. **Observable**: a turn truncated by `length` below the model's own output cap ends there and the user must re-prompt; pi compacts once, drops the truncated assistant message, and continues.

**VL-P11 · A prompt submitted during a manual compaction is accepted instead of rejected** — *small*
`pi/packages/coding-agent/src/core/agent-session.ts:1133-1137` — vs `crates/cyrup-session-svc/src/session.rs:623-628` (guards only `is_streaming`); `is_compacting` exists at `session.rs:4110` and its one production consumer is a status field at `crates/cyrup-modes/src/rpc.rs:1428`. **Observable**: submitting while `/compact` runs starts a turn against a transcript compaction is about to replace; pi throws.

**VL-P12 · Images returned by tool results are never normalized/auto-resized** — *medium*
`pi/packages/coding-agent/src/utils/tool-result-images.ts:22-62`, called immediately after the `tool_result` extension hook at `core/agent-session.ts:518-520` — vs `crates/cyrup-ext/src/hooks.rs:58-113` (`after_tool_call` does a pure field diff, no image pass). The primitive exists (`crates/cyrup-tools/src/tools/read.rs:265`) and the setting is plumbed (`crates/cyrup-session-svc/src/builder.rs:681`). **Observable**: an extension/MCP/screenshot tool returning an oversized base64 image puts it into history verbatim, so every later request carries it and the provider can reject the whole conversation.

**VL-P13 · An ambiguous bare `--model` silently picks the first catalog match** — *small*
`pi/packages/coding-agent/src/core/model-resolver.ts:469-503` (collects all exact matches, prefers the sole authenticated one, else errors `Model "…" is ambiguous across providers: …`) — vs `crates/cyrup-config/src/model.rs:1139-1143` (`all.iter().find(...)`, first match wins). **Observable**: `--model glm-4.7` (offered by six providers in cyrup's own catalogs) binds to whichever sorts first, possibly one with no credential.

**VL-P14 · `auth check` subcommand is unrecognized** — *medium*
`pi/packages/coding-agent/src/cli/auth-command.ts:51` (the `check` arm; usage `:18`, `:42`), implementation `cli/auth-check.ts:22-73` with the result shape at `:16-20` — vs `crates/cyrup/src/credential_print.rs:149-155`, which matches only `print-api-key`/`print-bearer-token`. **Observable**: `cyrup auth check --provider openai --json` exits non-zero with a usage error; pi answers `{status:"ready"|"not_ready"|"invalid", …}` without minting or refreshing a token unless asked.

**VL-P15 · A malformed `pi` block in `package.json` hard-fails the install** — *small*
`pi/packages/coding-agent/src/core/pi-manifest.ts:16-34` (whole body in `try/catch` returning `null`; `:26` skips any field that is not an array of strings) — vs `crates/cyrup-resources/src/package/manifest.rs:87` (`serde_json::from_str(&text)?`) and `:80` for the `cyrup.toml` branch. **Observable**: `"pi": {"extensions": [1,2]}` aborts the whole install with a serde error; pi drops the bad field and installs the rest.

**VL-P16 · Management HTTP fetches are not retried** — *small*
`pi/packages/coding-agent/src/utils/management-http.ts:25-68` (`fetchWithRetry`, 2 extra attempts, retryable set at `:3`), callers `core/remote-catalog-provider.ts:81`, `utils/version-check.ts:57`, `utils/tools-manager.ts:109` and `:127` — vs `crates/cyrup-provider/src/remote_catalog.rs:544-547` (one `request.send()`; any transport error or 5xx is terminal). **Observable**: a single transient 503 leaves cyrup on the stale cached catalog for the whole freshness window.

**VL-P17 · Terminal colour-scheme and background probes run sequentially** — *small*
`pi/packages/coding-agent/src/modes/interactive/theme/theme.ts:796-810` (both promises started, then awaited) — vs `crates/cyrup-tui/src/theme.rs:1334-1343` (early return, then fall through). **Observable**: on a terminal answering neither DSR ?996 nor OSC 11, `auto` detection blocks for two full timeouts at startup where pi blocks for one. Note cyrup's `TerminalProbe` is a synchronous `&dyn` trait, so the remedy is larger than the symptom.

**VL-P18 · `tui.editor.historyPrevious` / `historyNext` are not rebindable actions** — *small*
`pi/packages/tui/src/keybindings.ts:68-75` (both with `defaultKeys: []`), consumed `components/editor.ts:768-777` ahead of the cursor-movement arms — vs `crates/cyrup-tui/src/keymap.rs:157-186` (`EditorAction::from_id`, 24 ids, neither history id); recall exists only as the Up/Down-at-buffer-edge fallthrough over the state at `crates/cyrup-tui/src/editor.rs:90-93`. **Observable**: binding `"tui.editor.historyPrevious": "ctrl+p"` does nothing — the id resolves to `None`. In pi it browses history unconditionally, including from mid-buffer.

**VL-P19 · The fullscreen (alternate-screen) TUI program is entirely absent** — *large*
`pi/packages/tui/src/tui-alt-screen.ts` (1047 lines, new at v0.84.1) with `components/scroll-view.ts` (195) and `alt-screen-flash.ts` (51); entry `coding-agent/src/cli/args.ts:180-186` (`--tui-mode`), settings `core/settings-manager.ts:135-136` (getter from `:1128`), switch `modes/interactive/interactive-mode.ts:345`, 8 `tui.altScreen.*` bindings at `packages/tui/src/keybindings.ts:44-52` — vs `crates/cyrup-tui/` (no alt-screen module; `app.rs:7202` is `Event::Mouse(_) => None`; no `tui_mode` anywhere). **Observable**: `--tui-mode fullscreen` is an unknown flag; no pinned viewport, draggable scrollbar, wheel scroll, drag selection, OSC 8 link clicks, OSC 133 prompt jumping or flash notifications. Mechanism note: ratatui supports the alternate screen and mouse capture natively — the gap is the application layer.

**VL-P20 · Mermaid fences are not rendered and `markdown.mermaid` is not a setting** — *large*
`pi/packages/coding-agent/src/modes/interactive/components/mermaid.ts:14-30` (fence predicate `:15`), setting `core/settings-manager.ts:61` (getter from `:1251`) — vs `crates/cyrup-tui/src/markdown.rs:964-965`, whose two lines are the only "mermaid" occurrences in the crate and are a comment quoting upstream's predicate. **Observable**: a ` ```mermaid ` fence renders as a plain code block; pi renders box-drawn Unicode art with an off/streaming/final switch.

**VL-P21 · `registerMarkdownTransformer` and the transform pipeline are absent** — *large*
`pi/packages/coding-agent/src/core/extensions/types.ts:1153` (`MarkdownTransformer`), `:1292` (`registerMarkdownTransformer`), `:1703` (on the definition); pipeline `modes/interactive/components/markdown-transform.ts:3-29` (fail-open per transformer, width-aware) — vs `crates/cyrup-ext/wit/world.wit` (no transformer import/export; only the `markdown` widget at `:159`) and `crates/cyrup-tui/src/markdown.rs` (no transform seam). **Observable**: an extension cannot rewrite assistant Markdown before render — which is also why VL-P20 has nowhere to attach, since upstream ships mermaid AS a registered transformer.

**VL-P22 · A torn session-JSONL tail is never repaired, and fork is not published atomically** — *large*
`pi/packages/agent/src/harness/session/jsonl/storage.ts:33-46` (`publishFileAtomically`), `:83-90` (a final unparseable line truncates the file to its valid prefix), `:93-95` (unterminated tail gets a newline), `:99-109` (fork stages then renames) — vs `crates/cyrup-session/src/manager.rs:851-888` (`load` skips malformed lines and returns `recovered`) and `manager.rs:114-117`, where the rewrite is gated `if migrated && !recovered`, so a recovered file is provably never rewritten. `store.rs:68-82` does stage+rename for full rewrites, but `create_exclusive` (`store.rs:86-116`) writes header+entries straight to the destination fd. **Observable**: after a crash mid-append, cyrup re-parses and re-skips the same torn line on every open, forever, and keeps appending after it.

**VL-P23 · `packages/protocol` + `packages/client` entered the closure — no remote-session wire format** — *large*
`pi/packages/coding-agent/package.json:48-49` now depends on `pi-client` and `pi-protocol` (neither at v0.83.0), consumed at `coding-agent/src/client/remote-session.ts:7`, `:17`; sources `packages/protocol/src/{cbor,codec.ts,framing.ts,schemas.ts}` and `packages/client/src/*` — vs no `cyrup-protocol`/`cyrup-client` crate and nothing in `crates/` decoding framed CBOR. **Do not conflate** `crates/cyrup-intercom/src/transport/framing.rs` with it: that is a line/JSON protocol capped at 1 MiB (`:19`). **Observable**: cyrup can neither serve nor attach to a remote pi session.

**VL-P24 · `CredentialSynchronizationError` and serialized, cancellable credential operations** — *medium*
`pi/packages/coding-agent/src/core/model-runtime.ts:94-111` (the error class) and `:494` (`enqueueCredentialOperation`, serializing per provider and racing an abort signal; `synchronizeCredentialState` recomposes, does a `providers:[id]` offline refresh, then re-checks availability) — vs zero `CredentialSynchronizationError` hits in `crates/`; `/login` and `/logout` in `crates/cyrup-tui/src/app.rs` have no per-provider operation queue. **Observable**: a `/login` whose credential write succeeds but whose catalog/composition refresh fails reports plain success and leaves the model list inconsistent with the stored credential. Depends on PB-3's `providers:[id]` refresh scope.

**VL-P25 · Catalog set trails the provider set** — *small*
cyrup embeds **35** catalogs under `crates/cyrup-provider/src/providers/catalog/*.json` against v0.84.1's **39** `*.models.ts` (Together's is hand-written Rust). The four with no counterpart are `baseten`, `qwen-token-plan`, `qwen-token-plan-cn`, `qwen-token-plan-individual` — i.e. this closes with PB-2, VL-P1 and VL-P2. **Per-model accuracy beyond that cannot be audited from this workspace**: pi does not commit `packages/ai/src/providers/data/*.json` (every `*.models.ts` is a two-line re-export of a file generated by `npm run generate-models` from the network). Claims about pricing/context-window drift in the other 35 need a `generate-models` run. See OQ-5.

### 3b. `pi-subagents` → v0.43.0 (15)

**VL-S1 · No capability ceiling on child tools/agents/extensions** (v0.37.0) — *medium*
`src/runs/shared/capability-ceiling.ts:5`, `:95`, `:106` (209 lines); env write and the `MCP_DIRECT_TOOLS="__none__"` forcing at `src/runs/shared/pi-args.ts:741-756` — vs `crates/cyrup-ext-subagents/src/exec/mod.rs:1428`, the single workspace-wide mention, a comment reading "no capability ceiling in this port". **Observable**: a grandchild inherits its parent's full tool/extension surface; upstream clamps monotonically and stamps the ceiling so the child cannot re-widen.

**VL-S2 · `workflowScript` runtime (and `chatProgress`)** (v0.41.0) — *large*
`src/workflows/scripted-workflow.ts:311` (`runWorkflowScript`, 502 lines) plus `src/workflows/chat-progress.ts` (140); tool params `src/extension/schemas.ts:317`, `:318` — vs `crates/cyrup-ext-subagents/src/extension.rs:5327-5338` ("the identifier appears nowhere in this crate") and `missions/workflow_state.rs:26-30`. **Observable**: the model cannot express a dynamic workflow (`runs.run`/`runs.all`/`emit`/`state.get`/`state.set`), so mixed sequential/parallel phases and per-child gates are unreachable; conversely cyrup still exposes the `tasks`/`chain`/`concurrency`/`chainDir` shapes v0.43.0 removed.

**VL-S3 · Session lease — two runners can own one session file** (v0.35.0) — *medium*
`src/runs/shared/session-lease.ts:9`, `:59`, `:208` (299 lines); acquired `src/runs/background/subagent-runner.ts:4618`, released `:4648` — vs zero lease machinery anywhere in `crates/cyrup-ext-subagents/src`. **Observable**: nothing prevents two runner processes writing one async session file concurrently, and there is no dead-owner reclaim on the next revival.

**VL-S4 · Process-terminal record — a killed runner leaves an ambiguous run** (v0.37.0) — *medium*
`src/runs/background/process-terminal.ts:52`, `:163`, `:216` (280 lines) — vs `grep -ric 'process.terminal' crates/cyrup-ext-subagents/src` = 0; run state comes from `background/run_status.rs` and `background/reconcile.rs`. **Observable**: when a runner dies without writing a result, upstream still reports a definite terminal cause; cyrup can only report the reconciled "stale" guess, so `status` cannot distinguish a crash from a slow start.

**VL-S5 · Revival does not restore the child's effective config** (v0.35.0) — *small*
`src/runs/background/async-execution.ts:1358` builds a `SteeringRecoveryDescriptor` and `:1401` persists it as `recovery-descriptor.json`; `src/runs/background/async-resume.ts:276` reads it back and `:501-524` re-applies model, fallbackModels, thinking, tools, extensions, mcpDirectTools, systemPrompt, skills, completionGuard, memory, output, toolBudget and maxSubagentDepth — vs cyrup, which writes no descriptor (`grep -rn 'recovery_descriptor' crates/cyrup-ext-subagents/src` finds only two doc lines) and rebuilds the revived step with `model: None, tools: None, extensions: None` at `crates/cyrup-ext-subagents/src/extension.rs:4269-4285`, re-resolving the persona from disk. **Observable**: a run launched with per-call `model`/`tools`/`toolBudget` overrides revives without them. *(Revival ITSELF is ported and works — `ResumeOutcome::RespawnFromTranscript` at `background/control.rs:1214`, routed at `extension.rs:4210-4212` into `revive_from_transcript` at `:4232`, with tests at `control.rs:2698`/`:2758` and `extension.rs:18758`. The 2026-08-08-era claim that resume refuses terminal runs is FALSE and was dropped.)*

**VL-S6 · Herdr inspector subsystem** (v0.41.0) — *large*
`src/inspectors/herdr/actions.ts:15` (`HERDR_INSPECTOR_ACTIONS`) and `:158`, plus `client.ts` (130), `inspector-runner.ts` (141), `project-panes.ts` (154), `src/integrations/herdr-status.ts` (330) — vs `crates/cyrup-ext-subagents/src/tui/fleet.rs:1654` ("Herdr inspector controls are unavailable in this context."), the hard-coded `false` passed at `extension.rs:9863`, and no `inspector.*` verb in the enum at `extension.rs:6557`. **Observable**: the FleetView's advertised `H` key (footer at `tui/fleet.rs:2025`) always answers "unavailable".

**VL-S7 · Authority policy (confirm/forbid gates)** (v0.41.0) — *medium*
`src/policy/authority.ts:1-8` (`AUTHORITY_ACTIONS`), `:14-21` (defaults — discardWorktree/destructiveCleanup/spawnBudgetGrant default to `confirm`), `:23`, `:30`; consumed by `src/inspectors/herdr/actions.ts:205-206` (`allowSteer`/`allowStop`) and validated at `src/extension/config.ts:26` — vs `crates/cyrup-ext-subagents/src/extension.rs:7574` (a doc line naming upstream's dispatch arm) and no `authorityPolicy` config key anywhere. **Observable**: cyrup accepts no `authorityPolicy`, and its fleet steer/stop are unconditional. Scope caveat: upstream's `discardWorktree` gate hangs off a `worktree.discard` action cyrup does not have, so that arm has nothing to attach to yet; cyrup's own cleanup force-removes (`spawn/worktree.rs:813`).

**VL-S8 · Wait tool is still `wait`; no non-blocking subscriptions, no auto-drain** (v0.35.0/v0.41.0) — *large*
`src/runs/background/wait-tool.ts:9` (`name: "subagent_wait"`), backed by `subagent-wait.ts` (651), `wait-config.ts` (36) and `auto-drain.ts` (67) at v0.35.0 plus `wait-subscriptions.ts` (253) at **v0.41.0** — vs `crates/cyrup-ext-subagents/src/extension.rs:6704` (`WAIT_TOOL_NAME: &str = "wait"`, whose doc at `:6701-6703` dates the upstream rename to eight days after v0.34.0) and zero auto-drain/subscription hits. **Observable**: a child prompted by upstream's tool description calls `subagent_wait` and gets "unknown tool"; there is no `{id, nonBlocking:true}` wake subscription, and a headless run does not auto-drain at `agent_end`, so results can be lost when the turn ends.

**VL-S9 · `usageBudget`** (v0.39.0) — *small*
`src/runs/shared/usage-budget.ts:14`, `:44`, `:61` (65 lines); tool param `src/extension/schemas.ts:330` — vs zero hits in the crate and no such key among the 45 schema properties. **Observable**: a run cannot be capped by cost/token spend.

**VL-S10 · Parallel worktree handoff manifests** (v0.36.0) — *medium*
`src/runs/shared/parallel-handoff.ts:74`, `:158`, `:162`, `:183` (238 lines); `handoffPath` tool param at `src/extension/schemas.ts:274` — vs `crates/cyrup-ext-subagents/src/spawn/parallel.rs` (no manifest writer) and only three incidental mentions (`missions/types.rs:160`, `registration/resources.rs:175`, `registration/prompt_workflows.rs:611`). **Observable**: after a parallel run with `worktree: true` there is no handoff manifest, no `handoffPath` to hand preserved worktrees to a follow-up, and no `discardPreservedWorktrees` cleanup — the branches are left for the user to find by hand.

**VL-S11 · Three slash commands missing: `/subagents`, `/subagents-refine`, `/subagents-detach`** (v0.35.0/v0.43.0/v0.39.0) — *medium*
`src/slash/slash-commands.ts:651`, `:701`, `:724`; the admin surface is `src/slash/subagents-admin.ts` (432 lines) — vs the 16-variant match at `crates/cyrup-ext-subagents/src/registration/slash_commands.rs:127-145`, which has none of the three. **Observable**: no interactive admin surface for an agent's model/thinking/prompt, no way to detach a live foreground run from a slash command, no refinement overlay generation.

**VL-S12 · Four slash commands upstream deleted at v0.41.0 are still registered** — *small*
`git grep -oh 'registerCommand("[a-z-]*"' <tag> -- src` gives 19 unique names at v0.40.0 including `chain`, `parallel`, `run-chain`, `chain-prompts`, and 15 at v0.41.0 with all four gone (still gone at v0.43.0) — vs `crates/cyrup-ext-subagents/src/registration/slash_commands.rs:128` (`Chain`), `:129` (`Parallel`), `:130` (`RunChain`), `:142` (`ChainPrompts`). **Observable**: cyrup's palette advertises four commands upstream no longer has, whose function moved into `workflowScript`.

**VL-S13 · Agent refinement WRITE half** (v0.43.0) — *medium*
`src/agents/agent-refinements.ts:349` (`collectBoundedRefinementEvidence`), `:448` (`validateRefinementProposal`), `:546` (`handleRefinementAction`) — vs `crates/cyrup-ext-subagents/src/exec/agent_refinements.rs:12-20`, which states the port is the read half only, and no `refine*` verb in the enum at `extension.rs:6557`. **Observable**: an overlay written by upstream (or by hand) is applied correctly at spawn (`exec/mod.rs:1565`), but cyrup can never generate or roll one back.

**VL-S14 · `runner: external-cli` agents unsupported** (v0.41.0) — *medium*
`src/runs/shared/external-cli-runner.ts:12`, `:26`; `src/api/external-runs.ts` (129 lines); refusal text `src/runs/foreground/subagent-executor.ts:5023` — vs `crates/cyrup-ext-subagents/src/discovery/frontmatter.rs` (no `runner` key) and `discovery/types.rs` (no `runner`/`external` field); the sole trace is a doc citation at `background/runner_main.rs:4020`. **Observable**: `runner: {type:'external-cli'}` in frontmatter parses as if absent and the agent is launched as an ordinary cyrup re-exec instead of shelling out to the declared CLI (or being refused, as upstream does for foreground/clarify).

**VL-S15 · Native extensions cannot register a keyboard shortcut** — *small*
`src/slash/slash-commands.ts:719-722` (`pi.registerShortcut(Key.ctrlAlt("f"), … showFleet(ctx))`) — vs `crates/cyrup-ext/src/native.rs:240-297` (`InitApi` exposes `subscribe`, `register_tool`, `register_command` and three renderer registrations; no `register_shortcut`). The WASM-guest path HAS one (`crates/cyrup-ext/src/host/live.rs:98`), which proves the seam can carry it. **Observable**: the fleet inspector opens only by typing `/subagents-fleet`; Ctrl+Alt+F has no counterpart, and the same limit blocks every other native-extension shortcut (`crates/cyrup-intercom/src/extension.rs:465` records the identical complaint).

### 3c. `pi-intercom` v0.7.0 → v0.9.2 (6)

**VL-I1 · Broker has no mailbox** — *medium*
`broker/broker.ts:40-41` (24 h retention / 256 messages), `:219` (`mailboxMessages`), `:775` and `:1002` (`queueMailboxMessage`), `:992` (prune), `:1020` (`flushMailboxForSession`, called on register at `:510`), `:1110` (`findDisconnectedSessions`); `:784-791` refuses only when >1 disconnected session shares the name — vs `crates/cyrup-intercom/src/broker/mod.rs:792-797`, which replies `DeliveryFailed{reason:"Session not found"}` the moment no LIVE session resolves; the absence is documented at `:590`, `connect.rs:46` and `reply_tracker.rs:268`. **Observable**: sending to a named session that just restarted is lost; pi queues and delivers on reconnect.

**VL-I2 · Message receipts, receiver-side dedupe and delivery metadata** — *large*
`types.ts:49-56` (`MessageReceipt`); `index.ts:446` (`formatInboundDeliveryMetadata`), `:503`, `:515` (dedupe), `:533` (`emitMessageReceipt`), `:564`, `:588` ("Last known delivery state"), fired `:880`, `:887`, `:939`, `:949-950`, `:954`, `:961`, `:974`; `broker/client.ts:773`; broker routes `broker/broker.ts:698`, `:773`, `:809`, `:1053` — vs `crates/cyrup-intercom/src/inbound.rs:347-386` (no timestamps stamped, no `(from.id, message.id)` dedupe, no receipt), `transport/client.rs:635-640` (decodes and `tracing::debug!`s only), `broker/mod.rs:447-456` (validates then returns; rationale `:441-446`), `session_state.rs:295-298` (the v0.7.0 timeout string with no delivery-state clause). The envelope fields themselves ARE modelled (`transport/protocol.rs:301-347`). **Observable**: a pi peer sending to cyrup gets no receipts, so its `ask` timeout reports an unknown delivery state; a duplicate message id is injected twice; and cyrup's injected body omits the `id … seq … sent … delivered … injected` metadata line.

**VL-I3 · No cancel / supersede / retry controls** — *medium*
`index.ts:1795` (8-action enum including `cancel`), `:1813`/`:1816`/`:1819` (`messageId`/`supersedes`/`retryOf`), `:1927` (the `cancel` case), `:551-562` (`handleMessageControl`); `broker/client.ts:738` (`cancelMessage`); `broker/broker.ts:642` (supersede validation), `:822-866` (cancel: mailbox removal, route lookup, control frame, `delivered` ack) — vs `crates/cyrup-intercom/src/tools/intercom.rs:388` (six actions), `:26-36` (`IntercomParams`: action/to/message/attachments/reply_to), `:315` (unknown-action error), `transport/client.rs:46-57` (`SendOptions` without supersedes/retry_of/sender_sequence), and `broker/mod.rs:595-607`, where `handle_cancel_message` ALWAYS answers `DeliveryFailed{"Message cannot be cancelled by this session"}`. **Observable**: the model cannot cancel or supersede an in-flight message, and a pi peer's cancel/supersede against a cyrup session is silently discarded.

**VL-I4 · Extension bus: frames validated then dropped** — *large*
`types.ts:1` (`EXTENSION_BUS_FEATURE`), `:80`/`:88`/`:96` (client frames), `:115`-`:131` (broker frames); `extension-api.ts` (44 lines); `broker/extension-state.ts` (186 lines — persisted, sha256-checksummed, 64 KiB-capped, optimistic revisions); `broker/broker.ts:505` (`features: [EXTENSION_BUS_FEATURE]`), `:509` (owner election); `broker/client.ts:216` (`supportsFeature`) with gates at `:648` and `:817` — vs `crates/cyrup-intercom/src/broker/mod.rs:419-425`, routing the three frames to validation-only handlers (rationale `:460-463`), and `transport/client.rs:575`, which discards the `features` field the protocol models at `protocol.rs:767`. **Observable**: a pi extension registering an intercom namespace gets owner election, cross-session publish and a durable revisioned state store; against a cyrup broker it is told no feature is supported and any forced frame is dropped with no reply.

**VL-I5 · No restart-stable intercom session id** — *small*
`config.ts:38-39` (`stableId`) with fail-closed validation `:141-150`; `index.ts:39` (`STABLE_INTERCOM_SESSION_ID_ENV`), `:409-411` (env → config → session id), consumed `:1264`; absent at v0.7.0 — vs `crates/cyrup-intercom/src/config.rs:33-48` (7 fields, no `stable_id`) and `connect.rs:377-382` (session id with a `last_session_id()` fallback, no override). **Observable**: a cyrup session's intercom address changes on every restart, so peers holding the old id can no longer reach it.

**VL-I6 · No `list-cwd` action** — *medium*
`cwd.ts:13-27` (`normalizeCwd`) and `:29-31` (`sameCwd`); `index.ts:25`, `:1783-1784` (usage), `:1795` (enum), `:1822-1824` (`cwd` param), `:1874-1925` (the case, with the "your session's cwd has N peers" fail-loud note at `:1901-1908`); `cwd.ts` does not exist at v0.7.0 — vs `crates/cyrup-intercom/src/tools/intercom.rs:388` and `:26-36`; no `cwd.rs` in the crate. **Observable**: a cyrup agent cannot ask for peers in a given working directory; it must call `list` and eyeball paths. *(Do NOT also claim symlink-normalization breakage: upstream's only `sameCwd` consumers are this action and the mailbox flush, and pi's own session list compares raw strings exactly as `crates/cyrup-intercom/src/ui/session_list.rs:153` does.)*

### 3d. `pi-permission-system` v0.7.1 → v0.8.0

**Zero open version-lag items.** Every v0.8.0 change is ported — see §4. The four permission-system
entries in this document (PB-15…PB-18) are all port bugs against v0.7.1.

---

## 4. What batches 8–10 closed

Listed so the next reader can see this document is current. Each was verified at 04c1ba2.

**Subsystems that were "absent" and are now ported and wired**
- `watchdog/` — 22 modules under `crates/cyrup-ext-subagents/src/watchdog/`, including a real stdio LSP client (`lsp_diagnostics.rs`). Wiring: `register_main_watchdog` at `extension.rs:9055`, nine subscriptions at `:9338-9352`, `/subagents-watchdog` at `:9326`, four `watchdog.*` verbs at `:7567-7570`. *(Two no-op agents remain — UW-4, UW-5.)*
- `missions/` — 7 modules; six `mission.*` verbs in the action enum at `extension.rs:6557`, params parsed `:5423-5432`, goal-continuation notices from the `agent_end` handler. *(Write half of workflow state remains — UW-8.)*
- The interactive fleet inspector — `tui/fleet_overlay.rs:177` implements `InteractiveOverlay`; `extension.rs:9898` constructs it and `:9908` calls `open_overlay`, driven by `crates/cyrup-tui/src/app.rs:6310`. This is the seam PB-9 and PB-18 should now use.
- The persistent fleet status widget — `set_widget` called at `extension.rs:9489`, `:9889`, `:9978`. *(Input half remains — UW-7.)*
- Subagent tool renderers — `render_call` (`extension.rs:9616`) and `render_result` (`:9646`), consumed at `crates/cyrup-tui/src/app.rs:5220-5221`.
- Terminal-run **revival** — `ResumeOutcome::RespawnFromTranscript` (`background/control.rs:1214`) → `revive_from_transcript` (`extension.rs:4232`), preferring the original run's cwd. *(Only the recovery descriptor remains — VL-S5.)*
- Agent **aliases** end to end — `KNOWN_FIELDS` at `discovery/frontmatter.rs:79-80`, `normalize_agent_aliases` at `:387`, the `aliases ?? alias` parse at `:786-793`, alias-aware resolution at `discovery/mod.rs:500` with the verbatim `Ambiguous agent alias '…': …` message at `:558`.
- `/prompt-workflow`, `/chain-prompts`, `/subagents-fleet`, `/intercom-id`, bundled prompt+skill file registration (`extension.rs:9548`, `:9553`).
- All 11 OAuth login flows (`crates/cyrup-provider/src/auth/oauth/`), driven from `/login`.

**`pi-permission-system` v0.8.0, fully absorbed**
`PermanentApprovalStore` removed (only doc mentions survive; regression test `tests/permanent_approvals_file_is_inert.rs`) · review stream un-gated from `debug` (`logging.rs:168-170`) · wildcard 500-char cap → never-match (`wildcard.rs:24`, `:81-83`, `:57`) · forwarded-request id path containment, the critical one (`forwarding.rs:592`, `:596-597`, before `resolve_forwarded_decision` at `:601`) · `enabled` master switch (`ext_config.rs:51`, `:324`; early return `extension.rs:2221-2223`) · config save preserving non-extension keys (`ext_config.rs:408`) · prototype-pollution key skip (`common.rs:173-175`).

**`pi-intercom` v0.9.2 interop**
The full 16-tag `BrokerMessage` union decodes instead of tearing down the connection (`transport/protocol.rs:857-875`, `client.rs:635-651`) · the v0.9.2 message envelope round-trips with a `#[serde(flatten)] extra` capture (`protocol.rs:301-347`) so a cyrup broker relaying between two pi sessions strips nothing · broker refuses to replace a live broker (`broker/runtime_claim.rs`, called at `broker/mod.rs:1238` before the stale-socket unlink) · live context-window usage in presence/session lists (`format_context.rs:70` → `tools/intercom.rs:377`).

**`pi` core**
Deferred request contract (`StopReason::Deferred`) · Gemini 3 tool-call ids (`api/google_generative_ai.rs:529-533`) · Responses terminal-error fallback and length-stop mapping (`api/openai_responses.rs:1014-1024`, `:1498-1523`) · `OAuthAuth::isSubscription` and the footer `(sub)` marker (`auth/mod.rs:101`; `app.rs:2506`) · `--model` cycling filtered to authenticated models · JSON/RPC `message_update` delta projection · LaTeX rendering (`markdown/latex.rs`, 2242 lines) · `ctrl+home`/`ctrl+end` and editor page actions · batched colour-scheme reports settling on the last frame · OSC 9;4 terminal progress · searchable settings list · `AGENTS.override.md` as first context-file candidate.

**Corrections to CLAUDE.md that fall out of this pass** — worth folding back:
- "first-run predicate is a compile-time constant `false`" is **wrong**; it is `true` for this build (UW-2).
- "cyrup registers the 38 built-ins pi shipped at v0.83.0" is **wrong**; it registers 35 of upstream's 40, and three of the five missing are port bugs (PB-1, PB-2).
- `packages/telemetry` is **inside** the port's dependency closure as of v0.84.1 (VL-P5); `packages/{server,storage,evals}` remain outside it; `packages/{protocol,client}` newly entered (VL-P23).
- The `pi-subagents` baseline is not v0.33.x–v0.34.0 (see the header note).

---

## 5. Deletion candidates — dead code, no behavioural difference

Not gaps. Recorded so nobody mistakes them for unimplemented features and "finishes" them.

- `crates/cyrup-ext/src/facade.rs:766` (`render_message_result`) and `:772` (`render_message_result_outcome`) — zero references outside their definitions and two doc-links (`native.rs:264`, `registry.rs:179`). pi has no message-RESULT renderer surface at all: `MessageRenderer` (`coding-agent/src/core/extensions/types.ts:1153`) is a single call-side function consumed only by `getMessageRenderer` (`runner.ts:579`) at `interactive-mode.ts:3471`. This is invented surface wider than pi's. Scope check before deleting: `RenderKind::Result` and the routing behind it also exist on the extension trait.
- `crates/cyrup-provider/src/session_resources.rs:48` / `:67` — a faithful port of `packages/ai/src/session-resources.ts` with no registrant and no dispose caller. pi's only registrant is the codex WebSocket cleanup (`api/openai-codex-responses.ts:927`), and cyrup has no WS transport by design-note (`api/openai_codex_responses.rs:39-46`). It is pre-wiring for a transport that does not exist; the real item is that transport.
- `crates/cyrup-permission-system/src/jsonc.rs:33` (`parse_ordered`) — zero references including tests; every caller goes through `parse_ordered_config`. Upstream has no counterpart behaviour to lose.

Also dead on **both** sides, so purely cosmetic: `crates/cyrup-provider/src/legacy_api_aliases.rs` (mirrors pi's own deprecated shim) and `nested_events.rs`'s `is_top_level_async_dir` / `nested_artifact_env` (upstream's `nested-events.ts` definitions have no call sites at v0.43.0 either).

---

## 6. Open questions — need a human decision, not more analysis

1. **The npm package channel (PB-7).** The extension half is genuinely blocked — WASM guests cannot load a TypeScript extension. The skills/prompts/themes half is not blocked. Do we (a) support `npm:` for resource-only packages, (b) support it fully by treating `extensions` entries as unsupported-but-skipped, or (c) delete `npmCommand` from settings so it stops advertising a capability? Today it is (d): the setting exists and does nothing. The code comment at `package/source.rs:81` cites a requirement id (`R-09-021`) that **cannot be checked from this workspace** — do not treat it as a decision of record.
2. **The compact-read `docs` arm (PB-4).** Upstream resolves against `dirname(getReadmePath())` — the shipped npm package tree. A Rust binary ships no such tree beside it. Porting the behaviour requires deciding what cyrup's "shipped docs root" is (embedded? an install-relative dir? nothing?). Until that is decided, the arm has nothing to classify against.
3. **Windows (PB-19).** The broker's Unix-only bind is unambiguous, but `crates/` carries 161 `cfg(unix)` sites against 6 `cfg(windows)`, so this may be a property of the whole port. If Windows is out of scope for the binary, PB-19 reduces to its second half (the client resolves TCP targets a cyrup broker never serves, and the ported listen resolver is dead) — still a real gap, but smaller.
4. **`VL-P5` (telemetryContext) has no user-visible symptom** — only an embedder-facing one, and pi's own `harness/telemetry.ts` has no internal consumer at v0.84.1. Is SDK-surface parity in scope, or do we track it separately from behavioural parity?
5. **Catalog accuracy (VL-P25) cannot be audited here.** pi generates `providers/data/*.json` from the network and does not commit it. Either accept structural parity only, or run `npm run generate-models` in `pi/` and diff.

---

## 7. Method, and what to trust

**How this was produced.** Four surface surveys (pi core; pi-subagents; permission-system + intercom;
an unwired-code sweep across all crates), each followed by an adversarial re-check that re-resolved
every citation by reading both sides. This document is the intersection, plus my own re-verification
of every point where the survey and its re-check disagreed.

**Entries dropped as false, with the evidence that killed them.** Both were "proved" by a grep that
does not reproduce — the failure mode to watch for.
- *"Async run revival unported"* — cyrup implements it (`background/control.rs:1214` → `extension.rs:4232`), with three production tests. The survey's `grep -ric 'revive\|revival' = 0` is wrong; there are ~10 hits. Replaced by the much narrower VL-S5.
- *"Agent aliases unported"* — the whole feature is ported including the verbatim ambiguity message. The survey's `grep -ric alias = 0` is wrong; there are ~114 hits.

**Claims I corrected rather than dropped.** Mission workflow state is read in production and only
written nowhere (UW-8). `enableInstallTelemetry` has two live consumers, so only `collapseChangelog`
is inert (PB-6). The permission-system modal and the permission-event bus are **not** blocked on
missing host seams — `open_overlay` and `SharedBus` both exist and are used (PB-16, PB-18); the
in-tree comments asserting otherwise are stale and should be fixed with the code. The `npmCommand`
entry's "cyrup always uses its built-in invocation" was false — cyrup makes no npm call at all
(PB-7). The claim that the run-side skills warning was already ported was false; it is unported too,
which is why PB-14 covers both surfaces.

**Reclassifications, because port bugs rank first.** The compact-read `docs` arm and
`PI_CODING_AGENT` were filed as version lag; both exist at v0.83.0 (`read.ts:98`, `cli.ts:13`) and
are port bugs. Changelog-on-upgrade likewise exists at v0.83.0 (`interactive-mode.ts:997`).

**Citations.** Every line number in §1 and §2 was resolved by reading the target file — by me for
roughly sixty of them, by the re-check pass for the rest, and the two disagreed on nothing that
survived. In §3 I re-read the load-bearing line of each entry and the corrections the re-checks
raised; the remaining supporting lines carry a single verified reading, not two. Do not "fix" a
citation by shifting it: a previous renumber-by-uniform-shift pass introduced errors at 15% while
looking verified. One example from this pass: the re-check "corrected" the intercom
`package.json:17` to `:14` — both are right, at different tags (`:17` at v0.9.2, `:14` at v0.7.0),
and I cite v0.7.0 because that is the ported baseline.

**Exhaustive vs sampled.**
- Exhaustive: the v0.7.1→v0.8.0 permission-system diff (28 files) and its v0.7.1 baseline; the `pi-intercom` v0.7.0→v0.9.2 surface; `pi-subagents`' pre-baseline residue (five files, all first-tag-verified).
- Sampled: `pi` v0.83.0→v0.84.1 is 627 files / +52k lines; the 25 entries in §3a are the ones with a demonstrable behavioural difference, not the whole diff. `packages/agent/src/harness/session/`'s v0.84.0 retree yielded exactly one evidenced consequence (VL-P22); the rest is an internal API reshape I did not decompose.
- The unwired sweep is **incomplete by construction**: it indexed bare identifiers, not resolved
  paths, so any method whose name collides with a live method elsewhere was silently excluded
  (`SubagentFleetStatus::handle_key` was found only by reading). Of ~7,087 pub items it flagged 385,
  of which ~120 remain untriaged and many are benign convenience wrappers. **The true unwired set is
  larger than §2.** Closing this properly needs a type-resolved pass
  (`cargo +nightly rustdoc --output-format json` or rust-analyzer), not grep.

**Could not confirm, and therefore not filed.** Per-model catalog accuracy (OQ-5). The delegation
slash surface (`src/slash/delegation-*.ts`, ~1,400 lines with zero cyrup filename citations, but 58
`delegation` hits in cyrup and a real `registration/prompt_workflows.rs`) — needs a side-by-side
read before anyone claims a contract difference. `src/api/`'s public surface for other extensions.
Eight `runs/background/` modules and seven small `shared/` utilities with no citations either way.
Whether cyrup's `resolve_manifest` error actually aborts the whole install (VL-P15's blast radius).
Whether cyrup's reqwest client bounds the OAuth refresh in practice (VL-P6's worst case may be
shorter than "indefinitely").

**Finally: `spec/` and `ADR-0001` do not exist in this workspace.** Doc comments across the codebase
cite `spec/architecture/arch-NN-*.md`, `R-NN-NNN` ids and `ADR-0001` thousands of times. Those are a
useful search index and nothing more. **No entry in this document rests on one, and none should.**
Where a code comment invokes one to justify a divergence — as `package/source.rs:81` does for the
npm channel — treat it as an unverifiable claim, not as a decision of record.
