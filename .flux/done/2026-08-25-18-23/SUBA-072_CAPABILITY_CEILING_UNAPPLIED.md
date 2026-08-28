---
stage: qa
status: completed
updated: 2026-08-27 02:45
severity: critical
effort: medium
subsystem: exec / tool allowlisting
source: docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md
item: SUBA-072
---

# SUBA-072 — The capability ceiling's `allowedTools` and `denyExtensions` axes are resolved, intersected and propagated but never applied to the spawned child

## QA verdict: 6/10 — needs rework

**What's done, verified in production quality (do not re-derive):** the original Fix items (a)-(d) —
`explicit_tool_allowlist` including the ceiling term, both arms of the ceiling-aware declared-tools
computation (with the `read`-injection interaction correctly suppressed under a ceiling), the
`denyExtensions` extension-stripping block, and the `requireReadTool` throw — are all present in
`crates/cyrup-ext-subagents/src/exec/spawn_plan.rs` and pass their three dedicated tests plus the
pre-existing `the_capability_ceiling_refuses_an_out_of_ceiling_agent_and_reaches_the_child_env` test.
Verified directly against source (no `git` used, per this review's constraints):
`grep -rn 'allowed_tools\|deny_extensions' --include='*.rs' crates/cyrup-ext-subagents/src` shows real
consumers in `spawn_plan.rs` (lines 320-549), not only `capability_ceiling.rs`; the stale
"cyrup has no capability ceiling" comment is gone; `cargo test -p cyrup-ext-subagents --lib` is
**2,486 passed / 0 failed**, and `exec::spawn_plan::tests::*` alone is 74/74.

**What's still missing — two residual gaps in the SAME investigation, unaddressed since the prior
`/aug` pass documented them** (confirmed by reading current source directly: `spawn_plan.rs:397` and
`spawn_plan.rs:791` are byte-identical to their pre-`/aug` state):

1. **`MCP_DIRECT_TOOLS` env var** (`spawn_plan.rs:789-797`) still writes the agent's raw,
   ceiling-unfiltered MCP selector list unconditionally — a second consumer of the exact same ceiling
   data the (a)-(d) fix patched one consumer of, read by the separate `cyrup-mcp` crate to decide
   which MCP servers/tools the child activates. Under `denyExtensions` or a narrowing `allowedTools`,
   the ceiling does not reach this write at all.
2. **`fanout_authorized`** (`spawn_plan.rs:397-399`) is still computed from the pre-ceiling
   `builtin_tools`, not the ceiling-filtered declared-tools list — so a ceiling that excludes
   `subagent` from `allowedTools` fails to revoke the child's nested-delegation authorization
   (`FANOUT_CHILD_ENV`, and the route/parent-address coordinates it gates). This is the actively
   dangerous direction: the ceiling silently permits a capability (spawning further subagents) it was
   set up to deny — the exact class of bug this task item exists to close, on a fourth site.

Neither gap is "how small" — both are squarely inside this item's own title ("...propagated but never
applied to the spawned child") and both were reachable by the same investigation that produced the
(a)-(d) fix; closing only four of six real call sites is not yet done. Full evidence chain, exact
current-code citations, exact upstream (`pi-args.ts`) citations confirmed against the vendored source,
and ready-to-implement Rust snippets for both are below, carried over unchanged from the prior `/aug`
pass since neither has been touched.

---

## Background (for context; the mechanism below is already correctly implemented)

**upstream** — `git show v0.57.0:src/runs/shared/pi-args.ts`. `resolvePiLaunchToolPlan` at **`:423`**
intersects the call-site and inherited ceilings and builds `allowedToolSet` at **`:430-433`**. The
resolved ceiling then drives **four** independent narrowings, all four now correctly ported:
- **`:439-441`** the `requireReadTool` throw.
- **`:444-455`** `declaredBuiltinTools`'s two arms.
- **`:457-463`**/**`:514`**/**`:527`** `toolExtensionPaths`/`disableAmbientExtensions`/`configuredExtensions` under `denyExtensions`.
- **`:473-476`** `explicitToolAllowlist`'s ceiling term, driving `--tools`/`--no-tools`/`--no-extensions` emission.

**Relation to corpus** — REVISION of `SUBA-021` / PARITY-GAPS `VL-S1`; see the full finding at
[`09a-cyrup-ext-subagents-v0.57-drift.md` § SUBA-072](../../docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md).

## Scope

In scope: the two remaining gaps below, in `crates/cyrup-ext-subagents/src/exec/spawn_plan.rs` (and,
for Gap 1, a possible small addition to `crates/cyrup-ext-subagents/src/exec/mcp_direct_tools.rs` if a
helper is worth extracting — not required, the snippet below works inline).

Out of scope: the other SUBA items in this batch (each has its own file, including `SUBA-073` on the
same file); any refactor beyond what the fix needs; the ledger corrections in `SUBA-CORPUS-HEALTH.md`.

---

## Gap 1 — `MCP_DIRECT_TOOLS` env var still carries the raw, ceiling-unfiltered selector list

Current code, unchanged (`spawn_plan.rs:789-797`):

```rust
env_overlay.insert(
    MCP_DIRECT_TOOLS_ENV.to_string(),
    if mcp_direct_tools.is_empty() {
        MCP_DIRECT_TOOLS_NONE_SENTINEL.to_string()
    } else {
        mcp_direct_tools.join(",")
    },
);
```

`mcp_direct_tools` is the RAW list of `mcp:`-stripped selector strings the agent declared
(`:378-386`, before any ceiling logic runs) — independent of `effective_mcp_tools` (the ceiling-aware,
resolved-names list that already correctly feeds `--tools` and `MCP_DIRECT_CHILD_TOOLS_ENV` at
`:489-501` and `:996-1000`). This write ignores `ceiling_allowed_tools`/`ceiling_deny_extensions`
entirely.

`MCP_DIRECT_TOOLS` (no `_CHILD_` — `cyrup-mcp/src/registration.rs:154`, `DIRECT_TOOLS_ENV_VAR`) is read
by the separate `cyrup-mcp` crate's `register_surface` (`registration.rs:2089-2101`) to decide which
MCP servers/tools the child's own MCP adapter activates as direct-tool overrides — independent of the
host's `--tools` CSV. A `denyExtensions` ceiling (an MCP server is extension-provided, per this
crate's own comment at `spawn_plan.rs:494`) or a narrow `allowedTools` set therefore does not stop the
child's MCP subsystem from activating the agent's originally-requested direct MCP tools.

Upstream's exact three-way branch, confirmed against the vendored source
(`tmp/pi-subagents/src/runs/shared/pi-args.ts:916-926`):

```js
if (!toolPlan.capabilityCeiling && input.mcpDirectTools?.length)
    env.MCP_DIRECT_TOOLS = input.mcpDirectTools.join(",");
else if (
    toolPlan.capabilityCeiling &&
    toolPlan.effectiveMcpSelections.length &&
    !toolPlan.capabilityCeiling.denyExtensions
)
    env.MCP_DIRECT_TOOLS = toolPlan.effectiveMcpSelections
        .map((selection) => selection.selector)
        .join(",");
else env.MCP_DIRECT_TOOLS = "__none__";
```

(i) no ceiling at all → raw join, unfiltered; (ii) a ceiling present, not denying extensions, with at
least one MCP selection surviving the `allowedTools` filter → the FILTERED selectors, joined; (iii)
otherwise → the `__none__` sentinel.

**Wrinkle**: pi's `effectiveMcpSelections` carries `{selector, name}` pairs — filtered on `.name`, but
the env re-joins `.selector` (one selector, e.g. a whole-server selector, can expand to many names).
cyrup's `effective_mcp_tools` (`exec::mcp_direct_tools::resolve_mcp_direct_tool_names`) already
discards that mapping, returning only a flat `Vec<String>` of expanded names
(`mcp_direct_tools.rs:407-429,589-668`) — reusing it verbatim for `MCP_DIRECT_TOOLS_ENV` is wrong,
since that env var's one consumer parses SELECTOR syntax (`<server>` / `<server>/<tool>`), not
resolved names.

**Fix** — replace `spawn_plan.rs:789-797` with the three-way, selector-preserving, ceiling-aware
version (no changes needed to `mcp_direct_tools.rs`'s public surface — call
`resolve_mcp_direct_tool_names` once per raw selector to test whether that selector's own expansion
intersects `ceiling_allowed_tools`):

```rust
// SUBA-072 / pi `pi-args.ts:916-926`: MCP_DIRECT_TOOLS (no `_CHILD_` — the raw selector list
// `cyrup-mcp::registration::register_surface` reads to decide direct-tool overrides) must obey
// the SAME ceiling axes as `effective_mcp_tools` above, but on SELECTOR strings, not resolved
// names — a selector survives iff at least one of the tool names it expands to is still allowed.
let ceiling_filtered_selectors: Vec<String> = if ceiling_deny_extensions {
    Vec::new()
} else if let Some(allowed) = ceiling_allowed_tools.as_ref() {
    mcp_direct_tools
        .iter()
        .filter(|selector| {
            mcp_direct_tools::resolve_mcp_direct_tool_names(
                std::slice::from_ref(*selector),
                &opts.cwd,
            )
            .iter()
            .any(|name| allowed.contains(name))
        })
        .cloned()
        .collect()
} else {
    mcp_direct_tools.clone()
};
env_overlay.insert(
    MCP_DIRECT_TOOLS_ENV.to_string(),
    if ceiling_filtered_selectors.is_empty() {
        MCP_DIRECT_TOOLS_NONE_SENTINEL.to_string()
    } else {
        ceiling_filtered_selectors.join(",")
    },
);
```

`mcp_direct_tools` and `opts` are both already in scope at that point; no new parameters needed.

**New tests** (same `mod tests` in `spawn_plan.rs`, alongside the three existing SUBA-072 tests):
register a ceiling with `allowedTools` that includes some but not all of an agent's declared MCP
selector names, assert `env_overlay[MCP_DIRECT_TOOLS_ENV]` contains only the surviving selector(s);
register a `denyExtensions: true` ceiling with the agent declaring `mcp:` tools, assert
`env_overlay[MCP_DIRECT_TOOLS_ENV] == "__none__"`.

---

## Gap 2 — `fanout_authorized` is computed from the PRE-ceiling tool list, not the ceiling-filtered one

Current code, unchanged (`spawn_plan.rs:397-399`, runs BEFORE the ceiling-aware declared-tools
computation that starts at `:435`):

```rust
let fanout_authorized = builtin_tools
    .iter()
    .any(|tool| tool == crate::extension::TOOL_NAME);
```

`builtin_tools` is the RAW per-agent declared builtin list (`:379-387`), never touched by the ceiling
filter that runs later in the same function. `fanout_authorized` gates
`crate::spawn::nested_events::child_role_env` (`:742-743`) — whether the child may register the
restricted `ChildSafe` `subagent` tool and delegate to its own subagents, and (per that function's own
doc) whether it receives real nested-route/parent-address coordinates at all. Real security gate, not
cosmetic.

Upstream: `const fanoutAuthorized = declaredBuiltinTools.includes("subagent");` — computed from
`declaredBuiltinTools`, the SAME ceiling-filtered value that later feeds `effectiveToolAllowlist`, not
from the agent's raw `input.tools`. Two concrete divergences:

1. **A ceiling excluding `subagent` from `allowedTools` fails to revoke fanout authorization** — an
   agent declaring `tools: [subagent, read]` under `{allowedTools: ["read"]}` gets `--tools read`
   (correctly narrowed) but `fanout_authorized` still evaluates `true` from the untouched
   `builtin_tools`. Upstream's filtered `declaredBuiltinTools` does not contain `"subagent"`, so
   `fanoutAuthorized` is `false`. **This is the dangerous direction** — the ceiling silently fails to
   narrow a real delegation-authorization bit.
2. **A ceiling granting `subagent` via `allowedTools` on an agent with no `tools:` of its own fails to
   authorize fanout** (completeness gap, not a security one) — pi's `input.tools === undefined` arm
   sets `declaredBuiltinTools = [...allowedToolSet]`, so `fanoutAuthorized` is `true` if the ceiling's
   `allowedTools` includes `"subagent"`, even though the agent declared nothing. cyrup's
   `builtin_tools` stays empty in that arm, so `fanout_authorized` stays `false` regardless.

**Fix** — hoist the ceiling-aware "effective declared builtin tools" computation (currently inlined
inside `if agent.tools.is_some() { ... } else { ... }` at `:435-481`, i.e. what becomes `allowlist`'s
starting value) so it runs UNCONDITIONALLY, mirroring pi's own unconditional `declaredBuiltinTools`
ternary — computed before `explicitToolAllowlist` is even checked, not only inside
`if explicit_tool_allowlist { ... }`:

1. Extract the existing `if agent.tools.is_some() { ...; declared } else { ceiling_allowed_tools.clone().unwrap_or_default() }` expression into its own `let effective_builtin_tools: Vec<String> = ...;` binding, computed unconditionally at that point in the function. (When `agent.tools.is_none() && ceiling_allowed_tools.is_none()`, this still evaluates to `Vec::new()` — identical to today's `builtin_tools` in that case, so no other call site's behavior changes.)
2. Move `let fanout_authorized = ...` (currently `:397-399`) to run AFTER this new binding, deriving it from `effective_builtin_tools` instead of `builtin_tools`: `let fanout_authorized = effective_builtin_tools.iter().any(|tool| tool == crate::extension::TOOL_NAME);`
3. Inside `if explicit_tool_allowlist { ... }`, replace the `let mut allowlist = if agent.tools.is_some() { ... } else { ... };` computation with `let mut allowlist = effective_builtin_tools.clone();` — the MCP-appending and `--tools`/`--no-tools` logic below it is otherwise unchanged.
4. Update the now-inaccurate comment at `:483-487` ("keeps `fanout_authorized`... untouched — correct...") to reflect that `fanout_authorized` is now correctly derived from the ceiling-filtered value, matching `pi-args.ts`'s `declaredBuiltinTools.includes("subagent")`.

**New tests**: (a) an agent declaring `tools: [subagent, read]` under `{allowedTools: ["read"]}` must
NOT be fanout-authorized even though `--tools` still lists `read` — assert against
`FANOUT_CHILD_ENV`/the child-role env pair, reusing the assertion shape from the existing
`an_agent_declaring_the_subagent_tool_spawns_a_fanout_authorized_child_that_can_delegate` test
(`spawn_plan.rs:3569`); (b) an agent declaring no `tools:` at all under `{allowedTools: ["subagent"]}`
must BE fanout-authorized.

---

## What is NOT a gap (checked, ruled out — do not re-investigate)

- `required_child_tools` (`:511-524`) already built from the ceiling-filtered `allowlist` — no divergence.
- `MCP_DIRECT_CHILD_TOOLS_ENV` (`:996-1000`) already fed from the ceiling-filtered `effective_mcp_tools` — no divergence.
- `CAPABILITY_CEILING_ENV` (the encoded resolved ceiling handed to the child) is unconditional and unaffected by either gap.

## Branch/workflow note

The current checkout carries a branch (`claude/largest-rust-file-ynudqz`) with an already-MERGED pull
request in its history. Per this session's branching convention, restart it from the latest default
branch under the same name before committing this fix, rather than stacking on the merged history.

## Approach

1. Close **Gap 1**: replace the `MCP_DIRECT_TOOLS_ENV` write at `spawn_plan.rs:789-797` with the
   three-way, selector-preserving, ceiling-aware version above. Add its two tests.
2. Close **Gap 2**: hoist `effective_builtin_tools` out of the `explicit_tool_allowlist` gate, derive
   `fanout_authorized` from it instead of raw `builtin_tools`, reuse it as `allowlist`'s initializer,
   and fix the stale `:483-487` comment. Add its two tests.
3. Re-run the full `exec::spawn_plan` test module (74 tests today) plus the full
   `cyrup-ext-subagents` suite (2,486 tests today) — both must stay green, count growing by exactly
   the four new tests added above.

## Acceptance Criteria

- [x] `grep -rn 'allowed_tools\|deny_extensions' --include='*.rs' crates/cyrup-ext-subagents/src` shows consumers in `exec/spawn_plan.rs`, not only `capability_ceiling.rs` — verified
- [x] A ceiling `{allowedTools:["read"], denyExtensions:true}` narrows an agent's tools to `--tools read` and forces `--no-extensions` (verified via the two axis-specific tests already in the suite) — verified
- [x] The same ceiling spawns an agent declaring no `tools:` with `--tools read`, not the full ambient set — verified
- [x] A ceiling requiring `read` for lazy skill loading errors when the set lacks `read` — verified
- [x] The stale comment at `spawn_plan.rs:417-420` is gone — verified
- [ ] The child's `MCP_DIRECT_TOOLS` env var (Gap 1) reflects the ceiling: filtered selectors under a narrowing `allowedTools`, `__none__` under `denyExtensions` or when every selector is filtered away
- [ ] `fanout_authorized` (Gap 2) is derived from the ceiling-filtered declared-tools list: a ceiling excluding `subagent` revokes fanout authorization even when the agent's own `tools:` declares it; a ceiling granting `subagent` via `allowedTools` authorizes fanout even when the agent declares no `tools:` at all
- [x] `cargo test -p cyrup-ext-subagents` passes — verified, 2,486 passed / 0 failed at this review (re-verify after Gap 1/2 land; expect 2,490)
