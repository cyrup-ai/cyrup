---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Fail closed when a non-global config scope is invalid

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | high |
| **Kind** | absent |
| **Upstream area** | config loading / fail-closed clamp |
| **Verification** | Confirmed by an adversarial re-check that tried and failed to refute it |

## What upstream does that the port does not

Upstream marks a scope `invalid` when its config file is present but unparseable/schema-rejected
and then floors every `allow` in the composed ruleset to `ask`, so a higher scope meant to tighten
policy cannot silently fail open; the port drops the broken scope to an empty ruleset and keeps
every inherited `allow` intact.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

policy-loader.ts:237-240 and :286-290 (`invalid: true` for a present-but-rejected project / agent
scope); types.ts:24-33 (`ScopeConfig.invalid`); rule.ts:83-89 `floorAllowsToAsk`; permission-
manager.ts:229-238 (failClosedScopes → floorAllowsToAsk) and :152-164 (the operator-facing fail-
closed notice)

**Port** (`crates/cyrup-permission-system`):

/home/user/cyrup/crates/cyrup-permission-system/src/manager.rs:512-518
(`load_project_global_config`: on parse error → warn, return `AgentPermissions::default()`) and
manager.rs:583-597 `load_agent_permissions_from` (any read/parse failure →
`AgentPermissions::default()`, no warning, no invalid flag). `rg -n
'floor_allows|fail_closed|invalid' /home/user/cyrup/crates/cyrup-permission-system/src/manager.rs`
returns nothing; the `fail.closed` hits in the tree are all about the ask channel and forwarding
timeouts (ask.rs:84, extension/prompt.rs:213).

## Why it matters

Corrupting or truncating `.cyrup/agent/cyrup-permissions.jsonc` (or an agent frontmatter file)
silently removes exactly the tightening rules it contained while leaving the global `allow` rules
in force, and nothing tells the operator. Upstream turns that same situation into `ask` prompts
plus a visible notice.

## Also reported independently

Other area agents found this same gap from a different angle:

- **Fail closed when a non-global scope's config is invalid** (resolver / permission manager) — When a project/agent/project-agent config fails to load or parse, upstream floors every
`allow` in the composed ruleset to `ask` for the session and reports the reason through
`getConfigIssues`; the port merely warns and drops that layer, keeping all lower-scope allows
in force.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
