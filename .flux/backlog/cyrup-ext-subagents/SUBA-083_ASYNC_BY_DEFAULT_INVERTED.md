---
stage: new
status: done
updated: 2026-08-27 05:30
severity: high
effort: small
subsystem: config / launch mode
source: docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md
item: SUBA-083
---

> **Path note.** This task lives in a subdirectory. Every flux command globs a single level (`ls -1 "$FLUX_BASE/todo/"*.md`), so `/exec`, `/aug` and `/qa` will not list it — pass the absolute path explicitly.

# SUBA-083 — `asyncByDefault`'s default is inverted, and the documented `asyncByDefault:false` opt-out is a no-op

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** confirmed
**Subsystem** config / launch mode
**Window** in-baseline (≤ v0.43.0) — identical at `v0.43.0:src/extension/config.ts`.

**upstream** — `git show v0.57.0:src/extension/config.ts` **`:222-224`**:
```ts
export function resolveAsyncByDefault(config: Pick<ExtensionConfig, "asyncByDefault">): boolean {
	return config.asyncByDefault !== false;
}
```
— **an ABSENT key means TRUE.** `git show v0.57.0:src/extension/index.ts:9` states the contract in
the module header — *"Toggle: async parameter (default: true; set `asyncByDefault:false` in
config.json to opt out)"* — and the boolean is threaded into every launch surface:
`subagent-executor.ts`'s `const requestedAsync = params.async ?? asyncByDefault;`, the fanout-child
path and the slash bridge. `git show v0.57.0:src/extension/schemas.ts:324` repeats it in the `async`
param's own description: *"Run in background unless `asyncByDefault:false`."*

**cyrup** — `crates/cyrup-ext-subagents/src/registration/mod.rs:272` `async_by_default: false` inside
`impl Default for SubagentExtensionConfig`, pinned by the test at `:1016`
(`assert!(!cfg.async_by_default)`). `src/extension/tool/params.rs:337`
`let requested_async = async_param.unwrap_or(cfg.async_by_default);` — the same `??` shape with the
opposite tier-5 default. The field's doc comment (`registration/mod.rs:80-82`) describes the semantics
without noting the flip, and `grep -c 'CYRUP-DELTA' crates/cyrup-ext-subagents/src/registration/mod.rs`
→ **0**, so this is **not** a marked divergence. (Contrast the sibling field two lines down,
`max_subagent_spawns_per_session: 40`, whose doc cites `func-SA §4.7` as a cyrup requirement — that
one is a decision of record; this one is not.)

**Impact** — On a stock install with no `config.json`, `subagent({agent, task})` returns immediately
with an async run id upstream (the caller then waits or polls) and **blocks the parent turn until the
child finishes** in the port. Every launch surface — the tool, `/run`, fan-out children — takes the
opposite mode by default, and the `asyncByDefault: false` opt-out documented in upstream's own header
is inert in the port because the port already behaves that way. **Correction to the filing text,
applied:** the claim that "a user following upstream documentation cannot reach upstream's default
behaviour at all" is **false** — setting `asyncByDefault: true` does work in the port, proven by
`src/extension/tool/params.rs:532-544` (which deserializes the camelCase key from a real config value
and asserts an omitted `async` then backgrounds) and by
`crates/cyrup-it/tests/subagents/registration_commands_integration.rs:496-505`. The key is honoured in
both directions; only the absent-key default is inverted, which is what makes the documented
`false` opt-out a no-op. `high` not `critical`: no data loss, no wrong output, no bypass, no crash.

**Fix** — Either flip `registration/mod.rs:272` to `true` (and its pinning test at `:1016`), matching
`resolveAsyncByDefault`'s `!== false` semantics exactly — or, if the foreground default is an
intentional product decision, **write it down**: a `[CYRUP-DELTA]` block at the field naming the
divergence and its rationale, as the sibling `max_subagent_spawns_per_session` field does. What is not
acceptable is the current state: a silent flip of a documented default, with a doc comment that does
not mention it.

**Verify** — With no config file present, `subagent({agent, task})` must return an async run id
without blocking; with `asyncByDefault: false`, the same call must block until completion. Both
directions must be covered by the pinning test.

**Relation to corpus** — New. Not covered by `SUBA-061`'s four ignored keys — the key here **is**
honoured; its default is inverted.

---

## Scope

In scope: the behaviour described above, in `crates/cyrup-ext-subagents/`.

Out of scope: the other SUBA items in this batch (each has its own file); any refactor beyond what the fix needs; and the ledger corrections in `SUBA-CORPUS-HEALTH.md`.

Full finding, with the complete evidence chain: [`09a-cyrup-ext-subagents-v0.57-drift.md` § SUBA-083](../../../docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md)

## Approach

Invert the default to match upstream: an absent `asyncByDefault` backgrounds the launch. Make the
documented `asyncByDefault:false` opt-out actually force foreground.

## Acceptance Criteria

- [ ] An absent `asyncByDefault` backgrounds the launch
- [ ] `asyncByDefault:false` forces foreground and is no longer a no-op
- [ ] A test pins both directions
- [ ] `cargo test -p cyrup-ext-subagents` passes
