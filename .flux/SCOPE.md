# Active scope: the permission-gate enforcement spine

`todo/` holds **9 upstream-parity tasks** selected from `todo/_backlog/`, plus the 5 tasks
left over from the previous code-review scope.

The 9 are one coherent subsystem: **what policy is in effect, and what that policy is
matched against**, in `crates/cyrup-permission-system`. They are drawn from the
`pi-permission-system` v0.8.0 → v27.0.0 parity backlog — see
[`_backlog/UPSTREAM_PARITY_INDEX.md`](./todo/_backlog/UPSTREAM_PARITY_INDEX.md) for the
full 40-task set, the severity table, and how the analysis was produced.

The remaining 89 backlog files stay parked. A non-recursive `todo/*.md` glob — which every
flux command uses — does not see them, so `/aug N`, `/exec N` and `/qa N` operate on the
active queue only.

## The 9

| Severity | Verification | Task |
| --- | --- | --- |
| critical | verified | `PORT_BASH_COMMAND_ENUMERATION.md` |
| critical | verified | `PORT_PROJECT_TRUST_GATING.md` |
| critical | **single-source** | `PORT_FLAT_PERMISSION_POLICY_MODEL.md` |
| critical | **single-source** | `PORT_CROSS_CUTTING_PATH_SURFACE.md` |
| high | verified | `PORT_BASH_PATH_PROJECTION.md` |
| high | verified | `PORT_PATH_CANONICALIZATION.md` |
| high | **single-source** | `PORT_HOME_PREFIX_EXPANSION_IN_PATTERNS.md` |
| medium | verified | `PORT_BASH_WRAPPER_FLOOR.md` |
| medium | verified | `PORT_BASH_COMMENT_STRIPPING.md` |

## Why these nine

Four of the backlog's five criticals are here. The fifth
(`PORT_LOG_KEY_NAME_REDACTION`) is logging-side and belongs with the observability
cluster, not this one; it stays parked.

`PORT_BASH_COMMAND_ENUMERATION` is the anchor and the only finding in the whole backlog
where a rule the operator **wrote** is actively bypassable rather than merely absent:
`wildcard.rs:60-68` compiles `echo *` to `^echo .*$` and `manager.rs:221-243` matches it
against the entire command string, so `echo hi && rm -rf /` is allowed by an `echo *`
rule. Every deny rule on a bash sub-command is bypassable by prefixing an allowed command.

The other eight are the rest of that decision path. Fixing enumeration alone still leaves
each enumerated unit's path arguments ungated, and the flat `permission` model is the
schema most of the backlog's `high` items assume exists.

## Sequencing

Follows the index's suggested order; the edges below are hard.

| Must run first | Then | Why |
| --- | --- | --- |
| `PORT_BASH_COMMAND_ENUMERATION` | `PORT_BASH_WRAPPER_FLOOR` | `WrapperKind` is a property of an enumerated command unit (upstream calls `wrapper-analysis.ts` from `makeCommandUnit`). Not implementable before the enumerator exists — the task says sequence it as part of the same fix. |
| `PORT_BASH_COMMAND_ENUMERATION` | `PORT_BASH_PATH_PROJECTION` | Compound: enumeration without projection still leaves each unit's path arguments unexamined. |
| `PORT_FLAT_PERMISSION_POLICY_MODEL` | the `high` items | They assume the flat model. Doing it late means porting features twice. |

`PORT_FLAT_PERMISSION_POLICY_MODEL` and `PORT_CROSS_CUTTING_PATH_SURFACE` are **one
change** — v4.0.0 replaced `defaultPolicy`/`tools`/`bash`/`mcp`/`skills`/`special` with a
single flat `permission` object plus a cross-cutting `path` surface. Do them together.

Suggested run order:

1. `PORT_BASH_COMMAND_ENUMERATION` — alone
2. `PORT_PROJECT_TRUST_GATING` — read its `HostCtxRich::default()` note first
3. `PORT_FLAT_PERMISSION_POLICY_MODEL` + `PORT_CROSS_CUTTING_PATH_SURFACE` — together
4. `PORT_BASH_PATH_PROJECTION`, `PORT_PATH_CANONICALIZATION`, `PORT_HOME_PREFIX_EXPANSION_IN_PATTERNS`
5. `PORT_BASH_WRAPPER_FLOOR`, `PORT_BASH_COMMENT_STRIPPING`

## Two prerequisites before `/exec`

**1. The upstream reference checkout is missing.** Every one of the 9 cites its evidence as
`tmp/pi-packages/packages/pi-permission-system/<file>.ts:<line>`. `tmp/` is gitignored
(`.gitignore:7`) and absent from a fresh clone, so those citations cannot be read as-is:

    git clone https://github.com/gotgenes/pi-packages tmp/pi-packages

**2. Three of the 9 are single-source.** The parity analysis paired each of 7 compare
agents with an adversary that tried to refute its findings. The `policy-config`,
`prompts-ui` and `logging-redaction` adversaries died on a session limit and never ran.
`PORT_FLAT_PERMISSION_POLICY_MODEL`, `PORT_CROSS_CUTTING_PATH_SURFACE` and
`PORT_HOME_PREFIX_EXPANSION_IN_PATTERNS` carry that warning in their own files.
**Re-check each against the port before starting work** — the other six were adversarially
verified and can be trusted at face value.

Each file also ends with: *run `/ask` or `/aug` before `/exec` — it is a research-stage
finding, not a plan.* All 9 are `stage: new`.

## The 5 carried over

`COLLAPSE_NDJSON_PARSERS`, `DECOMPOSE_LONG_FUNCTIONS`, `REPOINT_STALE_TEST_LAYOUT_DOCS`,
`RESTRICT_PUB_VISIBILITY`, `UNIFY_DUPLICATE_TYPE_SHAPES` — unrelated to the parity work and
independently runnable.

## Restoring the full queue

    mv .flux/todo/_backlog/*.md .flux/todo/ && rmdir .flux/todo/_backlog
