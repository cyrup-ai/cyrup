---
stage: augment
status: ready
updated: 2026-09-22 00:00
---

# PB-9: `clarify: true` is advertised, but no preview/edit UI exists

> Branch `claude/subagents-delegation`, HEAD `521beaa`.
> Upstream pin: `git -C tmp/pi-subagents show v0.68.0:<path>` (`PARITY-GAPS.md:44` records v0.68.0 as
> the latest tag). Older tags are cited only as history, and each is named where it is used.
> This is a read-only research pass. No code was changed and cargo was not run. Every claim below comes from
> grep or from reading the file at the cited line.

## Verdict

**PB-9 is still open, but the row's premise is wrong at the pinned tag.**

- **The observable is true at HEAD.** cyrup advertises `clarify` with upstream's old description, "Show
  TUI to preview/edit before execution…" (`extension/tool/schema.rs:556`). It parses the flag
  (`params.rs:228`). The only effects are that it forces the run into the foreground
  (`params.rs:631-637`) and hides the `[async]` badge (`native_impl.rs:1126-1128`). No preview is
  ever shown. Bundled docs make the same promise (`resources/skills/pi-subagents/SKILL.md:402-416`)
  or get it wrong (`resources/docs/tool-reference.md:105`, "Ask the child to clarify before
  working").
- **The premise is false at v0.68.0.** Upstream does not show a preview at v0.68.0. It removed the
  feature in two steps:
  1. `39c37184` "feat: require workflowScript for public subagents" (first tag **v0.43.0**)
     deleted `clarify` from the public schema. From then on, the public boundary **refuses any
     `clarify` value**.
  2. `ef554d2a` "refactor: remove legacy chain workflow internals (#1166)" (first tag **v0.51.0**)
     deleted `src/runs/foreground/chain-clarify.ts` (−1354 lines) and `chain-execution.ts`.
     `ChainClarifyComponent` exists at no tag after v0.50.0.
- **Parity at v0.68.0 means refusing `clarify`, not building a preview.** Upstream's current
  answer to `{…, clarify: <anything>}` is the error
  `"Public workflowScript execution does not support clarify UI."`.
- **cyrup also has a real bug the row does not mention.** An RPC `spawn` with `clarify: true`
  runs in the FOREGROUND in cyrup, even though RPC spawn is documented and forced to be detached-only.
  Details are in the next section.

This file gives two resolutions. **Plan A** (small) is v0.68.0 parity and closes PB-9. **Plan B**
(large) ports the preview UI of v0.42.1 as a `[CYRUP-DELTA]`. Both are fully specified. The owner
chooses. Plan B is **not** ruled out: every host primitive it needs exists (see §6). Both plans
share Step 0.

---

## 1. Citation audit: what the row gets right and wrong

| Row claim | Status at HEAD / at tag | Evidence |
|---|---|---|
| `grep 'ChainClarify\|chain_clarify' crates/cyrup-ext-subagents/src` → 0 | **Right** (still 0) | re-greped |
| crate's other `clarify` hits are the `contact_supervisor` intercom feature | **Right** | §3 classification |
| upstream `src/runs/foreground/chain-clarify.ts:199` (`ChainClarifyComponent`, 1350-line file) | **Resolves only at v0.42.1–v0.43.0** (1350 lines; 1333 at v0.34.0; 1354 at v0.50.0). **Absent at v0.68.0.** No tag is named for it | `git cat-file -e v0.68.0:src/runs/foreground/chain-clarify.ts` fails; v0.50.0 has it, v0.51.0 does not |
| dispatched at `subagent-executor.ts:3190`, `:3572`, `chain-execution.ts:692` via `ctx.ui.custom<ChainClarifyResult>` | **Resolves exactly at v0.43.0.** At v0.42.1 the executor sites are `:3168`/`:3550` (chain-execution `:692` matches both). **At v0.43.0 the public tool already refused `clarify`**, so the lines the row cites could not be reached from the model | `public-execution.ts:32-34` @v0.43.0 |
| "First tag **v0.21.2**" | **Half right.** That is the first tag with the file at this path. The file existed as root `chain-clarify.ts` from **0.13.0** (`6c927b60`) | `git tag --contains 6c927b60` |
| cyrup `extension/tool/schema.rs:556` declares the param | **Right** | |
| "read at `extension/host/native_impl.rs:1098-1102` (the async→foreground downgrade and the `[async]`-badge suppression, now one site)" | **Wrong.** `:1098-1102` is `render_subagent_call`'s doc and header. The badge read is `native_impl.rs:1126-1128`. The async→foreground downgrade is a **different site**, `extension/tool/params.rs:616-638` (`SubagentToolParams::is_background`) | read |
| listed at `extension/tool/params.rs:782` | **Right** (`provided_keys`, `:781-782`) | |
| "`HostServices::open_overlay` is already consumed in production by this same crate at `extension/host/slash.rs:147`" | **Stale.** `slash.rs:147` is async-status widget code. The crate's production `open_overlay` call is `extension/host/terminal_input.rs:170` (fleet inspector) | `grep -rn open_overlay crates/cyrup-ext-subagents/src` |
| "cyrup accepts `clarify: true`, forces the run foreground, and launches immediately" | **Right**, and it applies to **RPC spawn** too (the new bug below) | |
| implied: upstream shows the human a preview | **False at v0.68.0** | §2.1 |

**The new bug:** `extension/rpc/params.rs:196-223` (`spawn_params`) calls
`normalize_public_subagent_execution` with `action` only, refuses `async: false`, and forces
`async: true`. It never looks at `clarify`. The request then goes to `Tool::execute`, where
`is_background` returns `requested_async && clarify != Some(true)` (`params.rs:637`), which is
**false**. The result is that the "detached-only" RPC spawn runs in the foreground and holds the
R-SA-069 single-dispatch token. Upstream refuses this at **both** tags: v0.42.1 `rpc.ts:432-433`
(`"RPC spawn cannot open the clarify UI; omit clarify or set clarify: false."`) and v0.68.0
`rpc.ts:516-517` (through `normalizePublicSubagentExecution`, which rejects any `clarify`). This
was found by reading the code, not by running it.

---

## 2. Upstream

### 2.1 At v0.68.0 (the pin): `clarify` is refused at the public boundary

- **Schema:** `git show v0.68.0:src/extension/schemas.ts | grep -ic clarify` returns **0**.
  Upstream does not advertise it.
- **Refusal:** `src/extension/public-execution.ts:143-145` @v0.68.0:
  ```ts
  if (params.clarify !== undefined) {
      return { ok: false, error: "Public workflowScript execution does not support clarify UI.", mode: "workflow" };
  }
  ```
  - The test is `!== undefined`, so `clarify: false` and `clarify: null` are **also** refused.
    Only a missing key passes.
  - The check sits **after** the blank-action refusal (`:133-135`), so `{action: "", clarify: true}`
    gets the blank-action text.
  - It sits **before** the legacy `resume`/`tasks`/`chain` refusals.
- **Who calls the boundary:**
  - `executePublic` (`subagent-executor.ts:7424-7434`) turns a refusal into
    `{ content:[{type:"text",text:error}], isError:true, details:{ mode, results:[] } }`.
  - Both the tool (`index.ts:736-739` `executeSubagentCollapsed`, used by `execute` at `:775-777`)
    and the RPC bridge (`index.ts:762`) call `executePublic`.
  - RPC `spawnParams` normalizes before its own two rules (`rpc.ts:514-525`).
  - The slash bridge does the same (`slash/slash-bridge.ts:82`).
- **Renderer:** `index.ts:800` @v0.68.0 is
  `const asyncLabel = args.async === true ? "[async]" : ""`. It **no longer** suppresses the badge
  for `clarify`. `formatWorkflowManifest` still takes a `clarify` argument (`:276-277`), but its
  only caller passes the literal `false` (`:791`).
- **Leftover code in the executor.** Nothing public can reach these lines:
  - `subagent-executor.ts:6922-6925` (`allowClarifyTaskPrompt`)
  - `:6975-6978` (`effectiveAsync = requestedAsync && clarify !== true`)
  - `:7080`
  - `:7413` (`runsForeground`)
  - `backgroundRequestedWhileClarifying` is written at `:7166` and declared at `:508`, but
    **never read**.
  - Internal callers force `clarify: false`: `top-level-async.ts:13` and
    `slash/delegation-adapters.ts:304`.
  - `scripted-workflow.ts:657` throws if a `runs.run` item carries `clarify`.
- **Docs:** `skills/pi-subagents/references/management-authoring-rpc.md:161` @v0.68.0 says spawn
  "rejects management actions, `async: false`, or `clarify: true`".
  `prompts/review-loop.md:11` @v0.68.0 still says "do not set `clarify: true` unless I explicitly
  want the foreground clarify UI". That upstream sentence is stale but harmless.

### 2.2 At v0.42.1 (the last tag that advertised it): what the preview did

This section is history. It is the specification for Plan B only.

- **Schema:** `schemas.ts:339` @v0.42.1, with exactly the description cyrup carries today.
- **What the human sees:** a centered modal (`{ overlay:true, overlayOptions:{ anchor:"center", width:84, maxHeight:"80%" } }`)
  listing each step, one per agent, with its task template and resolved behavior (model, thinking,
  output, reads, progress, skills).
- **Keys** (`chain-clarify.ts:440-521`, footer `:1108-1128`):
  - `Enter` runs.
  - `Esc`/`Ctrl+C` cancels.
  - `↑↓`/`j k` select a step.
  - `e` edits the task text (an inline buffer editor, `:120-170`).
  - `m` picks the model, with search (`:545-635`).
  - `t` sets thinking level, limited to the levels the model supports (`:636-700`).
  - `s` multi-selects skills with search and Space (`:700-770`).
  - `w` edits the output path (single and chain only).
  - `r` edits reads and `p` toggles progress for all steps (chain only).
  - `b` toggles Background.
  - **The agent cannot be changed.**
- **Result type** (`chain-clarify.ts:26-31`):
  `{ confirmed, templates: string[], behaviorOverrides: ({output?,reads?,progress?,model?,skills?}|undefined)[], runInBackground? }`.
  The model choice carries thinking as a suffix (`splitThinkingSuffix`).
- **What each choice returns to the model:**
  - **Cancel** returns `"Cancelled"` for single and parallel (`subagent-executor.ts:3185-3187`,
    `:3567-3569` @v0.42.1) and `"Chain cancelled"` for chain (`chain-execution.ts:713-718`). This is
    **not** `isError`, and no child is spawned.
  - **Run** replaces the task text with the edited templates, applies the per-step overrides, then
    executes in the foreground.
  - **Background (`b`)** relaunches the edited params through the async path: `executeAsyncSingle`
    (`:3208-3230`, `:3579+`), or for chains
    `{ content:"Launching in background...", requestedAsync:{ chain: updatedChain } }`
    (`chain-execution.ts:719-741`).
- **Modes and timing:**
  - All three modes support it: single (`:3546`), parallel (`:3162`), and chain
    (`chain-execution.ts:657`).
  - A chain containing parallel, dynamic-fanout or checkpoint steps **silently skips** the preview
    (`chain-execution.ts:655-657`).
  - An explicit `clarify: true` always keeps the run in the foreground, even when `async: true`
    (`:4885-4886`, `:5211`). A composite run whose background request was downgraded gets the
    note `" (background requested, but clarify kept this run foreground)"` (`:3467`).
  - A `workflowScript` with `clarify: true` is refused (`:4049-4050`).
  - A chain whose first step has no task is allowed only when the preview will open
    (`allowClarifyTaskPrompt`, `:4822-4825`, `:1789`).
- **Headless behavior** (the `ctx.hasUI` gate at every call site):
  - **Print/JSON mode** (`hasUI` is false): the preview is skipped, the run still launches, and
    `clarify` still forces the foreground because `effectiveAsync` ignores `hasUI`.
  - **RPC mode:** pi's `hasUI` is **true** (`runner.ts:492-494` @pi v0.85.1:
    `uiContext !== noOpUIContext`, and `rpc-mode.ts:320` installs a real one). But `ui.custom()`
    returns `undefined` (`rpc-mode.ts:228-231`), which hits `!result` and returns **"Cancelled"**.
    The run silently never happens.
  - **RPC `spawn` and scheduled runs** refuse `clarify: true` outright (`rpc.ts:432-433`,
    `scheduled-runs.ts:317`).

