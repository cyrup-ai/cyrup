# 18 — pi `packages/codemode` and the `codemode` tool: the `v0.87.1 → v1.0.0` window (re-read at `v1.0.4`, 2026-10-07)

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

> **UPDATE 2026-10-07 (re-pin to pi `v1.0.4`).** Area 18's pi pin is now **`v1.0.4`** (2026-10-05, the
> newest tag); `v1.0.0` and `v1.0.1` were the pins before. The window `v1.0.1..v1.0.4` was triaged for
> this area (`git -C tmp/pi log --oneline v1.0.1..v1.0.4 -- packages/codemode
> packages/coding-agent/src/extensions/codemode`; the two `CHANGELOG.md` diffs; every commit read with
> `git -C tmp/pi show <sha>`; **no working tree read**) and the result is the closure record
> *"Closure record, 2026-10-07 — the pi `v1.0.1..v1.0.4` window"* below. Everything *above* that record
> that cites upstream is still pinned as it says (`v1.0.0`/`v1.0.1`); the line cites that moved are listed
> in the record. The cyrup side of that record was read and measured at this branch's HEAD
> (`claude/eloquent-lovelace-vivq3h`), after `CODE-014`'s closure, with cargo.

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

> **Next free id: `CODE-021`** (2026-10-07: the pi `v1.0.1..v1.0.4` triage filed `CODE-018`…`CODE-020`; before that `CODE-018`, 2026-10-06, unchanged by the `CODE-014` closure, which filed no new row; after the closure pass filed `CODE-014`…`CODE-017`; before that `CODE-014` (2026-10-02, after this file filed `CODE-001`…`CODE-013`)). 2026-10-03: unchanged; the ledger-correction pass filed no new row here (corrections are CORRECTED notes on `CODE-001`, `-002`, `-003`, `-006`, `-008`, `-010`, `-011`, `-012`, `-013`).

