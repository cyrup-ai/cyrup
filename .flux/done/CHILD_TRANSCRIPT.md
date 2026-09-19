---
stage: qa
status: completed
updated: 2026-09-19
---

# Live child transcript — watch a delegated agent while it runs

OBJECTIVE: write `<base>_transcript.jsonl` LIVE as a child runs, so the FleetView transcript pane
and `/subagents status` show a RUNNING child instead of nothing until it finishes.

## What is broken — verified in the tree

- `tui/fleet.rs:1144-1150`: *"no writer emits `_transcript.jsonl` yet — the `.jsonl` event stream
  is still the artifact cyrup actually writes … When a transcript writer lands, switch this to
  `paths.transcript_path`."* The pane points at a file that does not exist until the child
  finishes, so it renders empty.
- `background/runner_main/status.rs:603` and `background/runner_main/executor.rs:1220` both publish
  `transcript_path: None`.
- `extension/executor/foreground_transcript.rs` is a **READER** (`read_fleet_transcript` at `:256`)
  formatting `transcript_path` for `/subagents status` — reading a file nothing writes.
- `artifacts.rs:82` (`ArtifactPaths::transcript_path`) and `:317` (minted as
  `<base>_transcript.jsonl`) — the PATH half landed (SCOPE_3a). The WRITER half did not.
- `spawn/mod.rs:1146`: *"cyrup has no `ChildTranscriptWriter` port yet, so its lines go to
  `tracing` at debug level."*

**This is a reader-without-writer, the same shape the handoff manifest was before #143, and the
same shape the recovery descriptor was before the sibling task landed it
(`background/recovery_descriptor.rs`, `## [EXEC — descriptor]` in `RECOVERY_DESCRIPTOR.md`).**

## ⚠ Attach to the RIGHT stream

`spawn/mod.rs:1146`'s comment sits on the **stderr** per-line reader (`CapturedStderr::pump`,
`:1163`, `:1237`). **That is not where the transcript comes from.** Upstream feeds the writer from
the PARSED CHILD-EVENT stream — stdout NDJSON:

- `src/runs/foreground/execution.ts:980` — `shared.transcriptWriter?.writeChildEvent(evt);`
- `src/runs/background/run-child-session.ts:410` —
  `input.transcriptWriter?.writeChildEvent(projectChildSessionEventForJson(raw) as ChildEvent);`

In cyrup that stream is `crate::exec::ndjson::parse_line`'s consumers — the foreground
`exec/drive_attempt.rs` (which already folds each event at `:372` `control.observe_event(&event, …)`)
and the background runner. Wiring the transcript to stderr would record diagnostics and miss the
conversation.

## Upstream, pinned at v0.68.0

Read ONLY via `git -C /home/user/cyrup/tmp/pi-subagents show v0.68.0:<path>`.

- **Writer**: `src/shared/child-transcript.ts` (264 lines). `createChildTranscriptWriter:102`,
  `CHILD_TRANSCRIPT_ARTIFACT_VERSION = 1` (`:30`), `MAX_TOOL_PAYLOAD_BYTES = 32 KiB` (`:6`),
  `DEFAULT_MAX_CHILD_TRANSCRIPT_BYTES = 50 MiB` (`:31`), `ChildTranscriptEvent:43`,
  `ChildTranscriptWriterInput:52`, `ChildTranscriptWriter:62` (`writeInitialUserMessage`,
  `writeChildEvent`, `getError`). Per-record `fs.appendFileSync` (`:133`).
- **Created**: background `subagent-runner.ts:879-889`; foreground `execution.ts:1841-1849`. Both
  immediately `writeInitialUserMessage(\`${PROMPT_REDACTED}; live Prompt Audit only.\`)` — the
  transcript NEVER carries the raw prompt.
- **Fed**: the two `writeChildEvent` sites above.
- **Error surfaced**: `subagent-runner.ts:1559`, `:1591` — `transcriptError: transcriptWriter?.getError()`
  on the result, so a transcript that stopped writing is reported, not silent.
- **Read**: `tui/fleet_transcript.rs` already ports the reader (`read_fleet_transcript`) with a
  `trusted_roots` gate.

## cyrup side — the seams

- **Bounded append**: `crate::jsonl::BoundedJsonlWriter` — `run_paths.rs:108-111` says any writer
  appending a capped `.jsonl` MUST go through it (the 50 MB-default cap is enforced there, not
  per-writer). **Reuse it; do not write a second capped appender.** Upstream's
  `DEFAULT_MAX_CHILD_TRANSCRIPT_BYTES` is the same 50 MiB.
- **Event schema**: `crate::exec::ndjson::SubagentEvent` — the crate's ONE NDJSON schema. The
  transcript records a projection of it (`projectChildSessionEventForJson`), with tool payloads
  truncated at 32 KiB.
- **Foreground feed**: `exec/drive_attempt.rs` around `:372`.
- **Background feed**: the runner's child-event loop under `background/runner_main/`.
- **Path**: `artifacts::artifact_paths(root, run_id, agent, Some(index)).transcript_path`
  (already consumed by `exec/mod.rs:580` and `foreground_transcript.rs:249`).
- **Publishers to flip**: `runner_main/status.rs:603`, `runner_main/executor.rs:1220`.
- **Viewer to switch**: `tui/fleet.rs:1144` — to `paths.transcript_path`, under upstream's
  `transcriptWriter ? … : undefined` gate (`execution.ts:513`).

## Rust shape

- `ChildTranscriptWriter` as a struct over `BoundedJsonlWriter`; `getError` becomes a
  `last_error(&self) -> Option<&TranscriptError>` with a typed error, not a string.
- The record type is a real struct with `#[serde(rename_all = "camelCase")]`, versioned.
- Payload truncation at 32 KiB is a function with a test, not inline arithmetic.
- **The writer is optional-by-construction** (`transcriptWriter?.`) — a run with no artifacts dir
  has no transcript and that is not an error. Model it as `Option<ChildTranscriptWriter>`.

## Definition of done

1. A running child's `_transcript.jsonl` grows WHILE it runs, on both the foreground and the
   background paths, from the parsed child-event stream.
2. The first record is the redacted initial-user-message sentinel, never the prompt.
3. `transcript_path` is published on `RunStatus` (both sites) and the FleetView pane reads it.
4. A writer error surfaces on the result, as upstream's `transcriptError`.
5. **Reachability test**: spawn a real child on the production path, read the transcript file
   MID-RUN (before completion) and assert it has records — then let it finish. A test that reads
   the file only after completion proves nothing about "live". **Mutation: detach the writer from
   the event feed → the mid-run read finds no records → the test fails.**

## Rules

- `[CYRUP-DELTA]` reasons must be TRUE — grep the premise first.
- No `allow(dead_code)`, no stub, no narrowing-and-reporting-done.
- Gates: fmt, clippy `--workspace --all-targets --features test-fixtures -- -D warnings`,
  `nextest run --workspace --features test-fixtures`. Baseline **10 558** / 9 skipped;
  `cyrup-it --features it` **552**.

## [AUG — transcript]

Every citation below was re-read on 2026-09-19: upstream via
`git -C /home/user/cyrup/tmp/pi-subagents show v0.68.0:<path>`, cyrup from the working tree.
Paths under `crates/cyrup-ext-subagents/src/` unless prefixed.

### A. Stale anchors in the seed — corrected

| Seed says | Verified truth |
|---|---|
| `execution.ts:513` is the `transcriptWriter ? … : undefined` gate | `:513` is `shouldNotifyControlEvent`. The gate is `execution.ts:499` (`progress.transcriptPath`), `:1526` (`buildTimeoutRecoverySummary`), result at `:1966-1967`; bg `subagent-runner.ts:1470`, `:1540`, `:1558-1559`, `:1590-1591`. |
| `fleet.rs:1144` should switch "under upstream's gate (`execution.ts:513`)" | Upstream `fleet.ts:649-654` reads `getArtifactPaths(root, runId, agent, index ?? 0).transcriptPath` for foreground-active with **no gate at all**. |
| `runner_main/status.rs:603` and `runner_main/executor.rs:1220` "publish `transcript_path: None`" | Both lines sit inside `#[cfg(test)]` and are `TimeoutRecoveryInput { transcript_path: None }` fixtures (so are `run_status.rs:1126/1166`). They publish nothing. `StepStatus` (`background/records.rs:24-143`) has **no `transcript_path` field at all**; nothing on the async path can carry it today. |
| "cyrup's TWO child-event loops — foreground `drive_attempt.rs:372` and background under `runner_main/`" | The background runner has no parse-and-fold loop of its own for children: `runner_main/executor.rs:961` calls `exec::run_sync`, so the ONE parse in `exec/drive_attempt.rs:303` (`handle_child_line`) drives **both** paths. `runner_main/status.rs:111` re-parses raw lines forwarded through `RunOptions::live_events` (`executor.rs:586-595`) for `status.json` telemetry only. Consequence: one seam feeds both, and a second writer at `status.rs:111` would double-write the same file. |
| `tui/fleet_transcript.rs` Delta 3 premise: "pi's fifth `ArtifactPaths` field `transcriptPath` has no cyrup analogue (`artifacts.rs:58`)" (`fleet_transcript.rs:52-55`) | Stale: `artifacts.rs:82` has `transcript_path`, minted at `:317`. Rewrite that doc when the writer lands. |
| `exec/mod.rs:576-579`: "the bundle's presence is the writer's presence" | False once `include_transcript` gates the writer. Replace with the writer's own path. |
| `spawn/mod.rs:1146` "no `ChildTranscriptWriter` port yet" | True today; becomes stale on landing. Rewrite `:1144-1147` and `:1230-1234` to say the transcript attaches to the parsed stdout stream in `drive_attempt` and that stderr lines stay at `tracing` (see §F scope note). |
| "Per-record `fs.appendFileSync` (`:133`)" | `:133` is the marker append; records append at `:160`. Harmless. |

