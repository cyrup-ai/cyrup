---
stage: new
status: done
updated: 2026-08-27 05:30
severity: high
effort: medium
subsystem: discovery settings / thinking
source: docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md
item: SUBA-078
---

> **Path note.** This task lives in a subdirectory. Every flux command globs a single level (`ls -1 "$FLUX_BASE/todo/"*.md`), so `/exec`, `/aug` and `/qa` will not list it — pass the absolute path explicitly.

# SUBA-078 — `subagents.maxThinking` ceiling is entirely absent — no settings parse, no per-agent bound, no enforcement, no env propagation to nested children

**Kind** not-ported · **Severity** high · **Effort** M · **Confidence** confirmed
**Subsystem** discovery settings / thinking level
**Window** v0.47.1..v0.57.0 (`547112ec feat: add max thinking ceiling for subagents #1397`).

**upstream** — `git show v0.57.0:src/shared/thinking-ceiling.ts` (56 lines):
**`:4`** `SUBAGENT_THINKING_CEILING_ENV = "PI_SUBAGENT_THINKING_CEILING"`; **`:8`**
`parseThinkingLevel`; **`:16`** `compareThinkingLevels`; **`:23`** `intersectThinkingCeilings`, which
takes the **LOWEST** so a bound can only tighten down a nested subtree; **`:29`**
`decodeThinkingCeiling`; **`:42`** `assertThinkingWithinCeiling`, which throws
``Thinking level '<x>' exceeds configured maximum '<y>' for agent '<a>' run '<r>'.``
`git show v0.57.0:src/agents/agents.ts` puts `maxThinking?: ThinkingLevel` on `SubagentSettings`, on
`AgentConfig` and on `AgentDiscoveryResult`, parses it with
``Subagent settings in '<file>' have invalid 'maxThinking'; expected one of off, minimal, low, medium,
high, xhigh, or max.``, and stamps it onto every merged agent via `resolveSubagentMaxThinking`
(project beats user) + `applySubagentMaxThinking`. It is enforced in
`src/runs/foreground/execution.ts` before the pi-args build **and** per model candidate, folded
monotonically, re-intersected and written to the child env in `src/runs/shared/pi-args.ts`, and
reported as a `"thinking_ceiling"` refusal by `src/api/preflight.ts`.

**cyrup** — `grep -rn 'max_thinking\|maxThinking\|thinking_ceiling\|THINKING_CEILING' --include=*.rs crates/cyrup-ext-subagents/src`
→ **0 hits** (a workspace-wide grep finds only two unrelated test function names in `cyrup-provider`
and `cyrup-config`). `src/discovery/types.rs:505-541 SubagentSettings` carries `default_model`,
`default_thinking`, `default_extensions`, `disable_builtins`, `disable_thinking`, `model_scope` — no
`max_thinking` — and `src/discovery/mod.rs:1174-1191 AgentDiscoveryResult` has no such field.
`parse_subagent_settings` (`src/discovery/mod.rs:655-705`) deserializes with no
`deny_unknown_fields`, so an authored `maxThinking` is dropped without diagnostic. The port's only
thinking handling on the launch path is `apply_thinking_suffix` (`src/exec/spawn_plan.rs:124-139`),
which applies the agent's level unconditionally.

**Impact** — An operator who sets `subagents.maxThinking: "low"` (or a per-agent `maxThinking`) gets
no bound and no error: an agent declaring `thinking: xhigh` is spawned with `--model <id>:xhigh` and
burns the reasoning budget the ceiling was configured to cap. Upstream hard-refuses the run — against
both the chosen model and every fallback candidate — and inherits the bound down every nesting level
through the ceiling env var, so a child can only tighten it. There is no
`CYRUP_SUBAGENT_THINKING_CEILING`, so even a bound that existed could not survive the re-exec.
`high` not `critical`: the run's answer is correct, it simply consumes more reasoning budget than the
operator capped — a configured resource bound silently ignored, not a permission bypass (the separate
CAPABILITY ceiling governs access and is `SUBA-072`).

**Fix** — Port `thinking-ceiling.ts` as `exec/thinking_ceiling.rs` (compare / intersect / decode /
assert), add `max_thinking` to `SubagentSettings`, `AgentDefinition` and `AgentDiscoveryResult` with
upstream's parse error and project-beats-user resolution, assert it in `exec/fallback.rs` per model
candidate as well as once before the spawn-plan build, and write the intersected ceiling into the
child env in `exec/spawn_plan.rs` beside the capability ceiling.

**Verify** — `subagents.maxThinking: "low"` plus an agent declaring `thinking: xhigh` must refuse the
run with pi's message; a nested child must inherit the bound through the env var and must not be able
to widen it; a fallback candidate that would exceed the ceiling must be refused too.

**Relation to corpus** — New. **NOT** covered by `SUBA-021` / `VL-S1`, which is the CAPABILITY
ceiling (tools/agents/extensions) — a different mechanism, a different env var, and already partly
landed. Merges three lens candidates (foreground-exec, discovery-settings, shared-config) that are
one subsystem.

---

## Scope

In scope: the behaviour described above, in `crates/cyrup-ext-subagents/`.

Out of scope: the other SUBA items in this batch (each has its own file); any refactor beyond what the fix needs; and the ledger corrections in `SUBA-CORPUS-HEALTH.md`.

Full finding, with the complete evidence chain: [`09a-cyrup-ext-subagents-v0.57-drift.md` § SUBA-078](../../../docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md)

## Approach

Port `subagents.maxThinking` end to end: settings parse, per-agent bound, enforcement at launch, and
env propagation so nested children inherit the ceiling.

## Acceptance Criteria

- [ ] `subagents.maxThinking` is parsed from settings
- [ ] A per-agent thinking level above the ceiling is clamped
- [ ] The ceiling propagates to nested children via env
- [ ] `cargo test -p cyrup-ext-subagents` passes
