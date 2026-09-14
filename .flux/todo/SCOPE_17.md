---
stage: exec
status: in-progress
updated: 2026-09-07 14:40
---

# SCOPE_17 — every subagent produces a reliable, bounded result summary, and every fan-out child's own summary reaches the orchestrator

## THE GOAL, IN ONE PARAGRAPH

A subagent's result has to cross into somebody else's context — the orchestrator's. Today that
crossing is all-or-nothing: either the child's entire output is pasted in, or (for a fan-out)
nothing usable arrives at all. Both are wrong. **Every subagent, foreground or background, should
end its work with a short, self-contained summary of its result, produced on request as a
structured block; and every surface that carries a child's result into another agent's context
should carry THAT summary plus a link to the full output — never an unbounded dump, and never
nothing.** The fan-out completion notification is the surface where this is currently broken badly
enough to fail a basic task, so it is the acceptance test; the mechanism is deliberately generic.

## THE ONLY TEST THAT MATTERS

> **When this is done, launching three async subagents that each generate a random number produces
> completion notifications in which all three agents report their own number, attributed to them,
> and the orchestrator sums them without touching `bash`.**

```bash
cargo run --manifest-path ./crates/cyrup/Cargo.toml -- \
  -p "run three async subagents. each will generate a random number and then you'll sum all three for me"
```

**Pass** = the three numbers arrive through the subagent surfaces and the agent sums them.
**Fail** = the transcript contains `bash`, `python3`, `events.jsonl`, or `status.json`.

No partial credit. If the run still needs `bash`, this is not done.

---

## §1 — What already landed, and the one line that made it invisible

A previous pass fixed the **payload** and stopped. That work is correct and stays — it is the
precondition for everything below, because a renderer cannot attribute per-child output that the
payload collapsed into one aggregate.

**Landed (keep, do not re-do):**

* [`spawn/chain_graph.rs`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/spawn/chain_graph.rs)
  `collapse_fan_out` folds children's `final_output`, derives `output_state`.
* [`runner_main/settle.rs`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/runner_main/settle.rs)
  flattens a `ParallelGroup` into one `SingleResult` per member (pi `subagent-runner.ts:4229-4230`).
  Verified on a real run: `results.len()==2`, each with its own `agent`, `finalOutput`, real `usage`,
  real `model`.
* [`runner_main/finish.rs`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/runner_main/finish.rs)
  run `agent` = `parallel:a+b+c` (pi `subagent-runner.ts:4765-4770`).

**Why the real run still failed:** the orchestrator reads the *rendered* text, and
[`result_display_summary`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/watch/message.rs)
throws the attribution away. Observed live:

```
wait            → Waited 10.8s for 1 async run(s); done. Outcome: 1 complete.
subagent status → Progress: 3/3 complete · Agent 1/3: delegate complete    ← no output
"I need the actual results from each agent, so let me check the events file"
  $ ls … && grep -o '"result":"[^"]*"' events.jsonl
  $ python3 -c "import json …"        ← three more
```

---

## §2 — Root cause, verified on both sides

**cyrup** ([`watch/message.rs:70-84`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/watch/message.rs)):

```rust
fn result_display_summary(result: &ResultFile) -> String {
    result.results.iter()
        .filter_map(|child| {
            let text = child.final_output.clone().or_else(|| child.error.clone())?;
            if text.trim().is_empty() { None } else { Some(text) }
        })
        .collect::<Vec<_>>().join("\n\n")
}
```

**upstream** ([`subagent-runner.ts:4744`](../../../workspace/pi-subagents/src/runs/background/subagent-runner.ts)) —
one line, and it is the entire feature:

```ts
let summary = results
    .map((r) => `${r.agent}:\n${r.output || (r.exitCode !== 0 ? r.error : undefined) || "(no output)"}`)
    .join("\n\n");
```

| # | divergence | consequence |
|---|---|---|
| 1 | no `{agent}:` prefix | three anonymous values; the orchestrator cannot attribute them |
| 2 | empty children silently dropped | 3 agents can render as 2 blocks; a silent child is indistinguishable from a nonexistent one |
| 3 | `error` used regardless of exit code | pi falls back to `error` **only** when `exitCode !== 0`; a stale error masks a successful child's output |
| 4 | no per-child `(no output)` | collapses to one run-level `(no output)` |
| 5 | no structured-output fallback | a child whose answer is structured renders `(no output)` while the payload holds the answer |
| 6 | **the saved-output POINTER is discarded** | a child whose answer went to a FILE renders `(no output)` while `artifact_paths.output_path` — documented as *"the child's delivered answer"* — names it on disk. This is the exact file the live agent `cat`'d by hand (`…_delegate_0_output.md`), i.e. the dead end that sends it to `bash` |

Divergence 5 is upstream's `childInlinePreview` (`notify.ts:198-212`), verbatim:

```ts
const raw = child.outputState === "absent" || referenceOnly
    ? structuredOutputText(child.structuredOutput)
    : isDegenerateOutput(output) ? structuredOutputText(child.structuredOutput) ?? output : output;
```

with (`notify.ts:184-196`, `:132`):

```ts
function structuredOutputText(v) { return JSON.stringify(v, null, 2); }   // pretty, 2-space
function isDegenerateOutput(t) { return !t.trim() || t.trim() === "</think>"; }
const CHILD_OUTPUT_PREVIEW_MAX_BYTES = 4 * 1024;
```

---

## SUBTASK0 — the summary contract: ask every child for a bounded result block

**Why prompting rather than truncation.** A child's answer is not uniformly distributed through its
output. Truncating from the HEAD yields *"I'll generate a random number using shuf…"* and discards
the number. Truncating from the TAIL is better — conclusions live at the end — but is still a
guess. Asking the child to *emit* a delimited summary turns the guess into an extraction, and
costs one paragraph of system prompt.

