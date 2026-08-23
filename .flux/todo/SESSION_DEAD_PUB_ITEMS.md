---
stage: new
status: done
updated: 2026-08-23 00:35
---

# Remove the two cyrup-session pub items that cleared a gap-analysis check

> **RESCOPED 2026-08-23 00:35.** This task originally proposed removing five items. Three of them
> — `TokenCache::estimate_entry`, `TokenCache::invalidate` and `EstimateKind` — are the
> implementation of **SESS-020, a CLOSED parity item**, which states: *"`TokenCache` keyed
> `(EntryId, EstimateKind)`; both projections cached independently and `invalidate` clears
> both. **The two projections genuinely differ, so the split key is load-bearing.**"*
> Removing them would re-open closed parity work. They are struck from this task permanently.
> **Priority:** low · **Effort:** small

## Description

Two `pub` items in `crates/cyrup-session` have zero references anywhere in the workspace
outside their own definition, **and** zero mentions in `docs/gap-analysis/`:

| Item | Refs | gap-analysis hits |
| --- | --- | --- |
| `ContextStore::from_snapshot` | 1 (its own definition) | 0 |
| `SessionError::NotPersisted` | 1 (its own definition) | 0 |

Both clearances were verified with
`grep -rn '\bNAME\b' docs/gap-analysis/` returning nothing.

## Do NOT touch

- `TokenCache::estimate_entry`, `TokenCache::invalidate`, `EstimateKind` — SESS-020, closed.
- `SessionListProgress` / `list_all_with_progress` — 4 gap-analysis hits including an OPEN
  item in `08-cyrup-session-svc-and-modes.md` about the `--resume` picker needing both a
  current and an all-sessions listing.

## Acceptance Criteria

- [ ] `ContextStore::from_snapshot` and `SessionError::NotPersisted` are removed
- [ ] `grep -rn 'from_snapshot\|NotPersisted' crates/ --include='*.rs'` returns 0 hits
- [ ] `EstimateKind`, `estimate_entry`, `invalidate` and `SessionListProgress` are UNCHANGED
- [ ] `cargo check --all-targets -p cyrup-session` clean; `cargo test -p cyrup-session` passes
- [ ] Before removing anything, re-run the gap-analysis clearance grep and record the result
