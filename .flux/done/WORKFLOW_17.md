---
stage: qa
status: completed
updated: 2026-09-14 01:00
---

# WORKFLOW_17 — `runs.status()` on a LIVE child

> **Created 2026-09-13.** `WorkflowRunHost::status` answers only from a settled `Vec`. A script
> that polls a running child gets an error string saying the child does not exist.

OBJECTIVE: make `runs.status(keyOrRunId)` report a **running** child's live progress, not an
error. This is the last `runs.*` verb whose failure mode actively misleads the script author.

**Alignment with pi-subagents core definition of done** (VISION.md "Evidence closes work" +
"Background work stays visible"):
A live child must surface concrete evidence of its state (activity line, turn/tool/token counters,
run_id) so the script author can observe progress without guessing. The current error
("names no launched child") is exactly the optimistic-failure mode the artisan contract
forbids — it hides running work and lies about launch state. The three-arm status (settled
→ live → unknown) plus `LaunchIdentity::run_id` back-fill close the evidence gap.

Stack: **Rust**. One crate: `cyrup-ext-subagents`.

**Hard prerequisite: [`WORKFLOW_13`](WORKFLOW_13.md)** — its step inventory (§3.3) is the on-disk
half of the answer and its `foreground_controls` registration is the live half.

---

## ⚠ RE-AUGMENTED 2026-09-14 AGAINST HEAD — READ THIS SECTION FIRST

This spec was first augmented before commit `68facb9` ("workflows 16,17") landed. **The objective
is NOT implemented at HEAD** — `status` is still settled-only. But the *mechanism* §1.1 prescribes
is now known to be **unworkable**, and several citations moved. Everything below is verified against
the tree as it stands today.

### A. Corrected file:line citations

| spec said | actual at HEAD |
|---|---|
| `workflow.rs:388-407` — `status` | **`workflow.rs:553-572`** (doc `546-552`) — body byte-identical, just moved |
| `workflow.rs:298-307` / `:300` — `or_insert_with` | **`workflow.rs:447-463`** — it is **NO LONGER `or_insert_with`**; WORKFLOW_14 replaced it with a `launched.entry(key)` `match` over `Entry::Occupied`/`Entry::Vacant`. `slot.insert(LaunchIdentity { … })` is **`workflow.rs:453-459`** |
| `engine.rs:983-1052` — `run_status` | **`crates/cyrup-ext-subagents/src/workflows/scripted/engine.rs:988-1054`** (note the `scripted/` path segment; there is no `workflows/engine.rs`) |
| `engine.rs:315` — `launches: HashMap<String, LaunchRecord>` | **`scripted/engine.rs:320`**, inside `RunInner` (declared `:313-321`); `LaunchRecord` itself is `scripted/engine.rs:301-305`. Claim holds: `run_status` still does not consult it |
| `notices.rs:20-48` — `ForegroundControlEntry` | **`notices.rs:20-96`** — the struct grew; the fields this task needs are named individually in §B below |
| `foreground_control.rs:20-44` — `ForegroundChildEntry` | **`foreground_control.rs:131-165`**; `:20-44` is now `ForegroundChildSteerHandle`'s doc |
| `routing.rs:590` — "the on-disk step inventory `status.steps`" | **WRONG FILE.** `routing.rs:590` is the workflow run-dir creation (`run_dir = run_dir_name.resolve_in(&async_root)`). `status.steps` is written by **`WorkflowRunHost::publish_steps`, `workflow.rs:200-222`**, from **`workflow_step_statuses`, `crates/cyrup-ext-subagents/src/workflows/child_summary.rs:690-716`** |
| `prelude.js:311` — `status(keyOrRunId)` | **HOLDS EXACTLY**, but the file lives in a **different crate**: `crates/cyrup-workflow-runtime/src/js/prelude.js:311-313`. It is read-only reference; this task touches one crate as stated |
| `workflow.rs:300` write is pre-await | **HOLDS.** Insert at `:453`; the `run_foreground_streaming` await is `:477-528`. The `launched` entry exists for the child's whole lifetime |

