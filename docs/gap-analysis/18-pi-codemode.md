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

> ### CLOSURES 2026-10-09 — `CODE-021` and `CODE-022` closed (filed the same day by the pi v1.1.0 drift triage below and by this pass); `CODE-023` stays open
>
> The pi v1.1.0 structured-results pass (`TOOL-054`, `TOOL-058` in area 04, closed 2026-10-09) ran the real
> `bash` and `read` through codemode on real V8 (`cyrup-session-svc`
> `tests::codemode::resolves_bash_calls_to_structured_results_also_for_non_zero_exit_codes` and
> `::resolves_read_calls_to_text_for_text_files_and_to_image_blocks_that_image_shows`). The bash test matches
> upstream exactly; the read test cannot, because upstream's expected text carries `==> text N/M <==` lines from
> pi `eb326d265` (v1.1.0, *"separate codemode output items"*), which is newer than this area's `v1.0.4` pin and
> had no row on this pass's base. Read on both sides at v1.1.0 and filed as **`CODE-021`** (open, low).
> This pass did not triage the rest of `v1.0.4..v1.1.0`; the pi v1.1.0 drift triage (#210, same day, §*Triage
> 2026-10-09* below) re-pinned the area to `f1b2e77f5` and filed the same two findings as `CODE-021` and `CODE-022`,
> so on the rebase onto #210 this pass's rows were merged into those ids rather than renumbered: the closures
> below close #210's rows, whose bodies are kept.
>
> **CLOSED 2026-10-09 (review pass, same day): `CODE-021`.** pi `eb326d265` is ported whole; see the row for the
> evidence.
>
> **FILED AND CLOSED 2026-10-09 (second review pass, same day): `CODE-022`.** pi `269121616` (v1.1.0, #10555),
> found on the globals line `CODE-021` edited: the description now says `await` before `searchTools`,
> `describeTool` and `describeNamespace`.
>
> **Counted set after the rebase onto #210 (`count_open_items.py`): 4 low open** (`CODE-015`, `CODE-016`, `CODE-017`,
> `CODE-023`), **19 closed** (was 6 low open, 17 closed on `main` @ `77daee4`). Next free id unchanged: `CODE-024`.

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

> **Re-pinned 2026-10-09 to pi `f1b2e77f5` (= `v1.1.0-11-gf1b2e77f5`)** by the pi v1.1.0 drift triage; the window record, the rows filed and the not-filed list are in §*Triage 2026-10-09 — pi `v1.0.4..f1b2e77f5` (codemode) and pi `v1.0.1..f1b2e77f5` (codemode extension)* below. The table here is the earlier pin, kept as history.

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

> **CORRECTED 2026-10-09 (grammar sampling).** 'Already in cyrup' above is the *declaration* and the
> resolver (`utils/constrained_sampling.rs`); no adapter sent a grammar tool as a `custom` tool, a
> `custom_tool_call` or its output until `PROV-101` closed on 2026-10-08 (`ad419d5b4`). Until then every
> provider got `{ "code": "<JSON-escaped script>" }` while the description promised raw JavaScript
> (`CODE-037`).

**Measurement at cyrup `592bf2c3`** — every one of these is **0 hits** under `crates/` with
`--include='*.rs'`: `codemode`, `CodemodeSandbox`, `toCodemodeIdentifier`, `renderDeclarations`,
`schemaToType`, `ToolExposure`, `ToolLoadout`, `ToolNamespace`, `NestedCallRecorder`, `Bm25`. The one
`quickjs` hit is `crates/cyrup-ext-subagents/src/workflows/scripted/mod.rs:80`, a comment asserting
the *absence* of a JS engine. **CORRECTED 2026-10-03:** that is wrong. The hit is the substring `quickjs` inside `rquickjs` in a comment (`mod.rs:76-84`) that says the Cut-4 CI guard (`rg -qi 'rquickjs|boa_engine|deno_core|v8' crates/cyrup-mcp/Cargo.toml`, `MCP-PORT-METHODOLOGY.md:1382`) is scoped to `crates/cyrup-mcp/Cargo.toml` and was left as written. The same module is cyrup's V8 engine: `deno_core` 0.411.0 at `Cargo.toml:403-415` (used only by `cyrup-ext-subagents` `workflows/scripted/`), with a heap limit, a near-heap-limit callback and `terminate_execution` at `engine.rs:2362-2404`. `wasmtime` is also a dependency (`crates/cyrup-ext/Cargo.toml:29-30`, optional `wasm-host`), but it runs guest WebAssembly extensions, not JavaScript. Before this file, the ledger had **one** mention of codemode anywhere:
`EXT-092` (`06-cyrup-ext.md`), which lists it among pi's four built-in extensions whose natives must
override `is_hidden`, and files nothing.

## Open items

