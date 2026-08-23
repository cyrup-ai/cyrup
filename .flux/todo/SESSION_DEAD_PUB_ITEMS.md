---
stage: new
status: done
updated: 2026-08-23 00:47
---

# Remove The Two cyrup-session Pub Items That Cleared A Gap-Analysis Check

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

## Evidence

Reference counts across the whole workspace, and the gap-analysis clearance that decides
whether an unreferenced item is dead or merely unwired:

```bash
$ for s in from_snapshot NotPersisted estimate_entry EstimateKind SessionListProgress; do
    printf '%-20s refs=%s gap=%s\n' "$s" \
      "$(grep -rn "\b$s\b" crates --include='*.rs' | wc -l)" \
      "$(grep -rn "\b$s\b" docs/gap-analysis/ | wc -l)"
  done
from_snapshot        refs=1  gap=0     <- clear to remove
NotPersisted         refs=1  gap=0     <- clear to remove
estimate_entry       refs=1  gap=0     <- BUT see SESS-020 below
EstimateKind         refs=8  gap=1     <- SESS-020, closed: struck
SessionListProgress  refs=?  gap=4     <- open item in 08: struck
```

`refs=1` means the only occurrence is the item's own definition.

The two struck entries are why this task shrank from five items to two:

- `docs/gap-analysis/03-cyrup-session.md:171` — **SESS-020, closed**: *"`TokenCache` keyed
  `(EntryId, EstimateKind)` at `compaction/tokens.rs:200-268`; both projections cached
  independently and `invalidate` clears both. The two projections genuinely differ, so the
  split key is load-bearing."* `estimate_entry` is the `Rendered` projection — one of the two.
  Its zero external references are the normal state of a cache accessor, not evidence of death.
- `docs/gap-analysis/08-cyrup-session-svc-and-modes.md:609-615` — an **open** item on the
  `--resume` picker needing a current listing and an all-sessions listing rather than one merged
  list. `SessionListProgress` / `list_all_with_progress` is the seam that work would use.

See `.flux/README.md` § *Standing rule: this is a port, so "unused" ≠ "dead"*.
