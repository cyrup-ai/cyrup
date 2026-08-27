---
title: Symlinked files inside the search tree are searched by cyrup but skipped by pi
priority: MEDIUM
tool: grep
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: qa
status: completed
updated: 2026-08-27 14:11
---

# Symlinked files inside the search tree are searched by cyrup but skipped by pi

## Core objective

`grep` must apply ripgrep's **subject filter** to every candidate the walk discovers: a
traversal-discovered entry is searched only when the entry's *own* file type is a regular file. A
symlink found inside the tree is not followed and not searched; a symlink named **explicitly** as
the `path` argument still is. cyrup currently searches everything the walk yields that is not a
directory, which follows every in-tree symlink and also opens FIFOs, sockets and device nodes.

The filter cannot be written today. [`WalkItem`, ops/mod.rs:250-255](../../../crates/cyrup-tools/src/ops/mod.rs)
carries `path` and `is_dir` and nothing else, so *whether an entry is a regular file is structurally
unavailable to every consumer of `FsOps::walk`*. The work is therefore three edits in three files,
and the one in `grep.rs` is the last and smallest of them.

## What pi does — verified, both halves

pi shells out to ripgrep with no `--follow`/`-L`. The argv is built at
[grep.ts:220](../../../tmp/pi/packages/coding-agent/src/core/tools/grep.ts) —
`["--json", "--line-number", "--color=never", "--hidden"]` — the positional tail is pushed at
`:224` (`args.push("--", pattern, searchPath)`), and the child is spawned at `:226`. Vendored pi is
**0.84.3** (`packages/coding-agent/package.json:3`). No flag in that list, and no flag pi can add,
enables link following.

ripgrep's rule lives in its `SubjectBuilder`: an entry is turned into a searchable `Subject` when it
is **explicit** (`dent.depth() == 0`, i.e. named on the command line) *or* when
`file_type().is_file()` holds. Traversed entries are depth > 0, so they must pass the regular-file
test. Both halves confirmed against rg 14.1.0 on a tree whose only member is
`link.txt -> ../outside/real.txt` (target contains `NEEDLE`):

| invocation | result |
| --- | --- |
| `rg --json --line-number --color=never --hidden -- NEEDLE .` (link **discovered**) | no `"type":"match"` events, exit 1 |
| `rg --json --line-number --color=never --hidden -- NEEDLE link.txt` (link **explicit**) | one match on `link.txt`, exit 0 |
| `rg --hidden -- NEEDLE .` with a symlink-**to-directory** inside | not descended, exit 1 |
| `rg --hidden -- NEEDLE dirlink` (symlink-to-directory **explicit**) | descended, `dirlink/real.txt:NEEDLE here`, exit 0 |

So the divergence is confined to entries the **traversal** produced. The explicit-argument path is
already at parity and must not be touched — see *What must not change*.

## What cyrup-tools does today

[grep.rs:398](../../../crates/cyrup-tools/src/tools/grep.rs) admits every walk item that is not a
directory:

```rust
Some(Ok(w)) if !w.is_dir => {
```

and hands `w.path` to `search_one`, which opens it at
[grep.rs:93](../../../crates/cyrup-tools/src/tools/grep.rs) via `FsOps::read_stream` —
`std::fs::File::open` at [fs.rs:73-79](../../../crates/cyrup-tools/src/ops/local/fs.rs), with no
`O_NOFOLLOW`. `open(2)` follows the link, so the **target's** bytes are searched under the **link's**
name.

The producing walker at [fs.rs:227-239](../../../crates/cyrup-tools/src/ops/local/fs.rs) never calls
`follow_links`, so `ignore` yields the symlink entry itself, and line 231 collapses it:

```rust
let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
Ok(WalkItem { path, is_dir })
```

A symlink is not a directory, so `is_dir == false`, so `!w.is_dir` passes, and the type information
that would have distinguished it is discarded one line before the struct is built. `WalkItem` is
re-exported from [lib.rs:44-49](../../../crates/cyrup-tools/src/lib.rs) and
[isolation/mod.rs:42](../../../crates/cyrup-tools/src/isolation/mod.rs), and both `FsOps` decorators
([protected.rs:154-155](../../../crates/cyrup-tools/src/isolation/protected.rs),
[traversal.rs:137-139](../../../crates/cyrup-tools/src/isolation/traversal.rs)) only delegate — no
downstream layer can reconstruct the file type, so no filter could exist anywhere but here.

Verified by running the tool on the same fixture: output is `link.txt:1: NEEDLE`.

The same guard also admits FIFOs, sockets and device nodes, which `File::open` will happily open — a
`grep` rooted anywhere above a named pipe can block in `open(2)` or `read(2)` on content ripgrep
would never have touched.

## Why the entry type survives the traversal, and why it does not survive `WalkItem`

