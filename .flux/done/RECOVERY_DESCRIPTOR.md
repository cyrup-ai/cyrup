---
stage: qa
status: completed
updated: 2026-09-19
---

# Recovery descriptor — a resumed async run is still the run you launched

OBJECTIVE: persist the resolved launch contract of an async run at launch, and thread it back
through resume — so a revived child keeps its model, tools, budgets, skills, prompt, output
contract and capability ceiling instead of degrading to a bare agent name.

## What is broken — verified in the tree, not from the ledger

`crates/cyrup-ext-subagents/src/extension/executor/control.rs:373`, `revive_from_transcript`:

```rust
let step = SingleStepSpec {
    skills: None, session_dir: None, agent: agent.clone(), task: revived_task, cwd: None,
    model: None, tools: None, extensions: None, session_file: Some(…),
    max_depth_override: None, structured_output_schema: None, output: None, output_path: None,
    output_mode: None, reads: None, acceptance: None, context: Some(ContextMode::Fork),
    agent_scope: None,
};
// …and on BackgroundStepsSpec: usage_budget: None, turn_budget: None, permission_rules: None
```

**Every per-call field is `None`.** A revived run carries the agent name, the task and the session
file. Two of the lost fields (`tools`/`excludeTools`, `maxSubagentDepth`) are capability
constraints, so the fallback WIDENS: a child launched with a narrowed tool set resumes with the
agent file's full default set. `permission_rules: None` means the child-side permission gate
(UW-6, closed) receives no policy on resume.

**PR #142 made this reachable programmatically**: the RPC bridge exposes `resume`, so any host or
sibling extension can now drive this path — not only a human typing a verb.

## Why: the reader exists, the writer does not — the same shape as the handoff manifest was

`background/async_retention/scan.rs:378`:
```
/// [CYRUP-DELTA] no cyrup writer produces `recovery-descriptor.json` today
```
`RECOVERY_DESCRIPTOR_FILE` at `:53`, read at `:392` by `has_resumable_contract` — which therefore
can never fire, so retention decides blind. **This is the third reader-without-writer in this
crate.** The first (handoff manifest) closed in PR #143. The other (child transcript) is the sibling
task running alongside this one.

## The ledger badly understates the gap

Ranked row 9 says the gain is *"a revived async run keeps its per-call `model`/`tools`/`toolBudget`"*.
Upstream's `SteeringRecoveryDescriptor` (`src/shared/types.ts:805` @v0.68.0) has **58 fields**.
It is the launch contract, not three fields.

## Upstream, pinned at v0.68.0

Read ONLY via `git -C /home/user/cyrup/tmp/pi-subagents show v0.68.0:<path>`.

- **Type**: `src/shared/types.ts:805` `SteeringRecoveryDescriptor` (58 fields).
- **Write**: `src/runs/background/async-execution.ts:1993-2051` — the descriptor is built from the
  RESOLVED launch (`recoveryAgentConfig = params.recoveryAgentConfig ?? agentConfig`, `:1992`) and
  written with `writePrivateAtomicJson(path.join(asyncDir, "recovery-descriptor.json"), …)` at
  `:2051`, gated `if (!externalRunner)`. A write failure FAILS THE LAUNCH (`:2053`) — a run that
  cannot be recovered is refused up front, not discovered later.
- **Read on resume**: `src/runs/background/async-resume.ts:312` (`descriptorPath`), consumed through
  the revive target at `:565-595` (`recoveryDescriptor?.agent`, `?.cwd`, `?.thinkingCeiling`,
  `?.launchContractDigest`, and the agent-mismatch refusal at `:566`).
- **Read by retention**: `src/runs/background/async-retention.ts:194` (`hasResumableContract`).
- **Read by retained-children**: `retained-children.ts:68` (`children.list` — one of the 12 verbs
  still missing; this task is its prerequisite, not its owner).

## cyrup side — the seams

- **Write site**: `extension/executor/background.rs:619` builds
  `crate::background::runner_main::RunnerConfig { … }` from the resolved launch. `RunnerConfig`
  (`background/runner_main/config.rs:34-65`) already carries `run_id`, `mode`, `steps`, `cwd`,
  `session_file`, `session_id`, plus `turn_budget` (SUBA-008) and `permission_rules` (SUBA-073) —
  i.e. much of the contract ALREADY crosses to the runner. The descriptor is the durable
  projection of that same resolved state.
- **Read site**: `extension/executor/control.rs:373` (above). `SingleStepSpec` and
  `BackgroundStepsSpec` are the two structs whose `None`s must become the descriptor's values.