### B. The real `LaunchIdentity`, verbatim at `workflow.rs:87-96`

The spec's §1.1 snippet has **three wrong facts**. The struct is not `pub(crate)`, and the two
override fields are `String`, not the typed ids:

```rust
#[derive(Clone)]                       // :87
struct LaunchIdentity {                // :88  — private, NOT pub(crate)
    agent: String,
    model: Option<String>,             // :91  — NOT Option<ModelId>
    agent_scope: Option<String>,       // :92  — NOT Option<AgentScope>
    context: Option<ContextRequest>,
    index: usize,                      // :95
}
```

(`model` is converted at the call site — `model.map(ModelId::from)`, `workflow.rs:493`; `agent_scope`
goes through `resolve_execution_agent_scope(agent_scope.as_deref())` at `:491`. A new field must match these
actual types or `launch` will not compile.)

### C. 🔴 §1.1's mechanism CANNOT WORK — the run id does not exist until after settlement

§1.1 says to write the run id "as soon as `run_foreground_streaming` mints it", and its proposed
doc comment claims `None` lasts "microseconds". **Both are false.** Verified chain:

- `run_foreground_streaming` (`extension/executor/foreground.rs:250-256`) returns
  `Result<(SingleResult, RunId), SubagentError>` — the `RunId` comes back **in the return tuple**.
- It delegates to `run_foreground_impl` (`foreground.rs:262`), which mints the id inside
  `resolve_run_channels` — **`let run_id = RunId::new();` at `foreground.rs:641`**.
- `ForegroundRunRequest` (`extension/executor/requests.rs`) has **no `run_id` field**; there is no
  way to pre-mint one and hand it in.
- So `launch` first observes the run id at **`workflow.rs:477`** (`let (result, run_id) = self…`),
  which is *after* the `.await` at `:527` — i.e. **after the child has settled**, at which point
  `map_child_result` (`:267-…`) already puts it on the settled entry and arm 1 answers.

**`LaunchIdentity.run_id` would therefore be `None` for the entire lifetime of every running child
— exactly the window it was added to serve.** Do not implement §1.1 as written; it is a field with
no live reader, which is the dead-code shape this programme's DoD forbids.

### D. ✅ The mechanism that DOES work — already in the tree, nothing to plumb

`ForegroundControlEntry` is registered **for exactly the duration of the child's run** and is
already stamped with both halves of the child's workflow identity:

- registered: `foreground.rs:341` (`register_foreground_controls`), inserted into the map at
  **`foreground.rs:1022-1025`**, keyed by **`run_id.as_str().to_string()`** — the child's real run id.
- torn down: `foreground.rs:377` (`settle_foreground_run`, defined `:1054`), which `remove`s it.
- the registry itself: **`extension/executor/mod.rs:137`** —
  `foreground_controls: Arc<std::sync::Mutex<HashMap<String, ForegroundControlEntry>>>`.

Identity + live counters on the entry (`notices.rs`):

| field | line | value for a workflow child |
|---|---|---|
| `parent_workflow_run_id: Option<RunId>` | `:73` | `Some(self.workflow_run_id)` — stamped `foreground.rs:981` |
| `workflow_key: Option<WorkflowKey>` | `:78` | `Some(WorkflowKey)` for the launch `key` — stamped `foreground.rs:982` |
| `session_id: Option<SessionId>` | `:68` | the live parent session |
| `active_children: BTreeMap<usize, ForegroundChildEntry>` | `:95-96` | exactly one entry, at index `0` |
| `current_activity_state` / `current_tool` / `current_path` | `:34` / `:44` / `:46` | the activity line |
| `turn_count` / `tool_count` / `tokens` (`Option<u64>`) | `:48` / `:50` / `:52` | the live counters |
| `current_agent` / `current_index` | `:26` / `:28` | derived by `begin_foreground_child`, never hand-written |
| `started_at` / `updated_at` (`i64` epoch ms) | `:57` / `:62` | |
| `mode: RunMode` / `description: Option<String>` | `:38` / `:41` | `RunMode::Single`; the child's task text |

