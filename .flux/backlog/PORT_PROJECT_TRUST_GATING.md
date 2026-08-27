---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Gate project-scoped config and policy on project trust

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./_backlog/UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | critical |
| **Kind** | absent |
| **Upstream area** | handlers: session lifecycle / turn prep |
| **Verification** | Confirmed by an adversarial re-check that tried and failed to refute it |

## What upstream does that the port does not

Upstream withholds every project-scoped permission scope (project policy file, project agents dir,
and the project-merged runtime config carrying yoloMode) whenever ctx.isProjectTrusted() is false,
and loudly warns; the port never consults project trust and always loads the project scope from
ctx.cwd.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

handlers/lifecycle.ts:54-60 (refreshConfig/resetForNewSession take projectTrusted), :92-96 (reload
path), :24-27 UNTRUSTED_PROJECT_MESSAGE, :109-115 warnProjectUntrusted (review
'project_trust.skipped' + warn); handlers/session-turn-prep.ts:52 (per-turn refresh also gated);
config-store.ts:99-113 (includeProjectScope: projectTrusted); permission-session.ts:106-110 and
:132-136 (configureForCwd(projectTrusted ? cwd : undefined))

**Port** (`crates/cyrup-permission-system`):

`rg -n "is_project_trusted|project_trusted|isProjectTrusted" /home/user/cyrup/crates/cyrup-
permission-system/src` returns nothing (the host does expose it — cyrup-
ext/src/facade.rs:195,1987). The port loads project scope unconditionally:
src/extension/native.rs:195 and src/extension/config.rs:55-62 call
refresh_config_and_manager(&ctx.cwd), and src/extension/paths.rs:51-68 always sets
project_global_config_path/project_agents_dir from cwd.

## Why it matters

Opening an untrusted repository lets that repository's checked-in .cyrup/agent permission policy
(and project-scoped agent policy) take effect — it can widen the allow set for bash/read/mcp
before the human has granted trust — and the reduced-scope state is never announced, so nothing
tells the operator which scopes are in force. Upstream's #644 hardening is entirely absent.

## Notes from the verification pass

These corrections and caveats come from the agent that tried to refute the finding —
read them before trusting the framing above.

Could not refute. Upstream confirmed at handlers/lifecycle.ts:54-60,:92-96,:109-115, config-
store.ts:99-113, permission-session.ts:106-110,:132-136. Port: `grep -rn "trust\|Trust" src/`
returns only the four-LAYER trust concept in manager.rs (trusted-floor merge), never project
trust; no hit for is_project_trusted/project_trusted anywhere in the crate.
src/extension/paths.rs:53-68 (manager_paths_for) unconditionally sets
project_global_config_path/project_agents_dir from cwd, and src/extension/config.rs:55-62
(refresh_config_and_manager) rebuilds from ctx.cwd with no trust argument. The host DOES expose
it: cyrup-ext/src/native.rs:115-116 (HostCtxRich::is_project_trusted) and :174-177
(HostCtx::is_project_trusted()), so the fix is a one-argument thread-through, not a host change.
Two caveats for the fixer: (a) the port has NO project-scope merge for the extension config at all
— `grep -n project src/ext_config.rs` returns nothing — so only the POLICY half of upstream's #644
applies here (yoloMode is global-only in this port, which is safe by accident); (b)
HostCtxRich::default() is is_project_trusted=false and cyrup-ext/src/native.rs:709-723 documents
that a --no-default-features build with no HostCtxSource attached hands every native built-in that
default, so gating naively would silently withhold the project scope on every such host — the gate
needs to key on a real ctx_source being attached, or the port trades a silent widening for a
silent narrowing. Severity kept critical: the project layer is untrusted in the merge so a trusted
global deny still floors it (src/manager.rs:739-748), but an untrusted repo's .cyrup/agent/cyrup-
permissions.jsonc can still ADD allow rules for anything the global policy does not explicitly
deny, converting an ask into a silent auto-allow before the human grants trust.

## Also reported independently

Other area agents found this same gap from a different angle:

- **Gate project-scoped policy on project trust** (gate / session lifecycle) — Upstream withholds the project and project-agent policy scopes (and loudly warns) when the
host reports the project as untrusted; the port always loads `<cwd>/.cyrup/agent` policy, so
an untrusted repo's own permission file silently widens policy.
- **Withhold project-scope config when the project is untrusted** (config loading / project trust) — Upstream skips both project config steps entirely when the project is not trusted, so an
untrusted repository cannot contribute permission rules or loosen runtime knobs like
`yoloMode`; the port always reads the project policy file and has no trust concept.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
