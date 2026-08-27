---
stage: qa
status: completed
updated: 2026-08-23 01:10
---

# Extend The Pi Citation Lint To The Whole cyrup-ext-sdk Src Tree And Fix The 11 Sites It Turns Red

**Severity:** high · **Effort:** M · **Crate:** `crates/cyrup-ext-sdk`

## What is wrong

The repo's pi-citation lint lives in `crates/cyrup-ext/src/tests/wit_world_sync.rs`. Its file set, `cited_files()` at **wit_world_sync.rs:218-245**, names exactly one cyrup-ext-sdk `.rs` file by hand — `../cyrup-ext-sdk/src/api.rs` (line 223) — plus a `read_dir` enumeration of `../cyrup-ext-sdk/src/ctx` (lines 234-243). Every other SDK source file is invisible to both lint bodies:

- `no_struck_pi_citation_is_restored_as_a_live_citation` (wit_world_sync.rs:289-313)
- `every_subscribed_at_citation_names_the_event_pi_subscribes_on_that_line` (wit_world_sync.rs:368-415)

Those unscanned files carry the crate's densest citation surface. Reproduce:

```
rg -c -e 'types\.ts:' -e 'loader\.ts:' -e 'runner\.ts:' -e '@v0\.83\.0' crates/cyrup-ext-sdk/src
```

events.rs 69, descriptor.rs 31, example.rs 27, guest.rs 10, provider.rs 6, macros.rs 5, tests/payload_fidelity.rs 13, widget.rs 2, autocomplete.rs 2, tool_factory.rs 2, lib.rs 1. `events.rs` alone (69) carries more citations than `api.rs` (45), the one SDK file that is pinned.

## Evidence: 11 live violations, suite green

Re-running the lint's own tables — `STRUCK_CITATIONS` (wit_world_sync.rs:255-283), `CORRECTIVE_MARKERS` (:249-251), `PI_EVENT_SUBSCRIPTION_LINES` (:326-366) — over the uncovered files yields 11 sites that would go RED, none carrying a corrective marker, while `cargo test -p cyrup-ext` passes 293.

Struck citations (10):

| site | struck value | what that pi line actually is |
|---|---|---|
| `src/events.rs:47` (`context`) | `types.ts:1144` | a closing brace; `context` subscribes at `:1207` |
| `src/events.rs:53` (`message_end`) | `types.ts:1143` | `expanded: boolean`; subscribes at `:1222` |
| `src/events.rs:59` (`before_agent_start`) | `types.ts:1135` | blank line; subscribes at `:1214` |
| `src/events.rs:100` (`before_provider_request`) | `types.ts:1160` | a banner rule; subscribes at `:1209` |
| `src/events.rs:106` (`after_provider_response`) | `types.ts:1161` | blank line; subscribes at `:1213` |
| `src/descriptor.rs:169` | `types.ts:1105-1111` | closing brace; `registerCommand` is `:1247` |
| `src/provider.rs:2` | `types.ts:1373-1392` | an `@example` JSDoc line; `registerProvider(name, config)` is `:1401` |
| `src/provider.rs:229` | `types.ts:1373` | same |
| `src/autocomplete.rs:1` | `types.ts:218` | `getEditorText` doc line; `addAutocompleteProvider` is `:225` |
| `src/autocomplete.rs:2` | `types.ts:117` | a `WorkingIndicatorOptions` doc line; `AutocompleteProviderFactory` is `:124` |

Wrong-event citation (1) — a **regression of a defect the lint was written to kill**: `crates/cyrup-ext-sdk/src/events.rs:224-225` documents `session_info_changed` and ends ``subscribed at `:1203`) — EXT-011.`` `PI_EVENT_SUBSCRIPTION_LINES` maps 1203 → `session_compact` and `session_info_changed` → 1193. The lint's own docstring (wit_world_sync.rs:330-331, 355-358) names this exact defect and lists the sites that were fixed; `rg -n 'subscribed at' crates/cyrup-ext-sdk/src/api.rs crates/cyrup-ext/src/event.rs crates/cyrup-ext-sdk/wit/world.wit crates/cyrup-ext/wit/world.wit` shows api.rs:60, event.rs:61 and both world.wit:423 now read ``:1193` — EXT-073: `:1203` is `session_compact``. The fourth site, SDK `events.rs:225`, still reads `:1203` solely because it is not in `cited_files()`.