**So the live arm resolves by `(parent_workflow_run_id == self.workflow_run_id) && (workflow_key == key)`,
or by the map key when the guest passed a run id.** `LaunchIdentity` supplies the agent name and
`index`; the control entry supplies everything live. §0.2's "Nothing new needs to be plumbed" is
*more* true than the spec knew — and `LaunchIdentity` needs no new field at all.

⚠ **`active_children`'s key is NOT this task's `index`.** It is the child's index within its *own*
foreground run, always `0` for a workflow child — stated at `foreground_control.rs:66-70` and
`foreground.rs:989-993`. `LaunchIdentity.index` is the WORKFLOW-flat index. Two namespaces; the
live counters live at `active_children[&0]`, or read the parent-level fields that
`sync_current_child` (`foreground_control.rs:171`) keeps derived from it.

**Prior art to copy, not reinvent:** `control_is_live_in_workflow`
(**`extension/executor/workflow_steering.rs:84-92`**) is the exact three-term predicate
(`parent_workflow_run_id` == workflow, `session_id` == current, `!active_children.is_empty()`), and
`resolve_workflow_foreground_steering_target` (`workflow_steering.rs:210-222`) shows the
lock → `iter().filter(…)` → `map(|(k, c)| (k.clone(), c.clone()))` → `collect` shape, with the
comment explaining why the run id must be carried out as the map key.

**Visibility:** `SubagentExecutor::foreground_controls()` (`mod.rs:461-467`) is **`#[cfg(test)]`** —
do not reach for it. The *field* at `mod.rs:137` has no `pub`, so it is visible throughout module
`extension::executor` **and its descendants**; `workflow.rs` is such a descendant, so
`self.executor.foreground_controls.lock()` compiles in production exactly as `workflow_steering.rs`
already does with `self.foreground_controls`. Use `.unwrap_or_else(PoisonError::into_inner)`, the
established idiom in both files (`unwrap_used`/`expect_used` are DENY workspace-wide,
`Cargo.toml:101-104`).

### E. 🔴 THE BLOCKER §1.2 MUST SOLVE — the engine turns `ok == false` into an ERROR

§1.2 says "`ok`/`stopped` remain false until settlement". **That does not meet the objective.**
`run_status` at **`scripted/engine.rs:1046-1050`**:

```rust
shared.bump();
if result.ok {
    Ok(omit_non_json_workflow_result_metadata(&result))
} else {
    Err(format!("Status '{key}' failed: {}", result.output))
}
```

A live-arm result with `ok: false` is re-wrapped as `Err("Status 'lane' failed: <live output>")` and
reaches the script as a **thrown error** — swapping one misleading error for another and leaving the
objective unmet. It also files a `WorkflowScriptTraceState::Failed` trace entry (`:1036`) for a child
that is merely running.

**Required resolution (implementor must pick one and state it in the code comment):**
1. **Host-only (preferred, keeps the one-crate/one-seam scope):** the live arm returns `ok: true` —
   meaning *the status query succeeded* — and carries running-ness in the payload
   (`output` = the activity line + counters, `run_id` = the live child run id, `stopped`/`interrupted`/
   `detached` all `false`, `error: None`). This is the only option that satisfies the objective
   without touching the engine.
2. Widen `run_status`'s gate at `engine.rs:1046-1050` to admit a live result. This edits the engine
   and must be justified against §3's out-of-scope list.

Option 1 is the single prescriptive path unless the implementor documents why it fails.

