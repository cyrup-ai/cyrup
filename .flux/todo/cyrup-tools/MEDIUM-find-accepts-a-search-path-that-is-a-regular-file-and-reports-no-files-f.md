---
title: Find accepts a search path that is a regular file and reports "No files found"; pi rejects it
priority: MEDIUM
tool: find
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: exec
status: in-progress
updated: 2026-08-27
---

# Find accepts a search path that is a regular file and reports "No files found"; pi rejects it

## The gap in one sentence

[find.rs](../../../crates/cyrup-tools/src/tools/find.rs) stats the search root only to learn whether the stat *succeeds* and throws the resulting `Meta` away, so a search root that is a regular file walks to exactly one entry, that entry is dropped as the root itself, and the tool answers with the **success** text `No files found matching pattern` — where pi answers with an **error**.

---

## What pi does — verified at v0.84.3

Reference: [pi find.ts](../../../tmp/pi/packages/coding-agent/src/core/tools/find.ts) (`packages/coding-agent` version `0.84.3`).

**There is no existence or type pre-check in the branch cyrup mirrors.** `ops.exists` appears exactly once in the file, inside the `if (customOps?.glob)` branch:

```ts
// find.ts:169-173
if (customOps?.glob) {
    if (!(await ops.exists(searchPath))) {
        settle(() => reject(new Error(`Path not found: ${searchPath}`)));
        return;
    }
```

cyrup implements the **default (fd) branch**, which begins at `find.ts:224` (`// Default implementation uses fd.`) and never probes the path. It hands the absolute search path to fd as fd's search root:

```ts
// find.ts:267
args.push("--", effectivePattern, searchPath);
```

`searchPath` is `resolveToCwd(searchDir || ".", cwd)` (`find.ts:164`), i.e. always absolute — so the path fd prints in its diagnostics is the same absolute string cyrup builds with `path::resolve_to_cwd`.

**fd is the validator.** Its `Opts::search_paths()` filters every root through one predicate and prints one line per rejected root (`fd/src/cli.rs:695-720`):

```rust
if filesystem::is_existing_directory(path) {
    Some(self.normalize_path(path))
} else {
    print_error(format!(
        "Search path '{}' is not a directory.",
        path.to_string_lossy()
    ));
    None
}
```

`is_existing_directory` is `path.is_dir() && (path.file_name().is_some() || path.normalize().is_ok())` (`fd/src/filesystem.rs:38-42`) — `Path::is_dir` follows symlinks and is `false` for a missing path, so **a regular file and a nonexistent path take the same branch and produce the same message.** `print_error` writes to stderr with a fixed prefix (`fd/src/error.rs`):

```rust
pub fn print_error(msg: impl Into<String>) {
    let msg = msg.into();
    let safe = maybe_sanitize(&msg, std::io::stderr().is_terminal());
    eprintln!("[fd error]: {safe}");
}
```

pi spawns fd with `stdio: ["ignore", "pipe", "pipe"]` (`find.ts:269`), so stderr is **not** a terminal and `maybe_sanitize` is a passthrough — the text arrives verbatim.

Because the only root was filtered out, the vector is empty and fd then bails — this half is in the vendored copy at [tmp/ref/fd/main.rs](../../../tmp/ref/fd/main.rs):

```rust
// main.rs:84-86
let search_paths = opts.search_paths()?;
if search_paths.is_empty() {
    bail!("No valid search paths given.");
}
```

```rust
// main.rs:68-71 — the error is printed through the SAME prefix, then exit code 1
Err(err) => {
    crate::error::print_error(format!("{err:#}"));
    ExitCode::GeneralError.exit();
}
```

`ExitCode::GeneralError` is `1` (`fd/src/exit_codes.rs`). So a `find` against a regular file leaves fd with **empty stdout, exit code 1, and exactly two stderr lines**, and pi rejects on that:

```ts
// find.ts:297-310
child.on("close", (code) => {
    cleanup();
    if (signal?.aborted) { … }
    const output = lines.join("\n");
    if (code !== 0) {
        const errorMsg = stderr.trim() || `fd exited with code ${code}`;
        if (!output) {
            settle(() => reject(new Error(errorMsg)));
            return;
        }
    }
```

**The exact message the model sees upstream** (`stderr.trim()`, two lines, `\n`-joined, no trailing newline):

```
[fd error]: Search path '/abs/path/src/index.ts' is not a directory.
[fd error]: No valid search paths given.
```

The success path pi keeps for a *valid* directory with no matches is `find.ts:311-319` — `No files found matching pattern`. That text is reachable in pi **only** for a real directory.

> Nuance, and the reason this task is MEDIUM rather than a redesign: pi's own `customOps.glob` branch is existence-only and behaves like cyrup does today. The divergence is against the fd branch — the branch [find.rs](../../../crates/cyrup-tools/src/tools/find.rs) declares it mirrors in every one of its in-source comments.

---

## What cyrup-tools does today — verified

[find.rs:122-129](../../../crates/cyrup-tools/src/tools/find.rs):

```rust
let search_root = path::resolve_to_cwd(input.path.as_deref().unwrap_or("."), &self.cwd);
self.fs
    .metadata(&search_root)
    .await
    // Pi: `Path not found: ${searchPath}` (find.ts:158).
    .map_err(|_| {
        error::not_found(format!("Path not found: {}", error::show(&search_root)))
    })?;
```

The `Meta` value is discarded — the call is a bare statement, and `Meta::is_dir` ([ops/mod.rs:34-41](../../../crates/cyrup-tools/src/ops/mod.rs)) is never read. A regular-file root therefore sails through, and the rest of the tool degrades quietly:

- `self.fs.walk(&search_root, …)` ([find.rs:148-154](../../../crates/cyrup-tools/src/tools/find.rs)) is built as `WalkBuilder::new(&root)` unconditionally ([ops/local/fs.rs:209-241](../../../crates/cyrup-tools/src/ops/local/fs.rs)), which yields exactly one item for a file root: the root itself.
- [find.rs:188-190](../../../crates/cyrup-tools/src/tools/find.rs) drops it — `if w.path == search_root { continue; }`.
- `results` is empty, so [find.rs:223-230](../../../crates/cyrup-tools/src/tools/find.rs) returns `Ok` with `No files found matching pattern`.

Two consequences, both model-visible:

1. `find(pattern: "*.ts", path: "src/index.ts")` — the model passing a file where a directory belongs — is reported as a **successful empty search**. The model concludes the tree holds nothing matching and stops looking, with nothing in the transcript naming the mistake. Upstream, the same call raises an error the model can read and correct.
2. A genuinely missing path produces `Path not found: <p>` here versus fd's two-line diagnostic upstream.

Also verified while auditing: the in-source citation `(find.ts:158)` on line 126 is **stale** — at v0.84.3 that literal lives at `find.ts:171`, and it is inside the `customOps.glob` branch, which is not the branch this code implements. The replacement below removes the line, so the stale cite goes with it.

---

## The idiom to reuse — the crate already has this guard, once

[ls.rs:86-98](../../../crates/cyrup-tools/src/tools/ls.rs), quoted verbatim — this is the crate's one existing tool-level directory guard, and the new code must look like it:

```rust
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

The parts to carry over: `error::invalid` for the not-a-directory rejection, `error::show(&path)` for rendering the path, the guard sited immediately after the metadata call and before any traversal, and an in-source comment naming the upstream source of the literal.

The part **not** to carry over: `ls`'s two separate messages. `ls` splits them because pi's `ls.ts` splits them (`Path not found:` at `ls.ts:129`, `Not a directory:` at `ls.ts:141`). `find`'s upstream does not split: `is_existing_directory` is a single predicate covering both cases and emits one message for both, so `find` gets **one** rejection site.

---

## Required change

One file changes:

- [crates/cyrup-tools/src/tools/find.rs](../../../crates/cyrup-tools/src/tools/find.rs)

Nothing else. No new imports (`error` and `path` are already in scope at `find.rs:9`), no new helper, no signature change, no change to `FsOps`.

### CURRENT — [find.rs:122-129](../../../crates/cyrup-tools/src/tools/find.rs)

```rust
        let search_root = path::resolve_to_cwd(input.path.as_deref().unwrap_or("."), &self.cwd);
        self.fs
            .metadata(&search_root)
            .await
            // Pi: `Path not found: ${searchPath}` (find.ts:158).
            .map_err(|_| {
                error::not_found(format!("Path not found: {}", error::show(&search_root)))
            })?;
```

### REPLACEMENT — write exactly this

```rust
        let search_root = path::resolve_to_cwd(input.path.as_deref().unwrap_or("."), &self.cwd);
        // Pi's fd branch has NO pre-check: it hands the absolute search path to fd as fd's root
        // (find.ts:267) and lets fd validate it. fd's gate is ONE predicate — `is_existing_directory`
        // = `path.is_dir() && …` (fd/src/filesystem.rs:38-42) — so a MISSING path and a path that
        // exists but is not a directory take the same branch and print the same line via
        // `print_error` (fd/src/error.rs), which prefixes `[fd error]: `. With the only root
        // filtered out, `search_paths()` returns empty and fd bails through the same prefix
        // (main.rs:84-86, :68-71), exiting 1 with empty stdout. pi rejects with `stderr.trim()`
        // (find.ts:304-309), i.e. both lines. So: one gate here, one two-line message, and the
        // `Path not found:` literal — which is pi's `customOps.glob` branch (find.ts:171), NOT the
        // fd branch this tool implements — does not belong on this path.
        // `FsOps::metadata` follows symlinks (ops/local/fs.rs:170-183), matching `Path::is_dir`, so
        // a symlink to a directory is still a valid search root.
        if !self
            .fs
            .metadata(&search_root)
            .await
            .is_ok_and(|meta| meta.is_dir)
        {
            return Err(error::invalid(format!(
                "[fd error]: Search path '{}' is not a directory.\n\
                 [fd error]: No valid search paths given.",
                error::show(&search_root)
            )));
        }
