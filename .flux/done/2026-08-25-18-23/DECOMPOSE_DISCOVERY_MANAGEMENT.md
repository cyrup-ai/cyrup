---
stage: exec
status: done
updated: 2026-08-25 20:15
---

# Decompose discovery/management.rs Into Submodules

## Description

`crates/cyrup-ext-subagents/src/discovery/management.rs` is 5,515 lines (3,675 production +
1,840 tests, one flat `#[cfg(test)] mod tests { use super::*; ... }` with 72 tests, no internal
nesting) — the second-largest file in the crate after the already-decomposed `exec/` cluster. This
crate is a Rust port of an upstream TypeScript codebase ("pi-subagents"); nearly every item here
carries a doc comment tying it to an `R-SA-###`/`arch-SA §#.#`/`func-SA §#.#` requirement ID and an
upstream `pi` source location. **Preserve every doc comment verbatim when moving code** — they are
the crate's requirement-traceability record, not incidental commentary.

**DO THE SPLIT.** This is an implementation task: physically move the code below into the new
files under a new `discovery/management/` directory, fix visibility, keep the crate compiling, and
land it. The plan has already been researched (full call graph traced below by grep against the
real file) — executing it is the deliverable.

### The file's own section banners mark 8 top-level concerns

`management.rs` contains banner comments (`// ---...` and `// ===...`) the original author left as
section dividers. They line up closely with the natural seams found by tracing the actual call
graph, with a handful of deliberate deviations noted below where the traced graph disagrees with
the banner grouping (same pattern as the `exec/mod.rs` split's `structured_output_absent`
correction — **trust the traced call graph over the banner text** wherever they conflict).

| Banner (paraphrased)                                                              | Lines       |
|-------------------------------------------------------------------------------------|-------------|
| R-SA-013: call-site-dependent `disabled` visibility                                | 73–137      |
| R-SA-014: read-only-source guard                                                    | 138–151     |
| Agent field-set (management-facing delta shape)                                     | 152–208     |
| Package-identifier validation                                                       | 209–270     |
| Agent create/update/delete/rename                                                   | 271–583     |
| Frontmatter serialization (write-back)                                              | 584–971     |
| Chain create/update/delete/rename                                                   | 972–1249    |
| Management-action dispatch + renderers (types, consts, `handle_management_action`)  | 1250–1281   |
| Small shared helpers (source/context rendering, scope parsing, name sanitization)   | 1412–1535   |
| config-object / package-name parsing                                                | 1536–1580   |
| applyAgentConfig                                                                     | 1581–1821   |
| parseStepList                                                                       | 1822–1938   |
| discovery-driven lookup helpers                                                     | 1939–2230   |
| Renderers (`formatAgentDetail`/`formatChainDetail`/`formatChainStepDetail`)          | 2231–2484   |
| `handleList`/`handleGet`/`handleModels`/`handleCreate`/`handleUpdate`/`handleDelete` | 2485–3194   |
| SUBA-005: tier-aware / settings-writing actions (`eject`/`disable`/`enable`/`reset`) | 3195–3675   |
| `#[cfg(test)] mod tests { use super::*; ... }`                                       | 3676–5515   |

Line numbers throughout this task are against the file as it stands before any extraction begins;
re-derive them as the file shrinks (grep for the item name rather than trusting a stale absolute
number after step 1 — the split itself will shift everything below each cut).

## ⚠️ THE CRITICAL GOTCHA: `super::` means something different one level down

`management.rs` is today a direct child of `discovery/` — every `super::X` inside it resolves to
`discovery::X`. This split converts it to `discovery/management/mod.rs` **plus sibling files under
`discovery/management/`**. For code that stays in `management/mod.rs`, `super::` still means
`discovery::` (a directory's `mod.rs` has the same parent as the flat file it replaces — no change
needed there). **But for every new submodule file** (`management/agent_crud.rs`,
`management/handlers.rs`, etc.), `super::` now means `management::`, NOT `discovery::` — a `super::`
reference that moves into one of these new files and is left unchanged will either fail to compile
or (worse, if a name happens to collide) silently resolve to the wrong item.

**Fix**: every reference below that currently reads `super::X` and moves into a new submodule file
must become `crate::discovery::X` (absolute path — do not use `super::super::X`, it works but reads
worse and every other cross-module reference in this crate's recent decompositions uses the
absolute-path style). The exhaustive list of `super::`-qualified items used in this file, and where
each one's callers end up:

| Original `super::` reference | New path | Needed in |
|---|---|---|
| `super::types::{AgentDefinition, AgentModelSourceInfo, AgentSource, ChainDefinition, ChainDiscoveryDiagnostic, ChainListBinding, ChainOutputBinding, ChainStepConfig, OutputSpec, OverrideScope, SystemPromptMode, ToolRef}` | `crate::discovery::types::{...}` | pervasive — nearly every new file needs some subset; import only what each file actually uses, let the compiler flag the rest |
| `super::{discover_agents_all, resolve_agent_name, AgentDiscoveryConfig, AgentDiscoveryResult, AgentNameResolution}` | `crate::discovery::{...}` | pervasive — every `handle_*` function takes `cfg: &AgentDiscoveryConfig` and calls `discover_agents_all(cfg)`; needed in `handlers.rs` and `tier_actions.rs` at minimum |
| `super::package_name::{collapse_repeated_char, is_valid_package_identifier}` | `crate::discovery::package_name::{...}` | wherever `normalize_package_identifier` ends up (→ `helpers.rs`, see below) |
| `super::frontmatter::parse_agent_file` (line 576, inside `reparse_agent_file`) | `crate::discovery::frontmatter::parse_agent_file` | `agent_crud.rs` |
| `super::merge::TieredAgents` (lines 3281, 3293, inside `find_bundled`/`writable_tier`) | `crate::discovery::merge::TieredAgents` | `tier_actions.rs` |
| `super::load_layered_override_settings` (line 3333, inside `with_settings_reread`) | `crate::discovery::load_layered_override_settings` | `tier_actions.rs` |
| `super::scan_agent_tiers` (lines 3360, 3593, inside `handle_eject`/`handle_reset`) | `crate::discovery::scan_agent_tiers` | `tier_actions.rs` |
| `super::settings_write::{merge_builtin_agent_override, remove_builtin_agent_override_fields, remove_builtin_agent_override}` (lines 3461, 3524, 3628) | `crate::discovery::settings_write::{...}` | `tier_actions.rs` |

Also check `discovery/mod.rs` itself (the parent): it does `use management::{AgentVisibility,
ChainVisibility};` (flat import) at line 86, plus several `[`management::AgentVisibility`]`-style
doc-comment intra-doc links. Converting `management.rs` to `management/mod.rs` does **not** by
itself require any change to `discovery/mod.rs`'s `pub mod management;` declaration — Rust resolves
that identically whether the target is `management.rs` or `management/mod.rs`. But
`AgentVisibility`/`ChainVisibility` move into `management/visibility.rs` (see layout below), so
`management/mod.rs` must re-export them (`pub use visibility::{AgentVisibility, ChainVisibility};`)
to keep `discovery/mod.rs`'s existing `use management::{AgentVisibility, ChainVisibility};` and its
doc-comment intra-doc links (`[`management::AgentVisibility::management`]`, etc.) resolving without
any edit to `discovery/mod.rs` itself.

