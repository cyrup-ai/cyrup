---
stage: aug
status: done
updated: 2026-08-27 04:59
---

# Take `cyrup-ext-subagents`'s Module Tree Out Of The Public API So `dead_code` Can See It

## Current state — re-measured 2026-08-27 against `origin/main` (`df64e81`)

The original framing is **correct in substance and wrong in method**. Re-measuring
[`crates/cyrup-ext-subagents`](../../crates/cyrup-ext-subagents):

| measurement | value | how |
| --- | --- | --- |
| module-level `pub` items | **1571** across 138 files | `grep -rnoE "^pub (async fn\|fn\|struct\|enum\|const\|trait\|type\|static) \w+" src/ \| wc -l` |
| …of those, **reachable from the crate root** | **1538 (97.9 %)** | reachability script in the DoD below |
| …already crate-private (inside a non-`pub` module) | **33** — 18 under `extension`, 15 under `discovery` | same script |
| indented `pub fn/const/type` (impl methods) | 532 | `grep -rnE "^[ \t]+pub (async fn\|fn\|const\|type) "` |
| indented `pub <field>:` (struct fields) | 2174 | `grep -rnE "^[ \t]+pub [a-z_0-9]+:"` |
| `pub(crate)` items at column 0 | 271 | `grep -rncE "^pub\(crate\) "` |
| `mod` declarations | 140 `pub mod` · 31 `pub(crate) mod` · 30 private `mod` | `grep -rnoE "^ *(pub(\(crate\)\|\(super\))? )?mod +\w+ *;"` |
| crate size | 204 `.rs` files, 181 306 lines | `find src -name '*.rs' \| xargs wc -l` |

**So the "already private, narrowing is churn" escape hatch does not apply here.** Only 33 of 1571
items (2 %) are `pub` inside a private module tree. The other 1538 really are the crate's public
API, and rustc's `dead_code` seed set really does cover all of them — which is why
`cargo check -p cyrup-ext-subagents` can be warning-free while 13 items have no reference anywhere.

### What actually consumes this crate

Dependents are exactly four: [`cyrup`](../../crates/cyrup/Cargo.toml) (`:67`),
[`cyrup-intercom`](../../crates/cyrup-intercom/Cargo.toml) (`:44`),
[`cyrup-permission-system`](../../crates/cyrup-permission-system/Cargo.toml) (`:36`),
[`cyrup-it`](../../crates/cyrup-it/Cargo.toml) (`:102`). Parsing every `cyrup_ext_subagents::…`
path in the workspace (brace groups expanded, comments stripped):

* **Production crates name 39 distinct paths**, resolving to **26** of the 1571 items.
* **`cyrup-it` names 154 further distinct paths**, bringing the total to **134** items (8.5 %).
* **1437 items (91.5 %) are never named outside this crate by any consumer, test crate included.**

Per top-level module (`items` / `named anywhere outside the crate` / `named by a production crate`):

```
exec               339   26    2      discovery         124    8    0
watchdog           221    0    0      missions           69    1    0
background         192   35   11      native_supervisor  43    4    0
tui                183   19    6      prompt_runtime     24    2    1
registration       161    8    2      artifacts          21    5    0
spawn              159   12    2      extension          18   11    2
formatters/paths/jsonl/time  12   0   0      fork_context/error   5   3   0
```

### What is genuinely dead, today

Cross-referencing all 1562 distinct names against every non-comment identifier occurrence in the
workspace's `.rs` files:

