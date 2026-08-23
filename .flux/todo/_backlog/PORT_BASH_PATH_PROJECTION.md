---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Port the bash path projection and its external_directory gate

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | high |
| **Kind** | absent |
| **Upstream area** | access intent: path surfaces |
| **Verification** | Confirmed by an adversarial re-check that tried and failed to refute it |

## What upstream does that the port does not

Upstream resolves every path-like token inside a bash command (cd-folded base, $HOME/$PWD
expansion, redirect targets, nested substitutions) into AccessPath values and runs the
external_directory gate over the ones outside cwd; the port never inspects bash command text for
paths at all.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

src/access-intent/bash/bash-path-resolver.ts:92-120 (BashPathResolver.resolve →
externalPaths/ruleCandidates, cd-base folding), :439 (external-path dedup with the
isSafeSystemPath exclusion); src/access-intent/bash/program.ts:116,131
(BashProgram.externalPaths()/pathRuleCandidates()); src/handlers/gates/bash-external-
directory.ts:24-46 (describeBashExternalDirectoryGate reads bashProgram.externalPaths() and
resolves each on the external_directory surface)

**Port** (`crates/cyrup-permission-system`):

crates/cyrup-permission-system/src/extension/decide.rs:117-121 — the external-directory guard is
reached only via `gate::get_path_bearing_tool_path(normalized, input)`; crates/cyrup-permission-
system/src/gate.rs:13 lists PATH_BEARING_TOOLS as ["read","write","edit","find","grep","ls"] (no
"bash") and gate.rs:110-125 reads only top-level `path`/`file_path`, so a `bash` input `{command:
...}` yields `None` and the guard is skipped entirely. Negative greps: `rg -n
"bash_path|path_candidate|external_paths|rule_candidate" /home/user/cyrup/crates/cyrup-permission-
system/src` → 0 matches.

## Why it matters

`bash` is the one tool with unrestricted filesystem reach, and it is the only tool completely
exempt from the external_directory boundary in the port. `cat ~/.ssh/id_rsa`, `cp secrets /tmp/x`,
`rm -rf /etc/foo` all escape the working-directory sandbox without ever reaching the
external_directory ask/deny that the same paths would trigger through `read`/`write`.

## Notes from the verification pass

These corrections and caveats come from the agent that tried to refute the finding —
read them before trusting the framing above.

Could not refute the absence, but severity is over-rated at critical. Verified:
extension/decide.rs:115-121 reaches the external-directory guard ONLY through
`gate::get_path_bearing_tool_path(normalized, input)`; gate.rs:107-124 requires a top-level
`path`/`file_path` before any name test, so a bash input `{command: ...}` returns None on the very
first `?` and the guard is skipped. gate.rs:31 PATH_BEARING_TOOLS does not include "bash", and
gate.rs:40-61 `is_likely_filesystem_tool_name` (suffixes read/write/edit/find/grep/search/list/ls)
does not match "bash" either. Negatives: `rg -n
"bash_path|path_candidate|external_paths|rule_candidate"` over src/ -> 0. Upstream verified at
src/handlers/gates/bash-external-directory.ts and src/access-intent/bash/bash-path-resolver.ts.
Downgrade rationale (not a refutation): this is a second layer, not the only one.
manager.rs:229-233 still resolves the bash command against compiled_bash / tools.bash /
DefaultCategory::Bash, which defaults to Ask (types.rs:55), so with no bash allow rule the call
still prompts. The hole opens only once an operator writes a permissive bash rule (`cat *`, `git
*`) — which is normal practice, hence high rather than critical. Note this claim and claim 1
compound: fixing enumeration without path projection still leaves each unit's path arguments
unexamined.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
