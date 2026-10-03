# ADR-0031 — One JavaScript engine: `deno_core`

**Status** accepted (owner direction, 2026-10-03: "why would we do anything other than `deno_core` which we've already invested extensively in?")
**Date** 2026-10-03
**Decides** which JavaScript engine cyrup embeds, if it embeds one for any purpose. It does **not** decide whether or when to build pi's `codemode` tool (`CODE-001`…`CODE-013`); it fixes only what such a tool would run on.
**Supersedes** the sentence "Do not raise a JS engine anywhere, for anything" in `docs/gap-analysis/MCP-PORT-METHODOLOGY.md` §1.2, Cut 4 — as to its scope, not its substance (see Decision 2).
**Blocks released** `CODE-001` (the owner decision it asked, on a premise this ADR corrects) and `CODE-002` (the sandbox row).

---

## Context

`MCP-PORT-METHODOLOGY.md` §1.2 records four cuts to the MCP port. Cut 4 removes `pi-mcp-adapter`'s
`mcpScript` and its JavaScript worker, and says of it: *"This removes the only JS-engine question in
the port. No `rquickjs`, no vendored C, no `boa`, no JS-in-WASM. **Do not raise a JS engine anywhere,
for anything.**"* The methodology says the cuts are "settled in ADR-0012". **No `ADR-0012` file was ever
written**: `docs/adr/README.md` reserves the numbers 0012–0027 for the MCP port, and the cuts' only
text is that table.

The sentence was true of the port when it was written. It stopped being true of the repository on
2026-09-08 (commit `8de74602`, where the `deno_core` dependency and `workflows/scripted/engine.rs` first appear),
when the `workflowScript` runtime landed and embedded **V8 through `deno_core`**:

- `Cargo.toml:403-415` — `deno_core = { version = "0.411.0" }`, commented "V8 via `deno_core`, the
  engine core Deno itself ships".
- `crates/cyrup-ext-subagents/Cargo.toml:175-190` and `crates/cyrup-workflow-runtime` — the runtime and
  its prelude.
- `crates/cyrup-ext-subagents/src/workflows/scripted/mod.rs` — the module documentation, which records
  *why* `deno_core` and why not the WebAssembly component that was written first (a 25 MB
  unreproducible binary committed to git, a Node/npm build chain that could not run on the dev machine,
  and a class of deadlocks from Component-Model async).
- `crates/cyrup-ext-subagents/src/workflows/scripted/engine.rs:2362-2404` — the isolate is created with
  a heap limit (`heap_limits(0, WORKFLOW_HEAP_LIMIT_BYTES)`), a near-heap-limit callback that terminates
  execution, and a thread-safe isolate handle (`terminate_execution`) so a runaway script can be
  killed from outside.

`wasmtime` is also a dependency (`cyrup-ext`, feature `wasm-host`), but it runs **WebAssembly guest
extensions**; it is not a JavaScript engine here.

The ledger's pi-1.0 pass then filed `CODE-001` and `CODE-002` copying the absolute sentence and
asserting "nothing of this kind exists anywhere"; a one-line `grep` of `Cargo.toml` refutes that. An
owner decision was framed on that premise.

## Decision

1. **If cyrup embeds JavaScript for any purpose, it uses `deno_core` (V8).** No second engine:
   not `rquickjs`, not `boa`, not QuickJS or StarlingMonkey compiled to WebAssembly.
2. **Cut 4 stands for what it cut.** `mcpScript` and its worker remain out of the MCP port, and the
   port still takes no engine *of its own* for it. The words "anywhere, for anything" are superseded:
   they described the port's scope and were written before there was an engine in the tree.
3. **This ADR decides the engine, not the feature.** Whether to build `codemode` (and in what order)
   stays a prioritisation call. Several `codemode` rows that need no engine (`CODE-003`…`CODE-006`,
   `CODE-009`, `CODE-012`) are wanted by the MCP and tool rows regardless.

## Why `deno_core`

- **Prior conformance.** The author of a script is a model filling in a tool-call argument. What matters
  is that the engine behaves the way the model expects, and the model's reference is V8 in Node.
  `deno_core` is V8. (`workflows/scripted/mod.rs` §0.2 makes the same argument.)
- **Measured, not assumed.** It is the engine already running model-written scripts in this repository,
  with the capability contract, the heap cap and the kill switch above.
- **One engine is one upgrade and one security burden.** A second engine doubles both and adds a
  bridge for every host function.
- **Async ops are native to tokio**, which is what every nested tool call in `codemode` is.

## Consequences, and what this does not buy

- **Weaker isolation than upstream's choice.** pi runs QuickJS compiled to WebAssembly, so its sandbox is
  enforced by the WebAssembly boundary. `deno_core` isolates are in-process: a V8 memory-safety bug is
  a risk the WebAssembly design does not carry. The exposure is bounded — a script has no ambient
  authority (no filesystem, network, timers or `eval`), only the functions injected into it, and it is
  capped and killable — and it is the exposure `workflowScript` already accepts. It is not zero, and a
  stronger posture (a separate process for the isolate) is a later option, not a precondition.
- **Version coupling.** `deno_core` moves fast; the pin in `Cargo.toml` is the one place to bump.
- **No behaviour changes today.** This is a decision of record plus two documentation corrections.

## Alternatives rejected

- **QuickJS in `wasmtime`** — closest to upstream's isolation; adds a second engine and a host-function
  bridge, and repeats the packaging trouble `workflowScript` left behind.
- **`rquickjs` / `boa`** — a second engine; `rquickjs` also vendors C, which Cut 4 ruled out for good
  reason.
- **A non-JavaScript script language** — gives up the prior that makes model-written scripts work.

## How to reverse this

Replace `deno_core` in `workflows/scripted` first (it is the larger, shipped user), then move any other
embedder; this ADR would be superseded by the one that does so. Nothing outside `crates/cyrup-ext-subagents`
and `crates/cyrup-workflow-runtime` depends on the engine today.
