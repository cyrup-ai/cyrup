---
stage: qa
status: completed
updated: 2026-09-14 01:00
---

## ⟦AUG 2026-09-14⟧ Re-verification against HEAD — READ THIS FIRST

This spec was augmented at **2026-09-13 02:52**, BEFORE commit `68facb9` ("workflows 16,17") landed
361 lines across `executor/workflow.rs`, `executor/workflow_steering.rs` and `tool/routing.rs`.
Every citation in §§0-3 was re-opened at HEAD. **Nothing below removes or softens scope** — this
section corrects line numbers, kills two claims that are now provably false, supplies the real
`execute_workflow_host_command` signature the spec told the implementor to go look up, and rewrites
the §2 acceptance commands, which as written **cannot pass**.

**Verdict: NOT implemented at HEAD. The whole task is live.**

```
grep -n 'fn supports_host'  crates/cyrup-ext-subagents/src/extension/executor/workflow.rs  -> 0 hits
grep -n 'fn host_command'   crates/cyrup-ext-subagents/src/extension/executor/workflow.rs  -> 0 hits
routing.rs:688   on_host_step: None,
routing.rs:857   host_steps: &[],          // `on_host_step: None` in this build — WORKFLOW_19
```

**The hard prerequisite is SATISFIED.** W13 §3.2 landed: `settle_foreground_workflow` now exists as
a real function at `/home/user/cyrup/crates/cyrup-ext-subagents/src/extension/tool/routing.rs:824`,
called from both arms (`:723` success, `:787` failure). Only its `host_steps: &[]` argument (`:857`)
is left to replace. Nothing about W13 blocks this task.

---

### A. Corrected citations (spec line → HEAD line)

All paths absolute; all verified by opening the file at HEAD.

**`/home/user/cyrup/crates/cyrup-ext-subagents/src/workflows/host_command.rs`** — the §0 table is
**entirely accurate**; this file did not move.

| spec says | HEAD |
|---|---|
| `:834` `execute_workflow_host_command` | ✅ `:834` (signature), doc block `:799-833` |
| `:664` child reaping / timeout | ✅ `run_and_drain` — doc at `:662`, `fn` just below |
| `:193` `normalize_workflow_host_command_params` | ✅ `:193` |
| `:101` params / `:139` result | ✅ `WorkflowHostCommandParams` `:101`, `WorkflowHostCommandResult` `:139` |
| `:337` `resolve_workflow_host_output_claim_path` | ✅ `:337` |

**`/home/user/cyrup/crates/cyrup-ext-subagents/src/workflows/host_step.rs`**

| spec says | HEAD |
|---|---|
| `:183-230` `HostStepNode` | ✅ `pub struct HostStepNode` at `:183`, closes `:229` |
| — | `HOST_STEP_MAX_COUNT = 32` at `:177`; `assert_unique_host_step_ids` at `:242` |

**`/home/user/cyrup/crates/cyrup-ext-subagents/src/workflows/scripted/engine.rs`** — the op body is
essentially unmoved; the *plumbing* around it shifted by ~+70 lines.

