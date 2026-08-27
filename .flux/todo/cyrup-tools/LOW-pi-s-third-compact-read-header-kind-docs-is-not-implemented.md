---
title: Pi's third compact read header kind docs is not implemented
priority: LOW
tool: read
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: aug
status: done
updated: 2026-08-27
---

# Pi's third compact read header kind docs is not implemented

## Core objective

Pi's collapsed `read` header has **three** compact forms. cyrup renders two. Close the third:
a read whose resolved path lands on `README.md`, or anywhere under `docs/` or `examples/` **inside
the agent's own shipped tree**, must collapse to

```text
read docs docs/providers.md (ctrl+o to expand)
```

instead of the generic `read ~/…/docs/providers.md`.

That requires two things, in this order:

1. a **shipped-asset root accessor** in `cyrup-config` — the cyrup analogue of pi's
   `getPackageDir()` / `getReadmePath()`, which does not exist anywhere in `crates/` today; and
2. a **third arm** in the transcript renderer's `compact_read_classification`, placed between the
   `SKILL.md` arm and the resource arm, exactly where pi places it.

The prerequisite is the reason the gap has stood: without a package root there is nothing to
resolve the read path *against*. Ship the accessor and the arm follows in twenty lines.

## What pi does

[pi read.ts](../../../tmp/pi/packages/coding-agent/src/core/tools/read.ts):

* **`:38-41`** declares the union — this is the type the Rust must mirror:

  ```ts
  interface CompactReadClassification {
      kind: "docs" | "resource" | "skill";
      label: string;
  }
  ```

* **`:100-102`** `toPosixPath` — the label is always `/`-joined, whatever the host separator:

  ```ts
  function toPosixPath(filePath: string): string {
      return filePath.split(sep).join("/");
  }
  ```

* **`:104-121`** `getPiDocsClassification` — the whole feature:

  ```ts
  function getPiDocsClassification(absolutePath: string): CompactReadClassification | undefined {
      const packageRoot = dirname(getReadmePath());
      const relativePath = relative(resolvePath(packageRoot), resolvePath(absolutePath));
      if (relativePath === "" || relativePath === ".." || relativePath.startsWith(`..${sep}`) || isAbsolute(relativePath)) {
          return undefined;
      }
      const label = toPosixPath(relativePath);
      if (label === "README.md" || label.startsWith("docs/") || label.startsWith("examples/")) {
          return { kind: "docs", label };
      }
      return undefined;
  }
  ```

  Note the three rejections: the package root itself (`""`), an ancestor (`..`, `../…`) and a
  different volume (`isAbsolute`). Note also that a bare `docs` directory read does **not** match —
  the guard is `startsWith("docs/")`, so the separator is required.

* **`:123-144`** `getCompactReadClassification` orders the three arms. `SKILL.md` wins first,
  **docs is consulted second (`:136-137`), before the resource set (`:139-141`)**:

  ```ts
  const absolutePath = resolveToCwd(rawPath, cwd);
  const fileName = basename(absolutePath);
  if (fileName === "SKILL.md") { … }

  const docsClassification = getPiDocsClassification(absolutePath);
  if (docsClassification) return docsClassification;

  if (COMPACT_RESOURCE_FILE_NAMES.has(fileName)) { … }
  ```

* **`:146-168`** `formatCompactReadCall` renders it. `docs` and `resource` share the non-skill
  branch, which interpolates the kind word straight into the title (`:161-167`):

  ```ts
  return (
      theme.fg("toolTitle", theme.bold(`read ${classification.kind}`)) + " " +
      theme.fg("accent", classification.label) +
      formatReadLineRange(args, theme) + expandHint
  );
  ```

* **`:336-344`** `renderCall` — the compact header is **collapsed-only**
  (`!context.expanded ? getCompactReadClassification(…) : undefined`).

The package root comes from [pi config.ts](../../../tmp/pi/packages/coding-agent/src/config.ts):

* **`:385-397`** `getPackageDir()` — `PI_PACKAGE_DIR` override (normalized), else
  `dirname(process.execPath)` for the single-file Bun binary, else `findNodePackageDir(__dirname)`;
