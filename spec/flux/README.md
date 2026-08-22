# Flux port — serial task breakdown

Parent spec: [`../flux.md`](../flux.md) (read it first — every task cites its sections).

**Twelve tasks, executed in serial order** (`FLUX_01` → `FLUX_12`). Each is one focused session.
Do not start a task until the previous task's definition of done is met — later tasks build on
earlier artifacts.

## ONE HOME: `crates/cyrup-flux`

Everything this series produces — the 15 prompt templates, the `flux` skill, the reference docs,
the three native renderers, the status overlay and the `ask_user_question` tool — lives in a
single crate inside the cyrup workspace:

```
cyrup/crates/cyrup-flux/
├── Cargo.toml
├── resources/
│   ├── prompts/flux/{15 templates}.md
│   ├── prompts/flux/_docs/{README,pipeline,cheatsheet,synopsis,about}.md
│   └── skills/flux/{SKILL.md, reference/*.md}
└── src/{lib,extension,resources,state,render_status,render_cheatsheet,render_about,overlay,ask_tool}.rs
```

The content reaches every session through the extension's `ResourcesDiscover` contribution
(FLUX_01), so **flux works out of the box with no install step**.

### What changed from the previous revision of this series

The earlier plan shipped Phase 1 as a **separate git repo** `cyrup-flux`, installed with
`cyrup install`, and then had a later task copy the whole content tree into a crate and keep the
two homes `rsync`-synchronised forever. That architecture is **gone**:

| removed | why |
|---|---|
| `git init` of a sibling repo | the work belongs in `cyrup`, which is already a git repo |
| `cyrup.toml` package manifest | a crate is not a package; the resource system is reached through `ResourcesDiscover`, not `cyrup install` |
| `cyrup install` / `cyrup list` / `cyrup remove` proofs | nothing is installed |
| the old FLUX_11 "vendor into the crate + re-sync the package" | there is nothing to vendor *from* and nothing to sync *to* — the crate is canonical from FLUX_01 |
| every `rsync -a --delete "$RES" "$PKG"` + `diff -r` gate in the old 11/12/13 | ditto |

That is a whole task's worth of work deleted, which is why the series is **12 tasks, not 13**.
The old numbering maps `01→01`, `02–06→02–06`, `07–10→07–10`, `12→11`, `13→12`; the old `11`
dissolved into the new `01`, where the resource contribution belongs.

Three corrections to the parent spec were found while re-planning and are recorded in
[FLUX_01](FLUX_01.md):

1. `NativeExtension`'s first method is **`fn id(&self) -> ExtensionId`**, not `fn name(&self) -> &str`.
2. There are **three** `with_native_extension` seams in `main.rs` (one per `AppMode`), not one.
3. `HostServices` / `InteractiveOverlay` / the overlay value types are **`wasm-host`-gated**
   re-exports, so the crate must never disable `cyrup-ext`'s default features.

## The tasks

| # | Task | Deliverable |
|---|------|-------------|
| 01 | [Crate scaffold + wiring + bundled-resource contribution](FLUX_01.md) | `cyrup-flux` loads in all 3 modes; `flux/*` names register namespaced |
| 02 | [Port `new.md` + `config.md`](FLUX_02.md) | state-bootstrap templates |
| 03 | [Port `ask.md` + `split.md` + `aug.md`](FLUX_03.md) | planning triad |
| 04 | [Port `exec.md` + `qa.md` + `tests.md`](FLUX_04.md) | execution triad |
| 05 | [Port git/GitHub templates + `auto-pilot.md`](FLUX_05.md) | all 15 templates; pipeline A complete |
| 06 | [The `flux` skill](FLUX_06.md) | `/skill:flux` loads the pipeline docs |
| 07 | [`state.rs` + `/flux/status`](FLUX_07.md) | native status renderer |
| 08 | [`/flux/cheatsheet` + `/flux/about`](FLUX_08.md) | remaining native renderers |
| 09 | [`ctrl+f` status overlay](FLUX_09.md) | interactive themed panel |
| 10 | [`ask_user_question` tool](FLUX_10.md) | agent-callable structured questions |
| 11 | [FLUX-GAP sweep — restore structured questions](FLUX_11.md) | all 25 sites upgraded |
| 12 | [Parallel-exec prompt alignment](FLUX_12.md) | multi-task mode matches `subagent` semantics |

## Shared conventions

- **No tests to be written** — another team owns tests.
- **No benchmarks to be written** — another team owns benchmarks.
- **No documentation work** beyond the content files a task explicitly creates.
- Definitions of done are behavioural and minimal — one manual run-through, not a test suite.
- Relative links resolve from this directory: `../flux.md` is the parent spec,
  `../../crates/…` is the cyrup workspace, `../../tmp/code-puppy/…` is the vendored source.

### Paths on this machine

```bash
CY=/home/d0m17bw/workspace/cyrup                                 # the cyrup checkout
RES=$CY/crates/cyrup-flux/resources                              # the one content home
CP=$CY/tmp/code-puppy/flux_bootstrap/bundled/commands/flux       # vendored code-puppy source
```

`$CY/tmp/` is gitignored (`cyrup/.gitignore`: `tmp/`) and holds clones of
`mpfaffenberger/code_puppy_core_plugins` (as `code-puppy`, with `flux_bootstrap` and
`customizable_commands` symlinked to the top level so every `../tmp/code-puppy/…` link in this
spec resolves) and `mpfaffenberger/code_puppy` (as `code-puppy-core`), plus pi's
`prompt-templates.ts`. FLUX_01 documents how to rebuild it if it is missing.

### The one verification command

Every task that adds a template, a skill or a native command verifies it the same way — a pure
local registry query that spends **no tokens**:

```bash
printf '{"type":"get_commands","id":"1"}\n' \
  | ANTHROPIC_API_KEY=${ANTHROPIC_API_KEY:-sk-fake} cyrup --mode rpc --no-session
```

The dummy key only satisfies the modelless hard stop that applies to every non-interactive mode
([`main.rs:950-955`](../../crates/cyrup/src/main.rs)); no provider is contacted. Prompt templates
come back as `{"name":"flux/<step>","source":"prompt"}`, skills as
`{"name":"skill:flux","source":"skill"}`, native commands as `{"source":"extension"}`.

**Never verify with `cyrup -p "/flux/…"`**: on a name miss
([`prompt.rs:159-173`](../../crates/cyrup-resources/src/prompt.rs)) the raw text is sent to the
model as a prompt, so a typo silently becomes a paid API call.

### The build gate

```bash
cargo build -p cyrup-flux && cargo build -p cyrup
cargo clippy --workspace --all-targets --features test-fixtures; echo "exit=$?"   # MUST be 0
cargo clippy -p cyrup-agent --all-targets --no-deps -- -D warnings; echo "exit=$?"   # MUST be 0 — cyrup-agent is warning-clean; keep it that way
```

The first line exits 0 on warn-level diagnostics, so only the deny-flagged second line can actually
fail. It is scoped to `cyrup-agent` because other crates are not warning-clean yet — `--no-deps` is
required for that scoping, since the deny flag otherwise reaches the workspace path dependencies too.
It carries no `--features` flag because `cyrup-agent` declares none; its target set is the same
either way.

Clippy is mandatory on every task that touches `src/`: the workspace's no-panic lints
(`unwrap_used`, `expect_used`, `panic`, `indexing_slicing`) are **clippy-only** and never fire
under `cargo build` or `cargo test`.