| spec says | HEAD |
|---|---|
| `:1160-1335` the whole flow | `pub(crate) async fn run_host_command` at **`:1162`**; body `:1168`–`:1345` |
| `:1169-1170` normalize runs first | ✅ `:1169-1170` |
| `:1172-1178` recovery-barrier refusal | ❌ **`:1171-1177`** |
| `:1179-1183` `HOST_STEP_MAX_COUNT` cap | ❌ **`:1178-1182`** |
| `:1186` the `supports_host` refusal string | ❌ **`:1185`** is the `if !shared.host.supports_host()`; the string is **`:1186`** |
| `:1200-1222` the `Running` node | ✅ `let started_step = HostStepNode {` **`:1200`**, closes `:1221`, emitted by `shared.host_step_changed(&started_step);` at **`:1222`** |
| `:1262-1283` the terminal node | ✅ terminal `shared.host_step_changed(&step);` at **`:1283`** |
| `:1323-1330` the error arm | ✅ error-arm `shared.host_step_changed(&step);` at **`:1330`** |
| `:450-453` `host_step_changed` | ❌ **`:450-454`** |
| `:2592-2595` the telemetry drain delivers it | ❌ **`:2662-2666`** (`TelemetryEvent::HostStep(step) => { if let Some(on_host_step) = &on_host_step { on_host_step(&step); } }`) |
| `:2304` `shared.host.supports_host()` read | ❌ **`:2351`** |
| `:2403` injection of `hostEnabled` | ❌ `install_sandbox(&mut runtime, state_enabled, host_enabled)` call at **`:2352`**; `install_sandbox` itself is declared **`:2440-2464`** and the `__cyrupWorkflowInstall({{ stateEnabled: …, hostEnabled: … }})` format string is at **`:2457`** |
| (not cited) | `trait WorkflowScriptHost` **`:122`**; default `fn supports_host` **`:175-178`**; default `async fn host_command` **`:180-188`** (the `"runs.host is unavailable in this host context."` literal is `:187`) |
| (not cited) | `pub type WorkflowHostStepCallback = Arc<dyn Fn(&HostStepNode) + Send + Sync>` **`:223`**; the option field `pub on_host_step: Option<WorkflowHostStepCallback>` **`:260`** |

**`/home/user/cyrup/crates/cyrup-workflow-runtime/src/js/prelude.js`** — ✅ unchanged.
`__cyrupWorkflowInstall` at `:572`, `if (!hostEnabled) delete surface.host;` at **`:574`**, the
`host(key, params)` member itself at `:276-287`, run-key grammar `:87`.

**`/home/user/cyrup/crates/cyrup-ext-subagents/src/extension/tool/routing.rs`**

| spec says | HEAD |
|---|---|
| `:553` `validate_workflow_script(script, false)` | ✅ `:553` — **but there is a SECOND call the spec never mentions, at `:1493`** (the `validate` action arm). Whatever §1.3 concludes about the analyzer must be applied to both, or the two surfaces drift. |
| `on_host_step: None` | ✅ **`:688`** (comment `:687`) |
| "pass the vec in place of `&[]`" | the `&[]` is **`:857`**, inside `async fn settle_foreground_workflow` (**`:824`**), whose signature already carries `#[allow(clippy::too_many_arguments)]` |

**`/home/user/cyrup/crates/cyrup-ext-subagents/src/extension/executor/workflow.rs`**

| what | HEAD |
|---|---|
| `pub(crate) struct WorkflowRunHost` | `:99`, fields `:102-149`. `executor: Arc<SubagentExecutor>` `:102`, **`cwd: PathBuf` `:105`**, `run_dir: PathBuf` `:137`, `async_root: PathBuf` `:141` |
| `#[async_trait::async_trait] impl WorkflowScriptHost for WorkflowRunHost` | **`:364-365`**, closes at **`:735`** |
| where the two new methods go | after `steer`, which ends at `:734` — i.e. immediately before the `}` at `:735`, matching how `supports_steer` (`:620`) sits beside `steer` (`:628`) |
| the module doc that must be updated | **`:8-11`**: *"`runs.host` and `state` are genuinely ABSENT from the guest realm (`js/prelude.js:574,578`) … `runs.steer` is now wired (WORKFLOW_14). Do not describe all four as 'absent'."* Flipping `supports_host` makes this sentence false for `runs.host`; WORKFLOW_14 amended this same paragraph when it wired steer, and this task must do the same. |

Also stale: §2's `cd /home/d0m17bw/workspace/cyrup`. In this environment the repo root is
**`/home/user/cyrup`**.

---

### B. Two claims in §0.1 are FALSE at HEAD — do not implement against them

#### B.1 ❌ "The static analyzer refuses `runs.host` earlier anyway"

It does not, and there is nothing to thread.

* `/home/user/cyrup/crates/cyrup-ext-subagents/src/workflows/scripted/analyzer.rs:58`
  `const WORKFLOW_GLOBALS: &[&str] = &["runs", "emit", "console"];` — `runs` is admitted
  **unconditionally**.
