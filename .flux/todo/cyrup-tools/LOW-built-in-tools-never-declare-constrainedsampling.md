---
title: Built-in tools never declare constrainedSampling
priority: LOW
tool: read/bash/edit/write
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: aug
status: in-progress
updated: 2026-08-27
---

# Built-in tools never declare `constrainedSampling`

> **Merged finding.** Five separate findings (bash, edit, read, write, and one cross-cutting)
> describe the *same* missing capability. They are one task: the opt-in is declared per tool,
> so the fix is a single mechanism applied at four call sites.

## What pi does

Every mutating/executing built-in declares `constrainedSampling: getExperimentalToolSampling()` on its `ToolDefinition`: read.ts:222, bash.ts:354 (which is `createShellToolDefinition`, so it covers `powershell` too), edit.ts:329, write.ts:200. `getExperimentalToolSampling()` returns `{ type: "json_schema", strict: "prefer" }` when `PI_EXPERIMENTAL === "1"` and `undefined` otherwise (core/experimental.ts:1,7-9). `wrapToolDefinition` copies the field verbatim onto the runtime `AgentTool` (tools/tool-definition-wrapper.ts:14) and `createToolDefinitionFromAgentTool` copies it back (tool-definition-wrapper.ts:42), so the agent loop and the provider adapters see it.

## What cyrup-tools does

`Tool::constrained_sampling()` exists on the trait with a default of `None` (crates/cyrup-core/src/tool.rs:156-158) and its doc-comment asserts "No pi built-in tool declares it" — which is stale. Ripgrep for `constrained_sampling` across `crates/cyrup-tools/src` returns zero hits: no built-in overrides it, so `ReadTool`/`BashTool`/`EditTool`/`WriteTool` always report `None`. The experimental flag itself is read only for a TUI status marker (crates/cyrup-tui/src/status.rs:468-482, `CYRUP_EXPERIMENTAL`/`PI_EXPERIMENTAL`), never for tool sampling.

## User-visible impact

With the experimental flag on, pi sends the built-in tool schemas to the provider with strict JSON-schema constrained sampling (`strict` on Anthropic/OpenAI adapters); cyrup sends them unconstrained. Malformed tool-call arguments that pi's providers reject at sampling time still reach cyrup's tools, producing schema-validation errors instead of correct calls.

## Parity action

Add an `experimental_tool_sampling()` helper reading `CYRUP_EXPERIMENTAL`/`PI_EXPERIMENTAL == "1"` and returning `ConstrainedSampling::Config(ConstrainedSamplingConfig::JsonSchema { strict: Prefer })`; override `constrained_sampling()` on `ReadTool`, `BashTool`, `EditTool` and `WriteTool` to return it, and update the now-incorrect claim in `crates/cyrup-core/src/tool.rs:150-155`.

## Per-tool detail

- **bash** — bash does not opt in to experimental constrained sampling
  - pi: `/home/user/cyrup/tmp/pi/packages/coding-agent/src/core/tools/bash.ts:354` sets `constrainedSampling: getExperimentalToolSampling()` on the shell tool definition (so both bash and powershell get it). `core/experimental.ts:1-9` resolves that to `{ type: "json_schema", strict: "prefer" }` when `PI_EXP
  - cyrup: `/home/user/cyrup/crates/cyrup-tools/src/tools/bash.rs` never implements `Tool::constrained_sampling`, so it keeps the trait default `None` (`/home/user/cyrup/crates/cyrup-core/src/tool.rs:156`, whose doc even asserts "No pi built-in tool declares it"). A ripgrep for `constrained_sampling` across `/
