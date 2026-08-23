---
stage: exec
status: done
updated: 2026-08-22 20:05
---

# Scope 13i — Protocol Tracer, Conformance And Verification

## Description

`13i` is the weakest surface in the port: **31 of its 50 units have no implementation at all**,
and 16 of its open units are critical-or-high (`docs/gap-analysis/13-cyrup-mcp-STATUS.md:369`).

This is a **scoping task, not an implementation task**. Every other open section is realigning
something that already exists; 13i means building absent surfaces, which is a different job and
should not be started by picking units off a list. The output is a plan, not code.

Produce:

1. A dependency order — several 13i units are verification harnesses for other sections and are
   worthless before those land. `MCP-498` explicitly notes its child-process harness cannot
   assert the cold-cache case until HA-1 exists (`13i-mcp-protocol-and-verification.md:1648`).
2. A split between what is genuinely missing and what is present but unrecognised. The audit's
   own skeptic pass overturned 15 rulings workspace-wide, so a `missing` row is a lead, not a
   verdict — confirm before scheduling.
3. A recommendation on the `host-addition` neighbours 13i depends on (`HA-1` late tool
   registration, `HA-2` argument completions, `HA-3` overlay geometry —
   `13i-mcp-protocol-and-verification.md:1718`).
4. Waves sized like the ones that worked in PR #30: grouped by shared obligation, not by file.
   Splitting by file is what put `runtime.rs` in a different agent's set than the unit whose
   obligation needed it, so the agent that found the `has_ui` bug could not fix it.

## Acceptance Criteria

- [ ] Every one of the 42 open 13i units is triaged as confirmed-missing, actually-present, or blocked-on-X
- [ ] A dependency order exists, naming which units are gated on HA-1/HA-2/HA-3 or on other sections
- [ ] Waves are proposed and sized, grouped by obligation
- [ ] The findings are written into `13-cyrup-mcp-STATUS.md` so the triage is not re-derived later
- [ ] No production code changes in this task