* **13 items with zero references of any kind** (each verified word-wise, excluding its own
  definition and every comment line):

  | item | site |
  | --- | --- |
  | `AsyncJobsPayload` | [tui/events.rs:834](../../crates/cyrup-ext-subagents/src/tui/events.rs) |
  | `CHECK_PROVIDER_CATALOG_FRESHNESS` | [registration/doctor.rs:239](../../crates/cyrup-ext-subagents/src/registration/doctor.rs) |
  | `NATIVE_SUPERVISOR_EXTENSION_DIR` | [native_supervisor.rs:58](../../crates/cyrup-ext-subagents/src/native_supervisor.rs) |
  | `read_child_metadata` | [native_supervisor.rs:369](../../crates/cyrup-ext-subagents/src/native_supervisor.rs) |
  | `build_agent_memory_injection_for` | [discovery/agent_memory.rs:416](../../crates/cyrup-ext-subagents/src/discovery/agent_memory.rs) |
  | `resolve_skills` | [discovery/skills.rs:149](../../crates/cyrup-ext-subagents/src/discovery/skills.rs) |
  | `fold_nested_summaries` | [tui/render.rs:236](../../crates/cyrup-ext-subagents/src/tui/render.rs) |
  | `project_nested_registry_for_root` | [spawn/nested_events.rs:1288](../../crates/cyrup-ext-subagents/src/spawn/nested_events.rs) |
  | `nested_results_path` | [spawn/nested_events.rs:1614](../../crates/cyrup-ext-subagents/src/spawn/nested_events.rs) |
  | `is_top_level_async_dir` | [spawn/nested_events.rs:1649](../../crates/cyrup-ext-subagents/src/spawn/nested_events.rs) |
  | `nested_artifact_env` | [spawn/nested_events.rs:1670](../../crates/cyrup-ext-subagents/src/spawn/nested_events.rs) |
  | `parse_no_args_command` | [registration/slash_commands.rs:1636](../../crates/cyrup-ext-subagents/src/registration/slash_commands.rs) — `pub fn parse_no_args_command(_raw_args: &str) {}`, referenced only by the prose comment at `:1681` |
  | `write_steer_ack` | [background/control.rs:1332](../../crates/cyrup-ext-subagents/src/background/control.rs) |

  **The original task's list has drifted** — `spawn/worktree.rs`'s `DEFAULT_HOOK_TIMEOUT` is already
  gone, `CHECK_PROVIDER_CATALOG_FRESHNESS` is new, and every `nested_events.rs` /
  `agent_memory.rs` / `native_supervisor.rs` line number moved. Re-run the scan before acting on any
  of it; commit `cb7afa5` already landed part of this hygiene queue.

* **71 items whose only references are `#[cfg(test)]` blocks, `src/tests/`, or `cyrup-it`.** These
  are *not* dead today only because everything is `pub`. Nine of them are inside `watchdog`; the
  rest spread across `exec` (12), `registration` (5), `discovery` (5), `spawn` (3), `tui` (2) and
  others.

## The method: demote module **declarations**, never sweep items

Two facts decide the whole shape of this task.

**(1) The original acceptance criterion measures the wrong thing.** Getting
`grep -c "^pub (fn|struct|…)"` below 400 requires editing ~1200 item declarations. That is a
1200-line diff whose every line is a chance to pick `pub(crate)` where `pub(super)` was needed, and
it produces no compiler signal when it is wrong in the *loose* direction. Meanwhile changing
`pub mod watchdog;` to `pub(crate) mod watchdog;` — **one keyword, one line** — removes 221
module-level items, 80 impl methods and 291 struct fields from the public API at once, and leaves
that grep count completely unchanged. **Count reachability, not `pub` tokens.**

**(2) Demotion is safe; deletion is what goes wrong.** The prior lib.rs `pub use` trim in this repo
was rejected for exactly this: the measurement was right, the proposed fix fired ~138
`unused_imports`, and the compiler loop converged on deleting the imports rather than reconciling
them — a demotion became a mass deletion. The same trap is live here in a nastier form:

> Every module-declaration demotion hands `dead_code` a set of items it could not see before.
> Those warnings are **the deliverable**, not an error to clear. A `cargo check` loop that treats
> them as breakage will delete a working, upstream-ported subsystem and its tests, and the build
> will go green.

Concretely, after each edit the compiler will say one of exactly four things:

| what the compiler says | what it means | correct response |
| --- | --- | --- |
| **`E0603` private module / `E0364` re-export of private item** — a *hard error* | you picked a visibility narrower than the in-crate callers need | widen one notch (`pub(super)` → `pub(crate)`), re-check. Never widen back to `pub`. |
| **`private_interfaces` / `private_bounds` warning** | a still-`pub` signature elsewhere names a type you just demoted | demote *that accessor* too (all four sites in Phase 1 are pre-identified below). Never re-promote the type. |
| **`dead_code` warning** | the item's only callers are `#[cfg(test)]`, `src/tests/`, or `cyrup-it` | **triage per item using the rule below.** This is the finding. |
| **`unused_imports` warning** | you deleted or moved an item, not merely demoted it | you have left the scope of this task — revert that edit. A pure visibility change cannot produce `unused_imports`. |

