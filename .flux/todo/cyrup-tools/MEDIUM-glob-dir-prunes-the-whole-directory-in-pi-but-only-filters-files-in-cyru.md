---
title: Glob: "!dir" prunes the whole directory in pi, but only filters files in cyrup
priority: MEDIUM
tool: grep
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: exec
status: in-progress
updated: 2026-08-27
---

# Glob: "!dir" prunes the whole directory in pi, but only filters files in cyrup

## What pi does

pi hands `glob` to real ripgrep verbatim — `if (glob) args.push("--glob", glob);` at
[grep.ts:223](../../../tmp/pi/packages/coding-agent/src/core/tools/grep.ts), spawned at
[grep.ts:226](../../../tmp/pi/packages/coding-agent/src/core/tools/grep.ts). ripgrep evaluates the
override against **both files and directories**, so a negated glob naming a directory ignores that
directory and prunes the entire subtree.

With `node_modules/pkg/a.js` and `src/b.js` both containing NEEDLE,
`rg --json --hidden --glob '!node_modules' -- NEEDLE .` returns only `./src/b.js`.

## What cyrup-tools does

[grep.rs:398](../../../crates/cyrup-tools/src/tools/grep.rs) reaches the glob test only for
non-directory walk items (`Some(Ok(w)) if !w.is_dir`, glob applied at
[grep.rs:411-414](../../../crates/cyrup-tools/src/tools/grep.rs)); directories fall into the
catch-all arm `Some(Ok(_)) => {}` at
[grep.rs:427](../../../crates/cyrup-tools/src/tools/grep.rs) untested, so the subtree is descended
and every file in it is offered to the file filter.

The file filter then keeps them. `!node_modules` compiles to `**/node_modules` with
`literal_separator(true)`, which does **not** match `node_modules/pkg/a.js`, so
`keeps_file` computes `hit = false`, `negated = true`, `false != true` → **kept**.

Running the tool: `{"pattern":"NEEDLE","glob":"!node_modules"}` returns `src/b.js:1: NEEDLE` **and**
`node_modules/pkg/a.js:1: NEEDLE`.

## Root cause

[`RgGlob`](../../../crates/cyrup-tools/src/tools/globmatch.rs) (globmatch.rs:85-168) is a port of
`Override::matched(path, is_dir)` that **dropped the `is_dir` parameter**. Its only entry point is
`keeps_file(&self, rel_posix)` at globmatch.rs:162-167, which hard-codes the file case:

```rust
let hit = !self.only_dir && self.matcher.is_match(rel_posix);
hit != self.negated
```

`only_dir` is therefore used in one direction only — "a dir-only glob never matches" — never in the
other, "matches *because* the candidate is a directory". So `glob: "!src/"` is a no-op as well.

## Verified upstream semantics (`ignore` 0.4.26)

The workspace pins `ignore = { version = "0.4.26" }`
([Cargo.toml:149](../../../Cargo.toml)), confirmed in `Cargo.lock`. **The doc comments in
globmatch.rs and grep.rs cite `ignore-0.4.33` line numbers, which do not hold for the pinned
version.** Every citation below was read out of the 0.4.26 source and is the corrected form:

| Item | Cited in repo (0.4.33) | Correct for pinned 0.4.26 |
| --- | --- | --- |
| `GitignoreBuilder::add_line` | gitignore.rs:460-539 | **gitignore.rs:447-528** |
| comment / empty line | gitignore.rs:466-471 | **gitignore.rs:454-462** |
| `!` and leading `/` | gitignore.rs:483-499 | **gitignore.rs:470-487** |
| trailing `/` → `is_only_dir` | gitignore.rs:501-511 | **gitignore.rs:488-498** |
| `**/` prefix rule | gitignore.rs:513-522 | **gitignore.rs:500-508** |
| `/**` → `/**/*` fixup | gitignore.rs:524-527 | **gitignore.rs:509-514** |
| `GlobBuilder` settings | gitignore.rs:528-536 | **gitignore.rs:515-524** |
| `Gitignore::strip` | gitignore.rs:286-315 | **gitignore.rs:275-304** |
| `allow_unclosed_class(false)` | overrides.rs:126 | **overrides.rs:125** |
| `Override::matched` | overrides.rs:97-110 | overrides.rs:97-110 (unchanged) |

