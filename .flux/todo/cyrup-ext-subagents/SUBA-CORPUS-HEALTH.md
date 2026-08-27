---
stage: new
status: done
updated: 2026-08-27 05:30
severity: medium
effort: small
subsystem: gap-analysis ledger
source: docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md
item: corpus-health
---

> **Path note.** This task lives in a subdirectory. Every flux command globs a single level
> (`ls -1 "$FLUX_BASE/todo/"*.md`), so `/exec`, `/aug` and `/qa` will not list it — pass the absolute
> path explicitly.

# Repair The Gap-Analysis Ledger Before The Next Parity Pass

## Description

The v0.57.0 audit surfaced defects in the ledger itself. These matter for the parity work in this
directory: an implementer who trusts a stale row will either re-derive a closed finding or, worse,
act on evidence that is now false. One row in particular is described as harmful if followed.

## Corpus health

Five things a maintainer should fix in the ledger before the next pass.

**(1) The corpus does not end at `SUBA-066` — it ends at `SUBA-071`.**
`09-cyrup-ext-subagents.md` carries `SUBA-067`…`SUBA-071` (three test-defects filed
`Status FIXED`/`OPEN`, plus `SUBA-070` and a REFUTED `SUBA-071`). A "start at `SUBA-067`" instruction
would have collided with five live ids. **This batch starts at `SUBA-072`.**

**(2) The README baseline table is a full major-version stale for this upstream.** It records
`pi-subagents` latest as **v0.47.1** with a delta of "151 files, +10,254 / −1,333". The tag is
**v0.57.0** and the unmeasured window is the table in `## Scope` above. Area 09's own header also
states that every claim was settled at v0.43.0 or v0.47.1. Update both, and re-read PARITY-GAPS §1d.

**(3) Three high-traffic rows now carry evidence that is factually wrong at HEAD, and one would cause
harm if followed.**
- `SUBA-021` / `VL-S1` says `rg 'capability_ceiling' = 0` and "no ceiling concept". The subsystem
  landed in sweep 10; the residual defect is *worse* than the one filed (`SUBA-072`).
- `VL-S14` rates `runner: external-cli` **medium** / "unsupported". The key is neither rejected nor
  applied, which is a capability widening, and the subsystem tripled and gained a second runner type
  inside the window (`SUBA-074`).
- **`SUBA-051`'s Fix line instructs *"Do not apply it to foreground runs, which already have their
  own default"* — the foreground path has no default at all** (`extension/tool/params.rs:264-280`),
  so following that instruction leaves the foreground unbounded permanently (`SUBA-077`).

This is the third edition's *"a true line number carrying an untrue claim"* class, and it is now the
dominant failure mode in this area's ledger.

**(4) The restructure trap is real and it cuts both ways.** `src/extension.rs` no longer exists, so
every `extension.rs:NNNN` citation in area 09 is **unresolvable**, not merely stale. The more
dangerous direction is the false negative: `restoreActiveJobs` reads as absent under every name
upstream uses and is fully present as `resume_tracking`, with a test pinning both of its subtleties.
Every absence claim in this batch was established by grepping the current tree for the behaviour by
identifier **and** by concept, in both camelCase and snake_case, plus env-var names — never by
resolving a cited path. Adopt that as the standing rule for this area.

**(5) Two in-source comments assert things about upstream that upstream contradicts, and both hid a
defect.**
- `background/watch.rs:605-609` says pi uses `display: true` unconditionally; `notify.ts:239`
  computes it (`SUBA-090`).
- `discovery/types.rs:411-414` says `AgentOverrideConfig` is *"a field-for-field port … and pi has no
  others"* while pi had four more at the measured baseline and nine more at v0.57.0 (`SUBA-081`).

**A completeness claim written in a doc comment is not evidence, and neither a citation audit nor a
compile catches it.** Add both to the known-traps list, and prefer a checked-in pinned copy of the
upstream field list plus an assertion over a prose claim.

### One note in the ledger's favour

The lenses independently confirmed large ported subsystems **complete and correct**: the acceptance
tree (~10,140 lines, nine evidence kinds, `stopRules`, verify memoization, workspace fingerprinting —
`SUBA-076` is a defect *inside* it, not a hole in it), nested events (1,992 lines plus the child
control inbox), MCP direct tools (2,816 lines including the header cache-identity fix), the fallback
ladder's R-SA-036 ordering, the turn / tool / usage / spawn budgets, agent memory, model scope, and
the four-tier discovery merge with its deliberately asymmetric same-tier rule.

**The remaining distance in this crate is concentrated in three places**, and a planner should read
the twenty items above through that partition:
1. **The parent side of policy surfaces whose child side is already implemented** — `SUBA-072`
   (capability ceiling), `SUBA-073` (permissions). Both are "the enforcement machinery is ported and
   permanently unreachable", and both are small relative to what they unlock.
2. **The agent-definition schema's missing keys** — `SUBA-074`, `SUBA-081`, `SUBA-082`, `SUBA-088`,
   with `SUBA-086` as the amplifier that converts all of them from silence into user-visible errors.
   **Land `SUBA-086` first.**
3. **The external-runner / `workflowScript` execution model** — `SUBA-074` stage 2, `VL-S2` and its
   dependents. This is the genuinely large remainder and the only part that needs design.

## Scope

In scope: correcting `docs/gap-analysis/09-cyrup-ext-subagents.md`, `PARITY-GAPS.md` and
`README.md` for the defects listed above.

Out of scope: implementing any SUBA item; rewriting the audited history of closed items beyond
correcting evidence that is factually wrong at HEAD.

## Approach

1. Fix the id-range error first — the corpus ends at SUBA-071, not SUBA-066. Anything that assumes
   067 is free will collide with five live ids.
2. Update the README baseline table: pi-subagents latest is v0.57.0, not v0.47.1, and record the
   measured window rather than the stale delta.
3. Correct the rows whose evidence is wrong at HEAD, giving each a dated correction note rather than
   silently editing it — the corpus convention is to show the correction, not hide it.
4. Re-anchor citations that point at `extension.rs`, which is now a directory.

## Acceptance Criteria

- [ ] The next free SUBA id is stated correctly and no id in this batch collides
- [ ] The README baseline table names v0.57.0 with the measured window
- [ ] Each corrected row carries a dated correction note explaining what was wrong
- [ ] No citation in the corrected rows points at a path that no longer exists
