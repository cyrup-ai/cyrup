# Sweep C — 00-residual-ledger.md, audited at 14e6c56

Read-only audit. Nothing in `crates/` or `docs/` was edited. Upstream reads are pinned
(`git -C tmp/pi-subagents show v0.68.0:<path>`). No cargo was run.

**Scope note.** The file's only true *residual rows* are the four `R-*` blocks at the top (all
CLOSED) and the twelfth edition's NAVIGATION block — its **RANKED SET** table and its **BLOCKED**
list. Everything below line 493 is archived provenance (editions 1–11), which this sweep did not
re-derive. Two exceptions were audited because they propose work and are cited as live: the
navigation block's own load-bearing prose claims (the census, the verb count, the area-09 tally),
and the tenth edition's **"Residual leads this batch produced"** list (line 1083), which the file
explicitly keeps as ownerless open leads.

## Summary

| row | verdict | note |
|---|---|---|
| `R-VLS11b-01` | **CLOSED** (confirmed) | closure note's greps all resolve at HEAD |
| `R-VLS11b-02` | **CLOSED** (confirmed) | both free entry points + the assigned call site are live |
| `R-VLS11b-03` | **CLOSED** (confirmed) | settings tier + both named tests exist |
| `R-SUBA087-01` | **CLOSED** (confirmed) | `StepStatus::child_id` is rung 1 of four |
| RANKED #3 `UW-10` | **STILL OPEN, citations drifted** | premise intact; `native.rs:270` → `:370`; "three other crates" → four |
| RANKED #7 `SUBA-096` | ~~STILL OPEN, premise holds~~ **CLOSED 2026-09-23 (`c7a56ed`)** | residuals filed as `SUBA-100`…`SUBA-103` in `09a` |
| RANKED #8 `PB-14` | ~~STILL OPEN, premise holds~~ **CLOSED 2026-09-23 (`f978396`)** | both surfaces |
| RANKED #13 `PB-9` | ~~STILL OPEN, premise holds~~ **CLOSED 2026-09-23 (`076a5f9`), Plan A** | the premise ("build the preview UI") was v0.42.1's; upstream refuses `clarify` at v0.68.0 and cyrup now does too |
| BLOCKED `SUBA-054` | **STILL OPEN, premise holds** | async half still `reads: None` |
| BLOCKED `UW-21` | **PREMISE FALSE** | closed 2026-09-18 by `PB-8`; the same block's row 1 says so |
| BLOCKED `UW-7` | **PREMISE FALSE** | the `on_terminal_input` seam exists, and did when the block was written |
| BLOCKED `DRIFT-009`/`PROV-071` | **STILL OPEN, premise holds** | data block unchanged |
| BLOCKED `children.list` | **PREMISE FALSE** | landed 2026-09-19; the block's own footnote contradicts it |
| BLOCKED `VL-S7` worktree arms | **PREMISE FALSE** | `VL-S10` landed the verbs; arms are attached |
| BLOCKED `SESS-S05` | **STILL OPEN, premise holds** | citation `types.rs:84-103` still exact |
| BLOCKED `PERM-032` | **STILL OPEN, premise holds** | — |
| BLOCKED areas 13/15 | **STILL OPEN, citations drifted** | "current `v2.33.0`" → `v2.36.0`; pi-subagents → `v0.70.1` |
| NAV census table | **STILL OPEN, citations drifted** | 76 → **75** open, 604 → **605** closed; script path wrong |
| NAV "17 missing verbs … seven remain" | **PREMISE FALSE** | cyrup names all 57 + 2 = **59**; zero missing |
| NAV "areas 09 + 09a carry 11 open / 66 closed" | **STILL OPEN, citations drifted** | now 10 open / 67 closed |
| NAV structural warning (`UW-21` member) | **PREMISE FALSE** (one member) | `UW-21` is wired and closed |
| NAV "`crates/cyrup-ext-subagents` is 387 `.rs` / 314 709 lines" | version-pinned — **add**, do not correct | at `14e6c56`: 473 files / 379 505 lines |
| LEAD `resolve_shortcuts` has no production caller | **STILL OPEN, premise holds** | only a test doc mentions it |
| LEAD `AFTER_TERMINAL` dead-but-shipped | **STILL OPEN, premise holds** | one self-referential test |
| LEAD 17 × `agent-session.ts:2782` | **STILL OPEN, citations drifted** | now **16** |
| LEAD `runner_to_json_string` capabilities comment | **PREMISE FALSE** (inverted) | no `Map`; and the workspace DOES enable `preserve_order` |
| LEAD export-leaf two-lock TOCTOU | **STILL OPEN, premise holds** | two awaits on the manager |
| LEAD two `exportSessionToHtml` guards unported | **PRESCRIPTION WRONG / half false** | the in-memory guard IS ported |
| LEAD `startup_resources_panel.rs` `:1986` | **STILL OPEN, premise holds** | `:550` still cites `(:1986, :5993)` |
| LEAD `[Extension issues]` middle tier | **STILL OPEN, premise holds** | only load + shortcut tiers fold |
| LEAD 29 bad markdown table rows | **UNVERIFIABLE here** | needs the measuring script, which is not in-tree |
| LEAD `cyrup-it` 492/493/494 unmeasured | **UNVERIFIABLE here** | cargo is forbidden this pass |

