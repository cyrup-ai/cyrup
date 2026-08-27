---
stage: new
status: done
updated: 2026-08-27 05:30
severity: high
effort: medium
subsystem: fork context / thinking level
source: docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md
item: SUBA-075
---

> **Path note.** This task lives in a subdirectory. Every flux command globs a single level (`ls -1 "$FLUX_BASE/todo/"*.md`), so `/exec`, `/aug` and `/qa` will not list it — pass the absolute path explicitly.

# SUBA-075 — Forked child sessions are not sanitized: signed and redacted Anthropic thinking blocks are inherited verbatim and no thinking-off override is applied to the branch

**Kind** not-ported · **Severity** high · **Effort** M · **Confidence** confirmed
**Subsystem** fork context / thinking level
**Window** in-baseline (≤ v0.43.0) — all three functions exist at `v0.43.0:src/shared/fork-context.ts`.

**upstream** — `git show v0.57.0:src/shared/fork-context.ts`: **`:106`**
`forkedChildRequiresThinkingOff(model, availableModels, preferredProvider)` — true for an unknown
model, a model whose `provider` is `anthropic`, or whose `api` is `anthropic-messages`; **`:118`**
`isUnsafeAnthropicThinkingBlock` (true for `redacted_thinking`, and for `thinking` blocks carrying
`redacted: true` or a non-empty `thinkingSignature`/`signature` on an Anthropic provider/api/model);
**`:140`** `appendThinkingOffEntry` appends a `{type:"thinking_level_change", thinkingLevel:"off"}`
entry to the branched session; **`:153`** `sanitizeUnsafeThinkingBlocks` strips those blocks from
every assistant entry; **`:189`** `createForkContextResolver` rewrites the branched session file with
the sanitized entries (default `forceThinkingOffForIndex` true) and returns `thinkingOverride: "off"`.
`subagent-executor.ts` builds `forkThinkingRequirements` per child index; the resulting override
becomes `options.thinkingOverride`, which `execution.ts` feeds to
`applyThinkingSuffix(model, thinking, /*replaceExisting=*/ options.thinkingOverride !== undefined)` —
and with `replaceExisting` true an existing `:<level>` suffix is **REPLACED**.

**cyrup** — `crates/cyrup-ext-subagents/src/fork_context.rs` is **529 lines** and
`grep -ci 'thinking' crates/cyrup-ext-subagents/src/fork_context.rs` returns **0**: no forced
thinking-off, no `forceThinkingOffForIndex` analogue, no branch sanitization. Crate-wide,
`grep -rniE 'redacted_thinking|sanitize_unsafe|thinking_off|requires_thinking_off|replace_existing' --include=*.rs`
returns exactly **one** hit and it is an unrelated test name
(`discovery/frontmatter.rs:1363 thinking_off_is_preserved_as_explicit_off_distinct_from_unset`).
`ForkContextResolver::resolve` (`fork_context.rs:140-208`) branches via `create_branched_session` and
hands the path straight back with no post-write pass. `src/exec/spawn_plan.rs:124-139`
`apply_thinking_suffix(model, thinking)` takes **no** `replace_existing` parameter and returns the
model UNCHANGED when it already carries a recognized suffix (`:133-137`); its only call site
(`spawn_plan.rs:323`) passes `agent.thinking` alone.

**Impact** — A `context: "fork"` subagent branching a parent session that contains signed or redacted
Anthropic thinking blocks is launched with those blocks intact and with thinking still enabled.
Upstream forces the branch to thinking-off and strips the blocks precisely because the Anthropic
messages API rejects thinking blocks whose signatures do not match the new request context — so the
cyrup child fails at the provider on a normal fork path against an Anthropic model. The missing
`replace_existing` compounds it: even if a thinking-off override existed, a model id already carrying
`:high` would keep `:high`. `high` not `critical`: the failure surfaces as a provider rejection turned
into a subagent error result, and it needs the non-default `context: "fork"`, an Anthropic-family
child model, and a parent transcript that actually carries such blocks.

**Fix** — Port the three functions into `fork_context.rs`, run `sanitize_unsafe_thinking_blocks` +
`append_thinking_off_entry` over the branched session before `ForkContextResolver::resolve` returns,
return a `thinking_override` on the resolution, and add the `replace_existing` arm to
`apply_thinking_suffix` (`exec/spawn_plan.rs:124`) so the override replaces an existing `:<level>`
suffix rather than deferring to it.

**Verify** — Fork a parent transcript containing one `redacted_thinking` block and one signed
`thinking` block against an Anthropic child model: the branched session file must contain neither
block and must end with a `thinking_level_change → off` entry, and the child must spawn with the
model id's `:high` suffix replaced by `:off`.

**Relation to corpus** — New. Area 09 has no fork-context row at all. Minor citation note for
whoever works it: the port's `apply_thinking_suffix` doc cites `pi-args.ts:186-200`, which is the
v0.43.0 range; at v0.57.0 `applyThinkingSuffix` has moved.

---

## Scope

In scope: the behaviour described above, in `crates/cyrup-ext-subagents/`.

Out of scope: the other SUBA items in this batch (each has its own file); any refactor beyond what the fix needs; and the ledger corrections in `SUBA-CORPUS-HEALTH.md`.

Full finding, with the complete evidence chain: [`09a-cyrup-ext-subagents-v0.57-drift.md` § SUBA-075](../../../docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md)

## Approach

Sanitize the forked branch before it is handed to the child: strip signed and redacted Anthropic
thinking blocks from the inherited transcript, and apply the thinking-off override upstream applies
to a forked branch.

## Acceptance Criteria

- [ ] A fork whose parent transcript contains signed/redacted thinking blocks yields a child branch with none
- [ ] The thinking-off override is applied to the forked branch
- [ ] A regression test covers both
- [ ] `cargo test -p cyrup-ext-subagents` passes
