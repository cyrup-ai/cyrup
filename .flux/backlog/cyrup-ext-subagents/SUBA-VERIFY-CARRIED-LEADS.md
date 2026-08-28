---
stage: new
status: done
updated: 2026-08-27 05:30
severity: unknown
effort: medium
subsystem: verification
source: docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md
item: SUBA-082,084,086,087,088,089,090,091
---

> **Path note.** This task lives in a subdirectory. Every flux command globs a single level
> (`ls -1 "$FLUX_BASE/todo/"*.md`), so `/exec`, `/aug` and `/qa` will not list it — pass the absolute
> path explicitly.

# Verify The Eight Carried pi-subagents Leads, Then File Them

## Description

The v0.57.0 parity audit confirmed eleven gaps through an adversarial refutation pass, but that pass
was capped at twelve items. These **eight leads were surveyed and deduped but never adversarially
verified**, so none of them is established fact yet. One item in the same batch (SUBA-080) was
refuted at exactly this stage — a plausible, well-argued finding that turned out to be wrong because
the behaviour lived in a host layer. Treat these the same way until each is checked.

Do NOT implement from these descriptions. Verify first, then file the survivors as their own tasks.

## Carried — NOT adversarially verified

> **READ THIS BEFORE ACTING ON ANYTHING IN THIS SECTION.** The refutation pass for this batch was
> capped at twelve items. The eight items below were produced by the same analyst lenses as the
> confirmed set and are carried forward **unrefuted**. They are held to a lower evidence standard and
> **must not be counted alongside the verified items** — the same treatment `README.md` gives
> `DRIFT-023` / `DRIFT-040` as *leads*.
>
> **What this writer did personally verify:** the **port-side zero-hit greps** for every one of them,
> re-run against the current tree at HEAD `6db22a7`, plus the port line ranges quoted in
> `SUBA-089`, `SUBA-090` and `SUBA-091`. Those results are marked *(re-verified)* below.
> **What was NOT re-verified: every upstream line number in this section.** They are the analyst's,
> reproduced as filed, and a maintainer must settle each with `git show v0.57.0:<path>` before
> scheduling the item. Where a filing asserts an upstream shape, treat it as a hypothesis.

### SUBA-082 — Agent `acceptanceRole:` and `acceptance:` frontmatter are not in the schema, so the acceptance classifier is driven purely by the agent-name regex

**Severity** high (as filed) · **Effort** M · **Window** in-baseline (≤ v0.43.0) · `3c635cc1`

*Upstream, as filed (unverified):* `agents.ts:144-145` puts `defaultAcceptance?: AcceptanceInput` and
`acceptanceRole?: AcceptanceRole` on `AgentConfig`; `:1873-1884` `parseAgentAcceptanceFrontmatter`
YAML-parses `frontmatter.acceptance` and validates it; `:2011-2015` parses `acceptanceRole`, throwing
``Agent '<name>' has invalid acceptanceRole frontmatter; expected 'read-only' or 'writer'.``;
`agent-serializer.ts:24-25` lists both in `KNOWN_FIELDS`. The role is the PRIMARY input to the
acceptance-level classifier, which falls back to name matching only when it is `undefined`:
`input.acceptanceRole === "read-only" || (input.acceptanceRole === undefined && /\b(?:reviewer|oracle|scout|researcher|analyst)\b/.test(agent))`.

*Port (re-verified at HEAD):* `grep -rn 'acceptance_role' --include=*.rs crates/cyrup-ext-subagents/src`
→ **0 hits**. `src/discovery/frontmatter.rs:72-116 KNOWN_FIELDS` contains neither `acceptance` nor
`acceptanceRole`, so both are demoted to `extra_fields`. `AcceptanceResolveInput`
(`src/exec/acceptance/model/level.rs:43-51`) has `explicit`, `agent_name`, `task`, `mode`, `is_async`,
`dynamic`, `dynamic_group` — no role. The only `acceptanceRole` mentions in the crate are comments in
`level.rs` and `src/tests/read_only_agent_name_alternation.rs`, each quoting upstream's
`acceptanceRole === undefined` branch and noting the port implements only that branch.

