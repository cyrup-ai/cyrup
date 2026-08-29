---
stage: new
status: done
updated: 2026-08-29 01:49
---

# PERM-034: "Allow Always" Does Not Stick

## Description

**"Allow Always" does not stick — the same tool/command is re-prompted repeatedly within one
session.** Owner report from live use 2026-08-15. Rated `critical`, class `port-bug`, size `M`.

This makes the permission system unusable: a user who approves an operation *permanently* is
asked again on the next identical call. Ledger rows:
`docs/gap-analysis/10-cyrup-permission-system.md:189`,
`docs/gap-analysis/00-residual-ledger.md:25`.

### Two things this is NOT — do not spend the session re-deriving them

**It is not the dead-seam class. Both halves are wired at HEAD.**

- The **write** happens: `extension.rs:1717` and `:2023` both call
  `SessionApprovalStore::approve_always(&check.tool_name, &subject)`, matching pi `index.ts:610`.
- The **read** happens: `get_rules()` at `extension.rs:1464`, `:1634`, `:2211`, matching pi
  `index.ts:571`.

**It is not "approvals don't survive a restart."** The store is intentionally SESSION-ONLY and
in-memory on both sides — `stores.rs:13` records pi's own change to that behaviour. Losing
approvals across a restart is CORRECT. The report is re-prompting **within one session**.

### Two candidate causes — establish which fires before changing anything

**(a) Subject mismatch.** The `subject` string recorded on approval may be narrower or
differently normalised than the one derived when the next call is evaluated — e.g. a full
command line with arguments where pi stores a pattern — so the stored rule never matches
again. Every approval would then create a fresh single-use rule that is never reused.
Compare cyrup's `subject` derivation at **both** the write and the match site against pi's,
which passes `result.toolName` plus a subject computed in `index.ts` around `:599-610`.

**(b) Over-eager clearing.** The store is cleared at `extension.rs:2617` and `:2705`; pi
clears at `index.ts:1830` and `:1864`. If cyrup clears on an event pi does not — a turn
boundary rather than a session switch — approvals evaporate between prompts. Diff the two
clear-trigger sets.

The two fail differently and one live run separates them: **(a) never matches at all;
(b) matches until something wipes it.**

### How to reproduce — read this before starting

Per `handoff/03-verification.md`: **log at the write and at the match, run once, read.** Do
not characterise this by re-running it.

`crates/cyrup-it/tests/permission/human_dialog.rs:219` already reads back the SAME
always-rule **a turn later** rather than a tool-call later — aimed squarely at candidate (b).
Read it first to see what it assumes, since it passes while the bug is live.

### Caution: citations may have drifted

The sibling row SEAM-112 was filed the same day, and its "verified at HEAD" line numbers were
found to have drifted — some modules have since been split and two cited files no longer
exist at the given paths. Treat the `extension.rs` / `stores.rs` line numbers above as
**symbol hints, not addresses**: navigate to `approve_always`, `get_rules` and the clear
sites by name, and repoint the row if they have moved.

### One bookkeeping trap

This row was **renumbered `PERM-033` → `PERM-034` on 2026-08-19** to resolve an id
collision: a struck forwarding-audit row claimed `PERM-033` first (sweep 6, filed and closed
2026-08-14, commit `e5c6933`), and this row took the same id a day later in `4a05330`. **Every
mention of `PERM-033` in history at or before 2026-08-18 that says "Allow Always" is this
row.** Do not read that closed row's fix as evidence this one is resolved.

## Acceptance Criteria

- [ ] Instrumentation added at the `approve_always` write and at the `get_rules` match site,
      logging the `subject` string produced by each.
- [ ] ONE live reproduction captured — approve an operation with "Allow Always", then trigger
      the identical call again — and the log retained as the evidence.
- [ ] Determined from that log which cause fires: **(a)** the two `subject` strings differ, or
      **(b)** the rule is present then cleared between prompts. State which, in one sentence,
      grounded in the log rather than inferred from reading.
- [ ] If (a): cyrup's subject derivation reconciled with pi's `index.ts:599-610` at both the
      write and the match site, so the same operation yields the same key twice.
- [ ] If (b): cyrup's clear-trigger set reconciled with pi's (`index.ts:1830`, `:1864`), so
      approvals survive a turn boundary and are cleared only on the events pi clears on.
- [ ] Session-only, in-memory behaviour preserved — approvals must still NOT survive a
      restart. Do not "fix" this by persisting the store.
- [ ] A second live run confirms an "Allow Always" approval is no longer re-prompted for the
      identical call within the session.
- [ ] Temporary instrumentation removed; `cargo check --workspace --all-targets` clean.

## Source

- **Ledger:** `PERM-034` — severity `critical`, class `port-bug`, size `M`
- **Filed:** 2026-08-15 (live use, owner report)
- **Renumbered:** `PERM-033` → `PERM-034` on 2026-08-19 (`4a05330`), id collision
