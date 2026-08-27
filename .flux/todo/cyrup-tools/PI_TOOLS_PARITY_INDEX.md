---
title: Pi Tools Parity Backlog
stage: new
status: done
updated: 2026-08-27
---

# Parity Backlog — `cyrup-tools` vs pi `coding-agent` tools

## The headline

`cyrup-tools` is a Rust reimplementation of pi's built-in tool set. This backlog is **25 tasks**
covering every caller-visible capability difference found between the two, audited against
pi at `e868230` (2026-08-26, the `earendil-works/pi-mono` monorepo — the standalone `pi` repo now
redirects there).

**Implementation differences are deliberately excluded.** tokio vs promises, rayon vs sequential
iteration, different error types and different module layout are all intended and were discarded on
sight. Every task here is a difference a *caller* can observe: a missing parameter, an unsupported
mode, an omitted output field, an absent guardrail, or a divergent limit.

## Severity

| Severity | Count |
| --- | --- |
| medium | 11 |
| low | 14 |
| **total** | **25** |

No critical or high findings. The largest single item is the missing `powershell` tool; the rest are
behavioural divergences in traversal, matching and error reporting.

## How this backlog was produced

A 57-agent workflow (`wf_e427a266-e16`). Seven lanes each read one tool family on both sides in full
and reported candidate gaps with citations on both sides. Every candidate then went to a **paired
adversary** whose only job was to refute it by finding the capability in the Rust under another name.

**50 candidates in, 30 confirmed, 20 refuted — a 40% kill rate.** The refuted ones are listed at the
bottom so the same ground is not re-covered. Several survivors had their severity corrected *down* by
the adversary; those corrections are quoted in each task file under *Why this gap is real*.

Two duplicate families were merged by hand after the run:

- `powershell` was reported by two lanes at different severities. Merged; both severity arguments are
  preserved in the task file.
- `constrainedSampling` was reported five times (once per tool, once cross-cutting). It is one
  capability declared at four call sites, so it is one task. Note its adversary **partially refuted**
  the original framing — the pipeline already exists in Rust; only the per-tool declaration is missing.

## The backlog