**Triage rule for every `dead_code` warning this task produces** — three buckets, in this order:

1. **Referenced by `cyrup-it`** → the item must stay `pub`. `cyrup-it` is a separate crate and links
   the library built *without* `cfg(test)`, so demoting such an item both fires `dead_code` and
   breaks the integration build. *The Phase-1 set below is already filtered to exclude every module
   `cyrup-it` touches, so this bucket should stay empty — if it appears, the demotion was too wide.*
2. **A deliberate ported-parity or test-observable primitive** (the doc comment already says so, or
   a live `_from_details` / `_with_X` sibling exists) → keep, annotate
   `#[allow(dead_code, reason = "…")]`. The crate already does this at
   [discovery/management/chain_crud.rs:36](../../crates/cyrup-ext-subagents/src/discovery/management/chain_crud.rs)
   (four items sharing one `reason` string), and
   [background/tracker.rs:591](../../crates/cyrup-ext-subagents/src/background/tracker.rs) uses the
   narrower `#[cfg_attr(not(test), allow(dead_code))]` for a test-observable field. Either form is
   in-repo precedent; prefer the `reason = "…"` form.
3. **A genuinely unwired subsystem** → **do not decide it here.** Annotate as in (2) with a `reason`
   naming the follow-up task, and let that task own wire-or-delete. Removing a subsystem is a design
   change; this task is a visibility change.

**Visibility target.** Workspace idiom is `pub(super)` for helpers shared inside one module tree —
126 occurrences in [`cyrup-tui/src/`](../../crates/cyrup-tui/src) (`transcript/` 55, `editor/` 45,
`app/` 12) and 123 in [`cyrup-intercom/src/`](../../crates/cyrup-intercom/src), against only 5 in
this crate. For **submodule** declarations prefer `pub(super) mod` and widen to `pub(crate) mod`
only when `E0603` says a sibling top-level module needs it — that error is a hard failure, so
guessing narrow is free. For **lib.rs-level** declarations `pub(super)` and `pub(crate)` mean the
same thing at the crate root; write `pub(crate) mod` explicitly.

**Doc links survive demotion.** [`.cargo/config.toml`](../../.cargo/config.toml) sets
`rustdocflags = ["--document-private-items"]` and the workspace pins
`rustdoc::private_intra_doc_links = "allow"` ([Cargo.toml:106-114](../../Cargo.toml)), so a
``[`crate::watchdog::…`]`` link from a public item into a now-crate-private one still resolves.
`rustdoc::broken_intra_doc_links` stays `deny` — which bites on *deletion*, not demotion. One more
reason the deletions belong in their own task.

## Scope of this task — Phase 1 only

Four `pub mod` declarations in
[`src/lib.rs`](../../crates/cyrup-ext-subagents/src/lib.rs) plus the four accessors that block them.
Chosen because these are the only top-level modules with **zero** references from any other crate,
`cyrup-it` included:

```
grep -rn 'cyrup_ext_subagents::watchdog' --include=*.rs crates/ --exclude-dir=cyrup-ext-subagents  →  0
… likewise ::paths, ::time, ::formatters  →  0 each;  ::jsonl  →  1, a doc comment
```

### Edit 1 — four declarations in `src/lib.rs`

| line | from | to | items removed from the public API |
| --- | --- | --- | --- |
| `:49` | `pub mod watchdog;` | `pub(crate) mod watchdog;` | 221 items + 80 methods + 291 fields |
| `:38` | `pub mod jsonl;` | `pub(crate) mod jsonl;` | 2 items + 6 methods |
| `:42` | `pub mod paths;` | `pub(crate) mod paths;` | 4 items |
| `:47` | `pub mod time;` | `pub(crate) mod time;` | 2 items |