* The analyzer only ever reports **unbound identifier references**
  (`check_global_reference`, `:306-333`). `is_reference_position` (`:368-382`) explicitly
  **excludes member properties**: `Node::MemberExpr(member) => !matches!(member.prop, MemberProp::Ident(prop) if prop.range() == ident.range())`.
  So the `host` in `runs.host` is never even a candidate finding.
* `pub struct AnalyzerOptions` (`analyzer.rs:177-181`) has **exactly one field, `state_enabled`**.
  There is no `host_enabled` parameter to thread.
* `pub fn validate_workflow_script` (`engine.rs:2532`) takes `(script: &str, state_enabled: bool)`
  and calls `analyze_workflow_script(script, AnalyzerOptions { state_enabled })` at `engine.rs:2554`.
  No host parameter exists anywhere in that chain.

**Consequence:** §1.3's "flipping `supports_host()` must also flip the analyzer's `host_enabled`, or
a valid script is rejected before it ever runs" describes a hazard that **does not exist**. A script
containing `runs.host(...)` passes `validate_workflow_script(script, false)` **today**. Do **not**
add an `AnalyzerOptions::host_enabled` field — that is unrequested scope (WORKFLOW_20 is the task
that legitimately touches `state_enabled`). The only gate that matters is `hostEnabled` →
`prelude.js:574`.

#### B.2 ❌ "a duplicate makes every receipt record each command twice"

It is worse than cosmetic: **it fails the settlement.**
`build_workflow_receipt` calls `assert_unique_host_step_ids(input.host_steps, "workflow receipt")`
at `/home/user/cyrup/crates/cyrup-ext-subagents/src/workflows/receipt.rs:577` (Rule 11), which
returns `Err("Invalid host step 'workflow receipt': duplicate host step id '{id}'.")`. That `Err`
propagates through `settle_foreground_workflow`'s
`.map_err(|error| format!("workflow completed but its receipt is invalid: {error}"))` (`routing.rs:862`) and
**turns a successful workflow into a `ToolError`**. A blind `push` in §1.2 therefore does not merely
dirty the receipt — it breaks every workflow that calls `runs.host` even once.

Rule 10 (`receipt.rs:571-575`) additionally refuses `> HOST_STEP_MAX_COUNT` (32) host steps.

**Extra hazard the spec does not mention:** `HostStepNode::id` is **the workflow key**
(`engine.rs:1204`: `id: key.clone()`), while the engine's own cap counts **call ids**
(`inner.hosts` keyed by `call_id`, `engine.rs:1190-1197`, `host_order.push` at `:1197`). Two `runs.host("gate", …)` calls with the
**same key** are two distinct ops but produce **one** `HostStepNode::id`. An upsert-by-`id` is
therefore not just the fix for the Running/terminal pair — it is the **only** accumulation shape the
receipt will accept at all. Same-key calls collapse to one node, last write wins. Do not "fix" that
by disambiguating the id; the id is upstream's and `assert_unique_host_step_ids` is the contract.

---

### C. The real `execute_workflow_host_command` signature (§1.1's "⚠ read it before writing the call")

It takes **six** arguments, not the three §1.1's sketch implies:

```rust
// /home/user/cyrup/crates/cyrup-ext-subagents/src/workflows/host_command.rs:834
pub async fn execute_workflow_host_command(
    key: &str,
    params: &WorkflowHostCommandParams,      // BORROWED; the trait hands you an OWNED one
    cwd: &Path,
    default_output_path: &Path,              // ← the host must MINT this; nothing else does
    claimed_output_path: Option<&Path>,      // ← and decide whether to claim
    cancel: &cyrup_core::CancelToken,        // BORROWED; the trait hands you an OWNED one
) -> Result<WorkflowHostCommandResult, String>
```

What each of the two unmentioned arguments means, read off the implementation (`:844-880`, `:906-926`):

