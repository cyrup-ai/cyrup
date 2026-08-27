---
stage: new
status: done
updated: 2026-08-27 05:30
severity: critical
effort: medium
subsystem: exec / tool allowlisting
source: docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md
item: SUBA-072
---

> **Path note.** This task lives in a subdirectory. Every flux command globs a single level (`ls -1 "$FLUX_BASE/todo/"*.md`), so `/exec`, `/aug` and `/qa` will not list it — pass the absolute path explicitly.

# SUBA-072 — The capability ceiling's `allowedTools` and `denyExtensions` axes are resolved, intersected and propagated but never applied to the spawned child

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

## Scope

In scope: the behaviour described above, in `crates/cyrup-ext-subagents/`.

Out of scope: the other SUBA items in this batch (each has its own file); any refactor beyond what the fix needs; and the ledger corrections in `SUBA-CORPUS-HEALTH.md`.

Full finding, with the complete evidence chain: [`09a-cyrup-ext-subagents-v0.57-drift.md` § SUBA-072](../../../docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md)

## Approach

1. In `src/exec/spawn_plan.rs`, consume the ceiling already resolved at `:309` instead of only
   asserting the agents axis at `:313`.
2. Build `allowed_tool_set` from the ceiling and apply all four upstream narrowings from
   `pi-args.ts:423-476`: reject when `require_read_tool` is set and `read` is absent; intersect the
   declared and undeclared tool arms against the set; empty `tool_extension_paths` and
   `resolved_mcp_selections` under `deny_extensions`, and filter surviving MCP selections through
   the set; force `disable_ambient_extensions`.
3. Change `let explicit_tool_allowlist = agent.tools.is_some();` (`:397`) to also be true whenever a
   ceiling set exists, so a ceilinged child always gets `--tools <set>` or `--no-tools`, and push
   `--no-extensions` under `disable_ambient_extensions`.
4. Delete the now-false comment at `spawn_plan.rs:417-420` claiming cyrup has no capability ceiling.

## Acceptance Criteria

- [ ] `grep -rn 'allowed_tools\|deny_extensions' --include='*.rs' crates/cyrup-ext-subagents/src` shows consumers in `exec/spawn_plan.rs`, not only `capability_ceiling.rs`
- [ ] A ceiling `{allowedTools:["read"], denyExtensions:true}` spawns an agent declaring `tools: [read, write, bash]` with `--tools read` and `--no-extensions`
- [ ] The same ceiling spawns an agent declaring no `tools:` with `--tools read`, not the full ambient set
- [ ] A ceiling requiring `read` for lazy skill loading errors when the set lacks `read`
- [ ] The stale comment at `spawn_plan.rs:417-420` is gone
- [ ] `cargo test -p cyrup-ext-subagents` passes