*Behaviour gap:* an agent named `security-reviewer` that declares `acceptanceRole: writer` is still
gated read-only, and a writer-named agent declaring `read-only` is gated as a writer — a silently
wrong acceptance level with no error telling the author the key did nothing. Agent-level `acceptance:`
policy defaults are likewise unreachable; only the per-call `explicit` input exists.

*Relation:* distinct from `SUBA-081`'s settings-override half (`agentOverrides.<n>.acceptanceRole`) —
this is the frontmatter half, in a different parser. Land them together.

### SUBA-084 — Runtime agent registration is entirely absent: no `registerAgent` API, no `runtime` source tier, no runtime/configured collision checks

**Severity** high (as filed) · **Effort** L · **Window** v0.47.1..v0.57.0 · `2c031d06 (#1320)`

*Upstream, as filed (unverified):* `src/agents/runtime-agent-registry.ts` (418 lines) — the registry
key `"pi-subagents.runtime-agents.v1"`, per-runtime caps (200 agents, 128-char name, 4 KiB
description, 1 MiB systemPrompt, 8 KiB per field), a 32-field `RuntimeAgentDefinition`, a per-field
validator, alias normalization plus `assertNoIdentityCollisions` / `assertNoRuntimeCollision` /
`assertNoBuiltinCollision`, `toAgentConfig` stamping `source: "runtime"` and
`filePath: "runtime:<name>"`, `registerRuntimeAgent` returning an idempotent `dispose()`, and
`mergeRuntimeAgents`. It is a PUBLIC API re-exported by `src/api/agents.ts` as `registerAgent(input)`
and wired end to end: merged into every discovery, cleared on dispose, merged by the slash commands
so a runtime agent is slash-invocable, merged into `{action:"list"}`, and given its own precedence
rank above `project` in `agents.ts`.

*Port (re-verified at HEAD):*
`grep -rnE 'runtime_agent|RuntimeAgent|register_agent' --include=*.rs crates/cyrup-ext-subagents/src`
→ **0 hits**. `src/discovery/types.rs:43-51` defines `pub enum AgentSource { Builtin, Package, User, Project }`
— four variants, no `Runtime` — and `precedence_rank` at `:53-66` covers exactly those four.
`src/discovery/merge.rs:104-124 merge_tiers` takes a
`TieredAgents { builtin, package, user, project }` with no fifth tier and no in-memory registry input.

*Behaviour gap:* nothing an embedder registers in-process can ever be delegated to; an agent can only
exist as a file on disk.

*Relation:* **NOT** the same as `SUBA-022` (typed extension delegation API — upstream's
`executeDelegated`, a way to RUN a subagent, not to DEFINE one). Two lenses filed it independently;
merged here.

### SUBA-086 — Per-agent parse diagnostics are absent: a malformed agent file is silently degraded to defaults instead of being reported by name and blocking its own agent name

**Severity** high (as filed) · **Effort** M · **Window** v0.47.1..v0.57.0 · `e973fa3c`

*Upstream, as filed (unverified):* `agents.ts:229-234` `AgentDiscoveryDiagnostic { source, filePath,
error, name?, runtimeName?, packageSpecified?, discoveryPriority? }`; `:1923-2110` wraps the whole
per-file parse in `try { … } catch` and `:2106` pushes a diagnostic naming the file, the agent name
and the exact throw message — so *every* validation throw in the parse body (invalid `package`,
`async`, `timeoutMs`, `toolTimeoutMs`, `turnBudget`, `acceptance`, `outputMode`, `acceptanceRole`,
`fast`, `toolBudget`, both-permission-spellings, runner profile) becomes a SURFACED diagnostic rather
than a silent skip; `:2267-2273` returns them as `agentDiagnostics`; `:244-264`
`agentDefinitionPriority` + `findBlockingAgentDiagnostic` make a broken definition BLOCK resolution of
that name with its parse error instead of falling through to "Unknown agent";
`agent-management.ts:760-764` renders them under `Invalid agent definitions:` in `{action:"list"}`.