---

## 3. cyrup at HEAD: every `clarify` hit, classified

`grep -rn -i clarify crates/cyrup-ext-subagents/src` gives 28 files and ≈190 hits. They fall into
three groups.

**(a) The PB-9 tool parameter (the launch preview).** All of these are production code unless marked:

| Site | Kind | What it does |
|---|---|---|
| `extension/tool/schema.rs:556` | prod | advertises `clarify` with the v0.42.1 description |
| `extension/tool/schema.rs:557` | comment | "`control` … between `clarify` and…" |
| `extension/tool/schema.rs:810` | **test** | `expected_properties` requires `"clarify"` in the schema |
| `extension/tool/params.rs:228` | prod | `pub(crate) clarify: Option<bool>` |
| `extension/tool/params.rs:616-638` | prod | `is_background`: forced `Some(false)` under `forceTopLevelAsync`, and `requested_async && clarify != Some(true)` |
| `extension/tool/params.rs:781-782` | prod | `provided_keys` liveness |
| `extension/tool/params.rs:43` | doc | lists `clarify (:32-34)` under "Not ported (blocked on `workflowScript`)". **That doc is stale**: `:48` says the `workflowScript` identifier "appears nowhere in it", but `SubagentToolParams::workflow_script` is at `:95` and `crate::workflows::scripted` exists. Rejecting `clarify` never depended on `workflowScript` anyway |
| `extension/tool/params.rs:1334-1339` | **test** | `clarify_true` must not be background |
| `extension/tool/mod.rs:221` | comment | historical list |
| `extension/host/native_impl.rs:1126-1128` | prod | `[async]` badge hidden while `clarify: true` (v0.43.0 renderer; v0.68.0 dropped this) |
| `crates/cyrup-it/tests/subagents/subagent_tool_renderer_integration.rs:157-161` | **test** | pins the hidden badge |
| `extension/host/slash.rs:839`, `registration/prompt_workflows.rs:501-503` | doc | upstream sends `clarify:false`; cyrup has no field (correct) |
| `resources/skills/pi-subagents/SKILL.md:99,402-416,705,768` | bundled text shown to the model | a whole "## Clarify TUI" section plus three "avoid `clarify: true` unless…" clauses. Upstream v0.68.0 `SKILL.md` has **none** of these |
| `resources/docs/tool-reference.md:105` | bundled text (`include_str!` at `registration/guide.rs:68`) | `` `clarify` \| single \| Ask the child to clarify before working `` — wrong even about the old feature |
| `resources/prompts/review-loop.md:11` | bundled text | identical to upstream v0.68.0 `prompts/review-loop.md:11` |