- **edit** — `edit` does not declare constrained/strict tool sampling
  - pi: The `edit` tool definition sets `constrainedSampling: getExperimentalToolSampling()` (/home/user/cyrup/tmp/pi/packages/coding-agent/src/core/tools/edit.ts:329), which returns `{ type: "json_schema", strict: "prefer" }` when `PI_EXPERIMENTAL=1` (/home/user/cyrup/tmp/pi/packages/coding-agent/src/core/
  - cyrup: `EditTool` in /home/user/cyrup/crates/cyrup-tools/src/tools/edit.rs declares no `constrained_sampling` override, so it inherits the `None` default from the trait (/home/user/cyrup/crates/cyrup-core/src/tool.rs:156). Grepping /home/user/cyrup/crates/cyrup-tools/src for `fn constrained_sampling` retur
- **read** — pi's read declares experimental constrained (strict-schema) tool sampling; cyrup's read never does
  - pi: /home/user/cyrup/tmp/pi/packages/coding-agent/src/core/tools/read.ts:222 sets `constrainedSampling: getExperimentalToolSampling()` on the read ToolDefinition; /home/user/cyrup/tmp/pi/packages/coding-agent/src/core/experimental.ts:1-9 returns `{ type: "json_schema", strict: "prefer" }` when `PI_EXPER
  - cyrup: /home/user/cyrup/crates/cyrup-tools/src/tools/read.rs:57-92 — the `impl Tool for ReadTool` block declares name/label/parameters/description/prompt_snippet/prompt_guidelines but never overrides `constrained_sampling`. The trait default is `None` (/home/user/cyrup/crates/cyrup-core/src/tool.rs:157-159
- **write** — write does not opt in to experimental constrained (strict json_schema) tool sampling
  - pi: pi's write ToolDefinition declares `constrainedSampling: getExperimentalToolSampling()` (write.ts:200). `getExperimentalToolSampling` (core/experimental.ts:1-9) returns `{ type: "json_schema", strict: "prefer" }` when `PI_EXPERIMENTAL === "1"` and `undefined` otherwise, and `wrapToolDefinition` copi
  - cyrup: /home/user/cyrup/crates/cyrup-tools/src/tools/write.rs:54-89 implements `Tool` for `WriteTool` and overrides `name`, `label`, `parameters`, `description`, `prompt_snippet`, `prompt_guidelines` — but never `constrained_sampling`, so it inherits the trait default `None` (/home/user/cyrup/crates/cyrup-

## Why this gap is real

Note the adversary **partially refuted** the original framing — read this before starting:

> I tried hard to refute this and could only refute half of it. What EXISTS in Rust (so the title's "the opt-in is dead" is wrong): the full constrained-sampling pipeline is live end-to-end — the wire types (crates/cyrup-core/src/constrained_sampling.rs, ConstrainedSampling/ConstrainedSamplingConfig::{JsonSchema,Grammar}/StrictSampling with pi's exact snake_case tags), the trait seam (crates/cyrup-core/src/tool.rs:156), the agent-loop forward (crates/cyrup-agent/src/agent/run/stream.rs:94 `constrained_sampling: t.constrained_sampling().cloned()`), the extension/WASM/SDK opt-in path (crates/cyrup-ext-sdk/src/descriptor.rs:163, crates/cyrup-ext/src/host/live.rs:1960, crates/cyrup-ext/src/wrapper.rs:123, crates/cyrup-ext/src/registry.rs:54), and the provider resolvers + adapter emission (crates/cyrup-provider/src/utils/constrained_sampling.rs:209/232, anthropic_messages.rs:1252-1305 emitting `strict: true` + the full schema when `supports_strict_tools`). Regression test PROV-011 at crates/cyrup-agent/src/tests/agent_loop.rs:1493 guards it. The experimental flag is also ported: crates/cyrup/src/startup.rs:76-83 `are_experimental_features_enabled()` reads CYRUP_EXPERIMENTAL/PI_EXPERIMENTAL exactly like pi's core/experimental.ts:3. What is genuinely ABSENT: nothing wires that flag to a `ConstrainedSampling` value, and no built-in overrides the trait method — `rg -c constrained_sampling crates/cyrup-tools/src crates/cyrup/src` returns zero hits, and the `impl Tool for` blocks at read.rs:58, bash.rs:84, edit.rs:128, write.rs:55 override prompt_guidelines/render_kind but never constrained_sampling. Pi's side of the claim also checks out in the vendored source at /home/user/cyrup/tmp/pi (read.ts:222, bash.ts:354, edit.ts:329, write.ts:200, plus server/create-harness.ts:34, and core/experimental.ts:7-9), and the stale doc-comment at crates/cyrup-core/src/tool.rs:152-155 asserting "No pi built-in tool declares it" is contradicted by that grep. So this is a real but very narrow gap: four missing one-line declarations plus a flag hookup, not a missing capability layer.

## Definition of done

1. Each of `read`, `bash`, `edit`, `write` declares the opt-in as pi does.
2. The existing pipeline (already present per the refutation above) is reached end-to-end.
3. A test asserts the declaration reaches the provider request.
