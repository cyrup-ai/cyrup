---
stage: aug
status: done
updated: 2026-09-13 02:52
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
