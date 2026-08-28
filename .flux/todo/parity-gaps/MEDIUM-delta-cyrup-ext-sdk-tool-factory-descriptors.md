---
title: "cyrup-ext-sdk's tool_factory descriptors are paraphrases, not Pi's tools"
priority: MEDIUM
crate: cyrup-ext-sdk
source: split out of MEDIUM-delta-cyrup-tools-src-tools-bash-rs-72.md §6
stage: task
status: todo
updated: 2026-08-28 09:15
---

# `cyrup-ext-sdk::tool_factory` hands extension authors paraphrased tools

Filed per the Definition of Done of
[bash-rs-72](./MEDIUM-delta-cyrup-tools-src-tools-bash-rs-72.md), which closed by verification
and required this sibling be filed rather than folded in. **Nobody has authorized this
divergence** — it is filed so it becomes a decision rather than an artifact.

[`crates/cyrup-ext-sdk/src/tool_factory.rs`](../../crates/cyrup-ext-sdk/src/tool_factory.rs) is
`pub mod` ([`lib.rs:59`](../../crates/cyrup-ext-sdk/src/lib.rs)) — reachable public API whose
strings reach a model.

## Measured against pi at `e8682309`

**`bash_descriptor` (`:17-33`)** vs `bashSchema` (`tools/bash.ts:42-45`):

| | pi | cyrup-ext-sdk |
| --- | --- | --- |
| `command` description | `Shell command to execute` | `The shell command to run.` |
| `timeout` | present, `number`, described | **absent** |
| `cwd` | **absent** | present, with `"default": cwd` |
| label | `bash` (`bash.ts:521`) | `Bash` |
| tool description | pi's real bash description | `Run a shell command in the project working directory.` |

**`read_descriptor` (`:36-48`)** vs `readSchema` (`tools/read.ts:21-25`): `path` carries **no
description** (pi: `Path to the file to read (relative or absolute)`); `offset` and `limit` are
**absent entirely**; label `Read` vs pi's `read` (`read.ts:217`).

**`write_descriptor` (`:50-62`)** vs `writeSchema` (`tools/write.ts:15-18`): neither `path` nor
`content` carries a description (pi describes both); label `Write` vs pi's `write`
(`write.ts:194`).

## Why this is a defect, not a style choice

pi does not paraphrase. `sdk.ts:122-129` re-exports the **actual** `createReadTool`,
`createBashTool`, `createWriteTool` and `createPowerShellTool`, so an extension author composing on
top of a built-in gets the byte-exact schema the core agent uses. A cyrup extension author gets a
different tool: different name casing, a missing `timeout`, an invented `cwd`, and undescribed
properties.

The module doc at `tool_factory.rs:4` claims these builders "reproduce the shapes of Pi's built-in
tools". That is false as written.

## The fix — determined by the source, not by preference

pi does not paraphrase, and it does not maintain a second copy. `sdk.ts:15-32` imports the real
factories from `./tools/index.ts` and `:122-129` re-exports them. `sdk.ts` and `tools/bash.ts` are
both `packages/coding-agent/src/core` — **the same package, a plain relative import**. There is no
boundary between them upstream.

The split between `cyrup-ext-sdk` and `cyrup-tools` is a cyrup packaging artifact that pi does not
have, and `tool_factory.rs:1-5` states outright that it is porting `sdk.ts:111-123`. So the
faithful port is for `cyrup-ext-sdk` to take the real descriptors from `cyrup-tools`, exactly as
`sdk.ts` takes them from `./tools/`.

The crate graph permits it: `cyrup-tools` depends only on `cyrup-core`, so
`cyrup-ext-sdk -> cyrup-tools` is acyclic.

An earlier revision of this file posed this as a choice between re-exporting and keeping a
hand-written builder set pinned by a test. That was a fabricated decision: the second option exists
only to avoid adding a dependency, and the dependency is what the source structure implies. It also
priced the change without checking the graph. Recorded so the reasoning is not re-derived.

The one real engineering question is mechanical, not architectural: `cyrup-tools` tools implement
`cyrup_core::Tool` (`tool.rs:90-105`: `name`, `parameters`, `description`, `label`), while
`cyrup-ext-sdk` hands out an owned [`ToolDescriptor`](../../crates/cyrup-ext-sdk/src/descriptor.rs)
(`name`, `label`, `description`, `parameters`, `execution_mode`). The four fields line up, so the
builders become a conversion from the real tool rather than a second transcription of it.

## Also in this file

- **Stale citation.** `tool_factory.rs:17` cites "Pi `createBashTool(cwd)`, bash.ts:451". At
  `e8682309`, `createBashTool` is `bash.ts:536` (`createBashToolDefinition` at `:529`);
  `bash.ts:451` is the `ops.exec` call inside the shared factory. Verified 2026-08-28.

## Definition of done

1. A cyrup extension author gets the same tool shape a pi extension author gets — `bash`, `read`,
   `write` and `powershell`, each with pi's exact property set, descriptions and label casing.
2. There is ONE definition of each tool in the workspace. `tool_factory`'s builders derive from the
   real `cyrup-tools` tools rather than transcribing them, so the shapes cannot drift apart.
3. A test fails if the extension-facing shape stops matching the real tool's `parameters()`.
4. `tool_factory.rs:1-5` either becomes true or stops claiming it reproduces pi's built-in tools.
5. The `bash.ts:451` citation is corrected to `:536` (`createBashTool`; `createBashToolDefinition`
   at `:529`).
6. `powershell` is included. This is not a judgement call: pi re-exports `createPowerShellTool`
   (`sdk.ts:129`) and `tool_factory` offers no PowerShell builder, so its absence is a gap.
