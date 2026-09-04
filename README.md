# cyrup

**cyrup** · /ˈsɪr.əp/ · *SIR-up* — rhymes with **syrup**, as in maple syrup.

A coding agent in Rust, inspired by the [Pi](https://github.com/earendil-works/pi) agent harness.
It takes Pi's design — a minimal core, everything-is-an-extension, an agent that can extend itself —
and rebuilds it on a Rust backbone with WebAssembly extensions.

> **Status:** not a released product. 21 crates, ~786k lines of Rust. The agent loop, provider
> layer, tool set, session tree, TUI, extension host, all four run modes, the MCP client and the
> Flux development pipeline are real and wired end to end. Both gates are green: **8,710 workspace
> tests** (9 skipped) and **492 integration tests**, no failures. Behavioural equivalence work is
> tracked openly in [`docs/gap-analysis/`](docs/gap-analysis/README.md) — **103 open items across
> the area files, no `critical` rows, and exactly one row above `medium`**, described under
> [Coverage and known gaps](#coverage-and-known-gaps). The full user guide is in
> [`docs/guide/`](docs/guide/introduction.md) — see [Documentation](#documentation) below.

## Inspired by, not transliterated from

cyrup follows Pi's behaviour closely and cites it obsessively — there are **23,901 citations** in
the source pointing at the exact upstream `.ts` file and line a given Rust item mirrors. That index
is how equivalence gets audited: `grep -rn "agent-loop.ts:226" crates` finds the code that answers
for it.

But Rust is not TypeScript, and pretending otherwise produces bugs rather than fidelity. Where the
languages genuinely differ, cyrup ports the *behaviour* and records the mechanism difference in a
`CYRUP-DELTA` comment naming the upstream line and the reason — there are **506** of them. Real
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
| `cyrup-provider` | vendor-neutral LLM layer — 39 built-in providers over 10 wire APIs, 35 embedded catalogs, auth, streaming, images |
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
cargo fmt --all -- --check              # the workspace is rustfmt-clean under the pinned toolchain
cargo check --workspace --all-targets   # type-check everything, including tests
cargo clippy --workspace --all-targets -- -D warnings  # REQUIRED — the no-panic policy only fires here
cargo clippy -p cyrup-ext-sdk --target wasm32-wasip2   # the guest SDK — --workspace does not reach it
cargo clippy -p cyrup-it --features it                 # the gated harness — likewise
cargo nextest run --workspace           # the everyday gate: 8,710 tests (9 skipped)
cargo run -p xtask -- feature-matrix    # non-default feature combos + RUNS the gated cyrup-it seam suite (--fast skips it)
cargo doc --workspace --no-deps --bins  # rustdoc links are denied, not warned — `--bins` or the binary is skipped
```

`cargo fmt` is new to this list and worth one sentence. The workspace was never rustfmt-clean:
a check run reported 14,889 hunks across 1,216 files, and no `rustfmt.toml` setting recovered the
hand-applied style, so the default-config reflow was applied wholesale. It is clean now, under
`rust-toolchain.toml`'s pinned stable; keep it that way rather than re-litigating the style.

`cargo clippy` is not optional. The no-panic policy is expressed as `[workspace.lints.clippy]`
denials, and **clippy tool lints do not fire under `cargo build` or `cargo test`.** There is no CI
in this repository, so nothing runs these for you.

**All three clippy surfaces are at zero findings**, re-verified on the current `main` together with
`cargo doc` and both test gates, so the workspace-wide `-- -D warnings` form passes rather than
failing on arrival. Keep it that way; the three lines above are three surfaces, not one repeated,
because `--workspace` reaches neither `cyrup-ext-sdk` (it compiles to `wasm32-wasip2`) nor
`cyrup-it` (it is behind `required-features`).

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
cargo nextest run -p cyrup-it --features it        # 492 tests across 9 binaries
```

`cyrup-it` is behind `required-features = ["it"]`, so the everyday gate never builds it. Its
`build.rs` resolves the fixture binaries and the WASM component **once** via
`cargo build --message-format json-render-diagnostics`. It carries **nine** targets — one per seam
(`subagents`, `verify_redaction`, `intercom`, `ext`, `permission`, `mcp`, `session_svc`, `bin`,
`misc`) — rather than one for the workspace, so a `process::exit`, an abort or a segfault in one seam
cannot take the others down with no report. The `mcp` target's contract is what MCP puts into a live
session's tool registry, so its proof has to be an assembled `AgentSession` rather than a double.

Set `CYRUP_IT_BIN_DIR` to a directory of pre-built binaries (`cargo build -p cyrup --features faux
--bin cyrup`, plus each fixture crate's `--features test-fixtures --bins`) and `build.rs` skips its
nested second link entirely. On a constrained disk that is the difference between the suite running
and the nested build failing with `No space left on device`.

**A `cargo check` on this crate is not a run of it.** The batch that closed 25 parity rows edited
about 30 files here and type-checked them only; running the suite afterwards found two stale
fixtures that had been green in nobody's report. Run it before trusting a seam.

Sixteen integration binaries remain in-crate under `crates/*/tests/`, and each is there because it
needs a process of its own: it mutates the process environment (`cyrup-tools/tests/bash_env_scrub.rs`,
`cyrup/tests/first_time_setup.rs`), spawns the shipped `cyrup` binary
(`cyrup/tests/bootstrap_http_proxy.rs`, `cyrup/tests/export_dispatch_order.rs`), or pins a
whole-crate wiring proof next to the crate it proves — the six `cyrup-flux/tests/flux_00*` binaries
are the newest of those.

This layout is deliberate and was earned. The suite previously had 310 integration binaries under
`crates/*/tests/` — and because Cargo makes every such file its own binary and process, a full run
took hours to execute roughly two minutes of assertions. It is now 16 in-crate binaries plus 9 gated
`cyrup-it` targets.

Two conventions worth knowing:

- **Tests must not hit real provider APIs or paid tokens.** Offline coverage comes from a faux
  provider with a seeded PRNG so snapshots reproduce.
- **The integration suite has guard tests that fail if ambient credentials leak in.** If you have
  `TOGETHER_API_KEY` or the `CYRUP_*` feature flags exported, scrub them before running — otherwise
  a test can make a real network call. Those guards have caught exactly that.

## Coverage and known gaps

Tracked honestly and in the open, so nobody mistakes an unported feature for a bug. The full ledger
is [`docs/gap-analysis/`](docs/gap-analysis/README.md), with the decisions in
[`docs/adr/`](docs/adr/README.md). ([`docs/PARITY-PLAN.md`](docs/PARITY-PLAN.md) is the older
execution plan and now lags the ledger; the ranked view lives in
[`00-residual-ledger.md`](docs/gap-analysis/00-residual-ledger.md).)

Two things about that ledger are worth stating up front. It is largely a **static** analysis — items
are evidenced by reading both sources, not by running anything — and it **lags the code**. Its own
measured error rate has run near 12%, and the two most recent closure batches were consistent with
that: of the rows worked, two were refuted outright and one closed as a duplicate of an already-fixed
row. Treat every entry as a lead to verify, not a fact. Items that *have* been observed against a
running binary are marked in [`REPRO-LOG.md`](docs/gap-analysis/REPRO-LOG.md).

**The count.** It is no longer counted by hand. `docs/gap-analysis/scripts/count_open_items.py` walks
every area file's `## Open items` table by a rule a second reader can re-run, and both cross-cutting
files are regenerated from its output:

| | rows | closed | open | critical | high | medium | low |
|---|---:|---:|---:|---:|---:|---:|---:|
| areas 01–12, 14 and the `09a` supplement | **667** | **564 (85%)** | **103** | 0 | 1 | 28 | 74 |

Two structural fixes came with that script. The `09a` `pi-subagents` drift supplement is now counted
in the same total instead of keeping its own summary table outside it, and the script's own
hand-maintained carry-over list — which had been double-counting five rows — was emptied.

Currently open, in brief:

- **The set above `medium` is one row.** `SUBA-074` stage 2: an agent's `runner:` frontmatter can
  name an external CLI to run the child under, and that adapter protocol is unported, so the
  frontmatter is refused rather than honoured. Effort L, and it needs the protocol designed before
  anyone ports it. Every row the previous edition of this README named above `medium` is now closed:
  `PERM-034` refuted (the behaviour is upstream's own, and is now pinned by a test rather than filed
  as a bug), `PROV-068` refuted, `SEAM-112` fixed, `SEAM-113` closed as stale against pi's current
  opt-in persist contract, and `TUI-091` closed on a real-terminal observation showing that reasoning
  blocks do render.
- **Every other open row is `medium` or `low`.** The 28 mediums cluster in `cyrup-ext`'s renderer
  and shortcut surfaces (5), pi-core drift not yet routed to an owning area (5), and the config,
  TUI and subagent files (4, 4 and 3); the rest are spread one or two to an area. A representative
  one: six `pi-subagents` environment variables still have no `CYRUP_` counterpart, because the
  subsystems they configure — a per-tool child timeout, a run fan-out budget — do not exist here yet.
- **MCP works, and its parity port is still in flight.** A model calls a real server's tools end to
  end over stdio and over OAuth-protected HTTP; sampling, elicitation and the JSON-RPC wire tracer
  all run, and the `mcp` seam has its own gated integration target. `docs/gap-analysis/13*` enumerates
  the port as **437 units** against `pi-mcp-adapter` — four upstream surfaces are cut by owner
  decision, which removes about 14% of the package — and last published a census of 212 implemented,
  100 partial, 98 missing, 27 not-applicable. **That census predates the wave of work that made the
  crate function**, and the clone has since moved from v2.26.1 to v2.32.1, so it needs re-deriving;
  the live figure is the source itself, where **9 distinct `TODO(MCP-NNN)` ids remain**. Area 13 is
  counted separately above because it plans code that does not exist yet rather than measuring drift
  in code that does. **That separation is a counting rule and nothing more** — earlier editions of
  this README and of the ledger said another team owned area 13, which was never true, and the claim
  has been retracted in both.
- **`CYRUP-DELTA` markers are not self-authorising, and a sweep found that some had been used that
  way.** An audit of `cyrup-tools`' deltas against pi turned 31 of them into an explicit triage
  backlog (`.flux/todo/parity-gaps/`): each had been an agent-authored "out of scope" note that
  nobody had signed off, and each now has to be closed, explicitly accepted, or done. A delta is a
  record of a decision — it is not the decision.
- Every upstream has moved past the baseline cyrup ported. Diff the ported baseline against upstream
  `HEAD` before treating a difference as a defect.

Several things this README previously listed as open are **done**:

- **Both `qwen-token-plan` variants and `radius` are registered**, along with the
  `qwen-token-plan-individual` provider pi added later. The guard test that used to assert their
  absence now asserts their presence. Their model catalogs are still resolved dynamically rather than
  embedded, which is the open half of `PROV-014`.
- **`cyrup-flux` is no longer the unaudited surface.** It went from zero tests at 1,513 lines to 37
  tests whose every expectation is the upstream Python's own output, which caught six real defects;
  its bundled prompt tree is now embedded at build time rather than resolved through the build
  machine's source path, so an installed binary can find its own templates.
- **Alternate-screen/fullscreen TUI mode ships.** `--tui-mode fullscreen` is an alternate-screen
  renderer with mouse capture, a scrollbar, text selection, image support and semantic-prompt
  navigation, built to [ADR-0005](docs/adr/ADR-0005-alt-screen-tui-mode.md). It no longer prints a
  fallback notice.
- **The `cyrup-tui` ⇄ pi port audit backlog is empty**, and the `pi-subagents` drift supplement's
  carried-but-unverified section is now empty too: every row in it was re-read against the upstream
  tag and either ported or closed.

## Upstreams

cyrup tracks six upstream projects — five TypeScript, one Python. The core is
[`earendil-works/pi`](https://github.com/earendil-works/pi); four optional subsystems follow
standalone Pi extensions, since Pi core deliberately ships no permission system and no MCP client of
its own. **Flux is the sixth and the odd one out**: it follows `code_puppy_core_plugins`, which is
Python. The `git show <tag>:<path>` rule below applies to it unchanged; only the language differs.

| upstream | followed by | ported baseline | latest tag | notes |
|---|---|---|---|---|
| `earendil-works/pi` | most crates | v0.83.0 | **v0.84.4** | window = 775 files, +68 885 / −20 827; filed as area 12 + drift rows in areas 01–08 |
| `nicobailon/pi-subagents` | `cyrup-ext-subagents` | ~v0.43.0 *(inferred — the crate records no version string)* | **v0.64.0** | the largest window of the six; opened in `09a`, and its carried-unverified section is now empty |
| `MasuRii/pi-permission-system` | `cyrup-permission-system` | v0.7.1 | **v0.8.0** | **caught up** — every v0.7.1 → v0.8.0 behavioural change is ported. Upstream has since moved into the `gotgenes/pi-packages` monorepo |
| `nicobailon/pi-intercom` | `cyrup-intercom` | v0.9.2 | **v0.13.0** | the full v0.9.2 → v0.13.0 window is swept; `ICOM-054`/`055` remain open from it |
| `nicobailon/pi-mcp-adapter` | `cyrup-mcp` | v2.26.1 | **v2.32.1** | port in flight — 437 units in `docs/gap-analysis/13*`, whose census predates both the working crate and this tag |
| `code_puppy_core_plugins` *(Python)* | `cyrup-flux` | v0.0.6 | **v0.0.40** | the *ported surface* (`flux_bootstrap/`) is byte-identical across all 34 intervening tags, so the version gap is not a behaviour gap |

Clone all six under `./tmp/` (gitignored) before working a ledger row; the area files cite them as
`git -C tmp/<repo> show <tag>:<path>`, and a working tree's line numbers will mislead you.
[`docs/gap-analysis/README.md`](docs/gap-analysis/README.md)'s Baselines table carries the exact
commits each pass measured against.

Re-measure the "latest tag" column rather than trusting it — `git ls-remote --tags` against a real
clone. This table has been wrong in both directions before: it read `pi-intercom` v0.10.1 as latest
for two weeks after v0.12.0 shipped, and it recorded a `pi-intercom` baseline of v0.7.0 for months
when the real one was v0.9.2, which quietly reclassified in-baseline port bugs as version lag. The
cadence and where the resulting items live are recorded in
[`ADR-0006`](docs/adr/ADR-0006-upstream-chase-cadence.md).

Each is the reference implementation of its own behaviour, and that is a working rule here rather
than a courtesy: where cyrup and an upstream disagree, the upstream is correct by definition and
cyrup is what changes. Every item in [`docs/gap-analysis/`](docs/gap-analysis/README.md) is
adjudicated that way, and it is why a divergence has to be recorded as a `CYRUP-DELTA` with a reason
instead of just being written. [`ACKNOWLEDGEMENTS.md`](ACKNOWLEDGEMENTS.md) describes what each
project contributed and why it was worth following this closely.

## License

MIT — see [`LICENSE`](LICENSE), which also carries the upstream copyright notices.