---

## R-VLS11b-01 · a user detach aimed at a WORKFLOW child
**Verdict:** CLOSED — confirmed. No change.
**Evidence:**
```text
$ grep -rn "hand_off_detached_foreground_run" crates/cyrup-ext-subagents/src/
.../executor/foreground.rs:528:            .hand_off_detached_foreground_run(
.../executor/foreground.rs:606:    async fn hand_off_detached_foreground_run(
.../executor/workflow.rs:932: (the detach hook's note)
.../executor/workflow_launch.rs:750: (the corrected scoping doc)
$ grep -n 'parent_workflow_run_id: Some' crates/cyrup-ext-subagents/src/extension/executor/workflow.rs
892:                    parent_workflow_run_id: Some(self.workflow_run_id.clone()),
```
The one production writer of `parent_workflow_run_id` is still `WorkflowRunHost::launch` at
`workflow.rs:892`, exactly as the closure note states.

## R-VLS11b-02 · the detach continuation cannot persist foreground history
**Verdict:** CLOSED — confirmed. No change.
**Evidence:**
```text
$ grep -rn 'fn persist_foreground_run_history' crates/cyrup-ext-subagents/src/
.../foreground_history/persist.rs:111:pub(crate) fn persist_foreground_run_history_from(
.../foreground_history/persist.rs:145:pub(crate) fn persist_foreground_run_history_in(
.../foreground_history/persist.rs:161:    pub(crate) async fn persist_foreground_run_history_for(&self, cwd: &Path)
$ grep -n 'mod persist' crates/cyrup-ext-subagents/src/extension/executor/foreground_history/mod.rs
62:pub(crate) mod persist;
```
The assigned call site is live at `foreground.rs:1726-1730`, inside
`spawn_detached_foreground_continuation`, after `reconcile_detached_foreground_child` — the
ordering the closure note requires. The two deleted `&self` methods are gone.

## R-VLS11b-03 · the detach chord is env-tier only and defaults ON
**Verdict:** CLOSED — confirmed. No change.
**Evidence:**
```text
$ grep -n 'foreground_detach_shortcut' crates/cyrup-ext-subagents/src/registration/mod.rs
481:    pub foreground_detach_shortcut: Option<String>,
571:            foreground_detach_shortcut: None,
$ grep -n 'fn nothing_is_bound_when_neither_tier_is_configured\|fn the_settings_tier_wins_over_the_env_tier' \
    crates/cyrup-ext-subagents/src/extension/host/shortcuts.rs
256:    fn nothing_is_bound_when_neither_tier_is_configured() {
281:    fn the_settings_tier_wins_over_the_env_tier() {
```
`:268` asserts `SubagentExtensionConfig::default().foreground_detach_shortcut` is `None` — the
off-by-default the note claims.

## R-SUBA087-01 · `step.childId`, the unported 4th identity rung
**Verdict:** CLOSED — confirmed. No change.
**Evidence:**
```text
$ grep -n 'child_id' crates/cyrup-ext-subagents/src/background/records.rs
128:    pub child_id: Option<String>,
233:            child_id: None,
$ sed -n '157p' crates/cyrup-ext-subagents/src/background/child_identity.rs
    for candidate in [child_id, workflow_key, run_id, Some(positional.as_str())]
```
Upstream's four rungs in upstream's order.

---

