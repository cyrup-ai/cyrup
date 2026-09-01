---
stage: exec
status: done
updated: 2026-08-29 05:55
severity: high
effort: small
subsystem: config / launch mode
source: docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md
item: SUBA-083
---

> **Path note.** This task lives in a subdirectory. Every flux command globs a single level
> (`ls -1 "$FLUX_BASE/todo/"*.md`), so `/exec`, `/aug` and `/qa` will not list it — pass the
> absolute path explicitly.

# SUBA-083 — remaining work: pin the seam tests that assert foreground

The production fix is **done and verified** — seed, citation, field doc, pi-parity label on the
default pin, and the both-directions `is_background` binding are all in place; `is_background` and
`routing.rs` are untouched. **Do not re-open any of it.** What follows is the entire remaining job.

---

## What this augmentation changes about the previous plan

The QA verdict said "pin all 11 files". **That is wrong, and pinning blindly would damage two
files.** Every dispatch site has now been read and classified by what it actually sends and what it
actually asserts. The result: **9 files need the pin, 2 do not**, and inside two of the nine only
some sites qualify.

The mechanism that decides it, in one line: `async_by_default` is consulted **only** by
`is_background`, which has exactly three callers — [`routing.rs:336`](../../../crates/cyrup-ext-subagents/src/extension/tool/routing.rs),
`:1456`, `:1599`, all on the **launch** path. So a site is affected **iff** it dispatches a launch
(an `agent`+`task` / `tasks` / `chain` payload) with `async` omitted **and** its assertion depends
on the call having completed.

Three kinds of site are therefore **immune**, and must be left alone:

| immune because | example |
|---|---|
| the payload is a **management verb** (`action`), never a launch | `{"action":"disable"}`, `{"action":"stop"}` |
| resolution **fails before** the launch-mode decision | unknown agent, ambiguous alias |
| the code path **renders** rather than launches | `render_tool_call(args)` is a pure function of the payload |

---

## The edit

One shape, applied at each site named below:

```rust
SubagentExtensionConfig { async_by_default: false, ..SubagentExtensionConfig::default() }
```

Where a site already builds a struct literal (the `scoped_config()` helpers), add the field to the
existing literal instead of nesting a second one.

Give each pin a one-line comment in the house style, e.g.:

```rust
// SUBA-083: this test asserts the child's completed output, so it states its launch mode
// rather than inheriting it — the config default backgrounds (pi `config.ts:222-224`).
```

**Do not** add `"async": false` to a dispatch payload — that swaps what the test exercises (an
explicit param for the config default) and would leave the suite with no coverage of the config
seam. **Do not** invert an expectation to accept a run id.

---

## Sites that need the pin — 9 files, verified individually

| file | site(s) | why it needs foreground |
|---|---|---|
| `extension_end_to_end_smoke.rs` | `:155`, and the failing-child test's own `::default()` | asserts `SMOKE_TEST_SUBAGENT_OUTPUT: the real child ran` **verbatim** (`:231-236`), and the sibling asserts the error text |
| `single_mode_overrides_integration.rs` | `:127`, `:235`, **`:359`**, **`:521`** | `:127`/`:235` assert `tool_result_text` content; `:359` and `:521` are the shared helpers `run_single_with` (`:331`) and `run_slash_run_with` (`:495`) — see the note below, this is the subtle one |
| `result_intercom_delivery_integration.rs` | `:138`, `:257` | `:278` says it outright: *"PARALLEL-mode **foreground** completion must attempt exactly one delivery"* |
| `acceptance_memo_key_and_live_wiring.rs` | `:636`, `:743` | both assert verify-command `execution_count` / memoization, which only a completed run produces |
| `control_notice_pipeline_integration.rs` | `:225` | asserts the notice pipeline fired with `agent="worker"`, `reason="idle"` — events only a settled run emits |
| `run_state_signal_and_stop_parity.rs` | **`:353` only** | `:353` is out-of-band delivery of a signal-killed child. **`:490` is immune** — it dispatches `{"action":"stop"}` |
| `tool_parallel_chain_integration.rs` | helper `:41` | spawns real children and asserts their output |
| `companions_wiring_proof.rs` | helper `:54` | asserts the parent-session anchor inside the real child's env |
| `subagent_tool_renderer_integration.rs` | helper `:39` | only for the launch at `:332`, `a_real_tool_result_renders_through_the_settled_branch` — **settled** is the foreground branch; backgrounded it would render the async-start branch instead |

