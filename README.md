# cyrup

**cyrup** · /ˈsɪr.əp/ · *SIR-up* — rhymes with **syrup**, as in maple syrup.

A coding agent in Rust. One static binary, 39 model providers, and extensions that run as sandboxed
WebAssembly components.

cyrup follows the design of the [Pi](https://github.com/earendil-works/pi) agent harness: a minimal
core, everything-is-an-extension, an agent that can extend itself. It rebuilds that design on a Rust
backbone.

> **Status:** pre-release, and not yet versioned. The agent loop, provider layer, tool set, session
> tree, terminal interface, extension host, all four run modes, the MCP client and the Flux
> development pipeline work end to end. 21 crates, ~786k lines of Rust, 8,710 workspace tests and
> 492 integration tests passing — those four figures were measured at code HEAD `6cf2cb9f`; the
> batch since then added tests across nine crates and neither total has been re-derived.

## Install

You need a stable Rust toolchain, 1.96 or newer. Nothing else: no Node, no Python, no runtime
alongside the binary.

```sh
cargo install --git https://github.com/cyrup-ai/cyrup cyrup
```

The first build takes several minutes, because it compiles a WebAssembly runtime, a git
implementation and a TLS stack from source. Later builds reuse the cache.

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

To sign in interactively instead, type `/login` inside the session. Six providers support OAuth:
`anthropic`, `kimi-coding`, `xai`, `openrouter`, `github-copilot` and `openai-codex`. The rest take
an API key. Credentials are saved to `~/.cyrup/agent/auth.json` at mode `0600`.

Installation itself writes only the binary. The agent directory appears the first time you log in,
change a setting, or answer a trust prompt.

## What you get

- **39 built-in providers** over 10 wire APIs, with 35 embedded model catalogs. Anthropic, OpenAI,
  Google, Bedrock, Vertex, Copilot, OpenRouter, Groq, Together, Mistral, Fireworks and more.
- **The built-in tool set**: `read`, `write`, `edit`, `bash`, `grep`, `find`, `ls`, over an
  `FsOps`/`ProcOps` interface that tests substitute.
- **A session tree on disk** as JSONL, with compaction, forking, import and export.
- **Four run modes.** The terminal interface, plus `--mode print`, `--mode json` and `--mode rpc`
  for scripting and embedding. `--tui-mode fullscreen` switches to an alternate-screen renderer with
  mouse capture, a scrollbar, text selection and image support.
- **Subagent delegation**, a runtime permission gate over every tool call, a Unix-socket broker for
  supervisor-to-subagent coordination, an MCP client, and the Flux development pipeline.
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

## Why Rust

The agent is a long-running process that supervises subprocesses, streams from network APIs, holds a
session tree in memory and repaints a terminal.

- **A single static binary.** No runtime, no `node_modules`, no version manager. It starts fast
  because there is nothing to warm up.
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

cyrup cites its upstream in the source: 23,901 citations pointing at the exact `.ts` file and line a
given Rust item mirrors. That index is how equivalence gets audited. `grep -rn "agent-loop.ts:226"
crates` finds the code that answers for it.

Rust is not TypeScript, so where the languages differ cyrup ports the behaviour and records the
mechanism difference in a `CYRUP-DELTA` comment naming the upstream line and the reason. There are
506 of them. For example:

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
| `cyrup-provider` | vendor-neutral LLM layer: 39 providers over 10 wire APIs, 35 embedded catalogs, auth, streaming, images |
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
| `cyrup-ext-subagents` | OS-subprocess subagent delegation (the largest crate) |
| `cyrup-permission-system` | runtime allow / ask / deny policy over every tool call |
| `cyrup-intercom` | Unix-socket broker for supervisor-to-subagent coordination |
| `cyrup-flux` | the Flux structured development pipeline |
| `cyrup-mcp` | MCP client: servers, tools, OAuth, sampling, elicitation, wire tracer, the `/mcp` surface |
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
- [How extensions work](docs/guide/extensions/overview.md) · [subagents](docs/guide/extensions/subagents.md) · [permissions](docs/guide/extensions/permissions.md) · [intercom](docs/guide/extensions/intercom.md) · [Flux](docs/guide/extensions/flux.md)
- [CLI reference](docs/guide/reference/cli.md) · [`settings.json`](docs/guide/reference/settings.md) · [environment variables](docs/guide/reference/environment.md) · [keybindings](docs/guide/reference/keybindings.md) · [troubleshooting](docs/guide/reference/troubleshooting.md)

The guide has no MCP chapter yet. Until it lands, `docs/gap-analysis/13-cyrup-mcp.md` and the module
docs in `crates/cyrup-mcp/src/` are the reference.

## Building and testing

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p cyrup-ext-sdk --target wasm32-wasip2   # --workspace does not reach the guest SDK
cargo clippy -p cyrup-it --features it                 # nor the gated harness
cargo nextest run --workspace                          # 8,710 tests, 9 skipped
cargo run -p xtask -- feature-matrix                   # non-default feature combos, and runs the integration suite
cargo doc --workspace --no-deps --bins                 # rustdoc links are denied, not warned
```

Run clippy. The no-panic policy is expressed as `[workspace.lints.clippy]` denials, and clippy tool
lints do not fire under `cargo build` or `cargo test`. There is no CI in this repository, so nothing
runs these for you. All three clippy surfaces are at zero findings; the three commands above are
three different surfaces, because `--workspace` reaches neither `cyrup-ext-sdk` (it compiles to
`wasm32-wasip2`) nor `cyrup-it` (it is behind `required-features`).

Deny-level lints are hard errors, so a crate carrying one fails to compile and every crate depending
on it is never linted at all. Clearing the first 9 errors let clippy reach the rest of the graph and
surfaced 9 further findings, plus three integration-test failures that had been invisible for the
same reason.

The commands above build one point in the feature space. Nine crates declare `[features]`, and
`feature-matrix` builds the rest: the `#[cfg(not(feature = "wasm-host"))]` arms of `cyrup-ext` and
`cyrup-session-svc`, every `impl Backend` with `ratatui/scrolling-regions` off, `cyrup-tools` without
`inline-images`, the `faux` and `test-fixtures` arms, and the guest SDK for `wasm32-wasip2`. Each row
states the obligation it discharges and prints it on failure. Two rows are easy to over-read on a
green run and say so in their own text: the `cyrup-session-svc --no-default-features` row compiles
that crate's native arms but does not produce a wasmtime-free build, and the workspace-wide
`--no-default-features` row today resolves to the same graph as the everyday gate, because every
in-workspace dependency edge asks for its dependency's default features.

Edition 2024, `resolver = "3"`, stable toolchain.

### The integration suite

Almost all tests are unit tests inline under `crates/*/src/`. The heavy ones, which spawn the binary,
drive a subagent, load a WASM component, open a broker socket or talk to a real MCP server child
process, live in one gated crate:

```sh
cargo build -p cyrup-ext-sdk --target wasm32-wasip2
cargo nextest run -p cyrup-it --features it        # 492 tests across 9 binaries
```

`cyrup-it` is behind `required-features = ["it"]`, so the everyday gate never builds it. Its
`build.rs` resolves the fixture binaries and the WASM component once. It carries nine targets, one
per subsystem (`subagents`, `verify_redaction`, `intercom`, `ext`, `permission`, `mcp`,
`session_svc`, `bin`, `misc`), so a `process::exit`, an abort or a segfault in one cannot take the
others down with no report.

Set `CYRUP_IT_BIN_DIR` to a directory of pre-built binaries and `build.rs` skips its nested second
link. On a constrained disk that is the difference between the suite running and the nested build
failing on disk space. Type-checking this crate does not exercise it; run it before trusting a
change to one of those subsystems.

Sixteen integration binaries remain in-crate under `crates/*/tests/`, each because it needs a process
of its own: it mutates the process environment, spawns the shipped `cyrup` binary, or pins a
whole-crate wiring proof next to the crate it proves.

Two conventions:

- Tests must not hit real provider APIs or paid tokens. Offline coverage comes from a faux provider
  with a seeded PRNG so snapshots reproduce.
- The integration suite fails if ambient credentials leak in. If you have `TOGETHER_API_KEY` or the
  `CYRUP_*` feature flags exported, scrub them before running.

## Parity with upstream

Behavioural differences from Pi are tracked in the open, in
[`docs/gap-analysis/`](docs/gap-analysis/README.md), so an unported feature is not mistaken for a
bug. 90 rows are open across the area files: no `critical`, no `high`, 14 `medium`, 76 `low`, with
584 closed. `docs/gap-analysis/scripts/count_open_items.py` produces those counts, measured at code
HEAD `79b0a578`.

**Nothing is open above `medium`**, which is a first for this ledger. `SUBA-074` — an agent's
`runner:` frontmatter naming an external CLI to run the child under — was the last such row, and the
external-CLI runner, its capability contract and the `claude-code` adapter are now ported; the
`codex-exec` and `cursor-agent` adapters and the `external-job` protocol are refused by name rather
than silently ignored. Read that as "no row currently carries a `critical` or `high` severity",
not as "nothing serious is left": whatever is open is a floor rather than a total, six of the
fifteen open `medium` rows were filed in the last two days, and the set above `medium` has turned
over completely inside a single pass before.

The ledger is mostly a static analysis. Items are evidenced by reading both sources rather than by
running anything, its measured error rate has run near 12%, and it lags the code. Treat an entry as a
lead to verify. Items that have been observed against a running binary are marked in
[`REPRO-LOG.md`](docs/gap-analysis/REPRO-LOG.md).

MCP is the largest piece still in flight. A model calls a real server's tools end to end, and the
port is enumerated in `docs/gap-analysis/13*` against `pi-mcp-adapter`, with four upstream surfaces
cut by owner decision. That census has now been re-derived against `v2.32.1`: **244 of 437 units
implemented, 166 open**, plus 40 units filed for the `v2.26.1..v2.32.1` delta (147 files,
+16,014 / −1,001 across 72 commits). The counted figure is a floor, not an answer — 159 open rows
were not re-opened, and the extrapolation over them carries a wide interval, so do not quote it as a
count. The 13 `TODO(MCP-NNN)` markers in `crates/cyrup-mcp/src` are likewise a floor: six further ids
are open with no marker. Area 13 is counted separately from the table above because it plans code
that does not exist yet rather than measuring drift in code that does.

## Upstreams

cyrup tracks six upstream projects, five TypeScript and one Python. The core is
[`earendil-works/pi`](https://github.com/earendil-works/pi). Four optional subsystems follow
standalone Pi extensions, since Pi core ships no permission system and no MCP client of its own.
Flux follows `code_puppy_core_plugins`, which is Python.

| upstream | followed by | ported baseline | latest tag |
|---|---|---|---|
| `earendil-works/pi` | most crates | v0.83.0 | v0.84.4 |
| `nicobailon/pi-subagents` | `cyrup-ext-subagents` | ~v0.43.0 (the crate records no version string) | v0.64.0 |
| `MasuRii/pi-permission-system` | `cyrup-permission-system` | v0.7.1 | v0.8.0, fully caught up |
| `nicobailon/pi-intercom` | `cyrup-intercom` | v0.9.2 | v0.13.0 |
| `nicobailon/pi-mcp-adapter` | `cyrup-mcp` | v2.26.1 | v2.32.1 |
| `code_puppy_core_plugins` (Python) | `cyrup-flux` | v0.0.6 | v0.0.40 |

The Flux row's version gap is not a behaviour gap: the ported surface (`flux_bootstrap/`) is
byte-identical across all 34 intervening tags.

Clone all six under `./tmp/` (gitignored) before working a ledger row. The area files cite them as
`git -C tmp/<repo> show <tag>:<path>`, and a working tree's line numbers will mislead you.
[`docs/gap-analysis/README.md`](docs/gap-analysis/README.md) records the exact commits each pass
measured against. Re-measure the latest-tag column with `git ls-remote --tags` rather than trusting
it; this table has been wrong in both directions, and a wrong baseline reclassifies in-baseline port
bugs as version lag. [`ADR-0006`](docs/adr/ADR-0006-upstream-chase-cadence.md) records the cadence
and where the resulting items live.

Where cyrup and an upstream disagree, the upstream is correct and cyrup is what changes. Every item
in the ledger is adjudicated that way, which is why a divergence has to be recorded as a
`CYRUP-DELTA` with a reason. [`ACKNOWLEDGEMENTS.md`](ACKNOWLEDGEMENTS.md) describes what each project
contributed.

## License

MIT, see [`LICENSE`](LICENSE), which also carries the upstream copyright notices.