Reproduce the file-set gap: `grep -n 'cyrup-ext-sdk' crates/cyrup-ext/src/tests/wit_world_sync.rs` (only world.wit, api.rs and src/ctx are named).
Reproduce the live struck values: `grep -n 'types.ts:1144\|types.ts:1143\|types.ts:1135\|types.ts:1160\|types.ts:1161\|types.ts:1203' crates/cyrup-ext-sdk/src/events.rs`.

## Why it matters

The repo's stated position (wit_world_sync.rs:196-213) is that a pi citation IS the evidence a port matches upstream, so a citation resolving to an unrelated-but-plausible line is worth less than none, and the guard exists to end the "fixed in the .rs sites and left standing elsewhere for two more sweeps" pattern. events.rs:225 proves the leak is not hypothetical. This is not covered by `.flux/todo/CARGO_DOC_WARNINGS.md` (rustdoc intra-doc links, a different class).

## Fix

1. In `cited_files()` (wit_world_sync.rs:218-245) replace the hardcoded `crate_dir.join("../cyrup-ext-sdk/src/api.rs")` and the `src/ctx` special case with one recursive walk of `../cyrup-ext-sdk/src` collecting every `*.rs`. Keep the existing non-vacuity assert and raise it to a count floor, the way `crates/cyrup-ext-sdk/src/tests/world_import_coverage.rs` asserts `scanned >= 13`. The rationale comment already in the file for enumerating `ctx/` — "so a later submodule cannot fall outside this lint by being added and not listed here" — is the same argument one level up.
2. Correct the 11 sites: events.rs:47 `:1144`→`:1207`, :53 `:1143`→`:1222`, :59 `:1135`→`:1214`, :100 `:1160`→`:1209`, :106 `:1161`→`:1213`, :225 `:1203`→`:1193`; descriptor.rs:169 `:1105-1111`→`:1247`; provider.rs:2 and :229 `:1373`→`:1401`; autocomplete.rs:1 `:218`→`:225`, :2 `:117`→`:124`.
3. Where a struck value must be quoted deliberately, follow the `api.rs:60-61` pattern and name the striking id on the same line (e.g. ``EXT-073: `:1203` is `session_compact```) so `CORRECTIVE_MARKERS` whitelists it.

All 11 edits are in doc comments and touch no literal `module::name(` WIT call path, so `src/tests/world_import_coverage.rs` is unaffected.

## Acceptance Criteria

- [ ] `grep -n 'cyrup-ext-sdk' crates/cyrup-ext/src/tests/wit_world_sync.rs` shows a recursive walk of `../cyrup-ext-sdk/src` and no hand-named `src/api.rs` entry
- [ ] `cargo test -p cyrup-ext` passes with at least 293 tests, with the widened file set in effect
- [ ] `grep -n 'types.ts:1144\|types.ts:1143\|types.ts:1135\|types.ts:1160\|types.ts:1161' crates/cyrup-ext-sdk/src/events.rs` returns nothing (or only lines that also contain an `EXT-` corrective marker)
- [ ] `grep -n 'subscribed at' crates/cyrup-ext-sdk/src/events.rs` shows `:1193` for `session_info_changed`, matching `crates/cyrup-ext-sdk/src/api.rs:60`
- [ ] `grep -n 'types.ts:1105-1111' crates/cyrup-ext-sdk/src/descriptor.rs`, `grep -n 'types.ts:1373' crates/cyrup-ext-sdk/src/provider.rs`, `grep -n 'types.ts:218\|types.ts:117' crates/cyrup-ext-sdk/src/autocomplete.rs` all return nothing without a corrective marker
- [ ] `cited_files()` retains a non-vacuity assertion with a numeric floor (not just `!files.is_empty()`)
- [ ] `cargo test -p cyrup-ext-sdk` still passes 17 tests (world_import_coverage unaffected)
