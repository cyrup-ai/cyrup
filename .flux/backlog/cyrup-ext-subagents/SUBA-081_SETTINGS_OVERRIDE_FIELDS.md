---
stage: new
status: done
updated: 2026-08-27 05:30
severity: high
effort: medium
subsystem: discovery / settings merge
source: docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md
item: SUBA-081
---

> **Path note.** This task lives in a subdirectory. Every flux command globs a single level (`ls -1 "$FLUX_BASE/todo/"*.md`), so `/exec`, `/aug` and `/qa` will not list it — pass the absolute path explicitly.

# SUBA-081 — Ten settings-override fields never apply, and a legal upstream `tools: "inherit"` fails the settings load instead of being applied

**Kind** parity-bug · **Severity** high · **Effort** M · **Confidence** confirmed
**Subsystem** discovery / merge (settings overrides)
**Window** in-baseline (≤ v0.43.0) for `description`/`extensions`/`toolBudget`/`acceptanceRole`; **v0.47.1..v0.57.0** for `output`/`outputMode`/`defaultReads`/`defaultProvider`/`fast` and the `"inherit"` tools variant.

**upstream** — `git show v0.57.0:src/agents/agents.ts:81-101` — `BuiltinAgentOverrideConfig` has
**22** fields, read in full and in order: `description`, `output?: string | false`,
`outputMode?: OutputMode`, `defaultReads?: string[] | false`, `model`, `defaultProvider?: string | false`,
`fallbackModels`, `fast?: boolean`, `thinking`, `systemPromptMode`, `inheritProjectContext`,
`inheritSkills`, `defaultContext`, `acceptanceRole?: AcceptanceRole | false`, `disabled`,
`systemPrompt`, `skills`, **`tools?: string[] | false | "inherit"`**, `extensions?: string[] | false`,
`subagentOnlyExtensions`, `completionGuard`, `toolBudget?: ToolBudgetConfig | false`.
`git show v0.43.0:src/agents/agents.ts:82` opens the same interface with **17** fields, which already
include `description`, `extensions`, `toolBudget` and `acceptanceRole` — so those four were portable
at the measured baseline. `applyBuiltinOverride` applies each field, with `false` meaning delete;
**`:1237-1246`** `applyToolsOverride` treats the literal `"inherit"` specially:
```ts
if (toolsOverride === "inherit") { delete target.tools; delete target.mcpDirectTools; return; }
```
— drop the allowlist so the builtin inherits the parent's full tool set.

**cyrup** — `crates/cyrup-ext-subagents/src/discovery/types.rs:432-477 AgentOverrideConfig` declares
exactly **13** fields, read in full: `model`, `fallback_models`, `thinking`, `system_prompt_mode`,
`inherit_project_context`, `inherit_skills`, `default_context`, `disabled`, `system_prompt`, `skills`,
`tools`, `subagent_only_extensions`, `completion_guard` — with **no** `description`, `extensions`,
`tool_budget`, `acceptance_role`, `output`, `output_mode`, `default_reads`, `default_provider` or
`fast`. `src/discovery/merge.rs:387-464 apply_builtin_override` applies only those 13. **The struct's
own doc at `types.rs:411-414` claims it is** *"a field-for-field port of pi's
`BuiltinAgentOverrideConfig` (`agents.ts:82-100`) — every field below is exactly one pi override
field, and pi has no others"* — which is false even at the v0.43.0 baseline the port measured
against. For `"inherit"`: `tools` is `OverrideField<Vec<ToolRef>>` (`:470`); `OverrideField`'s
hand-written `Deserialize` (`types.rs:359-386`) is an untagged `enum Raw<U> { Value(U), Clear(OverrideClearSentinel) }`
and `OverrideClearSentinel` (`types.rs:299-330`) accepts EXCLUSIVELY the JSON literal `false` — so the
string `"inherit"` matches neither arm, `serde_json::from_value` fails, and
`src/discovery/mod.rs:668-669` maps that to `SubagentError::MalformedSettings`, which
`mod.rs:787-794` propagates out of the settings read as a hard error.

**Impact** —
`subagents.agentOverrides.reviewer = {description: "…", extensions: ["./x.ts"], toolBudget: {…},
acceptanceRole: "read-only", outputMode: "file-only", defaultProvider: "anthropic"}` changes six real
things upstream and **silently changes nothing** in the port — no error tells the operator the
override did nothing. Worse, `subagents.agentOverrides.worker.tools = "inherit"` — a legal upstream
value meaning "drop the allowlist, inherit the parent's tools" — does not merely fail to apply: it
**fails the settings load** with `MalformedSettings`, so a pi-shaped `settings.json` takes agent
discovery down until the key is removed. `high` not `critical`: the nine ignored fields are a silent
config no-op rather than wrong run output, and the `"inherit"` case produces a named, surfaced error
rather than a crash — but a legal pi-shaped settings file killing discovery is squarely above
`medium`.

**Fix** — Add the nine missing fields to `AgentOverrideConfig` with the right `OverrideField` /
plain-bool shapes and apply each in `apply_builtin_override`, mirroring upstream's per-field
validators and their error text. Extend `OverrideField`'s `Deserialize` (or add a `tools`-specific
enum) with an `Inherit` arm accepting the literal string `"inherit"`, and have
`apply_builtin_override` clear both `tools` and `mcp_direct_tools` on that arm. **Delete the
completeness claim at `types.rs:411-414`** and replace it with an assertion pinned against a
checked-in copy of upstream's field list, so the set cannot silently drift again.

**Verify** — Each of the nine fields set in `agentOverrides` must be observable on the merged agent
(`acceptanceRole: false` must delete, per upstream's `| false` semantics).
`agentOverrides.<n>.tools = "inherit"` must load successfully and produce an agent with no tool
allowlist and no MCP direct tools. A settings file containing all 22 upstream fields must load
without error.

**Relation to corpus** — New. `SUBA-061` names four *config* keys, not override fields; nothing in
the corpus covers `AgentOverrideConfig`'s field set, and the port's own doc comment asserting
completeness is the reason no prior pass caught it. Merges the two discovery-lens override candidates
because they are one struct and one apply function. **Note:** the ignored `acceptanceRole` override is
permission-adjacent — an operator who writes `acceptanceRole: "read-only"` gets no restriction and no
error — but the port has no `acceptance_role` on `AgentDefinition` at all, which is `SUBA-082`'s
broader gap; land the two together.

---

## Scope

In scope: the behaviour described above, in `crates/cyrup-ext-subagents/`.

Out of scope: the other SUBA items in this batch (each has its own file); any refactor beyond what the fix needs; and the ledger corrections in `SUBA-CORPUS-HEALTH.md`.

Full finding, with the complete evidence chain: [`09a-cyrup-ext-subagents-v0.57-drift.md` § SUBA-081](../../../docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md)

## Approach

Apply the ten settings-override fields that currently parse and do nothing
(description, extensions, toolBudget, acceptanceRole, output, outputMode, defaultReads,
defaultProvider, fast, plus the tools variant), and accept `tools: "inherit"` as a legal value
instead of failing the whole settings load on it.

## Acceptance Criteria

- [ ] Each of the ten fields demonstrably changes behaviour when set
- [ ] `tools: "inherit"` loads and applies rather than erroring
- [ ] A test covers the inherit variant
- [ ] `cargo test -p cyrup-ext-subagents` passes
