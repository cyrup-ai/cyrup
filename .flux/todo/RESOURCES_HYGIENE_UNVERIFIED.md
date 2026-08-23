---
stage: new
status: done
updated: 2026-08-22 23:09
---

# cyrup-resources Hygiene Backlog — Findings That Were Never Verified

## Why this file exists

The hygiene audit ran 8 finder lenses and produced 28 findings, then queued an adversarial verifier
per finding. **26 of the 28 verifiers died on a session limit.** Only two findings were adversarially
verified by the workflow; I verified a further set by hand afterwards, and those became their own
task files.

Everything below is **finder output that nobody checked**. Treat each as a lead, not a defect. The
first job for any of these is to reproduce the evidence — the finders were accurate on the items I
did spot-check, but an unverified finding has a real chance of being taste, scope creep, or plain
wrong, and each one costs a task file and a reviewer's attention if it is wrong.

Run `/aug` on this file once the session limit resets to verify them properly before promoting any
to their own task.

## Ready — verified by hand, just small

### Dead `after` binding in `prompt.rs:367-371`

```rust
if let Some(after) = rest.strip_prefix("ARGUMENTS") {
    let consumed = 1 + "ARGUMENTS".len();
    let _ = after;                      // bound solely to be discarded
    return Some((all_args.to_string(), consumed));
}
if rest.starts_with('@') {              // <- the branch below already does it right
```

Confirmed by reading the file. **Fix:** `if rest.starts_with("ARGUMENTS") { return Some((all_args.to_string(), 1 + "ARGUMENTS".len())); }`
— identical behavior, matches the sibling branch three lines down. Touches `src/prompt.rs` only, which
no other queued task owns.

## Unverified — module-size leads

| id | Sev | Effort | Claim |
| --- | --- | --- | --- |
| `split-theme-module` | medium | M | `src/theme.rs` (837 lines) has extractable seams |
| `extract-install-git-plumbing` | medium | M | git plumbing inside `src/package/install.rs` (639) wants its own module |
| `split-manifest-pattern-engine` | low | M | the pattern engine in `src/package/manifest.rs` (729) is separable |

For each: a file being long is not a finding. Name the actual submodules and their line ranges, or
drop it. That is the bar the `discovery.rs` split cleared and it is why that one is a task.

## Unverified — the one that blocks a queued task

| id | Sev | Effort | Claim |
| --- | --- | --- | --- |
| `bundle-discovery-output-sinks-into-a-struct` | low | L | replace the six `&mut` accumulators threaded through `discover_blocking` with one sink struct |

**This matters for scheduling.** The verified `DISCOVERY_RS_DECOMPOSE` finding lists this as
`blocked_by` — the two edit the same signatures, so doing them in the wrong order means redoing one.

But the blocker is itself unverified, low severity, and L effort, while the thing it blocks is high
severity. **Do not let an unverified low-severity L block a verified high-severity task.** Either
verify this first and sequence properly, or drop it and do the decomposition as a pure move — the
sink-struct refactor can follow on the new layout, where it is easier anyway because each helper's
`&mut` needs are then explicit.

## Unverified — error handling and docs

| id | Sev | Effort | Claim |
| --- | --- | --- | --- |
| `theme-error-variant-misuse` | medium | S | three `ThemeWatcher` sites in `theme.rs` report watcher/lock failures through `ResourceError::Theme`, whose Display is `malformed theme {path}: {reason}` — implying a parse failure that did not happen |
| `unify-missing-path-warning-channel` | medium | S | `discovery.rs` reports "`<kind>` path does not exist" from three sites with identical wording but two different sinks (`diagnostics` vs `warnings`) |
| `narrow-manifest-error-catchall` | medium | M | a catch-all arm in manifest error mapping is too broad |
| `document-public-enum-variants` | low | M | several public enum variants carry no doc comment |

The first two look substantive and are cheap to check — start there.

## Unverified — test layout

| id | Sev | Effort | Claim |
| --- | --- | --- | --- |
| `relocate-public-api-test-mods-to-src-tests` | medium | M | the in-src `#[cfg(test)]` modules in `package/{store,git_url,manifest}.rs` test public API and belong under `src/tests/` |

Weigh this carefully. In-src test modules are the right home for tests that touch private items;
moving them is only correct for ones that genuinely exercise the public surface. Check what each
actually reaches before proposing a move — and note that `RESOURCES_LINT_SUPPRESSIONS` already
normalizes their allow lists, so land that first or this conflicts with it.

## Acceptance Criteria

- [ ] Each lead above is either reproduced with concrete `file:line` evidence and promoted to its own
      task, or struck with a one-line reason
- [ ] The `bundle-discovery-output-sinks-into-a-struct` ordering question is settled before
      `DISCOVERY_RS_DECOMPOSE` starts
- [ ] The `prompt.rs` dead binding is fixed (it needs no further verification)
