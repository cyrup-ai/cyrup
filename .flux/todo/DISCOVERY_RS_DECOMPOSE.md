---
stage: new
status: done
updated: 2026-08-22 23:09
---

# Decompose src/discovery.rs — Split The 644-Line discover_blocking

**Owns files:** `crates/cyrup-resources/src/discovery.rs` -> `crates/cyrup-resources/src/discovery/`

## Description

`src/discovery.rs` is 1,987 lines and one function is **32% of it**: `discover_blocking` spans
810-1453 (644 lines). The next longest function in the file is `resolve_configured_package` at 144
lines — a 4.5x gap.

The seams are not a matter of taste: the author already drew **eight section banners inside the
single function body**.

| Banner | Line | Proposed helper |
| --- | --- | --- |
| built-in themes | 821 | `collect_builtin_themes` |
| global loose resources | 826 | `collect_global_loose` |
| settings positive listings | 873 | `collect_settings_listings` |
| packages (settings-declared + install registry) | 907 | `collect_packages` (310 lines on its own) |
| project loose resources | 1217 | `collect_project_loose` |
| `resources_discover` contributions | 1326 | `collect_discover_contributions` |
| explicit CLI flags | 1362 | `collect_cli_paths` |
| top-level enable/disable filter | 1401 | `apply_disabled_filter` |

The function opens by declaring six mutable accumulators at 814-819 (`skills`, `prompts`, `themes`,
`warnings`, `diagnostics`, `ext_paths`) that every section mutates.

### Why this is a mechanical move, not a rewrite

Three refutations were tried against this and all three failed:

1. **"Locals leak across the banners, so extraction needs return values."** False. Every
   function-body-level `let` was enumerated: the six accumulators at 814-819, then 917/918/939/940/
   941/983/1012 (all inside the 907-1216 packages section) and 1402/1407 (inside 1401-1453). Nothing
   crosses a banner. The four locals that look most suspicious (`trees`, `ordered_cfg`,
   `project_deltas`, `seen_trees`) are last used at 1029, far short of the 1216 boundary.
2. **"There is `?`/early-return/cancel plumbing."** False. The body has exactly two `return`/`cancel`
   hits, both the `cancel: &CancelToken` parameter and its pass-through at 942, whose `Result` is
   consumed by a local `match`. No `?`, no early return — so every helper can be a plain
   `fn(..) -> ()`.
3. **"The public surface breaks."** False. The leaf scanners at 1455-1987 are referenced by nothing
   outside this file. The only cross-module uses are `lib.rs:43` (public items) and
   `src/tests/resources/settings_packages.rs:282` calling `pub(crate) install_declared_git_package`;
   both keep resolving through re-exports, so neither file needs editing. There is no in-file
   `mod tests` to relocate.

### Target layout

`src/discovery.rs` becomes `src/discovery/` with re-exports in `mod.rs` so `lib.rs`'s
`pub mod discovery;` and its `pub use discovery::{...}` list are **unchanged**:

- **`mod.rs`** — current 1-248 (`Named`, `ResourceSet`, `CliResourcePaths`, `DiscoveredPaths`,
  `ResourceOverrides`, `DiscoveryConfig`) plus 641-809 (`ResourceRegistry`, `DiscoveryReport`,
  `discover_system_prompt_file`, `discover_append_system_prompt_file`, `discover_prompt_override`,
  `pub async fn discover`)
- **`packages.rs`** — current 249-640
- **`scan.rs`** — current **1455**-1987 (start at 1455, **not** 1456: 1455 is the `///` doc comment
  for `emit_collisions` and starting at 1456 orphans it)
- **`blocking.rs`** — current 810-1453, with the eight sections extracted as private fns

`discover_blocking` then reads as its own table of contents.

### Method

Use the procedure that worked for `tests/resources.rs`: extract by **whole-line range copy** so the
diff is a verifiable move, and **re-derive every line number from the file immediately before
cutting** — do not trust the ranges in this document if other tasks have landed first. Precedent for
the directory layout already exists in-repo at `crates/cyrup-ext-subagents/src/discovery/`.

## Acceptance Criteria

- [ ] `src/discovery.rs` is gone; `src/discovery/` holds `mod.rs`, `packages.rs`, `scan.rs`, `blocking.rs`
- [ ] `discover_blocking` is under 80 lines and calls eight named helpers
- [ ] `src/lib.rs` is **unmodified**
- [ ] No logic edits — helper bodies are byte-identical to the ranges they came from
- [ ] `cargo test -p cyrup-resources` unchanged: `103 passed; 0 failed; 1 ignored`
- [ ] `cargo clippy -p cyrup-resources --all-targets` reports no new findings