Leave the doc comments above `:42` and `:47` in place. Leave the root re-exports at `:72-76`
untouched — a root `pub use` out of a crate-private module is the normal façade pattern and keeps
compiling; none of those five names lives in a demoted module anyway.

**`pub mod formatters;` (`:36`) is deliberately excluded.** All four of its items are unreferenced,
so demoting it fires four `dead_code` warnings and the loop converges on deleting the file — the
exact failure this task exists to avoid. That module is already owned by
[`.flux/review/medium/formatters-module-dead-unwired.md`](../review/medium/formatters-module-dead-unwired.md),
whose fix is to *wire it up* (six duplicate implementations still live in `registration/cost.rs`,
`tui/fleet.rs`, `tui/fleet_status.rs`, `tui/render.rs`, `background/fleet_view.rs`,
`background/run_status.rs`). Do not touch `formatters` here.

### Edit 2 — the four `private_interfaces` sites

These are the only four publicly-reachable signatures in the crate that name a `watchdog` type;
found by scanning every `crate::(watchdog|jsonl|paths|time)::` occurrence outside those modules and
walking back to its enclosing item head. The `paths`/`time` hits are all inside function *bodies*,
so they are inert. None of the four is referenced from any other crate
(`grep -rn "\.watchdog()\|with_watchdog\|with_permission_gate\|MainWatchdogRuntime\|ChildWatchdog"`
outside `cyrup-ext-subagents` → **0 hits**), so all four take `pub(crate)`:

| site | signature | why it breaks |
| --- | --- | --- |
| [prompt_runtime.rs:1627](../../crates/cyrup-ext-subagents/src/prompt_runtime.rs) | `pub fn with_watchdog(…, Option<Arc<crate::watchdog::register_child::ChildWatchdog>>, …)` | param type on `pub struct SubagentPromptRuntime` (`:1348`) |
| [prompt_runtime.rs:1696](../../crates/cyrup-ext-subagents/src/prompt_runtime.rs) | `pub fn watchdog(&self) -> Option<&Arc<…ChildWatchdog>>` | return type, same struct |
| [prompt_runtime.rs:1708](../../crates/cyrup-ext-subagents/src/prompt_runtime.rs) | `pub fn with_permission_gate(…, Arc<dyn crate::watchdog::permission_arbiter::WatchdogPermissionAgent>)` | param type, same struct |
| [extension/host/mod.rs:215](../../crates/cyrup-ext-subagents/src/extension/host/mod.rs) | `pub fn watchdog(&self) -> &Arc<crate::watchdog::runtime::MainWatchdogRuntime>` | inherent method on `SubagentsExtension`, re-exported publicly at [extension/mod.rs:80](../../crates/cyrup-ext-subagents/src/extension/mod.rs) |

All four are exercised by
[src/tests/watchdog_wiring.rs](../../crates/cyrup-ext-subagents/src/tests/watchdog_wiring.rs)
(`:130`, `:432`, `:457`, `:542`, `:600`), which is in-crate, so `pub(crate)` keeps them green.
[extension/tool/mod.rs](../../crates/cyrup-ext-subagents/src/extension/tool/mod.rs) already uses
`pub(crate) fn` for the equivalent builders (`:113`, `:114`) — no edit needed there. Private struct
fields holding watchdog types (`prompt_runtime.rs:1386`/`:1430`, `extension/host/mod.rs:64`,
`extension/tool/mod.rs:79`) are unaffected; `grep -rnE "^\s+pub [a-z_]+: .*(watchdog::|Watchdog)"`
outside `src/watchdog/` returns nothing.

### Edit 3 — triage the nine `dead_code` warnings Edit 1 produces

These are the only `watchdog` items whose sole callers sit past a `#[cfg(test)]`. **Every one falls
in bucket 2 or 3 — none is to be removed by this task.**

