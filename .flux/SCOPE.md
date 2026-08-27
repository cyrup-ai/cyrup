# Active scope: cyrup-ext-subagents v0.57.0 drift remediation

`todo/` holds **2 upstream-parity tasks** promoted from `todo/cyrup-ext-subagents/`, plus the
5 hygiene tasks left over from a prior scope. The 8 unstarted `cyrup-permission-system`
parity tasks (`PORT_*`) from the *previous* active scope are paused — none had begun
execution (all `stage: new`) — and have been returned to `backlog/`; see
[`backlog/UPSTREAM_PARITY_INDEX.md`](./backlog/UPSTREAM_PARITY_INDEX.md) to resume
that work later. `PORT_BASH_COMMAND_ENUMERATION`, the one task from that scope that did run,
is already filed in `done/2026-08-27-04-34/`.

The 2 are the critical half of a fresh drift pass over `crates/cyrup-ext-subagents/`, ported
from `nicobailon/pi-subagents`. They are drawn from
[`docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md`](../docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md),
which measured the port against upstream tag **v0.57.0** and added `SUBA-072`…`SUBA-091` to
the crate's existing `SUBA-001`…`SUBA-071` corpus (`docs/gap-analysis/09-cyrup-ext-subagents.md`).

A non-recursive `todo/*.md` glob — which every flux command uses — does not see
`todo/cyrup-ext-subagents/` or `backlog/`, so `/aug`, `/exec` and `/qa` operate on the
active queue below only.

## The 2

| Severity | Confidence | Task |
| --- | --- | --- |
| critical | confirmed | `SUBA-072_CAPABILITY_CEILING_UNAPPLIED.md` |
| critical as filed / **medium** as corrected (see file) | confirmed | `SUBA-073_CHILD_PERMISSION_POLICY_INERT.md` |

`SUBA-073`'s own body walks its filed severity back from `critical` to `medium` (`high`
defensible) during the adversarial refutation pass — read the "Severity note (correction
applied)" section before treating it as equal-priority to `SUBA-072`. It is included here
anyway because `SUBA-CORPUS-HEALTH.md` groups the two as the same defect class (see below),
not because the severities match.

## Why these two

Both are, in `SUBA-CORPUS-HEALTH.md`'s words, **"the enforcement machinery is ported and
permanently unreachable"**:

- `SUBA-072` — `exec/capability_ceiling.rs` correctly resolves and intersects
  `allowedTools`/`denyExtensions`, and even base64-encodes the result into the child env, but
  `exec/spawn_plan.rs` never gates the spawn on it — only the AGENTS axis is enforced. A
  registered ceiling *presents* as armed while silently permitting the exact widening it
  exists to prevent. That is a permission bypass, not merely an absent feature, hence
  `critical`.
- `SUBA-073` — `watchdog/permission_arbiter.rs` fully implements the child-side permission
  gate, but nothing in `exec/` ever writes the policy env var it reads, and neither
  `permission:` nor `permissions:` frontmatter is a known field — both round-trip silently
  into `extra_fields` with no diagnostic. Not a bypass of an *enforcing* system (a cyrup
  child is still gated by `cyrup-permission-system` regardless), which is why the severity
  was corrected down — but still a config key that is parsed and ignored.

Both fixes are scoped to `crates/cyrup-ext-subagents/`, both are effort `M`, and both touch
`exec/spawn_plan.rs`.

## Sequencing

No hard dependency between the two, but **both edit `exec/spawn_plan.rs`** — run them
sequentially in the same working tree (not as parallel worktrees) to avoid a mechanical merge
conflict. Suggested order: `SUBA-072` first (unambiguous critical, revises the stale
`SUBA-021` claim in-line), then `SUBA-073`.

## Reference checkouts

Both cited upstream files live in tag `v0.57.0` of `nicobailon/pi-subagents`, checked out at:

    tmp/pi              — earendil-works/pi              @ v0.84.3  (HEAD e868230)
    tmp/pi-subagents    — nicobailon/pi-subagents         @ v0.58.0  (HEAD a9d0ee1, one release past v0.57.0)

`v0.58.0` is one release ahead of what the drift pass measured; every citation in the two
tasks is pinned to `v0.57.0` evidence via `git show v0.57.0:<path>`; the working tree being a
release ahead does not affect either task's evidence, only a future drift pass.

## The rest of the batch — parked

11 more items from the same drift pass (`SUBA-074`, `075`, `076`, `077`, `078`, `079`, `081`,
`083`, `085` — 8 high, plus `SUBA-CORPUS-HEALTH` and `SUBA-VERIFY-CARRIED-LEADS`) remain in
`todo/cyrup-ext-subagents/`, each still carrying its own "lives in a subdirectory, pass the
absolute path" note since they have not been promoted. `SUBA-080` is refuted; `SUBA-082`,
`084`, `086`–`091` are carried-but-not-adversarially-verified — re-verify before promoting.
Promote further items into `todo/` the same way these two were: `git mv` the file up one
level, drop its "Path note" callout, and fix its now-shallower relative link to the
`docs/gap-analysis/` source.

`SUBA-CORPUS-HEALTH.md` also names three ledger rows in `09-cyrup-ext-subagents.md` that carry
evidence now factually wrong at HEAD (`SUBA-021`, `VL-S1`, `VL-S14`, `SUBA-051`'s Fix line) —
worth correcting before or alongside this scope, not urgent to block it.

## The 5 carried over

`COLLAPSE_NDJSON_PARSERS`, `DECOMPOSE_LONG_FUNCTIONS`, `REPOINT_STALE_TEST_LAYOUT_DOCS`,
`RESTRICT_PUB_VISIBILITY`, `UNIFY_DUPLICATE_TYPE_SHAPES` — unrelated to either parity effort
and independently runnable. `COLLAPSE_NDJSON_PARSERS` and `UNIFY_DUPLICATE_TYPE_SHAPES` are
`stage: exec`; per their own history both landed complete on `main` already but the flux
bookkeeping was never advanced to `done` — leave as-is unless you're the one closing that out.

## Restoring the permission-system scope

    git mv .flux/backlog/PORT_BASH_COMMENT_STRIPPING.md \
           .flux/backlog/PORT_BASH_PATH_PROJECTION.md \
           .flux/backlog/PORT_BASH_WRAPPER_FLOOR.md \
           .flux/backlog/PORT_CROSS_CUTTING_PATH_SURFACE.md \
           .flux/backlog/PORT_FLAT_PERMISSION_POLICY_MODEL.md \
           .flux/backlog/PORT_HOME_PREFIX_EXPANSION_IN_PATTERNS.md \
           .flux/backlog/PORT_PATH_CANONICALIZATION.md \
           .flux/backlog/PORT_PROJECT_TRUST_GATING.md \
           .flux/todo/

That scope's own prerequisites still apply when resumed — see its history in this file's
prior revision (`git log -p -- .flux/SCOPE.md`) for the `tmp/pi-packages` checkout note and
the three single-source warnings.
