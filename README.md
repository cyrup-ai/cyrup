# cyrup

**cyrup** · /ˈsɪr.əp/ · *SIR-up* — rhymes with **syrup**, as in maple syrup.

A coding agent in Rust. One static binary, 40 model providers, and extensions that run as sandboxed
WebAssembly components.

cyrup follows the design of the [Pi](https://github.com/earendil-works/pi) agent harness: a minimal
core, everything-is-an-extension, an agent that can extend itself. It rebuilds that design on a Rust
backbone.

> **Status:** pre-release, and not yet versioned. The agent loop, provider layer, tool set, session
> tree, terminal interface, extension host, all five run modes, the MCP client, the ACP adapter, the
> `workflowScript` runtime and the Flux development pipeline work end to end. 23 crates and 885,246
> lines of Rust under `crates/`.
>
> The workspace suite runs **9,884 tests: 9,882 passed, 2 failed, 9 skipped** — re-derived 2026-09-14
> at code HEAD `68facb9b` (`cargo nextest run --workspace --features test-fixtures --no-fail-fast`;
> `test-fixtures` is required or the two subagent-subprocess fixture binaries never build and their
> tests silently do not run). Both failures are
> `cyrup-ext-subagents workflows::host_command::tests::*`, reproducible rather than flaky, and both
> the same defect: `process_group_is_populated` answers upstream's `activeProcessGroupMembers` with
> one `kill(-pgid, 0)`, but `kill` succeeds on a **zombie**, so an orphan nothing reaps keeps the
> group looking populated and the cleanup ladder reports `verification-failed`. Upstream's `ps`
> scrape drops those rows (`owned-process-tree.ts`, `stat.startsWith("Z")`); the `CYRUP-DELTA` that
> replaced it claims the filter comes for free, and it does not. It is host-dependent — where an
> init or a job-control shell reaps orphans, the group really does empty.
>
> `cyrup-it`, the gated integration crate, carries 523 `#[test]`/`#[tokio::test]` functions across
> its nine binaries — a static count off the source, not a run of the suite.

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
- **The built-in tool set**: `read`, `write`, `edit`, `bash`, `grep`, `find`, `ls`, over an
  `FsOps`/`ProcOps` interface that tests substitute.
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
in one call before any agent script exists, so no script can observe a half-installed sandbox. Two
conditional globals — `state` and `runs.host` — are wired in the prelude but off in the host that
ships today, because the real `WorkflowScriptHost` implements only the two required trait methods and
leaves the optional capabilities at their refusing defaults.

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
cargo nextest run --workspace --features test-fixtures # 9,884 tests; 2 fail, see Status
cargo run -p xtask -- feature-matrix                   # non-default feature combos, and runs the integration suite
cargo doc --workspace --no-deps --bins                 # rustdoc links are denied, not warned
```

Run clippy. The no-panic policy is expressed as `[workspace.lints.clippy]` denials, and clippy tool
lints do not fire under `cargo build` or `cargo test`. There is no CI in this repository, so nothing
runs these for you. The three clippy commands above are three different surfaces, because
`--workspace` reaches neither `cyrup-ext-sdk` (it compiles to `wasm32-wasip2`) nor `cyrup-it` (it is
behind `required-features`).

**Measured 2026-09-14 at `68facb9b`, the gates are red, and it is worth knowing which way.**
`cargo fmt --all -- --check` reports diffs in 32 files, 22 of them under `cyrup-ext-subagents`.
`cargo clippy --workspace --all-targets -- -D warnings` stops on **one** finding —
`clippy::result_large_err` on `workflows/scripted/engine.rs`'s
`Result<WorkflowScriptResult, WorkflowScriptError>`, whose `Err` variant is at least 128 bytes. The
workspace suite has the two `host_command` failures named in **Status**, and the gated integration
crate does not compile (see **The integration suite**). All four are in the `workflowScript` batch;
nothing that predates it is red.

Deny-level lints are hard errors, so a crate carrying one fails to compile and every crate depending
on it is never linted at all — which is why one finding in `cyrup-ext-subagents` is not a cosmetic
red. Re-running with `-A clippy::result_large_err` reaches the whole graph and finds **nothing
else**, so this time the crates behind it were hiding no second wave; the last time it happened,
clearing the first 9 errors surfaced 9 further findings plus three integration-test failures that had
been invisible for the same reason. The guest-SDK surface is clean. The gated-harness surface needs
`--all-targets` to mean anything — without it `cyrup-it`'s dependency-free lib is all that gets
linted — and with it, that crate does not compile at all; see **The integration suite**.

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
cargo nextest run -p cyrup-it --features it        # 523 test functions across 9 binaries
```

**Measured 2026-09-14 at `68facb9b`, the `subagents` target of this crate does not compile.**
`ForegroundRunRequest` gained a `workflow_steer` field in the `workflowScript` batch and three
initializers in the harness were not updated —
`tests/subagents/foreground_progress_stream_integration.rs:132` and `:291`, and
`tests/subagents/startup_retry_lifecycle_integration.rs:514`. Those three are the only errors the run
reported, but the remaining eight targets were not independently confirmed: `build.rs`'s nested
second link wants ~15 GB and exhausted the disk on the retry, which is the constraint
`CYRUP_IT_BIN_DIR` below exists for. So the 523 figure is a count of source, not of tests that
currently run. This is the failure mode the crate's own gating creates: `--workspace` never builds
it, so nothing catches the drift until somebody runs the suite.

`cyrup-it` is behind `required-features = ["it"]`, so the everyday gate never builds it. Its
`build.rs` resolves the fixture binaries and the WASM component once. It carries nine targets, one
per subsystem (`subagents`, `verify_redaction`, `intercom`, `ext`, `permission`, `mcp`,
`session_svc`, `bin`, `misc`), so a `process::exit`, an abort or a segfault in one cannot take the
others down with no report.

Set `CYRUP_IT_BIN_DIR` to a directory of pre-built binaries and `build.rs` skips its nested second
link. On a constrained disk that is the difference between the suite running and the nested build
failing on disk space. Type-checking this crate does not exercise it; run it before trusting a
change to one of those subsystems.

Eighteen integration binaries remain in-crate under `crates/*/tests/`, each because it needs a process
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
bug. 85 rows are open across the area files: no `critical`, no `high`, 11 `medium`, 74 `low`, with
590 closed. `docs/gap-analysis/scripts/count_open_items.py` produces those counts, re-derived
2026-09-14.

**Nothing is open above `medium`.** Read that as "no row currently carries a `critical` or `high`
severity", not as "nothing serious is left": whatever is open is a floor rather than a total, and the
set above `medium` has turned over completely inside a single pass before.

Two things bound those counts harder than the counts themselves do.

**The ledger lags the code by a full feature.** The last pass that re-read area files measured at
`824a539e`; the `workflowScript` runtime and `cyrup-workflow-runtime` landed after it — 31 code
commits, 453 files, +98,179 / −15,880 under `crates/` — and **no area file has been re-read against
that**. A count of 85 is a count of what the ledger last looked at, not of what the port contains.

**The ledger lags the upstreams.** Every "latest tag" was re-measured 2026-09-14 and three of the
seven had moved — `pi` by two releases, `pi-subagents` by three, `pi-mcp-adapter` by one. Those
windows are measured below and **nothing in them is filed**. See
[`ADR-0006`](docs/adr/ADR-0006-upstream-chase-cadence.md) for the cadence and where the resulting
items are meant to live.

Concretely, the zombie defect in **Status** is a confirmed port bug — both sides read, upstream at a
named tag, and a failing test that reproduces it — and it carries no row. It was found by running the
suite during this refresh, which is the kind of thing a static ledger does not find.

The ledger is mostly a static analysis. Items are evidenced by reading both sources rather than by
running anything, its measured error rate has run near 12%, and it lags the code. Treat an entry as a
lead to verify. Items that have been observed against a running binary are marked in
[`REPRO-LOG.md`](docs/gap-analysis/REPRO-LOG.md). The tenth edition of
[`00-residual-ledger.md`](docs/gap-analysis/00-residual-ledger.md) records the four unfixed findings
its own batch merged with; a count of zero filed rows is not a claim that nothing is wrong.

MCP is the largest piece still in flight. A model calls a real server's tools end to end, and the
port is enumerated in `docs/gap-analysis/13*` against `pi-mcp-adapter`, with four upstream surfaces
cut by owner decision. That census was last re-derived against `v2.32.1`: **244 of 437 units
implemented**, plus 40 units filed for the `v2.26.1..v2.32.1` delta. The counted figure is a floor,
not an answer — 159 open rows were not re-opened, and the extrapolation over them carries a wide
interval, so do not quote it as a count. The 13 `TODO(MCP-NNN)` markers in `crates/cyrup-mcp/src` are
likewise a floor: six further ids are open with no marker. Upstream has since tagged `v2.33.0`, whose
window is measured below and unfiled. Area 13 is counted separately from the table above because it
plans code that does not exist yet rather than measuring drift in code that does.

## Upstreams

cyrup tracks seven upstream projects, six TypeScript and one Python. The core is
[`earendil-works/pi`](https://github.com/earendil-works/pi). Five optional subsystems follow
standalone Pi extensions, since Pi core ships no permission system, no MCP client and no editor
protocol of its own. Flux follows `code_puppy_core_plugins`, which is Python.

Latest tags re-measured 2026-09-14 with `git ls-remote --tags`; every window below is
`git diff --shortstat <baseline>..<tag>` against a freshly fetched clone.

| upstream | followed by | ported baseline | latest tag | baseline → latest |
|---|---|---|---|---|
| `earendil-works/pi` | most crates | v0.83.0 | **v0.85.1** | 1,087 files, +142,846 / −23,694 |
| `nicobailon/pi-subagents` | `cyrup-ext-subagents` | ~v0.43.0 (the crate records no version string) | **v0.67.0** | 613 files, +123,871 / −31,254 |
| `MasuRii/pi-permission-system` | `cyrup-permission-system` | v0.7.1 | v0.8.0, fully caught up | — |
| `nicobailon/pi-intercom` | `cyrup-intercom` | v0.9.2 | v0.13.0 | 26 files, +4,701 / −976 |
| `nicobailon/pi-mcp-adapter` | `cyrup-mcp` | v2.26.1 | **v2.33.0** | 204 files, +25,303 / −2,187 |
| `code_puppy_core_plugins` (Python) | `cyrup-flux` | v0.0.6 | **v0.0.50** | ported surface byte-identical |
| `svkozak/pi-acp` | `cyrup-acp` | v0.0.33 | v0.0.33, the newest upstream | — |

Three upstreams tagged new releases since the ledger last measured, and those windows are the
unfiled work:

| window | opened by | size |
|---|---|---|
| `pi` v0.84.4..v0.85.1 | 428 non-merge commits | 708 files, +96,348 / −25,254 — `packages/agent` +54,223, `coding-agent` +17,908, `chord` +10,840 (service wire semantics, delta-backed replicated state), `ai` +2,787, `tui` +2,485 |
| `pi-subagents` v0.64.0..v0.67.0 | 170 non-merge commits | 370 files, +42,205 / −24,183; `src/` alone is 159 files, +10,919 / −6,444 with 30 net-new source files — in-process pi child sessions, bounded SSH project execution, watchdog model-fallback chains, per-child async completion notification |
| `pi-mcp-adapter` v2.32.1..v2.33.0 | 33 non-merge commits | 123 files, +9,455 / −1,352 — one-URL server install, per-server HTTPS CA bundles, per-server env-inheritance opt-out, `directTools: "search"` lazy activation, trusted local Claude-plugin bundles, cross-process OAuth credential transactions |

`pi-permission-system`, `pi-intercom` and `pi-acp` published no new tag; each was re-checked against
the remote rather than inherited from the prior record.

The Flux row's version gap is still not a behaviour gap: the ported surface (`flux_bootstrap/`) is
byte-identical at every one of the 39 tags between `v0.0.6` and `v0.0.50`, re-checked tag by tag.

The pi-acp row remains the newest. `cyrup-acp` speaks the
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
`git -C tmp/<repo> show <tag>:<path>`, and a working tree's line numbers will mislead you.
[`docs/gap-analysis/README.md`](docs/gap-analysis/README.md) records the exact commits each pass
measured against. Re-measure the latest-tag column with
`git ls-remote --tags` rather than trusting it; this table has been wrong in both directions, and a
wrong baseline reclassifies in-baseline port bugs as version lag.

Where cyrup and an upstream disagree, the upstream is correct and cyrup is what changes. Every item
in the ledger is adjudicated that way, which is why a divergence has to be recorded as a
`CYRUP-DELTA` with a reason. [`ACKNOWLEDGEMENTS.md`](ACKNOWLEDGEMENTS.md) describes what each project
contributed.

## License

MIT, see [`LICENSE`](LICENSE), which also carries the upstream copyright notices.
