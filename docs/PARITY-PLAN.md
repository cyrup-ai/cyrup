# cyrup → pi parity plan

**What this is.** An execution order for the whole parity backlog: what to do first, what one batch
is, what "done" means for each, and — explicitly — what this plan does *not* schedule and why.
It is a plan, not a status report. It is meant to be argued with; §*The thesis* states the criterion
that produced this order so that disagreeing with it is cheap.

**Derived from.** [`docs/gap-analysis/`](gap-analysis/README.md) as regenerated **2026-08-12**
(second edition, post-repair-pass), against cyrup **HEAD `04c1ba2`** (last code commit; docs HEAD
`a9000b1`, branch `david/cyrup`). Every count below was read out of
[`00-residual-ledger.md`](gap-analysis/00-residual-ledger.md) at `:66-86`, not restated from any
intermediate artefact.

## The backlog, three figures, all published

| figure | crit | high | medium | low | total |
|---|---:|---:|---:|---:|---:|
| **raw ID count** — severity-bearing rows across the twelve `## Open items` tables | 6 | 22 | 197 | 223 | **448** |
| **deduplicated distinct defects** — less 15 machine-readable area-12 `duplicate-of:` rows and 13 editorial cluster-F4 rows | 6 | 21 | 188 | 205 | **~420** |
| **trackers, excluded from both** — rows that propose no schedulable work | — | — | — | — | **9** |

Plus 2 `partially-closed` provenance rows in area 02 (`AGENT-S01`, `AGENT-S04`) counted nowhere.
**Total rows across the twelve tables: 457.** Effort profile: **S 286 · M 130 · L 32** — 64% of the
backlog is `S`, and of the 28 criticals-plus-highs, 15 are `S`, 12 `M`, 1 `L`. The top of this
backlog is unusually cheap.

Use **448** for "how many rows must be dispositioned" and **~420** for "how many distinct things are
wrong". Neither is a total. Structural defect C in the ledger (`:544-548`) is confirmed and hard:
117 items closed this pass against **207 filed**, 31 of them from four upstream surfaces no file in
the directory had ever named. **448 is a floor.** This plan schedules seven opening reads whose
explicit purpose is to file more, so the number will go up before it goes down.

## Upstream lag at plan time

| upstream | cyrup ported baseline | latest tag | window | position |
|---|---|---|---|---|
| `pi` | v0.83.0 | **v0.84.1** (+1 minor; HEAD is 117 commits past it) | 627 files, +52 291 / −17 556 | frozen; drift absorbed per-batch |
| `pi-subagents` | ≈v0.43.0 *(inferred — the crate records no version string)* | **v0.47.1** (+4 minors, 358 commits, HEAD +14 more) | 151 files, +10 254 / −1 333 | frozen until batches 18/21/22 read `8902b4f` against v0.43.0 |
| `pi-permission-system` | v0.7.1 | **v0.8.0** (+1 minor) | 28 files, +4 023 / −1 851 | already fully absorbed — **zero** drift items |
| `pi-intercom` | v0.9.2 *(corrected; every prior doc said v0.7.0)* | **v0.10.1** (+1 minor, 14 commits) | 24 files, +2 495 / −700 | drift **is** the work — batches 24-26 |

See §*Chasing the upstreams* for the position and the re-baseline cadence.

---

# 1 · The next three moves

Startable tomorrow morning, in this order. Each names the file to open first and the condition that
ends it.

## Move 1 — Build it, run it in a real terminal, and write down what actually happened

Nothing in this backlog has ever been observed. The ledger says so at its own head (`:51-57`): no
binary was built, launched or tested for this pass or the repair pass, every `Verify` line in all
fifteen files is a *design*, and even the inherited "3932 passed / 0 failed / 8 ignored" count was
never executed by any pass that quotes it. An item is not *ranked* until someone has seen it happen.

**Do:**

1. `cargo build --workspace --release` at `04c1ba2`; record the result.
2. `cargo test --workspace`; record the **real** pass/fail/ignored counts and paste them in place of
   the inherited claim.
3. **Provision the prerequisites the repro rows need** — a working provider credential (nine of the
   sixteen rows need a live streaming turn or an existing session store), two project directories, a
   fresh session store, a stub `trash` on `PATH`, and one WASM guest fixture built with
   `cargo build -p cyrup-ext-sdk --target wasm32-wasip2` (the crate is in `members` but excluded from
   `default-members`, `Cargo.toml:26-27`, and no guest fixture ships — without one `EXT-054` cannot
   be reproduced at all).
4. Launch interactive, `print`, `json` and `--mode rpc` in a real terminal.
5. Walk 16 repro rows: [TUI-042](gap-analysis/07-cyrup-tui.md) (paste 40 lines, backspace into the
   marker, undo, Enter — what reaches the model?), [TUI-043](gap-analysis/07-cyrup-tui.md) (one
   Ctrl+W at a marker's end), [TUI-027](gap-analysis/07-cyrup-tui.md) (`/tree`, press `e`, type,
   Enter — does a label land in the session JSONL?), [SEAM-051](gap-analysis/08-cyrup-session-svc-and-modes.md)
   (`cyrup --tui-mode regular`), [SEAM-064](gap-analysis/08-cyrup-session-svc-and-modes.md) (count
   the trust-prompt options), [SEAM-062](gap-analysis/08-cyrup-session-svc-and-modes.md) (rename in
   `--resume`, relaunch), [SEAM-061](gap-analysis/08-cyrup-session-svc-and-modes.md) (two project
   dirs, press Tab), [SEAM-063](gap-analysis/08-cyrup-session-svc-and-modes.md) (delete a session,
   check the trash), [SESS-040](gap-analysis/03-cyrup-session.md) (Esc during compaction),
   [SEAM-047](gap-analysis/08-cyrup-session-svc-and-modes.md) (SIGTERM a live `--mode rpc`),
   [UW-2](gap-analysis/PARITY-GAPS.md) (first run with `CYRUP_EXPERIMENTAL=1`),
   [AGENT-020](gap-analysis/02-cyrup-agent.md) (steer during an active run),
   [PERM-009](gap-analysis/10-cyrup-permission-system.md) (`tools.bash: deny` plus a narrower allow),
   [EXT-054](gap-analysis/06-cyrup-ext.md) (a guest manifest declaring nothing),
   [TUI-045](gap-analysis/07-cyrup-tui.md) (hold an arrow key over SSH mid-stream),
   [TUI-016](gap-analysis/07-cyrup-tui.md) (is a queued message visible at all?).

**Open first:** `/Users/davidmaple/cyrup.ai/cyrup/crates/cyrup-tui/src/editor.rs:73` — `struct
Snapshot { lines, row, col }`, three fields, no `pastes`. That is TUI-042's cyrup half, half-proven
on a read before the terminal is even open. (The doc comment is at `:71`; the struct is at `:73` —
the earlier draft of this plan cited `:71` and was wrong.) **Nothing is edited in move 1.**

**Done when:** `docs/gap-analysis/REPRO-LOG.md` exists with 16 rows, each marked
**CONFIRMED / REFUTED / BLOCKED / NOT-REACHABLE**, each carrying a terminal transcript or an
asciinema cast. Every TUI row cites a **live** run — a ratatui `TestBackend` result is not admissible
and does not close a row. The real `cargo test --workspace` counts replace the inherited claim.

**Stop condition:** if **two or more rows come back REFUTED *or* BLOCKED**, re-plan before fixing.
REFUTED at that rate means the item-level error rate matches the measured ~20% citation error rate
and the whole crit+high set needs repro first; BLOCKED at that rate means the instrument was not
actually built and the log proves nothing.

## Move 2 — Land the two one-token fixes while the terminal is still open

- **[SEAM-064](gap-analysis/08-cyrup-session-svc-and-modes.md)** — flip `false` to `true` at
  `crates/cyrup/src/main.rs:1155`, which reads exactly
  `let options = cyrup_config::trust::trust_options(&dirs.cwd, false);`. The flag gates both
  "(this session only)" rows (`trust.rs:356-363`, `:370-377`), so today the startup security prompt
  offers three options, **every one of which persists a permanent verdict** — including a permanent
  lockout, from a prompt that never offers to reverse it. Leave the other call site
  (`cyrup-session-svc/src/session.rs:3255`) alone: pi's in-app selector genuinely passes false.
- **[SEAM-051](gap-analysis/08-cyrup-session-svc-and-modes.md)** — add `--tui-mode` to
  `KNOWN_LONG_FLAGS` (`crates/cyrup/src/cli.rs:757`) and `KNOWN_VALUE_LONG_FLAGS`, add a
  `TuiMode { Regular, Fullscreen }` value-enum to `Cli`, add pi's two diagnostics to
  `apply_arg_leniency` (`diagnostics.rs:90-152`), add the help line. Accept `regular` as a no-op and
  reject `fullscreen` with an explicit not-supported message. This is the shipped interim and it
  **must not wait** on OQ-3.
- Update `crates/cyrup/src/startup_ui.rs:504-537` to assert the five-option order and that a
  session-only index yields an **empty** `updates`, so `set_many` writes nothing.

**Open first:** `crates/cyrup/src/main.rs:1155`.

**Done when:** `cyrup --tui-mode regular` starts the binary instead of exiting 1 with "unknown
option" — no pi command line or wrapper script can launch cyrup today; `cyrup --tui-mode bogus`
prints exactly `Invalid TUI mode "bogus". Valid values: regular, fullscreen` and exits 1; the startup
trust prompt renders **five** options in a real terminal and choosing either session-only row leaves
`trust.json` byte-unchanged, verified by inspecting the store after quitting. Both repro rows flip to
FIXED with a second transcript attached.

## Move 3 — Close the run latch: hoist the guard **and** restore the drained queues

- **[AGENT-020](gap-analysis/02-cyrup-agent.md)** (critical) and
  **[AGENT-030](gap-analysis/02-cyrup-agent.md)** (high), as one change.
- In parallel, in a second worktree: start batch 5 (the paste invariant) — it contends with nothing
  in `cyrup-agent`.

**Open first:** `crates/cyrup-agent/src/agent.rs:1646` — `let steering = lock(&self.steering).drain();`
(and `:1650` for the follow-up drain), both of which run **before** `start_run` claims the latch at
`:1672-1682`. Add `PendingQueue::push_front` in `crates/cyrup-agent/src/queue.rs`; then switch
`crates/cyrup-session-svc/src/session.rs:627` and `:854` off `is_streaming()` onto a new
`is_run_active()` reading `driver_tx`.

**Done when:** two tests fail before the change and pass after — (a) `steer('keep-me')` with the
latch held, `continue_run()` returns `Err(RunActive)` **and** `has_queued_messages()` is still true;
(b) a prompt submitted in the post-run gap during auto-compaction is queued as steering instead of
starting a second run. Both drained vectors are restored via `push_front`, not merely guarded — **a
reviewer who accepts the hoist alone has accepted a narrower race window, not a fix.** Plus a live
run: type during a streaming turn, interrupt, and confirm the typed text arrives on the next turn
instead of vanishing.

---

# 2 · The batch sequence

**Two standing dependencies, not repeated in every row.**

1. **Every batch from 4 onward depends on batch 1's build-and-suite result** — you may not fix what
   nobody has built. *Where I refine the execution critic:* it asked for a blanket dependency on all
   of batch 1. I make the blanket dependency batch 1's items 1-3 (build, real test counts,
   prerequisites) and add a **row-level** dependency only where a batch owns an item that has a repro
   row — batches 4, 5, 6, 7, 14, 16, 17 and 29 may not start until *their own* rows are
   dispositioned. A crate with no repro rows (intercom) should not wait on a transcript that will
   never mention it.
2. **Every batch that converts a wall-clock test, consumes the unwired-symbol machine list, or cites
   the advertise-vs-consume checker depends on batch 3.** That is batches 7, 9, 10, 11, 12, 13, 14,
   15, 16, 17, 18, 20, 21, 22, 23, 24, 27, 28, 29 and 30 — i.e. nearly all of them. Batch 3 blocks
   nothing structurally (it touches no crate any other batch touches) but its output is the
   acceptance criterion downstream, so it must land early and it must not be skipped.

**Re-plan checkpoints.** After batches **11, 18, 22, 24 and 29** — the five with a first-ever read of
a large surface — newly filed items are assigned to an existing batch or to a new one *before* the
next batch starts. Without this, everything those reads find has no route into the schedule.

## Sequence

| # | goal | items | crates | size | depends on |
|---:|---|---|---|:--:|---|
| 1 | Boot it: build, run, and write down what the binary does | 16 repros + build + suite + fixtures | all (observation only) | S | — |
| 2 | Force the nine decisions | 9 OQs + ADR-0001 + 2 leads | — (docs) | S | — |
| 3 | The lying-control detector | 6 tools + PROV-041 + F4 graph | `xtask`(new), `cyrup-test-support` | **L** | — |
| 4 | Nothing you type while the agent is busy is lost | AGENT-020, AGENT-030 | agent, session-svc | S | 1 |
| 5 | The first prompt arrives intact | TUI-042/043/044/049/048 + editor read | tui | M | 1 |
| 6 | Remote terminals stop aborting your turns | TUI-045/050/047/046/039/005/009/040/013, TUI-S02, TUI-S10, CFG-045, DRIFT-045, UW-1 | tui | M | 1, 5 |
| 7 | A configured deny actually denies | PERM-009, PERM-023, PERM-017/019-022/024-031 | permission-system | M | 1, 3 |
| 8 | One keybinding id registry | CFG-048, TUI-028, TUI-008, TUI-051, TUI-035, CFG-038, TUI-N05, EXT-039, EXT-040, SEAM-067 | cyrup, tui, config, ext | M | 3, 6 |
| 9 | What `bash` is | TOOL-039+TOOL-007, TOOL-038/036/031/040/037/020/026/030, SEAM-015, DRIFT-029, DRIFT-046, DRIFT-004 | tools, session-svc, modes | M | 2, 3 |
| 10 | File tools and the mutation lock | 15 TOOL items | tools | M | 3, 9 |
| 11 | Your provider works, or it is not offered | PROV-027+028, PROV-029, +18 more, + closure audit | provider | L | 3 |
| 12 | Provider data boundary | PROV-048/049/050, PROV-030, PROV-018+039, +11 more | provider, config, xtask | L | 11 |
| 13 | One HTTP egress policy | PROV-047, CFG-006(+AGENT-031), PROV-043, PROV-044, DRIFT-014 | provider, agent, ext, config | M | 3, 12 |
| 14 | The first sixty seconds: argv, launch, trust, resume | 7 highs + 17 more + UW-2 | cyrup, tui, session-svc, config, ext | L | 2, 3, 8, 11 |
| 15 | Background processes stop, and clean up | SEAM-047+008+059+DRIFT-049, SEAM-S03, SEAM-070, SEAM-016/006/054, DRIFT-050/051 | cyrup, modes, tools, **session-svc** | M | 3, 9 |
| 16 | `/tree` stops writing labels; compaction stops billing after cancel | TUI-027, SESS-S05(+SEAM-060), SESS-040/041/042, TUI-031, TUI-016, +11 more | tui, session, session-svc, modes, provider, config | L | 3, 8 |
| 17 | The extension sandbox, before the first third-party guest | EXT-054, EXT-055, EXT-033/025/036/026/032/045/051 + symbol read | ext | M | 1, 3 |
| 18 | **The `8902b4f` closure audit, and the six holes inside it** | UW-3…UW-8 + 34 337-line audit + harness measurement | ext-subagents, tui | L | 3 |
| 19 | One WIT bump, thirty-four items | TOOL-021 first, cluster F2, TUI-014/030/033 | 8 crates | **XL** | 10, 17, 18 |
| 20 | Extension dispatch, events and renderers | 22 EXT/TUI/DRIFT items | ext, tui, session-svc | L | 3, 19 |
| 21 | Delegation you can trust: the advertised surface | SUBA-043, SUBA-014, `deny_unknown_fields`, +21 more | ext-subagents | L | 3 |
| 22 | Subagents: lifecycle and the crate tail | SUBA-049, SUBA-051, +20 more, SUBA-005 discharge | ext-subagents | L | 18, 19, 21 |
| 23 | Permission-system: the unported UI and audit surfaces | PERM-007/008/011/012/013/014 | permission-system, ext, session-svc | **L** | 3, 7, 20 |
| 24 | Intercom A — enumeration and the broker | ~20 ICOM items + 68-export enumeration | intercom | L | 3 |
| 25 | Intercom B — transport, framing, liveness | ~14 ICOM items | intercom | M | 24 |
| 26 | Intercom C — TUI, overlays, renderers | ~11 ICOM items + UW-10 | intercom, tui | M | 24 |
| 27 | The session file and the system prompt | 18 SESS items (+DRIFT-024/016/035) | session, session-svc | M | 3, 16 |
| 28 | cyrup-agent: the turn-loop tail | AGENT-017+029, AGENT-021, +20 more, AGENT-019(+DRIFT-039), DRIFT-036 | agent, session-svc | M | 3, 4, 20 |
| 29 | RPC wire payloads | 10 SEAM items + inner payload sweep | modes, session-svc | M | 3, 15, 19 |
| 30 | TUI presentation, and the two open questions | 20 TUI items, CFG-021, TUI-019 (conditional) | tui, session-svc | L | 2, 3, 6, 16 |