**(b) The intercom `contact_supervisor` clarification channel (R-SA-037/119/120).** This is a
**different feature** and must not be touched by this row. It covers `exec::RunOptions::clarify`
(`exec/agent_config.rs:551-560`), `ClarifyDispatch`/`ClarifyChannel`/`AskLock`/`NoOpClarifyChannel`/
`spawn_clarify` (`tui/intercom.rs`, 101 hits), `exec/drive_attempt.rs:47,82,181,353,370-377`,
`exec/fallback.rs:1508-1524`, `exec/mod.rs:319-326`, `exec/run_result.rs:56-60`,
`exec/attempt_runner.rs:226`, `extension/executor/{mod.rs:154-158,327,486-516, foreground.rs:991,1231-1235, paths.rs:1321,1401,1489, workflow.rs:224,300, workflow_launch.rs:728, workflow_detach/mod.rs:19}`,
`extension/host/{mod.rs:39-44,185-221, registration.rs:163-186}`,
`background/runner_main/executor.rs:660,889-892`, `herdr/{runtime.rs:52,310, state.rs:212}`,
and `exec/testsupport.rs:101`. In short, the parent's human answers a question the **child**
asked. Nothing here previews a launch.

**(c) Name coincidences:** the prompt template `gather-context-and-clarify`
(`registration/resources.rs:182`, `prompt_workflows.rs:618`,
`tests/bundled_resources_registration_integration.rs:113`).

