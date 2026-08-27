---
title: Ls does not observe an already-fired cancellation before touching the filesystem
priority: LOW
tool: ls
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: new
status: done
updated: 2026-08-27
---

# Ls does not observe an already-fired cancellation before touching the filesystem

## What pi does

/home/user/cyrup/tmp/pi/packages/coding-agent/src/core/tools/ls.ts:118-125 — the FIRST statements inside the executor are `if (signal?.aborted) { reject(new Error("Operation aborted")); return; }` followed by an `{ once: true }` abort listener that rejects with `Operation aborted` the instant the signal fires. So an `ls` whose signal is already aborted rejects with `Operation aborted` before `resolveToCwd`, before `ops.exists`, before `ops.stat` and before `ops.readdir` (ls.ts:129-152), and a cancel arriving mid-`readdir` rejects immediately rather than waiting for it.

## What cyrup-tools does

/home/user/cyrup/crates/cyrup-tools/src/tools/ls.rs:82-98 — `execute` starts with `serde_json::from_value`, then `fs.metadata` (which can return `Path not found:` / `Not a directory:`), then `fs.read_dir` at ls.rs:111-115, with no `cancel.is_cancelled()` guard anywhere before them. The token is first observed only at ls.rs:140, inside the per-entry stat loop. This is the exact defect that WAS fixed for the sibling tool — find.rs:115-117 carries `if cancel.is_cancelled() { return Err(error::aborted()); }` as its first statement, documented at find.rs:108-114 and pinned by /home/user/cyrup/crates/cyrup-tools/src/tests/find_abort.rs — but the fix was never applied to `ls`.

## User-visible impact

An `ls` dispatched with an already-cancelled token reports the wrong outcome: on a missing or non-directory path it returns `Path not found: <p>` / `Not a directory: <p>` instead of `Operation aborted`, and on an empty directory it returns `(empty directory)` as a success. A cancel (Esc) landing while `read_dir` is enumerating a huge or slow directory is not observed until the enumeration finishes, so the tool keeps working after the user cancelled.

## Parity action

Add `if cancel.is_cancelled() { return Err(error::aborted()); }` as the first statement of `LsTool::execute` (before the `serde_json::from_value` at ls.rs:83), matching find.rs:115-117, and race the `read_dir` await against `cancel.cancelled()` so a cancel arriving during enumeration is observed promptly.

## Why this gap is real

An adversary agent was tasked with **refuting** this finding by locating the capability in the Rust under another name. It could not:

> Confirmed absent. ls.rs:82-115 runs from_value, resolve_to_cwd, fs.metadata (line 87), the is_dir rejection (line 93) and fs.read_dir (line 111) with no token observation; the first cancel.is_cancelled() is line 140 inside the per-entry loop. I checked every alternate location the capability could live: the sibling tools all have the first-statement guard (find.rs:115, write.rs:103, edit.rs:263, read.rs:137, grep.rs:343, bash.rs:293), so it is not a stylistic difference; the registered-tool wrapper (cyrup-ext/src/wrapper.rs) only derives addedToolNames and registry.rs:98 registers LsTool bare; the agent layer (cyrup-agent/src/agent/run/tools/exec.rs) tests the token only AFTER each call (lines 90, 328) and its own comment at :96-99 says deferred calls are still started after an abort, and neither the parallel joinset nor the sequential select! races the tool future against the token, so there is no drop-based cancellation either; the FS layer is cancel-blind (ops/local/fs.rs:170-207 — metadata and read_dir take no token and read_dir drains next_entry to completion, so a mid-enumeration cancel is unobservable). No test pins it: find_abort.rs covers only find, and tools.rs:2062 abort_message_is_capitalized exercises write despite naming ls.ts in its comment. Severity lowered to low: the line-140 guard fires on the first loop iteration ahead of the limit check, so every non-empty directory (including limit<=0) already returns Operation aborted. The divergence is confined to an already-cancelled ls on an empty directory ((empty directory) reported as success), a missing path, a non-directory, or an unreadable directory — a wrong message string on a turn the user already cancelled — plus one wasted metadata/read_dir pair. Nothing is silently wrong on a non-cancelled run and no state is mutated.

## Definition of done

1. The capability described under *Parity action* is implemented in `crates/cyrup-tools`.
2. A test pins the new behaviour against the pi semantics quoted above.
3. `cargo check --workspace --all-targets` and `cargo clippy` stay clean.
4. Behaviour that pi does NOT have is not introduced — this is a parity task, not a redesign.