*Port (re-verified at HEAD):*
`grep -rnE 'AgentDiscoveryDiagnostic|agent_diagnostic' --include=*.rs crates/cyrup-ext-subagents/src`
→ **0 hits**. `src/discovery/mod.rs:1174-1191 AgentDiscoveryResult` carries
`diagnostics: Vec<ChainDiscoveryDiagnostic>` — **chain** diagnostics only. The port states the design
in-tree at `src/discovery/frontmatter.rs:753-757`: *"Never returns an `Err` for a malformed individual
agent file"* — `parse_agent_file` returns `Option<AgentDefinition>` and every field parser degrades a
bad value to `None`. `src/discovery/management/handlers.rs:109-113,142-150` filters and renders only
`ChainDiscoveryDiagnostic`; there is no `Invalid agent definitions:` section anywhere.

*Behaviour gap:* a typo (`timeoutMs: 30s`, `outputMode: file`, `defaultContext: forked`) is reported
upstream by name and file and blocks delegation to that name; in the port the bad field is silently
coerced to absent, the agent loads with default behaviour, and neither `list` nor the delegation path
ever mentions the file.

*Relation:* new. **Note for the planner: this is the highest-leverage item in the discovery cluster**,
because it converts `SUBA-081`, `SUBA-082` and every "key demoted to `extra_fields`" row in this batch
from silence into user-visible errors.

### SUBA-087 — Child-scoped stop (`childId`) is unported: `stop` can only terminate an entire async run and its whole descendant subtree

**Severity** medium (as filed) · **Effort** M · **Window** v0.47.1..v0.57.0 · `31a230cb (#1373)`, `de594cfd (#1375)`

*Upstream, as filed (unverified):* `src/runs/shared/child-identity.ts` (36 lines, new in the window) —
`asyncStatusChildIdentity(step, index)` = `step.workflowKey ?? step.runId ?? \`step:${index}\`` (the
`step:${index}` fallback is the NON-workflow case), `resolveAsyncStatusChild` returning
`{ok:false, code:"not_found"|"ambiguous"}`, `isStoppableAsyncStatusStep` restricted to
`pending`/`running`. `schemas.ts:278` advertises `childId`; `control-channel.ts:53-60` adds
`targetIndex?: number; childId?: string` to `StopRequest` plus a per-child `control/stop-requests/`
directory drained newest-last; `async-stop-action.ts:24-86` refuses a non-stoppable child with
``Child '<id>' in async run '<run>' is <status>; stop only supports pending or running children.``;
`subagent-runner.ts:2837-2887` flips only that step and emits
`subagent.step.stop_requested`/`stop_queued`/`stopped`.

*Port (re-verified at HEAD):* `grep -rn 'childId' crates/cyrup-ext-subagents/` → **0 hits** (the 27
`child_id` hits are unrelated test fixtures in `src/registration/cost.rs`).
`src/background/control.rs:624-651` `pub struct StopRequest { kind, ts, source, reason }` — no
`target_index`, no `child_id` — while the sibling `SteerRequest` at `:821-874` **does** carry
`pub target_index: Option<usize>`, so the per-child targeting machinery exists and stop simply does
not use it. `stop(async_root, results_dir, run_id_token, source, reason)` (`:691`) takes no child
argument; `grep -rn 'stop-requests\|stop_requests'` → 0 (the port has only the single
`control/stop.json`). `src/extension/tool/params.rs` has no `child_id` field and no
`deny_unknown_fields`, so a `childId` sent by an upstream-shaped caller is **silently discarded** by
serde and the stop proceeds against the whole run.

*Behaviour gap:* with a 5-wide fan-out running and one child gone bad, upstream stops that one and the
four healthy siblings run to completion. cyrup cannot express it: the entire run and its whole
descendant subtree are terminally stopped, and stopped runs are explicitly non-resumable, so the
siblings' partial work is lost.

