---
title: Pi's third compact read header kind docs is not implemented
priority: LOW
tool: read
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: exec
status: in-progress
updated: 2026-08-27 18:00
---

# Pi's third compact read header kind docs is not implemented

QA rating **7/10**. The feature itself landed correctly and faithfully — `asset_dir()`, the
`CompactReadKind` enum, `docs_classification`, the `resolve_to_cwd` fix and the precedence order all
match the spec, and `env.rs` was correctly left alone. One regression was introduced in the adjacent
`resource` arm that this change rewrote. Only the two items below remain, and both are required.

## 1. `to_posix_label` doubles the leading separator on an absolute path (BLOCKER)

### The defect

[crates/cyrup-tui/src/transcript/tool_args.rs](../../../crates/cyrup-tui/src/transcript/tool_args.rs)
`to_posix_label` (**`tool_args.rs:262-268`** — anchor by symbol, concurrent work shifts lines):

```rust
/// `toPosixPath` (`read.ts:100-102`) — `filePath.split(sep).join("/")`. A no-op on unix; on Windows
/// it is what keeps the label reading `docs/providers.md` rather than `docs\providers.md`.
fn to_posix_label(path: &std::path::Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}
```

The doc comment is right about the contract and the body does not implement it. `Component::as_os_str`
is, verbatim from `std`:

```rust
Component::Prefix(p)  => p.as_os_str(),
Component::RootDir    => OsStr::new(MAIN_SEP_STR),   // "/" on unix, "\\" on Windows
Component::CurDir     => OsStr::new("."),
Component::ParentDir  => OsStr::new(".."),
Component::Normal(s)  => s,
```

so an absolute path's `RootDir` contributes a separator of its own, and the `join("/")` then adds a
second one. Verified by execution (`rustc` on the two bodies side by side):

| input | current `to_posix_label` | upstream `.split(sep).join("/")` |
| --- | --- | --- |
| `/etc/cyrup/AGENTS.md` | `//etc/cyrup/AGENTS.md` | `/etc/cyrup/AGENTS.md` |
| `/home/u/.cyrup/AGENTS.md` | `//home/u/.cyrup/AGENTS.md` | `/home/u/.cyrup/AGENTS.md` |
| `docs/guide/x.md` | `docs/guide/x.md` | `docs/guide/x.md` |
| `/` | `/` | `/` |

The relative call sites — `docs_classification`'s `let label = to_posix_label(relative);`
(`tool_args.rs:251`) and the resource arm's `strip_prefix` **success** path — are unaffected, which
is why every existing test passes. The broken one is the resource arm's **fallback**
(`compact_read_classification`, `tool_args.rs:324-333`):

```rust
let label = absolute
    .strip_prefix(&base)
    .map(to_posix_label)
    .unwrap_or_else(|_| to_posix_label(&absolute));   // <- absolute path, doubled slash
```

