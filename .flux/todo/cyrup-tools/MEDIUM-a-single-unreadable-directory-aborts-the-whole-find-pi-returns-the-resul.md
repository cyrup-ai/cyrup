---
title: A single unreadable directory aborts the whole find; pi returns the results it collected
priority: MEDIUM
tool: find
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: exec
status: in-progress
updated: 2026-08-27
---

# A single unreadable directory aborts the whole find; pi returns the results it collected

## Core objective

`find` must survive a filesystem error met **during** traversal and return the paths it already
collected, because fd does. `grep` must **not** — because ripgrep does not, and pi's grep rejects on
ripgrep's exit code.

Both tools share one producer, [`LocalFs::walk`](../../../crates/cyrup-tools/src/ops/local/fs.rs),
which pushes per-entry `ignore::Error`s down the same channel as results. Today both consumers do
`Some(Err(e)) => return Err(e)`. For `find` that is wrong. For `grep` it is right in outcome but
wrong in *timing* and in *message*. This task fixes `find`, corrects `grep`'s timing and message,
and states the divergence in the code so the next reader does not "unify" the two arms.

## Ordering and sibling dependencies

Two finished briefs already prescribe edits to this same walker:

* [Find does not honor .fdignore or fd's global ignore file](./LOW-find-does-not-honor-fdignore-or-fd-s-global-ignore-file.md)
  (`stage: aug`, `status: done`) — adds `WalkFlavor { Plain, Fd, Rg }` to
  [`WalkOpts`](../../../crates/cyrup-tools/src/ops/mod.rs) and rewrites the **`WalkBuilder`
  construction** in `LocalFs::walk` from a single chained expression into
  `let mut builder = …; … let walker = builder.build();`.
* [.rgignore files are honored by pi but ignored by cyrup](./LOW-rgignore-files-are-honored-by-pi-but-ignored-by-cyrup.md)
  (`stage: aug`, `status: done`) — one match arm in `WalkFlavor::custom_ignore_filename`, stacked on
  the fd brief.

**There is no ordering dependency in either direction.** That pair edits the builder
(`fs.rs:213-226`); this task edits the `for result in walker { … }` loop **below** it
(`fs.rs:227-239`) — the fd brief says in as many words that that loop "is untouched". The hunks are
disjoint. If the fd task has already landed when this one is picked up, the loop will have shifted
down by roughly twenty lines: anchor on the `Err(e) => Err(ToolError::new(format!("walk: {e}")))`
expression, not on the line number.

**No change to `WalkItem` or to the stream's item type is required or permitted here.** The stream
stays `EventStream<Result<WalkItem, ToolError>>`, and `WalkItem` keeps exactly its two fields. The
tolerance is entirely a consumer-side decision, which is the whole reason it has to be: the two
consumers want opposite things from the same `Err`. (The separate
[symlink brief](./MEDIUM-symlinked-files-inside-the-search-tree-are-searched-by-cyrup-but-skipped.md)
*does* need a new `WalkItem` field; it is independent of this one and can land in either order.)

## Upstream behaviour — verified

### fd tolerates every traversal error, and there is no variant it treats as fatal

pi's `find` spawns the real `fd` binary
([find.ts:225](../../../tmp/pi/packages/coding-agent/src/core/tools/find.ts) `ensureTool("fd")`,
argv assembled at [find.ts:234-267](../../../tmp/pi/packages/coding-agent/src/core/tools/find.ts)):

```ts
const args: string[] = ["--glob", "--color=never", "--hidden"];
if (!insideGitRepo) args.push("--no-require-git");
args.push("--max-results", String(effectiveLimit));
// ... --full-path when the pattern contains "/"
args.push("--", effectivePattern, searchPath);
```

That argv contains **no `--show-errors`**. In fd's walker every `Err` the `ignore` iterator produces
is handed to the results channel and traversal continues
([fd 10.5.0 `src/walk.rs:500-505`](../../../tmp/ref/fd/walk.rs)):

```rust
Err(err) => {
    return match tx.send(WorkerResult::Error(err)) {
        Ok(_) => WalkState::Continue,
        Err(_) => WalkState::Quit,
    };
}
```

The receiver does nothing with it unless `--show-errors` was passed
([`src/walk.rs:227-231`](../../../tmp/ref/fd/walk.rs), fed from `opts.show_errors` at
[`src/main.rs:386`](../../../tmp/ref/fd/main.rs)):

```rust
WorkerResult::Error(err) => {
    if self.config.show_filesystem_errors {
        print_error(err.to_string());
    }
}
```

and the exit code is unconditionally success in non-`--quiet` mode
([`src/walk.rs:282-292`](../../../tmp/ref/fd/walk.rs)):

```rust
fn stop(&mut self) -> Result<(), ExitCode> {
    if self.mode == ReceiverMode::Buffering { self.buffer.sort(); self.stream()?; }
    if self.config.quiet { Err(ExitCode::HasResults(self.num_results > 0)) }
    else { Err(ExitCode::Success) }
}
```

**The precise answer to "which `ignore::Error` variants does fd tolerate": all of them.** fd's error
arm is a catch-all — it never inspects the variant. `WithPath { err: Io(EACCES) }` (an unreadable
directory), `WithPath { err: Io(EIO) }` (a dead mount), `WithDepth { err: Loop { .. } }` (a symlink
cycle), `Partial(..)` and `WithLineNumber { .. }` (a malformed pattern in an ignore file), bare
`Io(..)` from `add_parents` — every one of them takes the same `WalkState::Continue`. fd's only
non-success exit codes come from **outside** the traversal: no valid search path
([`main.rs:86`](../../../tmp/ref/fd/main.rs) `bail!("No valid search paths given.")`) and a failed
write to stdout ([`walk.rs:252-260`](../../../tmp/ref/fd/walk.rs) `ExitCode::GeneralError`).

The one arm fd has that is *not* pure tolerance is the broken-symlink recovery immediately above it
([`walk.rs:485-499`](../../../tmp/ref/fd/walk.rs)): a `WithPath { err: Io(NotFound) }` whose path is
itself a symlink is converted into a printable entry. **It has no counterpart to build here.** With
`follow_links` off — fd's default, ripgrep's default, and cyrup's (`fs.rs` never calls
`follow_links`) — the `ignore` walker yields a dangling symlink as `Ok`, not as an error, so the arm
is unreachable for pi's argv. Confirmed against the installed rg 14.1.0, which drives the same
crate: `rg --files --hidden --no-require-git` over a directory holding `real.txt` and
`dangling.txt -> /nonexistent/target` printed `real.txt`, printed nothing on stderr, and exited 0.

