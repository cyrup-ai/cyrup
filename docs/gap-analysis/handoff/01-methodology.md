# 01 — Methodology

How a sweep is structured, and why. This shape was arrived at by failing at the alternatives.

---

## The shape that works

**Big cuts, then one verification phase.** Do not plan a tiny scope, fix twenty lines, run the
suite, repeat — that pattern produced roughly zero progress over several hours. Plan a batch of
30–60 items, execute the whole batch, then verify once.

**Partition by crate, never by topic.** Two workers editing the same file collide and lose work.
Assign each worker a disjoint set of crates. Everything that needs a crate someone else owns comes
back as `BLOCKED`, and you run those **serially** afterwards with one worker owning every crate it
needs.

This is not theoretical: a sweep partitioned by crate closed 42 items and left 12 blocked *purely*
because they each needed `cyrup-tui` alongside `cyrup-session-svc`. A serial follow-up closed all six
distinct defects behind those 12 without incident.

**Beware the mismatch between an area file and a crate.** Area 05's rows are all `CFG-*`, but the
fixes land in five different crates — a config/resources split reached only 4 of its 30 rows. Decide
ownership by **which crate the fix lands in**, and have each worker *name* the rows it is leaving to
someone else so nothing falls between them.

**Verification is centralised.** Individual workers must **not** run the test suite (see below).
One verification step at the end runs `check`, `clippy` and the gate.

---

## Worker rules that matter most

Put these in every brief. They are the difference between a batch that lands and one that burns.

1. **Workers do NOT run `cargo test` or `cargo nextest`.** They verify with
   `cargo check -p <crate>` and `cargo clippy -p <crate>`. The gate is run once, centrally, at the
   end. Agents re-running the suite is the single largest source of wasted time in this project's
   history.
2. **No `| head` / `| tail` on output you need.** Redirect to a file, read the file.
3. **Check both cfg arms.** `crates/cyrup-ext` has `default = ["wasm-host"]`. The arm that ships and
   the arm that reads correctly have been *different code* here — that is exactly how `MCP-037a`
   stayed invisible. Any cfg-gated change must hold on both, with tests covering both.
4. **Report `FIXED` / `REFUTED` / `BLOCKED` per item**, with a test name and what that test sees
   pre-fix, or a measured size for a block.

---

## Always end with an adversarial reviewer

Add a final worker whose brief says **"assume at least one fix is wrong"** and, crucially,
**"read the diff, not the reports."** This catches things the authors do not, every single time.

Real catches from this pattern:

- A test whose first draft **passed pre-fix**. It checked that a cited line fell inside an event
  band — and all eight bad citations were inside the band. Rewritten to carry the full 33-entry
  event→line map, it went red at eight sites and surfaced a ninth instance the item did not have.
- An item whose **kind and every citation were wrong** while the code was right — filed as v0.84.x
  drift when the behaviour existed at v0.83.0, so it was a baseline gap, not version lag.
- A worker's own new test asserting a **wrong premise**: that a label entry would not be on a session
  branch. It is, in both implementations. The code was right; the test was wrong.

Have the reviewer also **cross-check the reports against the area tables** and name every open row
nobody reported. Silent omission is the failure mode a per-item report format cannot catch by itself.

---

## Brief template

Copy this, fill the slice, keep everything else.

```
# Context
Repo: /Users/davidmaple/cyrup.ai/cyrup (branch david/cyrup) — a Rust port of the pi coding agent.
Upstreams: /Users/davidmaple/cyrup.ai/{pi,pi-subagents,pi-intercom,pi-permission-system}
Backlog: docs/gap-analysis/, one file per area with an `## Open items` table.
Read docs/gap-analysis/handoff/README.md first — its nine rules are binding.

# Your slice
<crate(s) you own — and ONLY these. Report, do not make, any change outside them.>
<the area file and which rows are yours>

# Rules
1. Do NOT run cargo test or nextest. Verify with `cargo check -p <crate>` and
   `cargo clippy -p <crate>`. Clippy matters: the no-panic lints do not fire under check.
2. Never pipe output you need through head/tail — redirect to a file and read it.
3. Confirm each item at HEAD before fixing. REFUTED with evidence is a valid outcome.
   Also check whether it is HALF DONE — that is the most common surprise here.
4. 1:1 parity with pi. Port the mechanism, not the vibe; strings byte-identical; a forced
   divergence is a CYRUP-DELTA naming the upstream symbol and reason. But do NOT mistake a
   host-specific facility (node:vm, a JS worker) for the mechanism — ask what requirement it
   serves and whether cyrup already has a native facility. Test: would a USER notice?
5. Cite by file and symbol; RE-DERIVE line numbers and state the tag. Ledger citations are
   sometimes simply wrong — verify the path resolves.
6. Every fix needs a test that FAILS BEFORE it; state what it sees pre-fix. If it cannot go
   red (new API), say so and label it in-file as coverage, not proof.
7. No unapproved or silent deferrals. BLOCKED needs a measured size and what it needs.
   An item you did not attempt must be reported.
8. Rust: cleanup in Drop (futures are droppable); reap children; `biased;` in select! where
   upstream order is deterministic; watch over notify_waiters for level-triggered state.
9. Update the ledger row — closed as existing closed rows are marked, refutations with evidence.

# Report
Per item: id, FIXED|REFUTED|BLOCKED, detail, test name, what the test sees pre-fix,
measured size if blocked, whether the ledger row was updated.
```

---

## Two habits worth copying from the good runs

**Label tests by what they actually prove.** The best worker output here classified its own tests
unprompted and against its own interest: seven `RED-before`, three declared `MIRROR` (they stay green
through a revert — they guard against over-clearing, they do not prove the fix), six labelled in-file
as coverage because the types did not exist before. Nothing was left implied to be evidence it was
not.

**Record a decision you *didn't* take, and why.** One worker started a `HOST_WORLD` version bump,
re-derived the version check, found it only defends old-guest-on-new-host — a direction an added
import cannot fail — and abandoned the bump because it would refuse every already-built guest while
preventing nothing. That reasoning went into the source. A silent non-decision would have been
re-litigated by the next sweep.