**Other public entry points:**
- RPC `spawn` goes through `extension/rpc/mod.rs:275-278` → `rpc/params.rs:196` and does not check
  `clarify`. This is the bug in §1.
- The child-safe fan-out registration (`SubagentTool::new_child_safe`) shares `Tool::execute`, so
  whatever lands at `mod.rs:255` covers both registrations, exactly as upstream's one boundary
  does.

---

## 4. Step 0 (both plans): refuse `clarify` on RPC spawn

This is needed under either plan. Plan A covers it through the shared boundary. Plan B needs the
v0.42.1 rule (`clarify: true` refused on RPC spawn) no matter how the tool treats the flag.

---

## 5. Plan A (recommended for parity): stop advertising `clarify` and refuse it as v0.68.0 does

### Shape

**Put the refusal in the shared boundary** so the tool and RPC cannot drift apart.

1. `extension/tool/params.rs:57`: widen the signature.
   ```rust
   pub(crate) fn normalize_public_subagent_execution(
       action: Option<&str>,
       clarify_present: bool,
   ) -> Result<Option<&str>, ToolError>
   ```
   - The blank-action check stays first (upstream `:133-135` before `:143`).
   - Add `if clarify_present { return Err(ToolError::new(CLARIFY_REFUSAL)); }` after it.
   - The refusal must also cover `{action:"status", clarify:false}`, because upstream refuses before
     action dispatch. So the check must run **before** the `Some(trimmed)` return. Order: blank
     check, then clarify check, then return the trimmed action.
   - `clarify_present` means the **key is present**. JSON has no `undefined`, so `!== undefined` is
     exactly "key present", and that includes `null` and non-boolean values.