So, end to end: fd meets a `chmod 000` subdirectory, says nothing, exits **0**. pi's
`if (code !== 0)` guard ([find.ts:304](../../../tmp/pi/packages/coding-agent/src/core/tools/find.ts))
is never entered, and the collected paths are relativized
([find.ts:321-326](../../../tmp/pi/packages/coding-agent/src/core/tools/find.ts)) and returned as an
ordinary success. If fd emitted **nothing at all** (e.g. the search root itself is unreadable), the
`if (!output)` branch at [find.ts:311-320](../../../tmp/pi/packages/coding-agent/src/core/tools/find.ts)
resolves `"No files found matching pattern"` — still a success, still not an error.

### ripgrep does the opposite: it exits 2, and pi's grep rejects

Verified empirically against the installed **rg 14.1.0**, running as an unprivileged uid over a tree
containing `readable/a.txt` (matching) and a `chmod 000` `locked/` directory:

| invocation | stdout | stderr | exit |
| --- | --- | --- | --- |
| `rg --json --hidden -- NEEDLE tree` | 1 match event | `rg: …/tree/locked: Permission denied (os error 13)` | **2** |
| `rg --json --hidden -- ZZZNOPE tree` | — | same line | **2** |
| `rg --json --hidden -- NEEDLE tree/readable` | 1 match event | — | 0 |

pi's grep rejects on any code that is neither 0 nor 1
([grep.ts:309-313](../../../tmp/pi/packages/coding-agent/src/core/tools/grep.ts)):

```ts
if (!killedDueToLimit && code !== 0 && code !== 1) {
    const errorMsg = stderr.trim() || `ripgrep exited with code ${code}`;
    settle(() => reject(new Error(errorMsg)));
    return;
}
if (matchCount === 0) { /* "No matches found" */ }
```

Three things follow, and all three are behaviour this task must reproduce:

1. **The error wins over the matches.** Row 1 of the table: rg found a match *and* exited 2, so pi
   throws away the match and rejects. `grep` keeping `Err` is correct.
2. **The error wins over "No matches found".** The exit-code guard is at `:309`, the
   `matchCount === 0` reply at `:314`. An errored walk that found nothing reports the error.
