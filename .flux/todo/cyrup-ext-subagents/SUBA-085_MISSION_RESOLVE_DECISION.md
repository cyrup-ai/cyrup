---
stage: new
status: done
updated: 2026-08-27 05:30
severity: high
effort: small
subsystem: missions
source: docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md
item: SUBA-085
---

> **Path note.** This task lives in a subdirectory. Every flux command globs a single level (`ls -1 "$FLUX_BASE/todo/"*.md`), so `/exec`, `/aug` and `/qa` will not list it — pass the absolute path explicitly.

# SUBA-085 — `mission.resolve-decision` unported: a mission decision is write-once and permanently open, so the goal driver proposes the same next action forever

**Kind** not-ported · **Severity** high · **Effort** S · **Confidence** confirmed
**Subsystem** missions
**Window** v0.43.0..v0.47.1 (`1dec33dd feat: add mission dispatch ledger`).

**upstream** — `git show v0.57.0:src/missions/actions.ts` **`:32-39`** `MISSION_ACTIONS` has **seven**
entries — `mission.create`, `mission.list`, `mission.show`, `mission.update`,
**`mission.resolve-decision`**, `mission.attach-run`, `mission.close` — and the handler at
**`:391-397`**:
```ts
if (action === "mission.resolve-decision") {
	const missionId = requireMissionId(params);
	const decisionId = validateMissionId(params.id, "id");
	if (typeof params.summary !== "string" || !params.summary.trim())
		throw new Error("mission.resolve-decision requires a non-empty summary");
	const record = updateMission(location, missionId, { resolveDecision: { id: decisionId, resolution: params.summary.trim() } });
	return textResult(`Resolved decision ${decisionId} for mission ${record.id}. …`);
}
```
`MissionUpdateInput` carries `resolveDecision?: { id: string; resolution: string }`, and the verb is
listed in `MUTATING_MANAGEMENT_ACTIONS` (`subagent-executor.ts`) and in `SUBAGENT_ACTIONS`
(`shared/types.ts`).

**cyrup** — `grep -rn 'resolve_decision\|ResolveDecision' --include=*.rs crates/cyrup-ext-subagents/src`
→ **0 hits**. `src/missions/types.rs:700-717 MissionUpdateInput` carries
`add_decisions: Vec<MissionDecisionInput>` with the doc *"Append decisions (always as NEW, open
decisions with fresh ids)"* and has **no** `resolve_decision` field; `is_empty()` at `:721-737`
enumerates every field and confirms the set is closed. `MissionDecision` does carry
`status: Open|Resolved`, `resolved_at` and `resolution`, but `MissionDecisionStatus::Resolved` is
produced at **exactly one site** — `src/missions/store.rs:355`, the on-disk PARSER
(`Some("resolved") => …`) — never by a mutation. `src/extension/tool/text.rs:187-229` advertises six
`mission.*` verbs, not seven.

**Impact** — In cyrup a mission decision can be **opened and never closed**.
`src/missions/goal_driver.rs:382-394` computes the mission's next ready action as
`record.decisions.iter().find(|item| item.status == MissionDecisionStatus::Open)` — and since nothing
can flip that status, a mission that ever records one decision returns that same decision as its next
ready action on every subsequent evaluation, and its autonomous progression is wedged. Upstream clears
it with one `mission.resolve-decision` call. There is no workaround under another name: `mission.update`
can only append new open decisions. `high` not `critical`: nothing is lost (the decision persists
correctly, it simply cannot be closed), there is no bypass and no panic — it is a functional stall of
autonomous progression plus a permanently stale continuation notice.

**Fix** — Add `resolve_decision: Option<MissionDecisionResolution>` to `MissionUpdateInput` (and to
`is_empty()`), implement the find/guard/mutate block in `store.rs` mirroring upstream's
(`status = Resolved`, `resolved_at`, `resolution`), add the seventh enum variant and its wire strings
in `missions/actions.rs`, `extension/tool/text.rs` and `extension/tool/schema.rs`, and reproduce the
non-empty-summary and unknown-id errors verbatim.

**Verify** — Create a mission, record one decision, resolve it, and assert `goal_driver`'s next ready
action moves past it; a `mission.resolve-decision` with an empty `summary` must fail with upstream's
message; one with an unknown decision id must fail rather than silently no-op.

**Relation to corpus** — Discharges one of the seven unowned verbs `SUBA-005` (tracker) explicitly
owes an owner for. `SUBA-005` proposes no schedulable work by its own reclassification, so this is
the first schedulable filing of the behaviour and is not a duplicate of a counted row.

---

## Scope

In scope: the behaviour described above, in `crates/cyrup-ext-subagents/`.

Out of scope: the other SUBA items in this batch (each has its own file); any refactor beyond what the fix needs; and the ledger corrections in `SUBA-CORPUS-HEALTH.md`.

Full finding, with the complete evidence chain: [`09a-cyrup-ext-subagents-v0.57-drift.md` § SUBA-085](../../../docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md)

## Approach

Port `mission.resolve-decision` so a mission decision can be resolved rather than being write-once
and permanently open, and advertise the verb in the tool schema.

## Acceptance Criteria

- [ ] `mission.resolve-decision` exists and is advertised in the tool schema
- [ ] A resolved decision stops the goal driver re-proposing the same next action
- [ ] `cargo test -p cyrup-ext-subagents` passes