* **`:368-383`** `findNodePackageDir` — walk up to the nearest ancestor holding a `package.json`;
* **`:436-448`** `getReadmePath()` / `getDocsPath()` / `getExamplesPath()` —
  `resolve(join(getPackageDir(), "README.md" | "docs" | "examples"))`.

The shipped tree really does hold those three entries: `tmp/pi/packages/coding-agent/` contains
`README.md`, `docs/` and `examples/`.

## What cyrup does today

The renderer, not the tool crate, owns this. [tool_args.rs:152-191](../../../crates/cyrup-tui/src/transcript/tool_args.rs)
`compact_read_classification` implements the `skill` arm (`:170-180`) and the `resource` arm
(`:181-189`) and returns `None` otherwise. Its sole caller,
[tool_builtin.rs:15-27](../../../crates/cyrup-tui/src/transcript/tool_builtin.rs), then falls through
to the generic header:

```rust
let classification =
    if expanded { None } else { compact_read_classification(&run.args, opts.cwd) };
match classification {
    Some(c) => out.push(compact_read_call(&c, &run.args, opts.expand_key, theme)),
    None => {
        let mut spans = vec![Span::styled("read ", theme.tool_title_style())];
        spans.push(tool_path_span(&run.args, &["file_path", "path"], None, theme));
        …
    }
}
```

The omission is admitted in-place at [tool_args.rs:145-151](../../../crates/cyrup-tui/src/transcript/tool_args.rs):
*"The `docs` arm is the one piece that cannot be ported here … `getReadmePath` has no counterpart
anywhere in `crates/`."* That statement is still true:

* `crates/cyrup-config/src/paths.rs` carries `normalize_path`, `resolve_path_from_base` and
  `lexically_normalize` ([paths.rs:126-192](../../../crates/cyrup-config/src/paths.rs)) — no
  package/asset root resolution at all;
* the nearest relative, `DocsPointers` ([builder.rs:17-29](../../../crates/cyrup-session/src/prompt/builder.rs)),
  is the system-prompt progressive-disclosure struct, and
  [builder.rs:349-359](../../../crates/cyrup-session/src/prompt/builder.rs) records that its sole
  production caller still passes `DocsPointers::default()` (SESS-035) **because the path helpers do
  not exist**, naming `cyrup-config` as where they belong. This task builds exactly that helper;
* `package_dir` in `crates/` means something else entirely — see the trap below.

### The `CYRUP_PACKAGE_DIR` trap (verified, do not fall into it)

[env.rs:99](../../../crates/cyrup-config/src/env.rs) binds

```rust
package_dir: path(&["CYRUP_PACKAGE_DIR", "PI_PACKAGE_DIR"]),
```

and its doc comment claims this mirrors `getPackageDir()`. It does not. `ConfigDirs::package_dir`
defaults to `<agent_dir>/packages` and is fed to `PackageStore::new` as the **install store root**
for third-party packages ([discovery/mod.rs:157-165](../../../crates/cyrup-resources/src/discovery/mod.rs)),
whereas pi's `PI_PACKAGE_DIR` (its only use, [config.ts:387](../../../tmp/pi/packages/coding-agent/src/config.ts))
is the **shipped-asset** root for `README.md` / `docs/` / `themes/`. Binding one variable to both
roots would relocate a user's installed-package store the moment they pointed the asset root at a
Nix path. The new accessor therefore takes its own name, `CYRUP_ASSET_DIR`, and **must not** read
`CYRUP_PACKAGE_DIR` or `PI_PACKAGE_DIR`. Leave `env.rs` alone.

## Required implementation

### 1. `crates/cyrup-config/src/paths.rs` — the shipped-asset root

Append after `resolve_path_from_base_with_home`
([paths.rs:132-154](../../../crates/cyrup-config/src/paths.rs)); `lexically_normalize`
([paths.rs:164](../../../crates/cyrup-config/src/paths.rs)) is `pub(crate)` and already in scope.