2. `extension/tool/text.rs` (next to `BLANK_ACTION_REFUSAL`, `:540-543`): add
   `pub(crate) const CLARIFY_REFUSAL: &str = "Public workflowScript execution does not support clarify UI.";`
   Keep it verbatim. The text names no parameter that cyrup lacks, so the `BLANK_ACTION_REFUSAL`
   argument for a `[CYRUP-DELTA]` does not apply.
3. `extension/tool/mod.rs`: capture
   `let clarify_present = request.as_object().is_some_and(|m| m.contains_key("clarify"));`
   **before** `serde_json::from_value(request)` (`:213`), because parsing consumes `request`. Then
   pass it at `:255`. It is read from the raw map because, once the field is deleted (step 5),
   serde ignores the key (DI-SA-11 permissive parse), and a non-boolean `clarify` must still get
   upstream's refusal rather than a serde error.
4. `extension/rpc/params.rs:199`: pass `input.contains_key("clarify")`. This closes the
   foreground-RPC bug and matches `rpc.ts:516-517` @v0.68.0.
5. **Delete the field and everything that reads it:**
   - `params.rs:228` (field)
   - `:631-637` (`is_background`'s clarify term; the function becomes
     `async_param.unwrap_or(cfg.async_by_default)`; update its doc at `:616-623`)
   - `:781-782` (`provided_keys`)
   - `:1334-1339` (test assertion)
   - `schema.rs:556`, and fix the `:557` comment
   - `schema.rs:810` (remove from `expected_properties`)
   
   Upstream v0.68.0 keeps its leftover `effectiveAsync` term only because its internal
   `SubagentParamsLike` type still has the field. cyrup has one params type, so the term would be
   dead code. **cyrup can do better here by deleting it rather than copying the dead code.**
6. `extension/host/native_impl.rs:1126-1128`: change to
   `args.get("async") == Some(&Value::Bool(true))` only (v0.68.0 `index.ts:800`). Update the cite
   in the doc at `:1098` to @v0.68.0 for this branch.
7. **Docs:**
   - Rewrite `params.rs:41-53`: the `clarify` clause is now ported, and the "`workflowScript`
     appears nowhere" claim is false. Leave the remaining unported clauses as they are.
   - Delete `resources/docs/tool-reference.md:105`.
   - Delete `resources/skills/pi-subagents/SKILL.md:402-416` and the `clarify` clauses at
     `:99`, `:705`, `:768`. Upstream v0.68.0's SKILL.md has none of these, and telling the model to
     "set `clarify: false`" would now trigger a refusal.
   - `resources/prompts/review-loop.md:11` is byte-identical to upstream v0.68.0. Either keep it
     for parity, or apply `[CYRUP-DELTA]` by dropping the "unless I explicitly want…" clause.
     Recommend dropping it: the clause describes a UI that neither side has.

### Production call sites after the change
- `Tool::execute` (`extension/tool/mod.rs:255`) covers the root `subagent` tool and the child-safe
  fan-out tool.
- `rpc/params.rs:199` covers RPC `spawn`.
- There is no other public entry. The workflow engine's `runs.run` children and the slash/prompt
  adapters never set `clarify`.

### Tests (each with the mutation that kills it)

| # | Test (location) | Asserts | Mutation that kills it |
|---|---|---|---|
| A1 | `schema.rs` tests: `the_schema_does_not_advertise_clarify` | `subagent_tool_parameters()["properties"]` has no `"clarify"` key | re-insert `schema.rs:556` |
| A2 | `text.rs` tests (next to `the_public_execution_boundary_refuses_a_blank_action_and_trims_a_real_one`, `:959`): `the_public_boundary_refuses_any_clarify_value_on_both_registrations` | for **root and child-safe** tools, `dispatch_tool` with `{agent,task,clarify:true}`, `{…,clarify:false}`, `{…,clarify:null}`, `{…,clarify:"yes"}`, `{action:"status",clarify:false}` returns `Err` whose text `== CLARIFY_REFUSAL` | (i) drop the clarify check, so the dispatch proceeds and the text differs; (ii) test `parsed.clarify == Some(true)` instead of key presence, so the `false`/`null`/`"yes"` rows fail |
| A3 | same module: `a_blank_action_is_refused_before_clarify` | `{action:"  ",clarify:true}` returns `BLANK_ACTION_REFUSAL` | swap the two checks |
| A4 | `rpc/params.rs` tests (beside `:676-700`): `spawn_refuses_clarify` | `spawn_params(Some(&json!({"agent":"x","task":"t","clarify":true})))` is `Err(invalid_params)` with `CLARIFY_REFUSAL`; same for `clarify:false` | pass `false` for `clarify_present` at `:199`; the call then returns `Ok` with `async:true`, which is the foreground bug |
| A5 | `params.rs` tests: rewrite the `:1334-1339` assertion as `is_background_ignores_a_clarify_key` | `{agent,task,async:true,clarify:true}` parses (the key is ignored) and `is_background` is **true** | restore the `clarify != Some(true)` term |
| A6 | `cyrup-it/tests/subagents/subagent_tool_renderer_integration.rs:157-161` | `draw({"agent":"researcher","async":true,"clarify":true})` returns `"subagent researcher [async]"` | restore `native_impl.rs:1128` |
| A7 | `registration/guide.rs` tests: `bundled_docs_do_not_advertise_clarify` | `TOOL_REFERENCE` has no `` `clarify` `` table row, and the bundled pi-subagents SKILL has no `## Clarify TUI` heading and no `clarify: true` code example | restore either text |

