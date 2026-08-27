---
title: Ls does not observe an already-fired cancellation before touching the filesystem
priority: LOW
tool: ls
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: exec
status: done
updated: 2026-08-27
---

# Ls does not observe an already-fired cancellation before touching the filesystem

## Core objective

`LsTool::execute` must observe the cancellation token on **both** edges that pi observes it on, and nowhere else:

1. **Entry edge** — an `ls` dispatched with an already-fired token returns `Operation aborted` *before* any path resolution and *before* any filesystem syscall.
2. **In-flight edge** — a cancel that lands while the tool is inside `metadata` / `read_dir` / the per-entry stat loop is observed *promptly*, at the next suspension point, instead of after the whole enumeration drains.

Everything else about `ls` — its messages, its collation, its limit folding, its notices, its details — stays byte-for-byte as it is. This is a parity task, not a redesign.

## What pi does

[pi ls.ts:111-126](../../../tmp/pi/packages/coding-agent/src/core/tools/ls.ts) — the executor's body *is* a `new Promise`, and the first two things inside the executor are the already-aborted short circuit and a `{ once: true }` abort listener:

```ts
return new Promise((resolve, reject) => {
    if (signal?.aborted) {
        reject(new Error("Operation aborted"));
        return;
    }

    const onAbort = () => reject(new Error("Operation aborted"));
    signal?.addEventListener("abort", onAbort, { once: true });

    (async () => {
        try {
            const dirPath = resolveToCwd(path || ".", cwd);
            const effectiveLimit = limit ?? DEFAULT_LIMIT;
            ...
```

Two distinct mechanisms, and the port needs both:

* **[ls.ts:119-122](../../../tmp/pi/packages/coding-agent/src/core/tools/ls.ts)** — the entry check runs ahead of `resolveToCwd` ([ls.ts:129](../../../tmp/pi/packages/coding-agent/src/core/tools/ls.ts)), ahead of `ops.exists` ([ls.ts:133](../../../tmp/pi/packages/coding-agent/src/core/tools/ls.ts)), ahead of `ops.stat` ([ls.ts:139](../../../tmp/pi/packages/coding-agent/src/core/tools/ls.ts)) and ahead of `ops.readdir` ([ls.ts:148](../../../tmp/pi/packages/coding-agent/src/core/tools/ls.ts)). So `Path not found:`, `Not a directory:`, `Cannot read directory:` and `(empty directory)` are all unreachable on an already-cancelled call.
* **[ls.ts:124-125](../../../tmp/pi/packages/coding-agent/src/core/tools/ls.ts)** — the listener rejects the *outer* promise the instant the signal fires, so a cancel landing mid-`readdir` or mid-stat-loop surfaces `Operation aborted` immediately even though Node's `readdir` itself is not cancellable.

The listener's window has a precise end: **[ls.ts:178](../../../tmp/pi/packages/coding-agent/src/core/tools/ls.ts)** `signal?.removeEventListener("abort", onAbort)` runs *after* the per-entry stat loop and *before* the `(empty directory)` resolve at [ls.ts:180-183](../../../tmp/pi/packages/coding-agent/src/core/tools/ls.ts). Once the entries are collected, a cancel no longer changes the outcome — sorting, truncation, notice building and the final `resolve` are outside the abort window. The port must reproduce that boundary, not just "abort somewhere".

> Citation correction against the inherited note: the entry check is `ls.ts:118-125` only if you count the `new Promise(` line; the check itself is `:119-122` and the listener `:124-125`. The three error strings live at `:134`, `:141` and `:150` — not `:129`/`:141`/`:152`. `:129` is `resolveToCwd`. The stale `ls.ts:129` / `ls.ts:125` / `ls.ts:150` references embedded in the current `ls.rs` comments are off by the same few lines and are corrected in the replacement code below.

## What cyrup-tools does today

[ls.rs:76-98](../../../crates/cyrup-tools/src/tools/ls.rs) — `execute` opens straight into deserialization and filesystem work with no token observation at all:

```rust
    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let input: LsInput =
            serde_json::from_value(params).map_err(|e| error::invalid(format!("ls: {e}")))?;

        let abs = path::resolve_to_cwd(input.path.as_deref().unwrap_or("."), &self.cwd);
        let meta = self
            .fs
            .metadata(&abs)
            .await
            // Pi: `Path not found: ${dirPath}` (ls.ts:129).
            .map_err(|_| error::not_found(format!("Path not found: {}", error::show(&abs))))?;
        if !meta.is_dir {
            return Err(error::invalid(format!(
                "Not a directory: {}",
                error::show(&abs)
            )));
        }
```

then [ls.rs:111-115](../../../crates/cyrup-tools/src/tools/ls.rs) `self.fs.read_dir(&abs).await`. The token is first — and only — touched at [ls.rs:140-142](../../../crates/cyrup-tools/src/tools/ls.rs), inside the per-entry loop:

```rust
        for entry in &entries {
            if cancel.is_cancelled() {
                return Err(error::aborted());
            }
```

Neither edge is covered. There is no entry guard, and there is nothing racing the awaits: `cancel.cancelled()` is never awaited anywhere in this file.

### User-visible consequence

An `ls` dispatched with an already-cancelled token reports the wrong outcome — `Path not found: <p>` on a missing path, `Not a directory: <p>` on a file, `Cannot read directory: …` on an unreadable directory, and `(empty directory)` **as a success** on an empty one — instead of `Operation aborted`, after paying a wasted `metadata` + `read_dir` pair. A cancel (Esc) landing while `read_dir` is enumerating a huge or slow directory is not observed until the enumeration finishes, so the tool keeps working after the user cancelled. Non-empty directories already return `Operation aborted` today, because the loop guard at [ls.rs:140](../../../crates/cyrup-tools/src/tools/ls.rs) fires on the first iteration ahead of the limit check — which is why this is LOW and not higher.

### Why the in-flight edge cannot come from a lower layer

The FS seam is deliberately cancel-blind. [FsOps](../../../crates/cyrup-tools/src/ops/mod.rs) declares

```rust
    async fn metadata(&self, path: &Path) -> Result<Meta, ToolError>;
    async fn read_dir(&self, path: &Path) -> Result<Vec<DirEntry>, ToolError>;
```

at [ops/mod.rs:368-369](../../../crates/cyrup-tools/src/ops/mod.rs) — no `CancelToken` parameter, unlike [`ProcOps::exec`](../../../crates/cyrup-tools/src/ops/mod.rs) at [ops/mod.rs:386-392](../../../crates/cyrup-tools/src/ops/mod.rs) which does take one. And [`LocalFs::read_dir`](../../../crates/cyrup-tools/src/ops/local/fs.rs) at [ops/local/fs.rs:188-207](../../../crates/cyrup-tools/src/ops/local/fs.rs) drains `rd.next_entry()` to completion in a `loop` with no escape hatch. So the abort has to be applied **at the call site in `ls.rs`**, exactly as `find` and `grep` apply theirs — the trait is not to be widened for this.

Nor does the agent layer supply drop-based cancellation: [cyrup-agent exec.rs:90-92](../../../crates/cyrup-agent/src/agent/run/tools/exec.rs) and [exec.rs:328-330](../../../crates/cyrup-agent/src/agent/run/tools/exec.rs) test the token only *after* a call has finished, and the comment at [exec.rs:95-98](../../../crates/cyrup-agent/src/agent/run/tools/exec.rs) states outright that deferred calls are still started after an abort ("Calls deferred before an abort broke the loop are still started, exactly as Pi's already-pushed closures are"). `LsTool` is registered bare at [registry.rs:98](../../../crates/cyrup-tools/src/registry.rs) (`reg.insert(Arc::new(LsTool::new(backend.fs.clone(), cwd, opts.ls)))`), and the [cyrup-ext wrapper](../../../crates/cyrup-ext/src/wrapper.rs) only derives `addedToolNames` — it adds no cancellation behaviour.

## The established idiom in this crate — verified, and the audit's claim corrected

The source note this task inherited asserted that *"the sibling tools all have the first-statement guard (find.rs:115, write.rs:103, edit.rs:263, read.rs:137, grep.rs:343, bash.rs:293)"*. Reading the actual code, **that is wrong for five of the six**. Every one of those line numbers is a real guard, but only `find`'s is a *first* statement. The crate's real convention is narrower and more precise, and it is what the fix must follow:

> **Place the guard exactly where pi places its check — not mechanically at the top.**

| site | where it actually sits | pi position it mirrors |
| --- | --- | --- |
| [find.rs:115-117](../../../crates/cyrup-tools/src/tools/find.rs) | **first statement**, before `serde_json::from_value` | `find.ts:142-145`, first statement of the executor |
| [write.rs:103-105](../../../crates/cyrup-tools/src/tools/write.rs) | after `from_value`, after `resolve_to_cwd`, after `self.locks.guard(&abs, &cancel).await?` | `write.ts:220`, `throwIfAborted()` before `ops.writeFile` |
| [write.rs:119-121](../../../crates/cyrup-tools/src/tools/write.rs) | after `write_in_place` has landed the bytes | `write.ts:224`, `throwIfAborted()` after the write |
| [edit.rs:263-265](../../../crates/cyrup-tools/src/tools/edit.rs) | after the edits have been applied in memory | `edit.ts` pre-write check |
| [edit.rs:280-282](../../../crates/cyrup-tools/src/tools/edit.rs) | after `write_in_place` | `edit.ts:352` |
| [read.rs:137-139](../../../crates/cyrup-tools/src/tools/read.rs) | after macOS variant resolution and the `R_OK` access check, before `fs.read` | pi's check before the file read |
| [grep.rs:343-345](../../../crates/cyrup-tools/src/tools/grep.rs) | inside the `meta.is_file` branch, after root `metadata`, matcher build, limit folding and glob build | grep's per-data-path re-test |
| [bash.rs:293-295](../../../crates/cyrup-tools/src/tools/bash.rs) | strictly between `resolve_timeout_ms` and shell resolution — and returns `error::invalid(append_status("", "Command aborted"))`, **not** `error::aborted()` | `bash.ts:86-88`, deliberately positioned |

So `find` is the **only** true precedent for an entry-edge guard, and it is the right one, because `find.ts` and `ls.ts` share the identical upstream shape. [find.rs:107-120](../../../crates/cyrup-tools/src/tools/find.rs) is the idiom to copy verbatim:

```rust
    ) -> Result<ToolResult, ToolError> {
        // Pi's FIRST statement inside the executor, before `resolveToCwd`, before `ops.exists`,
        // before anything: `if (signal?.aborted) { reject(new Error("Operation aborted")); return; }`
        // (find.ts:142-145). cyrup observed the token for the first time only at the walk loop's
        // `select!`, so an already-cancelled `find` still paid `fs.metadata(search_root)` AND the
        // whole `inside_git_repo` ancestor walk — one `metadata` per parent up to the filesystem
        // root — before it could report the abort. (find.ts has no parameter validation at all, so
        // "abort first, parse second" is also pi's order.)
        if cancel.is_cancelled() {
            return Err(error::aborted());
        }

        let input: FindInput =
            serde_json::from_value(params).map_err(|e| error::invalid(format!("find: {e}")))?;
```

And [find.rs:168-185](../../../crates/cyrup-tools/src/tools/find.rs) is the idiom for the in-flight edge — an explicit guard **plus** a `biased;` `select!`, both, with the reason for `biased;` stated in place:

```rust
                // Pi re-tests `signal?.aborted` FIRST on every data path (find.ts:174, :182, :226,
                // :299, :355), so data can never win a race against an already-fired abort. The
                // `select!` below only observes a cancel while it is parked on `walk.next()`; one
                // that lands while the previous entry was being matched is observed here, on the
                // next turn. The sibling `grep.rs` already carries this guard.
                if cancel.is_cancelled() {
                    return Err(error::aborted());
                }
                tokio::select! {
                    // `biased;` — without it `select!` polls in RANDOM order, so with the token
                    // already cancelled AND an entry already buffered the walk arm won half the time
                    // and the tool kept consuming directory entries after Esc: bounded in expectation,
                    // unbounded in the worst case. Pi's abort is deterministic on both edges (the
                    // `{once:true}` listener at find.ts:158-160 rejects the instant the signal fires),
                    // so the cancel arm must be polled first here.
                    biased;
                    _ = cancel.cancelled() => return Err(error::aborted()),
                    item = walk.next() => {
```