```rust
/// The directory holding the assets shipped **with the agent itself** — `README.md`, `docs/`,
/// `examples/`. Pi `getPackageDir()` (`config.ts:385-397`), whose only consumers are the
/// shipped-asset paths at `config.ts:436-448`.
///
/// Resolution order, mirroring upstream's three tiers:
/// 1. `$CYRUP_ASSET_DIR`, run through [`normalize_path_buf`] — pi's `PI_PACKAGE_DIR` escape hatch
///    for Nix/Guix store paths (`config.ts:387-390`). It is deliberately NOT spelled
///    `CYRUP_PACKAGE_DIR`: that name is already bound to [`crate::ConfigDirs::package_dir`], the
///    installed-package STORE (`env.rs:99`), which is a different directory.
/// 2. The directory containing the running executable, when it directly holds a `README.md` —
///    the single-file-binary layout, pi's `dirname(process.execPath)` arm (`config.ts:392-394`).
/// 3. The nearest ancestor of that directory holding a `Cargo.toml` — the source-checkout arm,
///    pi's `findNodePackageDir` (`config.ts:368-383`) with `Cargo.toml` for `package.json`. A
///    `cargo run` binary lives at `<root>/target/<profile>/cyrup`, and `target/` carries no
///    manifest, so the walk lands on the workspace root without needing upstream's `dist/`
///    special case.
///
/// `None` means no asset root is discoverable; every caller must then behave as if the tree is
/// absent rather than substituting the cwd.
///
/// Resolved ONCE per process. The result is immutable for the process lifetime (unlike a session
/// cwd), and the render path calls it per paint, so the `existsSync`-equivalent walk must not run
/// per frame.
pub fn asset_dir() -> Option<&'static std::path::Path> {
    static ASSET_DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    ASSET_DIR.get_or_init(resolve_asset_dir).as_deref()
}

fn resolve_asset_dir() -> Option<PathBuf> {
    if let Some(raw) = std::env::var_os("CYRUP_ASSET_DIR")
        && !raw.is_empty()
    {
        return Some(lexically_normalize(&normalize_path_buf(&raw.to_string_lossy())));
    }
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    if exe_dir.join("README.md").is_file() {
        return Some(lexically_normalize(exe_dir));
    }
    let mut dir = exe_dir;
    loop {
        if dir.join("Cargo.toml").is_file() {
            return Some(lexically_normalize(dir));
        }
        dir = dir.parent()?;
    }
}
```

Re-export it from [lib.rs:67-69](../../../crates/cyrup-config/src/lib.rs):

```rust
pub use paths::{
    asset_dir, normalize_path, normalize_path_buf, normalize_path_with_home,
    normalize_windows_shell_path,
};
```

Do **not** add `readme_path()` / `docs_path()` / `examples_path()` here. Those are SESS-035's
wiring for `DocsPointers`; `asset_dir()` is `dirname(getReadmePath())` by construction and is the
only value this task consumes.

### 2. `crates/cyrup-tui/src/transcript/tool_args.rs` — make `kind` a real union

The TS side is a closed three-member union; the Rust models it as a bare `&'static str`, which is
why a third member could be forgotten in the first place. Replace it with an enum so the compiler
enforces the third arm.

**Current** ([tool_args.rs:124-129](../../../crates/cyrup-tui/src/transcript/tool_args.rs)):

```rust
/// One `CompactReadClassification` (`read.ts:37-40`) — `kind` is `"docs" | "resource" | "skill"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CompactRead {
    kind: &'static str,
    label: String,
}
```

**Replacement** (the citation is corrected to `read.ts:38-41`, which is where the interface
actually sits):

```rust
/// The `kind` union of `CompactReadClassification` (`read.ts:38-41`):
/// `kind: "docs" | "resource" | "skill"`. A closed enum rather than a `&'static str` so the
/// renderer cannot silently grow a fourth spelling, and so every `match` on it has to name all
/// three.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CompactReadKind {
    Docs,
    Resource,
    Skill,
}