The existing `every_advertised_schema_property_is_read_outside_provided_keys`
(`schema.rs:1209`) still holds, because the property and the field go away together.

### Size
**Small:** about 40 lines of production code across 6 `.rs` files, about 120 lines of tests, and
3 bundled-resource edits. No new types or dependencies.

### Risk
A model trained on the old skill text that sends `clarify: false` now gets an error instead of a
no-op. That matches upstream and is fixed by removing the skill text in the same change. An
external RPC client sending `clarify` gets `invalid_params`. Upstream does the same at both tags.

---

## 6. Plan B (`[CYRUP-DELTA]`, owner decision): port the v0.42.1 launch preview

### Why the owner might choose this
cyrup deliberately kept the legacy single/parallel/chain public shapes that upstream cut at v0.43.0
(`params.rs:41-53`). The v0.42.1 preview was designed for exactly those shapes. Keeping the shapes
while dropping their preview is a defensible choice (Plan A). Keeping both is also defensible, but
it is a **divergence from v0.68.0** and must be labeled as one.

### Host primitives needed. All of them exist; there is no blocker

| Need | Primitive | Where | Evidence it works from a tool call |
|---|---|---|---|
| Modal preview that blocks until closed | `HostServices::open_overlay(Box<dyn InteractiveOverlay>) -> bool` | `cyrup-ext/src/host/services.rs:332`; trait `host/overlay.rs:329-375` (`render`, `handle_key`, `options`) | TUI side: `LiveHostServices::open_overlay` (`cyrup-session-svc/src/host_services.rs:1318-1350`) blocks the calling task with `block_in_place`. Its only invariant is "not from the TUI run-loop task". A tool's `execute` runs on the agent task, and `routing.rs:1184` already blocks there on `services.confirm` |
| Get the result out of a `bool` return | an `Arc<Mutex<Option<R>>>` written on close | precedents `cyrup-mcp/src/ui.rs:5250-5270` (MCP-369) and `cyrup-permission-system/src/extension/command.rs:199-211` | shipped |
| hasUI / headless probe | `open_overlay` returns `false` when no overlay sink is installed (print, json, **and RPC**, `cyrup-tui/src/app/extension_ui.rs:240-262`) | same | shipped |
| Task text editing | `HostServices::editor(title, initial) -> Option<String>` | `services.rs:299` | used by this crate at `registration/subagents_admin.rs:1179` |
| Model / skills / thinking lists | `HostServices::models()` / `scoped_models()` (`services.rs:589,778`); `discovery::skills::discover_available_skills` (`discovery/skills.rs:242`); `exec/thinking_ceiling.rs` | | in-crate |
| Serialize with other human prompts | `HostServices::human_interaction_lock()` | `services.rs:495` | used by the permission gate and the intercom clarify |
| Keyboard | `OverlayKey{code: OverlayKeyCode::{Char,Enter,Escape,Up,Down,…}, ctrl, alt, shift}` | `overlay.rs:176-240` | there is **no paste variant**, which drives the design choice below |
| Registry of ready-made overlays in this crate | `tui/fleet_overlay.rs:178` (`impl InteractiveOverlay for FleetOverlay`) | | shipped |

The only new wiring is the preview hook in `Tool::execute`. No host crate change is required.

### Shape
- **Name it `launch_preview`, not `clarify`.** The crate already uses `clarify` for the intercom
  `ClarifyChannel`/`spawn_clarify` feature (§3b). A second, unrelated `clarify` module would repeat
  the confusion this row warns about.