**This is generic over foreground and background by construction**, because both paths compose
their child's argv through the same builder: `spawn_plan.rs`'s persona composition, which already
folds a run-level instruction into the persona body at
[`spawn_plan.rs:992-995`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/exec/spawn_plan.rs)
(inside `compose_persona`, the function both this fold and the new one live in):

```rust
let persona_owned = crate::exec::turn_budget::append_turn_budget_system_prompt(
    &persona_owned, opts.turn_budget.as_ref(),
);
```

**Verified, not assumed: the foreground/background claim.** `compose_persona`'s only budget input is
`opts: &RunOptions` — it never sees the separate `structured_runtime` parameter
[`build_attempt_spawn_plan`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/exec/spawn_plan.rs)
takes (that parameter exists only to wire the schema FILE PATH env vars, a different concern, in a
different function). The one signal reachable inside `compose_persona` is
`opts.structured_output_schema: Option<serde_json::Value>`
([`exec/agent_config.rs:444`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/exec/agent_config.rs)),
and it is the RIGHT one: production code derives `structured_runtime` FROM this same field, nowhere
else —
[`exec/mod.rs:1151-1164`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/exec/mod.rs)'s
`structured_guard` is `opts.structured_output_schema.as_ref().and_then(|schema|
create_structured_output_runtime(schema, …).ok())` — and the background runner populates the SAME
field on its own `RunOptions` the SAME way
([`runner_main/executor.rs:605`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/runner_main/executor.rs):
`structured_output_schema: step.structured_output_schema.clone()`) before calling the SAME
`exec::run_sync`. So `opts.structured_output_schema.is_none()` is not merely available at this
composition site — it is the crate's one canonical "does this run expect structured output"
signal, foreground and background alike. (One spawn_plan.rs unit test,
`a_declared_output_schema_reaches_the_child_as_both_env_vars`, passes a hand-built runtime without
setting this field — that is a deliberately narrow test of the env-var wiring alone, decoupled from
`RunOptions` on purpose, not a second production pathway; do not generalize from it.)

**Where:** a new `exec/result_summary.rs`, modelled field-for-field on
[`exec/turn_budget.rs:162-197`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/exec/turn_budget.rs)
— the same signature shape, the same `## Heading` markdown block, the same trim-and-join, the same
untouched passthrough when it does not apply. Declare the module in
[`exec/mod.rs`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/exec/mod.rs) alphabetically,
between the existing `pub mod permissions;` and `pub mod spawn_budget;` (both already there, in
that order):

```rust
pub mod permissions;
pub mod result_summary;
pub mod spawn_budget;
```

Compose it at the same site, immediately after the turn-budget fold, before `let persona_body: &str
= &persona_owned;` (`spawn_plan.rs:992-996`):

```rust
let persona_owned = crate::exec::turn_budget::append_turn_budget_system_prompt(
    &persona_owned,
    opts.turn_budget.as_ref(),
);
// SCOPE_17 — the OUTERMOST fold, same reasoning as the turn-budget one immediately above: nothing
// composed earlier can displace it, and it is the ONE seam a foreground child and a detached
// fan-out member both pass through.
let persona_owned = crate::exec::result_summary::append_result_summary_system_prompt(
    &persona_owned,
    opts.structured_output_schema.is_none(),
);
let persona_body: &str = &persona_owned;
```

```rust
/// Fold the result-summary contract onto the child's system prompt, so its answer can cross into
/// another agent's context bounded rather than whole.
///
/// Sibling of [`crate::exec::turn_budget::append_turn_budget_system_prompt`] and composed at the
/// same point in `spawn_plan`, which is the ONE seam every child argv is built through — so a
/// foreground child and a detached fan-out member get the identical contract with no per-path
/// wiring.
///
/// # Not applied when the child is already structured
///
/// An agent with an `output_schema` is contractually producing a machine-readable value; asking it
/// to ALSO emit a prose block invites it to satisfy one contract and break the other, and
/// `structured_output` is already a better summary than any prose tail. Returns the persona
/// untouched in that case, exactly as the turn-budget appender does with no budget.
#[must_use]
pub fn append_result_summary_system_prompt(system_prompt: &str, applies: bool) -> String {
    if !applies {
        return system_prompt.to_string();
    }
    let block = [
        "## Result summary".to_string(),
        "End your final response with a summary block, exactly:".to_string(),
        String::new(),
        "<<<RESULT>>>".to_string(),
        "<your result here>".to_string(),
        String::new(),
        "Put the ANSWER in that block — the value, decision, or outcome asked for — not a description of".to_string(),
        "what you did. Keep it under 400 characters. Everything above the marker is kept in full for anyone".to_string(),
        "who needs the detail; the block is what other agents see first.".to_string(),
    ]
    .join("\n");
    let trimmed = system_prompt.trim();
    if trimmed.is_empty() {
        block
    } else {
        format!("{trimmed}\n\n{block}")
    }
}
```

Byte-for-byte the same trim/join/empty-passthrough idiom as
[`append_turn_budget_system_prompt`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/exec/turn_budget.rs)
— reuse that shape, do not invent a second one.

The block renders as, in the same voice as the turn-budget block:

```text
## Result summary
End your final response with a summary block, exactly:

<<<RESULT>>>
<your result here>

Put the ANSWER in that block — the value, decision, or outcome asked for — not a description of
what you did. Keep it under 400 characters. Everything above the marker is kept in full for anyone
who needs the detail; the block is what other agents see first.
```

**Marker design, and why these choices:**

