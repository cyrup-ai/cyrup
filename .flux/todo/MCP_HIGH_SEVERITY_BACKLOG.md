---
stage: new
status: done
updated: 2026-08-22 06:00
---

# Plan The Remaining High-Severity cyrup-mcp Backlog

## Description

After PR #30 closed all 8 `critical` units except `MCP-394` and the `13c` high cluster, the bulk
of remaining work is the `high` tier: **73 of 147 `high` units are open**, inside a total of 198
open units (98 missing + 100 partial) — `docs/gap-analysis/13-cyrup-mcp-STATUS.md:349`.

That is too large to pick from ad hoc and too large for one wave. This task produces the batching,
in the same shape that worked in PR #30.

Open critical-or-high by section:

| § | open critical+high | missing | partial |
|---|---:|---:|---:|
| `13c` servers/transports/cache | 23 | 20 | 20 |
| `13i` protocol/verification | 16 | 31 | 11 |
| `13a` activation/lifecycle | 10 | 17 | 22 |
| `13h` panels/commands | 10 | 15 | 9 |
| `13b` config ladder | 9 | 6 | 13 |
| `13e` tools/naming/approval | 7 | 7 | 8 |
| `13d` proxy modes | 3 | 1 | 4 |
| `13g` oauth | 2 | 1 | 8 |
| `13f` credentials | 1 | 0 | 5 |

`13i` has its own scoping task (`MCP_13I_SCOPING.md`) — exclude it here. `13f` is the strongest
surface: nothing missing.

## What worked in PR #30, and what did not

- **Group by shared obligation, not by file.** Splitting by file put `runtime.rs` in a different
  agent's set than the unit whose obligation needed it, so the agent that found the `has_ui` bug
  could not fix it.
- **Measure, do not read.** Node 22 runs the upstream TypeScript directly
  (`node --experimental-strip-types`). Every parity bug worth having found in PR #30 — the missing
  `socket` key, the `~//x` cwd, the duplicate child processes — came from executing upstream, not
  from reading it.
- **Adversarial verification.** Implementer then a skeptic instructed to REFUTE; skeptic wins
  ties. The audit's own skeptic pass overturned 15 rulings, so treat any `missing` row as a lead
  rather than a verdict.
- **Ablation.** Disable the fix behind `if false` and confirm the test fails, or the test is not
  pinning what you think.

## Acceptance Criteria

- [ ] The 73 open `high` units (excluding 13i) are grouped into waves by shared obligation
- [ ] Each wave names its files, its verification approach, and what must land before it
- [ ] Units whose blocker is HA-1 are identified and sequenced after `HOST_LATE_TOOL_REGISTRATION.md`
- [ ] A sample of `missing` rows is spot-checked against the Rust before scheduling — the audit's false-positive rate was non-zero by construction
- [ ] The plan is written into `13-cyrup-mcp-STATUS.md`
- [ ] No production code changes in this task