- **New `crates/cyrup-ext-subagents/src/tui/launch_preview.rs`:**
  ```rust
  pub(crate) enum PreviewMode { Single, Parallel, Chain }
  pub(crate) struct PreviewStep { agent: String, task: String, model: Option<String>, thinking: Option<String>,
                                  output: Option<OutputOverride>, reads: Option<Vec<String>>, progress: Option<bool>,
                                  skills: Option<Vec<String>> }
  pub(crate) enum PreviewOutcome { Run { steps: Vec<PreviewStep>, background: bool },
                                   EditTask(usize), PickModel(usize), PickThinking(usize), PickSkills(usize),
                                   Cancelled }
  pub(crate) struct LaunchPreviewOverlay { mode, steps, selected, background, out: Arc<Mutex<Option<PreviewOutcome>>> }
  impl InteractiveOverlay for LaunchPreviewOverlay { fn render(..); fn handle_key(..); fn options(..) /* width 84, maxHeight 80% */ }
  ```
- **Where cyrup can do better:** the overlay **closes** with `EditTask(i)` / `PickModel(i)` / …
  The tool task then runs the real host dialog (`services.editor` / `select`) and reopens the
  overlay with the updated state. This reuses the TUI's inline editor, which handles paste,
  wrapping and the `Ctrl+G` external editor. It avoids porting upstream's ~200-line in-component
  buffer editor (`chain-clarify.ts:36-170`), which could not receive paste through `OverlayKey`
  anyway.
- **New `crates/cyrup-ext-subagents/src/extension/tool/launch_preview.rs`:**
  ```rust
  impl SubagentTool {
      pub(crate) async fn preview_launch(&self, p: &SubagentToolParams, cwd: &Path)
          -> Result<PreviewDecision, ToolError>;
  }
  pub(crate) enum PreviewDecision { Proceed(SubagentToolParams), Cancelled(ToolResult), Headless }
  ```
  This rewrites `task` / `tasks[i].task` / `chain[i].task` and the per-step `model`, `output`,
  `reads`, `progress` and `skill`, and sets `async: Some(true)` when Background is chosen. It
  **rewrites params before dispatch** instead of patching three mode arms. Upstream needed three
  copies (`:3162`, `:3546`, `chain-execution.ts:657`); cyrup gets one.
- **Hook** in `extension/tool/mod.rs`:
  - Place it after `canonicalize_execution_params` (`:370-372`), so the preview shows canonical
    agent names.
  - Place it **before** `prepare_mission_binding_for_dispatch` (`:383`), so a cancelled preview
    creates no mission. **This is better than upstream**, where `attachMission` wraps the whole
    execution.
  - Run it only when `clarify == Some(true)`, the call is not `has_workflow`, and it is not a chain
    with parallel steps.
  - Keep `is_background`'s `clarify != Some(true)` term. Background relaunch happens only by
    the human's `b` choice.
- **Refusals to add** (v0.42.1 behavior):
  - `workflowScript` + `clarify: true` gets `"workflowScript does not support clarify UI."`
    (`:4049-4050`).
  - RPC spawn + `clarify: true` gets the `rpc.ts:432-433` text (Step 0).
- **Headless (`open_overlay` returns `false`):**
  - Launch the unedited run in the foreground, as upstream's print mode does.
  - **Do better than upstream:** add a result note such as
    `"clarify preview unavailable: no interactive terminal; launched as requested"`, so the model
    is not misled.
  - Do **not** copy upstream's RPC quirk, where `custom()` returns `undefined` and the run is
    silently `"Cancelled"`.
- **Spawn budget.** `reserve_subagent_spawns` (`executor/spawn_budget.rs:71`) runs at `mod.rs:358`,
  before the hook, and there is no refund API. Upstream also charges before the preview
  (`reserveSpawnBudget` at `:5004` @v0.42.1). **To do better,** add
  `release_subagent_spawns(requested)` and call it on `Cancelled`.

### Tests (each with the mutation that kills it)

