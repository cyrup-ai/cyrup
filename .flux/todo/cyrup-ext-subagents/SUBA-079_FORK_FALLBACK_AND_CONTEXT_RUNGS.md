---
stage: new
status: done
updated: 2026-08-27 05:30
severity: high
effort: small
subsystem: fork context / launch policy
source: docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md
item: SUBA-079
---

> **Path note.** This task lives in a subdirectory. Every flux command globs a single level (`ls -1 "$FLUX_BASE/todo/"*.md`), so `/exec`, `/aug` and `/qa` will not list it — pass the absolute path explicitly.

# SUBA-079 — An agent's `defaultContext: fork` hard-fails the launch when the parent session is not yet persisted, where upstream falls back to fresh — plus no config `defaultSubagentContext` rung and no `context: "profile"`

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** confirmed
**Subsystem** fork context / launch context policy
**Window** v0.47.1..v0.57.0.

**upstream** — `git show v0.57.0:src/shared/fork-context.ts` **`:80-84`**
`resolveSubagentLaunchContext`:
```ts
if (input.explicitContext !== undefined) return input.explicitContext;
const preferredContext = input.defaultSubagentContext ?? input.agentDefaultContext ?? "fresh";
return preferredContext === "fork" && input.canUseImplicitFork ? "fork" : "fresh";
```
with the comment at **`:86-87`** *"Explicit `context: "fork"` stays strict and does not use this
preference"*, `canPreferFork` at **`:88`** and `canPreferForkFromSnapshot` at **`:95`** returning
false when there is no persisted parent session file or no leaf id. **The config rung OUTRANKS the
agent's own default.** `git show v0.57.0:src/extension/config.ts:140-142` refuses any
`defaultSubagentContext` other than `"fresh"`/`"fork"` with
`config.defaultSubagentContext must be "fresh" or "fork"`.
`git show v0.57.0:src/runs/foreground/subagent-executor.ts` **`:2521`**
`resolveAgentDefaultContextPolicy` adds the `params.context === "profile"` branch, which REQUIRES
every requested agent to declare `defaultContext` (**`:2532`**/**`:2537`**
``context: "profile" requires agent '<n>' to declare defaultContext.``) and ignores the config
default. `git show v0.57.0:src/extension/schemas.ts:319-322` declares
`enum: ["fresh", "fork", "profile"]`; the same enum is `["fresh","fork"]` at both v0.43.0 and v0.47.1.

**cyrup** — `crates/cyrup-ext-subagents/src/fork_context.rs:74-88` — the doc enumerates exactly three
rungs (*"1. `call_site_context` 2. `agent_default_context` 3. `ContextMode::default` (Fresh)"*) and
the body is `call_site_context.or(agent_default_context).unwrap_or_default()`: **no availability test
and no distinction between an explicit and an inherited `Fork`.** `resolve` (`:140-208`) then returns
`Err(SubagentError::ForkRequiresPersistedParent)` / `ForkRequiresLeaf` for either origin, and the
module doc at `:26-30` states it *"MUST fail hard rather than silently downgrading to fresh context"*.
Call sites propagate the error (`src/extension/executor/foreground.rs:156-159`,
`src/extension/executor/background.rs:104`).
`grep -rn 'can_prefer_fork\|implicit_fork\|default_subagent_context\|defaultSubagentContext' --include=*.rs`
→ **0 hits**. `src/extension/tool/schema.rs:399-403` declares `"enum": ["fresh", "fork"]` and the test
at `:741` pins that two-value enum.

**Impact** — Three user-visible behaviours. **(1)** An agent whose frontmatter says
`defaultContext: fork`, launched from a session that has not persisted yet (a brand-new session before
the first assistant append, or an in-memory session), runs **fresh** upstream and **errors out
entirely** in the port with "fork requires a persisted parent" — the user never asked for fork at the
call site, so the agent author's preference turns a working launch into a failed one. **(2)**
`subagents.defaultSubagentContext: "fork"` (or `"fresh"` to override agents that declare fork) has no
representation and is dropped. **(3)** `context: "profile"` is rejected by the port's closed enum, so
a caller cannot say "honour each agent's declared `defaultContext` and fail loudly if one has none."
`high` not `critical`: the port's behaviour is a loud, explicit error that aborts the launch before
any subprocess spawns (`exec/mod.rs:1402` proves zero filesystem side effects) — a failed launch the
user must retry, not silent corruption.

**Fix** — Split explicit from inherited in `resolve_effective_context`: keep the strict path for an
explicit call-site `Fork`, and for an inherited preference test availability first (a
`can_prefer_fork(session)` mirroring `canPreferForkFromSnapshot` — persisted parent file plus leaf id)
and downgrade to `Fresh` when unavailable. Add `default_subagent_context` to
`SubagentExtensionConfig` **above** the agent default in the precedence chain, with upstream's
validation error. Add `"profile"` to the schema enum at `tool/schema.rs:399` and the policy branch
with pi's message, updating the pinning test at `:741`.

**Verify** — An agent with `defaultContext: fork` launched from an unpersisted session must run fresh,
not error; an explicit `context: "fork"` from the same session must still error.
`defaultSubagentContext: "fresh"` must override an agent that declares fork.
`context: "profile"` against an agent with no `defaultContext` must fail with pi's message.

**Relation to corpus** — New. No `SUBA` row covers fork-context resolution policy; `VL-S2`'s
`chatProgress`/workflow scope does not reach it. Merges the foreground-exec-lens and
shared-config-lens candidates, which are the same function.

---

## Scope

In scope: the behaviour described above, in `crates/cyrup-ext-subagents/`.

Out of scope: the other SUBA items in this batch (each has its own file); any refactor beyond what the fix needs; and the ledger corrections in `SUBA-CORPUS-HEALTH.md`.

Full finding, with the complete evidence chain: [`09a-cyrup-ext-subagents-v0.57-drift.md` § SUBA-079](../../../docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md)

## Approach

Fall back to a fresh context when `defaultContext: fork` is requested and the parent session is not
yet persisted, instead of hard-failing the launch. Add the config `defaultSubagentContext` rung and
the `context: "profile"` variant.

## Acceptance Criteria

- [ ] `defaultContext: fork` against an unpersisted parent falls back to fresh and launches
- [ ] `defaultSubagentContext` is parsed and applied
- [ ] `context: "profile"` is accepted
- [ ] `cargo test -p cyrup-ext-subagents` passes