| item | verdict |
| --- | --- |
| [watchdog/settings.rs:850](../../crates/cyrup-ext-subagents/src/watchdog/settings.rs) `resolve_watchdog_config_strict` | **Keep + annotate.** Its own doc at `:843-846` already settles it: *"No caller — and none upstream either: `settings.ts` exports it at `:471` and no file in `pi-subagents@v0.43.0` imports it… Kept as ported public surface, not as pending wiring."* Reuse that sentence as the `reason`. |
| [watchdog/warning_format.rs:266](../../crates/cyrup-ext-subagents/src/watchdog/warning_format.rs) `create_watchdog_warning_message` | **Keep + annotate.** Parity twin of the live `create_watchdog_warning_message_from_details` (`:285`, called at [extension/host/mod.rs:252](../../crates/cyrup-ext-subagents/src/extension/host/mod.rs) and [watchdog/register_child.rs:278](../../crates/cyrup-ext-subagents/src/watchdog/register_child.rs)); the test at `:423-441` asserts the two agree exactly. |
| [watchdog/warning_format.rs:150](../../crates/cyrup-ext-subagents/src/watchdog/warning_format.rs) `details_as_warning` | **Keep + annotate.** The details→warning bridge that parity test runs through (`:442`, `:467`, `:483`, plus `register_main.rs:1375`/`:1400`). |
| [watchdog/warning_format.rs:252](../../crates/cyrup-ext-subagents/src/watchdog/warning_format.rs) `format_watchdog_warning_content` | **Keep + annotate.** Same cluster; the live path takes `format_watchdog_warning_content_from_details` (`:189`). |
| [watchdog/render.rs:198](../../crates/cyrup-ext-subagents/src/watchdog/render.rs) `render_watchdog_warning_plain` | **Keep + annotate.** Plain-text twin of the live `render_watchdog_warning` (`:159`, two in-crate callers); same parity shape. |
| [watchdog/tool_actions.rs:60](../../crates/cyrup-ext-subagents/src/watchdog/tool_actions.rs) `WATCHDOG_THINKING_VALUES` | **Keep + annotate.** Ported constant array (`tool-actions.ts:155`), pinned by its own test at `:486-487`. |
| [watchdog/child_status.rs:480](../../crates/cyrup-ext-subagents/src/watchdog/child_status.rs) `is_child_watchdog_status_event` | **Bucket 3 — keep + annotate, decision deferred.** |
| [watchdog/child_status.rs:497](../../crates/cyrup-ext-subagents/src/watchdog/child_status.rs) `child_watchdog_is_active` | **Bucket 3.** |
| [watchdog/child_status.rs:516](../../crates/cyrup-ext-subagents/src/watchdog/child_status.rs) `accept_child_watchdog_event` | **Bucket 3.** |

On the `child_status.rs` trio specifically: this is the parent-side ingest half of the child
watchdog (`child-status.ts:167-205`). Production wires only the *emit* half —
[exec/spawn_plan.rs:752-764](../../crates/cyrup-ext-subagents/src/exec/spawn_plan.rs) encodes the
config into the child's env — and all references to the three predicates sit past the
`#[cfg(test)]` at `child_status.rs:546`. A green suite asserting on `accept_child_watchdog_event`
therefore reads as proof the parent ingests child watchdog events, and nothing does. **That is a
real finding and a real design decision — it is not a visibility decision.** Annotate all three
with a shared `reason` naming `WIRE_OR_DROP_CHILD_WATCHDOG_INGEST` and stop there.

`discovery::management`'s three test-only items (`create_chain`, `update_chain`, `rename_chain`)
need no action in any phase — they already carry `#[allow(dead_code, reason = …)]` from the
[DECOMPOSE_DISCOVERY_MANAGEMENT](../done/2026-08-25-18-23/DECOMPOSE_DISCOVERY_MANAGEMENT.md) split,
which is this exact pattern applied once already.

### What Phase 1 achieves

Reachable module-level `pub` items **1538 → 1309** (−15 %), plus 86 impl methods and 291 struct
fields, from **8 keyword edits and 9 annotations**. 221 `watchdog` items come under `dead_code` for
the first time. Zero item declarations moved, zero imports touched, zero removals.

## Out of scope — named follow-ups