Before this change that arm read `absolute.to_string_lossy().into_owned()` and rendered a single
slash, so this is a **regression against DoD clause 9** ("the `skill` and `resource` headers render
byte-identically to before this change for every path that does not lie inside `<root>`").

The switch to `resolve_to_cwd` (correct, and required — do not revert it) makes this branch *more*
reachable than it was: `~/…/AGENTS.md`, `file://…/CLAUDE.md` and `@/abs/AGENTS.md` now resolve
outside the session cwd and land in the fallback, where `base.join(&raw_path)` used to keep them
cwd-relative. A collapsed read of `~/.cyrup/AGENTS.md` renders `read resource
//home/<user>/.cyrup/AGENTS.md` on screen today.

### What upstream actually does

[tmp/pi/packages/coding-agent/src/core/tools/read.ts](../../../tmp/pi/packages/coding-agent/src/core/tools/read.ts)
`toPosixPath` (**`read.ts:100-102`**):

```ts
function toPosixPath(filePath: string): string {
	return filePath.split(sep).join("/");
}
```

and [tmp/pi/packages/coding-agent/src/utils/paths.ts](../../../tmp/pi/packages/coding-agent/src/utils/paths.ts)
`formatPathRelativeToCwdOrAbsolute` (**`paths.ts:119-122`**):

```ts
export function formatPathRelativeToCwdOrAbsolute(filePath: string, cwd: string): string {
	const absolutePath = resolvePath(filePath, cwd);
	return (getCwdRelativePath(absolutePath, cwd) ?? absolutePath).split(sep).join("/");
}
```

Both are the **same string transform**: `split(sep).join("/")` is replace-every-`sep`-with-`/`. It
is not a component walk, it does no normalization, and it has no notion of a root. `/etc/x`
splits to `["", "etc", "x"]` and the leading empty segment rejoins to exactly one `/`.

### Windows and UNC — why the naive fixes are wrong

Do **not** special-case `Component::RootDir`, and do **not** "strip a leading empty segment". The
component walk is wrong on Windows in a second, independent way, and a `RootDir` patch would leave
it wrong:

| path | components | current `join("/")` | upstream `split("\\").join("/")` |
| --- | --- | --- | --- |
| `C:\a\b` | `Prefix("C:")`, `RootDir("\")`, `Normal("a")`, `Normal("b")` | `C:/\/a/b` | `C:/a/b` |
| `\\server\share\a` | `Prefix("\\server\share")`, `RootDir("\")`, `Normal("a")` | `\\server\share/\/a` | `//server/share/a` |
| `docs\guide\x.md` | three `Normal`s | `docs/guide/x.md` | `docs/guide/x.md` |

Note the UNC row: upstream **keeps** the doubled leading slash there, because both leading segments
are empty. That is correct and desirable — `//server/share/a` is the posix spelling of a UNC path —
and it is the reason "collapse `//` to `/`" is also the wrong fix. Only the plain string replace
reproduces every row.

### Required implementation (single required path)

Replace the body — keep the `&Path` signature so both call sites (`.map(to_posix_label)` and
`to_posix_label(&absolute)`) are untouched — with the plain separator replace:

```rust
/// `toPosixPath` (`read.ts:100-102`) — `filePath.split(sep).join("/")`, which is a plain
/// replace-every-`sep`-with-`/` over the STRING form. A no-op on unix; on Windows it is what keeps
/// the label reading `docs/providers.md` rather than `docs\providers.md`.
///
/// Deliberately NOT a `components()` walk joined on `"/"`: `Component::RootDir::as_os_str()` is
/// already `MAIN_SEP_STR`, so the join emits it a second time and every absolute label comes out
/// `//etc/…`; on Windows the `Prefix` + `RootDir` pair comes out `C:/\/a/b`. Upstream never
/// decomposes the path, and neither must this — a UNC path's `//server/share/a` is upstream's
/// output too, so a `//`-collapsing patch would be wrong in the other direction.
fn to_posix_label(path: &std::path::Path) -> String {
    let s = path.to_string_lossy();
    if std::path::MAIN_SEPARATOR == '/' {
        s.into_owned()
    } else {
        s.replace(std::path::MAIN_SEPARATOR, "/")
    }
}
```

This is the formulation already used in-tree by `to_posix`
([crates/cyrup-tools/src/tools/globmatch.rs:223-231](../../../crates/cyrup-tools/src/tools/globmatch.rs)) —
reuse the shape, not the symbol: `mod globmatch;` is private in
`crates/cyrup-tools/src/tools/mod.rs:12`, so it is not importable from `cyrup-tui`, and the local
copy is what carries the `read.ts:100-102` citation. Three other local `to_posix` copies already
exist across the workspace, so a local helper is the house pattern here.

Constraints on the change:

* Relative-path output must stay **byte-identical** (`docs/guide/x.md`, `README.md`,
  `docs/AGENTS.md`). It does: every input reaching this function comes from `resolve_to_cwd`, which
  lexically normalizes, or from `strip_prefix` on such a path, so there are no `.` segments,
  no `..` segments and no trailing separator for the component walk to have been silently eating.
* **Out of scope, do not touch:** the identically-shaped `to_posix` at
  [crates/cyrup-resources/src/discovery/scan.rs:156-161](../../../crates/cyrup-resources/src/discovery/scan.rs)
  and `crates/cyrup-resources/src/package/manifest.rs:565`. They are only ever fed `strip_prefix`
  output (relative paths), so they are correct in context; changing them is unrelated churn.
* **Out of scope, do not "fix":** upstream's `getCwdRelativePath` returns `relativePath || "."`
  (`paths.ts:112-116`), i.e. `"."` when the path IS the cwd, where the Rust `strip_prefix` arm
  yields `""`. That divergence is unreachable here (it needs a cwd whose own basename is
  `AGENTS.md`), predates this task, and is not part of DoD clause 9.

## 2. None of the ten documented resolutions is asserted anywhere (REQUIRED)

`rg 'CompactReadKind|docs_classification|read docs '` over `crates/cyrup-tui` finds no test. The
only compact-read coverage is `x7_agents_md_is_a_compact_resource_read`
([crates/cyrup-tui/src/transcript/tests/x_group.rs:199-215](../../../crates/cyrup-tui/src/transcript/tests/x_group.rs))
and `the_session_cwd_reaches_the_compact_read_classification`
([crates/cyrup-tui/src/tests/transcript_expand_wiring.rs:52-76](../../../crates/cyrup-tui/src/tests/transcript_expand_wiring.rs)),
both of which use a synthetic cwd of `/w/project` and pass a **relative** path, so they exercise the
`strip_prefix` SUCCESS path only. That is precisely why item 1 shipped: **the docs arm has zero
coverage and the resource fallback has zero coverage.** These tests are the QA-established
deliverable for this rework, not incidental scope — do not drop them.

### Where

All new tests go in
[crates/cyrup-tui/src/transcript/tests/x_group.rs](../../../crates/cyrup-tui/src/transcript/tests/x_group.rs),
immediately after `x7_agents_md_is_a_compact_resource_read` and before the `// --- X8 ---` banner.
The file already provides everything needed: `run_lines(name, args, result, expanded, opts)`,
`row(&lines, needle)`, `txt(line)` and `joined(&lines)` (`x_group.rs:17-55`), and
`#![allow(clippy::unwrap_used, clippy::expect_used, …)]` at `x_group.rs:7-12`.

### How the docs arm is reachable from a test

QA's route, confirmed: `asset_dir()`
([crates/cyrup-config/src/paths.rs:179-201](../../../crates/cyrup-config/src/paths.rs)) resolves via
**tier 3** in a test binary — the exe lives at `<root>/target/debug/deps/cyrup_tui-<hash>`, that
directory holds no `README.md` (tier 2 misses), and the ancestor walk finds the first `Cargo.toml`
at the workspace root. So a test takes `cyrup_config::asset_dir().expect(…)` and builds the read
path from it; it is correct under all three tiers because the expectation is derived from the same
value the renderer reads. `cyrup-config` is already a dependency (`crates/cyrup-tui/Cargo.toml:73`),
as is `cyrup-tools` (`:122`). Classification is purely lexical — `docs_classification` does no
filesystem access — so the paths need not exist.

### Required tests

```rust
/// **X7b — a read under the SHIPPED asset root is a `docs` read, not a resource read.**
///
/// `getPiDocsClassification` (`read.ts:104-121`) + its position AHEAD of
/// `COMPACT_RESOURCE_FILE_NAMES` in `getCompactReadClassification` (`read.ts:136-141`).
#[test]
fn x7b_reads_under_the_asset_root_classify_as_docs() {
    // Tier 3: in a test binary this is the workspace root.
    let root = cyrup_config::asset_dir().expect("asset_dir resolves in a test binary");
    // A cwd deliberately OUTSIDE the asset root, so nothing here can pass by cwd-relative accident.
    let opts = ImageOpts { cwd: Some(std::path::Path::new("/w/project")), ..ImageOpts::default() };
    let read = |p: std::path::PathBuf| {
        run_lines("read", json!({ "path": p.to_string_lossy() }), None, false, opts)
    };

    // `label === "README.md"` (`:117`).
    let lines = read(root.join("README.md"));
    assert_eq!(txt(row(&lines, "read docs")).trim_end(), " read docs README.md (ctrl+o to expand)");

    // `label.startsWith("docs/")` — a nested path keeps its posix-joined relative label.
    let lines = read(root.join("docs/guide/x.md"));
    assert_eq!(
        txt(row(&lines, "read docs")).trim_end(),
        " read docs docs/guide/x.md (ctrl+o to expand)"
    );

    // PRECEDENCE: `docs/AGENTS.md` inside the shipped tree is a DOCS read, not a resource read.
    // This is the ordering the `CompactReadKind` enum was introduced to protect.
    let lines = read(root.join("docs/AGENTS.md"));
    assert_eq!(
        txt(row(&lines, "read docs")).trim_end(),
        " read docs docs/AGENTS.md (ctrl+o to expand)"
    );
    assert!(!joined(&lines).contains("read resource"), "{}", joined(&lines));

    // `resolveToCwd` normalizes lexically, so `docs/../docs/x.md` is the same read as `docs/x.md`.
    let lines = read(root.join("docs/../docs/x.md"));
    assert_eq!(txt(row(&lines, "read docs")).trim_end(), " read docs docs/x.md (ctrl+o to expand)");
}

/// **X7c — the `docs/` guard requires the separator, and a non-docs sibling is not a docs read.**
///
/// `startsWith("docs/")` (`:117`), NOT `startsWith("docs")`: a read of the `docs` DIRECTORY itself
/// is an ordinary read upstream, and so is any other file at the asset root.
#[test]
fn x7c_the_docs_guard_needs_the_separator() {
    let root = cyrup_config::asset_dir().expect("asset_dir resolves in a test binary");
    let opts = ImageOpts { cwd: Some(std::path::Path::new("/w/project")), ..ImageOpts::default() };
    for path in [root.join("docs"), root.join("CHANGELOG.md")] {
        let lines = run_lines("read", json!({ "path": path.to_string_lossy() }), None, false, opts);
        let out = joined(&lines);
        assert!(!out.contains("read docs "), "generic header expected:\n{out}");
        assert!(!out.contains("read resource"), "generic header expected:\n{out}");
        // A generic (non-compact) read carries no expand hint — `x_group.rs` MIRROR 2.
        assert!(!out.contains("to expand"), "generic header expected:\n{out}");
    }
}

/// **X7d — a `resource` read that resolves OUTSIDE the cwd renders ONE leading slash.**
///
/// `formatPathRelativeToCwdOrAbsolute` (`utils/paths.ts:119-122`) falls back to the absolute path
/// and folds it with `.split(sep).join("/")`, where the leading empty segment rejoins to exactly
/// one `/`. The `resolveToCwd` port makes this the arm that `~`, `file://` and `@/abs` land in.
#[test]
fn x7d_a_resource_read_outside_the_cwd_keeps_one_leading_slash() {
    let cwd = std::path::Path::new("/w/project");
    let opts = ImageOpts { cwd: Some(cwd), ..ImageOpts::default() };
    let header = |raw: &str| {
        let lines = run_lines("read", json!({ "path": raw }), None, false, opts);
        txt(row(&lines, "read resource")).trim_end().to_string()
    };

    // A plain absolute path outside the cwd.
    assert_eq!(header("/etc/cyrup/AGENTS.md"), " read resource /etc/cyrup/AGENTS.md (ctrl+o to expand)");

    // A `file://` URL — resolved by `resolve_to_cwd`, so it too takes the fallback.
    assert_eq!(
        header("file:///etc/cyrup/CLAUDE.md"),
        " read resource /etc/cyrup/CLAUDE.md (ctrl+o to expand)"
    );

    // A `~`-expanded path. The home dir is environment-dependent, so derive the expectation from
    // the same resolver the renderer uses and assert the LABEL SHAPE explicitly.
    let expected = cyrup_tools::path::resolve_to_cwd("~/.cyrup/AGENTS.md", cwd);
    let expected = expected.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
    assert_eq!(header("~/.cyrup/AGENTS.md"), format!(" read resource {expected} (ctrl+o to expand)"));
    assert!(!expected.contains("//"), "sanity: the fixture itself must not be doubled");
    assert!(
        !header("~/.cyrup/AGENTS.md").contains("//"),
        "the doubled-separator regression: {}",
        header("~/.cyrup/AGENTS.md")
    );

    // REGRESSION GUARD (item 1) in its most direct form.
    assert!(!header("/etc/cyrup/AGENTS.md").contains("//"));
}
```

`ctrl+o` is the expand key `run_lines` produces via `ImageOpts::default()`, matching the existing
`x7` assertion at `x_group.rs:210`; if a sibling change moves it, mirror `x7`'s spelling rather than
hard-coding a different key.

## Verified and NOT outstanding — do not redo

* [crates/cyrup-config/src/paths.rs:155-201](../../../crates/cyrup-config/src/paths.rs) —
  `asset_dir()` / `resolve_asset_dir()` match the spec verbatim: `OnceLock` memoization,
  `$CYRUP_ASSET_DIR` → exe dir holding `README.md` → nearest ancestor `Cargo.toml`, `None` when
  nothing is discoverable.
* `crates/cyrup-config/src/lib.rs:67-70` — `asset_dir` re-exported.
* `crates/cyrup-config/src/env.rs:99` — untouched; `CYRUP_PACKAGE_DIR`/`PI_PACKAGE_DIR` still bind to
  `ConfigDirs::package_dir` (the install store) and nothing else. The trap was avoided.
* `CompactReadKind` enum + `as_str()` (`tool_args.rs:207-227`) replace `kind: &'static str`;
  `compact_read_call` (`tool_args.rs:351`) is an exhaustive `match` with `Docs | Resource` sharing
  the non-skill branch.
* `docs_classification` (`tool_args.rs:245-258`) — `strip_prefix`, empty-relative rejection, and the
  `README.md` / `docs/` / `examples/` guard with the separator required.
* `compact_read_classification` (`tool_args.rs:286`) uses
  `cyrup_tools::path::resolve_to_cwd(&raw_path, &base)` (`path.rs:330-353`); the docs arm sits
  between `SKILL.md` and `COMPACT_RESOURCE_FILE_NAMES`. **Keep it** — item 1 is not a reason to
  revert it.
* Stale citations corrected (`read.ts:123-144`, `:104-121`, `read.ts:43`, `read.ts:146-168`) and the
  "the docs arm cannot be ported" paragraph deleted.
* `tool_builtin.rs`, `tool_render.rs`'s `ImageOpts`, and `crates/cyrup-tools/src/` carry no change
  from this task.
