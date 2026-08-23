---
stage: new
status: done
updated: 2026-08-22 23:05
---

# Resolve symlinks for the path boundary and policy match values

> **Upstream parity gap.** `cyrup-permission-system` is a port of `pi-permission-system` **v0.8.0**;
> upstream is now **v27.0.0** (2026-08-21, 27 major releases later) and lives at
> [`gotgenes/pi-packages`](https://github.com/gotgenes/pi-packages) — the standalone repo the port
> cites is archived at v5.18.1. Reference checkout: `./tmp/pi-packages/packages/pi-permission-system`.
> Full backlog and ordering: [UPSTREAM_PARITY_INDEX.md](./UPSTREAM_PARITY_INDEX.md).


| | |
| --- | --- |
| **Severity** | high |
| **Kind** | partial |
| **Upstream area** | access intent: path surfaces |
| **Verification** | Confirmed by an adversarial re-check that tried and failed to refute it |

## What upstream does that the port does not

Upstream's AccessPath carries a canonical (realpath-resolved) form used for the outside-cwd
boundary decision and added to the external_directory match values; the port's path normalization
is purely lexical and never touches the filesystem.

## Evidence

**Upstream** (`tmp/pi-packages/packages/pi-permission-system`):

src/access-intent/access-path.ts:48-52 (matchValues() = lexical aliases ∪ canonical), :60-62
(boundaryValue() = canonical, "for the outside-CWD boundary decision"), :106-116 (forPath wires
canonicalNormalizePathForComparison); src/access-intent/path-normalization.ts:112-120
(canonicalNormalizePathForComparison); src/path/canonicalize-path.ts:15-37 (canonicalizePath,
best-effort realpathSync walking up to the first resolvable ancestor); src/path/path-
containment.ts:12-22 ("Both operands must already be canonical (symlink-resolved…)")

**Port** (`crates/cyrup-permission-system`):

crates/cyrup-permission-system/src/common.rs:74-98 `normalize_path_for_comparison` is quote-strip
+ `~` expand + join + `lexical_normalize` only; common.rs:36-38 documents the deliberate choice
not to `canonicalize`. crates/cyrup-permission-system/src/gate.rs:729-737
`is_path_outside_working_directory` compares the two lexical forms. Negative grep: `rg -n
"canonicalize_path|realpath|canonical_normalize" /home/user/cyrup/crates/cyrup-permission-
system/src` → 0 matches (only ext_config.rs:552, an unrelated config-file resolve).

## Why it matters

A symlink inside the working directory that points outside it defeats the external_directory
boundary: `<cwd>/link -> /etc` normalizes lexically to `<cwd>/link/passwd`, which
`is_path_within_directory` accepts, so `read <cwd>/link/passwd` is never gated. Conversely a
configured `/tmp/*` allow does not match a path whose canonical form is `/private/tmp/...` —
upstream issue #418, which AccessPath exists to fix.

## Notes from the verification pass

These corrections and caveats come from the agent that tried to refute the finding —
read them before trusting the framing above.

Could not refute. common.rs:74-98 `normalize_path_for_comparison` is quote-strip + `@`-strip + `~`
expand + cwd join + `lexical_normalize` (common.rs:36-64) — purely lexical, and common.rs:36-38
explicitly states `std::fs::canonicalize` is wrong here. IMPORTANT: that comment is NOT a CYRUP-
DELTA — I checked every one of the 27 `\[CYRUP-DELTA]` markers in the crate and none covers path
canonicalization; the comment is a faithfulness note to node `path.normalize` (pi v0.8.0's
`normalizePathForComparison`), i.e. the port is correct against its stated baseline and behind
v27. Negatives: `rg -n "canonicalize|realpath|canonical"` over src/ -> only common.rs:37/320
prose, manager.rs:1184 test name, and ext_config.rs:428/545-552 (config-file symlink realpath for
atomic write — genuinely unrelated). gate.rs:726-737 `is_path_outside_working_directory` compares
the two lexical forms. Upstream verified at src/path/path-containment.ts:1-23 (doc: "Both operands
must already be canonical (symlink-resolved…)"). High confirmed: with a common `tools: {"read":
"allow"}` or a `read` allow rule, `<cwd>/link -> /etc` makes `<cwd>/link/passwd` lexically within
cwd, so `is_path_within_directory` (common.rs:106-118) accepts it and the external_directory
ask/deny never runs. Fixer note: upstream's canonicalizePath is BEST-EFFORT — it walks up to the
first resolvable ancestor — because the permission subject is frequently a path that does not
exist yet (a `write` target). A naive `std::fs::canonicalize` will return Err on those and must
not be allowed to fail the path open.

## Acceptance Criteria

- [ ] The capability above is implemented in `crates/cyrup-permission-system`, matching the
      cited upstream behaviour
- [ ] The implementation's doc comments cite the upstream source the way the rest of the crate
      does (`file.ts:line`), so the next parity sweep can find it
- [ ] A deliberate divergence, if any, is marked `\[CYRUP-DELTA]` with its reasoning
- [ ] `cargo check -p cyrup-permission-system --all-targets` and `cargo clippy --all-targets` clean
- [ ] The existing suite still passes

> Run `/ask` or `/aug` on this file before `/exec` — it is a research-stage finding, not a plan.