---

## Batch 1 — Boot it

**Goal.** For the first time, someone builds cyrup, runs it in a real terminal, and writes down what
the assembled binary actually does — including whether the six criticals reproduce.

**Items.** As in *Move 1*: the build, the real suite counts, the credential/fixture prerequisites,
four launch modes, and the 16 repro rows.

**Why here.** It is the cheapest step in the plan (2-3 days) and the only one that touches the
instrument rather than the code. Eleven of the sixteen rows are items the ledger ranks in its top 17,
so this is not a detour around the criticals — it is the repro step that should have preceded them.
The standing project rule already says TUI work is not done until it is run in a terminal; the honest
extension is that an item is not *ranked* until someone has watched it happen.

**Verified by.** A checked-in `docs/gap-analysis/REPRO-LOG.md`, 16 rows, transcripts attached, TUI
rows from live runs only.

**Risk — this batch is a branch, not a formality.**
- **≥2 rows REFUTED or BLOCKED** → re-plan. Every unreproduced critical/high drops to *lead* status
  and a full repro pass over all 28 crit+high is inserted before batch 4.
- **The binary does not boot cleanly, or the assembled app has layout/empty-state defects no item
  describes** → item-by-item parity is the wrong frame and the plan pivots to end-to-end bring-up.
- **The suite is not green** → cluster F3's premise ("the suite is at zero failures, so a new red is
  signal") is false and the test-defect work moves ahead of everything.

## Batch 2 — Force the decisions

**Goal.** Nine questions only the maintainer can answer stop blocking work. Runs as a parallel track
from day one, because it is the only batch engineering cannot accelerate.

**Items.**
- Answer **OQ-1** (bash scope: TOOL-039 + TOOL-007 as **one** decision — half of option (ii) is not
  an option), **OQ-2** (agent harness; batch 18 supplies the measurement), **OQ-3** (alt-screen mode:
  TUI-019 and the rendering half of CFG-021), **OQ-4** (chase upstream before or after the port
  bugs), **OQ-5** (Windows: PB-19, DRIFT-046, TOOL-036's win32 leg, TOOL-038 — 161 `cfg(unix)` sites
  against 6 `cfg(windows)`), **OQ-6** (SDK-surface parity), **OQ-7** (TUI-FIDELITY.md's ~150
  unidentified findings), **OQ-8** (CFG-005's OAuth acquisition cluster), **OQ-9** (first-run wizard
  UW-2 — wire `startup.rs:256` or delete the predicate and correct the trap list).
- Write ADR-0001 into this workspace or delete every reference to it. `spec/`, `ADR-0001` and
  `CLAUDE.md` do not exist here and code comments cite them.
- Settle the two leads with the commands area 12 already records: `DRIFT-023`
  (`ModelRegistry` → `ModelRuntime`) and `DRIFT-040` — neither side has ever been read.

**Why here.** Started late, these block batches 9, 14, 22 and 30 on human latency no engineering
removes; started day one they cost nothing. OQ-1 must be answered *in writing* before a line of the
shell batch is written, because TOOL-039 and TOOL-007 are mutually contradictory as shipped.

**Verified by.** Nine decisions living in checked-in ADRs in this workspace, each citable by ID from
a gap item. Every one of the nine trackers escalated with a named owner or closed with the decision
recorded. `rg 'ADR-0001|spec/architecture|R-[0-9]{2}-[0-9]{3}' crates/` returns only references that
resolve to a readable file.

**Risk — branch.** If OQ-2 returns "absorb the harness", ~11.4k insertions no area file owns enter
scope and every batch after 18 is re-sized. If OQ-5 returns "Windows is in scope", the 161-vs-6
`cfg` imbalance is a port-wide problem, not four items.

## Batch 3 — The lying-control detector

**Goal.** A reviewer can no longer merge code that advertises a capability the dispatcher never
reads, and a test can no longer pass by sleeping.

**Items.**
1. Create `crates/xtask` and add it to `Cargo.toml`'s `members` — **there is no `xtask` crate today**;
   `crates/` has 18 members and none is named xtask. Every "run `cargo xtask …`" line elsewhere in
   this plan depends on this step.
2. `cargo xtask lint-unwired`: parse every crate's `src/**` with `syn`; flag `pub` items whose only
   referencing sites are under `tests/` or `#[cfg(test)]`; ship a reviewed allow-list with one reason
   per entry in the same diff (it will false-positive on genuine SDK surface — `cyrup-sdk`,
   `cyrup-ext-sdk`).
3. A **never-constructed enum variant** check, separate from (2). This is not the same analysis and
   the two must not be conflated — see the acceptance criteria.
4. The advertise-vs-consume drift check in `cyrup-test-support`, built as a **two-sided diff against a
   checked-in copy of the upstream schema at the ported tag** (`pi-subagents/src/extension/schemas.ts:349`
   @v0.43.0 for the subagents case), not as an internal consistency check.
5. A deterministic rendezvous + fake-clock primitive in `cyrup-test-support` — the shared root of 11
   of the 23 wall-clock test defects.
6. **[PROV-041](gap-analysis/01-cyrup-core-and-provider.md) itself**, which is three concrete text
   corrections and not merely a lint proposal: `crates/cyrup-provider/src/collection.rs:306-307`,
   `crates/cyrup-ext-subagents/src/extension.rs:11299-11302`, and
   `crates/cyrup-provider/src/providers/openai_codex.rs:134-136`.
7. **Create** the citation lint (there is none to widen) covering `docs/gap-analysis/*.md` against
   upstream worktrees pinned at v0.83.0 / v0.43.0 / v0.7.1 / v0.9.2, rejecting the string "identical
   at both tags" outright.
8. Publish the failing lists into `docs/gap-analysis/` as the F1/F3/F4 work register, and execute the
   F4 `duplicate-of:` graph across all twelve files.
9. One doc correction, because it will otherwise be re-litigated forever: **`EXT-055`'s title, status
   rows (`06-cyrup-ext.md:102`, `:141`) and body name `FsCaps::with_fs_root`, a symbol that does not
   exist.** `grep -rn "with_fs_root" crates/` returns zero. The real mutator is
   `GuestState::with_fs` at `crates/cyrup-ext/src/host/services.rs:1210`, which the item's own prose
   quotes correctly at `:1211`. Fix the name in the item.

**Why here.** Third, not zeroth — but before every crate batch, because each of those uses its output
as an acceptance criterion. It touches no crate any other batch touches, so it reviews and lands in
parallel with batches 4 and 5 and blocks nothing structurally. "Advertised but inert" is the dominant
defect shape in this analysis (≥31 IDs across ten areas, two criticals, four highs) and it has grown
every pass; it has been "suggested-order item 0" for three editions and was never built precisely
because it was framed as a prerequisite to everything.

**Verified by — each guard must FAIL on a known-bad case before it is trusted.**
- `cargo xtask lint-unwired` must emit ≥31 symbols and **must include** `inputs_fingerprint`
  (`crates/cyrup-session/src/prompt/builder.rs:224`, referenced only by `prompt/tests.rs`),
  `get_auth_status` (`crates/cyrup-config/src/auth.rs:273`), `filter_github_copilot_models`
  (`github_copilot.rs:363`) and `GuestState::with_fs` (`host/services.rs:1210`).
- **Deliberately NOT in that list, against the earlier draft of this plan:** `abort_compaction` has a
  production caller — `crates/cyrup-session-svc/src/command.rs:117`, inside the `C::AbortCompaction`
  arm — and `Snapshot::col` is a private field that **is** read in production at `editor.rs:1221`.
  SESS-040's real defect is that the `AbortCompaction` **variant** (`command.rs:32`) is never
  *constructed*, which is what check (3) exists for; TUI-044's real defect is that `undo()` writes
  `self.col = self.col.min(…)` instead of `snap.col`. A caller lint can see neither. I verified all
  three of these by reading the code.
- `cargo test -p cyrup-test-support advertise_vs_consume` must fail today naming **SUBA-043** — and
  it can only do so as a two-sided diff, because SUBA-043 is a property that is **absent** from the
  45 advertised properties (`extension.rs:6543`, `props.insert` × 45, no `outputSchema`). An
  advertised-implies-consumed check has no row to fail on. **PROV-029 is a caller-lint case, not a
  schema case** — a dead flow registry — and is scored there.
- Rendezvous primitive proven by converting **TOOL-020** (a wall-clock test in `cyrup-tools`) and
  running it 100× at `--test-threads=1` and `=16` with zero flakes. *Not* AGENT-019 — that item is
  owned by batch 28 and must not be double-booked.
- **A passing check on day one means the check is wrong.**

**Risk — branch.** If the unwired lint returns ≫31 members, cluster F1 is the dominant class by a
wide margin and batches 20-30 are re-driven off the machine list rather than the hand-counted IDs.

**Size: L, not M** — a new workspace crate, a `syn`-based whole-workspace analysis, a cross-cutting
schema differ, a clock primitive, a markdown citation lint against four pinned worktrees, and the F4
graph. The UI-string audit ("every affordance literal resolves to a live handler") was **cut** from
this batch: it is an unbounded static-analysis problem, and the affordance class is covered
empirically by the mandatory live runs in batches 6, 8, 14, 16 and 30.

## Batch 4 — Nothing you type while the agent is busy is lost

**Goal.** A steering message typed while a turn is in flight is never destroyed, on either branch of
the seam.

**Items.** [AGENT-020](gap-analysis/02-cyrup-agent.md) (critical) — hoist the `is_running()` check
*and*, the load-bearing half, capture each drained vec and restore it via a new
`PendingQueue::push_front` before propagating `Err(RunActive)`.
[AGENT-030](gap-analysis/02-cyrup-agent.md) (high) — add `is_run_active()` reading `driver_tx`,
switch `session.rs:627` and `:854` off `is_streaming()`, route post-run-gap submissions to
`queue_steer`/`queue_follow_up`.

**Why here.** The smallest complete diff in the backlog and the #1-ranked critical: two items, one
latch, two files. The ledger states plainly that fixing either alone moves the data loss to the other
branch, so they are one change or they are a regression. Nothing else in the plan touches `queue.rs`
or the `driver_tx` latch, so it runs concurrently with batches 3 and 5 at zero contention.

**Verified by.** Move 3's two red-today tests, then a live run.

**Risk.** The fast-path `is_running()` check is racy in Rust where pi gets atomicity from
single-threaded JS. Review must confirm the restore-on-`Err` path, not just the hoist.

## Batch 5 — The first prompt arrives intact

**Goal.** What the user sees in the prompt editor is what the model receives — no editing sequence
can silently substitute a 20-character marker for a 2 000-character paste.

**Items.** [TUI-042](gap-analysis/07-cyrup-tui.md) (critical) — add `pastes: BTreeMap<u32,String>` +
`paste_counter` to `Snapshot`, clone in `snapshot()`, restore in `undo()`, carry the same on
`history_draft` (`:93`, `:1199`, `:1218`), which reuses `Snapshot`.
[TUI-043](gap-analysis/07-cyrup-tui.md) (critical) — port `word-navigation.ts`'s `isAtomic` branches
(`:44-46` backward, `:97-99` forward) into `word_left_target` (`editor.rs:1074`) and
`word_right_target` **literally**, and make `delete_word_backward`/`forward` drop the registry entry
the way `backspace()` does at `:814`. [TUI-044](gap-analysis/07-cyrup-tui.md) — `undo()` discards
`snap.col`; ships with TUI-042 so `Snapshot` is corrected once.
[TUI-049](gap-analysis/07-cyrup-tui.md) — `marker_at` accepts any text between `[paste #N ` and `]`.
[TUI-048](gap-analysis/07-cyrup-tui.md) — word navigation classifies by character class instead of
Unicode word segmentation.
**Opening read:** `crates/cyrup-tui/src/editor.rs` (2 254 lines) line-for-line against
`pi packages/tui/src/components/editor.ts` @v0.83.0 (area 07 blind spot 9).

**Why here.** Two criticals, one crate, one invariant — the paste registry is a second piece of
editor state that nothing keeps in sync with the buffer, and three of the five items edit the same
7-line `Snapshot` struct at `editor.rs:73-77`. Shipped separately that is three conflicting edits to
one struct and three reviews of the same reasoning. The line-for-line read rides along because all
four items were found from **outside** `editor.rs`, which strongly implies more inside it.

**Verified by.** `tests/editor.rs`: paste 1500 chars → DeleteCharBackward → Undo → `expanded_text()`
still contains 1500 chars (returns the 21-char marker today); paste → Undo → paste again yields
marker #1, not #2; Ctrl+W at the end of `[paste #1 +42 lines]` removes the whole marker *and* its
registry entry; a hand-typed `[paste #1 see above]` does not expand; the history-draft path covered
explicitly. **Then a mandatory live terminal run:** bracketed-paste a 42-line file into a real
session, Ctrl+W, undo, Enter, and read the session JSONL to confirm the user entry holds the 42
lines. `TestBackend` closes none of this.

**Risk.** TUI-043's marker-atomic branch touches `take_range`, which every deletion path routes
through — a wrong boundary makes ordinary word deletion off by one. Port pi's branches literally.
**Branch:** if the line-for-line read of `editor.rs` yields ≥3 new items, the editor is a floor like
every other surface and a second editor batch is inserted before batch 8 rather than the findings
being spread across the tail. **An empty result must be stated, not implied.**

## Batch 6 — Remote terminals stop aborting your turns

**Goal.** An escape sequence split across `read(2)` boundaries is reassembled instead of being
delivered as a bare Escape plus literal characters, so a keypress over SSH, mosh or tmux never aborts
a running turn or types junk into the prompt.

**Items.** [TUI-045](gap-analysis/07-cyrup-tui.md) (no sequence-reassembly stage of cyrup's own;
crossterm emits a lone Esc whenever a read does not fill its 1024-byte buffer and ends on `0x1B`,
and the tail decodes as text) · [TUI-050](gap-analysis/07-cyrup-tui.md) (8-bit meta byte dropped
instead of converted to ESC + char) · [TUI-047](gap-analysis/07-cyrup-tui.md) (a late or unsolicited
DCS/APC frame shredded into ~20 typed characters; `stray_reply.rs` recognises only OSC 11) ·
[TUI-046](gap-analysis/07-cyrup-tui.md) (cyrup pushes Kitty keyboard flag 1 where pi pushes 7, and
neither guard flag 7 requires exists) · TUI-039 · TUI-005 · **TUI-009** (+CFG-045's inert
`doubleEscapeAction`) · TUI-S02 (dead-terminal EIO/EPIPE emergency exit) · TUI-S10 · TUI-040 ·
TUI-013 · DRIFT-045 · **[UW-1](gap-analysis/PARITY-GAPS.md)** — the native modifier probe has no
production caller (`native_modifiers.rs:62`), so the Apple-Terminal Shift+Enter rescue never fires;
it is terminal input and it belongs here, not in a deferral list.
**Opening read:** `pi packages/tui/src/stdin-buffer.ts` (434 lines, draws nothing, named by no file
before the repair pass) in full, as the port target.

**Why here — and with a corrected argument.** The earlier draft justified promoting this batch by
saying `app.rs:6585` documents an Esc during a turn as discarding every queued steering message.
**That is a misread and I confirmed it.** `crates/cyrup-tui/src/app.rs:6581-6592` is
`AppAction::InterruptRestoreQueued`, which drains both queues and calls `restore_queued_to_editor`
*before* `session.abort()`; the sentence at `:6584-6586` reads "**Without the restore**, an Esc
during a turn silently discards…" — it is the counterfactual justifying the code that is there, and
`app.rs:1911-1913` routes Esc to `InterruptRestoreQueued` when a queue exists. **The data-loss half
of the argument does not survive; the abort half does.** A fragmented arrow key over SSH/mosh/tmux
still aborts the running turn and types `[A` into the prompt, the user blames their terminal, and it
is therefore never reported. `stray_reply.rs:29-32` records having *seen* this exact split in the
wild while rescuing only OSC 11. That is enough to promote it above its medium/low labels, on
population × frequency × silence. **A correction is owed against `07-cyrup-tui.md`'s TUI-045 Impact
paragraph** — it is the same self-certifying-comment failure `README:208-212` warns about, read
backwards. Cohesion is untouched: TUI-045's own Fix says to scope TUI-046/047/050 with it.

