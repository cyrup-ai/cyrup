---
stage: new
status: done
updated: 2026-08-27 05:30
severity: high
effort: large
subsystem: external runners / agent schema
source: docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md
item: SUBA-074
---

> **Path note.** This task lives in a subdirectory. Every flux command globs a single level (`ls -1 "$FLUX_BASE/todo/"*.md`), so `/exec`, `/aug` and `/qa` will not list it — pass the absolute path explicitly.

# SUBA-074 — Agent `runner:` frontmatter is ignored entirely, so a profile upstream runs as a sandboxed read-only foreign CLI runs in cyrup as a full-capability native child

**Kind** not-ported · **Severity** high · **Effort** L · **Confidence** confirmed
**Subsystem** external runners / agent definition schema
**Window** in-baseline (≤ v0.43.0) for the `runner:` key and generic external-cli dispatch; **v0.47.1..v0.57.0** for the adapter ids, capability contract, preflight, hardened runner and the entire external-job protocol.

**upstream** — `git show v0.57.0:src/agents/agents.ts` **`:121`** `runner?: AgentRunnerConfig`;
**`:1803`** `parseAgentRunnerFrontmatter` (type must be `pi` | `external-cli` | `external-job`;
`external-cli` requires a non-empty `command`; an optional code-owned `adapter` id rejected unless one
of the recognised set; `args` alongside `adapter` rejected because the adapter owns its argv;
`promptDelivery: "stdin"` only); **`:1864`** `validateExternalRunnerProfile`, which HARD-FAILS such a
profile that also declares any of `tools, model, fallbackModels, thinking, extensions,
subagentOnlyExtensions, maxSubagentDepth, completionGuard, skills, skill, skillPath, toolBudget,
permission, permissions` — **`:1869`** ``Agent '${agentName}' uses runner.type='${runner.type}' and
declares unsupported Pi-only fields: ${unsupported.join(", ")}.`` **`:1950`**
`validateCodeOwnedProfileRunner`, imported at **`:12`** from `runs/shared/external-cli-contract.ts`.
The execution branch in `src/runs/background/subagent-runner.ts` never launches a pi child for
`external-cli`, and a separate branch handles `external-job`. **In-baseline:**
`git show v0.43.0:src/agents/agents.ts` already parses `runner.type` = `pi`|`external-cli` with
`command`/`args`/`promptDelivery`, and `git show v0.43.0:src/runs/shared/external-cli-runner.ts` is
already a working runner. The window added the capability contract (`external-cli-contract.ts`), the
hardened runner (env allowlists, bounded logs, JSONL framing, prompt-file delivery, process-tree
kill), the preflight probe (`external-cli-preflight.ts`), three adapters
(`claude-code-adapter.ts`, `codex-exec-adapter.ts`, `cursor-agent-adapter.ts`) and the whole
external-job protocol (`api/external-job-provider.ts`, `external-job-bridge.ts`,
`external-job-runner.ts`).

**cyrup** — `grep -rn 'runner' crates/cyrup-ext-subagents/src/discovery/frontmatter.rs` → **0 hits**,
and `KNOWN_FIELDS` (`frontmatter.rs:72-116`) has no `runner` entry, so the key falls through to
`extra_fields` — the nested-block round-trip is pinned by the crate's own
`permission_style_nested_block_round_trips_into_extra_fields` test at `frontmatter.rs:1209`.
`src/discovery/types.rs:702-838 AgentDefinition` has no `runner` or adapter field. Workspace-wide,
`grep -rniE 'external.cli|externalcli|external_job|external-job' --include=*.rs crates/cyrup-ext-subagents/src`
returns **one** hit and it is a doc comment (`src/background/runner_main.rs:4173`, citing
`external-cli-runner.ts:108` only for a verbatim error string). `grep -rn 'codex|cursor-agent|claude-code'`
across `crates/` matches only `cyrup-provider`'s OpenAI-Codex HTTP provider and
`cyrup-tui/src/auth_select.rs` — a different subsystem, with no argv construction, no process spawn
and no JSONL parser.