`WorkflowScriptChildResult` **derives `Default`** (`workflows/types.rs:309`), so the live arm is
`WorkflowScriptChildResult { key, ok, agent, run_id, output, ..Default::default() }` — do **not**
hand-write the ~20 `None`/`false` fields. Full shape: `types.rs:309-385`; every field is
`#[serde(rename_all = "camelCase")]` on the wire.

### F. Verified-unchanged facts (§0 and §0.1 still stand)

- `status` body is still settled-only and still emits *"names no launched child in this workflow."*
  (`workflow.rs:569-571`). **Objective is NOT satisfied at HEAD.**
- §0.1 holds verbatim: `run_status` resolves key→run_id from `inner.children` (settled) only
  (`engine.rs:996-1006`), so an in-flight child arrives at `host.status` as **the raw key string**.
  The seam is correct; the host must answer.
- The `launched` map is written **pre-await** (`:453` vs `:527`). Load-bearing fact confirmed.
- `run_status` refuses an empty string before reaching the host (`engine.rs:992-994`) and short-circuits
  with `Err("Workflow script aborted.")` while `finishing` (`:1014-1018`) — neither needs handling here.
- The only other `LaunchIdentity` construction site is the **test helper `host_with`,
  `workflow.rs:768-775`**. §1.3's "all construction sites" = these two (`:453` and `:768`) — and if §C
  is honoured, neither changes.

---

## 0. What is broken

```rust
// extension/executor/workflow.rs:553-572   [was cited as :388-407]
async fn status(&self, key_or_run_id: &str, _cancel: CancelToken)
    -> Result<WorkflowScriptChildResult, String> {
    let found = self.settled.lock().ok().and_then(|settled| {
        settled.iter().find(|c| c.key == key_or_run_id)
            .or_else(|| settled.iter().find(|c| c.run_id.as_deref() == Some(key_or_run_id)))
            .cloned()
    });
    found.ok_or_else(|| format!("runs.status('{key_or_run_id}') names no launched child in this workflow."))
}
```

The doc above it calls `settled` *"the ONLY thing `status` can answer from in a foreground host,
because a launch does not return until its child settles"*. That is true of the **result**, and
false of the **status** — which is the entire point of a status verb.

That doc comment is at **`workflow.rs:546-552`** (and the same claim is repeated on the `settled`
field itself, **`workflow.rs:111-123`**). Both must be corrected as part of this change: leaving a
comment that says the host *cannot* answer live, next to a host that now does, is the same
misleading-evidence failure the objective names.

The message is also wrong twice over: for a running child the key HAS been launched, and the
sentence tells the author it was not, so the natural fix is to add a launch that already exists.

### 0.1 The engine hands the host the raw KEY for an in-flight child — deliberately

`run_status` (`workflows/scripted/engine.rs:988-1054`) resolves key→run_id **from settled children
only** (`:996-1006`):

```rust
let (target, known_run_id, finishing) = {
    let inner = shared.lock();
    let known = inner.children.get(&key);            // SETTLED children only
    (known.and_then(|c| c.run_id.clone()).unwrap_or_else(|| key.clone()), ..)
};
```

For an in-flight child there is no `run_id` yet, so `target` is **the key string**, passed straight
to `host.status` (`engine.rs:1023`). There is no engine-side notion of "running" — *"that answer
comes entirely from the host."* The engine also tracks `launches: HashMap<String, LaunchRecord>`
(`engine.rs:320`, never removed on settlement) but `run_status` does not consult it.

So the seam is already correct. The host simply has to answer.

### 0.2 The host already has everything it needs

