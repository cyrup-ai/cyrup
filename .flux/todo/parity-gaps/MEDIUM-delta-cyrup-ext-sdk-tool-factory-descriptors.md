---
title: "cyrup-ext-sdk's tool_factory descriptors are paraphrases, not Pi's tools"
priority: MEDIUM
crate: cyrup-ext-sdk
source: split out of MEDIUM-delta-cyrup-tools-src-tools-bash-rs-72.md §6
stage: aug
status: done
updated: 2026-08-28 21:00
---

# `cyrup-ext-sdk::tool_factory` hands extension authors paraphrased tools

Filed per the Definition of Done of
[bash-rs-72](./MEDIUM-delta-cyrup-tools-src-tools-bash-rs-72.md), which closed by verification
and required this sibling be filed rather than folded in.

[`crates/cyrup-ext-sdk/src/tool_factory.rs`](../../crates/cyrup-ext-sdk/src/tool_factory.rs) is
`pub mod` ([`lib.rs:59`](../../crates/cyrup-ext-sdk/src/lib.rs)) — reachable public API whose
strings reach a model.

## Measured against pi at `e8682309` (re-verified 2026-08-28)

**`bash_descriptor` (`:19-34`)** vs `bashSchema` (`bash.ts:42-45`):

| | pi | cyrup-ext-sdk |
| --- | --- | --- |
| `command` description | `Shell command to execute` | `The shell command to run.` |
| `timeout` | present, `number`, described | **absent** |
| `cwd` | **absent** | present, with `"default": cwd` |
| label | `bash` (`bash.ts:521`, in `bashToolConfig` `:519-527`) | `Bash` |
| tool description | pi's real bash description | `Run a shell command in the project working directory.` |
| `executionMode` | **not declared** (no built-in declares it) | `Sequential` |

**`read_descriptor` (`:37-48`)** vs `readSchema` (`read.ts:21-25`): `path` carries **no
description** (pi: `Path to the file to read (relative or absolute)`); `offset` and `limit` are
**absent entirely**; label `Read` vs pi's `read` (`read.ts:217`).

**`write_descriptor` (`:51-63`)** vs `writeSchema` (`write.ts:15-18`): neither `path` nor
`content` carries a description (pi describes both); label `Write` vs pi's `write`
(`write.ts:194`); `executionMode: Sequential` that pi does not declare.

**`powershell`**: no builder at all. pi re-exports `createPowerShellTool` (`sdk.ts:129`).

All four also drop `promptSnippet` / `promptGuidelines`, which pi's definitions carry
(`read.ts:219-220`, `write.ts:197-198`, `bash.ts:524-525`, `powershell.ts:35,39-41`) and which the
cyrup `ToolDescriptor` has fields for (`descriptor.rs:118-127`).

## Why this is a defect, not a style choice

pi does not paraphrase and does not keep a second copy. `sdk.ts:19-32` imports the **actual**
factories from `./tools/index.ts` and `sdk.ts:117-130` re-exports them (`createReadTool` `:122`,
`createBashTool` `:123`, `createWriteTool` `:125`, `createPowerShellTool` `:129`), so an extension
author composing on a built-in gets the byte-exact schema the core agent uses. `sdk.ts` and
`tools/bash.ts` are both `packages/coding-agent/src/core` — the same package, a plain relative
import. There is no boundary between them upstream.

The split between `cyrup-ext-sdk` and `cyrup-tools` is a cyrup packaging artifact pi does not have,
and `tool_factory.rs:1-5` states outright that it is porting that sdk re-export. So the faithful
port is for `cyrup-ext-sdk`'s builders to be the real `cyrup-tools` descriptors, not a second
transcription of them. The module doc's claim that the builders "reproduce the shapes of Pi's
built-in tools" is false as written.

An earlier revision posed this as a choice between re-exporting and keeping a hand-written builder
set pinned by a test. Recorded so it is not re-derived: the shape is not a preference, it is
whatever makes the two copies provably one. What decides the MECHANISM is the guest build target,
below — not taste, and not a wish to avoid a dependency.

## The blocking mechanical fact: the guest target cannot link `cyrup-tools`