## RANKED #3 · `UW-10` — `/intercom <target>` opens no live compose box
**Verdict:** STILL OPEN, citations drifted.
**Evidence:**
```text
$ grep -rn "open_overlay" crates/cyrup-intercom/src/
$ (no output)
$ grep -n "handle_input" crates/cyrup-intercom/src/ui/compose.rs
109:    pub fn handle_input(&mut self, keybindings: &dyn Keybindings, data: &str) -> ComposeAction {
```
`ui/mod.rs:27-31` still records the overlay-host surface as unreachable. Two drifts in the row's
parenthetical:
```text
$ grep -n "pub fn register_message_renderer" crates/cyrup-ext/src/native.rs
370:    pub fn register_message_renderer(&mut self, custom_type: impl Into<String>) {
$ grep -rn "\.open_overlay(" crates/*/src --include=*.rs | grep -v "fn open_overlay"
cyrup-ext-subagents/src/extension/host/slash.rs:147
cyrup-flux/src/overlay.rs:398
cyrup-mcp/src/ui.rs:5270, :5291
cyrup-permission-system/src/extension/command.rs:211
cyrup-session-svc/src/host_services.rs:1252
```
**Replacement text:**
> | 3 | **`UW-10`** (§2, **no area-11 id — file one**) | `/intercom <target>` **opens a live compose box and session picker** instead of printing a picture of one and asking the user to retype the command. **Both blockers this row named are discharged** (`register_message_renderer` = `cyrup-ext/src/native.rs:370`; `VL-S15`/`register_shortcut` closed) and `HostServices::open_overlay` is production-consumed by **four** other crates (`cyrup-ext-subagents`, `cyrup-flux`, `cyrup-mcp`, `cyrup-permission-system`, plus `cyrup-session-svc`'s own facade) — intercom simply never calls it. The `handle_input` state machines are ported and unit-tested (`ui/compose.rs:109`) | S–M | no |

## RANKED #7 · `SUBA-096` — the `acceptanceRole` override is silently dropped
**Verdict:** STILL OPEN, premise holds. Nothing to change.
**Evidence:** `AgentOverrideConfig` (`crates/cyrup-ext-subagents/src/discovery/types.rs:694`) has
21 fields and none of them is `acceptance_role`, `output_mode` or `fast`; its attribute line
(`:693`) is `#[serde(rename_all = "camelCase", default)]` — no `deny_unknown_fields`.

## RANKED #8 · `PB-14` — the "skills not found" warning is unported on BOTH surfaces
**Verdict:** STILL OPEN, premise holds. Nothing to change.
**Evidence:**
```text
$ grep -rn 'skills_warning\|skillsWarning' crates/cyrup-ext-subagents/src/
artifacts.rs:544:/// `durationMs`/`skills`/`skillsWarning`, which `SingleResult` does not carry in this crate
```
Exactly one hit, and it is the confession `PARITY-GAPS.md` describes (the line moved `:537` → `:544`
— a PARITY-GAPS drift, not a ledger one). The unrelated hard-failure string is still
`exec/mod.rs:1175` `"Skills not found: {}"`.

## RANKED #13 · `PB-9` — `clarify: true` shows no preview/edit UI
**Verdict:** STILL OPEN, premise holds. Nothing to change.
**Evidence:** `grep -rn 'ChainClarify\|chain_clarify' crates/cyrup-ext-subagents/src/ | wc -l` → `0`.
The row's "The seam is live (`open_overlay`)" is still true (six production call sites, listed
above).

---

## BLOCKED · `SUBA-054` — which cwd a runner step's `reads` resolve against
**Verdict:** STILL OPEN, premise holds. Nothing to change.
**Evidence:** `crates/cyrup-ext-subagents/src/extension/executor/background.rs:253` is still
`reads: None,` on the async SINGLE launch; the foreground half is landed. It is a decision, not
effort, exactly as written.

## BLOCKED · `UW-21` — `async_status_snapshot` blocked on `PB-8` or `set_widget`
**Verdict:** **PREMISE FALSE.**
**The sentence:** *"**`UW-21`** — `background/async_status_snapshot/` (1 928 LOC, landed unwired) on
**`PB-8` or a `set_widget` capability**. Closes for free when either lands."*
**The truth:** `PB-8` landed on 2026-09-18 and took `UW-21` with it. This same NAVIGATION block
says so in its own ranked table (row 1: *"Closed **`UW-21`** with it, both halves"*), and
`PARITY-GAPS.md:2063` carries `UW-21` struck through and marked **CLOSED**. The BLOCKED list
contradicts the table 120 lines above it.
**Evidence:**
```text
$ grep -n "UW-21" docs/gap-analysis/PARITY-GAPS.md
2063:**~~UW-21 · The whole `async_status_snapshot` subsystem has no production caller …~~** — ~~*low*~~ **CLOSED**
$ ls crates/cyrup-ext-subagents/src/extension/rpc/
envelope.rs  fleet.rs  mod.rs  params.rs  ping.rs
```
**Replacement text:** delete the `UW-21` bullet from the BLOCKED list. If a trace is wanted:
> * ~~**`UW-21`** — `background/async_status_snapshot/` on **`PB-8`**~~ — **CLOSED 2026-09-18 with
>   `PB-8`**, both halves; see ranked row 1 and `PARITY-GAPS.md`'s struck row. Not blocked, not open.

