---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/tools/bash.rs:214"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: aug
status: done
updated: 2026-08-28 02:10
---

# Capability gap: `crates/cyrup-tools/src/tools/bash.rs:214`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

pi @e8682309 bash.ts:49 reads `"You can inspect PI_* ..."` — the same wording cyrup ships. At the nominal ported baseline v0.83.0 (bash.ts:330) it read the bare imperative `"Inspect PI_* environment variables ..."`.

## What cyrup does

Ships the later (v0.84.x) wording.

## What a caller sees

Against the audit reference commit e8682309 there is NO observable difference — cyrup and the reference tree agree. Listed as a gap rather than folded into the mechanism count so it stays visible: it is a model-facing prompt string that is knowingly ahead of the tag the rest of the port targets, i.e. the project is running a mixed baseline. Your call whether that is acceptable; it is not a mechanism detail.

## Decision required

One of:

1. **Close it** — bring cyrup to pi's behaviour.
2. **Accept it** — David explicitly accepts the divergence; the marker stays and is
   annotated as authorized, with the reason.
3. **Reshape it** — the divergence is right but the current form is wrong.

Do not silently keep option 2 by leaving the marker as-is; that is how this became a
backlog in the first place.

## Definition of done

1. The gap is closed, or the marker records an explicit authorized acceptance.
2. If closed, a test fails without the change.
3. No behaviour regression in the owning crate.

---

# AUGMENTATION (stage: aug — 2026-08-28)

## Verdict up front

**This is NOT a capability gap, and it is not a divergence of any kind against the audit
reference.** The wording axis this marker describes is byte-identical between cyrup and pi
@`e8682309`. The marker is a *stale historical note wearing a `CYRUP-DELTA` tag*, and because
`CYRUP-DELTA` is the grep every parity sweep runs, it inflates the gap count on every future
audit. That is the actual defect, and it is fixable.

Disposition: **option 3, reshape** — retire the tag, keep the history as a plain note, and
co-edit the test that currently pins the tag's existence. This is a *finding*, not a descope:
nothing is being declared out of scope, a concrete source change is prescribed below, and it is
pinned by a test that fails without it.

**Option 1 ("close it — bring cyrup to pi's behaviour") is actively harmful here.** cyrup already
*is* at pi's behaviour. "Closing" would mean reverting to the v0.83.0 imperative, which would
manufacture a divergence from the reference tree where none exists today, and would red two
existing tests. Do not take option 1.

## What the marker actually claims

Anchor by symbol: the doc comment on
`cyrup_tools::tools::bash::ShellTool::prompt_guidelines`
(`/home/user/cyrup/crates/cyrup-tools/src/tools/bash.rs`, the block between `fn prompt_snippet(`
and `fn prompt_guidelines(`). That block carries **two** independent `CYRUP-DELTA` tags:

| tag | subject | filed as |
| --- | --- | --- |
| `[CYRUP-DELTA — version lag, AHEAD of the ported tag; wording only]` | the `"You can inspect …"` prefix vs. v0.83.0's bare `"Inspect …"` | **this task (`:214`)** |
| `[CYRUP-DELTA — deliberate, value only; the variable-family name inside the string]` | `PI_*` → `CYRUP_*` | sibling task `:236` — out of scope here |

Line 214 falls inside the first block, so this task is about the **wording prefix only**. The
`PI_*` → `CYRUP_*` rename is the sibling task's subject and is deliberately not re-litigated here.

The claim, in one sentence: *cyrup emits a model-facing system-prompt guideline that the ported
baseline tag (v0.83.0) never shipped.*

## Verification against `./tmp/pi` @ `e8682309`

Reference tree identity: `tmp/pi/packages/coding-agent/package.json` line 3 is
`"version": "0.84.3"`. So the audit reference is a v0.84.x tree, one patch ahead of the v0.84.1
the doc comment cites.

**Upstream bytes** — `packages/coding-agent/src/core/tools/bash.ts:47-50`:

```ts
export const bashToolSystemPromptContribution = {
	snippet: "Execute bash commands (ls, grep, find, etc.)",
	guidelines: ["You can inspect PI_* environment variables for current model and session details."],
} as const;
```

and the byte-identical twin at `packages/coding-agent/src/core/tools/powershell.ts:18-21`:

```ts
export const powershellToolSystemPromptContribution = {
	snippet: "Execute PowerShell commands",
	guidelines: ["You can inspect PI_* environment variables for current model and session details."],
} as const;
```

**cyrup bytes** — `crates/cyrup-tools/src/tools/bash.rs` `BASH_CONFIG`:

```rust
prompt_guidelines: &[
    "You can inspect CYRUP_* environment variables for current model and session details.",
],
```

and `crates/cyrup-tools/src/tools/powershell.rs` `POWERSHELL_CONFIG`, same string.

**Byte diff, upstream vs cyrup:** exactly one token, `PI_*` → `CYRUP_*`. Everything else —
the softening prefix `"You can inspect "` and the tail
`" environment variables for current model and session details."` — is identical.

So on the axis this marker is about (**wording**), the delta at the audit reference is **zero
bytes**. The single surviving byte-delta belongs to the sibling marker at `:236`.

### Evidence limit, stated plainly

The marker's only asserted divergence is against **v0.83.0**, and this host carries exactly one
pi checkout (`tmp/pi`, at `e8682309`); `git` is off-limits by constraint. So the claim
"v0.83.0 `bash.ts:330` read `\"Inspect PI_* environment variables …\"`" is **unverifiable here** —
it is neither confirmed nor refuted by this pass. That does not change the disposition: whatever
v0.83.0 said, cyrup matches the reference tree today, and the reference tree is what the audit
measures against.

### Ancillary claims in the same block — all verified TRUE at e8682309

* *"`exposeSessionEnvironment` gate is unchanged"* — confirmed. `bash.ts:345`
  `const exposeSessionEnvironment = options?.exposeSessionEnvironment ?? true;` and `bash.ts:352`
  `promptGuidelines: exposeSessionEnvironment && config.promptGuidelines ? [...config.promptGuidelines] : undefined,`.
  cyrup's `ShellTool::prompt_guidelines` mirrors it: guideline vec when the flag is on, empty when
  off. Default-true on both sides.
* *"the `snippet` string is byte-identical"* — confirmed against the reference:
  `"Execute bash commands (ls, grep, find, etc.)"` matches `BASH_CONFIG::prompt_snippet`.
  (Byte-identity *across the two tags* is the unverifiable half; identity *against the reference*
  is verified.)
* *"the dedup in the prompt builder emits it once when both tools are selected"* — confirmed on
  **both** sides. Upstream `packages/coding-agent/src/core/system-prompt.ts:88-95` guards
  `addGuideline` with a `guidelinesSet: Set<string>`; cyrup does insertion-order dedup via
  `push_guideline(out, &mut seen, …)` in `cyrup-session/src/prompt/builder.rs` (steps 3a/3b/3c).
  Note upstream's `create-harness.ts:70` `flatMap` does **not** dedup — the dedup is downstream in
  the prompt builder, which is where cyrup also does it. Equivalent.
* *`undefined` vs empty-vec* — upstream returns `undefined` when the flag is off; cyrup returns
  `Vec::new()`. Indistinguishable downstream: both consumers coalesce
  (`create-harness.ts:70` `tool.promptGuidelines ?? []`, `system-prompt.ts:115`
  `for (const guideline of promptGuidelines ?? [])`). **Not a gap.**

## Secondary findings — citation defects in the same doc block

These are real (the doc claims to quote upstream and does not), but none is caller-visible. Fold
them into the same edit rather than filing them separately.