## External API surface that MUST stay reachable at `crate::discovery::management::<Name>`

Verified by crate-wide grep — these are consumed from outside `management.rs` today (from
`registration/profiles.rs`, `discovery/skills.rs`, `extension/tool/{text,routing,routing_tests,schema}.rs`,
`extension/host/{mod,slash_render}.rs`, `discovery/frontmatter.rs` (doc comment only), and the
in-crate integration test files `tests/discovery_integration.rs` /
`tests/management_actions_integration.rs`):

- `BUILTIN_AGENT_NAMES`, `MANAGEMENT_ACTIONS`, `MUTATING_MANAGEMENT_ACTIONS` (consts)
- `ManagementRequest`, `ProactiveSkillsInput`, `ManagementOutcome`
- `handle_management_action`

All four of these stay defined in `management/mod.rs` itself (see layout item 11 below) — they are
already at `management::` today and nothing needs to move for them to keep resolving. Do not
relocate any of these four into a leaf submodule even though the "Management-action dispatch"
banner is short; the alternative (moving them and re-exporting) works too but is strictly more
error-prone for zero benefit, since nothing else in this list needs to move to make its home
correct.

`AgentVisibility`/`ChainVisibility` (consumed by `discovery/mod.rs`, see above) need the
`pub use visibility::{AgentVisibility, ChainVisibility};` re-export noted above.

## Full item inventory (absolute line numbers, current file)