| fact | where | populated | citation |
|---|---|---|---|
| the key was launched, with which agent, at which flat index | `self.launched: Mutex<HashMap<String, LaunchIdentity>>` | `workflow.rs:453` — **before** the `run_foreground_streaming` await at `:527` | [`workflow.rs:125`](crates/cyrup-ext-subagents/src/extension/executor/workflow.rs#L125), [`:447-463`](crates/cyrup-ext-subagents/src/extension/executor/workflow.rs#L447) |
| the live child's run id, workflow owner and lane key | `foreground_controls[run_id]` → `parent_workflow_run_id` / `workflow_key` | `foreground.rs:981-982`, registered `:341`, dropped `:377` | [`notices.rs:73`](crates/cyrup-ext-subagents/src/extension/executor/notices.rs#L73), [`:78`](crates/cyrup-ext-subagents/src/extension/executor/notices.rs#L78) |
| live activity/tool/turn/token counters | `ForegroundControlEntry` fields + `active_children[&0]` (`ForegroundChildEntry`) | folded by `foreground_control_notifier`; kept derived by `sync_current_child` | [`notices.rs:34-62`](crates/cyrup-ext-subagents/src/extension/executor/notices.rs#L34), [`foreground_control.rs:131`](crates/cyrup-ext-subagents/src/extension/executor/foreground_control.rs#L131) |
| the child's flat index | `LaunchIdentity.index` | W14 §3.1 | [`workflow.rs:95`](crates/cyrup-ext-subagents/src/extension/executor/workflow.rs#L95) |
| the on-disk step inventory | `status.steps`, via `publish_steps` | W13 §3.3 | [`workflow.rs:200`](crates/cyrup-ext-subagents/src/extension/executor/workflow.rs#L200), [`child_summary.rs:690`](crates/cyrup-ext-subagents/src/workflows/child_summary.rs#L690) |

Nothing new needs to be plumbed. `launched` being written pre-await is the load-bearing fact —
**re-verified at HEAD: `:453` insert, `:527` await.** (Note the inventory only ever contains
*settled* children — `workflow_step_statuses` maps `(ok, stopped)` onto `StepState`
(`child_summary.rs:700-704`) and has no running variant — so it is background context, not the
live source.)

---

## 1. Required changes (prescriptive, single path)

### 1.1 `LaunchIdentity` gains the live run id (most feature-rich option)

> 🔴 **SUPERSEDED BY §C — DO NOT IMPLEMENT AS WRITTEN.** The run id is minted at
> `foreground.rs:641`, inside the awaited call, and is first observable by `launch` at
> `workflow.rs:477` — *after* the child has settled. A `LaunchIdentity.run_id` field would be
> `None` for every child's entire running lifetime, i.e. dead in exactly the window it targets.
> The original intent — *"`status` must be able to name the live child's real run id"* — is
> **preserved in full** and is satisfied by §D's mechanism, which reads that id as the
> `foreground_controls` **map key** (`foreground.rs:1022-1025`) for the entry whose
> `parent_workflow_run_id`/`workflow_key` match this host and this key. Requirement kept, source
> corrected. Do **not** synthesize a second id (that prohibition is unchanged and now binding on
> the §D path).

Original text, for the record:

> Add the child's real run id to the identity record, written as soon as `run_foreground_streaming`
> mints it:
>
> ```rust
> pub(crate) struct LaunchIdentity {
>     agent: String,
>     model: Option<ModelId>,
>     agent_scope: Option<AgentScope>,
>     context: Option<ContextRequest>,
>     index: usize,
>     /// The child's real run id, as soon as it exists. `None` only in the window between
>     /// `or_insert_with` (workflow.rs:300) and the run id being minted — microseconds.
>     run_id: Option<crate::background::RunId>,
> }
> ```
>
> Update the `or_insert_with` site and the place where the run id is first known (in `launch` or the
> streaming call site) to populate it. Do **not** synthesize a second id.

(Also note: the real struct is private and uses `Option<String>` for `model`/`agent_scope` — §B.
And the site is a `launched.entry(key)` `match`, not `or_insert_with` — §A.)

### 1.2 `status` implementation (three-arm: settled → live → unknown)

Replace the current `status` body (`workflow.rs:553-572`) with the prescriptive three-arm lookup
that prefers settled, then falls back to live `launched` + foreground control state, then unknown.

**Arm 1 — settled.** Unchanged: the existing `settled` lookup by key, then by `run_id`.

**Arm 2 — live.** Take `self.launched` for the agent name and flat `index` (key lookup), then lock
`self.executor.foreground_controls` and find the one entry where
`entry.parent_workflow_run_id.as_ref() == Some(&self.workflow_run_id)` **and**
(`entry.workflow_key.as_ref().map(WorkflowKey::as_str) == Some(key_or_run_id)` **or** the map key
`== key_or_run_id`, since the guest may pass either — `prelude.js:311`). Clone the entry out of the
lock (`ForegroundControlEntry: Clone`; the `workflow_steering.rs:210-222` pattern). Build the result
from `WorkflowScriptChildResult::default()` with: `key`, `agent` from `LaunchIdentity`, `run_id` =
the map key, and `output` = the live activity line assembled from
`current_activity_state`/`current_tool`/`current_path`/`turn_count`/`tool_count`/`tokens`
(parent-level fields, which `sync_current_child` keeps derived from `active_children[&0]`).
`stopped`/`interrupted`/`detached` stay `false` and `error` stays `None` — the child has not settled.

> 🔴 **`ok` MUST be `true` on this arm** — see §E. `engine.rs:1046-1050` converts `ok == false` into
> `Err("Status '{key}' failed: …")`, which would defeat the objective. `ok` here means *the status
> query succeeded*; running-ness is carried by the payload. The original sentence
> *"`ok`/`stopped` remain false until settlement"* is correct for `stopped` and **must be reversed
> for `ok`**, or the engine's gate must be widened (§E option 2) with justification.

**Arm 3 — unknown.** The existing `Err` — but its wording is only correct for a key that was truly
never launched. Reached now only when the key is in neither `settled` nor `launched`+controls.

### 1.3 `LaunchIdentity` construction sites updated (no duplication)

All sites that create `LaunchIdentity` must stay consistent. Verified: there are exactly **two** —
the production `slot.insert(LaunchIdentity { … })` at **`workflow.rs:453-459`** and the test helper
`host_with` at **`workflow.rs:768-775`**. Under §C's correction no new field is added, so neither
changes; the requirement stands as a check, and **no second run id is minted** either way.

---

## 2. Definition of done

The single required implementation path has been followed exactly:

- ~~`LaunchIdentity` has the `run_id: Option<RunId>` field~~ → **superseded by §C**: the live
  child's real run id is reported on the live arm, sourced from the `foreground_controls` map key.
  If the implementor finds a way to make the field live *before* settlement, §1.1 as written is
  still acceptable — but the field must have a real reader in the running window or it is dead code.
- `status` implements the three-arm lookup (settled → live from `launched` + foreground controls → unknown)
- A live child's status returns **`Ok`** to the script, not a thrown error (§E) — this is the objective
- All construction sites (`workflow.rs:453`, `:768`) consistent; no second run id is minted
- The stale doc comments at `workflow.rs:546-552` and `workflow.rs:111-123` ("the ONLY thing
  `status` can answer from") are corrected to match the new behaviour
- No test/benchmark/doc language remains

This closes the live-status evidence gap per VISION.

---

## 3. Explicitly out of scope

| deferred to | what |
|---|---|
| not this task | detached children (none exist per W13). |
| not this task | index-addressed status (key-unique). |
| not this task | changing `run_status`'s `ok` gate (`engine.rs:1046-1050`) — unless §E option 1 is shown not to work, in which case it is in scope and must be justified in the code. |
| not this task | the session-identity term of `control_is_live_in_workflow`. The live arm is already scoped to `self.workflow_run_id`, which is process-local and minted by this call (`routing.rs:583`); adding the `session_id` term is harmless and consistent with `workflow_steering.rs:84-92`, but is not required to make the lookup correct. |
