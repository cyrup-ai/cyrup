---
stage: augment
status: ready
updated: 2026-09-23 00:00
aug_against: branch claude/subagents-delegation HEAD 521beaa — every cyrup site below re-read at this HEAD; upstream pinned at pi-subagents v0.68.0 (git -C tmp/pi-subagents show v0.68.0:<path>) unless another tag is named
---

# SILENT_DROPS: the subagent surface accepts a key and does nothing with it

OBJECTIVE: every key that upstream declares on a subagent config surface (`agentOverrides.<name>`,
`<agent_dir>/subagents/config.json`, agent frontmatter) is either **typed and consumed**, or
**named in an "unported" table and reported when someone sets it**. Nothing is accepted and
then discarded without a word. That is one rule applied to six ledger rows plus the siblings this
pass found. The rows are verified first, because the last sweep found that about 80% of this
ledger's open rows were already closed or rested on a false premise.

> **READ §0 FIRST.** Two rows are closed or not what they say. Two rows' Fix lines would **move the
> silent drop down one layer instead of removing it**. And two new instances of the same failure
> turned up that no row owns yet.

---

## 0. Verdicts at a glance

| Row | Still open? | Premise still right? | Size | Class |
|---|---|---|---|---|
| **SUBA-096** `acceptanceRole`/`outputMode`/`fast` overrides | **yes** | **Partly.** Yes, the three keys are dropped. But the Fix ("struct + three apply arms, S") is wrong for two of them: `outputMode` and `fast` have **no consumer anywhere**, so adding them to the struct only moves the drop one layer down. It also says `outputMode` takes `false` as a clear. It does not: upstream **throws** on `false`. | **M–L** (acceptanceRole S; outputMode S+; fast M) | silent drop |
| **SUBA-061** `asyncWidget`/`inlineToolDisplay`/`fleetKeybindings` | **yes**, all three | **Yes.** The fleet roster from PR #151 does **not** touch `fleetKeybindings`: the key drives the full inspector, not the roster. Its Fix citations (`extension.rs:9489/…`) no longer exist; re-derived below. | **M** | silent drop |
| **PB-14** skills-not-found warning | **yes**, both surfaces | **Yes**, and its 2026-09-22 citations still resolve. | **S** | silent drop |
| **SUBA-063** zero-budget auth / ack path | **(a) not a silent drop. (b) open, but not a silent drop. (c) open, and a silent drop.** | **(a) is false as framed.** `hard: 0` is loudly refused on every surface cyrup has, which matches upstream. The only upstream producer of the authorisation is the prompt-template delegation bridge (= **SUBA-022**), and the env var the child reads was **removed upstream at v0.67.0**. | (a) 0 now, rides SUBA-022; (b) M; (c) S | mixed |
| **SUBA-097** false integration-test claim | **NO. CLOSED at `d00a314` (2026-09-17)** | The test was written and the comment was corrected. The ledger row is stale. | 0 (ledger edit) | — |
| **SUBA-098** stray `观察` | **yes** | **Yes**, and `cyrup-tui/src/tests/selector.rs:41` `末` is still there as well. | XS | cosmetic |

**New, same class, no owning row** (§7):
1. **`config.json` validators with no caller.** `SubagentExtensionConfig::validate_authority_policy`,
   `validate_artifact_dir` and `validate_artifact_config` (`registration/mod.rs:594,654,686`) each
   say they "must be refused at config load". The loader (`crates/cyrup/src/subagent_config.rs:58-69`)
   calls **only** `validate_missions`. An unknown `authorityPolicy` action key (a typo like
   `stopRuns`) is silently dropped, because `AuthorityPolicyConfig`
   (`registration/authority.rs:180-195`) has no `deny_unknown_fields`.
2. **The interactive async-jobs widget (C21) is ported but unwired.** `render_async_jobs_widget`
   (`tui/events.rs:865`) has one caller, its own test (`:1028`). Its module doc (`tui/events.rs:44-52`)
   says it "CANNOT BE" wired from this crate because `cyrup-ext` has no `set_widget`. That is false:
   `HostServices::set_widget(key, lines, placement)` exists (`cyrup-ext/src/host/services.rs:423`),
   and PARITY-GAPS UW-21 already struck the same false blocker for the RPC half. This is what
   `asyncWidget` is meant to toggle in interactive mode (§2.2).
3. v0.68.0 declares **three more** `agentOverrides` keys cyrup drops the same way: `machine`,
   `inheritGlobalContext` and `mutationTools` (§1.5). `09a`'s SUBA-096 records them as
   "out of scope, unmeasured window". They are the same silent drop and belong in the same
   diagnostic.
4. **The custom-agent override path still applies v0.57.0's frontmatter gate.** Upstream removed
   that gate at `31562d76` (v0.64.0). Recorded as a SUBA-092 residual with no open id. See §1.6.

---

## 1. SUBA-096: `acceptanceRole`, `outputMode` and `fast` in `agentOverrides`

### 1.0 Verdict

**Open.** `AgentOverrideConfig` (`discovery/types.rs:692-810`) has 21 fields and none of the three.
It is `#[serde(rename_all = "camelCase", default)]` (`:692-693`) with no `deny_unknown_fields`.
Its own doc (`:627-636`) re-counts the census at v0.68.0 as "26 upstream / 20 modeled / 6
unmodeled".

**The pinned test encodes the drop as a feature.** `parse_subagent_settings_accepts_every_pi_override_key`
(`discovery/mod.rs:2440-2470`) feeds `"acceptanceRole": "reviewer"`. That value is **invalid**
upstream: `agents.ts:1080-1083` accepts only `"read-only" | "writer" | false` and **throws**
otherwise. The test passes only because the key is discarded. It also feeds `"fast": true` and
`"outputMode": "file-only"` and asserts nothing about either. The moment `acceptance_role` is
modeled, this test goes red. That is correct behaviour. Change the fixture to `"writer"`.

