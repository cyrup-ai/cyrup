# Flux port — serial task breakdown

Parent spec: [`../flux.md`](../flux.md) (read it first — every task cites its sections).

Thirteen tasks, executed **in serial order** (`FLUX_01` → `FLUX_13`). Each is one focused
session of work. Do not start a task until the previous task's definition of done is met —
later tasks build on earlier artifacts (the package content becomes the crate's bundled
resources; the GAP sweep depends on the `ask_user_question` tool).

| # | Task | Phase | Deliverable |
|---|------|-------|-------------|
| 01 | [Package scaffold + manifest](FLUX_01.md) | 1 | `cyrup-flux/` repo installs via `cyrup install` |
| 02 | [Port `new.md` + `config.md`](FLUX_02.md) | 1 | state-bootstrap templates |
| 03 | [Port `ask.md` + `split.md` + `aug.md`](FLUX_03.md) | 1 | planning triad |
| 04 | [Port `exec.md` + `qa.md` + `tests.md`](FLUX_04.md) | 1 | execution triad |
| 05 | [Port git/GitHub templates + `auto-pilot.md`](FLUX_05.md) | 1 | pipeline A complete end-to-end |
| 06 | [`flux` skill](FLUX_06.md) | 1 | `/skill:flux` loads pipeline docs |
| 07 | [`cyrup-ext-flux` scaffold + state model + `/flux/status`](FLUX_07.md) | 2 | native status renderer, wired |
| 08 | [`/flux/cheatsheet` + `/flux/about`](FLUX_08.md) | 2 | remaining native renderers |
| 09 | [`ctrl+f` status overlay](FLUX_09.md) | 2 | interactive themed panel |
| 10 | [`ask_user_question` tool](FLUX_10.md) | 2 | agent-callable structured questions |
| 11 | [Bundled resources — crate becomes canonical](FLUX_11.md) | 2 | flux works out of the box |
| 12 | [FLUX-GAP sweep — restore structured questions](FLUX_12.md) | 2 | all 25 sites upgraded |
| 13 | [Parallel-exec prompt alignment](FLUX_13.md) | 3 | multi-task mode matches `subagent` semantics |

Conventions for every task file (per the flux `split` format):

- **No tests to be written** — another team owns tests.
- **No benchmarks to be written** — another team owns benchmarks.
- **No documentation work** beyond the content files a task explicitly creates.
- Definitions of done are behavioral and minimal — one manual run-through, not a test suite.
- Relative links resolve from this directory: `../flux.md` is the parent spec,
  `../../crates/…` is the cyrup workspace, `../../tmp/code-puppy/…` is the vendored source.