`cyrup-ext-sdk` is the **wasm guest** crate. It is excluded from `default-members` and built as
`cargo build -p cyrup-ext-sdk --target wasm32-wasip2` (root `Cargo.toml:41-43`), and that build is
a real gate: `crates/cyrup-it/build.rs:81` (`const WASM_PKG: &str = "cyrup-ext-sdk";`) and
`xtask/src/features.rs:186` (`&["-p", "cyrup-ext-sdk", "--target", "wasm32-wasip2"]`).

A normal `cyrup-ext-sdk -> cyrup-tools` edge is acyclic (cyrup-tools' only cyrup edge is
`cyrup-core`; its `cyrup-provider` edge is dev-only, `cyrup-tools/Cargo.toml` `[dev-dependencies]`),
but it is not linkable on that target:

- `cyrup-tools/src/ops/mod.rs:12` is `pub mod local;` — **un-gated** — and
  `src/ops/local/{proc,command,signal,fs}.rs` use `tokio::process`, `tokio::fs` and `std::os::unix`.
- `cyrup-tools`' manifest pulls `tokio` (workspace features `rt-multi-thread, macros, sync, fs,
  process, io-util, time, signal` — root `Cargo.toml:138`), `ignore`, `grep-matcher`,
  `grep-regex`, `grep-searcher`, `globset`, `similar`, `feruca`, `unicode-*`, `image` (default
  feature `inline-images`), and `libc` on unix; plus `cyrup-core`, which itself pulls
  `tokio`/`tokio-util`/`tokio-stream`/`dashmap`.
- Constructing the tools needs live backends anyway — `ReadTool::new(Arc<dyn FsOps>, …)`
  (`read.rs:34`), `WriteTool::new(fs, Arc<FileMutationLocks>, …)` (`write.rs:28`),
  `ShellTool::bash(Arc<dyn ProcOps>, …)` (`bash.rs:166`) — i.e. `LocalFs` / `Backend::default()`
  local fs+process authority, which is exactly what a sandboxed guest does not and must not have.

So the derivation is real but cannot be a link-time one on wasm. It is a **host-build** derivation,
enforced continuously — see the guard below. `cyrup-ext-sdk`'s SHIPPED (wasm) dependency footprint
stays exactly `serde` + `serde_json` (+ `wit-bindgen` on wasm32); the new edge is a
`[dev-dependencies]` entry, which the `--target wasm32-wasip2` cdylib build never compiles.

## The change — exact sites

### 1. `crates/cyrup-ext-sdk/Cargo.toml`

Add, after the existing `[dependencies]` block:

```toml
[dev-dependencies]
# Host-target only. The guest-facing descriptors in `src/tool_factory.rs` are the real built-in
# tools' model-facing surface; `src/tests/tool_factory_parity.rs` derives them from the actual
# `cyrup_core::Tool` impls and fails if the two ever differ. Dev-only because the guest links for
# wasm32-wasip2 and `cyrup-tools` is tokio/unix/process code (ops/mod.rs:12 `pub mod local`).
cyrup-tools = { workspace = true }
```

`cyrup-tools = { path = "crates/cyrup-tools", version = "0.0.0" }` already exists at root
`Cargo.toml:121`, so `workspace = true` resolves.

### 2. `crates/cyrup-ext-sdk/src/tool_factory.rs` — the four builders

Signatures:

- `pub fn bash_descriptor() -> ToolDescriptor` — **drop the `cwd: &str` parameter.** pi's
  `createBashTool(cwd)` (`bash.ts:536`) takes `cwd` as the EXECUTION directory; it never appears in
  `bashSchema` (`bash.ts:42-45`). A `ToolDescriptor` carries no execution, so the argument has
  nothing to bind to — which is why today it leaked into the schema as an invented `cwd` property.
- `pub fn powershell_descriptor() -> ToolDescriptor` — **new** (`sdk.ts:129`).
- `read_descriptor()`, `write_descriptor()` — unchanged signatures.

Each returns exactly what the conversion in §4 produces for the corresponding real tool. The values,
with their owning site in `cyrup-tools` (these are the bytes to write; the guard proves them):

| field | `bash` | `powershell` | `read` | `write` |
| --- | --- | --- | --- | --- |
| `name` | `bash` (`bash.rs:91`) | `powershell` (`powershell.rs:31`) | `read` (`read.rs:61`) | `write` (`write.rs:57`) |
| `label` | `bash` (`bash.rs:92`) | `powershell` (`powershell.rs:32`) | `read` (`read.rs:75`) | `write` (`write.rs:63`) |
| `parameters` | `bash.rs:135-142` | same object (`powershell.rs:50` → `ShellTool::new`) | `read.rs:40-48` | `write.rs:36-43` |
| `description` | `bash.rs:147-154`, `shell_name="bash"` | same, `shell_name="PowerShell"` (`powershell.rs:33`) | `read.rs:83-86` | `write.rs:81-82` |
| `prompt_snippet` | `bash.rs:94` | `powershell.rs:35` | `read.rs:89` | `write.rs:85` |
| `prompt_guidelines` | `bash.rs:95-97` | `powershell.rs:39-41` | `read.rs:92` | `write.rs:88` |
| `execution_mode` | `None` | `None` | `None` | `None` |

The two shell schemas are ONE object by construction (`bash.rs:130-142`: the shared factory builds
it; `powershell.rs:50` calls that factory), which is the same reason `pi_schema.rs` asserts both
against the single `PI_SHELL` constant (`crates/cyrup-tools/src/tests/pi_schema.rs:58-68` holds
seven `PI_*` schema constants for eight tools, `PI_SHELL` at `:63` serving `bash` and `powershell`).

`execution_mode` becomes `None` on bash and write — a behaviour change, not a cosmetic one. No
`cyrup-tools` built-in overrides `Tool::execution_mode` (zero `fn execution_mode` under
`crates/cyrup-tools/src/tools/`), and `write.rs:68-77` records why: pi declares no `executionMode`
on any built-in, and declaring `Sequential` made `cyrup-agent`'s `any_seq` (`agent.rs:905-908`)
route the WHOLE batch — reads and greps included — through `execute_sequential`. Today's
`tool_factory` reproduces that already-fixed defect on the extension surface; `live.rs:1885-1888`
maps `None` to `Parallel`, so `None` is both faithful (pi omits the field) and correct.

Also in this file:

- Rewrite the module doc `:1-5` so it stops claiming the builders "reproduce the shapes" and states
  what is now true: the builders ARE the built-in tools' model-facing surface, held here because the
  guest cannot link `cyrup-tools`, and proven equal by `src/tests/tool_factory_parity.rs`.
- Fix the stale citation at `:17`: `createBashTool` is `bash.ts:536` (`createBashToolDefinition`
  `:529`); `bash.ts:451` is the `ops.exec` call inside the shared factory.
- Fix the module-doc citation `sdk.ts:111-123` → the re-export block is `sdk.ts:117-130`, tool
  factories `:122-129`.

### 3. Call sites that move with the signature

- `crates/cyrup-ext-sdk/src/lib.rs:18-20` — the module list names three builders; add
  `powershell_descriptor` (the `#![deny(rustdoc::broken_intra_doc_links)]` at `:41` makes a missed
  link a build failure).
