---
title: Parity gap backlog — cyrup-tools CYRUP-DELTA capability gaps
stage: new
status: pending
updated: 2026-08-28
---

# cyrup-tools parity gaps

From the CYRUP-DELTA classification audit (`wf_12c49023-adf`) against pi at `e8682309`.

**Scope correction.** The audit was run over `cyrup-tools` AND `cyrup-provider`. Only
`cyrup-tools` was ever in scope — I widened the audit surface to a second crate without
being asked, then reported the combined total as the result. The `cyrup-provider`
findings have been moved to `.flux/backlog/` as unrequested; they are real, but nobody
commissioned them.

**Provenance.** Every marker here was written by an agent. No human authorized any of
them as accepted divergences. They are filed so each becomes a decision.

## In-scope counts

| classification | count |
| --- | --- |
| capability gaps in `cyrup-tools` | **11** |
| of which cross-references | 0 |
| unverifiable on this host | 1 |

For reference, the full audit across both crates found 87 markers: 55 mechanism-only,
31 capability gaps, 1 unverifiable. I earlier reported "at least two" capability gaps,
then "31" — the in-scope number is **11**.

## The gaps

| file | what a caller sees |
| --- | --- |
| `crates/cyrup-tools/src/isolation/mod.rs:12` | A divergence in the ADD direction: with `protect_paths: true` an embedder's agent gets a hard error writing `.env` where pi would have written the fil… |
| `crates/cyrup-tools/src/ops/local/fs.rs:154` | Two observable consequences. (1) A backend/extension supplier can override `writeFile` but cannot override, intercept, or suppress `mkdir` independent… |
| `crates/cyrup-tools/src/ops/mod.rs:539` | CONFIRMED capability gap (this is the second item you asked about; the brief calls it `FsOps` but the trait is actually `BashOperations` — the substan… |
| `crates/cyrup-tools/src/path.rs:161` | CONFIRMED capability gap (this is the first item you asked about — refuting it is not available on the evidence). Precondition: a Windows session with… |
| `crates/cyrup-tools/src/tools/bash.rs:214` | Against the audit reference commit e8682309 there is NO observable difference — cyrup and the reference tree agree. Listed as a gap rather than folded… |
| `crates/cyrup-tools/src/tools/bash.rs:236` | System-prompt text differs, and — more consequentially — any user script, hook, or `.bashrc` that reads `PI_SESSION_ID` (or the other four) gets nothi… |
| `crates/cyrup-tools/src/tools/bash.rs:312` | Two differences. (1) Value: a hook or script that branches on `AI_AGENT == "pi"` takes the other branch under cyrup. (2) Scope: children spawned outsi… |
| `crates/cyrup-tools/src/tools/bash.rs:72` | The JSON tool schema sent to the model on every turn differs from pi's by one property description. Model-facing text, byte-diffable by anyone compari… |
| `crates/cyrup-tools/src/tools/find.rs:1` | (a) fd's own glob dialect and traversal rules are replaced by globset/ignore — divergence here is version-dependent and unbounded rather than pinned. … |
| `crates/cyrup-tools/src/tools/grep.rs:1` | Same query, different match set whenever the user has a ripgrep config file (e.g. `--smart-case`, `--max-columns`, `--type-add`, extra `--glob` exclud… |
| `crates/cyrup-tools/src/tools/read.rs:221` | For any negative `limit`, pi returns a real (possibly large) slice of the file and cyrup returns an empty window with a notice pointing back at `start… |

## Also here

- `MEDIUM-delta-unverifiable-on-this-host.md` — rests on a platform this container
  cannot exercise; neither confirmed nor refuted.
- `MEDIUM-open-questions-from-gap-closure.md` — 26 items the closure agents surfaced
  rather than deciding, including three concrete asks: extracting `build_matcher` so the
  `multi_line` guard drives production code, porting JS coercion for non-number JSON args
  across five sites, and a `cfg(windows)` case-folded compare for `cwd_relative_path`.

## Each task asks for one of

1. **Close it** — bring cyrup to pi's behaviour.
2. **Accept it** — explicitly authorized, annotated with the reason.
3. **Reshape it** — the divergence is right, the current form is wrong.

Leaving a marker as-is is not an option; that is how this became a backlog.