### 1.1 What upstream does (v0.68.0)

Interface `src/agents/agents.ts:85-112`: `outputMode?: OutputMode` at `:89`, **no `| false`**;
`fast?: boolean` at `:93`; `acceptanceRole?: AcceptanceRole | false` at `:100`.

| Key | Parse (`parseBuiltinOverride`) | Apply (`applyBuiltinOverride`) | Consumers |
|---|---|---|---|
| `outputMode` | `:1016-1021`. Only `"inline"`/`"file-only"`; anything else, **including `false`**, throws `Builtin override '<n>' in '<path>' has invalid 'outputMode'; expected 'inline' or 'file-only'.` | `:1433` plain assign | `params.outputMode ?? agent.outputMode ?? "inline"` at **five** sites: `runs/foreground/subagent-executor.ts:3346` (parallel), `:3861` (single); `runs/background/async-execution.ts:1836` (async single); `runs/shared/child-launch-plan.ts:105` (chain step); `shared/settings.ts:390` (chain task). First present at v0.57.0 (`subagent-executor.ts:3601`). |
| `fast` | `:1029-1032`. Must be a boolean, else throws `… has invalid 'fast'; expected a boolean.` | `:1443` plain assign | `params.fast ?? agent.fast` (per step: `s.fast ?? params.fast ?? a.fast`). `runs/foreground/execution.ts:416,469`; `api/preflight.ts:402`; `runs/background/async-execution.ts:983,1118,1943,1967`. The effect is in `runs/shared/child-tool-plan.ts`: `FAST_MODE_ALLOWED_MODELS = {openai-codex/gpt-5.6-luna, openai-codex/gpt-5.6-sol}` (`:60-63`); `resolveFastModeExtension` (`:151-163`) throws `fast mode requires an explicit supported native OpenAI-Codex model for agent '<n>'.` / `fast mode supports only …; unsupported model(s): …`; `:494` throws when the capability ceiling denies extensions; `:495-498` loads `runs/shared/fast-mode-extension.ts`, whose entire body (`:3-10`) is `pi.on("before_provider_request", e => ({...e.payload, service_tier: "priority"}))`. External-CLI runners refuse it: `subagent-executor.ts:3353-3355`, `async-execution.ts:1012,1756`. Tool param on four schemas: `extension/schemas.ts:167` (ParallelTask), `:199` (DynamicParallelTemplate), `:229` (ChainItem), `:388` (top level). |
| `acceptanceRole` | `:1079-1084`. `"read-only" \| "writer" \| false`, else throws `… has invalid 'acceptanceRole'; expected 'read-only', 'writer', or false.` | `:1450`. `false` → `delete next.acceptanceRole` | `inferLevel` in `runs/shared/acceptance.ts`. **Already ported**: `exec/acceptance/model/level.rs:237-412` reads `input.acceptance_role`, and `exec/mod.rs:266` threads `agent.acceptance_role` into it. |

Custom agents take the same path: `applyCustomAgentOverride` (`:1527-1536`) is
`applyBuiltinOverride(agent, override, meta)` plus bookkeeping.

### 1.2 Where cyrup has somewhere to put each one

- **`acceptanceRole`**: target `AgentDefinition::acceptance_role` (`discovery/types.rs:1251`)
  exists. The consumer exists (`exec/mod.rs:266` → `level.rs`). `AcceptanceRole`
  (`exec/acceptance/model/types.rs:125-130`) derives `Deserialize` kebab-case, so
  `OverrideField<AcceptanceRole>` reads `false` as `ExplicitClear` and an invalid string as a
  serde error. **The only missing piece is the override field.** This key really is S.
- **`outputMode`**: target `OutputSpec::mode` (`types.rs:269-272`) exists. The runtime registry
  already writes it (`discovery/runtime_registry.rs:986-989`). **Nothing reads it.** Every
  resolution site takes the call param and falls straight to `Inline`:
  `extension/executor/foreground.rs:1061-1062`, `extension/executor/background.rs:198-199`,
  `background/runner_main/executor.rs:816-818`, `background/recovery_descriptor.rs:535`, and
  `extension/tool/task_items.rs:293`, whose `None` falls through at the runner. The comment at
  `background.rs:196-197`, *"pi never consults the persona's own mode here"*, was true at v0.43.0
  and is **false from v0.57.0 on**. The frontmatter parser validates `outputMode:` and then leaves it in
  `extra_fields` (`discovery/frontmatter.rs:1227-1238`; the pinned test is
  `valid_tool_timeout_output_mode_and_fast_still_round_trip_as_extra_fields`, `:2771-2791`), and
  `parse_output_spec` hardcodes `mode: None` (`:776-784`). **So the agent-level mode is dropped
  from all three sources (frontmatter, runtime registry, override) at the consumer.** That is
  `09a`'s "Agent-level outputMode default never consulted" lead (`09a:3478-3480`). It **must**
  land with this row. Without it, the override field is decorative.
  - Type trap: cyrup's `OutputMode` has a third variant, `FileAndInline` (`types.rs:257-261`),
    which upstream does not have. An `OverrideField<OutputMode>` would (i) accept
    `"file-and-inline"` and (ii) read `false` as `ExplicitClear`. Upstream throws on both.
