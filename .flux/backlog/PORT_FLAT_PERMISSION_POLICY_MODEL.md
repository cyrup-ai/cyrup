---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Port the flat `permission` policy model and its surface rules

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./_backlog/UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | critical |
| **Kind** | absent |
| **Upstream area** | policy model / config schema |
| **Verification** | **SINGLE-SOURCE — not adversarially verified.** The verifier for this area died on a session limit mid-run. Re-check the port before starting work: the claim may be a false positive if the capability exists under another name. |

## What upstream does that the port does not

Upstream v27 expresses all policy as a flat `permission` block keyed by arbitrary surface names
(with a universal `"*"` fallback, string-shorthand surfaces, and pattern maps) compiled into an
ordered `Ruleset`, while the port still reads the pre-v1 categorical file shape
(`defaultPolicy`/`tools`/`bash`/`mcp`/`skills`/`special`) and drops a v27 config entirely.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

config-schema.ts:76-111 (permissionSchema: any surface key → PermissionState | permissionMap);
normalize.ts:18-43 (normalizeFlatConfig → Ruleset); permission-manager.ts:193-223 (universal `"*"`
fallback extraction, config-rule tagging, composeRuleset); synthesize.ts:444-457 + 516-522
(defaults → baseline → config layering); config-loader.ts:289-292 ("legacy-format keys
(defaultPolicy, tools, bash, etc.) are not translated and contribute no permission rules")

**Port** (`crates/cyrup-permission-system`):

/home/user/cyrup/crates/cyrup-permission-system/src/manager.rs:1062-1081
`normalize_raw_permission` reads only `defaultPolicy`/`tools`/`bash`/`mcp`/`skills`/`special` from
the file root; manager.rs:496 `normalize_policy(value.get("defaultPolicy"))`; types.rs:339-348
`AgentPermissions` has exactly five fixed categories. `rg -n 'get\("permission"\)'
/home/user/cyrup/crates/cyrup-permission-system/src` returns only manager.rs:599 (agent-markdown
frontmatter) — no config-file `permission` block reader. `rg -n
'normalize_flat_config|Ruleset|RuleOrigin' src` returns nothing.

## Why it matters

An operator writing the documented v27 config (`{"permission": {"read": {...}, "path": {...}}}`)
gets a policy file that contributes zero rules; every surface silently falls through to the per-
category `ask` default, and any surface the port does not model (`read`, `write`, `edit`, `grep`,
`find`, `ls` as first-class surfaces, plus arbitrary extension-tool surfaces) can never be denied
by config at all.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