* **`default_output_path`** is used **only when `params.output` is `None`**. In that branch the
  function does a plain `create_dir_all(parent)` + `write_default_output` with **no containment
  check against `cwd`** — so the host may legitimately point it at its own run directory. `self.run_dir`
  (`workflow.rs:137`) is the natural home, and the key is filesystem-safe by construction: the guest
  pattern is `/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/` (`prelude.js:87`) and the host re-checks it via
  `validate_key` → `WorkflowKey::parse` (`engine.rs:712-720`) before this method is reached — no `/`,
  no leading `.`. Something of the shape `self.run_dir.join("host").join(format!("{key}.log"))`
  satisfies it. **Never `std::env::current_dir`** (the spec is right about that, and `clippy.toml`
  bans `std::env::set_var` for the same family of reasons).
* **`claimed_output_path`**, when `Some`, is compared post-run against
  `resolve_workflow_host_output_claim_path(&output_path)` (`host_command.rs:908-912`); a mismatch is
  the hard `output path changed after it was claimed.` failure. Passing `None` skips that guard.
  The in-tree example of claiming correctly is the test at `host_command.rs:1364-1380` (`a_matching_claim_passes`, `:1365`)
  (`let claimed = resolve_workflow_host_output_claim_path(&output);` then `Some(&claimed)`).
* When `params.output` **is** `Some`, the function itself runs `ContainedPath::assert_within(cwd, …)`
  (`exec/output.rs:1556`) before and after the spawn. The host supplies nothing extra for that path.

Ownership adapters needed in the impl: `&params`, `&cancel`.

`WorkflowHostCommandParams` / `WorkflowHostCommandResult` are re-exported from
**`crate::workflows`** (`workflows/mod.rs:99-103`) — **not** from `crate::workflows::scripted`, which
is where `workflow.rs` currently gets `WorkflowScriptHost` from (`workflow.rs:34-38`). Two `use`
groups, as the file already has.

**"Zero callers" is right about production and wrong in the letter:** `execute_workflow_host_command`
has ten `#[cfg(test)]` callers inside its own module — `host_command.rs:1172, 1207, 1230, 1250, 1275,
1297, 1328, 1346, 1369, 1395` — and they are the ready-made calling-convention reference.

---

### D. §2's acceptance script cannot pass as written — the engine turns a failed command into a THROW

This is the single most important correction in this section.

`run_host_command` does **not** return a failed result to the guest. At `engine.rs:1306-1320`:

```rust
if result.ok { Ok(result) } else {
    // … detail/fallback assembly …
    Err(format!("Host command '{key}' failed: {text}"))
}
```

An `Err` from the op is a **rejected promise in the guest**. So:

* `const b = await runs.host("bad", { command: "false", … });` **throws**. The script never reaches
  its `return`, the workflow fails, and §2's expected object `{ pass: …, fail: "failed", … }` is
  **unreachable**. Same for the `"timedOut"` snippet.
* `WorkflowHostCommandState` serializes **kebab-case** (`host_command.rs:123`, enum `:124-137`): the words are
  `"passed" | "failed" | "timed-out" | "stopped"`. **`"timedOut"` is not a value this type can ever
  produce.** Only `g.state == "passed"` in §2 is correct.
* `HostStepState` (`host_step.rs:104-115`, words at `as_str` `:121-130`) is
  `pending | running | done | cancelled | error` — there is **no `timedOut` state**. §2's
  "terminal `HostStepNode` with `state: done|error|timedOut`" is wrong in its third word. A timeout
  lands as `state: "error"` **plus `reasonCode: "timed_out"`** (`engine.rs:1269-1275`). The mapping,
  verbatim from `engine.rs:1261-1275`:

  | `WorkflowHostCommandState` | `HostStepState` | `reasonCode` | `verdict` |
  |---|---|---|---|
  | `Passed` (`ok`) | `Done` | — | `Pass` |
  | `Failed` | `Error` | `command_failed` | — |
  | `TimedOut` | `Error` | `timed_out` | — |
  | `Stopped` | `Cancelled` | `aborted` | — |
  | (host `Err` arm, `:1321-1330`) | `Error` | `execution_failed` | — |