### The subtle one — `single_mode`'s two shared helpers

`run_single_with` (`:331`) and `run_slash_run_with` (`:495`) each serve two groups of tests:

- the timeout tests (`:395`, `:407`, `:428`, `:444`, `:544`, `:574`) — plainly foreground; and
- **`:456` `an_agent_async_default_backgrounds_a_call_that_omits_async`** and **`:558`
  `the_run_slash_command_applies_the_agents_async_default_too`**.

Those two prove that a **persona-level** `launch.async` in agent frontmatter backgrounds a call that
omits `async`. Pinning the helper is not merely safe for them — **it is what keeps them meaningful.**
With the config seed now `true`, such a call backgrounds anyway, so the assertion could no longer
distinguish *"the agent's launch default did it"* from *"the config default did it"*: the test would
still pass while proving nothing. Pinning the config `false` restores the contrast the test was
written to draw. Say so in the comment at both helpers.

---

## Sites that must NOT be pinned

| file / site | reason |
|---|---|
| `discovery_project_root_wiring_integration.rs` (`:246`) | its only config site is the `dispatch()` helper (`:244`), used solely by the two ambiguous-alias tests, which **abort before** the launch-mode decision. The file needs no edit at all. |
| `management_actions_tool_dispatch_integration.rs` (`:74`) | dispatches `{"action":"disable"}` / `{"action":"enable"}` — management verbs that never reach a launch. **The previous plan listed this file in error.** |
| `run_state_signal_and_stop_parity.rs` `:490` | `{"action":"stop"}` — management verb. |
| `extension_end_to_end_smoke.rs` unknown-agent test (`:277`) | `"ghost"` fails resolution before the launch-mode decision. Indifferent; pin it only if you want the file uniform, and if you do, say in the comment that it is for consistency, not necessity. |

---

## Known blocker — read before starting

With the feature armed, `cargo test -p cyrup-it --features it` **does not compile**, for reasons
predating this task. ~18 seam files construct `RunnerConfig`, `AgentConfig`, `RunOptions`,
`ResolvedAgentPersona` and `SubagentSettings` without fields added by earlier work —
`permission_rules`, `runner`, `thinking_ceiling`, `max_thinking`. Confirmed: `permission_rules`
appears 14× in [`src/background/runner_main.rs`](../../../crates/cyrup-ext-subagents/src/background/runner_main.rs)
and 0× in `background_runner_main_integration.rs`, which constructs that struct at `:220` and `:330`.
No error mentions `async_by_default`.

Because every target sits behind `required-features = ["it"]`, plain `cargo test -p cyrup-it`
compiles nothing and reports success — **it is not evidence of anything.** Do not cite it.

**Two of the nine files carry pre-existing compile errors** — `run_state_signal_and_stop_parity.rs`
and `companions_wiring_proof.rs`. Edit them anyway; your one-line pin neither causes nor worsens
those errors. The other seven are compile-clean today.

Consequence for this task: **apply the pins, and state plainly in the completion report that they
are unexecuted.** Do not claim them verified. Do not attempt to repair the unrelated struct drift —
it spans at least three prior items and belongs in its own task, together with the more valuable
finding that `required-features` hides the whole crate from the merge gate.

---

## Definition of done

1. The nine files above carry `async_by_default: false` at exactly the sites named — including
   `run_state_signal_and_stop_parity.rs:353` but **not** `:490` — each with a brief `// SUBA-083:`
   comment naming what the test asserts.
2. The comments at `single_mode_overrides_integration.rs:359` and `:521` state that the pin
   preserves the contrast for `:456` / `:558`, which would otherwise become vacuous.
3. `discovery_project_root_wiring_integration.rs` and
   `management_actions_tool_dispatch_integration.rs` are **unmodified**.
4. No dispatch payload gains an `"async"` key; no expectation is inverted; `is_background`,
   `routing.rs` and `registration/mod.rs` are untouched.
5. `cargo clippy -p cyrup-ext-subagents --all-targets` and `cargo doc -p cyrup-ext-subagents
   --no-deps` stay clean — the only pre-existing warning in range is `extension/tool/text.rs:334`.
6. The completion report states that the seam pins could not be executed, and why.