The workspace pins the `ignore` crate at **0.4.26** (`Cargo.toml:149`, `Cargo.lock:3883-3885`). In
that version a traversal-discovered entry's type comes from `std::fs::DirEntry::file_type`
(`walk.rs:322-333` `from_entry`, `:353-367` `from_entry_os` on unix), which is `readdir`/`lstat`
semantics — **not followed**. `entry.file_type().is_symlink()` is therefore `true` and
`is_file()` is `false` for exactly the entries ripgrep skips. The information is present at
[fs.rs:231](../../../crates/cyrup-tools/src/ops/local/fs.rs) and is thrown away by the very next
line. That is the entire bug.

Two `ignore`-0.4.26 details the change relies on and must not accidentally break:

* **Depth-0 roots.** `WalkBuilder::build()` produces the single-threaded `Walk` (`walk.rs:1094-1163`),
  which is backed by `walkdir`. `walkdir` 2.5.0 `handle_entry` (`lib.rs:840-880`) applies
  `follow_root_links`: a depth-0 symlink **to a directory** is descended into, while the root entry
  itself is still yielded with its unfollowed type (`is_symlink`, so neither `is_dir` nor `is_file`).
  Skipping that root entry is correct — it is a directory link, it has no bytes — and the children
  it yields at depth 1 are unaffected.
* **`file_type()` is `Option`.** It is `None` only for the synthetic stdin entry
  (`DirEntry::new_stdin`, `walk.rs:67-78`), which a path-rooted `WalkBuilder` never produces.
  `unwrap_or(false)` reproduces ripgrep's own `map_or(false, |ft| ft.is_file())`: an entry of
  unknown type is not a regular file, so it is not searched.

## Sibling coordination — read this before editing

Three other briefs touch this same walker.

**Finished, already prescribed, no conflict with this task:**