**Verified by.** Byte-level driver: feed ESC and the rest of a CSI sequence in separate `read(2)`
chunks and assert **one** Up event, not Esc + `[` + `A`; same for a split DCS/APC frame and an 8-bit
meta byte. **Then mandatory live runs, because the fragmentation is a transport property no test
backend reproduces:** ssh to a second box (or tmux through a throttled pipe), start a long streaming
turn, hold an arrow key, confirm zero Escape-aborts and zero literal `[A`; repeat under mosh; verify
a kitty-protocol terminal (kitty/ghostty/foot) does not duplicate characters after the flag change;
verify a dead terminal (EIO) exits cleanly; verify double-Escape does what `/settings` advertises.

**Risk.** The tractable fix is a small cyrup-owned pre-parser between `read(2)` and crossterm, fed by
a raw-fd reader — a design decision on the hottest path in the TUI. Port pi's 10 ms bound and its
`isCompleteSequence` classifier **literally**: too short and the bug survives, too long and Escape
feels laggy. TUI-046 must not ship alone — raising Kitty flag 7 without its guards duplicates
characters and leaks CSI-u text, a louder regression than the bug. **Branch:** if `stdin-buffer.ts`
turns out to have no tractable Rust analogue at the crossterm boundary (i.e. the pre-parser must
replace crossterm's reader rather than sit in front of it), this becomes an L and TUI-046 is held
back rather than shipped on a half-built pipeline.

## Batch 7 — A configured deny actually denies

**Goal.** Every deny the permission system advertises is attached and enforced — no tool-level deny
defeated by a narrower allow, and no operator whose only artifact is a persona's frontmatter left
with silently inert rules.

**Items.** [PERM-009](gap-analysis/10-cyrup-permission-system.md) (critical) — delete the cyrup-only
bash bypass at `extension.rs:1651-1653` and its justification comment at `:1624-1631`.
[PERM-023](gap-analysis/10-cyrup-permission-system.md) (high) — `is_installed` returns true when
`<agent_dir>/agents/` or `<cwd>/.cyrup/agent/agents/` exists and is non-empty. Plus PERM-017,
PERM-019 through PERM-022, PERM-024 through PERM-031.

**Why here.** Fifteen of the area's twenty-one items in one crate, most in one 3 315-line file.
PERM-009's fix is a three-line deletion whose entire cost is re-reading `shouldExposeTool` against
both upstream tags — do that read once and it pays for fifteen items instead of one. A security
control that fails open is also the purest silent defect: the user cannot observe it, and when they
eventually can, it retroactively invalidates every assumption they made about everything else.

**Verified by.** The bypass end to end: `tools.bash: deny` plus `bash: {"git status": allow}` →
bash **absent** from the exposed tool set *and* a `git status` call does not execute (red today on
both). The mirror case (`tools.bash: deny` with no command rules) proves the ordinary deny path
survives. `tests/context_hygiene.rs:128-152` stays green unchanged. A project whose only artifact is
a persona `permission:` frontmatter → `is_installed() == true` and the deny enforced end to end,
serialised on `ext_config::env_lock()` using batch 3's primitive. Three F3 test defects in this crate
converted off wall-clock.

**Risk.** PERM-009's branch cites a spec mandate and `spec/` is absent from this workspace. Per
`README:208-212` an unverifiable in-source claim is not a decision of record, so **the deletion
proceeds** — but raise it in the same PR (OQ-6 below). If the mandate is later produced, the correct
shape is still not the current one: pi's read/skills bypass is paired with a handler that **re-gates**
execution, so any bash analogue must re-gate to the allow-listed commands rather than defer to
`manager.rs:205-215`'s command-first precedence.

## Batch 8 — One keybinding id registry

**Goal.** A user carrying a pre-rename `keybindings.json` gets it repaired and honoured, every id the
app advertises is bindable, and `/reload` re-reads the file — instead of the file being parsed once
at boot and ignored entry by entry in silence.

**Items.** [CFG-048](gap-analysis/05-cyrup-config-and-resources.md) — port pi's sixth startup
migration (`migrateKeybindingsConfigFile`, 59 legacy names) at write time **and** read time; the
table must map to cyrup's *current* `editor.*` spelling so it is correct at HEAD, and gain
`editor.* → tui.editor.*` rows in the same change as TUI-028.
[TUI-028](gap-analysis/07-cyrup-tui.md) — the editor/input ids use an `editor.*` namespace upstream
abandoned; 24 ids inert. [TUI-008](gap-analysis/07-cyrup-tui.md) — seven upstream global ids
unbound. [TUI-051](gap-analysis/07-cyrup-tui.md) — `/reload` never re-reads `keybindings.json` while
both its help text and its own source comment (the `C::Reload` arm at `app.rs:4235-4243`) claim it
does. TUI-035 · CFG-038 · TUI-N05 · EXT-039 · EXT-040 · SEAM-067.

**Why here — the plan's load-bearing inversion.** `keybindings.json` is read exactly once, at boot,
and no other surface reads it. CFG-048's own text and four passages of `07-cyrup-tui.md` (`:99`,
`:237`, `:564`, `:1084`) say the migration table must land **before** TUI-028's rename or it silently
breaks every `editor.*` config written against shipped cyrup — and before TUI-027 (batch 16), which
adds seven new `app.tree.filter.*` ids plus four rebinds. **CFG-048 is a medium scheduled ahead of a
critical, deliberately**, because ten separate landings means ten passes over `Action::from_id` and a
guaranteed ordering bug. Placed after batch 6 so that only one batch holds `cyrup-tui` at a time.
**Cross-crate edit order:** `cyrup-config` (migration table) → `cyrup` (startup wiring) → `cyrup-tui`
(id registry + `/reload`) → `cyrup-ext` (EXT-039/040), one commit each.

**Verified by.** `migrations.rs` unit tests: `{"cursorUp":"ctrl+p","interrupt":"ctrl+q","app.clear":"ctrl+k"}`
migrates to cyrup's declaration order with pi's trailing newline; a second run is a no-op; the
collision `{"interrupt":…,"app.interrupt":…}` keeps only the modern key; a read-time test binds a
legacy id without the file ever being migrated on disk. **Mandatory live run:** launch with a
pre-rename `keybindings.json` where `cursorUp: ctrl+p` actually moves the cursor; edit the file on
disk; `/reload`; confirm the new binding takes effect without restart. Then assert all 59 targets
resolve.

**Risk.** Ordering is load-bearing and easy to get wrong — landing TUI-028 first silently breaks
every user config in the field. **First action of the batch:** check whether EXT-039/EXT-040 need a
`world.wit` export change for `register-shortcut`'s description; if either does, it moves to batch 19
and this batch ships without it rather than dragging an ABI bump into a keymap diff.

## Batch 9 — What `bash` is

**Goal.** One written answer to "does cyrup constrain what the model may do through bash, and who
chooses the interpreter" — and code that matches the answer on every path, including the RPC backend
override.

**Items.** [TOOL-039](gap-analysis/04-cyrup-tools.md) (high) + TOOL-007 as **one** change per OQ-1's
answer; recommended: delete the `CYRUP_SHELL` arm at `ops/shell.rs:101-105` and require the
`shellPath` setting — three lines, pi's shape. Plus TOOL-038, TOOL-036 (+DRIFT-046), TOOL-031,
TOOL-040, TOOL-037, TOOL-020, TOOL-026, TOOL-030; [SEAM-015](gap-analysis/08-cyrup-session-svc-and-modes.md)
(+DRIFT-004) — the RPC `operations` backend override, the same surface from the wire side; DRIFT-029
— a single cancel slot makes abort miss and `is_bash_running` lie.

**Why here.** The two items are mutually contradictory as shipped and the analysis proves it: TOOL-007
concedes the `ProtectedFs` guard is security theatre **because** bash is undecorated, while TOOL-039
shows that same undecorated bash runs under whatever interpreter the ambient environment names —
first arm of `ShellConfig::detect()`, structurally impossible to place in `session_env_scrub_keys()`,
propagated into every subagent re-exec, with nothing recording which shell ran. "cyrup constrains
what the model can do through bash" and "cyrup does not control which shell bash is" cannot both be
true. Early because the code fix is three lines — the cost is the decision, which batch 2 made.
**Cross-crate edit order:** `cyrup-tools` first (the interpreter decision), then `cyrup-session-svc`
+ `cyrup-modes` for SEAM-015's wire override, which reads the settled shape.

**Verified by.** With `CYRUP_SHELL=/bin/false` exported into a **live** session, a model-issued bash
call runs under `/bin/bash` or refuses with the named message — and the same holds through a subagent
re-exec, the path the env scrub cannot reach. With the guard on, `write` to `.env` refused **and**
`bash 'echo x >> .env'` refused; with it off both succeed and no `ProtectedFs` is in the chain. RPC:
a `bash` call with an `operations` override reaches the named backend. Concurrency: two user bash
runs, abort one, assert the right one dies and `is_bash_running` is accurate. Three wall-clock test
defects converted to batch 3's rendezvous.

**Risk.** Option (ii) is legitimate **only** if all four limbs land: a `[CYRUP-DELTA]` stamp, the
resolved interpreter reported at session start *and* in bash result details, a second explicitly-named
scrub group (it cannot fit the `{CYRUP,PI}_<SUFFIX>` shape), and path validation per `shell.ts:73`.
A reviewer must reject a half-(ii).

## Batch 10 — cyrup-tools: file tools and the mutation lock

**Goal.** `read`/`edit`/`write`/`find`/`grep`/`ls` behave as pi's do on cost, cancellation, error text
and path semantics — and the file-mutation lock stops blocking a tokio worker.

**Items.** TOOL-006, TOOL-011, TOOL-014, TOOL-017, TOOL-018, TOOL-019, TOOL-023, TOOL-024, TOOL-025,
TOOL-029, TOOL-032, TOOL-033, TOOL-034, TOOL-035, TOOL-041.

**Why here.** Same crate as batch 9 and therefore strictly after it — never two batches in one crate
at once. Split from 9 because the test surface differs (file fixtures and golden error strings versus
process/interpreter behaviour) and because the bash decision must not be held hostage to a
fifteen-item fidelity pass. It is really three shared roots on one fixture set: TOOL-023/033/034 are
one performance root (walk-then-truncate instead of bounding the walk), TOOL-019/025/032 are one lock
root, and the rest are pi's exact error bodies.

**Verified by.** Golden-file tests against pi's exact error bodies (`Error code: <ERRNO>`,
`Cannot read directory: <message>`); find/grep bounded-walk tests asserting the walk **stops** at the
limit rather than sorting after (measure syscalls, not wall clock); post-write cancellation re-check;
`FileMutationLocks::key` no longer calls blocking `std::fs::canonicalize` inside `guard()`. Four F3
test defects converted to batch 3's rendezvous and each run 100×.

**Risk.** TOOL-017 and TOOL-015 are marked "residual only" — re-read both against upstream before
editing, since a residual framing usually means a prior closure already moved the code. TOOL-015
belongs to batch 19, not here.

## Batch 11 — Your provider works, or it is not offered

**Goal.** Every request cyrup sends carries the headers and auth scheme pi sends on all three wire
routes, and Copilot/Codex `/login` actually reaches the flows that are already written.

**Items.** [PROV-027](gap-analysis/01-cyrup-core-and-provider.md) (high) +
[PROV-028](gap-analysis/01-cyrup-core-and-provider.md) (high) in **one** edit — 028 needs the exact
provider guard 027 introduces; port `github-copilot-headers.ts` as three pure functions applied on
all three routes. [PROV-029](gap-analysis/01-cyrup-core-and-provider.md) (high) — one field
assignment per provider in `providers/builtin_oauth.rs:37`; delete the prose exemption at `:14-16`;
then populate or delete the flow registry at `auth/oauth/load.rs:111`. Plus PROV-032, PROV-021
(+DRIFT-030), PROV-023, PROV-024 (+PROV-033, DRIFT-020), PROV-034, PROV-045, PROV-019, PROV-051,
PROV-003, PROV-031, PROV-015, PROV-017, PROV-037, PROV-S04, PROV-S05, DRIFT-013, DRIFT-028,
DRIFT-048, DRIFT-042.
**Opening read (structural defect E):** amazon-bedrock (`bedrock_converse_stream.rs`, 4 501 lines),
openai-codex (`openai_codex_responses.rs`, 2 103) and `pi_messages.rs` (1 636) against upstream at
v0.83.0 — 8 240 lines shipped by the same sweep as google-vertex with **zero** read-against-upstream
passes — plus `cf26010`'s nine unreviewed OAuth flows (~12k lines) and
`packages/coding-agent/src/bun/register-bedrock.ts`, named by no file in the directory.

**Why here.** Every item edits one of three `build_headers` functions or the auth resolution
immediately above them — one review of "how a request is shaped" pays for twenty items. The closure
audit **opens** the batch rather than being a separate step, because its named scope is entirely
inside this crate and the reviewer is already reading these files against upstream. "I have a Copilot
subscription and cyrup cannot use it" is a total blocker for a large population, and `/login`
dead-ends so they cannot even get started.

**Verified by.** Per-route request-capture tests asserting the exact header set pi sends: Copilot
Claude models on `Authorization: Bearer` with the selective betas and deliberately **no** Claude-Code
identity headers; `X-Initiator` / `Openai-Intent` / `Copilot-Vision-Request` present on all three
routes, ordered after `model.headers` and before `opts.headers` — an image turn against Copilot must
stop being rejected. Batch 3's caller lint flips from failing to passing on PROV-029; that flip is the
acceptance signal. **Manual and required:** a real Copilot login through `/login` followed by an image
turn, and one live request against bedrock and against codex. The audit is not closed by reading
alone, and an empty audit result must be **stated**, not implied.

**Risk — branch.** If the audit yields highs (base rate says 3-8), structural defect E becomes a hard
gate: no batch closes an item until the closing code has been read against upstream, adding ~30% to
every remaining batch. If it comes back clean (<2 items across 20k lines), the closure record becomes
trustworthy for the first time and the gate is dropped. Largest single-crate batch before the WIT
bump: if review load is too high, split at the audit boundary — but do **not** split PROV-027 from
PROV-028. **Re-plan checkpoint after this batch.**

## Batch 12 — Provider data boundary: JSON repair, catalogs, and the provider that cannot serve

**Goal.** A malformed or astral-plane SSE frame no longer kills an assistant turn, a pi-written
session JSONL resumes, the embedded catalogs are generated rather than hand-maintained, and no
provider is offered that cannot serve a request.

**Items.** [PROV-048](gap-analysis/01-cyrup-core-and-provider.md) (high) + PROV-049 + PROV-050 — the
same three-line predicate in `repair_json`'s three arms; a lone `\uD800` in an SSE frame currently
kills the whole assistant turn. [PROV-030](gap-analysis/01-cyrup-core-and-provider.md) (high) — take
the S-sized mitigation **first and unconditionally** (refuse at construction to push any provider
whose catalog names an api the registry does not `contains()`), with the `all.rs:12-47` port-status
doc rewrite **mandatory in the same diff** — `all.rs:12` still prints amazon-bedrock as "**pending**"
while `:177` pushes it — then port `google-vertex.ts` as `api/google_vertex.rs` after factoring the
google-shared converters out of `google_generative_ai.rs`. PROV-018 + PROV-039 — `xtask gen-catalogs`
against pi's committed generator plus the drift check whose absence is why nobody noticed 35 catalogs
against upstream's 39 (retires tracker `PROV-004`). Plus PROV-016, PROV-046, PROV-020, PROV-038,
PROV-014 (+DRIFT-019), DRIFT-009, CFG-019, CFG-041, PROV-025 (+DRIFT-027), PROV-040.