1. **Not-verbatim quote.** The block is introduced as
   `Pi v0.84.1 coding-agent/src/core/tools/bash.ts:45-48,334:` followed by a ```` ```text ```` block
   whose last line reads
   `promptGuidelines: exposeSessionEnvironment ? [...bashToolSystemPromptContribution.guidelines] : undefined,`.
   **No such line exists at e8682309.** The real line (`bash.ts:352`) is
   `promptGuidelines: exposeSessionEnvironment && config.promptGuidelines ? [...config.promptGuidelines] : undefined,`,
   and the contribution reaches it indirectly via `bashToolConfig.promptGuidelines` at
   `bash.ts:525`. The quote conflates two call sites and silently drops the
   `&& config.promptGuidelines` presence guard.
2. **Line drift, contribution const.** Cited `bash.ts:45-48` / `bash.ts:47`; at e8682309 the const
   is `:47-50` and the guideline literal is `:49`.
3. **Line drift, the gate.** Cited `bash.ts:334` twice as "the `exposeSessionEnvironment` gate";
   at e8682309 `:334` is `promptGuidelines?: readonly string[];` inside `interface ShellToolConfig`.
   The gate is `:345` (default) and `:352` (application).
4. **Line drift, the twin.** "`powershell.ts:20`" is correct; "bash's (bash.ts:48)" is `:49`.
5. **Wrong path.** "`tests/pi_schema.rs` pins it" — the file is
   `crates/cyrup-tools/src/tests/pi_schema.rs`.

## Precedent this follows

`docs/PARITY-PLAN.md:37` records the port as baseline **v0.83.0**, upstream **v0.84.1**,
`frozen; drift absorbed per-batch`. `docs/PARITY-PLAN.md:1338-1339` supplies the governing
precedent verbatim: *"pi **adopted cyrup's** recursive settings merge at v0.84.1 — so fixing
toward v0.83.0 would be a regression, which is why `CFG-012` is *superseded*, not open."*

Identical shape, mirrored: cyrup adopted pi's v0.84.0 wording early; upstream then shipped it.
The lag closed itself. The correct status word is **superseded**, and a superseded item does not
carry a live `CYRUP-DELTA`.

## PRESCRIBED CHANGE (this is the deliverable — not a shrug)

Two edits, which must land together.

### Edit 1 — `crates/cyrup-tools/src/tools/bash.rs`, doc comment on `ShellTool::prompt_guidelines`

Replace the whole first tagged paragraph (the `[CYRUP-DELTA — version lag, AHEAD of the ported
tag; wording only]` paragraph **and** the `VERSION LAG, not a port bug:` paragraph that restates
it) with an untagged note. Suggested text:

```rust
/// **Version-lag note — SUPERSEDED, not a divergence.** The string below opens
/// `"You can inspect …"`. At the parity reference `e8682309` that prefix is byte-identical to
/// upstream's (`coding-agent/src/core/tools/bash.ts:49`; the twin at `powershell.ts:20` is the
/// same sentence), so on the WORDING axis cyrup and pi agree exactly and there is nothing here
/// for a parity sweep to close. History: this once carried a `CYRUP-DELTA` because cyrup adopted
/// v0.84.0's softening ahead of the nominal v0.83.0 baseline, where `bash.ts:330` read the bare
/// imperative `"Inspect PI_* environment variables for current model and session details."`.
/// Upstream has since shipped the softened form at the reference tag, so reverting toward v0.83.0
/// would now CREATE a divergence rather than remove one — same disposition as `CFG-012` in
/// `docs/PARITY-PLAN.md`: superseded, not open. The tag is deliberately GONE so this stops being
/// counted as an accepted divergence by the `CYRUP-DELTA` grep. The only live divergence in this
/// string is the `CYRUP_*` variable family, tagged separately below.
///
/// The `expose_session_environment` gate is upstream's, unchanged: `bash.ts:345` defaults it to
/// `true`, `bash.ts:352` applies it
/// (`exposeSessionEnvironment && config.promptGuidelines ? [...config.promptGuidelines] : undefined`),
/// and the contribution reaches that site through `bashToolConfig.promptGuidelines`
/// (`bash.ts:525`). cyrup returns an empty `Vec` where upstream returns `undefined`; both
/// consumers coalesce (`create-harness.ts:70`, `system-prompt.ts:115`), so the two are
/// indistinguishable.
```

While in the block, fix the five citation defects listed above — in particular replace the
non-verbatim ```` ```text ```` quote with the actual `bash.ts:352` + `bash.ts:525` bytes, and
re-anchor `:45-48`/`:334`/`:48` to `:47-50`/`:345`+`:352`/`:49`.

Leave the second `CYRUP-DELTA` (the `PI_*` → `CYRUP_*` one) **completely untouched** — it is task
`:236`'s subject and is a genuinely live, caller-visible divergence.

### Edit 2 — `crates/cyrup-tools/src/tests/pi_tool_semantics.rs`, `bash_prompt_guideline_deltas_are_tagged_cyrup_delta` (TOOL-043)