3. **…unless the match limit was hit.** `stopChild(true)` sets `killedDueToLimit`
   ([grep.ts:240-245](../../../tmp/pi/packages/coding-agent/src/core/tools/grep.ts), called from the
   limit check at [`:291-295`](../../../tmp/pi/packages/coding-agent/src/core/tools/grep.ts)), and
   that flag short-circuits the whole guard. So an error rg printed **early** is discarded if the
   100-match limit is eventually reached — which means rg kept walking past the error and its later
   matches still counted toward that limit.

Point 3 is the part cyrup gets wrong today, and it is why `grep` cannot simply keep
`return Err(e)`: returning at the first error means the limit can never subsequently be reached, so
cyrup fails calls that pi answers successfully. The error must be **remembered and deferred**, not
returned.

The message shape also differs. pi surfaces rg's stderr verbatim (`stderr.trim()`), i.e.
`rg: /abs/path: Permission denied (os error 13)`.

## Current Rust behaviour — verified

[`LocalFs::walk`, fs.rs:227-239](../../../crates/cyrup-tools/src/ops/local/fs.rs) is the only
producer:

```rust
for result in walker {
    let item = match result {
        Ok(entry) => {
            let path = entry.path().to_path_buf();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            Ok(WalkItem { path, is_dir })
        }
        Err(e) => Err(ToolError::new(format!("walk: {e}"))),
    };
    if tx.blocking_send(item).is_err() {
        break;
    }
}
```

The producer is already correct in the one respect that matters structurally: it sends the `Err`
and **keeps iterating**. `ignore::Walk::next` leaves its iterator intact after yielding an error
(ignore 0.4.26 `src/walk.rs:1124-1126` —
`Err(err) => { return Some(Err(Error::from_walkdir(err))); }` inside the `loop`), so valid entries
arrive both before and after it. Nothing about the stream needs to change.

Both consumers throw that away:

* [find.rs:212](../../../crates/cyrup-tools/src/tools/find.rs) — `Some(Err(e)) => return Err(e)`,
  discarding the whole `results` vec built since
  [find.rs:145](../../../crates/cyrup-tools/src/tools/find.rs).
* [grep.rs:428](../../../crates/cyrup-tools/src/tools/grep.rs) — the same arm, aborting the fused
  walk/search before the limit at
  [grep.rs:385-387](../../../crates/cyrup-tools/src/tools/grep.rs) can be reached.

The two `FsOps` decorators
([`TraversalFs::walk`, traversal.rs:133-142](../../../crates/cyrup-tools/src/isolation/traversal.rs),
[`ProtectedFs::walk`, protected.rs:150-156](../../../crates/cyrup-tools/src/isolation/protected.rs))
only forward the stream and need no edit. Nothing anywhere in the workspace matches on, or asserts
against, the `walk: ` prefix — `fs.rs:234` is its sole producer.

A missing search root is **already** rejected before the walk begins, by the
`self.fs.metadata(&search_root)` probe at
[find.rs:122-129](../../../crates/cyrup-tools/src/tools/find.rs) (pi's
`Path not found: …`, find.ts:158) and its twin at
[grep.rs:299-306](../../../crates/cyrup-tools/src/tools/grep.rs). Tolerating per-entry errors
therefore cannot hide a bad root.

## Citation corrections

* **The audit's *Parity action* is wrong in two places and must not be followed as written.**
  - It prescribes "only surface an error when the walk produced no rows at all (mirroring pi's
    `if (!output)` guard at find.ts:306)". There is no such gate to mirror: fd exits **0**, so pi's
    `code !== 0` branch — the only branch that can reject — is never entered for a filesystem
    error, whatever `output` holds. `find` must swallow the error **unconditionally**, including
    when it collected nothing (in which case it already answers
    `"No files found matching pattern"`, matching find.ts:311-320). Adding an empty-result gate
    would invent a failure mode pi does not have.
  - It prescribes "the same one-line change is needed in grep.rs:428". **It is not.** ripgrep exits
    2 and pi's grep rejects; making grep swallow the error would silently drop a divergence pi
    reports. grep needs a *different* change — defer, don't discard.
* The audit's `fs.rs:227-239` and `find.rs:212` / `find.rs:145` / `grep.rs:428` citations are
  **correct** as of this file's `updated` date.
* The vendored `ignore` version is **0.4.26** (`Cargo.lock`). The rgignore sibling's mention of
  `ignore-0.4.33` refers to a copy present in the cargo registry cache but not selected by the
  lockfile; all line citations here are against 0.4.26.

## Required changes

### 1. `crates/cyrup-tools/src/ops/local/fs.rs` — make the seam's error text flavor-neutral