impl CompactReadKind {
    /// The word interpolated into ``read ${classification.kind}`` (`read.ts:162`).
    fn as_str(self) -> &'static str {
        match self {
            CompactReadKind::Docs => "docs",
            CompactReadKind::Resource => "resource",
            CompactReadKind::Skill => "skill",
        }
    }
}

/// One `CompactReadClassification` (`read.ts:38-41`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CompactRead {
    kind: CompactReadKind,
    label: String,
}
```

### 3. `tool_args.rs` — the `docs` classifier

Add immediately above `compact_read_classification`:

```rust
/// Port of `getPiDocsClassification` (`read.ts:104-121`) — a read of the agent's OWN shipped
/// `README.md`, `docs/…` or `examples/…`.
///
/// `absolute` is already lexically resolved by the caller and [`cyrup_config::asset_dir`] is
/// normalized at construction, so `Path::strip_prefix` — which compares whole components — is the
/// entire `relative()` guard upstream spells out at `:107-112`: it fails for a sibling, for an
/// ancestor and for a different volume, and yields an EMPTY relative path for the root itself,
/// which `:107` rejects too.
fn docs_classification(absolute: &std::path::Path) -> Option<CompactRead> {
    let package_root = cyrup_config::asset_dir()?;
    let relative = absolute.strip_prefix(package_root).ok()?;
    if relative.as_os_str().is_empty() {
        return None;
    }
    let label = to_posix_label(relative);
    // `label === "README.md" || label.startsWith("docs/") || label.startsWith("examples/")`
    // (`:117`). The trailing separator is REQUIRED: a read of the `docs` directory itself is not a
    // docs read upstream, and must not become one here.
    if label == "README.md" || label.starts_with("docs/") || label.starts_with("examples/") {
        return Some(CompactRead { kind: CompactReadKind::Docs, label });
    }
    None
}

/// `toPosixPath` (`read.ts:100-102`) — `filePath.split(sep).join("/")`. A no-op on unix; on Windows
/// it is what keeps the label reading `docs/providers.md` rather than `docs\providers.md`.
fn to_posix_label(path: &std::path::Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}
```

### 4. `tool_args.rs` — wire the arm in, and resolve the path the way pi does

**Current** ([tool_args.rs:162-190](../../../crates/cyrup-tui/src/transcript/tool_args.rs)):

```rust
    // `resolveToCwd(rawPath, cwd)` — an absolute path is kept, a relative one is joined to the
    // session cwd. `Path::join` has exactly that semantic for an absolute right-hand side.
    let base = match cwd {
        Some(c) => c.to_path_buf(),
        None => std::env::current_dir().ok()?,
    };
    let absolute = base.join(&raw_path);
    let file_name = absolute.file_name()?.to_string_lossy().into_owned();
    if file_name == "SKILL.md" {
        …
        return Some(CompactRead { kind: "skill", label });
    }
    if COMPACT_RESOURCE_FILE_NAMES.contains(&file_name.as_str()) {
        let label = absolute
            .strip_prefix(&base)
            .map(|r| r.to_string_lossy().into_owned())
            .unwrap_or_else(|_| absolute.to_string_lossy().into_owned());
        return Some(CompactRead { kind: "resource", label });
    }
    None