- **`fast`**: **no field, no consumer.** `AgentDefinition` has no `fast`. Frontmatter
  validates it and leaves it in `extra_fields` (`frontmatter.rs:1240-1249`). The tool schema
  (`extension/tool/schema.rs`) advertises no `fast`. **But every piece the behaviour needs is
  already in the workspace:**
  - both allowlisted models are in `cyrup-provider/src/providers/catalog/openai-codex.json`
    (`gpt-5.6-luna`, `gpt-5.6-sol`);
  - the codex driver already honours `service_tier` in the body (`openai_codex_responses/request.rs:122-123`)
    and folds it back into pricing (`events.rs:117-127`);
  - the `before_provider_request` seam is live: `cyrup-ext/src/event.rs:39,434` (`HostEvent::BeforeProviderRequest`),
    `contract.rs:86,181` (`EventPatch::ProviderRequest` replaces the body), driven from
    `cyrup-provider/src/stream.rs:71` `apply_on_payload`, installed by `cyrup-session-svc/src/builder.rs:1907`;
  - the child already runs a native extension that subscribes to events conditionally
    (`prompt_runtime.rs:1884-1945`, armed from env at `:2268`).

  **So the honest outcome is to port the behaviour.** Nothing here is inapplicable to cyrup.

### 1.3 Rust shape (real sites)

1. **`discovery/types.rs:692-810`**: three fields.
   - `acceptance_role: OverrideField<AcceptanceRole>`.
   - `output_mode: Option<OverrideOutputMode>`, where `OverrideOutputMode` is a **new two-variant**
     enum `{Inline, FileOnly}` with `rename_all = "kebab-case"`. Its `Deserialize` rejects `false` and
     `"file-and-inline"`. Do **not** use `OverrideField`: pi has no clear for this key.
   - `fast: OverrideField<bool>` (same as `disabled`: `false` is `Value(false)`).
   - Add all three to `is_empty` (`:814-836`). Rewrite the census doc (`:627-677`): 23 modeled,
     3 unmodeled, and those 3 named in the unported table (§1.5).
2. **`discovery/mod.rs:803-806`**: add `validate_override_output_modes` / `…_fast` /
   `…_acceptance_roles` beside `validate_override_default_providers`, emitting pi's **exact**
   field-naming text from §1.1. That follows the sibling's precedent (`:798-802`): serde's own
   message names no field.
3. **`discovery/merge.rs`**:
   - builtin arm next to `inherit_skills` (`:659-664`):
     `apply_field_full_replace(&mut agent.acceptance_role, &delta.acceptance_role, None, |r| Some(*r))`;
     `output_mode`: when `Some(m)`, set `agent.output.get_or_insert(OutputSpec{path:None,mode:None}).mode = Some(m.into())`
     and leave `path` alone (the mirror image of `apply_output_override`'s mode-preservation,
     `:840-859`); `fast` into the new `AgentDefinition::fast`.
   - custom arm: the same three in `apply_custom_override` (`:933-…`). Read §1.6 before choosing
     fill-unset or unconditional.
   - `builtin_applied_keys` (`:484-525`): add `"outputMode"`, `"fast"`, `"acceptanceRole"`. Rewrite
     the "21 fields / six unmodeled" doc at `:474-482`.
4. **`discovery/types.rs:1084-`** `AgentDefinition`: add `pub fast: Option<bool>`.
   **`frontmatter.rs:1227-1249`**: carry the validated `outputMode` into `output.mode` and `fast`
   into `def.fast`, and add both to `KNOWN_FIELDS` (`:80-`) so they stop landing in `extra_fields`.
   The `:2771` test is then rewritten to assert the typed fields.
5. **outputMode consumer, all five sites**: `explicit.or(agent.output.and_then(|s| s.mode)).unwrap_or(Inline)`
   at `foreground.rs:1061`, `background.rs:198` (fix the false comment at `:196-197`),
   `runner_main/executor.rs:816`, `recovery_descriptor.rs:535`, and wherever `SingleStepSpec`'s
   `None` is resolved for parallel tasks (`task_items.rs:293` produces it). Pi's `file-only`
   no-path refusal (`background.rs:205`) must see the **agent's** mode too. That is the point.