*Relation:* deliberately **NOT** folded into `VL-S2` (`workflowScript`) despite the commit's title —
upstream's identity scheme falls back to `step:${index}` precisely for the non-workflow case, and the
port already has `RunStatus.steps` with per-step state plus per-child steer targeting, so this is
portable today with no workflow runtime. Three lenses filed it independently; merged.

### SUBA-088 — `subagents.defaultProvider` and per-agent `modelProvider` are unported, and the foreground launch path passes no preferred provider into candidate resolution at all

**Severity** medium (as filed) · **Effort** M · **Window** v0.47.1..v0.57.0 · `cc112354 (#1394)`

*Upstream, as filed (unverified):* `agents.ts:132` `modelProvider?: string` on `AgentConfig`, `:177`
`defaultProvider?: string` on `SubagentSettings`, `:86` `defaultProvider?: string | false` on
`BuiltinAgentOverrideConfig` (this one **is** confirmed — see `SUBA-081`'s verified 22-field list),
`:116` on `AgentModelSourceInfo`; `:997-1004` parses it; `:1045-1051` `resolveSubagentDefaultProvider`
(project beats user); `:1155-1168` `applySubagentDefaultModel` stamps `modelProvider` onto every agent
that has not pinned its own — including agents that already pin a MODEL. Consumed at
`execution.ts:1826-1831`:
`buildModelCandidates(options.modelOverride ?? agent.model, agent.fallbackModels, options.availableModels, agent.modelProvider ?? options.preferredModelProvider, {...})`,
whose 4th parameter drives `resolveRequiredSubagentModelCandidate`/`resolveSubagentModelCandidate`
(`model-fallback.ts:366-397`) — i.e. which provider a BARE model id resolves against.

*Port (re-verified at HEAD):*
`grep -rnE 'default_provider|defaultProvider|model_provider|modelProvider' --include=*.rs crates/cyrup-ext-subagents/src`
→ **0 hits** for every spelling. `src/discovery/types.rs:505-541 SubagentSettings` has no
`default_provider`; `AgentDefinition` has `model` and `model_source` but no provider field;
`src/discovery/merge.rs:223-250` applies only `defaultModel`, with no provider parameter.
`src/exec/fallback.rs:127-132 build_model_candidates(model_override, agent_primary_model, agent_fallback_models, available_models)`
has **no** provider parameter (nor does `build_model_candidates_scoped` at `:174-180`). The unrelated
`AgentConfig::preferred_provider` (`src/exec/agent_config.rs:349`) is `None` at every foreground call
site (`src/extension/executor/foreground.rs:361`, `src/background/runner_main.rs:2572`,
`src/exec/testsupport.rs:59`); only `src/extension/executor/reports.rs:182` populates it, and that is
the report surface, not the launch path.

*Behaviour gap:* `subagents.defaultProvider: "openai-codex"` has no effect — not parsed, not merged,
and no channel to reach candidate resolution. An agent naming a bare model id cannot be steered to a
particular provider, and a bare id that several providers offer resolves without the user's preference.

*Relation:* distinct from `SUBA-050` (`modelScope.strict`, an allowlist) and `SUBA-035` (surfacing the
scope policy) — this is provider *preference* feeding candidate resolution. Merges three lens
candidates sharing one fix site: `build_model_candidates`' signature plus the settings parse. Pairs
with `SUBA-081`'s `defaultProvider` override field.

### SUBA-089 — The model-fallback retry decision ignores whether the failed attempt already ran tools, so a half-completed mutating run is re-dispatched

**Severity** medium (as filed) · **Effort** S · **Window** v0.47.1..v0.57.0 · `d8d1408d`

*Upstream, as filed (unverified):* `src/runs/shared/model-fallback.ts:467-474`
`isRetryableModelFailureAttempt({error, messages, toolCount})` — retryable only if
`isRetryableModelFailure(error)` AND `(toolCount ?? 0) === 0` (`:469`
`if ((input.toolCount ?? 0) > 0) return false;`), with a further correlation requirement that the
error be the cold-start sentinel, or the run produced no messages, or some assistant message's own
`errorMessage` equals the run error (`:471-473`). `src/runs/foreground/execution.ts:2051` is the sole
foreground ladder gate, and `:2058` breaks the loop on `!retryableModelFailure`. At v0.43.0 and
v0.47.1 the same line was the bare `isRetryableModelFailure(result.error)`
(`v0.47.1:execution.ts:1633`), so the narrowing is new in the window.

*Port (re-verified at HEAD):* `crates/cyrup-ext-subagents/src/exec/fallback.rs:1265-1270` is the whole
retry gate —
```rust
if !is_retryable_model_failure(signal.error.as_deref()) {
    last_signal = Some(signal);
    last_attempt = Some(attempt);
    break 'ladder;
}
```
— the attempt's tool count and message set are never consulted.
`grep -rn 'is_retryable_model_failure_attempt' --include=*.rs crates/cyrup-ext-subagents/src` →
**0 hits**; `grep -n 'tool_count' src/exec/fallback.rs` shows the only uses are in `StartupEvidence`
and inside `is_retryable_subagent_startup_failure`, a different gate that fires before any model is
retried.

*Behaviour gap:* a foreground subagent that ran ten tool calls — edits, writes, git commands — and
then hit a transient `connection reset` / `overloaded` error is re-dispatched from scratch on the next
fallback model. Upstream stops the ladder because `toolCount > 0`, precisely so a half-completed
mutating run is not repeated. The port duplicates the child's side effects and doubles the token spend
on every mid-run provider blip.

*Relation:* new. This pass confirmed the rest of the fallback ladder (R-SA-036 ordering, retryable
patterns, attempt notes, usage aggregation, the startup-retry sub-ladder) present and correct — this
is a single missing predicate inside ported code.

### SUBA-090 — Completion notices are always rendered: the port hardcodes `display: true` where upstream hides a plain successful background completion and groups a batch

**Severity** medium (as filed) · **Effort** S · **Window** in-baseline (≤ v0.43.0)

*Upstream, as filed (unverified):* `src/runs/background/notify.ts:239` —
`const display = details.some((detail) => detail.source === "foreground" || detail.status !== "completed" || detail.scheduleOrigin !== undefined);`
then `:241-249` `pi.sendMessage({customType: "subagent-notify", content, display}, {triggerTurn: items.some((item) => item.triggerTurn)})`.
`v0.43.0:notify.ts:173` carries the same expression minus the `scheduleOrigin` clause, so it is
in-baseline. `:379` shows `triggerTurn: result.triggerTurn !== false` — per-completion, not a
constant. `:211-242` render a `Background tasks completed (N): …` header plus numbered blocks whenever
a batch holds more than one completion.

*Port (re-verified at HEAD):* `crates/cyrup-ext-subagents/src/background/watch.rs:741-746` —
```rust
CompletionMessage {
    custom_type: "subagent-notify".to_string(),
    content: lines.join("\n"),
    display: true,
    trigger_turn: true,
}
```
— both literals, with no branch on outcome or source anywhere in `format_completion_message`
(`:711-747`). **The struct's own doc at `:605-609` asserts** *"Always `true` (pi's `display: true`)"*
and *"Always `true` (pi's `{ triggerTurn: true }`)"* — a statement about upstream that
`notify.ts:239` contradicts at both tags. `grep -rn 'Background task'` shows only the singular header,
never the plural grouped form.