The `walk: ` prefix belonged to neither upstream, and after this task exactly one consumer reads the
text at all. Push the prefix to that consumer.

**Current** ([fs.rs:227-239](../../../crates/cyrup-tools/src/ops/local/fs.rs)):

```rust
            for result in walker {
                let item = match result {
                    Ok(entry) => {
                        let path = entry.path().to_path_buf();
                        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                        Ok(WalkItem { path, is_dir })
                    }
                    Err(e) => Err(ToolError::new(format!("walk: {e}"))),
                };
                if tx.blocking_send(item).is_err() {
                    break;
                }
            }
```

**Replacement:**

```rust
            // A per-entry `Err` is a NON-FATAL event on this stream and the walk CONTINUES past
            // it: `ignore::Walk::next` leaves its iterator intact after yielding one (ignore
            // 0.4.26 `src/walk.rs:1124-1126`), so valid entries arrive both before and after.
            // Consumers must never read one as end-of-stream.
            //
            // The message is the BARE `ignore::Error` text — `WithPath` renders as
            // `{path}: {io error}` (ignore 0.4.26 `src/lib.rs:333-335`), e.g.
            // `/srv/locked: Permission denied (os error 13)`. No prefix is added here because the
            // two consumers of this seam want different things from the same value: `find`
            // emulates fd, which discards it (fd 10.5.0 `src/walk.rs:227-231`, `:500-505`), and
            // `grep` emulates ripgrep, which reports it as `rg: {path}: {io error}` on stderr.
            // A `walk: ` prefix matched neither.
            for result in walker {
                let item = match result {
                    Ok(entry) => {
                        let path = entry.path().to_path_buf();
                        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                        Ok(WalkItem { path, is_dir })
                    }
                    Err(e) => Err(ToolError::new(e.to_string())),
                };
                if tx.blocking_send(item).is_err() {
                    break;
                }
            }
```

### 2. `crates/cyrup-tools/src/ops/mod.rs` — state the contract on the trait method

The tolerance decision now lives in the consumers, so the seam has to say that an `Err` does not end
the stream. One doc line, no signature change.

**Current** ([ops/mod.rs:378-379](../../../crates/cyrup-tools/src/ops/mod.rs)):

```rust
    /// Walk a tree for grep/find. Yields candidate paths (gitignore-aware for the local backend).
    fn walk(&self, root: &Path, opts: WalkOpts) -> EventStream<Result<WalkItem, ToolError>>;
```

**Replacement:**

```rust
    /// Walk a tree for grep/find. Yields candidate paths (gitignore-aware for the local backend).
    ///
    /// A yielded `Err` is a NON-FATAL per-entry event, not end-of-stream: the walk continues and
    /// further `Ok` items follow. Implementations MUST keep walking after emitting one, and
    /// consumers MUST keep polling. Whether such an error fails the *tool call* is the caller's
    /// decision and the two callers differ — `find` emulates fd, which swallows every traversal
    /// error and exits 0, while `grep` emulates ripgrep, which reports it and exits 2. The message
    /// carries no tool prefix for that reason.
    fn walk(&self, root: &Path, opts: WalkOpts) -> EventStream<Result<WalkItem, ToolError>>;
```

### 3. `crates/cyrup-tools/src/tools/find.rs` — swallow it, unconditionally

**Current** ([find.rs:212](../../../crates/cyrup-tools/src/tools/find.rs)):

```rust
                        Some(Err(e)) => return Err(e),
```

**Replacement:**

```rust
                        // fd NEVER fails a search over a filesystem error met while traversing.
                        // Its worker sends every `Err` from the `ignore` walker down the results
                        // channel and returns `WalkState::Continue` (fd 10.5.0
                        // `src/walk.rs:500-505`); the receiver prints it only under
                        // `--show-errors` (`:227-231`), which pi's argv does not pass
                        // (find.ts:234-267); and the exit code is `ExitCode::Success` regardless
                        // (`:282-292`). fd therefore exits 0 with empty stderr, pi's
                        // `if (code !== 0)` guard (find.ts:304) is never entered, and the paths fd
                        // did emit are returned as an ordinary success.
                        //
                        // There is NO `ignore::Error` variant fd treats as fatal — fd's arm is a
                        // catch-all that never inspects the variant. `WithPath{Io}` (permission
                        // denied, EIO, a stale mount), `WithDepth{Loop}` (symlink cycle),
                        // `Partial`/`WithLineNumber` (a malformed pattern in an ignore file) and
                        // bare `Io` from parent-ignore loading all take the same path. So this arm
                        // discriminates on nothing and simply keeps collecting.
                        //
                        // Swallowing here cannot hide a bad search root: a root that does not
                        // exist is already rejected by the `metadata(&search_root)` probe above
                        // (pi's `Path not found`, find.ts:158). A root that exists but cannot be
                        // opened yields zero rows and falls through to "No files found matching
                        // pattern" below — which is exactly what pi answers on fd's empty stdout
                        // (find.ts:311-320). Do NOT gate this on "the walk produced no rows": pi
                        // has no such gate, because fd never gives it a non-zero code to gate on.
                        Some(Err(_)) => continue,
```