> The standard `ID | Severity | Kind | Effort | Title` table, as README's *Item format* requires.
> **This table is the complete open set for area 18** — `CODE-001`…`CODE-013` all closed 2026-10-06 (see
> the closure record below), `CODE-014` closed 2026-10-06 (second closure record below), `CODE-018`…`CODE-020` filed from the pi `v1.0.1..v1.0.4` triage and closed the same day, 2026-10-07 (third closure record below), `CODE-015`…`CODE-017` open, no `-S` series and no second table. `scripts/count_open_items.py` lists this file as area `18`; it was added to
> `STANDARD_AREAS` in the same change that created the file, because an area the counter does not
> know is an area whose rows are silently missing from every total.
>
> **`CODE-001` is not a `tracker`.** It needs an owner decision, but it proposes work today — the
> decision itself, and the four rows (`CODE-003`, `CODE-004`, `CODE-005`, `CODE-006`) that do not wait
> on it — so it carries a severity and is counted. `CODE-002` and everything downstream of the
> sandbox are blocked on that decision and say so in their own bodies.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| ~~CODE-001~~ | ~~medium~~ **CLOSED 2026-10-06 — built; see the closure record** | not-ported | L | **~~DECISION REQUIRED~~ (answered 2026-10-05, see below) — `packages/codemode` and the `codemode` tool are unported in full** — new at v1.0.0; **ADR-0012 Cut 4** recorded "no JavaScript engine anywhere, for anything", and pi 1.0 makes codemode the default path to MCP tools. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** the premise is false. (a) cyrup already embeds V8 via `deno_core` 0.411 (`Cargo.toml:403-415`; `crates/cyrup-ext-subagents/src/workflows/scripted/engine.rs:2362-2404`), so "no JavaScript engine anywhere" is not true of the repository. (b) There is no ADR-0012 file; "ADR-0012 Cut 4" is `MCP-PORT-METHODOLOGY.md` §1.2 (`:67`). (c) The engine is decided by `docs/adr/ADR-0031-one-javascript-engine-deno-core.md`: `deno_core`, no second engine. The open owner decision is only **whether and when to build `codemode`**; options (a)'s `rquickjs`/`wasmtime`+`quickjs-wasi` sub-shapes in the body are rejected by ADR-0031. Severity left as filed (`medium` stays defensible: it still gates a whole upstream subsystem and the unowned MCP default-exposure divergence). **OWNER DECISION 2026-10-05: BUILD IT, SCHEDULED LATER.** This row is no longer awaiting a decision — the scope question the `CORRECTED 2026-10-03` note left as the only open one is answered: cyrup WILL port `codemode`, including the sandbox, and the work is deliberately deferred rather than declined. Nothing further is owed by the owner; what is owed is scheduling and an ADR before any lane starts. The engine is already fixed by `docs/adr/ADR-0031-one-javascript-engine-deno-core.md` (`deno_core`, no second engine), so the ADR this needs covers the SANDBOX surface, not the engine: worker/isolate boundary, the capability set a script gets, output and store budgets, and the deadline. Treat pi's own hardening history as the minimum bar — its CHANGELOG records fixes for a print loop growing host memory until it crashes (`MAX_OUTPUT_CHARS` 16 Mi / `MAX_OUTPUT_ITEMS` 100 000, #10283) and for `image()` accepting malformed base64 and unsupported types (#10215), both AFTER the initial spike. **Why it is wanted, recorded so this is not re-litigated:** MCP tools already work, so this is a SCALING gap, not a correctness one, on two axes. (1) Context cost per call — pi's README: *"Nested tool calls never enter the LLM context; only the script's output and return value do."* Twenty chained calls are twenty context round trips here and one script plus one result there. (2) Prompt cost per tool — `ToolExposure = "direct" | "model-only" | "codemode" | "deferred" | "hidden"` (`13e-mcp-tools.md:293`) lets a tool be REACHABLE without being ADVERTISED; cyrup has only advertise-or-absent, so every connected MCP server's full catalogue costs system-prompt tokens whether or not the model uses it. **`CODE-005` is severable and is the cheaper half.** The prompt-budget win comes from `ToolExposure`/`ToolLoadout`/`prepareLoadout`, NOT from the sandbox, and this file's own intro says those are prerequisites for the MCP port and for `tool_search` (`TOOL-052`) as well — so `CODE-005` can land on its own, ahead of the deferred sandbox work, and would also unblock `MCP-604`. It needs no further owner input. |
| ~~CODE-002~~ | ~~low~~ **CLOSED 2026-10-06 — V8 sandbox shipped; residuals CODE-016** | not-ported | L | **The sandbox host `CodemodeSandbox` is unported** — `runtime/host.ts:285` + `worker.ts` + `prelude-source.ts`: a worker thread running a QuickJS-on-wasm VM whose only imports are a WASI shim and one host-call bridge. Blocked on `CODE-001`. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** "a worker thread running a QuickJS-on-wasm VM" describes upstream and is right; but the body's "nothing of this kind exists anywhere" is false: cyrup has a V8/`deno_core` runtime for `workflowScript` (`Cargo.toml:403-415`, `engine.rs:2362-2404`). Per `docs/adr/ADR-0031-one-javascript-engine-deno-core.md`, a cyrup port of this sandbox would run on `deno_core`, not on `wasmtime`+`quickjs-wasi` or `rquickjs`. Honest isolation caveat (ADR-0031 Consequences): upstream's sandbox boundary is WebAssembly (own linear memory, one host import); a `deno_core` isolate is in-process, so a V8 memory-safety bug is a risk upstream's design does not carry. It is bounded by no ambient authority (only injected functions), a heap cap and a kill switch, and is the exposure `workflowScript` already accepts; a separate-process isolate is a later option. pi v1.0.1 growth: `PRELUDE_SOURCE` moved `prelude-source.ts:34` -> `:42`, and the prelude now enforces `MAX_OUTPUT_CHARS = 16 * 1024 * 1024` (`:36`) and `MAX_OUTPUT_ITEMS = 100_000` (`:37`) in `output()` (`:244-248`): a script past either limit throws a `RangeError` and `done()` has already reported the failure, so catching it does not resume output (`319fecb89`, `packages/codemode/CHANGELOG.md` [1.0.1], #10283). The sandbox port must carry these caps. Effort `L` left as filed; the engine choice makes it arguably lower, and that is not re-rated. |
| ~~CODE-003~~ | ~~low~~ **CLOSED 2026-10-06 — shipped** | not-ported | S | **`parseCodemodeSource()` and `CODEMODE_SOURCE_GRAMMAR` are unported** — the `// @options:` first line and its Lark grammar, `src/source.ts:100`/`:22`. Engine-independent and landable today: cyrup's grammar-constrained sampling already exists. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** upstream cites hold at v1.0.0 (`source.ts` `:11`, `:22`, `:100`). `source.ts` has eight `throw new` sites at v1.0.0 (`:58`, `:64`, `:69`, `:74`, `:81`, `:87`, `:102`, `:112`), not "six"; two share a message, so the number of distinct rejection messages was not re-counted here - enumerate them from source when porting. `ConstrainedSamplingConfig` is at `crates/cyrup-core/src/constrained_sampling.rs:76` (the enum), not `:78`. |
| ~~CODE-004~~ | ~~low~~ **CLOSED 2026-10-06 — shipped** | not-ported | M | **The declaration renderer is unported** — `renderDeclarations`/`schemaToType`/`renderToolSample`/`renderToolOutputType`/`MCP_TYPESCRIPT_PREAMBLE`/`toCodemodeIdentifier`, `src/declarations.ts:105,228,153,181,18` and `src/identifier.ts:5`. Engine-independent. **FILED 2026-10-02**; body below. |
| ~~CODE-005~~ | ~~low~~ **CLOSED 2026-10-06 — exposure model, loadout and transcript restore shipped; residuals CODE-014, CODE-015, CODE-017** | not-ported | L | **The `ToolExposure` / `ToolNamespace` / `ToolLoadout` / `prepareLoadout` model is unported** — `core/extensions/types.ts:509,527,540,552`; five exposures, three of which exist for codemode and MCP. **Shared prerequisite with area 13 and with `TOOL-052` (`04-…`).** **FILED 2026-10-02**; body below. |
| ~~CODE-006~~ | ~~low~~ **CLOSED 2026-10-06 — shipped, both extension tiers; residual CODE-016** | not-ported | L | **Nested tool calls are unported** — `ctx.executeTool()` (`core/extensions/types.ts:394`), `NestedCallRecorder` and `NESTED_CALL_LIMITS` (`core/nested-tool-calls.ts:47`, `:26`), `nestedCalls` on the tool-result message, `parentToolCallId` on the events. **Shared prerequisite.** **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** the body's cite `13-cyrup-mcp.md:2081` for `EventKind::ToolCall::fails_closed()` is `13-cyrup-mcp.md:2148` (line 2081 is blank). |
| ~~CODE-007~~ | ~~low~~ **CLOSED 2026-10-06 — shipped** | not-ported | M | **The model-facing `codemode` description and its catalog budget are unported** — `createCodemodeDescription` and the round-robin `selectCatalog`, `extensions/codemode/tool.ts:237`/`:211`, plus the `codemode.mode` and `codemode.inlineBudget` settings. **FILED 2026-10-02**; body below. |
| ~~CODE-008~~ | ~~low~~ **CLOSED 2026-10-06 — shipped; all five `models` functions backed** | not-ported | M | **The script `models` globals are unported** — `createModelGlobals` at `extensions/codemode/execute.ts:524`: `getModelsOfType`/`getAvailableOfType`/`getModelOfType`/`classify`/`generateImages`, a 4-call limiter, shape validation, and usage attribution. Depends on `PROV-102` and `PROV-105`. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** `CodemodeModelRuntime` is `packages/coding-agent/src/extensions/codemode/tool.ts:61-65`, not `:70-74` (same at v1.0.0 and v1.0.1). `execute.ts:524` (`createModelGlobals`) holds. |
| ~~CODE-009~~ | ~~low~~ **CLOSED 2026-10-06 — shipped** | not-ported | M | **The `codemode-store` session custom entry and its branch-scoped replay are unported** — `extensions/codemode/tool.ts:53` and `readCodemodeStore` at `execute.ts:220`: `store()`/`load()` survive resume and each branch sees only its own path's writes. **FILED 2026-10-02**; body below. |
| ~~CODE-010~~ | ~~low~~ **CLOSED 2026-10-06 — shipped (ranker, three script globals, `tool_search`)** | not-ported | M | **The script discovery globals and the BM25 ranker behind them are unported** — `createDiscoveryGlobals` at `execute.ts:451` (`searchTools`/`describeTool`/`describeNamespace`) over `Bm25Ranker` at `extensions/tool-search/tool.ts:119`. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** `isNamespaceName()` is `extensions/codemode/execute.ts:440`, not `:437` (same at v1.0.0 and v1.0.1). `createDiscoveryGlobals` `:451` holds. |
| ~~CODE-011~~ | ~~low~~ **CLOSED 2026-10-06 — shipped** | not-ported | M | **The codemode TUI renderer is unported** — `codemodeRenderers` at `extensions/codemode/renderer.ts:65`: a live nested-call list with per-call status glyph, duration and USD cost, over a collapsed syntax-highlighted script preview. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03 (v1.0.1 growth):** `renderer.ts` `codemodeRenderers` `:65` is unchanged at v1.0.1. v1.0.1 adds `pi.registerToolRenderer(resolver)` (`core/extensions/types.ts:1683`, `loader.ts:367`; `packages/coding-agent/CHANGELOG.md` [1.0.1], #10285) and the MCP extension uses it to draw `mcp__<server>__<tool>` calls before their server connects (`extensions/mcp/index.ts:363-366`). A cyrup port that wants parity for resumed sessions needs a renderer-resolver hook for tools that are not registered, in addition to the codemode renderer. |
| ~~CODE-012~~ | ~~low~~ **CLOSED 2026-10-06 — shipped** | not-ported | S | **The script output budget, truncation and temp-file spill are unported** — `DEFAULT_MAX_OUTPUT_TOKENS` 10 000, `truncateOutput` and `spillOutput` at `execute.ts:233`, `:277`, `:262`. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03 (v1.0.1 growth):** `execute.ts` `:233`, `:262`, `:277` still hold at v1.0.1. v1.0.1 adds a second, sandbox-level output cap beneath this budget: `MAX_OUTPUT_CHARS` 16 Mi and `MAX_OUTPUT_ITEMS` 100 000 in `packages/codemode/src/runtime/prelude-source.ts:36-37` (see `CODE-002`); a script exceeding either fails with a `RangeError` before `truncateOutput` sees the text. The two caps are independent. |
| ~~CODE-013~~ | ~~low~~ **CLOSED 2026-10-06 — no counterpart needed: V8 links into the binary** | tooling | M | **Shipping the sandbox runtime in cyrup's build has no counterpart** — `getQuickJSWasmPath()` / `resolveCodemodeWorkerSpecifier()` at `coding-agent/src/config.ts:488`, `:493` resolve a wasm asset and a worker entrypoint per release runtime. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** at v1.0.0 `getQuickJSWasmPath()` is `config.ts:488`, `setEmbeddedQuickJSWasmPath()` `:483` and `resolveCodemodeWorkerSpecifier()` `:493` (as cited); at v1.0.1 they are `:491`, `:486` and `:496`. `loadQuickJSWasm()` is `packages/codemode/src/wasm.ts:21`, not `:24` (same at both tags). The row's "if `CODE-002` lands on `rquickjs`, this row collapses" branch is moot: per `docs/adr/ADR-0031-one-javascript-engine-deno-core.md` a port would use `deno_core` (already linked, V8 fetched by `rusty_v8`'s build, `Cargo.toml:403-415`), so no wasm artifact is embedded and the open question is only how the V8 prebuilt is provisioned, which `workflowScript` already answers. "Nothing embeds a wasm artifact" for assets remains true. |
| ~~CODE-014~~ | ~~medium~~ **CLOSED 2026-10-06 — the prompt is written as `sections` diff rows and a pi-written file no longer carries it twice; both key orders pinned byte for byte** | not-ported | L | **System-prompt `sections` rows are unported, so a session file pi wrote carries its prompt twice and cyrup's `SystemMessage` key order differs from pi's loop** — `core/agent-session.ts:1689-1705` `_preparePromptAndToolLoadout`, `core/system-prompt.ts:121-230` `buildSystemPromptSections`/`diffSystemPromptSections`. The tool-declaration half shipped with `CODE-005`'s transcript restore; the prompt half did not: cyrup still sends the prompt through `Context::system_prompt` and writes no `sections`, so a pi-written file's `sections` rows replay in addition to cyrup's own prompt (pre-existing), and pi's loop writes `toolsAdded` after `timestamp` where cyrup writes `types.ts` declaration order (both load either way; a cyrup rewrite of a pi row reorders keys). **FILED 2026-10-06; CLOSED 2026-10-06** (closure record below); evidence is the `ACTIVESET` lane's measurement, `crates/cyrup-session/src/context.rs`, `crates/cyrup-agent/src/agent/run/declare.rs`. |
| CODE-015 | low | not-ported | M | **A WASM guest tool cannot supply `prepareLoadout`** — `ToolDefinition.prepareLoadout` (`types.ts:625-630`). `Tool::prepare_loadout` is synchronous because it runs inside the session's tool-state lock and a guest `setActiveTools` is synchronous by design (`HostServices::set_active_tools`); a guest export is an async wasmtime call, so `WasmTool` keeps the trait default. Native tools (the `codemode` tool) use hooks today. Needs either an async loadout resolution outside the lock or a cached per-guest answer; neither was built. **FILED 2026-10-06.** |
| CODE-016 | low | not-ported | M | **The guest side of `ctx.executeTool` has four gaps** — a guest tool cannot pass its own abort `signal` (a suspended guest has no abort handle: the nested call is cancelled only with the calling tool), a guest's `onUpdate` for a nested call receives its partial results batched after the call settles (the same results reach `tool_execution_update` events live), the instance whose own tool makes a nested call does not receive that call's `tool_call`/`tool_result`/`tool_execution_*` events (its lock is held by the call that makes them: a guest that is both permission gate and caller does not gate its own nested calls; recorded `[CYRUP-DELTA]` in `world.wit`), and a guest tool calling a tool of its own extension is refused by name (`GuestReentry`) rather than run. Native tier has none of these. **FILED 2026-10-06.** |
| CODE-017 | low | not-ported | S | **`ToolDefinition.annotations` and `ToolInfo.annotations` are unported** — `types.ts:601`, `agent-session.ts:1473` (MCP tool annotations: `readOnlyHint`/`destructiveHint`/`idempotentHint`/`openWorldHint`, "permission extensions can use them to decide which calls to confirm"). cyrup's `Tool` has no annotations accessor, so `getAllTools` rows omit them and the MCP adapter cannot attach them to the `deferred` tools it registers. **FILED 2026-10-06** (from `CODE-005`, `MCP-604` and the guest-rows lanes). |
| ~~CODE-018~~ | ~~low~~ **CLOSED 2026-10-07 — built-ins frozen before a script runs; the prelude's report decoded strictly** | upstream-drift | M | **Built-ins are not frozen, so a script that patches one breaks its own run, and the sandbox reports it badly.** pi `b223082bb` (v1.0.4, #10444; `packages/codemode/src/runtime/prelude-source.ts` `lockdown()`, `runtime/host.ts` `BridgeError`) freezes every object reachable from the built-in globals before the script runs, makes built-in globals read-only, and turns the commonly overridden prototype members (`constructor`, `name`, `message`, `toString`, `toLocaleString`, `valueOf`, `toJSON`, `Object.prototype`'s) into accessors so an instance can still override them; `describeError` coerces `name`/`message` with `String()`. cyrup's `crates/cyrup-codemode-runtime/src/sandbox/prelude.js` freezes only its own `tools`, `allTools`, `console` and namespaces and defines its own globals non-writable (`define`); the built-ins stay writable. **Measured on the V8 sandbox, 2026-10-07** (`sandbox/tests/probe_v104.rs`, output kept with the closure): none of the seven intrinsics upstream's test lists is `Object.isFrozen`; `Error.prototype.name = "Patched"` succeeds where upstream throws a `TypeError`; upstream's own *ignores patches to built-ins* script does not return `[[2], '{"a":1}']`, it runs to the deadline (`Timeout`); `Array.prototype.toJSON = () => null` makes **every** script fail with `sandbox: The script's store writes could not be read: invalid type: null, expected a sequence`; `Object.prototype.toJSON = () => 5; throw new Error("boom")` reports a `script` error with an empty message and no name; `error.message = 42` reports message `""` where upstream reports `"42"`. **The host-crash half of the commit is not a gap here.** The bridge is typed ops and every decode is a `Result` (`protocol.rs` `script_error`, `store_writes`; `execution.rs` `settle`): nothing unwraps and a malformed payload ends the execution as a `sandbox` error, so the process does not crash and `execute()` settles. What differs is strictness: `script_error` accepts a missing `message`, `store_writes` accepts entries of length 0 or 3+, and an unreadable return value is a `script` `RangeError` (a recorded delta), not upstream's `sandbox` `Sandbox bridge broken: …`. Severity low: one isolate per execution, so only the script that patched is affected. **FILED 2026-10-07.** |
| ~~CODE-019~~ | ~~low~~ **CLOSED 2026-10-07 — `image()` output saved to files readable only by the user, path named before each image; the spill is private too** | upstream-drift | M | **`image()` output is not saved to a file, and cyrup's output spill is created with the process umask.** pi `d677d0ee7` (v1.0.3, #10310; `extensions/codemode/execute.ts` `saveImages`, `utils/output-files.ts`): every distinct image a script shows is written to `<tmpdir>/pi-codemode-<16 hex>.<png/jpg/gif/webp>` (mode `0o600`, `wx`), a text item `[Image saved to <path> (<mime>, <size>)]` goes before it, a failed write becomes `[Image (<mime>, <size>) could not be saved: <error>]` and never discards the result, an image shown twice is saved once, and the tool description's globals line gains *"`image()` also saves the image to a temp file and the result names its path."* cyrup: `cyrup-codemode/src/output.rs` `plan_truncation` passes `OutputItem::Image { .. }` through untouched, `cyrup-codemode-runtime/src/tool/execute.rs` attaches the images as given, and the description's globals line is v1.0.1's. The text spill that does exist, `spill_output` / `write_spill` (`output.rs`), opens `OpenOptions::new().write(true).create_new(true)` with no mode, i.e. `0o666` minus the umask (upstream wrote the same at v1.0.1 and tightened it to `0o600` here). Model-visible effect: a later turn cannot refer to an image a script generated, and the model has no other way to reach its bytes (scripts cannot write files). **FILED 2026-10-07.** |
| ~~CODE-020~~ | ~~low~~ **CLOSED 2026-10-07 — hidden tools out of the rules, the tool list and the skills hint; guidelines shown with codemode declarations** | upstream-drift | M | **Hidden tools still shape the system prompt, and codemode does not show a tool's guidelines with its declaration.** pi `c30840c2e` (v1.0.4, #10343): `BuildSystemPromptOptions.hiddenTools`; `declaredTools = selectedTools − hiddenTools` drives the tool list, `buildRules` and the skills reader (`read`/`bash` declared → named; a hidden one that is still selected → `indirect`, *"Load a skill's file when the task matches its description."*); `ToolLoadout.getPromptGuidelines(name)`; `toCodemodeDeclaration(tool, guidelines)` appends a tool's guideline bullets to its description in the codemode tool's sections, in `ALL_TOOLS` and in `describeTool()`. cyrup shipped v1.0.1's rule (`CODE-005`) and `CODE-014` carried it: `PromptRebuilder::rebuild` (`cyrup-session-svc/src/tools.rs`) blanks only a hidden tool's snippet and says *"Its guidelines stay"*, and passes the whole active set as `selected_tools`, which `rules_section` and the skills reader (`cyrup-session/src/prompt/builder.rs`) read; `LoadoutView` has no `prompt_guidelines`; `to_codemode_declaration(tool)` (`cyrup-codemode-runtime/src/tool/description.rs`) takes none; the `getSystemPromptOptions()` bag has no `hiddenTools`. **Measured** (`cyrup-session-svc` `tests/codemode.rs::the_guidelines_of_hidden_tools_move_from_the_rules_to_their_codemode_sections`, `only` mode, `read` hidden, red before the fix): the request's `<rules>` still says *"- Use read to examine files instead of cat or sed."* for a tool the model cannot call directly, and the codemode description does not carry that guideline. Wording only, so low. **FILED 2026-10-07.** |

---

## Closure record, 2026-10-06 — what shipped for `CODE-001`…`CODE-013`

`CODE-001` was decided on 2026-10-05 ("build it, on `deno_core`, ADR-0031"). This section records what
was built, where, and what each row got wrong. Upstream is pi `v1.0.1`; every claim below was read at
that tag.

| row | shipped in | notes |
|---|---|---|
| `CODE-003` | `crates/cyrup-codemode/src/source.rs` | `parse_codemode_source`, the grammar byte-identical to `source.ts:22`; seven named error variants (the row said six rejections and a "four-rule" grammar: it is three rules and three terminals, and a non-object `@options` value is a seventh refusal) |
| `CODE-004` | `cyrup-codemode/src/{identifier,declarations}.rs` | byte parity checked against a corpus of ~1.4k schemas generated by running upstream's TypeScript; one `[CYRUP-DELTA]`: a malformed `%` escape in a `$ref` renders `unknown` where upstream's `decodeURIComponent` throws out of `renderDeclarations` |
| `CODE-010` | `cyrup-codemode/src/{rank,discovery}.rs`, `crates/cyrup-codemode-runtime/src/tool/discovery.rs`, `crates/cyrup-tool-search` | BM25 scores equal upstream's bit for bit (fdlibm `log`); the three script globals; the `tool_search` tool (`TOOL-052`) |
| `CODE-012` | `cyrup-codemode/src/output.rs` | budget, truncation, exclusive-create spill |
| `CODE-002` | `crates/cyrup-codemode-runtime/src/sandbox/` | V8 through `deno_core`, one isolate per execution; the V8-vs-QuickJS differences a script can observe are documented in `sandbox/mod.rs` and in `docs/codemode.md`: heap exhaustion is uncatchable (the uncaught result equals upstream's), `memory_limit_bytes` below 32 MiB is raised (V8 aborts the process otherwise), `WebAssembly`/`SharedArrayBuffer`/`Intl`/`queueMicrotask` are removed, ArrayBuffer memory is accounted in the prelude because V8's heap limit does not cover it, stack text is V8's |
| `CODE-005` | `cyrup-core/src/exposure.rs` (PR #189), `cyrup-agent/src/agent/run/declare.rs`, `cyrup-session*` | the exposure model, and its transcript half: tool declarations recorded as `system` rows and restored on resume, on `/tree` navigation and across a compaction; hidden declarations are recorded and never sent. **Residual: `CODE-014`, `CODE-015`, `CODE-017`.** The row's "hiddenDeclarations … survives resume" claim holds in pi's tests; pi's own CLI path always passes `initialActiveToolNames`, so the constructor restore is not what pi's CLI runs |
| `CODE-006` | `cyrup-core/src/message/nested.rs`, `cyrup-agent/src/agent/nested.rs`, `cyrup-session-svc/src/session/nested.rs`, `cyrup-ext/src/nested.rs` + WIT `host-tool` imports | `nestedCalls` on the persisted AND the live tool result (byte-compatible with pi rows), the runner and its limits, `parentToolCallId` on hook events, compaction file ops, HTML export, TUI skip. Differs from the row's fix text: the loop's events are untouched; nested events are their own variants. `cyrup:ext` world 0.15. **Residual: `CODE-016`** |
| `CODE-007` | `cyrup-codemode-runtime/src/tool/{description,loadout,mod}.rs`, `cyrup-config` `codemode.mode`/`codemode.inlineBudget` | description, round-robin catalog budget, `prepare_loadout` by exposure (not the active set), grammar-constrained sampling; the built-in is registered inactive and is `replaceable` (a same-name extension displaces it: `cyrup-ext/src/replaceable.rs`). `[CYRUP-DELTA]`: the description names the engine "V8" where upstream says "QuickJS". Settings are read at session build (cyrup settings are fixed until `/reload`) |
| `CODE-008` | `cyrup-codemode-runtime/src/tool/models.rs`, `cyrup-provider` (`Models::get_available_of_type`, `Provider::filter_all_models`) | all five functions backed, none stubbed. The row's "blocked on `PROV-102`/`PROV-105`" was stale: `ModelType` was already three-valued (`PROV-128`); `PROV-105` is closed by this change |
| `CODE-009` | `cyrup-codemode-runtime/src/tool/store.rs` | `codemode-store` custom entries with branch-scoped replay |
| `CODE-011` | `cyrup-codemode-runtime/src/renderer.rs`, `cyrup-ext/src/render_tree.rs`, `cyrup-tui` | the row's "one renderer, not a new capability" was false: the text-only render seam could not draw a live styled list, so a tree-render seam and a TUI layout for it were added. JS-exact number formatting (`Math.round`, `toFixed`, `toPrecision`). Partial results now reach registered text renderers too |
| `CODE-013` | — | no counterpart is needed: V8 links into the binary; there is no wasm asset or worker file to resolve per release runtime. The build's one external input is `rusty_v8`'s checksummed prebuilt archive (the workspace `Cargo.toml` note on `deno_core`), already true of `cyrup-ext-subagents` |
| `CODE-001` | all of the above | built |

Also closed by this work, with their own rows or without one: `TOOL-052` (`tool_search`), `TOOL-053`
(`defaultActive` and the two predicates), `MCP-604` (search-mode MCP tools at `deferred`: cyrup-mcp had
no search-mode mechanism at all, `MCP-561`, so it was built, not switched), `PROV-105`, the
`tool_result` `structuredContent` patch field in both extension tiers (`cyrup:ext` 0.16; pi
`ToolResultEventResult`, `types.ts:1442-1448`), `output_schema` delegation through the registered-tool
wrapper (every MCP tool looked like a text tool), `replaceable` built-in extensions, and the session-file
byte-compat of a zero or small `usage.cost` component (serde wrote `0.0` and `1.2e-5` where
`JSON.stringify` writes `0` and `0.000012`).

Stale evidence found: `CODE-002` (V8's heap limit does not bound ArrayBuffers; tiny limits abort the
process), `CODE-003` (rule and refusal counts), `CODE-005` ("0 hits" holds at the base; the
`agent-session.ts:1513-1518` cite is `:1515-1520`; the `emitError` cite is `:1556-1561`), `CODE-008`
(see above), `CODE-011` (see above), `CODE-013` (premise), `EXT-020` (`world.wit:180`), the `ToolInfo`
five-key pin (`EXT-060`: v1.0.1 adds `exposure`), `TOOL-052` ("survives resume": it does now, through
the transcript restore), `MCP-604` ("decide whether lazy-inactive is redundant": there was no lazy
mechanism), `SESS-035` (closed as "REFUTED (residual)": `builder.rs` still passes `DocsPointers::default()`,
so no production prompt carries the docs section; `docs_dir()` now exists, wiring it is a system-prompt
change that this work did not make).

## Closure record, 2026-10-06 — `CODE-014` (system-prompt `sections` rows) and `SESS-054` (the tagged layout)

Upstream is pi `v1.0.0` for this record, the pin area 18 names; every cite was read with
`git -C tmp/pi show v1.0.0:<path>`. The `v1.0.1` pin of the section above is not used here.

**What was wrong, in user terms.** A session file pi wrote starts with a `system` row whose
`sections` hold the whole prompt. cyrup read that row and replayed it, and *also* sent its own prompt
through `Context::system_prompt` — so the model was told who it was, what tools it had and what the
rules were twice, in two layouts, one of them from a harness the user had left. And a file cyrup
wrote carried no `sections` at all, so pi resuming a cyrup session found no prompt in it.

**What shipped**

| piece | where | pi |
|---|---|---|
| `build_sections` — the ordered section map | `cyrup-session/src/prompt/builder.rs` | `buildSystemPromptSections`, `system-prompt.ts:121` |
| `diff_system_prompt_sections`, `render_sections` | `cyrup-session/src/prompt/sections.rs` | `:204`, `getSystemMessageText` |
| the session prepares a prompt row at run start and at every turn boundary | `cyrup-session-svc/src/session/prompt_update.rs`, `run.rs::assemble_run_messages`, `hooks.rs::prepare_next_turn` | `_preparePromptAndToolLoadout`, `agent-session.ts:1689`, called at `:2058` (run start) and `:887` (turn boundary) |
| the agent is built with no prompt of its own | `cyrup-session-svc/src/builder.rs` | `sdk.ts:389` (`systemPrompt: ""`) |
| `before_agent_start`'s replacement projected onto the request, never persisted | `cyrup-session-svc/src/hooks.rs::project_forced_prompt` | `forceSystemPrompt`, `_installAgentForcedPromptProjection` |
| key order | `cyrup-core/src/message/system.rs` | `declareToolChanges` appends tool keys after `timestamp`; `getCurrentSystemMessage` (compaction `systemMessage`) puts `toolsAdded` before it |
| subagent prompt stripping | `cyrup-ext-subagents/src/prompt_runtime.rs` | the section delimiters are `<skills>`/`</skills>` now |

**Judgement calls**

1. *Does the diff change what is sent to the model, or only what is in the file?* Both, and they are the
   same change: pi has no other prompt path (`Agent` is built with an empty prompt; the loop sends
   `normalizeContext({messages})`), so cyrup's `Context::system_prompt` is now always empty and the
   model reads the replay of the file. The replay of the rows the session writes equals the prompt
   `build_sections` renders (`a_new_session_writes_its_prompt_once_and_the_model_reads_that_text`), so
   with the same inputs the model reads the same words — **except for the layout**, below.
2. **Model-visible behaviour change, stated plainly:** the prompt is laid out as pi's tagged sections
   (`<tools>`, `<rules>`, `<docs>`, `<addendum>`, `<project_context>`, `<skills>`, `<cwd>`, joined by a
   blank line) instead of `Available tools:` / `Guidelines:` / `Current working directory:`. That is
   `SESS-054`, closed here, because pi's `sections` *are* the tagged form: writing diff rows of the old
   layout would have been a layout pi's own rows never contain. The preamble keeps cyrup's identity
   line (the standing `[CYRUP-DELTA]`).
3. *Unknown section keys on read-modify-write.* A row cyrup did not write is preserved byte for byte
   (`Sections` keeps unknown names and their order; `a_rewrite_gives_every_pi_system_row_back_byte_for_byte`).
   A section a pi file carries that cyrup's builder does not produce (pi extensions add them) is
   **removed by a `null` in the next diff row, never edited** — which is what pi does when its rebuilt
   map lacks a name (`diffSystemPromptSections` emits `null` for every previous name absent from the
   current map). Pinned by `a_section_cyrup_does_not_build_is_removed_by_a_null_and_never_edited`.
4. *Two key orders.* Both are pi's and both are kept: a `message` row is `role, content, sections?,
   timestamp, toolsAdded?, toolsRemoved?`; a compaction entry's `systemMessage` is `role, content,
   sections?, toolsAdded?, timestamp` (never `toolsRemoved`). cyrup's old order, `role, content,
   sections?, toolsAdded?, toolsRemoved?, timestamp`, was neither.
   `serde_json/preserve_order` **is** enabled workspace-wide (root `Cargo.toml`, pinned by
   `preserve_order_is_declared_workspace_wide`), contrary to the `Sections` doc comment, which said it
   was not and is corrected; the byte assertions would catch the opposite anyway.

**Corrections to the row and the brief**

- `crates/cyrup-session/src/context.rs` is **not** the prompt path. It is the transcript→messages walk
  (`build_session_context`, pi `session-manager.ts:325-433`) and contains no `system_prompt`. The
  prompt path was: `cyrup-session/src/prompt/builder.rs` (`SystemPromptBuilder`) →
  `cyrup-session-svc/src/{builder,tools}.rs` (`PromptRebuilder`) → `AgentBuilder::system_prompt` →
  `cyrup-agent/src/agent/run/stream.rs:144-148` → `cyrup-provider/src/context.rs:8-14`
  (`Context::system_prompt`). The last two links are what the double prompt travelled through.
- "What already exists" was half true. Replay did exist (`cyrup-provider/src/utils/transcript.rs`,
  `get_current_system_message`/`get_current_system_prompt`, with the `:195-196` empty-sections rule
  there, not in `cyrup-core`; `text.rs:75` is `section_values`; `declare.rs:135` is the
  empty-system-row filter). **Nothing wrote a prompt row**, and the loop sent the replayed rows
  *and* its own `Context::system_prompt` — which is the bug.
- The row said the key-order difference was "`types.ts` declaration order". The old order was cyrup's
  own (above), and pi has two orders, not one.
- The old SESS-054 text and the doc comments in `builder.rs` cited `system-prompt.ts` line numbers of
  v0.87.1; re-derived at v1.0.0.
- `strip_inherited_skills` (subagents) removed a constant that never appeared in a real prompt, and its
  tests used fabricated fixtures; it now strips `<skills>…</skills>` and its fixtures come from the real
  builder.

**Not ported, with evidence**

- `AGENT-039`'s Agent-level fold (the initial `systemPrompt`/`tools` becoming a leading system message,
  `agent.ts:83-86`): not ported, a judgement call — cyrup's `Agent` is built without a prompt
  (as pi's `sdk.ts:389` does), so the fold has no input in the session path. It stays open for
  `Agent` users who still pass `system_prompt` directly.
- pi's docs section line "When asked about" (`system-prompt.ts:158`): omitted by `docs_section`; cyrup
  has no docs root to point at (`SESS-035`, residual).
- `usage` entries (`SESS-051`'s remaining clause) and `EXT-084` (mutable `systemPromptOptions`) stay open.
- The permission extension's text sanitizer still looks for `Available tools:` / `Guidelines:`
  (`cyrup-permission-system/src/sanitize/`, `extension/agent_start.rs:130`) and so strips nothing from
  the new layout (its own tests, `cyrup-it/tests/permission/context_hygiene.rs:148`, feed it the old
  literal). A denied tool is kept out of the prompt at the source instead — `build_sections` is given
  the filtered tool set, pinned by `a_tool_a_handler_hides_is_not_in_the_prompt_the_model_reads` — so
  this is a sanitizer whose premise went stale, not a leak, but it is dead text now. Recorded as a
  residual lead, not filed as a counted row. pi-subagents upstream still uses the old headers too.

**Evidence.** Differential: `cyrup-test-support/fixtures-capture/code014/` runs pi's own
`buildSystemPromptSections`, `diffSystemPromptSections` and agent loop under Node 22 and writes
`fixtures/pi/code014-prompt-sections.pi-captured.json`; `prompt_sections_differential.rs` compares
cyrup's sections, diffs and rewritten rows with it. Red proofs (revert production code, never the
test): re-adding the agent prompt fails `a_prompt_pi_wrote_is_replaced_not_repeated` and four more;
writing the whole blob instead of the diff fails seven; the old key order fails six; the compaction
snapshot order, the forced-prompt projection, the turn-boundary reconciliation, diff removals, the
base-prompt-follows-tools update and the differential anchor each fail their own named tests.

## Closure record, 2026-10-07 — the pi `v1.0.1..v1.0.4` window (`CODE-018`…`CODE-020`; `TOOL-057` in area 04)

Upstream is pi **`v1.0.4`** for this record, read only through git objects
(`git -C tmp/pi show v1.0.4:<path>`, `git -C tmp/pi log --oneline v1.0.1..v1.0.4 -- <path>`,
`git -C tmp/pi diff v1.0.1 v1.0.4 -- <path>`); the cyrup side was read at this branch's HEAD and measured by
running it. Where `v1.0.1` and `v1.0.4` were compared, the row says what differs; no claim is made that
anything is unchanged between them unless the row says both were read.

**What the window holds for this area.** Nine commits touch `packages/codemode` or
`packages/coding-agent/src/extensions/codemode` in `v1.0.1..v1.0.4`: three that matter here (`b223082bb`,
`d677d0ee7`, `c30840c2e`), three `Release` commits and three `[Unreleased]` housekeeping commits. Two more
that the coding-agent changelog lists for codemode touch other paths (`021eae60a` in `core/tools/read.ts`,
`1b094148b` in `config.ts` and the bundle script) and are in the table too.

| upstream | what it does | cyrup before | disposition |
|---|---|---|---|
| `b223082bb` (v1.0.4, #10444) `fix(codemode): survive scripts that patch built-ins` | `lockdown()` in `prelude-source.ts` freezes the reachable built-ins and makes built-in globals read-only, with "override mistake" accessors; `describeError` coerces with `String()`; `host.ts` `BridgeError` fails an execution whose worker payload is malformed with a `sandbox` error | built-ins writable, probes below | **`CODE-018` closed.** The *host-crash* half is **not a gap**: the bridge is typed ops and every decode is a `Result` (measured: nothing unwraps; a malformed payload already ended the execution as a `sandbox` error). The *strictness* half was a gap and closed with it |
| `d677d0ee7` (v1.0.3, #10310) `feat(coding-agent): save codemode image() output to temp files` | `image()` writes each distinct image to `pi-codemode-<16 hex>.<ext>` (0600, `wx`) and names the path in the result before the image; every output file becomes owner-only | images unlabelled; the spill file mode 644 | **`CODE-019` closed** (codemode half) and **`TOOL-057` closed** (the two bash spill sites, area 04). The MCP-blob half is the area-13 lead below |
| `c30840c2e` (v1.0.4, #10343) `fix(coding-agent): keep hidden tools out of prompt rules and skills hint` | `hiddenTools` in the prompt options; `ToolLoadout.getPromptGuidelines()`; codemode declarations carry guidelines | rules and skills hint named hidden tools | **`CODE-020` closed** |
| `021eae60a` (v1.0.4, #10251) `resolve codemode read calls on images to image blocks` | `read` declares `outputSchema` and sets `structuredContent` | no `output_schema` on `read` | **`TOOL-058` open** (area 04, S) |
| `1b094148b` (v1.0.3, #10439) `keep codemode working after the install is updated or removed` | resolves the QuickJS wasm path once, spawns the bundled worker from an in-memory `data:` URL, and shows a restart hint when errors follow an on-disk install change (`config.ts`, `interactive-mode.ts`, the bundle script) | there is no wasm asset or worker file to resolve at run time: V8 links into the binary (`CODE-013`) | no row. Read from the commit's stat and message; the diff was not read line by line, so "no counterpart" rests on `CODE-013`'s closure, not on a second reading |

**Measured on this branch before any change** (`cargo nextest`, V8 sandbox, session-svc; outputs in the
session scratchpad `redproofs-v104/code018-00-BEFORE-probe.txt`, `code019-01-BEFORE.txt`,
`code020-01-BEFORE.txt`; the throw-away probe module `sandbox/tests/probe_v104.rs` that produced the first was deleted when its cases became the real tests): none of the seven intrinsics upstream's test names was `Object.isFrozen`;
upstream's own *ignores patches to built-ins* script ran to the deadline; `Array.prototype.toJSON = () =>
null` made every script fail with `sandbox: The script's store writes could not be read`;
`Error.prototype.name` could be overwritten; `error.message = 42` reported `""`; the text spill was mode
`644`; no image carried a label; with `read` hidden in `only` mode the request's `<rules>` still said *"Use
read to examine files instead of cat or sed."*.

**What shipped, and where.**

| row | shipped in | notes |
|---|---|---|
| `CODE-018` | `cyrup-codemode-runtime/src/sandbox/{prelude.js,protocol.rs,execution.rs}` | `lockdown()` after the prelude's own setup and before the script, covering the intrinsics reachable only from instances (generator, async function, typed-array prototypes, …); `describeError` `String()`; `BridgeError`, a strict `script_error` and `store_writes` (a missing `message`, an entry of length 0 or 3+ is now refused with the reason upstream names). **Cost, measured in the dev profile: about 8–10 ms per execution** (one isolate per execution; release not measured). **Recorded delta:** an unreadable *return value* is still a `script` `RangeError`, where upstream reports a `sandbox` `Sandbox bridge broken: …` |
| `CODE-019` | `cyrup-codemode/src/output.rs`, `cyrup-codemode-runtime/src/tool/{execute,description}.rs`, `Cargo.toml` (`base64`) | labels are added **after** truncation, so a path is never cut; a write that fails becomes `[Image (<mime>, <size>) could not be saved: <error>]` and the image is kept; a type with no extension is refused before anything is written; sizes use `formatSize`'s `toFixed(1)` ties-away-from-zero rounding. The tool description's globals line and `docs/codemode.md` now say so |
| `CODE-020` | `cyrup-core/src/exposure.rs` (`LoadoutView::prompt_guidelines`, `normalized_prompt_guidelines`), `cyrup-session/src/prompt/{builder,skills_inject}.rs`, `cyrup-session-svc/src/{tools,builder}.rs`, `cyrup-codemode-runtime/src/tool/{description,loadout,execute}.rs` | the prompt follows the *declared* set (active minus hidden); a hidden reader that is still selected makes the skills hint say *"Load a skill's file when the task matches its description."* (pi's `indirect`); the fingerprint hashes the hidden set; the `getSystemPromptOptions()` bag gains `hiddenTools` |
| `TOOL-057` | `cyrup-tools/src/output.rs` (`create_output_file`), `cyrup-session-svc/src/bash.rs` | the row named one site; there were **two** `File::create` spills (the accumulator and the user-`!` bash buffer). **Recorded delta:** names keep cyrup's `pid-nanos-counter` suffix, not upstream's 8 random bytes; exclusive create, not the name, is what stops a planted path |

**Red proofs.** Every new test was seen to fail against the old behaviour, by reverting one production piece
at a time (a "mutation"; the mutation diff and the failing output of each are in the session scratchpad
`redproofs-v104/`, not in the repository): `CODE-018` 8 mutations (no lockdown → 4 tests fail; no `String()`
coercion, no override accessors, instance-only intrinsics left writable, globals writable, lenient
`script_error`, lenient `store_writes`, `settle` swallowing the bridge error → one named test each);
`CODE-020` 10 (declared names, skills reader, rebuild, build-time hidden set, loadout guidelines,
`describeTool`, view accessor, declaration bullets, options bag, indirect text → 1–3 tests each);
`CODE-019` 9 (no labels → 5 tests; no dedupe, not private → 6, not exclusive, failure label, no extension
check, `formatSize` ties, wiring → 6, description line); `TOOL-057` 4 (no mode → 2; not exclusive → 1; each
bash site back to `File::create` → 1 each).

**Upstream line cites that moved or were wrong** (re-found by symbol, not shifted): `PRELUDE_SOURCE`
`prelude-source.ts:42` → **`:46`**; `MAX_OUTPUT_CHARS` / `MAX_OUTPUT_ITEMS` `:36-37` → **`:40-41`**;
`CodemodeSandbox` `host.ts:285` → **`:340`**; the `constrainedSampling` grammar line `tool.ts:380` →
**`:395`**. **`CodemodeModelRuntime` `tool.ts:70-74` was wrong at every tag read** (`v1.0.0`, `v1.0.1`,
`v1.0.4`): the `export type` is at `:61-64`, and `:70-74` is the `models?: boolean` option.

**Not done, and leads for other areas** (filed there, not widened here):

- `TOOL-058` (open, area 04): `read` has no `output_schema`, so `image(await tools.read({ path }))` shows
  nothing in a cyrup script. Needs the structured result at every return site of `read.rs`.
- `MCP-616` (open, area 13, filed by this triage): `--tools` / `--exclude-tools` `*` patterns, `--tools`
  keeping MCP tools, and `--no-mcp` (pi `04b97ef00`). It couples to this area: `--tools codemode` is what
  `codemode.mode: only` invites, and there a script reaches no MCP tool in cyrup. Two other `v1.0.4` MCP
  commits (`147b50281`, OAuth `application_type`; `8c911797c`, `close()` of a connecting server) are leads in
  `13-cyrup-mcp-STATUS.md`, not rows.
- The *binary MCP resources* in `d677d0ee7`'s output-file change: cyrup-mcp's blob path already writes
  `0o600` + `create_new` (`cyrup-mcp/src/renderers.rs`, read, not measured on a running binary), so no row.
- Not in this area and not read for a row, only seen in the coding-agent changelog: the Azure provider rename
  and Foundry Chat Completions (`1.0.3`, area 01), `Home`/`End` (`1.0.3`, area 07),
  `samplingParamsByThinkingLevel` (`1.0.2`, area 01), the Bedrock retry and the syntax-highlighting fix
  (`1.0.4`). Not triaged; the next full pass over those areas should diff from their own pins.
- `packages/codemode/CHANGELOG.md` (`1.0.2`…`1.0.4`) holds exactly the two `1.0.4` entries (lockdown; the
  `sandbox` error for malformed payloads) and empty `1.0.2` / `1.0.3` sections. The coding-agent
  `CHANGELOG.md` adds, for this area, the image-save, hidden-tool and `read`-image entries above and the
  install-update fix (`1b094148b`).

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
- **CORRECTED 2026-10-07: `v1.0.1..v1.0.4` was read** for this area (closure record above); the range `v1.0.0..v1.0.1` was read on 2026-10-03 for the growth notes only. What remains unread is anything after `v1.0.4`.
- **The `v1.0.0..HEAD` range was not read.** Every claim here is pinned to the tag. `packages/codemode`
  is twelve commits old and moving; the next pass should diff `v1.0.0..<next>` before trusting a line
  number above.
- **No cyrup-side design was done.** The effort letters are upstream's size, measured in lines and
  surfaces, not an estimate of a Rust implementation. `CODE-001`(a)'s "largest single not-ported
  subsystem this pass found" is a statement about upstream's size, and it is the figure the owner
  decision should be made against, not a schedule.
