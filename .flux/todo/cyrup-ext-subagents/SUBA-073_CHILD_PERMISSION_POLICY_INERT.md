---
stage: new
status: done
updated: 2026-08-27 05:30
severity: critical
effort: medium
subsystem: config / permissions / discovery frontmatter
source: docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md
item: SUBA-073
---

> **Path note.** This task lives in a subdirectory. Every flux command globs a single level (`ls -1 "$FLUX_BASE/todo/"*.md`), so `/exec`, `/aug` and `/qa` will not list it — pass the absolute path explicitly.

# SUBA-073 — Subagent permission policy never reaches a spawned child: `config.permissions` and agent `permission:`/`permissions:` frontmatter are accepted and inert

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

## Scope

In scope: the behaviour described above, in `crates/cyrup-ext-subagents/`.

Out of scope: the other SUBA items in this batch (each has its own file); any refactor beyond what the fix needs; and the ledger corrections in `SUBA-CORPUS-HEALTH.md`.

Full finding, with the complete evidence chain: [`09a-cyrup-ext-subagents-v0.57-drift.md` § SUBA-073](../../../docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md)

## Approach

Wire the accepted-but-inert policy through to the spawn path: parse `config.permissions` and the
agent `permission:` / `permissions:` frontmatter into the resolved launch plan, then translate it
into the child's permission arguments/env exactly as upstream does. Treat a declared restriction that
cannot be expressed to the child as a hard launch error rather than a silent widening.

## Acceptance Criteria

- [ ] A `config.permissions` restriction demonstrably applies to a spawned child
- [ ] Agent frontmatter `permission:` / `permissions:` reaches the child
- [ ] A declared restriction that cannot be honoured fails the launch instead of running unrestricted
- [ ] `cargo test -p cyrup-ext-subagents` passes