```

**Replacement**:

```rust
    let base = match cwd {
        Some(c) => c.to_path_buf(),
        None => std::env::current_dir().ok()?,
    };
    // `resolveToCwd(rawPath, cwd)` (`read.ts:130`). `Path::join` is NOT that function: it keeps
    // `.`/`..` segments and expands neither `~` nor `@` nor `file://`, so a path pi resolves INTO
    // the asset root (`docs/../docs/x.md`, `~/pkg/README.md`) would miss `strip_prefix` below.
    // `cyrup_tools::path::resolve_to_cwd` IS the port of `resolveToCwd` (`path.rs:248-271`), and
    // `crate::app::event_extract` already reaches for it on the same argument.
    let absolute = cyrup_tools::path::resolve_to_cwd(&raw_path, &base);
    let file_name = absolute.file_name()?.to_string_lossy().into_owned();
    if file_name == "SKILL.md" {
        // `basename(dirname(absolutePath)) || fileName` — the containing directory names the skill,
        // and a `SKILL.md` at the filesystem root falls back to the file name itself.
        let label = absolute
            .parent()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or(file_name);
        return Some(CompactRead { kind: CompactReadKind::Skill, label });
    }
    // `const docsClassification = getPiDocsClassification(absolutePath);` (`:136-137`) — SECOND,
    // ahead of the resource set. A `docs/AGENTS.md` inside the shipped tree is a `docs` read
    // upstream, not a `resource` read, and the order is what decides that.
    if let Some(docs) = docs_classification(&absolute) {
        return Some(docs);
    }
    if COMPACT_RESOURCE_FILE_NAMES.contains(&file_name.as_str()) {
        // `formatPathRelativeToCwdOrAbsolute(absolutePath, cwd)` (`utils/paths.ts:119-122`): the
        // cwd-relative form when the file is under it, else the absolute path — and `.split(sep)
        // .join("/")` on the result, which is the same posix fold the docs label takes.
        let label = absolute
            .strip_prefix(&base)
            .map(to_posix_label)
            .unwrap_or_else(|_| to_posix_label(&absolute));
        return Some(CompactRead { kind: CompactReadKind::Resource, label });
    }
    None
```

Also correct the stale citations in this function's doc comment while it is open:
`getCompactReadClassification` is `read.ts:123-144` (not `:122-143`), `getPiDocsClassification` is
`:104-121` (not `:103-120`), and `COMPACT_RESOURCE_FILE_NAMES` is `read.ts:43` (not `:42`). Delete
the paragraph at `:145-151` that says the `docs` arm cannot be ported — it can, and this change
does it.

### 5. `tool_args.rs` — render the third kind

**Current** ([tool_args.rs:213-223](../../../crates/cyrup-tui/src/transcript/tool_args.rs)):

```rust
    let mut spans: Vec<Span<'static>> = Vec::new();
    if c.kind == "skill" {
        spans.push(Span::styled("[skill] ".to_string(), theme.custom_message_label_style()));
        spans.push(Span::styled(c.label.clone(), theme.custom_message_text_style()));
    } else {
        spans.push(Span::styled(format!("read {}", c.kind), theme.tool_title_style()));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(c.label.clone(), theme.accent_style()));
    }
```

**Replacement** — an exhaustive `match`, so the `Docs` variant is named rather than swept into an
`else`:

```rust
    let mut spans: Vec<Span<'static>> = Vec::new();
    match c.kind {
        // The `\x1b[1m…\x1b[22m` pair inside the interpolation is bold-on/bold-off around the
        // bracket label only; `custom_message_label_style` already carries BOLD.
        CompactReadKind::Skill => {
            spans.push(Span::styled("[skill] ".to_string(), theme.custom_message_label_style()));
            spans.push(Span::styled(c.label.clone(), theme.custom_message_text_style()));
        }
        // `read.ts:161-167` — docs and resource share ONE branch upstream; the kind word is
        // interpolated into the bold title and the label follows in accent.
        CompactReadKind::Docs | CompactReadKind::Resource => {
            spans.push(Span::styled(
                format!("read {}", c.kind.as_str()),
                theme.tool_title_style(),
            ));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(c.label.clone(), theme.accent_style()));
        }
    }