- `crates/cyrup-ext-sdk/src/tests/ergonomic.rs:276` — `bash_descriptor("/work")` →
  `bash_descriptor()`.

### 4. The conversion (specify it once, in the guard module)

`crates/cyrup-ext-sdk/src/tests/tool_factory_parity.rs`, a host-target test module registered in
`src/tests/mod.rs:17-21`:

```rust
fn from_tool(tool: &dyn cyrup_core::Tool) -> ToolDescriptor { … }
```

field-by-field, from `cyrup-core/src/tool.rs:88-107`:

| `ToolDescriptor` (`descriptor.rs:103-146`) | from `cyrup_core::Tool` |
| --- | --- |
| `name: String` | `tool.name().to_string()` (`:90`) |
| `label: String` | `tool.label().unwrap_or(tool.name()).to_string()` (`:105`) — `Option<&str>` where the descriptor's field is a bare `String` seeded from the name (`descriptor.rs:150-154`); `None` means "fall back to the name", the inverse of `live.rs`'s `descriptor_label` |
| `description: String` | `tool.description().to_string()` (`:99`) |
| `parameters: Value` | `tool.parameters().clone()` (`:92`) |
| `execution_mode: Option<ExecMode>` | `cyrup_core::ExecMode::Sequential => Some(ExecMode::Sequential)`, `Parallel => None`. Two ENUMS spelled alike (`cyrup-core/src/tool.rs:13-17` vs `descriptor.rs:11-18`) — map explicitly. `Parallel => None` is pi's omitted field and is behaviourally identical (`live.rs:1885-1888`) |
| `prompt_snippet: Option<String>` | `tool.prompt_snippet().map(str::to_owned)` (`:112`) |
| `prompt_guidelines: Vec<String>` | `tool.prompt_guidelines().into_iter().map(str::to_owned).collect()` (`:130`) |
| `render_shell: RenderShell` | `ToolRenderKind::Default => RenderShell::Default`, `SelfRendered => RenderShell::SelfRendered` (`tool.rs:66-73` vs `descriptor.rs:23-32`) |
| `has_renderer`, `prepare_arguments` | left `false`. They describe whether the GUEST exports a renderer / arg shim, which is the author's business, not the built-in's |
| `constrained_sampling` | left `None`, and the guard does NOT assert it. `Tool::constrained_sampling` (`tool.rs:162`) returns `cyrup_core::experimental_tool_sampling()`, which LATCHES `CYRUP_EXPERIMENTAL`/`PI_EXPERIMENTAL` from the process env in a `OnceLock` (`constrained_sampling.rs:100-124`) — asserting it would make the guard env-dependent. A guest author opts in explicitly via `ToolDescriptor::constrained_sampling` (`descriptor.rs:214`) |