Two 0.4.26 sites carry the whole behaviour.

**`Gitignore::matched_stripped`, gitignore.rs:248-271** — the `is_dir` gate, at gitignore.rs:262:

```rust
for &i in matches.iter().rev() {
    let glob = &self.globs[i];
    if !glob.is_only_dir() || is_dir {
        return if glob.is_whitelist() { Match::Whitelist(glob) } else { Match::Ignore(glob) };
    }
}
Match::None
```

**`Override::matched`, overrides.rs:97-110** — the inversion and the directory carve-out:

```rust
let mat = self.0.matched(path, is_dir).invert();
if mat.is_none() && self.num_whitelists() > 0 && !is_dir {
    return Match::Ignore(Glob::unmatched());
}
```

`Override::num_whitelists()` (overrides.rs:75-77) returns the inner gitignore's `num_ignores()` —
i.e. the count of **plain, non-`!`** globs. Reduced to the single glob cyrup compiles, the full
truth table is:

| glob | candidate | hit? | verdict |
| --- | --- | --- | --- |
| `!x` (negated) | file or dir | yes | **Ignore** (dropped / pruned) |
| `!x` (negated) | file or dir | no | not ignored (kept / descended) |
| `x` (plain) | file or dir | yes | Whitelist (kept / descended) |
| `x` (plain) | **file** | no | **Ignore** (dropped) — the `num_whitelists() > 0` fallback |
| `x` (plain) | **directory** | no | `Match::None` — the `!is_dir` guard, so a whitelist miss **never prunes a directory** |

A trailing `/` (`is_only_dir`) simply removes the glob from consideration unless `is_dir` is true.
So `!src/` prunes the directory `src` and nothing else.

## How ripgrep turns a directory Ignore into a prune

`ignore`'s walker calls `Ignore::matched(path, is_dir)` (dir.rs:401-425) with overrides at highest
precedence (dir.rs:416-425), through `should_skip_entry` (walk.rs:1939-1950) and `skip_entry`
(walk.rs:1057-1091). In `Walk::next`, a skipped **directory** event does this
(walk.rs:1131-1145):

```rust
if should_skip {
    self.it.as_mut().unwrap().it.skip_current_dir();
    ...
    continue;
}
```

Two consequences that the fix must reproduce exactly:

1. `skip_current_dir()` is called on a **private** `walkdir::IntoIter` inside `ignore::Walk`.
   `ignore::Walk` exposes **no** skip handle to a consumer of its `Iterator`. A consumer cannot ask
   the walker to prune.
2. `skip_entry` returns `Ok(false)` unconditionally at `ent.depth() == 0` (walk.rs:1058-1060) — the
   **search root itself is never pruned**, whatever the glob says.

## Does pruning require the walker to expose directories? — Yes, and it already does

[`WalkItem`](../../../crates/cyrup-tools/src/ops/mod.rs) (ops/mod.rs:252-255) already carries
`is_dir`:

```rust
pub struct WalkItem {
    pub path: PathBuf,
    pub is_dir: bool,
}
```

and [ops/local/fs.rs:227-234](../../../crates/cyrup-tools/src/ops/local/fs.rs) emits **every**
entry the walker yields, directories included, with `is_dir` populated from `entry.file_type()`.
`ignore::Walk` is pre-order (walkdir default, `contents_first` is not set), so a directory is always
delivered **before** anything beneath it.

That is the whole seam. The consumer sees the directory first, can decide to prune it, and can drop
everything that arrives underneath. This is observably identical to `skip_current_dir()`: the same
paths are searched, the same paths are excluded, and the match cap is spent on the same files.

**No change to the walker, to `WalkOpts`, or to the `FsOps` trait is required or permitted here.**
Pushing the glob down into `WalkBuilder::overrides(...)` would force a non-`Copy` field onto
`WalkOpts`, drag the tool's cwd (the override root) across the `FsOps` boundary, and leak `ignore`
types through a backend-agnostic trait — for a result the consumer-side prune already produces.

## Sibling coordination — read before editing

Two other briefs touch neighbouring code. **State of play, by symbol:**