## BLOCKED · `UW-7` — the fleet-status widget receives no keystrokes
**Verdict:** **PREMISE FALSE.** This is the `register_shortcut`-shaped one.
**The sentence:** *"**`UW-7`** — on a **new `on_terminal_input` host seam**; `VL-S15`'s
`register_shortcut` landed and did not help, because a registered chord is not a keystroke stream."*
**The truth:** the second clause is right and the first is wrong. The `on_terminal_input` seam is
not new work — it exists end to end, on both extension tiers, and has since **2026-09-04**, twelve
days before this block was written:
```text
$ grep -rn "on_terminal_input\|subscribe_terminal_input" crates/cyrup-ext/src/ --include=*.rs | grep -v tests/
native.rs:425:    pub fn subscribe_terminal_input(&mut self)          # the InitApi declaration
native.rs:757:    fn on_terminal_input(&self, _data: &str) -> Option<crate::TerminalInputResult>
registry.rs:595:    pub fn subscribe_terminal_input(&self, owner: ExtensionId)
registry.rs:605:    pub fn unsubscribe_terminal_input(&self, owner: &ExtensionId)
facade.rs:1472:    pub async fn terminal_input(&self, data: &str) -> TerminalInputDecision
facade.rs:1500:    async fn terminal_input_via(&self, owner: &ExtensionId, data: &str)
host/live.rs:323 / :1980                                  # the WASM tier
$ grep -rn "pub fn subscribe_terminal_input" crates/cyrup-ext-sdk/src/ctx/ui.rs
157:    pub fn subscribe_terminal_input(&self)
$ git log --reverse --format="%h %ad" --date=short -S "fn on_terminal_input" -- crates/cyrup-ext/src/native.rs | head -1
9e2bfda 2026-09-04
```
It is labelled `EXT-021`, and area 06 closed that residual on 2026-08-15. What is actually missing
is **two wires**, not a seam:
```text
$ grep -rn "terminal_input" crates/cyrup-tui/src/ | grep -v native_modifiers
$ (no output — the TUI input path never calls ExtensionHost::terminal_input)
$ grep -rn "\.terminal_input(" crates/ --include=*.rs
crates/cyrup-ext/src/tests/native_dispatch.rs:1619, :1639, :1670, :1676, :1722, :1758   # tests only
$ grep -rn "subscribe_terminal_input\|on_terminal_input" crates/cyrup-ext-subagents/src/
$ (no output — subagents neither declares the subscription nor implements the handler)
```
The handler it would feed also moved: `SubagentFleetStatus::handle_key` is now
`tui/fleet_status.rs:917` (the ledger's sibling file still says `:740`, and `PARITY-GAPS.md` says
`:764`), `press` is `:1452`, `is_widget_registered` is `:864`.
**Replacement text:**
> * **`UW-7`** — **not** on a missing seam. `on_terminal_input` exists on both tiers and has since
>   `9e2bfda` (2026-09-04): `InitApi::subscribe_terminal_input` (`cyrup-ext/src/native.rs:425`),
>   `NativeExtension::on_terminal_input` (`:757`), the registry pair (`registry.rs:595`/`:605`),
>   the dispatcher `ExtensionHost::terminal_input` (`facade.rs:1472`) and the WASM tier
>   (`host/live.rs:323`, `:1980`). It is blocked on **two wires**, both S: (1) the TUI input path
>   never calls `ExtensionHost::terminal_input` — every caller is a test in
>   `cyrup-ext/src/tests/native_dispatch.rs`; (2) `cyrup-ext-subagents` never calls
>   `subscribe_terminal_input` nor implements `on_terminal_input`, so `handle_key`
>   (`tui/fleet_status.rs:917`) still has no production caller. `VL-S15`'s `register_shortcut` is
>   a different seam and correctly did not help.

