---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Port the cross-cutting `path` surface gate

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | critical |
| **Kind** | absent |
| **Upstream area** | policy model / cross-cutting path rules |
| **Verification** | **SINGLE-SOURCE — not adversarially verified.** The verifier for this area died on a session limit mid-run. Re-check the port before starting work: the claim may be a false positive if the capability exists under another name. |

## What upstream does that the port does not

Upstream has a `path` surface that gates every filesystem access — Pi tools, MCP arguments,
extension tools, and each path token inside a bash command — and whose `deny` cannot be overridden
by a per-tool allow; the port has no `path` surface at all, only an `external_directory` guard
that fires when a path is outside the cwd.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

access-intent/path-surfaces.ts:26-30 (PATH_SURFACES includes `path`);
handlers/gates/path.ts:34,58,78 (per-tool path gate); handlers/gates/bash-path.ts:58,93,133-143
(same `path` surface applied to bash command tokens, most-restrictive-wins); rule.ts:166-181
`evaluateMostRestrictive`; permission-manager.ts:40 `SPECIAL_PERMISSION_KEYS = new
Set(["external_directory", "path"])`; config-schema.ts:87 ("A `path` deny cannot be overridden by
a per-tool allow")

**Port** (`crates/cyrup-permission-system`):

/home/user/cyrup/crates/cyrup-permission-system/src/manager.rs:41 `const SPECIAL_KEYS: [&str; 2] =
["doom_loop", "external_directory"]` — no `path`. `rg -n
'PATH_SURFACES|path_surface|evaluate_most_restrictive' /home/user/cyrup/crates/cyrup-permission-
system/src` returns nothing. The only path gate is extension/decide.rs:113-119
`resolve_external_directory`, which gate.rs:731 `is_path_outside_working_directory` limits to
paths outside cwd; bash commands are matched only as whole command strings (manager.rs:220-241)
with no path-token extraction.

## Why it matters

There is no way to protect a file from all tools at once. `path: {"*.env": "deny", "~/.ssh/*":
"deny"}` has no effect, an in-repo `.env` is unprotected because it is inside the cwd so
`external_directory` never fires, and `cat ~/.ssh/id_rsa` inside a bash command is never checked
against any path rule — only against whole-command bash patterns.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