* `<<<RESULT>>>` on its own line — not `### Result` — because a markdown heading is something an
  agent writes incidentally all the time, and a false positive silently truncates a real answer.
  The delimiter has to be improbable in ordinary prose.
* The block is a TAIL, not a wrapper: nothing follows it. That makes extraction "everything after
  the last marker" and keeps the fallback (plain tail truncation) aligned with the fast path.
* "under 400 characters" is guidance to the model, never enforcement — [`extract_result_summary`]
  bounds it regardless. Do not add a validation error for a child that overruns; that would fail a
  run over formatting.

**Five existing `spawn_plan.rs` assertions this fold changes the value of.** Every one of them
builds its `RunOptions` through the shared `base_opts` test fixture
([`exec/testsupport.rs:53`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/exec/testsupport.rs)),
which sets `structured_output_schema: None` — so `applies` is `true` for all five, and each pins the
persona as EXACTLY what it was before this fold existed. This is expected, unavoidable collateral of
making the contract reach every non-structured child with no opt-out, not a sign anything is wrong,
and every fix below is the same mechanical move: replace an exact-equality assertion with a
`starts_with`, because the persona is still a PREFIX of the new value, never something a substring
check could mistake for a different agent's text. No OTHER test in this crate or in `cyrup-it` is
affected — `delivered_system_prompt`/`read_system_prompt_arg` (the exact-content test helpers) exist
only in this one file, and the two structured-output-env tests
(`a_declared_output_schema_reaches_the_child_as_both_env_vars`,
`no_output_schema_means_no_structured_output_env`) assert only on `env_overlay` keys, never on
system-prompt content, so they are untouched.

| Test (`spawn_plan.rs`) | Current exact-match assertion | Change to |
| --- | --- | --- |
| `build_attempt_spawn_plan_delivers_the_persona_body_as_system_prompt_in_replace_mode` (`:2625`, assertion `:2661`) | `assert_eq!(delivered, "- You are the REVIEWER persona.\n- Only review.", …)` | `assert!(delivered.starts_with("- You are the REVIEWER persona.\n- Only review.\n\n## Result summary\n"), …)` |
| `an_agent_without_a_memory_scope_ships_only_its_persona` (`:3212`, assertion `:3231`) | `assert_eq!(delivered_system_prompt(&argv).as_deref(), Some("- persona"), …)` | `assert!(delivered_system_prompt(&argv).is_some_and(\|d\| d.starts_with("- persona\n\n## Result summary\n")), …)` |
| `build_attempt_spawn_plan_delivers_the_persona_body_as_append_in_append_mode` (`:3730`, assertion `:3758`) | `assert_eq!(delivered, "You are a delegate persona.", …)` | `assert!(delivered.starts_with("You are a delegate persona.\n\n## Result summary\n"), …)` |
| `build_attempt_spawn_plan_leaves_the_system_prompt_alone_without_an_output_path` (`:3870`, assertion `:3891`) | `assert_eq!(delivered_system_prompt(&argv).as_deref(), Some("- persona"), …)` | Same `starts_with` rewrite as row 2 — this test's OWN point (output-path alone leaves the prompt alone) is unaffected; turn-budget and result-summary already apply unconditionally, before this task ran |
| `the_turn_budget_notice_reaches_the_child_through_the_spilled_system_prompt_file` (`:5373`), first half, NO budget set | `assert_eq!(unbudgeted.trim(), "You are a careful worker.");` | `assert!(unbudgeted.trim().starts_with("You are a careful worker.\n\n## Result summary\n"), "{unbudgeted}");` — leave the very next line, `assert!(!unbudgeted.contains("## Turn budget"), …)`, exactly as it is |

That same test's second half (`budgeted.starts_with("You are a careful worker.\n\n## Turn budget\n")`
plus two `budgeted.contains(...)` checks) is already `starts_with`/`contains`, not exact-equality,
and needs no change — the result-summary block lands AFTER the turn-budget block in that ordering,
which a prefix/substring check does not see. Re-run
`rg 'delivered_system_prompt|read_system_prompt_arg' crates/cyrup-ext-subagents/src/exec/spawn_plan.rs`
before starting, in case a change lands between this research pass and implementation and adds a
sixth site.

---

## SUBTASK0b — extract it, with a fallback that cannot fail

**Where:** the same `exec/result_summary.rs`, used by every consuming surface.

```rust
/// The bounded summary of one child's output: the contract block when the child honoured it, the
/// output's TAIL when it did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultSummary {
    /// The summary text, bounded to `max_bytes` on a UTF-8 boundary.
    pub text: String,
    /// `true` when the child honoured the contract — diagnostic only; both sources are usable and
    /// neither is rendered differently.
    pub from_contract: bool,
    /// `true` when text was dropped to fit, so a renderer can say so and point at the full output.
    pub truncated: bool,
}

/// The tail marker [`append_result_summary_system_prompt`] asks every child to close its response
/// with.
const RESULT_MARKER: &str = "<<<RESULT>>>";

/// # The fallback is the load-bearing half
///
/// The marker is the fast path, not the contract. A child that ignores the instruction, a model
/// that reformats it, an agent definition predating it, a `Replace`-mode persona that dropped it —
/// all still yield a usable summary, because a conclusion sits at the END of a response. That is
/// why this degrades instead of breaking, and why no agent definition needs migrating.
///
/// TAIL, not head: [`crate::exec::child_protocol::BoundedByteTail`] is this crate's existing
/// UTF-8-boundary-safe tail ring (`.push(bytes)` then `.text()`) — the exact same primitive
/// `child-protocol.ts`'s stderr capture uses. Head truncation would return the preamble and drop
/// the answer — the exact failure this task exists to fix.
///
/// The LAST marker occurrence wins ([`str::rfind`], never `find`), so a child that quotes the
/// format while reasoning about it cannot spoof an earlier block. An empty block after the last
/// marker (the child emitted the delimiter but nothing after it) falls through to the tail exactly
/// as if no marker had been emitted at all.
#[must_use]
pub fn extract_result_summary(final_output: &str, max_bytes: usize) -> ResultSummary {
    let (source, from_contract) = match final_output.rfind(RESULT_MARKER) {
        Some(marker_at) => {
            let after = final_output
                .get(marker_at + RESULT_MARKER.len()..)
                .unwrap_or("")
                .trim();
            if after.is_empty() {
                (final_output, false)
            } else {
                (after, true)
            }
        }
        None => (final_output, false),
    };
    let source = source.trim();
    let cap = max_bytes.max(1);
    let mut tail = crate::exec::child_protocol::BoundedByteTail::new(cap);
    tail.push(source.as_bytes());
    ResultSummary {
        text: tail.text(),
        from_contract,
        truncated: source.len() > cap,
    }
}
```

