---
stage: qa
status: completed
updated: 2026-08-29 02:14
severity: low
effort: trivial
subsystem: discovery / settings merge
source: docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md
item: SUBA-081
---

> **Path note.** This task lives in a subdirectory. Every flux command globs a single level
> (`ls -1 "$FLUX_BASE/todo/"*.md`), so `/exec`, `/aug` and `/qa` will not list it — pass the
> absolute path explicitly.

# SUBA-081 — rework round 2 (citations only)

## QA verdict: 9/10

**Every behavioural and structural item from round 1 is complete and verified. None of it needs
touching again.** What remains is mechanical: thirteen upstream line numbers, all added by this
task, are off by one or two.

Verified done — do not revisit:

- **`description: false` is a no-op on both paths.** `merge.rs:406` and `merge.rs:631` are now
  behaviourally identical (`if let OverrideField::Value(v)`), and the custom path correctly leaves
  `applied_any` false so a no-op records no provenance. Confirmed the two paths are genuinely
  distinct code: `apply_overrides` dispatches `Builtin -> apply_builtin_agent` and
  `User | Project -> apply_custom_agent` (`merge.rs:200-215`), so the test's two iterations exercise
  different functions.
- **The test is honest.** Building the delta from `serde_json::json!({"description": false})` rather
  than the variant pins the deserialize step, and asserting `ExplicitClear` first means the apply
  assertions cannot silently degrade into testing nothing.
- **Both doc claims check out against the code.** `SystemPromptMode` derives `Deserialize` with no
  custom impl, so the "`Value` arm rejects a bool" claim holds for it as well as for `String`. The
  bullet's split by `T` is accurate, and it correctly declines to claim `systemPrompt`/
  `systemPromptMode` behaviour was changed — they still reset to their clear values, which is
  pre-existing and was out of scope.
- **The census is arithmetically true.** 18 `pub` fields, 18 `is_present()` terms in `is_empty`,
  18 + 4 = 22, and the three-plus-one reason split matches.
- **The `extensions_from_default` comment is accurate.** `editable_base` is real
  (`management/handlers.rs:53`) and is the only reader of the flag; the ordering claim
  (`apply_default_extensions` at `merge.rs:178`, before the dispatch loop at `:200`) holds.
- 2580 passing, 0 clippy findings in `discovery/`, 0 doc warnings — independently re-run.

---

## The one outstanding item: thirteen wrong upstream line numbers

Every citation below was added by SUBA-081 and is written against **v0.57.0**. Each is off by one or
two, and each lands on an **adjacent override field** — the most confusable possible target.

This is not pedantry about navigation. The `extensions` case inverts the reader's conclusion: the
comment "pi `extensions`/`false` -> `delete next.extensions` (`agents.ts:1281`)" points at
`applyToolsOverride`, whose clear semantics are the **opposite** (`Some(vec![])`, not `None`). A
reader verifying that comment lands on the one arm in the file that would tell them the rule is the
reverse of what the comment says — and `tools`-vs-`extensions` clear semantics is precisely the
distinction this whole task turns on. Same shape for the others: `description` points at `output`,
`toolBudget` points at `completionGuard`.

Round 1 was returned for a census that read plausibly and was wrong. This is the same defect class
in a different medium, so it gets fixed rather than waved through.

Ground truth, re-derived with `git show v0.57.0:src/agents/agents.ts | nl -ba`:

| # | file:line | currently cites | correct | what the cited line actually is |
|---|---|---|---|---|
| 1 | `merge.rs:398` builtin `description` | `:1259` | **`:1258`** | the `output` arm |
| 2 | `merge.rs:410` builtin `defaultReads` | `:1262` | **`:1261`** | blank / `model` arm |
| 3 | `merge.rs:460` builtin `extensions` | `:1281` | **`:1282`** | `applyToolsOverride` call |
| 4 | `merge.rs:470` `extensions_from_default` note | `:1281` | **`:1282`** | same |
| 5 | `merge.rs:489` builtin `toolBudget` | `:1284` | **`:1285`** | the `completionGuard` arm |
| 6 | `merge.rs:572` `apply_output_override` doc | `:1260` | **`:1259`** | the `outputMode` arm |
| 7 | `merge.rs:732` custom `tools` arm | `:1436-1439` | **`:1438-1441`** | starts mid-`skills` fill |
| 8 | `merge.rs:1630` test, `output`/`outputMode` | `:1260-1261` | **`:1259-1260`** | shifted one |
| 9 | `merge.rs:1730` test, `description` | `:1259`/`:1380` | **`:1258`**/`:1380` | first half only; `:1380` is right |
| 10 | `types.rs:604` `description` field doc | `:1259` | **`:1258`** | the `output` arm |
| 11 | `types.rs:662` `extensions` field doc | `:1281` | **`:1282`** | `applyToolsOverride` call |
| 12 | `mod.rs:728` `toolBudget` applied-at (builtin) | `:1286` | **`:1285`** | past the arm |
| 13 | `mod.rs:728` `toolBudget` applied-at (custom) | `:1454` | **`:1456`** | a closing brace |

**Confirmed CORRECT — do not "fix" these:** every field-declaration citation (`:81`, `:82`, `:84`,
`:98`, `:99`, `:100`, `:102`, and the `:80-103` range), `applyToolsOverride` at `:1237-1246`, the
custom `description` arm at `:1380-1383`, and the custom `output` fill at `:1384-1386`.

**Out of scope — leave alone:** `types.rs:1480`'s `agents.ts:82-100` (pre-existing test comment,
predates this task) and every `@v0.43.0`-marked citation in these files, which is deliberately
pinned to the older upstream and is not drift.

---

## Definition of done

1. All thirteen numbers in the table corrected; the four "confirmed correct" groups untouched.
2. Re-derive rather than trust the table: `git show v0.57.0:src/agents/agents.ts | nl -ba | sed -n '<n>p'`
   for each corrected number should print the arm the comment describes.
3. No source behaviour changes. Test count stays at 2580; clippy and `cargo doc` stay clean.