- **Retention**: `background/async_retention/scan.rs:53`, `:378`, `:392`.
- **Run dir**: `background/run_paths.rs` — `RunDir` has `status()`, `events()`, and (from #143)
  `handoff()`. The descriptor path belongs in this family.
- **Atomic write**: `background/atomic.rs:75` `write_atomic_json`. Upstream's
  `writePrivateAtomicJson` is the 0600 variant — check whether cyrup has one; the descriptor
  carries a system prompt and paths, so private is the right mode.
- **Digest**: `workflows/stable_json.rs:54` `stable_json_digest` for `launchContractDigest`.

## Rust shape — port the behaviour, not the spelling

The user's standing directive: *"we ship features … adapted to Rust best practices … pi names can
change … it's the features that matter."*

- The descriptor is a **struct with real types**, not `serde_json::Value`: `RunMode`, the existing
  `ContextMode`, `OutputMode`, `WorkflowLaneMetadata` (landed in #143), the thinking-level type,
  `PathBuf`s. `#[serde(rename_all = "camelCase", deny_unknown_fields)]`, as the crate does.
- `Option` ONLY where upstream genuinely elides the field. Upstream spreads
  `...(x ? { x } : {})` for optionals and writes `version`, `sourceRunId`, `agent`, `cwd`,
  `modelOrigin`, `systemPromptMode`, `inheritProjectContext`, `inheritGlobalContext`,
  `inheritSkills`, `outputMode`, `maxSubagentDepth`, `share`, `artifactConfig`, `runFanoutBudget`,
  `launchContractDigest`, `launchResolvedExtensions` UNCONDITIONALLY. Match that.
- `version: 1` as a unit-struct `DescriptorVersion` that refuses anything else on read, the way
  `handoff::ManifestVersion` does.
- **A write failure fails the launch**, as upstream. Do not log-and-continue.
- **The agent-mismatch refusal survives** (`async-resume.ts:566`): a descriptor for agent X on a
  run whose status says agent Y is an error, not a silent pick.
- **On-disk JSON stays upstream-compatible** — field names are pi's. The retention reader already
  parses this file; the write must satisfy it.

## Definition of done

1. Every async launch writes `<run_dir>/recovery-descriptor.json` from the resolved contract.
2. `revive_from_transcript` reads it and populates `SingleStepSpec` / `BackgroundStepsSpec` from it
   — `model`, `tools`, `extensions`, `skills`, `max_depth_override`, `structured_output_schema`,
   `output`/`output_path`/`output_mode`, `acceptance`, `usage_budget`, `turn_budget`,
   `permission_rules`, and the rest of the 58 as cyrup's structs can carry them. **Where cyrup's
   `SingleStepSpec` has no slot for a descriptor field, say so per field** — do not silently drop.
3. `has_resumable_contract` finds real files; the `[CYRUP-DELTA]` at `scan.rs:378` is deleted.
4. **The reachability test is a round-trip through production**: launch with N explicit
   overrides (a model, a narrowed tool list, a tool budget, a structured-output schema, an
   acceptance gate, a permission policy, a depth override), let it reach a resumable state, drive
   `control_resume`, and assert every override arrives on the revived spec. **Mutation: drop any
   one field from the writer → the test must fail on that field.** A test that checks three
   fields proves three fields.

## Rules

- `[CYRUP-DELTA]` reasons must be TRUE — grep the premise first. This programme has shipped
  deltas whose premise the same diff invalidated.
- No `allow(dead_code)`, no stub, no narrowing-and-reporting-done.
- Gates: fmt, clippy `--workspace --all-targets --features test-fixtures -- -D warnings`,
  `nextest run --workspace --features test-fixtures`. Baseline **10 558** / 9 skipped;
  `cyrup-it --features it` **552**.

## [AUG — descriptor]

Research-only augmentation (2026-09-19). Every citation below was re-read at v0.68.0 via
`git -C tmp/pi-subagents show v0.68.0:<path>` or in the cyrup tree; nothing is quoted from the ledger.

### 0. Anchor corrections (stale citations in the seed above)

| Seed says | Verified |
|---|---|
| `SteeringRecoveryDescriptor` has **58 fields** | **54** field declarations (`src/shared/types.ts:807-863`; lines 817, 849, 851 are comments). pi's reader allowlist (`async-resume.ts:322-330`) admits a 55th legacy key, `initialTurnBudget`, which it deletes on read (`:396-400`). |
| `RunnerConfig` at `config.rs:34-65` | the struct spans `background/runner_main/config.rs:27-361` (`#[derive]` at :27, `pub struct RunnerConfig` at :29, last field `artifact_config` at :360). |
| "check whether cyrup has a 0600 variant" | it does: `background/atomic.rs:148` `pub(crate) async fn write_private_atomic_json` (chmods the temp file to `0o600` at :167 BEFORE the rename; creates the parent). Use it. |
| DoD #2 lands `extensions` on `SingleStepSpec` | `SingleStepSpec::extensions` is a **dead slot**: no runner code reads `step.extensions` (`grep -rn "step\.extensions" src/background src/spawn src/exec` → 0 hits; `build_step_run_options` at `runner_main/executor.rs:546-720` never touches it). The consumed value is `ResolvedAgentPersona::extensions` via `to_agent_config` (`exec/agent_config.rs:338-361`). It must land on the **persona**, not the step. |
| `retained-children.ts:68` reads the descriptor | the read is `:57-65` (`readAsyncRecoveryDescriptor`, source-run check `:61`, agent check `:62`); `:68` is the cwd fallback. |
| `async-execution.ts:1993-2051` | build `:1993-2048`, write `:2051`, launch-failing catch `:2053`. `recoveryAgentConfig` at `:1992`. Correct. |
| `RECOVERY_DESCRIPTOR_FILE` read at `scan.rs:392` | `:392` is `let descriptor_path = …`; the function `has_resumable_contract` starts at `:383`, the delta at `:376-382`. |

### 1. Upstream, in full — what the descriptor IS

**Type** (`src/shared/types.ts:805-864`): 54 fields. Written UNCONDITIONALLY by the writer:
`version`, `launchContractDigest`, `runFanoutBudget`, `sourceRunId`, `agent`, `launchResolvedExtensions`,
`cwd`, `modelOrigin`, `systemPromptMode`, `inheritProjectContext`, `inheritGlobalContext`,
`inheritSkills`, `outputMode`, `maxSubagentDepth`, `share`, `artifactConfig`. Everything else is
spread `...(x ? { x } : {})`.

**Write** (`src/runs/background/async-execution.ts:1992-2055`, inside `executeAsyncSingle`):
- `recoveryAgentConfig = params.recoveryAgentConfig ?? agentConfig` (`:1992`) — on a revive the
  overlaid config is what gets re-persisted, so a chain of revives never drifts back to the file.
- Persona-derived fields come off `recoveryAgentConfig.*` (`:2014-2030`); per-call fields off
  `params.*` (`:2033-2038`); run-level off the resolved locals (`deadlineAt`, `resolvedToolBudget`,
  `shareEnabled`, `artifactsDir`, `artifactConfig`, `capabilityCeiling`, `controlConfig`).
- `maxSubagentDepth: resolveChildMaxSubagentDepth(maxSubagentDepth, recoveryAgentConfig.maxSubagentDepth)` (`:2041`) — the EFFECTIVE child ceiling, not the raw agent value.
- `writePrivateAtomicJson(path.join(asyncDir, "recovery-descriptor.json"), …)` (`:2051`), gated
  `if (!externalRunner)`, BEFORE `spawnRunner` (`:2060`). A throw → `formatAsyncStartError` (`:2053`):
  **the launch fails and nothing is spawned.**
- **Only `executeAsyncSingle` writes one.** `git grep "recovery-descriptor.json" v0.68.0 -- src`
  finds exactly three non-test sites: the write at `:2051`, the resume read, the retention read.
  Chain/parallel launches write NO descriptor. A revive IS an `executeAsyncSingle`
  (`subagent-executor.ts:2111`), so every revive writes a fresh descriptor for the NEW run.

**Read on resume** (`src/runs/background/async-resume.ts:310-440`, `readAsyncRecoveryDescriptor`):
- allowlist of keys, unknown → throw (`:322-333`); required non-empty strings `sourceRunId`,
  `agent`, `cwd`, `systemPromptMode`, `outputMode` (`:334-337`); `version !== 1` → throw (`:338`);
  per-field shape validation `:340-438`; `acceptance` re-validated through the same
  `validateAcceptanceInput` the tool boundary uses (`:293-297`).
- consumed by `resolveResumeTarget`: **agent-mismatch refusal** `:566`
  (`Async run '${runId}' has a recovery descriptor for '${recoveryDescriptor.agent}', not '${agent}'.`),
  `thinkingCeiling` `:574`, `cwd` as LAST fallback `:579`, `launchContractDigest` `:595`.
- **missing descriptor refuses the revive** for a non-workflow async child
  (`asyncReviveRequiresRecoveryDescriptor` `:299-302`; consumer `subagent-executor.ts:2059-2061`:
  `Async child '${target.runId}' is missing its required run fan-out recovery identity. Start a new run instead.`).

**Read on revive — the consumer that matters** (`src/runs/foreground/subagent-executor.ts:1882-2188`):
- `:1894-1905` — if discovery no longer finds the agent, a base config is SYNTHESISED from the
  descriptor (`description: "Persisted async recovery contract"`, `systemPrompt: ""`, the four
  prompt-mode/inherit flags, `filePath: descriptor.agentFilePath ?? …`).
- `:2065` `recoveryAgentConfig = applySteeringRecoveryAgentConfig(baseAgentConfig, descriptor)`
  (`async-resume.ts:604-632`): the descriptor OVERLAYS the discovered config field-for-field —
  `model`, `modelProvider`, `thinking`, `maxThinking = intersect(thinkingCeiling, agent.maxThinking)`,
  `tools`, `excludeTools`, `allowNestedSubagents`, `extensions`, `subagentOnlyExtensions`,
  `mcpDirectTools`, `mutationTools`, `systemPrompt ?? agent.systemPrompt`, `systemPromptMode`,
  `inheritProjectContext`, `inheritGlobalContext`, `inheritSkills`, `skills`, `skillPath`,
  `filePath`, `completionGuard`, `memory`, `output = outputPath`, `toolBudget = initialToolBudget`,
  `maxSubagentDepth`. **This is the behaviour to port: the descriptor wins over the file on disk.**
- Then into the new launch: `structuredOutputSchema` `:2072/:2177`, `acceptance` `:2077/:2179`,
  `agentContract` `:2105`, `artifactConfig` `:2106`, `artifactsDir` `:2107`, `modelResponseAliases`
  `:2126`, `maxOutput` `:2132`, `share` `:2135`, `sessionDir` `:2137`, `context` `:1883/:2145`,
  `model` `:2146`, `fast` `:2147`, `modelOverrideFromParent` `:2148`, `modelOrigin` `:2149`,
  `thinking` `:2150`, `thinkingCeiling` `:2151`, `extensionBindings` `:2152`, `requiredExtensions`
  `:2153`, `maxSubagentDepth` `:2155`, `baseRef` `:2161`, `lane` `:2167`, `controlConfig` `:2168`,
  `intercomBridge` `:2169`, `outputPath` `:2173`, `outputMode` `:2174`, `skills` `:2178`,
  `capabilityCeiling` (intersected) `:2183`, `runFanoutBudget` `:2184`.
- NOT restored on a plain `resume`: `absoluteDeadlineAt` / `initialToolBudget` as LIMITS. The
  plain revive uses the CALLER's `timeoutMs`/`toolBudget` (`:2180-2182`). The descriptor's two
  limit fields are consumed only by the steering-recovery path
  (`runs/background/steering.ts:217-241` `remainingSteeringRecoveryLimits`, called from
  `async-steering-action.ts:239-240`) — a path cyrup has not ported.

**Read by retention** (`src/runs/background/async-retention.ts:191-203`): tolerant `readJson`; only
`sourceRunId` and `sessionFile` are inspected. cyrup's port at `scan.rs:383-418` reads exactly
those two keys through `read_json_object` (`:481`). **The on-disk key names for those two are the
hard compatibility floor; everything else is read only by the writer's own crate.**

**Digest** (`src/shared/launch-contract.ts:170-198` `resolveLaunchBinding`): sha256 of the stable
JSON of `projectLaunchBinding` (`:111-135`): `{version: 2, definitionDigest, taskDigest, model,
fast, thinking, systemPromptDigest, systemPromptMode, inheritProjectContext, inheritGlobalContext,
inheritSkills, skills, tools, excludeTools, extensions, subagentOnlyExtensions, mcpDirectTools,
outputPath, outputMode, structuredOutputSchema, extensionBindings}`; `definitionDigest` is
`stableJsonDigest(projectAgentDefinition(agent))` (`:37-84`: the parsed frontmatter fields plus
`filePath` and a `fileContentDigest`). It is EVIDENCE — nothing on the revive path gates on it.

### 2. cyrup — what already exists (grep results, this pass)

| Probe | Result |
|---|---|
| `SteeringRecoveryDescriptor` / `recovery-descriptor` writer | **none.** Only `scan.rs:53` (const), `:378` (the delta), and prose at `requests.rs:257`, `background.rs:1922`. |
| `launch_contract` / `launchContractDigest` | `workflows/scripted/permit.rs:61-285` (scripted-workflow permit — a different surface); `workflows/stable_json.rs:54` `stable_json_digest`; prose at `foreground_history/record.rs:73-77` scoping it out for the FOREGROUND history. Nothing on the async launch/resume path. |
| `RunFanoutBudgetDescriptor` | REAL serde struct at `exec/run_fanout_budget.rs:92-108` (pi-shaped); `create_run_fanout_budget` at `:339` — **zero production callers** (`grep -rn "run_fanout_budget::" src/` → only `RunFanoutDoctor` and `resolve_max_spawns_per_run`). No async launch allocates a ledger. |
| `ResolvedCapabilityCeiling` | `exec/capability_ceiling.rs:80-90`, serde `{version, allowedTools?, allowedAgents?, denyExtensions, sources}` — pi-shaped. Resolved per PROCESS from `CAPABILITY_CEILING_ENV` (`:68`) + session registry (`resolve_current_capability_ceiling` `:437-443`). **Not on `RunnerConfig`/`BackgroundStepsSpec`/`SingleStepSpec`** (0 hits in `background.rs`, `runner_main/`, `requests.rs`, `chain_graph.rs`); the runner derives its own from inherited env inside `run_sync`. |
| thinking ceiling | `exec/thinking_ceiling.rs`: `THINKING_CEILING_ENV` `:32`, `inherited_thinking_ceiling()` `:125` (env), `intersect_thinking_ceilings` `:86`, `parse_thinking_level` `:54`. `RunOptions::thinking_ceiling` exists but the async path passes `None` and relies on env (`exec/mod.rs:403-416`). Same story as the capability ceiling. |
| `ResolvedAgentPersona` (`exec/agent_config.rs:193-296`) | serde camelCase; carries `name, model, model_provider, fallback_models, thinking, system_prompt_mode, system_prompt_body, tools, exclude_tools, allow_nested_subagents, extensions, subagent_only_extensions, output, inherit_project_context, inherit_skills, skills, completion_guard, max_subagent_depth, default_context, memory, tool_budget, runner, acceptance_role, default_acceptance`. **This IS cyrup's `recoveryAgentConfig`.** It lacks `file_path` (on `AgentDefinition` at `discovery/types.rs:1186` but not projected). |
| `AgentDefinition` has NO | `fast` (`types.rs:571`: "no counterpart here"), `inherit_global_context`, `skill_path`, `mcp_direct_tools`, `mutation_tools` — the last four are parsed ONLY on the runtime-registry record (`discovery/runtime_registry.rs:175-188`) and have no `AgentDefinition` field to land on. |
| `mcp_direct_tools` | `exec/tool_surface.rs:288` on `ResolvedToolSurface` — a FOREGROUND launch-time resolution threaded only into the async-started receipt (`routing.rs:892`); never crosses to hop 2. |
| `IntercomBridgeConfig` / `ExtensionBindings` / `RequiredChildExtension` / `LaunchResolvedChildExtensions` / `AgentContract` / `modelResponseAliases` / `modelOrigin` / `baseRef` (async single) / `MaxOutputConfig` as a launch input | none. (`ExtensionBindings` in `runner/status.rs:120,162` is an external-runner capability flag; `max_output` is `OutputCap::default()` seeded at dispatch, deliberately not on the persona — `agent_config.rs:183-190`.) |
| `ModelId` / `ProviderId` | `cyrup_core::str_id!` newtypes, `#[serde(transparent)]` (`cyrup-core/src/lib.rs:56-58`) — plain strings on disk, pi-compatible. |
| `ToolRef` (`discovery/types.rs:125-243`) | serialises TAGGED (`{kind, content}`); its `Deserialize` visitor (`:166-241`) accepts BOTH a plain string (`visit_str` → `from_tool_string`, `mcp:` prefix → `Mcp`) and the tagged object. So the descriptor can WRITE pi's `string[]` for `Builtin`/`Mcp` and still read back through the existing visitor. |
| Unit-struct versions to copy | `handoff::ManifestVersion` (`handoff/model.rs:400-426`), `workflows::LaneMetadataVersion` (`workflows/types.rs:507-525`), foreground-history version (`foreground_history/record.rs:~50-65`). |
| 64-hex digest newtype to copy | `handoff::ManifestDigest` (`handoff/model.rs:128-155`, `parse_labelled` + `string_newtype_impls!` `:72`). |
| Typed error to copy | `handoff::HandoffError` (`handoff/error.rs:15-210`, thiserror, `Io(#[from] io::Error)` at `:210`); `SubagentError` (`error.rs:17`) gains variants via `#[from]` the same way (`Handoff(#[from] …)`). |
| Run-dir accessor family | `RunDir::status()/events()/handoff()` (`background/run_paths.rs:67-91`); `HANDOFF_FILE_NAME` is spelled once there and `scan.rs` calls the accessor. |
| Detached spawn env | `spawn_detached_runner_with_command(…, env_overlay: &BTreeMap<String,String>)` (`background/spawn_detached.rs:203-209`) applies the overlay with `.envs()` and never `env_clear`s (`:224-228`), so the runner inherits the orchestrator's env PLUS the overlay. The overlay today is `detached_runner_env_overlay_in(&cfg.roots)` (`parent_anchor.rs:223`), built inline at `background.rs:756`. |
| Producers of `BackgroundStepsSpec` (all funnel into `spawn_background_steps`, `background.rs:424`) | `background.rs:265` (`spawn_background`, SINGLE), `chain.rs:425` (chain/parallel/graph, `mode` varies), `control.rs:396` (revive, SINGLE). `paths.rs:895` is under `#[cfg(test)]` (`:522`). |
| Production callers of `control_resume` | tool action `"resume"` at `extension/tool/routing.rs:2190-2205`; RPC `SubagentRpcMethod::Resume` at `extension/rpc/mod.rs:310-313` (via `execute_checked` → the same tool action). Both reach `revive_from_transcript` for a terminal run. |
| Existing round-trip skeleton | `background.rs:1086-1208` `a_revive_transfers_the_source_runs_slot_rather_than_charging_the_cap_twice`: sandboxed `Roots`, `spawn_command = "true"` (`:1109-1112`), persona fixture on disk (`:1091-1097`), `spawn_background`, hand-written terminal status with a real session file (`:1128-1155`), `control_resume` (`:1157-1169`), revived id parsed off the confirmation (`:1170-1174`). The RunnerConfig-carry tests at `:1853-2000` show the read-back-through-the-file assertion shape. |
| `resolve_plan_personas` (`extension/executor/resolve.rs:292-308`) | `resolve_agent(...)?` per name — a missing agent file is `AgentNotFound`, so today a revive of a run whose persona file was deleted FAILS where pi synthesises (`subagent-executor.ts:1894-1905`). |

### 3. THE FIELD MAP — all 54 upstream fields, plus 4 cyrup-only run-level fields

Legend for "lands on revive": **step** = `SingleStepSpec` (`spawn/chain_graph.rs:71-187`);
**persona** = `resolved_agents[agent]: ResolvedAgentPersona` (overlay, pi
`applySteeringRecoveryAgentConfig`); **run** = `BackgroundStepsSpec` (`requests.rs:358-456`) →
`RunnerConfig`; **env** = detached-runner env overlay (new, see §4.4); **—** = evidence only.
"Source at launch" is what `spawn_background_steps` has in hand when it writes (§4.3).

| # | upstream field (type) | written when (pi) | cyrup type on disk | source at launch | lands on revive | verdict |
|---|---|---|---|---|---|---|
| 1 | `version: 1` | always | `DescriptorVersion` (unit) | const | — (refuses ≠1 on read) | carried |
| 2 | `sourceRunId: string` | always | `RunId` | `run_id` | — (must equal `status.run_id`, refuse otherwise; pi `subagent-executor.ts:1493`, `retained-children.ts:61`) | carried |
| 3 | `agent: string` | always | `String` | `step.agent` | — (**agent-mismatch refusal** vs `status.steps[i].agent`, pi `async-resume.ts:566`) | carried |
| 4 | `cwd: string` | always | `PathBuf` | `cwd` (= `RunnerConfig::cwd`) | last rung of `effective_cwd` at `control.rs:362-364` (pi `:579`: `managedWorktree ?? status.cwd ?? … ?? descriptor.cwd`) | carried |
| 5 | `launchContractDigest?: string` | always (value always defined) | `LaunchContractDigest` (64-hex newtype, `ManifestDigest` pattern) | `stable_json_digest(LaunchBinding projection)` §4.5 | — | carried, evidence. `[CYRUP-DELTA]` premise (TRUE, grepped): no cyrup surface reads a step's launch digest on the async path — `launch_contract_digest` exists only in `workflows/scripted/permit.rs`. |
| 6 | `sessionFile?: string` | iff fork path | `Option<PathBuf>` | `session_file` (= `RunnerConfig::session_file`, the fork branch) | — (revive seeds from `status.steps[i].session_file`, pi `:567-569`; retention reads this key, `scan.rs:411-417`) | carried |
| 7 | `model?: string` | iff selected | `Option<ModelId>` | `step.model ⊕ persona.model ⊕ inherited_session_model` (pi `:1290-1295` `params.modelOverride ?? agent.model`, plus cyrup's session rung) | `Explicit` origin → `step.model`; `Configured`/`Inherited` → `persona.model` (the Inherit-source primary in `build_step_agent_config` `executor.rs:460-481`) so the revived ladder starts from the SAME model, never the reviving session's | carried |
| 8 | `modelOrigin?: "explicit"\|"inherited"\|"configured"` | always | `ModelOrigin` enum, `rename_all = "lowercase"` | derived: step.model→Explicit, persona.model→Configured, session→Inherited | decides #7's slot | carried |
| 9 | `modelOverrideFromParent?: boolean` | iff origin == inherited | `Option<bool>`, written iff `Some(true)` | `origin == Inherited` | none (redundant twin of #8; pi `:364` derives origin from it only when origin is absent) | carried |
| 10 | `modelProvider?: string` | iff agent has one | `Option<ProviderId>` | `persona.model_provider` | persona | carried |
| 11 | `fast?: boolean` | iff true | — | — | — | **cyrup has no such concept** (`discovery/types.rs:571`). Omitted. |
| 12 | `thinking?: string` | iff effective thinking | `Option<String>` | `persona.thinking` (caller's level already folded in at `background.rs:105-107`) `⊕ inherited_session_thinking` | persona.thinking | carried |
| 13 | `thinkingCeiling?: ThinkingLevel` | iff a ceiling applies | `Option<String>` validated by `parse_thinking_level` on read | `inherited_thinking_ceiling()` (env of the launching process) | **env**: `THINKING_CEILING_ENV = intersect(descriptor, inherited)` on the revive spawn's overlay (pi `:2151`; `applySteeringRecoveryAgentConfig` intersects, `:610`) | carried; **no spec/RunnerConfig slot — new `BackgroundStepsSpec::thinking_ceiling` → env overlay** |
| 14 | `tools?: string[]` | iff agent has an allowlist | `Option<Vec<ToolRef>>`, `serialize_with` → pi strings (`Builtin(n)`→`"n"`, `Mcp(n)`→`"mcp:n"`, `ExtensionPath` → the tagged object the visitor already accepts) | `persona.tools` (`spawn_background` leaves `step.tools = None`, `background.rs:230`; there is no per-call `tools` param on the tool, same as pi) | **persona.tools** (not `step.tools`: the launch value IS the persona's, and pi overlays `agentConfig.tools`) | carried — the capability-WIDENING fix |
| 15 | `excludeTools?: string[]` | iff non-empty | `Vec<String>`, `skip_serializing_if = Vec::is_empty` | `persona.exclude_tools` | persona.exclude_tools | carried |
| 16 | `allowNestedSubagents?: boolean` | iff declared | `Option<bool>` | `persona.allow_nested_subagents` | persona | carried |
| 17 | `extensions?: string[]` | iff declared | `Option<Vec<String>>` | `persona.extensions` | **persona.extensions** (NOT `step.extensions` — dead slot, §0) | carried |
| 18 | `subagentOnlyExtensions?: string[]` | iff declared | `Vec<String>`, skip-if-empty | `persona.subagent_only_extensions` | persona | carried |
| 19 | `mcpDirectTools?: string[]` | iff declared | — | — | — | **no cyrup slot**: not on `AgentDefinition`/persona; `ResolvedToolSurface::mcp_direct_tools` is foreground-only and never crosses to hop 2. Omitted. (To carry it later: add it to `AgentDefinition` + persona first.) |
| 20 | `mutationTools?: string[]` | iff declared | — | — | — | **cyrup has no such concept** (runtime-registry-only key, `runtime_registry.rs:188`, dropped before `AgentDefinition`). Omitted. |
| 21 | `systemPrompt?: string` | iff non-empty | `Option<String>`, written iff non-empty | `persona.system_prompt_body` | persona.system_prompt_body (pi `:618` `descriptor.systemPrompt ?? agent.systemPrompt`) | carried — this is why the file is `0600` |
| 22 | `systemPromptMode: "append"\|"replace"` | always | `SystemPromptMode` (cyrup enum, camelCase → `"append"/"replace"`, pi-identical) | `persona.system_prompt_mode` | persona | carried |
| 23 | `inheritProjectContext: boolean` | always | `bool` | `persona.inherit_project_context` | persona | carried |
| 24 | `inheritGlobalContext: boolean` | always | — | — | — | **cyrup has no such concept** on `AgentDefinition`/persona (runtime-registry-only, `:175`). pi's own reader defaults it to `inheritProjectContext` when absent (`:368`). Omitted. |
| 25 | `inheritSkills: boolean` | always | `bool` | `persona.inherit_skills` | persona | carried |
| 26 | `skills?: string[]` | iff the EFFECTIVE list is non-empty (`resolvedSkills`, `:2026`) | `Vec<String>`, skip-if-empty | `step.skills.clone().unwrap_or(persona.skills)` — pi's `params.skills ?? agentConfig.skills` | `step.skills = Some(list)` (pi `:2178` passes it as the per-call override) | carried |
| 27 | `skillPath?: string[]` | iff declared | — | — | — | **no cyrup concept** (runtime-registry-only, `:185`). Omitted. |
| 28 | `agentFilePath?: string` | iff known | `Option<PathBuf>` | `persona.file_path` — **new persona field** (`AgentDefinition::file_path` at `types.rs:1186` is not projected today; add `#[serde(default, skip_serializing_if)] pub file_path: Option<PathBuf>` to `ResolvedAgentPersona` and set it in `from_agent_definition`) | used only in the synthesised-persona fallback (§4.6; pi `:1903`) | carried |
| 29 | `completionGuard?: boolean` | iff declared | `Option<bool>` | `persona.completion_guard` | persona | carried |
| 30 | `memory?: {scope, path}` | iff declared | `Option<AgentMemoryConfig>` (`{scope: "project"\|"user", path}` — serde already pi-identical, `types.rs:918-935`) | `persona.memory` | persona | carried |
| 31 | `outputPath?: string` | iff resolved | `Option<String>` | `step.output_path` (already the resolved absolute path, `background.rs:244`) | `step.output_path` (pi `:2173`) | carried |
| 32 | `outputMode: "inline"\|"file-only"` | always | `OutputMode` (cyrup kebab enum: `inline`/`file-and-inline`/`file-only`; the async path only ever produces the two pi values via `parse_tool_output_mode`, `background.rs:198`) | `step.output_mode.unwrap_or(Inline)` | `step.output_mode = Some(v)` (pi `:2174`) | carried |
| 33 | `structuredOutputSchema?: object` | iff supplied | `Option<serde_json::Value>` (object-checked on read, pi `:389`) | `step.structured_output_schema` | `step.structured_output_schema` (pi `:2072/:2177`) | carried |
| 34 | `acceptance?: AcceptanceInput` | iff `!== undefined` | `Option<serde_json::Value>` (raw wire, as `step.acceptance` is; re-validated on read through `lower_acceptance_input`, pi `:293-297`) | `step.acceptance` | `step.acceptance` (pi `:2077/:2179`) | carried |
| 35 | `controlConfig?: ResolvedControlConfig` | iff resolved | `Option<ResolvedControlConfig>` (`exec/control.rs:73-83` — field-for-field pi's, minus pi's unvalidated `needsAttentionAfterMsIsExplicit?`) | `control` (= `RunnerConfig::control`) | `run.control = descriptor.control_config.or(today's fold)` (pi `resolveRevivalControlConfig`, `:2168`; cyrup's `resume` action carries no `control`, so the descriptor's is the only requested-control rung) | carried |
| 36 | `context?: "fresh"\|"fork"` | iff `params.context` | `Option<ContextMode>` (`fork_context.rs:54-63`, `rename_all = "lowercase"` — pi-identical) | `step.context` | `step.context = descriptor.context.or(Some(Fork))` — replaces today's hard-coded `Some(ContextMode::Fork)` at `control.rs:390` (pi `:1883/:2145`) | carried |
| 37 | `intercomBridge?: IntercomBridgeConfig` | iff supplied | — | — | — | **cyrup has no per-run bridge override** (only the orchestrator-target env, `spawn_plan.rs`). Omitted. |
| 38 | `lane?: WorkflowLaneMetadata` | iff supplied | `Option<WorkflowLaneMetadata>` (#143, `workflows/types.rs:560-573`, pi-identical incl. unit version) | **none on the SINGLE path**: cyrup attaches `lane` to `ParallelGroupSpec` only (`chain_graph.rs:212-226`); `BackgroundSingleRequest` has no lane | (would be a new `SingleStepSpec`/`BackgroundStepsSpec` field) | typed `Option`, always `None` today. `[CYRUP-DELTA]` premise (TRUE): a single run has no lane to record; keep the slot so a future single-lane lands without a format change. |
| 39 | `absoluteDeadlineAt?: number` | iff a deadline | `Option<u64>` epoch-ms | `deadline_at_ms` (computed at `background.rs:474-478`) | — (pi's plain `resume` does NOT restore it, `:2180-2181`; only the unported steering-recovery path does, `steering.ts:223-228`) | carried, evidence |
| 40 | `initialToolBudget?: ResolvedToolBudget` | iff budgeted | `Option<ResolvedToolBudget>` (`types.rs:983-989`: `{hard, soft?, block}` — pi-identical incl. `"*"`) | `persona.tool_budget` (caller's budget already folded in at `background.rs:96-98`; = pi `params.toolBudget ?? agentConfig.toolBudget`) | persona.tool_budget (pi `:629`) | carried |
| 41 | `maxSubagentDepth: number` | always | `u32` | `persona.max_subagent_depth.map_or(cfg.max_subagent_depth, \|a\| a.min(cfg.max_subagent_depth))` — pi's `resolveChildMaxSubagentDepth` (`:2041`) | `step.max_depth_override = Some(v)` (tightening-only, `next_envelope`; pi `:2155`) | carried — the second capability-WIDENING fix |
| 42 | `maxOutput?: MaxOutputConfig` | iff supplied | — | — | — | **cyrup has no launch-time max-output** (`OutputCap::default()` seeded at dispatch, `agent_config.rs:183-190`). Omitted. |
| 43 | `capabilityCeiling?: ResolvedSubagentCapabilityCeiling` | iff one resolves | `Option<ResolvedCapabilityCeiling>` (`exec/capability_ceiling.rs:80-90`, pi-identical) | `resolve_current_capability_ceiling(session_id)` (env + registry, parent-side) | **env**: `CAPABILITY_CEILING_ENV = encode_capability_ceiling(intersect(descriptor, current))` on the revive spawn's overlay (pi `:2183` intersects target/descriptor/current) | carried; **no spec/RunnerConfig slot — new `BackgroundStepsSpec::capability_ceiling` → env overlay** |
| 44 | `launchResolvedExtensions?: …` | always | — | — | — | **cyrup has no launch-resolved extension evidence.** Omitted. |
| 45 | `share: boolean` | always | `bool` | `share.unwrap_or(false)` | `run.share = Some(v)` (pi `:2135`) | carried |
| 46 | `sessionDir?: string` | iff resolved | `Option<PathBuf>` | `step.session_dir` (the resolved `<root>/run-0` LEAF, `background.rs:217-219`; pi persists the per-run ROOT and re-appends `run-0` on revive — same directory either way) | `step.session_dir` (pi `:2137`) | carried |
| 47 | `artifactsDir?: string` | iff enabled | `Option<PathBuf>` | `artifacts_dir` | `run.artifacts_dir` (pi `:2107`) | carried |
| 48 | `artifactConfig: ArtifactConfig` | always | `ArtifactConfig` (`artifacts.rs:99-110`; pi's reader requires the five bools + integer `cleanupDays`, which cyrup writes; cyrup's extra `includeTranscript`/`dir` keys are ignored by pi's reader) | `artifact_config` | `run.artifact_config` (pi `:2106`) | carried |
| 49 | `runFanoutBudget: RunFanoutBudgetDescriptor` | always (REQUIRED) | `Option<RunFanoutBudgetDescriptor>` (type at `run_fanout_budget.rs:92-108`) | **none**: no async launch calls `create_run_fanout_budget` (`:339`, zero production callers) | — (no `RunnerConfig` slot either) | `[CYRUP-DELTA]` premise (TRUE, grepped): cyrup allocates no per-run fan-out ledger at async launch, so there is no descriptor to record; `Option` because writing a ledger handle for a ledger that does not exist would be a lie on disk. Wiring `create_run_fanout_budget` into the launch is a separate feature (it would close a fourth tested-but-unreachable piece). |
| 50 | `modelResponseAliases?: Record<string,string[]>` | iff ctx has them | — | — | — | **no cyrup concept.** Omitted. |
| 51 | `extensionBindings?: ExtensionBindings` | iff supplied | — | — | — | **no cyrup concept for pi children** (`runner/status.rs:120` is an external-runner capability flag). Omitted. |
| 52 | `requiredExtensions?: RequiredChildExtensionSnapshot` | iff non-empty | — | — | — | **no cyrup concept.** Omitted. |
| 53 | `agentContract?: {version: 1}` | iff supplied | — | — | — | **no cyrup concept.** Omitted. |
| 54 | `baseRef?: string` | iff supplied | — | — | — | **not on cyrup's async single path** (`base_ref` lives in `workflows/scripted` + `scheduled_runs` only). Omitted. |

**Tally**: 33 carried-and-landed (1-4, 6-10, 12, 14-18, 21-23, 25-26, 28-36, 40-41, 45-48 — with 4 landing
on the persona overlay, 11 on the step, 4 on the run, 1 on cwd); 2 landed through the env overlay
(13, 43); 4 carried as evidence with no revive slot, exactly as upstream (5, 6, 39, and 28 which is
consumed only by the fallback); 2 typed-`Option` with a TRUE delta and no cyrup source today (38,
49); **13 omitted because cyrup has no such concept** (11, 19, 20, 24, 27, 37, 42, 44, 50-54).

**4 cyrup-only run-level fields (additive `[CYRUP-DELTA]`, on-disk keys camelCase):**

| key | type | source | lands | why |
|---|---|---|---|---|
| `turnBudget` | `Option<ResolvedTurnBudget>` (`turn_budget.rs:35-39`) | `turn_budget` | `run.turn_budget` | pi REMOVED its `initialTurnBudget` (reader deletes it, `:396-400`); cyrup's SUBA-008 budget is live and DoD #2 names it. Without it the revive is `turn_budget: None` — today's degradation. |
| `usageBudget` | `Option<UsageBudgetConfig>` (`usage_budget.rs:45-51`) | `usage_budget` | `run.usage_budget` | pi has no usage budget on the descriptor; SUBA-021 is cyrup's. DoD #2 names it. |
| `permissionRules` | `Option<PermissionRules>` (`BTreeMap<String, PermissionRuleDecision>`, `permission_arbiter.rs:430`) | `permission_rules` | `run.permission_rules` | pi resolves permissions from config+agent at every launch; cyrup's revive resolves NONE (`control.rs:400-402`), which is the UW-6 gap the seed names. DoD #2 names it. |
| `includeProgress` | `Option<bool>` | `include_progress` | `run.include_progress` | SUBA-N06 is cyrup-only (pi's async return has nothing to gate). Dropping it degrades to `None` silently — SUBA-041's defect class. |

Premise of the delta (TRUE): pi's reader allowlist (`async-resume.ts:322-330`) would reject these
four keys, but no pi process ever reads cyrup's async root — the writer and every reader of this
file are in this crate.

### 4. Rust shape

**4.1 Module**: `crates/cyrup-ext-subagents/src/background/recovery_descriptor.rs`, re-exported at
`crate::background` (`RecoveryDescriptor`, `RecoveryDescriptorError`, `DescriptorVersion`,
`ModelOrigin`, `LaunchContractDigest`). Path accessor in `run_paths.rs`:
`const RECOVERY_DESCRIPTOR_FILE_NAME: &str = "recovery-descriptor.json"` +
`RunDir::recovery_descriptor(&self) -> PathBuf` — the fourth member of the `status()/events()/handoff()`
family, and the single place the literal is spelled. `scan.rs:53` deletes its own const and calls the
accessor (the same drift-prevention `handoff()`'s doc at `run_paths.rs:84-87` describes).

**4.2 Types** (all `#[serde(rename_all = "camelCase")]`; the top-level struct also
`deny_unknown_fields`, matching pi's allowlist strictness at `:331-333`):

```rust
/// `version: 1` — refuses anything else on read (copy `handoff::ManifestVersion`, model.rs:400-426).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DescriptorVersion;            // Serialize → 1u32; Deserialize: raw == 1 else custom error

/// pi `modelOrigin` — `explicit` | `inherited` | `configured` (`types.ts:823`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelOrigin { Explicit, Inherited, Configured }

/// 64-hex sha256, lower-cased on parse (copy `handoff::ManifestDigest`, model.rs:128-155).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)] #[serde(transparent)]
pub struct LaunchContractDigest(String);  // `parse(&str) -> Result<Self, RecoveryDescriptorError>`; manual Deserialize via parse

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryDescriptor {
    pub version: DescriptorVersion,
    pub launch_contract_digest: LaunchContractDigest,
    pub source_run_id: RunId,
    pub agent: String,
    pub cwd: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub session_file: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub model: Option<ModelId>,
    pub model_origin: ModelOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub model_override_from_parent: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub model_provider: Option<ProviderId>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub thinking: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub thinking_ceiling: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", serialize_with = "tool_refs_as_pi_strings")]
    pub tools: Option<Vec<ToolRef>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub exclude_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub allow_nested_subagents: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub extensions: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub subagent_only_extensions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub system_prompt: Option<String>,
    pub system_prompt_mode: SystemPromptMode,
    pub inherit_project_context: bool,
    pub inherit_skills: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub agent_file_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub completion_guard: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub memory: Option<AgentMemoryConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub output_path: Option<String>,
    pub output_mode: OutputMode,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub structured_output_schema: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub acceptance: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub control_config: Option<ResolvedControlConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub context: Option<ContextMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub lane: Option<WorkflowLaneMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub absolute_deadline_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub initial_tool_budget: Option<ResolvedToolBudget>,
    pub max_subagent_depth: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub capability_ceiling: Option<ResolvedCapabilityCeiling>,
    pub share: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub session_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub artifacts_dir: Option<PathBuf>,
    pub artifact_config: ArtifactConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub run_fanout_budget: Option<RunFanoutBudgetDescriptor>,
    // ---- cyrup-only run-level (additive delta, §3) ----
    #[serde(default, skip_serializing_if = "Option::is_none")] pub turn_budget: Option<ResolvedTurnBudget>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub usage_budget: Option<UsageBudgetConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub permission_rules: Option<PermissionRules>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub include_progress: Option<bool>,
}
```

Read-side validation beyond serde (a `validate(&self, path) -> Result<(), RecoveryDescriptorError>`
called by `read`): `thinking_ceiling` through `parse_thinking_level` (pi `:346`); `acceptance` through
`lower_acceptance_input` (pi `:293-297`/`:438`); `structured_output_schema` must be an object
(`:389`); `agent`/`cwd` non-empty (`:334-337`); `model_origin == Inherited ⇔ model_override_from_parent == Some(true)`.
Two-arg cross-checks live on the reader, not the struct: `assert_belongs_to(&self, status: &RunStatus, step_agent: &str)` —
`source_run_id == status.run_id` else `SourceRunMismatch`; `agent == step_agent` else `AgentMismatch`.

**Error** (`thiserror`, added to `SubagentError` as `RecoveryDescriptor(#[from] RecoveryDescriptorError)` at `error.rs`):

```rust
pub enum RecoveryDescriptorError {
    #[error("Failed to persist async recovery descriptor for '{run_id}': {source}")]         Persist { run_id: RunId, #[source] source: io::Error },   // pi :2053 wording
    #[error("Failed to parse async recovery descriptor '{path}': {detail}")]                  Parse { path: PathBuf, detail: String },                  // pi :318
    #[error("Invalid async recovery descriptor '{path}': {detail}")]                          Invalid { path: PathBuf, detail: String },                // pi :320-437 family (unknown field, version, shapes)
    #[error("Async run '{run_id}' has a recovery descriptor for '{descriptor_agent}', not '{step_agent}'.")] AgentMismatch { run_id: RunId, descriptor_agent: String, step_agent: String },   // pi :566 verbatim
    #[error("Nested run '{run_id}' has a recovery descriptor for a different source run.")]   SourceRunMismatch { run_id: RunId, found: RunId },        // pi subagent-executor.ts:1493
    #[error("Async child '{run_id}' is missing its required run fan-out recovery identity. Start a new run instead.")] Missing { run_id: RunId },   // pi subagent-executor.ts:2060 verbatim
}
```

**4.3 Write** — `RecoveryDescriptor::write(&self, path) -> Result<(), RecoveryDescriptorError>`
wraps `background::atomic::write_private_atomic_json` (`atomic.rs:148`, the `0600` variant; pi
`writePrivateAtomicJson`). Constructor `RecoveryDescriptor::from_launch(LaunchInputs<'_>)` takes
borrowed views of exactly what `spawn_background_steps` holds: `&SingleStepSpec`,
`&ResolvedAgentPersona`, `cfg.max_subagent_depth`, `cwd`, `session_file`, `deadline_at_ms`,
`share`, `artifacts_dir`, `&artifact_config`, `control.as_ref()`, the four run-level values,
`inherited_session_model/thinking` (via `self.remembered_parent_*`), `thinking_ceiling`,
`capability_ceiling`, and `original_task` (for the digest).

**4.4 Where** — in `spawn_background_steps` (`background.rs:424`), gated
`mode == RunMode::Single && matches!(steps.as_slice(), [RunnerStep::SingleStep(_)])` (pi writes
only from `executeAsyncSingle`; chain/parallel graphs get none). Position: AFTER the capacity claim
(`:574-601`) and BEFORE `write_atomic_json(&cfg_path, &runner_config)` (`:736`), so a failed
descriptor leaves **no `runner-config.json` and no process**, and rolls the slot back exactly like
the two fallible steps that follow (`:732-739`, `:751-765`):

```rust
if let Some(descriptor) = RecoveryDescriptor::for_single_launch(/* … */) {
    if let Err(error) = descriptor.write(&RunDir::for_existing(&run_paths.run_dir).recovery_descriptor()).await {
        Self::rollback_capacity(capacity.as_mut(), &run_id).await;
        return Err(error.into());        // fails the launch — pi async-execution.ts:2053
    }
}
```

The two ceilings are resolved HERE, once, parent-side (`inherited_thinking_ceiling()`,
`resolve_current_capability_ceiling(session.as_deref())`) — the same "this process has the env and
the session; hop 2 must not re-derive" reasoning `RunnerConfig::model_scope`'s doc gives. Two new
`BackgroundStepsSpec` fields, `thinking_ceiling: Option<String>` and
`capability_ceiling: Option<ResolvedCapabilityCeiling>`, are `None` from every producer except the
revive; when `Some`, `spawn_background_steps` inserts `THINKING_CEILING_ENV` /
`CAPABILITY_CEILING_ENV` (`encode_capability_ceiling`) into the env overlay it already builds at
`:756` — the runner reads those env vars today, so no `RunnerConfig` field is needed. Intersect with
the current process's own ceiling before inserting (pi `:2183`, `:610`): a revive must never widen.

**4.5 Digest** — `launch_binding_digest(persona, step, task, model, thinking, output_path, output_mode, schema) -> LaunchContractDigest`
= `stable_json_digest(json!({ "version": 2, "definitionDigest": …, "taskDigest": stable_json_digest(task),
"model", "thinking", "systemPromptDigest": stable_json_digest(system_prompt), "systemPromptMode",
"inheritProjectContext", "inheritSkills", "skills", "tools", "excludeTools", "extensions",
"subagentOnlyExtensions", "outputPath", "outputMode", "structuredOutputSchema" }))` with `undefined`
keys omitted (pi `launch-contract.ts:111-135`) and `definitionDigest = stable_json_digest(persona as
JSON + agentFilePath + sha256 of the file's bytes when readable)` (pi `:37-84`). Evidence only.

**4.6 Read** — `RecoveryDescriptor::read(path) -> Result<Option<Self>, RecoveryDescriptorError>`
(`None` iff absent, pi `:313`). In `revive_from_transcript` (`control.rs:281`), directly after
`status` (`:297`) and the agent lookup (`:298-304`):

```rust
let descriptor = RecoveryDescriptor::read(&RunDir::new(&async_root, &source_id).recovery_descriptor()).await?
    .ok_or_else(|| RecoveryDescriptorError::Missing { run_id: source_id.clone() })?;   // pi :2059-2061
descriptor.assert_belongs_to(&status, &agent)?;                                           // pi :566, :1493
```

Then, in order:
1. **persona**: `resolve_plan_personas(...)` as today; on `Err(SubagentError::AgentNotFound(_))`
   substitute `descriptor.synthesised_persona()` (pi `:1894-1905`: name = agent, empty prompt, the
   prompt-mode/inherit flags, `file_path`); in both cases `descriptor.apply_to_persona(&mut persona)`
   (pi `applySteeringRecoveryAgentConfig`): rows 7(when origin ≠ Explicit), 10, 12, 14-18, 21-23,
   25, 28-30, 40. The descriptor wins over the file.
2. **step** (`control.rs:373-392`): `model` (row 7 when Explicit), `skills` (26), `max_depth_override`
   (41), `structured_output_schema` (33), `output_path` (31), `output_mode` (32), `acceptance` (34),
   `context` (36, replacing the hard-coded `Fork`), `session_dir` (46). `tools`/`extensions` stay
   `None` on the step — they rode the persona.
3. **run** (`control.rs:396-443`): `usage_budget`, `turn_budget`, `permission_rules`,
   `include_progress`, `share`, `artifacts_dir`, `artifact_config`, `control` (descriptor's
   `control_config` else today's fold), plus the two new ceiling fields. `timeout_ms` stays as
   today (pi's plain resume does not restore the deadline, §1).
4. **cwd**: append `.or_else(|| Some(descriptor.cwd.clone()))` before the `cwd.to_path_buf()`
   fallback at `control.rs:362-364` (pi `:579`).
5. The revive's own `spawn_background_steps` then writes a NEW descriptor for the revived run from
   the overlaid persona — pi's `recoveryAgentConfig` (`:1992`, `:2116`), so a second revive sees the
   same contract, not the file.

**4.7 Retention** (`scan.rs`): delete `RECOVERY_DESCRIPTOR_FILE` (`:53`) and the `[CYRUP-DELTA]`
paragraph (`:378-382`) — its premise ("no cyrup writer produces … today") becomes FALSE in the same
diff. Call `RunDir::for_existing(run_dir).recovery_descriptor()`. Keep the tolerant raw read
(pi's `readJson`, `:196-197`: unreadable → protected).

### 5. Production call sites (exact)

| | file:line | change |
|---|---|---|
| WRITE | `src/extension/executor/background.rs:619-739` (`spawn_background_steps`, between the capacity claim and the `runner-config.json` write) | build + `write_private_atomic_json`; failure → `rollback_capacity` + `Err` |
| READ | `src/extension/executor/control.rs:297-443` (`revive_from_transcript`) | read, `Missing`/`AgentMismatch`/`SourceRunMismatch` refusals, persona overlay/synthesis, step + run population, cwd fallback |
| READ | `src/background/async_retention/scan.rs:383-418` (`has_resumable_contract`) | path via `RunDir`; delta deleted |
| PATH | `src/background/run_paths.rs` | `RunDir::recovery_descriptor()` |
| SPEC | `src/extension/executor/requests.rs:358-456` (`BackgroundStepsSpec`) | `thinking_ceiling`, `capability_ceiling` |
| PRODUCERS | `background.rs:265`, `chain.rs:425`, `control.rs:396` | fill the two new fields (`None`, `None`, descriptor's) |
| PERSONA | `src/exec/agent_config.rs:193-331` | `file_path: Option<PathBuf>` + `from_agent_definition` |
| ERROR | `src/error.rs:17` | `RecoveryDescriptor(#[from] …)` |
| ENV | `background.rs:751-757` | bind the overlay to a `let mut`, insert the two ceiling vars when `Some` |
| REACH | tool `routing.rs:2190-2205`, RPC `rpc/mod.rs:310-313` | unchanged — both already reach `control_resume` → `revive_from_transcript` |

### 6. The round-trip test (DoD #4), designed

**Location**: `extension/executor/background.rs` tests, next to
`a_revive_transfers_the_source_runs_slot_rather_than_charging_the_cap_twice` (`:1086`), whose
scaffold it reuses verbatim: per-test `tempfile::tempdir()`, `Roots::sandboxed(dir)`,
`spawn_command = "true"` (`:1109-1112` — both hops exec `true(1)` and exit; nothing writes into
the sandbox), `FixedSessionHost`, persona fixture at `<cwd>/.cyrup/agents/worker.md`.

**Zero fields need a real process.** The descriptor is written BEFORE the spawn from data the
orchestrator resolved, and every revive landing is observable in the revived run's
`runner-config.json`, which `spawn_background_steps` writes BEFORE its spawn (`:736`). That file
is the entire hop-1→hop-2 contract (R-SA-073), and `run_single`/`build_step_agent_config`
(`executor.rs:411-528`) honouring each `RunnerConfig`/persona/step field is already proven by the
existing runner tests. A cyrup-it smoke that boots the fixture runner from a revived config is a
nice-to-have, not a DoD item.

**The load-bearing trick — mutate the persona file between launch and resume.** Persona-derived
rows (14-18, 21-25, 29-30, 40-41) would pass by RE-DISCOVERY even if the descriptor dropped them.
So after launch, REWRITE `worker.md` with a WIDER contract (`tools: [read, grep, bash]`,
`maxSubagentDepth: 9`, `excludeTools: []`, `completionGuard: true`, another model, another
thinking, `systemPromptMode: append`, different memory/skills). A revive that consults the file
instead of the descriptor then fails on every one of those rows — that is what makes the mutation
table below non-vacuous. (Optionally, as a second case, DELETE the file → the synthesised-persona
path, pi `:1894`.)

**Launch** (`spawn_background(BackgroundSingleRequest { … })` with cfg `max_subagent_depth: 4`,
`permissions: Some(json!({"write": "deny"}))`, `control: Some(…)`):
persona frontmatter `model: fixture/persona-model`, `tools: [read, grep]`, `excludeTools: [bash]`,
`toolBudget: {hard: 7, soft: 3}`, `maxSubagentDepth: 2`, `completionGuard: false`, `thinking: low`,
`systemPromptMode: replace`, `inheritSkills: false`, `inheritProjectContext: false`,
`allowNestedSubagents: true`, `extensions: [ext-a]`, `memory: {scope: project, path: notes.md}`,
`skills: [alpha]`, `modelProvider: fixture`; request overrides `model_override: Some("anthropic/override")`,
`thinking: Some("high")`, `tool_budget: Some({hard: 5})`, `turn_budget: Some({maxTurns: 9, graceTurns: 1})`,
`usage_budget: Some({tokens: {hard: 1000}})`, `structured_output_schema: Some(schema)`,
`acceptance: Some(json!({"level":"verified","verify":[{"id":"unit","command":"true"}]}))`,
`output: Some("out.md")`, `output_mode: Some("file-only")`, `skills: Some(["beta"])`,
`share: Some(true)`, `session_dir: Some(<dir>/sessions)`, `artifacts: Some(false)`,
`timeout_ms: Some(60_000)`, `include_progress: Some(true)`, `context: Some(Fresh)`.

**Assert A (write)**: `<run_dir>/recovery-descriptor.json` exists; `metadata.permissions().mode() & 0o777 == 0o600`;
raw JSON has pi's keys (`sourceRunId`, `agent`, `cwd`, `outputMode == "file-only"`, `tools == ["read","grep"]`,
`maxSubagentDepth == 2`, `share == true`, `version == 1`); typed `RecoveryDescriptor::read` succeeds and
equals the expected struct field-for-field.

**Settle** exactly as `:1128-1155` (terminal status, one step `worker`, real session file, `status.cwd`, `session_id`).
**Mutate** `worker.md` as above. **Resume**: `control_resume(dir, Some(id), Some("carry on"), None, None)` →
parse `Revived run: <id>`; read `<revived_run_dir>/runner-config.json` → `RunnerConfig`; also assert
`<revived_run_dir>/recovery-descriptor.json` exists (pi writes one per revive).

**Assert B — the mutation table** (one assertion per row; "drop F from the writer" = the writer
emits nothing for F, so the reader's `None`/default reaches the revive; each line names the assertion
that then fails):

| row | descriptor field | revived landing | assertion that fails if dropped |
|---|---|---|---|
| 7/8 | `model`/`modelOrigin` (Explicit) | `steps[0].model` | `== Some("anthropic/override")` (file now says another model) |
| 10 | `modelProvider` | `resolved_agents["worker"].model_provider` | `== Some("fixture")` (file mutated) |
| 12 | `thinking` | persona `.thinking` | `== Some("high")` (file says another) |
| 13 / 43 | `thinkingCeiling` / `capabilityCeiling` | env overlay on the revive spawn | the only two rows whose landing is a child-process env var: point `spawn_command` at a 3-line script in the tempdir (`env > "$CYRUP_ENV_DUMP"`, or the cyrup-it fixture binary, which already dumps its env) and assert `CYRUP_SUBAGENT_THINKING_CEILING == "low"` / `CYRUP_SUBAGENT_CAPABILITY_CEILING_V1` decodes to the launched ceiling in the dump. No runner, no model, no network — but it IS a spawned process, unlike every other row. (`set_var` is not an option: the crate is `#![forbid(unsafe_code)]`, so the launch-side ceiling comes from a sandboxed env only through the spawn.) |
| 14 | `tools` | persona `.tools` | `== Some([Builtin("read"), Builtin("grep")])` (file now has `bash`) |
| 15 | `excludeTools` | persona `.exclude_tools` | `== ["bash"]` (file now `[]`) |
| 16 | `allowNestedSubagents` | persona | `== Some(true)` (file now false) |
| 17 | `extensions` | persona `.extensions` | `== Some(["ext-a"])` (file now another) |
| 21 | `systemPrompt` | persona `.system_prompt_body` | `== <launch body>` (file body rewritten) |
| 22 | `systemPromptMode` | persona | `== Replace` (file now append) |
| 23/25 | `inheritProjectContext`/`inheritSkills` | persona | both `false` (file now true) |
| 26 | `skills` | `steps[0].skills` | `== Some(["beta"])` |
| 29 | `completionGuard` | persona | `== Some(false)` (file now true) |
| 30 | `memory` | persona | `== Some({Project, "notes.md"})` (file now user/other) |
| 31 | `outputPath` | `steps[0].output_path` | `.ends_with("out.md")` |
| 32 | `outputMode` | `steps[0].output_mode` | `== Some(FileOnly)` (default would be `Inline`) |
| 33 | `structuredOutputSchema` | `steps[0].structured_output_schema` | `== Some(schema)` |
| 34 | `acceptance` | `steps[0].acceptance` | `== Some(policy)` |
| 35 | `controlConfig` | `cfg.control` | `== Some(launch-resolved control)` where the launch `control` override sets `needs_attention_after_ms: 1234` — a dropped field re-folds from config and yields the default |
| 36 | `context` | `steps[0].context` | `== Some(Fresh)` (today's hard-coded `Fork` would fail) |
| 40 | `initialToolBudget` | persona `.tool_budget` | `== Some({hard: 5})` (file says 7) |
| 41 | `maxSubagentDepth` | `steps[0].max_depth_override` | `== Some(2)` (file now 9; persona re-discovery gives 9) |
| 45 | `share` | `cfg.share` | `== Some(true)` |
| 46 | `sessionDir` | `steps[0].session_dir` | `== Some(<launch leaf>)` |
| 47/48 | `artifactsDir`/`artifactConfig` | `cfg.artifacts_dir`/`cfg.artifact_config` | `is_none()` / `.enabled == false` (revive today sends `ArtifactConfig::default()`, enabled) |
| 4 | `cwd` | revived `cfg.cwd` | equals launch cwd when the settle step OMITS `status.cwd` (a second case: `status.cwd = None`) |
| + | `turnBudget` | `cfg.turn_budget` | `== Some({9,1})` |
| + | `usageBudget` | `cfg.usage_budget` | `== Some({tokens hard 1000})` |
| + | `permissionRules` | `cfg.permission_rules` | `== Some({"write": Deny})` |
| + | `includeProgress` | `cfg.include_progress` | `== Some(true)` |
| 2 | `sourceRunId` | refusal | separate case: hand-edit the descriptor's `sourceRunId` → `control_resume` is `Err` containing "different source run" |
| 3 | `agent` | refusal | separate case: hand-edit `agent: "other"` → `Err == "Async run '<id>' has a recovery descriptor for 'other', not 'worker'."` |
| 1 | `version` | refusal | hand-edit `version: 2` → `Err` mentions the version |
| — | (absent) | refusal | delete the file → `Err == "Async child '<id>' is missing its required run fan-out recovery identity. Start a new run instead."` |
| — | unknown key | refusal | add `"bogus": 1` → `Err` names `bogus` |
| — | write failure | launch fails | pre-create `<run_dir>/recovery-descriptor.json` as a DIRECTORY (the rename then fails) → `spawn_background` is `Err`, `runner-config.json` is ABSENT, and the capacity pool holds zero slots |
| 5 | `launchContractDigest` | evidence | asserted in A only: 64 lower-hex chars, and DIFFERENT for two launches whose only difference is the task |
| 6/39 | `sessionFile`/`absoluteDeadlineAt` | evidence | asserted in A only (`sessionFile` = the fork path when `context: Fork`; `absoluteDeadlineAt ≈ now + 60_000`) |

Rows 13 and 43 are the only two whose landing is an env var on a child process; they are proven
with the env-dumping spawn command (still no runner, no model, no network — a shell one-liner in
the tempdir), or left to a cyrup-it case with the fixture binary. Everything else is pure data on
disk.

**Retention test** (`scan.rs` tests): a descriptor written by `spawn_background` (not a
hand-written map) whose `sessionFile` exists, on a status whose own `session_file`s are absent,
makes `has_resumable_contract` `true`; delete the session file → `false`; a descriptor for another
`sourceRunId` → `true` (pi `:201`).

### 7. Decisions the implementer should NOT re-open (they are pi's, verified)

- Missing descriptor on a terminal async revive **refuses** with pi's sentence (`:2059-2061`).
  Runs launched before this feature have none; that is exactly upstream's stance and the
  alternative is the bare-agent revive this task removes. `children.list` (later task) also treats
  "missing recovery descriptor" as not-resumable (`retained-children.ts:60`).
- The descriptor **overlays** the discovered persona; the file on disk does not win
  (`applySteeringRecoveryAgentConfig`).
- `tools`/`extensions`/`toolBudget`/`maxSubagentDepth`/`systemPrompt*`/`inherit*`/`memory`/
  `completionGuard` land on the PERSONA; `model` (Explicit), `skills`, `output*`, `schema`,
  `acceptance`, `context`, `sessionDir`, `maxSubagentDepth` (as the tightening override) land on the
  STEP; budgets/policy/share/artifacts/control land on the RUN.
- The plain `resume` does not re-arm the launch deadline or subtract consumed tool budget; that is
  the unported steering-recovery path (`steering.ts:217-241`).
- Write BEFORE `runner-config.json`, fail the launch, roll the capacity slot back.

### 8. Files touched (predicted)

`src/background/recovery_descriptor.rs` (new) · `src/background/mod.rs` · `src/background/run_paths.rs` ·
`src/background/async_retention/scan.rs` · `src/extension/executor/background.rs` ·
`src/extension/executor/requests.rs` · `src/extension/executor/control.rs` · `src/extension/executor/chain.rs` ·
`src/exec/agent_config.rs` · `src/error.rs` · tests in `background.rs`, `control.rs`, `scan.rs`,
`recovery_descriptor.rs`.

## [EXEC — descriptor]

Executed 2026-09-19. Every anchor below is post-`cargo fmt --all`; every upstream citation is
@v0.68.0 via `git -C tmp/pi-subagents show v0.68.0:<path>`.

### Production call sites (file:line)

| | file:line | what |
|---|---|---|
| WRITE | `crates/cyrup-ext-subagents/src/extension/executor/background.rs:776-790` (`spawn_background_steps`) | `RecoveryDescriptor::for_single_launch(&runner_config, LaunchInputs{..})` (type renamed from `LaunchCeilings` in FIX D5) → `.write(&RunDir::for_existing(&run_paths.run_dir).recovery_descriptor())`, AFTER the capacity claim and BEFORE `write_atomic_json(&cfg_path, …)` (:792); `Err` → `rollback_capacity` (:788) + `return Err(error.into())` (:789) — pi `async-execution.ts:2051-2053`. Reached by `spawn_background` (background.rs:263, the SINGLE tool path) and by the revive (control.rs:451). |
| CEILINGS | `background.rs:535-556` | `own_thinking_ceiling` ∩ requested, `resolve_current_capability_ceiling(session)` ∩ requested — resolved once, parent-side, BEFORE the capacity claim (like `model_scope`), feeding the descriptor and the overlay. |
| ENV LANDING | `background.rs:818-842` | `env_overlay` = `detached_runner_env_overlay_in(&cfg.roots)` + `THINKING_CEILING_ENV` (:821) / `CAPABILITY_CEILING_ENV` (:831), inserted ONLY when the caller carried a ceiling (a revive); handed to `spawn_detached_runner_with_command` (:841). |
| READ | `crates/cyrup-ext-subagents/src/extension/executor/control.rs:315-322` (`revive_from_transcript`) | `RecoveryDescriptor::read(RunDir::new(&async_root, &source_id).recovery_descriptor())` → `Missing` refusal (:319, pi `subagent-executor.ts:2060`) → `assert_belongs_to(&status, &agent)` (:322, pi `async-resume.ts:566` / `subagent-executor.ts:1493`); cwd ladder's last rung `descriptor.cwd` (:386); `resolve_plan_personas` with `AgentNotFound` → `synthesised_persona()` (:403); `apply_to_persona` (:408); the step from the descriptor (:417-445, `max_depth_override` :435); `BackgroundStepsSpec` from the descriptor (:451-507: budgets/policy :455-457, control :481, share/artifacts :497-499, ceilings :504-505). |
| REACH | `control.rs:112` `control_resume` → `:257` `revive_from_transcript` ← tool action `"resume"` (`extension/tool/routing.rs:2190`) and RPC `SubagentRpcMethod::Resume` (`extension/rpc/mod.rs:310`) | unchanged, both already reach the revive. |
| READ (retention) | `crates/cyrup-ext-subagents/src/background/async_retention/scan.rs:382-417` (`has_resumable_contract`) | path via `RunDir::for_existing(run_dir).recovery_descriptor()` (:391); the const at old `:53` and the `[CYRUP-DELTA]` at old `:376-382` are DELETED (premise false in this diff). |
| PATH | `crates/cyrup-ext-subagents/src/background/run_paths.rs:30` + `:109` | `RECOVERY_DESCRIPTOR_FILE_NAME` + `RunDir::recovery_descriptor()`, the fourth member of `status()/events()/handoff()`. |
| SPEC | `crates/cyrup-ext-subagents/src/extension/executor/requests.rs:465,471` | `BackgroundStepsSpec::{thinking_ceiling, capability_ceiling}`. Producers: `background.rs:303-304` (None), `chain.rs:436-437` (None), `control.rs:504-505` (descriptor's), `paths.rs` test literal, and a FIFTH the seams map did not list — cyrup-it `subagent_persona_and_depth_integration.rs:1211-1212` (None, Chain mode). |
| PERSONA | `crates/cyrup-ext-subagents/src/exec/agent_config.rs:304` + `:339` | `ResolvedAgentPersona::file_path: Option<PathBuf>` (`default`, `skip_serializing_if`) set in `from_agent_definition`. |
| ERROR | `crates/cyrup-ext-subagents/src/error.rs:275` | `SubagentError::RecoveryDescriptor(#[from] RecoveryDescriptorError)`, `#[error(transparent)]`. |
| MODULE | `crates/cyrup-ext-subagents/src/background/recovery_descriptor.rs` (new; `background/mod.rs:63` + re-exports `:128`) | `RecoveryDescriptorError` :93 · `DescriptorVersion` :171 · `ModelOrigin` :204 · `LaunchContractDigest` :217 · `LaunchCeilings` :257 (renamed `LaunchInputs` in FIX, + `stored_model_origin`) · `RecoveryDescriptor` :272 · `tool_refs_as_pi_strings` :415 · `resolve_model_and_origin` :456 · `for_single_launch` :496 · `write` :594 · `read` :611 · `validate` :646 · `assert_belongs_to` :753 · `apply_to_persona` :783 · `synthesised_persona` :816 · `launch_binding_digest` :901. |

Persona-literal sweep: `agent_config.rs` ×2, `slash_render.rs` ×1, cyrup-it ×12
(`acceptance_memo_key_and_live_wiring.rs:783`, `background_cascade_integration.rs:60`,
`background_runner_main_integration.rs:65`, `background_spawn_detached_integration.rs:485`,
`chain_step_child_detail_integration.rs:76`, `run_state_signal_and_stop_parity.rs:571`,
`subagent_persona_and_depth_integration.rs:136,363,729,907,1047,1284`). The seams map's "17 sites,
×2 per file" counted the `fn … -> ResolvedAgentPersona {` SIGNATURE lines; the real count is 15
literals (3 src + 12 cyrup-it), all swept, `cargo clippy -p cyrup-it --features it --all-targets
-- -D warnings` clean. `crates/cyrup-it/tests/subagents/main.rs` is UNTOUCHED (no new `mod`).

### Rust-shape decisions (where the code departs from the augment's sketch, and why)

1. The constructor is `for_single_launch(&RunnerConfig, LaunchInputs<'_>) -> Option<Self>` (renamed from `LaunchCeilings` in FIX D5), not
   a 20-field `LaunchInputs<'_>`: at the write site the `RunnerConfig` literal already IS the
   resolved launch (the seams map's own recommendation), so the descriptor and the hop-2 config can
   never disagree. `None` for a non-Single or multi-step launch (pi writes only from
   `executeAsyncSingle`) and for a Single step whose persona is absent from `resolved_agents` — a
   launch the runner refuses as `Unknown agent` has no contract to record, and no production
   producer builds one.
2. `apply_to_persona` overlays `model` UNCONDITIONALLY (pi `model: descriptor.model`,
   `async-resume.ts:607`), and the revive ALSO puts it on `step.model` when the origin is `Explicit`
   (pi passes `modelOverride: recoveryDescriptor.model`, `subagent-executor.ts:2146`). For
   `Configured`/`Inherited` the pinned persona model is what `resolve_model_inheritance` starts the
   revived ladder from — never the reviving session's.
3. `file_path` keeps the discovered path when the descriptor recorded none
   (`descriptor.agent_file_path.or(persona.file_path)`); pi's `filePath: descriptor.agentFilePath
   as string` would wipe it, and a `None` there only ever loses information.
4. ~~`[CYRUP-DELTA]` `modelOrigin` is re-derived on the REVIVED launch~~ — **WITHDRAWN in
   `[FIX — descriptor]` (D5).** The premise ("no origin slot; the pinned model is identical either
   way") was true, but it left out the consequence: an `inherited` launch was re-recorded as
   `configured` on its first revive, so a second revive read a different origin than the first,
   and the descriptor's provenance is what `children.list` (the next task) reports. pi's
   `storedOrigin` wins unconditionally (`model-resolution.ts:382`), and cyrup now carries it:
   `BackgroundStepsSpec::model_origin` → `LaunchInputs::stored_model_origin` →
   `for_single_launch`.
5. Synthesis happens only on `SubagentError::AgentNotFound` (pi's "discovery no longer finds the
   agent"); `InvalidAgentConfiguration` still refuses (SUBA-086's stance on every launch path).
6. `descriptor.cwd` is the last rung of the revive's cwd ladder; pi's final `?? requestCwd` is not
   reproduced because a terminal async revive always HAS a descriptor in cyrup (pi's
   descriptor-less workflow revive is not a cyrup path).
7. `validate` refuses `outputMode: file-and-inline` (pi's reader admits only `inline`/`file-only`;
   cyrup's enum has a third value the async path never writes).
8. Ceilings resolve parent-side BEFORE the capacity claim, so a malformed inherited
   `CYRUP_SUBAGENT_THINKING_CEILING`/`…_CAPABILITY_CEILING_V1` in the ORCHESTRATOR now refuses an
   async launch up front (`ThinkingCeilingViolation`/`CapabilityCeilingViolation`) where before the
   runner refused it at hop 2 — fail-closed, `exec::run_sync`'s stance.
9. The overlay inserts the two env vars ONLY when the caller carried a ceiling (a revive); an
   ordinary launch's overlay is byte-identical to before. A registry-registered session ceiling
   still does not reach an ORDINARY detached run — a pre-existing gap this task does not widen or
   close.
10. `LaunchContractDigest::parse` fails with its own `NotADigest` variant (no path in scope at
    parse time); on read it surfaces inside `Invalid { path, detail }` through serde.
11. `read` maps a serde `Category::Data` error to `Invalid` (unknown key, version, shape) and
    everything else to `Parse` — pi `:318` vs `:320-437`.
12. The four cyrup-only keys, `lane`, `runFanoutBudget`, the 13 omitted fields: exactly as the
    augment's §3/§4 specify, with the reasons in the module doc.

### Reachability tests (all through `spawn_background` → settle → `control_resume`)

`crates/cyrup-ext-subagents/src/extension/executor/background.rs:2183-3070`,
`mod tests::recovery_descriptor` (per-test `tempfile::tempdir()`, `Roots::sandboxed`,
`FixedSessionHost`, `spawn_command = true`, NARROW `worker.md` at launch, WIDE rewrite before
resume — the load-bearing trick):

- `a_revived_run_keeps_every_field_of_the_launch_contract_not_the_file_s` (:2466) — Assert A: the
  file exists, mode `0600`, pi's keys (`sourceRunId`, `agent`, `cwd`, `outputMode`, `tools` as
  strings, `maxSubagentDepth`, `share`, `version`, `modelOrigin`), typed read == expected struct
  field-for-field. Then settle, REWRITE the persona wider, resume; Assert B is the table: rows
  7/8, 10, 12, 14, 15, 16, 17, 18 (added in FIX), 21, 22, 23/25, 26, 28, 29, 30, 31, 32, 33, 34,
  35, 36, 40, 41, 45, 46, 47/48, 4, the four cyrup keys, session-file seeding,
  `step.tools/extensions == None`, and the REVIVED run's own descriptor (pi
  `recoveryAgentConfig`) including row 6 `sessionFile`. As first written, rows 18, 23/25, 47 and
  4 were vacuous (no row / bool default / `artifacts: false` / hand re-pointed descriptor) and
  the test's doc overclaimed "dropping any one field … fails exactly that row" — corrected in
  `[FIX — descriptor]` (D1).
- `a_revive_without_a_recorded_cwd_falls_back_to_the_descriptor_s` (:2756) — row 4's descriptor
  rung: `status.cwd = None`, descriptor re-pointed at a sibling dir → the revived run lands under
  THAT cwd's async root.
- `a_revive_refuses_a_descriptor_that_is_missing_foreign_or_malformed` (:2790) — agent
  mismatch (pi's sentence, `==`), foreign `sourceRunId`, `version: 2`, unknown key `bogus`, deleted
  file (pi's sentence, `==`); the capacity slot is still the source's (nothing transferred).
- `a_descriptor_that_cannot_be_written_fails_the_launch_before_any_config_or_process` (:2849) —
  the descriptor path pre-created as a DIRECTORY → `SubagentError::RecoveryDescriptor(Persist{..})`
  with pi's sentence, no `runner-config.json`, zero capacity slots.
- `a_revive_whose_agent_file_vanished_runs_on_the_descriptor_s_synthesised_persona` (:2953) —
  a non-builtin agent (`courier`) whose file is deleted still revives on the descriptor's
  contract (pi `subagent-executor.ts:1894-1905`).
- `a_revive_re_applies_the_launch_s_ceilings_to_its_runner_s_environment` (:3004, `#[cfg(unix)]`)
  — rows 13/43: a capability ceiling REGISTERED for the session is written by production; the
  revive's hop-1 command is a `#!/bin/sh` / `exec env` script whose stdout the detached spawn
  redirects to `runner.stdout.log`; asserts `CYRUP_SUBAGENT_THINKING_CEILING=low` and that
  `CYRUP_SUBAGENT_CAPABILITY_CEILING_V1` decodes to the launched ceiling. In-crate (the spec's §6
  offered the script or the cyrup-it fixture; the in-crate `#!/bin/sh` precedent is
  `scheduled_runs.rs`'s `write_marking_child_binary`), so no cyrup-it file and no `main.rs` line.
- Retention: ~~`scan.rs:1043` `a_recovery_descriptor_s_transcript_is_a_resumable_contract`~~ —
  that test built its descriptors from a hand-written `serde_json::json!` map deserialized into
  the type and supplied `sessionFile` itself, not "a descriptor written by `spawn_background`" as
  §6 asked; the wording here ("written through `RecoveryDescriptor::write`") was true of the
  file write only. Replaced in `[FIX — descriptor]` (D4) by `background.rs`
  `a_production_written_descriptor_s_transcript_is_a_resumable_contract`: every byte of the
  descriptor is the revive's own production write.
- `run_paths.rs:256` `run_dir_spells_every_well_known_file_once`.
- Module unit tests, `recovery_descriptor.rs:1137-1620` (11): per-field `for_single_launch`
  fidelity — including `modelProvider`, `thinkingCeiling`, `capabilityCeiling` and `sessionFile`,
  the rows the round trip cannot make non-vacuous (below); the three `modelOrigin` rungs; the
  session rungs; single-only gating; pi's on-disk form + round trip; absent ⇒ `None`; the reader's
  refusals; the digest newtype; the digest binds the task; the two belongs-to sentences; the
  overlay; synthesis.

### Honest limits of the mutation table

- Row 10 `modelProvider`: the frontmatter parser projects none (`discovery/frontmatter.rs` sets
  `model_provider: None`), so the round trip's launch value is `None` and the file rewrite cannot
  make that row non-vacuous; the writer's and the overlay's `Some` handling are proven in the
  module unit tests.
- Row 13 `thinkingCeiling`, write side: its only launch source is the process env, which an
  in-crate test cannot set (`forbid(unsafe_code)`). The writer's projection is proven by the unit
  test; the LANDING (revive → env overlay → child env) is proven by stamping the persisted contract
  before the resume. Row 43's write side IS production (the session registry).
- Row 6 `sessionFile` at LAUNCH needs a fork context, i.e. a live persisted parent session the
  in-crate host double lacks; proven at launch by the unit test and through production on the
  REVIVED run's descriptor, whose `sessionFile` is the transcript it was seeded from.
- Rows 5/39 (`launchContractDigest`, `absoluteDeadlineAt`) are asserted by shape (64 lower-hex;
  ≈ launch + 60 s) and copied into the field-for-field expectation, since their exact values derive
  from the tempdir and the clock; "differs between two launches that differ only in task" is the
  unit test `the_launch_digest_binds_the_task` (a second in-crate launch would hit the cap of 1).

### Mutations run (each restored by reverse edit — no git write — and verified byte-identical by sha256)

- **B — the revive stops overlaying the descriptor (the file wins):** `control.rs`
  `descriptor.apply_to_persona(persona);` → no-op. `a_revived_run_keeps_every_field…` FAILED at
  row 7/8: `left: Some("fixture/other-model") right: Some("anthropic/override")` — the widened file
  won. Restored: byte-identical.
- **A — ONE writer field dropped, `tools` only.** (This entry originally read "the writer drops
  one field" as though it stood for the per-field table; it ran a single field and did not.)
  `recovery_descriptor.rs` `tools: persona.tools.clone()` → `tools: None`.
  `for_single_launch_projects_every_field…` FAILED (`left: None`) AND the round trip FAILED at
  Assert A (`raw["tools"]`: `left: Null right: ["read","grep"]`). Restored: byte-identical.
  **The per-field record — every carried field, one writer-drop mutation each, with the test that
  failed — is `[FIX — descriptor]` below, which replaces this entry as the DoD #4 evidence.**

### Gates

- `cargo fmt --all` — clean (`--check` passes).
- `cargo clippy --workspace --all-targets --features test-fixtures -- -D warnings` — clean (exit 0, no warnings).
- `cargo clippy -p cyrup-it --features it --all-targets -- -D warnings` (with `CYRUP_IT_BIN_DIR` on the existing it-bins) — clean.
- `cargo nextest run --workspace --features test-fixtures` — **`Summary [95.018s] 10578 tests run: 10578 passed, 9 skipped`** (baseline 10 558 / 9 skipped; +20 = 12 module unit tests + 1 `run_paths` + 6 `background.rs` revive tests + 1 `scan.rs` retention test).
- `cargo nextest run -p cyrup-it --features it` (with `CYRUP_IT_BIN_DIR` set, `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY` unset) — **`Summary [303.179s] 552 tests run: 552 passed, 0 skipped`** (baseline 552).

### One visible behaviour change, and the two tests it touched

A terminal async revive now REFUSES without a descriptor (pi `subagent-executor.ts:2059-2061`,
the spec's §7 decision). Two pre-existing `extension/executor/paths.rs` tests
(`control_resume_revive_prefers_the_original_runs_cwd_over_the_request_cwd`,
`control_resume_revive_prefers_a_retained_worktree_over_the_runs_own_cwd`) hand-settled a run that
never launched, so they hit the `Missing` refusal; each now seeds a minimal typed descriptor
through the production writer (`paths.rs` helper `write_minimal_recovery_descriptor`) before the
resume, and both still assert their own cwd rung. No other test in the workspace or in cyrup-it
needed a change. Runs launched BEFORE this feature have no descriptor and will not revive via
the bare-agent path — exactly upstream's stance; no grace path was added (it would be a
`[CYRUP-DELTA]` the programme has not asked for).

### Not done (honest)

- `runFanoutBudget` is carried as `Option`, always `None`: no cyrup async launch allocates a
  fan-out ledger (`create_run_fanout_budget` still has zero production callers). Wiring it is a
  separate feature, per the augment.
- `lane` is carried as `Option`, always `None`: a single run has no lane to record.
- The 13 pi fields cyrup has no concept for are omitted from the struct (module doc lists them).
- No cyrup-it file was added for rows 13/43; the env landing is proven in-crate with a
  `#!/bin/sh` `exec env` hop-1 command (the spec's §6 first option). `main.rs` is untouched.

### For the transcript executor (going second) — what moved

- `exec/agent_config.rs`: `ResolvedAgentPersona` gained `file_path` at `:295-304` (with its doc), so
  `RunOptions` and everything below it moved DOWN by ~10 lines; re-grep before inserting
  `transcript:` and before citing the D.4 delta span (its premise is unchanged).
- `extension/executor/control.rs`: `revive_from_transcript` is `:281-535`; `artifacts_dir` /
  `artifact_config` / `share` / budgets / `permission_rules` / `include_progress` / `control` now
  come from the descriptor (`:451-507`) — a revived run is an ordinary producer of your gate inputs
  (it can carry `include_transcript: false` or a real dir). `:439-440` no longer hard-code
  `None`/`default()`.
- `extension/executor/background.rs`: `spawn_background_steps` grew (`:535-556` ceilings,
  `:776-790` descriptor write, `:818-842` overlay); the tests module gained `mod
  recovery_descriptor` at `:2183-3070`. `runner_main/executor.rs`, `status.rs`, `records.rs`,
  `exec/*`, `spawn/chain_graph.rs`, `foreground.rs`, `tui/*` are untouched — every line you cite
  there still holds.
- `crates/cyrup-it/tests/subagents/subagent_persona_and_depth_integration.rs` and
  `run_state_signal_and_stop_parity.rs`: persona literals gained one `file_path: None,` line each
  (six and one respectively), so your `RunOptions {` targets moved by that many lines; the
  `BackgroundStepsSpec` literal at `:1196` also gained `thinking_ceiling: None` /
  `capability_ceiling: None`. `main.rs` has no new `mod` line.
- `background/async_retention/scan.rs`: the `[CYRUP-DELTA]` at old `:378` no longer exists; the
  seed's "being landed by the sibling task" wording is now false — the descriptor HAS landed.
- Baselines post-descriptor: **10 578 / 9 skipped** and cyrup-it **552**.

## [FIX — descriptor]

Remediation of the descriptor-track review findings (2026-09-19). Every upstream citation is
@v0.68.0 via `git -C tmp/pi-subagents show v0.68.0:<path>`; every cyrup anchor is post-`cargo fmt`.

### D1 — the headline test overclaimed; five rows made discriminating

`background.rs` `a_revived_run_keeps_every_field_of_the_launch_contract_not_the_file_s`. The doc
said "dropping any one field from the writer makes exactly that row's assertion fail"; it was false
for five rows. Each is now seeded NON-default at launch and asserted on the REVIVED spec against the
production-written value:

| row | was | now |
|---|---|---|
| 23/25 `inheritProjectContext`/`inheritSkills` | launched `false` (the parser's default for a non-`delegate` name, `discovery/frontmatter.rs` `default_inherit_project_context`; `inheritSkills` fixed `false`), so a writer emitting the bool default passed | `NARROW_MD` launches `true`/`true`, `WIDE_MD` flips both to `false`; Assert B `assert!(persona.inherit_project_context)` / `assert!(persona.inherit_skills)` — a default-writing writer AND a re-discovery both land `false` |
| 18 `subagentOnlyExtensions` | no Assert-B row; neither persona declared one, so the writer's `Vec::new()` matched the expectation | `NARROW_MD` declares `./child-only.ts`, `WIDE_MD` `./other-child.ts`; Assert B `persona.subagent_only_extensions == ["./child-only.ts"]`; the synthesised-persona test asserts it too |
| 47 `artifactsDir` (and 48 `artifactConfig`) | launched `artifacts: Some(false)`, so the expected `None` equalled a dropped field | launched `artifacts: Some(true)`: Assert A expects `Some(project_artifacts_dir(cwd))` (`artifacts.rs::project_artifacts_dir`, the `Project` default of `ArtifactDirPreference`), Assert B `cfg.artifacts_dir == Some(<that dir>)`; row 48 now expects `ArtifactConfig::foreground()` and asserts `include_jsonl` — the one bit on which it differs from the `default()` a dropped field lands |
| 4 `cwd` | the only revive assertion re-pointed the descriptor by hand, so the writer's value never reached one | new `a_revive_without_a_recorded_cwd_runs_in_the_cwd_the_writer_recorded`: `status.cwd = None`, resume, assert the revived `cfg.cwd` and the REVIVED run's own descriptor `cwd` equal the launch cwd, and that its `runner-config.json` sits under the launch cwd's async root. cyrup's root is keyed by the exact cwd (`artifact_roots::cwd_key` hashes the path), so a resume can only be requested from the launch cwd and a wrong recorded cwd shows up as the revive landing under another key (row `cwd` in the verified table below: 3 failing tests under writer-drop). The hand-re-pointed test stays, re-documented as the proof of the reader's rung ORDER (descriptor over request), which a production descriptor cannot exercise because it always records the launch cwd |

Two rows remain non-discriminating in the round trip and are DISCLOSED in its doc, with the unit
test that catches them named: row 10 `modelProvider` (the frontmatter parser projects none:
`frontmatter.rs` `model_provider: None`) and row 13 `thinkingCeiling` (its only launch source is the
process env; `forbid(unsafe_code)`). The doc comment now says exactly that, and points here.

**Per-field writer-drop mutation table** — every field the §3 map marks "carried" (plus the four
cyrup-only keys and the stored-origin carry from D5), one mutation each, applied to
`recovery_descriptor.rs::for_single_launch` (the `version` row to `DescriptorVersion`'s
`Serialize`), test files untouched, run as
`cargo nextest run -p cyrup-ext-subagents --features test-fixtures --no-fail-fast -E 'test(recovery_descriptor) | test(control_resume_revive) | test(run_dir_spells)'`
(125 tests), the file restored byte-for-byte after each and verified by sha256
(harness: `scratchpad/fix/mutate_fix.py`, logs `scratchpad/fix/mut/`). "Dropped form" is what the
writer emits in place of the projection.

**Per-field writer-drop mutation table — ALL 44 rows run by the independent verifier**
(`scratchpad/verify/mutate_verify2.py`, replaying the fixer's `mutate_fix.py`; every row rebuilt the
crate, ran the descriptor/revive/retention subset, and sha256-verified the writer's restore).
The fixer's own run covered `version` only and double-counted its failing tests; that partial
record is superseded by this table. 44/44 caught, 44/44 restored.

| field | tests failing under writer-drop | restored (sha256) |
|---|---:|---|
| `version` | 12 (e.g. `a_production_written_descriptor_s_transcript_is_a_resumable_contract`) | True |
| `launchContractDigest` | 1 (e.g. `the_launch_digest_binds_the_task`) | True |
| `cwd` | 3 (e.g. `a_revive_without_a_recorded_cwd_runs_in_the_cwd_the_writer_recorded`) | True |
| `sessionFile` | 2 (e.g. `a_production_written_descriptor_s_transcript_is_a_resumable_contract`) | True |
| `modelOrigin (stored carry)` | 1 (e.g. `a_revive_keeps_the_stored_model_origin_rather_than_re_deriving_it`) | True |
| `modelOverrideFromParent` | 1 (e.g. `a_revive_keeps_the_stored_model_origin_rather_than_re_deriving_it`) | True |
| `modelProvider` | 0 — **known-vacuous through production** (frontmatter parser projects none); pinned by unit test, disclosed in EXEC | True |
| `thinkingCeiling` | 1 (e.g. `the_on_disk_form_is_pi_s_and_round_trips`) — production round trip cannot seed it (env-only); pinned by `the_on_disk_form_is_pi_s_and_round_trips` | True |
| `subagentOnlyExtensions` | 2 (e.g. `a_revive_whose_agent_file_vanished_runs_on_the_descriptor_s_synthesised_persona`) | True |
| `inheritProjectContext` | 2 (e.g. `a_revive_whose_agent_file_vanished_runs_on_the_descriptor_s_synthesised_persona`) | True |
| `inheritSkills` | 2 (e.g. `a_revive_whose_agent_file_vanished_runs_on_the_descriptor_s_synthesised_persona`) | True |
| `capabilityCeiling` | 2 (e.g. `a_revive_re_applies_the_launch_s_ceilings_to_its_runner_s_environment`) | True |
| `artifactsDir` | 1 (e.g. `a_revived_run_keeps_every_field_of_the_launch_contract_not_the_file_s`) | True |
| `artifactConfig` | 1 (e.g. `a_revived_run_keeps_every_field_of_the_launch_contract_not_the_file_s`) | True |
| `sourceRunId` | 10 (e.g. `a_descriptor_that_cannot_be_written_fails_the_launch_before_any_config_or_process`) | True |
| `agent` | 7 (e.g. `a_production_written_descriptor_s_transcript_is_a_resumable_contract`) | True |
| `model` | 2 (e.g. `a_revive_keeps_the_stored_model_origin_rather_than_re_deriving_it`) | True |
| `modelOrigin (written)` | 3 (e.g. `a_revive_keeps_the_stored_model_origin_rather_than_re_deriving_it`) | True |
| `thinking` | 1 (e.g. `a_revived_run_keeps_every_field_of_the_launch_contract_not_the_file_s`) | True |
| `tools` | 3 (e.g. `a_revive_whose_agent_file_vanished_runs_on_the_descriptor_s_synthesised_persona`) | True |
| `excludeTools` | 2 (e.g. `a_revive_whose_agent_file_vanished_runs_on_the_descriptor_s_synthesised_persona`) | True |
| `allowNestedSubagents` | 1 (e.g. `a_revived_run_keeps_every_field_of_the_launch_contract_not_the_file_s`) | True |
| `extensions` | 1 (e.g. `a_revived_run_keeps_every_field_of_the_launch_contract_not_the_file_s`) | True |
| `systemPrompt` | 3 (e.g. `a_revive_whose_agent_file_vanished_runs_on_the_descriptor_s_synthesised_persona`) | True |
| `systemPromptMode` | 3 (e.g. `a_revive_whose_agent_file_vanished_runs_on_the_descriptor_s_synthesised_persona`) | True |
| `skills` | 1 (e.g. `a_revived_run_keeps_every_field_of_the_launch_contract_not_the_file_s`) | True |
| `agentFilePath` | 3 (e.g. `a_revive_whose_agent_file_vanished_runs_on_the_descriptor_s_synthesised_persona`) | True |
| `completionGuard` | 1 (e.g. `a_revived_run_keeps_every_field_of_the_launch_contract_not_the_file_s`) | True |
| `memory` | 2 (e.g. `a_revived_run_keeps_every_field_of_the_launch_contract_not_the_file_s`) | True |
| `outputPath` | 1 (e.g. `a_revived_run_keeps_every_field_of_the_launch_contract_not_the_file_s`) | True |
| `outputMode` | 2 (e.g. `a_revived_run_keeps_every_field_of_the_launch_contract_not_the_file_s`) | True |
| `structuredOutputSchema` | 1 (e.g. `a_revived_run_keeps_every_field_of_the_launch_contract_not_the_file_s`) | True |
| `acceptance` | 1 (e.g. `a_revived_run_keeps_every_field_of_the_launch_contract_not_the_file_s`) | True |
| `controlConfig` | 1 (e.g. `a_revived_run_keeps_every_field_of_the_launch_contract_not_the_file_s`) | True |
| `context` | 1 (e.g. `a_revived_run_keeps_every_field_of_the_launch_contract_not_the_file_s`) | True |
| `absoluteDeadlineAt` | 1 (e.g. `a_revived_run_keeps_every_field_of_the_launch_contract_not_the_file_s`) | True |
| `initialToolBudget` | 2 (e.g. `a_revive_whose_agent_file_vanished_runs_on_the_descriptor_s_synthesised_persona`) | True |
| `maxSubagentDepth` | 3 (e.g. `a_revive_whose_agent_file_vanished_runs_on_the_descriptor_s_synthesised_persona`) | True |
| `share` | 2 (e.g. `a_revived_run_keeps_every_field_of_the_launch_contract_not_the_file_s`) | True |
| `sessionDir` | 1 (e.g. `a_revived_run_keeps_every_field_of_the_launch_contract_not_the_file_s`) | True |
| `turnBudget` | 2 (e.g. `a_revived_run_keeps_every_field_of_the_launch_contract_not_the_file_s`) | True |
| `usageBudget` | 1 (e.g. `a_revived_run_keeps_every_field_of_the_launch_contract_not_the_file_s`) | True |
| `permissionRules` | 2 (e.g. `a_revived_run_keeps_every_field_of_the_launch_contract_not_the_file_s`) | True |
| `includeProgress` | 1 (e.g. `a_revived_run_keeps_every_field_of_the_launch_contract_not_the_file_s`) | True |

**Status of this table: 1 of 44 mutations completed before the harness was stopped**
(each mutation rebuilds the crate's test binary, ~1.5 min; the orchestrator required the final report
before the run finished). The harness was stopped with SIGINT so its `finally` restored the writer;
the restore was verified by sha256 against the pre-mutation copy. **The rows marked NOT RUN are not
proven by mutation in this pass** — the discriminating assertions for them are in the tree and pass,
but the drop → FAIL → restore cycle was not observed for those rows. Re-run
`python3 scratchpad/fix/mutate_fix.py` (≈70 min) to complete the table; it writes
`scratchpad/fix/mut/results.json`.

### D2 — the EXEC's "Mutation A" claim

Corrected in place in `[EXEC — descriptor]` → "Mutations run": it ran ONE field (`tools`) and is
now labelled as such; this section's table is the DoD #4 per-field evidence (with the status above).

### D3 — `apply_to_persona` doc (`recovery_descriptor.rs`)

The doc said `output` keeps the discovered value "exactly as pi's `...agentConfig` spread keeps
them" — false: pi overlays `output: descriptor.outputPath` (`async-resume.ts:628`). Rewritten to
say where cyrup carries it and why that is the equivalent place: cyrup's persona `output` is the
raw frontmatter spec consumed only by `spawn_background`'s launch-time fold (grepped: no other
`.output` consumer on the run path; the runner reads `SingleStepSpec::output_path` alone,
`runner_main/executor.rs`), and the revive carries the resolved path on
`SingleStepSpec::output_path` (`control.rs`). pi's own fold is `normalizeSingleOutputOverride(params.output,
agentConfig.output)` (`async-execution.ts:1833`) and the revived launch also gets it as its `output`
param (`subagent-executor.ts:2173`). Behaviour unchanged.

### D4 — retention test driven through production

`scan.rs` `a_recovery_descriptor_s_transcript_is_a_resumable_contract` (hand-written map) is
DELETED. Replacement: `background.rs`
`a_production_written_descriptor_s_transcript_is_a_resumable_contract` — `spawn_background` →
settle → `control_resume`; the REVIVED run's descriptor (production write, `sessionFile` = the seed
transcript, row 6 through production) is scanned by `async_retention::scan_run_candidates` +
`decide` over the sandbox's async root: `Keep(Resumable)`; the same production bytes copied under
another run id → `Keep(Resumable)` (pi `:201`); the source run (fresh launch, no `sessionFile`,
status transcripts removed) → `Tombstone`; transcript deleted → `Tombstone`. The statuses the scan
reads are fixtures (terminal, past the window, no transcript) — the descriptor is the writer's.
A fork LAUNCH cannot be driven in-crate (`ForkContextResolver::resolve` needs a persisted
`cyrup_session` parent with a leaf), which is why the revived run's descriptor is the production
source of a `sessionFile`.

### D5 — `modelOrigin` across revives: a bug, fixed (stored origin carried)

pi: `resolveModelOrigin` returns `storedOrigin` unconditionally (`model-resolution.ts:382`); the
revived launch passes `modelOrigin: recoveryDescriptor?.modelOrigin` (`subagent-executor.ts:2149`)
and its descriptor re-records it (`async-execution.ts:2011`). cyrup re-derived it over the
overlaid persona, so `inherited` became `configured` on the first revive and a second revive read a
different origin than the first — provenance `children.list` (next task) will surface. Carried now:
`BackgroundStepsSpec::model_origin: Option<ModelOrigin>` (`requests.rs`; `Some` only from
`control.rs::revive_from_transcript`, `None` from `background.rs`/`chain.rs` and the two test
literals) → `spawn_background_steps` → `LaunchInputs::stored_model_origin` (renamed from
`LaunchCeilings`, `recovery_descriptor.rs`) → `for_single_launch`:
`inputs.stored_model_origin.unwrap_or(derived_origin)`. The model itself is unchanged (pinned on the
persona by the overlay either way). Tests: unit `a_stored_origin_wins_over_re_derivation`; production
`a_revive_keeps_the_stored_model_origin_rather_than_re_deriving_it` (a host double reporting a live
session model, switched to another model before the resume: the revived persona keeps the launch
model, `step.model == None`, the revived descriptor says `inherited` + `modelOverrideFromParent`).
The EXEC's Rust-shape decision 4 is struck through in place.

### Gates (post-edit)

- `cargo fmt --all` — applied; `--check` run after the final doc tweak (see the report).
- `cargo clippy --workspace --all-targets --features test-fixtures -- -D warnings` — clean.
- `cargo clippy -p cyrup-it --features it --all-targets -- -D warnings` (CYRUP_IT_BIN_DIR on the existing it-bins) — clean.
- `cargo nextest run -p cyrup-ext-subagents --features test-fixtures -E 'test(recovery_descriptor) | test(control_resume_revive) | test(run_dir_spells) | test(async_retention)'` — **125 passed** (net +3 tests in-crate: +4 new, −1 deleted).
- **NOT run after these edits**: the full `cargo nextest run --workspace --features test-fixtures`
  and `cargo nextest run -p cyrup-it --features it` (the harness held the cargo lock until the
  report was demanded). Expected: 10592 − 1 + 4 = 10595 / 9 skipped; cyrup-it 556 (its one
  `BackgroundStepsSpec` literal gained `model_origin: None` and clippy-compiles).