**Impact** — An agent file declaring `runner: {type: external-cli, adapter: claude-code}` (or
`{type: external-job, provider: …}`) is **neither rejected nor honoured**: the block is round-tripped
into `extra_fields` and the agent loads as an ordinary native agent against the session's own model.
Because upstream FORBIDS `tools:` and `permission:` on such a profile, the profile carries no tools
declaration — and `AgentDefinition::tools == None` in the port means *"no allowlist restriction, all
builtin tools available"* (`discovery/types.rs:728-730`). So the exact profile upstream runs as a
plan-mode, read-only, no-MCP, curated-env foreign process, cyrup runs as a **native child with the
full builtin tool surface in the workspace**, with no error and no diagnostic. Upstream's guard
rejecting Pi-only fields on such a profile is also absent, so a contradictory definition is accepted
silently. `high` rather than `critical`: cyrup's own runtime permission system still gates the
resulting tool calls, so this is a silent widening of the *declared* capability envelope rather than
a bypass of an enforcement point.

**Fix** — Two landable stages. **(1) Stop the silent widening, today, at `S` effort:** add `runner`
to `KNOWN_FIELDS` and to `AgentDefinition`, port `parseAgentRunnerFrontmatter` +
`validateExternalRunnerProfile`, and **refuse to launch** a non-`pi` runner with a named error until
stage 2 lands. That converts a silent capability widening into a loud unsupported-feature error and
is worth doing independently. **(2) Port the runners:** `external-cli-runner.ts` + the capability
contract + preflight + the three adapters, then the external-job protocol; each is a separable
change.

**Verify** — An agent declaring `runner: {type: external-cli, command: "…"}` plus `tools: [read]`
must fail to load with pi's "unsupported Pi-only fields" message. After stage 1, an
`external-cli` profile must fail the launch with a named error rather than spawning a native child.
After stage 2, it must spawn the foreign CLI with the adapter's argv and never a pi child.

**Relation to corpus** — **REVISION of PARITY-GAPS `VL-S14`** (*"`runner: external-cli` agents
unsupported"*, `medium`), whose scope and severity are both now wrong: the subsystem tripled inside
the window and gained a second runner type (`external-job`) that `VL-S14` does not name, and the
consequence is not "unsupported" but a silent capability widening. Two provenance corrections for
whoever works it: **there is no `grok-build-adapter.ts` at v0.57.0** (renamed to
`cursor-agent-adapter.ts`; `grok-build` survives only as a legacy receipt id), and **the baseline
half was never ported either**, so this is not pure window lag.

---

## Scope

In scope: the behaviour described above, in `crates/cyrup-ext-subagents/`.

Out of scope: the other SUBA items in this batch (each has its own file); any refactor beyond what the fix needs; and the ledger corrections in `SUBA-CORPUS-HEALTH.md`.

Full finding, with the complete evidence chain: [`09a-cyrup-ext-subagents-v0.57-drift.md` § SUBA-074](../../../docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md)

## Approach

Largest item in the batch — split before executing if it does not fit one session.
1. Add `runner:` to the agent frontmatter schema and thread it into the launch plan.
2. Port the external-CLI capability contract, then the adapters (Claude Code, Codex exec, Grok Build,
   OpenAI-Codex fast mode) behind it.
3. Port the external-job protocol: follow-ups, the hardened one-shot runner, writer profiles.
4. Until an adapter exists for a declared `runner:`, FAIL the launch — never fall back to a
   full-capability native child, which is the present silent behaviour.

## Acceptance Criteria

- [ ] `runner:` is parsed, validated and carried into the launch plan
- [ ] An agent declaring an unimplemented `runner:` fails the launch rather than running natively
- [ ] At least the capability contract plus one adapter round-trips a real run
- [ ] `cargo test -p cyrup-ext-subagents` passes