*Behaviour gap:* upstream injects a plain successful background completion as a NON-displayed context
message that still triggers a turn, rendering the notice only when something needs attention (a
foreground detach, or a failed/paused/stopped/scheduled outcome). A session that fans out ten
successful background tasks shows ten notices the port renders and upstream would have kept invisible,
and the grouped multi-completion form is never rendered at all.

*Relation:* new, and it **partially refutes `SUBA-017`'s framing**: `SUBA-017` (completion batching,
low) treats grouping as the missing piece, but the load-bearing half is the `display` predicate — a
two-line fix independent of the batcher. The port's own doc comment asserting upstream uses
`display: true` unconditionally is the reason no prior pass caught it.

### SUBA-091 — The fleet inspector passes an EMPTY trusted-root list to the transcript reader, so the session-transcript fallback always refuses

**Severity** medium (as filed) · **Effort** S · **Window** v0.47.1..v0.57.0 · `9ceb5650 (#1174)`

*Upstream, as filed (unverified):* `src/tui/fleet.ts`'s `asyncDetail(item, state)` calls
`formatAsyncRunTranscript(status, item.run.asyncDir, { index, lines: TRANSCRIPT_LINES, sessionRoots: uniquePaths([...(state.trustedSessionRoots ?? []), trackedJob?.sessionRoot]), trustedSessionFiles: [...], trustedSessionFileRoot: state.trustedSessionFileRoot })`.
`sessionRoots` is exactly what `readSessionTranscriptTail` confines its read to. Landed as
`9ceb5650 fix: pass trusted session roots to fleet transcripts (#1174)`; before it the call omitted
`sessionRoots`, which is the state the port is still in.