## BLOCKED · `DRIFT-009` / `PROV-071` — the provider-catalog DATA block
**Verdict:** STILL OPEN, premise holds. Nothing to change.
**Evidence:** `PROV-071`'s row in `01-cyrup-core-and-provider.md` still records the three Qwen
members as `FleetCatalog::Dynamic` (`providers/fleet.rs:179,183,189`) and the `models.dev`/`pi.dev`
egress as unreachable. Egress was not re-tested this pass (out of scope for a read-only sweep), so
the 403 itself is carried, not re-proven.

## BLOCKED · `children.list` — blocked on retention
**Verdict:** **PREMISE FALSE.**
**The sentence:** *"**`children.list`** — on **retention**; `extension/tool/text.rs:215` records
that the listing would always be empty until the async/detached workflow shape lands."*
**The truth:** it landed on 2026-09-19. The verb is advertised, dispatched, and reads real data;
the cited line is also dead (`text.rs:215` is now a paragraph about `stop` in the child-safe tool
description).
**Evidence:**
```text
$ grep -n '"children.list"' crates/cyrup-ext-subagents/src/extension/tool/routing.rs
1228:            "children.list" => {
$ ls crates/cyrup-ext-subagents/src/background/retained_children.rs
crates/cyrup-ext-subagents/src/background/retained_children.rs
```
`routing.rs:1234-1240` calls `list_retained_children(&async_root, &results_dir, session)` and
renders through `format_retained_children`. `retained_children.rs:1-20` records the resolution:
cyrup's workflow children are retained as **step rows of the workflow's own `status.json`**, so one
upstream retained child = one `(workflow status, step index)` pair. The block's own footnote 150
lines above already says this (*"2026-09-19: `children.list` and `debug.run` are off that list"*).
**Replacement text:** delete the bullet.

## BLOCKED · `VL-S7`'s `worktree.discard`/`destructiveCleanup` arms
**Verdict:** **PREMISE FALSE.**
**The sentence:** *"**`VL-S7`'s `worktree.discard`/`destructiveCleanup` arms** — on `VL-S10` landing
those verbs."*
**The truth:** `VL-S10` closed on 2026-09-19 and landed them. This block's own ranked row 5 says
*"`VL-S7`'s authority arms are attached"*, and `PARITY-GAPS.md:1607-1609` says
*"**`VL-S7`'s authority arms now have something to attach to** … `registration/authority.rs` maps
`worktree.discard` → `discardWorktree` (Confirm by default) and the in-tree note … is discharged."*
**Evidence:**
```text
$ awk '/const SUBAGENT_ACTIONS/,/^\];/' crates/cyrup-ext-subagents/src/extension/tool/text.rs | grep '"worktree'
    "worktree.discard",
    "worktree.cleanup",
```
**Replacement text:** delete the bullet.

## BLOCKED · `SESS-S05` — one struct field
**Verdict:** STILL OPEN, premise holds. Nothing to change.
**Evidence:** `crates/cyrup-session-svc/src/session/types.rs:84-103` — `SessionDagNode` has
`entry_id`, `parent_id`, `depth`, `label`, `kind`, `foldable`, `is_leaf`, `has_label`, `timestamp`.
The cited line range is still exact.

## BLOCKED · `PERM-032` — the decisive experiment
**Verdict:** STILL OPEN, premise holds. Nothing to change.
**Evidence:** area 10's row still carries the 2026-09-14 finding that every cyrup-side hypothesis
(shape, `terminate` retype, denial content) is refuted, leaving only the live Together-side
measurement. The census script still reports `PERM-032` as the one unclassified kind.

