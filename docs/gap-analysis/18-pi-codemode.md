# 18 — pi `packages/codemode` and the `codemode` tool: the `v0.87.1 → v1.0.0` window

**This file is new (2026-10-02).** `packages/codemode` did not exist at `v0.87.1`. It is the first of
pi 1.0's headline features, it is a first-party package the coding agent registers as a built-in, and
no area file owned it. This file owns it, together with its pi-side consumer
`packages/coding-agent/src/extensions/codemode/`, and it adds the ids **`CODE-001`…`CODE-013`**.

**It does not own the three mechanisms codemode shares with other subsystems' owners.** `CODE-005`
(`ToolExposure` / `ToolLoadout` / `prepareLoadout`) and `CODE-006` (nested tool calls) are filed here
because codemode is where they first become load-bearing, but they are prerequisites for the MCP port
and for `tool_search` (`TOOL-052`, `04-…`) as well, and whoever schedules them should read all three
areas. The catalog gaps codemode's `models` global needs are already filed in area 01 as `PROV-102`
and `PROV-105`; the MCP default-exposure divergence is area 13's, and `CODE-001` records only
the coupling.

> **CORRECTED 2026-10-03 (file-level; read before `CODE-001` / `CODE-002`).** Three premises in this
> file were false when written and are corrected in place by CORRECTED notes. (1) **cyrup already
> embeds a JavaScript engine**: V8 through `deno_core` 0.411, used by the `workflowScript` runtime
> (`Cargo.toml:403-415`; `crates/cyrup-ext-subagents/src/workflows/scripted/engine.rs:2362-2404`;
> first added in `8de74602`, 2026-09-08, which is an ancestor of this file's code pin `592bf2c3`). So
> "nothing of this kind exists anywhere" and "a comment asserting the absence of a JS engine" are
> wrong. (2) **No `docs/adr/ADR-0012-*.md` file exists.** "ADR-0012 Cut 4" is text in
> `docs/gap-analysis/MCP-PORT-METHODOLOGY.md` §1.2 (the Cut 4 row, `:67`); `docs/adr/README.md`
> reserves 0012-0027 for the MCP port and none was written. Every "ADR-0012" below is a dangling
> reference to that methodology text. (3) **The engine question is decided:**
> `docs/adr/ADR-0031-one-javascript-engine-deno-core.md` (accepted 2026-10-03) says that if cyrup embeds
> JavaScript for any purpose it uses `deno_core`, no second engine, and supersedes the "anywhere, for
> anything" sentence as to scope. It does **not** decide whether or when to build `codemode`. The only
> open owner decision on `CODE-001` is therefore that one. Also stale: citations of
> `13-cyrup-mcp.md:493-512` / `:2081` (real: `:588`, `:2064`, `:2152`, `:2148`), the claim that
> `13e` has 0 hits for "exposure", and the upstream line cites noted on `CODE-008`, `CODE-010` and
> `CODE-013`. Upstream was re-read at **v1.0.1** (2026-10-03) for the growth notes on `CODE-002`,
> `CODE-011`, `CODE-012` and `CODE-013`; no other upstream claim in this file changed.

## Provenance and pins

| side | pin | how obtained |
|---|---|---|
| cyrup | code pin **`592bf2c3`** — the last commit touching `crates/`. Read at `fe875569`, the docs HEAD on 2026-10-02; `git diff 592bf2c3..fe875569` is docs-only, so the two name the same code | `git log -1`, `git diff --name-only 592bf2c3..fe875569` |
| pi | **`v1.0.0`** (2026-10-01), the newest tag; old pin `v0.87.1` | `git -C tmp/pi describe --tags` |

Every upstream claim was settled with `git -C tmp/pi show v1.0.0:<path>`,
`git -C tmp/pi cat-file -e <tag>:<path>` or `git -C tmp/pi log --oneline v0.87.1..v1.0.0 -- <path>`
at a named tag. **No working tree was read.** Every cyrup claim was read at `fe875569`. **No cargo
command was run.** This is a static reading.

## Scope

`packages/codemode` is **new at v1.0.0** (first commit `8562bcf66` *"feat(coding-agent): codemode and
MCP"*, first released as `0.99.0` on 2026-09-29, 12 commits in the window). It is a **standalone npm
package with zero pi dependencies**: `@earendil-works/pi-codemode`, 18 files, ~1 480 lines of `src/`,
one runtime dependency (`quickjs-wasi@3.6.2`).

**What it does.** It runs model-written JavaScript in a QuickJS VM compiled to WebAssembly, where the
*only* capability the script has is calling functions the host injected. A script's nested tool calls
never enter the LLM context — only the script's `text()`/`image()` output and its return value do. So
a model can batch twenty independent tool calls, filter a megabyte of output, and hand back three
lines.

**The shape, in the order to read it.**

| # | file @v1.0.0 | what it holds |
|---|---|---|
| 1 | `packages/codemode/README.md` | the whole contract in prose, including the `pi-agent-core` wiring recipe. Read this first. |
| 2 | `packages/codemode/src/types.ts` | `CodemodeTool` (`name`/`description`/`inputSchema`/`outputSchema`/`spread`/`signature`/`execute`), `CodemodeSandboxOptions`, `CodemodeExecuteOptions`, `CodemodeResult` (a discriminated `ok` union), `CodemodeError` with its four `kind`s (`script`/`timeout`/`aborted`/`sandbox`), `CodemodeOutputItem`, `CodemodeStoreWrites`. 136 lines, no logic — the type surface of the whole package. |
| 3 | `packages/codemode/src/source.ts` | the wire format: an optional first line `// @options: {"max_output_tokens":…, "timeout_ms":…}` followed by JS. `parseCodemodeSource()` at `:100`, `CODEMODE_SOURCE_GRAMMAR` (a Lark grammar for grammar-constrained tool input) at `:22`. |
| 4 | `packages/codemode/src/declarations.ts` | JSON Schema → TypeScript declaration rendering, so the model sees `read(args: { path: string }): Promise<string>` instead of a schema. `renderDeclarations()` `:105`, `schemaToType()` `:228`, `MCP_TYPESCRIPT_PREAMBLE` `:18`, `mcpStructuredContentSchema()` `:166`. |
| 5 | `packages/codemode/src/runtime/protocol.ts` | the host↔worker message set. Everything crosses as **JSON strings**; `WorkerData.interrupt` is a `SharedArrayBuffer` holding one `Int32` the host sets before terminating. |
| 6 | `packages/codemode/src/runtime/prelude-source.ts` | 383 lines of JavaScript-in-a-template-literal evaluated **inside** the VM. It closes over the single host bridge and builds `tools`, `ALL_TOOLS`, `text`/`image`/`exit`/`console`, `store`/`load` and the namespaced globals on top of it. `MAX_STORE_VALUE_CHARS` 256 KiB `:28`, `MAX_STORE_TOTAL_CHARS` 1 MiB. |
| 7 | `packages/codemode/src/runtime/worker.ts` | the worker thread: instantiates the wasm VM with a WASI shim (clock, random, discarded stdout) plus one host-call entry point, and polls the interrupt flag (`:60`) because Bun's `terminate()` cannot stop a thread spinning in wasm. |
| 8 | `packages/codemode/src/runtime/host.ts` | `CodemodeSandbox` `:285`. Owns the worker, the deadline (`DEFAULT_TIMEOUT_MS` 300 000 `:22`), the abort signal, the reserved-global list `:24`, and relays tool calls. |
| 9 | `packages/coding-agent/src/extensions/codemode/{index,tool,execute,renderer,worker}.ts` | pi's own consumer: the built-in `codemode` **tool**, registered **inactive** by a built-in extension. |
| 10 | `packages/coding-agent/docs/codemode.md` | the model-facing reference the tool description links to. |

## Is it user-visible in pi v1.0.0?

**Yes, but only on request — and that is the one way this area differs from area 17.** Unlike pico3
and `packages/durable`, codemode is on the shipped path: `createCodemodeExtension()`
(`extensions/codemode/index.ts:31`) registers one tool named `codemode` with `defaultActive: false`,
and a user turns it on with `--tools …,codemode` or `"defaultTools": ["+codemode"]`
(`docs/cli.md:148-162`). The tool takes **one** parameter, `code: string`, and declares
`constrainedSampling: { type: "grammar", variants: { openai_lark: CODEMODE_SOURCE_GRAMMAR } }`
(`extensions/codemode/tool.ts:380`) so capable models emit raw source instead of a JSON-escaped
string. Its `exposure` is `"model-only"` — scripts must not start scripts.

**And one default already routes through it.** At v1.0.0 the MCP extension's **default** tool
exposure is `"codemode"` (`extensions/mcp/index.ts:9-14`): MCP tools are registered callable but
**not declared to the model**, and scripts find them with `searchTools()` and `describeNamespace()`.
The extension *activates the codemode tool for that* unless `autoEnableCodemode` is false
(`extensions/mcp/config.ts:23-24`, `:61`). So in pi 1.0 the normal path to an MCP tool runs **through
codemode**. That is why every row below is a real parity gap rather than a scope note, and why
`CODE-001` is severity-bearing where area 17's rows are `tracker`s.

**What it sits beside, and what it replaces.** It sits beside the ordinary tool loop rather than
replacing it: `codemode.mode` (`on` | `only`, `core/settings-manager.ts:101`) decides whether the
other tools stay declared to the model (`on`, default) or are hidden and reachable only through
scripts (`only`). What it *does* replace is `pi-mcp-adapter`'s `mcpScript` — cyrup's **ADR-0012
Cut 4** (**CORRECTED 2026-10-03:** no ADR-0012 file exists; this is `MCP-PORT-METHODOLOGY.md` §1.2 Cut 4, and the engine it ruled out is superseded in scope by `docs/adr/ADR-0031-one-javascript-engine-deno-core.md`) — which is the decision `CODE-001` exists to surface rather than quietly assume.

**Three mechanisms codemode needs that are themselves new at v1.0.0** and are not part of
`packages/codemode`: the five-way `ToolExposure` / `ToolLoadout` / `prepareLoadout` model
(`core/extensions/types.ts:509`, `:540`, `:552`), nested tool calls through `ctx.executeTool()`
(`:394`) with their bounded record on the tool-result message (`core/nested-tool-calls.ts:26`,
`:47`), and the `codemode-store` session custom entry (`extensions/codemode/tool.ts:53`). They are
`CODE-005`, `CODE-006` and `CODE-009`.

**What is NOT new and is already in cyrup** — so a reader does not re-derive it: grammar-constrained
sampling (`ConstrainedSamplingConfig::Grammar { variants: GrammarVariants { openai_lark, .. } }`,
`crates/cyrup-core/src/constrained_sampling.rs:78`, `:94`, consumed at
`crates/cyrup-provider/src/utils/constrained_sampling.rs`), classifiers
(`crates/cyrup-provider/src/classifier.rs`, `Models::classify`), image generation
(`crates/cyrup-provider/src/images/`, `generate_images`), and the typed catalog reads
`get_models_of_type` / `get_model_of_type` (`crates/cyrup-provider/src/collection.rs:237`, `:249`).
The two catalog gaps codemode's `models` global needs are **already filed**: `PROV-102`
(`ModelType::Image` / `AnyModel::Image` unification) and `PROV-105` (`getAvailableOfType` /
`getAllAvailable` / `filterAllModels`).

**Measurement at cyrup `592bf2c3`** — every one of these is **0 hits** under `crates/` with
`--include='*.rs'`: `codemode`, `CodemodeSandbox`, `toCodemodeIdentifier`, `renderDeclarations`,
`schemaToType`, `ToolExposure`, `ToolLoadout`, `ToolNamespace`, `NestedCallRecorder`, `Bm25`. The one
`quickjs` hit is `crates/cyrup-ext-subagents/src/workflows/scripted/mod.rs:80`, a comment asserting
the *absence* of a JS engine. **CORRECTED 2026-10-03:** that is wrong. The hit is the substring `quickjs` inside `rquickjs` in a comment (`mod.rs:76-84`) that says the Cut-4 CI guard (`rg -qi 'rquickjs|boa_engine|deno_core|v8' crates/cyrup-mcp/Cargo.toml`, `MCP-PORT-METHODOLOGY.md:1382`) is scoped to `crates/cyrup-mcp/Cargo.toml` and was left as written. The same module is cyrup's V8 engine: `deno_core` 0.411.0 at `Cargo.toml:403-415` (used only by `cyrup-ext-subagents` `workflows/scripted/`), with a heap limit, a near-heap-limit callback and `terminate_execution` at `engine.rs:2362-2404`. `wasmtime` is also a dependency (`crates/cyrup-ext/Cargo.toml:29-30`, optional `wasm-host`), but it runs guest WebAssembly extensions, not JavaScript. Before this file, the ledger had **one** mention of codemode anywhere:
`EXT-092` (`06-cyrup-ext.md`), which lists it among pi's four built-in extensions whose natives must
override `is_hidden`, and files nothing.

## Open items

> **Next free id: `CODE-014`** (2026-10-02, after this file filed `CODE-001`…`CODE-013`). 2026-10-03: unchanged; the ledger-correction pass filed no new row here (corrections are CORRECTED notes on `CODE-001`, `-002`, `-003`, `-006`, `-008`, `-010`, `-011`, `-012`, `-013`).

> The standard `ID | Severity | Kind | Effort | Title` table, as README's *Item format* requires.
> **This table is the complete open set for area 18** — thirteen rows, no closures, no `-S` series and
> no second table. `scripts/count_open_items.py` lists this file as area `18`; it was added to
> `STANDARD_AREAS` in the same change that created the file, because an area the counter does not
> know is an area whose rows are silently missing from every total.
>
> **`CODE-001` is not a `tracker`.** It needs an owner decision, but it proposes work today — the
> decision itself, and the four rows (`CODE-003`, `CODE-004`, `CODE-005`, `CODE-006`) that do not wait
> on it — so it carries a severity and is counted. `CODE-002` and everything downstream of the
> sandbox are blocked on that decision and say so in their own bodies.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| CODE-001 | medium | not-ported | L | **~~DECISION REQUIRED~~ (answered 2026-10-05, see below) — `packages/codemode` and the `codemode` tool are unported in full** — new at v1.0.0; **ADR-0012 Cut 4** recorded "no JavaScript engine anywhere, for anything", and pi 1.0 makes codemode the default path to MCP tools. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** the premise is false. (a) cyrup already embeds V8 via `deno_core` 0.411 (`Cargo.toml:403-415`; `crates/cyrup-ext-subagents/src/workflows/scripted/engine.rs:2362-2404`), so "no JavaScript engine anywhere" is not true of the repository. (b) There is no ADR-0012 file; "ADR-0012 Cut 4" is `MCP-PORT-METHODOLOGY.md` §1.2 (`:67`). (c) The engine is decided by `docs/adr/ADR-0031-one-javascript-engine-deno-core.md`: `deno_core`, no second engine. The open owner decision is only **whether and when to build `codemode`**; options (a)'s `rquickjs`/`wasmtime`+`quickjs-wasi` sub-shapes in the body are rejected by ADR-0031. Severity left as filed (`medium` stays defensible: it still gates a whole upstream subsystem and the unowned MCP default-exposure divergence). **OWNER DECISION 2026-10-05: BUILD IT, SCHEDULED LATER.** This row is no longer awaiting a decision — the scope question the `CORRECTED 2026-10-03` note left as the only open one is answered: cyrup WILL port `codemode`, including the sandbox, and the work is deliberately deferred rather than declined. Nothing further is owed by the owner; what is owed is scheduling and an ADR before any lane starts. The engine is already fixed by `docs/adr/ADR-0031-one-javascript-engine-deno-core.md` (`deno_core`, no second engine), so the ADR this needs covers the SANDBOX surface, not the engine: worker/isolate boundary, the capability set a script gets, output and store budgets, and the deadline. Treat pi's own hardening history as the minimum bar — its CHANGELOG records fixes for a print loop growing host memory until it crashes (`MAX_OUTPUT_CHARS` 16 Mi / `MAX_OUTPUT_ITEMS` 100 000, #10283) and for `image()` accepting malformed base64 and unsupported types (#10215), both AFTER the initial spike. **Why it is wanted, recorded so this is not re-litigated:** MCP tools already work, so this is a SCALING gap, not a correctness one, on two axes. (1) Context cost per call — pi's README: *"Nested tool calls never enter the LLM context; only the script's output and return value do."* Twenty chained calls are twenty context round trips here and one script plus one result there. (2) Prompt cost per tool — `ToolExposure = "direct" | "model-only" | "codemode" | "deferred" | "hidden"` (`13e-mcp-tools.md:293`) lets a tool be REACHABLE without being ADVERTISED; cyrup has only advertise-or-absent, so every connected MCP server's full catalogue costs system-prompt tokens whether or not the model uses it. **`CODE-005` is severable and is the cheaper half.** The prompt-budget win comes from `ToolExposure`/`ToolLoadout`/`prepareLoadout`, NOT from the sandbox, and this file's own intro says those are prerequisites for the MCP port and for `tool_search` (`TOOL-052`) as well — so `CODE-005` can land on its own, ahead of the deferred sandbox work, and would also unblock `MCP-604`. It needs no further owner input. |
| CODE-002 | low | not-ported | L | **The sandbox host `CodemodeSandbox` is unported** — `runtime/host.ts:285` + `worker.ts` + `prelude-source.ts`: a worker thread running a QuickJS-on-wasm VM whose only imports are a WASI shim and one host-call bridge. Blocked on `CODE-001`. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** "a worker thread running a QuickJS-on-wasm VM" describes upstream and is right; but the body's "nothing of this kind exists anywhere" is false: cyrup has a V8/`deno_core` runtime for `workflowScript` (`Cargo.toml:403-415`, `engine.rs:2362-2404`). Per `docs/adr/ADR-0031-one-javascript-engine-deno-core.md`, a cyrup port of this sandbox would run on `deno_core`, not on `wasmtime`+`quickjs-wasi` or `rquickjs`. Honest isolation caveat (ADR-0031 Consequences): upstream's sandbox boundary is WebAssembly (own linear memory, one host import); a `deno_core` isolate is in-process, so a V8 memory-safety bug is a risk upstream's design does not carry. It is bounded by no ambient authority (only injected functions), a heap cap and a kill switch, and is the exposure `workflowScript` already accepts; a separate-process isolate is a later option. pi v1.0.1 growth: `PRELUDE_SOURCE` moved `prelude-source.ts:34` -> `:42`, and the prelude now enforces `MAX_OUTPUT_CHARS = 16 * 1024 * 1024` (`:36`) and `MAX_OUTPUT_ITEMS = 100_000` (`:37`) in `output()` (`:244-248`): a script past either limit throws a `RangeError` and `done()` has already reported the failure, so catching it does not resume output (`319fecb89`, `packages/codemode/CHANGELOG.md` [1.0.1], #10283). The sandbox port must carry these caps. Effort `L` left as filed; the engine choice makes it arguably lower, and that is not re-rated. |
| CODE-003 | low | not-ported | S | **`parseCodemodeSource()` and `CODEMODE_SOURCE_GRAMMAR` are unported** — the `// @options:` first line and its Lark grammar, `src/source.ts:100`/`:22`. Engine-independent and landable today: cyrup's grammar-constrained sampling already exists. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** upstream cites hold at v1.0.0 (`source.ts` `:11`, `:22`, `:100`). `source.ts` has eight `throw new` sites at v1.0.0 (`:58`, `:64`, `:69`, `:74`, `:81`, `:87`, `:102`, `:112`), not "six"; two share a message, so the number of distinct rejection messages was not re-counted here - enumerate them from source when porting. `ConstrainedSamplingConfig` is at `crates/cyrup-core/src/constrained_sampling.rs:76` (the enum), not `:78`. |
| CODE-004 | low | not-ported | M | **The declaration renderer is unported** — `renderDeclarations`/`schemaToType`/`renderToolSample`/`renderToolOutputType`/`MCP_TYPESCRIPT_PREAMBLE`/`toCodemodeIdentifier`, `src/declarations.ts:105,228,153,181,18` and `src/identifier.ts:5`. Engine-independent. **FILED 2026-10-02**; body below. |
| CODE-005 | low | not-ported | L | **The `ToolExposure` / `ToolNamespace` / `ToolLoadout` / `prepareLoadout` model is unported** — `core/extensions/types.ts:509,527,540,552`; five exposures, three of which exist for codemode and MCP. **Shared prerequisite with area 13 and with `TOOL-052` (`04-…`).** **FILED 2026-10-02**; body below. |
| CODE-006 | low | not-ported | L | **Nested tool calls are unported** — `ctx.executeTool()` (`core/extensions/types.ts:394`), `NestedCallRecorder` and `NESTED_CALL_LIMITS` (`core/nested-tool-calls.ts:47`, `:26`), `nestedCalls` on the tool-result message, `parentToolCallId` on the events. **Shared prerequisite.** **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** the body's cite `13-cyrup-mcp.md:2081` for `EventKind::ToolCall::fails_closed()` is `13-cyrup-mcp.md:2148` (line 2081 is blank). |
| CODE-007 | low | not-ported | M | **The model-facing `codemode` description and its catalog budget are unported** — `createCodemodeDescription` and the round-robin `selectCatalog`, `extensions/codemode/tool.ts:237`/`:211`, plus the `codemode.mode` and `codemode.inlineBudget` settings. **FILED 2026-10-02**; body below. |
| CODE-008 | low | not-ported | M | **The script `models` globals are unported** — `createModelGlobals` at `extensions/codemode/execute.ts:524`: `getModelsOfType`/`getAvailableOfType`/`getModelOfType`/`classify`/`generateImages`, a 4-call limiter, shape validation, and usage attribution. Depends on `PROV-102` and `PROV-105`. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** `CodemodeModelRuntime` is `packages/coding-agent/src/extensions/codemode/tool.ts:61-65`, not `:70-74` (same at v1.0.0 and v1.0.1). `execute.ts:524` (`createModelGlobals`) holds. |
| CODE-009 | low | not-ported | M | **The `codemode-store` session custom entry and its branch-scoped replay are unported** — `extensions/codemode/tool.ts:53` and `readCodemodeStore` at `execute.ts:220`: `store()`/`load()` survive resume and each branch sees only its own path's writes. **FILED 2026-10-02**; body below. |
| CODE-010 | low | not-ported | M | **The script discovery globals and the BM25 ranker behind them are unported** — `createDiscoveryGlobals` at `execute.ts:451` (`searchTools`/`describeTool`/`describeNamespace`) over `Bm25Ranker` at `extensions/tool-search/tool.ts:119`. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** `isNamespaceName()` is `extensions/codemode/execute.ts:440`, not `:437` (same at v1.0.0 and v1.0.1). `createDiscoveryGlobals` `:451` holds. |
| CODE-011 | low | not-ported | M | **The codemode TUI renderer is unported** — `codemodeRenderers` at `extensions/codemode/renderer.ts:65`: a live nested-call list with per-call status glyph, duration and USD cost, over a collapsed syntax-highlighted script preview. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03 (v1.0.1 growth):** `renderer.ts` `codemodeRenderers` `:65` is unchanged at v1.0.1. v1.0.1 adds `pi.registerToolRenderer(resolver)` (`core/extensions/types.ts:1683`, `loader.ts:367`; `packages/coding-agent/CHANGELOG.md` [1.0.1], #10285) and the MCP extension uses it to draw `mcp__<server>__<tool>` calls before their server connects (`extensions/mcp/index.ts:363-366`). A cyrup port that wants parity for resumed sessions needs a renderer-resolver hook for tools that are not registered, in addition to the codemode renderer. |
| CODE-012 | low | not-ported | S | **The script output budget, truncation and temp-file spill are unported** — `DEFAULT_MAX_OUTPUT_TOKENS` 10 000, `truncateOutput` and `spillOutput` at `execute.ts:233`, `:277`, `:262`. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03 (v1.0.1 growth):** `execute.ts` `:233`, `:262`, `:277` still hold at v1.0.1. v1.0.1 adds a second, sandbox-level output cap beneath this budget: `MAX_OUTPUT_CHARS` 16 Mi and `MAX_OUTPUT_ITEMS` 100 000 in `packages/codemode/src/runtime/prelude-source.ts:36-37` (see `CODE-002`); a script exceeding either fails with a `RangeError` before `truncateOutput` sees the text. The two caps are independent. |
| CODE-013 | low | tooling | M | **Shipping the sandbox runtime in cyrup's build has no counterpart** — `getQuickJSWasmPath()` / `resolveCodemodeWorkerSpecifier()` at `coding-agent/src/config.ts:488`, `:493` resolve a wasm asset and a worker entrypoint per release runtime. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** at v1.0.0 `getQuickJSWasmPath()` is `config.ts:488`, `setEmbeddedQuickJSWasmPath()` `:483` and `resolveCodemodeWorkerSpecifier()` `:493` (as cited); at v1.0.1 they are `:491`, `:486` and `:496`. `loadQuickJSWasm()` is `packages/codemode/src/wasm.ts:21`, not `:24` (same at both tags). The row's "if `CODE-002` lands on `rquickjs`, this row collapses" branch is moot: per `docs/adr/ADR-0031-one-javascript-engine-deno-core.md` a port would use `deno_core` (already linked, V8 fetched by `rusty_v8`'s build, `Cargo.toml:403-415`), so no wasm artifact is embedded and the open question is only how the V8 prebuilt is provisioned, which `workflowScript` already answers. "Nothing embeds a wasm artifact" for assets remains true. |

---

## CODE-001 — `packages/codemode` is unported. DECIDED 2026-10-05: build it, scheduled later

**upstream** — `packages/codemode/` at v1.0.0: 18 files, ~1 480 lines of `src/`, a standalone npm
package with one runtime dependency (`quickjs-wasi@3.6.2`) and no pi dependencies
(`packages/codemode/package.json`). Plus its pi-side consumer,
`packages/coding-agent/src/extensions/codemode/` (five files) and `docs/codemode.md`. New in the
window: `git log --oneline v0.87.1..v1.0.0 -- packages/codemode` is 12 commits, the first being
`8562bcf66 feat(coding-agent): codemode and MCP`; `git cat-file -e v0.87.1:packages/codemode/src/index.ts`
fails. The release post names it the first headline feature of 1.0.

**cyrup at HEAD `592bf2c3`** — nothing **for codemode**. `grep -rni 'codemode' crates/ --include='*.rs'` = 0;
`CodemodeSandbox`, `toCodemodeIdentifier`, `renderDeclarations`, `schemaToType` = 0 each. The ledger's
only mention is `EXT-092` (`06-cyrup-ext.md:620`), which lists `codemode` among pi's four built-in
extensions when arguing about `NativeExtension::is_hidden` — it anticipates a `builtin:codemode`
native existing one day but files nothing.

> **CORRECTED 2026-10-03 (whole section).** The decision framed below rests on a false premise. cyrup
> *does* embed a JavaScript engine: V8 via `deno_core` 0.411 for `workflowScript`
> (`Cargo.toml:403-415`, `engine.rs:2362-2404`, added in `8de74602`, 2026-09-08). There is no
> `ADR-0012` file (`ls docs/adr | grep 0012` is empty; the cuts' only text is
> `MCP-PORT-METHODOLOGY.md` §1.2 and `docs/adr/README.md`'s reservation of 0012-0027). The engine
> question is decided by `docs/adr/ADR-0031-one-javascript-engine-deno-core.md`: `deno_core`, no second
> engine, the "anywhere, for anything" sentence superseded as to scope. ADR-0031 explicitly leaves
> open only "whether or when to build `codemode`". Read the options below with that in mind: (a)'s
> engine is `deno_core`; `rquickjs` and `wasmtime`+`quickjs-wasi` are rejected; the cost of (a) is the
> sandbox host, prelude, limits and the rows downstream, not a new engine. Option (b) is unchanged.
> Also reframe the "default path" fact: cyrup ports `pi-mcp-adapter`, which replaces pi's built-in MCP
> extension, has `mcpScript` off by default (`pi-mcp-adapter` `CHANGELOG.md:60`, `:65` @v5.0.0, `settings.scriptMode`),
> and states "Pi's `codemode` scripts work with both" (`docs/pi-builtin-comparison.md:18` @v5.0.0). So the
> default-exposure divergence below is a property of pi's built-in MCP extension, not of the adapter cyrup ports.

**The decision this row exists for, stated plainly.** `docs/gap-analysis/MCP-PORT-METHODOLOGY.md:67`
and `13-cyrup-mcp.md:493-512` record **ADR-0012 Cut 4** (**CORRECTED 2026-10-03:** `13-cyrup-mcp.md:493-512` is unrelated text; Cut 4 is `13-cyrup-mcp.md:559` (section), `:588` (the 2% size row) and `:2064` (file-table row); and see the section note above - no ADR-0012 file exists) — `pi-mcp-adapter`'s `mcpScript` and its
JavaScript worker are cut by owner decision, with the consequence written in bold three times across
the ledger: *"This removes the only JavaScript-engine question in the entire port. No `rquickjs`, no
vendored C, no `boa`, no JS-in-WASM. **Do not raise a JS engine anywhere, for anything.**"*
`13-cyrup-mcp.md:2081` (**CORRECTED 2026-10-03:** `:2152`) lists "a JS engine (`rquickjs` / `boa` / JS-in-WASM), rated a from-zero
critical decision" under prerequisites that **dissolved**, resolved as "Cut 4. Not raised anywhere."

**Three facts that make Cut 4 insufficient to settle codemode, which is why this is a decision and
not a closure.** It is a severity-bearing row and not a `tracker`: it proposes work today
(the decision itself, and the four rows below that do not wait on it).

1. **Scope.** Cut 4 is scoped to `pi-mcp-adapter` — two files, 484 lines, 2% of that package
   (`13-cyrup-mcp.md` file table) — and its stated coverage argument was that
   `mcp({search}) → mcp({describe}) → mcp({tool,args})` is "the same discover/inspect/call loop, one
   call per turn instead of batched". Codemode is a **first-party pi package** that the coding agent
   itself registers as a built-in, and its scope is every tool in the session, not MCP tools.
2. **Default path.** At v1.0.0 the MCP extension's default exposure is `"codemode"`
   (`extensions/mcp/index.ts:9-14`): MCP tools are callable but **undeclared**, and the model is
   expected to reach them from scripts, with the codemode tool auto-activated unless
   `autoEnableCodemode` is false (`extensions/mcp/config.ts:23-24`). The thing Cut 4 called a 2%
   convenience became the default route. cyrup-mcp at HEAD has **no exposure concept at all**
   (`grep -rn 'exposure' crates/cyrup-mcp/src/` = 0; the mcp area files `13b`/`13e` have 0 hits for
   `exposure`), so cyrup's MCP port now diverges from upstream's default on this axis regardless of
   what is decided here. **Area 13 owns that half** — this row only records the coupling.
   **CORRECTED 2026-10-03:** "`13b`/`13e` have 0 hits" is false for `13e`: `13e-mcp-tools.md` has four hits (`MCP-604`, rows at `:98` and `:286-308`; `13-cyrup-mcp-STATUS.md:459` has one more); `13b` has none. And "area 13 owns that half" has no owning row: `MCP-604` (low) covers only the dependent piece (registering search-mode direct tools at the `deferred` exposure, and it defers the exposure model itself to `CODE-005`), and `autoEnableCodemode` appears in no `13*.md` file. The default-exposure and `autoEnableCodemode` divergence is currently unowned (not filed here; belongs to area 13).
3. **Non-LLM models.** The release post's framing — "native support for MCP, and non-LLM models like
   Jev and image models" — is, in code, `models.classify()` and `models.generateImages()` reachable
   *from scripts* (`docs/codemode.md`, "Models"). cyrup already has both engines
   (`crates/cyrup-provider/src/classifier.rs`, `crates/cyrup-provider/src/images/`). What cyrup lacks
   is only a **caller** the model can drive. That is `CODE-008`, and it is the one piece of the
   headline that is cheap only if `CODE-002` lands.

**What the three plausible decisions cost, so the decision can be made on numbers rather than
sentiment.**

- **(a) Port the sandbox.** Reverses Cut 4's scope for a new package. In Rust the natural shape is
  `rquickjs` (QuickJS bindings, the same engine) or `wasmtime` + the same `quickjs-wasi` wasm artifact
  upstream ships, which preserves the "the VM is its own wasm instance with its own linear memory"
  property verbatim and keeps the interrupt-flag design meaningful. Everything downstream
  (`CODE-003` … `CODE-013`) is then ordinary port work. `L`, and it is the largest single
  not-ported subsystem this pass found.
- **(b) Decline the sandbox, port the surfaces that do not need it.** `CODE-003`, `CODE-004`,
  `CODE-005`, `CODE-006`, `CODE-009` and parts of `CODE-012` are engine-independent, and `CODE-005`
  and `CODE-006` are needed by area 13 and by `TOOL-052` anyway. cyrup then has no `codemode`
  tool, and the MCP default-exposure divergence in (2) must be resolved by area 13 choosing
  `direct` or `deferred` as cyrup's default, **recorded as a `[CYRUP-DELTA]`**, not left implicit.
- **(c) Substitute a non-JS script host.** Rejected pre-emptively here, and recorded so nobody
  proposes it: the tool input is raw JavaScript under a Lark grammar the model is *constrained to*
  (`tool.ts:380`), and the whole value of the feature is that frontier models already write it. A
  different language is a different feature, not a port.

**Fix** — get the owner decision, write it as an ADR beside ADR-0012 (amending or confirming Cut 4
with its new scope), and only then open `CODE-002`. **CORRECTED 2026-10-03:** the ADR is written: `docs/adr/ADR-0031-one-javascript-engine-deno-core.md` (there is no ADR-0012 to sit beside). It settles the engine; what remains for the owner is whether/when to build `codemode`, after which `CODE-002` can open on `deno_core`. Until then, land the engine-independent rows
(`CODE-003`, `CODE-004`) and the two shared prerequisites (`CODE-005`, `CODE-006`) on their own merit.

**Verify** — the ADR exists and names codemode explicitly; and
`grep -rn 'rquickjs\|boa_engine\|deno_core\|wasmtime' crates/*/Cargo.toml` agrees with it in either
direction. The existing in-tree assertion at
`crates/cyrup-ext-subagents/src/workflows/scripted/mod.rs:80` must be updated or deliberately kept. **CORRECTED 2026-10-03:** that is a comment about the Cut-4 guard's scope (`crates/cyrup-mcp/Cargo.toml` only), not an assertion of absence; it can stay as written (`mod.rs:81-84` says so). The grep already shows `deno_core` (root `Cargo.toml:415`) and `wasmtime` (`cyrup-ext`).

**Severity** — `medium`, not higher and not lower. Nothing in cyrup breaks today: there is no
codemode tool to fail and no script to run. It is above `low` for one reason only — it gates a whole
upstream subsystem *and* it leaves a live, already-existing divergence in the MCP port's default tool
exposure unresolved. It is not `high`: no user-visible break, no correctness failure on a path cyrup
runs.

## CODE-002 — the sandbox host `CodemodeSandbox` is unported

**upstream** — `packages/codemode/src/runtime/host.ts:285` (`CodemodeSandbox`, with
`DEFAULT_TIMEOUT_MS = 300_000` at `:22`, the reserved-globals set at `:24`, `close()` at `:357`),
`runtime/worker.ts` (159 lines; wasm instantiation with the WASI shim, `interruptHandler` at `:60`,
the `parentPort` message loop at `:124`), `runtime/prelude-source.ts` (383 lines of in-VM JavaScript;
`PRELUDE_SOURCE` at `:34`), and `runtime/protocol.ts` (51 lines; everything crosses as JSON strings,
`WorkerData.interrupt` is a one-`Int32` `SharedArrayBuffer`). Behaviour the README pins and the
package's own tests enforce (`test/sandbox.test.ts`, 50+ cases): no timers, `fetch`, `process`,
`require`, modules or `WebAssembly` inside the VM; `eval`/`Function` work but stay inside it;
`memoryLimitBytes` overruns surface as `InternalError: out of memory`; deep recursion is a
**catchable** `RangeError` because QuickJS's stack guard is on; stack traces read `at f
(codemode.js:2:31)` with V8-style `Name: message` prefixes and exact line numbers; a script awaiting
a promise nothing can settle **fails immediately** rather than hanging (`stalled()` in the prelude);
unawaited in-flight calls are aborted through the tool's `signal` and reported `cancelled`.

**cyrup at HEAD** — nothing, and by recorded decision nothing of this kind exists anywhere:
`grep -rni 'quickjs' crates/ --include='*.rs'` returns exactly one hit,
`crates/cyrup-ext-subagents/src/workflows/scripted/mod.rs:80`, which is a comment *asserting the
absence* of `rquickjs`/`boa_engine`/`deno_core`/`v8`.
**CORRECTED 2026-10-03:** flatly false. The one hit is `rquickjs` inside a comment about the Cut-4 guard's scope (`mod.rs:76-84`); the same module is a shipped V8 runtime (`deno_core` 0.411, `Cargo.toml:403-415`; isolate with `heap_limits`, near-heap-limit callback and `terminate_execution`, `engine.rs:2362-2404`). Per `docs/adr/ADR-0031-one-javascript-engine-deno-core.md`, the **Fix** below should read `deno_core` where it says `wasmtime` + `quickjs-wasi`: the "separate wasm instance" property is not reproduced (in-process isolate; see the isolation caveat on the row), the interrupt flag becomes `IsolateHandle::terminate_execution`, `memoryLimitBytes` becomes a heap limit with a terminating callback (an out-of-memory script is terminated, not given a catchable `InternalError`), and V8's recursion limit is a catchable `RangeError` as well. Which of upstream's `test/sandbox.test.ts` expectations transfer to V8 was not checked here.

**Fix** — blocked on `CODE-001`. If (a): the faithful shape is `wasmtime` plus the same
`quickjs-wasi` wasm artifact, because that preserves the two properties the design rests on — the VM
is a separate wasm instance with its own linear memory, and the host's only reachable entry point is
one bridge function — and it keeps the interrupt-flag mechanism (`Atomics.load` on a shared `Int32`)
meaningful rather than vestigial. The prelude is JavaScript and ports as data, not code. The Rust
analogue of "worker thread" is a dedicated OS thread, not a task: QuickJS runs synchronously and
would block the runtime.

**Verify** — a port of `test/sandbox.test.ts`'s `escape hatches` and `limits and lifetime` groups:
no host globals reachable; `eval` confined; dynamic `import` rejected; `tools` and `console` frozen;
a synchronous `while(true){}` terminated on deadline; a microtask spin terminated; deep recursion a
catchable `RangeError`; an unsettleable await failing immediately.

## CODE-003 — `parseCodemodeSource()` and `CODEMODE_SOURCE_GRAMMAR` are unported

**upstream** — `packages/codemode/src/source.ts`: `CODEMODE_OPTIONS_PREFIX = "// @options:"` `:11`,
`CODEMODE_SOURCE_GRAMMAR` `:22` (a four-rule Lark grammar: `start: options_source | plain_source`),
`parseCodemodeSource()` `:100`. Two supported fields, `max_output_tokens` and `timeout_ms`; the
latter bounded by `MAX_TIMEOUT_MS = 2_147_483_647`. The contract carries exact error text for empty
input, invalid JSON, an unknown field, a non-safe-integer, a zero or over-bound `timeout_ms`, and an
options line with no code — all as `CodemodeSourceError`. **The options line is replaced by an empty
line, not removed**, so stack-trace line numbers still match the model's input. Re-exported from a
deliberately lightweight `@earendil-works/pi-codemode/source` entry so a caller can parse without
pulling in the VM.

**cyrup at HEAD** — 0 hits for `parseCodemodeSource`, `CODEMODE_SOURCE_GRAMMAR`, `@options`,
`max_output_tokens`. But the consumer side is **already there**:
`ConstrainedSamplingConfig::Grammar { variants: GrammarVariants { openai_lark, openai_regex } }` at
`crates/cyrup-core/src/constrained_sampling.rs:78`, `:94`, serialized exactly as pi does
(`:262-276`), with the provider half at
`crates/cyrup-provider/src/utils/constrained_sampling.rs::resolve_grammar_constrained_sampling` and
`GrammarToolInputJsonBuffer` for the bare-string wire form. `EXT-024` (closed 2026-09-04) and
`PROV-011` landed that. So a `cyrup`-side `codemode` tool could declare this grammar the day it
exists.

**Fix** — a small pure module: the parse, the two typed options, the six error messages verbatim, and
the grammar as a `&str` const. No dependency on `CODE-002`. Keep the "replace the line, do not remove
it" rule; it is the only non-obvious part.

**Verify** — a table test over the six rejection cases asserting the message text, plus one asserting
`parse("// @options: {...}\nfoo()").code` starts with `"\n"` so line 2 is still line 2.

## CODE-004 — the declaration renderer is unported

**upstream** — `packages/codemode/src/declarations.ts` (355 lines) and `src/identifier.ts` (12).
`renderDeclarations()` `:105` turns `CodemodeTool`s into `declare const tools: { … }` with
descriptions as doc comments; globals become `declare function` statements, and a `ns.member` global
becomes a member of `declare const ns`. `schemaToType()` `:228` is the JSON Schema → TypeScript
converter, with two guards worth porting exactly: `DEFAULT_INPUT_SCHEMA_MAX_CHARS = 16_000` (a larger
rendered type degrades to `unknown`) and `MAX_REF_EXPANSIONS = 32` local `$ref` expansions per schema,
so shared `$defs` cannot blow up the description. Recursive and remote refs render `unknown`.
`MCP_TYPESCRIPT_PREAMBLE` `:18` is the hand-written MCP `CallToolResult` type family, emitted once
when any listed tool's output schema is an MCP result (`mcpStructuredContentSchema()` `:166`).
`renderToolSample()` `:153` and `renderToolOutputType()` `:181` (the latter **new at 1.0.0**, per
`packages/codemode/CHANGELOG.md`) are what the pi-side description builder actually calls.
`toCodemodeIdentifier()` (`src/identifier.ts:5`) maps a tool name to a JS identifier by replacing
invalid characters with `_` — so `mcp__docs__search` is unchanged and `my-tool` becomes `my_tool`.

**cyrup at HEAD** — 0 hits for `renderDeclarations`, `schemaToType`, `renderToolSample`,
`MCP_TYPESCRIPT_PREAMBLE`, `toCodemodeIdentifier`. cyrup has JSON Schema handling for the *opposite*
direction (`make_strict_json_schema` in
`crates/cyrup-provider/src/utils/constrained_sampling.rs`), nothing that emits TypeScript.

**Fix** — a pure module, engine-independent, landable before `CODE-001` resolves. It is also the
piece most likely to be wanted outside codemode: any future "describe this tool to the model as
TypeScript" surface uses it. Port the two budget guards and the `unknown` degradations; they are load
bearing against a hostile MCP server's schema, not cosmetic.

**Verify** — golden-text tests over: a plain object schema; a schema with `$defs` referenced twice; a
recursive schema; a schema exceeding 16 000 rendered characters; an MCP `CallToolResult` output
schema (preamble emitted exactly once); and `toCodemodeIdentifier` over `mcp__docs__search`,
`my-tool`, `9lives` and `""`.

## CODE-005 — the `ToolExposure` / `ToolNamespace` / `ToolLoadout` / `prepareLoadout` model is unported

**Shared prerequisite. If area 13 or `TOOL-052` files the same surface, merge into whichever id is
lower and keep this body as the codemode-side rationale.**

**upstream** — `packages/coding-agent/src/core/extensions/types.ts`. `ToolExposure` `:509` is
`"direct" | "model-only" | "codemode" | "deferred" | "hidden"`, documented at `:495-507`: `direct`
and `model-only` are activated on registration, the other three are not; `codemode` is callable
whenever registered and listed in codemode descriptions; `deferred` is the same but *not* listed, and
tool search can find it; `hidden` is registered but unreachable. `ToolNamespace` `:527`
(`name`/`description`/`instructions`) groups the tools of one MCP server, with `instructions` served
only on request by `describeNamespace()`. `ToolLoadout` `:540` gives a `prepareLoadout` implementation
three ordered views — `declared`, `callable`, `registered` — plus `getExposure(name)` and
`getNamespace(name)`. `ToolLoadoutChanges` `:552` is what it may change: `descriptions` by tool name,
and `hiddenDeclarations` (tools that stay active and callable, and are still declared **in the
transcript** so the active set survives `/tree` and resume, but are left out of requests). The
agent-session side is `core/agent-session.ts:1513-1518`, whose callability predicate is
`exposure === "codemode" || exposure === "deferred" || (exposure === "direct" && active.has(name))`.
All of it is new at v1.0.0: `git grep -c ToolLoadout v0.87.1 -- <that file>` returns nothing, `v1.0.0`
returns 3.

**Why codemode needs it.** `prepareCodemodeLoadout` (`extensions/codemode/tool.ts:329`) is the only
thing that makes `codemode.mode` work, and it keys off **exposure, not the active set** — deliberately,
so that `tool_search` loading a tool does not change the codemode description and force a
redeclaration (the comment at `:325-328` says so). Without the model, `codemode.mode: "only"` cannot
exist and the description cannot stay stable.

**cyrup at HEAD** — 0 hits for `ToolExposure`, `ToolLoadout`, `ToolNamespace`, `prepare_loadout`,
`tool_namespace`. cyrup's nearest surface is permission-driven exposure filtering
(`crates/cyrup-permission-system/src/extension/agent_start.rs:20-47`, `crates/cyrup-ext/src/host/services.rs:1184`),
which is pi's `getAllTools` filtering — a different axis (allowed vs denied, not declared vs
callable). `cyrup-mcp` has no exposure concept at all.

**Fix** — the enum, the namespace record, the three loadout views with the two accessors, the
changes struct, and the callability predicate at the agent-session seam. The `hiddenDeclarations`
semantics are the subtle part: hidden from the *request* but present in the *transcript*, or resume
loses the active set.

**Verify** — a session registering one tool of each exposure: only `direct`+`model-only` are active;
only `direct`(active)/`codemode`/`deferred` are callable; `hidden` is neither; a `hiddenDeclarations`
entry is absent from the request and present in the persisted transcript, and survives a resume.

## CODE-006 — nested tool calls are unported

**Shared prerequisite with area 13** — `extensions/mcp/index.ts:20-21` states every MCP call
runs through the same pipeline, so hooks and permissions apply; that is this mechanism.

**upstream** — `core/extensions/types.ts:383-395`: `ExtensionToolContext` gains `readonly tools`
and `executeTool(name, args, options?): Promise<AgentToolCallOutcome>`, with `ExecuteToolOptions`
`:368` (`signal` defaulting to the calling tool's, and an `onUpdate`). The contract: the nested call
gets id `<calling id>/<n>`; `tool_call`, `tool_result` and `tool_execution_*` events carry
`parentToolCallId`; it does **not** appear in the transcript; and it **never rejects** for tool
failures — unknown tool, validation error, blocked call and thrown error all come back as
`isError: true`. The record is `core/nested-tool-calls.ts` (new file, same commit as codemode):
`NESTED_CALL_LIMITS` `:26` — `maxCalls: 256`, `maxArgumentBytesPerCall: 8 KiB`,
`maxArgumentBytesTotal: 32 KiB`, `maxErrorChars: 500` — and `NestedCallRecorder` `:47`, whose
snapshot becomes `nestedCalls` on the tool-result message with the summed nested `usage` folded into
that message's `usage`. One downstream consumer already depends on it:
`core/compaction/utils.ts:30-34` extracts file operations from `message.nestedCalls?.calls` when the
message is a `toolResult`, because *"calls made from codemode scripts are recorded on the script's
result"* — so without the record, compaction's file-operation tracking silently loses every edit a
script made.

**cyrup at HEAD** — 0 hits for `NestedCallRecorder`, `nested_calls`, `parentToolCallId` outside
`cyrup-ext-subagents`' unrelated workflow-summary fields
(`crates/cyrup-ext-subagents/src/workflows/types.rs:196`, which is a subagent's parent id, not this).
There is no re-entrant tool-execution path on an extension tool context:
`crates/cyrup-ext/src/host/live.rs:1720 execute_tool` is the host **calling into** a guest, the
opposite direction.

**Fix** — the re-entrant execute on the tool context (through the same validation, hooks and
permission checks as a model-issued call), the `<id>/<n>` id scheme, `parent_tool_call_id` on the
three event families, the bounded recorder with its four limits and its `complete` flag, the
`nested_calls` field on the tool-result message with usage folding, and the compaction consumer. Note
for whoever takes it: cyrup's permission gate is `ExtHooks::before_tool_call` +
`EventKind::ToolCall::fails_closed()` (recorded at `13-cyrup-mcp.md:2081` as what already serves the
MCP approval broker), so the "same checks apply" clause has an existing seam to reuse.

**Verify** — a tool that calls two others: both ids are `<parent>/1` and `<parent>/2`; neither is in
the transcript; both appear in the parent result's `nested_calls`; a denied nested call comes back
`isError` rather than unwinding; the 257th call sets `complete: false`; and a nested `edit` is visible
to compaction's file-operation extraction.

## CODE-007 — the model-facing `codemode` description and its catalog budget are unported

**upstream** — `packages/coding-agent/src/extensions/codemode/tool.ts`. `createCodemodeDescription()`
`:237` assembles: a fixed intro (raw JS, not JSON, no code fence; async function body; the
`// @options:` line), one line per global, the MCP type preamble when any listed tool needs it, and
one `### \`id\` (\`raw name\`)` section per listed tool, grouped under `## <namespace>` headings with
`(some tools not listed)` / `(tools not listed)` annotations. `selectCatalog()` `:211` is the budget:
*"like OpenCode's catalog — in each round every group places its cheapest remaining tool; a group
whose next tool does not fit drops out while the others continue"*, so every namespace is represented
before any namespace is complete. `DEFAULT_CODEMODE_INLINE_BUDGET = 3000` estimated tokens `:154`,
at 4 characters per token. **Deferred tools are never listed and do not affect the description at
all**, so it stays byte-identical while MCP servers connect or change their tools — that is the
property the whole design protects, and `prepareCodemodeLoadout` `:329` preserves it by keying on
exposure rather than the active set. Settings: `CodemodeMode = "on" | "only"` and
`CodemodeSettings { mode?, inlineBudget? }` at `core/settings-manager.ts:101-108`, read into
`Settings.codemode` at `:178`, documented at `docs/settings.md:41-42` (`inlineBudget: 0` lists only
namespaces).

**cyrup at HEAD** — nothing; 0 hits for `codemode`. The settings seam exists
(`crates/cyrup-config/src/settings/effective.rs:200 default_tools()` is pi's `defaultTools`, the
same list that would carry `+codemode`), so the config half is a getter, not a new subsystem.

**Fix** — depends on `CODE-004` (the sections are `renderToolSample` output) and `CODE-005` (the
loadout). The round-robin selection is the part to port literally rather than approximate: a naive
"cheapest first globally" starves a namespace entirely, which is exactly what the comment says it
avoids.

**Verify** — three namespaces whose cheapest tools fit but whose totals do not: every namespace has at
least one listed section; a `deferred` tool changes nothing in the rendered string; `inlineBudget: 0`
yields namespace headings and no tool sections; `mode: "only"` puts active `direct` tools into
`hiddenDeclarations` and lists them in the codemode description instead.

## CODE-008 — the script `models` globals are unported

**upstream** — `extensions/codemode/execute.ts:524 createModelGlobals()`, declared to the model as a
frozen `models` namespace and documented in full at `packages/coding-agent/docs/codemode.md`
("Models"): `getModelsOfType(type, provider?)`, `getAvailableOfType(type, provider?)`,
`getModelOfType(type, provider, id)`, `classify(model, context)`, `generateImages(model, context)`.
`CodemodeModelRuntime` (`tool.ts:70-74`) is exactly that `Pick` of the session's model registry.
Details that are contract, not incidental: at most `MAX_CONCURRENT_MODEL_CALLS = 4` `:51` model calls
in flight per script, the rest queued by `createLimiter`, so `Promise.all` over many items is safe;
`classify`/`generateImages` resolve the model **by `provider` and `id` only**, so a script-supplied
`baseUrl` or `headers` can never receive credentials, and `toModelInfo()` strips `headers` from
catalog entries for the same reason; chat models are listed but **cannot be run** from scripts;
neither call throws on a provider error — the script checks `stopReason` and `errorMessage`;
`checkClassifierContext` / `checkImagesContext` validate shape up front and fail with the expected
shape plus a pointer to the docs path, rather than letting a malformed call reach the provider; each
model call becomes a nested-call row carrying its USD cost; and script usage is returned as the tool
result's `usage` so it reaches the session cost total. A script that generates images and never calls
`image()` gets an explicit note appended.

**cyrup at HEAD** — the *engines* are ported and the *caller* is missing.
`crates/cyrup-provider/src/classifier.rs` has `ClassifierContext`, `ClassifierQuestion`,
`ClassifierAnswer`, `ClassifierStopReason` and `Models::classify`
(`crates/cyrup-provider/src/collection.rs:487`), pinned by `crates/cyrup-it/tests/llama/classify.rs`.
`crates/cyrup-provider/src/images/` has `ImagesContext`, `AssistantImages`, `ImagesApiImpl`,
`generate_images` and the OpenRouter impl (`images/openrouter.rs:34`). `get_models_of_type` and
`get_model_of_type` are at `collection.rs:237`, `:249`.

**Two gaps are already filed — reference them, do not duplicate.** `PROV-102`
(`01-cyrup-core-and-provider.md:953`, open, low, L): `ModelType` is `{Chat, Classifier}` only
(`classifier.rs:43`) and image models live in a separate stack, so there is no one catalog over all
three types — `models.getModelsOfType("image")` has nothing to answer from. `PROV-105` (`:956`, open,
low, M): `getAvailableOfType` / `getAllAvailable` / `filterAllModels` are unported, so
`models.getAvailableOfType()` — the method the docs tell scripts to use to find working ids — has no
backing. `collection.rs:215` already carries the in-tree note *"`ModelType::Image` is not ported"*.

**Fix** — after `CODE-002`: the five globals over cyrup's existing registry, the 4-slot limiter, the
two context validators with their message text, credential-safe resolution by `(provider, id)` only,
`headers` stripping, cost per nested row, and usage folded into the tool result. `PROV-102` and
`PROV-105` are hard prerequisites for two of the five.

**Verify** — a script calling `classify` over eight items never has more than four in flight; a
malformed `{ prompt }` to `generateImages` fails with the expected-shape message and never reaches a
provider; a catalog entry handed to a script carries no `headers`; a chat model passed to `classify`
is refused; the tool result's usage equals the sum of the model calls'.

## CODE-009 — the `codemode-store` session custom entry and its branch-scoped replay are unported

**upstream** — `extensions/codemode/tool.ts:53` `CODEMODE_STORE_ENTRY_TYPE = "codemode-store"` with
`CodemodeStoreEntryData { set, delete }`, and `execute.ts:220 readCodemodeStore(branch)`, which folds
every `custom` entry of that type **along the current branch, from the root**, applying deletes then
sets in order. The sandbox itself persists nothing: the caller passes the current values as
`options.store` and gets `result.storeWrites` back (`packages/codemode/README.md`, "Store"). Two
rules that are the whole point: **writes are kept only when the script succeeds** (a failed execution
reports no writes), and because the replay is per-branch, *"each branch sees only the values written
on its path"* — so `/tree` and resume behave. Limits live in the prelude and throw a `RangeError`
inside the script: `MAX_STORE_VALUE_CHARS` 256 KiB per value, `MAX_STORE_TOTAL_CHARS` 1 MiB total
(`runtime/prelude-source.ts:28`, message at `:193-195`). `load()` returns a copy, so mutating it does
not change the store; storing `undefined` deletes the key.

**cyrup at HEAD** — 0 hits for `codemode-store`. The *host* for it exists: cyrup has session custom
entries and branch reads (the `SessionEntry`/branch machinery the ledger's area `03`/`08` cover), so
this is a new entry type and a fold, not new session infrastructure.

**Fix** — the entry type, the fold (deletes before sets, root to leaf, branch-scoped), the
success-only commit, and the two size limits with their in-script `RangeError`. Independent of
`CODE-002` for the persistence half: the fold can be written and tested against synthesized entries
before any VM exists.

**Verify** — two sibling branches writing the same key see only their own value; a failed script's
writes are absent; a resumed session loads the values; a 300 KiB value throws inside the script and
commits nothing; `load` returning a copy is observable.

## CODE-010 — the script discovery globals and the BM25 ranker behind them are unported

**upstream** — `execute.ts:451 createDiscoveryGlobals()` injects three `spread: true` globals:
`searchTools(query, { limit?, namespace? })` (ranked, `DEFAULT_TOOL_SEARCH_LIMIT = 8` from
`extensions/tool-search/tool.ts:21`), `describeTool(name)` (the tool's rendered declaration, by raw
name **or** codemode identifier), and `describeNamespace(name)` (`{ name, description?, instructions?,
tools }`). The ranker is `Bm25Ranker` at `extensions/tool-search/tool.ts:119` over
`createToolSearchDocument(tool, namespace)` `:108`. `isNamespaceName()` (`execute.ts:437`) matches a
namespace by its name, its codemode identifier, or the part after its last `__` in either form — so a
script can write `describeNamespace("dev-radius")` for `mcp__dev-radius`. These three are how a
script reaches the tools the description deliberately left out, which is the entire story for MCP
tools under the default `codemode` exposure.

**cyrup at HEAD** — 0 hits for `Bm25`, `tool_search` as an extension, `searchTools`,
`describeNamespace`. The `supports_tool_search` hits in `crates/cyrup-provider/src/api/compat.rs:569`,
`:597` are a **different thing** — pi's `supportsToolSearch` provider flag for the OpenAI Responses
`tool_search_call`/`tool_search_output` pair, already ported. cyrup has no client-side ranker and no
`tool_search` tool.

**Fix** — the three globals plus a BM25 ranker. The ranker is shared with the `tool_search` tool
(which the release post's "deferred tool loading" headline rests on, and which `TOOL-052` owns):
whoever lands it first should put it where both reach it. `describeTool` accepting both the raw name
and the identifier, and `isNamespaceName`'s three-way match, are small and easy to get wrong.

**Verify** — a ranked query returns the expected ordering over a fixed tool set; `limit` and
`namespace` filters apply; `describeTool("mcp__dev_radius__search")` and
`describeTool("mcp__dev-radius__search")` both resolve; `describeNamespace("dev-radius")` resolves
`mcp__dev-radius` and returns its `instructions`.

## CODE-011 — the codemode TUI renderer is unported

**upstream** — `extensions/codemode/renderer.ts` (`codemodeRenderers` `:65`). `renderCall` shows the
script syntax-highlighted, collapsed to `CODE_PREVIEW_LINES = 10` with an expand hint bound to
`app.tools.expand`; the `// @options:` line is deliberately shown as part of the script.
`renderResult` lists nested calls **as they run** — `statusIcon` `:38` maps
`running`/`ok`/`error`/`cancelled` to `…`/`✓`/`✗`/`⊘` in `warning`/`success`/`error`/`muted`, and
`formatCall` `:51` appends truncated args (80 chars collapsed), a duration (`ms` under a second,
`0.0s` over), and for a `models.*` call its cost via `formatCost` `:28` (cents to two decimals above
$0.01, two **significant** digits below, because classifier calls cost fractions of a cent). Errors
show only when expanded. The `Script completed\nWall time …\nOutput:\n` header is stripped by the
`SCRIPT_HEADER` regex `:22` before the output is shown. The design note at `:1-8` is load bearing:
nested calls are **not** separate tool rows, because they never reach the model as tool calls.

**cyrup at HEAD** — nothing. The infrastructure is all there —
`crates/cyrup-tui/src/transcript/tool_render.rs` with per-tool renderers, `ToolRenderKind` /
`render_kind` plumbed end to end by `EXT-024` (closed 2026-09-04), and
`crates/cyrup-tui/src/markdown/highlight.rs` for syntax highlighting — so this is one renderer, not a
new capability.

**Fix** — after `CODE-002` and `CODE-006` (the rows come from the nested-call records). Port the two
number formats literally; `formatCost`'s significant-digit branch exists because the common case is
sub-cent and rounding it to `$0.00` makes the feature look free.

**Verify** — a snapshot test per status glyph; a sub-cent cost rendering as two significant digits; a
collapsed 40-line script showing 10 lines plus the hint; the script header absent from the rendered
result.

## CODE-012 — the script output budget, truncation and temp-file spill are unported

**upstream** — `execute.ts`: `DEFAULT_MAX_OUTPUT_TOKENS = 10_000` `:233` at `CHARS_PER_TOKEN = 4`,
overridable per script by `// @options: {"max_output_tokens": …}`. `truncateOutput()` `:277` joins
the text items, and when the total exceeds the budget replaces them with **one** item that keeps the
first half and the last half of the budget around an `…N tokens truncated…` marker, prefixed by
`Warning: truncated output (original token count: N)` and the total line count, with images appended
after it. `spillOutput()` `:262` writes the full text to
`<tmpdir>/pi-codemode-<16 hex>.txt` and the path is appended as
`[Full output: <path> (read with offset/limit)]`, also surfaced as
`CodemodeToolDetails.fullOutputPath` for the renderer; a failed write degrades to
`[Could not save the full output: <error>]` rather than failing the call. Result text begins
`Script completed` or `Script failed`, then `Wall time N.N seconds`, then `Output:`; a failed script
**keeps its partial output** and appends `Script error:` with the stack and a tool-call summary
stating explicitly that the calls made before the failure *are not undone*.

**cyrup at HEAD** — 0 hits. The pattern is already in cyrup for `bash`, which is where upstream says
it took it from ("Write the full text output to a temp file, like bash does"), so there is a local
precedent to match rather than invent.

**Fix** — small, and mostly independent of `CODE-002`: the budget arithmetic, the head/tail split, the
marker text, the spill path shape, and the two header forms. Worth landing with `CODE-003`, since
`max_output_tokens` arrives from the same parse.

**Verify** — output just under the budget passes through untouched; just over yields exactly one text
item whose head and tail are the original's; the spill file holds the full text; a spill failure still
returns a successful result; a failed script's partial output survives ahead of `Script error:`.

## CODE-013 — shipping the sandbox runtime in cyrup's build has no counterpart

**upstream** — `packages/coding-agent/src/config.ts:488 getQuickJSWasmPath()` resolves
`quickjs-wasi/quickjs.wasm` from the installed package, or an embedded path set by the Bun entry
through `setEmbeddedQuickJSWasmPath()` `:483`. `resolveCodemodeWorkerSpecifier(runtime, moduleUrl)`
`:493` answers per release runtime: `"./src/extensions/codemode/worker.ts"` (a **relative string**,
because Bun 1.3 on Windows cannot map an absolute `B:\~BUN` URL back to an embedded entrypoint),
`new URL("./codemode-worker.js", moduleUrl)` for the bundled Node build, and `undefined` unbundled —
in which case pi-codemode's own worker file next to the module is used.
`extensions/codemode/worker.ts` exists solely as that extra build entrypoint
(`import "@earendil-works/pi-codemode/worker";`). `loadQuickJSWasm()` (`packages/codemode/src/wasm.ts:24`)
compiles once per path and caches, retrying a failed load on the next call. Upstream has a test for
exactly this (`packages/coding-agent/test/codemode-worker-config.test.ts`), which is a signal that it
broke at least once.

**cyrup at HEAD** — no counterpart and no analogous asset. cyrup ships one Rust binary; nothing in
the workspace embeds a wasm artifact for *its own* use (`crates/cyrup-ext` loads **guest** component
wasm supplied by the user, which is the opposite direction). So the question this row records is
concrete: if `CODE-002` lands on `wasmtime` + `quickjs-wasi`, the wasm blob must be embedded
(`include_bytes!`) or installed beside the binary and resolved at runtime, and the compile-once cache
must be shared across sessions; if it lands on `rquickjs`, this row collapses to a build-dependency
note and should be closed as refuted rather than ported.

**Fix** — decide with `CODE-001`/`CODE-002`, then either embed the artifact with a
compiled-module cache keyed by path, or close this row. `tooling`, because nothing here is a behaviour
port.

**Verify** — whichever is chosen: a test that the runtime resolves its VM artifact from a fresh
install location, and that a second sandbox in the same process does not recompile it.

## Blind spots — read before the next pass

- **The wasm artifact itself was not read.** `quickjs-wasi@3.6.2` is an npm dependency, not a file in
  the pi tree, so the VM's own behaviour (its limits, its WASI surface, its build) is taken from
  `worker.ts`'s use of it and from upstream's own comments. A port that chooses `rquickjs` instead of
  the same wasm artifact (`CODE-001`(a)) is choosing a different VM, and the differences were not
  measured here.
- **`packages/codemode/test/**` was not read.** Upstream's own test suite is the cheapest source of
  the edge cases a port has to match — the interrupt path, the store limits, the output budget — and
  it is the first thing to read when `CODE-002` opens.
- **`docs/codemode.md` was read at the heading level plus its *Models* section.** It is the
  model-facing reference the tool description links to, so its exact wording is part of `CODE-007`'s
  obligation and has not been diffed word by word.
- **The `v1.0.0..HEAD` range was not read.** Every claim here is pinned to the tag. `packages/codemode`
  is twelve commits old and moving; the next pass should diff `v1.0.0..<next>` before trusting a line
  number above.
- **No cyrup-side design was done.** The effort letters are upstream's size, measured in lines and
  surfaces, not an estimate of a Rust implementation. `CODE-001`(a)'s "largest single not-ported
  subsystem this pass found" is a statement about upstream's size, and it is the figure the owner
  decision should be made against, not a schedule.
