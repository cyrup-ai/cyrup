# cyrup

**cyrup** · /ˈsɪr.əp/ · *SIR-up* — rhymes with **syrup**, as in maple syrup.

A coding agent in Rust. One static binary, 40 model providers, and extensions that run as sandboxed
WebAssembly components.

cyrup follows the design of the [Pi](https://github.com/earendil-works/pi) agent harness: a minimal
core, everything-is-an-extension, an agent that can extend itself. It rebuilds that design on a Rust
backbone.

> **Status:** pre-release, and not yet versioned. The agent loop, provider layer, tool set, session
> tree, terminal interface, extension host, all five run modes, the MCP client, the ACP adapter, the
> `workflowScript` runtime and the Flux development pipeline work end to end. 23 crates, 885,555
> lines of Rust and 9,886 workspace tests passing — measured at `9aeba769`
> (`cargo nextest run --workspace --features test-fixtures`, 0 failed, 9 skipped; `test-fixtures` is
> required or the two subagent-subprocess fixture binaries never build and their tests silently do
> not run). `cyrup-it`, the gated integration crate, adds 523 more across nine binaries.

## Install

You need a stable Rust toolchain, 1.96 or newer. Nothing else: no Node, no Python, no runtime
alongside the binary.

```sh
cargo install --git https://github.com/cyrup-ai/cyrup cyrup
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
- **A session tree on disk** as JSONL, with compaction, forking, import and export.
- **Five run modes.** The terminal interface, plus `--mode print`, `--mode json` and `--mode rpc`
  for scripting and embedding, and `--mode acp` to run as an
  [editor's agent](docs/guide/guides/zed-acp.md). `--tui-mode fullscreen` switches to an
  alternate-screen renderer with mouse capture, a scrollbar, text selection and image support.
- **Subagent delegation** — including `workflowScript`, where the agent writes its orchestration
  plan once as JavaScript and the plan runs without further inference — a runtime permission gate
  over every tool call, a Unix-socket broker for supervisor-to-subagent coordination, an MCP client,
  and the Flux development pipeline.
- **MCP over stdio and OAuth-protected HTTP**, with sampling, elicitation and a JSON-RPC wire tracer.

## Extensions are WebAssembly components

Pi loads TypeScript at runtime. cyrup runs extensions as WASM components under Wasmtime, against a
versioned WIT world (`cyrup:ext@0.10.0`). That buys three things a dynamic-import model cannot.

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

cyrup cites its upstream in the source: 25,892 citations pointing at the exact `.ts` file and line a
given Rust item mirrors, naming 15,209 distinct upstream locations
(`grep -rhoE '[A-Za-z0-9_./-]+\.ts:[0-9]+' crates --include='*.rs'`). That index is how equivalence
gets audited. `grep -rn "agent-loop.ts:226" crates` finds the code that answers for it.

Rust is not TypeScript, so where the languages differ cyrup ports the behaviour and records the
mechanism difference in a `CYRUP-DELTA` comment naming the upstream line and the reason. There are
748 of them. For example:

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

Dependencies point downward only. `cyrup-core` depends on nothing in-workspace, and
`cyrup-session-svc` is the single integration point the front-ends consume.

| Crate | Role |
|-------|------|
| `cyrup-core` | shared substrate: ids, `Content`/`Message`, `EventStream<T>`, `CancelToken`, the `Tool` trait |
| `cyrup-provider` | vendor-neutral LLM layer: 40 providers over 10 wire APIs, 35 embedded catalogs, auth, streaming, images |
| `cyrup-agent` | the turn loop: tool execution, hooks, steering and follow-up queues, abort |
| `cyrup-tools` | built-in tools over an `FsOps`/`ProcOps` interface |
| `cyrup-session` | JSONL session tree, compaction, system-prompt and context assembly |
| `cyrup-config` | layered settings, project trust, auth store, model resolution |
| `cyrup-ext` | WASM Component Model host (Wasmtime) plus the native built-in tier |
| `cyrup-resources` | skills, prompt templates, themes, packages |
| `cyrup-tui` | ratatui + crossterm front-end, regular and alternate-screen modes |
| `cyrup-modes` | print / json / rpc adapters, and an RPC client |
| `cyrup-sdk` | public embeddable API |
| `cyrup-session-svc` | the `AgentSession` facade wiring everything together |
| `cyrup` | the CLI binary |
| `cyrup-ext-subagents` | OS-subprocess subagent delegation and the `workflowScript` runtime (the largest crate) |
| `cyrup-workflow-runtime` | `workflowScript`'s `deno_core` ops and `prelude.js`, split out so a consumer's `build.rs` can snapshot them |
| `cyrup-permission-system` | runtime allow / ask / deny policy over every tool call |
| `cyrup-intercom` | Unix-socket broker for supervisor-to-subagent coordination |
| `cyrup-flux` | the Flux structured development pipeline |
| `cyrup-mcp` | MCP client: servers, tools, OAuth, sampling, elicitation, wire tracer, the `/mcp` surface |
| `cyrup-acp` | Agent Client Protocol adapter: an editor (Zed is the reference client) drives cyrup over ACP JSON-RPC on stdio |
| `cyrup-ext-sdk` | guest SDK for authoring extensions (`wasm32-wasip2`) |
| `cyrup-test-support` | faux provider plus differential, interop and golden harnesses |
| `cyrup-it` | the gated integration-test harness |

## Documentation

The user guide lives under [`docs/guide/`](docs/guide/introduction.md) as an
[mdBook](https://rust-lang.github.io/mdBook/):

```sh
cargo install mdbook
mdbook serve   # http://localhost:3000, live-reloads on save
```

- [Install](docs/guide/getting-started/install.md) · [Connect a provider](docs/guide/getting-started/authenticate.md) · [Your first session](docs/guide/getting-started/first-session.md)
- [The terminal interface](docs/guide/guides/tui.md) · [sessions](docs/guide/guides/sessions.md) · [models and thinking](docs/guide/guides/models.md) · [tools and permissions](docs/guide/guides/tools-and-permissions.md)
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
cargo nextest run --workspace --features test-fixtures # 9,886 tests, 9 skipped
cargo run -p xtask -- feature-matrix                   # non-default feature combos, and runs the integration suite
cargo doc --workspace --no-deps --bins                 # rustdoc links are denied, not warned
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

### The integration suite

Almost all tests are unit tests inline under `crates/*/src/`. The heavy ones, which spawn the binary,
drive a subagent, load a WASM component, open a broker socket or talk to a real MCP server child
process, live in one gated crate:

```sh
cargo build -p cyrup-ext-sdk --target wasm32-wasip2
cargo nextest run -p cyrup-it --features it        # 523 test functions across 9 binaries
```

`cyrup-it` is behind `required-features = ["it"]`, so the everyday gate never builds it. Its
`build.rs` resolves the fixture binaries and the WASM component once. It carries nine targets, one
per subsystem (`subagents`, `verify_redaction`, `intercom`, `ext`, `permission`, `mcp`,
`session_svc`, `bin`, `misc`), so a `process::exit`, an abort or a segfault in one cannot take the
others down with no report.

Set `CYRUP_IT_BIN_DIR` to a directory of pre-built binaries and `build.rs` skips its nested second
link. On a constrained disk that is the difference between the suite running and the nested build
failing on disk space; pointed at an empty directory it skips the link entirely, which is enough to
type-check the crate. Type-checking does not exercise it; run it before trusting a change to one of
those subsystems.

Eighteen integration binaries remain in-crate under `crates/*/tests/`, each because it needs a process
of its own: it mutates the process environment, spawns the shipped `cyrup` binary, or pins a
whole-crate wiring proof next to the crate it proves.

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

cyrup tracks seven upstream projects, six TypeScript and one Python. The core is
[`earendil-works/pi`](https://github.com/earendil-works/pi). Five optional subsystems follow
standalone Pi extensions, since Pi core ships no permission system, no MCP client and no editor
protocol of its own. Flux follows `code_puppy_core_plugins`, which is Python.

Each row records the upstream a subsystem follows and the newest tag tracked, re-checked with
`git ls-remote --tags`.

| upstream | followed by | latest tag tracked |
|---|---|---|
| `earendil-works/pi` | most crates | v0.85.1 |
| `nicobailon/pi-subagents` | `cyrup-ext-subagents` | v0.67.0 |
| `MasuRii/pi-permission-system` | `cyrup-permission-system` | v0.8.0 |
| `nicobailon/pi-intercom` | `cyrup-intercom` | v0.13.0 |
| `nicobailon/pi-mcp-adapter` | `cyrup-mcp` | v2.33.0 |
| `code_puppy_core_plugins` (Python) | `cyrup-flux` | v0.0.50 |
| `svkozak/pi-acp` | `cyrup-acp` | v0.0.33 |

`cyrup-permission-system` is fully caught up with its upstream. Flux's ported surface
(`flux_bootstrap/`) is byte-identical at every tag from `v0.0.6` through `v0.0.50`, so its version
span carries no behavioural difference at all.

The pi-acp row is the newest. `cyrup-acp` speaks the
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

Clone all seven under `./tmp/` (gitignored) before working a ledger row;
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