Note that `grep.rs` at [grep.rs:391-395](../../../crates/cyrup-tools/src/tools/grep.rs) carries the same guard-plus-`select!` pair but **without** `biased;`. Do not copy grep's version — `find`'s is the corrected one, and `find_abort.rs`'s own header ([tests/find_abort.rs:12-24](../../../crates/cyrup-tools/src/tests/find_abort.rs)) records exactly why the unbiased form was a defect.

The error constructor is [`error::aborted()`](../../../crates/cyrup-tools/src/error.rs) at [error.rs:113-119](../../../crates/cyrup-tools/src/error.rs), which produces pi's exact capital-O literal:

```rust
/// Cancellation (R-03-009). Pi throws `new Error("Operation aborted")` (capital O) on every
/// … (`bash` alone says `"Command aborted"` — it never routes through here.)
pub(crate) fn aborted() -> ToolError {
    ToolError::new("Operation aborted")
}
```

`CancelToken` is a re-export of `tokio_util::sync::CancellationToken` ([cyrup-core cancel.rs:9](../../../crates/cyrup-core/src/cancel.rs)), so both `is_cancelled()` and the `cancelled()` future are available with no new dependency, and it is already bound as the `cancel` parameter of [`Tool::execute`](../../../crates/cyrup-core/src/tool.rs) at [tool.rs:231](../../../crates/cyrup-core/src/tool.rs).

## Required change — the single path

**One file changes: [crates/cyrup-tools/src/tools/ls.rs](../../../crates/cyrup-tools/src/tools/ls.rs).** No trait, no backend, no registry, no agent-layer change. `FsOps` is left exactly as it is.

The change has two halves, and both are required. Half one is the entry guard, copied from `find.rs`. Half two reproduces pi's abort *window* — [ls.ts:124](../../../tmp/pi/packages/coding-agent/src/core/tools/ls.ts) through [ls.ts:178](../../../tmp/pi/packages/coding-agent/src/core/tools/ls.ts) — by hoisting the filesystem region into one `async` block and racing that whole block against `cancel.cancelled()` in a single `biased;` `select!`.

Racing only the `read_dir` await would leave `metadata` uncovered, which pi's listener does cover. Racing individual awaits one at a time would need four separate `select!`s, would still not cover the per-entry stat loop, and would put the abort boundary in a different place from pi's. One block, one race, whose lifetime is exactly the listener's lifetime.

### Current code — [ls.rs:82-157](../../../crates/cyrup-tools/src/tools/ls.rs)

```rust
    ) -> Result<ToolResult, ToolError> {
        let input: LsInput =
            serde_json::from_value(params).map_err(|e| error::invalid(format!("ls: {e}")))?;

        let abs = path::resolve_to_cwd(input.path.as_deref().unwrap_or("."), &self.cwd);
        let meta = self
            .fs
            .metadata(&abs)
            .await
            // Pi: `Path not found: ${dirPath}` (ls.ts:129).
            .map_err(|_| error::not_found(format!("Path not found: {}", error::show(&abs))))?;
        if !meta.is_dir {
            return Err(error::invalid(format!(
                "Not a directory: {}",
                error::show(&abs)
            )));
        }

        // Pi ls.ts:147-152: … (existing comment block)
        let mut entries = self
            .fs
            .read_dir(&abs)
            .await
            .map_err(|e| error::invalid(format!("Cannot read directory: {e}")))?;
        // Pi: `entries.sort(…)` (ls.ts:150) … (existing collation comment block)
        let mut collator = feruca::Collator::new(feruca::Tailoring::default(), false, true);
        entries.sort_by(|a, b| collator.collate(&a.name.to_lowercase(), &b.name.to_lowercase()));

        // Pi: `const effectiveLimit = limit ?? DEFAULT_LIMIT` (ls.ts:125) … (existing comment)
        let limit = input.limit.map_or(self.opts.limit, crate::jsnum::to_count);
        let mut lines: Vec<String> = Vec::new();
        let mut limit_reached = false;
        for entry in &entries {
            if cancel.is_cancelled() {
                return Err(error::aborted());
            }
            if lines.len() >= limit {
                limit_reached = true;
                break;
            }
            match self.fs.metadata(&entry.path).await {
                Ok(m) => {
                    if m.is_dir {
                        lines.push(format!("{}/", entry.name));
                    } else {
                        lines.push(entry.name.clone());
                    }
                }
                Err(_) => continue, // skip unstattable (R-03-035)
            }
        }

        if lines.is_empty() {
```