6. **fast, end to end** (this is the M):
   - `extension/tool/schema.rs`: `fast: boolean` on top-level props (`:586` neighbourhood) and the
     three item schemas (`:152`, `:178`, `:203`/`:266`), using upstream's descriptions verbatim.
     Parse in `extension/tool/routing.rs` / `task_items.rs`. Precedence is step > call > agent.
   - `exec/agent_config.rs` (`AgentConfig::from_agent_definition`, `:162`/`:337`): carry `fast`.
   - `exec/spawn_plan.rs:1163-1176` neighbourhood: when the effective `fast` is `true`, check the
     attempt's model (thinking suffix stripped) against `FAST_MODE_ALLOWED_MODELS`, using pi's two
     messages verbatim. Refuse when the capability ceiling denies extensions (pi `:494`), or declare
     a `[CYRUP-DELTA]` if cyrup's prompt runtime is not subject to that ceiling. Then write
     `CYRUP_SUBAGENT_FAST=1` into `env_overlay`.
   - `prompt_runtime.rs:2268` (`prompt_runtime_from_env`): read `CYRUP_SUBAGENT_FAST == "1"`; the
     subscribe set (`:1884-1945`) pushes `EventKind::BeforeProviderRequest` only then; `on_event`
     (`:1965`) returns `EventPatch::ProviderRequest({...payload, "service_tier": "priority"})`,
     which is pi's `rewriteFastModeProviderRequest` including its non-object passthrough.
   - External-CLI refusal: pi's `does not support fast mode` / `does not support: …, fast mode`
     texts at the foreground and async single entry points.
   - Fallback ladder `[CYRUP-DELTA]`: upstream checks the allowlist per launch against the ONE
     model (`v0.68.0` has no `fallbackModels`). cyrup's ladder can reach a non-codex model. Decide
     per attempt (upstream's granularity) and say so at the site.

### 1.4 Tests, each with the mutation it kills

| Test (location) | Asserts | Killing mutation |
|---|---|---|
| `discovery/mod.rs` tests: fix the `:2440` fixture (`"writer"`), then assert all three typed values | `acceptance_role == Value(Writer)`, `output_mode == Some(FileOnly)`, `fast == Value(true)` | delete any one field from the struct |
| new `override_acceptance_role_reaches_infer_level` (`exec/mod.rs` tests beside `:2265`) | builtin `reviewer` + `agentOverrides.reviewer.acceptanceRole: "writer"` → `run_sync`'s inferred floor is the writer floor | drop the `merge.rs` arm (the field parses, `agent.acceptance_role` stays `None`) |
| `override_acceptance_role_false_clears_a_frontmatter_role` | frontmatter `acceptanceRole: read-only` + override `false` → `None` | map `ExplicitClear` to a no-op |
| `override_output_mode_false_is_refused_with_pis_text` | `outputMode: false` → `MalformedSettings` with the exact `:1020` text | use `OverrideField<OutputMode>` (false → clear, silently) |
| `override_output_mode_file_and_inline_is_refused` | `"file-and-inline"` → refused | reuse cyrup's three-variant `OutputMode` |
| `agent_output_mode_default_reaches_the_run` (foreground single, `extension/executor/paths.rs` tests) | frontmatter `outputMode: file-only` + `output: x.md` + no call param → `opts.output_mode == FileOnly`, and with no path → `OutputPathRequired` before spawn | revert `foreground.rs:1061` to param-only |
| the same for async single (`background.rs` tests near `:622`) and a chain step (`runner_main` tests) | as above | revert that site |
| `output_override_preserves_path_and_output_mode_override_preserves_path` (`merge.rs` tests) | `output` then `outputMode` overrides compose | whole-struct replace in either arm |
| `fast_rejects_a_model_outside_the_allowlist` (`spawn_plan.rs` tests) | `fast: true` + `anthropic/…` → pi's message | drop the check |
| `fast_writes_the_env_marker_only_when_true` | env contains `CYRUP_SUBAGENT_FAST=1` iff effective `true`; step `false` beats agent `true` | invert the precedence, or write when `Some(false)` |
| `fast_child_rewrites_the_provider_request` (`prompt_runtime.rs` tests) | env marker set → `on_event(BeforeProviderRequest{payload:{model}})` returns a patch with `service_tier:"priority"` and `model` unchanged; marker absent → `BeforeProviderRequest` not in the subscribe set | skip the subscribe push, or replace the payload wholesale |
| end-to-end via `tests/child_prompt_runtime_integration.rs` | a codex-routed child's outbound body carries `service_tier` | cut the env hand-off |
| `external_cli_agent_refuses_fast` | pi's text | drop the refusal |
| **census**: `agent_override_config_models_every_v0_68_0_key_or_names_it_unported` (`types.rs` tests) | the literal 26-key list from `agents.ts:86-111` = modeled ∪ `UNPORTED_OVERRIDE_KEYS`, disjoint, `fallbackModels` handled explicitly | add a key upstream-side without either |

### 1.5 The three other v0.68.0 keys (`machine`, `inheritGlobalContext`, `mutationTools`)

These are the same silent drop today. They are not this row's port, and **this pass does not
decide they are out**:
- `machine` is Herdr remote placement (`agents.ts:87,1118-1121,1431`; consumers in
  `agents/agent-management.ts:701-717`, `runs/shared/herdr-connection.ts`, `child-launch.ts` …).
  cyrup has just landed a herdr client (#148). Porting this is **BIG**.
- `inheritGlobalContext` (`:97,1055-1060,1447`) and `mutationTools` (`:109,1152-1153,1459`) are
  both already listed as unrepresentable in `discovery/runtime_registry.rs:111-119`.

**Owed now (S):** one `UNPORTED_OVERRIDE_KEYS: [(&str, &str); 3]` table, using the same idiom as
`runtime_registry.rs:111` `UNREPRESENTABLE_FIELDS`. `parse_subagent_settings` walks each
`agentOverrides.<name>` raw object and, for a present key, pushes a **non-fatal** diagnostic:
`agentOverrides.<name>.<key> is not supported by this port (<landing>); it has no effect`. It is
surfaced through the existing `AgentDiscoveryDiagnostic` channel (`types.rs:1472`, with `file_path`
= the settings path), which `subagent list` and doctor already render. It is non-fatal on purpose:
R-SA-009's abort would make a valid pi-authored settings file break discovery wholesale, which is
worse than the drop. A test asserts the diagnostic appears. Its killing mutation is an empty table.

### 1.6 Interaction: the custom-agent frontmatter gate

`apply_custom_override`'s doc (`merge.rs:915-919`) says pi's custom arm is "fill-unset-only …
an explicitly present field blocks the override". That was true to **v0.57.0**
(`agentHasFrontmatterField` gate, `agents.ts:1374` @v0.57.0). At **v0.64.0** `31562d76`
(*"apply custom agent overrides consistently"*) it became `applyBuiltinOverride` unconditionally,
and it still is at v0.68.0 (`:1532`; `agentFrontmatterFields` is only carried along, never
consulted as a gate). So today a custom agent declaring `model:` plus a settings override of
`model` silently keeps the frontmatter value. That is the same failure class. It is recorded only
as a SUBA-092 residual and has **no open id**. The three new custom arms should not be written
against the gate. File it and land it first or with this row, then add the three arms
unconditionally.

---

## 2. SUBA-061: `asyncWidget`, `inlineToolDisplay`, `fleetKeybindings`

### 2.0 Verdict

**Open, all three.** `SubagentExtensionConfig` (`registration/mod.rs:79-81`, now **35** fields) is
`#[serde(rename_all = "camelCase", default)]` with no `deny_unknown_fields`. `git grep` for all six
spellings over `crates/` returns one hit, a comment (`extension/host/slash.rs:132`).
`legacyChainControls` stays struck: `git grep -c legacyChainControls v0.68.0 -- src` is empty.

**The PR #151 question:** the roster widget (`tui/fleet_status.rs`) is keyed by upstream's
hardcoded `fleet-status.ts`, which never reads `fleetKeybindings`. The key drives the **full
inspector** only (`tui/fleet.ts:834`). What #151 changed that matters here: every inspector open
(both `/subagents-fleet` and roster `Enter`) now goes through **one** function,
`extension/host/terminal_input.rs:104-170` (`FleetInspectorHandle::open`). That function builds
`FleetViewOptions::default()` at `:123`, the single place the bindings get threaded in.

None of the three keys has been retired upstream: declared at `shared/types.ts:2609,2611,2617`
@v0.68.0. So **port, don't refuse.**

### 2.1 `inlineToolDisplay` (S)

Upstream: `type InlineToolDisplay = "rich" | "summary"` (`types.ts:2541`);
`summaryInlineToolDisplay = config.inlineToolDisplay === "summary"` (`extension/index.ts:439`);
`renderResult` picks `renderSubagentSummary` (`tui/render.ts:3318-3356`) over `renderSubagentResult`
(`index.ts:812-814`). The summary is one line: `<glyph> <label> · <state>`. State order is
running > failed > stopped > paused > partial > completed; the label is
`foregroundSingleDisplayName(results[0])` for a single run, else `details.mode`. There is no
validator upstream: any value other than `"summary"` is rich.

Rust: an `inline_tool_display: Option<String>` field. It stays a **string**, so an unknown value
does not fail the typed parse and throw away the whole `config.json`. Capture it at
`extension/host/mod.rs:231` beside `fleet_view_enabled` (`render_result` is sync and cannot lock
the async config cell). Branch in `native_impl.rs:1091-1096` to a new
`render_subagent_summary(result)` next to `render_subagent_result` (`:1175`). **No silent value:**
a present value outside `{"rich","summary"}` gets a loader warning
`config.inlineToolDisplay must be "rich" or "summary"; using "rich"` (`[CYRUP-DELTA]`: upstream
silently treats it as rich).

Tests: `summary_mode_renders_one_line_per_state` (six states → six glyph/word pairs; mutation:
drop the `failed` precedence over `stopped`); `rich_is_the_default_and_unknown_is_rich_with_a_warning`
(mutation: `== "summary"` → `!= "rich"`).

### 2.2 `asyncWidget` (S gate + M wiring)

Upstream: `asyncWidgetEnabled = config.asyncWidget !== false` (`index.ts:438`) → tracker
`widgetEnabled` (`:567`) → `renderWidget(ctx, widgetEnabled === false ? [] : jobs)`
(`async-job-tracker.ts:114`, and `:129` skips the render request). So `false` clears the
`WIDGET_KEY` slot in **both** modes: the RPC JSON line and the interactive component. It is
independent of `fleetView` ("Defaults to true, including when FleetView is enabled",
`types.ts:2610`).

cyrup today:
- **RPC mode:** the slot is published by `publish_async_status_snapshot_widget`
  (`extension/host/slash.rs:104-154`), called from `refresh_fleet_status_widget` at `:239-241`.
  That caller **returns early when `fleet_view` is off** (`:206-208`), so `fleetView: false` also
  kills the async slot. Upstream does not do that. This is a second, unfiled divergence.
- **Interactive mode:** nothing is published under `"subagent-async"` at all. The human widget is
  C21, `render_async_jobs_widget` (`tui/events.rs:865`), ported with **zero production
  callers**. The stale "cannot" note is at `tui/events.rs:44-52` (§0 new-2).

Rust: `async_widget: Option<bool>` plus an accessor `async_widget_enabled() = != Some(false)`,
following `scheduled_runs_enabled`'s tri-state precedent (`registration/mod.rs:606-612`). Move the
async-slot publish **out from under** the `fleet_view_enabled` early-return. When disabled, publish
with an empty job list (so the clear still fires, as pi's `[]` does). In `ExtMode::Interactive`,
publish `lines_to_plain_text(render_async_jobs_widget(jobs, tick))` under
`ASYNC_STATUS_SNAPSHOT_WIDGET_KEY` via `set_widget`. Correct `tui/events.rs:44-52`.

Tests: `async_widget_false_clears_the_async_slot_in_rpc_mode` (a recording `HostServices`; the
mutation is to ignore the flag); `async_slot_survives_fleet_view_false` (mutation: leave it under
the early return); `interactive_mode_publishes_the_human_async_widget` (mutation: the RPC-only
guard).

### 2.3 `fleetKeybindings` (M)

Upstream: `FLEET_KEYBINDING_ACTIONS` (`types.ts:2552-2567`) has 14 actions. Validator
`extension/config.ts:64-74` (called at `:172`) produces four verbatim messages: `must be a JSON
object` / `.<a> is not a supported Fleet action` / `.<a> must be a non-empty array of strings` /
`.<a> entries must be non-empty strings`. `DEFAULT_FLEET_KEYBINDINGS` is at `tui/fleet.ts:33-48`.
`resolveFleetKeybindings` (`:52-56`) makes per-action **replacement**, not merge. A single
uppercase letter means `shift+<lower>` (`:58-61`). Dispatch is at `:1092-1095` (detail pane) and
`:1151-1206` (normal mode). **Footer hints are generated from the bindings** (`bindingLabel`,
`:67-72`, used at `:1367-1368`). Threaded through `index.ts:502` and `slash-commands.ts:863`.

cyrup: `SubagentFleetComponent::handle_input` hardcodes the defaults in its normal-mode `match`
(`tui/fleet.rs:1893-1947`). The footer is a literal (`:2306-2307`). Input arrives as `OverlayKey`,
narrowed by `to_fleet_key` (`tui/fleet_overlay.rs:305-334`), which **drops every ctrl-chord except
c/o and every alt-chord**. A user binding `ctrl+x` could never fire through it.

Rust:
- `FleetKeybindingsConfig`: `Option<BTreeMap<String, Vec<String>>>` on the config, plus
  `validate_fleet_keybindings(raw)` with the four messages verbatim. **Actually call it from the
  loader** (§7.1).
- A `FleetKeySpec` parser over pi `KeyId` strings (`"escape"`, `"return"`, `"pageUp"`, `"ctrl+o"`,
  `"K"` → shift) producing a matcher on `OverlayKey`. `cyrup-tui/src/keymap.rs:599-622`
  `Key::parse` already exists but lives in a crate this one does not depend on. Either lift it to
  `cyrup-ext` beside the #151 terminal-key table (`cyrup-ext/src/contract.rs:363-620`) or write a
  small parser here. **An unparseable spec is a config warning naming the action**. That is a
  `[CYRUP-DELTA]`: pi's `matchesKey` just never matches it, silently.
- `FleetViewOptions` (`tui/fleet.rs:239-254`) gains `keybindings: ResolvedFleetKeybindings`.
  `terminal_input.rs:123` passes the configured one. The normal-mode and detail-scroll arms dispatch
  through `bindings.action_for(key)`. The footer (`:2306`) is built by a `binding_label` port.
  `to_fleet_key` passes raw modifiers through instead of pre-filtering.

Tests: `fleet_keybindings_validator_messages_are_upstreams` (4 cases; mutation: drop any branch);
`a_rebound_stop_key_fires_and_the_default_no_longer_does` (`{"stop":["X"]}`: `X` enters
stop-confirm and `D` is ignored; mutation: merge instead of replace);
`a_ctrl_chord_binding_reaches_the_component` (mutation: keep `to_fleet_key`'s ctrl filter);
`footer_reflects_rebinding` (mutation: keep the literal).

---

## 3. PB-14: the skills-not-found warning (both surfaces)

### 3.0 Verdict

**Open on both surfaces. Premise and citations hold at `521beaa`.**
`grep -rn 'skills_warning\|skillsWarning' src/` returns only `artifacts.rs:544`, which is the
admission. `resolve_skills` (`discovery/skills.rs:152`) still has no caller outside its own module.

### 3.1 Upstream (v0.68.0)

- **Run side:** `runs/foreground/execution.ts:1902`
  `skillsWarning: missingSkills.length > 0 ? \`Skills not found: ${missingSkills.join(", ")}\` : undefined`
  (the `pi-subagents` case has already hard-failed at `:1783-1794`). The field is declared on
  `SingleResult` (`shared/types.ts:1282`, `execution.ts:346`), persisted to `_meta.json` (`:174`),
  and rendered `Warning: <text>` in the warning colour (`tui/render.ts:3484-3486` single,
  `:3660-3662` multi, indented). The async runner does **not** set it (no hit in
  `subagent-runner.ts`).
- **Management side:** `agents/agent-management.ts:221-230` `skillsWarning(cwd, agent)`
  → `Warning: skills not found: <a, b>.`, via `resolveSkills(skills, cwd, skillPath, dirname(filePath))`.
  It is appended after the create headline (`:1185-1186`), and on update only when `skills` or
  `skillPath` changed (`:1240-1243`).

### 3.2 Rust shape

- `exec/run_result.rs:25` `SingleResult`: `#[serde(skip_serializing_if = "Option::is_none")] pub skills_warning: Option<String>`.
- `exec/mod.rs:1151-1183`: from `resolution.missing` after the orchestration early-return, set
  `Skills not found: …` and carry it on `LadderSetup` (`:1107`) to the result construction (the
  `setup.resolved_skill_names` sibling at `:704`).
- `artifacts.rs:553` `run_artifact_metadata`: add `"skillsWarning"` and fix the `:544` doc.
- `tui/events.rs:699` `render_inline_result`: after `entry.stats_line()`, push a warning-role
  `Warning: <text>` line.
- Management: `discovery/management/handlers.rs:516` `handle_create` and `:651` `handle_update`
  become `async` (the dispatcher `management/mod.rs:198-209` is already async). Push
  `Warning: skills not found: … .` into the existing `warnings` vec (`:581`, `:746`) using
  `resolve_skills(&skills, cwd)` against `cfg`'s cwd with the agent file's directory as fallback
  (`resolve_skills_with_fallback`, `skills.rs:195`). `[CYRUP-DELTA]`: no `skillPath`, which is
  unrepresentable (`runtime_registry.rs:115`). On update, only when `skills` was in the patch.

### 3.3 Tests

| Test | Killing mutation |
|---|---|
| `run_with_a_typo_skill_sets_skills_warning` (`exec/mod.rs` tests): `skills:["typo"]` → `Some("Skills not found: typo")`, run still spawns | drop the assignment |
| `orchestration_skill_still_hard_fails_and_carries_no_warning` | reorder so the warning is set before the early-return |
| `inline_result_renders_the_skills_warning_line` (`tui/events.rs` tests) | drop the push |
| `metadata_carries_skills_warning` (`artifacts.rs` tests) | omit the key |
| `create_with_unknown_skill_warns_after_the_headline` (`tests/management_actions_integration.rs`) | drop the push, or put it before the headline |
| `update_without_skills_in_the_patch_does_not_warn` | always warn |

---

## 4. SUBA-063: re-derived from scratch

### 4(a) Zero-tool-budget authorisation: **not a silent drop; blocked on SUBA-022**

- cyrup: the child reader `HardMinimum::from_env` (`exec/tool_budget.rs:55-63`, env const `:39`)
  is read at `prompt_runtime.rs:2357`. **No writer exists.** The env overlay writes only
  `TOOL_BUDGET_ENV` (`exec/spawn_plan.rs:1169-1176`). `validate_tool_budget_config` hardcodes
  `HardMinimum::One` (`tool_budget.rs:115`). Every production validator funnels through it
  (`routing.rs:774`, `executor/background.rs:1500`, `discovery/mod.rs:921`, `frontmatter.rs:1045`,
  `runtime_registry.rs:589`, `management/frontmatter_write.rs:523`).
- upstream: the **only** producer of `allowZeroToolBudget` is the prompt-template delegation
  bridge. `slash/delegation-request.ts:90` validates with `{ minimumHard: 0 }`;
  `slash/delegation-adapters.ts:301` sets `delegatedAllowZeroToolBudget: true`; the executor
  moves it into a private `WeakSet` (`subagent-executor.ts:7465-7475`, `:4998`, `:5017`) and
  relaxes only its own run's validator (`:6867-6871`). It forwards authorisation only while the
  effective budget **is** the delegated one (`:4121`). The public tool path uses `minimumHard: 1`.
- **So `hard: 0` via the tool is loudly refused on every surface cyrup has, matching upstream.**
  Nothing is accepted and dropped. The "fail-closed dead end" the row describes is the correct
  behaviour until the delegation bridge exists. That bridge is **SUBA-022** (typed extension
  delegation API, L, `09:617`).
- **Transport note:** upstream **removed** `PI_SUBAGENT_TOOL_BUDGET_ZERO_AUTH` at **v0.67.0**
  (`git grep -c ZERO_AUTH` is non-empty at v0.64.0 and empty at v0.67.0/v0.68.0), because children
  became in-process (`ChildRuntimeConfig.toolBudget`, `subagent-prompt-runtime.ts:453`). cyrup
  still spawns subprocesses, so its env var is the right transport **for cyrup**. Keep it and
  mark it `[CYRUP-DELTA]` at `tool_budget.rs:30-38`, which currently cites a v0.64.0 line as if
  current.
- **Owed now:** nothing in code. **When SUBA-022 lands:** thread an
  `allow_zero_tool_budget` flag from the delegation request to (i) the run validator
  (`validate_tool_budget_config_with(.., HardMinimum::Zero)`) and (ii) `spawn_plan.rs:1176`,
  writing `TOOL_BUDGET_ZERO_AUTH_ENV = "1"` iff the flag is set **and** the effective budget is the
  request's (pi `:4121`). Test `a_delegated_zero_budget_child_blocks_its_first_blocked_call`;
  killing mutation: write the env var without the `effective == requested` guard (a config budget
  would then inherit the authorisation).

### 4(b) Runtime-extension acknowledgement: **open, M, not a silent drop**

Upstream: `runs/shared/runtime-acknowledged-extensions.ts` (event `subagent:acknowledge-extension`,
≤32 ids, id charset rule, `{version:1, source:"child-runtime", ids, omitted}`); collected
child-side (`subagent-prompt-runtime.ts:68-90`, finalized on `agent_end`/`session_shutdown`); carried
on `SingleResult`, the step and the run status (`async-status.ts:88,141,387,442`,
`subagent-runner.ts:1261,1397,3834,…`); advertised in RPC `ping` (`extension/rpc.ts:457`).
cyrup: zero hits. **This is honest, not silent**: `extension/rpc/ping.rs:64-66` states it is
not advertised, so no client is told it exists. It is a missing feature, not an accepted-and-ignored
key. It does not belong in this batch's rule. Size **M**: a child-side collector in `prompt_runtime`,
a file or protocol hand-back (the subprocess needs a channel; `exec/child_protocol.rs` is the
candidate), then `SingleResult` → `StepStatus` → `RunStatus` → RPC capability.

### 4(c) The `events.jsonl` truncation marker: **open, S, a silent drop, and the cap is on the wrong events**

Upstream (`runs/background/subagent-runner.ts:319-386` @v0.68.0; also present at v0.43.0
`:266-267`, so **in-baseline**): `appendDiagnosticJsonl` reserves 512 bytes, and on the first
overflow writes `{type:"subagent.events.truncated", ts, maxBytes, droppedEventType}` once, then
stops. **Only child diagnostic events are capped** (`:1223 appendChildEvent`). Lifecycle and
control events (`step.started`, `step.stopped`, `steering.notice`, … `:1642-3761`) go through the
**uncapped** `appendJsonl`.

cyrup: `jsonl.rs:9-28` makes the cap "silent" by contract, and calls that "a deliberate,
disclosed port of a known pi-subagents limitation". **It is not**: pi writes a marker. And
`background/runner_main/events.rs:75-110` routes **every** event, which here means only
**lifecycle** events (`run.started/completed/paused/stopped/timed_out`, `step.*`, `steer.requested`)
through the capped writer. cyrup does not journal child events into `events.jsonl` at all. So
`CYRUP_SUBAGENT_ASYNC_EVENTS_MAX_BYTES=0` silently erases the whole lifecycle trail, where pi's
`0` erases only child diagnostics.

Rust: (1) lifecycle `append_event` writes uncapped. That needs a second `BoundedJsonlWriter`
mode, or a raw append. The per-attempt stdout tee in `spawn` keeps the current silent cap unless
separately decided. (2) Add a `write_diagnostic_line(line, dropped_type)` with the 512-byte reserve
and one-shot marker, for when child-event journaling exists (upstream `:1223`; cyrup has no such
journal, which is a separate gap and is **not decided out here**). (3) Correct the `jsonl.rs`
module doc and the func-SA R-SA-136 citation.
Tests: `a_zero_cap_still_journals_lifecycle_events` (mutation: keep lifecycle on the capped
path); `the_first_overflowing_diagnostic_line_writes_one_marker_and_stops` (asserts exact marker
keys and that it fits in the reserve; mutation: drop the reserve, so the marker cannot fit).

---

## 5. SUBA-097: **CLOSED** (the ledger is stale)

`background/scheduled_runs/trigger.rs:888-899` now names
`extension::executor::scheduled_runs::tests::a_scheduled_run_really_spawns_a_process_through_the_production_launcher`
(`extension/executor/scheduled_runs.rs:490`). It explains that `tests/scheduled_runs_integration.rs`
never existed, and warns against re-pointing the comment without checking. Landed at `d00a314`
(2026-09-17, *"… prove scheduled runs really fire"*).

What the test covers (read in full, `:462-640`) is exactly what the row asked for:
`install_scheduled_runs` is the **only** construction site of `ExecutorScheduleLauncher`
(`:317`), and no launcher is injected. It drives the real `schedule.create` and `schedule.run`
verbs, then checks three pieces of on-disk evidence, all written by production code:
(1) a marker file written by the **spawned child process**; (2) the fired run's `status.json`,
terminal, `mode: workflow`, attributed to the live session, and under the project async root;
(3) the `ResultFile` carrying `scheduleOrigin.id == "nightly"`. A launcher that returns `Ok`
without spawning fails at (1).

**Owed:** strike `09-cyrup-ext-subagents.md:499` and `:634` (and the `:55-64`, `:511-513` prose,
and `00-residual-ledger.md:383,494`) as CLOSED at `d00a314`. No code.

---

## 6. SUBA-098: stray CJK token (XS)

Confirmed at `extension/tool/scheduled_runs_tests.rs:31`: `…would观察 a different identity…`
→ `…would observe a different identity…`. The sibling `cyrup-tui/src/tests/selector.rs:41`
`empty/末-position` → `empty/end-position` is area 07's row and is still present. A fresh
`rg '[\p{Han}\p{Hiragana}\p{Katakana}\p{Hangul}]' crates/ -g '*.rs'` shows every other hit is
deliberate data or code-point documentation (`cyrup-tui/src/editor/wrap.rs:526-536`). No test is
proportionate. A lint for Han in comments would false-positive on `wrap.rs`, so there is no
killing mutation to name.

---

## 7. Cross-cutting: the loader, and the shared census

### 7.1 `config.json` loader (`crates/cyrup/src/subagent_config.rs:40-90`), S

- Call **all** raw validators in one place: `validate_missions` (already called),
  `validate_authority_policy`, `validate_artifact_dir`, `validate_artifact_config`,
  and the new `validate_fleet_keybindings` / `inlineToolDisplay` / `asyncWidget` type checks.
  Keep the file's warn-and-default convention, with the validator's field-naming message.
- **Unknown top-level keys:** diff the raw object's keys against a `KNOWN_CONFIG_KEYS` list, which
  is upstream's `ExtensionConfig` keys at v0.68.0 (`shared/types.ts:2595-…`) ∪ cyrup's own. Emit
  `cyrup: warning: <path>: unknown key '<k>' (ignored)`, and for an upstream key cyrup has not
  ported, `… is not supported by this port (ignored)`. `[CYRUP-DELTA]`: upstream silently ignores
  unknown keys, but its keys are all consumed. cyrup's are not.
- Tests: `an_unknown_authority_action_is_refused_by_name` (mutation: remove the new call);
  `an_unported_upstream_key_warns` (mutation: an empty list).

### 7.2 What these rows share

- **SUBA-096 §1.5 and §7.1 share one helper**: a "declared-key census + unported-key diagnostic"
  over a raw JSON object, `(raw, known: &[&str], unported: &[(&str,&str)]) -> Vec<Diagnostic>`.
  Write it once (`discovery/` or `registration/`) and use it for `agentOverrides.<n>` and
  `config.json`. `runtime_registry.rs:784-788` is the third, pre-existing instance of the same idea
  and can move onto it.
- **SUBA-061 `inlineToolDisplay` and PB-14's render line** both edit the tool-result renderer
  (`extension/host/native_impl.rs:1091-1215`, `tui/events.rs:699-745`). Land them together or
  back to back.
- **SUBA-061 `fleetKeybindings` rides PR #151's single open path** (`terminal_input.rs:104-170`).
  Its key-spec parser may share #151's terminal-key table (`cyrup-ext/src/contract.rs`) if that
  table grows named-key parsing.
- **SUBA-096 `outputMode` and PB-14** both touch the foreground single path
  (`extension/executor/foreground.rs` / `exec/mod.rs`) but different lines. There is no ordering
  constraint.
- **SUBA-063(a)** shares nothing with the rest and rides SUBA-022. **(b)** is not this class.
  **(c)** is independent (`jsonl.rs`, `runner_main/events.rs`).
- **§1.6 (custom-override gate)** should land **before** SUBA-096's custom arms.

## 8. Size

| Piece | Size |
|---|---|
| SUBA-096 acceptanceRole | S |
| SUBA-096 outputMode (type + override + frontmatter + 5 consumer sites) | S+ |
| SUBA-096 fast (field, frontmatter, override, 4 tool schemas, allowlist, env, child hook, external-CLI refusal) | M |
| SUBA-096 §1.5 unported-key diagnostic | S |
| §1.6 custom-override gate (unfiled; precondition) | S |
| SUBA-061 inlineToolDisplay | S |
| SUBA-061 asyncWidget gate + C21 interactive wiring | S + M |
| SUBA-061 fleetKeybindings (validator, key-spec parser, dispatch, footer) | M |
| PB-14 both surfaces | S |
| SUBA-063(c) marker + lifecycle-uncapped | S |
| §7.1 loader (wire 3 idle validators + unknown-key warnings) | S |
| SUBA-098 | XS |
| SUBA-097 | ledger edit only |
| *(not in this batch)* SUBA-063(a) → SUBA-022 (L); SUBA-063(b) (M); `machine` / `inheritGlobalContext` / `mutationTools` ports (BIG, not decided out) | — |

**Batch total: L.** Two M pieces (fast, fleetKeybindings) and one M wiring (C21), plus about
nine S pieces. It splits cleanly into three PRs: (1) the census helper, §7.1, §1.5, §1.6,
acceptanceRole, outputMode and PB-14; (2) fast; (3) the three SUBA-061 keys with C21.