* The exact thrown text is computable. `settle_workflow_host_command` (`host_command.rs:595-632`)
  produces `error = "Command exited with code 1."` for `false` and
  `error = "Command timed out after 500ms."` for the `sleep 5` case; `engine.rs:1241-1259` folds
  `error` + (stderr else stdout), whitespace-collapsed and truncated to 200 chars, into `detail`.
  So the guest sees `Host command 'bad' failed: Command exited with code 1.` and
  `Host command 'slow' failed: Command timed out after 500ms.`

**Replacement §2 end-to-end check** (preserves §2's intent — a pass, a fail, and a real timeout,
all observable — while being runnable):

```bash
cd /home/user/cyrup
CYRUP_SUBAGENTS=1 ./target/debug/cyrup -p 'Use the subagent tool with workflowScript: "
  const g = await runs.host(\"gate\", { command: \"true\",  timeoutMs: 10000, role: \"gate\" });
  let failMsg = null, slowMsg = null;
  try { await runs.host(\"bad\",  { command: \"false\",   timeoutMs: 10000, role: \"gate\" }); }
  catch (e) { failMsg = String(e && e.message || e); }
  try { await runs.host(\"slow\", { command: \"sleep 5\", timeoutMs: 500 }); }
  catch (e) { slowMsg = String(e && e.message || e); }
  return { pass: g.state, passOk: g.ok, exit: g.exitCode, failMsg, slowMsg };
"'
# -> pass: "passed", passOk: true, exit: 0
#    failMsg: "Host command 'bad' failed: Command exited with code 1."
#    slowMsg: "Host command 'slow' failed: Command timed out after 500ms."
# NOT a TypeError on `runs.host` (that is the `hostEnabled` gate at prelude.js:574 still closed),
# and NOT "runs.host is unavailable in this host context." (engine.rs:1186, the Rust refusal —
# reachable ONLY by calling the op directly, since the guest surface is deleted wholesale).
```

**Replacement receipt check.** Both throws above are caught, so the workflow SUCCEEDS and settles
through the **`Ok` arm** (`routing.rs:723`). Three host steps, three distinct keys, all terminal:

```bash
jq '.hostSteps | length,
    (map(.id)|sort),
    (map(.state)|sort),
    (map(.reasonCode // "-")|sort)' <async_root>/<run_dir>/workflow-receipt.json
# -> 3
#    ["bad","gate","slow"]
#    ["done","error","error"]
#    ["-","command_failed","timed_out"]
# `"running"` anywhere is §1.2's upsert bug. `length == 6` is the same bug — except the receipt
# would not have been written at all (see B.2: duplicate ids are a hard build failure).
```

⚠ `hostSteps` is **omitted entirely, not written as `[]`**, when the vec is empty
(`receipt.rs:340-342`, `skip_serializing_if = "Vec::is_empty"`, Rule 13). So `jq '.hostSteps'`
returning `null` on a workflow that used `runs.host` is exactly the "wiring never landed" signal.

---

### E. Facts the implementor would otherwise rediscover

1. **`WorkflowScriptResult` has no `host_steps` field.** Neither does `WorkflowScriptPartial`
   (`workflows/scripted/types.rs`, the two structs carry only `value`/`emits`/`console`/`trace`/`children`).
   The `on_host_step` callback is therefore the **only** channel from the engine to the receipt — §1.2
   is not an optimization, it is load-bearing. This contrasts with `on_trace`, which `routing.rs:681-685`
   correctly leaves `None` *because* the trace is returned complete.
2. **The callback fires off the isolate thread.** `RunShared::host_step_changed` (`engine.rs:450-454`)
   only enqueues, and only `if self.on_host_step.is_some()` — so leaving it `None` costs nothing today,
   and supplying it costs one `TelemetryEvent::HostStep(Box<HostStepNode>)` per transition
   (`engine.rs:291`; boxed because the node is much larger than the other variants). Delivery is on
   the single drain task at `engine.rs:2645-2670` (the `HostStep` arm at `:2662-2666`). A `std::sync::Mutex<Vec<_>>` is the right container:
   the callback is a plain `Fn`, not async, and never runs concurrently with itself.
