---
title: Find does not honor .fdignore or fd's global ignore file
priority: LOW
tool: find
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: exec
status: in-progress
updated: 2026-08-27
---

# Find does not honor .fdignore or fd's global ignore file

## Core objective

`find` must reproduce **fd's complete default ignore set**, not a subset of it. cyrup's `find` is an
in-process re-implementation over `ignore::WalkBuilder`
([find.rs:1-2](../../../crates/cyrup-tools/src/tools/find.rs)) rather than a spawn of the real `fd`
binary, so every ignore source fd enables by default has to be enabled explicitly on the builder.
Two of them are not:

1. **`.fdignore` files** in the tree (and in ancestors of the search root).
2. **fd's global ignore file** at the platform config dir + `fd/ignore`.

Both are opt-in on `WalkBuilder`. Neither is turned on today, so both are silently inert.

The complication that makes this more than a one-line change: the same `LocalFs::walk` seam serves
`grep`, whose upstream is **ripgrep**, which reads `.rgignore` and has **no** global ignore file.
The sibling task
[.rgignore files are honored by pi but ignored by cyrup](./LOW-rgignore-files-are-honored-by-pi-but-ignored-by-cyrup.md)
states the constraint from the other side: `.rgignore` "must not be added unconditionally for the
find walker". So the real objective here is to (a) give the shared walk seam a way to name **which
upstream binary it is emulating**, and (b) implement the fd arm of it. The rg arm is the sibling's
one-line follow-up and this brief leaves a wired, empty slot for it.

## Upstream behaviour — verified

pi's `find` resolves the real `fd` binary (system `fd`/`fdfind`, else a download of the latest
`sharkdp/fd` release) at
[tools-manager.ts:27-47, :85](../../../tmp/pi/packages/coding-agent/src/utils/tools-manager.ts) and
invokes it at [find.ts:225-269](../../../tmp/pi/packages/coding-agent/src/core/tools/find.ts):

```ts
const args: string[] = ["--glob", "--color=never", "--hidden"];
if (!insideGitRepo) args.push("--no-require-git");
args.push("--max-results", String(effectiveLimit));
// ... --full-path when the pattern contains "/"
args.push("--", effectivePattern, searchPath);
```

That argv contains **no** `--no-ignore`, `--no-ignore-vcs`, `--no-global-ignore-file`,
`--no-ignore-parent`, and no `--rg-alias-hidden-ignore`. fd therefore computes
`read_fdignore: true`, `read_vcsignore: true`, `read_parent_ignore: true`, `read_global_ignore:
true` ([fd 10.5.0 `src/main.rs:310-321`](../../../tmp/ref/fd/main.rs), upstream
`sharkdp/fd/src/main.rs`) and builds its walker as
([fd 10.5.0 `src/walk.rs:347-386`](../../../tmp/ref/fd/walk.rs)):

```rust
let mut builder = WalkBuilder::new(first_path);
builder
    .hidden(config.ignore_hidden)
    .ignore(config.read_fdignore)
    .parents(config.read_parent_ignore && (config.read_fdignore || config.read_vcsignore))
    .git_ignore(config.read_vcsignore)
    .git_global(config.read_vcsignore)
    .git_exclude(config.read_vcsignore)
    .require_git(config.require_git_to_read_vcsignore)
    /* overrides / follow_links / same_file_system / max_depth */;

if config.read_fdignore {
    builder.add_custom_ignore_filename(".fdignore");
}

if config.read_global_ignore
    && let Ok(basedirs) = etcetera::choose_base_strategy()
{
    let global_ignore_file = basedirs.config_dir().join("fd").join("ignore");
    if global_ignore_file.is_file() {
        let result = builder.add_ignore(global_ignore_file);
        match result {
            Some(ignore::Error::Partial(_)) => (),
            Some(err) => print_error(format!("Malformed pattern in global ignore file. {err}.")),
            None => (),
        }
    }
}
```

Two facts to carry into the Rust:

* fd registers `.fdignore` with `add_custom_ignore_filename` — **not** a hard-coded name inside the
  crate. It is inert unless registered.
* fd's global ignore file is registered with `add_ignore`, and only when the path `is_file()`.

### Where fd's global ignore file actually lives