## BLOCKED · Areas 13 and 15 — on a re-census
**Verdict:** STILL OPEN, citations drifted (the gap is now larger, not smaller).
**Evidence:**
```text
$ git -C tmp/pi-mcp-adapter tag --sort=v:refname | tail -4
v2.33.0
v2.34.0
v2.35.0
v2.36.0
$ git -C tmp/pi-subagents tag --sort=v:refname | tail -4
v0.68.0
v0.69.0
v0.70.1
```
The ledger's *"a current `v2.33.0`"* is no longer current — the clone is at `v2.36.0`, so the
unmeasured window is `v2.32.1..v2.36.0`, not `v2.32.1..v2.33.0`. Likewise `pi-subagents` has moved
`v0.68.0` → `v0.70.1`, which opens a second unmeasured window against area 09 that this file does
not yet name. (Per rule 4 the ledger's own `@v0.68.0` pins stay correct as pins.)
**Replacement text:**
> * **Areas 13 and 15** — on a **re-census**; their pins are stale and their newest windows are
>   measured by nobody. **Wider than this block first said**: `tmp/pi-mcp-adapter` is now at
>   `v2.36.0`, so the unmeasured window is `v2.32.1..v2.36.0`, not `..v2.33.0`. And a NEW window
>   has opened on area 09 itself — `tmp/pi-subagents` is at `v0.70.1`, three tags past the
>   `v0.68.0` every subagents row in this directory is pinned to.

---

## NAV prose · "THE COUNT AT HEAD … `python3 scripts/count_open_items.py`"
**Verdict:** STILL OPEN, citations drifted — on two counts: the script path is wrong, and the table
is one row stale.
**Evidence:**
```text
$ python3 scripts/count_open_items.py
python3: can't open file '/home/user/cyrup/scripts/count_open_items.py': [Errno 2] No such file or directory
$ python3 docs/gap-analysis/scripts/count_open_items.py | tail -14
...
09        9     0     0     1     8         0      44
...
TOTAL    75     0     0     9    66         6     605
```
Area 09 has closed one more low since (10 → 9 open, 43 → 44 closed); every other area row is
unchanged. The invoking command in both this block and the tenth edition should be
`python3 docs/gap-analysis/scripts/count_open_items.py`.
**Replacement text:** re-run the script and paste; the delta lines are
> ```
> 09        9     0     0     1     8         0      44
> TOTAL    75     0     0     9    66         6     605
> ```
> and the heading figure **76 open** becomes **75 open / 605 closed**.

## NAV prose · "the difference is **17 missing** … Seven remain, all `inspector.*`/`project.*`"
**Verdict:** **PREMISE FALSE.** This is the trap most likely to send a reader to build something
that exists.
**The sentences:** *"upstream `SUBAGENT_ACTIONS` @`v0.68.0` … is **57 verbs**; cyrup's list
(`extension/tool/text.rs:215`) is **42**; the difference is **17 missing** — `children.list`,
`worktree.{discard,cleanup}`, `lane.{status,recordMerge,recordSupersession}`,
`refine{,.show,.rollback}`, `inspector.{open,command,status,close}`, `project.{open,status,close}`,
`debug.run` … **That 17, plus `PB-8`, plus `UW-4`/`UW-5`, is the whole remaining subagents story**"*
and the footnote *"Seven remain, all `inspector.*`/`project.*`."*
**The truth:** **zero remain.** All 57 are advertised and dispatched; cyrup's list is 59 (57 + the
two cyrup-only verbs `append-step` and `inspect`).
**Evidence:**
```text
$ awk '/pub\(crate\) const SUBAGENT_ACTIONS/,/^\];/' \
    crates/cyrup-ext-subagents/src/extension/tool/text.rs | grep -c '^    "'
59
$ git -C tmp/pi-subagents show v0.68.0:src/shared/types.ts | grep -o 'SUBAGENT_ACTIONS = \[.*\]'
   # 57 entries, ending "schedule.delete"
$ sed -n '234,236p' crates/cyrup-ext-subagents/src/extension/tool/text.rs
/// As of VL-S6 this list names all **57** verbs of upstream's `SUBAGENT_ACTIONS`
/// (`shared/types.ts:2801` @v0.68.0) plus **2** cyrup dispatches that upstream does not advertise
/// there — `append-step` and `inspect` — for **59**.
```
All seven `inspector.*`/`project.*` verbs are in the list with in-source VL-S6 notes, dispatched
through `InspectorAction::from_wire` and `ProjectPaneAction::from_wire` guard arms. The cited line
`text.rs:215` is also dead — the const is at `:265`.
**Replacement text:**
> **What is genuinely left is verb-level and it is measurable in one command.** *(SUPERSEDED
> 2026-09-21 — the verb gap is CLOSED.)* Upstream `SUBAGENT_ACTIONS` @`v0.68.0`
> (`src/shared/types.ts:2801`) is **57 verbs**; cyrup's list (`extension/tool/text.rs:265`) is
> **59** — all 57 of upstream's at upstream's own indices, plus the two cyrup-only dispatches
> `append-step` and `inspect`. `VL-S6` landed the last seven (`inspector.{open,command,status,close}`,
> `project.{open,status,close}`); `VL-S10` landed `worktree.*`/`lane.*`, `VL-S13` the `refine*`
> trio, and `children.list`/`debug.run` landed 2026-09-19. **`PB-8` and `UW-4`/`UW-5` are closed
> too, so the "whole remaining subagents story" this paragraph named is finished.** What remains in
> areas 09/09a is the census's own rows, led by `SUBA-096` and `SUBA-054`'s decision.