3. **The failure arm also settles.** A workflow that throws out of `runs.host` still reaches
   `settle_foreground_workflow` via `routing.rs:787` with `error.partial`, so the accumulated host
   steps must be passed at **both** call sites (`:723` and `:787`), not just the success one.
4. **The `supports_host` gate runs AFTER the barrier and the cap** (`engine.rs:1171-1187`), exactly as
   §1.3 says. It also runs **after** `normalize_workflow_host_command_params` (`:1169`) — so a script
   with malformed params gets the normalizer's message even in a host that refuses `runs.host`. Leave
   that order alone.
5. **The op is registered unconditionally.** `op_runs_host` is in the extension's op list at
   `/home/user/cyrup/crates/cyrup-workflow-runtime/src/lib.rs:274` (declared `:181`). `engine.rs:1157-1159`'s
   comment that it is "registered as an op ONLY for `workflow`-provenance runs" describes upstream's
   intent, not this build's wiring — the *only* thing that hides the verb is `delete surface.host` in
   `prelude.js:574`. This is why §0.1's "the Rust refusal is reachable only by calling the op directly"
   is true, and why the `supports_host` refusal string at `engine.rs:1186` stays reachable-in-principle
   even after this task flips the flag to `true` (it becomes dead for `WorkflowRunHost`, live for the
   trait default and for engine unit-test hosts).
6. **`cancel` is `shared.child_cancel`** (`engine.rs:1231`), the token cancelled at settlement
   (`engine.rs:2862`). A host command still running when the workflow settles therefore drains as
   `Stopped` → `HostStepState::Cancelled`, not as a hang.
7. **Unobserved-op settlement already covers host calls.** `engine.rs:3016-3021` collects unobserved
   `host_order` entries and `CompletionSettlement::decide` refuses on them — a fire-and-forget
   `runs.host(...)` is already a settlement error. Nothing to add.
8. **Engine unit-test hosts keep the default.** `on_host_step: None` at `engine.rs:3172` and `:3533`
   are test fixtures, not production wiring — do not "fix" them.
9. **`HostCommandKind` is a unit struct with a `Serialize`/`Deserialize` pair pinning it to
   `"command"`** (`host_command.rs:59-84`). §3's "not this task" on widening it is still accurate:
   there is no second kind anywhere in the tree.

---

### F. Scope guard

Unchanged from §3, plus: **do not touch the analyzer** (B.1 — there is nothing to thread), **do not
change `HostStepState`** to add a `timedOut` variant (D — `error` + `reasonCode: "timed_out"` is the
contract), and **do not change `run_host_command`'s Ok/Err split** (D — the throw-on-failure shape is
upstream's and is what `WorkflowSteerResult`-style value-returning deliberately is *not*).

`cargo fmt --all -- --check` is repo-wide RED on ~80 pre-existing files from a toolchain bump. Keep
only the files you touch well-formatted; do not fix the rest.

---

# WORKFLOW_19 — `runs.host`: the host-command surface

> **Created 2026-09-13.** `execute_workflow_host_command` — a real `tokio::process` runner with a
> full validating normalizer — has been in the tree with **zero callers**. The guest cannot even
> see the verb.

OBJECTIVE: implement `WorkflowScriptHost::supports_host` + `host_command` on `WorkflowRunHost` and
wire `on_host_step`, so a `workflowScript` can run a gate/CI command and branch on its verdict.

**Alignment with pi-subagents core definition of done** (VISION.md "Evidence closes work"):
A host command must emit concrete evidence (terminal `HostStepNode` with `state: done|error|timedOut`,
unique steps in receipt, `ok`/`state` verdicts) so the workflow can close on proven command results
rather than a missing or duplicate `Running` entry. The `on_host_step` upsert-by-id and the
`supports_host` flag (also flipping the analyzer) are the evidence surface; without them the
guest sees a TypeError and the receipt stays empty — exactly the optimistic-failure mode the
artisan contract forbids.

