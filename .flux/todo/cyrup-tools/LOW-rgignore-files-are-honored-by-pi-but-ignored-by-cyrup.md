---
title: .rgignore files are honored by pi but ignored by cyrup
priority: LOW
tool: grep
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: aug
status: done
updated: 2026-08-27
---

# .rgignore files are honored by pi but ignored by cyrup

## Ordering dependency — read this first

**This task does not land on its own. It lands on top of
[Find does not honor .fdignore or fd's global ignore file](./LOW-find-does-not-honor-fdignore-or-fd-s-global-ignore-file.md),
which MUST be implemented first.**

That sibling brief builds the entire mechanism this task needs: a `Copy` `WalkFlavor { Plain, Fd,
Rg }` enum added to [`WalkOpts`](../../../crates/cyrup-tools/src/ops/mod.rs), threaded through the
shared `FsOps::walk` seam, consumed in
[`LocalFs::walk`](../../../crates/cyrup-tools/src/ops/local/fs.rs) as
`if let Some(name) = opts.flavor.custom_ignore_filename() { builder.add_custom_ignore_filename(name); }`,
with [`grep`](../../../crates/cyrup-tools/src/tools/grep.rs) **already passing
`flavor: WalkFlavor::Rg`** and `WalkFlavor::custom_ignore_filename` **already carrying an explicit
`Self::Rg => None` arm** — behaviour-neutral, deliberately left as a wired empty slot for this task.

Do **not** re-derive that design, and do **not** touch the `WalkBuilder` construction in
`ops/local/fs.rs`. After the fd task lands, the whole behavioural change described here is **one
match arm** in [ops/mod.rs](../../../crates/cyrup-tools/src/ops/mod.rs), plus one now-stale comment
in [grep.rs](../../../crates/cyrup-tools/src/tools/grep.rs).

Verified 2026-08-27: the fd task has **not** landed yet.
[`WalkOpts`, ops/mod.rs:244-248](../../../crates/cyrup-tools/src/ops/mod.rs) still carries only
`include_hidden` and `require_git`, and `WalkFlavor` does not exist. If that is still true when this
task is picked up, stop and do the fd task first.

## Core objective

`grep` must reproduce **ripgrep's complete default ignore set**. cyrup's `grep` is an in-process
re-implementation over `ignore::WalkBuilder`
([grep.rs:1-3](../../../crates/cyrup-tools/src/tools/grep.rs)) rather than a spawn of the real `rg`
binary, so every ignore source ripgrep enables by default has to be enabled explicitly on the
builder. Exactly one is missing: **`.rgignore` files**.

`.rgignore` is opt-in on `WalkBuilder` — an `add_custom_ignore_filename` registration, not a name
baked into the `ignore` crate. It is not registered today, so it is silently inert.

## Upstream behaviour — verified

pi's `grep` resolves the real `rg` binary at
[grep.ts:177](../../../tmp/pi/packages/coding-agent/src/core/tools/grep.ts)
(`const rgPath = await ensureTool("rg");`) and spawns it at
[grep.ts:226](../../../tmp/pi/packages/coding-agent/src/core/tools/grep.ts) with the argv built at
[grep.ts:220-224](../../../tmp/pi/packages/coding-agent/src/core/tools/grep.ts):

```ts
const args: string[] = ["--json", "--line-number", "--color=never", "--hidden"];
if (ignoreCase) args.push("--ignore-case");
if (literal) args.push("--fixed-strings");
if (glob) args.push("--glob", glob);
args.push("--", pattern, searchPath);
```

That argv contains **no** `--no-ignore`, `--no-ignore-dot`, `--no-ignore-vcs`,
`--no-ignore-parent`, `--no-require-git`, and no `--ignore-file`. So `no_ignore_dot` is `false` and
ripgrep builds its walker as
([ripgrep 14.1.0 `crates/core/flags/hiargs.rs:868-899`](../../../tmp/ref/hiargs.rs), upstream
`BurntSushi/ripgrep`):

```rust
pub(crate) fn walk_builder(&self) -> anyhow::Result<ignore::WalkBuilder> {
    let mut builder = ignore::WalkBuilder::new(&self.paths.paths[0]);
    for path in self.paths.paths.iter().skip(1) {
        builder.add(path);
    }
    if !self.no_ignore_files {
        for path in self.ignore_file.iter() {
            if let Some(err) = builder.add_ignore(path) {
                ignore_message!("{err}");
            }
        }
    }
    builder
        /* max_depth / follow_links / max_filesize / threads / same_file_system / overrides / types */
        .hidden(!self.hidden)
        .parents(!self.no_ignore_parent)
        .ignore(!self.no_ignore_dot)
        .git_global(!self.no_ignore_vcs && !self.no_ignore_global)
        .git_ignore(!self.no_ignore_vcs)
        .git_exclude(!self.no_ignore_vcs && !self.no_ignore_exclude)
        .require_git(!self.no_require_git)
        .ignore_case_insensitive(self.ignore_file_case_insensitive);
    if !self.no_ignore_dot {
        builder.add_custom_ignore_filename(".rgignore");
    }
    /* sort … */
}
```

Four facts to carry into the Rust:

* ripgrep registers `.rgignore` with `add_custom_ignore_filename` — **not** a hard-coded name
  inside the `ignore` crate. It is inert unless registered. That is the entire gap.
* `.rgignore` is gated on the **same** knob as `.ignore` (`!no_ignore_dot`). cyrup never disables
  `.ignore` (the crate defaults `WalkBuilder::ignore` to `true` and `fs.rs` leaves it alone) and pi
  passes no `--no-ignore-dot`, so the cyrup-side registration is **unconditional** for the `Rg`
  flavor. rg 14.1.0's own `--no-ignore-dot` help text — *"Don't respect filter rules from .ignore or
  .rgignore files"* — confirms the two names move together.
* ripgrep has **no global ignore file**. The only `add_ignore` call in `walk_builder` is fed from
  `--ignore-file`, which pi never passes. So the `Rg` arm of `WalkFlavor::reads_fd_global_ignore`
  stays `false` and nothing else attaches here. (`git_global` — the user's `core.excludesFile`
  gitignore — is a different source and is already `true` in cyrup.)
* `.parents(!self.no_ignore_parent)` = `true`, matching cyrup's existing `parents(true)`. That is
  what makes `.rgignore` apply in **ancestors** of the search root too: `Ignore::add_parents` runs
  `add_child_path` per parent, and `add_child_path` compiles the custom-ignore matcher from
  `custom_ignore_filenames` for every directory it visits (ignore 0.4.26 `dir.rs:182-247` and
  `dir.rs:273-296`).

Empirically confirmed against the installed `rg 14.1.0`: in a git repo containing `a.txt`, `b.txt`
and `.rgignore` = `b.txt`, `rg --json --hidden -- NEEDLE .` reports only `./a.txt`.

## Current Rust behaviour — verified

[`LocalFs::walk`, fs.rs:209-226](../../../crates/cyrup-tools/src/ops/local/fs.rs) is the **only**
`WalkBuilder` in the workspace:

```rust
let walker = WalkBuilder::new(&root)
    .hidden(!opts.include_hidden)
    .git_ignore(true)
    .git_exclude(true)
    .git_global(true)
    .require_git(opts.require_git)
    .parents(true)
    .build();
```

`add_custom_ignore_filename` is never called anywhere under `crates/`, and there are zero
occurrences of `rgignore` in the workspace. `grep` reaches this walker at
[grep.rs:367-373](../../../crates/cyrup-tools/src/tools/grep.rs), and no `rg` subprocess exists
anywhere in the crate, so there is no alternate path by which `.rgignore` could be picked up.
Verified on the same `a.txt`/`b.txt`/`.rgignore` layout: cyrup searches **both** files.

`.ignore` **does** already work on both sides, and so does the whole gitignore family — this gap is
specific to `.rgignore`, which is why the priority is LOW: every match cyrup returns is still a
genuine match in a real file. The only user-visible harm is that a repository using `.rgignore` to
keep generated or vendored files out of searches gets those files back, where they consume the
100-match cap (`GrepOpts::limit` defaults to `GREP_MAX_MATCHES`,
[config.rs:271-284](../../../crates/cyrup-tools/src/config.rs); pi's `DEFAULT_LIMIT = 100`,
[grep.ts:44](../../../tmp/pi/packages/coding-agent/src/core/tools/grep.ts)).

### Citation correction

The original audit note cited *"Vendored ignore-0.4.33 (src/walk.rs:653)"* for the claim that
`.rgignore` is opt-in. Both halves are wrong for this workspace and are corrected here:

* The workspace pins **`ignore` 0.4.26**, not 0.4.33
  ([Cargo.toml:149](../../../Cargo.toml) — `ignore = { version = "0.4.26" }` — and `Cargo.lock`
  resolves to `0.4.26`). 0.4.33 exists in the local cargo registry cache but is not what builds.
* In 0.4.26 the API is `WalkBuilder::add_custom_ignore_filename` at `src/walk.rs:747`, documented
  *"These ignore files have higher precedence than all other ignore files."* (0.4.33's `walk.rs:653`
  is an unrelated doc-comment reference.)

The conclusion that note drew is unaffected: the name is opt-in, and cyrup never opts in.

The note's other citation — `cyrup-resources/src/discovery/scan.rs:172`, the hand-rolled
skill-discovery scanner iterating `[".gitignore", ".ignore", ".fdignore"]` — is **accurate as
written** and remains out of scope: that subsystem never touches `FsOps::walk`. The remaining
citations (`grep.ts:177`, `:226`; `fs.rs:213-226`; `grep.rs:367-373`) all verify.

## The change

### 1. `crates/cyrup-tools/src/ops/mod.rs` — the one load-bearing edit

**Current** — the state the fd task leaves behind, in
[ops/mod.rs](../../../crates/cyrup-tools/src/ops/mod.rs):

```rust
impl WalkFlavor {
    /// The custom ignore FILENAME this flavor's upstream reads, if any. Custom ignore files
    /// outrank `.ignore` and every gitignore source (ignore 0.4.26 `dir.rs:580-585`).
    pub fn custom_ignore_filename(self) -> Option<&'static str> {
        match self {
            Self::Plain => None,
            Self::Fd => Some(".fdignore"),
            // The `.rgignore` registration is the sibling `.rgignore` parity task; `grep`
            // already names itself here so that task lands as this one arm and nothing else.
            Self::Rg => None,
        }
    }
```

**Replacement**:

```rust
impl WalkFlavor {
    /// The custom ignore FILENAME this flavor's upstream reads, if any. Custom ignore files
    /// outrank `.ignore` and every gitignore source (ignore 0.4.26 `dir.rs:580-585`).
    ///
    /// Exactly ONE name is ever returned, so `find` can never see `.rgignore` and `grep` can
    /// never see `.fdignore`: the cross-contamination a shared walk seam otherwise invites is
    /// structurally impossible rather than merely avoided.
    pub fn custom_ignore_filename(self) -> Option<&'static str> {
        match self {
            Self::Plain => None,
            Self::Fd => Some(".fdignore"),
            // ripgrep registers `.rgignore` gated on `!no_ignore_dot` — the SAME knob that
            // gates `.ignore` (ripgrep 14.1.0 `crates/core/flags/hiargs.rs:891, :897-899`).
            // Pi's argv passes neither `--no-ignore` nor `--no-ignore-dot` (grep.ts:220-224)
            // and cyrup never disables `WalkBuilder::ignore`, so this is unconditional,
            // exactly as it is upstream.
            Self::Rg => Some(".rgignore"),
        }
    }
```

The `WalkFlavor::Rg` variant doc the fd task writes — *"Registers `.rgignore`; ripgrep has no global
ignore file, so nothing else attaches here."* — is already accurate for this end state and needs no
edit. `reads_fd_global_ignore` needs no edit either: ripgrep genuinely has no global ignore file.

### 2. `crates/cyrup-tools/src/tools/grep.rs` — retire the placeholder comment

The call site itself does not change. Only the comment inside it does — the fd task wrote it as a
forward reference to this task, and it becomes false the moment the arm above returns `Some`.

**Current** — the state the fd task leaves behind, at
[grep.rs:367-373](../../../crates/cyrup-tools/src/tools/grep.rs):

```rust
let mut walk = self.fs.walk(
    &search_root,
    WalkOpts {
        include_hidden: true,
        require_git: true,
        // Pi's `grep` IS ripgrep (grep.ts:177, :226). Behaviour-neutral today —
        // `WalkFlavor::Rg` registers nothing yet — and deliberately so: it is the seam the
        // sibling `.rgignore` task fills, and it guarantees `find`'s `.fdignore` can never
        // leak into grep.
        flavor: WalkFlavor::Rg,
    },
);
```

**Replacement**:

```rust
let mut walk = self.fs.walk(
    &search_root,
    WalkOpts {
        include_hidden: true,
        require_git: true,
        // Pi's `grep` IS ripgrep (grep.ts:177 `ensureTool("rg")`, spawned at `:226`), invoked
        // with no `--no-ignore`/`--no-ignore-dot`/`--ignore-file` (grep.ts:220-224), so
        // ripgrep's full default ignore set is in force: `.rgignore` on top of `.ignore` and
        // the gitignore family. ripgrep has no global ignore file, so unlike `find` nothing
        // else attaches. This also keeps `find`'s `.fdignore` out of grep.
        flavor: WalkFlavor::Rg,
    },
);
```

### Files that change

| File | Change |
| --- | --- |
| [crates/cyrup-tools/src/ops/mod.rs](../../../crates/cyrup-tools/src/ops/mod.rs) | `Self::Rg => Some(".rgignore")` in `WalkFlavor::custom_ignore_filename`, plus the doc lines above it |
| [crates/cyrup-tools/src/tools/grep.rs](../../../crates/cyrup-tools/src/tools/grep.rs) | comment text at the `WalkOpts` literal — no code change |

**No other file.** In particular
[ops/local/fs.rs](../../../crates/cyrup-tools/src/ops/local/fs.rs) is **not** touched: after the fd
task it already calls `opts.flavor.custom_ignore_filename()` and registers whatever comes back.
[find.rs](../../../crates/cyrup-tools/src/tools/find.rs),
[isolation/traversal.rs](../../../crates/cyrup-tools/src/isolation/traversal.rs),
[isolation/protected.rs](../../../crates/cyrup-tools/src/isolation/protected.rs),
[path.rs](../../../crates/cyrup-tools/src/path.rs),
[lib.rs](../../../crates/cyrup-tools/src/lib.rs) and
[isolation/mod.rs](../../../crates/cyrup-tools/src/isolation/mod.rs) are all unchanged — no new
type, no new field, no new re-export. `find.rs:150` and `grep.rs:369` are the only two `WalkOpts`
literals in the workspace and neither gains a field here.

## Resulting ignore precedence for `grep`

Highest to lowest, matching ripgrep exactly because both go through the same crate the same way
(ignore 0.4.26 `dir.rs:580-585`:
`m_custom_ignore.or(m_ignore).or(m_gi).or(m_gi_exclude).or(m_global).or(m_explicit)`):

1. `.rgignore` (custom ignore, including ancestors via `parents(true)`) — **new**
2. `.ignore`
3. `.gitignore`
4. `.git/info/exclude`
5. global gitignore (`core.excludesFile`)
6. *explicit ignores — empty for `grep`. ripgrep populates this slot only from `--ignore-file`,
   which pi never passes, and `WalkFlavor::Rg` registers nothing here.*

## Non-goals

* No `--no-ignore`-style escape hatch, no new `GrepOpts`
  ([config.rs:271-284](../../../crates/cyrup-tools/src/config.rs)) field, no `ToolsOptions` knob:
  ripgrep's ignore set is unconditional on pi's argv, so a toggle would be a capability pi does not
  have.
* No `--ignore-file` support and no global ignore file for `grep` — ripgrep has neither by default,
  and inventing one would be the mirror image of the gap being closed.
* No change to `hidden`, `git_ignore`, `git_exclude`, `git_global`, `require_git` or `parents`; all
  six already match ripgrep. `ignore_case_insensitive` already matches too — ripgrep defaults it
  `false`, cyrup never sets it, and the crate default is `false`.
* No change to `find`, to `WalkFlavor::Fd`, or to `reads_fd_global_ignore`.
* `cyrup-resources`' skill-discovery scanner
  ([scan.rs:172](../../../crates/cyrup-resources/src/discovery/scan.rs)) is a different subsystem
  and stays as-is, `.rgignore`-less.

## Genuinely uncertain

* **ripgrep version drift.** pi resolves `rg` through `ensureTool` — a system binary if one is
  present, otherwise a downloaded release — so the exact ripgrep in play is not pinned. This
  prescription targets 14.1.0. ripgrep has registered `.rgignore` for many major versions and its
  precedence has not moved, so the exposure is low, but it is not zero.
* **Case-insensitive filesystems.** ripgrep's `--ignore-file-case-insensitive` is off by default on
  every platform, so a file literally named `.RGIGNORE` is read by neither side. The two match by
  inaction rather than by construction.
* **Multiple custom ignore names.** `ignore` orders later-registered names *above* earlier ones.
  Only one name is ever registered per walk here, so that rule is unexercised — but it is the
  reason `custom_ignore_filename` returns a single `Option` rather than a list, and any future
  flavor wanting two names must confront it.

## Definition of done

Observable behaviour that must hold:

1. In a tree containing `.rgignore` with the line `b.txt`, where `a.txt` and `b.txt` both contain
   the search pattern, `grep` returns the match in `a.txt` and **no** match in `b.txt`. Without the
   `.rgignore`, both are returned.
2. A `.rgignore` placed in an **ancestor** of the search root is applied to the walk, matching
   ripgrep's `--no-ignore-parent`-off default.
3. A `.rgignore` line `!keep.txt` re-includes a file that a sibling `.gitignore` or `.ignore`
   excluded — `.rgignore` outranks both.
4. A `.rgignore` directory pattern such as `vendor/` removes every path beneath it from `grep`'s
   results, and those paths no longer consume the 100-match cap.
5. A missing, empty, or malformed `.rgignore` changes nothing about the results and surfaces no
   error to the caller; the patterns in it that did parse are still applied.
6. `find` results over a tree containing a `.rgignore` are **identical** to what they are after the
   fd task — `.rgignore` never reaches the find walker. Symmetrically, `grep` results over a tree
   containing a `.fdignore` or a `<config>/fd/ignore` are unchanged: neither reaches the grep
   walker.
7. Every path `grep` searched before this change, and that no `.rgignore` rule excludes, is still
   searched, in the same order, with the same `limit`/`max_bytes` truncation behaviour and the same
   output format.
8. No new `grep` parameter, `GrepOpts` field or configuration key exists — the behaviour is
   unconditional, as it is on pi's ripgrep argv.
