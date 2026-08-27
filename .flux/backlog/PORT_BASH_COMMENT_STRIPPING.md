---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Strip leading bash comment lines from the rule match value

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

Upstream matches bash rules against the command with `#` comment lines removed (falling back to
the raw text when nothing remains); the port matches the raw command string.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

src/access-intent/input-normalizer.ts:164 (`const matchValue = stripBashCommentLines(command) ||
command;`, with `resultExtras: { command }` keeping the raw text); src/bash-arity.ts:206-210
(stripBashCommentLines drops every `/^\s*#/` line and trims)

**Port** (`crates/cyrup-permission-system`):

crates/cyrup-permission-system/src/manager.rs:222-227 reads `input.command` and passes it verbatim
to `find_compiled_match`. Negative grep: `rg -n "strip_bash_comment|comment_lines"
/home/user/cyrup/crates/cyrup-permission-system/src` → 0 matches (the only `starts_with('#')` in
the crate is common.rs:190, YAML frontmatter comment handling).

## Why it matters

A deny rule is bypassed by prepending a comment. `wildcard.rs` anchors patterns, so `"bash": {"rm
-rf *": "deny"}` does not match `"# cleanup\nrm -rf /"`; combined with a `tools.bash: allow`
fallback (manager.rs:230-233) the command resolves to allow. Agents routinely prepend `#
<description>` lines, so this fires in normal operation as well as adversarially.

## Notes from the verification pass

These corrections and caveats come from the agent that tried to refute the finding —
read them before trusting the framing above.

Could not refute. manager.rs:222-227 passes `input.command` verbatim to `find_compiled_match`.
Negatives: `rg -n "strip_bash_comment|comment_lines"` -> 0; `rg -ni "arity"` over src/ -> 0 real
hits (only the word 'parity' in prose and forwarding.rs:1521 'granularity'), so pi's whole `bash-
arity.ts` module is unported. The only `starts_with('#')` in the crate is common.rs:190 (YAML
frontmatter), exactly as the finder said. Upstream verified at src/access-intent/input-
normalizer.ts (matchValue = stripBashCommentLines(command) || command) and src/bash-
arity.ts:206-210. Severity medium confirmed but note the mechanism precisely for the fixer: this
is NOT a fail-open on its own. wildcard.rs anchors to `^…$` with dotAll, so `"# cleanup\nrm -rf
/"` misses BOTH the deny rule and any allow rule; the result then falls through manager.rs:229-233
to tools.bash, then DefaultCategory::Bash, which is Ask by default (types.rs:55). It becomes an
allow only when a `tools.bash: allow` or `bash: {"*": "allow"}` fallback exists. Also note the fix
must keep the RAW command in `PermissionCheckResult.command` (manager.rs:239) — that field feeds
gate.rs:632-635 (ask prompt), gate.rs:274-275 (deny reason) and gate.rs:151-156
(`get_pattern_approval_subject`, which is what an "Allow Always" persists), so stripping it there
would silently rewrite the rule the user approves.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