Stack: **Rust**. One crate: `cyrup-ext-subagents`.

**Hard prerequisite: [`WORKFLOW_13`](WORKFLOW_13.md)** — host steps land in the receipt's
`host_steps`, which W13 §3.2 currently passes as `&[]`.

---

## 0. What exists, and what does not

**Everything except the host impl.**

| piece | site | state |
|---|---|---|
| the runner (real `tokio::process::Child`) | `workflows/host_command.rs:834` `execute_workflow_host_command` | ✅ **built, zero callers** |
| child reaping / timeout | `host_command.rs:664` | ✅ |
| the ONLY validating boundary | `host_command.rs:193` `normalize_workflow_host_command_params` | ✅ |
| params/result types | `host_command.rs:101` / `:139` | ✅ |
| output claim path | `host_command.rs:337` `resolve_workflow_host_output_claim_path` | ✅ |
| `HostStepNode` | `workflows/host_step.rs:183-230` | ✅ |
| engine flow: normalize → barrier → cap → gate → OpRecord → step events | `engine.rs:1160-1335` | ✅ |
| `on_host_step` delivery off the isolate thread | `engine.rs:450-453` → `:2592-2595` | ✅ |
| receipt carries `host_steps` | `receipt.rs` `BuildWorkflowReceipt::host_steps` | ✅ |
| **`supports_host()` / `host_command()`** | `extension/executor/workflow.rs` | ❌ **this task** |
| **`on_host_step` supplied** | `extension/tool/routing.rs` (`None`) | ❌ **this task** |

### 0.1 The guest does not see `runs.host` at all

```js
// crates/cyrup-workflow-runtime/src/js/prelude.js:572-579
globalThis.__cyrupWorkflowInstall = ({ stateEnabled, hostEnabled }) => {
  const surface = { ...runsSurface };
  if (!hostEnabled) delete surface.host;      // ← the property is REMOVED
  globalThis.runs = Object.freeze(surface);
```

`hostEnabled` comes from `engine.rs:2304` (`shared.host.supports_host()`) and is injected at
`engine.rs:2403`. With it false, `runs.host(...)` is a **TypeError**, not the Rust refusal string
at `engine.rs:1186` — that string is reachable only by calling the op directly. Any test asserting
it must do so.

The static analyzer refuses `runs.host` earlier anyway (`scripted/analyzer.rs`), so flipping
`supports_host()` must also flip the analyzer's `host_enabled`, or a valid script is rejected
before it ever runs. **Check `validate_workflow_script`'s call site** — `routing.rs:553` passes
`state_enabled: false`; confirm whether host-enablement is a second analyzer parameter and thread
it the same way.

---

## 1. Required changes

### 1.1 `extension/executor/workflow.rs`

```rust
fn supports_host(&self) -> bool { true }

async fn host_command(
    &self,
    key: &str,
    params: WorkflowHostCommandParams,
    cancel: CancelToken,
) -> Result<WorkflowHostCommandResult, String> {
    // `params` arrives ALREADY normalized — `engine.rs:1169-1170` runs
    // `normalize_workflow_host_command_params` before this is called, and that function is
    // documented as the ONLY validating boundary. Do NOT re-validate here, and do NOT accept an
    // unnormalized `WorkflowHostCommandParams` from anywhere else.
    crate::workflows::execute_workflow_host_command(/* key, params, cwd, cancel, … */).await
}
```

⚠ Read `execute_workflow_host_command`'s real signature at `host_command.rs:834` before writing
the call — it needs at minimum the cwd and the cancel token, and it owns the `output` claim path
via `resolve_workflow_host_output_claim_path` (`:337`). The host supplies `self.cwd`, which is the
request cwd `Tool::execute` already resolved — **never** `std::env::current_dir`.

### 1.2 `extension/tool/routing.rs` — supply `on_host_step`