## NAV prose · "Areas 09 + 09a carry **11 open rows, 2 medium, 66 closed**"
**Verdict:** STILL OPEN, citations drifted.
**Evidence:** the script now reports `09: 9 open / 1 medium / 44 closed` and `09a: 1 open /
1 medium / 23 closed` — **10 open, 2 medium, 67 closed**, and **8 of the 10 are low**.
**Replacement text:** *"Areas 09 + 09a carry **10 open rows, 2 medium, 67 closed** — and **8 of the
10 are `low`**"*.

## NAV prose · the structural warning's five unwired subsystems
**Verdict:** PREMISE FALSE for one named member.
**The sentence:** *"**Five subsystems have now shipped tested with no production caller** (`UW-1`,
`UW-8`, `UW-15`, `UW-4`/`UW-5`, `UW-21`)"*.
**The truth:** as a historical claim about shipping it is fine, but it is written in the present
perfect and reads as a live list. `UW-21` is wired and closed (`PB-8`, 2026-09-18) and `UW-8` is
struck CLOSED in `PARITY-GAPS.md:1989`. `UW-4`/`UW-5` are closed per this block's own ranked row 2.
**Replacement text:** keep the warning, add *"— of these, `UW-4`/`UW-5`, `UW-8` and `UW-21` have
since been connected and closed; the pattern is what this warning is about, not the current list."*

## NAV prose · "`crates/cyrup-ext-subagents` is **387 `.rs` files / 314 709 lines**"
**Verdict:** version-pinned to `cc7818b` — **add**, do not correct.
**Evidence at `14e6c56`:**
```text
$ find crates/cyrup-ext-subagents -name '*.rs' | wc -l
473
$ find crates/cyrup-ext-subagents -name '*.rs' -exec cat {} + | wc -l
379505
```
**Replacement text:** *"Measured rather than asserted, at `cc7818b`: **387 `.rs` files / 314 709
lines** against upstream's 286 `.ts` files @`v0.68.0` (at `14e6c56`: **473 files / 379 505 lines**)."*

---

## LEAD (tenth edition) · `ExtensionRegistry::resolve_shortcuts` has no production caller
**Verdict:** STILL OPEN, premise holds.
**Evidence:** `ExtensionHost::resolve_shortcuts` (`facade.rs:2283`) is the only wrapper, and its own
doc names `resolve_shortcut_specs` as *"the one production callers want"*. Nothing in
`cyrup-tui`, `cyrup-session-svc` or `cyrup` calls either `resolve_shortcuts`; the sole hit is a doc
comment in `cyrup-tui/src/tests/extension_shortcut.rs:87`.

## LEAD · `ClaudeCodeParser::AFTER_TERMINAL` / `AfterTerminal` dead-but-shipped
**Verdict:** STILL OPEN, premise holds.
**Evidence:** `adapters/claude_code.rs:184` defines the const; `:468-469` is the one test, and it
asserts `ClaudeCodeParser::AFTER_TERMINAL == AfterTerminal::RejectDuplicateTerminal` — a constant
against its own definition, exactly as the lead says. No other reader in `exec/external_cli/`.

## LEAD · "17 in-tree `agent-session.ts:2782` citations"
**Verdict:** STILL OPEN, citations drifted.
**Evidence:** `grep -rn "agent-session.ts:2782" crates/ --include=*.rs | wc -l` → **16**.
**Replacement text:** *"**16** in-tree `agent-session.ts:2782` citations (was 17 when filed) …"*

## LEAD · `runner_to_json_string`'s `capabilities` comment
**Verdict:** **PREMISE FALSE — and it has inverted, so the lead now points at the wrong file.**
**The sentence:** *"it claims upstream's capability ORDER survives, but the values are collected
into a `serde_json::Map` and this workspace deliberately does not enable `preserve_order`, so the
emitted object is alphabetical."*
**The truth:** (a) the function does **not** build a `Map` — it builds an ordered
`Vec<String>` of `"key":value` fragments (`runner/mod.rs:111-125`), so the order is hand-controlled
and does survive; and (b) the workspace **does** enable `preserve_order`:
```text
$ sed -n '189p' Cargo.toml
serde_json   = { version = "1", features = ["preserve_order"] }
$ sed -n '348,349p' Cargo.toml
# NOTE (2026-09-05): the `preserve_order` half of this rejection is now MOOT — the workspace
# declares `serde_json/preserve_order` deliberately …
```
The thing that is now wrong is the **in-source comment** at `runner/mod.rs:112-113`, which says
*"this crate does not enable serde_json's `preserve_order` feature"*. That statement is false at
HEAD and is the only thing left to fix.
**Replacement text:**
> * ~~**`runner_to_json_string`'s `capabilities` comment states the opposite of what the code
>   does**~~ — **INVERTED, re-read 2026-09-22.** The function never built a `serde_json::Map`; it
>   builds an ordered `Vec` of `"key":value` fragments (`runner/mod.rs:111-125`), so the order is
>   correct. What IS wrong is that comment's *reason*: it says *"this crate does not enable
>   serde_json's `preserve_order` feature"*, and the workspace has declared it since 2026-09-05
>   (`Cargo.toml:189`; `:348` records the change). One comment, no behaviour.