### Replacement

```rust
    ) -> Result<ToolResult, ToolError> {
        // Pi's FIRST statement inside the executor, before `resolveToCwd`, before `ops.exists`,
        // before `ops.stat`, before `ops.readdir`:
        // `if (signal?.aborted) { reject(new Error("Operation aborted")); return; }` (ls.ts:119-122).
        // Without it an already-cancelled `ls` reported the WRONG outcome — `Path not found:` /
        // `Not a directory:` / `Cannot read directory:` on a bad path, or `(empty directory)` as a
        // SUCCESS on an empty one — instead of the abort the user asked for, after paying a wasted
        // `metadata`/`read_dir` pair. `ls.ts` has no runtime parameter validation, so "abort first,
        // parse second" is also pi's order. Same guard, same position, as the sibling
        // `find.rs:115-117`.
        if cancel.is_cancelled() {
            return Err(error::aborted());
        }

        let input: LsInput =
            serde_json::from_value(params).map_err(|e| error::invalid(format!("ls: {e}")))?;

        let abs = path::resolve_to_cwd(input.path.as_deref().unwrap_or("."), &self.cwd);

        // Pi: `const effectiveLimit = limit ?? DEFAULT_LIMIT` (ls.ts:130) — no clamp at all. A
        // negative or zero limit satisfies `results.length >= effectiveLimit` on the very first
        // iteration (ls.ts:161), so the loop collects nothing and Pi returns "(empty directory)"
        // before any notice is built; folding negatives to 0 reproduces that exactly. `??` is
        // null/undefined-only, so a JSON `null` takes the default. Sits above the abort window
        // because it is pure arithmetic on the already-parsed input — no I/O, nothing to cancel.
        let limit = input.limit.map_or(self.opts.limit, crate::jsnum::to_count);

        // ---- Pi's abort-listener window: ls.ts:124-125 (register) … ls.ts:178 (remove) ----
        // `const onAbort = () => reject(new Error("Operation aborted"));
        //  signal?.addEventListener("abort", onAbort, { once: true });`
        // rejects the OUTER promise the instant the signal fires, so upstream a cancel landing
        // mid-`readdir` or mid-stat-loop surfaces `Operation aborted` immediately even though
        // Node's `readdir` is not itself cancellable. The `FsOps` seam is likewise cancel-blind
        // (`metadata`/`read_dir` take no token, ops/mod.rs:368-369, and `LocalFs::read_dir` drains
        // `next_entry` to completion, ops/local/fs.rs:188-207), so the equivalent has to be built
        // here at the call site — exactly as `find.rs` and `grep.rs` build theirs, rather than by
        // widening the trait for every backend. Everything the listener covers goes in this block
        // and nothing else does: pi removes the listener at ls.ts:178, BEFORE the
        // `(empty directory)` resolve (ls.ts:180) and before truncation and notice building, so
        // once the entries are collected a cancel no longer changes the result.
        let listing = async {
            let meta = self
                .fs
                .metadata(&abs)
                .await
                // Pi: `Path not found: ${dirPath}` (ls.ts:134).
                .map_err(|_| error::not_found(format!("Path not found: {}", error::show(&abs))))?;
            if !meta.is_dir {
                // Pi: `Not a directory: ${dirPath}` (ls.ts:141).
                return Err(error::invalid(format!(
                    "Not a directory: {}",
                    error::show(&abs)
                )));
            }

            // Pi ls.ts:147-152:
            // ```
            // try { entries = await ops.readdir(dirPath); }
            // catch (e: any) { reject(new Error(`Cannot read directory: ${e.message}`)); return; }
            // ```
            // — a THIRD stable prefix beside `Path not found:` (ls.ts:134) and `Not a directory:`
            // (ls.ts:141) above, distinguishing "exists, is a directory, cannot be enumerated"
            // (mode `0300`, EIO, a permissions-stripped `.git/objects`) from the other two. The `?`
            // used to propagate `FsOps::read_dir`'s raw `"<path>: <io error>"` wrapper, which
            // carries none of the three prefixes. `read_dir` now builds its error with
            // `error::io_errno`, so `{e}` renders Node-shaped — leading with the errno code, as
            // `e.message` does upstream.
            let mut entries = self
                .fs
                .read_dir(&abs)
                .await
                .map_err(|e| error::invalid(format!("Cannot read directory: {e}")))?;

            // Pi: `entries.sort((a, b) => a.toLowerCase().localeCompare(b.toLowerCase()))`
            // (ls.ts:155) — case-insensitive, locale-aware Unicode collation (ICU-backed in the JS
            // engine). Rust std orders by Unicode scalar value, which diverges for
            // accented/punctuation-adjacent names (e.g. `é` collates near `e` under UCA but after
            // `z` by scalar value). `feruca` is a pure-Rust Unicode Collation Algorithm impl. We
            // mirror the JS engine's default `localeCompare` (CLDR root collation, "non-ignorable"
            // variable handling, so leading punctuation like a dotfile's `.` keeps a real primary
            // weight and sorts BEFORE letters — matching Node's `".dot".localeCompare("a.txt")
            // === -1`). `feruca`'s default `Collator` uses "shifted" handling, which would IGNORE
            // that dot; so we build a non-ignorable collator: `Collator::new(Tailoring::default()
            // /* CLDR Root */, false /* shifting */, true /* byte-value tiebreak */)`. We
            // lower-case both keys first to mirror Pi's `.toLowerCase()` pre-step exactly.
            let mut collator = feruca::Collator::new(feruca::Tailoring::default(), false, true);
            entries
                .sort_by(|a, b| collator.collate(&a.name.to_lowercase(), &b.name.to_lowercase()));

            let mut lines: Vec<String> = Vec::new();
            let mut limit_reached = false;
            for entry in &entries {
                // The `select!` below observes a cancel only while this future is PARKED on an
                // await; one that lands while the previous entry was being formatted is observed
                // here, on the next turn. Same belt-and-braces pairing as `find.rs:173-175`
                // sitting beside its `select!`.
                if cancel.is_cancelled() {
                    return Err(error::aborted());
                }
                if lines.len() >= limit {
                    limit_reached = true;
                    break;
                }
                match self.fs.metadata(&entry.path).await {
                    Ok(m) => {
                        if m.is_dir {
                            lines.push(format!("{}/", entry.name));
                        } else {
                            lines.push(entry.name.clone());
                        }
                    }
                    Err(_) => continue, // skip unstattable (R-03-035)
                }
            }
            Ok::<(Vec<String>, bool), ToolError>((lines, limit_reached))
        };

        let (lines, limit_reached) = tokio::select! {
            // `biased;` — without it `select!` polls its arms in RANDOM order, so with the token
            // already cancelled AND the listing future ready to make progress, the listing arm
            // would win roughly half the time and the tool would keep statting entries after Esc.
            // Pi's abort is deterministic on this edge (the `{once:true}` listener at ls.ts:124-125
            // rejects the instant the signal fires), so the cancel arm must be polled first.
            // `find.rs:183` carries the same `biased;` for the same reason; `grep.rs:394` is the
            // one site that still lacks it and is NOT the model to follow.
            biased;
            _ = cancel.cancelled() => return Err(error::aborted()),
            r = listing => r?,
        };

        if lines.is_empty() {
```

Everything from `if lines.is_empty()` at [ls.rs:159](../../../crates/cyrup-tools/src/tools/ls.rs) onward — the `(empty directory)` early return, `truncate_head`, the notices join, the `LsDetails` construction, the final `ToolResult` — is **unchanged and stays outside the abort window**, which is precisely where pi puts it relative to `removeEventListener` at [ls.ts:178](../../../tmp/pi/packages/coding-agent/src/core/tools/ls.ts).

### Mechanics the compiler will care about

* The `async` block is **not** `move`. It borrows `&self`, `&abs`, `limit` (a `usize`, copied) and `&cancel` immutably; `cancel.cancelled()` in the sibling `select!` arm also takes `&self`, so the two immutable borrows coexist.
* The explicit `Ok::<(Vec<String>, bool), ToolError>(…)` turbofish is required — the block mixes `?` on two differently-mapped errors with `return Err(...)` arms and a tail `Ok`, and inference through `select!` is not reliable without it. `ToolError` is already in scope from [ls.rs:8](../../../crates/cyrup-tools/src/tools/ls.rs).
* `tokio::select!` pins the block on the stack; nothing needs boxing. `tokio` with the `macros` feature is already a dependency of this crate — [find.rs:176](../../../crates/cyrup-tools/src/tools/find.rs) and [grep.rs:394](../../../crates/cyrup-tools/src/tools/grep.rs) already use `tokio::select!` here.
* `return Err(error::aborted())` inside the `select!` arm returns from `execute`, not from the block — that is the intent, and it matches [find.rs:184](../../../crates/cyrup-tools/src/tools/find.rs).
* Dropping `listing` mid-`read_dir` is safe: [`LocalFs::read_dir`](../../../crates/cyrup-tools/src/ops/local/fs.rs) is `tokio::fs`, whose blocking work finishes on the blocking pool and whose result is simply discarded. No file handle is leaked into the tool's own state.
* The `collation_tests` module at [ls.rs:214-238](../../../crates/cyrup-tools/src/tools/ls.rs) is untouched.
* `mod.rs`, [registry.rs](../../../crates/cyrup-tools/src/registry.rs), [ops/mod.rs](../../../crates/cyrup-tools/src/ops/mod.rs) and [ops/local/fs.rs](../../../crates/cyrup-tools/src/ops/local/fs.rs) are untouched.

## Definition of done

Observable behaviour of `LsTool::execute` — all of the following must hold:

1. **Already-cancelled token, any input** — `execute` returns `Err` whose message is exactly `Operation aborted`. This holds for every one of: a path that does not exist, a path that is a regular file, an unreadable directory, an empty directory, a populated directory, `limit: 0`, `limit: -1`, and a `params` value that would otherwise fail deserialization. None of `Path not found: …`, `Not a directory: …`, `Cannot read directory: …`, `(empty directory)`, `ls: <serde message>`, or any `Ok` result, is reachable when the token is already cancelled.
2. **No filesystem contact on an already-cancelled call** — an `FsOps` backend that counts invocations records zero `metadata`, zero `read_dir` and zero `access` calls for an `ls` dispatched with an already-fired token, and `path::resolve_to_cwd` is not reached.
3. **Cancel during enumeration** — a token fired while `read_dir` is enumerating a large or slow directory causes `execute` to return `Operation aborted` without waiting for the enumeration to drain. The same holds for a token fired during the root `metadata` call and for one fired during the per-entry stat loop.
4. **Deterministic on the race** — with the token already cancelled and the listing future simultaneously able to make progress, the outcome is `Operation aborted` every time, never sometimes. `biased;` is present on the `select!`.
5. **Cancel after collection changes nothing** — a token fired after the per-entry loop has completed does not alter the result: byte truncation, the `[… entries limit reached. Use limit=N for more]` and `[… limit reached]` notices, the `LsDetails` payload and the `(empty directory)` return are produced exactly as before, matching pi's `removeEventListener` at `ls.ts:178`.
6. **Nothing else moves** — on a run whose token never fires, `ls` produces byte-identical content, details and error strings to the current implementation for every input: the `Path not found:` / `Not a directory:` / `Cannot read directory:` prefixes, the `feruca` collation order, the limit folding, the doubled-`limit` hint, the 50KB notice, and the `details: None`-when-empty rule.
7. **Scope** — the diff touches only [crates/cyrup-tools/src/tools/ls.rs](../../../crates/cyrup-tools/src/tools/ls.rs). The `FsOps` trait signature, `LocalFs`, the registry and the agent layer are unchanged, and no behaviour pi does not have is introduced.