* [LOW-find-does-not-honor-fdignore-or-fd-s-global-ignore-file.md](./LOW-find-does-not-honor-fdignore-or-fd-s-global-ignore-file.md)
  (finished) adds `WalkFlavor { Plain, Fd, Rg }` to `WalkOpts` in
  [ops/mod.rs](../../../crates/cyrup-tools/src/ops/mod.rs) and a `flavor: WalkFlavor::Rg` line to the
  `WalkOpts` **struct literal** at [grep.rs:367-373](../../../crates/cyrup-tools/src/tools/grep.rs).
  **This task never touches that literal, `WalkOpts`, `WalkFlavor`, `ops/mod.rs`, or
  `ops/local/fs.rs`.** No overlap.
* [MEDIUM-find-glob-matching-is-always-case-sensitive-pi-fd-applies-smart-case-by.md](./MEDIUM-find-glob-matching-is-always-case-sensitive-pi-fd-applies-smart-case-by.md)
  (in progress) edits **`PatternMatcher::build`** and inserts a free function above `impl
  PatternMatcher`, i.e. globmatch.rs:20-45 only. **This task never touches `PatternMatcher`,
  `PatternMatcher::build`, `PatternMatcher::is_match`, or `to_posix`.** No overlap.

`RgGlob` and `PatternMatcher` are distinct structs in the same file. **Functions this task touches
and no others:** the `RgGlob` type-level doc block, `RgGlob::keeps_file` (replaced by
`RgGlob::ignores` + `RgGlob::keeps_file` + `RgGlob::prunes_dir`), and the walk loop body inside
`Grep::execute` in grep.rs. If the smart-case sibling lands first, every globmatch.rs line number
below shifts by the length of its inserted function — **re-anchor by symbol name, never by line
number**.

## Required path

One path. Three edits, in two files.

### 1. `crates/cyrup-tools/src/tools/globmatch.rs` — the `RgGlob` doc block

The trailing-`/` bullet states only half the rule, and the crate-version citations are wrong for the
pinned `ignore`.

**CURRENT** ([globmatch.rs:67-84](../../../crates/cyrup-tools/src/tools/globmatch.rs)):

```rust
/// ripgrep's rule for a single `--glob` argument.
///
/// Pi's `grep` passes `glob` to real ripgrep untouched (grep.ts:218), so the pattern is parsed as
/// one gitignore-style *override* line. This is a 1:1 port of that parse — `ignore-0.4.33`
/// `src/gitignore.rs:460-539` (`GitignoreBuilder::add_line`) plus the single-glob reduction of
/// `src/overrides.rs:97-110` (`Override::matched`):
///
/// * a leading `#` (or an empty/whitespace-only line) is a comment — no glob is added, so the
///   override set is empty and **every** file passes;
/// * a leading `!` inverts the match (rg's whitelist override): with one negated glob the file is
///   kept iff it does *not* match. `\!`/`\#` escape a literal leading `!`/`#`;
/// * a leading `/` is stripped and anchors the glob to the root;
/// * a trailing `/` means directories only, so no file ever matches;
/// * `**/` is prepended **only when the pattern contains no `/` at all** and is not already
///   `**`-prefixed — the exact opposite of fd's rule above;
/// * a trailing `/**` gains a `/*` so it matches the contents of a directory, not the directory;
/// * the result always compiles with `literal_separator(true)` and always matches the *full* path
///   relative to the override root — never the basename. (`**/foo` covers the basename case.)
```

**REPLACEMENT**:

```rust
/// ripgrep's rule for a single `--glob` argument.
///
/// Pi's `grep` passes `glob` to real ripgrep untouched (grep.ts:223), so the pattern is parsed as
/// one gitignore-style *override* line. This is a 1:1 port of that parse — `ignore-0.4.26`
/// `src/gitignore.rs:447-528` (`GitignoreBuilder::add_line`) plus the single-glob reduction of
/// `src/overrides.rs:97-110` (`Override::matched`):
///
/// * a leading `#` (or an empty/whitespace-only line) is a comment — no glob is added, so the
///   override set is empty and **every** file passes;
/// * a leading `!` inverts the match (rg's whitelist override): with one negated glob the file is
///   kept iff it does *not* match. `\!`/`\#` escape a literal leading `!`/`#`;
/// * a leading `/` is stripped and anchors the glob to the root;
/// * a trailing `/` restricts the glob to DIRECTORIES: no file ever matches it, and a directory
///   matches it exactly as it would without the slash (gitignore.rs:262);
/// * `**/` is prepended **only when the pattern contains no `/` at all** and is not already
///   `**`-prefixed — the exact opposite of fd's rule above;
/// * a trailing `/**` gains a `/*` so it matches the contents of a directory, not the directory;
/// * the result always compiles with `literal_separator(true)` and always matches the *full* path
///   relative to the override root — never the basename. (`**/foo` covers the basename case.)
///
/// The override applies to **directories as well as files**, which is what makes `!node_modules`
/// remove the whole subtree rather than nothing at all — see [`RgGlob::ignores`].
```

`RgGlob::build` is **unchanged**. Its internal `gitignore.rs:NNN` comments may be re-pointed to the
0.4.26 numbers in the table above; no code in `build` moves.

### 2. `crates/cyrup-tools/src/tools/globmatch.rs` — restore the `is_dir` parameter

**CURRENT** ([globmatch.rs:159-167](../../../crates/cyrup-tools/src/tools/globmatch.rs)):

```rust
    /// Whether a **file** survives this filter. `rel_posix` is the candidate path relative to the
    /// override root — for ripgrep that root is its own cwd, not the search path
    /// (gitignore.rs:286-315 `strip`), so `grep` must strip the tool's cwd, not its search root.
    pub fn keeps_file(&self, rel_posix: &str) -> bool {
        // overrides.rs:105-109: a whitelist hit inverts to Ignore (dropped); a miss with at least
        // one non-whitelist glob present inverts to Ignore as well (dropped).
        let hit = !self.only_dir && self.matcher.is_match(rel_posix);
        hit != self.negated
    }
