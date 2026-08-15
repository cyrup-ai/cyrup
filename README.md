# cyrup

**cyrup** · /ˈsɪr.əp/ · *SIR-up* — rhymes with **syrup**, as in maple syrup.

A coding agent in Rust, inspired by the [Pi](https://github.com/earendil-works/pi) agent harness.
It takes Pi's design — a minimal core, everything-is-an-extension, an agent that can extend itself —
and rebuilds it on a Rust backbone with WebAssembly extensions.

> **Status:** not a released product. 19 crates, ~543k lines of Rust. The agent loop, provider
> layer, tool set, session tree, TUI, extension host and all four run modes are real and wired end
> to end. Both gates are green: 6,740 unit tests in ~18s, 473 integration tests in ~92s. Behavioural
> equivalence work is tracked openly in [`docs/gap-analysis/`](docs/gap-analysis/README.md).

## Inspired by, not transliterated from

cyrup follows Pi's behaviour closely and cites it obsessively — there are **18,827 citations** in
the source pointing at the exact upstream `.ts` file and line a given Rust item mirrors. That index
is how equivalence gets audited: `grep -rn "agent-loop.ts:670" crates` finds the code that answers
for it.

But Rust is not TypeScript, and pretending otherwise produces bugs rather than fidelity. Where the
languages genuinely differ, cyrup ports the *behaviour* and records the mechanism difference in a
`CYRUP-DELTA` comment naming the upstream line and the reason. Real examples from the codebase:

- A JavaScript `async` function always settles. A Rust future can be dropped at any `.await`, so
  anything registered before an await and cleaned up only on the success path leaks forever. cyrup
  puts that cleanup in `Drop`.
- JavaScript has no locks, so a re-entered handler is an ordinary nested call. In Rust the same
  shape can re-take a held `tokio::Mutex` and hang with no deadlock detection.
- `tokio::select!` picks at random when both arms are ready. JavaScript cannot express that race at
  all, so cyrup marks those `biased;` to reproduce upstream's deterministic ordering.
- `{ ...tool, execute }` preserves every field by construction. A hand-written Rust trait impl must
  name each method, so cyrup's fixtures give every method a distinct non-default value — otherwise a
  forgotten delegation is invisible to its own test.

Those aren't compromises; they are the parts of the design that only become visible when you rebuild
it in a language with different guarantees.

## Why Rust here

The agent is a long-running process that supervises subprocesses, streams from network APIs, holds a
session tree in memory and repaints a terminal. That workload rewards the things Rust is good at:

- **A single static binary.** No runtime, no `node_modules`, no version manager. It starts fast
  because there is nothing to warm up.
- **Predictable memory.** A session tree with thousands of entries and a long transcript stays flat.
  There is no GC pause in the middle of a token stream.
- **Real concurrency for a genuinely concurrent problem.** Parallel tool calls, a streaming provider
  response, a TUI repaint, and several subagent processes are separate tasks on a work-stealing
  runtime rather than callbacks on one loop.
- **Cancellation you can trust — once you respect it.** Dropping a future actually stops the work.
  That is a real advantage over a promise you can only ignore, and it is also the sharpest edge in
  the codebase: most of the `CYRUP-DELTA` notes above exist because cleanup has to survive a drop.
- **A no-panic policy that is enforced.** `unwrap`, `expect`, `panic!` and raw indexing are denied
  workspace-wide by clippy lints. An agent that crashes mid-turn loses your session.

## Extensions are WebAssembly components

Pi loads TypeScript at runtime through `jiti`. cyrup runs extensions as **WASM components** under
Wasmtime, against a versioned WIT world (`cyrup:ext@0.7.0`). That buys three things a dynamic-import
model cannot:

**A real sandbox.** An extension declares what it needs — filesystem roots, process execution,
network, UI — in its manifest, and the host enforces it. A component that declares nothing gets
nothing; it cannot read a file it did not ask for, because the capability was never handed to it.
Enforcement is host-side, so a buggy or hostile guest cannot opt itself back in.

**A typed contract that breaks loudly.** The WIT world is the API. Change it and the version moves;
a guest built against an older world fails a clear version check instead of throwing a
`TypeError` three seconds into a turn. Both copies of the world are byte-identical by test, and the
ABI fingerprint invalidates the build when it moves.

**Any language that targets components.** Extensions are not tied to the host's language. The
in-tree guest SDK (`cyrup-ext-sdk`) is Rust and compiles to `wasm32-wasip2`, but the boundary is
WIT, not Rust.

The tradeoff is honest and worth stating: values cross the boundary as data, not references. Where
Pi hands an extension a live object, cyrup passes a serialized value or an explicit round-trip. That
is a genuine constraint on API shape, and it is documented as such rather than hidden.

Three larger subsystems ship as **native built-in extensions** rather than WASM — subagent
delegation, the permission gate, and the intercom broker — because they supervise OS processes and
own Unix sockets. All three are **default off**, each arming on its own env flag
(`CYRUP_SUBAGENTS` / `CYRUP_PERMISSION_SYSTEM` / `CYRUP_INTERCOM`) or on the presence of its config
file. Note that dropping a policy file into a repository is enough to arm the permission gate.

## Workspace layout

Dependencies point downward only. `cyrup-core` depends on nothing in-workspace, and
`cyrup-session-svc` is the single seam the front-ends consume.

| Crate | Role |
|-------|------|
| `cyrup-core` | shared substrate: ids, `Content`/`Message`, `EventStream<T>`, `CancelToken`, the `Tool` trait |
| `cyrup-provider` | vendor-neutral LLM layer — 23 built-in providers, 10 wire APIs, auth, streaming, catalogs, images |
| `cyrup-agent` | the turn loop: tool execution, hooks, steering and follow-up queues, abort |
| `cyrup-tools` | built-in tools (`read`/`write`/`edit`/`bash`/`grep`/`find`/`ls`) over an `FsOps`/`ProcOps` seam |
| `cyrup-session` | JSONL session tree, compaction, system-prompt and context assembly |
| `cyrup-config` | layered settings, project trust, auth store, model resolution |
| `cyrup-ext` | WASM Component Model host (Wasmtime) + the native built-in tier |
| `cyrup-resources` | skills, prompt templates, themes, packages |
| `cyrup-tui` | ratatui + crossterm front-end |
| `cyrup-modes` | print / json / rpc adapters, and an RPC client |
| `cyrup-sdk` | public embeddable API |
| `cyrup-session-svc` | the `AgentSession` facade wiring everything — **the one seam** |
| `cyrup` | the CLI binary |
| `cyrup-ext-subagents` | OS-subprocess subagent delegation (the largest crate) |
| `cyrup-permission-system` | runtime allow / ask / deny policy over every tool call |
| `cyrup-intercom` | Unix-socket broker for supervisor↔subagent coordination |
| `cyrup-ext-sdk` | guest SDK for authoring extensions (`wasm32-wasip2`) |
| `cyrup-test-support` | faux provider + differential / interop / golden harnesses |
| `cyrup-it` | the integration-test harness crate (gated, see below) |

## Build

```sh
cargo check --workspace --all-targets   # type-check everything, including tests
cargo clippy --workspace --all-targets  # REQUIRED — the no-panic policy only fires here
cargo nextest run --workspace           # the everyday gate: ~6,740 tests, ~18s
```

`cargo clippy` is not optional. The no-panic policy is expressed as `[workspace.lints.clippy]`
denials, and **clippy tool lints do not fire under `cargo build` or `cargo test`.** There is no CI
in this repository, so nothing runs these for you.

Edition 2024, `resolver = "3"`, stable toolchain. A cold first build compiles the full dependency
graph (wasmtime, cranelift, gix, reqwest/rustls) — budget a few minutes and don't mistake it for a
hang.

## Testing

Every crate holds **unit tests only**, inline under `crates/*/src/`. Integration tests — the ones
that spawn the binary, drive a subagent, load a WASM component or open a broker socket — live in one
gated harness crate:

```sh
cargo build -p cyrup-ext-sdk --target wasm32-wasip2
cargo nextest run -p cyrup-it --features it        # ~473 tests, ~92s
```

`cyrup-it` is behind `required-features = ["it"]`, so the everyday gate never builds it. Its
`build.rs` resolves the fixture binaries and the WASM component **once** via
`cargo build --message-format=json`.

This layout is deliberate and was earned. The suite previously had 310 files under `crates/*/tests/`
— and because Cargo makes every such file its own binary and process, a full run took hours to
execute roughly two minutes of assertions. It is now 7 in-crate binaries plus 8 gated targets.

Two conventions worth knowing:

- **Tests must not hit real provider APIs or paid tokens.** Offline coverage comes from a faux
  provider with a seeded PRNG so snapshots reproduce.
- **The integration suite has guard tests that fail if ambient credentials leak in.** If you have
  `TOGETHER_API_KEY` or the `CYRUP_*` feature flags exported, scrub them before running — otherwise
  a test can make a real network call. Those guards have caught exactly that.

## Coverage and known gaps

Tracked honestly and in the open, so nobody mistakes an unported feature for a bug. The full ledger
is [`docs/gap-analysis/`](docs/gap-analysis/README.md), with the execution order in
[`docs/PARITY-PLAN.md`](docs/PARITY-PLAN.md) and the decisions in [`docs/adr/`](docs/adr/README.md).

Two things about that ledger are worth stating up front. It is largely a **static** analysis — items
are evidenced by reading both sources, not by running anything — and its own measured error rate is
about **12%**: of roughly 430 items worked, ~53 turned out not to match the code at HEAD. Treat
every entry as a lead to verify, not a fact. Items that *have* been observed against a running
binary are marked in [`REPRO-LOG.md`](docs/gap-analysis/REPRO-LOG.md).

Currently open, in brief:

- Some upstream behaviour is still unported or partial; see the ledger for the live count.
- Alternate-screen/fullscreen TUI mode is decided-in-scope ([ADR-0005](docs/adr/)) and not yet built;
  `--tui-mode fullscreen` says so explicitly rather than failing obscurely.
- Every upstream has moved since the version cyrup tracks. Diff the tracked tag against upstream
  `HEAD` before treating a difference as a defect.

## Upstreams

cyrup tracks four TypeScript projects. The core is [`earendil-works/pi`](https://github.com/earendil-works/pi);
three optional subsystems follow standalone Pi extensions, since Pi core deliberately ships no
permission system of its own.

| upstream | followed by | tracked version |
|---|---|---|
| `earendil-works/pi` | most crates | v0.83.0 |
| `nicobailon/pi-subagents` | `cyrup-ext-subagents` | ~v0.43.0 |
| `MasuRii/pi-permission-system` | `cyrup-permission-system` | v0.7.1 |
| `nicobailon/pi-intercom` | `cyrup-intercom` | v0.9.2 |

These are mature, maintained projects and the reference implementations of their own behaviour —
if you are choosing between them and cyrup, use them. [`ACKNOWLEDGEMENTS.md`](ACKNOWLEDGEMENTS.md)
describes what each one contributed and why it was worth following closely.

## License

MIT — see [`LICENSE`](LICENSE), which also carries the upstream copyright notices.