The full mechanical sweep is **72 `pub mod` declarations across 8 files, removing 825 items**
(1538 → 713). That is well beyond one session once each batch's `dead_code` fallout is triaged
honestly, so it is split by owning `mod.rs`. Every module listed below was checked the same way: its
entire subtree is named by no crate outside `cyrup-ext-subagents`, `cyrup-it` included.

| follow-up task | declarations | file(s) | items freed | `dead_code` fallout |
| --- | --- | --- | --- | --- |
| `RESTRICT_PUB_EXEC_SUBMODULES` | 29 | `src/exec/mod.rs`, `src/exec/acceptance/{model,lattice}/mod.rs`, `.../model/report/mod.rs` | 191 | 12 test-only (`structured` 4, `capability_ceiling` 2, `completion_guard` 2, `ndjson` 2, `usage_budget` 2, `mcp_direct_tools` 1, `verify` 1, `report` 1) |
| `RESTRICT_PUB_TUI_SUBMODULES` | 7 | `src/tui/mod.rs` (`fleet`, `fleet_overlay`, `fleet_state`, `fleet_theme`, `fleet_transcript`, `notices`, `render`) | 115 | 1 dead (`fold_nested_summaries`), 2 test-only |
| `RESTRICT_PUB_DISCOVERY_SUBMODULES` | 7 | `src/discovery/mod.rs` | 72 | 1 dead (`resolve_skills`), 5 test-only (3 already annotated) |
| `RESTRICT_PUB_SPAWN_AND_BACKGROUND_SUBMODULES` | 12 | `src/spawn/mod.rs`, `src/background/mod.rs` | 109 | 3 test-only, all `spawn::worktree` |
| `RESTRICT_PUB_REGISTRATION_AND_MISSIONS_SUBMODULES` | 12 | `src/registration/mod.rs`, `src/missions/mod.rs` | 105 | 1 dead (`CHECK_PROVIDER_CATALOG_FRESHNESS`), 6 test-only |
| `DELETE_DEAD_SUBAGENT_ITEMS` | — | the 13-item table above | — | independent of visibility; doing it **first** removes 4 of the fallout decisions above. `rustdoc::broken_intra_doc_links = deny` makes this the one phase where removal can break the doc build — sweep ``[`name`]`` links alongside each removal. |
| `WIRE_OR_DROP_CHILD_WATCHDOG_INGEST` | — | `src/watchdog/child_status.rs:480/:497/:516` and their tests | — | design decision deferred out of Phase 1 |
| `TRIAGE_TEST_ONLY_SUBAGENT_ITEMS` | — | the remaining ~50 test-only items | — | run only after the demotion phases; the compiler enumerates the set for you |

Also out of scope everywhere: `src/formatters.rs` (owned by the open review item); the top-level
`pub mod` declarations for `artifacts`, `background`, `discovery`, `error`, `exec`, `extension`,
`fork_context`, `missions`, `native_supervisor`, `prompt_runtime`, `registration`, `spawn` and
`tui` — every one of them is named by name from another crate; and any change to module boundaries,
item names, behaviour, or ported-parity comments.

## Definition of Done

Record the baseline **before the first edit**
(`cargo check -p cyrup-ext-subagents --all-targets --message-format short 2>&1 | tee /tmp/baseline.txt`);
the original survey asserts it is warning-free, but re-measure rather than trust it.

- [ ] `sed -n '36p;38p;42p;47p;49p' crates/cyrup-ext-subagents/src/lib.rs` shows `pub(crate) mod` for
      `jsonl`, `paths`, `time` and `watchdog`, and an unchanged `pub mod formatters;`.
- [ ] The four accessors read `pub(crate) fn`:
      `grep -n "fn with_watchdog\|fn watchdog(\|fn with_permission_gate" crates/cyrup-ext-subagents/src/prompt_runtime.rs crates/cyrup-ext-subagents/src/extension/host/mod.rs`
      returns four lines, none of them beginning `pub fn`.