**Edit 1 alone will red this test.** It currently hard-pins the marker's existence and its exact
citations:

```rust
assert_eq!(tags.len(), 2, "TOOL-043: the guideline carries TWO independent divergences …");
assert!(doc.contains("[CYRUP-DELTA — version lag, AHEAD of the ported tag; wording only]"), …);
assert!(doc.contains("bash.ts:47"), "the wording delta must cite v0.84.1 bash.ts:47");
assert!(doc.contains("bash.ts:330"), "the wording delta must cite v0.83.0 bash.ts:330");
```

Required changes:

* `tags.len()` expectation `2` → `1`, message rewritten: the guideline carries **one** live
  divergence (`PI_*` → `CYRUP_*`); the wording lag is superseded and must NOT be tagged.
* Delete the `[CYRUP-DELTA — version lag, …]` assertion and **invert** it — assert the doc block
  does *not* contain that tag, so a future agent cannot silently re-add it and re-inflate the
  count. This is the assertion that makes the change test-pinned (DoD #2): it is RED before
  Edit 1 and GREEN after.
* Delete the `bash.ts:47` assertion; replace with `bash.ts:49` (the reference-tag line) so the
  citation stays anchored to something that exists in `tmp/pi`.
* Keep the `bash.ts:330` mention only as prose inside the note — drop it as a test assertion, or
  keep it and retarget the message to "the superseded history must stay recorded". Either is
  defensible; the count assertion is the load-bearing one.
* Update the test's own TOOL-043 header comment, which still says "diverges from the ported tag
  TWICE".

### Not affected — leave alone

* `crates/cyrup-tools/src/tests/bash_session_env.rs::the_guideline_uses_pi_v0_84_1_softened_phrasing`
  — still correct and still valuable: it pins the softened prefix and explicitly rejects a
  regression to the imperative, which is exactly the guard this reshape relies on. Optionally
  re-anchor its `bash.ts:47` citation to `bash.ts:49`.
* `crates/cyrup-tools/src/tests/bash_session_env.rs::the_prompt_guideline_tracks_the_exposure_flag`
  — unaffected.
* `crates/cyrup-tools/src/tests/pi_schema.rs` `PI_BASH_GUIDELINES` / `PI_POWERSHELL_GUIDELINES`
  — values unchanged; optionally re-anchor the `bash.ts:47` citation in the comment at `:137`.
* **No runtime behaviour changes.** No `BASH_CONFIG` / `POWERSHELL_CONFIG` string is edited, so
  the emitted system prompt is byte-for-byte what it is today.

## Ledger / audit-count consequence

`INDEX.md` lists `crates/cyrup-tools/src/tools/bash.rs:214` as one of the **11** in-scope
capability gaps. On this evidence it is **not one**. After the reshape lands, the in-scope count
should read **10**, and the INDEX row for `:214` should be struck with the reason ("superseded at
`e8682309`; marker retired to an untagged note"). Note the anchors for `:236` and `:312` shift when
Edit 1 changes the line count of this doc block — re-derive those two line numbers rather than
trusting the filenames.

## Definition of done (revised for this task)

1. The `[CYRUP-DELTA — version lag, AHEAD of the ported tag; wording only]` tag is **gone** from
   `ShellTool::prompt_guidelines`, replaced by the untagged superseded-note above; the five
   citation defects in the same block are corrected against `e8682309`.
2. `bash_prompt_guideline_deltas_are_tagged_cyrup_delta` expects **one** tag and asserts the
   version-lag tag is **absent** — RED before Edit 1, GREEN after.
3. `the_guideline_uses_pi_v0_84_1_softened_phrasing` and the `pi_schema.rs` guideline pins still
   pass unchanged; no emitted-string change anywhere.
4. `INDEX.md` in-scope capability-gap count drops 11 → 10 with `:214` struck and the reason given.
5. Escalation, if any: the only thing David actually needs to rule on is whether "cyrup ahead of
   the v0.83.0 baseline on a string upstream has since adopted" is a *policy* concern worth
   tracking somewhere other than a `CYRUP-DELTA`. The parity plan already answers it for `CFG-012`
   (superseded), so the default is "no separate ruling needed".
