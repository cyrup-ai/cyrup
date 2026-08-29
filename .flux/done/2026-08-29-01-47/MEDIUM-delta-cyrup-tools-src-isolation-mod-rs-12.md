---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/isolation/mod.rs:12"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: qa
status: completed
updated: 2026-08-29 23:10
---

# Protected paths — CLOSED RECORD. Do not execute; the code shipped and nothing else is owed.

> The three code limbs landed in `a3c1abe`, RED-proven and verified. Five documentation edits were
> then proposed, argued over five rounds, and are **withdrawn**. This file is now the finished record
> of why. Every number below was obtained by **reading each row**, not by grepping — see §4.

---

## 1. The code — landed and green

| limb | site |
|---|---|
| matcher covers the dotenv family | [`isolation/protected.rs:80`](../../../crates/cyrup-tools/src/isolation/protected.rs) — `s.as_bytes().get(n.len()) == Some(&b'.')` |
| `ProtectedFs` rooted at the session cwd | `protected.rs:135` (`pub fn rooted`); [`builder.rs:886`](../../../crates/cyrup-session-svc/src/builder.rs) |
| the five `crates/` sites corrected | `grep -rn '195-225' crates/` → 0; `grep -rn 'protected-path concept' crates/` → 0 |

346 tests pass, clippy 0.

## 2. Why the five doc edits are withdrawn

### 2.1 The rule, quoted exactly

[`docs/adr/README.md:73`](../../../docs/adr/README.md):

> An ADR is never **silently** amended: it is superseded by a higher-numbered ADR that names it.

The operative word is *silently*. **ADRs in this repo are amended, routinely and openly**, recorded
in `## Contradictions resolved in this pass` (`:83-113`) — **15 numbered rows**, naming every one of
ADR-0001, 0002, 0003, 0004, 0005, 0006, 0007, 0008, 0009 and 0011 as edited.

**ADR-0003 itself is edited in two of them**: row `:91` (`§5`→`§6`, `OQ-9`→`q9=OQ-1`) and row `:98`
(*"discharged, not pending — Windows in scope, strike refused, `TOOL-036` stays `low` in batch 9"*).
Row `:97` mentions ADR-0003 but edits ADR-0007; it does not count.

So "never touch an ADR" is **not** this repo's rule, and any argument resting on it is wrong.

### 2.2 The principle that actually distinguishes

Read all 15 rows: every amendment either **fixes something wrong at the ADR's own reading frame**
(mis-cited governing anchors at `:90`; an OQ-numbering collision at `:91`) or **reconciles two ADRs
that contradicted each other** (`:92`-`:95`, `:97`-`:104`).

**Not one retro-fits an ADR's finding to code that changed afterwards.** That is the whole
distinction. `ADR-0003:118`'s *"component-equality match"* was **correct at `72cd292`**, the commit
the ADR pins itself to (`:18-20`). What drifted is the code, not the ADR.

Were an amendment warranted, it goes in that table with an entry saying what was edited and why. The
five withdrawn instructions proposed silent edits, which is the one thing `:73` forbids.

### 2.3 Editing §3 would contradict D5

`ADR-0003:114` reports `builder.rs:208` setting `protect_paths: **true**`. `D5` at `:194` instructs
*"Flip … to `protect_paths: false`."* §3 is the pre-decision state D5 acts on. Rewriting it to match
HEAD leaves the ADR ordering a flip to a value it already reports.

### 2.4 The ADR's own documentation bill is paid in full

`ADR-0003:255-261` states TOOL-007's complete fix — *"one-line default flip at `builder.rs:208`, two
doc corrections (`builder.rs:152`, `isolation/mod.rs:3-6`), a `[CYRUP-DELTA]` stamp, and the D8(4)
tests"*. All five exist: flip at `builder.rs:259`; the `builder.rs:180` doc; the `mod.rs:3-6` text;
stamps at `mod.rs:12` and `:18`; `tests/isolation.rs:159` passing.

### 2.5 The ledger owes nothing

`TOOL-007` closed **2026-08-14** (`04-cyrup-tools.md:177`). The four open rows in the current table
(`:172`) are `TOOL-022`, `TOOL-015`, `TOOL-017`, `TOOL-042` — none protected-path. `a3c1abe` moved
cyrup *closer* to pi inside already-closed territory.

### 2.6 One line that is already correct

`04-cyrup-tools.md:315` scopes its claim to `pi/packages/coding-agent/src/core/tools/`, which is
**true** — pi's guard lives at `examples/extensions/protected-paths.ts`. Do not "fix" it.

## 3. Definition of done

Archive after confirming, by reading:

1. Both `crates/` greps return 0; `protected.rs:80`, `:135` and `builder.rs:886` are as described.
2. `README:73` contains *silently*; `:83-113` holds 15 numbered rows; rows `:91` and `:98` edit
   ADR-0003.
3. `ADR-0003:114` says `true`; `:194` instructs the flip to `false`.
4. All five limbs of `ADR-0003:255-261` exist at HEAD.
5. No source file, ADR or gap-analysis file is modified.

## 4. Why this file took five rounds — the method note that matters

Rounds three, four and five each produced a **new wrong number**, and every one came from measuring
by `grep` instead of reading:

- *"an ADR is superseded, never amended"* — dropped *silently* from the quoted rule.
- *"six times"* — counted the first six rows visible in one `sed` window; there are 15.
- *"ADR-0005 and 0011 appear without an `Edited` row"* — false; row `:91` edits both, in a
  continuation clause a first-ADR-after-`Edited` pattern cannot see.

A fourth was nearly shipped this round: a naive scan of text following `Edited` attributed row `:97`
to ADR-0003, when that row edits ADR-0007 and merely mentions ADR-0003. Reading the row caught it.

**The rule this file exists to demonstrate: a claim about a document is only as good as having read
the lines it rests on.** Every number in §2 was obtained that way.

## 5. Out of scope

- Do not make the five withdrawn doc edits.
- Do not edit `docs/adr/ADR-0003-bash-scope.md` or `docs/gap-analysis/04-cyrup-tools.md`.
- Do not append anything to `TOOL-007`; it closed 2026-08-14.
- Do not change the matcher, `rooted`, or the `protect_paths: false` default.
- Do not touch any file under `crates/`.
