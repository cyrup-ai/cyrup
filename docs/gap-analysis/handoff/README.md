# Handoff — read this first

You are continuing a long-running effort to bring **cyrup** (Rust) to 1:1 behavioural parity with
**pi** (TypeScript). This directory orients you. Read all four files before touching code; they are
short, and each one exists because something expensive went wrong without it.

| file | what it holds |
|---|---|
| `README.md` (this file) | the non-negotiable rules, and the two ways to lose a day |
| `01-methodology.md` | how a sweep is structured, with a copy-paste brief template |
| `02-state-and-next.md` | what is closed, what is open, what to do next |
| `03-verification.md` | exact commands, and the environment hazard that will bite you |

---

## The one-paragraph version

`docs/gap-analysis/` is a ledger of behavioural differences between cyrup and its four upstreams,
one file per area, each with an `## Open items` table. You pick open rows, verify them against the
code **at HEAD**, fix what is real, refute what is not, and mark the row either way. The upstreams
are checked out beside the repo. **Where cyrup and pi disagree, pi is correct by definition and
cyrup is what changes** — that is the adjudication rule for every item, and it is the whole job.

---

## Non-negotiable rules

**1. Verify before you fix.** The ledger's measured error rate is high and rising — in the last two
batches, **refutations roughly equalled fixes** (29/29, then 15/11). Most refuted rows say "already
closed at HEAD": earlier sweeps fixed the code and never marked the row. So an open row is a *lead*,
not a fact. Open the file, read the symbol, confirm the defect exists **now**.

**`REFUTED` with evidence is a full, valuable outcome.** Manufacturing a fix for a defect that is not
there is worse than leaving the gap, because it ships a test asserting wrong behaviour.

**2. Also check whether the item is half done.** This is the most common surprise here. `SUBA-057`
was filed as unported and the entire *read* half already existed — only the writer was missing, and
the file already carried a rustdoc link pointing at the symbol that did not exist. `PROV-042`'s
residual re-measured from three seams to one. `PROV-014` was half done and the row did not say so.
Establish what exists before you estimate what is left.

**3. Port the mechanism, not the vibe — but do not mistake a *host-specific facility* for the
mechanism.** These pull in opposite directions and both errors have been made here.

- pi spawns an OS subprocess ⇒ cyrup spawns an OS subprocess. Do **not** substitute an "idiomatic
  Rust" in-process design. A user can observe the difference (process tree, signals, artifacts).
- pi sandboxes a script with `node:vm` ⇒ cyrup uses its WASM component host. That is **not** a
  divergence; it is the same requirement in each host's own idiom, like a TypeScript class becoming
  a Rust trait.

**The test that separates them: would a USER notice?** If the only difference is the implementation
language, translate it. If behaviour changes, port it literally or record a `CYRUP-DELTA`.

An item blocked as *"that's a JS/Node/TUI subsystem"* in a project that ships a WASM host, a
ratatui selector stack and a socket broker deserves a hard second look. `SUBA-026` was blocked that
way and turned out to be 147 lines of *usage* of primitives cyrup already has.

**4. Strings and constants are byte-identical.** Copy pi's error messages; do not improve them. In
`cyrup-intercom` and `cyrup-permission-system` the constants *are* the behaviour.

**5. Cite upstream by file and symbol, and RE-DERIVE line numbers — never carry the ledger's.**
`setWorkingVisible` moved `:1877` → `:2091` between v0.83.0 and v0.84.2. A stale line still exists
and still has code on it, so trusting one silently cites the wrong function and nothing complains.
**A ledger citation can also simply be wrong:** `SUBA-026` cited `tui/selector.ts`, a path that
exists at no tag. Verify the path resolves before trusting the row's framing. Always state the tag
you read.

**6. Never write a citation you did not verify by opening the file.** A fabricated citation has
already cost this project real time — a brief once claimed four pi built-ins declared a field that
appears nowhere in them, and an agent had to open all four to disprove it.

**7. Every fix needs a test that FAILS BEFORE it.** State, per test, what the assertion sees pre-fix.

This is easy to get wrong in a specific way: a test that drives a *newly extracted shared helper*
will pass against the unfixed tree, and a `no_run` doctest never executes its assertion at all. If a
test genuinely cannot go red — because the API is new — **say so in your report and label it in-file
as coverage rather than proof.** Do not let it imply evidence it is not. Several tests in this tree
are labelled `MIRROR` or `coverage, not proof` for exactly this reason; follow that precedent.

**8. No unapproved deferrals, and no silent ones.** A `TODO` nobody signed off on is an incomplete
build. If an item is too large, report it `BLOCKED` with a **measured** size — files, line counts,
upstream modules — and what it needs. An item you did not attempt must be *reported*, not omitted.

**9. Update the ledger.** Mark the row closed the way existing closed rows are marked, and record a
refutation's evidence *in the row*. A fix that leaves its row open gets re-worked by the next sweep,
which is exactly how the ledger decayed into its current state.

---

## The two ways to lose a day

Both of these have actually happened here. They are not hypothetical.

**Running the test suite in a loop instead of reading the code.** An agent was once given an
unbounded "get the workspace green" instruction and ran the full suite eleven times over three and a
half hours. Separately, a hang was misdiagnosed as a deadlock **twice**; it was not a deadlock at
all — the binary was resolving a real provider from an ambient `TOGETHER_API_KEY` and making a
network call.

The cure the project owner prescribed, which works: **add file logging, run once, read the log,
isolate the spot, compare that spot to pi, align the code, remove the logging.** Not: run it again
and see.

**Truncating command output.** Piping a test run through `tail -120` once discarded 3,900 results and
produced a confidently wrong conclusion. **Redirect to a file and read the file.** Never `| head` or
`| tail` output you actually need.

---

## Rust correctness — where every real bug here has come from

cyrup is Rust; pi is JavaScript. The bugs cluster in the guarantees that differ:

- **A JS `async` function always settles. A Rust future can be dropped at any `.await`.** Anything
  registered before an await and cleaned up only on the success path leaks forever. **Put cleanup in
  `Drop`.** This produced a real deadlock and a permanently-disabled event bus.
- **JS has no locks, so a re-entered handler is an ordinary nested call.** In Rust the same shape
  re-takes a held `tokio::Mutex` and hangs, with **no deadlock detection**.
- **`tokio::select!` polls at random** when both arms are ready. JS cannot express that race, so
  upstream ordering is always deterministic — use `biased;`. One of these shipped as a 50/50
  cancellation coin flip.
- **`Notify::notify_waiters()` is edge-triggered and stores no permit**; a late waiter waits forever.
  `watch` is usually the right level-triggered replacement.
- **A spawned child must be reaped.** A detached child outliving teardown has shipped here once
  already.
- **`{ ...tool, execute }` preserves every field by construction.** A hand-written Rust trait impl
  must name each method, so a forgotten delegation is silent — give every fixture method a distinct
  non-default value.

The workspace denies `unwrap`/`expect`/`panic!`/raw indexing via `[workspace.lints.clippy]`.
**Those lints do not fire under `cargo build` or `cargo check`.** You must run `cargo clippy`.