**Why here.** Two shared roots in the same crate sharing one golden-file suite: what cyrup parses off
the wire (`json_parse.rs`) and what cyrup embeds about providers (the catalogs). PROV-048 carries the
interop guarantee this port exists to provide — that a pi-written session opens in cyrup. PROV-030's
mitigation ships first on user-impact grounds: a user who sees google-vertex in `/model` and gets "no
API implementation" on every request is worse off than one who never sees it. Serialised after batch
11 by design — both are `cyrup-provider` (62k lines) and must never be in flight together.

**Verified by.** `parse_json_with_repair` on a `content_block_delta` carrying `"hi \ud83d there"`
returns `Some` with text `"hi  there"`, and a decoder test feeding that frame then `message_stop`
terminates with `StreamEvent::Done` rather than the parse-error terminal — plus a paired-surrogate
regression guard (😀 must survive as an emoji, not be dropped). A pi-written session JSONL containing
a lone surrogate resumes. Construction refuses any provider whose catalog names an unregistered api.
`cargo xtask gen-catalogs` reproduces the committed catalogs byte-identically and
`cargo test catalog_drift` goes red on a stale one. `all.rs:12-47` no longer contains the word
"pending" for any of the four registered providers.

**Risk.** PROV-030's full port is the only effort-L item in the crit+high set; if it slips, the
S-sized refusal plus the doc rewrite must still land. `gen-catalogs` depends on a node toolchain to
run pi's generator — size that question in the first hour; the fallback (a vendored generator port)
is L, not M.

## Batch 13 — One HTTP egress policy

**Goal.** Every byte cyrup sends obeys the same proxy, timeout and retry policy — OAuth flows, the
agent proxy transport and extension HTTP included, not just the streaming wire APIs.

**Items.** [PROV-047](gap-analysis/01-cyrup-core-and-provider.md) (high) — `build_client()` becomes
`build_client_for(target_url)` across seven call sites, plus the resolver and `.no_proxy()` in
`cyrup-ext/src/caps/http.rs`. CFG-006 (+AGENT-031) — the websocket-timeout duplicate pair, dissolved
by construction. PROV-043 — bedrock alone has no retry budget. PROV-044, DRIFT-014.

**Why here.** One shared root: every non-streaming egress path builds its own client. Threading the
target URL through the seven call sites closes the proxy hole, the websocket-timeout hole, the
bedrock retry hole and the extension cap's competing reqwest env detection in one diff; five separate
landings would each re-derive the same resolver. After the provider batches so `cyrup-provider` is
free, and before the ext batches so `caps/http.rs` is settled before batch 20 touches the loader.

**Verified by.** A local proxy that logs CONNECT; assert all seven paths transit it (five OAuth flows
at anthropic/openai_codex/xai/openrouter/radius, `cyrup-agent/src/proxy.rs`,
`cyrup-provider/src/wire.rs`) and that `cyrup-ext`'s http cap does too. Unit test that `no_proxy` is
honoured and reqwest's own env detection is disabled. Bedrock retry test with an injected 5xx.
DNS/transport failure classified retryable across all seven literals.

**Risk.** Touching four crates makes this the least cohesive batch in the plan — justified only
because the root is genuinely single. If review splits it, split by **call-site group**, never by
crate.

## Batch 14 — The first sixty seconds: argv, launch, trust, resume

**Goal.** Any pi command line launches cyrup, and the screens a user meets before their first prompt
tell the truth and record nothing they did not intend: no permanent trust verdict from a prompt
offering no way to decline, no cross-project sessions under a "Current Folder" header, no irreversible
delete, no discarded rename.

**Items (seven highs).** [SEAM-051](gap-analysis/08-cyrup-session-svc-and-modes.md) (shipped in move
2 if it has already landed; otherwise here) · [SEAM-064](gap-analysis/08-cyrup-session-svc-and-modes.md)
(likewise) · [SEAM-065](gap-analysis/08-cyrup-session-svc-and-modes.md) — delete the trust block from
`resolve_startup_ui`, give `SessionServiceBuilder` a `with_trust_prompt` callback fired only on
`TrustOutcome::NeedsPrompt`, restoring pi's tier order; retires `builder.rs`'s `saved: None` ·
[SEAM-062](gap-analysis/08-cyrup-session-svc-and-modes.md) ·
[SEAM-063](gap-analysis/08-cyrup-session-svc-and-modes.md) ·
[SEAM-061](gap-analysis/08-cyrup-session-svc-and-modes.md) (both halves land together or the screen
keeps lying) · [CFG-035](gap-analysis/05-cyrup-config-and-resources.md) — `.cyrup/SYSTEM.md` and
`APPEND_SYSTEM.md` are never discovered, so the trust gate **prompts the user to trust a file cyrup
will never read**. Plus SEAM-066, SEAM-068, SEAM-069, SEAM-052, SEAM-057, SEAM-029, SEAM-020,
SEAM-050, TUI-018, TUI-037, TUI-N04, EXT-003, CFG-013, CFG-049, CFG-050, CFG-051, CFG-047, **and
UW-2's implementation** — batch 2 decides it (OQ-9: wire `startup.rs:256` or delete the predicate and
correct the trap list), and this batch, which already owns `main.rs`'s startup block, *does* it. A
decision is not the work.

