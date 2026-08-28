---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/tools/grep.rs:1"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: new
status: pending
updated: 2026-08-28
---

# Capability gap: `crates/cyrup-tools/src/tools/grep.rs:1`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

pi spawns the real ripgrep binary (grep.ts:177 `ensureTool("rg")`, :226 `spawn(rgPath, args)`) with args `--json --line-number --color=never --hidden [--ignore-case] [--fixed-strings] [--glob G] -- PATTERN PATH` (grep.ts:218-224). It does NOT pass `--no-config`, so ripgrep loads `$RIPGREP_CONFIG_PATH` and applies whatever the user put there. pi also exposes `GrepOperations { isDirectory, readFile }` (grep.ts:56-61) as an override point, and downloads rg on demand.

## What cyrup does

In-process `grep_regex`/`grep_searcher`/`ignore`. No external process, no `$RIPGREP_CONFIG_PATH`, no `ensureTool` download, no rg version in the loop.

## What a caller sees

Same query, different match set whenever the user has a ripgrep config file (e.g. `--smart-case`, `--max-columns`, `--type-add`, extra `--glob` excludes) — pi honours it, cyrup ignores it silently. Error strings diverge too: pi can emit `ripgrep (rg) is not available and could not be downloaded`, `Failed to run ripgrep: ...`, `ripgrep exited with code N`; cyrup never emits any of these. Regex-dialect skew between the installed rg and the linked grep-regex crate is a second, open-ended source of differing results.

## Decision required

One of:

1. **Close it** — bring cyrup to pi's behaviour.
2. **Accept it** — David explicitly accepts the divergence; the marker stays and is
   annotated as authorized, with the reason.
3. **Reshape it** — the divergence is right but the current form is wrong.

Do not silently keep option 2 by leaving the marker as-is; that is how this became a
backlog in the first place.

## Definition of done

1. The gap is closed, or the marker records an explicit authorized acceptance.
2. If closed, a test fails without the change.
3. No behaviour regression in the owning crate.