`.get(marker_at + RESULT_MARKER.len()..)`, never `[marker_at + RESULT_MARKER.len()..]` — this crate
denies `clippy::indexing_slicing` crate-wide
([`lib.rs:19-24`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/lib.rs)). The offset is
always a valid char boundary here regardless (an `rfind` match start, plus the byte length of an
ASCII pattern, both land on boundaries), so `.get` never actually returns `None` in practice —
`unwrap_or` is defensive, not a real branch.

---

## SUBTASK1 — port `subagent-runner.ts:4744` into the renderer

**Where:** [`background/watch/message.rs`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/watch/message.rs).
Current imports there are just `use super::classify::{ClassifiedOutcome, classify_outcome};` and
`use crate::background::ResultFile;` — add three more the code below needs:

```rust
use crate::exec::SingleResult;
use crate::exec::output_state::SubagentOutputState;
use crate::exec::result_summary::extract_result_summary;
```

`SubagentOutputState` specifically as `crate::exec::output_state::SubagentOutputState` — NOT
`crate::exec::SubagentOutputState`: `exec/mod.rs`'s re-export block only globs `agent_config`,
`progress`, `run_result` and `spawn_plan` at the crate root (`exec/mod.rs:102-106`); `output_state`
is not among them, and every existing call site in this crate (`run_result.rs`, `exec/mod.rs`)
spells it out in full — match that, do not assume a root re-export that does not exist.

**Scope note on `NO_OUTPUT`:** this crate already has three OTHER, independent `"(no output)"`
literals —
[`extension/tool/routing.rs:777`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/tool/routing.rs)
(a FOREGROUND single-run tool result),
[`extension/host/native_impl.rs:747`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/host/native_impl.rs)
(an MCP/host bridge rendering a generic tool-result JSON blob), and
[`extension/executor/paths.rs:414`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/paths.rs)
(a job/export formatter) — each porting a DIFFERENT upstream fallback for a DIFFERENT surface this
task does not touch, and none reachable from the fan-out completion path. "One `NO_OUTPUT`
constant" in the Definition of Done means one inside `watch/message.rs`, not a crate-wide dedup of
the string literal — leave those three alone; touching them is scope creep.