**Why here.** The surface every user meets before anything else, containing two irreversible acts: a
trust answer always persisted (including a permanent lockout, from a prompt that never offers to
reverse it) and a session delete with no undo where pi's is recoverable from the OS trash. It is the
clearest demonstration of this plan's criterion: seven highs plus mediums and lows on one screen
pair, all found by the `packages/coding-agent/src/cli/` sweep that had never been run, all needing the
**same live-run harness** — two project dirs, a stub `trash` on `PATH`, a fresh store, a non-default
theme, a rebound key — built **once** in batch 1 and reused. Internal ordering is serialised
(SEAM-065 and CFG-035 both edit `main.rs`'s startup block after 061/062/063/064 land).

**Verified by — a mandatory live terminal session over the whole surface, explicitly not TestBackend.**
`cyrup --tui-mode regular` starts and `--tui-mode bogus` prints exactly
`Invalid TUI mode "bogus". Valid values: regular, fullscreen` and exits 1; two project dirs →
`--resume` heads "Current Folder", lists only this folder's sessions, and Tab flips to "All" with the
cwd column on; rename a session and relaunch to confirm it survived; with `trash` installed, delete
one and confirm "moved to trash" and the file in the OS trash; in an untrusted folder with `.cyrup/`
resources, confirm **five** trust rows, pick "Trust (this session only)" and confirm `trust.json` is
byte-unchanged; set `"theme":"light"` and confirm every pre-launch surface renders light; rebind
`tui.select.confirm` and confirm both the binding and the printed hint follow. Unit: a stub
trust-policy extension gets first say and its verdict wins over the store; a trusted project's
`.cyrup/SYSTEM.md` reaches the system prompt and `--append-system-prompt X` appends **only** X (pi
replaces, it does not accumulate — correct `prompt/overrides.rs:15-16`, which documents accumulation).

**Risk.** SEAM-065 is structural and SEAM-064 changes the option set of the callback it introduces —
land them as one change or the callback renders the wrong options. SEAM-063's helper is called from
two sites (startup and `cyrup-session-svc`); both must convert or the swallowed-error half survives
in-app. SEAM-020's `--list-models` half shares the credential-blind `registry_models()` defect with
PROV-031 — confirm batch 11 landed the `getAvailable()` equivalent or this half regresses.

## Batch 15 — Background processes stop, and clean up after themselves

**Goal.** A supervisor can stop cyrup in every mode: the first SIGTERM/SIGHUP tears down the
**current** session, disposes the runtime, notifies extensions, kills detached children and returns
pi's exit code.

**Items.** [SEAM-047](gap-analysis/08-cyrup-session-svc-and-modes.md) (high) + SEAM-008 + SEAM-059 +
[DRIFT-049](gap-analysis/12-upstream-drift-pi-core.md) — four IDs, one defect, one function; publish
`ShutdownSignal` on a watch/oneshot and **add** a cancellation arm to `rpc_driver` rather than
restructuring its `select!`. SEAM-S03 — no detached-child registry: setsid-detached bash children
(dev servers, watchers) outlive cyrup on every exit path, holding ports and file locks. SEAM-070
(+DRIFT-051) — process-title role suffix; correct the `main.rs:53-57` comment in the same change
(`prctl` is a syscall on the current process and does not carry the `set_var` hazard the comment
cites). Plus SEAM-016, SEAM-006, SEAM-054, DRIFT-050.

**Why here.** Four IDs, one defect, one function — the cleanest illustration of this plan's criterion:
a severity cut books SEAM-047 now and SEAM-008/SEAM-059 never, leaving the exit codes computed and
unused and the watcher holding a disposed session while the live turn keeps burning tokens. The
affected population is embedders and operators, and within it the failure is total: you cannot deploy
under systemd or in a container without it. **Depends on batch 9** — SEAM-S03's edits are
`crates/cyrup-tools/src/ops/local.rs:272` and `:334` (the setsid/killpg path) and batch 9 is rewriting
how bash children are spawned; two batches must not hold that file. **Crate list includes
`cyrup-session-svc`**, which the earlier draft omitted while asserting that the signal must abort the
*replacement* session.

**Verified by.** Per-mode integration tests with a real child: SIGINT/SIGTERM/SIGHUP to `--mode rpc`,
`--mode print`, `--mode json` and interactive; assert the process exits with 130/143/129 respectively,
**after** `runtime.dispose()` ran and a `session_shutdown` reached a stub extension. After an RPC
`new_session`, the signal aborts the **replacement** session, not the disposed one (today the disposed
one is aborted and the live one keeps running). A setsid child spawned from a bash tool call has its
process group gone rather than reparented to init. **Manual:** run under systemd or docker, SIGTERM,
confirm exit within the stop timeout without SIGKILL; confirm `ps` shows `cyrup-rpc`.

**Risk.** The teardown rewrite touches the one path that must never hang. `run_rpc` is parked on a
stdin read no signal disturbs; the cancellation arm must set `reader_open=false` so the existing
drain-and-break runs rather than racing a second teardown path. Keep the second-delivery force-exit
intact as the escape hatch throughout.

## Batch 16 — `/tree` stops writing labels you never meant to write, and compaction stops billing after you cancel

**Goal.** Typing into the session tree filters it as it does upstream instead of persisting a label to
the JSONL; the Escape key the compaction indicator advertises actually cancels; and a prompt typed
during compaction is queued and **visible** rather than assembled against a context being rewritten
under it.

**Items.** [TUI-027](gap-analysis/07-cyrup-tui.md) (critical) — add `search_query` to `TreeSelector`,
accumulate printable chars in the fall-through arm (replacing the digit filter), rebind `z`/`x`/`e`/`t`
to alt+left / alt+right / shift+l / shift+t, add the seven `app.tree.filter.*` ids to
`TreeAction::from_id`, move filter modes 1-5 to pi's ctrl+d/t/u/l/a. SESS-S05 (+SEAM-060) — `get_tree`
drops pi's `labelTimestamp`. [SESS-040](gap-analysis/03-cyrup-session.md) (high) + SESS-041 + SESS-042
as **one** shipment — save and replace the default-editor Escape handler on `CompactionStart`, restore
on `CompactionEnd`, route through `command.rs:116-118` (and **construct** the `AbortCompaction`
variant, which nothing does today). [TUI-031](gap-analysis/07-cyrup-tui.md) (high) — check
`is_compacting()` before `is_streaming()` in the Submit arm, queue on a new
`AppState::compaction_queue`, suppress the optimistic echo, drain on `CompactionComplete`.
**[TUI-016](gap-analysis/07-cyrup-tui.md) in full** — not merely the `{n} queued` footer count. Its
upstream half is `interactive-mode.ts:4190-4207` `updatePendingMessagesDisplay`, which renders
per-message `Steering: {text}` / `Follow-up: {text}` rows above the editor plus the
`to edit all queued messages` hint, fed from `getAllQueuedMessages` folding `compactionQueuedMessages`;
cyrup's `QueueUpdate` (`app.rs:4612-4614`) discards the texts entirely. Restoring only a count would
close the symptom and leave the item below parity. Plus SESS-017, SESS-022, SESS-032, SESS-034,
SESS-028, SESS-030 (+SEAM-034), SESS-012, TUI-003, PROV-035 (+CFG-014, TUI-021), PROV-036
(+DRIFT-031). **TUI-009 is not here** — it is batch 6's, whose Escape rewrite owns the same
`Action::Interrupt` code and whose live run is the only one that tests it.

**Why here.** A keystroke that mutates durable user data is the worst defect in the backlog:
destructive **and** silent **and** the user believed they were doing something else. It sits here and
not earlier for exactly one reason: it needs batch 8's keybinding registry to hold the seven new ids,
or every user who has written a tree binding loses it silently. SESS-041/042 are latent **only**
because SESS-040 has no caller — wiring 040 alone activates two dormant defects, so the three ship
together or not at all.

**Verified by — mandatory live runs.** (a) Open `/tree` on a real session, type a word, confirm it
**filters** and fires no `TreeAction`; quit, reopen `/tree`, confirm no entry gained a label — then
check the session JSONL for appended label entries. Assert specifically that no
`SelectorOutcome::Apply` carrying `FIELD_SEP` is produced and `host_services.set_label` is never
called; verify **both** callers, since an extension's `setLabel` uses the same live path. (b) Start a
compaction, press Escape: it stops, nothing is appended to the session file, and the queued text is
delivered afterwards; repeat for an **auto** compaction. (c) Type a prompt mid-compaction and confirm
it is queued, rendered as a per-message row with its text, and drained exactly once. Unit: `get_tree`
returns `labelTimestamp` on labelled nodes and omits the key on unlabelled ones; `CompactionResult`
carries pi's usage; `rg -n AbortCompaction crates/` now shows a production **construction** site.

**Risk.** The `z/x/e/t` rebind is a breaking change for anyone who has learned cyrup's keys — correct
(they are pi's search characters) but it needs a changelog line and CFG-048's alias table must carry
it. The digit-filter arm is **replaced** by the printable fall-through, so filter modes 1-5 must move
to pi's ctrl bindings in the same change or the feature is lost rather than rebound.

## Batch 17 — The extension sandbox, before the first third-party guest

**Goal.** A WASM guest gets exactly the host surface its manifest declares, deny-by-default, before
the first third-party component ever ships.

**Items.** [EXT-054](gap-analysis/06-cyrup-ext.md) (critical) — `load_wasm` takes
`&ExtensionManifest` (or a resolved `Capabilities`); seed `ProcCaps`/`HttpCaps`/`FsCaps` in
`GuestState` from the grant instead of `Default`; make exec/net/ui host imports in `host/live.rs`
return a typed denial when the bit is false. [EXT-055](gap-analysis/06-cyrup-ext.md) — the mutator
with zero callers is **`GuestState::with_fs` (`host/services.rs:1210`)**, not `FsCaps::with_fs_root`,
which does not exist; ext-fs is permanently denied for every guest — the mirror failure, same root
cause, same change. Plus EXT-033, EXT-025, EXT-036, EXT-026, EXT-032, EXT-045, EXT-051.
**Opening read:** symbol-by-symbol enumeration of pi `core/extensions/{types,runner,loader,index}.ts`
(area 06 blind spot 8).

**Why here.** One crate, one call chain: `load_discovered` holds the manifest, `load_wasm`'s signature
cannot receive it, `GuestState` seeds `Default` capabilities, and `with_fs` has zero callers — four
IDs describing one broken seam. Blast radius today is zero shipping guests, which is precisely the
argument for landing it **before** the first third-party component rather than after. Deliberately
before the WIT bump so the grant model is settled before 27 new exports are added to a world that
would otherwise inherit the inert one. If a third-party extension story ships sooner than this batch,
it moves to the front unchanged.

**Verified by.** A guest whose manifest declares nothing gets typed denials from every exec/net/ui/fs
host import — not a panic, not a default-allow. A guest declaring only `{fs}` gets a root-scoped
`FsCaps` within the declared root and nothing wider. Both `loader.rs` synthesis sites (`:213`, `:259`)
proven to be the **empty** grant by a test that fails if either is widened. Batch 3's caller lint no
longer reports `with_fs`. A configured extension path that does not exist produces a diagnostic.

**Risk.** Deny-by-default will break any in-tree guest fixture that silently relied on the full
surface — that breakage is the **finding**, not a reason to widen the default. **Branch:** if the
`core/extensions/*` enumeration returns WIT-shaped items, they must be folded into batch 19's world
before it closes; if it returns >8 non-WIT items, a second `cyrup-ext` batch is inserted before 19
rather than pushing them into batch 20's tail.

## Batch 18 — The `8902b4f` closure audit, and the six holes inside it

**Goal.** The ~34 000 lines that landed in a single commit have finally been read against upstream,
and the six documented no-op holes inside them are closed.

**Items.** Read `8902b4f`'s three subtrees against pi-subagents @v0.43.0, measured: `watchdog/`
18 107 lines + `missions/` 7 371 + `tui/fleet*` 8 859 = **34 337 lines**, of which exactly six holes
are documented. Then close them: [UW-3](gap-analysis/PARITY-GAPS.md) (unread child NDJSON status),
UW-4 (the watchdog review that never runs a model turn, so **every review comes back silently clean**),
UW-5 (the permission arbiter that never runs a model turn, so **every `ask` denies**), UW-6, UW-7 (the
fleet widget receives no keystrokes — **consumer half only; its WIT export half is batch 19's**), UW-8
(mission workflow state never written). Also **measure** `pi packages/agent/src/harness/**` (file
count, line count, shape — no port, no items), so OQ-2 is decided against a number.

**Why here, and why it is its own batch.** The ledger ranks this audit at **position 2** on expected
yield (`00-residual-ledger.md:460-468`) and this plan honours that ranking for the 20k-line provider
half (batch 11) — deferring the larger 34k half to the back of a subagents batch was the earlier
draft's least defensible sequencing. It must also precede the WIT bump: a WIT-shaped finding
discovered *after* batch 19 closes forces the `0.6.0` bump that batch exists to prevent.
*Where I refine the execution critic:* it asked for the audit to be split out; I split it at the
**SUBA-item** boundary, not at the audit/fix boundary — the six UW holes stay with the audit because
they are inside the code being read and finding them and fixing them is one reading. Only the 20
crate-tail SUBA items move to batch 22.

**Verified by.** Every file in scope exits with a Coverage row naming the upstream file **and the tag
it was read at**, plus one of three dispositions: a filed item with two-sided citations at v0.43.0, an
explicit "confirmed-covered", or "mechanism-N/A" with a reason. **No file exits with silence.** Child
NDJSON status is read; review and the permission arbiter each perform a **real** model turn, and a
denied ask is denied for a reason rather than by default; mission workflow state is written.
**Mandatory live terminal run** for the fleet widget and the watchdog — `TestBackend` closes neither.

**Risk.** UW-4 and UW-5 are "implement a model turn inside a subsystem that shipped as a no-op" —
genuinely unknown effort. **Timebox:** if either exceeds M, make the arbiter fail **closed and
loudly** with a diagnostic and ship that, then file the model turn. A silent always-deny is the worst
of both worlds; a loud not-implemented is honest. **Branch:** if the audit yields highs, structural
defect E's gate extends to the subagents crate and batches 21/22/23 are re-sized; if it yields
WIT-shaped items, batch 19 is not safe to close until they are folded in. **Re-plan checkpoint after
this batch.**

## Batch 19 — One WIT bump, thirty-four items

**Goal.** Take the extension ABI to `cyrup:ext@0.5.0` exactly once, carrying every pending export
change, instead of thirty-four minor bumps and thirty-four guest-refusal cliffs.

**Items.** **TOOL-021 first, in its own commit** — `cyrup-core::Tool::prompt_guidelines` must return
owned strings before `impl Tool for WasmTool` can ever carry guidelines. Then cluster F2: EXT-009
(+PROV-042), EXT-014, EXT-015, EXT-016, EXT-021, EXT-023, EXT-024, EXT-035, EXT-037, EXT-042, EXT-043,
EXT-044, EXT-046, EXT-047, EXT-048, EXT-049, EXT-S04, EXT-028, SEAM-011 (+SEAM-028), SEAM-012,
SEAM-025, TOOL-015, TOOL-016, TOOL-022, PROV-011 (+DRIFT-018). Then TUI-014, TUI-030, TUI-033 — the
widget/header/footer surface needs the WIT payload before the TUI has anything to render. Fold in
whatever WIT-shaped items batches 11, 17 and **18** produced — this is the last cheap moment to add an
export. Decide the `on_terminal_input` seam (UW-7's WIT half, TUI-030) and the callable-API registry
(UW-9) here or they force a `0.6.0`.

**Why here.** `EXT-028`'s contract says any export change bumps the minor, so 34 IDs shipped
separately means thirty-four bumps and thirty-four guest-refusal cliffs. It is XL once versus M
thirty-four times.

**Size: XL, with an explicit internal sequence** — (1) TOOL-021's signature change, its own commit,
rippling through every `Tool` impl in the workspace, rebased green; (2) the conformance guest written
**first** against the intended 0.5.0 world, so the WIT is reviewed before any host code moves;
(3) host wiring; (4) the guest-refusal path. Five of its members (EXT-021, TOOL-022, PROV-011,
DRIFT-018, TUI-030) are effort-L in their own rows and it spans eight crates; "L" here would read as
a schedule rather than a size.

**Verified by.** `world.wit`'s two copies are byte-identical, both say `0.5.0` on line 1 and in the
package line, and `ABI_FINGERPRINT` changes exactly **once** across the batch. The conformance guest
exercises every new export: `toolName` and args reach `tool_execution_update`/`end`; session-lifecycle
events keep their discriminating fields; `project_trust` carries cwd; `ctx.cwd` resolves; `set-widget`
carries pi's key and placement; a guest tool's `promptGuidelines` reach the first system prompt; a
grammar-constrained tool call round-trips. A 0.4.0 guest is refused with a clear, actionable version
message rather than a panic. **Mandatory live terminal run** — TUI-014/030/033 are widgets, headers
and footers, i.e. things that draw; a conformance guest and a fingerprint check cannot see an
empty-state bug.

**Risk.** The only all-or-nothing batch in the plan. The 0.5.0 bump refuses every 0.4.0 guest by
design and needs an SDK migration note. If OQ-6 (SDK-surface parity) comes back "out of scope",
re-check the member list before starting.

## Batch 20 — cyrup-ext: dispatch, events and renderers

**Goal.** Extension commands, autocompletions, bus events and renderers behave deterministically and
reach the surfaces they are registered for.

**Items.** EXT-006, EXT-007, EXT-011, EXT-013, EXT-017, EXT-018, EXT-019 (+TUI-034, DRIFT-015),
EXT-022, EXT-029, EXT-030, EXT-031, EXT-034, EXT-038, EXT-041, EXT-050, EXT-052, EXT-053, EXT-056,
EXT-057, TUI-029, TUI-006, DRIFT-033.

**Why here.** Everything left in area 06 after the sandbox and the ABI, and it is one coherent
subject: registration order, dispatch determinism and event delivery. EXT-017/053/056 are one
first-wins-versus-last-wins inconsistency; EXT-018/034/050/057 are one bus; EXT-019/TUI-034/DRIFT-015
are one markdown-transformer defect with three IDs. After the ABI bump because several need the
payloads batch 19 adds.

**Verified by.** Command listing is deterministic and `name:N` disambiguation resolves; a colliding
command is executable and its shadowing is **diagnosed** rather than silently dropped; a bus event
emitted from inside an event handler is delivered; the round bound drops nothing silently and listener
faults surface; an extension renderer survives replay; a mid-run tool addition reaches the **rebuilt**
system prompt; an extension-supplied `streamSimple` fires `before_provider_request` and
`after_provider_response`. Batch 3's caller lint reports zero unwired symbols in this crate.
**Mandatory live terminal run** for TUI-029 — extension autocomplete providers never consulted by the
interactive editor is exactly the class where a green caller lint coexists with a dead affordance.

**Risk.** EXT-031 (turn-boundary refresh propagates tools but not the rebuilt system prompt) overlaps
AGENT-017's turn-update seam in batch 28 — agree the ownership boundary before both start, or the same
`TurnUpdate` struct gets edited twice.

## Batch 21 — Delegation you can trust: the advertised surface

**Goal.** Every parameter and config key the subagents tool advertises is honoured, and every
capability it honours is advertised.

**Items.** [SUBA-043](gap-analysis/09-cyrup-ext-subagents.md) (high) — a caller's `outputSchema` is
dropped without error and the run returns prose; add it to the 45 properties
(`extension.rs:6543`), deserialize onto `SubagentToolParams`, thread into both constructors
(`:1934`, `:2295`). [SUBA-014](gap-analysis/09-cyrup-ext-subagents.md) (high) — `requireReadTool`
unported: an agent with an explicit `tools:` list omitting `read` plus any resolved skill is told to
"use the read tool" it does not have, and the failure surfaces as a model apology. Add
`deny_unknown_fields` to `SubagentToolParams` and pin batch 3's two-sided schema diff to it — **the
shape fix, which prevents the next instance.** Plus SUBA-047, SUBA-046, SUBA-025, SUBA-048, SUBA-061,
SUBA-059, SUBA-045, SUBA-035, SUBA-038, SUBA-065, SUBA-066, SUBA-008, SUBA-044, SUBA-050, SUBA-052,
SUBA-053, SUBA-029, SUBA-030, SUBA-058, SUBA-060, SUBA-055.

**Why here.** One crate whose `extension.rs` is 19 698 lines — two people cannot be in it at once,
which forces subagents into sequential whole-crate batches under any criterion. This half is the
schema and config surface: the root is `additionalProperties: true` and the params struct has no
`deny_unknown_fields`, so "accepted, then silently ignored" recurs across a dozen items.
SUBA-044/050 are pi-subagents v0.43→v0.47 drift on the same surface and are absorbed here rather than
in a rebase — upstream made the shipped `reviewer` lane read-only while cyrup still grants it
bash/edit/write.

**Verified by.** Batch 3's advertise-vs-consume diff flips to passing for this schema, and a new test
asserts the enum of advertised properties **equals** the set the constructors read. A caller-supplied
`outputSchema` reaches both single-run paths and returns structured output. A skill-carrying agent
with an explicit `tools:` list omitting `read` gets `read` head-injected exactly when
`!resolved_skills.is_empty()`. Diff every file under `resources/agents/` against
`git -C pi-subagents show v0.47.1:agents/<name>.md` — only the researcher.md divergence (SUBA-062) may
remain, and only with a recorded `[CYRUP-DELTA]`. `modelScope.strict` hard-rejects; YAML literal block
scalars parse; `~` expands in chain paths. **Live run:** delegate to the shipped reviewer and to a
skill-carrying worker.

**Risk.** The crate's `settings.json` read-modify-write is unlocked (SUBA-029) — fix it in this batch
**before** the config-key items multiply the number of writers.

## Batch 22 — Subagents: lifecycle and the crate tail

**Goal.** Async children are bounded, steers are acknowledged, orphaned groups are impossible, and
the crate's remaining advertised-but-unbuilt surface is either built or descoped in writing.

**Items.** SUBA-049 (steer ack, delivery mode, `steeringRecovery` — a steer is fire-and-forget today),
SUBA-051 (30-minute default async **child** timeout; parents deliberately unbounded), SUBA-023,
SUBA-028, SUBA-031, SUBA-034, SUBA-037, SUBA-039, SUBA-032, SUBA-033, SUBA-054, SUBA-056, SUBA-057,
SUBA-063, SUBA-064, SUBA-021, SUBA-062, SUBA-016, SUBA-017, SUBA-022, SUBA-024, SUBA-026. **UW-7's
consumer half** (the fleet widget's keystroke routing) lands here against the export batch 19 added.
**Tracker SUBA-005 is discharged here:** named owners for `worktree.discard`, `approve-checkpoint`,
`reject-checkpoint`, `project.open`/`status`/`close`, `mission.resolve-decision`, plus a completeness
assertion pinning cyrup's 27-verb enum against a checked-in copy of upstream's 53-verb array.

**Why here.** The other half of the crate, after its audit (batch 18) and its schema surface (batch
21). Eight of its rows are effort-L in their own right (SUBA-016, SUBA-021, SUBA-022, SUBA-023,
SUBA-024, SUBA-026, SUBA-056, SUBA-062), which is why the audit is no longer inside it.

**Verified by.** An async child with no explicit bound is killed at 30 minutes; a steer returns an ack
and honours its delivery mode; dropping a drive future does not orphan a group (Drop guard); `wait`
scopes by session, not cwd. Live run for the fleet widget's keystrokes.

**Risk.** `workflowScript` (VL-S2) is a whole execution model nobody has decomposed — size it on day
one and split it into its own diff rather than letting it swallow the batch. **Re-plan checkpoint
after this batch.**

## Batch 23 — Permission-system: the unported UI and audit surfaces

**Goal.** An operator configures the permission system in a live overlay, duplicate asks collapse, and
the forwarding path writes the audit trail pi writes.

**Items.** PERM-007, PERM-008 (the forwarding path writes **no** audit entries at all — 8 review + 3
debug sites unported across 1 125 lines), PERM-011 (+UW-9), PERM-012, PERM-014 (+UW-15), PERM-013.

**Why here.** The six items batch 7 deliberately left behind, held until now because five need
`HostServices::open_overlay` and the extension event seam, which batches 19 and 20 settle. PERM-011
and PERM-014 are both "implemented and never wired", so batch 3's caller lint names them and this
batch discharges them. Closing it closes area 10 entirely: 21 of 21. **Size: L, not M** — six
effort-M items across three crates is an L in aggregate.

**Verified by.** `/permission-system` opens a **live overlay in a real terminal**, not a text return —
this is a rendering change and a live run is mandatory; toggling `yoloMode` inside it changes
`config.json` on disk **and** the running gate auto-approves the next ask. Two identical concurrent
asks produce **one** prompt. The forwarding path writes all 8 review and 3 debug audit entries.
`registerModelOptionCompatibilityGuard` strips temperature for models that reject it. Batch 3's caller
lint reports zero unwired symbols in this crate.

**Risk.** PERM-007's in-tree rationale ("HostServices exposes no custom-overlay seam") is **already
stale** and has a live impl plus a production caller — do not trust any other in-source rationale in
this crate without checking it. PERM-011 needs a callable-API publish seam; `SharedBus` is an event
bus, not a registry. Decide seam-or-delete in batch 19; do not leave three tested uninvokable methods.

## Batches 24-26 — cyrup-intercom, split by module

**Goal.** Messaging between sessions matches pi's semantics end to end — targeting, presence,
liveness, delivery metadata and copy — and the crate's zero highs stops being an artefact of never
having been swept.

The earlier draft made this one L batch. It is not one batch: **45 IDs in one crate, four of them
effort-L (ICOM-010, ICOM-016, ICOM-017, ICOM-042), plus a first-ever enumeration of 68 top-level
exports, plus `broker/mod.rs` (1 559 lines) and `transport/client.rs` (1 268) + `framing.rs` (367)
that have only ever been grepped.** The split is unconditional, not gated on what the enumeration
returns.

- **Batch 24 — enumeration and the broker (L).** Opens with pi-intercom's 68 top-level exports at
  v0.9.2, `broker/` first — no pass has ever walked this surface (area 11 blind spot 10). Then the
  broker-owned ICOM items: targeting and sender-ID disambiguation, presence, the pending-asker reply
  rule, steering a busy target rather than parking it until idle.
- **Batch 25 — transport, framing, liveness (M).** The v0.9.1 bounded-reader rewrite read properly
  rather than grepped; heartbeat/dead-client detection; `_deliveryMetadata_` on injected content;
  attachments surviving `reply`; a malformed config failing closed **loudly**. `lib.rs`'s version
  banner reads v0.9.2 (ICOM-012).
- **Batch 26 — TUI, overlays, renderers (M).** UW-10 (the intercom compose and session-picker
  overlays are render-only); renderers for `intercom` and `contact_supervisor`; the card not frozen at
  width 80.

**Why here.** 22 of the 45 are the v0.9.2→v0.10.1 window — one of exactly two places where version lag
**is** the work, because the drift is a whole crate rather than scattered lines. Its zero highs must be
read as un-swept, not clean. Independent of everything else, so it runs in parallel by a second person
from batch 14 onward.

**Verified by.** Two live cyrup sessions on one machine: send/ask by sender-ID prefix with all four
disambiguation errors; a send to the sole pending asker treated as its reply; a busy target
**steered** rather than parked; the heartbeat detects a dead client; `_deliveryMetadata_` appears on
injected content; attachments survive `reply`. Live terminal, resized, for batch 26. Two F3 test
defects converted off wall-clock.

**Risk.** The enumeration **will** file new items; budget for filing, not just fixing, and do not let
the new items block the 45 already scoped. **Re-plan checkpoint after batch 24.**

## Batch 27 — The session file and the system prompt

**Goal.** A session file round-trips without losing data, and the system prompt is assembled from what
pi assembles it from.

**Items.** SESS-004, SESS-007, SESS-013 (+DRIFT-024), SESS-014, SESS-016, SESS-018, SESS-019
(+DRIFT-016, DRIFT-035), SESS-024, SESS-025, SESS-026, SESS-027, SESS-029, SESS-033, SESS-035,
SESS-036, SESS-037, SESS-043, SESS-044.

**Why here.** Everything left in area 03 after the tree and compaction batch, splitting into exactly
two shared roots that share one crate and one fixture set: JSONL fidelity (unknown fields dropped on
rewrite, blank first line, `cwd: ""` headers, key order, `encode_cwd`) and system-prompt assembly
(docs pointer never emitted, skills preamble, the stale `Current date:` footer, AGENTS.md double-load
in nested worktrees). SESS-019/DRIFT-016/DRIFT-035 is a three-ID single defect where the **test pins
the wrong behaviour** — fix and test must be one diff.

**Verified by.** Golden round-trip corpus: a pi-written session file with unknown fields on known
entry types survives read-modify-write byte-for-byte except the intended change; a file whose first
physical line is blank opens; a missing/empty file does not write `"cwd": ""`. System-prompt golden
comparison against pi's output for the same inputs, including the **absence** of the `Current date:`
footer and the **presence** of the docs-pointer section. AGENTS.md loads once in a nested git linked
worktree.

**Risk.** SESS-043 (agent transcript re-seeded from the flattened context) may interact with batch
28's reducer work — sequence them, do not parallelise.

## Batch 28 — cyrup-agent: the turn-loop tail

**Goal.** A mid-run model or thinking-level change reaches the loop with the right attribution
headers, and the loop's error, hook and reducer paths match pi's text and event order.

**Items.** **AGENT-017 + AGENT-029 — the must-ship-together pair.** `02-cyrup-agent.md:54-55` states
it verbatim: "shipping AGENT-017 alone turns AGENT-029 from latent into live." AGENT-029 is latent
today only because nothing produces a `TurnUpdate::model`, and AGENT-017's entire purpose is to start
producing one — shipping 017 alone converts a dormant item into a live cross-vendor header leak,
sending an opencode session UUID to `api.anthropic.com`. Plus AGENT-021 (the same headers seam;
`gen_config.headers` is this crate's "parsed and never read" instance), AGENT-003, AGENT-009,
AGENT-010, AGENT-011, AGENT-012, AGENT-013, AGENT-015, AGENT-016, AGENT-018, **AGENT-019 (+DRIFT-039
— literally the same test at `agent_loop.rs:327`; fold DRIFT-039's better fix sketch in, and note that
this item is owned here and nowhere else)**, AGENT-022, AGENT-023, AGENT-024, AGENT-025, AGENT-026,
AGENT-027, AGENT-032, AGENT-033, AGENT-S02, AGENT-S03, DRIFT-036.

**Verified by.** A recording `StreamFn` capturing `(ModelRef, StreamOptions.headers)` per request: a
`prepare_next_turn` hook returning `TurnUpdate { model: Some(other_provider_model) }` after turn 1
makes request #2 carry `attribution_headers(other)` and specifically **no** `x-opencode-session` from
the first provider — this fails today. The same harness proves thinking-level changes reach the loop.
`crates/cyrup-agent/tests/model_boundary.rs:691-720` stays green. AGENT-019's wall-clock assertion
replaced by batch 3's rendezvous and run 100×. Every fixed test is revert-proved: revert the
production fix it covers, confirm red, restore.

**Risk.** The correction note in AGENT-017 matters, and so does the crate it points at: the file is
**`crates/cyrup-session-svc/src/hooks.rs`**, not `cyrup-agent/src/hooks.rs` (whose `:154` is inside a
`Debug for TurnUpdate` impl), and AGENT-017's edit site is `session-svc/src/hooks.rs:178-180`.
`hooks.rs:154-156` is **accurate** about cyrup's behaviour; only the "matches Pi exactly" sentence is
false. Correct that sentence, not the whole comment.

## Batch 29 — RPC wire payloads

**Goal.** An RPC client written against pi's types works against cyrup: every verb exists, every
payload carries pi's fields, and optional fields are omitted rather than nulled.

**Items.** SEAM-014 (+DRIFT-010), SEAM-017, SEAM-027, SEAM-030, SEAM-033, SEAM-048, SEAM-049,
SEAM-053, SEAM-055, SEAM-056.
**Opening sweep:** the inner payload shapes area 08 names as its largest unswept remaining target —
`entries_json()` versus `SessionEntry`, `BashResult`, the `Model` serialization behind
`set_model`/`get_available_models`/`get_state`, `AgentMessage` in `get_messages`, and the
`AgentSessionEvent` union.

**Why here.** Area 08's last block. SEAM-011/012/025 moved into the WIT batch (they are extension
payloads) and SEAM-060 into the tree batch, so what is left is genuinely one subject: the JSON-RPC
contract. After batches 15 and 19 the transport and the extension payloads are settled, so this batch
changes only shapes.

**Verified by.** Field-by-field diff of every response payload against pi's `rpc-types.ts` @v0.83.0,
driven by a checked-in conformance client: 32 verbs present and dispatched
(`get_available_thinking_levels` included); `--mode json` subscribes for the process lifetime, not
per-run, and no between-prompt event is dropped; a fork before the first message keeps `parentSession`;
optional fields are **absent**, not `null`; `session_start` follows `--name`/`--models`. Two F3 test
defects converted off wall-clock.

**Risk — branch.** This batch **will** file new items; the payload shapes have never been swept. If
the sweep files ≥10, the RPC contract is a floor of its own and a second wire batch is scheduled
rather than the items being absorbed silently. **Re-plan checkpoint after this batch.**

## Batch 30 — TUI presentation, and the two open questions

**Goal.** The remaining rendering, `/settings` and export surfaces match pi's, and the two recorded
open questions are answered in writing rather than encoded as severities.

**Items.** TUI-002, TUI-004, TUI-010, TUI-012, TUI-015, TUI-017, TUI-020, TUI-025, TUI-032, TUI-036,
TUI-038, TUI-041, TUI-N01, TUI-N02, TUI-N03, TUI-N06, TUI-N07, TUI-N08, TUI-N09, DRIFT-041; CFG-021 —
the `tuiMode` settings key (the settings half; must **not** wait on OQ-3); TUI-019 — **only** after
OQ-3 is answered, and if the answer is "no alt-screen mode", reclassify with the behavioural cost
written down rather than silently closing. TUI-N08 specifically: stop pinning the invented 🖼
placeholder and the rasterize-anyway fallback, which currently pin a user-visible defect as correct.

**Why here.** Last because it is the only block whose **scope** depends on a decision (OQ-3, hence the
dependency on batch 2), and because the flag half already shipped in batch 14 so nothing is blocked by
waiting. Grouping the presentation tail here also forces OQ-7: `TUI-FIDELITY.md` holds ~150
presentation findings with no IDs, invisible to every count in this plan, and it has already cost
behaviour once — its C14 recommendation deleted the `{n} queued` footer, which is what turned TUI-016
from "wrong surface" into "no surface at all" and is why batch 16 has to rebuild it.

**Verified by.** **Live terminal runs across at least three terminals with different capabilities**
(image protocol present/absent, OSC-8 present/absent): images are not rasterized where no protocol
exists and the `Show images` / `Image width` rows are hidden there; a theme chosen in `/settings`
persists across restart; env-overridden rows show the effective value; Ctrl+O expands tool output
while a bash block is live; `/reload` re-emits the resources/diagnostics panel, passes the permission
gate, and persists an implicitly-granted project trust. HTML export compared against pi's templated
document on the same session. Two F3 test defects converted off wall-clock.

**Risk.** If OQ-3 is answered "no alt-screen mode", TUI-019 must be **reclassified with its
behavioural cost recorded**, not silently closed — its previous `low` rested on ADR-0001, which is
unreadable in this workspace, so the severity was encoding a decision nobody made.

---

# 3 · The thesis

**The criterion that won: cohesion and dependency.** One batch is one coherent, reviewable diff, cut
by code locality and shared root cause, ordered by prerequisites, serialising the highest-churn crates
so that no two batches ever hold the same file, and starting where the diff is smallest.

Three measured reasons, not three arguments.

1. **Tail coverage.** This backlog is 94% medium-and-low. A severity-ordered plan schedules the 28
   criticals-and-highs and leaves 420 rows with no batch and no owner. Cohesion batching schedules
   **196 of 197 mediums and 222 of 223 lows** — not by promising to get to them, but because cutting
   by crate and shared root routes them into a diff someone was opening anyway. The two it does not
   schedule (`CFG-005`, `EXT-027`) are both in §*Deferred*.
2. **It is this repo's measured throughput regime.** The ledger records the medium closure rate moving
   **3.9% → ~29%** at exactly the moment commits stopped being item-targeted and became area-targeted
   subsystem batches — `8902b4f` at 39 files, the ten `fix(tui): batch N` commits at 103 files
   (`00-residual-ledger.md:139-168`). Cohesion batching is the regime that produced the good number;
   severity batching is the regime that produced 3.9%.
3. **It is the only cut that catches the couplings the area files state verbatim.** AGENT-017 /
   AGENT-029 (`02-cyrup-agent.md:54-55`). SEAM-020's dependence on PROV-031's `getAvailable()`
   semantics. The CFG-048 → TUI-028 / TUI-051 / TUI-027 / SEAM-067 chain, warned about in four passages
   of `07-cyrup-tui.md` (`:99`, `:237`, `:564`, `:1084`). A severity cut books each of these on a
   different side of a batch boundary.

**The defining inversion, stated so it can be attacked.** CFG-048 is a **medium** and it is scheduled
(batch 8) ahead of TUI-027, a **critical** (batch 16). The reason: TUI-027 adds seven
`app.tree.filter.*` ids and four rebinds into a keybinding registry that has no migration table, so
shipping it first silently breaks every `editor.*` config written against shipped cyrup. This is the
single most contestable ordering decision in the plan and it is not hedged.

**What was grafted from the runners-up.**

- *From the "observe first" plan:* **batch 1 exists at all.** Nothing in 448 items or 482k lines of
  Rust has ever been built, launched or reproduced, and even the test-count claim was never executed.
  Everything downstream rests on it.
- *From the same plan:* **the decisions track runs in parallel from day one** (batch 2). Nine open
  questions gate four batches on human latency no engineering removes.
- *From the same plan:* **risk stated as a branch, not a caveat.** A discovery step that cannot change
  the schedule is not worth scheduling — so batches 1, 2, 3, 5, 6, 11, 17, 18, 24 and 29 each carry an
  explicit fork.
- *From the same plan:* **`git cat-file -e v<ported-tag>:<path>` runs before any `upstream-drift` kind
  is assigned.** This gate has already moved twelve items **out** of `upstream-drift` and zero in.
- *From the "user impact" plan:* **batch goals are stated as user-visible outcomes**, not code
  locations. Cohesion decides the cut; the user decides how done-ness is described and tested.
- *From the same plan:* **the terminal input pipeline moved from the back of the plan to batch 6** — on
  population × frequency × silence, not on its medium/low labels. (With its supporting argument
  corrected; see batch 6.)
- *From the same plan:* **PROV-030 ships its S-sized mitigation first and unconditionally**, with the
  `all.rs:12-47` doc rewrite in the same diff, so the fix lands even if the google-vertex wire port
  slips.
- *From the same plan:* **one shared live-run harness**, built once in batch 1 and reused by batches 5,
  6, 8, 14, 16, 18, 19, 20, 23, 24-26 and 30.

**What this ordering deliberately sacrifices.**

- **Time-to-first-critical-closed.** Three batches (~1-2 weeks) run before the first critical lands.
  A severity-first plan closes AGENT-020 on day one. The trade is deliberate: day-one fixes to items
  nobody has reproduced, scored against a suite whose real state is unknown, is how this backlog got a
  ~20% citation error rate and a 3.9% medium closure rate.
- **Mergeability with upstream.** All four comparison tags are frozen for the plan's duration. See
  §*Chasing the upstreams* — this is the position most likely to be wrong.
- **Parallelism.** Serialising `cyrup-provider` (11 → 12 → 13), `cyrup-tools` (9 → 10),
  `cyrup-tui` (5 → 6 → 8 → 16 → 30), `cyrup-ext-subagents` (18 → 21 → 22) and
  `cyrup-permission-system` (7 → 23) means the crates with the most work are the crates that cannot be
  parallelised. Two people can run this plan; six cannot.
- **Legibility of progress.** A cohesion batch closes a mixed severity bundle, so "criticals
  remaining" moves in steps rather than smoothly, and a manager watching the crit count will see
  nothing happen for two weeks and then four close at once.

---

# 4 · The tail strategy — the ~189 mediums nobody has ever aimed at

**The problem, measured.** Highs closed at 82% and mediums at 3.9% in the 2026-08-07 pass, and the
ledger diagnosed it correctly: *every commit was explicitly high-targeted* — `513e45a`'s own message
reads "close the 8 remaining **high-severity** gap items", and all seven medium closures that pass were
collateral of a high fix, never independent work. Seven whole areas came back 100% open.

**The mechanism, and it is not a promise.** The mechanism is the batching itself. Batches are cut by
**crate and shared root cause**, not by severity, so a medium is scheduled because it lives in a file
someone is already opening — not because someone decided mediums matter this quarter. Consequences,
each checkable:

- **Every medium has a named batch.** 196 of 197. Not "a sweep later": batch 10 owns fifteen `TOOL-*`
  mediums, batch 20 owns twenty-two `EXT-*` items, batch 27 owns eighteen `SESS-*` items.
- **Duplicates collapse for free.** Cutting by locality routes the 28 duplicate IDs across ~21 defects
  into the same diff by construction: SEAM-047/008/059/DRIFT-049 in batch 15; SESS-S05/SEAM-060 in 16;
  CFG-006/AGENT-031 in 13; PROV-014/DRIFT-019 in 12; TOOL-036/DRIFT-046 and SEAM-015/DRIFT-004 in 9;
  AGENT-019/DRIFT-039 in 28; PROV-011/TOOL-016/EXT-024/DRIFT-018 in 19; EXT-019/TUI-034/DRIFT-015 in
  20; CFG-015/TUI-011 in the config batch; PROV-035/CFG-014/TUI-021 in 16;
  SESS-019/DRIFT-016/DRIFT-035 in 27; SESS-030/SEAM-034 in 16; PROV-025/DRIFT-027 in 12. **Area 12
  needs no batch at all** — all 34 of its rows route to owner batches.
- **The acceptance criterion is a machine list, not a hand-written one.** Batch 3 emits the unwired
  symbols, the schema drift and the wall-clock tests; the config, ext and permission batches are scored
  against *that* list. "Zero unwired symbols in this crate" closes mediums nobody enumerated.
- **F3's 23 test defects are converted inside the batch that owns the code**, three or four at a time,
  against one shared rendezvous primitive — never as a standalone "test hygiene" sweep that nobody
  schedules.

**The honest caveat, which the coverage number must always carry.** 196/197 is coverage of *today's
table*, not of the work. 448 is a floor (structural defect C), this plan schedules seven opening reads
whose purpose is to file more, and the re-plan checkpoints after batches 11, 18, 22, 24 and 29 exist
precisely because newly filed items otherwise have no route into a batch. **Do not read "196 of 197"
as "one medium left after this plan."**

**The alternative this plan did not take** is in §*Deferred* and in OQ-8: converting the ~163
non-user-observable lows into a mechanically-executed conformance suite. That is a maintainer
decision, not a planner's.

---

# 5 · Chasing the upstreams

**Position: freeze all four comparison tags for the duration of this plan, absorb drift batch-by-batch,
and re-baseline once, after batch 26.** There is no rebase batch.

**Why freeze, with the only measurement anyone has taken.** Re-measuring against the **named ported
tag** rather than a floating HEAD reclassified **twelve items out of `upstream-drift` and zero in**;
six of nine commit-hash-only items proved misfiled; and the ledger's own conclusion is *"None of these
will be swept up by a rebase"* (`:335`). Some drift also runs **backwards**: pi deleted
`sendSessionIdHeader`, and pi **adopted cyrup's** recursive settings merge at v0.84.1 — so fixing
toward v0.83.0 would be a regression, which is why `CFG-012` is *superseded*, not open. `pi-permission-system`
is the control case: it absorbed v0.7.1 → v0.8.0 to **zero** drift items, because its surface was small
and known.

**How drift is absorbed without a rebase batch.** By whoever has the file open: SEAM-051 and CFG-021 in
batches 14 and 30; SUBA-044/050/051 in batches 21-22; the PROV drift items in 11-13. The only two places
where drift **is** the work are whole-crate and are scheduled as such: pi-subagents in batches 18/21/22,
pi-intercom in batches 24-26.

**The cost, stated.** The window grows while ~30 batches run. pi is already 117 commits past the diffed
v0.84.1 (HEAD `581d75a89`); pi-subagents is 14 past v0.47.1 including `run-fanout-budget.ts`. Those
windows are **deliberately unanalysed** — a commit is a hypothesis, and every classification turns on
which side of the *ported* tag a symbol landed, which an untagged commit cannot answer.

**Cadence, so this is not a one-time position.**

1. **Before any batch that names an upstream file:** `git -C <repo> describe --tags` and
   `git cat-file -e v<ported-tag>:<path>`. First commands of the batch, not the last.
2. **After batch 26** (both lag-driven crates done): re-tag all four baselines, re-run the four
   `git diff --stat <old-latest>..<new-latest>` windows, and file only what the diff shows. Budget it
   as a real batch, not as cleanup.
3. **Every re-baseline censuses the baseline rather than inheriting it** — count in-tree `vX.Y.Z`
   citations per crate before trusting any recorded number. That is how `pi-intercom`'s baseline was
   found to be v0.9.2 and not v0.7.0, which moved six items from "lag" to "port bug".
4. **A standing escalation trigger, already written:** the moment pi's `main()` references
   `experimentalCli`, `SEAM-058` stops being a tracker.

**If the maintainer's goal is "stay mergeable with upstream" rather than "be correct for today's
users", this ordering is wrong** and a rebase belongs at position 3. That is OQ-4, and it is the
position in this plan I would bet against first. A cheap middle path exists: take `pi-intercom` (24
files, 14 commits, fully accounted) and `pi-permission-system` (already at v0.8.0 with zero drift
items) early, and hold **only** pi-subagents until batch 18 has read `8902b4f` against v0.43.0.

---

# 6 · Deferred, with reasons

The standing rule is that there are no unapproved deferrals, and that disclosure is not approval.
**Anything this plan does not schedule and does not list here is a defect in the plan.**

| # | what is not scheduled | why | needs a human decision |
|---:|---|---|:--:|
| 1 | **A rebase / catch-up phase onto pi HEAD.** All four tags frozen; no version-bump batch. | See §*Chasing the upstreams*. The only measurement taken supports freezing (twelve items out of `upstream-drift`, zero in). Rebasing pi-subagents across 358 commits would land four minors of new drift on top of 34 337 lines never read against v0.43.0, after which in-baseline port bugs and post-baseline drift are permanently indistinguishable. **I flag this as the position most likely to be wrong.** | **yes** (OQ-4) |
| 2 | **pi's agent-harness v2** — `packages/agent/src/harness/**`, ~11.4k insertions / ~10.9k deletions, the `agent-harness.ts` rewrite, a 667-line `reducer.ts`, a new `session/` subtree with a 993-line conformance suite, typed telemetry, `session-backends/sqlite-node`. | The single largest unowned surface in the workspace, and it cannot be ranked because nobody knows what behaviour it would change. All four trackers (`AGENT-028`, `SESS-038`, `DRIFT-040`, VL-P22) say *do not port speculatively*. **Batch 18 measures it** — file count, line count, shape, no port, no items — so the decision is made against a number. | **yes** (OQ-2) |
| 3 | **The alt-screen / fullscreen renderer** — `TUI-019`, effort L+: the alt-screen App variant, mouse capture, the scrollbar, semantic prompt navigation, eight `tui.altScreen.*` ids. Batch 30 schedules it **conditionally**. | Its severity currently encodes a decision nobody made: the old `low` rested on ADR-0001, unreadable in this workspace. Its two **separable halves are not deferred** — `SEAM-051` ships in move 2 / batch 14, `CFG-021` in batch 30. Only the renderer waits. If the answer is "no fullscreen", TUI-019 is reclassified with its cost written down, not silently closed. | **yes** (OQ-3) |
| 4 | **`cyrup/TUI-FIDELITY.md`** — 464 lines, ~150 presentation divergences against v0.84.1, no stable IDs, no status table, therefore invisible to the ledger and to this plan's coverage claim. | Either answer is defensible; silence is not, and silence has already cost behaviour once — its C14 recommendation to delete the `{n} queued` footer was **applied**, which is what turned TUI-016 from "wrong surface" into "no surface at all". Batch 30's scope is undefined until this is settled. | **yes** (OQ-7) |
| 5 | **`EXT-027` / `DRIFT-032`** — pi's bundled llama.cpp router extension and Hugging Face model search. Effort L, a whole subtree, present at v0.83.0 (a baseline miss, not drift). In no batch. | Upstream ships it as a bundled **extension**, so porting it is a product decision about whether cyrup ships local-model routing at all, not a parity repair inside an existing subsystem. The guest capability model it needs does not exist until batch 19. Scope it after 19, or decide it is out. | **yes** |
| 6 | **`CFG-005`'s residual** — the two multi-prompt api-key login flows (cloudflare, google-vertex), medium / L. In no batch. | A recorded maintainer deprioritisation, and the scope has genuinely narrowed (login.rs now covers login/logout/env-key/status/selectors; refresh lives in `auth/resolve.rs:146-239`), so the residual is two flow bodies. Recorded here rather than left silent because under 1:1 parity a deprioritisation is not the same as being out of scope: **two registered providers cannot be authenticated interactively at all.** | **yes** (OQ-8) |
| 7 | **Four unported subsystems that ARE scheduled but at the back of batch 22**, and which a maintainer may prefer to descope entirely: `SUBA-016` (nine `schedule.*` verbs, not four, L), `SUBA-021` (capability-ceiling / usage-budget / spawn-budget, all three **in-baseline** at v0.43.0, L), `SUBA-026` (the interactive `/subagents` admin UI, L), `SUBA-056` (durable completion replay and output archives, L). | Each is L and none is advertised in cyrup today, so no user can miss what was never offered. But `SUBA-021` is a capability **ceiling** — a safety bound — and it is in-baseline, never ported rather than newly added. Under 1:1 parity all four are required work; scheduling them at the back rather than descoping them is my call, and the alternative is a scope decision above a planner's level. | **yes** |
| 8 | **~13 `PARITY-GAPS.md`-owned IDs that live outside the twelve area tables** and are in no batch: `PB-8` (subagent RPC bridge entirely absent, **large**), `PB-9` (`clarify: true` advertised with no preview/edit UI, **large**), `PB-12` (no live child transcript writer; the `transcriptPath` artifact is missing), `PB-13`, `PB-14`, `VL-S2` (`workflowScript` runtime + `chatProgress`, **large**), `VL-S5`, `VL-S6` (Herdr inspector subsystem, **large**), `VL-S8` (wait tool v2 — no non-blocking subscriptions, no auto-drain, **large**), `VL-S12` (four slash commands upstream deleted at v0.41.0 still registered — reverse lag), `VL-S13`, `VL-S14`. | `PARITY-GAPS.md:306` records that area 09 deliberately does not restate these, so they fall outside every count in this plan. **This plan plainly treats that ID space as schedulable** — it schedules UW-1 (batch 6), UW-2 (batch 14), UW-3…UW-8 (batch 18), UW-9/UW-10/UW-15 — so dropping the rest in silence would be exactly the omission the standing rule forbids. Five are effort-large. `VL-S2` is sized (not scheduled) inside batch 22. They need either an owning batch or an explicit descope. | **yes** |
| 9 | **Windows support** — `PB-19` (the broker's Unix-only bind, dead TCP listen resolver), `DRIFT-046` (`normalizeWindowsShellPath`), `TOOL-036`'s win32 leg, `TOOL-038` (the cmd.exe fallback). Batches 9 and 24 schedule the items; what is deferred is **whether they mean anything**. | `crates/` carries 161 `cfg(unix)` sites against 6 `cfg(windows)`, so this is plausibly a property of the whole port rather than four items. Note `TOOL-036`'s `~`/`os.homedir()` half is a v0.83.0 parity bug on **every** platform and does not wait on this answer — it stays in batch 9. | **yes** (OQ-5) |
| 10 | **SDK-surface parity** — `SESS-038` (session-backends/sqlite-node), `SEAM-058` (`packages/{server,protocol,client}`, 81 files rewritten in the window), `DRIFT-047` / VL-P5 (telemetry). `PROV-031` **is** scheduled (batch 11) because it also breaks `--list-models`. | All are embedder-facing with no user-visible symptom in the cyrup binary, and four separate area files asked the same question independently — the signal that it needs deciding once rather than re-litigating per item. Their `Verify` lines are a re-diff at the next tag, not an implementation. `SEAM-058` carries a written escalation trigger. | **yes** (OQ-6) |
| 11 | **The ~163 low items with no user-observable consequence.** This plan **schedules** them inside their owning crate batches. What is deferred is the **alternative**: converting them into a conformance-test backlog (one test per item against upstream fixtures, closed mechanically). | The conformance route changes their cost by an order of magnitude, but it also changes what "done" means for 163 items under a hard 1:1 parity rule. If the maintainer takes it, batches 10, 20, 24-26, 27 and 30 each shrink materially. | **yes** (OQ-8) |
| 12 | **Whether a duplicate ID is CLOSED as `duplicate-of` when its owner lands, or kept open forever as a cross-reference.** This plan schedules each defect once, at its owner, never at its duplicate. | Deferred **bookkeeping**, not deferred work — listed so the omission is not read as one. As things stand the duplicates sit open after their owner lands and re-inflate the next count. Where a pairing is separable halves rather than a true duplicate (`SEAM-051` the flag / `CFG-021` the settings key / `TUI-019` the renderer), both halves are scheduled, in different batches, and said so. | **yes** |
| 13 | **The 117 pi commits past v0.84.1 (HEAD `581d75a89`), the 14 pi-subagents commits past v0.47.1 including `run-fanout-budget.ts`, and anything past pi-intercom v0.10.1.** | Deferred by an existing rule this plan endorses: a commit is a hypothesis, and every classification turns on which side of the ported tag a symbol landed. Area 05 also names `getExperimentalToolSampling()`'s constrained-sampling request on the four built-in tools as sitting inside this window and deliberately unfiled. | no |
| 14 | **Auditing the pre-source-control history of `docs/gap-analysis/` itself.** | Structurally impossible from this workspace: the directory has exactly **one** commit in cyrup's history (`a9000b1`), so any ID dropped before it came under source control is invisible to every renumber and deletion check — to this plan and to every pass before it. The `SEAM-035`…`SEAM-046` hole is confirmed a numbering artifact, but only against that single edition. | no |
| 15 | **A standalone fifth surface-driven sweep as its own batch.** Every named unrun axis IS scheduled — as the **opening step of the batch that already owns the files**: `editor.rs` line-for-line opens batch 5; `stdin-buffer.ts` opens batch 6; the bedrock/codex/OAuth closure audit opens batch 11; the `core/extensions/*` enumeration opens batch 17; `8902b4f`'s 34 337 lines **are** batch 18; pi-intercom's 68 broker exports open batch 24; the RPC inner payload shapes open batch 29. | A sweep run by someone who is not about to edit the code files items and fixes nothing — this backlog already went 117 closed against 207 filed. Each opening step carries an explicit *"an empty result must be STATED, not implied"* clause, because that exact silence is how pi's committed catalog generator went unfound for three editions and produced a wrong Fix. The still-unabsorbed reads — `bun/{cli.ts, restore-sandbox-env.ts}`, `ai/utils/{hash.ts, typebox-helpers.ts}`, `tui/{editor-component.ts, terminal-colors.ts}`, `pi-subagents/install.mjs` — are one read each and are the cheapest remaining unknown. | no |
| 16 | **Sizing unknowns inside three scheduled batches:** `workflowScript` (VL-S2, batch 22), `CFG-003` (settings-package auto-install, L, in the config block), the google-vertex wire port (L, batch 12). | All three are scheduled, not deferred, but any could dominate its batch. Each carries a first-day sizing step with instructions to split it into its own diff rather than push the other items out; google-vertex additionally carries an S-sized mitigation that ships regardless. Flagged here so a split is a **planned** outcome, not a surprise mid-batch. | no |

---

# 7 · Open questions

Nine decisions the plan stops at rather than guessing. Batch 2 exists to force them.

> **All nine are now decided.** Each question below keeps its original text — it is the record of why
> the question existed and what was on the table — and gains a **Decided** line naming the ADR that
> settled it. The decisions live in [`docs/adr/`](adr/README.md); that index also carries the ledger
> changes they imply, the contradictions reconciled between them, and the convention for overturning
> one. **One half of one question survives:** OQ-8 bundles two unrelated questions and only the
> `CFG-005` half is decided — the `~163 non-user-observable lows` half is still open and unowned.
>
> Two numbering notes, because both cost time this pass. **`OQ-N` here is not `OQ-N` in
> `PARITY-GAPS.md`** — that document's §6 carries its own nine numbered questions, and the mapping is
> `PG §6 q3 = OQ-5` · `q4 ⊂ OQ-6` · `q6 = OQ-9` · `q7 = OQ-2` · `q8 = OQ-3` · `q9 = OQ-1`. And
> `PARITY-PLAN.md:242`'s gloss of **OQ-6** as "SDK-surface parity" is a partial reading: OQ-6 is the
> `spec/` question, which *also* carries the SDK decision (see `:1465`).

**OQ-1 — What is `bash` allowed to be? (`TOOL-039` + `TOOL-007` as ONE decision.)**
*Why:* the two items are mutually contradictory as shipped. TOOL-007 concedes the `ProtectedFs` guard
is theatre **because** bash is undecorated; TOOL-039 shows that same bash runs under whatever
`CYRUP_SHELL` names — first arm of `ShellConfig::detect()`, structurally impossible to put in
`session_env_scrub_keys()`, propagated into every subagent re-exec, with nothing recording which
interpreter ran. pi's `getShellConfig` reads no env var at all.
*Options:* **(i) recommended** — delete the `CYRUP_SHELL` arm at `ops/shell.rs:101-105` and require the
`shellPath` setting (three lines, pi's shape), and flip `protect_paths` to false behind a flag since it
has no pi analogue and bash bypasses it anyway. **(ii)** keep the env var, but then **all four** limbs
are mandatory: a `[CYRUP-DELTA]` stamp, the resolved interpreter reported at session start and in bash
result details, a second explicitly-named scrub group, and path validation per `shell.ts:73`. Half of
(ii) is not an option.
*Blocks:* batch 9 entirely (14 items) and transitively batch 10 (15 more in the same crate).
***Decided:*** [`adr/ADR-0003-bash-scope.md`](adr/ADR-0003-bash-scope.md) — **option (i), both
halves**: delete the `CYRUP_SHELL` arm (`ops/shell.rs:101-105`), add none of option (ii)'s four
compensating limbs, and default `protect_paths` to `false`, keeping `ProtectedFs` as an inert
embedder-only opt-in. The same "cyrup never silently picks an interpreter the user did not choose"
rule also decides `TOOL-038` — the `cmd.exe` arm becomes pi's `No bash shell found` error — under
either answer to OQ-5. *(This is also `PARITY-GAPS` §6 q9.)*

**OQ-2 — Is `pi packages/agent/src/harness/**` in scope?**
*Why:* ~11.4k insertions / ~10.9k deletions owned by **no** area file. Four trackers point at it and
none proposes work, because the answer is a scope decision.
*Options:* absorb it (every batch after 18 is re-sized and a new area file must own it) · explicitly
out of scope (record the behavioural cost, close all four trackers with the reason) · decide after
batch 18's measurement, which is what this plan schedules.
*Blocks:* `AGENT-028`, `SESS-038`, `DRIFT-040`, VL-P22; the trustworthiness of the 448 figure; and any
date anyone tries to attach to this plan.
***Decided:*** [`adr/ADR-0004-agent-harness-scope.md`](adr/ADR-0004-agent-harness-scope.md) — **port
the behaviour the harness pins, not the harness.** Measured: harness-v2 is a published SDK that pi's
own shipping binary does not consume (its ten symbols reach exactly one file, which nothing in `src/`
imports and no export path publishes), so it contributes **zero** behaviour to be 1:1 with; absorbing
it would add a second unused agent stack and make cyrup *less* faithful. All four trackers close, the
`:262` re-sizing branch does **not** fire, batch 18 loses its measurement task, and the headline
figure was wrong by 2.3× (`4,977/2,936`, not `~11.4k/~10.9k`). *(This is `PARITY-GAPS` §6 q7.)*

**OQ-3 — Does cyrup build an alt-screen / fullscreen TUI mode at all? (`TUI-019`, L+.)**
*Why:* TUI-019's `low` rested on ADR-0001, which is unreadable in this workspace; the severity was
encoding a decision nobody made. Under the no-accepted-divergence rule, the behavioural cost (no
fullscreen, no mouse scroll, no scrollbar, no jump-to-prompt) stays on the list as work regardless.
*Options:* port it · no-op with an explicit not-supported message (the batch-14 interim becomes
permanent and TUI-019 is reclassified with its cost written down) · out of scope with the reason
recorded in the flag's own error text.
*Blocks:* batch 30's scope; `TUI-019`; the rendering half of `CFG-021`; tracker `DRIFT-022`. **Not
blocked:** `SEAM-051` and `CFG-021`'s settings half ship under every answer.
***Decided:*** [`adr/ADR-0005-alt-screen-tui-mode.md`](adr/ADR-0005-alt-screen-tui-mode.md) — **port
it.** The mechanism-impossibility argument is refuted by cyrup's own code (crossterm's
`EnterAlternateScreen` already executes at `startup_selector.rs:44`, `Viewport::Fullscreen` is
ratatui's default, `ratatui::widgets::Scrollbar` exists), leaving an ordinary application layer, which
under the parity rule is work. Batch 30 splits into **30a** (21 presentation items, L) and **30b**
(`TUI-019`, now **unconditional**, L+, decomposed into fourteen named units B-1…B-14). Premise
correction: `tui-alt-screen.ts` does not exist at v0.83.0, so this is `upstream-drift`, never a
divergence. *(This is `PARITY-GAPS` §6 q8 and area 07's `OQ-07-1`.)*

**OQ-4 — Do we chase the four moving upstreams before or after the port bugs?**
*Why:* this plan freezes all four tags and absorbs drift batch-by-batch. That is a strategy call.
*Options:* **freeze** (this plan) · **rebase first** (correct if the goal is mergeability rather than
correctness for today's users — then a version-bump batch belongs at position 3 and every citation in
fifteen documents is re-baselined) · **partial concession** (take pi-intercom and pi-permission-system
early, hold only pi-subagents until batch 18).
*Blocks:* whether a rebase batch exists at all; the ordering of batches 18 and 24-26; and the meaning
of every `upstream-drift` classification made in the meantime.
***Decided:*** [`adr/ADR-0006-upstream-chase-cadence.md`](adr/ADR-0006-upstream-chase-cadence.md) —
**pin each upstream to its latest *tag*, re-baseline on the tag event, never on a commit**, splitting
the single "baseline" field into three that move on three different triggers (ported baseline /
comparison tag / upstream HEAD, the last cited for nothing). Re-baseline pi-permission-system to
v0.8.0 and pi-intercom's comparison tag to v0.10.1 **today** — both have HEAD == latest tag. pi and
pi-subagents stay pinned not because rebasing is expensive but because their HEADs are **untagged**,
and the project's own evidence rule forbids classifying against an untagged commit. The question's
premise was false: all four windows to the latest tag are already chased and filed. The post-batch-26
rebase batch is **deleted** in favour of an event-triggered procedure, and the 74 `upstream-drift`
rows are ordinary work at filed severity — never "deferred until the next bump".

**OQ-5 — Is Windows in scope?**
*Why:* 161 `cfg(unix)` sites against 6 `cfg(windows)`. `PB-19`, `DRIFT-046`, `TOOL-036`'s win32 leg
and `TOOL-038` are four items whose meaning depends on one answer.
*Options:* in scope (the imbalance becomes a port-wide problem) · out of scope (record it and close the
four) · tier-2 best-effort.
*Blocks:* the value of four scheduled items, and how batch 9 writes `TOOL-036`.
***Decided:*** [`adr/ADR-0007-windows-scope.md`](adr/ADR-0007-windows-scope.md) — **Windows is in
scope.** pi gates its own releases on producing `pi-windows-x64.zip` and `pi-windows-arm64.zip`,
ships `docs/windows.md`, a Windows-only regression test and hand-written win32 C, so under the parity
rule cyrup ports the behaviour. The measurement in the *Why* above is wrong — it is **162 unix sites
against 62 Windows-aware sites**, not 6, because cyrup branches predominantly through the runtime
`cfg!(windows)` macro — and **17 of 18 crates already cross-compile**; the whole binary is blocked on
one file (`PB-19`, re-rated `low` → `high`). The named prerequisite is verification, not scope: until
a Windows runner exists the enforceable gate is `cargo check --target {x86_64,aarch64}-pc-windows-msvc
--workspace` in xtask. Opens a new area file `13-windows-platform.md`. *(This is `PARITY-GAPS` §6 q3.)*

**OQ-6 — Does `spec/` exist anywhere outside this workspace, and does it mandate `PERM-009`'s bash
bypass?**
*Why:* batch 7 **deletes** the bypass regardless, because an unverifiable in-source claim is not a
decision of record and the consequence is a defeated `tools.bash: deny`. But if the mandate genuinely
exists, the correct shape is still not the current one: pi's read/skills bypass is paired with a
`tool_call` handler that **re-gates** execution.
*Options:* produce `spec/` (then re-implement in the re-gating shape, not by restoring the deleted
branch) · confirm it does not exist (the deletion is final, and every other `R-NN-NNN` / ADR citation
in the tree is suspect for the same reason) · write ADR-0001 and the requirement ids into **this**
workspace, which batch 2 already schedules.
*Blocks:* the final shape of PERM-009 (not its deletion, which proceeds); and whether the thousands of
`R-NN-NNN` and ADR citations in cyrup's source are decisions of record or decoration. **This question
also carries the SDK-surface decision** (`SESS-038`, `SEAM-058`, `DRIFT-047`/VL-P5) — four area files
asked it independently; answer it once.
***Decided:***
[`adr/ADR-0008-requirement-ids-and-sdk-surface.md`](adr/ADR-0008-requirement-ids-and-sdk-surface.md) —
**`spec/` does not exist and is unrecoverable** (it lived in an untracked workspace root on a
disposable VM; `.workflows/check-citations.py:24` and commit `a9000b1`'s own message are the receipt),
so all ~2 195 in-source citations across **five** schemes are a **grep index carrying no authority**:
keep them, never let one justify a divergence or hold a severity, close the `R-NN-NNN` namespace to
new mints, and quarantine the ~45 normative ones behind `cargo xtask lint-citations`. `PERM-009`'s
premise is false — the bash branch cites no id at all — so it **deletes cleanly** and the "produce the
mandate" option is struck, not deferred. **SDK-surface parity is in scope, by capability rather than
export list.** The TUI and extension halves are written out separately as
[`adr/ADR-0001-tui-substrate.md`](adr/ADR-0001-tui-substrate.md) (the substrate carve-out covers
**drawing only**) and
[`adr/ADR-0002-extension-io-is-serde.md`](adr/ADR-0002-extension-io-is-serde.md) (extension I/O
crosses as serialized data, on the native tier too). *(The SDK half is `PARITY-GAPS` §6 q4.)*

**OQ-7 — Is `cyrup/TUI-FIDELITY.md` merged with real IDs, or formally retired?**
*Why:* 464 lines, ~150 presentation findings, no IDs, no status rows — invisible to every count. It has
already cost behaviour once (the C14 footer deletion → TUI-016).
*Options:* merge with real IDs into batch 30's scope (expect the medium/low counts to rise materially)
· formally retire it as non-normative and delete it · keep it as-is, the option that has already caused
one regression.
*Blocks:* batch 30's scope; this plan's coverage claim, which explicitly excludes those ~150 findings;
and the credibility of any future "the TUI is at parity" statement.
***Decided:*** [`adr/ADR-0009-tui-fidelity-doc.md`](adr/ADR-0009-tui-fidelity-doc.md) — **it is not a
backlog; it is a work order that was executed in full**, and none of the four documents asking this
question checked. All ten of its §7 batches shipped (`0aaca00`…`922d90c`), and a 46-of-117 sample
re-read at HEAD against pi v0.84.1 — one sample mechanical, one **adversarially** selected against the
conclusion — found **46 landed, 0 open**. So: archive it to
`docs/audits/2026-08-09-tui-presentation-fidelity.md` stamped EXECUTED AND CLOSED / non-normative,
merge **none** of its 117 rows, and change no count — the premise that the medium/low counts would
rise materially is the false one. It creates **zero** new ids. Two consequences survive: `TUI-016`'s
fix shape is corrected to pi's real surface (`updatePendingMessagesDisplay`, not a footer segment
pi does not have), and §8's 15 killed claims must migrate into the README traps list before the
archive goes non-normative.

**OQ-8 — Are the ~163 non-user-observable lows ordinary work items, or a mechanically-executed
conformance suite? And does `CFG-005`'s deprioritisation still hold?**
*Why (lows):* this plan schedules them inside their owning crate batches, which honours 1:1 parity
item-by-item; the conformance route changes their cost by an order of magnitude and changes what "done"
means. *Why (CFG-005):* the residual is now two flow bodies rather than an architectural gap, and batch
11 lands `PROV-003`'s `ApiKeyAuth::login` trait member — the seam they plug into.
*Options (lows):* keep as scheduled work · convert and close mechanically · split (convert the
protocol-field and formatting classes, keep anything with a reachable code path).
*Options (CFG-005):* hold the deprioritisation against the narrowed scope · schedule both flow bodies
into batch 11 now that the seam exists · ship google-vertex's flow only (it pairs with PROV-030 in
batch 12) and hold cloudflare.
*Blocks:* the size of batches 10, 20, 24-26, 27 and 30; interactive auth for two registered providers;
and part of batch 12's end-to-end verification, which currently needs a manually-provisioned ADC
credential.
***Decided (the `CFG-005` half only):***
[`adr/ADR-0010-oauth-acquisition.md`](adr/ADR-0010-oauth-acquisition.md) — **withdraw the
deprioritisation and schedule all FOUR missing api-key login bodies** (cloudflare-workers-ai,
cloudflare-ai-gateway, google-vertex, amazon-bedrock) into batch 11 in the same diff as `PROV-003`'s
trait member, deleting the `api_key_strategy_supports_login` name sniffer. Three premises were false:
none of this is OAuth (all four are `ApiKeyAuth.login`), there are four bodies not two, and the
providers are not un-loginnable — `/login` offers them, **reports success, and silently stores an
unusable partial credential**. `CFG-005` goes `medium`→`high`, `not-ported`→`parity-bug`, `L`→`M`.
***Still open:*** the **`~163 non-user-observable lows`** half of this question is **not** decided by
that ADR and has no owner. It is the one §7 question this batch leaves unanswered.

**OQ-9 — The first-run wizard (`UW-2`): wire `startup.rs:256`, or delete the predicate and correct the
trap list?**
*Why:* it is one of the "known traps" fed to every analysis pass ("the deliberately unreachable
first-run wizard"), and `PARITY-GAPS.md` now records that the trap is contested by evidence. A trap
that is wrong poisons every future pass.
*Options:* wire it · delete the predicate and correct the trap list · leave it and document why.
*Blocks:* `UW-2`'s implementation, which batch 14 owns.
***Decided:*** [`adr/ADR-0011-first-run-wizard.md`](adr/ADR-0011-first-run-wizard.md) — **wire it,
delete nothing.** pi ships and still invokes the wizard at v0.84.1; cyrup has a complete, unit-tested
port of every piece and is missing only the call. The trap's premise is **inverted**:
`is_official_distribution()` is a compile-time **`true`** for this build, and the repo's own test
asserts it (`tests/first_time_setup.rs:124-134`) — nobody read the trap and the test together. The fix
grows by two things a naive patch would miss: the missing `cli.list_models.is_none()` conjunct (or
`--list-models` mounts a full-screen wizard) and the call-site position. The wizard entry is
**removed** from the known-traps list; a wrong trap is not downgraded. *(This is `PARITY-GAPS` §6 q6;
`PARITY-GAPS.md:508`'s bare "OQ-6" is ambiguous under both numbering schemes.)*

---

# 8 · How this was produced, and what it is not

**The workflow.** Three planners proposed orderings under different criteria (user-visible impact;
observe-then-decide with explicit falsification branches; cohesion and dependency). A judge scored all
three on correctness, sequencing, honesty of deferral, executability and tail coverage, selected the
cohesion plan as the spine and grafted eleven specific elements from the runners-up. An execution
critic then tried to break the synthesis and returned 25 findings; **this document is the synthesis
with those corrections applied in place, not appended.** Where I refined rather than accepted a
correction, it is stated inline in one sentence at the point of disagreement — there are two: the scope
of the batch-1 dependency (§2, standing dependency 1) and the boundary at which the `8902b4f` audit was
split out (batch 18).

**Corrections applied that a reader should be able to check.** The batch-1 gate is now a real
dependency and its stop condition counts BLOCKED as well as REFUTED. Batch 3 is L not M, creates
`crates/xtask` (there is none today), does PROV-041's three text edits rather than only proposing a
lint, drops `abort_compaction` and `Snapshot::col` from its must-emit list (both have production
readers — verified), fixes `FsCaps::with_fs_root` to `GuestState::with_fs` at
`crates/cyrup-ext/src/host/services.rs:1210` (the former does not exist anywhere in `crates/`), and
re-points the advertise-vs-consume check as a two-sided diff, because SUBA-043 is a **missing**
property that an internal consistency check can never find. Batch 19 is XL with an internal sequence.
The `8902b4f` audit is its own batch, before the WIT bump. Intercom is three batches. `TUI-009` is
booked once (batch 6). `AGENT-019` is booked once (batch 28). `UW-7`'s two halves have an ordering.
`TUI-016` is scoped to its upstream half, not to a count. Live-terminal runs were added to the WIT,
extension-dispatch and permission-overlay batches. Batch 6's promotion argument was rewritten because
its supporting citation was a misread of a Rust comment — `app.rs:6584-6586` is a counterfactual
justifying the code that is there, and I confirmed that by reading it. `editor.rs` anchors are `:73`
and `:1074`, not `:71` and `:1073`. `hooks.rs` in batch 28 is `cyrup-session-svc`'s, not
`cyrup-agent`'s.

**What this is not.**

- **This rests on a STATIC analysis. Nothing was reproduced by running cyrup or pi.** No binary was
  built, launched or tested for the pass that produced the backlog or for the repair pass, and the
  inherited "3932 passed / 0 failed / 8 ignored" figure was never executed by any pass that quotes it.
  Every `Verify` line in all fifteen area files is a **design**, not an observation. That is why batch
  1 is batch 1.
- **Every item is a lead to verify, not a fact.** Severity and effort are judgements. The plan's file
  anchors were spot-checked, not exhaustively re-derived — and two of the three the previous draft
  claimed to have verified did not resolve.
- **The known error residue is about MECHANISM, not merely staleness.** Prior passes produced items
  that were wrong about how the code works: `DRIFT-005` was already fixed before anyone worked it;
  `DRIFT-001`'s `addedToolNames` is a cache-**placement** record, not what the item said; `TUI-002`'s
  claimed `thinkingText` palette never existed; `PROV-005` named xAI/Groq/DeepSeek as missing when they
  were always implemented; `SEAM-019` named two CLI flags (`--ui-mode`, `--alt`) that exist at neither
  tag; `SUBA-024` and `SUBA-021` cite files (`chain-validation.ts`, `launch-contract.ts`) that have no
  history at any tag; `EXT-055` names a symbol (`FsCaps::with_fs_root`) that does not exist. **Expect
  more of the same.** A batch that opens by reading the code before editing it is not ceremony.
- **Citation drift between the two tags is systemic.** The repair pass found ~25 citations quoting a
  v0.84.1 offset while asserting it held at v0.83.0 — including on the highest-ranked item in the
  backlog. Never write "identical at both tags"; give per-tag offsets and re-resolve by opening the
  file. Batch 3 builds the lint that enforces it.
- **448 is a floor, in both directions.** It is inflated by ~28 duplicate IDs and it is short by
  everything nobody wrote an item for. Any "we are N items from done" claim is unsupported, including
  one made from this document.

---

*Plan written against `docs/gap-analysis/` at 2026-08-12, cyrup HEAD `04c1ba2`. If the gap analysis is
re-baselined, re-derive the counts in the header before reusing the sequence.*