## LEAD · the two-lock TOCTOU on the export leaf
**Verdict:** STILL OPEN, premise holds.
**Evidence:** `cyrup-session-svc/src/session/transcript.rs:178-188` — the manager lock is taken and
dropped for `export_jsonl` (`:179-183`), then `self.export_state().await` is called separately at
`:187`. Two acquisitions, as filed.

## LEAD · "pi's two `exportSessionToHtml` guards are unported"
**Verdict:** **PRESCRIPTION WRONG — half of it landed, and re-doing it would duplicate a guard.**
**Evidence:** the in-memory guard IS ported, in the TUI rather than in the service:
```text
$ sed -n '153,158p' crates/cyrup-tui/src/app/execute_session.rs
                                // pi throws "Cannot export in-memory session to HTML"
                                // (`export-html/index.ts:243-245`) when there is no session file;
                                // an unpersisted session has no basename to build the name from.
                                None => self.state.transcript.push_status(
                                    "export error: cannot export an in-memory session to HTML",
                                ),
$ grep -rn "Nothing to export yet" crates/ --include=*.rs
crates/cyrup-acp/src/commands.rs:1138:   # a doc comment only — the guard itself is not ported
```
**Replacement text:**
> * **ONE of pi's two `exportSessionToHtml` guards is unported.** *"Cannot export in-memory session
>   to HTML"* (`export-html/index.ts:243-245`) **IS** ported, as the TUI's `None` arm
>   (`cyrup-tui/src/app/execute_session.rs:156`) — do not re-add it in `cyrup-session-svc`.
>   *"Nothing to export yet - start a conversation first"* (`:247-250`) is still missing: cyrup
>   writes a valid-but-empty document and reports success. `cyrup-acp/src/commands.rs:1138` names
>   the sentence in a doc comment but implements no guard.

## LEAD · `startup_resources_panel.rs` cites `(:1986, :5993)`
**Verdict:** STILL OPEN, premise holds. `:550` of that file still reads `(`:1986`, `:5993`)` while
`:467`, `:615`, `:636` and `:650` all use the correct `:1981-1982`.

## LEAD · the `[Extension issues]` panel omits pi's MIDDLE tier
**Verdict:** STILL OPEN, premise holds.
**Evidence:** `cyrup-tui/src/startup.rs` folds `resource_diagnostics` and `shortcut_diagnostics`
(`:266-267`, `:371`) and has no command-conflict tier. No
`getCommandDiagnostics`/`getBuiltInCommandConflictDiagnostics` analogue exists in `cyrup-tui` or
`cyrup-session-svc`.

## LEAD · "29 markdown table rows have the wrong cell count" / the unmeasured `cyrup-it` figures
**Verdict:** UNVERIFIABLE this pass. The first needs the five-line script the lead describes, which
was never committed (`docs/gap-analysis/scripts/` holds only `count_open_items.py`); the second
needs `cargo nextest`, forbidden this pass.

---

## Rows that are obsolete rather than open

None. Every non-CLOSED row in this file still describes a subsystem that exists. The four
PREMISE-FALSE entries (`UW-21`, `UW-7`, `children.list`, `VL-S7`'s worktree arms) are not obsolete
subsystems — three are *finished work the BLOCKED list does not know about*, and one (`UW-7`) is a
real open row whose stated blocker is the wrong one.

The one structural observation worth the orchestrator's attention: **all four false premises are in
the BLOCKED list, and three of them are contradicted by the RANKED table 120 lines above in the
same block.** The BLOCKED list was not re-derived when the ranked table was; it should be re-derived
from the ranked table every edition, or dropped in favour of a `blocked?` column the table already
has.
