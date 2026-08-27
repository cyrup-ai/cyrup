# 09a — cyrup-ext-subagents: the v0.47.1 → v0.57.0 drift pass

**This file supplements `09-cyrup-ext-subagents.md`. It does not supersede it, and no item in it is
edited by this pass.** `09` remains the area's file of record for `SUBA-001`…`SUBA-071`, for the
status table, and for the Trackers section. This document adds `SUBA-072`…`SUBA-091` and records
three corrections to `09`'s existing evidence (see `## Corpus health`). `PARITY-GAPS.md`,
`00-residual-ledger.md` and the README baseline table are likewise untouched here; where this pass
found one of them factually wrong at HEAD, the correction is written below rather than applied there,
so a single maintainer can reconcile all of them in one pass.

## Scope

**Port measured:** `crates/cyrup-ext-subagents/` at cyrup HEAD `6db22a7`
(*Merge pull request #67 — claude/decompose-discovery-management*), working tree clean. **204 `.rs`
files, 181,306 lines under `src/`** (tests included; the crate keeps most of its tests in-module).

**Upstream measured:** `nicobailon/pi-subagents` at tag **v0.57.0**. Clone HEAD is `9593a1cd`
(*chore: deslop utility comments (#1569)*, 2026-08-26), which is **ahead of the tag**; everything
below is settled at the tag. v0.57.0 carries **229 `.ts` files under `src/`**.

**The unanalyzed window.** `09` settled every claim at **v0.43.0** (the ported baseline) or
**v0.47.1** (then-latest). The range `v0.47.1..v0.57.0` had never been analyzed. Measured in this
clone:

| measurement | value |
|---|---|
| commits in range (all paths, incl. merges) | 305 |
| commits touching `src/` (incl. merges) | 245 |
| commits touching `src/` (`--no-merges`) | 244 |
| `src/` diffstat | **168 files changed, +21,385 / −7,307** |
| whole-tree diffstat | 357 files changed, +49,921 / −13,590 |
| `feat:` / `BREAKING` subjects, `src/`-touching | **54** |

The tasking brief for this pass carried the figures *330 commits / 174 src files / +22,871 / −7,443 /
58 feats*. Those are close but not reproducible from this clone at these tags; the table above is
what `git log`/`git diff` return here and is what the reader should use. The discrepancy does not
change any finding — it is recorded so the next pass does not re-derive it as a contradiction.

Ten of the twenty items below entered inside that window. Nine are **in-baseline (≤ v0.43.0)** — that
is, they were portable at the tag `09` measured against and were missed, not lagged. One
(`SUBA-085`) entered in `v0.43.0..v0.47.1`. The window's headline features — external CLI adapters
and the CLI capability contract, workflow scripts from files, runtime MCP direct tools, runtime agent
registration, separated global context inheritance, live context-window usage, subagent default
provider, the max-thinking ceiling — are represented below by `SUBA-074`, `SUBA-078`, `SUBA-079`,
`SUBA-084` and `SUBA-088`; the rest were either already tracked or cut (`## Already tracked`).

**Item count added by this pass: 20 ids (`SUBA-072`…`SUBA-091`), of which 1 (`SUBA-080`) is REFUTED
and 8 are CARRIED-UNVERIFIED.** Counted, verified, schedulable: **11 items — 1 critical, 8 high,
2 medium.**

## Methodology

Every upstream claim was settled with `git show v0.57.0:<path>`, never by reading the clone's working
tree, because clone-HEAD line numbers and file existence both mislead here (the clone is 12 commits
past the tag). Where an item's window matters, the same claim was re-checked at `v0.43.0` and/or
`v0.47.1` with the same command, and the two are never mixed silently: each item carries a
`**Window**` line stating which tag the behaviour first existed at.

**The restructure trap, and how absence was established.** `crates/cyrup-ext-subagents/src/extension.rs`
**no longer exists as a file** — it is now the directory `src/extension/` (`index`/`mod`, `executor/`,
`host/`, `models/`, `tool/`, …). Every `extension.rs:NNNN` citation in `09` is therefore
*unresolvable*, not merely stale: `SUBA-005`'s `:6557`, `SUBA-043`'s `:6543-6690`, `SUBA-047`'s
`:18993`, `SUBA-064`'s `:7805`/`:7825`, and the citations inside `SUBA-016`, `SUBA-055` and
`SUBA-057`. **A reader of `09` must not conclude a feature is absent because its cited path is gone.**
The more dangerous direction is the false negative, and this pass hit one: upstream's
`restoreActiveJobs` reads as absent under every name upstream uses and is **fully present** in the
port as `resume_tracking` (`src/extension/executor/status.rs:27`, wired on `SessionStart` at
`src/extension/host/native_impl.rs:347`, with `src/extension/executor/paths.rs:566-630` pinning both
of its subtleties). It is recorded under `## Refuted`-adjacent notes in `## Already tracked` so nobody
re-derives it.

Accordingly, **no absence claim below rests on resolving a cited path.** Each was established by
grepping the current tree for the behaviour *by identifier and by concept*, in both camelCase and
snake_case, plus the env-var spellings, plus the crate's own tests — the port's tests are treated as
evidence of presence, and several candidate findings died there.

**Severity** is `docs/gap-analysis/README.md:509-512` applied literally: `critical` = data loss,
silent wrong output, a permission bypass, or a crash on a normal path, **with no reachability
qualifier**. **Effort** is `S` under a day · `M` a few days · `L` a week+ or needs design.
**`[CYRUP-DELTA]`** in a port comment marks a deliberate divergence and is a decision, not a gap —
two candidate findings were dropped on that basis, and one item below (`SUBA-083`) exists precisely
*because* the divergence carries no such marker.

Each confirmed item passed an adversarial refutation pass instructed to reject anything it could not
personally re-read on both sides. Two severities were corrected downward by that pass and both
corrections are applied and recorded at the item.

---

## Summary — confirmed items

| ID | Sev | Eff | Subsystem | Title |
|---|---|---|---|---|
| SUBA-072 | critical | M | foreground exec / tool allowlisting | Capability ceiling's `allowedTools` and `denyExtensions` axes are resolved and propagated but never applied to the child |
| SUBA-073 | medium | M | config / permissions / frontmatter | Subagent permission policy never reaches a spawned child; `permission:` frontmatter is accepted and inert |
| SUBA-074 | high | L | external runners / agent schema | `runner:` frontmatter is ignored entirely, so a sandboxed foreign-CLI profile runs as a full-capability native child |
| SUBA-075 | high | M | fork context / thinking | Forked child sessions are not sanitized: signed/redacted Anthropic thinking blocks inherited, no thinking-off override |
| SUBA-076 | high | S | acceptance / evidence scoring | Evidence checks are scored binary where upstream is tri-state, producing two spurious acceptance rejections |
| SUBA-077 | high | S | foreground exec / deadlines | A foreground run with no explicit timeout has NO wall-clock deadline, and there is no global `timeoutMs` |
| SUBA-078 | high | M | discovery settings / thinking | `subagents.maxThinking` ceiling entirely absent — no parse, no bound, no enforcement, no env propagation |
| SUBA-079 | high | S | fork context / launch policy | `defaultContext: fork` hard-fails when the parent is unpersisted where upstream falls back to fresh; no config rung; no `context:"profile"` |
| SUBA-081 | high | M | discovery / settings overrides | Ten `agentOverrides` fields never apply, and a legal `tools: "inherit"` fails the settings load |
| SUBA-083 | high | S | config / launch mode | `asyncByDefault` default is inverted, making the documented `asyncByDefault:false` opt-out a no-op |
| SUBA-085 | high | S | missions | `mission.resolve-decision` unported: a decision is write-once and permanently open, wedging the goal driver |

Carried-but-unverified (`## Carried — NOT adversarially verified`): `SUBA-082`, `SUBA-084`,
`SUBA-086` (high); `SUBA-087`, `SUBA-088`, `SUBA-089`, `SUBA-090`, `SUBA-091` (medium).
Refuted: `SUBA-080`.

---

## SUBA-072 — The capability ceiling's `allowedTools` and `denyExtensions` axes are resolved, intersected and propagated but never applied to the spawned child

**Kind** parity-bug · **Severity** critical · **Effort** M · **Confidence** confirmed
**Subsystem** foreground execution / tool allowlisting (`exec/spawn_plan.rs`)
**Window** in-baseline (≤ v0.43.0) — `git cat-file -e v0.43.0:src/runs/shared/capability-ceiling.ts` succeeds.

**upstream** — `git show v0.57.0:src/runs/shared/pi-args.ts`. `resolvePiLaunchToolPlan` at **`:423`**
intersects the call-site and inherited ceilings and builds `allowedToolSet` at **`:430-433`**. The
resolved ceiling then drives **four** independent narrowings:
- **`:439-441`** throws ``Capability ceiling from ${sources} excludes required tool 'read' for lazy skill loading.`` when `requireReadTool` is set and the set lacks `read`.
- **`:444-455`** `declaredBuiltinTools` becomes `[...allowedToolSet]` on the `input.tools === undefined` arm, and is `.filter((tool) => !allowedToolSet || allowedToolSet.has(tool))` on the declared arm.
- **`:457-463`** `toolExtensionPaths` is `[]` when `denyExtensions`; **`:464`** `resolvedMcpSelections` is likewise `[]`; **`:467-469`** the surviving MCP selections are filtered through `allowedToolSet`; **`:514`**/**`:527`** force `disableAmbientExtensions` and empty `configuredExtensions`.
- **`:473-476`** `explicitToolAllowlist` is true whenever `allowedToolSet !== undefined`, so `buildPiArgs` at **`:662`** always emits `--tools <ceiling set>` or `--no-tools` for a ceilinged child, and **`:668`** pushes `--no-extensions` under `disableAmbientExtensions`.

**cyrup** — `crates/cyrup-ext-subagents/src/exec/capability_ceiling.rs` defines `allowed_tools` at
`:85` and `deny_extensions` at `:91`, parses them at `:192`/`:195` and intersects them at `:390`/`:392`.
`grep -rn 'allowed_tools\|deny_extensions' --include=*.rs crates/cyrup-ext-subagents/src` returns
**only** `capability_ceiling.rs` (definition, parse, intersect, tests) plus the unrelated
`watchdog/review.rs` — **no consumer anywhere in `src/exec/spawn_plan.rs`**. In `spawn_plan.rs` the
ceiling is resolved at `:309` and only the AGENTS axis is enforced (`assert_agent_allowed`, `:313`);
the tool-allowlist branch gates solely on `let explicit_tool_allowlist = agent.tools.is_some();`
(**`:397`**) and builds the allowlist from `builtin_tools` + `effective_mcp_tools` with no ceiling
filter; the extension branch gates solely on `agent.extensions`; the ceiling is then only
base64-encoded into the child env at `:876-891`. **The port's own comment at `spawn_plan.rs:417-420`
still asserts** *"cyrup has no capability ceiling (tracked as SUBA-021), so `allowedToolSet` is
permanently `undefined`"* — stale since `capability_ceiling.rs` landed. Nothing on the CHILD side
reads it either: `CAPABILITY_CEILING_ENV` hits only `spawn_plan.rs` (write side) and
`capability_ceiling.rs` (constants).

**Impact** — A host that registers a ceiling `{allowedTools: ["read"], denyExtensions: true}` for a
session gets **no tool bound and no extension bound at all** in cyrup. An agent whose frontmatter
declares `tools: [read, write, bash]` is spawned with `--tools read,write,bash`; an agent that
declares no `tools:` is spawned with the **full ambient tool set and full ambient extension
discovery** — no `--tools`, no `--no-extensions` — because `explicit_tool_allowlist` is `false`.
Upstream spawns the first with `--tools read` and the second with `--tools read`, both with
`--no-extensions` and with MCP direct tools and tool-extension paths stripped. Because the agents
axis *is* enforced and the ceiling *is* propagated to the child env, **the ceiling presents as armed
while two of its three axes silently permit exactly the widening it exists to prevent.** That is a
permission bypass under `README.md:510`, hence `critical`.

**Fix** — In `exec/spawn_plan.rs`, feed the already-resolved `capability_ceiling` into the tool plan:
(a) `explicit_tool_allowlist = agent.tools.is_some() || !effective_mcp_tools.is_empty() || ceiling_allowed_tools.is_some()`, mirroring `pi-args.ts:473-476`; (b) intersect `builtin_tools` and the
MCP selections against the allowed set on both arms of `pi-args.ts:444-455`; (c) under
`deny_extensions`, empty the extension paths and MCP selections and push cyrup's `--no-extensions`
equivalent; (d) land the `requireReadTool` throw at the same time — it is `SUBA-014`'s companion and
the two share the branch. Delete the stale claim at `spawn_plan.rs:417-420` in the same change.

**Verify** — With a ceiling `{allowedTools:["read"]}` registered: an agent declaring
`tools: [read, write, bash]` must spawn with `--tools read`; an agent declaring no `tools:` must also
spawn with `--tools read`. With `{denyExtensions:true}`: the child argv must carry the
no-extensions flag and no tool-extension path or MCP direct tool. With `requireReadTool` and a
ceiling lacking `read`, the launch must fail with pi's message.

**Relation to corpus** — **REVISION of `SUBA-021` / PARITY-GAPS `VL-S1`.** `SUBA-021`'s evidence
(`rg 'capability_ceiling' = 0`, "no ceiling concept") is now factually wrong at HEAD — the subsystem
landed in sweep 10 — and the residual defect is *materially worse* than the one `SUBA-021` filed,
because an unimplemented ceiling is visibly absent whereas this one presents as enforced. Either
raise `SUBA-021` to `critical` and rewrite its body, or supersede it with this row.

---

## SUBA-073 — Subagent permission policy never reaches a spawned child: `config.permissions` and agent `permission:`/`permissions:` frontmatter are accepted and inert

**Kind** not-ported · **Severity** medium *(corrected down from `critical` as filed — see the note below; `high` is defensible)* · **Effort** M · **Confidence** confirmed
**Subsystem** config / permissions / discovery frontmatter
**Window** in-baseline (≤ v0.43.0) — `v0.43.0:src/runs/shared/permissions.ts` and `v0.43.0:src/shared/types.ts` both carry it.

**upstream** — `git show v0.57.0:src/shared/types.ts` **`:2268`** declares
`permissions?: PermissionConfig` on `ExtensionConfig`, documented at `:2267` as *"Opt-in native tool
permissions. Bash remains outside this policy."* `git show v0.57.0:src/runs/shared/permissions.ts`
(99 lines) defines `PERMISSION_POLICY_ENV = "PI_SUBAGENT_PERMISSION_POLICY"` (**`:8`**),
`validatePermissionRules` (**`:21`**), `validatePermissionConfig` (**`:35`**), `resolvePermissionRules`
(**`:44`**), `permissionDecision` (**`:50`**) and `encodePermissionRules` (**`:55`**).
`src/extension/config.ts` runs `validatePermissionConfig(config.permissions)` on every config read.
`git show v0.57.0:src/agents/agents.ts` **`:2033`** throws
``Agent '${localName}' cannot declare both permission and permissions frontmatter.`` and then parses
`frontmatter.permissions ?? frontmatter.permission` through `validatePermissionRules`;
`agent-serializer.ts` carries both spellings in `KNOWN_FIELDS`. `async-execution.ts`,
`api/preflight.ts` call `resolvePermissionRules(ctx.permissions, agentConfig.permissions)` and
`pi-args.ts` writes the encoded policy into the child env.

**cyrup** — `grep -rn 'PERMISSION_POLICY_ENV' crates/cyrup-ext-subagents/src/exec/ crates/cyrup-ext-subagents/src/spawn/`
→ **0 hits**; there is no writer anywhere in the workspace. Every hit crate-wide is a READ site: the
child-side gate `src/watchdog/permission_arbiter.rs:355` (cyrup's `CYRUP_SUBAGENT_*` spelling) and
`src/prompt_runtime.rs:1399,1442,2225-2227,2446,2467`. The crate states it in-tree at
`src/watchdog/permission_arbiter.rs:60-63`: *"The parent-side half (`validatePermissionConfig`,
`resolvePermissionRules`, `encodePermissionRules`, and `pi-args.ts:713-758`'s env writes) is still
unported, so a policy reaches a child today only if something outside this crate sets
`PERMISSION_POLICY_ENV`; that is the remaining work, and it lives in `exec/`, not here."* On the
frontmatter side, `src/discovery/frontmatter.rs:72-116 KNOWN_FIELDS` contains **neither** `permission`
nor `permissions` (grep for `permission` in that range: 0 hits), and the crate's own tests PIN the
demotion — `frontmatter.rs:1213-1216` asserts a `permission:` block lands in `extra_fields` and
`present_fields`. `SubagentExtensionConfig` (`src/registration/mod.rs:79-245`) has no `permissions`
key.

**Impact** — An operator who writes `{"permissions": {"rules": {"write": "deny"}}}` in subagent
config, or an agent author who writes `permission: {"*": ask, bash: {"*": ask, "git *": allow}}` in
an agent file, gets the value accepted with no error and silently not enforced: the child spawns with
no policy env var, `permission_arbiter`'s gate is never armed, and the denied tool runs. Upstream's
mutual-exclusion error for declaring both spellings is also absent. The child-side enforcement
machinery is fully ported and permanently unreachable.

**Severity note (correction applied).** Filed `critical`; corrected to `medium` by the refutation
pass, on three grounds read literally against `README.md:510`. (1) This is not a bypass of an
*enforcing* system: a cyrup subagent child is still gated by `cyrup-permission-system`, wired into
every spawn, with the child→parent ask-forwarding spool live at `spawn/nested_events.rs:781`; upstream
itself documents `permissions` as **opt-in** and leaves bash to pi-guard. (2) Upstream's own normal
state is "no policy, no gate" — `resolvePermissionRules` returns `undefined` on an empty merged map
and no handler is installed — which is exactly the state cyrup is permanently in; the divergence is
that cyrup cannot *leave* it. (3) No data loss, no crash, no silent wrong output. **`high` is
defensible** on the frontmatter half alone: an agent file that literally reads
`permission: {...}` is accepted, round-tripped through `extra_fields`, re-serialized on rewrite and
never enforced, with no diagnostic — and `registration/authority.rs:22` states the crate's own
principle that *"a config key that is parsed and ignored is a permission bypass"*. `critical` is not
defensible given (1).

**Fix** — Port `permissions.ts`'s parent half as `exec/permissions.rs`
(`validate_permission_rules`/`validate_permission_config`/`resolve_permission_rules`/`encode_permission_rules`),
add `permissions` to `SubagentExtensionConfig` with the config-load validation, add both `permission`
and `permissions` to `frontmatter.rs`'s `KNOWN_FIELDS` with upstream's mutual-exclusion error, and
write the encoded policy into the child env in `exec/spawn_plan.rs` beside the existing tool-budget
encoder. The child side needs no work.

**Verify** — A child launched under `{"permissions":{"rules":{"write":"deny"}}}` must have
`CYRUP_SUBAGENT_PERMISSION_POLICY` set and must refuse a `write`; an agent declaring both
`permission:` and `permissions:` must fail to load with pi's message; an agent-level rule must merge
over the global config per `resolvePermissionRules`'s precedence.

**Relation to corpus** — New. Not covered by `SUBA-061` (whose four keys are `asyncWidget`,
`inlineToolDisplay`, `fleetKeybindings`, `legacyChainControls`), not by `SUBA-064` (`authorityPolicy`),
and not by area `10`, which owns the permission-system crate rather than this crate's parent-side
encoder. The discovery-lens and config-lens halves are merged here because both land in one place:
the env write in `exec/spawn_plan.rs`.

---

## SUBA-074 — Agent `runner:` frontmatter is ignored entirely, so a profile upstream runs as a sandboxed read-only foreign CLI runs in cyrup as a full-capability native child

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

## SUBA-075 — Forked child sessions are not sanitized: signed and redacted Anthropic thinking blocks are inherited verbatim and no thinking-off override is applied to the branch

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

## SUBA-076 — Acceptance evidence checks are scored binary where upstream is tri-state, so an honest `changedFiles: []` and an omitted `noStagedFiles` each produce a spurious acceptance REJECTION

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** confirmed
**Subsystem** acceptance / evidence scoring
**Window** in-baseline (≤ v0.43.0) for the `changed-files`/`tests-added` tri-state; **v0.47.1..v0.57.0** for the `no-staged-files` skip.

**upstream** — `git show v0.57.0:src/runs/shared/acceptance.ts`. `reportEvidenceStatus` at **`:932`**
returns `AcceptanceRuntimeCheckStatus`, not a boolean: for `"changed-files"` it returns `"failed"`
only when the field is not a string array, and otherwise
`report.changedFiles.length === 0 ? "not-applicable" : "passed"` — identically for `"tests-added"`.
Every other kind is binary. `runStructuralChecks` at **`:961`** opens its loop with
**`:964`** `if (kind === "no-staged-files" && report.noStagedFiles === undefined) continue;` — the
report-derived check is SKIPPED, and only the parent-side real `checkNoStagedFiles(cwd)`
(`git status --short`, pushed unconditionally at **`:976`** when the kind is requested) decides. The
tri-state is recorded as the check status with the message at **`:972`**
``${kind} evidence explicitly reported as not applicable.`` `evaluateAcceptance` rejects on
`runtimeChecks.some((check) => check.status === "failed")` only, so `not-applicable` does **not**
reject. The `no-staged-files` `continue` is absent from `git show v0.47.1:src/runs/shared/acceptance.ts`
(`bd5664a0 fix: trust parent staged-file acceptance check (#1385)`).

**cyrup** — `crates/cyrup-ext-subagents/src/exec/acceptance/model/checks.rs:14-42`
`report_evidence_present` returns a plain `bool`:
`ChangedFiles => report.changed_files.as_ref().is_some_and(|v| !v.is_empty())`,
`TestsAdded => …is_some_and(|v| !v.is_empty())`, `NoStagedFiles => report.no_staged_files == Some(true)`.
`run_structural_checks` (`:170-196`) iterates `for kind in evidence` with **no skip clause** and maps
the bool binary: `status: if present { RuntimeCheckStatus::Passed } else { RuntimeCheckStatus::Failed }`,
message `"{kind} evidence missing from child report."`, then pushes the parent-side
`check_no_staged_files(cwd)` at `:192-194`. `grep -rn 'NotApplicable' --include=*.rs` shows
`RuntimeCheckStatus::NotApplicable` is produced at exactly two sites, `checks.rs:125,132` — both
inside `check_no_staged_files`'s git-unavailable branch — **never** for an evidence check.
`src/exec/acceptance/model/evaluate.rs:160,208,219` reject on
`.any(|c| c.status == RuntimeCheckStatus::Failed)`.

**Impact** — Two spurious rejections on normal paths. **(1)** A child under
`acceptance: {evidence: ["changed-files"]}` that correctly reports `changedFiles: []` — a reviewer, an
oracle, a genuine no-op task — is accepted upstream with `evidence:changed-files = not-applicable` and
**REJECTED** by the port with `evidence:changed-files failed / changed-files evidence missing from
child report`. **(2)** With `evidence: ["no-staged-files"]` and a clean workspace, a child that simply
omits `noStagedFiles` is accepted upstream (the parent's own `git status` passes) and rejected by the
port — even though the port's own `git status` check *in the very same list* passed. In both cases
the ledger flips to `rejected` and the caller is told the child failed acceptance when it did not.
`high` not `critical`: the wrong verdict is loud (an explicit `rejected` status carrying a named
message), it fails closed rather than admitting bad work, and nothing is lost or bypassed.

**Fix** — One function. Change `report_evidence_present` to return `RuntimeCheckStatus`, giving
`ChangedFiles`/`TestsAdded` upstream's three arms (not-a-string-array → `Failed`, empty →
`NotApplicable`, else `Passed`), add the third message arm, and add the
`NoStagedFiles && report.no_staged_files.is_none() → continue` skip at the top of
`run_structural_checks`'s loop.

**Verify** — `evidence: ["changed-files"]` with `changedFiles: []` must accept, with the
`not-applicable` status and pi's message; with `changedFiles: "oops"` (not an array) must reject.
`evidence: ["no-staged-files"]` with `noStagedFiles` omitted and a clean worktree must accept with
exactly one `no-staged-files` check in the list.

**Relation to corpus** — New. Area 09 has no acceptance-scoring row (`SUBA-028` is acceptance
*cancellation*), and this pass confirmed the acceptance tree is otherwise substantially complete —
this is a defect inside ported code, not a missing subsystem. Both halves are one function; file and
fix together.

---

## SUBA-077 — A foreground subagent run with no explicit timeout has NO wall-clock deadline, and there is no global `config.timeoutMs`

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** confirmed
**Subsystem** foreground execution / deadlines
**Window** in-baseline (≤ v0.43.0) for the 30-minute foreground default; **v0.47.1..v0.57.0** for the `config.timeoutMs` rung.

**upstream** — `git show v0.57.0:src/runs/foreground/subagent-executor.ts` **`:2656`**
`export const DEFAULT_FOREGROUND_TIMEOUT_MS = 30 * 60 * 1000;`; **`:2684`**
`resolveConfigDefaultTimeoutMs` validates `config.timeoutMs` as a positive integer; **`:2719`**
`resolveSingleAgentLaunchTimeout(params, async, configDefaultTimeoutMs)` computes **`:2721`**
`const foregroundDefault = configDefaultTimeoutMs ?? DEFAULT_FOREGROUND_TIMEOUT_MS` and applies it to
every non-async launch. Its `!async` arm does not test `isComposite`, so the backstop applies to
single, `tasks: []` and chain launches alike at the one shared call site **`:5914-5917`**; the same
resolution appears again at **`:4440`**. The `config.timeoutMs` rung is a window addition, and commit
`0ed0afee` states the defect it fixes verbatim: agent frontmatter `timeoutMs` was applied only to
single-agent launches, so parallel and chain launches "never adopt it and fall back to the built-in
30-minute foreground default … with no global knob to raise the default."

**cyrup** — `crates/cyrup-ext-subagents/src/extension/tool/params.rs:264-280`
`resolve_foreground_timeout` validates `0` and the `timeoutMs`/`maxRuntimeMs` alias mismatch and then
returns `Ok(p.timeout_ms.or(p.max_runtime_ms))` — **no default at all**. Its caller
`src/extension/tool/routing.rs:370-372` does `resolve_foreground_timeout(p)…?.or(launch_defaults.1)`,
where `launch_defaults.1` is only the agent's own frontmatter `timeoutMs`
(`src/extension/executor/nested_control.rs:148-172`).
`grep -rn '1_800_000\|1800000\|30 \* 60' --include=*.rs` hits only `src/background/wait.rs:86`,
`src/background/mod.rs:43,57` (`DEFAULT_ASYNC_CHILD_TIMEOUT_MS`) and `src/extension/wait_tool.rs:65`
— the async side has its default, the foreground side has none. `SubagentExtensionConfig`
(`src/registration/mod.rs:79-245`) has no `timeout_ms` field; grepping `timeout_ms` in that file
returns only `worktree_setup_hook_timeout_ms`.

**Impact** — `subagent({agent:"x", task:"…"})` run in the foreground against an agent with no
frontmatter `timeoutMs` has **no wall-clock deadline** in cyrup: a child whose bash tool blocks
forever hangs the orchestrator's turn indefinitely with no signal, where upstream terminates it at 30
minutes with `Subagent timed out after 1800000ms.` Separately, an operator who sets
`subagents.timeoutMs` gets nothing — upstream uses it to replace the backstop for single, parallel,
chain and plain single-agent async launches alike, which is the only way to raise a long fan-out's
ceiling without passing `timeoutMs` on every call. `high` not `critical`: an unbounded hang is none
of the four `critical` conditions — but it sits at the top of `high`, because the failure is silent
and open-ended.

**Fix** — Give `resolve_foreground_timeout` a `config_default: Option<u64>` parameter and have it
return `p.timeout_ms.or(p.max_runtime_ms).or(agent_default).or(config_default).or(Some(DEFAULT_FOREGROUND_TIMEOUT_MS))`
for every non-async launch, mirroring `:2719-2725`'s precedence, and add `timeout_ms` to
`SubagentExtensionConfig` with upstream's positive-integer validation. Apply it at **all** foreground
call sites in `routing.rs`, not just the single-agent one — the parallel path drops an explicit
`timeoutMs` today.

**Verify** — A foreground agent with no frontmatter `timeoutMs` whose child sleeps must be terminated
at the default with pi's message; `subagents.timeoutMs: 60000` must replace that default on single,
`tasks: []` and chain launches; an explicit call-site `timeoutMs` must still win over both.

**Relation to corpus** — **REVISION-adjacent to `SUBA-051`**, which covers the ASYNC child default
and whose Fix line explicitly instructs *"Do not apply it to foreground runs, which already have
their own default."* **That instruction is wrong at HEAD and following it would leave the foreground
path unbounded forever.** Distinct item because the fix site (`extension/tool/params.rs` + a new
config key) differs from `SUBA-051`'s (`background` step construction).

---

## SUBA-078 — `subagents.maxThinking` ceiling is entirely absent — no settings parse, no per-agent bound, no enforcement, no env propagation to nested children

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

## SUBA-079 — An agent's `defaultContext: fork` hard-fails the launch when the parent session is not yet persisted, where upstream falls back to fresh — plus no config `defaultSubagentContext` rung and no `context: "profile"`

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

## SUBA-081 — Ten settings-override fields never apply, and a legal upstream `tools: "inherit"` fails the settings load instead of being applied

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

## SUBA-083 — `asyncByDefault`'s default is inverted, and the documented `asyncByDefault:false` opt-out is a no-op

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

## SUBA-085 — `mission.resolve-decision` unported: a mission decision is write-once and permanently open, so the goal driver proposes the same next action forever

**Kind** not-ported · **Severity** high · **Effort** S · **Confidence** confirmed
**Subsystem** missions
**Window** v0.43.0..v0.47.1 (`1dec33dd feat: add mission dispatch ledger`).

**upstream** — `git show v0.57.0:src/missions/actions.ts` **`:32-39`** `MISSION_ACTIONS` has **seven**
entries — `mission.create`, `mission.list`, `mission.show`, `mission.update`,
**`mission.resolve-decision`**, `mission.attach-run`, `mission.close` — and the handler at
**`:391-397`**:
```ts
if (action === "mission.resolve-decision") {
	const missionId = requireMissionId(params);
	const decisionId = validateMissionId(params.id, "id");
	if (typeof params.summary !== "string" || !params.summary.trim())
		throw new Error("mission.resolve-decision requires a non-empty summary");
	const record = updateMission(location, missionId, { resolveDecision: { id: decisionId, resolution: params.summary.trim() } });
	return textResult(`Resolved decision ${decisionId} for mission ${record.id}. …`);
}
```
`MissionUpdateInput` carries `resolveDecision?: { id: string; resolution: string }`, and the verb is
listed in `MUTATING_MANAGEMENT_ACTIONS` (`subagent-executor.ts`) and in `SUBAGENT_ACTIONS`
(`shared/types.ts`).

**cyrup** — `grep -rn 'resolve_decision\|ResolveDecision' --include=*.rs crates/cyrup-ext-subagents/src`
→ **0 hits**. `src/missions/types.rs:700-717 MissionUpdateInput` carries
`add_decisions: Vec<MissionDecisionInput>` with the doc *"Append decisions (always as NEW, open
decisions with fresh ids)"* and has **no** `resolve_decision` field; `is_empty()` at `:721-737`
enumerates every field and confirms the set is closed. `MissionDecision` does carry
`status: Open|Resolved`, `resolved_at` and `resolution`, but `MissionDecisionStatus::Resolved` is
produced at **exactly one site** — `src/missions/store.rs:355`, the on-disk PARSER
(`Some("resolved") => …`) — never by a mutation. `src/extension/tool/text.rs:187-229` advertises six
`mission.*` verbs, not seven.

**Impact** — In cyrup a mission decision can be **opened and never closed**.
`src/missions/goal_driver.rs:382-394` computes the mission's next ready action as
`record.decisions.iter().find(|item| item.status == MissionDecisionStatus::Open)` — and since nothing
can flip that status, a mission that ever records one decision returns that same decision as its next
ready action on every subsequent evaluation, and its autonomous progression is wedged. Upstream clears
it with one `mission.resolve-decision` call. There is no workaround under another name: `mission.update`
can only append new open decisions. `high` not `critical`: nothing is lost (the decision persists
correctly, it simply cannot be closed), there is no bypass and no panic — it is a functional stall of
autonomous progression plus a permanently stale continuation notice.

**Fix** — Add `resolve_decision: Option<MissionDecisionResolution>` to `MissionUpdateInput` (and to
`is_empty()`), implement the find/guard/mutate block in `store.rs` mirroring upstream's
(`status = Resolved`, `resolved_at`, `resolution`), add the seventh enum variant and its wire strings
in `missions/actions.rs`, `extension/tool/text.rs` and `extension/tool/schema.rs`, and reproduce the
non-empty-summary and unknown-id errors verbatim.

**Verify** — Create a mission, record one decision, resolve it, and assert `goal_driver`'s next ready
action moves past it; a `mission.resolve-decision` with an empty `summary` must fail with upstream's
message; one with an unknown decision id must fail rather than silently no-op.

**Relation to corpus** — Discharges one of the seven unowned verbs `SUBA-005` (tracker) explicitly
owes an owner for. `SUBA-005` proposes no schedulable work by its own reclassification, so this is
the first schedulable filing of the behaviour and is not a duplicate of a counted row.

---

## Carried — NOT adversarially verified

> **READ THIS BEFORE ACTING ON ANYTHING IN THIS SECTION.** The refutation pass for this batch was
> capped at twelve items. The eight items below were produced by the same analyst lenses as the
> confirmed set and are carried forward **unrefuted**. They are held to a lower evidence standard and
> **must not be counted alongside the verified items** — the same treatment `README.md` gives
> `DRIFT-023` / `DRIFT-040` as *leads*.
>
> **What this writer did personally verify:** the **port-side zero-hit greps** for every one of them,
> re-run against the current tree at HEAD `6db22a7`, plus the port line ranges quoted in
> `SUBA-089`, `SUBA-090` and `SUBA-091`. Those results are marked *(re-verified)* below.
> **What was NOT re-verified: every upstream line number in this section.** They are the analyst's,
> reproduced as filed, and a maintainer must settle each with `git show v0.57.0:<path>` before
> scheduling the item. Where a filing asserts an upstream shape, treat it as a hypothesis.

### SUBA-082 — Agent `acceptanceRole:` and `acceptance:` frontmatter are not in the schema, so the acceptance classifier is driven purely by the agent-name regex

**Severity** high (as filed) · **Effort** M · **Window** in-baseline (≤ v0.43.0) · `3c635cc1`

*Upstream, as filed (unverified):* `agents.ts:144-145` puts `defaultAcceptance?: AcceptanceInput` and
`acceptanceRole?: AcceptanceRole` on `AgentConfig`; `:1873-1884` `parseAgentAcceptanceFrontmatter`
YAML-parses `frontmatter.acceptance` and validates it; `:2011-2015` parses `acceptanceRole`, throwing
``Agent '<name>' has invalid acceptanceRole frontmatter; expected 'read-only' or 'writer'.``;
`agent-serializer.ts:24-25` lists both in `KNOWN_FIELDS`. The role is the PRIMARY input to the
acceptance-level classifier, which falls back to name matching only when it is `undefined`:
`input.acceptanceRole === "read-only" || (input.acceptanceRole === undefined && /\b(?:reviewer|oracle|scout|researcher|analyst)\b/.test(agent))`.

*Port (re-verified at HEAD):* `grep -rn 'acceptance_role' --include=*.rs crates/cyrup-ext-subagents/src`
→ **0 hits**. `src/discovery/frontmatter.rs:72-116 KNOWN_FIELDS` contains neither `acceptance` nor
`acceptanceRole`, so both are demoted to `extra_fields`. `AcceptanceResolveInput`
(`src/exec/acceptance/model/level.rs:43-51`) has `explicit`, `agent_name`, `task`, `mode`, `is_async`,
`dynamic`, `dynamic_group` — no role. The only `acceptanceRole` mentions in the crate are comments in
`level.rs` and `src/tests/read_only_agent_name_alternation.rs`, each quoting upstream's
`acceptanceRole === undefined` branch and noting the port implements only that branch.

*Behaviour gap:* an agent named `security-reviewer` that declares `acceptanceRole: writer` is still
gated read-only, and a writer-named agent declaring `read-only` is gated as a writer — a silently
wrong acceptance level with no error telling the author the key did nothing. Agent-level `acceptance:`
policy defaults are likewise unreachable; only the per-call `explicit` input exists.

*Relation:* distinct from `SUBA-081`'s settings-override half (`agentOverrides.<n>.acceptanceRole`) —
this is the frontmatter half, in a different parser. Land them together.

### SUBA-084 — Runtime agent registration is entirely absent: no `registerAgent` API, no `runtime` source tier, no runtime/configured collision checks

**Severity** high (as filed) · **Effort** L · **Window** v0.47.1..v0.57.0 · `2c031d06 (#1320)`

*Upstream, as filed (unverified):* `src/agents/runtime-agent-registry.ts` (418 lines) — the registry
key `"pi-subagents.runtime-agents.v1"`, per-runtime caps (200 agents, 128-char name, 4 KiB
description, 1 MiB systemPrompt, 8 KiB per field), a 32-field `RuntimeAgentDefinition`, a per-field
validator, alias normalization plus `assertNoIdentityCollisions` / `assertNoRuntimeCollision` /
`assertNoBuiltinCollision`, `toAgentConfig` stamping `source: "runtime"` and
`filePath: "runtime:<name>"`, `registerRuntimeAgent` returning an idempotent `dispose()`, and
`mergeRuntimeAgents`. It is a PUBLIC API re-exported by `src/api/agents.ts` as `registerAgent(input)`
and wired end to end: merged into every discovery, cleared on dispose, merged by the slash commands
so a runtime agent is slash-invocable, merged into `{action:"list"}`, and given its own precedence
rank above `project` in `agents.ts`.

*Port (re-verified at HEAD):*
`grep -rnE 'runtime_agent|RuntimeAgent|register_agent' --include=*.rs crates/cyrup-ext-subagents/src`
→ **0 hits**. `src/discovery/types.rs:43-51` defines `pub enum AgentSource { Builtin, Package, User, Project }`
— four variants, no `Runtime` — and `precedence_rank` at `:53-66` covers exactly those four.
`src/discovery/merge.rs:104-124 merge_tiers` takes a
`TieredAgents { builtin, package, user, project }` with no fifth tier and no in-memory registry input.

*Behaviour gap:* nothing an embedder registers in-process can ever be delegated to; an agent can only
exist as a file on disk.

*Relation:* **NOT** the same as `SUBA-022` (typed extension delegation API — upstream's
`executeDelegated`, a way to RUN a subagent, not to DEFINE one). Two lenses filed it independently;
merged here.

### SUBA-086 — Per-agent parse diagnostics are absent: a malformed agent file is silently degraded to defaults instead of being reported by name and blocking its own agent name

**Severity** high (as filed) · **Effort** M · **Window** v0.47.1..v0.57.0 · `e973fa3c`

*Upstream, as filed (unverified):* `agents.ts:229-234` `AgentDiscoveryDiagnostic { source, filePath,
error, name?, runtimeName?, packageSpecified?, discoveryPriority? }`; `:1923-2110` wraps the whole
per-file parse in `try { … } catch` and `:2106` pushes a diagnostic naming the file, the agent name
and the exact throw message — so *every* validation throw in the parse body (invalid `package`,
`async`, `timeoutMs`, `toolTimeoutMs`, `turnBudget`, `acceptance`, `outputMode`, `acceptanceRole`,
`fast`, `toolBudget`, both-permission-spellings, runner profile) becomes a SURFACED diagnostic rather
than a silent skip; `:2267-2273` returns them as `agentDiagnostics`; `:244-264`
`agentDefinitionPriority` + `findBlockingAgentDiagnostic` make a broken definition BLOCK resolution of
that name with its parse error instead of falling through to "Unknown agent";
`agent-management.ts:760-764` renders them under `Invalid agent definitions:` in `{action:"list"}`.

*Port (re-verified at HEAD):*
`grep -rnE 'AgentDiscoveryDiagnostic|agent_diagnostic' --include=*.rs crates/cyrup-ext-subagents/src`
→ **0 hits**. `src/discovery/mod.rs:1174-1191 AgentDiscoveryResult` carries
`diagnostics: Vec<ChainDiscoveryDiagnostic>` — **chain** diagnostics only. The port states the design
in-tree at `src/discovery/frontmatter.rs:753-757`: *"Never returns an `Err` for a malformed individual
agent file"* — `parse_agent_file` returns `Option<AgentDefinition>` and every field parser degrades a
bad value to `None`. `src/discovery/management/handlers.rs:109-113,142-150` filters and renders only
`ChainDiscoveryDiagnostic`; there is no `Invalid agent definitions:` section anywhere.

*Behaviour gap:* a typo (`timeoutMs: 30s`, `outputMode: file`, `defaultContext: forked`) is reported
upstream by name and file and blocks delegation to that name; in the port the bad field is silently
coerced to absent, the agent loads with default behaviour, and neither `list` nor the delegation path
ever mentions the file.

*Relation:* new. **Note for the planner: this is the highest-leverage item in the discovery cluster**,
because it converts `SUBA-081`, `SUBA-082` and every "key demoted to `extra_fields`" row in this batch
from silence into user-visible errors.

### SUBA-087 — Child-scoped stop (`childId`) is unported: `stop` can only terminate an entire async run and its whole descendant subtree

**Severity** medium (as filed) · **Effort** M · **Window** v0.47.1..v0.57.0 · `31a230cb (#1373)`, `de594cfd (#1375)`

*Upstream, as filed (unverified):* `src/runs/shared/child-identity.ts` (36 lines, new in the window) —
`asyncStatusChildIdentity(step, index)` = `step.workflowKey ?? step.runId ?? \`step:${index}\`` (the
`step:${index}` fallback is the NON-workflow case), `resolveAsyncStatusChild` returning
`{ok:false, code:"not_found"|"ambiguous"}`, `isStoppableAsyncStatusStep` restricted to
`pending`/`running`. `schemas.ts:278` advertises `childId`; `control-channel.ts:53-60` adds
`targetIndex?: number; childId?: string` to `StopRequest` plus a per-child `control/stop-requests/`
directory drained newest-last; `async-stop-action.ts:24-86` refuses a non-stoppable child with
``Child '<id>' in async run '<run>' is <status>; stop only supports pending or running children.``;
`subagent-runner.ts:2837-2887` flips only that step and emits
`subagent.step.stop_requested`/`stop_queued`/`stopped`.

*Port (re-verified at HEAD):* `grep -rn 'childId' crates/cyrup-ext-subagents/` → **0 hits** (the 27
`child_id` hits are unrelated test fixtures in `src/registration/cost.rs`).
`src/background/control.rs:624-651` `pub struct StopRequest { kind, ts, source, reason }` — no
`target_index`, no `child_id` — while the sibling `SteerRequest` at `:821-874` **does** carry
`pub target_index: Option<usize>`, so the per-child targeting machinery exists and stop simply does
not use it. `stop(async_root, results_dir, run_id_token, source, reason)` (`:691`) takes no child
argument; `grep -rn 'stop-requests\|stop_requests'` → 0 (the port has only the single
`control/stop.json`). `src/extension/tool/params.rs` has no `child_id` field and no
`deny_unknown_fields`, so a `childId` sent by an upstream-shaped caller is **silently discarded** by
serde and the stop proceeds against the whole run.

*Behaviour gap:* with a 5-wide fan-out running and one child gone bad, upstream stops that one and the
four healthy siblings run to completion. cyrup cannot express it: the entire run and its whole
descendant subtree are terminally stopped, and stopped runs are explicitly non-resumable, so the
siblings' partial work is lost.

*Relation:* deliberately **NOT** folded into `VL-S2` (`workflowScript`) despite the commit's title —
upstream's identity scheme falls back to `step:${index}` precisely for the non-workflow case, and the
port already has `RunStatus.steps` with per-step state plus per-child steer targeting, so this is
portable today with no workflow runtime. Three lenses filed it independently; merged.

### SUBA-088 — `subagents.defaultProvider` and per-agent `modelProvider` are unported, and the foreground launch path passes no preferred provider into candidate resolution at all

**Severity** medium (as filed) · **Effort** M · **Window** v0.47.1..v0.57.0 · `cc112354 (#1394)`

*Upstream, as filed (unverified):* `agents.ts:132` `modelProvider?: string` on `AgentConfig`, `:177`
`defaultProvider?: string` on `SubagentSettings`, `:86` `defaultProvider?: string | false` on
`BuiltinAgentOverrideConfig` (this one **is** confirmed — see `SUBA-081`'s verified 22-field list),
`:116` on `AgentModelSourceInfo`; `:997-1004` parses it; `:1045-1051` `resolveSubagentDefaultProvider`
(project beats user); `:1155-1168` `applySubagentDefaultModel` stamps `modelProvider` onto every agent
that has not pinned its own — including agents that already pin a MODEL. Consumed at
`execution.ts:1826-1831`:
`buildModelCandidates(options.modelOverride ?? agent.model, agent.fallbackModels, options.availableModels, agent.modelProvider ?? options.preferredModelProvider, {...})`,
whose 4th parameter drives `resolveRequiredSubagentModelCandidate`/`resolveSubagentModelCandidate`
(`model-fallback.ts:366-397`) — i.e. which provider a BARE model id resolves against.

*Port (re-verified at HEAD):*
`grep -rnE 'default_provider|defaultProvider|model_provider|modelProvider' --include=*.rs crates/cyrup-ext-subagents/src`
→ **0 hits** for every spelling. `src/discovery/types.rs:505-541 SubagentSettings` has no
`default_provider`; `AgentDefinition` has `model` and `model_source` but no provider field;
`src/discovery/merge.rs:223-250` applies only `defaultModel`, with no provider parameter.
`src/exec/fallback.rs:127-132 build_model_candidates(model_override, agent_primary_model, agent_fallback_models, available_models)`
has **no** provider parameter (nor does `build_model_candidates_scoped` at `:174-180`). The unrelated
`AgentConfig::preferred_provider` (`src/exec/agent_config.rs:349`) is `None` at every foreground call
site (`src/extension/executor/foreground.rs:361`, `src/background/runner_main.rs:2572`,
`src/exec/testsupport.rs:59`); only `src/extension/executor/reports.rs:182` populates it, and that is
the report surface, not the launch path.

*Behaviour gap:* `subagents.defaultProvider: "openai-codex"` has no effect — not parsed, not merged,
and no channel to reach candidate resolution. An agent naming a bare model id cannot be steered to a
particular provider, and a bare id that several providers offer resolves without the user's preference.

*Relation:* distinct from `SUBA-050` (`modelScope.strict`, an allowlist) and `SUBA-035` (surfacing the
scope policy) — this is provider *preference* feeding candidate resolution. Merges three lens
candidates sharing one fix site: `build_model_candidates`' signature plus the settings parse. Pairs
with `SUBA-081`'s `defaultProvider` override field.

### SUBA-089 — The model-fallback retry decision ignores whether the failed attempt already ran tools, so a half-completed mutating run is re-dispatched

**Severity** medium (as filed) · **Effort** S · **Window** v0.47.1..v0.57.0 · `d8d1408d`

*Upstream, as filed (unverified):* `src/runs/shared/model-fallback.ts:467-474`
`isRetryableModelFailureAttempt({error, messages, toolCount})` — retryable only if
`isRetryableModelFailure(error)` AND `(toolCount ?? 0) === 0` (`:469`
`if ((input.toolCount ?? 0) > 0) return false;`), with a further correlation requirement that the
error be the cold-start sentinel, or the run produced no messages, or some assistant message's own
`errorMessage` equals the run error (`:471-473`). `src/runs/foreground/execution.ts:2051` is the sole
foreground ladder gate, and `:2058` breaks the loop on `!retryableModelFailure`. At v0.43.0 and
v0.47.1 the same line was the bare `isRetryableModelFailure(result.error)`
(`v0.47.1:execution.ts:1633`), so the narrowing is new in the window.

*Port (re-verified at HEAD):* `crates/cyrup-ext-subagents/src/exec/fallback.rs:1265-1270` is the whole
retry gate —
```rust
if !is_retryable_model_failure(signal.error.as_deref()) {
    last_signal = Some(signal);
    last_attempt = Some(attempt);
    break 'ladder;
}
```
— the attempt's tool count and message set are never consulted.
`grep -rn 'is_retryable_model_failure_attempt' --include=*.rs crates/cyrup-ext-subagents/src` →
**0 hits**; `grep -n 'tool_count' src/exec/fallback.rs` shows the only uses are in `StartupEvidence`
and inside `is_retryable_subagent_startup_failure`, a different gate that fires before any model is
retried.

*Behaviour gap:* a foreground subagent that ran ten tool calls — edits, writes, git commands — and
then hit a transient `connection reset` / `overloaded` error is re-dispatched from scratch on the next
fallback model. Upstream stops the ladder because `toolCount > 0`, precisely so a half-completed
mutating run is not repeated. The port duplicates the child's side effects and doubles the token spend
on every mid-run provider blip.

*Relation:* new. This pass confirmed the rest of the fallback ladder (R-SA-036 ordering, retryable
patterns, attempt notes, usage aggregation, the startup-retry sub-ladder) present and correct — this
is a single missing predicate inside ported code.

### SUBA-090 — Completion notices are always rendered: the port hardcodes `display: true` where upstream hides a plain successful background completion and groups a batch

**Severity** medium (as filed) · **Effort** S · **Window** in-baseline (≤ v0.43.0)

*Upstream, as filed (unverified):* `src/runs/background/notify.ts:239` —
`const display = details.some((detail) => detail.source === "foreground" || detail.status !== "completed" || detail.scheduleOrigin !== undefined);`
then `:241-249` `pi.sendMessage({customType: "subagent-notify", content, display}, {triggerTurn: items.some((item) => item.triggerTurn)})`.
`v0.43.0:notify.ts:173` carries the same expression minus the `scheduleOrigin` clause, so it is
in-baseline. `:379` shows `triggerTurn: result.triggerTurn !== false` — per-completion, not a
constant. `:211-242` render a `Background tasks completed (N): …` header plus numbered blocks whenever
a batch holds more than one completion.

*Port (re-verified at HEAD):* `crates/cyrup-ext-subagents/src/background/watch.rs:741-746` —
```rust
CompletionMessage {
    custom_type: "subagent-notify".to_string(),
    content: lines.join("\n"),
    display: true,
    trigger_turn: true,
}
```
— both literals, with no branch on outcome or source anywhere in `format_completion_message`
(`:711-747`). **The struct's own doc at `:605-609` asserts** *"Always `true` (pi's `display: true`)"*
and *"Always `true` (pi's `{ triggerTurn: true }`)"* — a statement about upstream that
`notify.ts:239` contradicts at both tags. `grep -rn 'Background task'` shows only the singular header,
never the plural grouped form.

*Behaviour gap:* upstream injects a plain successful background completion as a NON-displayed context
message that still triggers a turn, rendering the notice only when something needs attention (a
foreground detach, or a failed/paused/stopped/scheduled outcome). A session that fans out ten
successful background tasks shows ten notices the port renders and upstream would have kept invisible,
and the grouped multi-completion form is never rendered at all.

*Relation:* new, and it **partially refutes `SUBA-017`'s framing**: `SUBA-017` (completion batching,
low) treats grouping as the missing piece, but the load-bearing half is the `display` predicate — a
two-line fix independent of the batcher. The port's own doc comment asserting upstream uses
`display: true` unconditionally is the reason no prior pass caught it.

### SUBA-091 — The fleet inspector passes an EMPTY trusted-root list to the transcript reader, so the session-transcript fallback always refuses

**Severity** medium (as filed) · **Effort** S · **Window** v0.47.1..v0.57.0 · `9ceb5650 (#1174)`

*Upstream, as filed (unverified):* `src/tui/fleet.ts`'s `asyncDetail(item, state)` calls
`formatAsyncRunTranscript(status, item.run.asyncDir, { index, lines: TRANSCRIPT_LINES, sessionRoots: uniquePaths([...(state.trustedSessionRoots ?? []), trackedJob?.sessionRoot]), trustedSessionFiles: [...], trustedSessionFileRoot: state.trustedSessionFileRoot })`.
`sessionRoots` is exactly what `readSessionTranscriptTail` confines its read to. Landed as
`9ceb5650 fix: pass trusted session roots to fleet transcripts (#1174)`; before it the call omitted
`sessionRoots`, which is the state the port is still in.

*Port (re-verified at HEAD):* `crates/cyrup-ext-subagents/src/tui/fleet.rs:842-848` —
`format_async_run_transcript(&run.status, &run.paths, step_index, Some(TRANSCRIPT_LINES as i64), &[])`
— the final `session_roots` argument is **a literal empty slice**, with no `[CYRUP-DELTA]` note in
`fleet.rs` justifying it. `src/background/fleet_view.rs:143-161 read_contained_text_tail` opens with
`if trusted_roots.is_empty() { return TextTail::failed(path, format!("Refusing to read {label} transcript path without a trusted root: {}", path.display())); }`,
and `read_session_transcript_tail` (`:618-632`) turns that into a `Warnings:` line.
`grep -rn 'trusted_session_roots\|trusted_session_file' --include=*.rs` → 0, so no plumbed equivalent
exists on the fleet side — **but the resolver already exists for the other consumer**:
`src/extension/executor/status.rs:389-396 transcript_session_roots(cwd)` builds
`[default_async_root, project_subagents_dir, temp_artifacts_dir]` and passes it at `:359`.

*Behaviour gap:* when a background run has no readable output log — its `output-*.log` is missing, or
the run only ever recorded a session file — upstream falls back to the child's session-JSONL tail and
shows it in the fleet inspector's detail pane. The port instead emits a `Warnings:` block saying it
refuses to read the path for lack of a trusted root, then shows
`(no transcript output captured yet)` — on a run whose transcript is on disk and which the port's own
`subagent status` path reads fine.

*Relation:* new. Same file as `SUBA-080` (refuted) but a different defect — argument value, not
missing sanitization. The fix is one call-site change reusing `status.rs:389`'s existing resolver.

---

## Refuted

Recorded so it is never re-derived.

### SUBA-080 — REFUTED — "Fleet-view run transcripts are rendered without terminal-control sanitization"

**The upstream evidence was accurate.** `git show v0.57.0:src/runs/background/fleet-view.ts` does
define `safeTranscriptLines` and apply it at four sites; `run-status.ts` applies
`lines.map(safeTerminalText)` at the end of `formatRememberedForegroundTranscript`;
`shared/display-text.ts:139` is `safeTerminalText` with the described code-point classes.
**The port-side claim is wrong: neutralization is provided by the host layer.**

- **TUI fleet-detail path.** `crates/cyrup-ext-subagents/src/tui/fleet.rs:837 async_detail` →
  `src/tui/fleet_overlay.rs` (`cyrup_ext::OverlayLine`) → `crates/cyrup-tui/src/overlay.rs:167,195`
  `to_ratatui_line`/`to_ratatui_span` → painted by ratatui `Paragraph`/`frame.render_widget`.
  ratatui-core 0.1.2 filters every control-character grapheme at **both** paint points:
  `…/ratatui-core-0.1.2/src/text/span.rs:314` `.filter(|g| !g.contains(char::is_control))`
  (`Span::styled_graphemes`) and `…/src/buffer/buffer.rs:351`, the same filter in
  `Buffer::set_stringn`.
- **`subagent` status-tool path.** `src/extension/executor/status.rs:354` returns the string as a
  tool result, rendered through `crates/cyrup-tui/src/transcript/tool_result.rs:61,68,90,94,99`
  `result_text`, every branch of which calls `crate::ansi::sanitize_display_text` —
  `crates/cyrup-tui/src/ansi.rs:25` `sanitize_binary_output(strip_ansi(text)).replace('\r', "")`,
  where `strip_ansi` (`:60`) consumes whole CSI/OSC sequences and `sanitize_binary_output` (`:36`)
  drops every C0 except TAB/LF/CR plus U+FFF9..=U+FFFB.
- `crates/cyrup-tui/src/ansi.rs:7-18` states the contract explicitly: *"ratatui filters control
  characters out of every grapheme run before it reaches a cell … so a bare `ESC` can never be written
  to the terminal and an escape sequence cannot **execute** — no cursor moves, no title rewrite, no
  hidden text."*
- An in-crate sanitizer exists as well: `src/tui/fleet_transcript.rs:88,123,152`
  (`BINARY_CONTENT_PLACEHOLDER`, `looks_like_binary_content`, `safe_display_text`), `:553`
  `safe_transcript_event`, with tests at `:1835-1864` and `:2106`. In `wrapped_detail`
  (`src/tui/fleet.rs:1803-1856`) the sanitized structured-transcript renderer is the **preferred**
  branch; `detail_lines`/`async_detail` is only the fallback when the child has no readable transcript
  events.
- Workspace-wide check for a second sanitizer:
  `grep -rnE "is_unsafe_display_code_point|safe_terminal_text|binary content omitted|fn safe_display_text" --include=*.rs crates/ | grep -v cyrup-ext-subagents`
  → 0 hits — i.e. the neutralization is not a duplicated in-crate copy, it is the host's.

**Verdict:** a different shape achieving the same observable behaviour. Not a gap. Note that
`SUBA-091` is a *different* defect in the same function and remains open.

---

## Already tracked

Dropped rather than filed, with the item that owns each. Recorded in full so the next pass does not
re-derive them.

| Candidate | Owner | Why dropped |
|---|---|---|
| 23 of upstream's 52 tool verbs unadvertised (port advertises 30) | `SUBA-005` (tracker) | This is the census, not the work, and `SUBA-005` already owns exactly it and names the same unowned verbs. Its re-measured figures (**30 vs 52**; `+validate`/`+debug.run`, `−append-step`/`−approve-checkpoint`/`−reject-checkpoint` via `7ece6f35`) belong in `SUBA-005`'s body as a maintenance update. The one verb with real wedging behaviour behind it is filed as `SUBA-085`. |
| `schedule.*` (nine verbs) unported | `SUBA-016` / PARITY-GAPS `PB-11` | Fully covered, including the corrected nine-verb count and the BLOCKED-on-`workflowScript` determination; the port states it at `background/control.rs:367`. |
| `refine` / `refine.show` / `refine.rollback` + `/subagents-refine` | `VL-S13` (+ `VL-S11`) | Covered as the agent-refinement WRITE half; `exec/agent_refinements.rs:12-20` documents the split and the read half is live on the production spawn path. |
| Herdr subsystem: six inspector/project verbs, focus, `/subagents-inspect-rpc` | `VL-S6` (+ `PB-8`) | Covered. The port carries a written decision at `tui/fleet.rs:58-60` with an implemented and tested fallback; the RPC half belongs to the unported RPC bridge. |
| `/subagents-detach` unregistered; no configurable detach shortcut | `VL-S11` + `VL-S15` | The command is in `VL-S11`'s three-command list; the shortcut half is the host-seam gap `VL-S15`, not a subagents defect. |
| `workflowScript` runtime, `chatProgress`, `workflowScriptPath`, `action:"validate"`, mission workflow state | `VL-S2` | Covered; the port documents the absence at ~29 sites. **Worth recording in `VL-S2`'s body: the file TRIPLED inside the window (502 / 703 / 1522 lines at v0.43.0 / v0.47.1 / v0.57.0).** |
| Durable workflow receipts, `workflowChildren` summaries, one-use child permit, detach-reconcile | `VL-S2` | A genuinely new layer (four files, all zero bytes at **both** v0.43.0 and v0.47.1), but every one is unreachable without the `workflowScript` runtime and a Workflow RunMode the port cannot represent. A scope-growth note in `VL-S2`, not four rows. The one piece independent of the runtime — `childId` stop — is kept as `SUBA-087`. |
| `children.list` unadvertised | `SUBA-055` closure note / `SUBA-005` | The port declines it with a written, correct reason at `extension/tool/text.rs:190-197`: it lists retained workflow children, so it would advertise a permanently-empty listing. |
| `agentContract` at run level and on every child schema (and `gate`/`gateOn`) | `SUBA-024` / `VL-S10` | Covered; the port names it at `spawn/chain_graph.rs:1859`. Upstream defines `gateOn` as applying only "for chain steps with `agentContract`", so it is the same item. |
| `worktree.discard` + `handoffPath`; parallel-handoff manifest never written | `SUBA-024` / `VL-S10` / `SUBA-005` / `SUBA-064` | The manifest half is the parallel-handoff item; the verb is on `SUBA-005`'s unowned list; and `SUBA-064` already records the prerequisite that whoever lands `worktree.discard` lands the authority gate in the same change. |
| `authorityPolicy` validators exist but the production loader never calls them | `SUBA-064` | Still open, and its Fix already prescribes wiring `validate_authority_policy` into config load beside `validate_missions`. This refines `SUBA-064`'s evidence (the subsystem has since landed; only the loader call is missing) rather than adding a behaviour it does not own. |
| `subagent_wait` missing `nonBlocking`, `stopOnAttention`, auto-drain; no durable wake subscription | `VL-S8` | The non-blocking-subscription and auto-drain half is `VL-S8` verbatim; `stopOnAttention` is a one-flag window addition on the same schema and belongs in that item's body. |
| Durable completion replay / output archives / wait-completion payloads | `SUBA-056` | Covered in full, including the archive and replay-record shapes. |
| Async status snapshots + on-demand child inspection over RPC | `PB-8` | Downstream of the entirely-unported RPC bridge; adds nothing schedulable until `PB-8` lands. |
| Session lease / process-terminal record / owned-process-tree verification | `VL-S3` / `VL-S4` / `SUBA-023` | Already filed on both sides. Additionally established: **the port's KILL path is stronger than upstream's** (process-group negation at `spawn/signal.rs:503-510` vs upstream's single-pid kills); only the terminal-proof record is missing, which is what the existing rows name. |
| Four ignored config keys (`asyncWidget`, `inlineToolDisplay`, `fleetKeybindings`, `legacyChainControls`) + `mainWindowRenderer` | `SUBA-061` | The four are `SUBA-061` verbatim; `mainWindowRenderer` is the same shape and belongs in that list as a fifth key. |
| 21 of 44 `ExtensionConfig` keys unmodelled (bulk census) | `SUBA-061` + the rows above | A census, not a behaviour. Its high-value members are filed individually here (`permissions` → `SUBA-073`, `timeoutMs` → `SUBA-077`, `maxThinking` → `SUBA-078`, `defaultProvider` → `SUBA-088`, `defaultSubagentContext` → `SUBA-079`) or already owned; filing the census too would double-count every one. |

### Dropped as justified divergences or non-gaps

- **`maxSubagentSpawnsPerSession` defaults to 40 where upstream is unlimited** — the field's doc at
  `registration/mod.rs:89-91` cites `func-SA §4.7` as the cyrup requirement setting the default: a
  decision of record. (Contrast `async_by_default` two lines above, which carries no such citation —
  filed as `SUBA-083`.)
- **`PI_SUBAGENT_FS_RETRY_MAX_TOTAL_MS` clamp** — not a gap. Upstream's knob exists because its
  writers are synchronous and `Atomics.wait` parks the Node event loop; the port's writers are async
  (`background/atomic.rs:75`) and never block a shared loop, so there is no behaviour to reproduce.
- **`/prompt-workflow` and `/chain-prompts` reimplemented over `workflowScript` upstream** — not a
  gap. The port reaches the same observable behaviour through the chain shape: same three-tier
  discovery, same eight reserved names, same `' -> '` / `' -- '` splitting, same `{previous}`
  threading, same two error strings (`registration/prompt_workflows.rs`).
- **`--no-context-files` not passed to the child** — not a gap. `src/prompt_runtime.rs`'s
  `BeforeAgentStart` hook (`:1966-1981`) runs `strip_project_context` (`:850-852`) over the child's
  assembled system prompt, removing the project-context block the child itself loaded. Same
  observable result by a different mechanism.
- **`restoreActiveJobs` / restart resumption of in-flight runs** — **not a gap; present under a
  different name.** `src/extension/executor/status.rs:27 resume_tracking`, wired at
  `extension/host/native_impl.rs:347` on `SessionStart`, pinned by `executor/paths.rs:566-630`, which
  asserts both upstream behaviours (terminal runs not re-tracked; restored events cursor seeded at
  EOF). **This is exactly the restructure trap the methodology warns about** — the behaviour reads as
  absent under every name upstream uses.
- **`alignForkedSessionCwd`** — absence could not be established. The port passes the child's cwd into
  `SessionLayout::new(root, cwd)` before branching (`extension/executor/resolve.rs:316-325` →
  `fork_context.rs:190`), so the header may already carry the correct cwd structurally. Dropped rather
  than softened, per the evidence rules.

### Cut at the twenty-item cap — confirmed absent, file next pass

Each was confirmed absent and each was cut on ratio, not on doubt. Recorded so the next pass does not
spend the search again.

- **Per-run logical fan-out budget** (`maxSubagentSpawnsPerRun`, hard cap 64) — zero hits for
  `run_fanout`/`fanout_budget`/`spawns_per_run`. The port does bound a run tree by depth
  (`spawn/depth.rs`, enforced at `runner_main.rs:1219-1241`) and by each step's own width, so the
  exposure is cost, not correctness. M–L.
- **Per-session active async-run capacity** (`maxActiveAsyncRunsPerSession` +
  `capacity.abandonedSlotRelease`) — L: a file-backed slot pool with process-liveness reclamation, for
  a resource-accounting behaviour with no correctness consequence.
- **`toolTimeoutMs` at every level** (call param, frontmatter, config, env) and the fast-tool defaults
  — zero hits crate-wide for every spelling. **Partly subsumed:** once `SUBA-077` restores the
  foreground run-level deadline, a wedged tool call is bounded by the run rather than unbounded.
  Re-file after `SUBA-077` lands, when the residual is per-call granularity alone.
- **Agent-level `outputMode` default never consulted** — the port states it in-tree at
  `frontmatter.rs:730-744`. Same fix session as `SUBA-081`'s `agentOverrides.<n>.outputMode`; land
  the two together.
- **Context-overflow classification** (`contextOverflow` flag + terminal note) — zero hits for
  `context_overflow`; S effort, but the consequence is a less actionable error message.
- **TTL model-exclusion store for fallback candidates** (`modelExclusions`) — M–L, a latency and quota
  optimisation with no correctness consequence.
- **Async retention sweep (30-day) and the active-run / terminal-run / result index layer** — these
  compound (an unswept async root makes the full-directory rescan in `run_status.rs:592-655`
  progressively slower), but both are performance and disk growth, both L.
- **Live context-window usage** (`window` / `windowPeak` across `TokenUsage`, progress, formatters and
  every status surface) — zero hits for `window_peak`; `format_fleet_tokens` is single-argument and
  its tests pin the old string. Observability, M across four surfaces.
- **Prompt Audit drawer; external jobs in FleetView; active task labels; async capacity in the status
  line; fast mode; pruned fork context; `extensionBindings`; structured-output acceptance capture;
  launch-contract preflight; capability audit; `debug.run`** — each confirmed absent, each cut on
  ratio: large-effort UI subsystems, dependents of other unported subsystems (external jobs →
  `SUBA-074`), or low-value diagnostics.

---

## Corpus health

Five things a maintainer should fix in the ledger before the next pass.

**(1) The corpus does not end at `SUBA-066` — it ends at `SUBA-071`.**
`09-cyrup-ext-subagents.md` carries `SUBA-067`…`SUBA-071` (three test-defects filed
`Status FIXED`/`OPEN`, plus `SUBA-070` and a REFUTED `SUBA-071`). A "start at `SUBA-067`" instruction
would have collided with five live ids. **This batch starts at `SUBA-072`.**

**(2) The README baseline table is a full major-version stale for this upstream.** It records
`pi-subagents` latest as **v0.47.1** with a delta of "151 files, +10,254 / −1,333". The tag is
**v0.57.0** and the unmeasured window is the table in `## Scope` above. Area 09's own header also
states that every claim was settled at v0.43.0 or v0.47.1. Update both, and re-read PARITY-GAPS §1d.

**(3) Three high-traffic rows now carry evidence that is factually wrong at HEAD, and one would cause
harm if followed.**
- `SUBA-021` / `VL-S1` says `rg 'capability_ceiling' = 0` and "no ceiling concept". The subsystem
  landed in sweep 10; the residual defect is *worse* than the one filed (`SUBA-072`).
- `VL-S14` rates `runner: external-cli` **medium** / "unsupported". The key is neither rejected nor
  applied, which is a capability widening, and the subsystem tripled and gained a second runner type
  inside the window (`SUBA-074`).
- **`SUBA-051`'s Fix line instructs *"Do not apply it to foreground runs, which already have their
  own default"* — the foreground path has no default at all** (`extension/tool/params.rs:264-280`),
  so following that instruction leaves the foreground unbounded permanently (`SUBA-077`).

This is the third edition's *"a true line number carrying an untrue claim"* class, and it is now the
dominant failure mode in this area's ledger.

**(4) The restructure trap is real and it cuts both ways.** `src/extension.rs` no longer exists, so
every `extension.rs:NNNN` citation in area 09 is **unresolvable**, not merely stale. The more
dangerous direction is the false negative: `restoreActiveJobs` reads as absent under every name
upstream uses and is fully present as `resume_tracking`, with a test pinning both of its subtleties.
Every absence claim in this batch was established by grepping the current tree for the behaviour by
identifier **and** by concept, in both camelCase and snake_case, plus env-var names — never by
resolving a cited path. Adopt that as the standing rule for this area.

**(5) Two in-source comments assert things about upstream that upstream contradicts, and both hid a
defect.**
- `background/watch.rs:605-609` says pi uses `display: true` unconditionally; `notify.ts:239`
  computes it (`SUBA-090`).
- `discovery/types.rs:411-414` says `AgentOverrideConfig` is *"a field-for-field port … and pi has no
  others"* while pi had four more at the measured baseline and nine more at v0.57.0 (`SUBA-081`).

**A completeness claim written in a doc comment is not evidence, and neither a citation audit nor a
compile catches it.** Add both to the known-traps list, and prefer a checked-in pinned copy of the
upstream field list plus an assertion over a prose claim.

### One note in the ledger's favour

The lenses independently confirmed large ported subsystems **complete and correct**: the acceptance
tree (~10,140 lines, nine evidence kinds, `stopRules`, verify memoization, workspace fingerprinting —
`SUBA-076` is a defect *inside* it, not a hole in it), nested events (1,992 lines plus the child
control inbox), MCP direct tools (2,816 lines including the header cache-identity fix), the fallback
ladder's R-SA-036 ordering, the turn / tool / usage / spawn budgets, agent memory, model scope, and
the four-tier discovery merge with its deliberately asymmetric same-tier rule.

**The remaining distance in this crate is concentrated in three places**, and a planner should read
the twenty items above through that partition:
1. **The parent side of policy surfaces whose child side is already implemented** — `SUBA-072`
   (capability ceiling), `SUBA-073` (permissions). Both are "the enforcement machinery is ported and
   permanently unreachable", and both are small relative to what they unlock.
2. **The agent-definition schema's missing keys** — `SUBA-074`, `SUBA-081`, `SUBA-082`, `SUBA-088`,
   with `SUBA-086` as the amplifier that converts all of them from silence into user-visible errors.
   **Land `SUBA-086` first.**
3. **The external-runner / `workflowScript` execution model** — `SUBA-074` stage 2, `VL-S2` and its
   dependents. This is the genuinely large remainder and the only part that needs design.