> **Next free id: `CODE-056`** (2026-10-10, after the codemode follow-up, rebased onto `main`: the follow-up closed `CODE-042`, `CODE-043` and `CODE-045` and filed `CODE-046`…`CODE-055`, of which `CODE-046`…`CODE-051` were closed on filing and `CODE-052`…`CODE-055` are open; 2026-10-09: the codemode end-to-end repair filed `CODE-024`…`CODE-045` (the branch had numbered them `CODE-021`…`CODE-042`; `main`'s pi v1.1.0 drift triage and structured-results pass had taken `CODE-021`…`CODE-023`, so the rebase moved the whole run up three); before that `CODE-024`, 2026-10-09, unchanged by the pi v1.1.0 structured-results pass, rebased onto #210: it closed `CODE-021` and `CODE-022`, which it had filed under the same ids for the same findings as the drift triage; 2026-10-09: the pi v1.1.0 drift triage filed `CODE-021`…`CODE-023`; 2026-10-07: the pi `v1.0.1..v1.0.4` triage filed `CODE-018`…`CODE-020`; before that `CODE-018`, 2026-10-06, unchanged by the `CODE-014` closure, which filed no new row; after the closure pass filed `CODE-014`…`CODE-017`; before that `CODE-014` (2026-10-02, after this file filed `CODE-001`…`CODE-013`)). 2026-10-03: unchanged; the ledger-correction pass filed no new row here (corrections are CORRECTED notes on `CODE-001`, `-002`, `-003`, `-006`, `-008`, `-010`, `-011`, `-012`, `-013`).

> The standard `ID | Severity | Kind | Effort | Title` table, as README's *Item format* requires.
> **This table is the complete open set for area 18** — `CODE-001`…`CODE-013` all closed 2026-10-06 (see
> the closure record below), `CODE-014` closed 2026-10-06 (second closure record below), `CODE-018`…`CODE-020` filed from the pi `v1.0.1..v1.0.4` triage and closed the same day, 2026-10-07 (third closure record below), `CODE-015` and `CODE-017` closed 2026-10-09, `CODE-016` narrowed to three gaps and `CODE-023` open, `CODE-021`…`CODE-023` filed 2026-10-09 by the pi v1.1.0 drift triage and `CODE-021`, `CODE-022` closed the same day (the structured-results pass had filed them under the same ids), `CODE-024`…`CODE-045` filed 2026-10-09 by the codemode end-to-end repair, renumbered on its rebase onto `main` (`CODE-024`…`CODE-041` closed on filing, `CODE-042`…`CODE-045` open; fourth closure record below), `CODE-042`, `CODE-043` and `CODE-045` closed 2026-10-10 and `CODE-046`…`CODE-055` filed that day (`CODE-046`…`CODE-051` closed on filing, `CODE-052`…`CODE-055` open; the *Follow-up, 2026-10-10* subsection of the fourth closure record; `CODE-044` stays open), no `-S` series and no second table. `scripts/count_open_items.py` lists this file as area `18`; it was added to
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
| ~~CODE-002~~ | ~~low~~ **CLOSED 2026-10-06 — V8 sandbox shipped; residuals CODE-016** | not-ported | L | **The sandbox host `CodemodeSandbox` is unported** — `runtime/host.ts:285` + `worker.ts` + `prelude-source.ts`: a worker thread running a QuickJS-on-wasm VM whose only imports are a WASI shim and one host-call bridge. Blocked on `CODE-001`. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** "a worker thread running a QuickJS-on-wasm VM" describes upstream and is right; but the body's "nothing of this kind exists anywhere" is false: cyrup has a V8/`deno_core` runtime for `workflowScript` (`Cargo.toml:403-415`, `engine.rs:2362-2404`). Per `docs/adr/ADR-0031-one-javascript-engine-deno-core.md`, a cyrup port of this sandbox would run on `deno_core`, not on `wasmtime`+`quickjs-wasi` or `rquickjs`. Honest isolation caveat (ADR-0031 Consequences): upstream's sandbox boundary is WebAssembly (own linear memory, one host import); a `deno_core` isolate is in-process, so a V8 memory-safety bug is a risk upstream's design does not carry. It is bounded by no ambient authority (only injected functions), a heap cap and a kill switch, and is the exposure `workflowScript` already accepts; a separate-process isolate is a later option. pi v1.0.1 growth: `PRELUDE_SOURCE` moved `prelude-source.ts:34` -> `:42`, and the prelude now enforces `MAX_OUTPUT_CHARS = 16 * 1024 * 1024` (`:36`) and `MAX_OUTPUT_ITEMS = 100_000` (`:37`) in `output()` (`:244-248`): a script past either limit throws a `RangeError` and `done()` has already reported the failure, so catching it does not resume output (`319fecb89`, `packages/codemode/CHANGELOG.md` [1.0.1], #10283). The sandbox port must carry these caps. Effort `L` left as filed; the engine choice makes it arguably lower, and that is not re-rated. **CORRECTED 2026-10-09:** the in-process isolate caveat above is no longer the shipped placement. Since `dbe7ee5f1` each execution's isolate runs in a sandbox process (`CODE-025`), so a V8 memory-safety bug or an allocation the heap cannot satisfy takes down that process, not cyrup; the in-process placement remains for tests and embedders. |
| ~~CODE-003~~ | ~~low~~ **CLOSED 2026-10-06 — shipped** | not-ported | S | **`parseCodemodeSource()` and `CODEMODE_SOURCE_GRAMMAR` are unported** — the `// @options:` first line and its Lark grammar, `src/source.ts:100`/`:22`. Engine-independent and landable today: cyrup's grammar-constrained sampling already exists. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** upstream cites hold at v1.0.0 (`source.ts` `:11`, `:22`, `:100`). `source.ts` has eight `throw new` sites at v1.0.0 (`:58`, `:64`, `:69`, `:74`, `:81`, `:87`, `:102`, `:112`), not "six"; two share a message, so the number of distinct rejection messages was not re-counted here - enumerate them from source when porting. `ConstrainedSamplingConfig` is at `crates/cyrup-core/src/constrained_sampling.rs:76` (the enum), not `:78`. |
| ~~CODE-004~~ | ~~low~~ **CLOSED 2026-10-06 — shipped** | not-ported | M | **The declaration renderer is unported** — `renderDeclarations`/`schemaToType`/`renderToolSample`/`renderToolOutputType`/`MCP_TYPESCRIPT_PREAMBLE`/`toCodemodeIdentifier`, `src/declarations.ts:105,228,153,181,18` and `src/identifier.ts:5`. Engine-independent. **FILED 2026-10-02**; body below. |
| ~~CODE-005~~ | ~~low~~ **CLOSED 2026-10-06 — exposure model, loadout and transcript restore shipped; residuals CODE-014, CODE-015, CODE-017** | not-ported | L | **The `ToolExposure` / `ToolNamespace` / `ToolLoadout` / `prepareLoadout` model is unported** — `core/extensions/types.ts:509,527,540,552`; five exposures, three of which exist for codemode and MCP. **Shared prerequisite with area 13 and with `TOOL-052` (`04-…`).** **FILED 2026-10-02**; body below. |
| ~~CODE-006~~ | ~~low~~ **CLOSED 2026-10-06 — shipped, both extension tiers; residual CODE-016** | not-ported | L | **Nested tool calls are unported** — `ctx.executeTool()` (`core/extensions/types.ts:394`), `NestedCallRecorder` and `NESTED_CALL_LIMITS` (`core/nested-tool-calls.ts:47`, `:26`), `nestedCalls` on the tool-result message, `parentToolCallId` on the events. **Shared prerequisite.** **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** the body's cite `13-cyrup-mcp.md:2081` for `EventKind::ToolCall::fails_closed()` is `13-cyrup-mcp.md:2148` (line 2081 is blank). |
| ~~CODE-007~~ | ~~low~~ **CLOSED 2026-10-06 — shipped** | not-ported | M | **The model-facing `codemode` description and its catalog budget are unported** — `createCodemodeDescription` and the round-robin `selectCatalog`, `extensions/codemode/tool.ts:237`/`:211`, plus the `codemode.mode` and `codemode.inlineBudget` settings. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-09:** 'shipped' overstated the grammar half. The tool declared `constrainedSampling`, but no adapter sent a custom tool until `PROV-101` closed on 2026-10-08 (`ad419d5b4`); see the 2026-10-09 record. |
| ~~CODE-008~~ | ~~low~~ **CLOSED 2026-10-06 — shipped; all five `models` functions backed** | not-ported | M | **The script `models` globals are unported** — `createModelGlobals` at `extensions/codemode/execute.ts:524`: `getModelsOfType`/`getAvailableOfType`/`getModelOfType`/`classify`/`generateImages`, a 4-call limiter, shape validation, and usage attribution. Depends on `PROV-102` and `PROV-105`. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** `CodemodeModelRuntime` is `packages/coding-agent/src/extensions/codemode/tool.ts:61-65`, not `:70-74` (same at v1.0.0 and v1.0.1). `execute.ts:524` (`createModelGlobals`) holds. |
| ~~CODE-009~~ | ~~low~~ **CLOSED 2026-10-06 — shipped** | not-ported | M | **The `codemode-store` session custom entry and its branch-scoped replay are unported** — `extensions/codemode/tool.ts:53` and `readCodemodeStore` at `execute.ts:220`: `store()`/`load()` survive resume and each branch sees only its own path's writes. **FILED 2026-10-02**; body below. |
| ~~CODE-010~~ | ~~low~~ **CLOSED 2026-10-06 — shipped (ranker, three script globals, `tool_search`)** | not-ported | M | **The script discovery globals and the BM25 ranker behind them are unported** — `createDiscoveryGlobals` at `execute.ts:451` (`searchTools`/`describeTool`/`describeNamespace`) over `Bm25Ranker` at `extensions/tool-search/tool.ts:119`. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** `isNamespaceName()` is `extensions/codemode/execute.ts:440`, not `:437` (same at v1.0.0 and v1.0.1). `createDiscoveryGlobals` `:451` holds. |
| ~~CODE-011~~ | ~~low~~ **CLOSED 2026-10-06 — shipped** | not-ported | M | **The codemode TUI renderer is unported** — `codemodeRenderers` at `extensions/codemode/renderer.ts:65`: a live nested-call list with per-call status glyph, duration and USD cost, over a collapsed syntax-highlighted script preview. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03 (v1.0.1 growth):** `renderer.ts` `codemodeRenderers` `:65` is unchanged at v1.0.1. v1.0.1 adds `pi.registerToolRenderer(resolver)` (`core/extensions/types.ts:1683`, `loader.ts:367`; `packages/coding-agent/CHANGELOG.md` [1.0.1], #10285) and the MCP extension uses it to draw `mcp__<server>__<tool>` calls before their server connects (`extensions/mcp/index.ts:363-366`). A cyrup port that wants parity for resumed sessions needs a renderer-resolver hook for tools that are not registered, in addition to the codemode renderer. |
| ~~CODE-012~~ | ~~low~~ **CLOSED 2026-10-06 — shipped** | not-ported | S | **The script output budget, truncation and temp-file spill are unported** — `DEFAULT_MAX_OUTPUT_TOKENS` 10 000, `truncateOutput` and `spillOutput` at `execute.ts:233`, `:277`, `:262`. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03 (v1.0.1 growth):** `execute.ts` `:233`, `:262`, `:277` still hold at v1.0.1. v1.0.1 adds a second, sandbox-level output cap beneath this budget: `MAX_OUTPUT_CHARS` 16 Mi and `MAX_OUTPUT_ITEMS` 100 000 in `packages/codemode/src/runtime/prelude-source.ts:36-37` (see `CODE-002`); a script exceeding either fails with a `RangeError` before `truncateOutput` sees the text. The two caps are independent. |
| ~~CODE-013~~ | ~~low~~ **CLOSED 2026-10-06 — no counterpart needed: V8 links into the binary** | tooling | M | **Shipping the sandbox runtime in cyrup's build has no counterpart** — `getQuickJSWasmPath()` / `resolveCodemodeWorkerSpecifier()` at `coding-agent/src/config.ts:488`, `:493` resolve a wasm asset and a worker entrypoint per release runtime. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** at v1.0.0 `getQuickJSWasmPath()` is `config.ts:488`, `setEmbeddedQuickJSWasmPath()` `:483` and `resolveCodemodeWorkerSpecifier()` `:493` (as cited); at v1.0.1 they are `:491`, `:486` and `:496`. `loadQuickJSWasm()` is `packages/codemode/src/wasm.ts:21`, not `:24` (same at both tags). The row's "if `CODE-002` lands on `rquickjs`, this row collapses" branch is moot: per `docs/adr/ADR-0031-one-javascript-engine-deno-core.md` a port would use `deno_core` (already linked, V8 fetched by `rusty_v8`'s build, `Cargo.toml:403-415`), so no wasm artifact is embedded and the open question is only how the V8 prebuilt is provisioned, which `workflowScript` already answers. "Nothing embeds a wasm artifact" for assets remains true. |
| ~~CODE-014~~ | ~~medium~~ **CLOSED 2026-10-06 — the prompt is written as `sections` diff rows and a pi-written file no longer carries it twice; both key orders pinned byte for byte** | not-ported | L | **System-prompt `sections` rows are unported, so a session file pi wrote carries its prompt twice and cyrup's `SystemMessage` key order differs from pi's loop** — `core/agent-session.ts:1689-1705` `_preparePromptAndToolLoadout`, `core/system-prompt.ts:121-230` `buildSystemPromptSections`/`diffSystemPromptSections`. The tool-declaration half shipped with `CODE-005`'s transcript restore; the prompt half did not: cyrup still sends the prompt through `Context::system_prompt` and writes no `sections`, so a pi-written file's `sections` rows replay in addition to cyrup's own prompt (pre-existing), and pi's loop writes `toolsAdded` after `timestamp` where cyrup writes `types.ts` declaration order (both load either way; a cyrup rewrite of a pi row reorders keys). **FILED 2026-10-06; CLOSED 2026-10-06** (closure record below); evidence is the `ACTIVESET` lane's measurement, `crates/cyrup-session/src/context.rs`, `crates/cyrup-agent/src/agent/run/declare.rs`. **CORRECTED 2026-10-09:** this closure shipped a regression. No provider request carried a system prompt, and the closure record's statement that `Context::system_prompt` is now always empty and the model reads the replay of the file is false for cyrup, whose adapters read only that field. Found on 2026-10-07 by running the real binary; fixed and pinned at the request by `CODE-024` (`1ea706526`, `637fc636a`). The `sections` rows, the two key orders and the differential tests of this closure stand. |
| ~~CODE-015~~ | ~~low~~ **CLOSED 2026-10-09 — a guest tool declares `prepare-loadout` (`cyrup:ext` 0.17) and the host calls the export when it is declared** | not-ported | M | **A WASM guest tool cannot supply `prepareLoadout`** — `ToolDefinition.prepareLoadout` (`types.ts:625-630`). `Tool::prepare_loadout` is synchronous because it runs inside the session's tool-state lock and a guest `setActiveTools` is synchronous by design (`HostServices::set_active_tools`); a guest export is an async wasmtime call, so `WasmTool` keeps the trait default. Native tools (the `codemode` tool) use hooks today. Needs either an async loadout resolution outside the lock or a cached per-guest answer; neither was built. **FILED 2026-10-06.** **CLOSED 2026-10-09** (`3adb8437d`, `EXT-116`): `tool-descriptor.prepare-loadout` and `events.prepare-loadout` (WIT 0.17); `WasmTool::prepare_loadout` (`cyrup-ext/src/host/live.rs`) passes the `{ declared, callable, registered }` rows and reads `{ descriptions, hiddenDeclarations }`; the SDK has `ToolExec::prepare_loadout` with `ToolLoadout` and `ToolLoadoutChanges`. **[CYRUP-DELTA]** `Tool::prepare_loadout` is synchronous under the session's tool-state lock and a guest export is not: the host drives the export on a scoped thread, and when the instance is busy (a running tool applying the loadout from inside its own call) it answers with the tool's last changes instead of waiting on a lock its caller holds, so an answer can lag one application behind; the synchronous hook also blocks the session thread for the guest call, bounded by the epoch deadline. Live (`-t loadout_demo,signal_probe,demo_echo,codemode`): the first request lists `[codemode, demo_echo, loadout_demo]` (`signal_probe` hidden) and `loadout_demo`'s description is 'Orchestrates other tools. Callable: demo_echo,signal_probe,loadout_demo.'. Tests: `cyrup-ext` `tests::guest_tool_surface` (the flag, the answer, the busy case) and `cyrup-it` `wasm_guest_tool_surface` (the hide and show directions through a real session and provider request), red-proven. |
| CODE-016 | low | not-ported | M | **The guest side of `ctx.executeTool` has four gaps** — a guest tool cannot pass its own abort `signal` (a suspended guest has no abort handle: the nested call is cancelled only with the calling tool), a guest's `onUpdate` for a nested call receives its partial results batched after the call settles (the same results reach `tool_execution_update` events live), the instance whose own tool makes a nested call does not receive that call's `tool_call`/`tool_result`/`tool_execution_*` events (its lock is held by the call that makes them: a guest that is both permission gate and caller does not gate its own nested calls; recorded `[CYRUP-DELTA]` in `world.wit`), and a guest tool calling a tool of its own extension is refused by name (`GuestReentry`) rather than run. Native tier has none of these. **FILED 2026-10-06.** **NOTE 2026-10-07 (EXT-109, the guest virtual-model router).** The world now has a SECOND async guest export the host calls per request, `events.route-model`, and it hits NONE of these four. (a) No `onUpdate`, so no batching gap: a route returns one answer and streams nothing. (b) The abort handle is not missing: the route carries its own `route-id` and `host-router.is-route-cancelled(route-id)` is the poll, in a `route_cancel` slot of its own — deliberately not `host-tool`'s, whose token `ToolCallBinding::drop` clears, so a route sharing it would be torn down by an unrelated tool call's unwind. (c) No event-delivery hole: routing emits no events into the guest. (d) The reentrancy case IS refused by name, as here — `GuestReentry::RouterOfBusyInstance` — and for the same reason (an instance runs one call at a time and the holder is suspended wasm), but the refusal lands on upstream's own documented contract for a failing router (*"If `route()` throws … the request ends with an error response"*, pi `docs/virtual-models.md` @v1.0.4) rather than on a behaviour upstream would have run. And CODE-015's hazard does NOT apply: `prepare_loadout` is synchronous because it runs under the session's tool-state lock (`dynamic_tools: Mutex<Option<Arc<Mutex<DynamicToolState>>>>`, `cyrup-session-svc/src/host_services.rs`, where that `Mutex` is `std::sync::Mutex` — `tokio::sync::Mutex` is separately aliased `AsyncMutex` in the same file — so a guard is `!Send` and cannot be held across an `.await` at all), whereas routing is reached from `PolicyHooks::prepare_request` and awaited bare at a turn boundary holding no lock. The one wait the epoch budget does not cover is the instance mutex itself, because every export arms `set_epoch_deadline` only AFTER taking it; `route_model` therefore wraps acquisition-plus-call in a `tokio::time::timeout`, which is the bound no existing export has. **NARROWED 2026-10-09 (not closed): one of four gaps is closed, three remain.** Closed (`3adb8437d`, `EXT-116`): a guest can pass a named `signal-id` and a `timeout-ms` (`host-tool.execute-options`; SDK `ExecuteToolOptions`), and a nested call is cancelled by a signal the guest already aborted and by the deadline. The signal id is read once at call start, so only an already-aborted signal and the host-side deadline can cancel a call in flight; pi's `AbortSignal.timeout` becomes a host-enforced deadline because a suspended guest has no timer and nothing else can abort its signals while it waits. `cyrup-it` `wasm_nested_tool_calls::a_nested_call_is_cancelled_by_a_signal_the_guest_already_aborted` and `..._when_its_deadline_passes`, red-proven; live, a nested `bash` `sleep 30` with `timeout_ms` 700 returned `Command aborted` and the process wall was 6.6 s against a 5.8 s no-op baseline. **Still open** (re-verified by the wasm lane against `live.rs` `execute_tool`, `in_flight_ancestor_of` and `invoke_with_parent`; `GuestReentry` re-checked by the ledger pass): (2) a guest's `onUpdate` for a nested call receives its partial results batched after the call settles; (3) the instance whose own tool makes a nested call does not receive that call's events, so a guest that is both permission gate and caller does not gate its own nested calls; (4) a guest tool calling a tool of its own extension is refused by name (`GuestReentry`). All three follow from the single-instance `Store`: the caller is suspended in the import holding the instance lock, so nothing can call back into it; lifting them needs component-model async or a reentrant instance, not a WIT change. `world.wit` and the guide say so. |
| ~~CODE-017~~ | ~~low~~ **CLOSED 2026-10-09 — `Tool::annotations`, `getAllTools` rows, MCP search-mode tools and guest descriptors carry the four hints** | not-ported | S | **`ToolDefinition.annotations` and `ToolInfo.annotations` are unported** — `types.ts:601`, `agent-session.ts:1473` (MCP tool annotations: `readOnlyHint`/`destructiveHint`/`idempotentHint`/`openWorldHint`, "permission extensions can use them to decide which calls to confirm"). cyrup's `Tool` has no annotations accessor, so `getAllTools` rows omit them and the MCP adapter cannot attach them to the `deferred` tools it registers. **FILED 2026-10-06** (from `CODE-005`, `MCP-604` and the guest-rows lanes). **CLOSED 2026-10-09** (`fa76720d2`, `3adb8437d`): `cyrup_core::ToolAnnotations` (the four optional hints, camelCase on the wire) and `Tool::annotations` (default `None`, delegated by the loadout's described-tool wrapper and the extension wrapper); the session's `ToolInfo` and the guest-facing `getAllTools` row carry `annotations` only for a tool that has them (`agent-session.ts:1489` @v1.0.4, as read by the lane); guest descriptors carry them over `tool-annotations`; `cyrup-mcp`'s `deferredToolFields` keeps the four boolean hints of the cached tool's annotations without `title` (`pi-mcp-adapter` `index.ts:441-455` @v5.0.0, read by the ledger pass), so a search-mode tool reports them through `Tool::annotations`. **Not attached:** the annotations of eager (non-lazy) MCP tools: in the adapter at v5.0.0 a registration carries `annotations` only through `deferredToolFields` (`index.ts:450`), and `direct-tools.ts:263` hands them to the approval call, which is `MCP-601`'s. Tests: `exposure::tests` (2: a described tool keeps its annotations; camelCase and only the hints that are set), `registration::tests::a_search_mode_tool_carries_the_boolean_hints_of_its_mcp_annotations`, `host_services::tests::all_tools_reports_the_whole_merged_registry_in_pis_toolinfo_shape`, `cyrup-it` `a_guest_tools_annotations_reach_the_session_tool_info`. Not exercised on a live MCP server. |
| ~~CODE-018~~ | ~~low~~ **CLOSED 2026-10-07 — built-ins frozen before a script runs; the prelude's report decoded strictly** | upstream-drift | M | **Built-ins are not frozen, so a script that patches one breaks its own run, and the sandbox reports it badly.** pi `b223082bb` (v1.0.4, #10444; `packages/codemode/src/runtime/prelude-source.ts` `lockdown()`, `runtime/host.ts` `BridgeError`) freezes every object reachable from the built-in globals before the script runs, makes built-in globals read-only, and turns the commonly overridden prototype members (`constructor`, `name`, `message`, `toString`, `toLocaleString`, `valueOf`, `toJSON`, `Object.prototype`'s) into accessors so an instance can still override them; `describeError` coerces `name`/`message` with `String()`. cyrup's `crates/cyrup-codemode-runtime/src/sandbox/prelude.js` freezes only its own `tools`, `allTools`, `console` and namespaces and defines its own globals non-writable (`define`); the built-ins stay writable. **Measured on the V8 sandbox, 2026-10-07** (`sandbox/tests/probe_v104.rs`, output kept with the closure): none of the seven intrinsics upstream's test lists is `Object.isFrozen`; `Error.prototype.name = "Patched"` succeeds where upstream throws a `TypeError`; upstream's own *ignores patches to built-ins* script does not return `[[2], '{"a":1}']`, it runs to the deadline (`Timeout`); `Array.prototype.toJSON = () => null` makes **every** script fail with `sandbox: The script's store writes could not be read: invalid type: null, expected a sequence`; `Object.prototype.toJSON = () => 5; throw new Error("boom")` reports a `script` error with an empty message and no name; `error.message = 42` reports message `""` where upstream reports `"42"`. **The host-crash half of the commit is not a gap here.** The bridge is typed ops and every decode is a `Result` (`protocol.rs` `script_error`, `store_writes`; `execution.rs` `settle`): nothing unwraps and a malformed payload ends the execution as a `sandbox` error, so the process does not crash and `execute()` settles. What differs is strictness: `script_error` accepts a missing `message`, `store_writes` accepts entries of length 0 or 3+, and an unreadable return value is a `script` `RangeError` (a recorded delta), not upstream's `sandbox` `Sandbox bridge broken: …`. Severity low: one isolate per execution, so only the script that patched is affected. **FILED 2026-10-07.** |
| ~~CODE-019~~ | ~~low~~ **CLOSED 2026-10-07 — `image()` output saved to files readable only by the user, path named before each image; the spill is private too** | upstream-drift | M | **`image()` output is not saved to a file, and cyrup's output spill is created with the process umask.** pi `d677d0ee7` (v1.0.3, #10310; `extensions/codemode/execute.ts` `saveImages`, `utils/output-files.ts`): every distinct image a script shows is written to `<tmpdir>/pi-codemode-<16 hex>.<png/jpg/gif/webp>` (mode `0o600`, `wx`), a text item `[Image saved to <path> (<mime>, <size>)]` goes before it, a failed write becomes `[Image (<mime>, <size>) could not be saved: <error>]` and never discards the result, an image shown twice is saved once, and the tool description's globals line gains *"`image()` also saves the image to a temp file and the result names its path."* cyrup: `cyrup-codemode/src/output.rs` `plan_truncation` passes `OutputItem::Image { .. }` through untouched, `cyrup-codemode-runtime/src/tool/execute.rs` attaches the images as given, and the description's globals line is v1.0.1's. The text spill that does exist, `spill_output` / `write_spill` (`output.rs`), opens `OpenOptions::new().write(true).create_new(true)` with no mode, i.e. `0o666` minus the umask (upstream wrote the same at v1.0.1 and tightened it to `0o600` here). Model-visible effect: a later turn cannot refer to an image a script generated, and the model has no other way to reach its bytes (scripts cannot write files). **FILED 2026-10-07.** |
| ~~CODE-020~~ | ~~low~~ **CLOSED 2026-10-07 — hidden tools out of the rules, the tool list and the skills hint; guidelines shown with codemode declarations** | upstream-drift | M | **Hidden tools still shape the system prompt, and codemode does not show a tool's guidelines with its declaration.** pi `c30840c2e` (v1.0.4, #10343): `BuildSystemPromptOptions.hiddenTools`; `declaredTools = selectedTools − hiddenTools` drives the tool list, `buildRules` and the skills reader (`read`/`bash` declared → named; a hidden one that is still selected → `indirect`, *"Load a skill's file when the task matches its description."*); `ToolLoadout.getPromptGuidelines(name)`; `toCodemodeDeclaration(tool, guidelines)` appends a tool's guideline bullets to its description in the codemode tool's sections, in `ALL_TOOLS` and in `describeTool()`. cyrup shipped v1.0.1's rule (`CODE-005`) and `CODE-014` carried it: `PromptRebuilder::rebuild` (`cyrup-session-svc/src/tools.rs`) blanks only a hidden tool's snippet and says *"Its guidelines stay"*, and passes the whole active set as `selected_tools`, which `rules_section` and the skills reader (`cyrup-session/src/prompt/builder.rs`) read; `LoadoutView` has no `prompt_guidelines`; `to_codemode_declaration(tool)` (`cyrup-codemode-runtime/src/tool/description.rs`) takes none; the `getSystemPromptOptions()` bag has no `hiddenTools`. **Measured** (`cyrup-session-svc` `tests/codemode.rs::the_guidelines_of_hidden_tools_move_from_the_rules_to_their_codemode_sections`, `only` mode, `read` hidden, red before the fix): the request's `<rules>` still says *"- Use read to examine files instead of cat or sed."* for a tool the model cannot call directly, and the codemode description does not carry that guideline. Wording only, so low. **FILED 2026-10-07.** |
| ~~CODE-021~~ | ~~low~~ **CLOSED 2026-10-09 — output items laid out as pi v1.1.0 does** (body below) | upstream-drift | M | **A codemode result does not separate its output items: several `text()` items run together and `console.*` lines are mixed in with them** — pi `eb326d265` (v1.1.0): `console.*` emits a `console` output item (`packages/codemode/src/runtime/prelude-source.ts:444`; `CodemodeOutputItem` gains `console?: true`, `src/types.ts:48`); `formatOutput` (`extensions/codemode/execute.ts:262`) prefixes each non-console text item with `==> text N/M <==` when there is more than one (the returned value counts as one) and groups all console lines into one trailing `<console_output>…</console_output>` item; `joinAdjacentText` (`:284`) joins adjacent text items with a newline before truncation and again after `saveImages` (`:511`, `:521`, `:526`); and the model-facing description says so (`extensions/codemode/tool.ts:145`). The script error now follows the formatted output. cyrup: `crates/cyrup-codemode-runtime/src/sandbox/prelude.js:344` sends `console.*` as `output("text", …)`, `cyrup_codemode::types::OutputItem` (`crates/cyrup-codemode/src/types.rs:71`) has no console marker, `tool/execute.rs:337-366` appends the value and the error to the raw items with no headers, no console block and no join, and `tool/description.rs:45` carries the v1.0.4 sentence. Model-visible: with two `text()` calls the model sees `ab`-style concatenation where pi shows two headed items. **FILED 2026-10-09** twice: by the pi v1.1.0 drift triage (#210; filed open, body below, merged there from its extensions and tools/codemode lanes) and, independently, by the area-04 structured-results pass, which closed it the same day; the two filings name the same commit and were merged under this id when that pass was rebased onto #210. The structured-results pass's own record: the cyrup test that had to diverge from upstream is `cyrup-session-svc` `tests::codemode::resolves_read_calls_to_text_for_text_files_and_to_image_blocks_that_image_shows` (comment in the test). **Fix:** port the four pieces together (prelude kind, `OutputItem` console marker through `sandbox/protocol.rs` and `execution.rs`, `format_output` + `join_adjacent_text` in `execute.rs` at pi's three call points, the description sentence), then make that test expect upstream's bytes and port the `agent-session-codemode.test.ts` and `sandbox.test.ts` cases `eb326d265` changed. — **CLOSED 2026-10-09** (review of the area-04 structured-results pass; re-read at v1.1.0). `console.*` sends `output("console", …)` (`prelude.js:344`, pi `prelude-source.ts:444`); `op_codemode_output` maps it to the new `OutputItem::Console` (`sandbox/isolate.rs:108`, pi `worker.ts:80-85`; `cyrup-codemode/src/types.rs:77`, pi `types.ts:48`), which `sandbox/protocol.rs` and `execution.rs` carry unchanged. `cyrup_codemode::output::format_output` (`output.rs:170`, pi `execute.ts:262-281`) and `join_adjacent_text` (`:205`, pi `:284-296`) are called where pi calls them: `format_output` over the script output plus the returned value, before the script error and the generated-images note are appended (`tool/execute.rs:361`, `:364`; pi `:502-512`), then `join_adjacent_text` before truncation (`:384`, pi `:521`) and after the image labels (`:395`, pi `:526`). The description sentence is pi's (`tool/description.rs:45`, pi `tool.ts:145`), and `docs/codemode.md` carries pi's two changed passages. Tests now expect upstream's bytes: `cyrup-session-svc` `tests::codemode::resolves_read_calls_to_text_for_text_files_and_to_image_blocks_that_image_shows` (the divergence comment is gone), `::runs_nested_calls_in_parallel_and_returns_only_the_script_result`, `::output_items_keep_their_order_and_each_image_follows_the_path_it_was_saved_to`, `::reports_script_failures_as_results_that_keep_partial_output_and_the_calls_that_ran`, `::truncates_output_to_the_token_budget_and_spills_the_full_text` (`max_output_tokens` 30, headed spill file) and `::generates_images_with_catalog_auth_and_attaches_them_through_image`; `cyrup-codemode-runtime` `tool::engine_tests::each_text_item_is_marked_and_console_lines_come_last_in_one_block` (pi's new case on real V8), `::a_throwing_script_keeps_its_output_and_names_the_calls_that_ran` (`codemode.js:4`), the matching `tool::execute::tests` cases, `sandbox::tests::port::script_execution::collects_text_image_and_console_output_in_order` (pi `sandbox.test.ts`'s `console: true`) and `sandbox::tests::runtime::output_items_keep_their_order_across_text_and_image`; `cyrup-codemode` `output::tests::several_text_items_get_numbered_headers_and_console_lines_go_last_in_one_block`, `::a_single_text_item_gets_no_header_and_images_keep_their_place`, `::join_adjacent_text_adds_a_newline_only_where_a_part_does_not_end_one`. |
| ~~CODE-022~~ | ~~low~~ **CLOSED 2026-10-09 — the lookup helpers are marked `await` in the description** (body below) | upstream-drift | S | **The codemode description lists `searchTools`, `describeTool` and `describeNamespace` without `await`, so a model can serialize the unawaited promise as `{}`** — pi `269121616` (v1.1.0, #10555, *"mark codemode lookup helpers as async in description"*): the third Globals line reads ``- `ALL_TOOLS`, `await searchTools(query, { limit?, namespace? })`, `await describeTool(name)`, `await describeNamespace(name)`: find unlisted tools, such as MCP tools.`` (`extensions/codemode/tool.ts:147` @v1.1.0), checked by `test/tool-search.test.ts`. cyrup's `tool/description.rs:47` had the v1.0.4 line, while the comment above `DESCRIPTION_INTRO` (`:36-38`) says everything but the engine word is verbatim. Found by the second review of the area-04 structured-results pass, on the line `CODE-021` edited; no row covered it on that pass's base. The pi v1.1.0 drift triage (#210) filed the same finding as `CODE-022` the same day (open, body below); the two were merged under this id when the pass was rebased onto #210. The port is correct for cyrup: its helpers are host globals whose `caller` (`sandbox/prelude.js:88-101`) returns a `Promise`, as pi's do. **FILED AND CLOSED 2026-10-09:** `tool/description.rs:47` carries pi's line. Verify: `cyrup-codemode-runtime` `tool::description::tests::the_globals_line_marks_the_lookup_helpers_as_async` (pi's three `toContain` checks). |
| CODE-023 | low | upstream-drift | M | **Codemode `models.classify()` does not accept `context.images`, and the classifier context has no images field to carry them, so a script's images are dropped silently** **Filed 2026-10-09 from the pi v1.1.0 drift triage**; body below. |
| ~~CODE-024~~ | ~~critical~~ **CLOSED 2026-10-09 — the request carries the prompt again (`request_context`); a body-level test per adapter and a wire suite guard it** | parity-bug | M | **The `CODE-014` closure shipped a regression: no provider request carried a system prompt.** — Found on 2026-10-07 by running the real binary against a scripted OpenAI-compatible server that saves each request body: request 0 and request 1 (after a tool call) each had exactly one system message and its content was the empty string (measured on the chat-completions route; the loop and the field below are shared by every adapter). `--system-prompt`, `--append-system-prompt`, `<tools>`, `<rules>`, the `AGENTS.md` project context and the `codemode` section never reached the model. Cause: `CODE-014` builds the session's `Agent` with no prompt of its own (pi `sdk.ts:389`) and carries the prompt as `sections` rows in the transcript, but every cyrup adapter reads only `Context::system_prompt` (they skip `Message::System`, `PROV-133`), and the loop built `Context { system_prompt: Some("") }`. `CODE-014`'s tests rendered the prompt from the transcript themselves (the `cyrup-session-svc` helpers `rendered_prompt` and `Seen.system_prompt` replayed the rows) and none looked at the provider request, so they proved the rows and never the request. **Fix** (`1ea706526`): `cyrup_provider::request_context` folds the shorthand prompt and every system row into `Context::system_prompt` (`get_current_system_prompt` over `normalize_context`) and drops the rows from the message list so nothing is sent twice; the loop (`cyrup-agent` `run/stream.rs`) builds its request through it, after the forced-prompt projection, so a `before_agent_start` replacement still wins and is still never persisted. The session-svc helpers now read the field the adapters read. **Measured on the real binary, before to after:** the system message was 0 characters, now 2647, starting `You are a coding assistant operating inside cyrup`, with `<tools>`, `<rules>` and the `codemode` section, on both requests; with `--system-prompt SENTINEL-PROMPT --append-system-prompt APPENDED-TEXT` 0 to 1135 characters, both strings in both requests (no `<tools>` or `<rules>` there: a custom prompt replaces them, as in pi's `buildSystemPromptSections`); with `--append-system-prompt` alone, `<tools>`, `<rules>` and the appended text. **Pinned by:** one serialized-body test per adapter (openai-completions, openai-responses, azure-openai-responses, anthropic-messages, google-generative-ai, bedrock-converse-stream, mistral-conversations, openai-codex-responses: `the_transcripts_prompt_reaches_the_request_body`; `google_vertex.rs` wraps the google builder and has none of its own), two `request_context` tests in `cyrup-provider` `utils::tests::transcript`, five agent-level context tests (`cyrup-agent` `tests::system_prompt`: replay on the first and post-tool-call request, a later diff row, an agent prompt leading the transcript's and sent once, none anywhere, a forced prompt not persisted), two through `SessionBuilder` (`system_prompt_flags`), and the request-level wire suite `cyrup-it` `codemode_wire` (`637fc636a`, 26 tests that read the bytes a provider receives; `the_first_request_opens_with_the_system_prompt_its_tools_and_its_rules` and `system_prompt_flags_reach_the_provider` fail when the `request_context` call is reverted). Red-proven by reverting the loop's `Context` construction and by breaking `request_context` three ways. Corrects the `CODE-014` closure record's statement that `Context::system_prompt` is now always empty and the model reads the replay of the file: false for cyrup as shipped on 2026-10-06. **[CYRUP-DELTA]** on `request_context`; the open part is `PROV-133`: adapters still ignore `Message::System`, so the transcript is collapsed to one leading prompt (what `collapseSystemMessages` yields) and a model with `supportsMidConvoSystemMessages` gets a later patched prompt at the head of the request instead of in place; tool declarations still travel only through `Context::tools`, so `resolveTranscriptTools` anchoring and Anthropic `tool_addition`/`tool_removal` stay unwired. The `before_agent_start` replacement is covered by session-svc and agent-level tests, not by a live check through the real binary. **FILED AND CLOSED 2026-10-09** (the codemode end-to-end repair; closure record in `18-pi-codemode.md`). |
| ~~CODE-025~~ | ~~high~~ **CLOSED 2026-10-09 — each script's isolate runs in a sandbox process of its own; one huge allocation fails the script, not cyrup** | parity-bug | L | **A script could abort the whole cyrup process with one allocation the heap cannot satisfy.** — Measured on the real binary before the change (scripted model, `timeout_ms` 8000): `new Array(2 ** 27).fill(0)` under the 256 MiB heap limit ended the process with SIGTRAP after 7.0 s (peak RSS 726 MB, no tool result, the session gone); `Array.from({ length: 2 ** 28 })` the same after 7.4 s. V8 aborts on one allocation the heap cannot satisfy and the near-heap-limit callback cannot save it; pi's QuickJS-in-wasm throws a catchable `InternalError: out of memory` instead. **Fix** (`dbe7ee5f1`; `docs/adr/ADR-0031` updated): in production each execution's isolate runs in `current_exe() __codemode-sandbox`, one process per script, over a framed JSON pipe that carries the messages the isolate thread already exchanged with the supervisor. The process has a cleared environment (`LD_LIBRARY_PATH`, `DYLD_LIBRARY_PATH` and `SYSTEMROOT` are passed on), no core file and an `RLIMIT_DATA` ceiling sized from the memory budget; a deadline, a cancel or `close()` kills it, which also stops a native built-in that ignores `terminate_execution`; V8's out-of-memory handler reports the abort over the pipe, so the script fails with the usual `InternalError: out of memory` and keeps its earlier output; a process that dies silently is a `sandbox` failure naming the signal. The hidden argv verb is a `SEAM-109`-class hop, classified by `predispatch`. **After:** rc 0, `InternalError: out of memory`, cyrup's own peak 130 MB (process tree 781 MB), 7.6 s, and a second `codemode` call in the same session returned `[2, 4, 6]`; the same for the `2 ** 28` array; `'x'.repeat(2 ** 29)` a `RangeError` (142 MB), `new Uint8Array(2 ** 31)` out-of-memory (203 MB), `for (;;) {}` with `timeout_ms` 3000 ended at 3.0 s. An idle sandbox process (debug build) has VmData 325 MB, VmRSS 78 MB and 13 threads; spawn to first result frame is 31-39 ms (this replaces the 8-10 ms per execution measured for `CODE-018`). **Tests:** `sandbox::tests::process` (13: a script and its tools and output cross the pipe; one allocation fails the script and the host carries on; output printed before the abort survives; a spinning script is killed at its deadline and reaped; `close()` kills a running process; the child lowers its own core and memory limits; an abort names the signal and a silent exit the status; a fatal out-of-memory report is the script's out-of-memory error; an unreadable message ends the script and the process; a process that ignores everything is killed at the deadline; an empty environment; a program that cannot start is a failed script), `sandbox::wire` (3), `predispatch` and `codemode_sandbox_cmd` hop classification, and `tests/codemode_sandbox_hop.rs` against the real binary. **[CYRUP-DELTA]** the in-process placement (`EngineSandboxFactory`, for tests and embedders) still aborts its host; a unit-test binary cannot be a sandbox host (re-executed with `__codemode-sandbox` it reads the token as a test filter and exits 0), so `session_launch.rs` uses the in-process factory under `#[cfg(test)]` and the production wiring is covered by the real-binary hop test and live runs. Residuals: `CODE-044`. **FILED AND CLOSED 2026-10-09** (the codemode end-to-end repair; closure record in `18-pi-codemode.md`). |
| ~~CODE-026~~ | ~~high~~ **CLOSED 2026-10-09 — the species-resolving typed-array and ArrayBuffer methods report what they allocate** | parity-bug | S | **A script could grow the process past its memory cap with no error by taking the typed-array species path.** — The prelude accounts ArrayBuffer memory because V8's heap limit does not cover it, but `slice`, `map`, `filter`, `ArrayBuffer#slice` and `Uint8Array.fromBase64`/`fromHex` resolve their result constructor through species, and after `Object.defineProperty(seed, "constructor", { value: undefined })` they reach the engine's intrinsic, past the prelude's guards. Measured before: a loop of `seed.slice()` over a 4 MiB buffer grew the process to 3610 MB in 3.3 s with no error and was killed by the harness at its 3.5 GB line. **Fix** (`dbe7ee5f1`): those methods are wrapped in the prelude and report what they return. **After:** `InternalError: out of memory at codemode.js:2:144`, process-tree peak 432 MB, 0.2 s, and the next call returned `[2, 4, 6]`; with the prelude fix removed (the `RLIMIT_DATA` backstop alone) a clean out-of-memory at 1185 MB. Pinned per method by `sandbox::tests::runtime::a_buffer_whose_constructor_is_gone_cannot_be_copied_past_the_limit` (slice, map, filter, `ArrayBuffer#slice`, `species = undefined`, `fromBase64`), red-proven method by method. Without `RLIMIT_DATA` (macOS, Windows) only the prelude fix bounds this: `CODE-044`. **FILED AND CLOSED 2026-10-09** (the codemode end-to-end repair; closure record in `18-pi-codemode.md`). |
| ~~CODE-027~~ | ~~medium~~ **CLOSED 2026-10-09 — the memory check is a regular op; `close()` gives up on a stuck isolate thread after 5 s** | parity-bug | S | **Two defects the review of the process-isolation work found: the memory-limit check ran a garbage collection as a fast op, and `close()` could hang a tool call.** — (a) `op_codemode_memory_exceeded` was a `#[op2(fast)]` op although it collects garbage inside the call; it is now `nofast`. **No behavioural test can tell the two apart** (`an_exceeded_limit_stays_exceeded_for_thousands_of_allocations_until_the_buffers_go`, 4000 refused allocations and then recovery, passes with `fast` too), so the pin is a test of the op declaration (`the_memory_check_is_a_regular_op_because_it_collects_garbage`), not of behaviour; the finding was true by reading. (b) `CodemodeSandbox::close()` waited for ever on an isolate thread stuck in a native built-in, which hung the tool call; it now gives up after 5 s and returns (`close_gives_up_on_a_thread_that_cannot_be_stopped_and_returns`, red-proven by removing the timeout); in the process placement the child is killed, so no thread is left. `dbe7ee5f1`. **FILED AND CLOSED 2026-10-09** (the codemode end-to-end repair; closure record in `18-pi-codemode.md`). |
| ~~CODE-028~~ | ~~high~~ **CLOSED 2026-10-09 — a script without `timeout_ms` is bounded: 120 s of its own running time and 30 min in all** | cyrup-original | M | **A script that set no `timeout_ms` had no limit at all.** — pi's tool passes `timeoutMs: sourceOptions.timeoutMs ?? Number.POSITIVE_INFINITY` (`extensions/codemode/execute.ts:427` @v1.0.4), and `host.ts:147` arms a timer only for a finite value, so upstream has no default deadline either; cyrup's `tool/execute.rs` passed `Deadline::Never`. Measured before on the real binary: `while (true) {}` with no options line ran until the harness killed it at 40 s. **Fix** (`53f6e8841`, [CYRUP-DELTA]): when `timeout_ms` is unset the script gets 120 s of its own running time (`DEFAULT_ACTIVE_LIMIT`: the isolate thread's time outside the wait for the host, kept by a watchdog in the sandbox process and reported over the pipe, so a script waiting on tools is not charged) and 30 min in all (`DEFAULT_TOOL_WALL_TIMEOUT`); an explicit `timeout_ms` replaces both; the timeout text says how to raise them with a ready-to-paste `// @options: {"timeout_ms": 3600000}` line. **After (live):** `while (true) {}` ended with `Script timed out` at 120.1 s (process wall 120.5 s, peak RSS 131 MB); `await tools.bash({ command: "sleep 2" })` ran 2.1 s and completed; at the final acceptance run `while (true) {}` ended at 120.4 s (rc 0, 213 MB tree peak) and `await tools.bash({ command: "sleep 3600" })` at 1800.4 s with `Execution timed out after 1800000 ms`, `bash (cancelled)` and no `sleep` left. **Tests:** `contract::a_spinning_script_stops_at_the_active_limit_long_before_the_deadline` (a 300 ms limit stops a spin in 0.33 s with a 60 s deadline), `time_spent_waiting_for_tool_calls_does_not_count_against_the_active_limit` (two 700 ms waits, 1.4 s, complete under a 400 ms limit), `without_an_active_limit_a_busy_script_runs_until_the_deadline`, and over the pipe `process::a_spinning_script_in_a_process_stops_at_the_active_limit_and_is_reported_as_a_timeout` (0.55 s). Not pinned by a committed test: that the tool really passes 30 min (asserted as `Deadline::After(DEFAULT_TOOL_WALL_TIMEOUT)`; the full duration was measured only live). The limit is wall time of the isolate thread while it is not parked on the host, not CPU time. **FILED AND CLOSED 2026-10-09** (the codemode end-to-end repair; closure record in `18-pi-codemode.md`). |
| ~~CODE-029~~ | ~~medium~~ **CLOSED 2026-10-09 — the options line is the first non-blank line, one surrounding fence is stripped, a later options line is an error** | cyrup-original | S | **A blank line before the options line, or a code fence round the script, silently dropped the options or failed the script far from the cause.** — pi `source.ts` `parseCodemodeSource` reads only the first line (`input.indexOf("\n")`, @v1.0.4, read by the ledger pass). Measured before on the real binary: a script whose `// @options: {"timeout_ms": 3000}` line followed a blank line ran unbounded (the options were ignored); a script wrapped in a markdown fence reached the engine as a tagged template and failed with `TypeError: "" is not a function at codemode.js:1:31`. **Fix** (`63cfd2ecc`, [CYRUP-DELTA]): the options line is the first non-blank line; one surrounding fence is stripped (the fence line becomes an empty line, so line numbers stay the model's); a fence that never closes, and an options line anywhere later, are explicit errors (`The // @options: line must be the first non-blank line of the script, but one is on line N`). The grammar and every other case of the upstream corpus are unchanged; the one corpus input that is now an error (`text(1)` followed by an options line) is asserted as such in `agrees_with_upstream_on_the_corpus`. **After (live and at the acceptance run):** the blank-line script ends at its timeout (also with CRLF, a BOM, inside a fence); the fenced script prints its result; a late options line fails the call naming line 2. Also checked, no change: `max_output_tokens: 0` is accepted by pi too (`isSafeInteger` is `>= 0`, `source.ts` @v1.0.4). **Tests:** `source::tests` (5: a comment that merely starts like the prefix is not an options line, the first non-blank line keeps line numbers, a late line names its line, one fence stripped, an unclosed fence refused) and `tool::engine_tests` (2, real V8). Known false positive: a script that writes a string literal holding a line that starts with `// @options:` now fails the call as a late options line. **FILED AND CLOSED 2026-10-09** (the codemode end-to-end repair; closure record in `18-pi-codemode.md`). |
| ~~CODE-030~~ | ~~medium~~ **CLOSED 2026-10-09 — wrapper scope, call frames, rendering, nesting depth and image size: five script-contract gaps** | cyrup-original | M | **Five gaps in what a script could rely on, each reproduced on the real binary.** — All [CYRUP-DELTA]s from pi `v1.0.4` (`53f6e8841`; `sandbox::tests::contract`, 14 tests; the V8-versus-QuickJS list in `sandbox/mod.rs` and `docs/codemode.md` is updated). (1) **Wrapper scope.** pi evaluates `(async (tools, console) => {...})` (`worker.ts:146` @v1.0.4), so here `const tools = await searchTools(...)` was `SyntaxError: Identifier 'tools' has already been declared`. The wrapper now has no parameters, `tools` and `console` are the prelude's globals, a script may declare either, and columns on line 1 are the script's own (negative column origin): `a_script_may_declare_tools_and_console_itself`, `positions_in_errors_are_those_of_the_script_as_written`. `text`, `image`, `store`, `load` and `exit` remain shadowable globals, as upstream (the tool description warns). (2) **A rejected call carried no script frame** (pi's `settle()` rejects with a bare `new ErrorCtor(payload)`, `prelude-source.ts:462`, unusable under `Promise.all`): now `Error: ENOENT: ...` plus `at codemode.js:1:17`, the position of the failing call (`a_rejected_call_carries_the_script_frame_that_made_it`). (3) **Rendering.** `text(err)` and a returned `Error`, `Map` or `Set` rendered `{}`; a returned `BigInt` failed the script (`TypeError: Do not know how to serialize a BigInt`); empty output was a bare `Output:`; `console.table/dir/group/assert/count/time` threw `is not a function`. Now `Error: boom` plus its frame, `[["a",1],["b",{"c":2}]]`, `5`, `(no output)`, and `console.table` prints a Markdown table; JSON values are byte-identical to before (`json_values_render_exactly_as_before`). The ported upstream test `reports_a_non_serializable_return_value_as_a_script_error` now uses a cyclic value, because a `BigInt` return is the intended delta. (4) **Depth.** A `store()` value or tool argument nested over serde_json's 128 levels ended the run as `Sandbox bridge broken` after tool calls had already run; over 64 levels it is now a catchable `RangeError` at the call, and the `bash` call made before it is still reported as made (64 leaves room for the session's store entry wrapper; a store entry at exactly the limit was not round-tripped through a real session file). (5) **`image()` had no size cap** (pi has none): over 5 MiB decoded it is a `RangeError` (exactly 5 MiB accepted, plus one byte refused); a 5.25 MiB PNG gave `image is 5.2 MiB, more than the limit of 5 MiB`. Live before and after for each is in the closure record. A script's returned value nested deeper than 128 levels is still a host-side `RangeError` script error (documented, unchanged). **FILED AND CLOSED 2026-10-09** (the codemode end-to-end repair; closure record in `18-pi-codemode.md`). |
| ~~CODE-031~~ | ~~critical~~ **CLOSED 2026-10-09 — nested calls are capped at 16 running, the live call list is bounded and rate-limited, and the transcript is shared** | parity-bug | L | **A script that made thousands of nested calls exhausted the host's memory.** — Measured before on the real binary (scripted model): 3000 parallel `tools.read` calls under `Promise.all` held the process at 3432 MB RSS and were killed by the harness at 16.4 s, unfinished; 3000 *sequential* awaited reads took 47 s in one run and 72.6 s and 3199 MB in another (the cost was quadratic, which the brief had not predicted). Three causes, three fixes. (1) **Every nested call ran at once** (`02241b4ba`, [CYRUP-DELTA]; upstream has no cap on tool calls, only `models.*` is capped at 4): at most 16 nested calls of one script run at once, the rest queue for a slot and are cancelled with the script; a queued call has no row until it gets one, and calls that wait on each other can deadlock past 16. (2) **Every begin and end re-cloned and re-serialised all rows to every subscriber** (O(n squared)): the recorder (`tool/recorder.rs`) publishes at most one snapshot per 100 ms with a guaranteed final one and keeps the latest 500 rows plus one `... N earlier calls` row ([CYRUP-DELTA]; pi publishes every row on every change); older rows are folded into a tally, the failure summary the model reads comes from the same capped list, and the persisted `nestedCalls` caps in `cyrup-core` are untouched. (3) **Every nested call deep-cloned the agent's transcript** (`460e21773`): `Agent::nested_context` hands out the system prompt and transcript as shared handles, copied once and handed out again until either changes (pi passes the live array by reference); the session's nested host had used a full `snapshot()` and re-wrapped every message for every call. **After:** 3000 parallel reads rc 0, 2.0 s, 138 MB peak, result 3000 (at the acceptance run 2.5 s and 226 MB, and 3000 parallel `grep` 4.7 s, 242 MB); 3000 sequential reads 7.8 s, 139 MB; a `--mode json` run of 40 sequential reads emitted 3 `tool_execution_update` events (1 running; 26 ok and running; 40 ok) and the final `tool_execution_end` carried all 40 calls, so a json consumer sees the final state. **Tests:** `tool::execute::tests` (the cap and queue, queued calls cancelled with the script, few snapshots with every call in the last, a summary of the oldest), `tool::recorder::tests` (9), `cyrup-agent` `tests/nested_context.rs` (4, pointer equality). Peak RSS in these numbers is the cyrup process only, or the process tree where the record says so. A partial result now shows a call only after it gets a slot, so with more than 16 queued calls the TUI shows at most the 16 running plus finished rows; `models.*` rows still begin before their 4-slot limiter, as upstream does, and the eviction path was not tested through `models.rs`. The TUI consumer was not run live. Residuals: `CODE-042`, `CODE-045`. **FILED AND CLOSED 2026-10-09** (the codemode end-to-end repair; closure record in `18-pi-codemode.md`). |
| ~~CODE-032~~ | ~~medium~~ **CLOSED 2026-10-09 — a late reply no longer makes the sandbox process exit before it has written the script's end** | parity-bug | S | **A script that returned with calls still in flight failed as `the sandbox process crashed ... exited with status 0`.** — Found while measuring `CODE-031`. In the sandbox process a late reply made the reader thread call `process::exit(0)` while the writer thread was still putting the script's `Done` frame on the pipe; measured at about 4000 unawaited `tools.read` calls. Dropping the reply instead lets `run_sandbox_process` end the process once the writer has drained (`27f29405d`). Test: `sandbox::tests::process::replies_that_arrive_after_the_script_ended_do_not_cost_it_its_result`. The concurrency lane noted this may be the same family as the unexplained 'fixture child exits zero having written nothing under saturated load' recorded in `00-residual-ledger.md` (the detach closure); it did not test that, and that child is a subagent child, not a sandbox process, so no claim is made. **FILED AND CLOSED 2026-10-09** (the codemode end-to-end repair; closure record in `18-pi-codemode.md`). |
| ~~CODE-033~~ | ~~medium~~ **CLOSED 2026-10-09 — at most 10000 calls may be started and unsettled per script** | cyrup-original | S | **A script could start calls faster than they settle without limit.** — Measured before: `for (;;) tools.read({ path: "a.txt" })` with `timeout_ms` 5000 reached 616 MB and 9.0 s; under the default limits 380 MB and 104.7 s (ended by the 120 s own-time limit, memory growing about 2 MB/s). **Fix** (`27f29405d`, [CYRUP-DELTA]; `sandbox/prelude.js`, `MAX_PENDING_CALLS`): a script may have at most 10000 calls started and not settled; the call past it throws a `RangeError` synchronously, so a loop that does not await ends at once (a rejection would have nobody awaiting it). **After:** 146 MB peak and 3.8 s under the default limits; at the acceptance run 9.1 s and 228 MB with `More than 10000 tool calls are in flight at once. ... do not start calls in a loop without awaiting them`, and with `timeout_ms` 5000 the loop ended at 5.5 s (228 MB) with the call list capped at the latest 500 rows. Tests: `contract::a_loop_that_starts_calls_without_awaiting_them_is_stopped_by_the_pending_limit`, `calls_that_settled_free_their_place_under_the_pending_limit`. The cap counts calls, not bytes: `CODE-042`. **FILED AND CLOSED 2026-10-09** (the codemode end-to-end repair; closure record in `18-pi-codemode.md`). |
| ~~CODE-034~~ | ~~medium~~ **CLOSED 2026-10-09 — tools whose names normalise to one identifier each get their own (`gh_search`, `gh_search_2`)** | cyrup-original | S | **Two tools whose names normalise to the same identifier: one became unreachable.** — pi's prelude keeps the first tool of a colliding pair and drops the second (`prelude-source.ts:213-219` @v1.0.4, as read by the tool-surface lane). `IdentifierTable` (`cyrup-codemode/src/identifier.rs`, `c262f5852`, [CYRUP-DELTA]) gives each its own identifier in `tools`, `ALL_TOOLS`, `searchTools`, `describeTool` and the description, and `tools["gh-search"]` still works. Live (acceptance run, stdio MCP fixture, `codemode.mode: only`): `fx_gh-search` and `fx_gh_search` list as `### fx_gh_search_2 (fx_gh-search)` and `### fx_gh_search`, and each calls the right tool; a third `gh.search` is dropped earlier, at MCP registration (`skipping duplicate direct tool`), not by codemode. Tests: `cyrup-codemode` `identifier` (5), `declarations` (1), `discovery` (1), `cyrup-codemode-runtime` `engine_tests` and the loadout collision test. Documented in `prelude.js`, `identifier.rs` and `docs/codemode.md`. **FILED AND CLOSED 2026-10-09** (the codemode end-to-end repair; closure record in `18-pi-codemode.md`). |
| ~~CODE-035~~ | ~~medium~~ **CLOSED 2026-10-09 — the catalog says how many tools the budget cut and lists built-ins first; `describeTool` carries the MCP result types** | cyrup-original | S | **The codemode description hid what its budget cut, and `describeTool` of a deferred MCP tool named result types it never defined.** — (`c262f5852`; `description.rs` `select_catalog`, `discovery.rs`.) The namespace-less catalog group (built-ins and tools without a namespace) rendered no 'not listed' marker, as upstream's does not, so a budget cut was invisible, and built-ins could be crowded out by MCP tools. Now the group counts what the budget cut (`N tools not listed; use ALL_TOOLS / searchTools`) and built-ins are placed first; display order is unchanged ([CYRUP-DELTA]). `describeTool` for a deferred MCP tool appends the `Shared MCP Types` block (it rendered `CallToolResult<...>` without defining it), and the line `MCP tools resolve to their CallToolResult` of upstream's `docs/codemode.md:44` @v1.0.4 is restored. Measured (snapshot, only mode, 40 MCP tools plus built-ins, `inlineBudget` 1500): `bash`, `edit`, `read`, `write` and 12 MCP tools listed, `30 tools not listed`; at the default budget all 40 and `2 tools not listed`; at the acceptance run with 150 MCP tools: the 7 built-ins first, 18 headings, `139 tools not listed`, and `ALL_TOOLS`, `searchTools`, `describeTool` and a call to the last tool all worked. Tests: `description/tests.rs` (6: catalog, heading and call-note), `discovery/tests.rs` (2), `cyrup-session-svc` `tests/codemode.rs`. **FILED AND CLOSED 2026-10-09** (the codemode end-to-end repair; closure record in `18-pi-codemode.md`). |
| ~~CODE-036~~ | ~~medium~~ **CLOSED 2026-10-09 — the script reference is embedded in the binary and written to `<agent dir>/docs/codemode.md`; pointers say `read` or `tools.read`** | cyrup-original | S | **The description and the `models` errors sent the model to a docs file an installed binary did not have, and the spill footer to a tool it might not have.** — pi points at `<package dir>/docs/codemode.md` (`CODEMODE_DOCS_PATH`), a file the npm package always contains. A cyrup binary installed on its own has no `docs/` directory (`cyrup_config::docs_dir` finds one only next to a source checkout or under `CYRUP_ASSET_DIR`), so the path was not there; and the spill footer said `read with offset/limit` where `codemode.mode: only` hides `read` (only `tools.read` exists). **Fix** (`c262f5852`, [CYRUP-DELTA]): `tool/docs.rs` embeds `docs/codemode.md` (`include_str!`) and `CodemodeExtension::with_agent_dir`, called for every session by `session_launch::attach_native_extensions`, writes it to `<agent dir>/docs/codemode.md`, replacing a page left by an older build, atomically; the footer reads `read or tools.read with offset/limit`. Live: the description's `models` line names that path and the file exists (20618 bytes then) and is byte-equal to `docs/codemode.md` after a rebuild. The page is also in the user guide (`7c5a6a1e4`; `the_guide_page_is_the_shipped_page_with_the_links_of_its_own_directory` and two more `tool::docs` tests). **Not done:** the system prompt's docs section still never renders in production, because `cyrup-session-svc/src/builder.rs` still passes `DocsPointers::default()` (`SESS-035`, unchanged; checked by the ledger pass at HEAD). `c262f5852`'s message says 'the system prompt docs section names the codemode page', which holds for the template (`docs_guidance` in `cyrup-session/src/prompt/builder.rs`) only. Under an armed permission policy the file is outside the project and the model's own `read` of it was gated: `PERM-039`. **FILED AND CLOSED 2026-10-09** (the codemode end-to-end repair; closure record in `18-pi-codemode.md`). |
| ~~CODE-037~~ | ~~low~~ **CLOSED 2026-10-09 — the description and schema name the `code` argument instead of claiming a raw channel** | cyrup-original | S | **The tool text said the input is raw JavaScript, not JSON, on routes where it is a JSON `{ code }` argument.** — pi's description says the input is raw JavaScript (not JSON, no code fence) and the schema says 'Raw JavaScript source.', which is true only on a grammar (custom tool) route. Until `PROV-101` no adapter sent the grammar, and on every function-tool route (every chat-completions model, and any model without `supportsOpenAIGrammarTools`) the model sends `{ "code": "<script>" }`. `ad419d5b4` rewords description and schema to name the `code` argument ([CYRUP-DELTA], `description.rs` and `tool/mod.rs`); the tool-declaration bytes differ from pi here, as the intro already did (V8 for QuickJS). Test: the description names the `code` argument and does not say 'not JSON'; the live fake-server run showed the function tool with schema `{ code: string }` and the new wording. **FILED AND CLOSED 2026-10-09** (the codemode end-to-end repair; closure record in `18-pi-codemode.md`). |
| ~~CODE-038~~ | ~~medium~~ **CLOSED 2026-10-09 — lone surrogates are replaced by U+FFFD at the bridge** | cyrup-original | S | **A string with a lone UTF-16 surrogate failed a tool call, the return value, a `store()` write or a thrown error, and blamed the script.** — A slice that cuts an emoji produces a lone surrogate; `JSON.stringify` writes it as an escape that serde_json refuses. The thrown-error and `store()` cases were reported as `Sandbox bridge broken ... modified built-ins`. **Fix** (`07698f0c0`, [CYRUP-DELTA]): the prelude writes U+FFFD for each lone surrogate in every JSON text it hands to the host, and in `store()` keys so `load()` finds them, skipping the text of an escape (`wellFormedJson`, `wellFormed` in `prelude.js`). pi's `JSON.parse` accepts the escape (`host.ts:58`, `:261`, `:278` as read by the fresh-eyes lane), so this is lossy where pi is not, like `text()`. Seven contract tests, red-proofed; documented in `sandbox/mod.rs` and `docs/codemode.md`. **FILED AND CLOSED 2026-10-09** (the codemode end-to-end repair; closure record in `18-pi-codemode.md`). |
| ~~CODE-039~~ | ~~medium~~ **CLOSED 2026-10-09 — the sandbox process starts from the running image (`/proc/self/exe`) on Linux** | cyrup-original | S | **A cyrup upgraded or reinstalled during a session broke every later script.** — Sandbox processes were started from the path `current_exe()` returned at startup, so after an upgrade the old file was gone, or a newer binary spoke another protocol. **Fix** (`07698f0c0`, [CYRUP-DELTA]): `HostCommand::running_executable` uses `/proc/self/exe` on Linux, the image the session runs; elsewhere the failures say to restart cyrup (a missing program, and an exit code before the process said anything). Tests with the binary removed and replaced, red-proofed. The non-Linux path is `cfg`'d and was not compiled or run: `CODE-044`. **FILED AND CLOSED 2026-10-09** (the codemode end-to-end repair; closure record in `18-pi-codemode.md`). |
| ~~CODE-040~~ | ~~low~~ **CLOSED 2026-10-09 — the result ends with a note naming the calls that were cancelled when the script ended** | cyrup-original | S | **A script that succeeded with tool calls still running read as a plain success.** — `main();` without `await`, or `items.forEach(async ...)`, finished with `(no output)` while the writes landed nondeterministically afterwards. pi marks them cancelled in the details only (`execute.ts:442` @v1.0.4). **Fix** (`07698f0c0`, [CYRUP-DELTA]): the result ends with a note naming the cancelled calls (`format_unfinished_calls_note`; `engine_tests`, red-proofed). **FILED AND CLOSED 2026-10-09** (the codemode end-to-end repair; closure record in `18-pi-codemode.md`). |
| ~~CODE-041~~ | ~~medium~~ **CLOSED 2026-10-09 — the JSON of a script's return value counts against the 16M-character output limit** | cyrup-original | S | **A script's return value bypassed the output cap and cost the host about thirty times its size.** — pi's `outputChars` is incremented in `output()` only (`prelude-source.ts:335-344` @v1.0.4: text, image, console), so a `return` is not counted upstream either; the host parses the whole text into a `serde_json::Value` and `value_text()` renders it again. Measured with `return Array.from({ length: 3e6 }, (_, i) => ({ a: i }))` (40888891 characters of JSON): 17.0 s and a 1722 MB process-tree peak before, 2.2 s and 509 MB after, now a `RangeError` that says to return a summary or write the data to a file (`7abba7cef`, [CYRUP-DELTA]; same prefix as the output limit's error). A return just under the limit is still delivered, at a host cost the row `CODE-043` records. **FILED AND CLOSED 2026-10-09** (the codemode end-to-end repair; closure record in `18-pi-codemode.md`). |
| ~~CODE-042~~ | ~~low~~ **CLOSED 2026-10-10 — the arguments of started-and-unsettled nested calls are weighed against a 32 Mi limit, and the call that passes it throws a `RangeError`** | cyrup-original | S | **Argument bytes held by pending or queued nested calls are not bounded.** — `CODE-031`'s cap of 16 running and `CODE-033`'s 10000 in flight count calls, not bytes: a script can queue thousands of calls with large arguments, each held host-side until it runs, and only the script's own 256 MB heap limit bounds that, indirectly. By reading `sandbox/prelude.js` and `tool/execute.rs`; not measured. **Fix** -- count the argument bytes of started-and-unsettled calls against a limit and throw a `RangeError` at the call, like the depth and call-count limits. **Verify** -- a loop that starts 5000 calls with 1 MiB arguments ends with a `RangeError` and a bounded peak. **FILED 2026-10-09** (the codemode end-to-end repair; closure record in `18-pi-codemode.md`). **CLOSED 2026-10-10** (`1114f485a`, [CYRUP-DELTA]; sandbox-residuals lane of the follow-up). **Measured on the real binary (debug build) before the change:** a loop of 5000 `tools.read` calls with a 1 MiB pad: host peak 1055 MB, process tree 2098 MB, 82.0 s, the sandbox process killed by SIGTRAP and the script failed with 'sandbox process crashed'. The row's premise (bytes of queued calls) was the smaller half: **two parallel calls with a 1 000 000-element array of `{ a: n }` as argument (12.9M characters each) took the host to 3236 MB, the tree to 3406 MB, in 31.5 s**, because a small value costs the host far more than its characters (2M integers +654 MB, 1M short strings +447 MB, 250k small objects +589 MB, one 30M-character string +204 MB, measured on a characters-only build). **Fix:** `sandbox/prelude.js` weighs the JSON of each call's arguments (a character 1, a comma `ARGUMENT_COMMA_WEIGHT` 48, an array or object `ARGUMENT_CONTAINER_WEIGHT` 256; the limit is `MAX_PENDING_ARGUMENT_WEIGHT` = 32 Mi, all in `sandbox/mod.rs`, with the derivation in their doc comments) and the call that would take the unsettled calls past it throws a `RangeError` at the call site, like the 10000-call limit; a settled call frees its weight. **After:** the 5000-call loop ends with a `RangeError` after 31 calls, rc 0, 3.2 s, host peak 178-184 MB, tree 279-282 MB (without a `catch` it ends the script at once: 3.0 s, 183 MB); the two object arrays are refused at the first call, 137 MB, 1.0 s; one legitimate 30M-character string argument is still allowed (host peak 339-350 MB against an idle codemode host of 133-135 MB); 16 parallel 2 MiB strings, 185 MB. Tests, all red-proven by the lane: six in `sandbox::tests::contract` (`a_loop_of_calls_with_large_arguments_is_stopped_by_the_argument_limit`, `an_unawaited_loop_of_large_calls_ends_the_script_at_the_argument_limit`, `calls_that_settled_free_their_arguments_under_the_argument_limit`, `a_call_whose_arguments_alone_pass_the_limit_is_refused_and_can_be_caught`, `arguments_made_of_many_small_values_weigh_more_than_their_characters`, `a_large_array_of_small_objects_is_refused_where_a_string_of_its_length_is_not`) and the real-binary `codemode_wire::a_loop_of_calls_with_large_arguments_ends_at_the_argument_limit`. **[CYRUP-DELTA]** a script that legitimately keeps more than about 30 MB of arguments in flight must now await in batches; the limit is deliberately conservative (a limit's worth costs the host 200-260 MB) and the `RangeError` says how to batch; `docs/codemode.md` and the guide copy list it. The weights are calibrated on the pipeline as it is and not derived: see `CODE-053`; unawaited results are not bounded: see `CODE-054`. |
| ~~CODE-043~~ | ~~low~~ **CLOSED 2026-10-10 — a returned value stays the text the script produced; the host no longer rebuilds it as a `serde_json::Value`** | cyrup-original | M | **A returned value under the output limit still round-trips through `serde_json::Value` on the host, at about 35 times its text.** — `CODE-041` closed the bypass, not the amplification. Measured by the fresh-eyes-3 lane (debug build) with a 16388891-character array of small objects: parsing it into a `Value` held about 560-580 MB for 1.9 s and rendering it again about 100 MB transient for 2.6 s; the run took 7.4 s with an 800 MB process tree. **Fix** -- keep the return value as text (`serde_json`'s `RawValue` is already enabled workspace-wide) in `CodemodeResult::Completed.value`, whose type is public and so a breaking change, and check the documented 128-level depth `RangeError` separately; or count the parsed size against the limit. **Verify** -- the 16.4 MB return above stays under 200 MB. The `/tmp/pi-codemode-*.txt` spill files are still never removed by cyrup (unchanged); a returned value can no longer make one larger than about 16M characters. **FILED 2026-10-09** (the codemode end-to-end repair; closure record in `18-pi-codemode.md`). **CLOSED 2026-10-10** (`1114f485a`, [CYRUP-DELTA]; sandbox-residuals lane of the follow-up). `CodemodeResult::Completed.value` is now `types::ReturnValue` (a validated `RawValue`, so a breaking change to a public type; every in-workspace consumer is updated and `testkit::completed` keeps its `Option<Value>` signature) and `execute_codemode` prints it with `into_text`, a string unescaped and anything else as written. **Measured (the lane, debug build), a return of `Array.from({length: 1250000}, (_, i) => ({ a: i }))` (16 388 891 characters):** before, host peak 830 MB in 7.0 s; after, host peak 209-214 MB in 2.8-3.0 s, process tree 348-387 MB. The new probe test measures the host side alone in a child test process (Linux only): growth of 49 MiB shipped against 621 MiB with the tree rebuilt, threshold 300 MiB. A 15 MiB string (222 MB before, 216 MB after) and 1000 strings of 15 000 characters (218 MB, 208 MB) were already text-shaped and are unchanged; an idle codemode host peaks at 133-135 MB. **Against the row's own verify (`stays under 200 MB`):** the absolute host peak of 209-214 MB is 9-14 MB above that figure; the amplification the row named (about 35 times the text, 560-580 MB held, 800 MB tree) is gone, and the growth over an idle host is about 75-80 MB. The row is closed on the amplification, and this sentence is the record that its literal figure was not met. **Tests:** `sandbox::tests::contract::a_returned_value_prints_from_its_text_as_it_printed_from_a_tree` (a corpus of 40 values; the printed bytes equal the old tree path), `sandbox::tests::process::a_large_returned_value_costs_the_host_a_few_copies_of_its_text`, return-value cases added to `sandbox::protocol::tests::a_malformed_payload_is_refused_with_the_reason_upstream_names`, `sandbox::execution::tests::a_settlement_that_does_not_decode_is_a_sandbox_error_naming_the_reason` and `tool::execute::tests::a_returned_value_is_appended_like_text`, and the real-binary `codemode_wire::a_returned_array_of_a_million_small_objects_is_shown_as_the_text_it_was`. **Side effect, also [CYRUP-DELTA] and toward parity:** a returned value nested past 128 levels is no longer a `RangeError` (`RawValue` validation does not recurse and upstream's `JSON.parse` has no such bound; `sandbox::tests::runtime::a_result_nested_deeper_than_serde_json_reads_is_returned_as_it_is` replaces the 128-level test), and text that is not one JSON value is a sandbox 'Sandbox bridge broken: return value is not valid JSON' error, as pi's `parseBridgeJson`. |
| CODE-044 | low | port-divergence | M | **The sandbox process's limits are unverified off Linux, and some of its failures are less specific than the in-process ones.** — (1) `RLIMIT_DATA` is not enforced on macOS and does not exist on Windows (the `nix` dependency is `cfg(unix)`-gated and the code has non-unix stubs); there the process is bounded only by V8's heap limit and the prelude guards, so the backstop that `CODE-026` relies on is missing. None of the sandbox-process code, the `/proc/self/exe` start (`CODE-039`) or the `ps` descendant fallback (`SUBA-216`) was compiled or run on macOS or Windows, and the acceptance run was Linux only. (2) The sandbox process has no seccomp, namespace, chroot or user change (a case-insensitive grep for `seccomp|unshare|chroot|setuid|landlock|pivot_root` over `sandbox/` finds nothing; `child.rs` sets `RLIMIT_CORE` and `RLIMIT_DATA` only, best effort); it relies on a cleared environment, no core file and `RLIMIT_DATA`. (3) V8 takes 6-8 s to fail a 1 GB allocation, so a script whose `timeout_ms` is shorter gets a timeout error instead of out-of-memory (both are clean failures). (4) `return "y".repeat(2 ** 28)` ends as `Script sandbox failed: The sandbox process crashed ... signal 6 (SIGABRT)` (acceptance run) rather than a specific error; the session survives and the next call works. (5) The in-process placement still aborts its host (`CODE-025`). (6) The child is the cyrup binary, so its cost is a tokio runtime with one thread per core. **Fix** -- build and run the process tests on macOS and Windows runners, add the missing `RLIMIT_AS` or job-object equivalent, and map a silent abort over a large returned string to the out-of-memory error. **Verify** -- `new Array(2 ** 27).fill(0)` fails the script, not the session, on all three platforms. **FILED 2026-10-09** (the codemode end-to-end repair; closure record in `18-pi-codemode.md`). **NOTE 2026-10-10 (the row stays open; nothing in the follow-up could verify it).** No lane compiled or ran anything on macOS or Windows. Added by the follow-up: (a) the argument-weight limit of `CODE-042` lives in the prelude, so it bounds the host and the pipe traffic on every platform, whatever `RLIMIT_DATA` does; (b) the descendant kill that `SUBA-217` extended to the timeout and drop paths uses the same `ps` fallback and non-Linux tree kill that item (1) says were never compiled off Linux; (c) under a limited address space (`ulimit -v`) the sandbox process needs more than 32 GiB of address space for V8's reservation: a script failed with SIGTRAP at 16, 20, 24 and 32 GiB and ran at 40 GiB and up (follow-up lane 1, with the host starting at every limit since `EXT-117`), and `9fe06661c` makes that crash say so ('The address space of cyrup is limited to N GiB (`ulimit -v`), and the script sandbox needs more than 32 GiB of it: raise the limit to run scripts.') without making such a script run; (d) item (4), the 256M-character return that ends as SIGABRT, was seen again by the final live acceptance run (rc 0, 473 MB peak, the session intact). |
| ~~CODE-045~~ | ~~low~~ **CLOSED 2026-10-10 — a `cyrup-session-svc` test fails when the nested host stops sharing the agent's transcript** | test-defect | S | **No test fails if the session's nested host stops using `Agent::nested_context`.** — `460e21773` made `cyrup-session-svc/src/session/nested.rs` call `agent.nested_context()` so nested calls share one transcript copy; the pointer-equality test (`cyrup-agent` `tests/nested_context.rs`) pins the primitive only, and reverting the session wiring to a per-call `snapshot()` passes every test (reported by the concurrency lane; not re-run by the ledger pass). **Fix** -- a `cyrup-session-svc` test that makes two nested calls against an unchanged transcript and asserts they hold the same allocation, or counts the snapshots taken. **Verify** -- reverting `nested.rs` to `snapshot()` fails it. **FILED 2026-10-09** (the codemode end-to-end repair; closure record in `18-pi-codemode.md`). **CLOSED 2026-10-10** (`51e73ae0e`; sandbox-residuals lane of the follow-up): `cyrup-session-svc` `src/tests/nested_transcript_sharing.rs::nested_calls_made_against_one_transcript_are_shown_the_same_allocation_of_it` makes two concurrent nested calls through a capturing hook (a `cfg(test)` seam, `execute_nested_tool_through`) and asserts that both were handed the same transcript allocation. Red-proofed by the lane: reverting `session/nested.rs` to a `snapshot()` per call fails it with two different addresses; the shipped code passes. The system-prompt half is not pinned in `cyrup-session-svc`, because the agent's prompt is empty there (pi builds it with `systemPrompt ''`); it stays pinned at the primitive in `cyrup-agent` `tests/nested_context.rs`. |
| ~~CODE-046~~ | ~~medium~~ **CLOSED 2026-10-10 — an error the script never looked at is named in its result** | cyrup-original | M | **A tool call the script forgot to await, and that failed, is invisible: the result reads as plain success.** — Found by the fresh-eyes review, round 2. `tools.write({...}); text('saved')` with no `await`, an `async` function that threw and was not awaited, a `Promise.reject(...)` nothing handled: the script ended as 'Script completed' and the model was told a write or an edit had happened. pi v1.0.4 has no handling of unhandled rejections either (`git -C tmp/pi grep -i unhandled v1.0.4 -- packages/codemode` is empty, the lane's search), so this is cyrup's own behaviour. **Fix** (`3dbce52aa`, [CYRUP-DELTA]): the prelude keeps the rejections the engine reports as unhandled (`setUnhandledPromiseRejectionHandler`, dropping those a handler reaches later with `setHandledPromiseRejectionHandler`) and drains them at the script's end (`op_drain_pending_rejections`); the host remembers which calls failed that the isolate never heard of because the script ended first (`sandbox/execution.rs`, `UnobservedErrors`); the result carries 'Note: N errors were never handled and did not fail the script: ...'. A script that ran to its end is settled after the event loop has run once, bounded at one second: a script that returned and left a microtask loop spinning is reported as it stood at the return (without the bound it hung until the script's time limit), and what a leftover continuation starts or prints after the return is still ignored, as before. Real-binary before and after and the red proofs are the lane's (scratchpad `wf/fresh-eyes-2-fix2/red`, not in the repository). **A process finding:** the frame the sandbox sends at the end of a script gained a `report` field, and `cyrup` `tests/codemode_sandbox_hop.rs::the_hop_runs_a_script_and_answers_on_stdout_alone` failed at the branch head from `3dbce52aa` until `32b2b06fe`, because the lane that changed the frame did not run that crate's tests. The first version over-reported and then under-reported: `CODE-048`, `CODE-049`; its limits: `CODE-052`. **FILED AND CLOSED 2026-10-10** (the codemode follow-up; closure record in `18-pi-codemode.md`, *Follow-up, 2026-10-10*). |
| ~~CODE-047~~ | ~~medium~~ **CLOSED 2026-10-10 — an error the script never looked at is named in its result (the failure lands while the script runs)** | cyrup-original | M | **A tool call a script starts without awaiting, and that fails before the script ends, is invisible to the model.** — The second finding of the fresh-eyes review, round 2, fixed by the same change as `CODE-046` (`3dbce52aa`). The lane records that the two sets of failures are disjoint by construction: the prelude's half covers rejections delivered while the script runs, the host's half covers calls that fail while the script is still on the line that made them. Evidence, delta and limits as `CODE-046`. **FILED AND CLOSED 2026-10-10** (the codemode follow-up; closure record in `18-pi-codemode.md`, *Follow-up, 2026-10-10*). |
| ~~CODE-048~~ | ~~medium~~ **CLOSED 2026-10-10 — a failed call something was waiting on is not an unhandled error** | cyrup-original | S | **A false 'errors were never handled ... await every call' note when a script ends while sibling tool calls are still in flight and then fail.** — Found by the fresh-eyes review, round 3, in the fix for `CODE-046`: `try { await Promise.all([tools.read(a), tools.read(b), tools.read(c)]) } catch (e) { ... }` and `tools.read(a).catch(() => {}); text('done')` returned 'Note: N errors were never handled and did not fail the script ... A tool call or async function that is not awaited loses its error' for the calls `Promise.all` had already given up on and for the call that had a handler: the note named every call that failed after the script ended (the isolate had not been told of the failure, the host had), whether or not the script had attached a handler. **Fix** (`29effddd8`; the failing hop-frame test it left in `cyrup` was repaired by `32b2b06fe`): the isolate says which of the calls it left unsettled had nothing waiting on them, asked of the engine (V8 `Promise::HasHandler`, a new fast op `op_codemode_promise_handled`; `await` on a native promise never calls `then`, so the prelude cannot tell), and only those are named. After the change both reproductions and a `Promise.all(...)` kept in a variable with a `.catch` return no note and `tools.read(missing); text(..)` still returns it; the process-isolated sandbox behaves the same (a second test). [CYRUP-DELTA]: pi has no such note. The change removed too much: `CODE-049`. **FILED AND CLOSED 2026-10-10** (the codemode follow-up; closure record in `18-pi-codemode.md`, *Follow-up, 2026-10-10*). |
| ~~CODE-049~~ | ~~medium~~ **CLOSED 2026-10-10 — a call that failed after the script ended while something waited on it is named, without advice** | cyrup-original | S | **The fix for `CODE-048` dropped every failed call that had a reaction attached, so a forgotten `await` inside an `async` function, a `forEach(async ...)` callback or a `.then(f)` with no `.catch` ended as 'Script completed' again.** — Found by the follow-up-3 lane, in the round-3 fix of `CODE-048` (the round's own fix verdict had been a pass). Measured through the real binary before `36fff979e`: `async function main() { await tools.read({path:"n1"}) } main(); text("end")`, `[1,2].forEach(async (n) => { await tools.read({path:"n"+n}) }); text("end")` and `tools.read({path:"zz"}).then(x => x); text("end")` all returned the script's output and no note. **Fix** (`36fff979e`, [CYRUP-DELTA]): the isolate reports the calls it left unsettled in two lists, those nothing was waiting on (unchanged: the 'never handled' note with its advice is right for them) and those something was waiting on (new field `waited`); the engine cannot tell whether the script was done with the second kind (a `Promise.all` sibling it caught and a `main()` it forgot to await look the same from outside), so the host names the ones that failed in a note that claims neither: 'Note: 2 calls failed after the script ended, so the script did not see the errors: read: ...; read: ...' (`tool/execute.rs` `format_late_failures_note`). Through the real binary the three shapes above return it, `tools.read(missing); text(..)` keeps the 'never handled' note and the `Promise.all` in `try/catch` case returns the new note and no advice. Each change is pinned by sandbox runtime and process tests, the engine note-text test and the hop frame test, red-proofed by the lane. The old 'known limit' paragraph in `docs/codemode.md` (a failure inside an unawaited async function is named only if it reaches the script before it ends) is removed because it is no longer true. Limits: `CODE-052`. **FILED AND CLOSED 2026-10-10** (the codemode follow-up; closure record in `18-pi-codemode.md`, *Follow-up, 2026-10-10*). |
| ~~CODE-050~~ | ~~low~~ **CLOSED 2026-10-10 — the docs name the `cyrup-bash-*.log` files a script's nested `bash` calls leave** | cyrup-original | S | **A script's nested `bash` calls leave unreferenced, never-removed `cyrup-bash-*.log` files in the temp dir, and the docs said a script leaves only two kinds of file.** — Found by the fresh-eyes review, round 4. Reproduced by the lane: `tools.bash({command:"seq 1 5000"})` resolved to `{truncated: false}` with no `full_output_path` and left a 23 893-byte, mode 0600 `cyrup-bash-<id>.log`; only a result that is itself truncated (`seq 1 400000`) names its file, because the spill threshold of the `bash` tool (2000 lines or 50 KiB) is lower than the 1 MiB a script receives. **This is the `bash` tool's own behaviour and pi's:** pi v1.0.4 `bash.ts` builds the same `OutputAccumulator` with the `pi-bash` prefix for the same call (spill past 2000 lines or 50 KiB; read by the lane), so the files are documented, not suppressed (`20214f417`): `docs/codemode.md`, its guide copy and `docs/guide/guides/scripting.md` now name them in 'Files a script leaves behind', in 'Temp files pile up' and in the clean-up advice. cyrup still never removes any spill file (unchanged). **FILED AND CLOSED 2026-10-10** (the codemode follow-up; closure record in `18-pi-codemode.md`, *Follow-up, 2026-10-10*). |
| ~~CODE-051~~ | ~~medium~~ **CLOSED 2026-10-10 — an ACP tool result's text blocks are lines, and its images are shown** | port-divergence | S | **In an ACP editor (Zed) a `codemode` result's output is glued together and an `image()` is not shown.** — Found by the fresh-eyes review, round 3. `console.log('first line'); console.log('second line'); console.log({a:1}); return [1,2]` showed as `Output:\nfirst linesecond line{"a":1}[1,2]`, and an `image()` existed only inside `rawOutput`, which no client draws; `session/load` replay showed the same glued text. pi-acp joins a result's text blocks with `''` (`toolResultToText`, `texts.join('')` @v0.0.33), which works only because pi's own tools return one block; `codemode` emits one block per `text()` or console call and an MCP result has one per item. The model and the terminal UI see the blocks on separate lines. **Fix** (`de2b5d79b`, [CYRUP-DELTA]; `crates/cyrup-acp/src/translate.rs`, `sessions.rs`): the generic result text joins the non-empty text blocks with a newline (upstream's `.filter(Boolean)` is kept) and the result's image blocks are sent as ACP image content after the text; an image-only result shows the image, not the pretty-JSON fallback that would print its base64; live and replayed results share one function, and a terminal's output (`bash`) stays a plain concatenation (the chunks of one process are one stream). **Through the real binary over ACP** the same script ends 'Output:\n\nfirst line\nsecond line\n{"a":1}\n[1,2]', and `text('before'); image(png); text('after'); return 'ret'` yields the text with the saved-file label on its own line plus an `image/png` content block. **Not measured against Zed itself.** Documented in `docs/codemode.md` and the guide copy. Residual: `CODE-055`. Area 15 is a port plan and not a status table, so the row lives here. **FILED AND CLOSED 2026-10-10** (the codemode follow-up; closure record in `18-pi-codemode.md`, *Follow-up, 2026-10-10*). |
| CODE-052 | low | cyrup-original | M | **Limits of the report on errors a script never looked at.** — Residuals of `CODE-046`…`CODE-049`, from the follow-up-2 and follow-up-3 lanes. (a) The late-failure note is deliberately neutral, and trades noise for the safe default: a script that caught a `Promise.all` failure, or put a `.catch(() => {})` on a call, gets one extra line when other calls fail after it ended. No heuristic was tried: intercepting `then` cannot tell `.then(f)` from `.catch(f)`, and `.finally(f)` would hide a lost error; a cleaner split needs the engine to expose a promise's reaction chain. (b) Only calls that failed in the host before the script's `Done` is processed are named; a call still in flight at the end keeps the older 'still running ... cancelled' note. (c) The notes are text only: no TUI row marker (the call row already shows the error status). (d) On the `exit()` path the report is made at the call, without the event-loop turn, so a rejection handled later in the same turn could be reported. (e) A leftover continuation that outlives the return is cut at one second and its calls and output after the return are ignored. **Fix** -- an engine-side reaction-chain query for (a); (b) to (e) are accepted limits unless a user trips on them. **Verify** -- a caught `Promise.all` failure with failing siblings returns no note. **FILED 2026-10-10** (the codemode follow-up; closure record in `18-pi-codemode.md`, *Follow-up, 2026-10-10*). |
| CODE-053 | low | cyrup-original | M | **The nested-call pipeline copies a running call's arguments several times, and the weights of `CODE-042` are calibrated, not derived.** — Residuals of `CODE-042`'s closure, from the sandbox-residuals lane, measured: one running call with a 30M-character string argument costs the host about 6.8 times its text (+204 MB). `NestedToolCallRunner::execute` clones `call.arguments` into `args_value` and again for the start event, and `NestedCallRecorder::start` serialises the arguments to size them; queued calls parse their arguments to a `Value` on arrival rather than when they get a slot, and deferring that would need the `ToolCallback` signature to carry text. The 32 Mi limit bounds the total (about 200-260 MB over an idle host, measured per shape) instead of removing the copies. The weights (comma 48, container 256) were calibrated on a debug build of the pipeline as it is; if its copying changes they must be re-measured (the derivation is in the doc comments of `ARGUMENT_COMMA_WEIGHT` and `ARGUMENT_CONTAINER_WEIGHT`). **Fix** -- clone once and pass the text; re-measure the weights. **Verify** -- the 30M-character call costs the host less than 3 times its text. **FILED 2026-10-10** (the codemode follow-up; closure record in `18-pi-codemode.md`, *Follow-up, 2026-10-10*). |
| CODE-054 | low | cyrup-original | M | **Memory for unawaited huge tool RESULTS (host to script replies) is not bounded.** — Residual of `CODE-042`, from the sandbox-residuals lane; **not measured**. The weight limit covers arguments only. The reply path (`sandbox/execution.rs` `complete`, `HostReply`, the prelude's `settle`, `JSON.parse` in the isolate) is bounded by the script's own 256 MB heap and the process's `RLIMIT_DATA`, but a script that starts many calls whose results are large is counted against nothing host-side. **Fix** -- weigh the results of started-and-unsettled calls the way their arguments are weighed, or cap a reply. **Verify** -- an unawaited loop of calls whose results are about 10 MB ends with a `RangeError` and a bounded peak. **FILED 2026-10-10** (the codemode follow-up; closure record in `18-pi-codemode.md`, *Follow-up, 2026-10-10*). |
| CODE-055 | low | cyrup-original | S | **ACP sends a `codemode` result's image data twice: as an image content block and again inside `rawOutput`.** — From the follow-up-3 lane. `crates/cyrup-acp/src/translate.rs` (`image_contents`) is marked [CYRUP-DELTA]: the image blocks go out as content (`CODE-051`) and `rawOutput` is kept for pi-acp parity (`raw_output: result.clone()` when the result has no diff), so an ACP editor receives the base64 twice. Stated in `docs/codemode.md` and the guide copy; **not measured against Zed**. **Fix** -- measure what Zed does with `rawOutput`, and drop the image data from it if it is unused. **Verify** -- an `image()` result's frame carries the data once, and the editor still shows the image. **FILED 2026-10-10** (the codemode follow-up; closure record in `18-pi-codemode.md`, *Follow-up, 2026-10-10*). |

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
| `CODE-002` | `crates/cyrup-codemode-runtime/src/sandbox/` | V8 through `deno_core`, one isolate per execution; the V8-vs-QuickJS differences a script can observe are documented in `sandbox/mod.rs` and in `docs/codemode.md`: heap exhaustion is uncatchable (the uncaught result equals upstream's), `memory_limit_bytes` below 32 MiB is raised (V8 aborts the process otherwise), `WebAssembly`/`SharedArrayBuffer`/`Intl`/`queueMicrotask` are removed, ArrayBuffer memory is accounted in the prelude because V8's heap limit does not cover it, stack text is V8's **CORRECTED 2026-10-09:** 'heap exhaustion is uncatchable' now fails the script inside a sandbox process and not cyrup (`CODE-025`), and the prelude's ArrayBuffer accounting is backstopped by `RLIMIT_DATA` on Unix (`CODE-026`, `CODE-044`). |
| `CODE-005` | `cyrup-core/src/exposure.rs` (PR #189), `cyrup-agent/src/agent/run/declare.rs`, `cyrup-session*` | the exposure model, and its transcript half: tool declarations recorded as `system` rows and restored on resume, on `/tree` navigation and across a compaction; hidden declarations are recorded and never sent. **Residual: `CODE-014`, `CODE-015`, `CODE-017`.** The row's "hiddenDeclarations … survives resume" claim holds in pi's tests; pi's own CLI path always passes `initialActiveToolNames`, so the constructor restore is not what pi's CLI runs |
| `CODE-006` | `cyrup-core/src/message/nested.rs`, `cyrup-agent/src/agent/nested.rs`, `cyrup-session-svc/src/session/nested.rs`, `cyrup-ext/src/nested.rs` + WIT `host-tool` imports | `nestedCalls` on the persisted AND the live tool result (byte-compatible with pi rows), the runner and its limits, `parentToolCallId` on hook events, compaction file ops, HTML export, TUI skip. Differs from the row's fix text: the loop's events are untouched; nested events are their own variants. `cyrup:ext` world 0.15. **Residual: `CODE-016`** |
| `CODE-007` | `cyrup-codemode-runtime/src/tool/{description,loadout,mod}.rs`, `cyrup-config` `codemode.mode`/`codemode.inlineBudget` | description, round-robin catalog budget, `prepare_loadout` by exposure (not the active set), grammar-constrained sampling; the built-in is registered inactive and is `replaceable` (a same-name extension displaces it: `cyrup-ext/src/replaceable.rs`). `[CYRUP-DELTA]`: the description names the engine "V8" where upstream says "QuickJS". Settings are read at session build (cyrup settings are fixed until `/reload`) **CORRECTED 2026-10-09:** 'grammar-constrained sampling' here is the tool *declaring* `ConstrainedSamplingConfig::Grammar`. No adapter sent it as a custom tool, so it never constrained a request, until `PROV-101` closed on 2026-10-08 (`ad419d5b4`); the closure did not list `PROV-101` as a residual. The description's 'raw JavaScript, not JSON' wording was also false on every function-tool route (`CODE-037`). |
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
   > **CORRECTED 2026-10-09 (read this before relying on judgement call 1).** For cyrup as shipped on
   > 2026-10-06 the model read *nothing*. Its adapters read only `Context::system_prompt` and skip
   > `Message::System`, and the loop built `Context { system_prompt: Some("") }`, so every request
   > carried an empty system message; the tests compared the transcript's replay with itself. The
   > statement above holds only after `request_context` (`1ea706526`) folds the replay into that
   > field. See `CODE-024` and the 2026-10-09 record.
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
| `021eae60a` (v1.0.4, #10251) `resolve codemode read calls on images to image blocks` | `read` declares `outputSchema` and sets `structuredContent` | no `output_schema` on `read` | **`TOOL-058` open** (area 04, S) *(2026-10-09: **CLOSED** by the pi v1.1.0 structured-results pass)* **CORRECTED 2026-10-09: closed** (`c262f5852`; found independently, see `TOOL-058`) |
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
| `CODE-018` | `cyrup-codemode-runtime/src/sandbox/{prelude.js,protocol.rs,execution.rs}` | `lockdown()` after the prelude's own setup and before the script, covering the intrinsics reachable only from instances (generator, async function, typed-array prototypes, …); `describeError` `String()`; `BridgeError`, a strict `script_error` and `store_writes` (a missing `message`, an entry of length 0 or 3+ is now refused with the reason upstream names). **Cost, measured in the dev profile: about 8–10 ms per execution** (one isolate per execution; release not measured). **CORRECTED 2026-10-09:** since `dbe7ee5f1` each execution also starts a sandbox process, 31-39 ms from spawn to the first result frame in the debug build (`CODE-025`). **Recorded delta:** an unreadable *return value* is still a `script` `RangeError`, where upstream reports a `sandbox` `Sandbox bridge broken: …` |
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
  nothing in a cyrup script. Needs the structured result at every return site of `read.rs`. **CORRECTED 2026-10-09: closed** (`c262f5852`).
  *2026-10-09: **CLOSED** — every successful read carries `to_read_output(&content)`; see `TOOL-058`'s row.*
- `MCP-616` (open, area 13, filed by this triage): `--tools` / `--exclude-tools` `*` patterns, `--tools`
  keeping MCP tools, and `--no-mcp` (pi `04b97ef00`). It couples to this area: `--tools codemode` is what
  `codemode.mode: only` invites, and there a script reaches no MCP tool in cyrup. **CORRECTED 2026-10-09: closed** (`843a4c123`). Two other `v1.0.4` MCP
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

## Closure record, 2026-10-09 — the codemode end-to-end repair

Upstream is pi **`v1.0.4`**, read only through git objects (`git -C tmp/pi show v1.0.4:<path>`, `git -C tmp/pi grep -n <pattern> v1.0.4 -- <dir>`), plus `pi-mcp-adapter` **`v5.0.0`**, `pi-permission-system` **`v0.8.0`**, `pi-subagents` **`v0.74.0`** and `pi-acp` **`v0.0.34`** where a row says so. The cyrup side is the branch `claude/eloquent-lovelace-vivq3h` at code HEAD **`bd58662ee`**: 38 non-merge commits, `1ea706526..bd58662ee`, on top of `main` at `4f649ab66` (253 files, +23 333 / −809). The work was done by sixteen lanes, one at a time (thirteen feature lanes, then three fresh-eyes review-and-fix lanes, each followed by an independent verifier), each driving the real `cyrup` binary against a scripted fake OpenAI chat-completions server with stdin from `/dev/null`; two live acceptance runs of sixteen scenarios each (a second round, and a final one built at `bd58662ee`) checked the whole and all sixteen scenarios passed in both. **This record was written by the ledger pass, which edited documentation only and ran no cargo command**; it ran `scripts/count_open_items.py` before and after, and re-read what *What the ledger pass checked itself* lists. Every other measurement below is a lane's report, and says so.

> **CORRECTED 2026-10-10.** Parts of this record were overtaken by the work in *Follow-up, 2026-10-10* at the end of it, and are left as written with a dated note where they are false. `PERM-038` is closed (item 7 below says 'fixed, with a regression that is still open'). `CODE-042`, `CODE-043`, `CODE-045`, `SEAM-154`, `SEAM-155`, `SEAM-156`, `SUBA-217` and `SUBA-219` are closed. `SUBA-218` and `ICOM-087` were already fixed on `main` when this record was written (stale rows), so the list of failing `cyrup-it` targets in *Not done* is false. `CODE-044` is still open. The full `cyrup-it` suite has now been run once (840 of 840, at `1a037e1c9`).

### What was wrong, in user terms

1. **The `CODE-014` closure of 2026-10-06 shipped a regression: cyrup sent no system prompt to any provider.** Found on 2026-10-07 by running the real binary against a scripted OpenAI-compatible server, not by any test. Every request on the route the harness drives (OpenAI chat completions), the first and the one after a tool call, carried one system message whose content was the empty string, and the loop that builds the request and the field every adapter reads are shared by all of them: no `--system-prompt`, no `--append-system-prompt`, no tool list, no rules, no `AGENTS.md`, no `codemode` section. The closure's tests had not looked at the provider request: they replayed the transcript's `sections` rows themselves and compared that text, so they proved the rows and never what an adapter sends (`CODE-024`). The 2026-10-06 closure record said the model reads the replay of the file; for cyrup it read nothing.
2. **`defaultTools: ["+codemode"]`, the line pi's own docs tell a user to write, did not turn `codemode` on** (`CFG-110`; `CFG-097`, closed 2026-10-04, had resolved the list but never activated an extension tool). **`--tools read,grep,codemode` and `--exclude-tools bash` did not restrict**: the permission system's next activation, or a script's `tools.bash`, brought `bash`, `edit` and `write` back (`SEAM-153`).
3. **One model-written line, `new Array(2 ** 27).fill(0)`, killed the whole cyrup process after 7 s**, taking the session with it (`CODE-025`); a typed-array trick grew it past its cap with no error (`CODE-026`). A script with no `timeout_ms` could run for ever (`CODE-028`); 3000 parallel `tools.read` calls took 3.4 GB and were killed, and 3000 sequential ones took a minute and 3.2 GB (`CODE-031`).
4. **Scripts could not rely on basic things:** `const tools = ...` was a SyntaxError, a rejected call had no frame, `text(err)` printed `{}`, a fenced script failed with a type error far from its cause, a blank line before the options line dropped the timeout (`CODE-029`, `CODE-030`); a `bash` that exited non-zero rejected the whole script (`TOOL-054`); `tools.read` of an image produced text (`TOOL-058`).
5. **MCP:** a first script of a `-p` run ran before a slow server's tools existed (`MCP-618`); eager tools returned text only (`MCP-619`); a slow server held the first prompt for 5 s, printed an extension error and still left the tools out of the first request (`MCP-620`); the gateway's `describe` showed a placeholder instead of parameters (`MCP-211`); `--tools` had no patterns and there was no `--no-mcp` (`MCP-616`).
6. **Subagents:** a child's script calls were counted as the child's own tool calls (`SUBA-220`); a child could not use `codemode` under `--no-extensions` (`SUBA-215`); aborting a child left its `bash` commands running (`SUBA-216`).
7. **Permissions:** an open approval dialog froze `timeout_ms`, abort, `new_session` and SIGTERM (`PERM-037`, `MCP-621`); with the permission system armed the first prompt described the wrong tools (`PERM-036`); a repository's own policy could turn the user's `ask` into an `allow` without the user being asked to trust it (`PERM-040`); the model's own read of the codemode docs page was blocked (`PERM-039`); a subagent child under a headless root waited 10 minutes and then said 'User denied' for a user it never asked — **fixed, with a regression that is still open** (`PERM-038`). **[CORRECTED 2026-10-10: the `PERM-038` regression is fixed (`34ed0e2d9`); see *Follow-up*.]**
8. **Providers, config, TUI, guests:** `codemode` was never sent as a grammar-constrained tool (`PROV-101`); a `models.json` provider block deleted that provider's image and classifier models (`CFG-111`); Escape on a running tool left a stale block and drew a second (`TUI-184`); a WASM guest tool could not declare an output schema, return structured content or be reported as failed (`EXT-116`, `EXT-115`, `CODE-015`, `CODE-017`).

### What shipped, and where

| lane | commits | where | rows |
|---|---|---|---|
| system-prompt | `1ea706526` | `cyrup-provider` `utils::request_context` and every adapter's test; `cyrup-agent` `run/stream.rs`; `cyrup-session-svc` test helpers | `CODE-024`; `PROV-133` narrowed |
| default-tools | `f70ecce2b` | `cyrup-session-svc` `default_tools.rs`, `builder.rs` | `CFG-110`; `CFG-097` corrected; `SEAM-155` |
| allowlist-enforcement | `ed01bbb5d` | `cyrup-session-svc` `tools.rs` (`ToolAccess` in `DynamicToolState`), `cyrup/src/session_launch.rs` tests | `SEAM-153` |
| sandbox-memory | `dbe7ee5f1` | `cyrup-codemode-runtime` `sandbox/{process,child,wire}.rs`, `prelude.js`; hidden `__codemode-sandbox` argv verb (`cyrup` `codemode_sandbox_cmd.rs`, `predispatch.rs`) | `CODE-025`, `CODE-026`, `CODE-027`; `CODE-044` |
| script-contract | `63cfd2ecc`, `53f6e8841` | `cyrup-codemode` `source.rs`; `cyrup-codemode-runtime` `sandbox/{prelude.js,isolate.rs}`, `tool/execute.rs`, `types.rs` | `CODE-028`, `CODE-029`, `CODE-030` |
| concurrency | `27f29405d`, `02241b4ba`, `460e21773`, `4d4a455ed` | `tool/{execute,recorder}.rs`, `sandbox/child.rs`, `prelude.js`; `cyrup-agent` `Agent::nested_context`; docs | `CODE-031`, `CODE-032`, `CODE-033`; `CODE-042`, `CODE-045` |
| tool-surface | `c262f5852`, `7f9199581` | `cyrup-tools` `read.rs`, `bash.rs`, `output.rs`; `cyrup-codemode` `identifier.rs`, `discovery.rs`; `cyrup-codemode-runtime` `tool/{description,discovery,docs}.rs` | `TOOL-054`, `TOOL-058`, `CODE-034`, `CODE-035`, `CODE-036` |
| grammar-sampling | `ad419d5b4` | `cyrup-provider` `api/{openai_responses,azure_openai_responses,openai_codex_responses,openai_completions}`, `utils/constrained_sampling.rs`; `cyrup-codemode-runtime` `tool/grammar_tests.rs` | `PROV-101`, `CODE-037` |
| mcp | `3a982df2f`, `843a4c123` | `cyrup-mcp` `extension.rs`, `registration.rs`, `proxy/call.rs`; `cyrup-core` `tool_names.rs`; `cyrup-session-svc` `tools.rs`, `builder.rs`; `cyrup` `cli/` | `MCP-616`, `MCP-618`, `MCP-619` |
| subagents-permissions | `05fadd4aa`, `43d363908`, `f39072532`, `1b889b7b4` | `cyrup-ext-subagents` `exec/`, `tui/`, `background/`; `cyrup-permission-system`; `cyrup-session-svc` `builder.rs` | `SUBA-220`, `SUBA-215`, `PERM-035` |
| wasm-guests | `fa76720d2`, `3adb8437d` | `cyrup-core` `exposure.rs`, `tool.rs`; `cyrup-mcp`; `cyrup:ext` 0.17 (`cyrup-ext`, `cyrup-ext-sdk`) | `CODE-015`, `CODE-017`; `CODE-016` narrowed; `EXT-116`, `EXT-115` |
| e2e-tests | `637fc636a`, `a70e344fc` | `cyrup-it` `support::fake_openai`, `tests/bin/codemode_wire.rs` (26 tests), `session_svc` `codemode_real_run_screen` | tests only; found `SUBA-218`, `ICOM-087` |
| docs | `682841674`, `7c5a6a1e4`, `16774e5e9` | `docs/guide/**`, `docs/codemode.md`, `cyrup --help` example | docs only |
| fresh-eyes-1 | `07698f0c0`, `9a0cbf754`, `6c869417b`, `0bcb1564f`, `69eea1df0` | `prelude.js`, `sandbox/process.rs`, `tool/execute.rs`; `cyrup-permission-system`; `cyrup-mcp` `proxy/tool_metadata.rs`, `extension.rs`; `cyrup-ext-subagents` `spawn/signal.rs` | `CODE-038`, `CODE-039`, `CODE-040`, `PERM-036`, `MCP-211`, `MCP-620`, `SUBA-216`; `SUBA-217`, `SUBA-219` |
| fresh-eyes-2 | `0047c157a`, `3202e28ce` | `cyrup-config` `provider_compose.rs`; `cyrup-provider` `wire.rs`; `cyrup-permission-system` `ask.rs`; `cyrup-mcp` `owner.rs` | `CFG-111`, `PERM-037`, `MCP-621`; `SEAM-154`, `SEAM-156` |
| fresh-eyes-3 | `7abba7cef`, `05d632b25`, `f403d4319`, `38b1f6ccc`, `bd58662ee` | `prelude.js`, `tool/execute.rs`; `cyrup-tui` `app/input.rs`; `cyrup-permission-system` | `CODE-041`, `TUI-184`, `PERM-039`, `PERM-040`; **`PERM-038` open**; `CODE-043` |

### Measured, before and after

All from the debug binary driven by a scripted model unless a row says otherwise; peak RSS is the cyrup process alone, or the process tree where stated. These are the lanes' measurements, not the ledger pass's.

| what | before | after | row |
|---|---|---|---|
| system message on request 0 and 1 | 0 characters | 2647 (1135 with `--system-prompt` and `--append-system-prompt`, both strings present) | `CODE-024` |
| first-request tools, `defaultTools: ["+codemode"]` | `read,bash,edit,write,mcp,ask_user_question` | the same plus `codemode` | `CFG-110` |
| first-request tools, `-t read,grep,codemode` (permission system armed) | `bash, codemode, edit, find, grep, ls, powershell, read, write` | `codemode, grep, read` | `SEAM-153` |
| `new Array(2 ** 27).fill(0)` | SIGTRAP after 7.0 s, process dead, no result | rc 0, `InternalError: out of memory`, 7.6 s, a second call in the same session returned `[2, 4, 6]` | `CODE-025` |
| typed-array species `slice` loop | 3610 MB, killed by the harness, no error | `InternalError: out of memory`, 432 MB tree, 0.2 s | `CODE-026` |
| `while (true) {}` with no options line | ran until killed at 40 s | `Script timed out` at 120.1 s | `CODE-028` |
| 3000 parallel `tools.read` | killed at 3432 MB, 16.4 s, unfinished | 2.0 s, 138 MB, result 3000 | `CODE-031` |
| 3000 sequential awaited reads | 47 s; 72.6 s and 3199 MB in a second run | 7.8 s, 139 MB | `CODE-031` |
| `for (;;) tools.read()` under the default limits | 104.7 s, 380 MB, growing | 3.8 s, 146 MB, `RangeError` at 10000 in flight | `CODE-033` |
| a 40.9M-character `return` | 17.0 s, 1722 MB tree | 2.2 s, 509 MB tree, `RangeError` | `CODE-041` |
| `grep -c nomatch a.txt` in `tools.bash` | rejected the script, `Error: 0  Command exited with code 1` | resolved to `{"output":"0\n","truncated":false,"exit_code":1,"wall_time_seconds":0}` | `TOOL-054` |
| `tools.read` of a PNG | the note text | an image block; `image()` saved it and the next request carried it | `TOOL-058` |
| first script vs a cold MCP server (answering at once, and after 4 s) | `ALL_TOOLS` held no `fx_*` tool at either delay | four `fx` tools at both (0.6 s and 4.4 s) | `MCP-618` |
| first request vs a stdio MCP server answering `initialize` after 2 s and 7 s | tools declared from the second request | tools in request 0 at 2.4 s and 7.4 s; a 13 s server left after 10.6 s without an error | `MCP-620` |
| child script of write, read, read: settled `toolCount` | 4 | 1 | `SUBA-220` |
| `sleep 61` started by an aborted child's `bash` | still running, parent pid 1 | gone | `SUBA-216` |
| nested call parked on an unanswered dialog: `timeout_ms` 2000 / abort / `new_session` / SIGTERM | no result in 15 s / no reply in 15 s / no reply in 15 s / still running after 20 s | 2.5 s / 0.01 s / 0.08 s / exit 143 in 0.03 s | `PERM-037` |
| `getModelsOfType("image")` with a `providers.openrouter` block | 0 | 55 | `CFG-111` |
| global `bash: ask` plus project `bash: allow`, untrusted project | ran `echo PWNED > pwned.txt` with no prompt | the project's `allow` is not honoured | `PERM-040` |
| child `ask` under a headless (`-p`) root | still waiting at the 90 s harness timeout | refused in under 2 s (see the regression, `PERM-038`) | `PERM-038` |
| guest tool with an `outputSchema`, called from a script | the text | `{"type":"object","doubled":6,"tags":["a","b"]}` | `EXT-116` |

Process and platform numbers: the sandbox process costs 31-39 ms from spawn to the first result frame (debug build; the in-process isolate of `CODE-018` cost 8-10 ms), an idle one has VmData 325 MB, VmRSS 78 MB and 13 threads; a run of 40 sequential reads in `--mode json` emitted 3 `tool_execution_update` events and the final end carried all 40 calls.

### Corrections to earlier closure text

- **The `CODE-014` closure record (2026-10-06), judgement call 1,** says `Context::system_prompt` is now always empty and the model reads the replay of the file. For cyrup that was false until `1ea706526`: the adapters read only `Context::system_prompt`. Corrected in place and on the `CODE-014` row; `SESS-054` and `EXT-084` carry notes (the layout and the forced-prompt projection reached the transcript, not the wire).
- **The `CODE-007` closure** lists 'grammar-constrained sampling' among what shipped. The tool *declared* `constrainedSampling`; no adapter sent a custom tool, so the grammar never constrained a request, `PROV-101` (open since 2026-09-30) was not named as a residual, and the description told the model the input is raw JavaScript on routes where it was a JSON `{ code }` argument. Shipped by `ad419d5b4` (`PROV-101`, `CODE-037`). The area's opening paragraph (*What is NOT new and is already in cyrup*) and `CODE-003`'s body carry the same over-reading and are annotated.
- **The `CODE-002` closure** describes an in-process `deno_core` isolate and heap exhaustion as 'uncatchable (the uncaught result equals upstream's)'. Since `dbe7ee5f1` production runs each isolate in a sandbox process; heap exhaustion kills that process and fails the script, not cyrup (`CODE-025`). The in-process placement remains for tests and embedders and still aborts its host.
- **The 2026-10-07 record** lists `TOOL-058` and `MCP-616` as open and `CODE-018`'s cost as 8-10 ms per execution. Both rows are closed; the cost is now the process hop above.
- **`CFG-097`** (closed 2026-10-04) over-claimed: see `CFG-110`. **`PROV-133`** is the row the code comments call `PROV-083b` (the lanes used that name); it is narrowed, not closed.
- **`CODE-016`** is **narrowed, not closed**: the signal gap closed, three gaps remain by construction (the single-instance `Store`). **`MCP-614`** is narrowed: a live-stdio-server `cyrup-it` case now covers the script half of the search-mode path (`codemode_wire::search_mode_mcp_tools_are_found_described_and_called_from_a_script`), not `mcp({ search })` loading a tool for the next request. **`MCP-612`** part (b) is half moot: every MCP tool now declares the `CallToolResult` schema, but the cache still carries no per-tool `outputSchema`.
- **Docs lane claims overtaken or refuted:** 'cyrup does not hold the first prompt for direct-tool servers (upstream waits up to 10 seconds)' (`16774e5e9`) was corrected by `0bcb1564f`. Two 'parity' residuals the docs lane filed are refuted below.

### Rows closed, narrowed and filed

- **Closed existing rows:** `CODE-015`, `CODE-017`, `TOOL-054`, `TOOL-058`, `PROV-101` (area 01), and in area 13 `MCP-211`, `MCP-616`.
- **Narrowed, left open with a dated note:** `CODE-016`, `PROV-133`, `MCP-612`, `MCP-614`.
- **Filed and closed:** `CODE-024` … `CODE-041` (18 rows), `CFG-110`, `CFG-111`, `EXT-116`, `EXT-115`, `TUI-184`, `SEAM-153`, `SUBA-220`, `SUBA-215`, `SUBA-216`, `PERM-035`, `PERM-036`, `PERM-037`, `PERM-039`, `PERM-040`, and in area 13 `MCP-618` … `MCP-621`.
- **Filed open:** `CODE-042`, `CODE-043`, `CODE-044`, `CODE-045`; `SEAM-154`, `SEAM-155`, `SEAM-156`; `SUBA-217`, `SUBA-218`, `SUBA-219`; **`PERM-038` (medium)**; `ICOM-087`. Next free ids at the end of the repair, in the numbering the rebase onto `main` gave the branch's rows: `CODE-046`, `CFG-112`, `EXT-117`, `TUI-185`, `SEAM-157`, `SUBA-221`, `PERM-041`, `ICOM-088`, `MCP-622`; the repair moved no other counter. **[CORRECTED 2026-10-10: of these, `CODE-042`, `CODE-043`, `CODE-045`, `SEAM-154`, `SEAM-155`, `SEAM-156`, `SUBA-217`, `SUBA-219` and `PERM-038` are closed by the Follow-up; `SUBA-218` and `ICOM-087` are struck as stale; `CODE-044` is open. Next free ids are those the Follow-up gives.]**

### `[CYRUP-DELTA]`s this work introduced

Each is marked in the code, named in its row, and a delta that costs behaviour has a row. Request path: the system rows are collapsed to one leading prompt (`request_context`, `CODE-024`/`PROV-133`); the grammar map is `Context::tools` plus the transcript's declared tools (`PROV-101`); the tool description names the `code` argument (`CODE-037`). Tools: `defaultTools` pending names, the unmatched-name warning and the reload baseline (`CFG-110`, `SEAM-155`); the allowlist bound lives in `DynamicToolState` (`SEAM-153`); `Tool::is_mcp_tool` (`MCP-616`). Sandbox: a process per script (`CODE-025`), default limits (`CODE-028`), first-non-blank options line and fence stripping (`CODE-029`), wrapper scope, frames, rendering, depth 64 and the 5 MiB image cap (`CODE-030`), 16 concurrent calls, the 100 ms/500-row recorder and the shared transcript (`CODE-031`), 10000 in flight (`CODE-033`), unique identifiers (`CODE-034`), the catalog marker (`CODE-035`), the embedded docs page (`CODE-036`), U+FFFD for lone surrogates (`CODE-038`), `/proc/self/exe` (`CODE-039`), the unfinished-calls note (`CODE-040`), the return value counted (`CODE-041`). MCP: the bounded `tool_call` wait, failures as results, the 10 s first-prompt bound (`MCP-618`, `MCP-619`, `MCP-620`). Subagents: nested events split from the child's own calls (`SUBA-220`), the child keeps a named `codemode` (`SUBA-215`), descendants killed at abort (`SUBA-216`). Permissions: the nested-call label, the mirrored-prompt refresh, dialogs on the blocking pool, the shipped-page exemption, the headless-root marker and tighten-only untrusted policy (`PERM-035` … `PERM-040`, `MCP-621`). Guests: a synchronous `prepare_loadout` on a scoped thread with a lagging answer when the instance is busy, and a deadline in place of `AbortSignal.timeout` (`CODE-015`, `CODE-016`). `cyrup --help`'s codemode example only enables the tool, because `mcp__<server>__*` does not narrow cyrup's MCP tools (`682841674`). The book carries a link-rewritten copy of `docs/codemode.md` because the file is embedded in the binary and read by the model at `<agent dir>/docs/codemode.md` while mdBook cannot include a file from outside `docs/guide`; a test pins the two together.

### Checked and not a gap

The docs and live-acceptance lanes recorded these as residuals or observations. The first five bullets were read on both sides by the ledger pass; the next two are the lanes' reading of the upstream side; the tool-order remark inside the third bullet was compared with nothing. None is filed.

- *'Nothing turns `codemode` or `tool_search` on when an MCP server connects; there is no `autoEnableCodemode`.'* That is pi's **built-in** MCP extension (`docs/mcp.md:204`, `:230` @v1.0.4). The adapter cyrup ports says the opposite: `pi-mcp-adapter` `config.ts:1148` @v5.0.0, 'Pi's top-level `autoEnableCodemode` has no adapter equivalent and is ignored', and nothing in the adapter activates `tool_search`. This also settles the 'unowned default-exposure divergence' in `CODE-001`'s body: it belongs to pi's extension, not to the thing cyrup ports.
- *'Eager MCP tools and the `mcp` gateway are unreachable from a script under `--tools read,codemode`.'* pi's `_getCallableTools` (`agent-session.ts` @v1.0.4) makes a `direct` tool callable only while it is active, and `codemode`/`deferred` tools always; cyrup's split is the same. `docs/mcp.md:228` ('`pi --tools read,codemode` keeps every MCP tool callable') describes pi's built-in extension, whose default exposure is `codemode`.
- *'An armed permission system activates `codemode`, `tool_search`, `powershell`, `grep`, `find` and `ls`, and `defaultTools`/`--no-builtin-tools` do not narrow it.'* pi-permission-system `index.ts:1880-1896` @v0.8.0: `pi.setActiveTools(allowedTools)` over `pi.getAllTools()` filtered only by `shouldExposeTool`. Upstream behaviour; documented in the guide. The active-tool *order* with the permission system (`[codemode, read]` against the flag order `[read, codemode]` without it, stable across turns) was seen by the e2e lane and compared with nothing.
- *`max_output_tokens: 0` is accepted and truncates all output.* `isSafeInteger` is `>= 0` upstream (`source.ts` @v1.0.4).
- *ACP shows a running script's progress as a raw JSON dump.* pi-acp's `toolResultToText` ends in `JSON.stringify(result, null, 2)` for a result with no text (`translate/pi-tools.ts` @v0.0.34), and the docs lane's live ACP run shows cyrup's output taking the same form.
- *SIGTERM mid-script exits 143 or 1* (the e2e lane read `print-mode.ts` @v1.0.4: a signal handler `process.exit(143)` against `exitCode = 1` for an aborted last message, the same two paths).
- *A nested `bash` that exits non-zero is drawn as a failed call while the script sees a result* (the tool-surface lane read `bash.ts:259` @v1.0.4: `isError: true` with the structured value).
- **Findings the lanes refuted:** the mcp lane's 'tools.mcp__x__y is missing' (cyrup names MCP tools by `toolPrefix`: `fx_echo`) and 'the `--tools` carve-out can key on the `mcp__` name' (a name-only port would have dropped every MCP tool under `--tools`); the e2e lane's 'the exit-1 SIGTERM race is recorded in the codemode ledger' (it was not), 'persisted `nestedCalls` feed the TUI rows' (the screen test stays green without the stamping; rows come from live `details.calls`), and 'cyrup-mcp's `with_call_tool_result` decides what a working call resolves to' (it only wraps failures; `proxy/call.rs` `script_call_tool_result` does). The `MCP-616` known limit (an allowlist entry naming a prefixed MCP tool does not switch the allowlist to deciding for MCP tools) is *more permissive* than pi + the adapter, where `isMcpToolName` (`core/mcp-servers.ts` @v1.0.4) is `mcp__`-prefix-or-resource-tool and an adapter tool named `fx_echo` is dropped by `--tools`.

### Not done, not verified, and left open

- **Open and in the table:** `PERM-038` (the regression above), `SEAM-154`, `SEAM-155`, `SEAM-156`, `SUBA-217`, `SUBA-218`, `SUBA-219`, `CODE-016` (three gaps), `CODE-042` … `CODE-045`, `ICOM-087`, `MCP-612`, `MCP-614`, `PROV-133`. **[CORRECTED 2026-10-10: of this list `CODE-016` (three gaps), `CODE-044`, `MCP-614` and `PROV-133` are still open; `MCP-612` shows CLOSED 2026-10-08 in `13e`, so the 'narrowed' above is overtaken; see the Follow-up for the rows it filed.]**
- **Residuals recorded here only (no row, no mechanism or no second side read):** (a) *one unexplained flake, seen once more.* The subagents lane saw `cyrup-session-svc` `host_services::tests::proc_spawn_defaults_omitted_cwd_to_the_session_cwd` fail once with empty child stdout under concurrent build load; it passed on the next run and was not checked against the base commit. It looks like the family the detach closure in `00-residual-ledger.md` calls 'a starved child on a saturated box', which is still not understood; the concurrency lane suspected a relation to the late-reply exit of `CODE-032` without testing it. File it as a row on the next sighting that comes with a log. (b) `terminate_on_timeout` aside (`SUBA-217`), the abort ladder's `ps` fallback and the restart hints of `CODE-039` were not compiled off Linux (`CODE-044`). (c) The root `codemode_skipped_diagnostic` is non-fatal and visible only in the interactive `[Extension issues]` panel; print and json stay silent. (d) The permission extension's text sanitizer still looks for pi's `Available tools:` and `Guidelines:` headers and so removes nothing from cyrup's `<tools>`/`<rules>` (re-read in `sanitize/tools.rs`); the prompt follows the declared set at its source, and the sanitizer is equally dead against pi 1.0's own layout. (e) The placed-child prompt hook (`SUBA-100`) is `SUBA-219`; the prompt rebase of `PERM-036` applies only when no earlier handler edited the prompt. (f) A partial renderer result shows a call only after it gets a slot, so with more than 16 queued calls the TUI shows at most the 16 running plus finished rows (`CODE-031`). (g) `SESS-035` is **unchanged**: `cyrup-session-svc/src/builder.rs` still passes `DocsPointers::default()`, so no production prompt carries the docs section, whatever the template's `docs_guidance` now says.
- **Not exercised by anyone:** a real OpenAI, Azure OpenAI or Codex endpoint (the grammar path is fixtures and loopback only; **reopen `PROV-101` on the first real endpoint that rejects the `custom` tool, a replayed `custom_tool_call` or the `ctc_` id handling**); the interactive TUI by hand (one pty run through a pyte harness for `TUI-184`); `--mode rpc` approval dialogs and ACP `request_permission` beyond the paths named in `PERM-037`; macOS and Windows (`CODE-044`); the other provider adapters through the real binary (only `openai-completions` is exercised end to end; the eight per-adapter body tests are the guard); MCP over HTTP/OAuth and MCP servers that fail; a detached (async) subagent child running codemode, `--resume` and the session selector (only a foreground child and `-c` are covered); the 30-minute default for its full duration in a committed test; the full workspace `cargo nextest run`, which no lane ran.
- **Gates, as the lanes reported them (not re-run):** `cyrup-codemode-runtime` 244/244, `cyrup-agent` 251/251, `cyrup-session-svc` 671/671 (concurrency lane); 2745 passed across core, ext, session-svc, mcp, cyrup, tool-search and codemode-runtime, and 5037 in `cyrup-ext-subagents` (mcp lane); clippy `-D warnings` with real `Checking` lines and `cargo fmt --all -- --check` on the crates each lane touched. **Failing before this work and not caused by it:** the `cyrup-it` `subagents` target does not compile (`SUBA-218`; confirmed by reading to predate the branch), the `intercom` target fails clippy (`ICOM-087`), `embedding::reads_state_after_a_run` fails (`SEAM-156`; not run on `main`), and the `cyrup-it` suite's `no_ambient_provider_credentials` guard fails in a shell that exports `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY` (an environment trap, not a repo defect: the documented route is `cargo run -p xtask -- it`, which clears the environment; one lane saw 2 of 107 tests fail on it and another 27 extra failures in a wide run). `cyrup-it` was therefore run per target only. **[CORRECTED 2026-10-10: the `subagents` target compiles and the `intercom` target is clippy-clean at HEAD (the two rows were stale), `embedding::reads_state_after_a_run` was a wrong assertion and is fixed, and the full suite was run once, 840 of 840, at `1a037e1c9`, with `cargo run -p xtask -- it`, which clears the credential environment. The README and `docs/TEST-ARCHITECTURE.md` had advertised a bare `cargo build --workspace --bins` for `CYRUP_IT_BIN_DIR`; the suite needs four builds (`cyrup` with `faux`, and the intercom and subagent fixture binaries with `test-fixtures`), and both documents now say so, and count ten targets and about 840 tests where they said nine and about 590.]**
- **Disk:** the shared target directory ran to 0 free more than once; lanes deleted superseded duplicate rlibs, test executables and, once, the nested `cyrup-it` bin build, all rebuilt on demand. Nothing committed was affected.

### Evidence that is not in the repository

The red-proof outputs, live transcripts and request logs are in the session scratchpad, `/tmp/claude-0/-home-user-cyrup/6ac38d4c-a09c-59fa-8063-f22ab09bc198/scratchpad/wf/<lane>/` (`system-prompt`, `default-tools`, `allowlist`, `sandbox-memory`, `script-contract`, `concurrency`, `tool-surface`, `grammar-sampling`, `mcp`, `subagents-permissions`, `wasm-guests`, `e2e-lane`, `docs`, `fresh-eyes-1`, `-2`, `-3` with their `-verify` and `-fix` siblings, `live-acceptance-r1`, `-r2`, `-final`) and the harness it used in `.../scratchpad/live/` (`fake_openai.py`, `jsrun.py`). **That directory is not part of the repository and will not survive the session.** What stays is the tests and the commit messages, which carry the measured numbers above. Red-proof counts as the lanes reported them (each is a production piece reverted or broken, never the test, with the intended failure seen before restoring): `default-tools` 10; `allowlist` 5; `grammar-sampling` 24 production mutations and one description reversion; `e2e-tests` 28 mutations over 13 binary builds, in which every one of the 27 new tests failed for its intended reason at least once, and two mutations that did *not* turn a test red exposed a vacuous image assertion (a script's source holds the same data URI as the request; now `image_parts()` looks at the `image_url` part) and a denylist case that `-xt` alone did not pin. The other lanes name their red proofs in the rows above. Three overlapping process-kill mechanisms (`send_sigkill_tree`, `KillTreeOnDrop::drop`, `kill_tracked_detached_children`) mean each `codemode_wire` process test only goes red when all are removed.

### What the ledger pass checked itself

At HEAD, by `grep` and reading: the code anchors of `CODE-015` (`world.wit` `prepare-loadout`, `host/live.rs`), `CODE-017` (`ToolAnnotations`, `Tool::annotations`, the `host_services` row), `TOOL-054` and `TOOL-058` (`output_schema` and `structured_content` in `bash.rs` and `read.rs`), `MCP-616` (`tool_names.rs`, `--no-mcp`), `MCP-211` (`format_schema` in `proxy/tool_metadata.rs`) and `PROV-101` (the `custom_tool_call` arms); that the cited test names exist (twenty-one sampled); the `PERM-038` regression by tracing `sync_unattended_marker`; the `SUBA-218`, `ICOM-087` and `SEAM-156` failures by reading the structs, literals and test (not by compiling or running); `SUBA-217` and `SUBA-219` by reading both the cyrup and the upstream side; `SESS-035` (`builder.rs`); and upstream at tags: `execute.ts:427` and `host.ts:147` (no default deadline), `worker.ts:146` and `prelude-source.ts:335-344`, `:462` (wrapper, output count, bare reject), `source.ts` (`max_output_tokens`), `rpc-mode.ts:802-805` (`onInputEnd`), `agent-session.ts` `_isAllowedTool`/`_isActivatable`/`_getCallableTools`, `core/mcp-servers.ts` `isMcpToolName`, `docs/mcp.md`, pi-permission-system `index.ts:1880-1896` and `README.md`, pi-mcp-adapter `config.ts:1148`, `index.ts:432-452` and `direct-tools.ts:263` (annotations are attached to a registration only through `deferredToolFields`; `direct-tools.ts:263` feeds the approval call), pi-subagents `herdr-pi-bridge.ts`, and pi-acp `toolResultToText`. Everything else, notably every measurement, every red proof and every upstream cite not listed here, is the lane's.

### Follow-up, 2026-10-10 — the open rows of the repair, four more fresh-eyes rounds, and whether the review converged

Upstream is pi **`v1.0.4`**, read only through git objects, plus `pi-permission-system` **`v0.8.0`**, `pi-subagents` (**`v0.74.0`** and, for `SUBA-217`, **`v0.34.0`**), `pi-acp` **`v0.0.33`** and, for the MCP rows of the last review round, `pi-mcp-adapter` **`v5.2.0`**. Area 13's own pin is still `v5.0.0`; **the window `v5.0.0..v5.2.0` was not triaged by this pass** (the lanes read the lines they cite at `v5.2.0`; no triage of the window was among the inputs). The cyrup side is the branch `claude/eloquent-lovelace-vivq3h` at code HEAD **`998ae8a53`**: 29 non-merge commits, `34ed0e2d9..998ae8a53`, on top of the merge `ed3192f7e` with `main` (120 files, +8 364 / −678, nine of them documentation). The work was done by four residual lanes (`perm-038`, `it-suite`, `sandbox-residuals`, `headless-subagent-residuals`), then four fresh-eyes review rounds each followed by a fix lane (`fresh-eyes-followup-1` to `-4`), one at a time, and two live acceptance runs of sixteen scenarios against the real binary and a scripted OpenAI-compatible server (the first built at `450b35f3c`, before any fresh-eyes fix; the last at `998ae8a53`, the HEAD). **This record was written by the ledger pass, which edited documentation only and ran no cargo command**; it ran `scripts/count_open_items.py` before and after and re-read what *What the ledger pass checked itself* lists. Every other measurement below is a lane's report, and says so. **The six commits that fixed review round 3** (`29effddd8`, `32b2b06fe`, `40188e00b`, `de2b5d79b`, `81492ec08`, `122cfc955`) **have no lane report among the inputs the ledger pass was given**; they are described from their commit messages and diffs.

#### What was wrong, in user terms

1. **A person who ran `cyrup -p ...` and then resumed that session in a TUI or an rpc client was never asked about a subagent child's `bash`** (`PERM-038`, the regression the previous record left open): the headless run's `no-ui` marker outlived it, and the child was refused with 'requires approval, but no interactive UI is available' although a UI was attached.
2. **One `read` could grow the host process to gigabytes in seconds**, from a model call or from a script's `tools.read`: `/dev/zero` took it from 740 MB to 4 GB in four seconds and a sparse 3 GiB file to 5.8 GB (`TOOL-062`). A script's argument and returned-value memory had the same shape: 5000 calls with 1 MiB arguments took the host to 1055 MB and killed the sandbox process, two calls with million-element arrays took it to 3236 MB, and one returned array of 1.25 million small objects held 830 MB (`CODE-042`, `CODE-043`).
3. **With an armed permission policy the model was told to read a `/tmp` spill file and then refused it with 'Hard stop' (`PERM-041`); after the first answered subagent permission prompt a WARN line scrolled the terminal UI away every 250 ms (`PERM-042`); and with `ulimit -v` below about 512 GiB cyrup did not start at all (`EXT-117`).**
4. **A script that forgot an `await` before a failing `tools.write` ended as 'Script completed'** (`CODE-046`, `CODE-047`). The first fix then reported failures that had been handled (`CODE-048`), and the fix of that in turn hid the forgotten awaits inside async functions again (`CODE-049`).
5. **After `/new`, an rpc `new_session` or a second ACP `session/new`, the permission policy stopped shaping the tool set** (`codemode`, `grep`, `find`, `ls` disappeared and the denied `bash` came back) **and an async subagent's completion was injected into the replaced session**, which ran a model turn nobody could see (`EXT-118`).
6. **MCP:** a model's first `mcp({...})` or direct MCP call while a cold server was starting got 'MCP not initialized' (`MCP-625`); a server that never answered hung the call, the connect and the first prompt for good, there being no default request timeout (`MCP-626`); the first prompt was held up to 10 s for servers that declare no tools (`MCP-622`) and every discovery script waited two minutes for a hung server (`MCP-623`); parallel first calls to a lazy server failed with 'Server "x" is not connected' in 6 of 30 and 11 of 30 trials (`MCP-627`); a malformed `mcp.json` was invisible in the TUI and printed ten times under `-p` (`MCP-624`); a server that failed at once was announced nowhere in the TUI (`SEAM-157`).
7. **A script that started its calls together put up one approval dialog for each, and each needed its own answer** (`PERM-043`; 25 asks in the lane's tmux run); in an ACP editor a script's output was glued into one line and its images were not shown (`CODE-051`); a script's nested `bash` calls left `cyrup-bash-*.log` files that no document named (`CODE-050`).
8. **`--mode rpc` did not shut down at stdin EOF**, it waited for the run (46 s, or for ever behind an unanswered dialog; `SEAM-154`); `/reload` forgot what the setting held before (`SEAM-155`); `/subagent-cost` reported 0 turns for an async child (`SUBA-175`); a subagent's timeout or dropped handle left what its `bash` had started (`SUBA-217`); a placed child's prompt described the wrong tools (`SUBA-219`).
9. **Tests:** a `cyrup-it` test asserted two messages where a session has three (`SEAM-156`); nothing failed if the nested host stopped sharing the agent's transcript (`CODE-045`); and two rows the previous record counted as failing gates, `SUBA-218` and `ICOM-087`, had been fixed on `main` already and were false at HEAD.

#### What shipped, and where

| lane | commits | where | rows |
|---|---|---|---|
| perm-038 | `34ed0e2d9` | `cyrup-permission-system` `forwarding.rs`, `extension/watcher.rs`, `forwarding/unattended_tests.rs`; `cyrup-it` `codemode_wire.rs`; `docs/guide/extensions/permissions.md` | `PERM-038`; `PERM-044`, `PERM-045` |
| it-suite | `1a037e1c9` | `cyrup-it` `tests/bin/embedding.rs`; the repository `README.md` and `docs/TEST-ARCHITECTURE.md` | `SEAM-156`; `SUBA-218` and `ICOM-087` struck as stale |
| sandbox-residuals | `51e73ae0e`, `1114f485a` | `cyrup-session-svc` `src/tests/nested_transcript_sharing.rs`; `cyrup-codemode-runtime` `sandbox/{mod.rs,prelude.js}`, `types.rs`, `tool/execute.rs`, tests; `cyrup-it` `codemode_wire.rs`; `docs/codemode.md` | `CODE-042`, `CODE-043`, `CODE-045`; `CODE-053`, `CODE-054` |
| headless-subagent | `da7bea4b0`, `3aef3784b`, `920215c29`, `7c3a5baa6`, `450b35f3c` | `cyrup-ext-subagents` `artifacts.rs`, `spawn/`, `prompt_runtime.rs`; `cyrup-session-svc` `runtime.rs`, `services.rs`, `factory.rs`; `cyrup-modes` `rpc/mod.rs` | `SUBA-175`, `SUBA-217`, `SUBA-219`, `SEAM-155`, `SEAM-154`; `SEAM-158`, `SEAM-159`, `SEAM-161`, `SUBA-221` |
| review round 1 | `576c06086`, `0e6049258`, `c809adfcf`, `9fe06661c` | `cyrup-tools` `read.rs`, `ops/cancel_read.rs`; `cyrup-core` `spilled_files.rs`; `cyrup-permission-system` `extension/decide.rs`, `forwarding.rs`; `cyrup` `bootstrap.rs`; `cyrup-ext` `host/engine.rs`; `cyrup-session-svc` `builder.rs` | `TOOL-062`, `PERM-041`, `PERM-042`, `EXT-117`; `TOOL-063` |
| review round 2 | `3dbce52aa`, `65ce39c98`, `5a2eef9ea`, `199e29618` | `cyrup-codemode-runtime` `sandbox/`, `tool/execute.rs`; `cyrup-session-svc` `host_services.rs`; `cyrup-mcp` `extension.rs`, `registration.rs`; `cyrup-permission-system` `extension/prompt.rs` | `CODE-046`, `CODE-047`, `SEAM-157`, `MCP-622`, `MCP-623`, `PERM-043`; `PERM-046`, `SEAM-160`, `MCP-628`, `MCP-629` |
| review round 3 | `29effddd8`, `32b2b06fe`, `40188e00b`, `122cfc955`, `de2b5d79b`, `81492ec08` (no lane report), then `36fff979e` (fix lane 3) | `cyrup-codemode-runtime` (`HasHandler` op); `cyrup-ext` `host/services.rs` and six extension crates; `cyrup` `session_launch/replacement_tests.rs`; `cyrup-acp` `translate.rs`, `sessions.rs`; `cyrup-mcp` `config.rs`, `runtime.rs`, `ui.rs` | `CODE-048`, `CODE-049`, `EXT-118`, `CODE-051`, `MCP-624`; `CODE-052`, `CODE-055`, `MCP-632` |
| review round 4 | `f50d8be91`, `c3fa141a2`, `a25dc01e9`, `20214f417`, `998ae8a53` | `cyrup-mcp` `extension.rs`, `dispatch.rs`, `live.rs`, `runtime.rs`, `server_manager.rs`; docs | `MCP-625`, `MCP-626`, `MCP-627`, `CODE-050`; `MCP-630`, `MCP-631`, `MCP-633` |

#### Measured, before and after

The lanes' measurements, on the debug binary driven by a scripted model unless a row says otherwise; none is the ledger pass's.

| what | before | after | row |
|---|---|---|---|
| rpc resume of a session a `-p` run left a no-ui marker for; the child's `bash` under an ask policy | dialog not shown, child refused 'requires approval, but no interactive UI is available' (0.9 s) | dialog shown, child returns `from-child` (1.1 s), marker gone | `PERM-038` |
| a `-p` root still running; its child's ask | refused at once | refused at once (0.9-1.0 s against a 15 s forwarding wait) | `PERM-038` |
| `cyrup-permission-system` unit tests | 235 | 252 | `PERM-038` |
| `cyrup-it` `bin` target | 122 run, 121 passed, 1 failed (`embedding`) | full suite 840 of 840, 976.4 s, at `1a037e1c9` | `SEAM-156` |
| 5000 `tools.read` calls with 1 MiB arguments | host 1055 MB, tree 2098 MB, 82.0 s, sandbox SIGTRAP | `RangeError` after 31 calls, 3.2 s, host 178-184 MB, tree 279-282 MB | `CODE-042` |
| two calls with 1 000 000-element object arrays as arguments | host 3236 MB, tree 3406 MB, 31.5 s | refused at the first call, 137 MB, 1.0 s | `CODE-042` |
| a returned array of 1.25M small objects (16 388 891 characters) | host 830 MB, 7.0 s | host 209-214 MB, 2.8-3.0 s (grew about 50 MB) | `CODE-043` |
| rpc, stdin closed with a nested `bash` parked on an unanswered dialog | still running 20 s later | rc 0 in 0.07 s | `SEAM-154` |
| rpc, stdin closed during a `sleep 47` `bash` call | 46.57 s | 0.03 s, sleep gone | `SEAM-154` |
| rpc, a `prompt` and an immediate close | 47.70 s | 0.37-0.43 s, 5 of 5 | `SEAM-154` |
| cost report of an async child that took 3 turns | `turns` 0 | 3 | `SUBA-175` |
| a `defaultTools` name removed by one reload and re-added by the next | stays inactive | activates | `SEAM-155` |
| `read` of `/dev/zero`, direct or through `tools.read` | host 740 MB to 4 GB in 4 s | fails in 0.5 s at 140 MB | `TOOL-062` |
| `read` of a sparse 3 GiB file | host 5.8 GB | refused at 140 MB | `TOOL-062` |
| WARN lines after a forwarded permission prompt | 41 in 11 s | 0 | `PERM-042` |
| `cyrup -p` under `RLIMIT_AS` of 2 to 384 GiB | exit 1, 'wasm engine init failed' | starts with one WARN; scripts need more than 32 GiB | `EXT-117` |
| a failing `tools.read` the script did not await | 'Script completed', no note | a note naming the error | `CODE-046`, `CODE-047` |
| `Promise.all` in `try/catch`, siblings failing after the script ended | the false 'never handled ... await every call' note | a neutral note, no advice | `CODE-048`, `CODE-049` |
| tools declared after `new_session` with `bash` denied | `read, bash, edit, write, mcp, ask_user_question` | the same eleven tools as before | `EXT-118` |
| `agent_end` events after an async child finished following `new_session` | 1 (injected into the replaced session) | 2 (reaches the active session) | `EXT-118` |
| an ACP script result | `first linesecond line{"a":1}[1,2]` | lines, plus an `image/png` block for `image()` | `CODE-051` |
| `mcp({ search })` during a 3 s `initialize` | 'MCP not initialized' | waits and answers at 3.4 s | `MCP-625` |
| a direct MCP call with no `requestTimeoutMs`, silent server | running when the harness killed it at 75 s | rejects at 60.7 s, 'MCP request timed out after 60000 ms' | `MCP-626` |
| trials with a 'not connected' failure, parallel first calls to a lazy server | 6 of 30, 11 of 30 | 0 of 30, 0 of 30 | `MCP-627` |
| first request vs a server that never answers (gateway, search) | 10.4 s | 0.5 s (still 10.4 s with `directTools: true`) | `MCP-622` |
| a script with `timeout_ms` 3000 vs a server that never answers | two minutes | 3.1 s (the next script that allows it waits 123.7 s once) | `MCP-623` |
| WARN lines for one malformed `mcp.json` under `-p` | 10 | 1 | `MCP-624` |
| 25 nested asks in the TUI | 25 dialogs | one dialog | `PERM-043` |
| notice for a MCP server that failed at once, TUI | never (9 s) | 0.8 s | `SEAM-157` |

Both live acceptance runs passed all sixteen scenarios (system prompt and flags, `defaultTools`, allowlists, built-ins, bash exit codes, images, options and fences, deadlines, memory, parallelism, errors and rendering, name collisions and the catalog, MCP, permissions, subagents). The second ran at `998ae8a53`. Their one reading call: scenario 14's clause 'a `--tools codemode` keeps MCP tools' is true for `directTools: "search"` servers only; eager and gateway MCP tools are registered and not callable from scripts, which is documented and is pi's `_getCallableTools`; both runs flagged it, and if the scenario meant every exposure mode, clause 4 fails. Neither run drove the TUI, ACP or `--mode rpc` front ends (both used `-p` and `--mode json`); the first run also lists the cost lines, the classifier and image models, and macOS and Windows as not verified.

#### Rows closed, struck as stale, narrowed and filed

- **Closed existing rows (12 counted):** `PERM-038` (medium), `SEAM-154`, `SEAM-155` (for `/reload`; the rest is `SEAM-159`), `SEAM-156`, `CODE-042`, `CODE-043` (on the amplification; see its note about the literal 200 MB), `CODE-045`, `SUBA-175`, `SUBA-217` (the probe timeout is `SUBA-221`), `SUBA-219` (not driven end to end), and **struck as stale: `SUBA-218`, `ICOM-087`**.
- **Left open with a dated note:** `CODE-044` (nothing could verify it; four facts added).
- **Filed and closed (13 counted, 6 in area 13):** `TOOL-062`, `PERM-041`…`PERM-043`, `EXT-117`, `EXT-118`, `SEAM-157`, `CODE-046`…`CODE-051`; `MCP-622`…`MCP-627`.
- **Filed open (13 counted, 6 in area 13):** `CODE-052`…`CODE-055`, `PERM-044`…`PERM-046`, `TOOL-063`, `SEAM-158`…`SEAM-161`, `SUBA-221`; `MCP-628`…`MCP-633`.
- **Annotated:** `MCP-618` and `MCP-620` (their waits are narrowed by `MCP-622`, `MCP-623`), `SEAM-005` (the EOF path is now compared). **Next free ids** (after the rebase onto `main`, whose own filings moved these counters): `CODE-056`, `PERM-047`, `TOOL-064`, `EXT-115`, `SEAM-162`, `SUBA-222`, `MCP-634`, `CFG-112`, `TUI-185`, `ICOM-088`; `PROV-153`, `SESS-073` and `AGENT-048` are `main`'s and this branch did not move them.
- **Rows the lanes showed false:** `SUBA-218` and `ICOM-087`. `SUBA-218` names `4f649ab66` as its base and `ICOM-087` was filed on the same pre-merge branch (as `ICOM-083`); both were carried into the previous record's gate list as failing `cyrup-it` targets without being re-read after `main` was merged. Counting every `SingleStepSpec` and `ForegroundRunRequest` literal under `crates/cyrup-it/tests/subagents` at four revisions (the ledger pass, by reading each literal): 29 of 30 lack `worktree` at `4f649ab66` and at `cc1fcd627^`, none at `cc1fcd627` and at HEAD; the `fn session` helper and the single-arm `match` that `ICOM-087` names exist at the first two and not at HEAD. `cc1fcd627` is `main`'s area-11 backlog, an ancestor of HEAD. The previous record's claims 'the `cyrup-it` `subagents` target does not compile', 'the `intercom` target fails clippy' and 'cyrup-it was therefore run per target only' are corrected in place. The review also dropped one finding in round 4 (`tools.read` in a script is cut at 2000 lines or 50 KB with a footer inside the returned string, while the docs say it resolves to the file's text); the ledger pass was not given the reason and did not examine it.

#### `[CYRUP-DELTA]`s this follow-up introduced

Each is marked in the code and named in its row. Permissions: the process-scoped, swept no-ui marker (`PERM-038`); the spill-file exemption from the external-directory guard (`PERM-041`); the self-healing inbox and the log-file redirect of tracing (`PERM-042`); 'Reject All From This Script' (`PERM-043`). Tools: `read` refuses devices, FIFOs and sockets, and images over 512 MiB (`TOOL-062`). Sandbox: the 32 Mi argument weight (`CODE-042`), the returned value held as text, no 128-level `RangeError` (`CODE-043`), the unobserved-error and late-failure notes (`CODE-046`…`CODE-049`), a one-second settle bound after the script's end. Hosting: the on-demand wasm engine and the native-only host (`EXT-117`), the rebindable host-services slot (`EXT-118`), the early `Notify` queue (`SEAM-157`), the rpc EOF settle window of 5 s (`SEAM-154`), the `/reload` baseline (`SEAM-155`), the tree kill (`SUBA-217`). ACP: newline-joined text blocks and image content (`CODE-051`). MCP: the first-prompt wait on the whole connect pass and the remembered stalled wait (`MCP-622`, `MCP-623`, `MCP-629`), the 30 s bound on a direct call that arrives during the build (`MCP-625`, `MCP-631`), the 128-level message limit now ending at 60 s (`MCP-626`, `MCP-630`).

#### Residuals the lanes reported, and where each one went

Rule applied: a residual that names a behaviour gap, a bounded-but-unfixed resource cost or a missing test has a row; a residual that is a verification not run, an environment fact or a hand-off is listed here or under *Not done* and has none.

| lane | residual | disposition |
|---|---|---|
| perm-038 | two live processes on one session id | `PERM-044` |
| perm-038 | a background child outliving an exited headless root is no longer refused at once | a recorded behaviour change in `PERM-038`; replaces the `38b1f6ccc` remark |
| perm-038 | other PID namespace, non-Linux start identity, sweep bounded to 256 entries | `PERM-045` |
| perm-038 | `SessionShutdown` does not withdraw the marker | deliberate; recorded in `PERM-038` |
| perm-038 | the red-proof runner restored files with their old mtime, so cargo kept a stale mutant's test binary once | scratchpad only; every figure re-run on a rebuilt, verified-clean tree |
| it-suite | the 840-test suite was not re-run after the lane's last (comment-only) edit; `SEAM-156` not run on `main`; a pending 'fixture child exits zero' note did not appear | in the `SEAM-156` row and under *Not done*; the earlier unexplained flake (a) stays unexplained |
| sandbox | `CODE-044` unverifiable | open, dated note |
| sandbox | weights calibrated on a debug build; the pipeline copies a running call's arguments several times | `CODE-053` |
| sandbox | batching more than about 30 MB of arguments in flight needs awaiting in batches | a documented delta in `CODE-042` |
| sandbox | unawaited huge results unbounded (not measured) | `CODE-054` |
| sandbox | the host-memory probe test is Linux only | noted in `CODE-043` |
| headless | `/reload` restores the transcript's last-declared loadout; cold resume baseline | `SEAM-159` |
| headless | `agent_start` re-abort has no deterministic red proof | `SEAM-161` |
| headless | `dispatch_owned` futures dropped when the 5 s bound expires | a documented delta in `SEAM-154` |
| headless | an extension dialog in `before_agent_start` stalls the rpc loop | `SEAM-158` |
| headless | preflight probe timeout signals the group only; `ps` fallback and non-Linux tree kill not compiled | `SUBA-221`; `CODE-044` note |
| round 1 | the `main.rs` tracing redirect has no test | `SEAM-161` |
| round 1 | `edit`, `grep` context and ACP `read` unguarded | `TOOL-063` |
| round 1 | a wasm guest never loaded under a limited address space | in `EXT-117`; under *Not done* |
| round 1 | scripts need more than 32 GiB of address space | `CODE-044` note |
| round 1 | an unattributed SIGKILL of the sandbox in the reviewer's 48-snippet session, no OOM evidence, not reproduced | recorded here only; **file it on the next sighting that comes with a log** |
| round 2 | 'Reject All' does not abort the parent call | `PERM-046` |
| round 2 | a server that never answers `initialize` stays connecting | overtaken by `MCP-626` (60 s default; measured 60.7 s) |
| round 2 | the script's `timeout_ms` is not subtracted from its deadline; nothing tells the model under `-p` | `MCP-628` |
| round 2 | the first-prompt wait is on the whole connect pass | `MCP-629` |
| round 2 | only `Notify` effects are held early, 32 at most | `SEAM-160` |
| round 2 | the unobserved-error note is text only; the `exit()` path; a one-second bound | `CODE-052` |
| round 2 | `squash` and `rebase` onto `main` not done | hand-off to the stage that owns them; see *Not done* |
| round 3 | a caught `Promise.all` failure gets one neutral line; only host-seen failures are named | `CODE-052` |
| round 3 | ACP sends image data twice | `CODE-055` |
| round 3 | `log_diagnostic` logs an identical error once per process | `MCP-632` |
| round 4 | a message nested more than 128 levels ends at 60 s | `MCP-630` |
| round 4 | a direct call mid-build waits at most 30 s and does not observe its cancel token while it waits | `MCP-631` |
| round 4 | the never-answers case has only a paused-clock test; the HTTP arm has none; the deduped-connect test depends on wake order | `MCP-633` |
| round 4 | intermediate commits were never built or tested alone; `cyrup-it` `--test mcp` not run (ENOSPC) | under *Not done* |
| acceptance | the 256M-character return ends as 'sandbox crashed ... SIGABRT' | `CODE-044` item (4) |
| acceptance | a 3-way MCP name collision prints a WARN that also shows under `-p`; compared with nothing upstream | observed; not filed |
| acceptance | two stale `fake_openai.py` processes from earlier agents (ports 18785 and 19511) were still running | harness leftovers; not touched |

#### Checked and not a gap

Read on both sides by the ledger pass: **`cyrup --help`'s line for `--tools` ('Keeps MCP tools unless an entry starts with mcp__') is pi's own text** (`cli/args.ts:315` @v1.0.4), and the nuance that eager and gateway MCP tools are not callable from scripts under `--tools codemode` is pi's `_getCallableTools`, documented in `docs/codemode.md`; the final acceptance run's remark about the wording is therefore not a divergence. The lanes' reading, not re-read: the deny-rule message that starts with no subject is a port of `formatDenyReason` (`pi-permission-system`); an image replaced by '(tool image omitted: model does not support images)' with the saved-file label kept is the provider layer; the script wall time excludes the pre-start MCP wait, as the docs say.

#### Did the fresh-eyes review converge?

**No.** Four rounds ran and every one still confirmed findings: **4, 6, 5 and 5, twenty confirmed in all, and one more dropped in round 4**. Every fix lane's verdict was a pass and every confirmed finding was fixed, with a red-proven test where the lane could write one, but **no round came back dry, so no claim of the form 'the review found nothing more' can be made, and none is made here.** Two facts argue against reading the passes as closure: the fix of a round-2 finding left a test in another crate (`cyrup`) failing at the branch head for five commits (`3dbce52aa` to `32b2b06fe`), and the fix of a round-3 finding introduced a regression of its own (`CODE-049`), found afterwards by the follow-up-3 lane although the round's own fix verdict had been a pass. The ledger pass was not told why the review stopped after round 4 and does not guess. Eight of the twenty were in MCP start-up and timing, four in the codemode result notes and docs, two around a session replacement and four in round 1's host-resource and permission-spool findings; what the rounds never reached is not known.

#### Squash and rebase onto `main`, 2026-10-10

The branch's 73 commits (71 non-merge, two merges of `main`) were squashed into **one commit** and rebased onto `origin/main` at **`c9efa3c11`** (merge of #223), 38 commits past the old merge-base `6b145755e` (#209). Nothing was pushed. **Every short commit id quoted in the records above and in the rows (`576c06086`, `7c3a5baa6`, `da7bea4b0`, `998ae8a53`, `1a037e1c9`, `ed3192f7e` and the rest) names a commit of the pre-squash history that no longer exists in the branch**; the content they point at is in the squashed commit, and the ids are kept as the lanes recorded them.

- **Ids renumbered** (only the ids this branch filed; `main`'s were not touched; the list is the rule, not a hand edit): `CODE-021`…`CODE-052` to `CODE-024`…`CODE-055`; `CFG-103`, `CFG-104` to `CFG-110`, `CFG-111`; `EXT-110`…`EXT-113` to `EXT-111`…`EXT-114`; `MCP-617`…`MCP-632` to `MCP-618`…`MCP-633`; `SEAM-148`…`SEAM-156` to `SEAM-153`…`SEAM-161`; `SUBA-176`…`SUBA-182` to `SUBA-215`…`SUBA-221`; `TOOL-059`, `TOOL-060` to `TOOL-062`, `TOOL-063`; `TUI-171` to `TUI-184`. The remap was applied to the lines this branch added, by hunk, never to lines that were already on `main`, and every 'next free id' paragraph was rebuilt on `main`'s chain.
- **Second rebase, onto `0fa9c89fe` (merge of #224), 2026-10-10:** `main` moved 19 commits (the llama.cpp cluster: `EXT-094`, `EXT-098`, `EXT-100`, `EXT-108`, `EXT-110`, `PROV-104`; the System One port) and filed `EXT-111`…`EXT-114` and `PROV-153` itself, so this branch's `EXT-111`…`EXT-114` became `EXT-115`…`EXT-118` (the remap was applied to the lines this branch added, by line, never to lines that were already on `main`; `EXT-115` is the next id on `main`'s chain, and area 06's counter now reads `EXT-119`). Five files conflicted and were resolved keeping both sides: `cyrup-provider` `wire.rs` (`main`'s `classifiers` registry and this branch's `operations_base` composition, `CFG-111`, both kept: a composed provider answers `classify` and `generate_images` from its base, a provider that owns a registry from the registry), `session_launch.rs` (codemode attaches only when `builtin:codemode` is not disabled, as `main`'s `EXT-094` has it, and keeps its sandbox factory and agent directory; the test helper `factory_with` is `main`'s, `factory_configured` was dropped for it), `cli/help.rs`, `guide/reference/cli.md` and the area-06 ledger (the counter and the row block). The `cyrup:ext` world is still **0.20** (`main` is at 0.19).
- **Found independently on both sides and merged into one mechanism:** the structured `bash` and `read` results (`TOOL-054`, `TOOL-058`, closed by `main`'s structured-results pass too: `main`'s `read_full_output`, `to_read_output` and output schemas are kept, this branch's size and file-type guard and spill-file registry are layered on them); `defaultTools` activation and `/reload` (`CFG-108`, `CFG-109` on `main`; `CFG-110`, `SEAM-155` here: `default_tools::plan` replaced `main`'s `activate_default_extension_tools`, and `main`'s `reload_default_tools` / `build_for_reload` / `activate_added_default_tools` replaced this branch's `previous_default_tools` plumbing); the turn count of an async child (`SUBA-175`: `main`'s `run_artifact_metadata` change was identical and was dropped, its test kept); the console layout (`main`'s `OutputItem::Console` and `==> text N/M <==` headers: the prelude's console lines now go out as `Console` items, `FromProcess::Console` carries them over the sandbox pipe, and the sandbox tests that expected `Text` items for console lines or unheadered text lines were rewritten to the layout); and the `--tools` list (`main`'s `ToolSelection` and `check_tool_list` for the `+name` / `-name` modifiers, its `SEAM-148`, are kept; this branch's `without_disabled_builtins` and registry-level allowlist enforcement, `SEAM-153`, formerly `SEAM-148`, sit beside them in `SessionBuilder`).
- **[CYRUP-DELTA]s kept or added by the merge:** `bash`'s `exit_code` schema description (a non-zero code is an error for the model, the call still resolves; `bash.rs` `output_schema`), which `main`'s exact-schema test `both_shell_tools_declare_pi_bash_output_schema` and the session-svc declaration test `the_bash_and_read_declarations_render_their_output_schemas` now assert; `default_tools::DefaultToolInputs::names_apply` (the modifier-applied selection has neither `tools` nor `noTools`; see the `CFG-110` row); the `cyrup:ext` world is **0.20** (`main` is at 0.19; both `world.wit` copies are byte-identical and `HOST_WORLD` agrees), so no renumbering was needed.
- **Tests changed by the merge, not by a new fix:** 10 tests in `cyrup-codemode-runtime` (`sandbox::tests::contract` x6, `sandbox::tests::process` x2, `tool::engine_tests` x2) expected the old layout and were rewritten to `main`'s; `cyrup-tools`' exact-schema test and the session-svc declaration test carry the `exit_code` description; main's `a_modifier_tools_list_adjusts_the_default_selection` failed on the first merged tree and is what found the `names_apply` case; and `cyrup-it`'s `tests/mcp/resumed_render.rs` did not compile (a textual merge left `duration_ms: None` twice in a `Message::ToolResult` literal, one line from each side; the branch's was removed), which only a build of the `it`-armed suite shows, since `cargo check --workspace --all-targets` skips the suite. `main`'s `activate_default_extension_tools` unit test went with that function, and this branch's two `FullOutput` tests in `cyrup-tools` `output.rs` went with the `FullOutput` code `main` replaced. No new test was added by the rebase, so none needed a red proof of its own; each rewritten test was seen failing on the merged tree before it was rewritten (10 of 442 in `cyrup-codemode-runtime`; 2 of 6342 in the session-svc, agent and ext-subagents run, one of them `main`'s own).
- **Verified on the rebased tree** (one invocation at a time, disk under 1 GB free throughout; stale test executables were deleted between runs): `cargo nextest run -p cyrup-codemode -p cyrup-codemode-runtime`, 442 of 442; `-p cyrup-tools`, 434 of 434; `-p cyrup-session-svc`, 867 of 867; `-p cyrup-agent -p cyrup-ext-subagents --features cyrup-ext-subagents/test-fixtures`, passed in the 6342-test run that also failed the two session-svc tests above; `cargo clippy ... --all-targets -- -D warnings` over the 22 crates the squash touches, exit 0; `cargo clippy -p cyrup-it --features it --all-targets -- -D warnings`, exit 0 after the fix above (run with `CYRUP_IT_BIN_DIR` and `CYRUP_EXT_FIXTURE_COMPONENT` pointed at stand-ins, so the suite's nested 3.7 GB binary build did not run: it type-checks and lints, it was not executed); `cargo clippy -p cyrup-ext-sdk --target wasm32-wasip2 --all-targets -- -D warnings`, exit 0 (the guest SDK against the merged `world.wit`); `cargo fmt --all -- --check`, exit 0; `cargo check --workspace --all-targets`, exit 0. **Not run:** the workspace-wide `nextest`; the `cyrup-it` suite (`codemode_wire` and the rest: it needs the `cyrup` binary and the wasm component, and the disk had under 1 GB free); the tests of `cyrup-modes`, `cyrup-permission-system`, `cyrup-mcp`, `cyrup-tui`, `cyrup-acp`, `cyrup-provider`, `cyrup-config`, `cyrup-core`, `cyrup` and `cyrup-ext` after the rebase. The counters were re-measured: `00-residual-ledger.md` has the figures (123 open, 1112 closed, 17 trackers; area 13: 570 rows).

#### Not done, not verified

- **Push was not done**, by design. **The squash and the rebase onto `main` were done afterwards** (see the section above); the next sentences describe the state before them and are kept as the follow-up recorded it. The local `main` is stale (`0a5b296dc`; HEAD is 202 commits ahead of it); `origin/main` is `8052de522`, 18 commits past the merge-base `6b145755e`. Follow-up lane 2 reports `git merge-tree` against `origin/main` conflicting in 18 files, 17 of them source (`cyrup-codemode-runtime` `prelude.js`, `execute.rs`, `engine_tests.rs`, `tool/description/tests.rs`; `cyrup-session-svc` `builder.rs`, `runtime.rs`, `services.rs`, `tests/codemode.rs`; `cyrup-tools` `read.rs`, `bash.rs`, `output.rs`, `tests/tools.rs`; `cyrup-config`, `cyrup-ext-subagents`, two `cyrup-provider` files, `cyrup-it` `embedding.rs`) and `docs/codemode.md`, **and in `docs/gap-analysis/*`: this ledger pass will conflict there too, and the stage that rebases owns the resolution, including the id renumbering `00-residual-ledger.md` already did once.**
- **Gates were not re-run by the ledger pass.** The full `cyrup-it` suite (840 of 840, clippy and fmt clean) was run once, at `1a037e1c9`, **with only the first two of the 29 commits present**; the other 27 were not gated by it. After that: `codemode_wire` only (37 of 37 at the sandbox lane's final binary, and in follow-up 2), the seam suites type-checked and not run (follow-up 3), `--test mcp` not run (follow-up 4, ENOSPC), the workspace-wide `nextest` never run, and the `cyrup-session-svc` and `cyrup-tui` suites not run in follow-up 3. Round 4's intermediate commits were never built alone; only the final tree was gated.
- **Not exercised by anyone:** macOS and Windows; a real OpenAI, Azure OpenAI or Codex endpoint (the standing `PROV-101` reopening condition); the interactive TUI by hand beyond the tmux runs for `PERM-042`, `PERM-043` and `SEAM-157`; ACP against a real Zed; a placed (Herdr) child (`SUBA-219`); a wasm guest under a limited address space (`EXT-117`); an interactive `/reload` (`SEAM-155`); a live run of the `SUBA-217` timeout and drop paths; the 30-minute default deadline in a committed test.
- **Disk:** the shared target directory ran to under 1 GB more than once; lanes deleted stale test executables, once the nested `cyrup-it` bin build (3.7 GB, rebuildable) and once a private 6 GB build; nothing committed was affected.

#### Evidence that is not in the repository

The red-proof outputs, live transcripts and request logs are in the session scratchpad, `/tmp/claude-0/-home-user-cyrup/6ac38d4c-a09c-59fa-8063-f22ab09bc198/scratchpad/wf/<lane>/` (among them `perm-038` and `perm-038-verify`, `it-suite` and `it-suite-verify`, `sandbox-residuals`, `headless-subagent-residuals`, `live-acceptance-followup-1`, `live-acceptance-final`, `live-accept`, `fresh-eyes-2-fix2/red` for round 2's red proofs, and the `fresh-eyes-*`, `fresh-surface-r*`, `operator-r*`, `sandbox-r*` and `surface-r*` directories of the review rounds; which round each belongs to is not recorded in what the ledger pass was given) and the harness in `.../scratchpad/live/`. **That directory is not part of the repository and will not survive the session.** What stays is the tests and the commit messages.

#### What the ledger pass checked itself

At HEAD, by `git`, `grep` and reading: the 29 commits and their messages; for `SUBA-218` and `ICOM-087`, the literal counts and anchors at `4f649ab66`, `cc1fcd627^`, `cc1fcd627` and HEAD (a throwaway script over `git show`); the shape of `embedding::reads_state_after_a_run` at HEAD; the code anchors of `PERM-038` (`prune_unattended_markers`, `write_unattended_marker`, `process_demonstrably_gone`, `UNATTENDED_SWEEP_LIMIT`, the rewritten docs paragraph), `CODE-042`, `CODE-043`, `CODE-045` (`MAX_PENDING_ARGUMENT_WEIGHT`, `ARGUMENT_COMMA_WEIGHT`, `ReturnValue`, `nested_transcript_sharing.rs`, `nested_context()` in `session/nested.rs`), `SEAM-154` and `SEAM-155` (`abort_at_eof`, `EOF_SETTLE_TIMEOUT`, `default_tools_in_effect`), `SUBA-175`, `SUBA-217`, `SUBA-219`, the preflight `send_sigkill` that `SUBA-221` names, `MAX_READ_BYTES`, and the unhandled-rejection ops in `prelude.js`; that ten of the cited test names exist (`grep 'fn <name>'`); and upstream at tags: `rpc-mode.ts` `case "prompt"` and cyrup's `is_inline_command`, `handle` and `prompt_with` (both sides of `SEAM-158`), `agent-session.ts` `reload` (`previousDefaultTools` and `getActiveToolNames`), `pi-permission-system` `src/index.ts` `startForwardedPermissionPolling` and its four callers @v0.8.0, `pi-mcp-adapter` `direct-tools.ts` @v5.2.0 (the direct executor awaits `initPromise` with no bound), `bash.ts` `OutputAccumulator` and `pi-bash` @v1.0.4, `cli/args.ts:315`, `pi-acp` `translate/pi-tools.ts` `texts.join('')` @v0.0.33, and that `git grep -i unhandled v1.0.4 -- packages/codemode` is empty. Everything else, notably every measurement, every red proof and every upstream cite not listed here, is the lane's.

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
exists. **CORRECTED 2026-10-09:** declare, yes; sending it was `PROV-101`, closed 2026-10-08.

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

## Triage 2026-10-09 — pi `v1.0.4..f1b2e77f5` (codemode) and pi `v1.0.1..f1b2e77f5` (codemode extension)

> **PIN 2026-10-09 — cyrup `6b14575` × pi `f1b2e77f5` (= `v1.1.0-11-gf1b2e77f5`).** This area's window moves from
> `v1.0.4` to `f1b2e77f5`. `packages/codemode` was read over `v1.0.4..f1b2e77f5` (area 18 had already triaged
> `v1.0.1..v1.0.4`), and `extensions/codemode` over `v1.0.1..f1b2e77f5` by the extensions lane. Upstream read
> through `git -C tmp/pi show` only; nothing run. The pin is untagged, deliberately (README *CURRENT PINS*).
>
> **Filed (3, all low):** `CODE-021` (output items run together; `eb326d265`; filed by both the extensions and the
> tools/codemode lanes and merged), `CODE-022` (the `await` on the discovery helpers; `269121616`), `CODE-023`
> (`models.classify()` images; `ce8972a0e`; depends on `PROV-148`). Related rows elsewhere: `PROV-147` (the
> `openai-decisions` classifier, area 01).
>
> **Read in scope and deliberately NOT filed:** `021eae60a` (read `outputSchema` image block) is the open
> `TOOL-058` (closed later the same day by the pi v1.1.0 structured-results pass); `d677d0ee7` is `TOOL-057` / `CODE-019` (closed); `b223082bb` is `CODE-018` (closed); `c30840c2e` is
> `CODE-020` (closed); `7f9e1198f` swaps two literals for `SETTINGS_DEFAULTS` values, unchanged; `1b094148b`
> (Node install-layout restart hint) stays ruled at `:335` above; `36a686ee8` is ported (ledger UPDATE 2026-10-09).
> `packages/coding-agent/suite` named in the lane brief does not exist; the real path, `test/suite`, adds only
> test files whose sources are other lanes' (`04b97ef00` → `MCP-616`; `27c7b6ff4` → `SEAM-149`; `503c60552` →
> `TUI-171`; `ce8972a0e` → `PROV-147`/`PROV-148`/`CODE-023`; `4c6b724ea` → `TUI-180`).

## ~~CODE-021~~ — Codemode output items run together: several `text()` items get no `==> text N/M <==` headers, and `console.*` lines are interleaved with them instead of being grouped in one `<console_output>` block — **CLOSED 2026-10-09**

> **CLOSED 2026-10-09** by the pi v1.1.0 structured-results pass, which had filed the same finding under the same id
> the same day; the evidence and the tests are on the summary row. Kept below as #210 filed it.

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi `v1.0.4..f1b2e77f5` (codemode) / `v1.0.1..f1b2e77f5` (extension), cyrup `6b14575`).

**upstream** — `eb326d265` (v1.1.0~21). `packages/codemode/src/types.ts:48-50` adds `console?: true` to the text arm of `CodemodeOutputItem`; `packages/codemode/src/runtime/prelude-source.ts:444` makes `console[level]` emit `output("console", …)`, and `runtime/worker.ts:83-86` maps it to `{ type: "text", text, console: true }`. `packages/coding-agent/src/extensions/codemode/execute.ts` @f1b2e77f5: `formatOutput()` (`:255-283`) numbers the non-console text items (`text()` and the returned value) with `==> text N/M <==\n` when there is more than one, keeps images in place, and appends all console lines as one trailing `<console_output>\n…\n</console_output>` item; `joinAdjacentText()` (`:284-298`) joins neighbouring text blocks with a newline before truncation and again after `saveImages`. The returned value is pushed before formatting (`:509`), `formatOutput` runs at `:511`, and `Script error:` is appended after it (`:512`). The model-facing description (`tool.ts:146`) and `docs/codemode.md:18`, `:27` document the layout.

**cyrup** — `crates/cyrup-codemode-runtime/src/sandbox/prelude.js:341-345` sends `console.*` as `output("text", …)` (`:344`), the same kind as `text()` (`:271`). `crates/cyrup-codemode-runtime/src/sandbox/isolate.rs:97-110` `op_codemode_output` knows only `"image"`; everything else becomes `OutputItem::Text(data)`. `crates/cyrup-codemode/src/types.rs:71-78` `OutputItem::Text(String)` has no console marker. `crates/cyrup-codemode-runtime/src/tool/execute.rs:336-384` passes the raw item list (value at `:357`, `Script error:` pushed inside the `Failed` arm at `:361-362`) straight to `truncate_output` (`:380`) and `label_images`. `truncate_output` joins text items with `"\n"` only on its over-budget path (`crates/cyrup-codemode/src/output.rs:109-123`). `grep -rn 'console_output\|==> text' crates/` finds nothing. The description (`tool/description.rs:45`) and `docs/codemode.md:31` ("Like `text()`") keep the v1.0.4 wording.

**Impact** — Providers join adjacent text blocks with a newline or with nothing, so a script that calls `text()` twice and logs with `console.log` reaches the model as one undifferentiated blob: it cannot tell items apart, or debug logging from deliberate output. The prompt also describes the old layout. Readability for the model only; no data loss.

**Fix** — Carry a console marker end to end: emit `output("console", …)` in `prelude.js`; accept the `"console"` kind in `op_codemode_output` and the worker decode; model it as `OutputItem::Text { text, console: bool }` or a third `OutputItem::Console(String)`. Port `formatOutput` and `joinAdjacentText` as pure functions in `crates/cyrup-codemode/src/output.rs` and apply them in pi's order in `execute.rs`: value, format, `Script error:`, generated-images note, join, truncate, label images, join. `Script error:` is pushed inside the `Failed` arm today, so it must move after `format_output`. `join_adjacent_text` must run before the budget check, not only on `truncate_output`'s over-budget path. Update `description.rs:45` and `docs/codemode.md:31` to the v1.1.0 text, in the same edit as `CODE-022`'s neighbouring line so the description-snapshot test is regenerated once.

**Verify** — Port pi's `test/suite/agent-session-codemode.test.ts` cases from `eb326d265`: `text("a"); text("b")` gives `==> text 1/2 <==\na` … `==> text 2/2 <==\nb`; a single text item gets no header; `console.log(1); text("x"); console.log(2)` gives `x` followed by `<console_output>\n1\n2\n</console_output>`; a returned value counts as a text item; a failed script's `Script error:` comes after the formatted output. A sandbox test asserts that a console item carries the marker and a `text()` item does not; the description-snapshot test picks up the new Globals line.

**Notes** — Filed by two lanes (extensions and tools/codemode) and merged here. Effort M: the change crosses three crates (codemode types, the runtime op, execute).

## ~~CODE-022~~ — The codemode tool description does not tell the model that `searchTools`, `describeTool` and `describeNamespace` must be awaited — **CLOSED 2026-10-09**

> **CLOSED 2026-10-09** by the pi v1.1.0 structured-results pass, which had filed the same finding under the same id
> the same day; the evidence and the tests are on the summary row. Kept below as #210 filed it.

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi `v1.0.4..f1b2e77f5` (codemode) / `v1.0.1..f1b2e77f5` (extension), cyrup `6b14575`).

**upstream** — `269121616` (v1.1.0~35, #10555), `packages/coding-agent/src/extensions/codemode/tool.ts:148` @f1b2e77f5: the Globals line now reads ``- `ALL_TOOLS`, `await searchTools(query, { limit?, namespace? })`, `await describeTool(name)`, `await describeNamespace(name)`: find unlisted tools, such as MCP tools.``

**cyrup** — `crates/cyrup-codemode-runtime/src/tool/description.rs:47` still has the pre-fix text with no `await`. The helpers are asynchronous in cyrup too: `tool/discovery.rs:65` / `:96` / `:111` register them through `spread_global` (`tool/globals.rs:24-37`, an async host call), and `sandbox/prelude.js:88-90` `caller()` returns a `new Promise`.

**Impact** — The model is told the lookup helpers are synchronous. A script that writes `searchTools(q).map(...)` or reads `describeTool(n).parameters` gets a Promise and fails with a TypeError (or serializes it as `{}`), costing a codemode round trip before the model retries with `await`.

**Fix** — Copy upstream's line verbatim into `describe_globals` at `description.rs:47`. The "With several text items…" sentence `eb326d265` added to the line above belongs to `CODE-021`; land the two together so the description snapshot is regenerated once.

**Verify** — Extend the description test in `crates/cyrup-codemode-runtime/src/tool/description/tests.rs` so the Globals block matches pi `tool.ts:144-149` @f1b2e77f5 byte for byte, and confirm it fails against the current line.

## CODE-023 — Codemode `models.classify()` does not accept `context.images`, and the classifier context has no images field to carry them, so a script's images are dropped silently

**Kind** upstream-drift · **Severity** low · **Effort** M · **Confidence** confirmed (both sides read; static, nothing run) · **Filed** 2026-10-09

> **Filed 2026-10-09 from the pi v1.1.0 drift triage** (pi `v1.0.4..f1b2e77f5` (codemode) / `v1.0.1..f1b2e77f5` (extension), cyrup `6b14575`).

**upstream** — `ce8972a0e` (v1.1.0~20): `packages/ai/src/types.ts:682-690` @f1b2e77f5 adds `ClassifierContext.images?: ImageContent[]` ("Only models whose `input` includes `image` accept them; other models return an error result"). `packages/coding-agent/src/extensions/codemode/execute.ts:118-119` adds `images?: [{ type: "image", data: <base64>, mimeType }]` to `CLASSIFIER_CONTEXT_SHAPE`, and `:129-142` validates `context.images`: it must be an array, and each entry must be `{type:"image", data:string, mimeType:string}`, with the errors `context.images must be an array, got …` and `context.images[i] must be an image block, got …`. pi's `llama-cpp-classify` never sends images: the same commit makes it throw `${LABEL} classification does not support image input` (`llama-cpp-classify.ts:437`), which becomes an error result, and llama classifier twins declare `input: ["text"]` only.

**cyrup** — `crates/cyrup-codemode-runtime/src/tool/models.rs:199` `CLASSIFIER_CONTEXT_SHAPE` has no `images?`, and `check_classifier_context` (`:203`) does not read it. `crates/cyrup-provider/src/classifier.rs:676-680` `ClassifierContext { state, questions }` has no images field, so an image a script passes is dropped. cyrup's only classifier api is `LlamaCppClassify`, so it has no vision-capable classifier today.

**Impact** — A script that passes images gets an answer computed from `state` alone, with no error, where pi returns an error result. The model also never learns the field exists. The "judge a screenshot with a vision-capable classifier" scenario becomes possible only after `PROV-147` (openai-decisions) or `PROV-104` (System One) is ported.

**Fix** — (1) The provider half is `PROV-148` (area 01): `images: Option<Vec<ImageContent>>` on `ClassifierContext`, the model-input check, and `llama_cpp_classify` returning an error result whenever images is non-empty. (2) In `models.rs`, port pi's shape string and the `context.images` validation with its exact messages, and pass the images through. This row depends on `PROV-148`.

**Verify** — Codemode tests modelled on pi's `agent-session-codemode.test.ts` cases from `ce8972a0e`: `images: 'x'` and `images: [{type:'text'}]` fail with pi's messages and the shape string; a valid image reaches the fake classifier's context; on the llama-cpp classifier a valid image gives the `does not support image input` error result.

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
