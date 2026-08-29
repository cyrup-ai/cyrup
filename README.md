# cyrup

**cyrup** · /ˈsɪr.əp/ · *SIR-up* — rhymes with **syrup**, as in maple syrup.

A coding agent in Rust, inspired by the [Pi](https://github.com/earendil-works/pi) agent harness.
It takes Pi's design — a minimal core, everything-is-an-extension, an agent that can extend itself —
and rebuilds it on a Rust backbone with WebAssembly extensions.

> **Status:** not a released product. 21 crates, ~700k lines of Rust. The agent loop, provider
> layer, tool set, session tree, TUI, extension host, all four run modes and the Flux development
> pipeline are real and wired end to end — and so are the two subsystems this README last listed as
> unfinished. The **MCP client** now serves real tool calls against real servers, over stdio and over
> OAuth-protected HTTP, with sampling, elicitation and a JSON-RPC wire tracer. The **alternate-screen
> TUI** (`--tui-mode fullscreen`) is built to [ADR-0005](docs/adr/ADR-0005-alt-screen-tui-mode.md)
> rather than printing a fallback notice. Both gates are green: **8,255 unit tests** (8 skipped)
> and **486 integration tests**, no failures. Behavioural equivalence work is tracked openly in
> [`docs/gap-analysis/`](docs/gap-analysis/README.md) — **133 open items across the area files,
> down from 237 at the last published reconciliation**, with the two `critical` rows and the method
> question behind them described under [Coverage and known gaps](#coverage-and-known-gaps). The full
> user guide is in [`docs/guide/`](docs/guide/introduction.md) — see
> [Documentation](#documentation) below.

## Inspired by, not transliterated from

cyrup follows Pi's behaviour closely and cites it obsessively — there are **22,289 citations** in
the source pointing at the exact upstream `.ts` file and line a given Rust item mirrors. That index
is how equivalence gets audited: `grep -rn "agent-loop.ts:226" crates` finds the code that answers
for it.

But Rust is not TypeScript, and pretending otherwise produces bugs rather than fidelity. Where the
languages genuinely differ, cyrup ports the *behaviour* and records the mechanism difference in a
`CYRUP-DELTA` comment naming the upstream line and the reason — there are **454** of them. Real
examples from the codebase:

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
Wasmtime, against a versioned WIT world (`cyrup:ext@0.8.0`). That buys three things a dynamic-import
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

Five larger subsystems ship as **native built-in extensions** rather than WASM — subagent
delegation, the permission gate, the intercom broker, the MCP client, and Flux — because the first
three supervise OS processes and own Unix sockets, MCP spawns and speaks JSON-RPC to server child
processes, and Flux's whole point is to work with no install step at all.

The first three are **default off**, each arming on its own env flag (`CYRUP_SUBAGENTS` /
`CYRUP_PERMISSION_SYSTEM` / `CYRUP_INTERCOM`) or on the presence of its config file — note that
dropping a policy file into a repository is enough to arm the permission gate. Flux and MCP attach
unconditionally instead: Flux turns itself off only inside a subagent child process, and MCP is
inert until it discovers an `mcp.json` to act on.

## Workspace layout

Dependencies point downward only. `cyrup-core` depends on nothing in-workspace, and
`cyrup-session-svc` is the single seam the front-ends consume.

| Crate | Role |
|-------|------|
| `cyrup-core` | shared substrate: ids, `Content`/`Message`, `EventStream<T>`, `CancelToken`, the `Tool` trait |
| `cyrup-provider` | vendor-neutral LLM layer — 35 built-in chat providers, 10 wire APIs, auth, streaming, catalogs, images |
| `cyrup-agent` | the turn loop: tool execution, hooks, steering and follow-up queues, abort |
| `cyrup-tools` | built-in tools (`read`/`write`/`edit`/`bash`/`grep`/`find`/`ls`) over an `FsOps`/`ProcOps` seam |
| `cyrup-session` | JSONL session tree, compaction, system-prompt and context assembly |
| `cyrup-config` | layered settings, project trust, auth store, model resolution |
| `cyrup-ext` | WASM Component Model host (Wasmtime) + the native built-in tier |
| `cyrup-resources` | skills, prompt templates, themes, packages |
| `cyrup-tui` | ratatui + crossterm front-end — regular and alternate-screen (`fullscreen`) modes |
| `cyrup-modes` | print / json / rpc adapters, and an RPC client |
| `cyrup-sdk` | public embeddable API |
| `cyrup-session-svc` | the `AgentSession` facade wiring everything — **the one seam** |
| `cyrup` | the CLI binary |
| `cyrup-ext-subagents` | OS-subprocess subagent delegation (the largest crate) |
| `cyrup-permission-system` | runtime allow / ask / deny policy over every tool call |
| `cyrup-intercom` | Unix-socket broker for supervisor↔subagent coordination |
| `cyrup-flux` | the Flux structured development pipeline, on by default |
| `cyrup-mcp` | MCP client — servers, tools, OAuth, sampling, elicitation, a wire tracer and the `/mcp` surface |
| `cyrup-ext-sdk` | guest SDK for authoring extensions (`wasm32-wasip2`) |
| `cyrup-test-support` | faux provider + differential / interop / golden harnesses |
| `cyrup-it` | the integration-test harness crate (gated, see below) |

## Documentation

The full user guide lives under [`docs/guide/`](docs/guide/introduction.md), written as an
[mdBook](https://rust-lang.github.io/mdBook/) (`book.toml` at the repo root). Install `mdbook` and
run it locally:

```sh
cargo install mdbook
mdbook serve   # http://localhost:3000, live-reloads on save
mdbook build   # renders static HTML into target/guide
```

Start at [Introduction](docs/guide/introduction.md), or jump straight to a topic:

- [Install](docs/guide/getting-started/install.md) / [Connect a provider](docs/guide/getting-started/authenticate.md) / [Your first session](docs/guide/getting-started/first-session.md)
- [The terminal interface](docs/guide/guides/tui.md), [sessions](docs/guide/guides/sessions.md), [models and thinking](docs/guide/guides/models.md), [tools and permissions](docs/guide/guides/tools-and-permissions.md)
- [How extensions work](docs/guide/extensions/overview.md), [subagents](docs/guide/extensions/subagents.md), [the permission system](docs/guide/extensions/permissions.md), [intercom](docs/guide/extensions/intercom.md), [**Flux**](docs/guide/extensions/flux.md)
- [Command-line reference](docs/guide/reference/cli.md), [`settings.json`](docs/guide/reference/settings.md), [environment variables](docs/guide/reference/environment.md), [keybindings](docs/guide/reference/keybindings.md), [troubleshooting](docs/guide/reference/troubleshooting.md)

The guide has **no MCP chapter yet** — the client shipped ahead of its documentation. Until it lands,
`docs/gap-analysis/13-cyrup-mcp.md` and the module docs in `crates/cyrup-mcp/src/` are the reference.

## Build

```sh
cargo check --workspace --all-targets   # type-check everything, including tests
cargo clippy --workspace --all-targets -- -D warnings  # REQUIRED — the no-panic policy only fires here
cargo clippy -p cyrup-ext-sdk --target wasm32-wasip2   # the guest SDK — --workspace does not reach it
cargo clippy -p cyrup-it --features it                 # the gated harness — likewise
cargo nextest run --workspace           # the everyday gate: 8,255 tests (8 skipped)
cargo run -p xtask -- feature-matrix    # the non-default feature combinations (--fast skips cyrup-it)
cargo doc --workspace --no-deps --bins  # rustdoc links are denied, not warned — `--bins` or the binary is skipped
```

`cargo clippy` is not optional. The no-panic policy is expressed as `[workspace.lints.clippy]`
denials, and **clippy tool lints do not fire under `cargo build` or `cargo test`.** There is no CI
in this repository, so nothing runs these for you.

**All three clippy surfaces are at zero findings** as of `769ec3d`, so the workspace-wide
`-- -D warnings` form now passes rather than failing on arrival — the previous edition of this README
had to scope the deny flag to `cyrup-agent` alone because everything else carried warnings. Keep it
that way; the three lines above are three surfaces, not one repeated, because `--workspace` reaches
neither `cyrup-ext-sdk` (it compiles to `wasm32-wasip2`) nor `cyrup-it` (it is behind
`required-features`).

That cleanup is worth one paragraph of its own, because the *first* survey of this undercounted by a
factor of more than two. Deny-level lints (`unwrap_used`, `indexing_slicing`) are hard errors, so a
crate carrying one fails to compile — and every crate depending on it is then never linted at all.
Clearing 9 deny-level errors let clippy reach the rest of the graph and surfaced 9 further findings
no run had ever reported, plus three integration-test failures that had been invisible for the same
reason. **A clean clippy run on a graph that did not fully build is not a clean clippy run.**

The lines above build **one point** in the feature space. Nine crates declare `[features]`,
and `feature-matrix` builds the rest of them — the `#[cfg(not(feature = "wasm-host"))]` arms of
`cyrup-ext` and `cyrup-session-svc`, every `impl Backend` with `ratatui/scrolling-regions` off,
`cyrup-tools` without `inline-images`, the `faux` and `test-fixtures` arms, and the guest SDK for
`wasm32-wasip2`. Each row states the obligation it discharges, and prints it when the row fails.
Two of them are easy to over-read on a green run and say so in their own text: the
`cyrup-session-svc --no-default-features` row compiles that crate's native arms but does **not**
produce a wasmtime-free build (that is EXT-026), and the workspace-wide `--no-default-features` row
is a tripwire that today resolves to the same graph as the everyday gate, because every
in-workspace dependency edge asks for its dependency's default features.

Edition 2024, `resolver = "3"`, stable toolchain. A cold first build compiles the full dependency
graph (wasmtime, cranelift, gix, reqwest/rustls) — budget a few minutes and don't mistake it for a
hang.

## Testing

Almost all tests are **unit tests inline under `crates/*/src/`**. The heavy integration tests — the
ones that spawn the binary, drive a subagent, load a WASM component, open a broker socket or talk to
a real MCP server child process — live in one gated harness crate:

```sh
cargo build -p cyrup-ext-sdk --target wasm32-wasip2
cargo nextest run -p cyrup-it --features it        # 486 tests across 8 binaries
```

`cyrup-it` is behind `required-features = ["it"]`, so the everyday gate never builds it. Its
`build.rs` resolves the fixture binaries and the WASM component **once** via
`cargo build --message-format json-render-diagnostics`. It carries **eight** targets — one per seam
(`subagents`, `intercom`, `ext`, `permission`, `mcp`, `session_svc`, `bin`, `misc`) — rather than one
for the workspace, so a `process::exit`, an abort or a segfault in one seam cannot take the others
down with no report. The `mcp` target is the newest: MCP's whole contract is what it puts into a live
session's tool registry, so its proof has to be an assembled `AgentSession` rather than a double.

Nine integration binaries remain in-crate under `crates/*/tests/`, and each is there because it needs
a process of its own: it mutates the process environment (`cyrup-tools/tests/bash_env_scrub.rs`,
`cyrup/tests/first_time_setup.rs`), spawns the shipped `cyrup` binary
(`cyrup/tests/bootstrap_http_proxy.rs`, `cyrup/tests/export_dispatch_order.rs`), or pins a
whole-crate wiring proof next to the crate it proves.

This layout is deliberate and was earned. The suite previously had 310 integration binaries under
`crates/*/tests/` — and because Cargo makes every such file its own binary and process, a full run
took hours to execute roughly two minutes of assertions. It is now 9 in-crate binaries plus 8 gated
`cyrup-it` targets.

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

Three things about that ledger are worth stating up front. It is largely a **static** analysis —
items are evidenced by reading both sources, not by running anything — its own measured error rate is
about **12%** (of roughly 465 rows worked, ~56 turned out not to match the code at HEAD), and it
**lags the code**. Treat every entry as a lead to verify, not a fact. Items that *have* been observed
against a running binary are marked in [`REPRO-LOG.md`](docs/gap-analysis/REPRO-LOG.md).

**The count, recounted at this commit.** The ledger's own prose asks for a mechanical recount rather
than an adjusted one, because four successive editions rested on a counting rule a second reader
could not reproduce. Counting the thirteen `## Open items` tables — one per area file, areas 01–12 plus
14 — by a rule stated so a second reader can re-run it (a row is closed if its ID is struck through
or its severity or kind cell carries `CLOSED`/`FIXED`/`REFUTED`; `tracker` rows are excluded;
everything else is open at its stated severity) gives:

| | rows | closed | open | critical | high | medium | low |
|---|---:|---:|---:|---:|---:|---:|---:|
| areas 01–12 + 14 | **639** | **499 (78%)** | **133** | 2 | 3 | 49 | 79 |

That is down from the last published reconciliation (237 open, 2026-08-14) and from the correction
that superseded it (2026-08-19). Two caveats on the table above, both in the direction of the ledger
overstating what is open:

- **At least one open row is already closed in the code.** `PROV-068` (`high`) was closed as
  *refuted* in `24b6ffe`, with the reasoning left in-source at
  `crates/cyrup-provider/src/providers/together.rs:275-281`; its row was never struck. That is the
  general shape of the lag, not an isolated slip.
- **The `09a` drift file is outside the table.** `docs/gap-analysis/09a-*` opened the
  `pi-subagents` v0.47.1 → v0.57.0 window with 11 confirmed items plus 8 carried-unverified, and
  keeps them in its own summary table rather than area 09's. Six of the confirmed eleven have since
  had code land against them (`bf8b0f9`, `c94a360`), and none of those closures is reflected here.

Currently open, in brief:

- **The above-medium set is four live rows** — `SEAM-112` (`/resume` yields a broken session and
  bash calls repeat endlessly) and `PERM-034` ("Allow Always" does not stick), both `critical`, plus
  `TUI-091` (reasoning blocks never render) and `SEAM-113` (`/model` does not survive into the next
  session), both `high`. The table above counts five because `PROV-068`'s row was never struck.
  **Every one of them was filed from live use rather than from reading**, and that is the ledger's
  most useful finding about itself: nine reading sweeps and a nine-surface enumeration produced an
  above-medium set of wire-and-wiring defects a careful reader can see, while four days of actually
  running the binary produced three rows rated `critical` on arrival that no reading pass had a row
  for. **The above-medium set is what a static method is structurally worst at populating**, and the
  counter is hours in a real terminal, not a better sweep.
- **MCP is built and working, and its parity port is still in flight.** A model can call a real
  server's tools end to end over stdio and over OAuth-protected HTTP; sampling, elicitation and the
  JSON-RPC wire tracer all run, and the `mcp` seam has its own gated integration target.
  `docs/gap-analysis/13*` enumerates the port as **436 units** against `pi-mcp-adapter` v2.26.1 — four
  upstream surfaces are cut by owner decision, which removes ~14% of the package — and last published
  a census on 2026-08-21: 212 implemented, 100 partial, 98 missing, 27 not-applicable, so 198
  carrying open work. **That census predates the wave of work that made the crate function** and has
  not been re-derived; the live figure is the source itself, where **12 distinct `TODO(MCP-NNN)` ids
  remain** against their unit. The 13\* series is excluded from the counts above, because it tracks
  a port in flight rather than a gap in a shipped crate.
- Three of pi's built-in providers — `qwen-token-plan`, `qwen-token-plan-cn`, `radius` — are not
  registered (`PROV-014`, partially closed: the `pi-messages` wire API half is done, the provider
  half is not). A guard test asserts their absence so a half-finished provider cannot silently
  answer requests it cannot serve.
- **`cyrup-flux` is the newest surface and the least audited.** It got its first area file
  ([`14-cyrup-flux.md`](docs/gap-analysis/14-cyrup-flux.md)) only in the 2026-08-19 batch — 7 rows,
  none closed — so every figure in the ledger that predates it has the wrong denominator, not just
  the wrong numerator.
- **`CYRUP-DELTA` markers are not self-authorising, and a sweep found that some had been used that
  way.** An audit of `cyrup-tools`' deltas against pi turned 31 of them into an explicit triage
  backlog (`.flux/todo/parity-gaps/`, filed in `878d181`): each had been an agent-authored "out of
  scope" note or delta marker that nobody had signed off, and each now has to be closed, explicitly
  accepted, or done. A delta is a record of a decision — it is not the decision.
- Every upstream has moved past the baseline cyrup ported. Diff the ported baseline against upstream
  `HEAD` before treating a difference as a defect.

Two things the previous edition of this README listed as open are **done**:

- **Alternate-screen/fullscreen TUI mode ships.** `--tui-mode fullscreen` is an alternate-screen
  renderer with mouse capture, a scrollbar, text selection, image support and semantic-prompt
  navigation — `crates/cyrup-tui/src/altscreen/`, built to [ADR-0005](docs/adr/ADR-0005-alt-screen-tui-mode.md)
  B-1…B-14 in `dbcf59a`, with the session-erasure and four dead behaviours fixed in `b31f7c4`. It no
  longer prints a fallback notice.
- **The `cyrup-tui` ⇄ pi port audit backlog is empty.** All 26 audited gaps are in `.flux/done/`
  (`f8eeef3`). The three provider-catalog highs that headlined the previous edition
  (`PROV-054`/`055`/`056`) closed together on 2026-08-15, as did `SESS-040` and `PROV-047`.

## Upstreams

cyrup tracks six upstream projects — five TypeScript, one Python. The core is
[`earendil-works/pi`](https://github.com/earendil-works/pi); four optional subsystems follow
standalone Pi extensions, since Pi core deliberately ships no permission system and no MCP client of
its own. **Flux is the sixth and the odd one out**: it follows `code_puppy_core_plugins`, which is
Python, and it went unrecorded here — and in the gap ledger's own counts — until
[`14-cyrup-flux.md`](docs/gap-analysis/14-cyrup-flux.md) opened in the 2026-08-19 batch. The
`git show <tag>:<path>` rule below applies to it unchanged; only the language differs.

| upstream | followed by | ported baseline | drift window measured to | notes |
|---|---|---|---|---|
| `earendil-works/pi` | most crates | v0.83.0 | v0.84.1 | window = 627 files, +52 291 / −17 556; filed as area 12 + drift rows in areas 01–08 |
| `nicobailon/pi-subagents` | `cyrup-ext-subagents` | ~v0.43.0 *(inferred — the crate records no version string)* | **v0.57.0** | the v0.47.1 → v0.57.0 window (168 files, +21 385 / −7 307) was opened in `09a` and is partly closed |
| `MasuRii/pi-permission-system` | `cyrup-permission-system` | v0.7.1 | v0.8.0 | **caught up** — every v0.7.1 → v0.8.0 behavioural change is ported |
| `nicobailon/pi-intercom` | `cyrup-intercom` | v0.9.2 | v0.10.1 | v0.9.2 → v0.10.1 closed; **v0.10.1 → v0.12.0 is unopened**, filed as `ICOM-054`…`058` |
| `nicobailon/pi-mcp-adapter` | `cyrup-mcp` | v2.26.1 *(retargeted from v2.25.0 on 2026-08-20)* | v2.26.1 | port in flight — 437 units in `docs/gap-analysis/13*` |
| `code_puppy_core_plugins` *(Python)* | `cyrup-flux` | v0.0.6 | v0.0.6 | first audited 2026-08-19; also needs `code_puppy` core itself (v0.0.720) for `FLUX-002` |

The "measured to" column moves without warning; re-measure it with `git tag --sort=-v:refname`
against a real clone rather than trusting this table — the intercom row above is what happens when
you don't, since it read v0.10.1 as "latest" for two weeks after v0.12.0 shipped. The window between
the columns is measured and filed, not ignored — [`ADR-0006`](docs/adr/ADR-0006-upstream-chase-cadence.md)
records it per upstream and says where the resulting items live.

Each is the reference implementation of its own behaviour, and that is a working rule here rather
than a courtesy: where cyrup and an upstream disagree, the upstream is correct by definition and
cyrup is what changes. Every item in [`docs/gap-analysis/`](docs/gap-analysis/README.md) is
adjudicated that way, and it is why a divergence has to be recorded as a `CYRUP-DELTA` with a reason
instead of just being written. [`ACKNOWLEDGEMENTS.md`](ACKNOWLEDGEMENTS.md) describes what each
project contributed and why it was worth following this closely.

## License

MIT — see [`LICENSE`](LICENSE), which also carries the upstream copyright notices.