| # | Test | Asserts | Mutation |
|---|---|---|---|
| B1 | overlay unit: `enter_confirms_with_edited_state` | `handle_key(Enter)` gives `Close` and writes `Run{steps, background:false}` | write `Cancelled` on Enter |
| B2 | overlay unit: `escape_and_ctrl_c_cancel` | both write `Cancelled` | drop the `ctrl` arm |
| B3 | overlay unit: `b_toggles_background_and_footer_shows_it` | `Background:ON` shows in the rendered footer and `Run{background:true}` | make `b` a no-op |
| B4 | overlay unit: `w_is_ignored_in_parallel_r_p_only_in_chain` | the upstream per-mode key gating (`:497-515`) | remove the mode guard |
| B5 | tool: `cancel_spawns_no_child_and_returns_cancelled` (a fake `HostServices` whose `open_overlay` drives Esc) | the result text is `"Cancelled"`, not an error; the executor recorded zero spawns; no mission file | skip the `Cancelled` early return |
| B6 | tool: `edited_task_reaches_the_child` (a fake that writes a new task via the `EditTask`→`editor` loop) | the spawned child's argv/prompt contains the edited text | dispatch the original `parsed` |
| B7 | tool: `background_choice_relaunches_async` | the result is the async launch receipt | ignore `background` |
| B8 | tool: `headless_launches_unedited_with_note` (`open_overlay` returns `false`) | the child runs with the original task and the note is present | return `Cancelled` on `false` (upstream's RPC behavior) |
| B9 | tool: `chain_with_parallel_step_skips_preview` | `open_overlay` is never called | drop the `hasParallelSteps` guard |
| B10 | tool: `workflow_script_with_clarify_is_refused` | the `:4049-4050` text | delete the refusal |
| B11 | rpc: `spawn_refuses_clarify_true` | the `rpc.ts:432-433` text | drop the check, which lets the RPC spawn run in the foreground |
| B12 | tool: `cancel_refunds_spawn_budget` (only if the refund is added) | the snapshot count is unchanged after cancel | remove the release call |

### Size
**Large:** about 700–1000 lines for the overlay (render for three modes, model/skills/thinking
pickers as host `select` round-trips), about 250 lines for the hook and param rewrite, and about
600 lines of tests with a scripted fake `HostServices`. It is less than upstream's 1350+ lines
because task editing is delegated to the host editor.

**The hardest part** is the param rewrite. The single spec, `ToolTaskItem` (parallel, with
`count` expansion, `routing.rs:2628-2635`) and the chain graph (`parse_tool_chain_items`, where
parallel steps are nested) each have different shapes. Edits must map back onto the **unexpanded**
input so that re-dispatch re-validates the same way. A second difficulty is proving the
`EditTask` → `editor` → reopen loop never runs on the TUI loop task.

---

## 7. Real blockers
**None.** Every primitive Plan B needs is shown to exist in §6, with its file and line. Plan A
needs no new primitive.

## 8. Corrected row text (for `PARITY-GAPS.md:1428`)

> **PB-9 · `clarify` is advertised and accepted, but upstream refuses it at v0.68.0** — *small (parity) / large (if the v0.42.1 preview is kept as a `[CYRUP-DELTA]`)* · **OPEN at `521beaa`, re-derived 2026-09-22**
> - upstream @v0.68.0: `clarify` is **not in the schema** (`schemas.ts`, 0 hits) and the public boundary refuses **any** value: `public-execution.ts:143-145` → `"Public workflowScript execution does not support clarify UI."`, reached from the tool (`index.ts:738`), RPC spawn (`rpc.ts:516`) and slash (`slash-bridge.ts:82`). The preview UI (`ChainClarifyComponent`) was removed from the schema in `39c37184` (first tag v0.43.0) and **deleted** in `ef554d2a` #1166 (first tag v0.51.0). The previous `chain-clarify.ts:199` / `subagent-executor.ts:3190,:3572` / `chain-execution.ts:692` cites resolve only at v0.43.0, a tag where the public tool already refused `clarify`.
> - cyrup: advertises it at `extension/tool/schema.rs:556` with the v0.42.1 description, parses it at `params.rs:228`, and uses it only in `is_background` (`params.rs:631-637`, forces foreground) and the `[async]` badge (`native_impl.rs:1126-1128`). The bundled `SKILL.md:402-416` and `docs/tool-reference.md:105` advertise a UI that does not exist.
> - **bug:** RPC `spawn` (`rpc/params.rs:196-223`) never checks `clarify`, so `{…, clarify:true}` runs in the FOREGROUND despite being forced `async:true`. Upstream refuses this at v0.42.1 (`rpc.ts:432-433`) and at v0.68.0 (`rpc.ts:516`).
> - fix: `.flux/todo/CLARIFY_PREVIEW.md`. Plan A (v0.68.0 parity) or Plan B (port the preview as a `[CYRUP-DELTA]`; every host seam exists — `open_overlay`, `editor`, `select`, `human_interaction_lock`; the in-crate `open_overlay` precedent is `extension/host/terminal_input.rs:170`, not the stale `slash.rs:147`).