```

The tail below it is unchanged: the `:offset-limit` range in `warning_style`, then the whole-run
`dim` ` (<key> to expand)`.

Update the doc-comment citation on `compact_read_call` from `read.ts:145-167` to `read.ts:146-168`.

## Exact rendered output

For a collapsed `read` of `docs/providers.md` inside the asset root, with
`{"path": "docs/providers.md"}` and `app.tools.expand` bound to `ctrl+o`, the header line is
exactly:

```text
read docs docs/providers.md (ctrl+o to expand)
```

span by span:

| text | style |
| --- | --- |
| `read docs` | `tool_title_style()` |
| ` ` | unstyled (`Span::raw`) |
| `docs/providers.md` | `accent_style()` |
| `:2-4` *(only when `offset`/`limit` are present)* | `warning_style()` |
| ` (ctrl+o to expand)` | `dim_style()` |

Other resolutions, all against an asset root of `/opt/cyrup`:

| read path | header |
| --- | --- |
| `/opt/cyrup/README.md` | `read docs README.md` |
| `/opt/cyrup/docs/guide/x.md` | `read docs docs/guide/x.md` |
| `/opt/cyrup/examples/e.rs` | `read docs examples/e.rs` |
| `/opt/cyrup/docs` (the directory) | generic `read /opt/cyrup/docs` — `startsWith("docs/")` needs the separator |
| `/opt/cyrup/CHANGELOG.md` | generic `read /opt/cyrup/CHANGELOG.md` |
| `/opt/cyrup` (the root itself) | generic — the empty-relative rejection |
| `/opt/cyrup/docs/AGENTS.md` | `read docs docs/AGENTS.md` — docs is checked **before** the resource set |
| `/opt/cyrup/docs/x/SKILL.md` | `[skill] x` — `SKILL.md` is checked **before** docs |
| `/opt/other/docs/x.md` | generic — outside the root |
| any of the above while **expanded** | generic `read <path>` plus the body |

## Files that change

| file | change |
| --- | --- |
| [crates/cyrup-config/src/paths.rs](../../../crates/cyrup-config/src/paths.rs) | new `asset_dir()` + `resolve_asset_dir()`, appended after `resolve_path_from_base_with_home` |
| [crates/cyrup-config/src/lib.rs](../../../crates/cyrup-config/src/lib.rs) | add `asset_dir` to the `pub use paths::{…}` list at `:67-69` |
| [crates/cyrup-tui/src/transcript/tool_args.rs](../../../crates/cyrup-tui/src/transcript/tool_args.rs) | `CompactReadKind` enum replaces `kind: &'static str`; new `docs_classification` + `to_posix_label`; `compact_read_classification` switches to `cyrup_tools::path::resolve_to_cwd` and gains the docs arm; `compact_read_call` becomes an exhaustive `match`; stale upstream line citations corrected |

Nothing else moves. In particular:

* [tool_builtin.rs](../../../crates/cyrup-tui/src/transcript/tool_builtin.rs) is untouched — it
  already switches on `Option<CompactRead>` and never names the kind;
* [tool_render.rs:94-138](../../../crates/cyrup-tui/src/transcript/tool_render.rs) `ImageOpts` gains
  **no** field. Upstream's `getPiDocsClassification` takes only `absolutePath` and reaches for
  `getReadmePath()` ambiently; the value is process-global and immutable, unlike `cwd`, which is
  per-session and is why *that* one is threaded. Adding a field would churn all three construction
  sites ([cache.rs:141](../../../crates/cyrup-tui/src/transcript/cache.rs),
  [tool_render.rs:128](../../../crates/cyrup-tui/src/transcript/tool_render.rs),
  [draw.rs:143](../../../crates/cyrup-tui/src/app/draw.rs)) plus `Default` to carry a constant;
* [env.rs](../../../crates/cyrup-config/src/env.rs) is untouched — see the `CYRUP_PACKAGE_DIR` trap;
* `crates/cyrup-tools/src/` is untouched. It already exports the one primitive this needs,
  `resolve_to_cwd` ([path.rs:248-271](../../../crates/cyrup-tools/src/path.rs)), and
  [cyrup-tui/Cargo.toml:122](../../../crates/cyrup-tui/Cargo.toml) already depends on it.

## Coordination with the sibling `:offset-limit` task