Verified correct as cited: `child-transcript.ts:6/30/31/43/52/62/102`, `subagent-runner.ts:870-889`,
`execution.ts:1830-1850`, `execution.ts:980`, `run-child-session.ts:406-410`,
`subagent-runner.ts:1559/1591`, `artifacts.rs:82/317`, `run_paths.rs:108-111`,
`drive_attempt.rs:372`, `foreground_transcript.rs:249-256`, `fleet.rs:1144-1159`.

### B. Upstream facts the port rests on (v0.68.0)

- **`projectChildSessionEventForJson`** (`src/runs/shared/child-session.ts:24-30`): rewrites ONLY
  `message_update` (drops `assistantMessageEvent.partial`, keeps `usage`); every other event passes
  through untouched. `writeChildEvent` (`child-transcript.ts:221-248`) records only `message_end`,
  `tool_result_end`, `tool_execution_start`, `tool_execution_end` — never `message_update`. For the
  transcript it is therefore an identity; there is nothing to port.
- **What the 32 KiB cut applies to** (`boundedPayload`, `:9-28`): ONLY tool payloads — the
  `toolResult` message text (`:177`) and `tool_execution_start` args (`:228`, pretty JSON,
  `JSON.stringify(v, null, 2)`). Assistant/user message `text` is **unbounded** (`:197-207`); the
  50 MiB file cap is the only bound on it. The cut: blank → omitted; ≤ 32 768 bytes → verbatim;
  else keep the first `32 768 − 23` bytes (marker `"\n\n… payload truncated"` is 23 bytes: U+2026 is
  3 bytes), back off to a UTF-8 char boundary (`(b & 0xC0) == 0x80` loop), append the marker. A
  non-serialisable value → omitted.
- **Record base** (`:108-121`): `version: 1`, `recordType`, `source: "foreground" | "async"`,
  `runId`, `agent`, `childIndex` (omitted when undefined), `cwd`, `ts` (epoch ms), `timestamp`
  (ISO-8601).
- **Message record** (`:174-208`): `sourceEventType`, `role`; for `toolResult`: `toolCallId`,
  `toolName`, `isError`, `text` (bounded), `outputTruncated`, and a projected `message`
  `{role,toolCallId,toolName,isError,content:[{type:"text",text}] | [],timestamp?}`; otherwise
  `text` (unbounded), `model`, `stopReason`, `errorMessage`, `usage` (normalised
  `{input,output,cacheRead,cacheWrite,cost}` — `:80-94`: `input ?? inputTokens ?? 0`,
  `cost.total ?? cost ?? 0`, finite numbers only) and the FULL `message`.
- **Initial record** (`:212-220`): `sourceEventType:"initial_prompt"`, `role:"user"`, `text`, and
  `message:{role:"user",content:[{type:"text",text}]}`. Both creators pass
  `` `${PROMPT_REDACTED}; live Prompt Audit only.` `` with `PROMPT_REDACTED = "[prompt redacted]"`
  (`src/shared/utils.ts:13`) — the sentinel is `"[prompt redacted]; live Prompt Audit only."`.
- **`tool_start`** (`:226-238`): `toolCallId?`, `toolName`, `argsPreview` (only when args non-empty,
  via `extractToolArgsPreview`), `argsPayload` (bounded pretty JSON). **`tool_end`** (`:239-247`):
  `toolCallId?`, `toolName?`, `isError?` — NO output field.
- **Init** (`:167-172`): `mkdir -p` the parent then `writeFileSync(path, "")` — the file is
  TRUNCATED on creation; failure sets `writeError`, and every later write is a no-op while
  `transcriptPath` is still published and `getError()` surfaces the message.
- **Cap** (`:142-158`): before each record, if `bytes + line > max` OR `bytes + line + markerProbe
  > max`, write the `truncated` marker (`{…base, maxBytes, message:"Child transcript exceeded
  <max> bytes; further records were omitted."}`) if it fits, set `truncated`, and drop everything
  after. `writeRecord` early-returns on `writeError || truncated` (`:143`).
- **Creators**: bg `subagent-runner.ts:870-889` (`source:"async"`, `childIndex: ctx.flatIndex`,
  `cwd: step.cwd ?? ctx.cwd`, gated `ctx.artifactsDir && artifactConfig?.enabled !== false &&
  includeTranscript !== false`); fg `execution.ts:1830-1850` (`source:"foreground"`,
  `childIndex: options.index`).
- **Feeds**: fg `execution.ts:977-980` (`processEvent`, before any lifecycle fold); bg
  `run-child-session.ts:406-410`. The fg stderr per-line reader (`execution.ts:1047-1052`) also  
> **[FIX — corrected 2026-09-19]** the sentence above is FALSE at v0.68.0: the only feeder of `writeStderrLine` is `src/runs/shared/child-hooks.ts:32` (an `onExtensionError` notice), and `execution.ts:1047-1052` is the `tool_execution_start` arm. See `## [FIX — transcript]` T3 and `exec/child_transcript.rs`'s Scope section for the true premise.
  feeds `writeStderrLine`; the bg in-process child has no stderr, so bg transcripts never carry
  `stderr` records.
- **Published**: `progress.transcriptPath` `:499`; `result.transcriptPath/transcriptError`
  `:1966-1967`; bg `StepResult` `:1590-1591`; **status step at declaration** `:1965-1986` via
  `resolveAsyncStepTranscriptPath` (`:1806-1821`: `index = flatStepCount > 1 ? flatIndex :
  undefined`); post-step `:3829-3830` (`singleResult.transcriptPath ?? step.transcriptPath`,
  `transcriptError`); wait/collect `:5063-5064`.
- **Viewer** (`fleet.ts:647-687`): fg-active = minted path, no gate; fg-recent =
  `child.transcriptPath`; async = `step?.transcriptPath ?? sessionFile` (`:671`).

### C. cyrup seams — verified line by line