- [ ] Reachable module-level `pub` items drop from **1538 to 1309**:
      ```
      python3 - crates/cyrup-ext-subagents/src <<'PY'
      import os,re,sys
      SRC=sys.argv[1]
      M=re.compile(r'^[ \t]*(pub\(crate\)|pub\(super\)|pub\(in [^)]*\)|pub)?\s*mod\s+([a-z_0-9]+)\s*;')
      I=re.compile(r'^pub (async fn|fn|struct|enum|const|trait|type|static) \w+')
      f={}
      for dp,_,fn in os.walk(SRC):
          for n in fn:
              if not n.endswith(".rs"): continue
              p=os.path.join(dp,n); r=os.path.relpath(p,SRC)
              k=() if r=="lib.rs" else tuple(r[:-3].split(os.sep)[:-1] if r.endswith("mod.rs") else r[:-3].split(os.sep))
              f[k]=p
      v={}
      for k,p in f.items():
          for l in open(p,encoding='utf-8',errors='replace'):
              m=M.match(l)
              if m: v[k+(m.group(2),)]=m.group(1) or "priv"
      reach=lambda k: all(v.get(k[:i])=="pub" for i in range(1,len(k)+1))
      print(sum(sum(1 for l in open(p,encoding='utf-8',errors='replace') if I.match(l)) for k,p in f.items() if reach(k)))
      PY
      ```
- [ ] The raw `pub`-token count is **unchanged at 1571** —
      `grep -rnoE "^pub (async fn|fn|struct|enum|const|trait|type|static) \w+" crates/cyrup-ext-subagents/src/ | wc -l`.
      Any other number means item declarations were edited, which this phase forbids.
- [ ] **The build stays warning-free.** `cargo check -p cyrup-ext-subagents --all-targets 2>&1 | grep -c '^warning'`
      reports no more than the recorded baseline, and
      `cargo check -p cyrup-ext-subagents --all-targets 2>&1 | grep -E 'dead_code|unused_imports|private_interfaces|private_bounds'`
      is empty. A demotion that leaves any of those four standing is **not done**.
- [ ] `grep -rn "allow(dead_code" crates/cyrup-ext-subagents/src/watchdog/` shows exactly the nine
      annotations from Edit 3, each carrying a `reason = "…"`, and the three `child_status.rs` ones
      name `WIRE_OR_DROP_CHILD_WATCHDOG_INGEST`.
- [ ] Nothing was removed and no `use` statement was touched: `git diff --stat` covers only
      `src/lib.rs`, `src/prompt_runtime.rs`, `src/extension/host/mod.rs` and the five `src/watchdog/`
      files, and `git diff -U0 -- crates/cyrup-ext-subagents | grep -c '^-use '` is 0.
- [ ] `cargo clippy -p cyrup-ext-subagents --all-targets --no-deps -- -D warnings` exits 0.
- [ ] `cargo test -p cyrup-ext-subagents` shows no new failures against the recorded baseline.
- [ ] `cargo build --workspace` and `cargo build -p cyrup-it --tests --features it` both still
      compile — the 39 production paths and the 154 `cyrup-it` paths are untouched by design, and
      this is the check that proves it.
- [ ] `cargo doc -p cyrup-ext-subagents --no-deps 2>&1 | grep -c '^warning'` is unchanged from the
      baseline (`rustdoc::broken_intra_doc_links` is `deny` workspace-wide, `private_intra_doc_links`
      is `allow`, so demotion alone must not move this number).
- [ ] Port fidelity preserved: no `pi-subagents` citation, `[CYRUP-DELTA]` note or `R-SA-*`
      reference removed or reworded.
- [ ] The seven follow-up tasks above exist as files under `.flux/todo/` or `.flux/backlog/`.

## Source

- Identified by the `subagents-hygiene-survey` workflow (13 agents, 21 raw findings, 16 confirmed
  after adversarial verification). Effort re-scoped from *large* to **small for Phase 1**, with
  seven named follow-ups carrying the rest.
- Method precedents in-repo:
  [BROKER_PUB_MOD_CONTRADICTS_DOC](../backlog/BROKER_PUB_MOD_CONTRADICTS_DOC.md) — the same
  `pub mod` → `pub(crate) mod` move, four declarations, done cleanly — and
  [PUBLIC_SURFACE_AUDIT](../done/2026-08-22-15-26/PUBLIC_SURFACE_AUDIT.md), which did item-level
  demotion and fixed the one intra-doc link it broke.