```

### Why this exact shape

- **One gate, not two.** fd rejects missing and non-directory roots through a single predicate with a single message; splitting them here would invent a distinction pi does not surface.
- **`error::invalid`, matching `ls`.** Both `error::invalid` and `error::not_found` construct the same flat `ToolError::new(msg)` ([error.rs:12-19](../../../crates/cyrup-tools/src/error.rs)), so the model-visible result is the message alone; `invalid` is the one `ls` uses for its directory guard and the one this rejection is.
- **`error::show`** renders the absolute path exactly as `path.to_string_lossy()` does inside fd, and cyrup's `search_root` is the same absolute string pi passes as `searchPath`.
- **Placement is unchanged** — the guard replaces the existing stat at the existing site, before `PatternMatcher::build`, before `inside_git_repo`, before `fs.walk`. A rejected root must never reach the ancestor `.git` probe or the walker.
- **The cancellation check at [find.rs:115-117](../../../crates/cyrup-tools/src/tools/find.rs) stays first.** An already-cancelled call still reports the abort without touching the filesystem.
- The `\` line continuation inside `format!` keeps the literal two lines joined by a single `\n` with no leading whitespace on the second line and no trailing newline — byte-identical to pi's `stderr.trim()`.

### Explicitly out of scope

- `grep.rs`'s `Path not found: <p>` ([grep.rs:303-306](../../../crates/cyrup-tools/src/tools/grep.rs)) — `grep`'s upstream is ripgrep, not fd, and its message is its own parity question. Do not touch it.
- `ls.rs`'s two messages — they match `ls.ts` and are correct as they stand.
- The walker, the matcher, the limit/truncation logic, `details`, and the empty-result success path for real directories. This is a parity fix at one gate, not a redesign.

---

## Why this gap is real

An adversary agent was tasked with **refuting** this finding by locating the capability in the Rust under another name. It could not:

> Confirmed absent after exhaustive search. crates/cyrup-tools/src/tools/find.rs:122-129 calls fs.metadata(&search_root) purely for its Err arm (Path not found: <p>) and discards the Metadata value — is_dir is never consulted. Repo-wide ripgrep for is_dir across crates/cyrup-tools/src and crates/cyrup-core/src returns only one tool-level directory guard, ls.rs:93-98 (Not a directory: …); every other hit is the Metadata/WalkItem field itself (ops/mod.rs:37,254), its population (ops/local/fs.rs:166,178,231-232), find.rs:208's trailing-slash formatting, or grep.rs:398's dir-skip while walking. Searches for "Not a directory|ENOTDIR|NotADirectory" find only ls.rs and lock.rs's isMissingPathError errno set (unreachable from find). No wrapper supplies it: isolation/traversal.rs:133-142 walk only confines against root escape, isolation/protected.rs:150-156 walk is a passthrough, and ops/local/fs.rs:209-241 builds WalkBuilder::new(&root) unconditionally — a file root yields exactly one entry, which find.rs:188-190 drops as w.path == search_root, falling into the empty branch at find.rs:223-230 and returning the success text "No files found matching pattern". Upstream verified: find.ts's ops.exists guard is inside the customOps?.glob branch only; the default branch hands searchPath to fd (find.ts:267) and rejects on non-zero exit with trimmed stderr, and fd rejects a non-directory search path. Nuance: pi's own custom-ops glob branch is exists-only and behaves exactly like cyrup, so the divergence is against the fd branch that cyrup's comments mirror. Severity medium, not high: it requires the model to pass a file where a directory is expected, and the harm is a misleading success (model concludes the tree is empty) rather than data loss; the separate error-text difference for a genuinely missing path is cosmetic.

Citations re-verified during this augmentation pass. Two corrections to the original filing, both applied above:

- `Path not found` is `find.ts:171`, not `find.ts:158`, and it sits inside the `customOps.glob` branch.
- fd's stderr for this case is **two** lines, not one — the per-path `Search path '…' is not a directory.` **and** the `No valid search paths given.` bail, both carrying the `[fd error]: ` prefix, both inside pi's `stderr.trim()`.

Everything else in the filing (the `ls.rs:93-98` guard, `find.ts:267`, `find.ts:304-309`, `find.rs:188-190`, `find.rs:223-230`) checked out unchanged.

---

## Definition of done

1. `find` with `path` naming an existing **regular file** fails, and the failure message is exactly:

   ```
   [fd error]: Search path '<absolute path>' is not a directory.
   [fd error]: No valid search paths given.
   ```

   where `<absolute path>` is the cwd-resolved search path. It no longer returns `No files found matching pattern`.
2. `find` with `path` naming a **nonexistent** path fails with that same two-line message. The literal `Path not found:` no longer appears anywhere in `find.rs`.
3. `find` with `path` naming a **directory that contains no match** still succeeds with `No files found matching pattern`.
4. `find` with `path` naming a **symlink to a directory** still searches through it and returns that directory's matches.
5. `find` with `path` naming a directory that does contain matches returns the identical output it returns today — same rows, same order, same `[… results limit reached]` / `[… limit reached]` notices, same `details`.
6. A `find` invoked with an already-cancelled token still fails with the abort error, and does so without stat-ing the search path.
7. `ls` and `grep` messages are byte-for-byte unchanged: `ls` still answers `Path not found: <p>` and `Not a directory: <p>`, `grep` still answers `Path not found: <p>`.
8. Nothing pi lacks is introduced — the only new behaviour is the rejection fd already performs.