| Severity | Tool | Task |
| --- | --- | --- |
| medium | `edit` | [Edits sent as a bare single edit object is rejected instead of wrapped into a one-element array](./MEDIUM-edits-sent-as-a-bare-single-edit-object-is-rejected-instead-of-wrapped-i.md) |
| medium | `edit` | [Edits sent as a JSON string that parses to a single edit object is left unwrapped](./MEDIUM-edits-sent-as-a-json-string-that-parses-to-a-single-edit-object-is-left.md) |
| medium | `find` | [A single unreadable directory aborts the whole find; pi returns the results it collected](./MEDIUM-a-single-unreadable-directory-aborts-the-whole-find-pi-returns-the-resul.md) |
| medium | `find` | [Find accepts a search path that is a regular file and reports "No files found"; pi rejects it](./MEDIUM-find-accepts-a-search-path-that-is-a-regular-file-and-reports-no-files-f.md) |
| medium | `find` | [Find glob matching is always case-sensitive; pi (fd) applies smart-case by default](./MEDIUM-find-glob-matching-is-always-case-sensitive-pi-fd-applies-smart-case-by.md) |
| medium | `grep` | [A pattern containing a newline errors in pi but silently yields "No matches found" in cyrup](./MEDIUM-a-pattern-containing-a-newline-errors-in-pi-but-silently-yields-no-match.md) |
| medium | `grep` | [Glob: "!dir" prunes the whole directory in pi, but only filters files in cyrup](./MEDIUM-glob-dir-prunes-the-whole-directory-in-pi-but-only-filters-files-in-cyru.md) |
| medium | `grep` | [Grepping a binary file by explicit path returns no matches in cyrup; pi returns the matching lines](./MEDIUM-grepping-a-binary-file-by-explicit-path-returns-no-matches-in-cyrup-pi-r.md) |
| medium | `grep` | [Symlinked files inside the search tree are searched by cyrup but skipped by pi](./MEDIUM-symlinked-files-inside-the-search-tree-are-searched-by-cyrup-but-skipped.md) |
| medium | `powershell` | [The entire powershell built-in tool is missing from cyrup](./MEDIUM-the-entire-powershell-built-in-tool-is-missing-from-cyrup.md) |
| medium | `write` | [Same-path mutation lock is not granted in dispatch order](./MEDIUM-same-path-mutation-lock-is-not-granted-in-dispatch-order.md) |
| low | `all` | [Rendered tool paths are not OSC-8 hyperlinks](./LOW-rendered-tool-paths-are-not-osc-8-hyperlinks.md) |
| low | `bash` | [A UTF-8 BOM in command output is stripped by pi but retained by cyrup](./LOW-a-utf-8-bom-in-command-output-is-stripped-by-pi-but-retained-by-cyrup.md) |
| low | `bash` | [Bash child processes are not spawned with the console-window suppression flag on Windows](./LOW-bash-child-processes-are-not-spawned-with-the-console-window-suppression.md) |
| low | `bash` | [On Windows with no bash installed, pi's actionable No bash shell found error is replaced by an opaque spawn failure](./LOW-on-windows-with-no-bash-installed-pi-s-actionable-no-bash-shell-found-er.md) |
| low | `bash` | [Shell detection is cached at tool construction instead of re-resolved on every command](./LOW-shell-detection-is-cached-at-tool-construction-instead-of-re-resolved-on.md) |
| low | `find` | [Find does not honor .fdignore or fd's global ignore file](./LOW-find-does-not-honor-fdignore-or-fd-s-global-ignore-file.md) |
| low | `grep` | [Cancellation is only observed between candidate files, not during a file's search](./LOW-cancellation-is-only-observed-between-candidate-files-not-during-a-file.md) |
| low | `grep` | [.rgignore files are honored by pi but ignored by cyrup](./LOW-rgignore-files-are-honored-by-pi-but-ignored-by-cyrup.md) |
| low | `ls` | [Ls does not observe an already-fired cancellation before touching the filesystem](./LOW-ls-does-not-observe-an-already-fired-cancellation-before-touching-the-fi.md) |
| low | `read` | [Pi aborts a read the instant the signal fires; cyrup checks the cancel token exactly once and never during the file read](./LOW-pi-aborts-a-read-the-instant-the-signal-fires-cyrup-checks-the-cancel-to.md) |
| low | `read` | [Pi's third compact read header kind docs is not implemented](./LOW-pi-s-third-compact-read-header-kind-docs-is-not-implemented.md) |
| low | `read` | [The :offset-limit header range disappears when offset/limit arrive as JSON floats](./LOW-the-offset-limit-header-range-disappears-when-offset-limit-arrive-as-jso.md) |
| low | `read/bash/edit/write` | [Built-in tools never declare constrainedSampling](./LOW-built-in-tools-never-declare-constrainedsampling.md) |
| low | `write` | [Write/mkdir failure messages drop the errno code and Node's message shape](./LOW-write-mkdir-failure-messages-drop-the-errno-code-and-node-s-message-shap.md) |

## Refuted — do not re-open without new evidence

These 20 candidates were reported by a lane and then **killed** by an adversary that located
the capability in the Rust. Each is listed with where it was found.

| Tool | Claim | Found at |
| --- | --- | --- |
| `bash` | Windows process-tree kill invokes bare `taskkill` instead of the absolute System32 path | `/home/user/cyrup/crates/cyrup-tools/src/ops/local/signal.rs:37-48` |
| `bash` | The spawn hook cannot replace or clear the child environment, only add and delete named keys | `/home/user/cyrup/crates/cyrup-tools/src/ops/local/command.rs:22-27 (with /home/user/cyrup/crates/cyrup-tools/src/config.rs:21-26 and /home/user/cyrup/crates/cyrup-tools/src/tools/bash.rs:248-258, 308-311)` |
| `bash` | The `command` parameter description sent to the model differs | `/home/user/cyrup/crates/cyrup-tools/src/tools/bash.rs:69` |
| `bash` | The `truncated` flag / truncation footer keys off raw bytes where pi keys off decoded bytes | `/home/user/cyrup/crates/cyrup-tools/src/output.rs:211` |
| `bash` | The final streaming update omits the `details` object and is emitted even when nothing changed | `/home/user/cyrup/crates/cyrup-tools/src/tools/bash.rs:597-611 (dirty gate), :536-563 (details object), :467-471 (final settle emit)` |
| `edit` | Edit-preview file-access failure reports the raw OS error text instead of pi's `Error code: E…` form | `/home/user/cyrup/crates/cyrup-tools/src/error.rs:100` |
| `edit` | Pre-execution edit diff preview is silently skipped for files over 4 MiB | `/home/user/cyrup/crates/cyrup-tools/src/tools/edit_diff.rs:643` |
| `edit` | A call cancelled during the access/read window surfaces a file error instead of `Operation aborted` | `/home/user/cyrup/crates/cyrup-tools/src/lock.rs:175-186 (via /home/user/cyrup/crates/cyrup-tools/src/tools/edit.rs:223)` |
| `edit` | Preview gives up when the `edits` array holds a malformed entry instead of falling back to top-level oldText/newText | `/home/user/cyrup/crates/cyrup-tui/src/app/event_extract.rs:157` |
| `read` | A negative `limit` returns file content in pi but an empty result in cyrup | `/home/user/cyrup/crates/cyrup-tools/src/tools/read.rs:189-198` |
| `read` | pi coerces string-valued `offset`/`limit`; cyrup fails the entire call at deserialization | `/home/user/cyrup/crates/cyrup-provider/src/validate.rs:310` |
| `read` | `limit: 0` renders a bogus `-0` end in the header range | `/home/user/cyrup/crates/cyrup-tui/src/transcript/tool_args.rs:58` |
| `write` | No cancellation check between parent-directory creation and the file write | `/home/user/cyrup/crates/cyrup-tools/src/tools/write.rs:103` |
| `powershell` | System prompt has no PowerShell file-exploration guideline variants | `crates/cyrup-session/src/prompt/builder.rs:224` |
| `all` | `shortenPath` resolves the home directory from `HOME` only, not the OS home | `crates/cyrup-tools/src/path.rs:88` |
| `all` | The base directory is joined verbatim — pi normalizes it (tilde, Windows drive form, relative base) | `crates/cyrup-config/src/paths.rs:139` |
| `cli @file` | CLI `@file` resolution reimplements a reduced subset of `resolveReadPath` | `crates/cyrup-tools/src/path.rs:281` |
| `bash` | Appending to a finished output accumulator is silently accepted instead of erroring | `crates/cyrup-tools/src/tools/bash.rs:387` |
| `all (image results)` | `[Image: …]` stand-in dimensions come from a full pixel decode, not a header parse | `crates/cyrup-tui/src/transcript/images.rs:95` |
| `read (image results)` | Image stand-in is rendered even when the tool's own renderer produces no output | `crates/cyrup-tui/src/transcript/images.rs:37` |

## Caveats worth reading before starting

- **The audit compared `packages/coding-agent/src/core/tools` only.** pi has a second, smaller tool
  set at `packages/agent/src/harness/tools` (which additionally has `image.ts`) that was not diffed.
- Severities are the adversary's corrected values, not the finder's.
- Several tasks note the fix is **not local** to the tool file — the grep symlink task, for example,
  needs a file-type field added to `WalkItem` first.