**The ONE feed (both paths).** `exec/drive_attempt.rs:280-286` `fn handle_child_line(line, state,
progress, control, opts) -> LineAction` (sync). `:303` `parse_line` (the crate's one parse),
`:372` `control.observe_event(&event, …)`, `:379` `progress.record_event(event)` consumes by value.
The transcript write goes at **`:372`, immediately before `control.observe_event`** (any line before
`:379`). Called from `drive_attempt` (`:528-534`, async) at **`:589`** inside a `tokio::select!`
arm. `BoundedJsonlWriter::write_line` is async, so `handle_child_line` becomes `async fn` and `:589`
gains `.await` — the same cost profile as the existing per-line tee await at `spawn/mod.rs:917`.

**Call chain to thread the writer.** `run_sync` (`exec/mod.rs:319`) → `drive_fallback_ladder`
(`:458`; def `:845-877`, already `#[allow(clippy::too_many_arguments)]`, builds
`SpawnedChildAttemptRunner` at `:856-872`) → `run_fallback_ladder` (`fallback.rs:1739` trait) →
`SpawnedChildAttemptRunner::run_attempt` (`exec/attempt_runner.rs:107-141`) → `drive_attempt(child,
&mut progress, self.opts, deadline_sleep, &mut control)` (`:134-141`). One writer per `run_sync`
survives every fallback attempt — upstream's `shared.transcriptWriter` (`execution.ts:1905`).

**Where the bundle is minted.** `exec/mod.rs:515-524`: `artifact_paths(dir, opts.run_id
.map_or("run", …), &agent.name, opts.child_index)`, computed AFTER the ladder but documented as "a
pure derivation over `opts`/`agent`" — hoist it above `:456`. Both callers already hand `run_sync`
the same quadruple their own artifact writers use: bg `runner_main/executor.rs:753-754`
(`run_id: self.run_id.clone(), child_index: Some(ctx.step_slot.index())`) vs
`write_step_input_artifact` `:841-847` (`Some(index)`); fg `extension/executor/foreground.rs:899`
(`child_index: Some(0)`) — which is what the two live readers mint too (`tui/fleet.rs:1150-1155`
`Some(item.index.unwrap_or(0))`, `foreground_transcript.rs:250` `Some(child.index)`). So creating
the writer from `run_sync`'s bundle names exactly the file every transcript reader opens.
(Observation, out of scope: `foreground.rs:1159` mints the `_input.md`/`_output.md` quadruple with
`None`, so a single foreground run's other four files are un-suffixed while its bundle and
transcript are `_0`-suffixed. Pre-existing; do not touch here.)

**The gate has no carrier.** `RunOptions` (`exec/agent_config.rs:396`) has `cwd:397`, `run_id:566`,
`child_index:571`, `artifacts_dir:640` and NO `artifact_config`. `ArtifactConfig::include_transcript`
(`artifacts.rs:110-114`, default `true`) has **zero readers** anywhere in the crate (grep
`include_transcript` → only its definition and `Default`). `PROMPT_REDACTED` / `[prompt redacted]`
does not exist in cyrup. Callers that set `artifacts_dir`: `foreground.rs:952`
(`artifacts_enabled.then(...)`, `artifacts_enabled: art_cfg.enabled` at `:138-139/:340/:772`) and
`executor.rs:792-795` (`self.artifacts_dir.filter(|_| self.artifact_config.enabled)`).

**Substrate.** `jsonl.rs` `BoundedJsonlWriter`: tokio-async; `create`/`create_with_cap` open with
`.create(true).append(true)` (**never truncates**) and seed `bytes_written` from the file's size
(`:97-111`); `write_line(&str) -> io::Result<()>` silently drops the first line that would cross
the cap and every later one (`:127-147`); exposes `is_capped()`, `bytes_written()`, `cap_bytes()`;
`DEFAULT_JSONL_CAP_BYTES = 50 MiB` (`:51`) == upstream `DEFAULT_MAX_CHILD_TRANSCRIPT_BYTES`.
Composition (see §D): the transcript writer does upstream's marker pre-check against
`bytes_written()`/`cap_bytes()` and writes the marker through the same `write_line`; the
substrate's own silent cap remains the backstop, so there is still exactly one byte-budget
enforcer, honouring `run_paths.rs:108-111`. The `_transcript.jsonl` cap is the same 50 MiB.

**Reader contract.** `tui/fleet_transcript.rs:716-855` `parse_transcript_lines`: a record carrying
`recordType` passes through untouched (`:740`); it keys on `recordType`, `ts` (number),
`toolName`, `toolCallId`, `argsPreview`, `argsPayload`, `isError`, `text`, `role`, `model`,
`outputTruncated`, and `message.{role,text,content,toolCallId,toolName,isError}`
(`:752-836`, `:866-873`); `"toolResult" | "tool_result"` both accepted (`:809`). Reusable helpers:
`fleet_transcript.rs:1101` `fn content_text(Option<&Value>) -> Option<String>` (private; the
`extractTextFromContent` analogue) and `exec/tool_call_summary.rs:97`
`pub fn extract_tool_args_preview(&Value) -> String` (the `extractToolArgsPreview` port).

**Viewer/status/result surfaces.**
- `tui/fleet.rs:1136-1203` `transcript_target`: fg-active `:1150-1159` points at `jsonl_path`;
  async `:1176-1200` reads `step.telemetry.output_file` — which **no production code ever sets**
  (`background/telemetry.rs:130` is only read at `fleet_view.rs:1066`), so the async pane is
  `None` for every running child today. Detail lines `:916` (`child.transcript_path`) and `:925`
  (`child.transcript_error`) already exist; `tui/fleet_state.rs:331/341` carry them.
- Foreground history record `extension/executor/foreground_history/record.rs:99/112`;
  `:228-231` derives `transcript_path` from `result.artifact_paths` (ungated); **`:239`
  hard-codes `transcript_error: None`**.
- `StepStatus` `background/records.rs:24-143` (camelCase; optional fields use
  `#[serde(default, skip_serializing_if = "Option::is_none")]`, e.g. `:122-123`); every
  struct-literal site uses `..StepStatus::pending(..)` (e.g. `tracker.rs:754-757`), so a new field
  costs `pending()` (`:148`) only.
- Async publish sites: declaration `runner_main/entry.rs:265-269` (`config.steps … flat_map
  (pending_step_statuses_for)`, `background/flat_index.rs:99-115`; `config` has `run_id:34`,
  `artifacts_dir:350`, `artifact_config:360`); appended steps `runner_main/turn_loop.rs:854-865`;
  post-step `runner_main/status.rs:376` (per-member) and `:411` (single slot) — both
  `entry.session_file = …` lines; `mark_step_running` `:311-317`.
- `StepResult` `spawn/chain_graph.rs` (fields incl. `artifact_paths`, `session_file`; constructors
  `success`/`failure` `:1270-1320`); `build_step_result` `runner_main/executor.rs:988-1070`
  (`:1052 artifact_paths`); `step_result_to_single_result_with` `runner_main/settle.rs:579-618`;
  wait projector `background/wait_completions/project.rs:225-243`.
- `SingleResult` `exec/run_result.rs:25-296`: no transcript fields; `artifact_paths` `:210`; the
  serde discipline to copy is `:199-202`. `pre_spawn_failure` `exec/mod.rs:993-1020`; construction
  `:686-766`.
- Error convention: module-local enums with `Display` + `std::error::Error`
  (`exec/run_fanout_budget.rs:152-169`) or `thiserror` (a dependency, `Cargo.toml:98`). Lints:
  `unwrap_used`/`expect_used`/`indexing_slicing` deny (root `Cargo.toml:101-104`).
- `crate::time::now_epoch_millis() -> i64` (`time.rs:18`); ISO formatter
  `background/run_status.rs:229` `pub(crate) fn format_iso8601_millis(i64) -> String`.

### D. Rust shape

New module **`exec/child_transcript.rs`** (`pub mod child_transcript;` in `exec/mod.rs` beside
`ndjson`), because it consumes `exec::ndjson::SubagentEvent` and is owned by `exec::run_sync`.

```rust
pub const CHILD_TRANSCRIPT_ARTIFACT_VERSION: u32 = 1;               // :30
pub const MAX_TOOL_PAYLOAD_BYTES: usize = 32 * 1024;                 // :6
pub const TOOL_PAYLOAD_TRUNCATION_MARKER: &str = "\n\n… payload truncated"; // :7
pub const DEFAULT_MAX_CHILD_TRANSCRIPT_BYTES: u64 = crate::jsonl::DEFAULT_JSONL_CAP_BYTES; // :31
pub const PROMPT_REDACTED: &str = "[prompt redacted]";               // utils.ts:13
pub const INITIAL_PROMPT_SENTINEL: &str = "[prompt redacted]; live Prompt Audit only.";

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptSource { Foreground, Async }                       // :33

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptRecordType { Message, ToolStart, ToolEnd, Stdout, Stderr, Truncated } // :34

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceEventType { InitialPrompt, MessageEnd, ToolExecutionStart, ToolExecutionEnd }

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MessageRole {
    #[serde(rename = "user")] User,
    #[serde(rename = "assistant")] Assistant,
    #[serde(rename = "toolResult")] ToolResult,   // reader accepts this spelling, :809
}

/// Who/where — the five base fields every record repeats (`:108-121`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptIdentity {
    pub source: TranscriptSource,
    pub run_id: String,
    pub agent: String,
    pub child_index: Option<usize>,   // upstream omits the key when undefined
    pub cwd: PathBuf,
}

/// pi `normalizeUsage` (`:80-94`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptUsage { pub input: u64, pub output: u64, pub cache_read: u64, pub cache_write: u64, pub cost: f64 }

/// One JSONL line. Flat, camelCase, every kind-specific key `skip_serializing_if = Option::is_none`
/// — the exact serde mirror of upstream's per-kind object spreads (`:178-247`). Fields are private;
/// the kind-specific constructors below are the only way to build one, so an impossible
/// combination (a `tool_end` with `usage`) is unconstructible from outside the module.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildTranscriptRecord {
    version: u32, record_type: TranscriptRecordType, source: TranscriptSource,
    run_id: String, agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")] child_index: Option<usize>,
    cwd: PathBuf, ts: i64, timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")] source_event_type: Option<SourceEventType>,
    #[serde(default, skip_serializing_if = "Option::is_none")] role: Option<MessageRole>,
    #[serde(default, skip_serializing_if = "Option::is_none")] text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] stop_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] usage: Option<TranscriptUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")] message: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")] output_truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")] tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] is_error: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")] args_preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] args_payload: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] max_bytes: Option<u64>,
}
// constructors (all take `&TranscriptIdentity` and stamp ts/timestamp via
// `crate::time::now_epoch_millis` + `format_iso8601_millis` — lift the latter to `crate::time`,
// it is duplicated at run_status.rs:229 and missions/mod.rs:159 already):
//   initial_prompt(identity)                                   -> message/user/initial_prompt, text = SENTINEL, message = {role:"user",content:[{type:"text",text}]}
//   message(identity, source_event_type, &Value)               -> the two `writeMessage` arms (:174-208)
//   tool_start(identity, tool_call_id, tool_name, args: &Value) -> :226-238
//   tool_end(identity, tool_call_id, tool_name, is_error)      -> :239-247
//   truncated(identity, max_bytes)                             -> :125-129

/// Typed `writeError` (`:104`); `Display` reproduces upstream's three sentences verbatim
/// (`:137`, `:163`, `:171`) so `transcript_error` reads identically.
#[derive(Debug, thiserror::Error)]
pub enum ChildTranscriptError {
    #[error("Failed to initialize child transcript '{path}': {source}")]
    Initialize { path: PathBuf, #[source] source: std::io::Error },
    #[error("Failed to write child transcript '{path}': {source}")]
    Write { path: PathBuf, #[source] source: std::io::Error },
    #[error("Failed to serialize child transcript record for '{path}': {source}")]
    Serialize { path: PathBuf, #[source] source: serde_json::Error },
}

pub struct ChildTranscriptWriter {
    path: PathBuf,
    identity: TranscriptIdentity,
    /// `None` after a failed initialisation — upstream still returns a writer whose every write is
    /// a no-op while `transcriptPath` stays published and `getError()` names the cause (`:167-172`).
    sink: Option<crate::jsonl::BoundedJsonlWriter>,
    max_bytes: u64,
    truncated: bool,
    last_error: Option<ChildTranscriptError>,
}
impl ChildTranscriptWriter {
    pub async fn create(path: &Path, identity: TranscriptIdentity) -> Self { Self::create_with_cap(path, identity, DEFAULT_MAX_CHILD_TRANSCRIPT_BYTES).await }
    pub async fn create_with_cap(path, identity, max_bytes) -> Self;   // mkdir -p parent; tokio::fs::write(path, "") [truncate, :169]; BoundedJsonlWriter::create_with_cap(path, max_bytes); any Err -> sink None + last_error Initialize
    pub fn path(&self) -> &Path;
    pub async fn write_initial_prompt_sentinel(&mut self);            // no prompt parameter: the type cannot leak it (both upstream callers pass the constant, :889/:1849)
    pub async fn write_child_event(&mut self, event: &SubagentEvent);  // §D.1
    pub fn last_error(&self) -> Option<&ChildTranscriptError>;        // `getError`
    pub fn is_truncated(&self) -> bool;
}
pub fn bounded_payload(value: &serde_json::Value) -> Option<String>;  // :9-28 (string as-is, else to_string_pretty)
pub fn bounded_text(text: &str) -> Option<String>;                    // the string arm, shared by both callers
```

**D.1 `write_child_event` mapping (`SubagentEvent`, `exec/ndjson.rs:91-247`).**
- `MessageEnd { message }` → `record::message(identity, MessageEnd, message)`:
  `role = message.role`; `text = content_text(message.content)`. If role is `toolResult`/
  `tool_result` → `text = bounded_text(text)`, `output_truncated = text.contains(MARKER)`,
  `tool_call_id`/`tool_name`/`is_error` from the message, projected `message` as upstream
  `:186-193`; else `text` unbounded, `model`, `stop_reason`, `error_message`, `usage =
  normalise(message.usage)`, `message = message.clone()`.
- `ToolExecutionStart { tool_call_id, tool_name, args }` → `tool_start` with `args_preview` only
  when `args` is a non-empty object (`extract_tool_args_preview`), `args_payload =
  bounded_payload(args)`.
- `ToolExecutionEnd { tool_call_id, tool_name, is_error, result }` → **two** records:
  `tool_end` (upstream `:239-247`) **and** a `message`/`toolResult` record whose `text =
  bounded_text(content_text(result.content).or(content_text(result)))`, `is_error`, ids —
  `[CYRUP-DELTA]` cyrup's wire carries the tool output inline in `tool_execution_end.result`
  (`ndjson.rs:142-156`; `exec/output.rs:540-543` "that variant plays pi's `role === "toolResult"`
  role"); pi's separate `toolResult` message never exists on this wire, so without this record every
  tool row renders with a status glyph and no output — the exact regression
  `fleet_transcript.rs:935-937` documents and `rewrite_cyrup_record` `:1065-1089` already applies to
  the raw stream. `source_event_type: ToolExecutionEnd` on both.
- Every other variant (incl. `Unknown`, `MessageUpdate`) → no record (upstream `:221-248` falls
  through).

**D.2 `write_record` (`:142-165`).** `if self.last_error.is_some() || self.truncated { return }`;
serialise (`Err` → `Serialize`); `let Some(sink) = self.sink.as_mut() else { return }`;
`line_bytes = line.len() as u64 + 1`; `probe = serialised truncated-marker line bytes` (compute per
call — `ts` width can change); if `sink.bytes_written() + line_bytes > max || … + probe > max` →
`write_truncated_marker().await; return`; `sink.write_line(&line).await` `Err` → `Write`.
`write_truncated_marker`: `truncated = true`; if `bytes_written + marker_bytes > max` → return
(`:131`); write; `Err` → `Write`. The substrate is constructed with `cap_bytes = max_bytes`, so it
can never be reached first, and stays the backstop.

**D.3 `content_text`.** Move `fleet_transcript.rs:1101-1120` to `exec/ndjson.rs` as
`pub fn content_text(value: Option<&Value>) -> Option<String>` (it is an accessor over the wire's
`content` shape, beside `assistant_usage`), and re-point the four `fleet_transcript.rs` call sites.
`tui` → `exec` is the existing dependency direction (`fleet_transcript.rs:1055` already calls
`exec::tool_call_summary`).

**D.4 Carrier on `RunOptions`.** One new field, enum-typed, replacing upstream's two separate
facts (`includeTranscript` + the creator-known `source`):
`pub transcript: Option<TranscriptSource>` — `None` = `includeTranscript === false`;
`Some(source)` = write. The writer exists iff `opts.artifacts_dir.is_some() &&
opts.transcript.is_some()` (upstream's three-term gate `subagent-runner.ts:872-878` /
`execution.ts:1831-1840`, with the first two terms already folded into `artifacts_dir` by both
callers). `[CYRUP-DELTA]` premise (true): `RunOptions` carries no `artifact_config`
(`agent_config.rs:396-640`, grep-verified), and `run_sync` has no other way to learn `source`.

**D.5 Result/status fields (all `#[serde(default, skip_serializing_if = "Option::is_none")]`,
camelCase → `transcriptPath` / `transcriptError`, upstream's own names `subagent-runner.ts:276-277`).**
- `SingleResult` (`exec/run_result.rs`): `transcript_path: Option<PathBuf>`,
  `transcript_error: Option<String>` (stringified `Display` of the typed error at the result
  boundary, exactly as `SingleResult::error: Option<String>` already is; the typed value lives on
  the writer). `Some(path)` iff the writer existed — the gate, not the bundle.
- `StepResult` (`spawn/chain_graph.rs`): same two; `success()`/`failure()` default `None`;
  `build_step_result` (`executor.rs:997-1016` destructure + carry beside `:1052`);
  `step_result_to_single_result_with` (`settle.rs:615-618`) projects them; wait projector
  (`project.rs:236`) reads `transcriptPath`/`transcriptError` from the child JSON (upstream
  `:5063-5064`).
- `StepStatus` (`records.rs`): same two; `pending()` → `None`.

### E. Production call sites (the reachability chain)

1. `exec/mod.rs` `run_sync`: hoist `:515-524` above `:456`; then
   `let mut transcript = match (&artifact_paths, opts.transcript) { (Some(p), Some(source)) =>
   Some(ChildTranscriptWriter::create(&p.transcript_path, TranscriptIdentity { source, run_id,
   agent: agent.name.clone(), child_index: opts.child_index, cwd: opts.cwd.clone() }).await), _ =>
   None };` then `if let Some(w) = transcript.as_mut() { w.write_initial_prompt_sentinel().await }`
   (upstream `:889`/`:1849` — before the first spawn). Pass `&mut transcript` into
   `drive_fallback_ladder` (10th param; the existing `allow` covers it). After the ladder:
   `transcript_path: transcript.as_ref().map(|w| w.path().to_path_buf())`,
   `transcript_error: transcript.as_ref().and_then(|w| w.last_error()).map(ToString::to_string)`;
   `:580-583` `TimeoutRecoveryInput.transcript_path` from the writer (rewrite the `:576-579`
   comment); `pre_spawn_failure` `:993` → `None`/`None`.
2. `exec/attempt_runner.rs:32-63` `SpawnedChildAttemptRunner` gains
   `pub(crate) transcript: &'a mut Option<ChildTranscriptWriter>`; `:134-141` passes
   `self.transcript.as_mut()`.
3. `exec/drive_attempt.rs`: `drive_attempt` (`:528-534`) gains `mut transcript:
   Option<&mut ChildTranscriptWriter>`; `:589` → `handle_child_line(&line, &mut state, progress,
   control, opts, transcript.as_deref_mut()).await`; `handle_child_line` (`:280-286`) becomes
   `async fn` with the extra `transcript: Option<&mut ChildTranscriptWriter>` and, at `:372`:
   `if let Some(writer) = transcript { writer.write_child_event(&event).await; }`.
   **This is the single production feed for both foreground and background.**
4. `extension/executor/foreground.rs`: carry `include_transcript: art_cfg.include_transcript`
   beside `artifacts_enabled` (`:138-139`, `:340`, `:772`); at `:952` add
   `transcript: (artifacts_enabled && include_transcript).then_some(TranscriptSource::Foreground)`.
5. `background/runner_main/executor.rs:792-795`: add `transcript: (self.artifact_config.enabled
   && self.artifact_config.include_transcript).then_some(TranscriptSource::Async)` — first
   production reader of `ArtifactConfig::include_transcript`.
6. Async status publish — declaration: `runner_main/entry.rs:265-269` — after `flat_map`, stamp
   `steps[i].transcript_path = resolve_async_step_transcript_path(&config, &steps[i].agent, i)`
   where the helper (new, in `background/flat_index.rs` beside `pending_step_statuses_for`) returns
   `config.artifacts_dir.as_ref().filter(|_| config.artifact_config.enabled &&
   config.artifact_config.include_transcript).map(|dir| artifact_paths(dir, config.run_id.as_str(),
   agent, Some(flat_index)).transcript_path)` — `[CYRUP-DELTA]` always `Some(flat_index)`, not
   upstream's `flatStepCount > 1 ? flatIndex : undefined` (`:1819`): this hop's `run_sync` bundle
   is minted with `child_index: Some(ctx.step_slot.index())` unconditionally (`executor.rs:754`,
   `:847`), and the status must name the file the writer actually opens. Same stamp in
   `turn_loop.rs:854-865` `append_steps` (appended steps). Post-step: `status.rs:376` and `:411`
   → `entry.transcript_path = outcome.transcript_path.clone().or(entry.transcript_path.take());
   entry.transcript_error = outcome.transcript_error.clone();` (upstream `:3829-3830`).
7. `tui/fleet.rs:1144-1159` → `path: paths.transcript_path` (upstream `fleet.ts:652`, no gate;
   delete the stale comment). `:1176-1200` → `let transcript_path = step.transcript_path.clone()
   .or_else(|| step.session_file.clone())?;` (upstream `:671`); trusted roots unchanged (`:1193-1199`
   already include the run dir and both artifacts roots). Rewrite `fleet_transcript.rs:52-64`.
8. `extension/executor/foreground_history/record.rs:228-231` → `result.transcript_path.clone()`;
   `:239` → `result.transcript_error.clone()` (fills `fleet.rs:925`).
9. `spawn/mod.rs:1144-1147`, `:1230-1234`: rewrite the two "no port yet" comments.
10. `foreground_transcript.rs` (`/subagents status`): unchanged — it already mints the same path.

Struct-literal churn (mechanical `None`s): `RunOptions {` — `exec/testsupport.rs`,
`extension/executor/foreground.rs`, `runner_main/executor.rs`, and 12 files under
`crates/cyrup-it/tests/{subagents,intercom,permission}/` (2 each). `SingleResult {` — ~25 files
(`background/{control,reconcile,records,run_history,watch/*}`, `runner_main/{finish,mod,settle,
status}`, `exec/{mod,external_cli/mod}`, `extension/executor/{foreground_history/record,paths,
workflow_detach/*}`, `tui/{fleet_transcript,intercom}`, 7 cyrup-it files). `StepResult {` —
constructors only (`chain_graph.rs:1270-1320`) plus `tests/dynamic_collect_record_fidelity.rs`.

### F. Scope note (disclosed, not a delta)

Upstream's `writeStdoutLine`/`writeStderrLine`/`writeStderrText` (`:249-259`) — fed only on the
foreground subprocess path from the bounded per-line stderr reader (`execution.ts:1047-1052`) —  
> **[FIX — corrected 2026-09-19]** the sentence above is FALSE at v0.68.0: the only feeder of `writeStderrLine` is `src/runs/shared/child-hooks.ts:32` (an `onExtensionError` notice), and `execution.ts:1047-1052` is the `tool_execution_start` arm. See `## [FIX — transcript]` T3 and `exec/child_transcript.rs`'s Scope section for the true premise.
are not ported here. The DoD names the child-event stream; the stderr pump is a separate task
(`spawn/mod.rs:1173-1191`) and the writer is single-owner on the drive loop, so feeding it would
need `Arc<Mutex<_>>` sharing across tasks. State this in the module doc as a follow-up
(`stderr` records render as red notices, `fleet_transcript.rs:792-800`); do not describe the port
as equivalent on that axis. `TranscriptRecordType::{Stdout, Stderr}` stay in the enum because the
reader dispatches on them and the on-disk vocabulary is upstream's.

### G. Tests

**Workspace tier — `exec/child_transcript.rs` `#[cfg(test)]` (real tempdirs, no child):**
1. `bounded_payload_cuts_at_32_kib_on_a_char_boundary_and_appends_the_marker` — 40 KiB of `é`
   → `len() <= 32_768`, valid UTF-8, ends with the marker, no split code point.
2. `bounded_payload_keeps_small_inputs_and_omits_blank_ones` — exactly 32 768 bytes unchanged;
   `"  \n"` → `None`; a 2-space-pretty object for a non-string.
3. `the_first_record_is_the_redacted_sentinel_and_never_a_prompt` — line 0:
   `recordType=message`, `role=user`, `sourceEventType=initial_prompt`, `text == SENTINEL`,
   `message.content[0].text == SENTINEL`; API has no prompt parameter.
4. `child_events_project_to_pis_record_vocabulary` — feed `MessageEnd` (assistant with
   `model`/`stopReason`/`usage`), `ToolExecutionStart` (args object), `ToolExecutionEnd`
   (`result.content` text, `isError`), then `AgentStart`/`Unknown` → 4 records (sentinel not
   written here), exact key set per record, `usage` normalised, `ts` numeric, `childIndex` absent
   when `None`.
5. `the_production_reader_folds_what_the_writer_wrote` — write via the writer, read with
   `read_fleet_transcript(path, trusted_roots = [dir])` → `Text(assistant …)`,
   `Tool { name: "bash", args: Some(preview), status: Complete, output: Some("done") }`. This is
   the writer/reader vocabulary contract in one process.
6. `records_past_the_cap_end_in_one_truncated_marker` — `create_with_cap(…, 700)`: last line is
   `recordType=truncated` with `maxBytes=700`; `is_truncated()`, `last_error().is_none()`; on-disk
   size ≤ 700; `parse_transcript_lines(..).explicit_truncation`.
7. `an_unwritable_path_surfaces_a_typed_initialize_error` — path beneath a regular file →
   `matches!(last_error(), Some(Initialize{..}))`, `Display` starts with `Failed to initialize child
   transcript '`; later writes are silent no-ops.
8. `create_truncates_a_stale_transcript` — pre-seed junk; after `create` the file is empty
   (upstream `:169`; distinct from `BoundedJsonlWriter::create`'s append semantics).
9. `exec/run_result.rs` / `records.rs` / `chain_graph.rs`: serde round-trips under the wire names
   `transcriptPath`/`transcriptError`; a pre-field `status.json`/ResultFile (keys absent) → `None`.
10. `background/flat_index.rs`: `resolve_async_step_transcript_path` → `None` for `artifacts_dir:
    None`, `enabled: false`, or `include_transcript: false`; else
    `<dir>/<run>_<agent>_<i>_transcript.jsonl`.

**cyrup-it tier — new `crates/cyrup-it/tests/subagents/child_transcript_live_integration.rs`,
registered in `tests/subagents/main.rs` under "child process protocol".** Only cyrup-it can spawn a
child (no in-crate test sets `spawn_command: Some`; `docs/TEST-ARCHITECTURE.md:142/1181`). The
fixture (`src/bin/cyrup_subagent_fixture.rs`, steps `emit` / `sleep_ms`, no barrier step) is
resolved via `crate::support::bins::subagent_fixture()`; one `tempfile::tempdir()` per test, never a
label-derived `/tmp` path (#143).

Script for both tests — the child parks itself mid-conversation:
```json
{"steps":[
 {"kind":"emit","line":"{\"type\":\"agent_start\"}"},
 {"kind":"emit","line":<message_end_line("FIRST: the child is talking")>},
 {"kind":"emit","line":"{\"type\":\"tool_execution_start\",\"toolCallId\":\"call-1\",\"toolName\":\"bash\",\"args\":{\"command\":\"sleep 30\"}}"},
 {"kind":"sleep_ms","ms":30000},
 {"kind":"emit","line":"{\"type\":\"tool_execution_end\",\"toolCallId\":\"call-1\",\"toolName\":\"bash\",\"isError\":false,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"done\"}]}}"},
 {"kind":"emit","line":<message_end_line("SHOULD-NOT-REACH")>},
 {"kind":"emit","line":"{\"type\":\"agent_end\"}"}],
 "exit_code":0}
```

- **A. Foreground, `exec::run_sync` direct** (copy the `RunOptions`/`AgentConfig` literals from
  `exec_run_sync_integration.rs:53-146`): `artifacts_dir: Some(art_dir)`, `run_id:
  Some(RunId::from_token("livetx0001"))`, `child_index: Some(0)`, `transcript:
  Some(TranscriptSource::Foreground)`, `cancel: cancel.clone()`, `spawn_command: Some(fixture)`.
  `let handle = tokio::spawn(run_sync(…))`. Expected path =
  `artifact_paths(&art_dir, "livetx0001", "worker", Some(0)).transcript_path`. Poll every 25 ms
  under a 10 s `tokio::time::timeout` until the file has ≥ 3 lines. Then, **while the run is
  unfinished**: `assert!(!handle.is_finished())`; records `[0]` is the sentinel (and no line
  contains the task text), `[1]` `message/assistant` text `FIRST…`, `[2]` `tool_start/bash` with
  `argsPreview`; no line contains `SHOULD-NOT-REACH`. Then `cancel.cancel()`; `let result =
  handle.await`; `assert_eq!(result.transcript_path, Some(expected))`,
  `assert!(result.transcript_error.is_none())`. **Why provably mid-run:** the drive loop leaves
  only on EOF/terminal stop/cancel (`drive_attempt.rs:586-620`); the terminal events sit behind a
  30 s sleep; the assertions complete inside a 10 s budget with the run future still pending.
  **Mutation:** delete the `write_child_event` line at `drive_attempt.rs:372` → only the sentinel
  ever lands → the ≥ 3-line poll times out → fail. Delete the sentinel call → record 0 is the
  assistant message → fail.
- **B. Background, `runner_main::run_with`** (copy the `RunnerConfig` literal from
  `background_runner_main_integration.rs:1411-1456` with `artifacts_dir: Some(art_dir)`,
  `artifact_config: ArtifactConfig::default()`; `RunnerOverrides { spawn_command: Some(fixture_cmd)
  }`). Spawn in a task. (i) Poll `status.json` (`read_status`, `:176`) until `state == Running` and
  assert `steps[0].transcript_path == Some(<art_dir>/<run>_worker_0_transcript.jsonl)` —
  declaration-time stamp, present before any record. (ii) Poll that file to ≥ 3 lines; assert
  `!handle.is_finished()` and the same three records. (iii) Deliver an `InterruptRequest` to
  `run_paths.control_inbox` (precedent `:1462-1478`), await the run, assert the terminal
  `status.json` `steps[0].transcript_path` is still `Some`, `transcript_error` is `None`, and
  `ResultFile.results[0].transcript_path` is `Some`. Same mutation kills it (the runner feeds the
  same `drive_attempt` seam through `run_sync`).
- **C. Gate:** `transcript: None` (A) / `artifact_config.include_transcript: false` (B) → no
  `_transcript.jsonl` is created; `transcript_path == None`; the other artifact files still land.

Gate commands (disk-tight): `cargo nextest run -p cyrup-ext-subagents --features test-fixtures`;
`CYRUP_IT_BIN_DIR=$(ls -d target/debug/build/cyrup-it-*/out/it-bins/debug | head -1)
env -u AWS_ACCESS_KEY_ID -u AWS_SECRET_ACCESS_KEY cargo nextest run -p cyrup-it --features it -E
'test(child_transcript)'`; then the three seed gates. Baselines: 10 558 / 9 skipped; cyrup-it 552
(+3 new).

### H. `[CYRUP-DELTA]`s to write (premises grep-verified) and things that are NOT deltas

1. `tool_execution_end` → `tool_end` **plus** a `toolResult` message record — premise
   `ndjson.rs:142-156`, `output.rs:540-543`, `fleet_transcript.rs:935-937,1065-1089`.
2. Async status stamps `Some(flat_index)` always — premise `executor.rs:754`, `:847`.
3. `RunOptions::transcript: Option<TranscriptSource>` in place of `includeTranscript` + `source` —
   premise `agent_config.rs:396-640` has no `artifact_config`.
4. `create` truncates (`tokio::fs::write(path, "")`) before wrapping `BoundedJsonlWriter` — premise
   `jsonl.rs:98-103` (`append(true)`, no truncate) vs upstream `:169`.
5. `write_initial_prompt_sentinel()` takes no prompt — premise `:889`/`:1849` both pass the constant.
6. Marker pre-check done by the transcript writer over `bytes_written()`/`cap_bytes()`, substrate
   cap as backstop — premise `jsonl.rs:127-147` drops silently with no marker.
Not deltas (do not tag): serde sorts `argsPayload` object keys where JS keeps insertion order
(cosmetic, mention in a doc line); moving `content_text` to `exec::ndjson`; making
`handle_child_line` async.

### I. Blockers / risks

- None hard. Two soft ones: (a) the `SingleResult {` churn (~25 files) is mechanical but must not
  miss the seven cyrup-it literals or `cargo clippy -p cyrup-it --features it` fails; (b) the
  foreground `_input.md` index mismatch (`foreground.rs:1159` `None` vs bundle `Some(0)`) is
  pre-existing and stays out of scope — do not "fix" it here, the transcript readers agree with
  the bundle.

## [EXEC — transcript]

Executed 2026-09-19, second in the descriptor-first order the seams map set. Every anchor below
is post-`cargo fmt --all`; every upstream citation is @v0.68.0 via
`git -C tmp/pi-subagents show v0.68.0:<path>`. Line numbers the augment cited for
`exec/agent_config.rs`, `runner_main/executor.rs`, `status.rs`, `records.rs`, `fleet.rs`,
`chain_graph.rs` and the cyrup-it literal files were re-read after the descriptor landed, not
pasted from the augment.

### Production call sites (file:line) — the reachability chain

| | file:line | what |
|---|---|---|
| FEED | `crates/cyrup-ext-subagents/src/exec/drive_attempt.rs:381` (`handle_child_line`, now `async fn` :285 with `transcript: Option<&mut ChildTranscriptWriter>` :291) | `writer.write_child_event(&event).await` on every parsed event, BEFORE `control.observe_event`/`record_event` — pi `execution.ts:980` / `run-child-session.ts:410`. THE one feed: the foreground executor and the detached runner both reach it through `run_sync`. |
| DRIVE | `drive_attempt.rs:542-548` (`drive_attempt` gains `mut transcript: Option<&mut ChildTranscriptWriter>`) → `:610` (`transcript.as_deref_mut()` into `handle_child_line(..).await`) | one reborrow per line. |
| ATTEMPT | `crates/cyrup-ext-subagents/src/exec/attempt_runner.rs:68` (`SpawnedChildAttemptRunner::transcript: &'a mut Option<ChildTranscriptWriter>`) → `:146` (`self.transcript.as_mut()` into `drive_attempt`) | pi `shared.transcriptWriter` (`execution.ts:1905`): ONE writer per run, lent to every fallback attempt. |
| CREATE | `crates/cyrup-ext-subagents/src/exec/mod.rs:468-471` (`run_token`), `:472-474` (the artifact bundle, HOISTED above the ladder), `:481-496` (`ChildTranscriptWriter::create(&paths.transcript_path, TranscriptIdentity{..}).await` iff `artifact_paths.is_some() && opts.transcript.is_some()`), `:498` (`write_initial_prompt_sentinel().await` BEFORE the first spawn) | pi `execution.ts:1841-1849` / `subagent-runner.ts:879-889`. |
| LADDER | `exec/mod.rs:517` (`&mut transcript`, 10th arg) → `:887-897` (`drive_fallback_ladder` param `transcript: &'a mut Option<ChildTranscriptWriter>`) | already `allow(too_many_arguments)`. |
| RESULT | `exec/mod.rs:763-769` (`SingleResult { transcript_path: writer.path(), transcript_error: writer.last_error().map(ToString::to_string) }`) · `:611` (`TimeoutRecoveryInput.transcript_path` from the WRITER, the `:576-579` comment rewritten) · `:1060-1061` (`pre_spawn_failure` → `None`/`None`) | pi `execution.ts:1966-1967`, `:1512`. |
| CARRIER | `crates/cyrup-ext-subagents/src/exec/agent_config.rs:663` (`RunOptions::transcript: Option<TranscriptSource>`, after `artifacts_dir` :650) | `[CYRUP-DELTA]` premise re-verified in the post-descriptor tree: `grep artifact_config exec/agent_config.rs` → 0 hits (`RunOptions` :406-767). |
| PRODUCER fg | `crates/cyrup-ext-subagents/src/extension/executor/foreground.rs:143` (`ForegroundRunOptionsInput::include_transcript`), `:345` (`art_cfg.include_transcript`), `:778` (destructure), `:962-963` (`transcript: (artifacts_enabled && include_transcript).then_some(TranscriptSource::Foreground)`) | pi `execution.ts:1831-1840`. |
| PRODUCER bg | `crates/cyrup-ext-subagents/src/background/runner_main/executor.rs:800-801` (`transcript: (self.artifact_config.enabled && self.artifact_config.include_transcript).then_some(TranscriptSource::Async)`) | pi `subagent-runner.ts:872-878`; the FIRST production reader of `ArtifactConfig::include_transcript` (grep before this diff: definition + `Default` only). A revived run is an ordinary producer here: the descriptor hands `artifacts_dir`/`artifact_config` back (control.rs:497-499), and this gate reads them unchanged. |
| STEP RESULT | `runner_main/executor.rs:1021-1022` (`build_step_result` destructures `transcript_path, transcript_error`) → `:1066-1067` (carried onto `StepResult`) | pi `subagent-runner.ts:1590-1591`. |
| STATUS decl | `crates/cyrup-ext-subagents/src/background/runner_main/entry.rs:270-281` (every declared `StepStatus` stamped via `resolve_async_step_transcript_path(config, &step.agent, flat_index)`, BEFORE the first dispatch) · `background/runner_main/turn_loop.rs:859-884` (`append_steps` gains `config: &RunnerConfig`, stamps appended entries at `base + offset`) · `:707` (`append_steps(io.config, ..)`) | pi `subagent-runner.ts:1965-1986`. |
| RESOLVER | `crates/cyrup-ext-subagents/src/background/flat_index.rs:131-143` (`resolve_async_step_transcript_path(&RunnerConfig, agent, flat_index) -> Option<PathBuf>`) | pi `resolveAsyncStepTranscriptPath` `:1806-1821`; `[CYRUP-DELTA]` always `Some(flat_index)` — premise re-verified: `runner_main/executor.rs` `child_index: Some(ctx.step_slot.index())` (the `RunOptions` literal) and `write_step_input_artifact(.., Some(index))`. |
| STATUS post | `crates/cyrup-ext-subagents/src/background/runner_main/status.rs:387-390` (per-member arm) and `:428-431` (single-slot arm): `if let Some(path) = outcome.transcript_path.clone() { entry.transcript_path = Some(path) }; entry.transcript_error = ..` | pi `:3829-3830` (`singleResult.transcriptPath ?? step.transcriptPath`). |
| RESULT FILE | `crates/cyrup-ext-subagents/src/background/runner_main/settle.rs:628-629` (`step_result_to_single_result_with` projects both) | onto the terminal `ResultFile`; the wait projector (`wait_completions/project.rs`) is UNTOUCHED — see decision 6. |
| RECORDS | `crates/cyrup-ext-subagents/src/background/records.rs:72,76` (`StepStatus::{transcript_path, transcript_error}`, camelCase `transcriptPath`/`transcriptError`, `default` + `skip_serializing_if`), `:172-173` (`pending()`) · `crates/cyrup-ext-subagents/src/exec/run_result.rs:242,249` (`SingleResult`, same serde) · `crates/cyrup-ext-subagents/src/spawn/chain_graph.rs:1267,1271` (`StepResult`), constructors `:1281`/`:1311` default `None` | |
| VIEWER | `crates/cyrup-ext-subagents/src/tui/fleet.rs:1156` (foreground-active → `paths.transcript_path`, the stale delta comment deleted) · `:1186-1191` (async → `step.transcript_path` else `step.session_file` else `run.status.session_file`; `telemetry.output_file`, which nothing sets, is no longer read) | pi `fleet.ts:649-654`, `:669-671`. |
| HISTORY | `crates/cyrup-ext-subagents/src/extension/executor/foreground_history/record.rs:232` (`result.transcript_path`), `:240` (`result.transcript_error`, was hard-coded `None`) | fills `fleet.rs:916/925`. |
| PROJECTIONS | `extension/executor/workflow_detach/children.rs:345-346`, `background/reconcile.rs:685-686` | the two `StepStatus → SingleResult` synthesizers carry the step's own stamps, as they already do for `session_file`. |
| MODULE | `crates/cyrup-ext-subagents/src/exec/child_transcript.rs` (new; `exec/mod.rs:46`) | consts `:87-101` · `TranscriptSource` :106 · `TranscriptRecordType` :116 · `SourceEventType` :135 · `MessageRole` :152 · `TranscriptIdentity` :181 · `TranscriptUsage` :197 (`normalize` :210) · `ChildTranscriptRecord` :242 with constructors `initial_prompt` :320 / `message` :335 / `tool_result` :371 / `tool_start` :420 / `tool_end` :445 / `truncated` :461 · `bounded_payload` :486 · `bounded_text` :497 · `ChildTranscriptError` :516 · `ChildTranscriptWriter` :550 (`create` :564, `create_with_cap` :572, `path` :607, `last_error` :613, `is_truncated` :619, `write_initial_prompt_sentinel` :629, `write_child_event` :643, `write_record` :702, `write_truncated_marker` :756). |
| LIFTS | `crates/cyrup-ext-subagents/src/time.rs:47` (`format_iso8601_millis`, from `run_status.rs`; `background/run_status.rs:227` re-exports so 12 callers keep their path) · `crates/cyrup-ext-subagents/src/exec/ndjson.rs:338` (`content_text`, from `fleet_transcript.rs`; its 4 call sites re-pointed) | |
| COMMENTS | `spawn/mod.rs` (the two "no `ChildTranscriptWriter` port yet" comments) · `tui/fleet_transcript.rs` module-doc delta 3 (stale "has no cyrup analogue" premise) · `exec/mod.rs` `TimeoutRecoveryInput` comment | rewritten. |

### Reachability tests

`crates/cyrup-it/tests/subagents/child_transcript_live_integration.rs` (new; registered at
`crates/cyrup-it/tests/subagents/main.rs:77` under "child process protocol"; the "all 35 files"
header is untouched). Real `cyrup-subagent-fixture` child per test, per-test `tempfile::tempdir()`,
fixture named through `RunOptions::spawn_command` / `RunnerOverrides::spawn_command` — no env
mutation, no `tests/support/` helper added.

- `:482` **A. foreground, `exec::run_sync` direct** — `artifacts_dir: Some`, `run_id`
  `livetx0001`, `child_index: Some(0)`, `transcript: Some(Foreground)`. The child emits
  `agent_start`, an assistant `message_end` (`stopReason: "toolUse"`), `tool_execution_start(bash)`,
  then `sleep_ms 30000`. The run future is `pin!`ed and raced under `tokio::select! { biased; }`
  with the RUN ARM FIRST against a 25 ms poll of `artifact_paths(dir,"livetx0001","worker",Some(0))
  .transcript_path` for ≥ 3 lines (10 s budget): a run that had settled wins the race and panics.
  Mid-run assertions: record 0 is the sentinel (`initial_prompt`/`user`/`INITIAL_PROMPT_SENTINEL`,
  `source: foreground`, `runId`, `agent`, `childIndex: 0`), record 1 `message/assistant "FIRST…"`,
  record 2 `tool_start/bash argsPreview "sleep 30"`, no line carries the task text or
  `SHOULD-NOT-REACH`. Then `cancel.cancel()`, await, `result.transcript_path == Some(expected)`,
  `transcript_error.is_none()`, post-sleep line still absent on disk.
- `:540` **B. background, `runner_main::run_with`** — `RunnerConfig` copied from
  `background_runner_main_integration.rs` with `artifacts_dir: Some`, `ArtifactConfig::default()`.
  (i) race the run against a poll of `status.json` until `Running`: `steps[0].transcript_path ==
  Some(<art>/livetx0002_worker_0_transcript.jsonl)` on the FIRST `Running` status (the
  declaration stamp, written before dispatch); (ii) race it against the ≥ 3-line poll, same three
  records with `source: async`; (iii) `InterruptRequest` into `control_inbox`, await (< 20 s, i.e.
  torn down rather than waited out), terminal `status.json` `Paused` with `steps[0].transcript_path`
  still `Some(expected)`, `transcript_error` `None`, and `ResultFile.results[0].transcript_path ==
  Some(expected)`, `interrupted`.
- `:642` / `:675` **C. the gate off** — foreground `transcript: None` and background
  `include_transcript: false`: `transcript_path == None` on the result / status / ResultFile, no
  `_transcript.jsonl` on disk, the bundle still minted (A) and `_input.md` still written (B).

Why "mid-run" is a proof, not a hope: the fixture's first turn ends `"toolUse"`, so the drive
loop's 1 s final-stop grace drain is never armed (a first draft used `"stop"`, and mutation 1
then failed at 1.1 s via `forcedDrainAfterFinalSuccess` instead of the poll timeout — the test
doc was corrected rather than left claiming nothing could end the run); the loop then leaves only
on EOF / terminal stop / cancel / interrupt, none possible during the 30 s sleep; and the passing
runs read their three lines in ~40 ms with the run arm of a biased select still pending.

### Mutations run (both observed, both restored)

1. `drive_attempt.rs:380-382` `if let Some(writer) = transcript { writer.write_child_event(&event).await; }`
   → `let _ = transcript;`. Result: A and B FAIL after 10.17 s / 10.19 s — "the transcript at
   …_transcript.jsonl never reached 3 lines within 10s — the writer is not being fed while the
   child runs"; C ×2 still pass (they assert absence). Restored from a scratchpad copy; `grep -c`
   of the call line = 1.
2. `exec/mod.rs:497-499` `writer.write_initial_prompt_sentinel().await` deleted. Result: A and B
   FAIL — only two records (assistant, tool_start) land before the sleep, so the same ≥ 3-line
   poll times out (were the file read, record 0 would be the assistant message). Restored;
   `grep -c` = 1. Post-restore: 4/4 pass.

Workspace-tier tests (`exec/child_transcript.rs` `mod tests` :775+, real tempdirs): bounded
payload 32 KiB char-boundary + marker (`é` × 20 Ki → 32 745 bytes head + 23-byte marker),
small/blank/pretty-JSON arms, sentinel-first, event → exact per-record key sets (`usage`
normalised, `childIndex` absent when `None`, `ts` numeric), writer → `read_fleet_transcript`
round-trip (assistant text + model, ONE `bash` tool row `Complete` with `output: "done"`),
1 KiB cap → single `truncated` marker with `maxBytes` + `explicit_truncation`, unwritable path →
typed `Initialize` error whose `Display` is upstream's sentence and whose later writes are no-ops,
`create` truncates a stale file, `MessageRole` wire spellings. `background/flat_index.rs` tests:
the stamp names `<dir>/<run>_<agent>_<i>_transcript.jsonl` (always suffixed) and is `None` for
each gate term off. `background/records.rs`: `transcriptPath`/`transcriptError` round-trips on
`StepStatus` and `SingleResult`, keys omitted while `None`, pre-field JSON → `None`.
`tui/fleet.rs`: async target reads `step.transcript_path`, falls back to `session_file`, never
`telemetry.output_file`.

### Rust-shape decisions

1. `RunOptions::transcript: Option<TranscriptSource>` — one enum-typed field for upstream's
   `includeTranscript` switch + creator-known `source` (`[CYRUP-DELTA]`, premise grep-verified
   post-descriptor). The writer exists iff `artifacts_dir.is_some() && transcript.is_some()`:
   upstream's first two gate terms are already folded into `artifacts_dir` by both producers.
2. `ChildTranscriptRecord` is one flat camelCase struct with PRIVATE fields and per-kind
   constructors — the serde mirror of upstream's per-kind object spreads; an impossible
   combination is unconstructible from outside the module. `MessageRole` names the three roles
   the reader dispatches on and carries any other wire role verbatim (`#[serde(untagged)]
   Other(String)`) because cyrup's `AgentMessage` also emits `custom`/`bashExecution`/… and
   upstream writes `role: message.role` for any role rather than dropping the record.
3. `ChildTranscriptError` is `thiserror` with upstream's three `Display` sentences verbatim;
   stringified ONLY at the result boundary (`SingleResult::transcript_error`), exactly like
   `SingleResult::error`. No `SubagentError` variant (the descriptor added its own; this one is
   not a run failure by design).
4. `write_initial_prompt_sentinel()` takes no prompt (`[CYRUP-DELTA]`, both upstream creators
   pass the constant); `create` truncates before wrapping `BoundedJsonlWriter` (`[CYRUP-DELTA]`,
   the substrate appends); the marker pre-check is done here over `bytes_written()`/`cap_bytes()`
   with the substrate as backstop (`[CYRUP-DELTA]`, the substrate drops silently). All three
   premises grep-verified against `jsonl.rs`.
5. `tool_execution_end` → `tool_end` PLUS a `toolResult` message record (`[CYRUP-DELTA]`; cyrup's
   wire carries the output inline in `result`, `ndjson.rs`/`output.rs`/`fleet_transcript.rs`
   `rewrite_cyrup_record` already correct for the raw stream). `content_text` moved to
   `exec::ndjson` so writer and reader share one definition of "the text".
6. `WaitCompletionChild` is UNTOUCHED, against the augment's §E/§D.5 note: upstream's
   `WaitCompletionChild` (`shared/types.ts:1338-1360`) carries no `transcriptPath`/
   `transcriptError`; the `:5063-5064` copy is onto the runner's results array, which cyrup
   already carries through `SingleResult`'s own serde inside `ResultFile`. Adding fields pi does
   not have would be an invention.
7. `resolve_async_step_transcript_path` takes `&RunnerConfig` (the two call sites already hold
   it) rather than four primitives; `append_steps` gains the config so appended entries are
   stamped at their real flat index. The seams map's "NEITHER adds a `RunnerConfig` field" holds:
   the resolver READS `artifacts_dir`/`artifact_config`/`run_id`.
8. `format_iso8601_millis` lifted to `crate::time` (the clock module) with a `pub(crate) use`
   re-export in `run_status.rs`, so no caller moved.
9. The `stopReason: "stop"` lesson (above) is recorded in the test file's docs, not hidden.

### Struct-literal churn (mechanical, compiler-verified)

`RunOptions {`: src 3 (`testsupport.rs`, `foreground.rs`, `runner_main/executor.rs`), cyrup-it 12
(`intercom/child_bridge_activation.rs`, `permission/forwarding_spawn_env.rs`, `subagents/` ×10)
— one `transcript` line each, nothing else in those literals touched. `SingleResult {`: 20 src
literals gained `transcript_path`/`transcript_error` (`None` except the two projections above and
`settle.rs`); the seams map's "7 cyrup-it `SingleResult {` literals" were all `-> SingleResult {`
return types (0 literals, 0 edits). `StepResult {`: 4 literals (`chain_graph.rs` aggregate,
`turn_loop.rs` imported root, `settle.rs` ×2) + the two constructors. `StepStatus {`:
`registration/cost.rs` full literal (the one site without `..pending()`, as the handoff warned).

### Not done / disclosed

- Upstream's stdout/stderr transcript records (`child-transcript.ts:249-259`) are not ported.
  Their ONE feeder at v0.68.0 is `src/runs/shared/child-hooks.ts:30-34` — an `onExtensionError`
  hook on the in-process child session writing one `stderr` record per contained extension
  fault, on both paths (`execution.ts:1381`, `run-child-session.ts:641`); `writeStdoutLine`/
  `writeStderrText` have no callers and no path records the child's stderr stream. cyrup has no
  such event on the parsed wire (`SubagentEvent` has no extension-error variant; the `--mode
  json` child prints the fault to its stderr via `cyrup_modes::print::extension_error_sink`),
  so nothing can feed the writer — the notice is visible in the stderr tail instead. Stated in
  the module doc ("Scope") and in the two `spawn/mod.rs` comments — corrected in
  `[FIX — transcript]` (the original text here claimed a foreground per-line stderr feed at
  `execution.ts:1047-1052` that does not exist at the pinned tag);
  `TranscriptRecordType::{Stdout, Stderr}` stay because the reader renders them.
- `foreground.rs` still mints the `_input/_output/_meta` quadruple with index `None` while
  `run_sync`'s bundle and both transcript readers use `Some(0)` — pre-existing, out of scope, not
  touched (the transcript readers agree with the bundle, which is what matters here).
- Members of a `DynamicGroup` still share one flat slot (SUBA-093 residual), so their transcripts
  collide on `<run>_<agent>_<slot>_transcript.jsonl` exactly as their `_input.md` already do; the
  status stamp for that slot names the `<dynamic:…>` placeholder's path. Pre-existing layout, not
  widened.
- `cargo doc` was not run (disk); intra-doc links were checked by hand against existing paths.

### Gates

Run in this order after the descriptor landed (all post-`cargo fmt --all`, disk-tight: no `cargo doc`):

- `cargo fmt --all -- --check` → clean.
- `cargo clippy --workspace --all-targets --features test-fixtures -- -D warnings` → exit 0, no warnings.
- `cargo nextest run --workspace --features test-fixtures` →
  `Summary [  97.397s] 10592 tests run: 10592 passed, 9 skipped`
  (seed baseline 10 558 / 9 skipped was PRE-descriptor; this track adds 14 in-crate tests —
  `exec/child_transcript.rs` ×9, `background/flat_index.rs` ×2, `background/records.rs` ×2,
  `tui/fleet.rs` ×1 — and moves `iso8601_formats_a_known_epoch` from `run_status.rs` to `time.rs`).
- `CYRUP_IT_BIN_DIR=target/debug/build/cyrup-it-87b505e3149e7da5/out/it-bins/debug`, `AWS_ACCESS_KEY_ID`/
  `AWS_SECRET_ACCESS_KEY` unset: `cargo nextest run -p cyrup-it --features it` →
  `Summary [ 303.234s] 556 tests run: 556 passed, 0 skipped` (552 → 556: the four
  `child_transcript_live_integration` tests).
- Mutation runs (same cyrup-it filter `-E 'test(child_transcript)'`): baseline 4/4 · mutation 1
  `2 passed, 2 failed` (A, B at 10.17 s / 10.19 s, poll timeout) · mutation 2 `2 passed, 2 failed`
  (same shape) · post-restore 4/4.

## [FIX — transcript]

Remediation of the six transcript-track findings, 2026-09-19. Upstream re-read only via
`git -C tmp/pi-subagents show v0.68.0:<path>` / `git -C tmp/pi-subagents grep … v0.68.0`.

| | finding | what changed | status |
|---|---|---|---|
| T1 | foreground producer gate (`foreground.rs:962-963`) unpinned | `crates/cyrup-it/tests/subagents/child_transcript_live_integration.rs` §D `the_production_foreground_run_writes_a_transcript_labelled_foreground`: drives `SubagentExecutor::run_foreground` (the `subagent` tool's path, via `SubagentsExtension::with_config_and_cwd` + a project persona), asserts `result.transcript_path == artifact_paths(project_artifacts_dir(cwd), run, "worker", Some(0)).transcript_path`, the file exists, record 0 is the sentinel with `source: "foreground"`, `runId`, `agent`, `childIndex: 0`, and the child's message follows. | test PASSES; gate mutation (`.then_some` → `None`) NOT observed — see "Not done". |
| T2 | `source` label derived from the executor KIND (`runner_main/executor.rs` `run_single` → `build_step_run_options` stamped `Async` unconditionally) | CONFIRMED from the call graph: `extension/executor/chain.rs:138` builds `ExecSingleStepExecutor::foreground(..)` for a foreground `/chain`//`/parallel` walk, whose `run_single` is the same method. Fix: `ExecSingleStepExecutor::transcript_source: TranscriptSource` (field, doc names the true premise: pi's two creators `subagent-runner.ts:881` / `execution.ts:1843` each live in one process, cyrup has ONE executor for both), set `Foreground` in `::foreground()`, `Async` in `turn_loop.rs`'s runner literal and the three test literals (`executor.rs` ×2, `control_watcher.rs`); the gate reads `self.transcript_source`. Pinned by `executor::tests::the_transcript_source_follows_the_executor_s_origin_not_the_dispatch_method` at the `RunOptions` seam — the last point the label is observable for a foreground walk, whose executor carries `artifacts_dir: None` (SUBA-N03) and so opens no writer (upstream v0.68.0 has no foreground chain writer either: `createChildTranscriptWriter` has exactly two callers). | test PASSES; mutation (`Foreground` → `Async` in `::foreground()`) NOT observed. |
| T3 | stderr disclosure's premise false | Established: `git grep` at v0.68.0 → ONE feeder, `src/runs/shared/child-hooks.ts:30-34` `withChildSessionErrorReporting` (an `onExtensionError` hook on the in-process child session writing `Extension error (<path>, <event>): <msg>` as a `stderr` record), wired on BOTH paths through `child-launch.ts:150` from `execution.ts:1381` and `run-child-session.ts:641`; `writeStdoutLine`/`writeStderrText` have no callers; no upstream path records the child's stderr stream. Rewrote all four notes: `exec/child_transcript.rs` "Scope", `spawn/mod.rs` (`CapturedStderr` doc and `drain_stderr_lines` doc), and the "Not done" entry above. NOT ported, because cyrup has no such EVENT on the parsed wire: the child runs `--mode json` (`exec/spawn_plan.rs:399-400`), whose extension faults go to its stderr via `cyrup_modes::print::extension_error_sink` (`eprintln!`, bound in `cyrup_modes::json::bind_and_subscribe`), and `SubagentEvent` (`exec/ndjson.rs`) has no extension-error variant — the notice reaches the parent as stderr bytes on the `CapturedStderr` pump (traced per line, kept in the failure tail). Matching the notice text on that stream would record a heuristic, not the hook. | done. |
| T4 | 32 KiB cut unpinned at `:380`/`:438`; 50 MiB default unpinned through `create` | `child_transcript::tests::tool_payloads_are_cut_where_the_writer_applies_them_not_only_in_the_helper` (40 KiB tool result → `text.len() == 32768`, marker suffix, `outputTruncated: true`, same text in the projected `message`; 40 KiB args → `argsPayload ≤ 32768` + marker; 40 KiB assistant text stays 40960) and `create_caps_the_sink_at_the_50_mib_default` (`DEFAULT_MAX_CHILD_TRANSCRIPT_BYTES == 50 MiB`; `create()` → `max_bytes` and the sink's `cap_bytes()` both equal it). | tests PASS; the four mutations NOT observed. |
| T5 | DoD #4 unproven (`transcript_error` never `Some` on a production path) | §E `a_writer_error_surfaces_on_the_foreground_result_and_the_run_still_completes` (production `run_foreground`, `<cwd>/.cyrup-subagents/artifacts` pre-created as a regular FILE → `exit_code == 0`, output delivered, `transcript_error` starts with `Failed to initialize child transcript '`, `transcript_path` still published, no file) and `a_writer_error_surfaces_on_the_background_status_and_result_file` (`run_with`, artifacts dir beneath a file → `status.json` `Complete`, `steps[0].transcript_error` Some with the same sentence, `steps[0].transcript_path` still the declaration stamp, `ResultFile.results[0].transcript_error` identical, `exit_code == 0`). | tests PASS; the four hop mutations NOT observed. |
| T6 | `cyrup-it/tests/subagents/main.rs` header "all 35 files" | 37 registered (35 drained by the migration at e4d5e63, minus `verify_redaction_inherited_env` (removed in 3f9380f), plus `watchdog_model_turn_integration`, `watchdog_permission_arbiter_integration`, `child_transcript_live_integration`). Header rewritten to say exactly that; line 9's "these 35" is the migration set and stays. | done. |

### Runs

- `cargo fmt --all` → clean.
- `cargo nextest run -p cyrup-ext-subagents --features test-fixtures -E 'test(child_transcript) | test(transcript_source) | test(runner_main::executor)'` → 17/17 (the 2 new `child_transcript` tests + the new `executor` test included).
- cyrup-it, `CYRUP_IT_BIN_DIR=/home/user/cyrup/target/debug/build/cyrup-it-87b505e3149e7da5/out/it-bins/debug` (ABSOLUTE — nextest runs the binary from `crates/cyrup-it`, a relative dir spawns nothing: "spawn failed: No such file or directory"), AWS keys unset, `-E 'test(child_transcript)'` → 7/7 (A, B, C×2, D, E×2).

### Not done — disclosed

- **Mutations**: the harness `scratchpad/mutate_fix.py` (ten mutations: T1 gate→`None`; T2 `Foreground`→`Async`; T4 `text.and_then(bounded_text)`→unbounded, `bounded_payload(&args)`→raw pretty JSON, `create`→`create_with_cap(.., u64::MAX)`, const→64 MiB; T5 `run_sync`'s `transcript_error:`→`None`, `step_result.transcript_error = …` dropped, `record_step_outcome`'s single-slot `entry.transcript_error = …` dropped, `settle.rs` projection→`None`; each apply→run→restore with sha check) was started and interrupted during M1's rebuild by the structured-output enforcement before any result landed. It was stopped with SIGINT (its `finally` restores) and the tree verified: `git diff --stat` on `foreground.rs`, `exec/mod.rs`, `status.rs`, `settle.rs` is empty (identical to HEAD), every original line present once, no mutation text present. NO mutation result is claimed. Re-run: `python3 scratchpad/mutate_fix.py` (optionally with prefixes, e.g. `M2 M3`).
- **Gates not run**: `cargo clippy --workspace --all-targets --features test-fixtures -- -D warnings`, `cargo nextest run --workspace --features test-fixtures` (baseline 10592 / 9 skipped; this track adds 3 in-crate tests), and the full `cyrup-it --features it` (baseline 556; this track adds 3 → 559 expected). Only the filtered runs above were executed.
- Hygiene: no `#[allow(dead_code)]`, no stub, no `todo!()`; no git write commands were run; no `cargo doc`.