### 4. `crates/cyrup-tools/src/tools/grep.rs` — defer it, and prefix it `rg:`

Three edits inside the `else` branch that owns the walk.

**4a. Declare the slot.** **Current**
([grep.rs:367-373](../../../crates/cyrup-tools/src/tools/grep.rs), the statement immediately
preceding it):

```rust
            let mut walk = self.fs.walk(
                &search_root,
                WalkOpts {
                    include_hidden: true,
                    require_git: true,
                },
            );
```

**Replacement** (add the binding above; leave the `WalkOpts` literal exactly as the sibling fd/rg
tasks leave it — if `flavor: WalkFlavor::Rg` is already present, keep it):

```rust
            // ripgrep does not abort a search on a traversal error, but it does REPORT one: it
            // prints `rg: {path}: {os error}` to stderr and exits 2 (verified, rg 14.1.0), and
            // pi's grep rejects on any code that is neither 0 nor 1 (grep.ts:309-313). The
            // rejection happens at CLOSE, though, after rg has walked the whole tree — and does
            // not happen at all if the match limit was hit first, because `stopChild(true)` sets
            // `killedDueToLimit` (grep.ts:240-245, :291-295) and that flag short-circuits the
            // guard. So the error is REMEMBERED here and decided after the loop; returning at the
            // first one would stop the walk and make the limit unreachable, failing calls pi
            // answers successfully. First error only — pi reports rg's whole stderr, but rg's
            // parallel walk does not order it against ours, so the first is the only stable
            // choice.
            let mut walk_error: Option<ToolError> = None;
            let mut walk = self.fs.walk(
                &search_root,
                WalkOpts {
                    include_hidden: true,
                    require_git: true,
                },
            );
```

**4b. Record instead of return.** **Current**
([grep.rs:427-429](../../../crates/cyrup-tools/src/tools/grep.rs)):

```rust
                            Some(Ok(_)) => {}
                            Some(Err(e)) => return Err(e),
                            None => break,
```

**Replacement:**

```rust
                            Some(Ok(_)) => {}
                            Some(Err(e)) => {
                                if walk_error.is_none() {
                                    // pi surfaces rg's stderr verbatim (`stderr.trim()`,
                                    // grep.ts:310) and rg writes `rg: {path}: {os error}`.
                                    // `LocalFs::walk` yields the bare `{path}: {os error}` because
                                    // `find` must carry no prefix at all, so the `rg: ` half is
                                    // added here, at the one consumer that emulates ripgrep.
                                    walk_error =
                                        Some(ToolError::new(format!("rg: {}", e.message)));
                                }
                            }
                            None => break,
```

**4c. Decide after the loop.** **Current** ([grep.rs:430-436](../../../crates/cyrup-tools/src/tools/grep.rs)):

```rust
                        }
                    }
                }
            }
        }

        if out.is_empty() {
```

**Replacement:**

```rust
                        }
                    }
                }
            }

            // pi: `if (!killedDueToLimit && code !== 0 && code !== 1) { reject(stderr.trim()); }`
            // (grep.ts:309-313). `count >= limit` IS `killedDueToLimit`: it is the condition that
            // breaks the loop above, exactly as it kills the rg child upstream. `limit` is
            // `.max(1)` (grep.ts:189), so this cannot be vacuously true on entry.
            //
            // This check precedes the `out.is_empty()` reply below because pi's does — the
            // exit-code guard is at grep.ts:309 and the `matchCount === 0` reply at `:314` — so an
            // errored walk that found nothing reports the error rather than "No matches found".
            // It also precedes the successful formatting path, because rg exiting 2 makes pi
            // reject even when matches were found.
            if count < limit
                && let Some(e) = walk_error
            {
                return Err(e);
            }
        }

        if out.is_empty() {
```

## Files changed

