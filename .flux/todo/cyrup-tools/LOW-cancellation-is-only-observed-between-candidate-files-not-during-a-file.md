---
title: Cancellation is only observed between candidate files, not during a file's search
priority: LOW
tool: grep
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: new
status: done
updated: 2026-08-27
---

# Cancellation is only observed between candidate files, not during a file's search

## What pi does

pi registers an abort listener that kills the ripgrep child immediately (`onAbort` → `stopChild()`, /home/user/cyrup/tmp/pi/packages/coding-agent/src/core/tools/grep.ts:246-250, :240-245), so an abort mid-file terminates the search at once and the promise rejects with `Operation aborted` (:305-307).

## What cyrup-tools does

/home/user/cyrup/crates/cyrup-tools/src/tools/grep.rs:391-395 checks `cancel` only at the top of each walk iteration and inside a `tokio::select!` parked on `walk.next()`; the actual per-file search (`search_one`, running `search_reader` inside `spawn_blocking`, :119-135) takes no cancel token and runs to completion. The limitation is acknowledged in the comment at :388-390.

## User-visible impact

Pressing Esc while grep is scanning a very large single file does not stop the work until that file is finished, so the abort takes visibly longer to take effect than in pi.

## Parity action

Thread the cancel token into `search_one` (e.g. have the `Sink` return `Ok(false)` once the token is cancelled) so an in-flight file search stops at the next matched/consumed chunk rather than at the next file boundary.

## Why this gap is real

An adversary agent was tasked with **refuting** this finding by locating the capability in the Rust under another name. It could not:

> Confirmed absent after searching both crates. search_one (grep.rs:73-137) takes no CancelToken and its blocking searcher.search_reader runs to completion inside spawn_blocking; the only cancel observations are grep.rs:343, :391-395 and the select! arm parked on walk.next() at :394-396. No cancel-aware io::Read wrapper exists anywhere in crates/ (no `impl io::Read` at all); LocalFs::read_stream (ops/local/fs.rs:73-80) returns a bare std::fs::File and the FsOps default (ops/mod.rs:341-343) a plain Cursor; MatchSink (grep.rs:243-260) has no token, so there is no per-chunk abort hook. Only one grep implementation exists (search_reader/grep_searcher appear only in grep.rs plus ops/tests), so there is no alternate implementation under another name. No outer race hides it either: cyrup-agent/src/agent/run/tools/exec.rs awaits tool.execute to completion in both the parallel (:127-137) and sequential (:246-284) paths and only checks cancel afterwards. Severity is low rather than higher because the blocking window is ONE file, not the whole run (pi's rg child spans the entire traversal, which is why it needs the listener), binary quit-detection and the remaining-match cap cut most large files short, results are never wrong, and the walk branch still returns error::aborted() on the next turn. The only behavioral divergence beyond latency is the single-file branch (grep.rs:342-360), where a mid-scan abort yields a normal successful result instead of pi's "Operation aborted" rejection — and the agent loop ends the turn as aborted regardless.

## Definition of done

1. The capability described under *Parity action* is implemented in `crates/cyrup-tools`.
2. A test pins the new behaviour against the pi semantics quoted above.
3. `cargo check --workspace --all-targets` and `cargo clippy` stay clean.
4. Behaviour that pi does NOT have is not introduced — this is a parity task, not a redesign.