Currently `on_host_step: None`, which is why `BuildWorkflowReceipt::host_steps` is `&[]`. The
callback is delivered off the isolate thread through the telemetry drain (`engine.rs:2592-2595`),
so it is safe to accumulate from:

```rust
// Unlike `on_emit` (which re-hands the WHOLE accumulated snapshot on every call and is therefore
// quadratic to accumulate from — see routing.rs's own note), `on_host_step` delivers ONE node per
// call. Accumulating is correct here.
on_host_step: Some({
    let steps = std::sync::Arc::clone(&host_steps);
    std::sync::Arc::new(move |node: &crate::workflows::HostStepNode| {
        if let Ok(mut steps) = steps.lock() { steps.push(node.clone()); }
    })
}),
```

⚠ The engine emits a node **twice** per command — once `Running` at `engine.rs:1200-1222` and once
terminal at `:1262-1283` (or `:1323-1330` on the error arm). Upsert by `HostStepNode::id`, do not
append blindly, or every receipt records each command twice with the first copy permanently
`Running`.

Then pass the accumulated vec to W13 §3.2's `settle_foreground_workflow` in place of `&[]`.

### 1.3 The analyzer and the cap

* `HOST_STEP_MAX_COUNT` (`engine.rs:1179-1183`) already refuses past N with
  `"workflowScript supports at most N runs.host calls."` — nothing to add.
* the recovery-barrier refusal at `engine.rs:1172-1178` runs **before** the `supports_host` gate;
  leave that order alone.

---

## 2. Definition of done

**Evidence closes the host step** (pi-subagents VISION): `host_command` returning a terminal
`WorkflowHostCommandResult` + the `on_host_step` upsert producing unique terminal `HostStepNode`s
(`done`/`error`/`timedOut`, never stuck `Running`) is the concrete proof the command ran and
verdict is usable. The `supports_host` flag (also enabling the analyzer) makes the surface visible;
without it the guest sees a TypeError and receipt evidence is absent.

```bash
cd /home/d0m17bw/workspace/cyrup
grep -n 'fn supports_host' crates/cyrup-ext-subagents/src/extension/executor/workflow.rs   # -> true
grep -n 'on_host_step' crates/cyrup-ext-subagents/src/extension/tool/routing.rs            # -> Some(...)
cargo clippy --workspace --all-targets --features test-fixtures                            # exits 0
```

**End to end — a workflow gates on a real command:**

```bash
CYRUP_SUBAGENTS=1 ./target/debug/cyrup -p 'Use the subagent tool with workflowScript: "
  const g = await runs.host(\"gate\", { command: \"true\", timeoutMs: 10000, role: \"gate\" });
  const b = await runs.host(\"bad\",  { command: \"false\", timeoutMs: 10000, role: \"gate\" });
  return { pass: g.state, fail: b.state, passOk: g.ok, failOk: b.ok };
"'
# -> { pass: "passed", fail: "failed", passOk: true, failOk: false }
# NOT a TypeError on `runs.host`, and NOT "runs.host is unavailable in this host context."
```

**The receipt must record both steps, once each, terminal:**

```bash
jq '.hostSteps | length, (map(.state) | unique)' <async_root>/<id>/workflow-receipt.json
# -> 2   and   ["done","error"]   — never "running" (that would be §1.2's upsert bug)
```

**And the timeout path is real, not simulated:**

```bash
CYRUP_SUBAGENTS=1 ./target/debug/cyrup -p 'Use the subagent tool with workflowScript: "const r = await runs.host(\"slow\", { command: \"sleep 5\", timeoutMs: 500 }); return r.state;"'
# -> "timedOut"
```

---

## 3. Explicitly out of scope

| deferred to | what |
|---|---|
| not this task | widening `HostCommandKind` past `"command"` — it is a unit struct today and upstream has no second kind |
| not this task | `provider`/`role` semantics beyond carrying them onto the `HostStepNode`. No consumer reads them yet. |
| [`WORKFLOW_20`](WORKFLOW_20.md) | `state.get/set`, the other guest global gated off by a `supports_*`-style flag |