* [LOW — find does not honor .fdignore or fd's global ignore file](LOW-find-does-not-honor-fdignore-or-fd-s-global-ignore-file.md)
  adds a `WalkFlavor { Plain, Fd, Rg }` enum plus a `flavor` field to **`WalkOpts`**
  (ops/mod.rs:244-248) and restructures the `WalkBuilder` **chain** at fs.rs:213-226 into
  `let mut builder = …; …; let walker = builder.build();`. Its brief states in terms that the
  `for result in walker { … }` loop below the chain is untouched.
* [LOW — .rgignore files are honored by pi but ignored by cyrup](LOW-rgignore-files-are-honored-by-pi-but-ignored-by-cyrup.md)
  is one match arm inside `WalkFlavor::custom_ignore_filename` in ops/mod.rs and touches
  `ops/local/fs.rs` not at all.

Neither of those touches `WalkItem` (a different struct from `WalkOpts`, immediately below it) and
neither touches the loop body at fs.rs:227-239. **This task edits `WalkItem` and the loop body
only.** If the builder chain in the *Current* snippet below already reads `let mut builder = …`, that
is the fd task having landed: leave it exactly as found and edit the loop underneath it.

**In progress, and the one that can collide:**

* [MEDIUM — a single unreadable directory aborts the whole find](MEDIUM-a-single-unreadable-directory-aborts-the-whole-find-pi-returns-the-resul.md)
  makes the walk tolerate per-entry errors instead of aborting. Its *Parity action* names
  `find.rs:212` and `grep.rs:428` — both currently `Some(Err(e)) => return Err(e)` — and it may also
  change how `LocalFs::walk` maps an `ignore::Error` into the channel at fs.rs:234.

The interaction is exact and narrow:

> **This task adds a FIELD to `WalkItem` and changes the guard on the `Ok` arm. That task changes
> how walk ERRORS are yielded and consumed — the `Err` arm.** In `grep.rs` the two arms sit in the
> same `match item { … }` block (lines 397-430), so they are adjacent but textually disjoint. In
> `ops/local/fs.rs` this task edits the `Ok(entry) => { … }` branch at lines 229-233 and that task
> edits the `Err(e) => …` branch at line 234 of the same `match result`.

**Required ordering: this task lands FIRST.** The reason is asymmetric, not stylistic. This task
changes a public struct with public fields, so it invalidates every `WalkItem` struct literal in the
workspace; the unreadable-directory task changes only arm *bodies* and adds no literals, so it
rebases onto a widened `WalkItem` with no edit at all. The reverse order forces a re-read of a match
block that has already moved. There is exactly **one** `WalkItem` literal in the whole tree —
fs.rs:232 (`rg 'WalkItem \{' crates/` returns the definition and that single construction) — so
adding the field is a one-site change, and no re-export list changes.

If the unreadable-directory task has already landed when this one is executed, the only consequence
is that `grep.rs`'s `Err` arm no longer reads `Some(Err(e)) => return Err(e)`. **Leave it exactly as
found.** This task must not restore, rewrite or reason about that arm; its only edit in `grep.rs` is
the guard on line 398.

## The change

### 1. `crates/cyrup-tools/src/ops/mod.rs` — give `WalkItem` the file type

**Current** ([ops/mod.rs:250-255](../../../crates/cyrup-tools/src/ops/mod.rs)):

```rust
/// A single walked path.
#[derive(Clone, Debug)]
pub struct WalkItem {
    pub path: PathBuf,
    pub is_dir: bool,
}
```

**Replacement**:

```rust
/// A single walked path.
///
/// `is_dir` and `is_file` are BOTH carried because they are not complements. A symlink, a FIFO, a
/// socket and a device node are each neither, and separating those from a regular file is the
/// reason this type exists rather than a bare `PathBuf`. Both flags describe the ENTRY's own type
/// — `lstat` semantics, never followed — because that is the type the upstream binaries decide on:
/// `ignore` 0.4.26 builds a traversed entry's type from `std::fs::DirEntry::file_type`
/// (`walk.rs:322-333`, `:353-367`), which does not resolve the link.
///
/// `grep` filters on `is_file` ALONE, reproducing ripgrep's `SubjectBuilder`: a traversal-discovered
/// entry is searched only when `file_type().is_file()` holds, so an in-tree symlink is never opened
/// and its target is never searched under the link's name. `find` filters on `is_dir` ALONE and must
/// keep doing so — fd DOES list symlinks, and `find` uses the flag only to decide the trailing `/`
/// that marks a directory in its output.
#[derive(Clone, Debug)]
pub struct WalkItem {
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_file: bool,
}
```

`is_file`, not `file_type: Option<FileType>`: `bool` keeps `WalkItem` free of a `std::fs` type in the
backend seam's public surface, and `is_file()` is precisely — and only — the predicate ripgrep
applies. Nothing downstream needs to distinguish a socket from a FIFO.

### 2. `crates/cyrup-tools/src/ops/local/fs.rs` — populate it

**Current** ([fs.rs:227-239](../../../crates/cyrup-tools/src/ops/local/fs.rs), the loop below the
`WalkBuilder` chain):

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

**Replacement** (the `Err` arm and the `blocking_send` are reproduced only for placement — copy
whatever they currently say, see *Sibling coordination*):

```rust
for result in walker {
    let item = match result {
        Ok(entry) => {
            let path = entry.path().to_path_buf();
            // One `file_type()` read feeds both flags. `ignore` 0.4.26 derives a traversed
            // entry's type from `std::fs::DirEntry::file_type` (`walk.rs:322-333`,
            // `:353-367`), which is `lstat` semantics: a symlink reports itself as a symlink,
            // so it is neither a dir nor a file and both flags are false — exactly the entry
            // ripgrep declines to search. `None` occurs only for the synthetic stdin entry
            // (`walk.rs:67-78`), which a path-rooted walker never yields, so `unwrap_or(false)`
            // mirrors ripgrep's own `file_type().map_or(false, |ft| ft.is_file())`: unknown
            // type is not a regular file.
            let ty = entry.file_type();
            let is_dir = ty.map(|t| t.is_dir()).unwrap_or(false);
            let is_file = ty.map(|t| t.is_file()).unwrap_or(false);
            Ok(WalkItem {
                path,
                is_dir,
                is_file,
            })
        }
        Err(e) => Err(ToolError::new(format!("walk: {e}"))),
    };
    if tx.blocking_send(item).is_err() {
        break;
    }
}
```

`std::fs::FileType` is `Copy`, so binding `ty` once and mapping it twice needs no clone and no second
syscall. The import at [fs.rs:8](../../../crates/cyrup-tools/src/ops/local/fs.rs) is unchanged.

### 3. `crates/cyrup-tools/src/tools/grep.rs` — the filter

One line. Nothing else in the arm moves.

**Current** ([grep.rs:398](../../../crates/cyrup-tools/src/tools/grep.rs)):

```rust
Some(Ok(w)) if !w.is_dir => {
```

**Replacement**:

```rust
// ripgrep's subject filter. A traversal-discovered entry is searched only when its OWN
// file type is a regular file (`SubjectBuilder::build` → `Subject::is_file`); pi passes
// no `--follow`/`-L` (grep.ts:220-224), so a symlink inside the tree is not followed and
// not searched. `!w.is_dir` admitted symlinks — and FIFOs, sockets and device nodes —
// then handed the path to `read_stream`'s `File::open`, which has no `O_NOFOLLOW` and
// DOES follow, searching the target's bytes under the link's name.
//
// This filter is for WALK-discovered candidates only. A `path` argument that is a
// symlink to a file never reaches this loop: `FsOps::metadata` (fs.rs:170-182) stats
// through the link, so `meta.is_file` is true at grep.rs:342 and the file is searched
// directly — which is what ripgrep does for a depth-0 explicit subject.
Some(Ok(w)) if w.is_file => {
```

The `Some(Ok(_)) => {}` arm at [grep.rs:427](../../../crates/cyrup-tools/src/tools/grep.rs) already
absorbs everything the guard rejects; after this change that set is directories *plus* symlinks,
FIFOs, sockets and device nodes. It needs no edit.

## What must not change

* **[find.rs:208](../../../crates/cyrup-tools/src/tools/find.rs) —
  `let entry = if w.is_dir { format!("{rel}/") } else { rel };` — must keep reading `is_dir` and must
  keep its current behaviour.** fd DOES list symlinks, so `find` is already at parity; it uses the
  flag purely to append the directory-marking `/`. A symlink must continue to be listed, and must
  continue to be listed without the trailing slash. `find` must not gain an `is_file` filter, and no
  `find` result may appear or disappear as a result of this task.
* **The explicit-path branch at [grep.rs:342](../../../crates/cyrup-tools/src/tools/grep.rs)
  (`if meta.is_file { … }`).** `FsOps::metadata` uses `tokio::fs::metadata`
  ([fs.rs:170-182](../../../crates/cyrup-tools/src/ops/local/fs.rs)), which follows symlinks, so
  `grep path=<symlink-to-file>` already takes this branch and searches the target — matching
  ripgrep's explicit-subject rule, confirmed above against rg 14.1.0. Do not add a filter here.
* **The `Err` arm of the `match item` block in `grep.rs`**, whatever it currently says — owned by the
  unreadable-directory sibling.
* **The `WalkBuilder` chain at fs.rs:213-226** — owned by the `.fdignore` sibling. `follow_links` is
  not called today and must not start being called: following at the walker would defeat the whole
  point.
* The stale `ignore-0.4.33` version in the in-source comment at
  [grep.rs:405](../../../crates/cyrup-tools/src/tools/grep.rs) (the workspace pins 0.4.26) sits
  inside the arm body being kept verbatim. It is unrelated to this gap; leave it.

## Files changed

| File | Change |
| --- | --- |
| [crates/cyrup-tools/src/ops/mod.rs](../../../crates/cyrup-tools/src/ops/mod.rs) | `WalkItem` gains `pub is_file: bool`, plus the doc explaining why both flags are carried |
| [crates/cyrup-tools/src/ops/local/fs.rs](../../../crates/cyrup-tools/src/ops/local/fs.rs) | The one `WalkItem` construction in the workspace populates `is_file` from the same `file_type()` read |
| [crates/cyrup-tools/src/tools/grep.rs](../../../crates/cyrup-tools/src/tools/grep.rs) | Walk-candidate guard becomes `w.is_file` |

No re-export list changes: `WalkItem` is already exported from
[lib.rs:44-49](../../../crates/cyrup-tools/src/lib.rs) and
[isolation/mod.rs:42](../../../crates/cyrup-tools/src/isolation/mod.rs), and adding a field to an
exported struct does not alter either list. Both `FsOps` decorators delegate `walk` verbatim and are
untouched.

## Definition of done

Fixture: a directory `tree/` whose only entry is `link.txt -> ../outside/real.txt`, where
`outside/real.txt` contains `NEEDLE`.

1. `grep pattern="NEEDLE" path="tree"` reports **`No matches found`**. Today it reports
   `link.txt:1: NEEDLE`.
2. `grep pattern="NEEDLE" path="tree/link.txt"` still reports the match on `link.txt` — an explicitly
   named symlink is searched, matching rg 14.1.0 exit 0 on the same argument.
3. `grep pattern="NEEDLE" path="dirlink"`, where `dirlink` is a symlink to a directory containing a
   matching regular file, still reports the match under `dirlink/…`; a symlink to a directory
   *discovered inside* the search tree is not descended and contributes nothing.
4. In a tree containing both a regular file and a symlink pointing at it, `grep` reports the match
   once, under the regular file's name, and never under the link's name.
5. A named pipe, socket or device node inside the search tree is never opened by `grep`; a `grep`
   rooted above one completes instead of blocking.
6. Matches from files outside the search tree no longer appear, and no longer consume the match
   budget that stops the fused walk at
   [grep.rs:373-375](../../../crates/cyrup-tools/src/tools/grep.rs); a tree whose in-tree matches
   previously exceeded the limit because of symlink hits now returns those in-tree matches.
7. For every input where the search root contains no symlinks and no non-regular files, `grep`
   output is byte-identical to before this change — same matches, same order, same notices.
8. `find` output is unchanged for every input, symlinked trees included: symlinks are still listed,
   still without a trailing `/`.
