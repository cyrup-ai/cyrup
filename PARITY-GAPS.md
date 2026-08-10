# Cyrup parity gap analysis — refreshed against current upstream tags

## 1. Headline

The port is behind on all four upstreams, with **147 surviving gaps** (2 critical, 49 high, 79 medium, 17 low) after a three-lens adversarial panel that refuted none of them: **73 against `pi` (v0.83.0 → v0.84.1)**, **55 against `pi-subagents` (~v0.34.0 → v0.43.0)**, **12 against `pi-intercom` (v0.7.0 → v0.9.2)** and **7 against `pi-permission-system` (v0.7.1 → v0.8.0)**. Roughly **20 of those are bugs in the existing port, not version lag** — features that were already in the tag cyrup targets and were never ported (subagent `steer`/`schedule`/budgets/`memory`, `--model` cycling without an auth filter, the Apple-Terminal Shift+Enter probe, OSC 9;4 progress, intercom's Windows/TCP broker transports) — and those rank ahead of everything post-baseline. `pi-subagents` is the widest single front: three entire upstream subtrees (`watchdog/`, `missions/`, `tui/fleet*`) totalling ~5,500 LOC have zero cyrup counterpart.

**Do G134 first.** A forwarded permission request's `id` is joined into the responses path with no containment check, and cyrup resolves the decision — including showing the human dialog — *before* it ever touches the path, so a child can make the parent prompt for, and then write a signed response to, a location outside `responsesDir` (`crates/cyrup-permission-system/src/forwarding.rs:556-587`). It is critical severity, small effort, on a live watcher reachable from `crates/cyrup/src/main.rs:483`.

---

## 2. Per-upstream backlog

Ordered severity, then effort. `[BUG]` = missing despite being in the ported baseline (see §3). `[UNWIRED]` = code exists, call site missing (see §4). Merged items name both source areas.

### 2.1 `pi` v0.83.0 → v0.84.1 — 73 items (17 high)

| item | status | sev | effort | cyrup location | upstream ref |
|---|---|---|---|---|---|
| G9 Gemini 3 must echo tool-call ids (`requiresToolCallId` third arm) | absent | high | small | `cyrup-provider/src/api/google_generative_ai.rs:522-526` | `ai/src/api/google-shared.ts:71-79` |
| G10 Copilot Individual: restore policy-state models (dual list + fallback) | absent | high | small | `cyrup-provider/src/providers/github_copilot.rs:331-350`; `auth/oauth/github_copilot.rs:613-635` | `ai/src/auth/oauth/github-copilot.ts:92-133` |
| G39 `--model` cycling walks the whole catalog, not authenticated models **[BUG]** | absent | high | small | `cyrup-session-svc/src/session.rs:3767-3787` | `coding-agent/.../agent-session.ts:1650`; v0.83.0:1644 |
| G17+G32 Blocked tool call cannot request batch early-termination *(agent + core)* | absent | high | small | `cyrup-agent/src/hooks.rs:48-52`; `agent.rs:1028` | `agent/src/agent-loop.ts:636-645`; `types.ts:61-68` |
| G43 JSON/RPC `message_update` not narrowed to delta-only (`toJsonEvent`) | absent | high | small | `cyrup-modes/src/json.rs:59-63`; `rpc.rs:1414` | `coding-agent/src/modes/json-event.ts` |
| G63 Native modifier probe for Shift+Enter (win32 new, Apple Terminal **[BUG]**) | absent | high | medium | `cyrup-tui/src/keyboard_protocol.rs`; `keymap.rs` | `tui/src/terminal.ts:14,44-56,318-325` |
| G8 Responses: only `max_output_tokens` is a length stop; others are errors | absent | high | medium | `cyrup-provider/src/api/openai_responses.rs:1429-1439` | `ai/.../openai-responses-shared.ts:565-573,755-762` |
| G13 Catalog corrections (GPT-5.6 pricing, Fireworks GLM-5.2/Kimi K3, Groq Qwen, Copilot Grok 4.5) | absent | high | medium | `cyrup-provider/src/providers/catalog/*.json` | `ai/scripts/generate-models.ts:389-393,2308-2313` |
| G27 Compact-and-retry after a recoverable `length` stop | absent | high | medium | `cyrup-session-svc/src/session.rs:4087,794,825` | `coding-agent/.../agent-session.ts:1993-1994,665,2195` |
| G29 Normalize/auto-resize images in every tool result, after the `tool_result` hook | absent | high | medium | `cyrup-session-svc/src/hooks.rs:141-147` | `coding-agent/src/utils/tool-result-images.ts:22-60` |
| G4+G19+G37 Arbitrary `samplingParams` (model, request, override, proxy) *(ai + agent + core)* | absent | high | medium | `cyrup-provider/src/utils/simple_options.rs:62-95`; `model.rs:54` | `ai/src/api/simple-options.ts:27-33`; `types.ts:186-189` |
| G1+G23 Deferred request contract + `StopReason::Deferred` *(ai + agent)* — strict-deserialize truncates pi sessions | absent | high | large | `cyrup-core/src/message.rs:160-169`; `cyrup-session/src/manager.rs:850-887` | `ai/src/types.ts:391,393-405`; `harness/session/context.ts:72` |
| G2 Catalog-refresh rework: generation-checked `publish()`, mandatory signal, providers subset | partial | high | large | `cyrup-provider/src/collection.rs:72,78,83,317` | `ai/src/models.ts:44-70,320-430` |
| G3 Cancellation through every auth op + 15 s OAuth-refresh bound (`CredentialStore::list` **[BUG]**) | absent | high | large | `cyrup-provider/src/auth/resolve.rs:180-201`; `auth/store.rs:24-38` | `ai/src/auth/resolve.ts:120,143-159`; `auth/types.ts:45-93` |
| G46+G54 Fullscreen (alternate-screen) TUI mode: `--tui-mode`, runtime switch, viewport tree *(modes + tui)* | absent | high | large | `cyrup-tui/src/app.rs` (single scrollback path) | `tui/src/tui-alt-screen.ts`; `interactive-mode.ts:788-833` |
| G69 `packages/protocol` — remote-session CBOR wire format now in closure | absent | high | large | none (no `cyrup-protocol`) | `pi/packages/protocol/`; `coding-agent/package.json:49` |
| G70 `packages/client` — `PiClient`, unix transport, connection state machine | absent | high | large | none | `pi/packages/client/`; `coding-agent/package.json:48` |
| G12 Responses terminal error: empty message never falls back **[BUG]** | partial | medium | small | `cyrup-provider/src/api/openai_responses.rs:996-1000` | `ai/src/api/openai-responses.ts:174` |
| G6 `compat.supportsFinishReason` — infer stop/toolUse when absent | absent | medium | small | `cyrup-provider/src/api/openai_completions.rs:1519-1537` | `ai/.../openai-completions.ts:578-584` |
| G14 `OAuthAuth.isSubscription` metadata | absent | medium | small | `cyrup-provider/src/auth/mod.rs:74-110` | `ai/src/auth/types.ts:210-211`; `helpers.ts:40-52` |
| G18 `Agent::reset()` does not reject mid-run | absent | medium | small | `cyrup-agent/src/agent.rs:1599-1613` | `agent/src/agent.ts:333-336` |
| G28 Reject prompts submitted during a manual compaction | partial | medium | small | `cyrup-session-svc/src/session.rs:623,849-897` | `coding-agent/.../agent-session.ts:1133-1138` |
| G31 Ambiguous bare `--model` must error, not first-wins | absent | medium | small | `cyrup-config/src/model.rs:1124-1135` | `coding-agent/.../model-resolver.ts:469-500` |
| G38 Retry transient management HTTP failures (remote catalog) | absent | medium | small | `cyrup-provider/src/remote_catalog.rs:544-547` | `coding-agent/src/utils/management-http.ts:3,25-67` |
| G40 Package manifest must degrade, not hard-fail, on a malformed `pi` block | absent | medium | small | `cyrup-resources/src/package/manifest.rs:84-92` | `coding-agent/src/core/pi-manifest.ts:11-33` |
| G53 Start colour-scheme and background probes concurrently | partial | medium | small | `cyrup-tui/src/theme.rs:936-946` | `coding-agent/.../theme/theme.ts:791-810` |
| G62 `ctrl+home`/`ctrl+end` aliases + editor page actions **[BUG on page actions]** | partial | medium | small | `cyrup-tui/src/keymap.rs:121-176,779-834` | `tui/src/keybindings.ts:92-97,108-109` |
| G48+G61 Rebindable `editor.historyPrevious`/`historyNext` *(modes + tui)* | partial | medium | small | `cyrup-tui/src/keymap.rs:147-176`; `editor.rs:1083,1097` | `tui/src/keybindings.ts:66-73`; `components/editor.ts:767-777` |
| G65 Batched colour-scheme reports: last wins, not first | partial | medium | small | `cyrup-tui/src/terminal_query.rs:265-270` | `tui/src/terminal-colors.ts:29` |
| G73 `AI_AGENT` identity env not stamped on CLI **or** RPC entry | absent | medium | small | `crates/cyrup/src/main.rs:53-57`; `cyrup-tools/src/tools/bash.rs:138-153` | `coding-agent/src/cli.ts:14`; `rpc-entry.ts:8` |
| G35+G49 Footer ` (sub)` marker never computed; upstream narrowed to real subscriptions **[UNWIRED]** *(core + modes)* | unwired | medium | small | `cyrup-tui/src/status.rs:100,185,277` | `coding-agent/.../model-runtime.ts:458-460`; `footer.ts:138-143` |
| G34+G47 `tuiMode`/`fullscreenScrollbar` settings + optional `scrollbarThumb` token *(core + modes)* | absent | medium | small | `cyrup-config/src/settings.rs`; `cyrup-tui/src/theme.rs:390-400` | `settings-manager.ts:135-147`; `theme/theme.ts:48,321-331` |
| G5 vLLM `thinking_token_budget` via `compat.supportsThinkingTokenBudget` | absent | medium | medium | `cyrup-provider/src/api/compat.rs:71-120`; `utils/simple_options.rs:22` | `ai/.../openai-completions.ts:851-866`; `simple-options.ts:54` |
| G26 Atomic JSONL publication for fork/first-flush + torn-tail repair | partial | medium | medium | `cyrup-session/src/store.rs:85-113`; `manager.rs:114-116` | `agent/.../session/jsonl/storage.ts:33-46` |
| G36 `auth check` preflight + `ReadOnlyAuthStorage` for `--no-refresh` | absent | medium | medium | `crates/cyrup/src/credential_print.rs:147-157` | `coding-agent/src/cli/auth-check.ts:22-71`; `auth-storage.ts:204-268` |
| G52 Non-blocking `/login` `/logout` + `CredentialSynchronizationError` | partial | medium | medium | `cyrup-tui/src/app.rs:2001,3323,2269-2303` | `interactive-mode.ts:5197,5397-5402,5463-5483` |
| G56 `tui.altScreen.*` keybindings + OSC 133 prompt jumping | absent | medium | medium | `cyrup-tui/src/keymap.rs:102-103`; `transcript.rs:419` | `tui/src/keybindings.ts:45-52`; `tui-alt-screen.ts:56-57,366-379` |
| G58 Proportional scrollbars with drag + transient `auto` mode | absent | medium | medium | none in `cyrup-tui/` | `tui/src/components/scroll-view.ts:4-110` |
| G66 OSC 9;4 terminal progress — setting wired, emitter absent **[BUG on parent]** | partial | medium | medium | `cyrup-config/src/settings.rs:779-783`; `cyrup-tui/src/app.rs:4668-4670` | `tui/src/terminal.ts:11-13,417,530` |
| G68 Searchable settings list + space-filters-not-activates | absent | medium | medium | `cyrup-tui/src/settings_selector.rs` (367 lines, no search) | `tui/src/components/settings-list.ts:185-194` |
| G7+G42 Baseten provider, `thinkingFormat "baseten"`, `chatTemplateArgs`, default-model entry *(ai + core)* | absent | medium | medium | `cyrup-provider/src/api/compat.rs:27-39,95`; `cyrup-config/src/model.rs:936-974` | `ai/src/providers/baseten.ts`; `openai-completions.ts:779-795` |
| G16+G42 Qwen Token Plan family (`-individual` new; parents **[BUG]**) *(ai + core)* | absent | medium | medium | `cyrup-provider/src/providers/all.rs`; `catalog/` (35 files) | `ai/src/providers/qwen-token-plan*.ts`; `model-resolver.ts:54-56` |
| G71 `coding-agent/src/client/` + `./client` export — `RemoteSession` + transcript reducers | absent | medium | medium | none | `coding-agent/src/client/remote-session.ts:25-60` |
| G72 `packages/telemetry` threading (`telemetryContext` on request options) | absent | medium | medium | `cyrup-provider/src/stream.rs:145` | `ai/src/types.ts:122-123`; `api/simple-options.ts:36` |
| G20 Compaction re-seeds from previous `retainedTail` | absent | medium | large | `cyrup-session/src/compaction/prepare.rs:87-90` | `agent/.../compaction/compaction.ts:634-651,674-680` |
| G33+G45 `registerMarkdownTransformer` API + transform pipeline *(core + modes)* | absent | medium | large | `cyrup-ext/wit/world.wit:272-279`; `cyrup-tui/src/transcript.rs:1842` | `extensions/types.ts:1147-1153,1291-1292` |
| G44 Mermaid diagrams as Unicode + `markdown.mermaid` setting | absent | medium | large | `cyrup-tui/src/markdown.rs` (0 mermaid hits) | `modes/interactive/components/mermaid.ts:1-89` |
| G51 Caller-owned, 15 s-bounded, non-blocking catalog refresh for model selectors | partial | medium | large | `cyrup-provider/src/remote_catalog.rs:314-320,457` | `interactive-mode.ts:4340-4375,4762-4857` |
| G55 LaTeX math rendering in Markdown | absent | medium | large | `cyrup-tui/src/markdown.rs:34` | `tui/src/latex.ts` (1373 lines) |
| G57 Mouse viewport interaction (wheel, drag selection, OSC 8 click) | absent | medium | large | `cyrup-tui/src/app.rs:5901` (`Event::Mouse(_) => None`) | `tui/src/tui-alt-screen.ts:48-52,385-501,605-914` |
| G11 Structured Bedrock failure diagnostics | absent | low | small | `cyrup-provider/src/api/bedrock_converse_stream.rs:454-462` | `ai/.../bedrock-converse-stream.ts:225,318-320` |
| G15 OpenCode Go display name | absent | low | small | `cyrup-provider/src/providers/opencode_go.rs:44` | `ai/src/providers/opencode-go.ts:11` |
| G21 `fromHook` still suppresses file-list inheritance | absent | low | small | `cyrup-session/src/compaction/prepare.rs:92`; `branch.rs:176-179` | `agent/.../compaction/compaction.ts:52` |
| G30 `AGENTS.override.md` as first context-file candidate | absent | low | small | `cyrup-session/src/prompt/context_files.rs:63` | `coding-agent/.../resource-loader.ts:71` |
| G41 Softened `PI_*` env guideline in the bash prompt contribution | absent | low | small | `cyrup-tools/src/tools/bash.rs:96-103` | `coding-agent/.../tools/bash.ts:45-48` |
| G50 Shorter length-stop notice | absent | low | small | `cyrup-tui/src/app.rs:4097-4106` | `components/assistant-message.ts:177-181` |
| G59 Stacked transient flash notifications | absent | low | small | none (`cyrup-tui/` has `overlay.rs`, no `flash.rs`) | `tui/src/components/alt-screen-flash.ts` |
| G60 Reduced (button-motion) mouse tracking under tmux/Zellij/Screen | absent | low | small | `cyrup-tui/src/image.rs:449-457` (no `ZELLIJ`/`STY`) | `tui/src/tui-alt-screen.ts:237-249` |
| G64 Assume truecolor on Windows consoles without `WT_SESSION` | partial | low | small | `cyrup-tui/src/image.rs:478-485` | `tui/src/terminal-image.ts:74,122-129` |
| G67 Width-aware Markdown `transform` hook + exported parser types | absent | low | small | `cyrup-tui/src/markdown.rs:34` | `tui/src/components/markdown.ts:225-226,285` |
| G22 Harness entry vocabulary shrink (`custom_message`/`label`/`session_info`) | absent | low | large | `cyrup-session/src/entry.rs:114-150`; `context.rs:61-72` | `agent/.../compaction/compaction.ts:335-342` |
| G24 Harness v2 (lane Session/SessionRepo, durable ops, swappable index) | absent | low | large | `cyrup-session/src/{store,manager,layout,listing}.rs` | `agent/src/harness/session/` (retree at v0.84.0) |
| G25 Typed telemetry schemas + span emission | absent | low | large | none (0 `TelemetryContext` hits) | `agent/src/harness/telemetry.ts` (615 lines) |

### 2.2 `pi-subagents` ~v0.34.0 → v0.43.0 — 55 items (25 high)

| item | status | sev | effort | cyrup location | upstream ref |
|---|---|---|---|---|---|
| G81 `structured_output` params must rewrite local JSON-Pointer `$ref`s | absent | high | small | `src/prompt_runtime.rs:403-410` | `runs/shared/structured-output.ts:23-69` |
| G88 Two-stage explicit→inherited model resolution; tool failures not retryable | absent | high | small | `src/extension.rs:8524-8535`; `exec/fallback.rs:350-518` | `runs/shared/model-fallback.ts:222-243,303,316-323` |
| G96 YAML folded scalars `>`/`>-` + block lists (indent anchor **[BUG]**) | absent | high | small | `src/discovery/frontmatter.rs:349-355,210-230` | `agents/frontmatter.ts:12-59,102-105` |
| G100 Profile apply replaces the whole `subagents` block instead of merging | partial | high | small | `src/registration/profiles.rs:247-261,577-616` | `profiles/profiles.ts:483-495` |
| G102 Prune the discovery walk (`.git`, `node_modules`, nested roots) | absent | high | small | `src/discovery/mod.rs:556-593` | `agents/agents.ts:1373-1405` |
| G103 Explicitly-empty `tools:` must mean "no tools" (incl. bridge injection) | absent | high | small | `src/discovery/frontmatter.rs:588-589` | `agents/agents.ts:541`; `intercom/intercom-bridge.ts:170` |
| G90 `steer` management action **[BUG]** | absent | high | medium | `src/extension.rs:5259` (15-verb enum) | `runs/foreground/subagent-executor.ts:3194` (present at v0.34.0) |
| G75 Bounded child NDJSON reader + `protocol_output_limit` + aggregate projection | absent | high | medium | `src/spawn/mod.rs:331-337,418-419,531`; `exec/ndjson.rs:367` | `runs/shared/child-protocol.ts:6-9,244-346` |
| G76 `agent_settled` starts the drain; `agent_end{willRetry}` cancels it | absent | high | medium | `src/exec/mod.rs:2156-2165`; `exec/ndjson.rs:155-157` | `runs/shared/child-protocol.ts:394-400` |
| G80 Verify-command workspace memoization + secret redaction | absent | high | medium | `src/exec/acceptance.rs:3299-3312` | `runs/shared/acceptance.ts:974-1130` |
| G106 Native supervisor channel + intercom-bridge config/env **[BUG on channel]** | partial | high | large | `src/spawn/intercom_target.rs:1-2,118-128` | `intercom/native-supervisor-channel.ts` (at v0.34.0) |
| G89 Child tool budgets and turn budgets **[BUG]** | absent | high | large | `src/extension.rs:5250-5348`; `discovery/frontmatter.rs:68-90` | `extension/schemas.ts:77-92,106,267-268` (at v0.34.0) |
| G87 Session-scoped capability ceiling on child tools/agents/extensions | absent | high | large | `src/exec/mod.rs:1255` ("no capability ceiling in this port") | `runs/shared/capability-ceiling.ts:3-105` |
| G78 Acceptance: `reviewed` no longer requestable; status vs `evidenceStatus` | partial | high | large | `src/exec/acceptance.rs:82-123,5318-5350` | `runs/shared/acceptance.ts:28-36,181-196,1302-1352` |
| G98 Agent-level launch defaults (async/timeoutMs/turnBudget/acceptance/skillPath/permissions/runner) | absent | high | large | `src/discovery/frontmatter.rs:68-90`; `discovery/types.rs:609-687` | `agents/agents.ts:1509-1584` |
| G105 v0.43 tool-schema properties + narrowed acceptance enum | partial | high | large | `src/extension.rs:5250-5348,5337` | `extension/schemas.ts:68-101,122-131,189-191,346-350` |
| G77+G104 `stopped` as a first-class terminal state + result framing *(runs + registration)* | absent | high | large | `src/background/run_status.rs:46-53`; `tui/intercom.rs:117-125` | `runs/foreground/async-stop-action.ts`; `intercom/result-intercom.ts:20-52` |
| G107 Watchdog settings schema, layered resolution, writers | absent | high | medium | none (no `src/watchdog/`) | `watchdog/settings.ts` (568 lines) |
| G111 Turn-delta formatter with edit/write payload redaction | absent | high | medium | none | `watchdog/turn-delta.ts` (161 lines) |
| G114 Scope artifact + blocker auto-follow (and the stalemate stop) | absent | high | medium | none; `src/extension.rs:7176-7178` subscribes only to session start/shutdown | `watchdog/scope.ts`; `runtime.ts:304-322,643-683` |
| G118 Child watchdog: env handoff, NDJSON status events, parent tail-wait | absent | high | medium | `src/exec/ndjson.rs:90-205` (15 variants, none watchdog) | `watchdog/child-status.ts:3-4,47-73,167-205` |
| G119 Watchdog permission arbiter (model-decided approve/deny, fail-closed) | absent | high | medium | none; `src/prompt_runtime.rs` has no permission hook | `watchdog/permission-arbiter.ts` (145 lines) |
| G120 Mission store: records, global pointer index, terminal retention | absent | high | medium | none (0 mission code) | `missions/store.ts` (507 lines); `types.ts` (157) |
| G108 `MainWatchdogRuntime` review state machine | absent | high | large | none | `watchdog/runtime.ts` (868 lines) |
| G112 Watchdog review agent (read-only nested Agent, `watchdog_warn`) | absent | high | large | none; `cyrup-agent`/`cyrup-provider` already direct deps | `watchdog/review.ts` (302 lines) |
| G121 Automatic mission binding at launch + result attachment | absent | high | large | none | `missions/lifecycle.ts` (346 lines) |
| G84 Mutation detection: `git add/commit/push`, cursor tool, checkpoint evidence | partial | medium | small | `src/exec/completion_guard.rs:170-172,656-700` | `runs/shared/long-running-guard.ts:128-152` |
| G97 Agent aliases + alias-aware resolution and ambiguity error | absent | medium | small | `src/discovery/types.rs:609-687`; `frontmatter.rs:68-90` | `agents/agents.ts:495,511-521` |
| G99 Builtin roster: drop planner/context-builder, add advisor, re-tier profiles | absent | medium | small | `src/discovery/management.rs:1210`; `registration/profiles.rs:354` | `agents/agents.ts:38-45`; `profiles.ts:263,405-409` |
| G110 Warning emission guard: content-free rejection, dedupe, per-update budget | absent | medium | small | none | `watchdog/emission-guard.ts` (123 lines) |
| G74 Retry zero-activity child startup exits on the same model | absent | medium | medium | `src/exec/fallback.rs:109,504`; `exec/mod.rs:2615` | `runs/shared/subagent-startup-retry.ts:9-101` |
| G82 Authorship from the child's own successful write + read-only instruction | absent | medium | medium | `src/exec/output.rs:781-810,964-989` | `runs/shared/single-output.ts:14-108` |
| G83 Task mutation-intent classifier (scoped vs blanket, `taskMayMutate`) | partial | medium | medium | `src/exec/completion_guard.rs:530-568` | `runs/shared/task-intent.ts:39-178` |
| G91 Scheduled subagent runs (4 verbs) **[BUG]** | absent | medium | medium | `src/extension.rs:5259` | `subagent-executor.ts:3224`; `runs/background/scheduled-runs.ts` (at v0.34.0) |
| G92 Fleet/transcript status views (`view`+`lines`) and `/subagents-fleet` **[BUG]** | absent | medium | medium | `src/extension.rs:5250-5348`; `registration/slash_commands.rs:150-215` | `extension/schemas.ts:233-237` (at v0.34.0) |
| G93 `/prompt-workflow` + `/chain-prompts`; 7 bundled prompts inert **[BUG, UNWIRED]** | absent | medium | medium | `src/registration/resources.rs:61` (test-only caller) | `slash/prompt-workflows.ts:269,303` (at v0.34.0) |
| G94 Remove `/subagents-companions` + `companionSuggestions` **[BUG — removal]** | absent | medium | medium | `src/registration/slash_commands.rs:92,212`; `extension.rs:7603,8018-8074` | deleted upstream 2026-07-03, before v0.34.0 |
| G95 Per-agent persistent memory scopes (`memory:`) **[BUG]** | absent | medium | medium | `src/discovery/frontmatter.rs:68-90` | `agents/agent-memory.ts` (at v0.34.0) |
| G101 `defaultThinking` / `defaultExtensions` / `projectRootResolution` | absent | medium | medium | `src/discovery/types.rs:505-528`; `discovery/mod.rs:181` | `agents/agents.ts:161-169,640-673,945-1000` |
| G109 Repo change signature: gate reviews on real edits to tracked files | absent | medium | medium | none (no `rev-parse` outside `spawn/worktree.rs`) | `watchdog/change-signature.ts` (220 lines) |
| G116 Watchdog warning message type + TUI renderer | absent | medium | medium | `src/tui/render.rs`; `extension.rs:7078` (no `render_result`) | `watchdog/warning-format.ts` (73); `render.ts` (54) |
| G117 `/subagents-watchdog` + four watchdog tool actions | absent | medium | medium | `src/registration/slash_commands.rs:100-118` (13 cmds) | `watchdog/register-main.ts` (440); `tool-actions.ts` (155) |
| G122 Six mission tool actions | absent | medium | medium | `src/extension.rs:5259` | `missions/actions.ts` (410 lines) |
| G123 Goal missions: turn-end continuation notices | absent | medium | medium | none; no `AgentEnd` subscription | `missions/goal-driver.ts` (162 lines) |
| G124 Mission workflow state: locked, bounded KV store | absent | medium | medium | none; no scripted-workflow runtime (`RunMode` = Single/Parallel/Chain) | `missions/workflow-state.ts` (250 lines) |
| G126 Persistent fleet status widget | absent | medium | medium | `set_widget` live at `cyrup-session-svc/src/host_services.rs:651`, never called | `tui/fleet-status.ts` (564 lines) |
| G127 Fleet transcript reader: bounded, sanitized JSONL tailing | absent | medium | medium | none (no terminal-escape sanitizer anywhere) | `tui/fleet-transcript.ts` (577 lines) |
| G128 `inlineToolDisplay:"summary"` + renderer wiring **[UNWIRED]** | partial | medium | medium | `src/tui/events.rs:612,738` (test-only callers) | `tui/render.ts:55,1636-1684` |
| G79 Acceptance report parsing: aliases, file-output source, fence recovery | partial | medium | large | `src/exec/acceptance.rs:4018-4043,4390-4470` | `runs/shared/acceptance.ts:484-770,972-978` |
| G85 Chain approval checkpoints + parallel worktree handoff manifests | absent | medium | large | `src/spawn/chain_graph.rs:377-394`; `background/mod.rs:1310-1337` | `chain-execution.ts:760-769`; `parallel-handoff.ts` |
| G86 Wait rework: `subagent_wait` rename, non-blocking subscriptions, auto-drain | partial | medium | large | `src/extension.rs:5358` (`WAIT_TOOL_NAME = "wait"`) | `runs/background/{subagent-wait,wait-tool,wait-config,wait-subscriptions,auto-drain}.ts` |
| G115 LSP diagnostics probe with a fresh-only ledger | absent | medium | large | none (cyrup has no LSP client) | `watchdog/lsp-diagnostics.ts` (537 lines) |
| G125 FleetView interactive inspector overlay | absent | medium | large | none; no native-extension `register_shortcut` (`cyrup-ext/src/native.rs:244-278`) | `tui/fleet.ts` (879 lines) |
| G113 Strong-watchdog model recommendation | absent | low | small | none | `watchdog/model-selection.ts` (167 lines) |

### 2.3 `pi-permission-system` v0.7.1 → v0.8.0 — 7 items

| item | status | sev | effort | cyrup location | upstream ref |
|---|---|---|---|---|---|
| G134 Forwarded request `id` can escape the responses directory (and prompts first) | absent | **critical** | small | `src/forwarding.rs:556-587` (decide at :572, join at :583) | `index.ts:673-679,1158-1163` |
| G131 Un-gate the security review audit stream from `debug` | absent | high | small | `src/logging.rs:147-164` | `logging.ts:98-100` |
| G129 Delete `PermanentApprovalStore` — approvals must not cross sessions | absent | high | medium | `src/stores.rs:85-180`; `gate.rs:161-179`; `extension.rs:645,813,1216` | `permanent-approval-store.ts` deleted; `index.ts:557-578` |
| G130 `enabled` master switch (early return before every registration) | absent | medium | small | `src/ext_config.rs:29-45,178-187` | `extension-config.ts:11-12,88`; `index.ts:1473-1477` |
| G132 Cap wildcard patterns at 500 chars → never-match regex | absent | medium | small | `src/wildcard.rs:32-66` | `wildcard-matcher.ts:15-27` |
| G133 Config save: preserve non-extension keys, refuse corrupt, follow symlinks | absent | medium | medium | `src/ext_config.rs` (no save fn at all) | `extension-config.ts:140-268` |
| G135 Drop `__proto__`/`constructor`/`prototype` in the frontmatter YAML parser | absent | low | small | `src/common.rs:149-187` | `common.ts:111-113,132-135` |

### 2.4 `pi-intercom` v0.7.0 → v0.9.2 — 12 items

| item | status | sev | effort | cyrup location | upstream ref |
|---|---|---|---|---|---|
| G136 Unknown protocol frames tear down the connection | absent | **critical** | medium | `src/transport/client.rs:505-517`; `broker/mod.rs:288-295,695` | `types.ts:78-131` (11 client / 15 broker tags) |
| G144 Broker does not refuse to replace a live broker (unlinks its socket) | absent | high | small | `src/broker/mod.rs:835-838` | `broker/runtime-claim.ts:3-21`; `broker.ts:231` |
| G138 Broker mailbox: queue + redeliver for recently disconnected named sessions | absent | high | medium | none; `tests/reconnect.rs:137` pins the opposite | `broker/broker.ts:39-41,709-786,975-1150` |
| G140 Explicit message cancellation and supersede controls | absent | high | medium | `src/tools/intercom.rs:377` (6 actions) | `types.ts:44-52,84,114`; `broker.ts:640-687,822-869` |
| G139 Delivery metadata, lifecycle receipts, receiver-side inbound dedupe | absent | high | large | `src/transport/protocol.rs:51-64` (5-field `Message`) | `types.ts:24-58`; `index.ts:446-461,508-544` |
| G137 Extension bus: namespaced non-conversational channel | absent | high | large | none (0 hits) | `types.ts:1,54-102`; `broker/extension-state.ts` |
| G141 Restart-stable intercom session id (`PI_INTERCOM_STABLE_ID`/`stableId`) | absent | medium | small | `src/config.rs:33-48`; `identity.rs:11-20` | `index.ts:39,409-411`; `config.ts:141-149` |
| G142 `list-cwd` action + symlink-aware cwd normalization | absent | medium | medium | `src/tools/intercom.rs:355,377`; `ui/session_list.rs:149` | `cwd.ts:13,29`; `index.ts:1874-1925` |
| G143 Live context-window usage in presence and session lists | absent | medium | medium | `src/transport/protocol.rs:21-46,167-177` | `types.ts:14-22,86`; `format-context.ts` |
| G146 Broker server transports: Windows named pipe + opt-in TCP loopback **[BUG]** | partial | medium | large | `src/broker/mod.rs:836,846`; client half at `transport/target.rs:5-15` | `broker/paths.ts:44,107`; `broker.ts:263-266` (at v0.7.0) |
| G147 Ship the bundled `pi-intercom` skill **[BUG]** | absent | medium | medium | no `crates/cyrup-intercom/resources/` | `skills/pi-intercom/SKILL.md` (513 lines at v0.7.0) |
| G145 `/intercom-id` slash command inserting a handoff snippet | absent | low | small | `src/extension.rs:266` (only one `register_command`) | `index.ts:412-414,2270-2289,2365-2368` |

---

## 3. Port bugs vs version lag

### 3.1 Port bugs — missing despite being in the ported baseline (do these first)

Each was verified present at the tag cyrup targets, by `git show <baseline-tag>:<file>`.

| item | baseline evidence | why it matters now |
|---|---|---|
| **G39** `--model` cycling | v0.83.0 `agent-session.ts:1644` already `await this._modelRuntime.getAvailable()` (auth-filtered) | cyrup cycles the whole composed catalog (`session.rs:3772`), landing on providers the user cannot call |
| **G12** Responses empty error | v0.83.0 `openai-responses.ts:174` already threw `"An unknown error occurred"` unconditionally | cyrup can emit an error terminal with `error_message: None` |
| **G3** (part) `CredentialStore::list()` | v0.83.0 `auth/types.ts:71` already declares `list()` | cyrup's trait (`auth/store.rs:24-38`) has no `list` at all; the signal threading + 15 s bound *are* new |
| **G62** (part) editor page actions | v0.83.0 `keybindings.ts:89` already has `tui.editor.pageUp`/`pageDown` | `EditorAction` has no page variant; `PageUp` always scrolls the transcript |
| **G63** (part) Apple-Terminal Shift+Enter | v0.83.0 `terminal.ts:44` exports `normalizeAppleTerminalInput`; darwin probe ships | Shift+Enter submits instead of inserting a newline; the win32 half is the *new* part |
| **G66** OSC 9;4 progress | v0.83.0 `terminal.ts:11-13` already defines the sequences and writes them | the whole emitter is unported; v0.84.1 only drops a stray `;` |
| **G16/G42** (part) qwen-token-plan, -cn | v0.83.0 `types.ts:67-68` lists both as `KnownProvider` | only `-individual` is new work; the parents are port debt |
| **G89** tool/turn budgets | v0.34.0 `extension/schemas.ts:77-92`; `agent-serializer.ts` KNOWN_FIELDS has `toolBudget` | `toolBudget:` in an agent file is silently demoted to `extra_fields` |
| **G90** `steer` | v0.34.0 `subagent-executor.ts:3194` is literally `if (action === "steer") {` | no way to inject non-terminal guidance into a live child |
| **G91** `schedule*` (4 verbs) | v0.34.0 ships `runs/background/scheduled-runs.ts`; dispatch at `:3224` | — |
| **G92** `view`/`lines` + `/subagents-fleet` | v0.34.0 `schemas.ts:233-237` | — |
| **G93** `/prompt-workflow`, `/chain-prompts` | v0.34.0 `slash/prompt-workflows.ts:269,303` | 7 bundled prompt templates ship on disk and are unreachable |
| **G94** remove `/subagents-companions` | deleted upstream 2026-07-03, *before* v0.34.0 | cyrup ships a 14th slash command and a config key upstream has not had for a year |
| **G95** `memory:` frontmatter | v0.34.0 `agents/agent-memory.ts`; KNOWN_FIELDS has `memory` | — |
| **G96** (part) dedent indent anchor | v0.34.0 `frontmatter.ts` already used `/^([ \t]+)/m` | cyrup takes the prefix from the block's first characters (`frontmatter.rs:210-230`) |
| **G106** (part) native supervisor channel | v0.34.0 `intercom/native-supervisor-channel.ts` exists; `intercom-bridge.ts:8` exports `NATIVE_INTERCOM_EXTENSION_DIR` | with no intercom extension a child has *no* supervisor channel; upstream falls back to a file channel |
| **G146** (part) broker server transports | v0.7.0 `broker/paths.ts:44,107` | cyrup's client emits a `state_id` its own broker cannot validate |
| **G147** intercom skill | `skills/pi-intercom/SKILL.md` is 513 lines at v0.7.0, 514 at v0.9.2 | essentially all of it is baseline debt |

### 3.2 Version lag — postdates the ported baseline

Everything else (≈127 items) landed upstream after the tag cyrup targets and is next-version-bump work: the whole `pi` v0.84.0 provider/auth/telemetry/harness-v2 rework, the fullscreen TUI program, `packages/{protocol,client,telemetry}` entering the closure, the `pi-subagents` v0.35–v0.43 watchdog/missions/fleet subtrees and acceptance rework, all seven `pi-permission-system` v0.8.0 items, and the `pi-intercom` v0.8/v0.9 extension bus, mailbox and receipts. Two version-lag items nonetheless carry outsized live risk and are pulled forward in §6: **G129** (a hand-written approvals file is still an active last-match-wins policy-override channel) and **G136** (a pi ≥0.9.0 peer disconnects a cyrup broker on its first inbound message).

---

## 4. Present-but-unwired — the cheapest wins

Code exists and compiles; only a call site is missing.

1. **G35+G49 — footer ` (sub)`.** `crates/cyrup-tui/src/status.rs:100/185/186/277/278` define the field, setter and render; the *only* caller of `set_using_subscription` workspace-wide is `crates/cyrup-tui/tests/render.rs:387`, a test. The marker can never render in a real session. Wiring it also needs the new narrowed predicate (`is_subscription`, 0 hits), not the old OAuth-wide one.
2. **G93 — bundled prompt templates.** `bundled_prompt_files()` at `crates/cyrup-ext-subagents/src/registration/resources.rs:61`; only caller is `resources.rs:204`, inside its own `#[cfg(test)]`. Seven `.md` recipes ship at `crates/cyrup-ext-subagents/resources/prompts/` and nothing registers the directory.
3. **G147 — bundled skill.** `bundled_skill_files()` at `registration/resources.rs:77`; only caller `:226`, a test. The 58 KB `resources/skills/pi-subagents/SKILL.md` ships and is never registered. Note `src/prompt_runtime.rs:69-70` claims "this crate has no skills/ directory" — that comment is now factually stale.
4. **G128 — subagent renderers.** `tui/events.rs:612` (`render_inline_result`) and `:738` (`render_async_jobs_widget`) are called only from `events.rs:854,867` in their own test module. `SubagentsExtension` (`extension.rs:7078`) implements neither `render_call` nor `render_result` and never calls `register_tool_renderer`. The host substrate is proven live by `crates/cyrup-tui/tests/extension_renderers.rs:46`. The already-written spinner animation is dead code today.
5. **G126 — widget host.** `HostServices::set_widget` is implemented for real at `crates/cyrup-session-svc/src/host_services.rs:651` and consumed at `crates/cyrup-tui/src/app.rs:2690`; `cyrup-ext-subagents` never calls it.
6. **G145 — editor-text seam.** `HostServices::editor_text`/`set_editor_text` (`cyrup-ext/src/host/services.rs:209,250`) are implemented at `host_services.rs:667-669` and consumed at `cyrup-tui/src/app.rs:2441,2643`. `cyrup-intercom` registers exactly one command (`extension.rs:266`). `/intercom-id` is a second `register_command` plus an arm — no new seam required.
7. **G143 — context usage.** `HostServices::context_usage` is live at `host_services.rs:690-702` and `cyrup-intercom` already holds the `Arc<dyn HostServices>` (used at `inbound.rs:229`); nothing in the crate reads it.
8. **G9 — Gemini 3.** `gemini_major_version` already exists at `google_generative_ai.rs:536-540` with the same `>= 3` test used by its neighbour; `requires_tool_call_id` at `:524` simply does not consult it. One `||` clause; the three consumption sites already route through it.
9. **G13 (Fireworks half) — compat keys.** `supportsLongCacheRetention` / `sendSessionAffinityHeaders` are already supported at `cyrup-provider/src/api/compat.rs:107-110` and already set on the `glm-5p1` catalog entries; the `glm-5p2` entries just lack them. JSON edit only.
10. **G12 — unknown-error fallback.** The exact pattern exists at `bedrock_converse_stream.rs:454-457`; it is absent from `openai_responses.rs` (Azure inherits via the shared import at `azure_openai_responses.rs:25`).
11. **Inverse shape — G66.** The `/settings` row and the setting are fully wired (`cyrup-config/src/settings.rs:779-783` ← `cyrup-tui/src/app.rs:4668-4670`); the mechanism behind them emits nothing. A user can turn on "Terminal progress" and get no indicator.
12. **Inverse shape — G146.** The intercom *client* transport half is complete and reachable in production (`transport/target.rs:5-15`, `stream.rs:34-44`, `protocol.rs:138-145`, called at `spawn.rs:226,299`); the *server* half binds `UnixListener` unconditionally. Those paths only ever function against a pi broker.

---

## 5. Wire/protocol compatibility risks

Cases where a cyrup process and a pi process would disagree on a format.

| risk | mechanism | items |
|---|---|---|
| **Intercom frames are fatal, both directions** | `StopReason`-style strict tag matching: an unknown `ClientMessage`/`BrokerMessage` tag sets `close_reason` and `break 'outer` (`transport/client.rs:505-517`) or returns `FrameOutcome::ProtocolError`, which ends the connection loop (`broker/mod.rs:288-295,695`). A pi ≥0.9.0 client sends `message_receipt` on its **first** inbound message and a pi ≥0.9.0 broker sends `message_control` on any peer cancel — so the very first interop message disconnects. Unknown *fields* are fine (serde default). | G136, G139, G140 |
| **Intercom metadata is silently stripped** | `broker/mod.rs:427` re-parses into the typed 5-field `Message` and forwards that, dropping `senderSequence`/`supersedes`/`retryOf`/`broker*At` on re-serialize. | G136, G139 |
| **Intercom credential asymmetry** | cyrup sends `state_id` on register/health (`protocol.rs:138-145,246-250`) and can resolve a TCP target, but its broker never publishes `broker.port.json` and never validates the secret. | G146 |
| **Session JSONL: pi-written `"stopReason":"deferred"` silently truncates a cyrup session** | `StopReason` has no `#[serde(other)]` (`cyrup-core/src/message.rs:159-169`, doc at :153-157 states the failure mode); `cyrup_session::manager::load` (`manager.rs:850-887`) keeps only the valid prefix, raises `recovered`, and `manager.rs:114` then declines to rewrite. This is an R-00-013 interop break independent of whether cyrup can *produce* a deferred turn. | G1+G23 |
| **JSON/RPC stdout shape** | both modes serialize the raw `AgentSessionEvent` (`cyrup-modes/src/json.rs:59`, `rpc.rs:1414`), so `message_update` ships the full `message` snapshot and `assistantMessageEvent.partial` on every delta — quadratic output where pi is linear, and a different record shape for any consumer. | G43 |
| **Subagent NDJSON child stream** | cyrup's parent reader enumerates 15 event types (`exec/ndjson.rs:90-205`) with no `subagent.watchdog.status`; a pi child emitting one is not understood. Separately the reader is unbounded — no 16 MiB line cap, no `protocol_output_limit` diagnostic. | G118, G75 |
| **Subagent tool names and schemas** | cyrup registers the tool as `wait`, upstream renamed it `subagent_wait` (`extension.rs:5358`); the acceptance enum still advertises `none`/`verified`/`reviewed` to the model, which upstream v0.43 rejects; `sj_chain_item` is `additionalProperties: false`, so a v0.43-shaped chain step carrying `checkpoint`/`gateOn`/`agentContract` is rejected outright. | G86, G78, G105 |
| **Remote-session wire format absent entirely** | pi now ships a framed-CBOR `PROTOCOL_VERSION 1` (16 MiB frames) that cyrup can neither produce nor consume; cyrup's nearest surface is line-delimited JSON over stdio, and `cyrup-intercom/src/transport/framing.rs:19` caps JSON at 1 MiB — a different protocol that must not be merged into it. | G69, G70, G71 |
| **Child process environment** | pi stamps `AI_AGENT="pi"` in both `cli.ts:14` and `rpc-entry.ts:8`; cyrup stamps neither that nor a coding-agent marker, so every bash/extension/subagent child sees a different environment. | G73 |
| **Extension/config surface drift** | cyrup persists `companionSuggestions` and registers a 14th slash command upstream deleted before the ported tag; cyrup reads a third approvals file upstream removed. | G94, G129 |

---

## 6. Recommended sequence

Twelve batches, each independently landable and testable, ordered so nothing depends on a later batch.

**Batch 1 — Permission gate: security, audit, policy surface.** G134 (containment check between the target-session filter at `forwarding.rs:570` and `resolve_forwarded_decision` at `:572`, reusing `common::is_path_within_directory`), G131 (drop the `debug` gate at `logging.rs:148-155`; six live call sites become non-no-ops), G132 (500-char cap in `wildcard.rs:42`), G135 (three-name skip in `common.rs:166`), G129 (delete `PermanentApprovalStore` and its three read sites), G130 (`enabled` switch — see the open question on `is_pristine_default_file` first), G133 (config save: preserve non-extension keys, refuse corrupt, follow symlinks — no save fn exists at all today). No dependencies.

### 2.3a Items discovered while working Batch 1 (not in the original 147)

Each was found by the batch's own review, not by the survey — the survey listed primitives without
checking that their CONSUMERS were ported. Counted separately so the 147 stays comparable.

| item | status | sev | effort | cyrup location | upstream ref |
|---|---|---|---|---|---|
| G131b Forwarding half of the review audit trail — cyrup has 6 of upstream's 18 `writeReviewEntry` sites; `forwarding.rs` holds no logger at all | absent | high | medium | `src/forwarding.rs` (no logger reference) | v0.8.0 `index.ts` (18 sites) |
| G133b Extension-provided-API registry in `cyrup-ext`, so one extension can call another's methods | absent | medium | large | none; `NativeExtension` has no "publish an API object" hook | `yolo-mode-api.ts:23-43` (`globalThis.__piPermissionSystem`) |
| G133c `HostServices` custom-overlay seam, so `/permission-system` can render the real settings modal instead of text | absent | low | medium | `HostServices` has `select`/`input`/`notify`/`set_status`, no custom render | `config-modal.ts:63-123` (`ctx.ui.custom`) |
| G133d Version-qualify the 334 `index.ts` citations in `cyrup-permission-system` — 26 provably point past EOF at v0.8.0 | absent | medium | medium | 16 files; run `.workflows/check-citations.py` | n/a (provenance hygiene) |

### 2.5 Found while working Batch 4 (2026-08-09)

| item | status | sev | effort | cyrup location | upstream ref |
|---|---|---|---|---|---|
| G30b Compact-resource read classification — `getCompactReadClassification` + `formatCompactReadCall` are entirely unported | absent | medium | medium | `cyrup-tui/src/transcript.rs:1144` (`render_read`, no classification branch) | v0.83.0 `coding-agent/src/core/tools/read.ts:37,117,133-134,331` |

**G30b is a PORT BUG at v0.83.0, not version lag.** Commit `8ecf8a988` (which gave us G30) touched
TWO files; only `resource-loader.ts` was ported. The other half classifies a read result as
`{kind:"resource"|"docs", label}` for compact display, and the whole mechanism predates the ported
baseline — upstream's commit merely prepended `"AGENTS.override.md"` to an existing set.

It is a RENDERING concern (`read.ts:334-343` -> `renderCall`), so it belongs in `cyrup-tui`, not
`cyrup-tools` where a first search for it looked. It is blocked on plumbing, not on understanding:
`render_read` has no `cwd` in scope (needed by `resolveToCwd`/`formatPathRelativeToCwdOrAbsolute`)
and the `docs` arm needs `getReadmePath()` (`config.ts:427`), whose cyrup analog `DocsPaths` lives
in `cyrup-session/src/prompt/builder.rs` and never reaches the renderer. Landing it means threading
two inputs into the transcript renderer plus four pi theme keys and the expand-hint text — a feature
port. A partial version would create a NEW divergence harder to audit than the current clean absence.

**G50 IS COUPLED TO G8 AND G27 — do not verify it alone.** G50 (the shorter length-stop notice) came
from upstream `32850ef7c`, which also changed four behavioural files cyrup does not yet implement:
`ai/src/utils/overflow.ts` (classify a length stop by comparing reported output usage against the
model's output limit), `ai/src/api/openai-responses-shared.ts` (only `max_output_tokens` is a length
stop; other reasons surface as errors), and `coding-agent/src/core/agent-session.ts`
(compact-and-retry once, then strip the truncated assistant message). Upstream shortened the wording
BECAUSE a length stop became ambiguous — pi now sometimes recovers. cyrup shows the neutral notice
and does not recover, so porting the string alone strictly REDUCES the information a user gets.
That was a sequencing error in §6: G50 belongs in the batch that lands G8 and G27, and those batches
must re-verify the notice end-to-end rather than treat G50 as done.

**G136c and G136d are FIXED, not filed** (2026-08-08). They were briefly recorded here as tracked
items with a rationale for deferring them; that was wrong — deferring a known-broken wire guard is
shipping a defect, and the fixes were small once pi's source was actually read rather than inferred.

- **G136c** — an overflowing numeric literal (`1e400`) failed the WHOLE frame in `serde_json`
  (`number out of range`), in positions cyrup does not model and pi never type-checks. Fixed in
  `transport/framing.rs::from_frame_slice`: on a syntax error, overflowing literals are rewritten to
  `null` and the frame is re-parsed. That is exactly what pi puts on the wire — `JSON.parse` yields
  `Infinity`, `JSON.stringify` emits `null` — so an unmodelled key now round-trips byte-identically.
  The slow path runs only after a real overflow error, so well-formed traffic pays nothing.
  Six tests, revert-proved; mirrors confirm malformed JSON is still rejected and that a
  numeric-looking substring inside a string is never touched.

  Correcting the record on the earlier claim that "pi would have served" a `1e400` in a MODELLED
  field: it would not. pi's broker accepts it, answers the sender `delivered`, relays `null`, and
  the RECEIVER's own `isMessage` then rejects it and destroys that receiver's socket
  (v0.9.2 `broker/client.ts:106-116`, `:321-329`) — a hostile sender disconnecting a third party.
  cyrup answers `delivery_failed` to the sender instead. That is the one place cyrup deliberately
  does not reproduce upstream, and it is fail-closed.

- **G136d** — the five residual acceptance deltas (`registered.features: null`, the
  `extension_owner`/`extension_message` XOR-presence rule, `revision` beyond `MAX_SAFE_INTEGER`,
  `extension_state_result.reason: null`, and absent `payload` wrongly REJECTED where pi accepts it).
  All fixed in `transport/protocol.rs` with a `js_safe_integer` bound and `present_non_null` guards,
  each transcribed from the pi guard it ports. Six tests, full revert proof.

**On G133b/G133c**: `set_yolo_mode`, `toggle_yolo_mode` and `yolo_mode` are ported and correct but
UNREACHABLE, because the host seam they need does not exist. That is a named, tracked gap — not the
same thing as G133's original defect, where a primitive was unwired with no plan. Do not "fix" it by
re-routing the `/permission-system` command through them: upstream's modal sends every row through
`setConfig`, and an earlier revision of this batch distorted that routing to manufacture a caller.

**On G133d**: a BARE `index.ts:1447` is correct when the citing code ports v0.7.1 and wrong when it
ports v0.8.0. A crate mid-upgrade contains both, so bare citations cannot be checked at all. The
checker only proves the *impossible* ones (past EOF); a clean run means "no proven breakage", never
"citations verified".

> **Scheduling correction (2026-08-08).** As first written, §6 scheduled only 144 of the 147 items:
> **G133, G98 and G113 appeared in the §2 tables but in no batch**, so following the plan literally
> would have completed 12 batches and silently left three gaps — exactly the "no silent caps"
> failure this document exists to prevent. G133 is added to Batch 1 above (same crate), G98 to
> Batch 7 (it is agent-frontmatter launch defaults, alongside the other `discovery/` work), and
> G113 to Batch 12's watchdog cluster (it belongs with `watchdog/model-selection.ts`). Re-run the
> arithmetic — every id in §2 must appear in §6 — after any edit to this section.

**Batch 2 — Intercom wire survivability.** G136 (tolerant tag decoding on both `transport/client.rs` and `broker/mod.rs`; preserve unknown `Message` fields through the broker's re-forward at `broker/mod.rs:427`), G144 (liveness gate before the `remove_file` at `broker/mod.rs:835`). Makes cyrup safe to point at a modern pi broker before any feature work lands.

**Batch 3 — Session/interop safety in the core types.** G1+G23 interop slice only: add `StopReason::Deferred` and the context-exclusion filter so a pi-written session round-trips instead of truncating; G12 (apply the `bedrock_converse_stream.rs:454-457` pattern at `openai_responses.rs:996-1000`); G43 (`to_json_event` projection ahead of both serializers). Batch 12's full deferred-run model builds on the variant landed here.

**Batch 4 — One-line and one-string corrections.** G9, G64, G15, G30, G50, G41, G65(a), G21, G11, G6, and the JSON-only half of G13. Each is a single site with an existing helper; land together with the tests that currently pin the old values (`cyrup-tools/tests/bash_session_env.rs:145`, `cyrup-tui/tests/stop_reason.rs:24`).

**Batch 5 — Wire the present-but-unwired.** G14 then G35+G49 (the flag must exist before the footer can read it), G93 + G147 resource registration, G128 renderer wiring via `register_message_renderer`/`render_result`, G145, G143. All host seams already exist and are proven live (§4).

**Batch 6 — pi pre-baseline port bugs.** G39, G3 (`list()` only), G62 (editor page actions then the ctrl aliases), G63 (Apple Terminal path first, win32 second), G66 (OSC 9;4 emitter behind the already-wired setting), G16/G42 parent qwen providers.

> **SEQUENCING CORRECTION (2026-08-10).** The batches below were organised around "pre-baseline
> debt" vs "version lag". That split is useful for ORDERING WITHIN a target — something broken today
> outranks something never added — but it is NOT the goal, and letting it organise the plan made
> reaching latest look like an optional later phase. **The goal is the current state of all four
> upstreams.**
>
> Status by upstream: pi-permission-system is AT v0.8.0 (batch 1); pi-intercom is AT v0.9.2
> (batch 2); pi is mid-way to v0.84.1 (batches 3-6 done). **pi-subagents is the outlier — nine minor
> versions behind at ~v0.34.0 against v0.43.0**, with 54 backlog items scattered across batches 7,
> 8, 9 and 12, and three entirely-new subtrees (`watchdog/` +4,395, `missions/` +1,659,
> `tui/fleet*` +1,856) parked last behind unrelated pi work.
>
> **Batches 7-9 and the subagents half of 12 now run CONTIGUOUSLY as one effort: pi-subagents to
> v0.43.0.** pi's remaining batches (10-11) follow. Nothing is dropped; the route changes so that
> "at latest" is reached per-upstream rather than left as a trailing phase.

**Batch 7 — Subagents pre-baseline debt.** G90 `steer`, G91 `schedule*`, G92 `view`/`lines` + `/subagents-fleet`, G95 `memory:`, G89 budgets, G98 agent launch defaults, G96 frontmatter parser, G106 native supervisor channel, G94 companions removal. Note the crate's own advertise-vs-dispatch invariant (`extension.rs:10036-10041`) means every new enum value needs a real dispatch arm in the same change.

**Batch 8 — Subagents run-loop correctness.** G75 bounded NDJSON reader, G76 drain start/cancel (consume the already-parsed `will_retry` at `ndjson.rs:156`), G88 model resolution + `TOOL_FAILURE` prefix guard (live today: cyrup's own formatter emits the guarded prefix at `exec/output.rs:352-359`), G74 startup retry, G81 `$ref` rewrite, G103 empty tools list, G102 discovery pruning, G84 mutation detection, G100 profile merge.

**Batch 9 — Subagents acceptance, output and state model.** G78, G79, G80, G82, G83, G77+G104 `stopped`, G97, G99, G101. G78 must precede G105's narrowed enum.

**Batch 10 — pi v0.84 provider and auth layer.** G8 (lands `is_recoverable_length`) → G27 (consumes it); G2, G3 (full cancellation threading), G4+G19+G37 samplingParams, G5, G7+G42 baseten, G10, G13 (remaining catalogs), G31, G38, G40, G36. Order within: G8 before G27; G2 before G51.

**Batch 11 — pi v0.84 agent, session and modes.** G17+G32, G18, G20, G21, G22, G26, G28, G29, G51 (needs the cancellable refresh from Batch 10), G52, G53, G73. Then the TUI program in dependency order: G34+G47 settings and theme token → G46+G54 fullscreen renderer → G56, G57, G58, G59, G60, G68, G48+G61 → G33+G45 transformer API → G44 mermaid, G55 latex, G67.

**Batch 12 — New upstream subsystems.** In closure order: G69 protocol → G70 client → G71 `RemoteSession`; G72/G25 telemetry; G24 harness v2 (see open question); the subagents watchdog cluster G107 → G113 → G111 → G109 → G110 → G108 → G112 → G114 → G116/G117 → G115 → G118 → G119; missions G120 → G121 → G122 → G123 → G124; fleet G125 → G126 → G127; G86 wait rework, G87 capability ceiling, G105 v0.43 schemas, G85 checkpoints; intercom G141 → G142 (prerequisite for G138's identity scoping) → G138 → G139 → G140 → G143 → G137 → G146.

---

## 7. Killed claims

**None.** The panel refuted zero of the 147 gaps; all three lenses returned "stands" on every item.

It did, however, correct evidence inside surviving items. These corrections are already folded into the tables above and are listed so nobody re-files the wrong version:

- **G52** — "`set_provider_count` has no caller" is **wrong**: there is a production caller at `crates/cyrup/src/main.rs:1749` (startup). The true claim is narrower: no *post-login* provider-count refresh exists.
- **G12** — "`An unknown error occurred` is present in `bedrock_converse_stream.rs` only" is wrong; it appears 16 times across five API files. The load-bearing part holds: it appears nowhere in `openai_responses.rs` or `azure_openai_responses.rs`.
- **G51** — `refresh_providers` has **one** non-test caller, not two (the survey counted the definition). The TUI half of the gap is unaffected.
- **G54** — `fullscreenExitOutput` does **not** exist at v0.84.1 (`git grep` empty); it appears only at pi HEAD, 33 commits later. That sub-claim is outside the declared diff window and has been dropped.
- **G22** — `"leaf"` is not in cyrup's `KNOWN_TYPES` (`entry.rs:140-150`); the item covers `custom_message`/`label`/`session_info` only.
- **G30** — the "11 hits" figure was regex-dot noise; a literal `grep -rn "AGENTS\.override" crates` returns 0.
- **G69** — the CBOR "matches" in cyrup are all substring hits on `DynamicBorder`; no CBOR crate is in the graph.
- **G88** — `resolve_subagent_model_override`'s two call sites (`extension.rs:2956,3015`) are both inside the `models` management-action report formatter, confirmed by reading them; the launch ladder is `build_model_candidates` only.
- **G21 / G111 / G117 / G128** — upstream line anchors were out of range or misread (`extractFileOperations` is `compaction.ts:44-67` not `:630-641`; `turn-delta.ts` is 161 lines; `tool-actions.ts` is 155 lines; the `render.ts` diffstat is +456/−175, not "+631/−175"). Corrected refs are in the tables.
- **G19 / G61 / G146** — off-by-a-few citations (`openai-responses.ts:330-333` not `:338-339`; `keybindings.ts:66-73` not `:68-75`; `getBrokerListenTarget` is `paths.ts:107`).

Two in-tree justifications were examined and do **not** close their items: `crates/cyrup/src/main.rs:53-57` declines the `AI_AGENT` stamp citing a "residual ledger" that does not exist in this workspace (and the argument fails anyway — the observable behaviour is child inheritance, and `cyrup-tools/src/tools/bash.rs:138-153` already pushes `CYRUP_*` onto a child env vector with no `unsafe`); and `crates/cyrup-ext-subagents/src/extension.rs:10017-10032` asserts `steer` and `schedule*` are deferred per `SUBA-013`/`SUBA-016`, requirement ids with no corresponding document in this workspace.

---

## 8. Open questions

1. **Which compaction/session-entry lineage should cyrup track?** cyrup's entry model cites `session-manager.ts` (`context.rs:157,204`) and its compaction ports the *harness* copy, but v0.84.0 diverged the two: `retainedTail` and the shrunken entry vocabulary are harness-only (`git diff v0.83.0..v0.84.1 -- packages/coding-agent/src/core/compaction/` is empty). Adopting the harness semantics is a session-file format change. Look at `crates/cyrup-session/src/compaction/prepare.rs:81-92` and `crates/cyrup-session/src/entry.rs:57-150` against both upstream copies. Blocks G20, G21, G22.
2. **Does harness v2 enter scope?** At v0.84.1 the only in-repo consumer of `AgentHarness` is `pi/packages/coding-agent/src/server/create-harness.ts`, i.e. the `packages/server` closure that cyrup does not port; the interactive and print paths still go through `session-manager.ts`. Confirm at that file whether coding-agent's live paths are moving. Sizes G24 (and part of G25).
3. **What value should cyrup stamp for `AI_AGENT`?** Upstream writes `"pi"` in both entrypoints and nothing in pi reads it — it exists purely for downstream child processes. A tool gating on `$AI_AGENT == "pi"` will not recognise `"cyrup"`. Decide against the `PI_*`→`CYRUP_*` precedence convention at `crates/cyrup-config/src/env.rs:68-91`. Affects G73.
4. **Is renaming `wait` → `subagent_wait` safe for existing cyrup users?** The tool name is a wire contract for any skill or caller written against either side. See `crates/cyrup-ext-subagents/src/extension.rs:5356-5358` and the registration at `:7158-7159`. Affects G86.
5. **Does adding an `enabled` key break the permission-system install probe?** `ExtensionConfig::is_pristine_default_file` (`crates/cyrup-permission-system/src/ext_config.rs:205-207`) does an exact string compare against `default_config_content()`, and `is_installed()` (`extension.rs:947`) depends on it. A fourth key would stop an existing three-key file reading as pristine. Affects G130.
6. **What does `CompiledWildcard` do with `regex: None`?** G132's never-match arm can be either `$^` or `None`, depending on how the `Option<Regex>` consumers treat absence. Check the consumers of `crates/cyrup-permission-system/src/wildcard.rs:32-66` before choosing.
7. **Is cyrup's blank-first-line dedent divergence reachable?** cyrup takes the prefix from `raw_block`'s first characters (`crates/cyrup-ext-subagents/src/discovery/frontmatter.rs:210-230`) where v0.34.0 already used `/^([ \t]+)/m`, but a blank line may flush the block in both parsers, in which case the difference is unobservable. Determine before sizing G96.
8. **How should catalogs be kept current?** `packages/ai/src/providers/data/*.json` does not exist at v0.84.1 — the `.models.ts` files are generated, so `scripts/generate-models.ts` is the only oracle for the pricing and compat corrections. Decide whether `crates/cyrup-provider/src/providers/catalog/` gets a regeneration pipeline or stays hand-maintained. Affects G13 and every future catalog drift.