fd resolves it with `etcetera::choose_base_strategy().config_dir()`. `choose_base_strategy` maps to
the `Windows` strategy on Windows and to the **`Xdg`** strategy everywhere else — **including
macOS**, which is the non-obvious part
([etcetera `src/base_strategy.rs:53-63`](../../../tmp/ref/etcetera/base_strategy.rs)):

```rust
cfg_select! {
    target_os = "windows" => { create_strategies!(Windows, Windows); }
    any(target_os = "macos", target_os = "ios") => { create_strategies!(Apple, Xdg); }
    _ => { create_strategies!(Xdg, Xdg); }
}
```

(the macro's **second** argument is the one `choose_base_strategy` returns, `base_strategy.rs:47-49`).

* **Xdg** (`config_dir` = `env_var_or_default("XDG_CONFIG_HOME", ".config/")`,
  [etcetera `src/base_strategy/xdg.rs`](../../../tmp/ref/etcetera/xdg.rs)) — `$XDG_CONFIG_HOME` is
  used only when it is set **and absolute**; otherwise `$HOME/.config`. So the file is
  `$XDG_CONFIG_HOME/fd/ignore`, falling back to `~/.config/fd/ignore`, on Linux **and** macOS.
* **Windows** (`config_dir` delegates to `data_dir` = `dir_inner("APPDATA")`,
  [etcetera `src/base_strategy/windows.rs:123-127, :190-196`](../../../tmp/ref/etcetera/windows.rs))
  — `%APPDATA%` when set and non-empty, else a `SHGetKnownFolderPath` CRT lookup, else
  `{home}\AppData\Roaming`. So `%APPDATA%\fd\ignore`.

## Current Rust behaviour — verified

[`LocalFs::walk`, fs.rs:209-242](../../../crates/cyrup-tools/src/ops/local/fs.rs) is the **only**
`WalkBuilder` in the crate (`rg WalkBuilder crates/` hits `fs.rs:10`, `fs.rs:213` and a doc comment
at `find.rs:2`; `cyrup-core` contains no `ignore::` usage at all):

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

`add_custom_ignore_filename` and `add_ignore` are never called anywhere under `crates/` — the only
hits for `fdignore` are
[cyrup-resources `discovery/scan.rs:101, :172`](../../../crates/cyrup-resources/src/discovery/scan.rs),
a hand-rolled skill-discovery scanner that never touches `FsOps::walk` and is out of scope.

`.ignore` files **do** work already: the `ignore` crate defaults `WalkBuilder::ignore` to `true` and
`fs.rs` never disables it, matching fd's `.ignore(config.read_fdignore)` = `true`. `parents(true)`
matches fd's computed `parents` = `true`. The delta is exactly `.fdignore` + the global fd ignore
file, and nothing else.

[`WalkOpts`, ops/mod.rs:237-248](../../../crates/cyrup-tools/src/ops/mod.rs) carries only
`include_hidden` and `require_git`, so no caller can express "I am fd". The two construction sites
are [find.rs:148-154](../../../crates/cyrup-tools/src/tools/find.rs) and
[grep.rs:367-373](../../../crates/cyrup-tools/src/tools/grep.rs) — they are the **only** two in the
workspace, and `WalkOpts::default()` is never called anywhere.

## The mechanism: `WalkFlavor` on `WalkOpts`

The seam must be told which upstream binary it is emulating. A `Copy` field on `WalkOpts` is the
required shape — `WalkOpts` is `Copy` and crosses the `FsOps` trait boundary through two decorators
([`TraversalFs::walk`, traversal.rs:133-142](../../../crates/cyrup-tools/src/isolation/traversal.rs)
and [`ProtectedFs::walk`, protected.rs:150-156](../../../crates/cyrup-tools/src/isolation/protected.rs),
both of which forward `opts` verbatim and need **no edit**), so a `Vec<OsString>` of filenames is
ruled out. A three-variant enum, with a `Plain` default that means "no tool-specific ignore
sources", keeps the derived `Default` on `WalkOpts` honest: a defaulted walker must never silently
acquire fd semantics.

### 1. `crates/cyrup-tools/src/ops/mod.rs`

**Current** ([ops/mod.rs:237-248](../../../crates/cyrup-tools/src/ops/mod.rs)):

```rust
/// Options for a tree walk (grep/find). Hidden files are skipped by default (ripgrep/fd parity).
///
/// `require_git` mirrors fd/ripgrep's `--require-git` behavior: when `false` (fd's
/// `--no-require-git`), `.gitignore` files are honored even outside a git repository; when `true`
/// (fd/ripgrep default), git-ignore semantics only apply inside a repo, so parent `.gitignore`
/// rules stop at nested repo boundaries. Pi's `find` sets this per search path (find.ts:226-240,
/// issue #5960); `grep` keeps the historical unconditional `false`.
#[derive(Clone, Copy, Debug, Default)]
pub struct WalkOpts {
    pub include_hidden: bool,
    pub require_git: bool,
}
```

**Replacement** — add the enum immediately above `WalkOpts` and the field to it:

```rust
/// Which upstream binary's ignore-file set a walk reproduces.
///
/// `.fdignore` and `.rgignore` are BOTH opt-in `WalkBuilder::add_custom_ignore_filename`
/// registrations in the `ignore` crate, and each is read by exactly ONE of the two tools pi
/// shells out to: fd reads `.fdignore` and a global `<config>/fd/ignore`; ripgrep reads
/// `.rgignore` and has no global ignore file of its own. Because `find` and `grep` share the one
/// `FsOps::walk` seam, that seam cannot register either name unconditionally without giving one
/// tool an exclusion source its upstream does not have. Naming the caller is the whole job of
/// this enum.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WalkFlavor {
    /// No tool-specific ignore sources: `.ignore` plus the gitignore family only. This is the
    /// `Default` so that a defaulted `WalkOpts` can never silently confer fd or ripgrep
    /// semantics on a walker that did not ask for them.
    #[default]
    Plain,
    /// fd (`find`, find.ts:225-269). Registers `.fdignore` and fd's global ignore file.
    Fd,
    /// ripgrep (`grep`, grep.ts:177 `ensureTool("rg")`, argv at `:220-224`). Registers
    /// `.rgignore`; ripgrep has no global ignore file, so nothing else attaches here.
    Rg,
}

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

    /// Whether this flavor's upstream reads a GLOBAL ignore file. Only fd does
    /// (fd 10.5.0 `src/walk.rs:371-386`); ripgrep has no equivalent.
    pub fn reads_fd_global_ignore(self) -> bool {
        matches!(self, Self::Fd)
    }
}

/// Options for a tree walk (grep/find). Hidden files are skipped by default (ripgrep/fd parity).
///
/// `require_git` mirrors fd/ripgrep's `--require-git` behavior: when `false` (fd's
/// `--no-require-git`), `.gitignore` files are honored even outside a git repository; when `true`
/// (fd/ripgrep default), git-ignore semantics only apply inside a repo, so parent `.gitignore`
/// rules stop at nested repo boundaries. Pi's `find` sets this per search path (find.ts:226-240,
/// issue #5960); `grep` keeps the historical unconditional `false`.
///
/// `flavor` names the upstream binary being emulated so the shared walk seam can register the
/// tool-specific ignore sources — see [`WalkFlavor`].
#[derive(Clone, Copy, Debug, Default)]
pub struct WalkOpts {
    pub include_hidden: bool,
    pub require_git: bool,
    pub flavor: WalkFlavor,
}
```

### 2. `crates/cyrup-tools/src/path.rs` — resolve fd's global ignore file

`path.rs` already owns the crate's single home-directory resolution
([`home_dir`, path.rs:88-100](../../../crates/cyrup-tools/src/path.rs)), so the config-dir rule
belongs beside it and can call it directly without a visibility change (same module). `etcetera` is
**not** a workspace dependency ([Cargo.toml `[workspace.dependencies]`](../../../Cargo.toml)) and
must not become one for two `var_os` reads; reproduce its two rules here.

Append after `windows_home_from` (path.rs:136):

```rust
/// fd's global ignore file, or `None` when there is no resolvable config dir or the file does not
/// exist.
///
/// fd joins `fd/ignore` onto `etcetera::choose_base_strategy().config_dir()` and registers it only
/// when `is_file()` holds (fd 10.5.0 `src/walk.rs:371-375`). pi passes no
/// `--no-global-ignore-file` (find.ts:235-267), so `read_global_ignore` is true on every call.
pub(crate) fn fd_global_ignore_file() -> Option<PathBuf> {
    let file = fd_config_dir()?.join("fd").join("ignore");
    file.is_file().then_some(file)
}

/// `etcetera::choose_base_strategy().config_dir()`, reproduced.
///
/// `choose_base_strategy` selects the `Windows` strategy on Windows and the **`Xdg`** strategy on
/// every other target INCLUDING macOS (etcetera `src/base_strategy.rs:53-63`; the macro's second
/// argument is the base strategy) — so a macOS user's fd ignore file is `~/.config/fd/ignore`, not
/// `~/Library/Application Support/fd/ignore`.
///
/// * Xdg: `$XDG_CONFIG_HOME` when set AND ABSOLUTE, else `$HOME/.config`
///   (`base_strategy/xdg.rs`, `env_var_or_none` + `env_var_or_default`).
/// * Windows: `%APPDATA%` when set and non-empty, else `{home}\AppData\Roaming`
///   (`base_strategy/windows.rs:123-127, :190-196`).
///
/// **[CYRUP-DELTA — the Windows arm omits etcetera's `SHGetKnownFolderPath` fallback]** etcetera's
/// `dir_inner` falls back to a win32 known-folder lookup between the `%APPDATA%` read and the
/// home-relative default. A Windows session with `%APPDATA%` unset but a redirected roaming folder
/// would therefore have fd read a file cyrup does not. Stated rather than papered over, because
/// the direction of the divergence is "cyrup excludes fewer paths", which is invisible in output.
fn fd_config_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        if let Some(appdata) = std::env::var_os("APPDATA").filter(|s| !s.is_empty()) {
            return Some(PathBuf::from(appdata));
        }
        return home_dir().map(|h| h.join("AppData").join("Roaming"));
    }
    #[cfg(not(windows))]
    {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
            && xdg.is_absolute()
        {
            return Some(xdg);
        }
        home_dir().map(|h| h.join(".config"))
    }
}
```

### 3. `crates/cyrup-tools/src/ops/local/fs.rs` — the WalkBuilder change

The import at [fs.rs:8](../../../crates/cyrup-tools/src/ops/local/fs.rs) —
`use crate::ops::{Access, DirEntry, FsOps, Meta, WalkItem, WalkOpts};` — stays **as it is**: the
flavor is reached through `opts.flavor` and its two methods, so `WalkFlavor` is never named here.

**Current** ([fs.rs:213-226](../../../crates/cyrup-tools/src/ops/local/fs.rs)):

```rust
let walker = WalkBuilder::new(&root)
    .hidden(!opts.include_hidden)
    .git_ignore(true)
    .git_exclude(true)
    // Pi runs `rg`/`fd` which honor the user's global gitignore (`~/.gitignore`,
    // arch-03:404). Mirror that with `git_global(true)`.
    .git_global(true)
    // `require_git(false)` (fd's `--no-require-git`) honors `.gitignore` even outside a
    // repo; `require_git(true)` is fd/ripgrep's default nested-repo-boundary behavior.
    // The caller sets this per search path (find.ts:226-240): `false` outside a repo,
    // `true` inside one. See `WalkOpts::require_git`.
    .require_git(opts.require_git)
    .parents(true)
    .build();
```

**Replacement** (the `for result in walker { … }` loop below it is untouched):

```rust
// fd builds its walker in two phases — the chained knobs, then the tool-specific ignore
// SOURCES (fd 10.5.0 `src/walk.rs:352-386`) — and so must this. `add_ignore` returns
// `Option<ignore::Error>`, not `&mut WalkBuilder` (ignore 0.4.26 `walk.rs:718`), so the
// chain cannot stay a single expression terminating in `.build()`.
let mut builder = WalkBuilder::new(&root);
builder
    .hidden(!opts.include_hidden)
    .git_ignore(true)
    .git_exclude(true)
    // Pi runs `rg`/`fd` which honor the user's global gitignore (`~/.gitignore`,
    // arch-03:404). Mirror that with `git_global(true)`.
    .git_global(true)
    // `require_git(false)` (fd's `--no-require-git`) honors `.gitignore` even outside a
    // repo; `require_git(true)` is fd/ripgrep's default nested-repo-boundary behavior.
    // The caller sets this per search path (find.ts:226-240): `false` outside a repo,
    // `true` inside one. See `WalkOpts::require_git`.
    .require_git(opts.require_git)
    // Also what makes the custom ignore filename below apply in ANCESTORS of the search
    // root: `Ignore::add_parents` runs `add_child_path` per parent, which compiles the
    // custom-ignore matcher too (ignore 0.4.26 `dir.rs:182-248`, `:286-292`). fd computes
    // this same `true` (`read_parent_ignore && (read_fdignore || read_vcsignore)`).
    .parents(true);

// `.fdignore` for fd / `.rgignore` for ripgrep. Both are inert until registered; neither
// tool reads the other's. Custom ignore files outrank `.ignore` and EVERY gitignore source
// (ignore 0.4.26 `dir.rs:580-585`: `m_custom_ignore.or(m_ignore).or(m_gi).or(m_gi_exclude)
// .or(m_global).or(m_explicit)`), so a `!keep.txt` negation in `.fdignore` re-includes a
// path a `.gitignore` excluded — same as fd.
if let Some(name) = opts.flavor.custom_ignore_filename() {
    builder.add_custom_ignore_filename(name);
}

// fd's GLOBAL ignore file (fd 10.5.0 `src/walk.rs:371-386`). Registered via `add_ignore`,
// which lands in `explicit_ignores` — the LOWEST precedence source, below the global
// gitignore (`dir.rs:585`), exactly as it is for fd. ripgrep has no global ignore file, so
// this is gated on the fd flavor alone.
if opts.flavor.reads_fd_global_ignore()
    && let Some(global) = crate::path::fd_global_ignore_file()
{
    // fd prints a warning for a malformed pattern and KEEPS WALKING, with the rules that
    // did parse still in force (`ignore::Error::Partial`). pi buffers fd's stderr but only
    // surfaces it when fd exits non-zero AND produced no output (find.ts:284-310), so that
    // warning is invisible upstream on a successful run. `walk` has no warning channel;
    // dropping the error reproduces both halves.
    drop(builder.add_ignore(&global));
}

let walker = builder.build();
```

### 4. `crates/cyrup-tools/src/tools/find.rs` — the find call site

**Current** ([find.rs:148-154](../../../crates/cyrup-tools/src/tools/find.rs)):

```rust
let mut walk = self.fs.walk(
    &search_root,
    WalkOpts {
        include_hidden: true,
        require_git: inside_git_repo,
    },
);
```

**Replacement** (import `WalkFlavor` alongside `WalkOpts` at
[find.rs:6](../../../crates/cyrup-tools/src/tools/find.rs):
`use crate::ops::{FsOps, WalkFlavor, WalkOpts};`):

```rust
let mut walk = self.fs.walk(
    &search_root,
    WalkOpts {
        include_hidden: true,
        require_git: inside_git_repo,
        // Pi's `find` IS fd (find.ts:225 `ensureTool("fd")`) invoked with no
        // `--no-ignore`/`--no-global-ignore-file` (find.ts:235-267), so fd's full default
        // ignore set is in force: `.fdignore` files plus `<config>/fd/ignore`.
        flavor: WalkFlavor::Fd,
    },
);
```

### 5. `crates/cyrup-tools/src/tools/grep.rs` — the grep call site

Grep must keep exactly its current ignore set, and must name itself so the sibling `.rgignore` task
becomes one arm in `WalkFlavor::custom_ignore_filename` and touches nothing else.

**Current** ([grep.rs:367-373](../../../crates/cyrup-tools/src/tools/grep.rs)):

```rust
let mut walk = self.fs.walk(
    &search_root,
    WalkOpts {
        include_hidden: true,
        require_git: true,
    },
);
```

**Replacement** (import `WalkFlavor` alongside `WalkOpts` at
[grep.rs:6](../../../crates/cyrup-tools/src/tools/grep.rs)):

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

### 6. Re-exports

* [lib.rs:44-49](../../../crates/cyrup-tools/src/lib.rs) — add `WalkFlavor` to the `pub use ops::{…}`
  list, alphabetically between `Transport` and `WalkItem`:
  `… Transport, WalkFlavor, WalkItem, WalkOpts, kill_pid, …`.
* [isolation/mod.rs:42](../../../crates/cyrup-tools/src/isolation/mod.rs) — the decorators are
  written against these seam types, so add it there too:
  `pub use crate::ops::{Access, DirEntry, FsOps, ImageMime, Meta, WalkFlavor, WalkItem, WalkOpts};`

No other file changes. `TraversalFs::walk` and `ProtectedFs::walk` forward `opts` by value and
compile unchanged; no `WalkOpts` struct literal exists anywhere else in the workspace, so the new
field breaks no other construction site.

## Resulting ignore precedence for `find`

Highest to lowest, matching fd exactly because both go through the same crate the same way
(ignore 0.4.26 `dir.rs:580-585`):

1. `.fdignore` (custom ignore, incl. ancestors via `parents(true)`) — **new**
2. `.ignore`
3. `.gitignore`
4. `.git/info/exclude`
5. global gitignore (`core.excludesFile`)
6. `<config>/fd/ignore` (explicit ignore) — **new**

## Non-goals

* No `--no-ignore` escape hatch, no new `FindOpts`
  ([config.rs:286-290](../../../crates/cyrup-tools/src/config.rs)) or `ToolsOptions` knob: fd's
  ignore set is unconditional on pi's argv, so a toggle would be capability pi does not have.
* No `.rgignore` registration — that is the sibling task, and adding it here would give grep an
  exclusion source before that task's own verification has run.
* No change to `hidden`, `git_ignore`, `git_exclude`, `git_global`, `require_git` or `parents`;
  all six already match fd.
* `cyrup-resources`' skill-discovery scanner
  ([scan.rs:172](../../../crates/cyrup-resources/src/discovery/scan.rs)) is a different subsystem
  and stays as-is.

## Genuinely uncertain

* **fd version drift.** pi downloads the *latest* `sharkdp/fd` release at run time
  ([tools-manager.ts:242-264](../../../tmp/pi/packages/coding-agent/src/utils/tools-manager.ts)),
  and can also pick up an arbitrarily old system `fd`/`fdfind` first (`:76`, `:85`). The
  prescription above is pinned to fd 10.5.0 / etcetera 0.11. Older fd releases resolved the config
  dir with `dirs-next`, whose macOS `config_dir()` is `~/Library/Application Support` — so on a
  macOS host with an old system fd, the global ignore file cyrup reads and the one fd reads are
  different paths. The 10.5.0 behaviour is the correct target; the divergence is unfixable without
  probing the installed binary, which cyrup has none of.
* **Windows `%APPDATA%`-unset hosts** — the `SHGetKnownFolderPath` fallback etcetera has and this
  prescription does not (documented inline as a CYRUP-DELTA above).
* **Symlinked global ignore file.** fd's gate is `is_file()`, which follows symlinks; the
  prescription uses the same call, so this matches — but neither side has been observed against a
  broken symlink, where `is_file()` is `false` on both and the file is simply skipped.

## Definition of done

Observable behaviour that must hold:

1. In a tree containing `.fdignore` with the line `build/` and a file `build/out.txt`, `find` with
   pattern `*.txt` rooted at that tree does not return `build/out.txt`. Without the `.fdignore`, it
   does.
2. A `.fdignore` placed in an **ancestor** of the search root is applied to the walk, matching
   fd's `--no-ignore-parent`-off default.
3. A `.fdignore` line `!keep.txt` re-includes a file that a sibling `.gitignore` excluded — custom
   ignore files outrank the gitignore family.
4. With `XDG_CONFIG_HOME` set to an absolute directory containing `fd/ignore` whose body is
   `vendor/`, `find` does not return paths under `vendor/`. With `XDG_CONFIG_HOME` unset, the same
   holds for `$HOME/.config/fd/ignore`. With `XDG_CONFIG_HOME` set to a *relative* path, the
   `$HOME/.config` location is the one consulted.
5. A pattern in `<config>/fd/ignore` is overridden by a `!`-negation in any `.gitignore`,
   `.ignore` or `.fdignore` in the tree — the global fd ignore file is the lowest-precedence
   source.
6. A missing, empty, or non-file `<config>/fd/ignore` changes nothing about the results. A
   malformed pattern inside it neither fails the walk nor surfaces an error to the caller; the
   patterns that parsed are still applied.
7. `grep` results over a tree containing a `.fdignore` and a `<config>/fd/ignore` are **identical**
   to what they are today — neither source reaches the grep walker.
8. Every path `find` returned before this change and that no `.fdignore` or fd global ignore rule
   excludes is still returned, in the same order, with the same `limit`/`max_bytes` truncation
   behaviour.
9. No new `find` parameter, `FindOpts` field or configuration key exists — the behaviour is
   unconditional, as it is on pi's fd argv.