```rust
/// pi's `"(no output)"` (`subagent-runner.ts:4744`, `notify.ts:242`). ONE constant, both sites.
const NO_OUTPUT: &str = "(no output)";

/// pi `CHILD_OUTPUT_PREVIEW_MAX_BYTES` (`notify.ts:132`).
const CHILD_OUTPUT_PREVIEW_MAX_BYTES: usize = 4 * 1024;

/// pi's terminal `summary` (`subagent-runner.ts:4744`), ported line for line.
///
/// # Every child gets a block, and every block names its agent
///
/// This IS the feature, not formatting. A fan-out's completion is the only channel carrying a
/// background child's text to the orchestrator; without the `{agent}:` prefix three children
/// arrive as three anonymous values it can neither attribute nor count.
///
/// Three rules that are easy to get wrong, all upstream's:
///
/// * **A child NEVER disappears.** An empty child renders `agent:\n(no output)`. The previous
///   implementation filtered empties out, so N children could render as fewer than N blocks.
/// * **`error` is a fallback ONLY for a non-zero exit** (`r.exitCode !== 0 ? r.error : undefined`).
///   A child that succeeded while carrying a stale `error` must still report its output.
/// * **`||` is JS string truthiness**: an EMPTY string falls through to the next arm. `Some("")`
///   must fall through too, so the test is "non-empty after trim", never `is_some`.
fn result_display_summary(result: &ResultFile) -> String {
    let total = result.results.len();
    result
        .results
        .iter()
        .enumerate()
        .map(|(index, child)| {
            format!(
                "{}{}:\n{}",
                child.agent,
                child_position(index, total),
                child_display_body(child)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// pi's `taskInfo` suffix (` (1/3)`, `notify.ts:481-484` from `taskIndex`/`totalTasks`).
///
/// Upstream's `CompletionNotification.taskIndex`/`totalTasks` (`notify.ts:103-104`) have NO
/// producer at pi HEAD `df26ebc8` — verified by grep; the fields are vestigial there. The shape is
/// exactly what a fan-out needs and is adopted here: three children of the SAME agent (`delegate`,
/// `delegate`, `delegate`) are otherwise indistinguishable, so the orchestrator cannot tell three
/// results from one repeated three times, nor notice that a fourth is missing.
///
/// Omitted for a single-child run so a plain `subagent({agent, async})` notification stays
/// byte-identical to today.
fn child_position(index: usize, total: usize) -> String {
    if total <= 1 {
        return String::new();
    }
    format!(" ({}/{total})", index + 1)
}

/// One child's rendered body: pi's `r.output || (exitCode !== 0 ? r.error : undefined) ||
/// "(no output)"` (`subagent-runner.ts:4744`) with `childInlinePreview`'s structured-output
/// fallback (`notify.ts:198-212`) spliced in where upstream's `resultPreview` would apply it.
///
/// # `(no output)` is the LAST rung, and reaching it must mean there is genuinely nothing
///
/// `NO_OUTPUT` does not deliver output — it is the admission that none was found, and every rung
/// above it exists so that admission is TRUE rather than merely convenient. A terminal that says
/// "nothing" while a file on disk holds the answer is precisely the dead end that sent the live
/// orchestrator to `bash` to `cat` that file itself.
///
/// So rung 3 is a POINTER. [`crate::artifacts::ArtifactPaths::output_path`] is documented as "the
/// child's delivered answer" and is written for every child whose artifacts are enabled (the
/// default); [`SingleResult::saved_output_path`] is the R-SA-031 handoff's own concrete file.
/// Naming either one hands the orchestrator something it can act on with its `read` tool — no
/// shell, no `python3`.
///
/// **[CYRUP-DELTA]** upstream surfaces this path only inside the workflow-gated `Child outputs:`
/// block (`notify.ts:214-238`, gated on `workflowRunId` at `buildCompletionDetails:526`), so a
/// plain pi fan-out does render a bare `(no output)` here. cyrup promotes it to the shared ladder
/// because the fan-out case is the one this task exists for, and a reachable answer must never be
/// reported as absent. Stated here rather than left as an unexplained difference.
fn child_display_body(child: &SingleResult) -> String {
    let link = child_output_path(child).map(|path| format!("\nOutput saved to: {path}"));
    if let Some(text) = inline_preview(child) {
        // CONTEXT EFFICIENCY — the body is the child's SUMMARY, never a dump. Three chatty
        // children would otherwise put three unbounded transcripts into the orchestrator's context
        // on every fan-out completion, which is what makes a useful notification unaffordable.
        //
        // SUBTASK0's contract block when the child honoured it, its output's TAIL when it did not
        // — both bounded, both ending on the conclusion rather than the preamble.
        let bounded = extract_result_summary(&text, CHILD_OUTPUT_PREVIEW_MAX_BYTES);
        return match (bounded.truncated, &link) {
            // Truncated WITH a file: the marker is not decoration, it is the contract that the
            // orchestrator can still reach the whole answer without guessing that it was cut.
            (true, Some(link)) => format!("{}\n[truncated]{link}", bounded.text),
            (true, None) => format!("{}\n[truncated]", bounded.text),
            // Untruncated: still name the file. It costs one line and is what lets a follow-up
            // read the full artifact without a `status` round-trip.
            (false, Some(link)) => format!("{}{link}", bounded.text),
            (false, None) => bounded.text,
        };
    }
    if child.exit_code != 0
        && let Some(error) = non_empty(child.error.as_deref())
    {
        // Same bounded extractor as the success path, on the ERROR text instead of
        // `final_output` — a stale error is not a contract-emitting child, so this always takes
        // the tail-truncation fallback, never the marker branch.
        return extract_result_summary(&error, CHILD_OUTPUT_PREVIEW_MAX_BYTES).text;
    }
    if let Some(path) = child_output_path(child) {
        // pi's own reference wording (`childInlinePreview`'s `referenceOnly` probe,
        // `notify.ts:201`, tests `output.trim().startsWith("Output saved to:")`), so a body this
        // renderer emits is recognised as a reference by the same predicate that detects one.
        return format!("Output saved to: {path}");
    }
    NO_OUTPUT.to_string()
}

