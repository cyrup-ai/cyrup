---
title: Built-in tools never declare constrainedSampling
priority: LOW
tool: read/bash/edit/write
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: qa
status: needs-rework
updated: 2026-08-27 14:08
---

# Outstanding: DoD 8 — one stale doc-comment still denies that built-ins declare it

QA rating **9/10**. The implementation is otherwise complete and faithful: the strict-schema
machinery, the null-stripping validation stage, all six adapter emission sites and the four
declarations all landed and were verified by reading. DoD clauses 1–7 hold. **DoD clause 8 does
not.**

> 8. No doc-comment anywhere in the workspace still asserts that no pi built-in declares
>    `constrainedSampling`.

Note that §6 of the previous revision of this task ("Nothing in `cyrup-ext/src/wrapper.rs` …
changes") was **wrong** and contradicted its own DoD 8. Fix the doc, not the DoD.

---

## 1. Required — `crates/cyrup-ext/src/wrapper.rs:113-125`

The doc-comment on `RegisteredTool::constrained_sampling` (or whichever wrapper type owns the
override at that offset) currently reads, at **lines 118-121**:

```rust
    /// Rust has no spread: each surface method must be delegated by hand, and this one was the
    /// method the hand-written list missed. Extension-registered and WASM-guest tools are the ONLY
    /// tools that can declare `constrainedSampling` (no pi built-in does), and every one of them
    /// reaches the loop through this wrapper — so without this override the whole opt-in path was
    /// dead on arrival: `WasmTool::constrained_sampling` read the guest's declaration off the
    /// descriptor and this wrapper dropped it one frame later, silently.
```

Two false claims in one sentence, both now contradicted by code in this same workspace:

* **"no pi built-in does"** — `read.ts:222`, `bash.ts:354` (`createShellToolDefinition`, so
  `powershell` inherits), `edit.ts:329`, `write.ts:200` all declare it as of pi `7915cdac`
  (first tagged v0.84.2), and `crates/cyrup-tools/src/tools/{read,bash,edit,write}.rs` now mirror
  that. This is verbatim the assertion DoD 8 names.
* **"the ONLY tools that can declare `constrainedSampling`"** — strictly stronger and equally
  false; any `impl Tool` may override the accessor, and four built-ins do.

Rewrite the sentence so the *rationale for the override* survives (it is a good rationale and must
not be lost) while the factual claim is corrected. Something in this shape:

```rust
    /// Rust has no spread: each surface method must be delegated by hand, and this one was the
    /// method the hand-written list missed. Extension-registered and WASM-guest tools reach the
    /// loop ONLY through this wrapper, so without this override their opt-in path was dead on
    /// arrival: `WasmTool::constrained_sampling` read the guest's declaration off the descriptor
    /// and this wrapper dropped it one frame later, silently. (Built-ins do not pass through here
    /// — since pi `7915cdac` @v0.84.2 the four coding built-ins declare it directly; see
    /// `cyrup_core::constrained_sampling::experimental_tool_sampling`.)
```

Wording is yours; the constraints are: no surviving claim that built-ins do not declare the field,
no surviving "ONLY tools that can declare", and the dead-on-arrival rationale preserved.

## 2. Minor, fix while you are there — `crates/cyrup-core/src/constrained_sampling.rs:4-6`

The module header still points at the provider port with the pre-bump tag:

```rust
//! The resolvers that consume them live provider-side in
//! `cyrup-provider/src/utils/constrained_sampling.rs` (a port of pi
//! `packages/ai/src/api/constrained-sampling.ts` @v0.83.0).
```

That file's own header was correctly bumped to `@v0.84.2` in this change; this cross-reference was
not. Change `@v0.83.0` → `@v0.84.2` so the two agree.

(The `@v0.83.0`-tagged citations at `constrained_sampling.rs:10`, `:22`, `:34-35` and
`tool.rs:142` are **fine as-is** — each is explicitly qualified to the tag it was true at, and
`:22` is immediately followed by the v0.84.2 correction. Do not churn them.)

---

## Verification after the fix

```
rg -n 'no pi built-in|no built-in does|ONLY tools that can declare' crates/ --glob '*.rs'
```

must return only `crates/cyrup-core/src/constrained_sampling.rs:22`, whose sentence begins
"At v0.83.0 no pi built-in declared the field" and is correct.

No source behaviour changes; no test changes expected.

---

## Not outstanding — verified passing, do not touch

For the next reviewer's benefit, these were each read and confirmed:

| DoD | Status |
| --- | --- |
| 1 — flag unset ⇒ `None`, requests byte-identical | PASS (`constrained_sampling.rs` tests `a_tool_that_did_not_opt_in_is_never_converted`, `arguments_without_nulls_are_unchanged_by_the_new_stage`, `openai_completions.rs` DoD-1 arm) |
| 2 — either flag ⇒ `prefer`; `grep`/`find`/`ls` still `None` | PASS (`experimental_tool_sampling_reads_both_flags_and_nothing_else`; only the four tool files contain `constrained_sampling`) |
| 3 — declaration survives the agent loop | PASS (`cyrup-agent/src/agent/run/stream.rs:94`) |
| 4 — strict route ⇒ `strict: true` + converted schema | PASS (all six adapters serialize `json_schema_tool_parameters`; `strict_conversion_requires_every_key_and_makes_optionals_nullable`, `strict_conversion_recurses_through_array_items`) |
| 5 — non-strict route degrades silently, raw schema | PASS (`a_non_strict_route_keeps_the_raw_schema_and_does_not_fail`) |
| 6 — optional `null` executes as ABSENT, never `0` | PASS (`validate.rs` stage 0 **deletes** the key; `an_optional_null_is_deleted_rather_than_coerced_to_zero`) |
| 7 — `require` fails with pi's message shape | PASS (`an_unconvertible_schema_degrades_under_prefer_and_fails_under_require`) |
| 8 — no stale doc-comment | **FAIL** — §1 above |

`bash.rs` hosting the declaration on the shared `ShellTool` engine (so `powershell` inherits) is
**correct**, not a miss: pi puts it on `createShellToolDefinition` at `bash.ts:354`, verified in the
vendored checkout.