*Port (re-verified at HEAD):* `crates/cyrup-ext-subagents/src/tui/fleet.rs:842-848` —
`format_async_run_transcript(&run.status, &run.paths, step_index, Some(TRANSCRIPT_LINES as i64), &[])`
— the final `session_roots` argument is **a literal empty slice**, with no `[CYRUP-DELTA]` note in
`fleet.rs` justifying it. `src/background/fleet_view.rs:143-161 read_contained_text_tail` opens with
`if trusted_roots.is_empty() { return TextTail::failed(path, format!("Refusing to read {label} transcript path without a trusted root: {}", path.display())); }`,
and `read_session_transcript_tail` (`:618-632`) turns that into a `Warnings:` line.
`grep -rn 'trusted_session_roots\|trusted_session_file' --include=*.rs` → 0, so no plumbed equivalent
exists on the fleet side — **but the resolver already exists for the other consumer**:
`src/extension/executor/status.rs:389-396 transcript_session_roots(cwd)` builds
`[default_async_root, project_subagents_dir, temp_artifacts_dir]` and passes it at `:359`.

*Behaviour gap:* when a background run has no readable output log — its `output-*.log` is missing, or
the run only ever recorded a session file — upstream falls back to the child's session-JSONL tail and
shows it in the fleet inspector's detail pane. The port instead emits a `Warnings:` block saying it
refuses to read the path for lack of a trusted root, then shows
`(no transcript output captured yet)` — on a run whose transcript is on disk and which the port's own
`subagent status` path reads fine.

*Relation:* new. Same file as `SUBA-080` (refuted) but a different defect — argument value, not
missing sanitization. The fix is one call-site change reusing `status.rs:389`'s existing resolver.

---


## Scope

In scope: verifying each of the eight leads against the port and against `git show v0.57.0:<path>`,
then writing a task file per survivor into this directory and recording refutations back into
`docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md`.

Out of scope: implementing any of them in this task.

## Approach

For each lead, try to REFUTE it, defaulting to refuted:

1. Search `crates/cyrup-ext-subagents/src/` by behaviour and by symbol — including the decomposed
   `extension/` tree and `tests/`. A behaviour proven by a test is present.
2. Check whether a sibling crate provides it (`cyrup-ext`, `cyrup-intercom`,
   `cyrup-permission-system`, `cyrup-session-svc`). If so it is out of scope, not missing.
3. Re-read the cited upstream with `git show v0.57.0:<path>` and confirm it says what the lead says.
4. Never conclude "missing" from a stale path — `src/extension.rs` is now a directory, so any
   citation in `09-cyrup-ext-subagents.md` at that path cannot be resolved as written.

## Acceptance Criteria

- [ ] Each of the eight leads has a written verdict: confirmed (with the evidence) or refuted (with where the behaviour actually lives)
- [ ] Every confirmed lead has its own task file in this directory, in the format of the eleven verified ones
- [ ] Every refutation is recorded in the `## Refuted` section of the gap document so it is never re-derived
- [ ] No lead is left in an undecided state
