# The standing bar — read by every //flux agent, every batch

## The directive

  "we ship features ... adapted to Rust best practices ... pi names can change ...
   it's the features that matter"
  "the deliverable is fully working code that brings the rich functionality of pi and
   pi-agents to cyrup ... nothing more narrow"
  "we can do better than pi where valuable. no issue there"

## What "done" means

The deliverable is a feature that WORKS AT THE APPLICATION LEVEL — something a human using
cyrup, or an agent delegating work under it, can actually rely on. Not a module that compiles.
Not a verb that is advertised. Not a shape that matches upstream's file. If a user could hit
this feature and get a hollow answer, it is NOT done.

## Size is never a reason to cut

Big is fine. Slow is fine. Half-finished is not. Do not descope to make the work tractable —
SEQUENCE it instead. Tokens and wall-clock are not a cost worth optimizing. A half-usable
feature is the expensive thing.

## Upstream is the floor, not the ceiling

pi is a REFERENCE, not a limit. Where cyrup can do better — because it is Rust, because it has
data or capability pi lacks, because the dependency's own protocol offers more than pi uses —
**do better, and mark it `[CYRUP-EXCEEDS-UPSTREAM]` with the reason.** Reproducing an upstream
limitation on purpose is not fidelity; it is choosing someone else's constraint.

Two rules keep this honest:
- Upstream's STRUCTURE is the contract: on-disk formats, message strings, decision ORDER.
  Exceeding means doing MORE, never doing it differently for its own sake.
- Read the DEPENDENCY's own source and docs, not just pi's consumption of it. pi's integration
  with a tool is one consumer's subset. (Worked example: pi drives 6 herdr CLI verbs — `pane
  get`/`split`/`run`/`close` plus `tab focus` and `workspace focus`, `inspectors/herdr/focus.ts`
  — while herdr v0.9.1 exposes 105 socket methods, a first-class agent protocol and an event
  stream. **Both of those numbers were wrong the first time this file stated them (4 and 99),
  and the error survived into code comments until a QA lens greped it.** Count with the grep,
  cite the grep, and expect to be checked.)

## Residuals

**The ONLY thing that may be filed as a residual is a dependency that genuinely does not exist,
with the grep that proves it.** Not "this is large". Not "this crosses into another module".
Not "upstream does not do it either". Before writing "out of scope": grep first, and check what
the ledger row you are closing actually claims. In this project's last research pass, three
"residuals" were refuted by one grep each, and one hand-written "out of scope" line would have
made its batch unable to close the row it was scoped for.

An agent may report that something is BIG. An agent may not decide that something is OUT.

## Premises

Every `[CYRUP-DELTA]` and doc comment states a premise that is TRUE. GREP every premise before
writing it. A delta whose premise has become FALSE is DELETED, never softened.

## Proof

REACHABLE through a PRODUCTION caller; a test drives the production path end to end. No
`#[allow(dead_code)]`, no stub, no `todo!()`. Every new test proven by GUTTING the
implementation, observing RED, and restoring byte-for-byte.

## Upstream reading rule

    git -C /home/user/cyrup/tmp/<repo> show <pinned-tag>:<path>

Never a working tree, never unpinned HEAD, always cite file:line. Pinned references in `tmp/`:
`pi-subagents` @v0.68.0 · `pi` · `pi-intercom` · `pi-permission-system` · **`herdr` @d59d060
(v0.9.1)** · `code_puppy_core_plugins` (a second, independent herdr client).
