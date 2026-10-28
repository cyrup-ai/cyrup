# cyrup

**cyrup** · /ˈsɪr.əp/ · *SIR-up* — rhymes with **syrup**, as in maple syrup.

A coding agent in Rust. One static binary, 40 model providers, and extensions that run as sandboxed
WebAssembly components.

cyrup follows the design of the [Pi](https://github.com/earendil-works/pi) agent harness: a minimal
core, everything-is-an-extension, an agent that can extend itself. It rebuilds that design on a Rust
backbone.

> **Status:** pre-release, at `0.1.0` — see [`CHANGELOG.md`](CHANGELOG.md). The agent loop, provider layer, tool set, session
> tree, terminal interface, extension host, all five run modes, the MCP client, the ACP adapter, the
> `workflowScript` runtime, the Flux development pipeline, background and remote subagent delegation,
> and the [herdr](#herdr) integration work end to end.

| measured at `4fa902b0` (2026-10-11) | |
|---|---|
| crates | 33 |
| lines of Rust under `crates/` | 1,441,937 |
| workspace tests | **16,225 passed, 0 failed**, 12 skipped |
| line coverage | **88.4%** (474,264 of 536,289 executable lines) |
| function coverage | 86.1% (48,933 of 56,834) |
| region coverage | 88.3% |
| gated integration suite (`cyrup-it`) | 862 more tests across eleven binaries |
| upstream citations in source | 37,016, naming 21,726 distinct `.ts` locations |

Tests: `cargo nextest run --workspace --features test-fixtures`. `test-fixtures` is required, or the
two subagent-subprocess fixture binaries never build and their tests silently do not run. Coverage:
the same run under `cargo llvm-cov`, see [Measuring coverage](#measuring-coverage). It counts only
the workspace suite, so crates whose real exercise lives in `cyrup-it` (`cyrup-ext`, `cyrup-ext-sdk`,
the binary, `cyrup-sdk`) read lower than they are tested.

Since the previous measure (`ea23ca2`, 2026-09-24: 24 crates, 1,044,991 lines, 11,311 tests) the
workspace grew by 336 commits and +476,409 / −38,176 lines under `crates/`, across nine new crates
and four that each gained more than 40,000 lines:

- **llama.cpp as a first-class provider.** `cyrup-llama` (+21,654) ports pi's bundled llama.cpp
  extension: the provider, model discovery, Hugging Face search and the `/llama` manager.
  `cyrup-llama-cpp-wire` is a test-only crate holding the `llama-server` wire as one cited
  definition the workspace's fake servers answer from.
- **codemode on an embedded V8.** Split in two so a consumer's `build.rs` can snapshot the engine
  half: `cyrup-codemode` (+5,005) is the script format, TypeScript declaration renderer, BM25 tool
  ranker and output budget; `cyrup-codemode-runtime` (+19,203) is the `deno_core` sandbox and the
  tool's script globals. `cyrup-tool-search` adds pi's `tool_search` over deferred tools.
- **The Pico5 durability kernel**, four crates (+35,414) separating a pure document core from the
  imperative shell, the storage contract and a JSONL backend with its lock, marker protocol and
  recovery.
- **The front-end and the facade.** `cyrup-tui` (+50,147) and `cyrup-session-svc` (+47,126): the
  alternate screen, extension overlay seams, and the host services an extension reads.
- **Providers and MCP.** `cyrup-provider` (+43,207) and `cyrup-mcp` (+13,409).
- **Subagent delegation** continued to be the largest crate, at +50,470.


## Install

You need a stable Rust toolchain, 1.96 or newer. Nothing else: no Node, no Python, no runtime
alongside the binary.

```sh
cargo install --locked --git https://github.com/cyrup-ai/cyrup cyrup
```

The first build takes several minutes, because it compiles a WebAssembly runtime, a JavaScript
engine, a git implementation and a TLS stack from source. Later builds reuse the cache.

## Start a session

Export a provider key and run `cyrup` in a repository:

```sh
export ANTHROPIC_API_KEY=sk-ant-...
cd ~/code/my-project
cyrup
```

You get a transcript with an editor at the bottom and a status line showing the model and thinking
level. Ask a question and the answer streams in. On anything about your code the agent reaches for a
tool first, and each call appears as a compact block you can expand with `Ctrl+O`. `Esc` aborts a
run and puts whatever you typed during it back in the editor.

To sign in interactively instead, type `/login` inside the session. Seven providers support OAuth:
`anthropic`, `kimi-coding`, `xai`, `openrouter`, `radius`, `github-copilot` and `openai-codex`. The
rest take an API key. Credentials are saved to `~/.cyrup/agent/auth.json` at mode `0600`.

Installation itself writes only the binary. The agent directory appears the first time you log in,
change a setting, or answer a trust prompt.

### From an editor instead

The terminal interface is one front-end. `cyrup --acp` serves the
[Agent Client Protocol](https://agentclientprotocol.com) on stdio, so an editor that speaks ACP can
drive the same agent — same tools, same permission prompts, same session files — inside its own UI.
For Zed, add cyrup to `agent_servers` in `settings.json`:

```json
{
  "agent_servers": {
    "cyrup": { "command": "/usr/local/bin/cyrup", "args": ["--acp"], "env": {} }
  }
}
```

Then pick **External Agent → cyrup** in the agent panel. The editor launches the process and owns
both pipes, so this is not a command you run yourself.
[Zed and other ACP editors](docs/guide/guides/zed-acp.md) covers credentials, thinking levels and
the command palette.

## What you get

- **40 built-in providers** over 10 wire APIs, with 35 embedded model catalogs. Anthropic, OpenAI,
  Google, Bedrock, Vertex, Copilot, OpenRouter, Groq, Together, Mistral, Fireworks and more.
- **The built-in tool set**: `read`, `bash`, `powershell`, `edit`, `write`, `grep`, `find`, `ls`, over
  an `FsOps`/`ProcOps` interface that tests substitute.
- **Codemode**, an optional tool (off by default) with which the model writes one JavaScript script that
  calls the other tools, in parallel if it likes, and only the script's output comes back. Each script
  runs in a V8 sandbox in a process of its own, and every call it makes passes the same permission gate
  as any other. Turn it on with `"defaultTools": ["+codemode"]` or
  `--tools read,bash,edit,write,codemode`; see [Codemode](docs/guide/guides/codemode.md).
- **A session tree on disk** as JSONL, with compaction, forking, import and export.
- **Five run modes.** The terminal interface, plus `--mode print`, `--mode json` and `--mode rpc`
  for scripting and embedding, and `--mode acp` to run as an
  [editor's agent](docs/guide/guides/zed-acp.md). The terminal interface runs in the
  alternate-screen renderer by default — mouse capture, a scrollbar, text selection and image
  support; `--tui-mode regular` (or `"tuiMode": "regular"`) keeps the terminal's normal scrollback.
- **Subagent delegation** in the foreground or background, as single runs, chains, parallel fans
  or `workflowScript` plans, where the agent writes its orchestration once as JavaScript and the plan
  runs without further inference. Background runs survive a restart via a session index and durable
  replay, can be steered, detached, waited on (`bg_wait`) and inspected, and are watched by a child
  watchdog with model-fallback chains. Children can be placed on a herdr saved machine. All 18 of
  upstream's `/subagents-*` slash commands are present, and a fleet roster in the TUI lets you pick a
  running child with the keyboard and open its inspector.
- **A runtime permission gate** over every tool call, a **Unix-socket broker** for
  supervisor-to-subagent coordination, and the **Flux** development pipeline.
- **MCP over stdio and OAuth-protected HTTP**, with sampling, elicitation and a JSON-RPC wire tracer.

## herdr

Run cyrup inside [herdr](https://github.com/herdrdev/herdr) and the herdr sidebar shows which pane is
blocked waiting on you: permission and MCP dialogs raise the signal, and it is released on exit so
no phantom `working` row is left behind. An agent can open, drive, inspect and close inspector and
project panes through seven tool verbs, and subagents can run on a herdr saved machine, with events
and exit relayed back to the supervisor. Without herdr installed, every herdr surface reports that
it is unavailable and does nothing else.

## Extensions are WebAssembly components

Pi loads TypeScript at runtime. cyrup runs extensions as WASM components under Wasmtime, against a
versioned WIT world (`cyrup:ext@0.20.0`). That buys three things a dynamic-import model cannot.

**A real sandbox.** An extension declares what it needs in its manifest: filesystem roots, process
execution, network, UI. The host enforces it. A component that declares nothing gets nothing, and
cannot read a file it did not ask for, because the capability was never handed to it. Enforcement is
host-side, so a buggy or hostile guest cannot opt itself back in.

**A typed contract that breaks loudly.** The WIT world is the API. Change it and the version moves,
so a guest built against an older world fails a version check instead of throwing a `TypeError`
three seconds into a turn. Both copies of the world are byte-identical by test, and the ABI
fingerprint invalidates the build when it moves.

**Any language that targets components.** The in-tree guest SDK (`cyrup-ext-sdk`) is Rust and
compiles to `wasm32-wasip2`, but the boundary is WIT, not Rust.

The constraint that comes with it: values cross the boundary as data, not references. Where Pi hands
an extension a live object, cyrup passes a serialized value or an explicit round-trip, which shapes
what an extension API can look like.

Five larger subsystems ship as native built-in extensions rather than WASM, because they supervise
OS processes, own Unix sockets, or speak JSON-RPC to child processes: subagent delegation, the
permission gate, the intercom broker, the MCP client, and Flux. The first three are default-off and
arm on their own environment flag (`CYRUP_SUBAGENTS`, `CYRUP_PERMISSION_SYSTEM`, `CYRUP_INTERCOM`)
or on the presence of a config file. Dropping a policy file into a repository is enough to arm the
permission gate. Flux and MCP attach unconditionally: Flux disables itself inside a subagent child
process, and MCP stays inert until it finds an `mcp.json`.

## Agent-authored workflows run on an embedded V8

The static `chain`/`parallel` subagent plan cannot branch on a result, and branching in the
orchestrator's own loop costs a full inference round-trip plus a context payload per branch.
`workflowScript` lets the agent write the plan once — plain JavaScript, top-level `await`, `runs.*`
— and the plan then executes with no further inference. It ships inside `cyrup-ext-subagents`, so it
arms with `CYRUP_SUBAGENTS` like the rest of that subsystem.

Upstream runs these scripts in a bare `vm.createContext({ runs, Promise, emit, console })`. cyrup
embeds V8 through `deno_core` and reproduces exactly that capability set: `runs`, `emit`, `console`
and every ECMAScript built-in are present; `setTimeout`, `fetch`, `require`, `Buffer`, `process`,
`TextEncoder`, `URL`, `crypto`, `eval` and `new Function` are not. The realm is assembled by the host
in one call before any agent script exists, so no script can observe a half-installed sandbox, and
the two optional globals the protocol allows — `state` and `runs.host` — are installed only when the
embedding host grants them.

Timers are excluded on purpose rather than by limitation — `deno_core` would supply them — because a
workflow's waits are waits on *work*, and every `runs.*` call blocks on a real event that appears in
the trace, on the live card, and is cancellable. Because the contract is a closed set, cyrup's
analyzer enforces it at `action:"validate"` time and rejects a script reaching for `setTimeout`
*before a child is spent*, where upstream discovers it as a runtime `ReferenceError` after the fact.

`cyrup-workflow-runtime` exists for one mechanical reason, recorded here because it looks arbitrary
otherwise: a Cargo build script is compiled and run before its own crate's `src/`, so it can never
import from the crate whose build it is running. The ops and `prelude.js` therefore live in a
dependency-free crate that `cyrup-ext-subagents/build.rs` takes as a `[build-dependencies]` entry,
snapshots, and `include_bytes!`s back as `RuntimeOptions.startup_snapshot`.

## Why Rust

The agent is a long-running process that supervises subprocesses, streams from network APIs, holds a
session tree in memory and repaints a terminal.

- **A single static binary.** No runtime, no `node_modules`, no version manager. It starts fast
  because there is nothing to warm up. The WASM and JavaScript engines are linked in, not installed
  beside it.
- **Predictable memory.** A session tree with thousands of entries and a long transcript stays flat,
  with no GC pause in the middle of a token stream.
- **Real concurrency.** Parallel tool calls, a streaming provider response, a terminal repaint and
  several subagent processes are separate tasks on a work-stealing runtime rather than callbacks on
  one loop.
- **Cancellation that stops the work.** Dropping a future actually cancels it. This is also the
  sharpest edge in the codebase, since cleanup has to survive a drop.
- **An enforced no-panic policy.** `unwrap`, `expect`, `panic!` and raw indexing are denied
  workspace-wide by clippy lints. An agent that crashes mid-turn loses your session.

## Following Pi closely

cyrup cites its upstream in the source: 37,016 citations pointing at the exact `.ts` file and line a
given Rust item mirrors, naming 21,726 distinct upstream locations
(`grep -rhoE '[A-Za-z0-9_./-]+\.ts:[0-9]+' crates --include='*.rs'`). That index is how equivalence
gets audited. `grep -rn "agent-loop.ts:226" crates` finds the code that answers for it.

Rust is not TypeScript, so where the languages differ cyrup ports the behaviour and records the
mechanism difference in a `CYRUP-DELTA` comment naming the upstream line and the reason. There are
1,498 of them (`grep -rhoE 'CYRUP-DELTA' crates --include='*.rs'`). For example:

- A JavaScript `async` function always settles. A Rust future can be dropped at any `.await`, so
  anything registered before an await and cleaned up only on the success path leaks forever. cyrup
  puts that cleanup in `Drop`.
- JavaScript has no locks, so a re-entered handler is an ordinary nested call. In Rust the same shape
  can re-take a held `tokio::Mutex` and hang with no deadlock detection.
- `tokio::select!` picks at random when both arms are ready. JavaScript cannot express that race, so
  cyrup marks those `biased;` to reproduce upstream's deterministic ordering.
- `{ ...tool, execute }` preserves every field by construction. A hand-written Rust trait impl must
  name each method, so cyrup's fixtures give every method a distinct non-default value, or a
  forgotten delegation is invisible to its own test.

## Workspace layout

Line counts are every line of every `.rs` file in the crate, comments and inline tests included,
measured at `4fa902b0`. Coverage comes from the workspace run above. `cyrup-it` is measured by running
it, not by coverage. Dependencies point downward only. `cyrup-core` depends on nothing in-workspace, and
`cyrup-session-svc` is the single integration point the front-ends consume.

| Crate | Lines | Line cov. | Role |
|-------|------:|------:|------|
| `cyrup-core` | 9,053 | 91.2% | shared substrate: ids, `Content`/`Message`, `EventStream<T>`, `CancelToken`, the `Tool` trait |
| `cyrup-provider` | 125,976 | 93.6% | vendor-neutral LLM layer: 40 providers over 10 wire APIs, 35 embedded catalogs, auth, streaming, images |
| `cyrup-agent` | 22,673 | 94.3% | the turn loop: tool execution, hooks, steering and follow-up queues, abort |
| `cyrup-tools` | 28,706 | 92.9% | built-in tools over an `FsOps`/`ProcOps` interface |
| `cyrup-session` | 22,062 | 92.0% | JSONL session tree, compaction, system-prompt and context assembly |
| `cyrup-config` | 30,330 | 93.3% | layered settings, project trust, auth store, model resolution |
| `cyrup-ext` | 52,867 | 77.7% | WASM Component Model host (Wasmtime) plus the native built-in tier |
| `cyrup-resources` | 15,417 | 91.0% | skills, prompt templates, themes, packages |
| `cyrup-tui` | 166,306 | 86.1% | ratatui + crossterm front-end, regular and alternate-screen modes |
| `cyrup-modes` | 10,799 | 72.1% | print / json / rpc adapters, and an RPC client |
| `cyrup-sdk` | 944 | 0.0% | public embeddable API |
| `cyrup-session-svc` | 90,012 | 91.6% | the `AgentSession` facade wiring everything together |
| `cyrup` | 28,221 | 74.7% | the CLI binary |
| `cyrup-ext-subagents` | 451,714 | 91.2% | OS-subprocess subagent delegation and the `workflowScript` runtime (the largest crate) |
| `cyrup-workflow-runtime` | 322 | 63.9% | `workflowScript`'s `deno_core` ops and `prelude.js`, split out so a consumer's `build.rs` can snapshot them |
| `cyrup-permission-system` | 23,903 | 85.8% | runtime allow / ask / deny policy over every tool call |
| `cyrup-intercom` | 46,883 | 79.3% | Unix-socket broker for supervisor-to-subagent coordination |
| `cyrup-flux` | 5,275 | 98.0% | the Flux structured development pipeline |
| `cyrup-herdr` | 13,797 | 86.7% | the one client for herdr's NDJSON socket API: typed methods, event subscription, reconnect, the pane status bridge |
| `cyrup-mcp` | 93,340 | 84.8% | MCP client: servers, tools, OAuth, sampling, elicitation, wire tracer, the `/mcp` surface |
| `cyrup-acp` | 21,539 | 94.1% | Agent Client Protocol adapter: an editor (Zed is the reference client) drives cyrup over ACP JSON-RPC on stdio |
| `cyrup-llama` | 21,654 | 96.5% | llama.cpp router support: provider, model discovery, Hugging Face search and the `/llama` manager, as a native extension |
| `cyrup-codemode` | 5,005 | 98.8% | codemode's engine-independent half: script source format, TypeScript declaration renderer, BM25 tool ranker, output budget |
| `cyrup-codemode-runtime` | 19,203 | 92.0% | codemode's engine-bound half: the `deno_core` sandbox, the `codemode` tool and its script globals |
| `cyrup-tool-search` | 940 | 98.5% | pi's `tool_search`: finds codemode and deferred tools by BM25 and loads the matches |
| `cyrup-pico` | 12,710 | 76.1% | Pico5 durability kernel, imperative shell: the mutation line, the transaction, drafts, the durability witnesses |
| `cyrup-pico-doc` | 4,932 | 76.6% | Pico5's functional core: `DocValue`, the operation applier, the replay planner — total functions over explicit inputs |
| `cyrup-pico-store` | 8,806 | 81.0% | Pico5's storage layer: identity, sequence and value newtypes, and the storage contract |
| `cyrup-pico-store-jsonl` | 8,966 | 78.4% | Pico5's JSONL backend: store lock, marker protocol, recovery |
| `cyrup-ext-sdk` | 15,559 | 45.7% | guest SDK for authoring extensions (`wasm32-wasip2`) |
| `cyrup-test-support` | 3,822 | 87.0% | faux provider plus differential, interop and golden harnesses |
| `cyrup-llama-cpp-wire` | 1,077 | 100.0% | test-only: the llama.cpp `llama-server` wire as one cited definition the workspace's fake servers answer from |
| `cyrup-it` | 79,124 | — | the gated integration-test harness |

## Documentation

The user guide lives under [`docs/guide/`](docs/guide/introduction.md) as an
[mdBook](https://rust-lang.github.io/mdBook/):

```sh
cargo install mdbook
mdbook serve   # http://localhost:3000, live-reloads on save
```

- [Install](docs/guide/getting-started/install.md) · [Connect a provider](docs/guide/getting-started/authenticate.md) · [Your first session](docs/guide/getting-started/first-session.md)
- [The terminal interface](docs/guide/guides/tui.md) · [sessions](docs/guide/guides/sessions.md) · [models and thinking](docs/guide/guides/models.md) · [tools and permissions](docs/guide/guides/tools-and-permissions.md) · [codemode](docs/guide/guides/codemode.md)
- [Scripting and automation](docs/guide/guides/scripting.md) · [Zed and other ACP editors](docs/guide/guides/zed-acp.md)
- [How extensions work](docs/guide/extensions/overview.md) · [subagents](docs/guide/extensions/subagents.md) · [permissions](docs/guide/extensions/permissions.md) · [intercom](docs/guide/extensions/intercom.md) · [Flux](docs/guide/extensions/flux.md)
- [CLI reference](docs/guide/reference/cli.md) · [`settings.json`](docs/guide/reference/settings.md) · [environment variables](docs/guide/reference/environment.md) · [keybindings](docs/guide/reference/keybindings.md) · [troubleshooting](docs/guide/reference/troubleshooting.md)

The guide has no MCP chapter and no `workflowScript` chapter yet. Until they land,
`docs/gap-analysis/13-cyrup-mcp.md` and the module docs in `crates/cyrup-mcp/src/` are the reference
for MCP, and `crates/cyrup-ext-subagents/src/workflows/scripted/mod.rs` is the reference for
workflows.

## Building and testing

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p cyrup-ext-sdk --target wasm32-wasip2   # --workspace does not reach the guest SDK
cargo clippy -p cyrup-it --features it --all-targets   # nor the gated harness
cargo nextest run --workspace --features test-fixtures # 16,225 tests, 12 skipped
cargo run -p xtask -- feature-matrix                   # non-default feature combos, and runs the integration suite
cargo doc --workspace --no-deps                        # every lib and bin; rustdoc links are denied
```

Run clippy. The no-panic policy is expressed as `[workspace.lints.clippy]` denials, and clippy tool
lints do not fire under `cargo build` or `cargo test`. There is no CI in this repository, so nothing
runs these for you. All three clippy surfaces are at zero findings; the three commands above are
three different surfaces, because `--workspace` reaches neither `cyrup-ext-sdk` (it compiles to
`wasm32-wasip2`) nor `cyrup-it` (it is behind `required-features`). The gated-harness one needs
`--all-targets`: without it, `cyrup-it`'s dependency-free lib is the only thing linted and its nine
test binaries are never built.

The commands above build one point in the feature space. Nine crates declare `[features]`, and
`feature-matrix` builds the rest: the `#[cfg(not(feature = "wasm-host"))]` arms of `cyrup-ext` and
`cyrup-session-svc`, every `impl Backend` with `ratatui/scrolling-regions` off, `cyrup-tools` without
`inline-images`, the `faux` and `test-fixtures` arms, and the guest SDK for `wasm32-wasip2`. Each row
states the obligation it discharges and prints it on failure.

Edition 2024, `resolver = "3"`, stable toolchain.

### Measuring coverage

```sh
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov --locked
eval "$(cargo llvm-cov show-env --export-prefix)"
export LLVM_PROFILE_FILE="$PWD/target/cyrup-%8m.profraw"
cargo llvm-cov clean --workspace
cargo nextest run --workspace --features test-fixtures
cargo llvm-cov report --summary-only
```

Do not use `cargo llvm-cov nextest` directly on this workspace. Nextest runs each test in its own
process and the default profile pattern includes `%p`, so it writes one raw profile per test. That
comes to 11,000 files of a multi-megabyte binary's counters, and it filled a 28 GB disk before the
run finished. `%8m` without `%p` merges the counters into eight pooled files per binary. The
instrumented build takes about 11 GB.

### The integration suite

Almost all tests are unit tests inline under `crates/*/src/`. The heavy ones, which spawn the binary,
drive a subagent, load a WASM component, open a broker socket or talk to a real MCP server child
process, live in one gated crate:

```sh
cargo build -p cyrup-ext-sdk --target wasm32-wasip2
cargo nextest run -p cyrup-it --features it        # ~860 test functions across 11 binaries
```

`cyrup-it` is behind `required-features = ["it"]`, so the everyday gate never builds it. Its
`build.rs` resolves the fixture binaries and the WASM component once. It carries eleven targets, one
per subsystem (`subagents`, `verify_redaction`, `intercom`, `ext`, `permission`, `mcp`,
`session_svc`, `bin`, `misc`, `llama`, `llama_live`), so a `process::exit`, an abort or a segfault in
one cannot take the others down with no report. `llama_live` drives a real `llama-server` and
reports each of its tests as skipped, on stderr, unless `LLAMA_SERVER_BIN` and
`CYRUP_LLAMA_MODELS_DIR` are both set.

Set `CYRUP_IT_BIN_DIR` to a directory of pre-built binaries and `build.rs` skips its nested second
link. On a constrained disk that is the difference between the suite running and the nested build
failing on disk space (the nested build is a second full debug build of the `cyrup` graph, about
9 GB); pointed at an empty directory it skips the link entirely, which is enough to type-check the
crate. Type-checking does not exercise it; run it before trusting a change to one of those
subsystems. The directory must hold the binaries built with the features the tests need, so not a
bare `cargo build --workspace --bins`:

```sh
cargo build -p cyrup --features faux --bin cyrup                 # `faux` is what --model faux/faux-1 reaches
cargo build -p cyrup-intercom --features test-fixtures --bins    # broker, child fixture, scripting client
cargo build -p cyrup-ext-subagents --features test-fixtures --bins
cargo build -p cyrup-ext-sdk --target wasm32-wasip2
export CYRUP_IT_BIN_DIR="$PWD/target/debug"
export CYRUP_EXT_FIXTURE_COMPONENT="$PWD/target/wasm32-wasip2/debug/cyrup_ext_sdk.wasm"
cargo run -p xtask -- it                       # nextest, in a credential-free environment
cargo run -p xtask -- it --test bin            # one target (`--test <name>` is passed to nextest)
cargo clippy -p cyrup-it --features it,wasm-host --all-targets -- -D warnings
```

Run it through `cargo run -p xtask -- it` rather than raw: the suite refuses to start in a shell that
exports provider credentials (`AWS_ACCESS_KEY_ID`, `ANTHROPIC_API_KEY`, ...), and the wrapper clears
them.

Thirty-four integration binaries remain in-crate under `crates/*/tests/`, each because it needs a
process of its own: it mutates the process environment, spawns the shipped `cyrup` binary, asserts a
compile failure, or pins a whole-crate wiring proof next to the crate it proves.

Two conventions:

- Tests must not hit real provider APIs or paid tokens. Offline coverage comes from a faux provider
  with a seeded PRNG so snapshots reproduce.
- The integration suite fails if ambient credentials leak in. If you have `TOGETHER_API_KEY` or the
  `CYRUP_*` feature flags exported, scrub them before running.

## Parity with upstream

Fidelity to Pi is tracked in the open. [`docs/gap-analysis/`](docs/gap-analysis/README.md) carries a
per-area ledger of how each subsystem maps onto its upstream, area by area, with both sides cited —
the Rust at a named commit and the TypeScript at a named tag. Every divergence the languages force
is recorded as a `CYRUP-DELTA` naming the upstream line and the reason.

[`REPRO-LOG.md`](docs/gap-analysis/REPRO-LOG.md) records what happened when the binary was actually
built, launched and driven, through a real pty where the surface needed one.
[`ADR-0006`](docs/adr/ADR-0006-upstream-chase-cadence.md) records the cadence for chasing upstream
tags.

MCP is the largest piece in flight. A model calls a real server's tools end to end — stdio and
OAuth-protected HTTP, sampling, elicitation, a JSON-RPC wire tracer and the `/mcp` surface — and the
port is enumerated unit by unit in `docs/gap-analysis/13*` against `pi-mcp-adapter`, with four
upstream surfaces cut by owner decision: the legacy HTTP+SSE transport, MCP Apps, the raw
unix-socket transport, and `mcpScript`.

## Upstreams

cyrup tracks eight upstream projects: six TypeScript, one Python and one Rust. The core is
[`earendil-works/pi`](https://github.com/earendil-works/pi). Five optional subsystems follow
standalone Pi extensions, since Pi core ships no permission system, no MCP client and no editor
protocol of its own. Flux follows `code_puppy_core_plugins`, which is Python. `cyrup-herdr` is a
client for herdr's socket API rather than a port, pinned to the version whose schema it was checked
against.

Each row records the upstream a subsystem follows, the newest upstream tag, and the point the
[parity ledger](docs/gap-analysis/README.md) was last measured against. The ledger was re-pinned on
2026-10-09 against cyrup `6b14575`, at pi v1.1.0+11. Four of the pins are untagged commits, chosen
so that fixes landed after each release are in scope; see
[Baselines measured against](docs/gap-analysis/README.md#baselines-measured-against).

| upstream | followed by | newest tag | ledger measured to |
|---|---|---|---|
| `earendil-works/pi` | most crates | v1.1.0 | `f1b2e77f5` = v1.1.0+11 |
| `nicobailon/pi-subagents` | `cyrup-ext-subagents` | v0.76.1 | `ad11b7ab` = v0.76.1+29 (ported surface ≈ v0.68.0) |
| `MasuRii/pi-permission-system` | `cyrup-permission-system` | v0.8.0 | v0.8.0 |
| `nicobailon/pi-intercom` | `cyrup-intercom` | v0.16.1 | v0.16.1 |
| `nicobailon/pi-mcp-adapter` | `cyrup-mcp` | v5.1.0 | `2ccf648` = v5.1.0+3 |
| `code_puppy_core_plugins` (Python) | `cyrup-flux` | v0.0.76 | v0.0.62 — 39 commits unswept, a stated blind spot |
| `svkozak/pi-acp` | `cyrup-acp` | v0.0.34 | `04d0a15` = v0.0.34+3 |
| [`herdrdev/herdr`](https://github.com/herdrdev/herdr) (Rust) | `cyrup-herdr` (a client, not a port) | v0.9.3 | v0.9.3, checked for client conformance |

"Measured to" means the newest window was read and its drift filed as ledger items. It does not
mean that drift is closed. `cyrup-permission-system` is fully caught up with its upstream. Flux's
ported surface (`flux_bootstrap/`) is byte-identical at every tag from `v0.0.6` through `v0.0.62`,
so its version span carries no behavioural difference at all.

`cyrup-acp` is the newest port. It speaks the
[Agent Client Protocol](https://agentclientprotocol.com) over stdio so an editor — Zed is the
reference client — can drive cyrup the way it drives any other ACP agent: `initialize`,
`session/new`, `session/prompt` and the rest, with the turn streamed back as `session/update`
notifications and `session/request_permission` requests. It is a **fourth front-end** beside the
TUI, `--mode rpc` and print/json, not a new agent.

[`docs/gap-analysis/15-cyrup-acp.md`](docs/gap-analysis/15-cyrup-acp.md) records the decision that
shapes everything else: pi-acp *must* spawn `pi --mode rpc` as a child and scrape untyped NDJSON off
its stdout, because it is a separate npm package; `cyrup-acp` is a workspace crate and binds to
`AgentSession` in-process instead, which deletes the whole subprocess surface and replaces
`Record<string, unknown>` event probing with the typed `AgentSessionEvent`.

Clone the seven Pi-family upstreams under `./tmp/` (gitignored) before working a ledger row;
`.claude/hooks/session-start.sh` does it for you. The area files cite them as
`git -C tmp/<repo> show <tag>:<path>`, at a named tag rather than from a working tree, so a citation
still resolves months later. [`docs/gap-analysis/README.md`](docs/gap-analysis/README.md) records
the exact commit and tag every area was measured against.

Where cyrup and an upstream disagree, the upstream is correct and cyrup is what changes. Every item
in the ledger is adjudicated that way, which is why a divergence has to be recorded as a
`CYRUP-DELTA` with a reason. [`ACKNOWLEDGEMENTS.md`](ACKNOWLEDGEMENTS.md) describes what each project
contributed.

## License

MIT, see [`LICENSE`](LICENSE), which also carries the upstream copyright notices.
