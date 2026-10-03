# Rust type-driven design review

The standard for every new subsystem and every significant redesign in this workspace. It applies
to design proposals and to review of existing code alike. cyrup is a port, so the pressure to
transliterate an upstream TypeScript shape is constant; this document is what that pressure is
measured against.

`docs/adr/ADR-0028-cyrup-acp-type-design.md` is the worked precedent: it applied these rules to
`crates/cyrup-acp` and concluded **enums, not typestate**, because an ACP agent is driven by an
editor sending JSON-RPC in whatever order it likes. The conclusion mattered less than the fact that
the rule chose it.

## The four patterns

1. Newtypes with **parse, don't validate**
2. Typestate
3. Explicit domain enums with exhaustive matching
4. Functional core, imperative shell

**This is an opportunity review, not a mandate.** Prefer the smallest design change that captures a
meaningful invariant. **"No meaningful opportunity" is an acceptable conclusion**, and reaching for a
pattern that is not earned is a finding against the design, not for it.

## Decision rules

| signal | consider |
|---|---|
| invalid **value** | newtype |
| invalid **sequence** of operations | typestate |
| state that must be **inspected dynamically** at runtime | enum |
| domain logic tangled with I/O or shared mutation | functional core / imperative shell |
| expected **business outcome** | named enum variant |
| technical failure that **aborts** the operation | `Result<T, E>` |

Patterns combine. But **never reach for typestate when a newtype, an ordinary enum, a private
constructor, or a smaller pure function gives the same guarantee more simply.**

## When typestate is justified

All of these must hold:

- the legal states and transitions are finite and reasonably stable;
- the compiler can know the current state at the relevant program point;
- preventing the invalid sequence is materially valuable;
- the state count stays understandable;
- the resulting API is clearer than runtime checks;
- the workflow is **not** primarily driven by arbitrary external events.

Prefer consuming `self` when the previous state must not be reused; state-specific types holding
only the data valid in that state; methods exposed only where they are legal.

### When typestate is the wrong answer

Do not use it because a function is long, the code is sequential, or helpers could be extracted.
Specifically **reject** it when:

- the state must be **persisted, resumed, stored heterogeneously, or selected by external events**;
- a runtime enum already models genuinely dynamic state well;
- it would create a combinatorial explosion of generic states;
- requirements frequently reorder or insert stages;
- **the compiler-visible lifecycle ends before the real distributed or external lifecycle does.**

That last rule is the one most often missed. A durable, resumable state machine is reconstructed from
storage, so the compiler never sees its transitions: it is an enum, whatever its in-process lifecycle
looks like.

## Newtypes that actually hold

A newtype is worthless if it can be forged. Audit every one for:

- public fields or public unchecked constructors;
- `derive(Default)` where no valid default exists;
- `derive(Deserialize)` that bypasses the validating path — **serde is a construction path**;
- overly broad `From` impls;
- test-only constructors leaking into production;
- APIs that immediately unwrap it back to the primitive;
- `Clone`/`Copy` on a type meant to be one-use authority.

Create it at the system boundary, keep the inner representation private, construct through
`TryFrom`/`FromStr` or another fallible parser, and keep it intact downstream.

## Explicit domain enums

Replace outcomes encoded implicitly as empty collections, `bool` plus optional data, sentinels,
nested `Option`/`Result`, generic `Either`, or outcome strings. Name them for the domain —
`NothingToProcess`, `PartiallyCompleted`, `RejectedByPolicy`, `CompletedWithItemErrors` — not
`Left`/`Right`.

Keep the categories apart: expected domain outcomes are enum variants; technical failures that abort
the operation are `Result<T, E>`. Do not fuse unrelated fatal and recoverable outcomes into one vague
error type without a stated reason.

## Functional core, imperative shell

Separate acquiring inputs, performing I/O, retries and transport (shell) from transforming explicit
inputs into explicit domain decisions (core). Signals: functions needing many mocks; domain rules
buried in async orchestration; mutable accumulators threaded through call chains; functions that both
decide and perform.

Not every function must be pure. Target the domain decisions whose isolation materially improves
reasoning and testing. Keep typestate transitions pure where practical, and pass a dependency only to
the operation that needs it rather than carrying it through every state.

## State the exact guarantee

Every recommendation states **both**:

1. what becomes impossible or harder to express, and
2. what remains possible and must still be tested.

Do not overstate. A `VerifiedToken` does not prove the verifier is correct. Typestate constrains calls
through the encoded ownership path; it does not stop an external system changing underneath.
Compile-time ordering does not solve crashes, retries, distributed transactions or partial external
effects. A newtype guarantees nothing if its constructor can be bypassed — including by serde.

## Priorities

- **P1** high-value, concrete, favourable cost
- **P2** useful, with meaningful tradeoffs
- **P3** plausible, needs more domain evidence
- **Reject** more ceremony than protection

Weigh impact if violated, likelihood of misuse under the current API, how completely the compiler
could prevent it, duplicated validation removed, testability, migration scope, boilerplate, and risk
of state explosion. **Cap the main findings at five** unless more represent serious correctness or
security risk.

## Required output

1. **Executive verdict** — which pattern the design primarily needs, the highest-value opportunity,
   whether typestate is actually justified, the major tradeoff, and any missing domain information.
2. **Invariant and state map** —

   | Location | Domain fact or state | Current encoding | Failure mode | Best representation |
   |---|---|---|---|---|

3. **Findings** — per finding: location; current representation; the invariant or legal sequence; a
   **concrete** failure mode the current API permits (not a theoretical one); recommended pattern;
   why it beats the simpler alternatives; a minimal API sketch; guarantee gained; **guarantee not
   gained**; migration cost; benefit versus ceremony; confidence.
4. **Highest-value refactor sketch** — before/after for the single strongest recommendation.
5. **Deliberately rejected opportunities** — **required.** Where these patterns look attractive and
   should not be applied, and what is better instead. This section is what prevents
   pattern-driven overengineering.
6. **Incremental migration plan** — the smallest safe sequence that keeps the code compiling at every
   stage. Centralize parsing, introduce the type, convert boundaries, propagate, add outcome enums,
   isolate pure transforms, and **add typestate only once the transitions are clear**.
7. **Test implications** — which validation tests become parser tests, which behaviour tests remain,
   which become redundant because the invalid state no longer compiles, whether compile-fail tests
   are worth it, and which shell-level integration tests are still needed.

## Standards

Tie every finding to concrete code or a concrete design commitment. Do not infer domain requirements
silently — label an inference an inference and an open question an open question, and never invent a
business rule. More types is not better design. Prefer newtypes at boundaries over halfway through the
call graph. Do not unwrap a newtype immediately after constructing it. Keep constructors and fields
private where they establish an invariant. Account for serde and every other construction path. Be
explicit about readability and migration cost. Distinguish compile-time guarantees from runtime and
distributed-system guarantees.