```

**REPLACEMENT**:

```rust
    /// `Override::matched(path, is_dir)` (ignore-0.4.26 overrides.rs:97-110) reduced to this
    /// crate's single compiled glob, returning `true` when the override says **IGNORE**.
    ///
    /// `is_dir` is the parameter the original port dropped, and it is the whole difference between
    /// `!node_modules` filtering nothing and `!node_modules` removing the subtree.
    ///
    /// `rel_posix` is the candidate path relative to the override root — for ripgrep that root is
    /// its own cwd, not the search path (gitignore.rs:275-304 `strip`), so `grep` must strip the
    /// tool's cwd, not its search root.
    fn ignores(&self, rel_posix: &str, is_dir: bool) -> bool {
        // gitignore.rs:262 (`matched_stripped`, gitignore.rs:248-271): a glob with a trailing `/`
        // is passed over unless the candidate IS a directory.
        let hit = (is_dir || !self.only_dir) && self.matcher.is_match(rel_posix);
        if hit {
            // overrides.rs:105 `.invert()`: in an override set a `!` glob is stored as a gitignore
            // *whitelist*, so a hit on it inverts to Ignore; a plain glob's hit inverts to
            // Whitelist and is kept.
            return self.negated;
        }
        // overrides.rs:106-108: a miss falls through to Ignore only when at least one plain
        // (non-`!`) glob exists AND the candidate is not a directory. That `!is_dir` guard is why
        // a whitelist miss never prunes a directory — rg still descends into it looking for files
        // the glob does match.
        !self.negated && !is_dir
    }

    /// Whether a **file** survives this filter.
    pub fn keeps_file(&self, rel_posix: &str) -> bool {
        !self.ignores(rel_posix, false)
    }

    /// Whether this directory must be PRUNED — dropped together with everything beneath it, the
    /// way `ignore`'s walker drops it via `skip_current_dir()` (walk.rs:1131-1145) when
    /// `should_skip_entry` (walk.rs:1939-1950) sees an Ignore verdict for a directory.
    pub fn prunes_dir(&self, rel_posix: &str) -> bool {
        self.ignores(rel_posix, true)
    }
