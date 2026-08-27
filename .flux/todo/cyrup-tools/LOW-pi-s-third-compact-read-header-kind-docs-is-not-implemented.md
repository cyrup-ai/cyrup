---
title: Pi's third compact read header kind docs is not implemented
priority: LOW
tool: read
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: qa
status: needs-rework
updated: 2026-08-27 14:06
---

# Pi's third compact read header kind docs is not implemented

QA rating **7/10**. The feature itself landed correctly and faithfully — `asset_dir()`, the
`CompactReadKind` enum, `docs_classification`, the `resolve_to_cwd` fix and the precedence order all
match the spec, and `env.rs` was correctly left alone. One regression was introduced in the adjacent
`resource` arm that this change rewrote. Only the items below remain.

## 1. `to_posix_label` doubles the leading separator on an absolute path (BLOCKER)

[crates/cyrup-tui/src/transcript/tool_args.rs](../../../crates/cyrup-tui/src/transcript/tool_args.rs)
`to_posix_label`:

```rust
fn to_posix_label(path: &std::path::Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}
```

`Component::RootDir::as_os_str()` is `"/"` (this is the literal example in the `std` docs for
`Components::as_os_str`), so for an absolute path the root contributes a `"/"` element that then gets
a second `"/"` from the `join`. Verified by execution:

| input | `to_posix_label` | upstream `.split(sep).join("/")` |
| --- | --- | --- |
| `/etc/cyrup/AGENTS.md` | `//etc/cyrup/AGENTS.md` | `/etc/cyrup/AGENTS.md` |
| `docs/guide/x.md` | `docs/guide/x.md` | `docs/guide/x.md` |

The relative call sites (`docs_classification`, and the resource arm's `strip_prefix` success path)
are unaffected. The broken one is the resource arm's fallback:

```rust
let label = absolute
    .strip_prefix(&base)
    .map(to_posix_label)
    .unwrap_or_else(|_| to_posix_label(&absolute));   // <- absolute path, doubled slash
```

Before this change that arm read `absolute.to_string_lossy().into_owned()` and rendered a single
slash, so this is a **regression against DoD clause 9** ("the `skill` and `resource` headers render
byte-identically to before this change for every path that does not lie inside `<root>`") and a
divergence from `formatPathRelativeToCwdOrAbsolute` (`utils/paths.ts:119-122`), which is
`(getCwdRelativePath(...) ?? absolutePath).split(sep).join("/")` — a string split, where the leading
empty segment rejoins to exactly one `/`.

The switch to `resolve_to_cwd` (correct, and required) makes this branch *more* reachable than it
was: `~/…/AGENTS.md`, `file://…/CLAUDE.md` and `@/abs/AGENTS.md` now resolve outside the session cwd
and land in the fallback, where `base.join(&raw_path)` used to keep them cwd-relative. A collapsed
read of `~/.cyrup/AGENTS.md` renders `read resource //home/<user>/.cyrup/AGENTS.md`.

Fix `to_posix_label` so the root component does not contribute a separator of its own — e.g. emit the
root once and join only the `Normal`/`Prefix` remainder, or fold on `MAIN_SEPARATOR` over the string
form the way upstream does. Keep the relative-path output byte-identical.

## 2. None of the ten documented resolutions is asserted anywhere

`rg 'CompactReadKind|docs_classification|read docs '` over `crates/cyrup-tui` finds no test. The
only compact-read coverage is `x7_agents_md_is_a_compact_resource_read`
([crates/cyrup-tui/src/transcript/tests/x_group.rs:199-215](../../../crates/cyrup-tui/src/transcript/tests/x_group.rs))
and `transcript_expand_wiring.rs:73`, both of which use a synthetic cwd of `/w/project` — outside the
asset root — so they exercise the `resource` arm only and would not have caught item 1.

The docs arm is testable without touching the environment: in a test binary `asset_dir()` resolves via
tier 3 to the workspace root, so a test can take `cyrup_config::asset_dir().unwrap()` and build the
read path from it. Add coverage for at least the discriminating rows of the spec's table:

* `<root>/README.md` → `read docs README.md`;
* `<root>/docs/guide/x.md` → `read docs docs/guide/x.md`;
* `<root>/docs` (the directory) and `<root>/CHANGELOG.md` → generic header (the `startsWith("docs/")`
  separator guard and the non-docs sibling);
* `<root>/docs/AGENTS.md` → `read docs docs/AGENTS.md`, **not** `read resource` (the precedence that
  the enum was introduced to protect);
* `<root>/docs/../docs/x.md` → identical to `<root>/docs/x.md` (the `resolve_to_cwd` fix);
* a `resource` read that resolves OUTSIDE the cwd → single leading slash (the regression in item 1).

## Verified and NOT outstanding — do not redo

* `crates/cyrup-config/src/paths.rs:155-201` — `asset_dir()` / `resolve_asset_dir()` match the spec
  verbatim: `OnceLock` memoization, `$CYRUP_ASSET_DIR` → exe dir holding `README.md` → nearest
  ancestor `Cargo.toml`, `None` when nothing is discoverable.
* `crates/cyrup-config/src/lib.rs:67-70` — `asset_dir` re-exported.
* `crates/cyrup-config/src/env.rs:99` — untouched; `CYRUP_PACKAGE_DIR`/`PI_PACKAGE_DIR` still bind to
  `ConfigDirs::package_dir` (the install store) and nothing else. The trap was avoided.
* `CompactReadKind` enum + `as_str()` replace `kind: &'static str`; `compact_read_call` is an
  exhaustive `match` with `Docs | Resource` sharing the non-skill branch.
* `docs_classification` — `strip_prefix`, empty-relative rejection, and the
  `README.md` / `docs/` / `examples/` guard with the separator required.
* `compact_read_classification` uses `cyrup_tools::path::resolve_to_cwd(&raw_path, &base)`; the docs
  arm sits between `SKILL.md` and `COMPACT_RESOURCE_FILE_NAMES`.
* Stale citations corrected (`read.ts:123-144`, `:104-121`, `read.ts:43`, `read.ts:146-168`) and the
  "the docs arm cannot be ported" paragraph deleted.
* `tool_builtin.rs`, `tool_render.rs`'s `ImageOpts`, and `crates/cyrup-tools/src/` carry no change
  from this task.