### 5. The guard

One test in that module — it is the whole quality bar for this task; no other test, bench or doc
deliverable is being asked for:

```rust
#[test]
fn extension_facing_descriptors_equal_the_real_builtin_tools() { … }
```

builds the four real tools exactly as `pi_schema.rs:80-99` does — `ReadTool::new(Arc::new(LocalFs),
cwd, ReadOpts::default())`, `WriteTool::new(fs, Arc::new(FileMutationLocks::new()), cwd, WriteOpts)`,
`ShellTool::bash(Backend::default().proc, cwd, BashOpts::default())`,
`ShellTool::powershell(proc, cwd, PowerShellOpts::default())` — and asserts
`from_tool(&t) == <builder>()` for each (compare the whole struct minus `constrained_sampling`;
`ToolDescriptor` is not `PartialEq` today, so either derive it or assert the eight fields, and say
which in the diff).

It fails at HEAD on every one of the four: `bash` (missing `timeout`, invented `cwd`, `Bash`,
paraphrased description, `Sequential`), `read` (missing `offset`/`limit`, undescribed `path`,
`Read`), `write` (undescribed properties, `Write`, `Sequential`), and `powershell` (no builder — it
does not compile). It fails again the day anyone edits a schema literal in `cyrup-tools/src/tools/`.

## Definition of done

1. `bash_descriptor()`, `powershell_descriptor()`, `read_descriptor()` and `write_descriptor()`
   return descriptors byte-identical to the corresponding `cyrup-tools` tool's `name`, `label`,
   `description`, `parameters`, `prompt_snippet`, `prompt_guidelines` and mapped `execution_mode`.
   A cyrup extension author gets what a pi extension author gets from `sdk.ts:122-129`.
2. `bash_descriptor` takes no `cwd`; the invented `cwd` property is gone from the schema.
3. `crates/cyrup-ext-sdk/src/tests/tool_factory_parity.rs` exists, is registered in
   `src/tests/mod.rs`, holds the single `from_tool` conversion, and is RED without change 2/1.
4. `cyrup-tools` is added to `[dev-dependencies]` ONLY. `cargo build -p cyrup-ext-sdk --target
   wasm32-wasip2` still succeeds and the shipped guest still links only `serde`/`serde_json`
   (+`wit-bindgen`).
5. `tool_factory.rs:1-5` no longer claims to "reproduce the shapes"; `:17`'s `bash.ts:451` is
   corrected to `bash.ts:536` (`createBashToolDefinition` `:529`) and `sdk.ts:111-123` to
   `sdk.ts:117-130`.
6. `lib.rs:18-20` and `tests/ergonomic.rs:276` are updated with the new builder set and signature.