```

`keeps_file` keeps its exact signature and verdict for every input it already handled: the old
`hit = !only_dir && is_match` is the new `hit` at `is_dir == false`, and `hit != negated` is
`!(if hit { negated } else { !negated })`.

### 3. `crates/cyrup-tools/src/tools/grep.rs` — apply the override to directories

Two changes inside `Grep::execute`, both in the directory-walk branch.

**3a. CURRENT** ([grep.rs:367](../../../crates/cyrup-tools/src/tools/grep.rs)):

```rust
            let mut walk = self.fs.walk(
```

**REPLACEMENT** — declare the pruned-root list immediately above it:

```rust
            // Directory roots the override PRUNED. `ignore::Walk` prunes internally with
            // `skip_current_dir()` on a private iterator and exposes no skip handle to a consumer
            // (walk.rs:1131-1145), so the prune is reproduced here: the walk is pre-order, so a
            // directory always arrives before its contents, and every later item beneath a pruned
            // root is dropped. Same paths searched, same paths excluded, same match cap.
            let mut pruned: Vec<PathBuf> = Vec::new();
            let mut walk = self.fs.walk(
```

`PathBuf` is already imported at [grep.rs:14](../../../crates/cyrup-tools/src/tools/grep.rs); no
import changes.

**3b. CURRENT** ([grep.rs:397-430](../../../crates/cyrup-tools/src/tools/grep.rs)):

```rust
                        match item {
                            Some(Ok(w)) if !w.is_dir => {
                                let rel_path = w.path.strip_prefix(&search_root).unwrap_or(&w.path);
                                let rel = to_posix(rel_path);
                                // The glob is matched against the path relative to the OVERRIDE
                                // ROOT, which for ripgrep is its own cwd — Pi spawns `rg` with no
                                // `cwd` option and passes `searchPath` positionally, so a `path`
                                // argument narrows the walk but does NOT re-anchor the glob
                                // (ignore-0.4.33 gitignore.rs:286-315 `strip`). A candidate outside
                                // the root keeps its full path, as `strip` leaves it.
                                let glob_rel = w
                                    .path
                                    .strip_prefix(&self.cwd)
                                    .map_or_else(|_| to_posix(&w.path), to_posix);
                                if let Some(g) = &glob
                                    && !g.keeps_file(&glob_rel) {
                                        continue;
                                    }
                                self.search_one(
                                    &w.path,
                                    &rel,
                                    &matcher,
                                    context,
                                    limit,
                                    &mut count,
                                    &mut out,
                                    &mut any_line_truncated,
                                )
                                .await?;
                            }
                            Some(Ok(_)) => {}
                            Some(Err(e)) => return Err(e),
                            None => break,
                        }
```

**REPLACEMENT**:

```rust
                        match item {
                            Some(Ok(w)) => {
                                if let Some(g) = &glob {
                                    // Anything under a directory the override already pruned is
                                    // gone — files AND nested directories — before any further
                                    // test. This is `skip_current_dir()`'s effect, applied on the
                                    // consumer side (walk.rs:1131-1145).
                                    if pruned.iter().any(|p| w.path.starts_with(p)) {
                                        continue;
                                    }
                                    // The glob is matched against the path relative to the OVERRIDE
                                    // ROOT, which for ripgrep is its own cwd — Pi spawns `rg` with
                                    // no `cwd` option and passes `searchPath` positionally, so a
                                    // `path` argument narrows the walk but does NOT re-anchor the
                                    // glob (ignore-0.4.26 gitignore.rs:275-304 `strip`). A
                                    // candidate outside the root keeps its full path, as `strip`
                                    // leaves it.
                                    let glob_rel = w
                                        .path
                                        .strip_prefix(&self.cwd)
                                        .map_or_else(|_| to_posix(&w.path), to_posix);
                                    if w.is_dir {
                                        // ripgrep evaluates the override for directories too
                                        // (dir.rs:416-425), and an Ignore verdict takes the whole
                                        // subtree. A plain (non-`!`) glob that simply misses does
                                        // NOT prune — overrides.rs:106 guards that fallback with
                                        // `!is_dir` — so `prunes_dir` is the only test here.
                                        //
                                        // walk.rs:1057-1060: `skip_entry` returns false at depth 0,
                                        // so the search root itself is never prunable.
                                        if w.path != search_root && g.prunes_dir(&glob_rel) {
                                            pruned.push(w.path.clone());
                                        }
                                        continue;
                                    }
                                    if !g.keeps_file(&glob_rel) {
                                        continue;
                                    }
                                } else if w.is_dir {
                                    continue;
                                }
                                let rel_path = w.path.strip_prefix(&search_root).unwrap_or(&w.path);
                                let rel = to_posix(rel_path);
                                self.search_one(
                                    &w.path,
                                    &rel,
                                    &matcher,
                                    context,
                                    limit,
                                    &mut count,
                                    &mut out,
                                    &mut any_line_truncated,
                                )
                                .await?;
                            }
                            Some(Err(e)) => return Err(e),
                            None => break,
                        }
```

Notes on the shape, so it is not "simplified" back into a bug:

* The `Some(Ok(w)) if !w.is_dir` guard is **gone**. Directories must reach the arm body; the
  `else if w.is_dir { continue; }` branch preserves the old no-glob behaviour exactly (directories
  are skipped, nothing else changes) and keeps the no-glob path free of the `glob_rel` allocation.
* `continue` inside a `tokio::select!` arm already works here — the arm body is inside the `loop`,
  and the existing code at grep.rs:413 relies on it.
* `Path::starts_with` compares whole components, so a pruned `node_modules` does **not** swallow a
  sibling `node_modules2`.
* `w.path != search_root` is a valid depth-0 test: the walker is built with
  `WalkBuilder::new(&root)` from the same `search_root`
  ([ops/local/fs.rs:213](../../../crates/cyrup-tools/src/ops/local/fs.rs)), so the root entry's path
  is byte-identical to `search_root`.
* The `meta.is_file` branch at [grep.rs:341-360](../../../crates/cyrup-tools/src/tools/grep.rs) is
  **unchanged**: an explicitly-named file is depth 0 for ripgrep too and is never filtered by the
  override.

## Files changed

* [crates/cyrup-tools/src/tools/globmatch.rs](../../../crates/cyrup-tools/src/tools/globmatch.rs) —
  `RgGlob` doc block; `keeps_file` replaced by `ignores` (new, private, carries `is_dir`) +
  `keeps_file` (same signature) + `prunes_dir` (new, public). `PatternMatcher` and `to_posix`
  untouched.
* [crates/cyrup-tools/src/tools/grep.rs](../../../crates/cyrup-tools/src/tools/grep.rs) — one new
  local `pruned` in the directory-walk branch; the walk-loop `match` arm rewritten. The `WalkOpts`
  struct literal, the `meta.is_file` branch, `search_one`, and the notice/truncation tail are
  untouched.

**No other file changes.** [ops/mod.rs](../../../crates/cyrup-tools/src/ops/mod.rs),
[ops/local/fs.rs](../../../crates/cyrup-tools/src/ops/local/fs.rs),
[find.rs](../../../crates/cyrup-tools/src/tools/find.rs), and
[lib.rs](../../../crates/cyrup-tools/src/lib.rs) need no edit: `WalkItem.is_dir` already crosses the
seam, and `find` uses the unrelated `PatternMatcher`.

## Scope guard

This is parity, not a redesign. Do **not** add multi-glob override sets, case-insensitive overrides,
`--iglob`, or a `.overrides(...)` wiring into the walker. pi passes exactly one `--glob` string
through to `rg`, and one compiled glob is the whole contract.

## Definition of done

Given a tree containing `node_modules/pkg/a.js`, `src/b.js`, and `vendor/src/c.js`, each containing
`NEEDLE`, with none of them git-ignored:

1. `{"pattern":"NEEDLE","glob":"!node_modules"}` returns `src/b.js` and `vendor/src/c.js`, and does
   **not** return `node_modules/pkg/a.js`.
2. `{"pattern":"NEEDLE","glob":"!src/"}` — trailing slash, directory-only — does not return
   `src/b.js`, and does not return `vendor/src/c.js` (the `src` directory beneath `vendor` is pruned
   too, since the glob is unanchored).
3. `{"pattern":"NEEDLE","glob":"!node_modules"}` searching a tree whose excluded subtree holds more
   than the 100-match cap still returns the matches from outside that subtree, instead of spending
   the cap inside it.
4. `{"pattern":"NEEDLE","glob":"*.js"}` still returns all three files: a plain glob that misses a
   directory does not prune it, so the walk still descends and finds the matching files inside.
5. `{"pattern":"NEEDLE","glob":"src/**/*.js"}` returns `src/b.js` only, and not `vendor/src/c.js` —
   anchored-path behaviour is unchanged.
6. `{"pattern":"NEEDLE"}` with no `glob` returns all three files — the no-glob walk is byte-for-byte
   what it was.
7. Searching with `path` set to a directory whose own name matches the negated glob still searches
   that directory: the search root is never pruned.
8. `{"pattern":"NEEDLE","path":"node_modules/pkg/a.js","glob":"!node_modules"}` — an explicitly
   named file — still returns the match.