| File | Change |
| --- | --- |
| [ops/local/fs.rs](../../../crates/cyrup-tools/src/ops/local/fs.rs) | drop the `walk: ` prefix; document that an `Err` does not end the stream |
| [ops/mod.rs](../../../crates/cyrup-tools/src/ops/mod.rs) | doc contract on `FsOps::walk` (no signature change) |
| [tools/find.rs](../../../crates/cyrup-tools/src/tools/find.rs) | `Some(Err(_)) => continue` |
| [tools/grep.rs](../../../crates/cyrup-tools/src/tools/grep.rs) | defer the first walk error; surface it after the loop only when the limit was not reached; `rg: ` prefix |

Nothing else. `WalkItem`, `WalkOpts`, the `FsOps::walk` signature, the two isolation decorators and
every re-export are untouched.

## Non-goals

* **No empty-result gate on `find`.** pi has none for this case; see *Citation corrections*.
* **No error channel, no warning sink, no `details` field** carrying skipped paths. fd emits nothing
  without `--show-errors` and pi never passes it, so any user-visible trace of the skipped subtree
  is capability pi does not have.
* **No aggregation of multiple walk errors in `grep`.** pi concatenates rg's whole stderr; rg's
  parallel walk gives no stable ordering against cyrup's serial one, so matching the *set* of lines
  is not reachable. First error only.
* **No `follow_links`, no broken-symlink synthesis.** fd's `DirEntry::broken_symlink` arm is
  unreachable without `--follow`, which pi never passes.
* **No change to `find`'s or `grep`'s root pre-check**, their limits, their truncation, or the
  ignore-source work owned by the two sibling briefs.

## Genuinely uncertain

* **`killedDueToLimit` timing under `grep`.** cyrup breaks the fused walk/search loop the moment
  `count >= limit`, which is the closest available analogue of pi killing the rg child at the same
  threshold — but rg is a separate process walking in parallel, so the exact set of directories
  visited before the kill differs. A tree where the unreadable directory and the 100th match race
  each other can therefore land on either side of the guard. The prescription above makes the two
  agree on the *rule*; it cannot make them agree on the race.
* **Multi-error stderr for `grep`**, as noted under *Non-goals*: pi's message may list several
  paths where cyrup's lists one.
* **fd version drift.** pi resolves a system `fd`/`fdfind` first and otherwise downloads the latest
  `sharkdp/fd` release, so the binary is not pinned. The error-tolerance behaviour cited here
  (`WalkState::Continue`, `--show-errors` opt-in, `ExitCode::Success`) is long-standing in fd, but
  it is 10.5.0 that was read.

## Definition of done

Observable behaviour that must hold. "Unreadable directory" means one whose contents cannot be
enumerated by the running user.

1. `find` rooted at a tree that contains an unreadable subdirectory returns the matching paths from
   the readable part of the tree, as a normal successful result. It does not return an error, and
   it does not mention the unreadable directory anywhere in its output.
2. The set of paths `find` returns in (1) is identical to what it returns when the same tree has no
   unreadable directory in it, minus only the paths under that directory.
3. `find` whose search root itself exists but cannot be enumerated returns
   `No files found matching pattern` — not an error.
4. `find` whose search root does not exist still returns `Path not found: <path>`.
5. `find` over a tree containing a symlink cycle, a malformed `.gitignore`/`.ignore` pattern, or a
   dead mount point returns the paths it collected rather than an error, on the same terms as (1).
6. `find`'s `limit` and byte-truncation notices behave in (1) exactly as they do on a clean tree:
   the unreadable directory neither consumes a result slot nor suppresses a notice.
7. `grep` rooted at a tree containing an unreadable subdirectory, where the total match count stays
   **below** the limit, returns an error whose message is
   `rg: <absolute path>: <os error text>` — for a permission failure,
   `rg: /abs/locked: Permission denied (os error 13)`. This holds whether the readable part of the
   tree produced matches or produced none; the error replaces both the match output and
   `No matches found`.
8. `grep` over that same tree where the readable part produces **at least `limit` matches** returns
   those matches normally, with the usual limit notice, and no error — the walk continues past the
   unreadable directory so that matches found after it still count toward the limit.
9. `grep` over a tree with no unreadable directory is unchanged in every respect.
10. No `find` or `grep` parameter, `FindOpts`/`GrepOpts` field, or configuration key controls any of
    the above; the behaviour is unconditional, as it is on pi's argv.
11. No error message emitted by any tool begins with `walk: `.