```
1     module doc comment (`//!`)
54    imports (`use` block, see gotcha table above for `super::` entries)
73    struct AgentVisibility;                                    [pub] + impl (83)
122   struct ChainVisibility;                                    [pub] + impl (124)
144   fn require_writable_source(..) -> Result<(), ..>            [private -> needs pub(crate)]
167   struct AgentFields { .. }                                   [pub]
204   struct AgentMutationOutcome { .. }                          [pub]
231   fn normalize_package_identifier(..) -> Option<String>       [private -> needs pub(crate)]
290   pub fn create_agent(..) -> Result<Option<AgentMutationOutcome>, ..>
342   pub fn update_agent(..) -> Result<Option<AgentMutationOutcome>, ..>
383   pub fn delete_agent(&AgentDefinition) -> Result<(), ..>
394   pub fn rename_agent(..) -> Result<AgentMutationOutcome, ..>
439   fn agent_file_path(&Path, &str) -> PathBuf                  [private -> needs pub(crate)]
443   fn build_definition(..) -> ..                                [private, sole caller same file]
496   fn merge_fields(..) -> ..                                    [private, sole caller same file]
574   fn reparse_agent_file(&Path, AgentSource) -> Result<..>       [private, sole caller same file]
603   fn write_agent_file(..) -> Result<(), ..>                    [private -> needs pub(crate)]
616   fn serialize_agent(..) -> String                             [private, sole caller same file]
881   fn preserved_frontmatter_fields(..) -> HashSet<String>       [private -> needs pub(crate)]
965   fn tool_ref_to_frontmatter_entry(&ToolRef) -> String          [private, sole caller same file (used as fn-pointer via `.map(...)`, not a `(`-call — grep for the bare name, not `name(`]
985   struct ChainFields { .. }                                   [pub]
995   fn placeholder_chain_step() -> ChainStepConfig                [private]
1003  pub fn create_chain(..) -> Result<Option<ChainDefinition>, ..>   (no production caller — tested directly, see Test triage)
1036  pub fn update_chain(..) -> Result<Option<ChainDefinition>, ..>   (no production caller — tested directly)
1066  pub fn delete_chain(&ChainDefinition) -> Result<(), ..>
1076  pub fn rename_chain(..) -> Result<ChainDefinition, ..>           (no production caller — tested directly)
1122  pub fn create_chain_with_steps(..) -> Result<ChainDefinition, ..>
1161  pub fn update_chain_full(..) -> Result<ChainDefinition, ..>
1199  fn chain_file_path(&Path, &str) -> PathBuf                   [private, sole caller same file]
1203  fn write_chain_file(&Path, &ChainDefinition) -> Result<(), ..> [private, sole caller same file]
1217  fn serialize_chain_json(&ChainDefinition) -> String            [private, sole caller same file]
1294  const BUILTIN_AGENT_NAMES: [&str; 7]                          [pub — external API, see above]
1307  struct ManagementRequest<'a> { .. }                           [pub — external API]
1340  struct ProactiveSkillsInput<'a> { .. }                        [pub — external API]
1351  struct ManagementOutcome { .. }  + impl (1356)                [pub — external API]
1369  const MANAGEMENT_ACTIONS: [&str; 10]                          [pub — external API]
1379  const MUTATING_MANAGEMENT_ACTIONS: [&str; 7]                  [pub — external API]
1391  pub async fn handle_management_action(..) -> ..                [pub — external API, the dispatcher]
1417  fn source_str(AgentSource) -> &'static str                    [private -> needs pub(crate), widely used]
1426  fn context_str(ContextMode) -> &'static str                   [private -> needs pub(crate)]
1433  fn override_scope_str(OverrideScope) -> &'static str          [private -> needs pub(crate)]
1442  fn disambiguation_scope(Option<&str>) -> Option<AgentSource>  [private -> needs pub(crate)]
1452  fn normalize_list_scope(Option<&str>) -> Option<AgentSource>  [private -> needs pub(crate) — single caller (handle_list) but keep with its helper siblings]
1462  fn sanitize_name(&str) -> String                              [private -> needs pub(crate), widely used]
1488  fn parse_csv(&str) -> Vec<String>                             [private, sole callers inside 1543-1826 — see note below, do NOT put in helpers.rs]
1504  fn parse_tools(&str) -> Vec<ToolRef>                          [private, sole caller inside 1543-1826 — same note]
1524  fn default_system_prompt_mode(&str) -> SystemPromptMode       [private, sole caller = handle_create]
1532  fn default_inherit_project_context(&str) -> bool              [private, sole caller = handle_create]
1543  fn config_object(..) -> Result<Option<Map>, String>            [private -> needs pub(crate), called only from handle_create/handle_update]
1568  fn parse_package_config(..) -> Result<Option<String>, String>  [private -> needs pub(crate), called only from handle_create/handle_update]
1587  fn apply_agent_config(..) -> Result<(), String>                [private -> needs pub(crate), called only from handle_create/handle_update]  (234 lines — largest single fn in this file after the handlers)
1826  fn parse_step_list(..) -> Result<Vec<ChainStepConfig>, String> [private -> needs pub(crate), called only from handle_create/handle_update]
1960  fn find_agents(..) -> Vec<AgentDefinition>                     [private -> needs pub(crate)]
1998  fn distinct_agent_names(..) -> Vec<String>                     [private -> needs pub(crate), sole caller handle_get]
2003  fn find_chains(..) -> Vec<ChainDefinition>                     [private -> needs pub(crate)]
2018  fn available_agent_names(..) -> Vec<String>                    [private -> needs pub(crate) — used by BOTH handlers.rs and tier_actions.rs]
2027  fn available_chain_names(..) -> Vec<String>                    [private -> needs pub(crate), used by handlers.rs only]
2038  fn name_exists_in_scope(..) -> bool                            [private -> needs pub(crate) — used by BOTH handlers.rs and tier_actions.rs (handle_eject, line 3381)]
2059  fn unknown_chain_agents(..) -> Vec<String>                     [private -> needs pub(crate)]
2077  trait MutableTarget: Clone { .. }                              [private -> needs pub(crate)]
2086  impl MutableTarget for AgentDefinition { .. }
2098  impl MutableTarget for ChainDefinition { .. }
2111  enum TargetKind { Agent, Chain }  + impl (2116)                 [private -> needs pub(crate)]
2134  fn resolve_target<T: MutableTarget>(..) -> ..                  [private -> needs pub(crate)]
2236  fn format_agent_detail(&AgentDefinition) -> String              [private -> needs pub(crate)]
2365  fn format_chain_step_detail(..) -> Vec<String>                  [private, sole caller = format_chain_detail, same file]
2465  fn format_chain_detail(&ChainDefinition) -> String              [private -> needs pub(crate)]
2489  fn agent_in_list_scope(..) -> bool                              [private, sole caller handle_list — keep co-located there, do NOT put in a lookup/render file]
2493  fn chain_in_list_scope(..) -> bool                              [private, sole caller handle_list — same]
2515  fn handle_list(..) -> Result<ManagementOutcome, ..>
2614  fn handle_get(..) -> Result<ManagementOutcome, ..>
2684  fn format_model_source(..) -> String                            [private -> needs pub(crate), sole caller handle_models]
2712  fn handle_models(..) -> Result<ManagementOutcome, ..>
2809  fn pick_scope_dir(..) -> Option<PathBuf>                        [private -> needs pub(crate) — used by BOTH handlers.rs (handle_create) and tier_actions.rs (handle_eject, line 3388)]
2823  fn handle_create(..) -> Result<ManagementOutcome, ..>
2942  fn editable_base(&AgentDefinition) -> AgentDefinition           [private, sole caller handle_update — keep co-located]
2963  fn handle_update(..) -> Result<ManagementOutcome, ..>
3146  fn handle_delete(..) -> Result<ManagementOutcome, ..>
3222  fn action_scope(..) -> Result<AgentSource, ManagementOutcome>   [private, callers all within 3195-3675 — stays local to tier_actions.rs]
3250  fn resolve_effective_agent(..) -> ..                            [private, same — local to tier_actions.rs]
3280  fn find_bundled(..) -> ..                                        [private, same]
3293  fn writable_tier(..) -> &[AgentDefinition]                       [private, same]
3307  fn scope_settings_path(..) -> ..                                 [private, same]
3331  fn with_settings_reread(..) -> Result<AgentDiscoveryConfig, ..>  [private, same]
3348  fn handle_eject(..) -> Result<ManagementOutcome, ..>
3429  async fn handle_disable(..) -> Result<ManagementOutcome, ..>
3494  async fn handle_enable(..) -> Result<ManagementOutcome, ..>
3577  async fn handle_reset(..) -> Result<ManagementOutcome, ..>
3676  #[cfg(test)] mod tests { .. }   (1,840 lines, flat, `use super::*;`, 72 tests)
```

## Cross-file call graph (traced by grep against the real file — every claim below is `grep -n`
verified; re-verify before moving code, since the split will shift line numbers as you go)

This is the load-bearing part of the plan — it determines exactly which items need a visibility
bump to `pub(crate)` and which stay private because they move as a unit with their sole caller:

- **`require_writable_source`** is called from `create_agent`(297)/`update_agent`(346)/
  `delete_agent`(384)/`rename_agent`(398) — all `agent_crud.rs` — **and** `create_chain`(1009)/
  `update_chain`(1040)/`delete_chain`(1067)/`rename_chain`(1080)/`create_chain_with_steps`(1130)/
  `update_chain_full`(1168) — all `chain_crud.rs`. Shared across two files → `pub(crate)`, lives in
  `visibility.rs` with `AgentVisibility`/`ChainVisibility` (its own section banner already groups
  it there, R-SA-013/014 are adjacent requirements).
- **`normalize_package_identifier`** is called from `create_agent`(300)/`update_agent`(349) —
  `agent_crud.rs` — **and** `parse_package_config`(1573) — `config_parse.rs`. Shared across two
  files → `pub(crate)`. Neither file is a more natural home than the other; put it in `helpers.rs`
  (it is a small, ~40-line, self-contained string-validation function, consistent with that file's
  other small cross-cutting helpers). **Do not move its accompanying comment block verbatim without
  re-reading it first** — this exact function's header comment was already corrected in a prior
  landed change (commit `67fc120`) to say it imports `collapse_repeated_char`/
  `is_valid_package_identifier` from `crate::discovery::package_name` rather than duplicating them;
  preserve that corrected version, not an older one.
- **`write_agent_file`** is called only from `create_agent`(326)/`update_agent`(373)/
  `rename_agent`(429) — all `agent_crud.rs`, a different file from where `write_agent_file` itself
  lives (`frontmatter_write.rs`, per the "Frontmatter serialization" banner) → `pub(crate)`.
  `serialize_agent` (called only inside `write_agent_file` itself, line 611) and
  `tool_ref_to_frontmatter_entry` (called only inside `serialize_agent`'s body via `.map(...)`
  function-pointer syntax, line 653 — grep for the bare name, `tool_ref_to_frontmatter_entry(` with
  a literal paren misses this call site) both stay **private** to `frontmatter_write.rs`.
- **`preserved_frontmatter_fields`** is called only from `update_agent`(372) — `agent_crud.rs`,
  cross-file from `frontmatter_write.rs` → `pub(crate)`.
- **`agent_file_path`** is called from `create_agent`(317)/`rename_agent`(405)/`build_definition`(488)
  — all `agent_crud.rs` — **and** `handle_eject`(3395) — `tier_actions.rs`. Shared across two files
  → `pub(crate)`. Lives in `frontmatter_write.rs` alongside its sibling `write_agent_file` (both are
  "where does an agent file for this scope/name live, and how do we write to it" concerns) — despite
  the name suggesting it might fit `agent_crud.rs`, its actual definition site in the original file
  (line 439) sits inside the CRUD section, but keeping path-computation next to the sibling
  `chain_file_path` pattern was considered and rejected: `chain_file_path` has zero cross-file
  callers (stays private in `chain_crud.rs`), so there is no file where BOTH path functions can be
  `pub(crate)`-free; putting `agent_file_path` in `agent_crud.rs` itself (not `frontmatter_write.rs`)
  is equally valid — either works, just be consistent and update the target-layout section below if
  you choose `agent_crud.rs` instead.
- **`build_definition`**, **`merge_fields`**, **`reparse_agent_file`** are each called only from
  within `agent_crud.rs`'s own CRUD functions (308, 366, 327/374/432 respectively) — stay **private**
  to `agent_crud.rs`. `reparse_agent_file` needs `crate::discovery::frontmatter::parse_agent_file`
  (see gotcha table).
- **`chain_file_path`**, **`write_chain_file`**, **`serialize_chain_json`** are each called only
  from within `chain_crud.rs`'s own CRUD functions — stay **private** to `chain_crud.rs`. No
  visibility bumps needed anywhere in the chain-CRUD cluster.
- **`create_chain`/`update_chain`/`rename_chain`** (the "bare skeleton" primitives, per their own
  doc comments) have **zero production callers** — `handle_create`/`handle_update` call
  `create_chain_with_steps`/`update_chain_full` instead. Confirmed these three ARE exercised
  directly by tests (`create_chain_rejects_builtin_source`, `update_chain_rejects_builtin_and_package_sources`,
  `rename_chain_rejects_builtin_and_package_sources`, `delete_and_rename_chain_succeed_for_project_source`,
  etc.) — this is intentional pre-existing asymmetry (the bare primitives are public API + tested,
  just not wired into any handler), not something this task fixes. Move them and their tests to
  `chain_crud.rs` as-is.
- **`source_str`** is used extremely widely: inside `resolve_target` (`lookup.rs`), inside
  `format_agent_detail`/`format_chain_detail` (`render.rs`), inside `handle_list`/`handle_create`/
  `handle_update` (`handlers.rs`), and inside nearly every function in the tier-actions cluster
  (`scope_settings_path`, `with_settings_reread`, `handle_eject`, `handle_disable`,
  `resolve_effective_agent`, `handle_enable`, `find_bundled`/`writable_tier` call sites) —
  `tier_actions.rs`. Needs `pub(crate)`, lives in `helpers.rs`.
- **`context_str`** used in `render.rs` (`format_agent_detail`) and `handlers.rs` (`handle_list`) →
  `pub(crate)` in `helpers.rs`.
- **`override_scope_str`** used in `render.rs`-adjacent `format_model_source` (which itself lives in
  `render.rs`, see below) and heavily in `tier_actions.rs` → `pub(crate)` in `helpers.rs`.
- **`disambiguation_scope`** used in `lookup.rs` (`resolve_target`) and `handlers.rs`
  (`handle_update`, `handle_delete`) → `pub(crate)` in `helpers.rs`.
- **`normalize_list_scope`** used only in `handle_list` (`handlers.rs`) — single call site, but keep
  it in `helpers.rs` with its scope-parsing siblings rather than inlining into `handlers.rs`; the
  banner groups it there and it costs nothing (`pub(crate)`, one extra `use`).
- **`sanitize_name`** used in `lookup.rs` (`find_agents`, `find_chains`), `handlers.rs`
  (`handle_create`, `handle_update` ×2), and `tier_actions.rs` (`resolve_effective_agent`,
  `handle_eject`, `handle_reset`) — the single most cross-cutting helper in the file → `pub(crate)`
  in `helpers.rs`.
- **`parse_csv`**/**`parse_tools`** are called ONLY from within `apply_agent_config`'s own body
  (`parse_tools` itself also calls `parse_csv` once, at line 1506) — both entirely inside the
  1543–1826 range that becomes `config_parse.rs`. **Deviates from the banner grouping**: the file's
  own banner lists these under "Small shared helpers" alongside `sanitize_name`/`source_str`/etc.,
  but the traced call graph shows they have no caller outside `config_parse.rs` — move them there
  instead of `helpers.rs`, staying **private** (no `pub(crate)` needed, same-file callers only).
- **`default_system_prompt_mode`**/**`default_inherit_project_context`** are called only from
  `handle_create` (2913, 2914) — single caller, `handlers.rs`. **Also deviates from the banner
  grouping**: move these two into `handlers.rs` directly (private, co-located with their sole
  caller) rather than `helpers.rs`, avoiding an unnecessary `pub(crate)` bump for a 1:1 relationship.
- **`config_object`**, **`parse_package_config`**, **`apply_agent_config`**, **`parse_step_list`**
  are called ONLY from `handle_create`/`handle_update` (`handlers.rs`) — verified: no caller
  anywhere else, including no cross-calls between each other except `apply_agent_config` calling
  `parse_csv`/`parse_tools` (which move with it, see above). Despite this 1:1-directional
  relationship with `handlers.rs`, keep them in their own `config_parse.rs` file rather than
  inlining into `handlers.rs` — "parse a JSON config blob into a typed delta" is a genuinely
  distinct concern from "orchestrate a CRUD action", the separation is exactly what this task asks
  for, and `apply_agent_config` alone is 234 lines (would make `handlers.rs` unwieldy if inlined).
  All four need `pub(crate)`.
- **`find_agents`/`distinct_agent_names`/`find_chains`/`available_chain_names`/`unknown_chain_agents`/
  `MutableTarget`/`TargetKind`/`resolve_target`** are called exclusively from `handlers.rs`
  (`handle_get`, `handle_update`, `handle_delete`) — need `pub(crate)`, live in `lookup.rs`.
- **`available_agent_names`** is called from `handlers.rs` (`handle_get`, `handle_update`,
  `handle_delete`) **and** `tier_actions.rs` (`handle_eject` 3362, `handle_disable` 3450,
  `handle_enable` 3515, `handle_reset` 3607) — shared across two files, `pub(crate)`, lives in
  `lookup.rs`.
- **`name_exists_in_scope`** is called from `handlers.rs` (`handle_create` 2868) **and**
  `tier_actions.rs` (`handle_eject` 3381) — shared, `pub(crate)`, lives in `lookup.rs`.
- **`format_agent_detail`**/**`format_chain_detail`** called only from `handle_get` (`handlers.rs`)
  → `pub(crate)` in `render.rs`. `format_chain_step_detail` called only from `format_chain_detail`
  itself (same file) → stays private in `render.rs`.
- **`format_model_source`** called only from `handle_models` (`handlers.rs`) → `pub(crate)` in
  `render.rs` (keep with its renderer siblings even though single-caller, for thematic consistency).
- **`agent_in_list_scope`/`chain_in_list_scope`** called only from `handle_list`'s own body — do
  **not** extract into `lookup.rs` or `render.rs`; move them into `handlers.rs` directly, private,
  co-located with their sole caller.
- **`pick_scope_dir`** called from `handlers.rs` (`handle_create` 2860) **and** `tier_actions.rs`
  (`handle_eject` 3388) — shared, `pub(crate)`, lives in `helpers.rs`.
- **`editable_base`** called only from `handle_update`'s own body — private, co-located in
  `handlers.rs`, not extracted anywhere else.
- **`action_scope`/`resolve_effective_agent`/`find_bundled`/`writable_tier`/`scope_settings_path`/
  `with_settings_reread`** are each called only from within the tier-actions cluster itself
  (`handle_eject`/`handle_disable`/`handle_enable`/`handle_reset`, lines 3348–3675) — **fully
  self-contained**, all stay private within `tier_actions.rs`, zero visibility bumps needed for this
  whole sub-cluster.
- **`AgentFields`** (struct) is constructed in `handlers.rs` (`handle_create`, `handle_update`) and
  consumed by `create_agent`/`update_agent`/`build_definition`/`merge_fields` (`agent_crud.rs`),
  `preserved_frontmatter_fields` (`frontmatter_write.rs`), and `apply_agent_config`
  (`config_parse.rs`) — already `pub`, no bump needed, but it is genuinely a 4-file shared type.
  Define it in `agent_crud.rs` (its conceptual home — the delta CRUD operates on) and re-export via
  `management/mod.rs`'s `pub use agent_crud::AgentFields;` so every consumer can
  `use crate::discovery::management::AgentFields;` uniformly rather than reaching into a sibling
  submodule's path directly.
- **`AgentMutationOutcome`** is self-contained to `agent_crud.rs` (constructed and consumed only by
  `create_agent`/`update_agent`/`rename_agent`) but its return type is part of these functions' `pub`
  signatures, so it must be reachable wherever those are called from (`handlers.rs`, via
  `handle_create`/`handle_update`/`handle_delete` — actually verify: does `handlers.rs` ever bind the
  `AgentMutationOutcome` value by name, or only call `create_agent(...)?` and use `.file_path`
  through the `Some(created)` pattern? If only field-accessed through a binding, `handlers.rs` still
  needs the TYPE in scope to pattern-match `Some(created)` — import it via the same
  `management/mod.rs` re-export as `AgentFields`).
- **`ChainFields`** is fully self-contained to `chain_crud.rs` (only referenced in `update_chain`'s
  signature and its own doc comment) — no cross-file use, no re-export needed.

## Target layout — create these files under `discovery/management/`

Convert `discovery/management.rs` to `discovery/management/mod.rs` plus the leaf files below.
Declare nothing new in `discovery/mod.rs` — its existing `pub mod management;` resolves to the
directory transparently. Every new leaf file is declared `mod <name>;` (private — none of these
need to be reachable as `crate::discovery::management::<file>::<item>` from OUTSIDE this crate's
`discovery/management/` directory, per the "External API surface" section above; use `pub(crate)`
on the individual items that cross a file boundary WITHIN `management/`, not `pub mod` on the file
itself). This differs from the `exec/` split's `pub mod` choice — `exec/`'s leaf modules needed
`pub` reachability because callers elsewhere in the crate address them by qualified path
(`crate::exec::attempt_runner::...`); nothing outside `discovery/management/` needs to address a
leaf module by qualified path here, only the flat re-exported names, so `mod` (crate-private to the
`management` module, meaning accessible only via `management::mod.rs`'s own re-exports) is the
tighter and correct choice. If you find a genuine need for external qualified-path access while
executing, switch that one declaration to `pub(crate) mod`, not the whole set.

1. **`discovery/management/visibility.rs`** (~80 lines) — `AgentVisibility` + `impl`,
   `ChainVisibility` + `impl`, `require_writable_source` (bump to `pub(crate)`).
   *Rationale*: exactly R-SA-013/014's own banner scope; already self-contained except for
   `require_writable_source`'s two CRUD-file callers.
2. **`discovery/management/agent_crud.rs`** (~440 lines) — `AgentFields`, `AgentMutationOutcome`,
   `create_agent`/`update_agent`/`delete_agent`/`rename_agent`, `agent_file_path` (bump to
   `pub(crate)`, or leave in `frontmatter_write.rs` — see the call-graph note above, pick one),
   `build_definition`, `merge_fields`, `reparse_agent_file` (needs
   `crate::discovery::frontmatter::parse_agent_file`).
   *Rationale*: the agent-mutation primitives, symmetric with `chain_crud.rs`.
3. **`discovery/management/frontmatter_write.rs`** (~380 lines) — `write_agent_file` (bump to
   `pub(crate)`), `serialize_agent`, `preserved_frontmatter_fields` (bump to `pub(crate)`),
   `tool_ref_to_frontmatter_entry`.
   *Rationale*: the "write-back" concern is a faithful port of a specific upstream serialization
   contract (the file's own banner cites it), genuinely distinct from "decide what the new/updated
   definition's fields should be" (`agent_crud.rs`).
4. **`discovery/management/chain_crud.rs`** (~280 lines) — `ChainFields`, `placeholder_chain_step`,
   `create_chain`/`update_chain`/`delete_chain`/`rename_chain` (bare primitives, tested but no
   production caller — move as-is), `create_chain_with_steps`/`update_chain_full` (the composed
   versions handlers actually call), `chain_file_path`, `write_chain_file`, `serialize_chain_json`.
   *Rationale*: symmetric with `agent_crud.rs`; entirely self-contained, zero `pub(crate)` bumps
   needed anywhere in this file.
5. **`discovery/management/helpers.rs`** (~180 lines) — `normalize_package_identifier` (bump to
   `pub(crate)`), `source_str`, `context_str`, `override_scope_str`, `disambiguation_scope`,
   `normalize_list_scope`, `sanitize_name`, `pick_scope_dir` (all bump to `pub(crate)`).
   *Rationale*: the crate-wide-shared small helpers — every one of these has at least two callers
   across different new files. Deliberately excludes `parse_csv`/`parse_tools` (move to
   `config_parse.rs` instead, per the call-graph deviation noted above) and
   `default_system_prompt_mode`/`default_inherit_project_context` (move to `handlers.rs`, same
   reason).
6. **`discovery/management/config_parse.rs`** (~400 lines) — `config_object`, `parse_package_config`,
   `apply_agent_config` (234 lines — the largest single function in this file after the handlers),
   `parse_step_list`, `parse_csv`, `parse_tools` (all four public-facing ones bump to `pub(crate)`;
   `parse_csv`/`parse_tools` stay private, same-file callers only).
   *Rationale*: "parse a caller-supplied JSON config blob into a typed delta" — a pure,
   validation-heavy concern with zero I/O, genuinely separable from CRUD orchestration even though
   today only `handlers.rs` calls into it.
7. **`discovery/management/lookup.rs`** (~300 lines) — `find_agents`, `distinct_agent_names`,
   `find_chains`, `available_agent_names`, `available_chain_names`, `name_exists_in_scope`,
   `unknown_chain_agents`, `MutableTarget` trait + 2 impls, `TargetKind` enum + impl,
   `resolve_target` (all bump to `pub(crate)` except the impls, which follow their trait/type).
   *Rationale*: "given a discovery snapshot, find/disambiguate/validate a name" — the file's own
   banner already groups these exactly this way.
8. **`discovery/management/render.rs`** (~250 lines) — `format_agent_detail`, `format_chain_detail`
   (both bump to `pub(crate)`), `format_chain_step_detail` (private, sole caller same file),
   `format_model_source` (bump to `pub(crate)`).
   *Rationale*: pure `&T -> String` rendering, zero mutation, zero discovery I/O — a distinct
   concern from both "find the target" (`lookup.rs`) and "act on the target" (`handlers.rs`).
9. **`discovery/management/handlers.rs`** (~730 lines, the largest leaf) — `agent_in_list_scope`,
   `chain_in_list_scope`, `default_system_prompt_mode`, `default_inherit_project_context`,
   `editable_base` (all four private, co-located per the call-graph notes above),
   `handle_list`/`handle_get`/`handle_models`/`handle_create`/`handle_update`/`handle_delete`.
   *Rationale*: the six non-tier-aware management actions — matches the file's own
   "handleList/handleGet/handleModels/handleCreate/handleUpdate/handleDelete" banner exactly. This
   is deliberately the largest leaf file (730 lines vs the ~250–440 range of the others) because
   splitting the six handlers further would fragment a single cohesive orchestration layer without
   a clean sub-seam — six independently-named functions in one file, each already an obvious
   single-responsibility unit at the function level, is a reasonable stopping point (compare:
   `exec/mod.rs`'s split left `run_sync` as an 829-line single function at the root rather than
   force-splitting it further, for the same reason — no clean internal seam).
10. **`discovery/management/tier_actions.rs`** (~480 lines) — `action_scope`,
    `resolve_effective_agent`, `find_bundled`, `writable_tier`, `scope_settings_path`,
    `with_settings_reread` (all six private, fully self-contained), `handle_eject`/`handle_disable`/
    `handle_enable`/`handle_reset`.
    *Rationale*: SUBA-005's own banner already separates this from the six above — the tier-aware /
    settings-writing four actions are a genuinely distinct concern (they read/write
    `subagents.agentOverrides` settings layers, not agent/chain files directly) with their own
    six-function private helper cluster that never leaks outside this file.
11. **`discovery/management/mod.rs` (slimmed root, ~160 lines before tests)** — keeps: module doc
    comment, the full original `use` block (each new leaf file additionally imports its own subset
    directly rather than relying on glob-reexport from the root — see note below),
    `pub use visibility::{AgentVisibility, ChainVisibility};` (preserves `discovery/mod.rs`'s
    existing `use management::{AgentVisibility, ChainVisibility};`), `pub use agent_crud::{AgentFields,
    AgentMutationOutcome};` (preserves the 4-file shared-type reachability noted above),
    `BUILTIN_AGENT_NAMES`, `ManagementRequest`, `ProactiveSkillsInput`, `ManagementOutcome` + impl,
    `MANAGEMENT_ACTIONS`, `MUTATING_MANAGEMENT_ACTIONS`, `handle_management_action` (the dispatcher —
    imports `handle_list`/`handle_get`/.../`handle_reset` from `handlers`/`tier_actions` as
    `pub(crate)` items).
    *Rationale*: mirrors `exec/mod.rs`'s own root-keeps-the-dispatcher pattern. The module doc
    comment (lines 1–46) already self-describes this file as owning three concerns
    (read-only-source rejection, call-site-dependent visibility, on-demand re-scanned semantics) —
    keep that claim true by keeping the types/consts/dispatcher that anchor those claims at the
    root, with every concrete implementation delegated to a leaf.

This yields 10 new leaf files plus a root that drops from 5,515 to roughly 160 lines of production
code (before its own trimmed test module, if any tests end up staying at the root — see triage
below; likely none do, since `handle_management_action` itself has no dedicated direct-call tests
distinct from the six/four handler-specific ones).

## Shared test fixtures — `discovery/management/test_support.rs`

The flat test module has 6 fixture functions shared across many of the 72 tests, at drastically
different usage counts (verified by grep across the whole test block):

| Fixture | Uses | Needed by |
|---|---|---|
| `mgmt_cfg(tmp: &Path) -> AgentDiscoveryConfig` | 30 | almost every handler test |
| `sample_agent(source, file_path) -> AgentDefinition` | 16 | agent CRUD + visibility tests |
| `agent_named(name, disabled) -> AgentDefinition` | 12 | visibility tests |
| `write_agent_md(dir, file, body)` | 12 | frontmatter round-trip tests |
| `sample_chain(source, file_path) -> ChainDefinition` | 9 | chain CRUD tests |
| `seed_two_agents_sharing_a_skill(cfg) -> Vec<AvailableSkill>` | 5 | `handle_list` proactive-skills tests |

Create `discovery/management/test_support.rs`, gated `#[cfg(test)] pub(crate) mod test_support` (or
a plain `#[cfg(test)]` file per the crate's existing convention — check how
`exec/testsupport.rs`/`exec/acceptance/{model,lattice}/testsupport.rs` are declared and match that
exact pattern, including whether they're `#[cfg(test)]`-gated at the `mod` declaration in
`management/mod.rs` or gated inside the file itself), housing all six. Every new leaf file's own
`#[cfg(test)] mod tests { use super::*; ... }` additionally does
`use super::super::test_support::*;` (or whatever exact re-export shape the existing
`exec/testsupport.rs` convention uses) to reach these.

## Test triage (3676–5515, 1,840 lines, one flat `mod tests { use super::*; }`, 72 `#[test]` fns)

No internal `mod` nesting exists today. Route each test to the file owning the production item it
calls DIRECTLY (not transitively) — verify via grep before moving, the same discipline the
call-graph section above used:

- `create_agent_rejects_{builtin,package}_source`, `create_agent_succeeds_for_{user,project}_source`,
  `update_agent_rejects_{builtin,package}_source`, `update_agent_succeeds_for_project_source`,
  `delete_agent_rejects_{builtin,package}_source`, `delete_agent_succeeds_for_user_source`,
  `rename_agent_rejects_{builtin,package}_source`, `rename_agent_succeeds_for_user_source`,
  `create_agent_fails_when_file_already_exists`, `rename_agent_fails_when_destination_already_exists`,
  `create_agent_with_{invalid,valid}_package_identifier_*`, `update_agent_with_invalid_package_identifier_*`,
  `create_agent_with_absent_package_field_is_not_skipped` → call `create_agent`/`update_agent`/
  `delete_agent`/`rename_agent` directly → **`agent_crud.rs`** (even the ones testing R-SA-014's
  read-only-source guard or package-identifier normalization — they call the CRUD function, not
  `require_writable_source`/`normalize_package_identifier` directly, so per the "moves with its
  direct caller" rule they belong here, not `visibility.rs`/`helpers.rs`).
- `create_chain_rejects_{builtin,package}_source`, `create_chain_succeeds_for_user_and_project_sources`,
  `update_chain_rejects_builtin_and_package_sources`, `delete_chain_rejects_builtin_and_package_sources`,
  `rename_chain_rejects_builtin_and_package_sources`, `delete_and_rename_chain_succeed_for_project_source`
  → call `create_chain`/`update_chain`/`delete_chain`/`rename_chain` directly → **`chain_crud.rs`**.
- `management_view_includes_disabled_agents`, `delegation_view_excludes_disabled_agents`,
  `list_view_excludes_disabled_agents_independently_of_delegation`,
  `three_visibility_views_diverge_exactly_on_disabled_agents`,
  `chain_visibility_views_are_all_unfiltered_passthroughs` → call `AgentVisibility::*`/
  `ChainVisibility::*` directly → **`visibility.rs`** (uses the `agent_named` fixture).
- `normalize_package_identifier_matches_frontmatter_rs_validation_fixtures` → calls
  `normalize_package_identifier` directly → **`helpers.rs`**.
- `created_agent_round_trips_through_discovery_parser`, `serialize_agent_round_trips_*` (4 variants:
  memory/tool budget, turn budget launch default, async/timeout launch defaults, aliases),
  `serialize_agent_round_trip_preserves_unknown_keys_including_block_values`,
  `update_preserves_unknown_frontmatter_key_and_explicit_off_on_disk` → verify each individually,
  but names strongly suggest `serialize_agent`/`write_agent_file` direct calls →
  **`frontmatter_write.rs`** (uses `write_agent_md` fixture).
- `resolve_target_rejects_package_source_with_read_only_message` → calls `resolve_target` directly
  → **`lookup.rs`**.
- `the_extension_config_bridge_preserves_disable_and_the_tuning_knobs`,
  `proactive_config_rejects_non_positive_min_and_max_like_pis_positive_integer` → names suggest
  `apply_agent_config` direct calls (config-parsing behavior) → verify and likely **`config_parse.rs`**.
- `a_settings_defaulted_extension_list_is_never_baked_into_the_agent_file` → verify which function it
  calls directly (candidates: `apply_agent_config` → `config_parse.rs`, or `serialize_agent` →
  `frontmatter_write.rs`) before placing.
- Any remaining tests not named above (the flat block has 72 total; roughly 45 are named/grouped
  explicitly above) — grep each remaining `#[test] fn` name, find what it calls directly, place
  accordingly. Expect most of the untriaged remainder to be `handle_list`/`handle_get`/
  `handle_models`/`handle_create`/`handle_update`/`handle_delete`/`handle_eject`/`handle_disable`/
  `handle_enable`/`handle_reset` integration-style tests (the handler functions are the ones actually
  exercised end-to-end by the bulk of a 72-test suite covering 6+4 dispatch actions) → these go to
  **`handlers.rs`**/**`tier_actions.rs`** respectively, matching which handler each test's own name
  or body references.

Each new file's test module keeps the crate-standard shape (`#[cfg(test)] mod tests { use
super::*; ... }`), copying the existing `#![allow(clippy::unwrap_used, clippy::expect_used,
clippy::indexing_slicing)]` attribute (line 3677 today) onto every new test module that needs it.

## Visibility summary (everything that must change)

| Item | Current | Required |
|---|---|---|
| `require_writable_source` | private `fn` | `pub(crate) fn` |
| `normalize_package_identifier` | private `fn` | `pub(crate) fn` |
| `write_agent_file` | private `fn` | `pub(crate) fn` |
| `preserved_frontmatter_fields` | private `fn` | `pub(crate) fn` |
| `agent_file_path` | private `fn` | `pub(crate) fn` |
| `source_str`, `context_str`, `override_scope_str`, `disambiguation_scope`, `normalize_list_scope`, `sanitize_name`, `pick_scope_dir` | private `fn` | `pub(crate) fn` |
| `config_object`, `parse_package_config`, `apply_agent_config`, `parse_step_list` | private `fn` | `pub(crate) fn` |
| `find_agents`, `distinct_agent_names`, `find_chains`, `available_agent_names`, `available_chain_names`, `name_exists_in_scope`, `unknown_chain_agents` | private `fn` | `pub(crate) fn` |
| `MutableTarget` (trait), `TargetKind` (enum), `resolve_target` | private | `pub(crate)` |
| `format_agent_detail`, `format_chain_detail`, `format_model_source` | private `fn` | `pub(crate) fn` |
| `build_definition`, `merge_fields`, `reparse_agent_file`, `serialize_agent`, `tool_ref_to_frontmatter_entry`, `chain_file_path`, `write_chain_file`, `serialize_chain_json`, `placeholder_chain_step`, `parse_csv`, `parse_tools`, `default_system_prompt_mode`, `default_inherit_project_context`, `format_chain_step_detail`, `agent_in_list_scope`, `chain_in_list_scope`, `editable_base`, `action_scope`, `resolve_effective_agent`, `find_bundled`, `writable_tier`, `scope_settings_path`, `with_settings_reread` | private | **no change** — each moves with its sole caller, stays private |
| everything already `pub` (`AgentVisibility`, `ChainVisibility`, `AgentFields`, `AgentMutationOutcome`, `create_agent`/`update_agent`/`delete_agent`/`rename_agent`, `ChainFields`, `create_chain`/`update_chain`/`delete_chain`/`rename_chain`/`create_chain_with_steps`/`update_chain_full`, `BUILTIN_AGENT_NAMES`, `ManagementRequest`, `ProactiveSkillsInput`, `ManagementOutcome`, `MANAGEMENT_ACTIONS`, `MUTATING_MANAGEMENT_ACTIONS`, `handle_management_action`) | `pub` | **no change**, just re-export (`AgentFields`/`AgentMutationOutcome`/`AgentVisibility`/`ChainVisibility` via `management/mod.rs`) or `use crate::discovery::management::<name>::...` at each new call site |
| all `super::X` references | `super::X` (meaning `discovery::X`) | `crate::discovery::X` in every file EXCEPT `management/mod.rs` itself (see the gotcha table above — this is the single most likely source of a silent wrong-resolution bug if missed) |

## Execution order (DO THESE STEPS — this is the actual task)

Extract leaves before roots, so at every intermediate step `cargo check -p cyrup-ext-subagents`
passes. Re-grep line numbers before each step — the file shrinks as you go and stale line numbers
will point at the wrong code. Fix every `super::` reference per the gotcha table AS you move each
block, not as a separate pass at the end — it is much easier to get right file-by-file than to
audit 3,675 lines of already-moved code afterward.

1. Create `discovery/management/visibility.rs` — move `AgentVisibility`+impl, `ChainVisibility`+impl,
   `require_writable_source` (bump `pub(crate)`), and their tests (the 5 visibility tests +
   `agent_named` fixture reference, though the fixture itself moves to `test_support.rs`). Convert
   `discovery/management.rs` → `discovery/management/mod.rs` in this same step (create the
   directory). Declare `mod visibility;` + `pub use visibility::{AgentVisibility, ChainVisibility};`
   in the new `mod.rs`. `cargo check`.
2. Create `discovery/management/test_support.rs` — move the 6 shared test fixtures, gated per the
   crate's existing `testsupport.rs` convention. Declare `mod test_support;` (test-gated) in
   `mod.rs`.
3. Create `discovery/management/chain_crud.rs` — move `ChainFields`, `placeholder_chain_step`,
   `create_chain`/`update_chain`/`delete_chain`/`rename_chain`/`create_chain_with_steps`/
   `update_chain_full`, `chain_file_path`, `write_chain_file`, `serialize_chain_json`, and their
   tests. Fully self-contained, zero new `pub(crate)` needed here. Declare `mod chain_crud;`.
   `cargo check`.
4. Create `discovery/management/frontmatter_write.rs` — move `write_agent_file` (bump `pub(crate)`),
   `serialize_agent`, `preserved_frontmatter_fields` (bump `pub(crate)`),
   `tool_ref_to_frontmatter_entry`, and their tests (the `serialize_agent_round_trips_*` cluster +
   `write_agent_md` fixture usage). Declare `mod frontmatter_write;`. `cargo check`.
5. Create `discovery/management/agent_crud.rs` — move `AgentFields`, `AgentMutationOutcome`,
   `create_agent`/`update_agent`/`delete_agent`/`rename_agent`, `agent_file_path` (bump `pub(crate)`
   — or leave in step 4's file, pick one and be consistent), `build_definition`, `merge_fields`,
   `reparse_agent_file` (needs `crate::discovery::frontmatter::parse_agent_file`), and their tests.
   Import `write_agent_file`/`preserved_frontmatter_fields` from `frontmatter_write`. Declare
   `mod agent_crud;` + `pub use agent_crud::{AgentFields, AgentMutationOutcome};` in `mod.rs`.
   `cargo check`.
6. Create `discovery/management/helpers.rs` — move `normalize_package_identifier` (bump
   `pub(crate)`, needs `crate::discovery::package_name::{collapse_repeated_char,
   is_valid_package_identifier}`), `source_str`, `context_str`, `override_scope_str`,
   `disambiguation_scope`, `normalize_list_scope`, `sanitize_name`, `pick_scope_dir` (all bump
   `pub(crate)`), and the one test that calls `normalize_package_identifier` directly. Declare
   `mod helpers;`. `cargo check`.
7. Create `discovery/management/config_parse.rs` — move `config_object`, `parse_package_config`,
   `apply_agent_config`, `parse_step_list`, `parse_csv`, `parse_tools` (first four bump
   `pub(crate)`), and their tests (the `apply_agent_config`-exercising ones — verify each of the
   "extension_config_bridge"/"proactive_config"/"settings_defaulted_extension_list" candidates
   individually). Declare `mod config_parse;`. `cargo check`.
8. Create `discovery/management/lookup.rs` — move `find_agents`, `distinct_agent_names`,
   `find_chains`, `available_agent_names`, `available_chain_names`, `name_exists_in_scope`,
   `unknown_chain_agents`, `MutableTarget`+2 impls, `TargetKind`+impl, `resolve_target` (all bump
   `pub(crate)`), and the `resolve_target_rejects_package_source_with_read_only_message` test.
   Declare `mod lookup;`. `cargo check`.
9. Create `discovery/management/render.rs` — move `format_agent_detail`, `format_chain_step_detail`,
   `format_chain_detail`, `format_model_source` (bump the three cross-file ones `pub(crate)`).
   Declare `mod render;`. `cargo check`.
10. Create `discovery/management/handlers.rs` — move `agent_in_list_scope`, `chain_in_list_scope`,
    `default_system_prompt_mode`, `default_inherit_project_context`, `editable_base` (all private,
    co-located), `handle_list`/`handle_get`/`handle_models`/`handle_create`/`handle_update`/
    `handle_delete`, and their (majority of the suite's) tests. Import from `visibility`,
    `agent_crud`, `frontmatter_write` (transitively via `agent_crud`'s re-exports where applicable),
    `helpers`, `config_parse`, `lookup`, `render` as needed. Declare `mod handlers;`. `cargo check`.
11. Create `discovery/management/tier_actions.rs` — move `action_scope`, `resolve_effective_agent`,
    `find_bundled`, `writable_tier`, `scope_settings_path`, `with_settings_reread` (all private,
    self-contained), `handle_eject`/`handle_disable`/`handle_enable`/`handle_reset`, and their
    tests. Needs `crate::discovery::{merge::TieredAgents, load_layered_override_settings,
    scan_agent_tiers, settings_write::{merge_builtin_agent_override,
    remove_builtin_agent_override_fields, remove_builtin_agent_override}}` plus `available_agent_names`/
    `name_exists_in_scope` from `lookup`, `agent_file_path` from wherever it landed, `pick_scope_dir`/
    `sanitize_name`/`source_str`/`override_scope_str` from `helpers`. Declare `mod tier_actions;`.
    `cargo check`.
12. In `management/mod.rs`: confirm what remains is the module doc comment, the trimmed `use` block,
    `BUILTIN_AGENT_NAMES`, `ManagementRequest`, `ProactiveSkillsInput`, `ManagementOutcome`+impl,
    `MANAGEMENT_ACTIONS`, `MUTATING_MANAGEMENT_ACTIONS`, `handle_management_action` (updated to call
    `handlers::handle_list`/etc. and `tier_actions::handle_eject`/etc., or import them by name at
    the top and call bare per the file's existing style), and the two re-export lines from steps 1
    and 5. Confirm no leftover test module at the root, or a legitimately-empty one if every test
    triaged into a leaf (expected outcome per the triage section above).
13. Grep every moved doc comment for `[`management::...`]`/`[`super::...`]`/bare unqualified
    intra-doc links and fix the path per the gotcha table — this crate has
    `[workspace.lints.rustdoc] broken_intra_doc_links = "deny"`, so a missed one is a hard
    `cargo doc` build failure, not a warning (see the recently-landed `BROKEN_INTRA_DOC_LINKS` fix
    in this crate's own history for exactly this failure mode and its resolution pattern —
    `crate::discovery::management::<item>` style, matching the rest of this task's `crate::` prefix
    convention).
14. Run `cargo check -p cyrup-ext-subagents`, `cargo test -p cyrup-ext-subagents --lib` (must show
    the same 2483+ test count as before this split — 0 dropped, 0 newly ignored, adjusted only by
    however many net-new/net-removed tests this task itself introduces, which should be zero),
    `cargo clippy -p cyrup-ext-subagents --all-targets` (only the two pre-existing findings —
    `exec/spawn_plan.rs` too-many-arguments, `extension/tool/text.rs` doc_lazy_continuation — should
    remain), and `cargo doc -p cyrup-ext-subagents --no-deps --lib` (must exit 0, per step 13).

## Definition of done

- [ ] `discovery/management.rs` no longer exists as a flat file; `discovery/management/mod.rs`
      exists at roughly 160 lines of production code (verify with `wc -l`).
- [ ] `discovery/management/{visibility,agent_crud,frontmatter_write,chain_crud,helpers,
      config_parse,lookup,render,handlers,tier_actions,test_support}.rs` all exist, each containing
      the items listed above plus their triaged tests.
- [ ] Every `super::X` reference that moved into a leaf file became `crate::discovery::X` (grep
      `super::` in every new leaf file — any hit that isn't a same-file-relative reference to
      something also defined in that leaf, or `super::*`/`super::super::` intentionally, is a bug).
- [ ] `crate::discovery::management::{BUILTIN_AGENT_NAMES, MANAGEMENT_ACTIONS,
      MUTATING_MANAGEMENT_ACTIONS, ManagementRequest, ProactiveSkillsInput, ManagementOutcome,
      handle_management_action}` all still resolve exactly as before (this crate's external callers
      — `extension/tool/routing.rs` etc. — are NOT edited by this task; if any of them fail to
      compile, a re-export is missing).
- [ ] `discovery/mod.rs`'s existing `use management::{AgentVisibility, ChainVisibility};` (line 86)
      and its `[`management::AgentVisibility::...`]`-style doc links resolve unchanged, with zero
      edits to `discovery/mod.rs` itself.
- [ ] Every doc comment moved verbatim; every intra-doc link updated to its item's new module path.
- [ ] `cargo check -p cyrup-ext-subagents` is clean.
- [ ] `cargo test -p cyrup-ext-subagents --lib` passes (no test dropped, none newly ignored, same
      count as the branch's starting baseline).
- [ ] `cargo clippy -p cyrup-ext-subagents --all-targets` shows only the two pre-existing findings.
- [ ] `cargo doc -p cyrup-ext-subagents --no-deps --lib` exits 0 (this crate pins
      `broken_intra_doc_links = "deny"` — verify explicitly, don't assume `cargo check` passing
      covers this).
- [ ] `cargo check --workspace --all-targets` is clean (catches any cross-crate breakage, though
      none is expected since no external-facing symbol changes path).

## Source

- Requested directly by the user: "execute a logical decomposition of cyrup-ext-subagents
  src/discovery/management.rs based on separation of concerns into submodules".
- Full call graph traced by `grep -n` against `discovery/management.rs` at branch
  `claude/decompose-discovery-management`, base commit `6d805fe` (post-PR-#66 `main`).
- Format and rigor modeled on the prior landed `EXEC_MOD_DECOMPOSITION.md`
  (`.flux/done/2026-08-23-16-32/`), which used the same call-graph-over-banner-text methodology
  for `exec/mod.rs`'s decomposition in this same crate.