[LOW-the-offset-limit-header-range-disappears-when-offset-limit-arrive-as-jso.md](./LOW-the-offset-limit-header-range-disappears-when-offset-limit-arrive-as-jso.md)
rewrites `read_line_range` ([tool_args.rs:58-69](../../../crates/cyrup-tui/src/transcript/tool_args.rs))
to read the numbers as `f64` and fold them through the `jsnum` truncation. The two tasks touch the
same file but **disjoint functions**, and they compose without further work: `compact_read_call`
already calls `read_line_range` unconditionally
([tool_args.rs:224-226](../../../crates/cyrup-tui/src/transcript/tool_args.rs)), so the new `docs`
header inherits the float fix the moment it lands — `{"path":"docs/x.md","offset":2.0,"limit":3.0}`
renders `read docs docs/x.md:2-4`. Neither task should edit the other's function. That task also
owns correcting `read_line_range`'s stale citation (`read.ts:67-72` → `read.ts:73-78`); leave it.

## Genuine uncertainties

1. **Whether cyrup ever ships `README.md` / `docs/` / `examples/` beside its binary.** It does not
   today: themes are compiled in (`cyrup_resources::theme::builtin_themes`), and there is no
   packaging step that copies a docs tree next to the executable. Tier 3 of `asset_dir()` makes the
   branch live for a source checkout — `/home/user/cyrup` has `README.md` and `docs/` but no
   `examples/` — and `CYRUP_ASSET_DIR` makes it live for a packager. If neither holds at runtime the
   arm is simply never taken, which is upstream's own behaviour for an install with no docs tree.
   This is the reason the item is LOW and cosmetic, not a reason to leave the arm out.
2. **Tier ordering inside `resolve_asset_dir`.** Upstream picks the exe directory only when
   `isBunBinary` — a build-time constant cyrup has no equivalent of. The `README.md`-beside-the-exe
   probe is the closest runtime-detectable stand-in; a distributor who ships `docs/` but no
   top-level `README.md` next to the binary would fall through to the `Cargo.toml` walk and find
   nothing. `CYRUP_ASSET_DIR` is the escape hatch, and it is the same escape hatch upstream offers.
3. **Symlinked asset roots.** `strip_prefix` is lexical, as is pi's `relative()` on `resolve()`d
   inputs. A read reaching the docs tree through a symlink outside the root classifies as generic on
   both sides. No canonicalization should be added.

## Definition of done

Observable behaviour, with `<root>` standing for the resolved asset root:

1. A collapsed `read` of `<root>/README.md` renders the header `read docs README.md`, followed by
   the ` (<expand key> to expand)` hint, and no file body.
2. A collapsed `read` of any path under `<root>/docs/` or `<root>/examples/` renders
   `read docs <posix relative path>` — `<root>/docs/guide/x.md` renders `read docs docs/guide/x.md`.
3. A collapsed `read` of `<root>` itself, of `<root>/docs` as a directory, of `<root>/CHANGELOG.md`,
   or of any path outside `<root>`, renders the generic `read <path>` header unchanged.
4. `<root>/docs/AGENTS.md` renders `read docs docs/AGENTS.md`, not `read resource …`; and
   `<root>/docs/x/SKILL.md` renders `[skill] x`. The precedence is SKILL.md, then docs, then the
   resource set.
5. Expanding any of the above (`Ctrl+O`) replaces the compact header with the generic
   `read <path>` header plus the file body, exactly as it already does for `skill` and `resource`.
6. A `docs`-classified read carrying `offset`/`limit` renders the range suffix between the label
   and the expand hint: `read docs docs/x.md:2-4 (ctrl+o to expand)`.
7. `read docs` headers still resolve correctly for a path spelled with `~`, a leading `@`,
   `file://`, or `.`/`..` segments — the classifier resolves through `resolve_to_cwd`, so
   `<root>/docs/../docs/x.md` classifies exactly as `<root>/docs/x.md` does.
8. Setting `CYRUP_ASSET_DIR` before the process starts relocates every judgement above to that
   directory; unsetting it restores the exe-directory / workspace-root resolution. `CYRUP_PACKAGE_DIR`
   has no effect on any of it, and the installed-package store still resolves to `<agent_dir>/packages`.
9. The `skill` and `resource` headers render byte-identically to before this change for every path
   that does not lie inside `<root>`.
10. Behaviour that pi does NOT have is not introduced — this is a parity task, not a redesign.
