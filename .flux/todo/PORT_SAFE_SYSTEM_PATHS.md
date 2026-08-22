---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Exempt safe system device paths from the external-directory check

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | low |
| **Kind** | absent |
| **Upstream area** | access intent: path surfaces |
| **Verification** | Confirmed by an adversarial re-check that tried and failed to refute it |

## What upstream does that the port does not

Upstream's containment predicate returns false for OS device paths (`/dev/null`,
`/dev/std{in,out,err}`) so they never trigger external_directory; the port's boundary check has no
such exemption.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

src/path/path-containment.ts:20-22 (`if (isSafeSystemPath(canonicalPath)) return false;` inside
isPathOutsideWorkingDirectory); src/safe-system-paths.ts:5-18 (SAFE_SYSTEM_PATHS /
isSafeSystemPath); also honoured by the bash external-path dedup at src/access-intent/bash/bash-
path-resolver.ts:439

**Port** (`crates/cyrup-permission-system`):

crates/cyrup-permission-system/src/gate.rs:729-737 `is_path_outside_working_directory` returns
`!is_path_within_directory(path, cwd)` with no device-path branch. Negative grep: `rg -n
"safe_system|SAFE_SYSTEM|dev/null" /home/user/cyrup/crates/cyrup-permission-system/src` → 0
matches.

## Why it matters

A write to `/dev/null` (or a read from `/dev/stdin`) raises a spurious external_directory prompt
and, on a deny rule or a headless session (extension/decide.rs's confirmation_unavailable branch),
is blocked outright — a functional regression that also trains users to approve external-directory
prompts reflexively.

## Notes from the verification pass

These corrections and caveats come from the agent that tried to refute the finding —
read them before trusting the framing above.

Could not refute. gate.rs:726-737 `is_path_outside_working_directory` is
`!normalized_cwd.is_empty() && !normalized_path.is_empty() && !is_path_within_directory(...)` with
no device branch. Negatives: `rg -ni "safe_system|SAFE_SYSTEM|/dev/null|dev/std"` over src/ -> 0
hits. Upstream verified at src/safe-system-paths.ts:5-18 (exact set: /dev/null, /dev/stdin,
/dev/stdout, /dev/stderr) and src/path/path-containment.ts:19-21. Low confirmed and this is the
one claim whose direction is FAIL-CLOSED, not fail-open — it produces a spurious prompt or an
over-block, never an unwanted allow, so it should be sequenced last. Practical reach is also
narrower than the finder implies: because bash paths are never inspected at all (claim 2), the
common `cmd > /dev/null` case does not reach this code today; it only fires for an explicit
`write`/`read` tool call naming a device path. Fixer note: the exemption must be applied to the
CANONICAL form (upstream tests `isSafeSystemPath(canonicalPath)`), so land it after claim 4 or it
will miss any aliased spelling.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