> **Correction to an earlier revision of this file.** A previous draft bounded this with
> `utf8_safe_prefix` (HEAD truncation, pi's `truncateUtf8Head`) and explicitly told the implementor
> NOT to use `BoundedByteTail`. That was backwards. Head-truncating a chatty child returns
> *"I'll generate a random number using shuf…"* and drops the number — the precise failure this
> task exists to fix. Summaries come from the END. `utf8_safe_prefix` remains correct for the
> STRUCTURED fallback below (JSON's meaning is front-loaded); it is wrong for prose.

/// The file holding this child's delivered answer when the text did not travel inline.
///
/// [`SingleResult::saved_output_path`] first — it is the explicitly requested R-SA-031 handoff and
/// therefore the caller's own chosen location — then the artifact bundle's `output_path`, which is
/// written unconditionally under the default artifact config.
fn child_output_path(child: &SingleResult) -> Option<String> {
    non_empty(child.saved_output_path.as_deref()).or_else(|| {
        child
            .artifact_paths
            .as_ref()
            .map(|paths| paths.output_path.display().to_string())
    })
}

/// pi `childInlinePreview`'s `raw` derivation (`notify.ts:198-212`), returning the text to show or
/// `None` when there is genuinely nothing.
///
/// The ordering is upstream's and is load-bearing: an `Absent` output-state or a reference-only
/// body goes STRAIGHT to structured output (the text is known not to be the answer), and only
/// then does a degenerate body fall back with the raw text as the second chance.
///
/// Returning `None` here is NOT the end of the line — [`child_display_body`]'s pointer rung runs
/// next. That matters most for the `reference_only` case: routing away from a "Output saved to:"
/// body and finding no structured output must not discard the reference, or a child whose answer
/// is on disk reports nothing.
fn inline_preview(child: &SingleResult) -> Option<String> {
    let output = child.final_output.as_deref().unwrap_or("");
    // pi `referenceOnly` (`:201`) — the body is just a pointer to a file, not the answer.
    let reference_only = child.saved_output_path.is_some()
        && output.trim_start().starts_with("Output saved to:");
    let raw = if child.output_state == SubagentOutputState::Absent || reference_only {
        structured_output_text(child.structured_output.as_ref())
    } else if is_degenerate_output(output) {
        structured_output_text(child.structured_output.as_ref())
            .or_else(|| non_empty(Some(output)))
    } else {
        non_empty(Some(output))
    };
    non_empty(raw.as_deref())
}

/// pi `isDegenerateOutput` (`notify.ts:194-196`) — blank, or a bare unclosed reasoning tag.
fn is_degenerate_output(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.is_empty() || trimmed == "</think>"
}

/// pi `structuredOutputText` (`notify.ts:184-192`) — `JSON.stringify(value, null, 2)`, bounded.
///
/// PRETTY (2-space), not compact: this lands in a model's context, and a wall of minified JSON is
/// exactly what the orchestrator was resorting to `python3` to unpack. `serde_json::to_string_pretty`
/// is 2-space by default, so it is upstream's output byte for byte.
///
/// Bounded by pi's own [`CHILD_OUTPUT_PREVIEW_MAX_BYTES`] through
/// [`crate::exec::output::utf8_safe_prefix`] — cyrup's existing port of `truncateUtf8Head`
/// (`exec/output.rs:1393`), which walks down to a char boundary rather than slicing bytes. Widen
/// that `fn` to `pub(crate)`; do NOT write a second boundary walk, and do NOT reach for
/// `BoundedByteTail`, which keeps the TAIL — for JSON the head is the part that carries meaning.
fn structured_output_text(value: Option<&serde_json::Value>) -> Option<String> {
    let text = serde_json::to_string_pretty(value?).ok()?;
    Some(crate::exec::output::utf8_safe_prefix(&text, CHILD_OUTPUT_PREVIEW_MAX_BYTES).to_string())
}

/// JS `value || next` on a string: empty (after trim) is falsy.
fn non_empty(value: Option<&str>) -> Option<String> {
    value.filter(|text| !text.trim().is_empty()).map(str::to_string)
}
```

`format_completion_message`'s existing run-level `(no output)` substitution **stays**: with the
above it can only fire for a run with ZERO children, which is the case it was always for. Reuse
[`NO_OUTPUT`] there rather than leaving a second literal.

Rendered result — the scope sentence, satisfied. **Note the `(i/n)` suffix**: three children
sharing the agent name `delegate` are exactly why [`child_position`] exists — without it these
three blocks are indistinguishable from one result repeated three times:

```text
Background task completed: **parallel:delegate+delegate+delegate**

delegate (1/3):
42

delegate (2/3):
17

delegate (3/3):
8
```

---

## SUBTASK2 — `wait` answers with the same blocks, so the race cannot bite

**The race, precisely.** `wait` wakes on [`CompletionBus`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/watch/observer.rs),
which is published from the **observer band** — and the observer band runs BEFORE delivery and
before the payload is unlinked (`watch/install.rs`: `should_observe()` → observers →
`should_deliver()` → `sink.deliver` → `delete_after_notify`). So `wait` can, and in the live run
did, return *before* the notification is injected. The orchestrator then asked `status`, got
nothing, and started digging. A perfect notification alone does not close this — the answer has to
be in the surface being asked.

**The seam already exists and is already subscribed.** `wait.rs` holds a
`broadcast::Receiver<CompletionEvent>` and, at
[`wait.rs:601-606`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/wait.rs),
**throws the received value away**:

```rust
outcome = receiver.recv() => {
    wake_closed = matches!(
        outcome,
        Err(tokio::sync::broadcast::error::RecvError::Closed)
    );
}
```

### 2a. `CompletionEvent` carries the rendered summary

**Where:** [`watch/observer.rs`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/watch/observer.rs)

```rust
pub struct CompletionEvent {
    pub run_id: RunId,
    pub outcome: ClassifiedOutcome,
    /// SCOPE_17 — the SAME per-child body [`format_completion_message`] renders, so a `wait` and
    /// the notification can never disagree about what a child said.
    ///
    /// # Why a rendered String and not the `ResultFile`
    ///
    /// This type's existing doc rules out carrying the whole [`crate::background::ResultFile`],
    /// and that reasoning is unchanged: a `broadcast` channel keeps every queued value alive for
    /// every receiver, and the result file holds the full per-step vector. A bounded, already
    /// rendered string is a fixed small cost and is the exact thing both consumers want — the
    /// alternative (re-reading the payload from `wait`) races the unlink this event precedes.
    ///
    /// Bounded to [`COMPLETION_EVENT_SUMMARY_MAX_BYTES`] through
    /// [`crate::exec::output::utf8_safe_prefix`], so `COMPLETION_BUS_CAPACITY` (64) events are a
    /// bounded worst case rather than an unbounded one.
    pub summary: String,
}

/// The bus copy's ceiling. Distinct from [`CHILD_OUTPUT_PREVIEW_MAX_BYTES`], which bounds ONE
/// child's structured fallback: this bounds the whole multi-child render that rides the broadcast
/// buffer. The notification itself is NOT bounded by this — it is delivered once, not buffered 64
/// times — so a long body still reaches the orchestrator in full through the notify.
const COMPLETION_EVENT_SUMMARY_MAX_BYTES: usize = 16 * 1024;
```

Populate it in the one publisher — `impl CompletionObserver for CompletionBus`
(`observer.rs:159-165`), which already receives the full `&CompletionNotification`:

```rust
async fn observe(&self, notification: &CompletionNotification) -> bool {
    self.publish(CompletionEvent {
        run_id: notification.result.run_id.clone(),
        outcome: classify_outcome(&notification.result),
        // SCOPE_17 — rendered HERE because this is the last point that still holds the parsed
        // payload: `deliver_pending_completions` unlinks it a few lines later.
        summary: bounded_completion_summary(&notification.result),
    });
    true
}
```

`bounded_completion_summary` is `result_display_summary` + `utf8_safe_prefix` — export
`result_display_summary` as `pub(crate)` from `watch/message.rs` (SUBTASK1) rather than duplicating
it, and reuse the SAME `pub(crate)` widening of [`crate::exec::output::utf8_safe_prefix`] SUBTASK1
already requires (do not widen it twice):

```rust
/// [`super::message::result_display_summary`] bounded to
/// [`COMPLETION_EVENT_SUMMARY_MAX_BYTES`] — the bus copy this run's [`CompletionEvent`] carries,
/// computed HERE because `observe` is the last point that still holds the parsed payload before
/// `deliver_pending_completions` unlinks it a few lines later.
fn bounded_completion_summary(result: &crate::background::ResultFile) -> String {
    crate::exec::output::utf8_safe_prefix(
        &super::message::result_display_summary(result),
        COMPLETION_EVENT_SUMMARY_MAX_BYTES,
    )
    .to_string()
}
```

### 2b. `wait` accumulates and reports them

**Where:** [`background/wait.rs`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/wait.rs)

Two declarations, added beside the existing `let mut wake = deps.completion_bus.as_ref().map(…)`
binding (`wait.rs:466-469`, immediately before `let mut active = active_runs(...).await?;` at
`:471`). `wait.rs` has no `std::collections` import today — add one, beside the existing
`use std::path::PathBuf;` / `use std::time::{Duration, Instant};` pair:

```rust
use std::collections::BTreeMap;
```

```rust
let mut observed_summaries: BTreeMap<String, String> = BTreeMap::new();
```

Then capture the value the wake arm currently discards (`wait.rs:601-606`, quoted in full at the
top of §2), keeping only events for runs this wait is actually tracking. This WRAPS the existing
`wake_closed = matches!(...)` statement rather than replacing it — the closed-detection is
untouched, only a new capture is added ahead of it:

```rust
outcome = receiver.recv() => {
    // SCOPE_17 — the wake arm already receives the completion; keeping its summary is what
    // lets a resolved wait ANSWER instead of merely reporting a count. Filtered to
    // `initial_ids` so a concurrent turn's run cannot inject its output into this wait's
    // report; `or_insert_with` (never `insert`) so a re-published completion cannot overwrite
    // the first observation.
    if let Ok(event) = &outcome
        && initial_ids.iter().any(|id| id == event.run_id.as_str())
    {
        observed_summaries
            .entry(event.run_id.as_str().to_string())
            .or_insert_with(|| event.summary.clone());
    }
    wake_closed = matches!(
        outcome,
        Err(tokio::sync::broadcast::error::RecvError::Closed)
    );
}
```

```rust
/// The per-child blocks for the runs this wait resolved, in `initial_ids` order.
///
/// Empty when the wait resolved through the poll rather than the bus (no bus wired, or the
/// completion was observed by another process). That is upstream's own no-bus degradation and
/// leaves today's text exactly as it is — the sentence only ever GAINS content.
fn format_observed_completions(initial_ids: &[String], observed: &BTreeMap<String, String>) -> String {
    let blocks: Vec<&str> = initial_ids
        .iter()
        .filter_map(|id| observed.get(id).map(String::as_str))
        .filter(|text| !text.trim().is_empty())
        .collect();
    if blocks.is_empty() {
        return String::new();
    }
    format!("\n\nResults:\n\n{}", blocks.join("\n\n"))
}
```

**Placement: the very END of `text`, in BOTH terminal branches, right before the final `Err`/`Ok`
split — not "before `resume_guidance`".** An earlier draft of this file said to splice this in
"immediately after `outcome` and before `resume_guidance`", which contradicted its own rendered
example below: `resume_guidance`/`attention_note`/`notification` are short clauses that read as one
continuing sentence, while this block is multi-line (`"\n\nResults:\n\n…"`) — splicing it into the
middle would leave the notification sentence dangling AFTER the numbers. Appending it last, once
the sentence is already complete, is both what the example shows and the only placement that reads
correctly. Concretely, in the `wait_for_all` branch (`wait.rs:693-701`):

```rust
        let text = format!(
            "Waited {elapsed} for {scope}; \
             {status}.{outcome}{resume_guidance}{attention_note} {notification}"
        );
        // SCOPE_17 — appended last, after the sentence is already complete; empty when the bus
        // produced nothing (no bus wired, or this wait resolved through the poll instead), so an
        // ordinary wait with no observed summaries is byte-identical to before.
        let text = format!("{text}{}", format_observed_completions(&initial_ids, &observed_summaries));
        // pi `:706-710` — the SAME text either way; the flags only flip the result's error bit,
        // which is what makes auto-drain throw instead of exiting quietly.
        return if (deps.fail_on_failed_runs && failed_count > 0)
            || (deps.fail_on_attention && !attention.is_empty())
        {
            Err(text)
        } else {
            Ok(text)
        };
```

and identically in the first-completion branch (`wait.rs:730-745`): one `let text = format!(…)`
line inserted between the existing `let text = format!(...)` binding and the trailing `if
(deps.fail_on_failed_runs …) { Err(text) } else { Ok(text) }` that closes the function.

`wait`'s result text becomes (three children sharing the agent name `delegate` — the SAME renderer
as SUBTASK1, so [`child_position`]'s `(i/n)` suffix applies here too):

```text
Waited 10.8s for 1 async run(s); done. Outcome: 1 complete. Completion events have been observed; inspect status if the notification is not visible yet.

Results:

delegate (1/3):
42

delegate (2/3):
17

delegate (3/3):
8
```

---

## §3 — `subagent status` is deliberately NOT in scope, and this is not deferral

`status` renders from `RunStatus` on disk.
[`StepStatus`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/records.rs) has
**no output field** — and neither does upstream's
[`AsyncStatus.steps[]`](../../../workspace/pi-subagents/src/shared/types.ts) (`:1863-1905`, checked
field by field). `telemetry.recent_output` is a bounded ring of streamed LINES, which is why the
live run saw acceptance-ledger tailings there rather than the answer.

Upstream surfaces a child's output through `inspect-rpc.ts`, which reads the payload — or, once the
payload is unlinked, the durable **completion replay + archive**
([`SCOPE_4.md`](SCOPE_4.md)). Making `status` answer therefore requires that record; it is not a
renderer change.

It is out of scope because **the acceptance test does not need it**: SUBTASK2 makes `wait` answer
at the exact moment the orchestrator asks, and SUBTASK1 makes the notification answer a beat later.
Both channels carry the same blocks from the same function. Adding a third that requires an
unported subsystem would be the decomposition trap, not scope discipline.

---

## Definition of done

**One criterion.** The command at the top produces a transcript in which three async subagents each
report their own random number, attributed to that agent, and the orchestrator sums them — with no
`bash`, no `python3`, and no hand-parsing of `events.jsonl` or `status.json`.

Supporting invariants, all readable off that same run:

* the notification body has exactly one `agent:` block per child, in child order;
* a child that produced nothing renders `agent:\n(no output)` rather than vanishing;
* a child whose answer is structured renders that structured JSON, pretty-printed, not `(no output)`;
* a child whose answer went to a file renders `Output saved to: <path>`, never `(no output)` —
  `(no output)` appears only when no text, no error, no structured value and no output file exist;
* **context efficiency**: every child body is its bounded SUMMARY — the contract block when the
  child emitted one, its output's tail when it did not — never the full output; a cut body says
  `[truncated]` and names the file holding the whole answer, so notification size is bounded by
  `children × 4 KiB` however chatty the children were;
* the summary contract reaches **every** child through `spawn_plan`'s persona composition — a
  foreground `subagent({agent, task})` and a detached fan-out member get the identical instruction,
  with no per-path wiring;
* an agent with an `output_schema` does NOT receive the prose contract (its `structured_output` is
  already the better summary), and its persona is byte-identical to today;
* a child that ignores the contract still yields a usable summary from the tail — verify by
  reading the fallback, not by making a child misbehave;
* each block is headed `agent (i/n):` for a multi-child run, so three same-named agents are
  distinguishable and a missing fourth is visible; a single-child run keeps today's bare `agent:`;
* `wait` returns the same blocks under a `Results:` heading when it resolved through the bus;
* a child that succeeded with a stale `error` still reports its output, not the error;
* one `NO_OUTPUT` constant inside `watch/message.rs` (this crate's three OTHER, unrelated
  `"(no output)"` literals — `extension/tool/routing.rs`, `extension/host/native_impl.rs`,
  `extension/executor/paths.rs` — are separate surfaces this task does not touch; see SUBTASK1),
  one boundary-walk helper, one summary renderer — no second copies of any of those three;
* `cargo clippy -p cyrup-ext-subagents --all-targets` exit 0 and the existing suites stay green
  (3258 lib + 198 `cyrup-it` subagents were green when the payload work landed).

---

## Research notes

* pi builds `summary` once at `subagent-runner.ts:4744` and carries it as a top-level `summary`
  string on the payload; cyrup has no `summary` field on `ResultFile` and re-derives it in
  `result_display_summary`. Re-deriving is fine — matching `:4744`'s OUTPUT is not optional.
* pi sends ONE notification per RUN (`sendCompletion`, `notify.ts:438-440`), never one per child.
  Per-agent attribution comes from the `{agent}:` blocks INSIDE the body — which is why SUBTASK1 is
  the whole feature and "send three messages" is the wrong shape.
* `notify.ts:214-238`'s `Child outputs:` block is gated on `workflowRunId`
  (`buildCompletionDetails:526`) and is a workflow-only surface. Do not port it here.
* The notification DOES reach the model: `coding_agent_convert_to_llm`
  (`cyrup-session-svc/src/hooks.rs:65-72`) renders `AgentMessage::Custom` through
  `custom_to_message`. `display: false` (pi `notify.ts:402`) controls DRAWING only. The delivery
  path — sink → `inject_message` → inject sink → `agent.steer` → `poll_steering`
  (`agent/run/turn.rs:205`) — is sound and must not be touched.
* `utf8_safe_prefix` (`exec/output.rs:1393`) is cyrup's existing port of pi's `truncateUtf8Head`
  and is currently private; widening it is the whole of that dependency.
* Faux-provider integration tests passed on the broken behaviour — they assert payload structs, and
  the defect was three layers above them in the renderer. Trust the real run.
* `rg -i resultsummary` also turns up
  [`WorkflowResultSummary`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/workflow_graph.rs)
  (`background/workflow_graph.rs:244`, re-exported at `background::mod.rs:110`) — an unrelated
  per-node workflow-graph type in a different module (`background::workflow_graph`, not
  `exec::result_summary`) with a different name. Noted only so a search during implementation is
  not mistaken for a collision with this task's [`ResultSummary`].
* `StepStatus` (§3) has no `output`/`result`/`summary` field of any kind
  (`background/records.rs:25-130`) — confirmed against current source, not inherited from an older
  pass — which is what makes "`status` is out of scope" true today, not just at some earlier commit.

---

/home/d0m17bw/.flux/-home-d0m17bw-workspace-cyrup/todo/SCOPE_17.md
