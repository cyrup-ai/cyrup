# Sweep A — UW rows, audited at `14e6c56`

Scope: every `UW-` row in §2 of `docs/gap-analysis/PARITY-GAPS.md` (21 rows, `UW-1`…`UW-21`).
Method: re-grep every cyrup citation at `14e6c56`; re-verify every upstream citation at the tag the
row names, or — where the row names none but its numbers match a tag exactly — at that tag, and say
so rather than "correcting" a pinned citation. No cargo was run; greps and reads only.

**Rule-4 note, applied twice below.** `UW-3`'s and `UW-7`'s upstream line numbers name no tag in the
row text but match **v0.43.0 exactly**, which is the tag those cyrup files are pinned to. They are
NOT stale. v0.68.0 coordinates are offered as **additions**, never as corrections.

---

## Summary

| row | current text says | verdict | severity still right? |
|---|---|---|---|
| UW-1 | probe has no production caller — CLOSED | **CLOSED, citations current** | n/a (closed) |
| UW-2 | first-run wizard gated, never invoked — FIXED | **CLOSED, citations drifted** (the fix moved to `bootstrap.rs`) | n/a (closed) |
| UW-3 | child-watchdog NDJSON never read by parent | **STILL OPEN, citations drifted** (cyrup half only; upstream half is v0.43.0-pinned and correct) | yes — `medium` |
| UW-4 | watchdog review never runs a model turn — CLOSED | **CLOSED, citations current** | n/a (closed) |
| UW-5 | permission arbiter never runs a model turn — CLOSED | **CLOSED, citations current** | n/a (closed) |
| UW-6 | nothing ships a policy to a child — CLOSED | **CLOSED, citations drifted** (test range only) | n/a (closed) |
| UW-7 | fleet-status widget receives no keystrokes; "the host seam is still absent" | **PREMISE FALSE** (gap survives, stated reason does not) | yes — `medium`, but it is now a *wiring* job, not a *seam-build* job |
| UW-8 | mission workflow state never written — CLOSED | **CLOSED, citations drifted** | n/a (closed) |
| UW-9 | yolo-mode API has no publish seam — CLOSED (decision) | **CLOSED, citations current** | n/a (closed) |
| UW-10 | intercom overlays are render-only | **NARROWED** — overlay gap real; the "blocked by VL-S15" half is false; citations drifted | severity should drop `medium` → `low` (see row) |
| UW-11 | Copilot/Codex login unreachable — CLOSED-REFUTED, residual un-greped | **CLOSED, and the named residual is now PROVEN LIVE** | the residual deserves its own row at `low` |
| UW-12 | compaction cannot be cancelled — body says CLOSED | **CLOSED, citations drifted**; header still reads `*high*` unstruck | **no** — header must be struck |
| UW-13 | `capabilities` parsed and never read — body says FIXED | **CLOSED, citations drifted**; header still reads `***critical***` unstruck; `FsCaps::with_fs_root` no longer exists | **no** — header must be struck |
| UW-14 | SINGLE-mode `outputSchema` unadvertised | **CLOSED** | **no** — not `high`, not anything; it is closed |
| UW-15 | dedup implemented and never wired — CLOSED | **CLOSED, citations drifted** (the `:14` cite is the `use`, not the construction) | n/a (closed) |
| UW-16 | `defaultReads` never reaches a single run | **CLOSED** — the last third shipped | **no** — the whole row is now closed |
| UW-17 | ext widgets/headers/footers stored where nothing renders — CLOSED on area-file strength | **CLOSED, now independently verified** | n/a (closed) |
| UW-18 | settings rows that toggle values nothing reads | **STILL OPEN (partially), citations drifted** | yes — `low` |
| UW-19 | `keybindings.json` read once at boot — CLOSED on area-file strength | **CLOSED, now independently verified** | n/a (closed) |
| UW-20 | fuzzy matcher ported and unused — CLOSED on area-file strength | **CLOSED, now independently verified** | n/a (closed) |
| UW-21 | `async_status_snapshot` has no production caller — CLOSED | **CLOSED, citations drifted**; one cross-reference now false | n/a (closed) |

**CLOSED this pass (new closures the ledger does not yet record): `UW-14`, `UW-16`.**
**FALSE PREMISES: `UW-7` (whole row), `UW-10` (one clause), plus the unstruck headers on `UW-12`/`UW-13`.**
**Newly proven live defect with no id: the OAuth flow-loader registry (`UW-11`'s named residual).**

---

## UW-1 · The native modifier probe has no production caller

**Verdict:** CLOSED, citations current.

**Evidence:**
```
$ grep -n 'set_native_modifier_probe' crates/cyrup/src/main.rs crates/cyrup-tui/src/native_modifiers.rs
crates/cyrup/src/main.rs:241:    cyrup_tui::set_native_modifier_probe(native_modifier_probe::probe);
crates/cyrup-tui/src/native_modifiers.rs:62:pub fn set_native_modifier_probe(probe: ModifierProbe) -> Option<ModifierProbe> {
```

**Replacement text:** none needed. The "not live-verified on macOS Apple Terminal" caveat still stands.

---

## UW-2 · The first-run setup wizard is gated but never invoked

**Verdict:** CLOSED, citations drifted. The fix is real but has MOVED out of `main.rs` into
`bootstrap.rs` since the 2026-08-13 filing, so the row's own fix-description cites a call site that
is no longer there.

**Evidence:**
```
$ grep -rn 'run_first_time_setup' crates/cyrup/src/
crates/cyrup/src/bootstrap.rs:140:pub async fn maybe_run_first_time_setup(
crates/cyrup/src/bootstrap.rs:149:        || !crate::startup::should_run_first_time_setup(
crates/cyrup/src/bootstrap.rs:166:    let _ = crate::startup::run_first_time_setup(&theme, settings, detected).await?;
crates/cyrup/src/main.rs:445:    if bootstrap::maybe_run_first_time_setup(mode, &cli, &dirs, &env, &mut startup_settings).await?
crates/cyrup/src/startup.rs:98:pub fn should_run_first_time_setup(settings_path: &Path, agent_dir_overridden: bool) -> bool {
crates/cyrup/src/startup.rs:273:pub async fn run_first_time_setup(
```
The `--list-models` conjunct the row says is load-bearing is still present, now at
`crates/cyrup/src/bootstrap.rs:148` (`|| cli.list_models.is_some()`).

**Replacement text** (first bullet, opening sentence only — the rest of the bullet is unchanged):

- **FIXED 2026-08-13, per ADR-0011 (which decided `OQ-9` / `PARITY-GAPS` §6 q6 — *not* `OQ-6`; the escalation below is one of the two mis-citations ADR-0011 records).** The call now lives in its own gate function: `crates/cyrup/src/bootstrap.rs:140` (`maybe_run_first_time_setup`) evaluates the condition at `:148-149` and calls `crate::startup::run_first_time_setup(&theme, settings, detected)` at `:166`; `crates/cyrup/src/main.rs:445` invokes the gate at **pi's position** — after `startupSettingsManager` + its diagnostics, **before** `apply_settings_session_dir` — mirroring `main.ts:610 → 615-617 → 625-630` *(citations refreshed 2026-09-22: the call was in `main.rs` when this row was filed and moved to `bootstrap.rs` afterwards; `startup.rs:256` is now `startup.rs:273`)*.

---

## UW-3 · Child-watchdog NDJSON status events are never read by the parent

**Verdict:** STILL OPEN, citations drifted. The central claim holds: the three parent-side predicates
have **zero** callers of any kind outside their own module. The cyrup coordinates have all moved and
the emit-side pair is now wrong. The **upstream half is v0.43.0-pinned and correct — do not touch
it.**

**Evidence — the claim itself (still true):**
```
$ grep -rn 'is_child_watchdog_status_event\|child_watchdog_is_active\|accept_child_watchdog_event' crates/ --include=*.rs
watchdog/child_status.rs:489:pub fn is_child_watchdog_status_event(value: &Value) -> bool {
watchdog/child_status.rs:512:pub fn child_watchdog_is_active(snapshot: Option<&ChildWatchdogStateSnapshot>) -> bool {
watchdog/child_status.rs:531:pub fn accept_child_watchdog_event(
  (all other hits are this file's own doc comments and its `#[cfg(test)]` block at :732-:864)
$ grep -n 'watchdog' crates/cyrup-ext-subagents/src/exec/ndjson.rs
  (no output)
```

**Evidence — cyrup citations that drifted:**

| row says | current |
|---|---|
| `child_status.rs:480` | `:489` (already fixed in the refresh bullet, still wrong in the `cyrup:` bullet) |
| `child_status.rs:497` | `:512` (ditto) |
| `child_status.rs:516` | **`:531`** — the refresh bullet never gave this one a new number |
| `child_status.rs:461-473` "states this in-tree" | **`:471-485`** |
| emit side `prompt_runtime.rs:1701` | **`prompt_runtime.rs:2394`** (`register_child_watchdog(...)`) + **`:2404`** (`stdout_status_sink()`) |
| emit side `exec/mod.rs:1727-1737` | **gone** — `grep -n watchdog crates/cyrup-ext-subagents/src/exec/mod.rs` returns nothing |
| `exec/spawn_plan.rs:1045-1055` | current ✓ (`:1045` resolve, `:1052` encode, `:1055` env key) |
| `exec/spawn_plan.rs:2333-2341` | current ✓ |

**Evidence — the "two readers" clause is now half-wrong:** `exec/ndjson.rs` still has zero watchdog
references, but `background/` now has three (`recovery_descriptor.rs:81`, `runner_main/config.rs:257`,
`runner_main/executor.rs:253`) — all `permission_arbiter::PermissionRules`, none of them a child
status reader. The substance survives; the sentence does not.

**Evidence — upstream, PINNED, verify before changing:** the row's numbers are **v0.43.0**:
```
$ git -C tmp/pi-subagents show v0.43.0:src/watchdog/child-status.ts | grep -n 'export function'
167:export function isChildWatchdogStatusEvent(...)
181:export function childWatchdogIsActive(...)
186:export function acceptChildWatchdogEvent(...)
$ git -C tmp/pi-subagents show v0.43.0:src/runs/foreground/execution.ts | sed -n '585p;846p;848p;857p'
   (585 = startFinalDrain / childWatchdogIsActive; 846/848/857 = the isChildWatchdogStatusEvent fold)
$ git -C tmp/pi-subagents show v0.43.0:src/runs/background/subagent-runner.ts | sed -n '626p;628p;640p;831p;2711p'
   (all four land on the fold / the tail timer / the second fold)
```
All correct at v0.43.0. At v0.68.0 they are `child-status.ts:159`/`:182`/`:187`,
`execution.ts:692`/`:1015`/`:1017`/`:1082`, `subagent-runner.ts:3016`/`:3017`/`:3033`.

**Severity:** `medium` still right.

**Replacement text** (replace the first and third bullets; leave the `upstream:` bullet ALONE):

- **Re-greped 2026-09-22 at `14e6c56`:** `is_child_watchdog_status_event` (`watchdog/child_status.rs:489`), `child_watchdog_is_active` (`:512`) and `accept_child_watchdog_event` (`:531`) have **zero** callers of any kind outside their own module. The CONFIG half IS wired and was not before: `exec/spawn_plan.rs:1045-1055` resolves and encodes `CHILD_WATCHDOG_CONFIG_ENV` into the child env, and `:2333-2341` decodes it. **So the child is now told to run a watchdog and the parent still cannot read what it reports** — the gap moved one step later, it did not close
- upstream: `pi-subagents/src/runs/foreground/execution.ts:846`, `:848`, `:857`, `:585`; `runs/background/subagent-runner.ts:626`, `:628`, `:640`, `:831`, `:2711-2712`; definitions `src/watchdog/child-status.ts:167`, `:181`, `:186` — **all @v0.43.0, the tag `child_status.rs` is pinned to, and all re-verified correct at that tag 2026-09-22.** At v0.68.0 the same symbols are `child-status.ts:159`/`:182`/`:187`, `execution.ts:692`/`:1015`/`:1017`/`:1082` and `subagent-runner.ts:3016`/`:3017`/`:3033`; the fold's shape is unchanged
- cyrup: `watchdog/child_status.rs:489`, `:512`, `:531` — production callers zero. `exec/ndjson.rs` contains no watchdog reference at all; `background/`'s three watchdog references (`recovery_descriptor.rs:81`, `runner_main/config.rs:257`, `runner_main/executor.rs:253`) are all `permission_arbiter::PermissionRules`, not a child-status reader. The child EMIT side is wired (`prompt_runtime.rs:2394` calls `register_child_watchdog`, bound to `stdout_status_sink()` at `:2404`) *(citations refreshed 2026-09-22; the old `prompt_runtime.rs:1701` and `exec/mod.rs:1727-1737` are both dead — `exec/mod.rs` has no watchdog reference of any kind now)*
- observable: a child mid-watchdog-review when its agent settles is killed by the ordinary final-drain timer instead of held open by the watchdog tail timer, so its blocker/concern warnings are lost. cyrup emits `subagent.watchdog.status` frames its own parent discards as an unknown event type. `child_status.rs:471-485` states this in-tree.

---

## UW-4 · Watchdog review never runs a model turn — CLOSED

**Verdict:** CLOSED, citations current.

**Evidence:**
```
$ grep -n 'ModelTurnReviewAgent' crates/cyrup-ext-subagents/src/watchdog/register_main.rs crates/cyrup-ext-subagents/src/prompt_runtime.rs
watchdog/register_main.rs:177:                Arc::new(super::review::ModelTurnReviewAgent::new(
prompt_runtime.rs:2491:            Arc::new(crate::watchdog::review::ModelTurnReviewAgent::new(
```
Both production bindings the row names are at the lines it names.

**Replacement text:** none needed.

---

## UW-5 · Watchdog permission arbiter never runs a model turn — CLOSED

**Verdict:** CLOSED, citations current.

**Evidence:**
```
$ grep -n 'ModelTurnPermissionAgent' crates/cyrup-ext-subagents/src/prompt_runtime.rs
2445:                crate::watchdog::permission_arbiter::ModelTurnPermissionAgent::new(
2864:    /// Production binds the real `ModelTurnPermissionAgent` here (...)   ← doc comment in the cfg(test) block
```
`:2445` is the single production binding, exactly as the row says, and the row's own correction
about `:2858` being inside `#[cfg(test)]` remains accurate (the test module's doc is now at `:2864`).

**Replacement text:** none needed. The `cancel: None` residual the row names is still live.

---

## UW-6 · Nothing ever ships a permission policy to a child — CLOSED

**Verdict:** CLOSED, citations drifted (one range only).

**Evidence:**
```
$ grep -n 'PERMISSION_POLICY_ENV' crates/cyrup-ext-subagents/src/exec/spawn_plan.rs
1178:    // SUBA-073 — pi ships the resolved permission policy to the child in `PERMISSION_POLICY_ENV`
1202:            crate::watchdog::permission_arbiter::PERMISSION_POLICY_ENV.to_string(),
3202:            .get(pa::PERMISSION_POLICY_ENV)
3252:                .contains_key(pa::PERMISSION_POLICY_ENV)
```
`:1178` and `:1202` are current. The pinning test the row cites as `:3171-3252` now spans
`:3202-3252`.

**Replacement text** (closure bullet, one clause):

> `:3202-3252` is the test that pins it reaching the child as `CYRUP_SUBAGENT_PERMISSION_POLICY`

---

## UW-7 · The fleet-status widget receives no keystrokes

**Verdict:** **PREMISE FALSE.** The observable gap is real — the widget still receives no keystrokes —
but the row's stated *reason* is false, and it is false twice, in the sentence the orchestrator
already caught and in the cross-reference beside it.

**The false sentence, verbatim:**
> "The host seam is still absent — `cyrup-ext/src/host/services.rs` has nothing resembling
> `on_terminal_input`"

and, in the `cyrup:` bullet:
> "The host has no seam to wire it to: `cyrup-ext/src/host/services.rs` has `set_widget` (`:260`) and
> `open_overlay` (`:224`) but nothing resembling `on_terminal_input`."

**What is true instead:** the seam is built end to end, under the id `EXT-021`, and it is `native.rs`
+ `facade.rs` + `registry.rs`, not `host/services.rs` — which is why a grep confined to the one path
the row names missed it.

```
$ grep -rn 'on_terminal_input\|subscribe_terminal_input' crates/ --include=*.rs
cyrup-ext/src/native.rs:334:    /// EXT-021: whether this extension subscribed to raw terminal input. ...
cyrup-ext/src/native.rs:425:    pub fn subscribe_terminal_input(&mut self) {          ← InitApi declaration
cyrup-ext/src/native.rs:757:    fn on_terminal_input(&self, _data: &str) -> Option<crate::TerminalInputResult> {   ← trait method
cyrup-ext/src/registry.rs:319:    terminal_input_subscribers: Vec<ExtensionId>,
cyrup-ext/src/registry.rs:595:    pub fn subscribe_terminal_input(&self, owner: ExtensionId) -> Result<(), ExtError> {
cyrup-ext/src/registry.rs:605:    pub fn unsubscribe_terminal_input(&self, owner: &ExtensionId) -> Result<(), ExtError> {
cyrup-ext/src/facade.rs:528:            self.registry.subscribe_terminal_input(id.clone())?;   ← load-order registration
cyrup-ext/src/facade.rs:1472:    pub async fn terminal_input(&self, data: &str) -> TerminalInputDecision {
cyrup-ext/src/facade.rs:1507:                native.on_terminal_input(data)                      ← native dispatch
cyrup-ext/src/facade.rs:1522:            return match ext.on_terminal_input(data).await {        ← WASM dispatch
cyrup-ext/src/host/live.rs:323:    async fn subscribe_terminal_input(&mut self) {
cyrup-ext/src/host/live.rs:1980:    pub async fn on_terminal_input(
cyrup-ext-sdk/src/api.rs:907:    pub fn on_terminal_input(&mut self, handler: impl TerminalInputHandler) {
cyrup-ext-sdk/src/guest.rs:380:pub fn on_terminal_input(data: String) -> Option<...>
cyrup-ext-sdk/src/macros.rs:118:                fn on_terminal_input(                              ← export_extension! macro
```

**What is actually missing — two things, both callers, neither a seam:**

1. **Nothing pumps keystrokes into the seam.** `ExtensionFacade::terminal_input` (`facade.rs:1472`)
   has only test callers:
   ```
   $ grep -rn '\.terminal_input(' crates/ --include=*.rs
   crates/cyrup-ext/src/tests/native_dispatch.rs:1619, :1639, :1670, :1676, :1722, :1758
   ```
   No hit anywhere in `cyrup-tui` or `cyrup`.
2. **The subagents extension never subscribes.** `grep -rn terminal_input crates/cyrup-ext-subagents/`
   returns nothing, so `SubagentFleetStatus::handle_key` is never bound to the seam even if it were fed.

**Evidence — cyrup citations that drifted (all three symbols moved):**

| row says | current |
|---|---|
| `tui/fleet_status.rs:740` (refresh bullet) / `:764` (`cyrup:` bullet) | **`:917`** |
| `press` at `:1169` | **`:1452`** (`FleetStatusKey::press`) |
| `is_widget_registered` at `:711` | **`:864`** |
| test callers `:1554, :1569, :1586, :1588, :1599` | **`:1839, :1854, :1871, :1873, :1884, …`** |
| `host/services.rs` `set_widget (:260)` / `open_overlay (:224)` | **`set_widget` `:401`, `open_overlay` `:332`** |
| PUBLISH half `extension.rs:9489, :9889, :9978` | **`extension.rs` no longer exists** — the crate was split; the widget publish path is `extension/host/slash.rs` (`refresh_fleet_status_widget`, the `ExtMode::Rpc` arm at `:248`) |

**Evidence — upstream is v0.43.0-pinned and CORRECT; do not renumber:**
```
$ git -C tmp/pi-subagents show v0.43.0:src/tui/fleet-status.ts | grep -n 'onTerminalInput\|handleKey(data: string)'
282:		if (typeof ui.onTerminalInput === "function") {
283:			this.inputUnsubscribe = ui.onTerminalInput((data) => this.handleKey(data));
352:	handleKey(data: string): {...}
```
At v0.68.0 the same lines are `:577-578` and `:696`.

**Evidence — the cross-reference is now wrong in the other direction too.** The row was corrected on
2026-09-16 to say UW-7 "needs its own seam" and should not be scheduled behind VL-S15. That
correction is itself stale: the seam UW-7 needs is `EXT-021`, and `EXT-021` **landed** (the grep
above; `facade.rs:524` even carries the "EXT-021: terminal-input subscription, also in LOAD ORDER"
comment). The row's closing "Same missing seam family as VL-S15 and area 06 `EXT-021`" names two
things that have both shipped.

**Severity:** `medium` still right as a user-visible rating, but the *shape* changed: this is no
longer a seam-build blocked on `cyrup-ext`, it is two call sites. It should be re-scoped from `M` to
`S` wherever it carries a size.

**Replacement text** (replace the whole row body; keep the header line as-is except the dating):

**UW-7 · The fleet-status widget receives no keystrokes** — *medium* · **STILL OPEN at `14e6c56`; PREMISE CORRECTED + citations refreshed 2026-09-22**
- **Re-greped this pass, and the row's own explanation was FALSE.** `SubagentFleetStatus::handle_key` is now `tui/fleet_status.rs:917` (was `:740`, before that `:764`) and every caller is a test in the same file (`:1839`, `:1854`, `:1871`, `:1873`, `:1884`, …). `press` is `:1452` (was `:1169`) and `is_widget_registered` is `:864` (was `:711`)
- **"The host seam is still absent" is wrong, and so is the path it looked in.** The raw-terminal-input seam is built end to end under area 06 `EXT-021`, in `native.rs`/`registry.rs`/`facade.rs` rather than `host/services.rs`: `InitApi::subscribe_terminal_input` (`cyrup-ext/src/native.rs:425`), the trait method `NativeExtension::on_terminal_input` (`:757`), the load-ordered subscriber list (`registry.rs:319`, registered `facade.rs:528`, `registry.rs:595`), the fold `ExtensionFacade::terminal_input` (`facade.rs:1472`) with both dispatch arms (native `:1507`, WASM `:1522`), the WASM host side (`host/live.rs:1980`) and the guest/SDK half (`cyrup-ext-sdk/src/api.rs:907`, `guest.rs:380`, `macros.rs:118`). **`EXT-021` has landed; this row must stop naming it as a blocker.**
- **What is actually missing is two CALLERS, not a seam.** (1) Nothing pumps keystrokes in: `grep -rn '\.terminal_input(' crates/` finds only `cyrup-ext/src/tests/native_dispatch.rs:1619,:1639,:1670,:1676,:1722,:1758` — no hit in `cyrup-tui` or `cyrup`, so the TUI's key loop never reaches the facade. (2) The subagents extension never subscribes: `grep -rn terminal_input crates/cyrup-ext-subagents/` returns **nothing**, so `handle_key` is bound to no source. **Re-scope from `M` to `S`: this is two call sites over a finished seam.**
- **Cross-reference, corrected twice now.** The 2026-09-16 correction ("VL-S15's `register_shortcut` landed and did not help, because a registered chord is not a per-keystroke stream") is still true about `register_shortcut` (`cyrup-ext/src/native.rs:406`, dispatched `facade.rs:501-502`) — but its conclusion that "UW-7 needs its own seam" is now stale, because that seam was built. Both VL-S15 and `EXT-021` have shipped; UW-7 is blocked on neither
- upstream: `src/tui/fleet-status.ts:282-283` (`ui.onTerminalInput((data) => this.handleKey(data))`), handler `:352` — **both @v0.43.0, re-verified correct at that tag 2026-09-22.** At v0.68.0 the same lines are `:577-578` and `:696`
- cyrup: `tui/fleet_status.rs:917` (`handle_key`) has zero production callers, as do `press` (`:1452`) and `is_widget_registered` (`:864`). `cyrup-ext/src/host/services.rs` carries `set_widget` (`:401`) and `open_overlay` (`:332`); the terminal-input seam is not in that file and never was — it is `native.rs`/`facade.rs`/`registry.rs`, addresses above. The PUBLISH half is live via `extension/host/slash.rs`'s `refresh_fleet_status_widget` (`:248`) *(the old `extension.rs:9489`/`:9889`/`:9978` citations are dead — that file was split)*
- observable: while subagents run, pressing ↓ or ← on an empty editor expands pi's widget into a selectable roster whose Enter opens the fleet inspector; in cyrup the widget is display-only and those keys fall through to the editor.

---

## UW-8 · Mission workflow state is never written — CLOSED

**Verdict:** CLOSED, citations drifted.

**Evidence:**
```
$ grep -n 'MissionWorkflowStateStore' crates/cyrup-ext-subagents/src/extension/tool/routing.rs
606:                    crate::missions::MissionWorkflowStateStore::create(
$ grep -n 'pub fn create\|create_mission_workflow_state\|WorkflowStateStore for' crates/cyrup-ext-subagents/src/missions/workflow_state.rs
248:pub fn create_mission_workflow_state(
292:    pub fn create(location: &MissionStoreLocation, mission_id: &str) -> MissionResult<Self> {
300:impl crate::workflows::scripted::WorkflowStateStore for MissionWorkflowStateStore {
```

| row says | current |
|---|---|
| `extension/tool/routing.rs:571` | **`:606`** |
| `missions/workflow_state.rs:294` (`create_mission_workflow_state`) | **`:248`** (the free fn) / **`:292`** (the store's `create`) |
| `missions/workflow_state.rs:298` (`impl WorkflowStateStore`) | **`:300`** |

**Replacement text** (closure bullet, first sentence):

> **CLOSED, re-greped 2026-09-16 at `cc7818b`, citations refreshed 2026-09-22 at `14e6c56`, and it closed exactly the way this row predicted.** `extension/tool/routing.rs:606` constructs `crate::missions::MissionWorkflowStateStore::create(…)` (`missions/workflow_state.rs:292`) on the production workflow-launch path; that store wraps `create_mission_workflow_state` (`:248`) and implements `workflows::scripted::WorkflowStateStore` (`:300`), so the scripted runtime's `state.get` / `state.set` now reach the real file.

---

## UW-9 · The yolo-mode runtime API has no publish seam and no caller — CLOSED

**Verdict:** CLOSED, citations current. Spot-checked only — the closure is a recorded *decision*
("a reader checking this row by grepping for callers will still find none; that is the answer"), so
a zero-caller grep is consistent with the closure rather than evidence against it. I did not re-read
`command.rs:57-60` line by line.

**Replacement text:** none.

---

## UW-10 · The intercom compose and session-picker overlays are render-only

**Verdict:** **NARROWED**, and one clause is **PREMISE FALSE**.

- **Still open:** `open_overlay` is never called from `crates/cyrup-intercom/` — `grep -rn 'open_overlay' crates/cyrup-intercom/` returns **nothing**. The overlays remain render-only. That half is intact.
- **False clause:** the row's last line says *"so only the `alt+m` path stays blocked by VL-S15 — the slash-command path is reachable today."* VL-S15 is **CLOSED** — `InitApi::register_shortcut` exists at `cyrup-ext/src/native.rs:406` and is dispatched through `facade.rs:501-502`. Nothing external blocks `alt+m` any more; what is missing is that `IntercomExtension::init` never calls it, and its in-tree comment still claims the seam does not exist.

**Evidence:**
```
$ grep -rn 'open_overlay' crates/cyrup-intercom/
   (no output)
$ grep -n 'api\.' crates/cyrup-intercom/src/extension.rs
538: api.register_tool(...)          548: api.register_tool(...)
559: api.register_entry_renderer(...) 563: api.register_message_renderer(...)
567: api.register_command(...)        577: api.register_command(...)
586: api.subscribe(...)               610/611: api.subscribe_bus(...)
   ← no register_shortcut call
$ sed -n '564,566p' crates/cyrup-intercom/src/extension.rs
        // The `/intercom` overlay command (pi `registerCommand("intercom", …)`, index.ts:1877). cyrup
        // has no `register_shortcut`, so the `alt+m` binding degrades to this command (the port doc
        // §4.3); `execute_command` renders the session picker + drives the compose send.
$ grep -n 'fn register_shortcut' crates/cyrup-ext/src/native.rs
406:    pub fn register_shortcut(&mut self, key: impl Into<String>, description: Option<String>) {
```
**`crates/cyrup-intercom/src/extension.rs:565` is a false in-tree comment** — the fourteenth instance
of §1's "a comment that documents a divergence the code no longer has". `ui/compose.rs:9-10` and
`ui/session_list.rs:7` carry the same stale "Phase-6 `register_shortcut` gap" claim.

**Evidence — citations that drifted:**

| row says | current |
|---|---|
| `ui/compose.rs:86` (`handle_input`) | **`:109`** |
| `ui/compose.rs:74`, `:79` | `input()` **`:74`** ✓, `set_sending` **`:80`** |
| `ui/session_list.rs:75` | **`:88`** |
| `ui/mod.rs:29` | **`:29`** ✓ (the "is likewise not reachable yet" line) |
| `ui/mod.rs:12-19` "blames a missing `register_message_renderer` AND `register_shortcut`" | **`:17-18`** now says the opposite about the renderer ("`InitApi::register_message_renderer` DOES exist … that half of this note was stale, corrected 2026-08-15"); the stale `register_shortcut` claim lives at `:27-30` and in `compose.rs:9-10` / `session_list.rs:7` |
| `extension.rs:404-407` one-shot render | **`:405-420`** (`ComposeOverlay::new(...).render(...)` at `:416-417`, the "Type `/intercom …`" tail at `:420`) |
| `register_message_renderer` at `native.rs:270` | **`:370`** |

**Severity:** should drop `medium` → `low`. With both seams present and unused, this is now a
three-call-site wiring job inside one crate, and the slash-command path already delivers the
function (degraded).

**Replacement text** (replace the final `observable:` bullet and append one new bullet):

- observable: `/intercom <target>` prints a picture of a compose box and asks the user to retype the whole command with a body.
- **Rationale correction, second pass (2026-09-22) — the previous correction is itself stale.** The row said "only the `alt+m` path stays blocked by VL-S15". **VL-S15 is CLOSED**: `InitApi::register_shortcut` is `cyrup-ext/src/native.rs:406`, dispatched `facade.rs:501-502`, and `HostServices::open_overlay` is `host/services.rs:332`. **Nothing outside this crate blocks the overlay path any more.** What blocks it is three call sites inside `cyrup-intercom` that were never written: `IntercomExtension::init` (`extension.rs:538-611`) registers tools, renderers, commands and bus subscriptions and never calls `register_shortcut`; nothing calls `open_overlay`; and three in-tree comments still assert the seam is absent — `extension.rs:565` ("cyrup has no `register_shortcut`"), `ui/compose.rs:9-10` and `ui/session_list.rs:7` ("the Phase-6 `register_shortcut`/overlay-host gap"). **All three comments are false at HEAD and must be fixed in the same change as the code.** Re-rate `low`: this is wiring inside one crate, not a seam
- cyrup: `ui/compose.rs:109` (`handle_input`), `:74`, `:80` and `ui/session_list.rs:88` have zero production callers; `open_overlay` is never called from this crate (`grep -rn open_overlay crates/cyrup-intercom/` → nothing). The only production use is a one-shot `render` at `extension.rs:405-420` whose output ends "Type `/intercom {target} <message>` to send." *(citations refreshed 2026-09-22; `register_message_renderer` is now `cyrup-ext/src/native.rs:370`)*

---

## UW-11 · Copilot and Codex login flows are fully written and unreachable — CLOSED-REFUTED

**Verdict:** CLOSED stands. **The residual this row explicitly left un-greped is now PROVEN LIVE**,
and it is worse than the row guessed: not just the registrar, the whole registry.

The row says, verbatim:
> "`register_bundled_oauth_flow_loaders` (`auth/oauth/load.rs:111`) was not re-greped this pass. If
> it still has zero callers it is a live UW-class defect with no id."

**Evidence:**
```
$ grep -rn 'register_bundled_oauth_flow_loaders' crates/ --include=*.rs
crates/cyrup-provider/src/auth/oauth/load.rs:12:   (module doc)
crates/cyrup-provider/src/auth/oauth/load.rs:64:   (doc link)
crates/cyrup-provider/src/auth/oauth/load.rs:111:pub fn register_bundled_oauth_flow_loaders(loaders: OAuthFlowLoaders) {
crates/cyrup-provider/src/auth/oauth/load.rs:239,277,322   ← all three AFTER `#[cfg(test)]` at :198
crates/cyrup-provider/src/auth/oauth/mod.rs:62     (re-export)
crates/cyrup-provider/src/auth/mod.rs:19           (re-export)
crates/cyrup-provider/src/auth/oauth/github_copilot.rs:838  (doc link)
crates/cyrup-provider/src/auth/oauth/kimi_coding.rs:766     (doc link)
$ grep -n '#\[cfg(test)\]' crates/cyrup-provider/src/auth/oauth/load.rs
198:#[cfg(test)]
$ grep -rn 'registered_oauth_flows' crates/ --include=*.rs | grep -v 'auth/oauth/load.rs'
   (no output)
```
So: **zero production callers for the registrar, and zero callers of any kind outside the module for
`registered_oauth_flows` (`load.rs:118`) — its doc even advertises it "for status UIs that list which
logins are actually available", and no status UI reads it.** The registry (`load.rs:104`) is written
by nobody and read only by its own two accessors. This is textbook UW-class: a complete, tested,
documented subsystem with no production caller.

**Severity of the residual:** `low` — `/login` works through `provider_auth().oauth`, so nothing
user-visible is broken; this is dead weight plus a doc comment that promises a consumer.

**Replacement text** (replace the last two sentences of the closure bullet):

> **The second half of this row's Fix is NOT discharged and is not tracked anywhere**: "either populate the flow registry or delete it". **Re-greped 2026-09-22 at `14e6c56` and the answer is: zero.** `register_bundled_oauth_flow_loaders` (`auth/oauth/load.rs:111`) has no production caller — its only calls are `:239`, `:277` and `:322`, all inside the `#[cfg(test)]` block that opens at `:198` — and the registry is dead on the read side too: `registered_oauth_flows` (`:118`), whose own doc says it exists "for status UIs that list which logins are actually available", has **zero** callers outside the module. The backing `registry()` (`:104`) is written by nobody. **This is a live UW-class defect at `low`, with no owning id; area 01 must file one.** **The body below is the original filing and is kept as history.**

---

## UW-12 · Compaction cannot be cancelled from the shipped binary

**Verdict:** CLOSED (the body already says so), citations drifted, **and the header is wrong** — the
title is not struck and still reads `*high*`, so a reader scanning headers schedules a closed row.
This is exactly the failure mode the sweep exists to prevent.

**Evidence — the chain is live at `14e6c56`:**
```
$ grep -n 'CompactionStart\|compacting' crates/cyrup-tui/src/app/events_fold.rs
255:            AgentSessionEvent::CompactionStart { reason } => {
264:                // ` (<key> to cancel)` suffix is appended by the band from the live keymap.
266:                    CompactionReason::Manual => "Compacting context...".to_string(),
284:                self.state.compacting = true;
$ grep -n 'AbortCompaction' crates/cyrup-tui/src/app/input.rs crates/cyrup-tui/src/app/run_action.rs crates/cyrup-session-svc/src/command.rs
app/input.rs:295:                    return AppAction::AbortCompaction;
app/run_action.rs:91:            AppAction::AbortCompaction => {
app/run_action.rs:92:                ctx.session.abort_compaction();
cyrup-session-svc/src/command.rs:34:    AbortCompaction,
cyrup-session-svc/src/command.rs:151/152:            C::AbortCompaction => { self.abort_compaction(); }
$ grep -n 'pub fn abort_compaction' crates/cyrup-session-svc/src/session/compaction.rs
406:    pub fn abort_compaction(&self) {
$ grep -n 'AbortCompaction' crates/cyrup-tui/src/tests/escape_chain.rs
276, 287   (escape_during_a_compaction_aborts_the_compaction, and the compaction_end restore)
$ grep -n 'escape to cancel' crates/cyrup-tui/src/tests/compaction_status.rs
133:            s.contains("Compacting context... (escape to cancel)"),
```

**Evidence — citations that drifted:**

| row says | current |
|---|---|
| `app/events_fold.rs:195-223` (CompactionStart arm) | **`:255-284`** (`compacting = true` at `:284`) |
| `app/events_fold.rs:222` arms `state.compacting`, band at `:220` | **`:284`** / band text `:266` |
| `app/input.rs:144-146` | **`:295`** |
| `app/run_action.rs:53-54` | **`:91-92`** |
| `session.rs:1900` (`abort_compaction`) | **`cyrup-session-svc/src/session/compaction.rs:406`** |
| `cyrup-session-svc/src/command.rs:32`, `:116-118` | **`:34`**, **`:151-152`** |
| `app/render.rs:86-90` prints "(esc to cancel)" | **no literal there** — the band composes `(<key> to cancel)` from the live keymap; the comment is `app/render.rs:98`, the string is asserted at `tests/compaction_status.rs:133` |
| `cyrup-tui/src/tests/escape_chain.rs:233`/`:244` | **`:276`/`:287`** |

**Severity:** **no.** Strike the header.

**Replacement text** (header line; the body bullets keep their text with the numbers above
substituted):

**~~UW-12 · Compaction cannot be cancelled from the shipped binary, and the indicator advertises the dead key~~** — ~~*high*~~ **CLOSED — REFUTED** · area 03 `SESS-040` struck 2026-08-15; whole chain re-verified at `14e6c56` 2026-09-22

and the closure bullet:

- **CLOSED — do not schedule.** Area 03 struck `SESS-040` as REFUTED on 2026-08-15 and the whole chain is live at `14e6c56`: `app/events_fold.rs:284` arms `state.compacting` beside the band text at `:266`; `app/input.rs:295` returns `AppAction::AbortCompaction` on Escape ahead of pi's four-branch chain, exactly as `interactive-mode.ts:3080-3086` shadows it; `app/run_action.rs:91-92` calls `ctx.session.abort_compaction()` (`cyrup-session-svc/src/session/compaction.rs:406`), reached from the command enum at `command.rs:34`/`:151-152`. Pinned by `cyrup-tui/src/tests/escape_chain.rs:276`/`:287`; the band's "(escape to cancel)" is pinned at `tests/compaction_status.rs:133` — note the suffix is composed from the live keymap, not a literal in `app/render.rs` (see the comment at `render.rs:98`). `REPRO-LOG.md`'s "one of seventeen still open" is corrected in the same direction

`SESS-041`/`SESS-042` were not re-verified this pass and stay as the row leaves them.

---

## UW-13 · `ExtensionManifest.capabilities` is parsed and never read

**Verdict:** CLOSED (the blockquote already says FIXED), citations drifted, **header wrong** — the
title is unstruck and reads `***critical*** (raised from high this pass)`. Same scanning hazard as
UW-12, on the register's only critical.

**Evidence — the consumer exists:**
```
$ grep -n 'load_wasm_with_caps\|manifest.capabilities' crates/cyrup-ext/src/facade.rs
1915:    pub async fn load_wasm(
1921:        self.load_wasm_with_caps(id, bytes, services, &Capabilities::host_granted())
1938:    pub async fn load_wasm_with_caps(
2116:        // with no manifest parameter — so `disc.manifest.capabilities` was parsed and dropped, and a
2129:        self.load_wasm_with_caps(id.clone(), &bytes, services, &disc.manifest.capabilities)
```
`load_discovered` passes the grant. The row's core claim ("the manifest provably cannot reach
instantiation") is dead.

**Evidence — the deny-by-default requirement the row set was met:**
```
$ grep -n 'capabilities' crates/cyrup-ext/src/loader.rs
345:        capabilities: crate::manifest::Capabilities::none(),
426:                    capabilities: crate::manifest::Capabilities::none(),
```
Both synthesis sites are the EMPTY grant, which is precisely what the row's last sentence demanded.

**Evidence — one cited symbol is GONE, not moved:**
```
$ grep -rn 'with_fs_root' crates/ --include=*.rs
   (no output)
```
`EXT-055`'s `FsCaps::with_fs_root` does not exist at any path in the workspace. It was replaced by
`FsCaps::single(root)` (`host/services.rs:1782`) and `FsCaps::from_grants(base, grants)` (`:1800`),
both with live callers on `HostConfig`, and the enforcement messages are at `:1446`, `:1469`, `:1473`
("path `{path}` is not inside a `write:` grant in `capabilities.fs`"). Per the rules I do not
substitute a replacement citation for the row's symbol; I report it gone with the grep above.

**Evidence — citations that drifted:**

| row says | current |
|---|---|
| `manifest.rs:20` declares the field | **`:46`** (`pub capabilities: Capabilities` on `ExtensionManifest`, struct at `:36`) |
| `manifest.rs:23-35` the `Capabilities{fs,exec,net,ui}` shape | **`:62`** |
| `loader.rs:213`/`:259` synthesise defaults | **`:345`/`:426`**, and they are `Capabilities::none()`, not `Default::default()` |
| `facade.rs:1166-1184` (`load_discovered`) | **`~:2110-2130`** (the `load_wasm_with_caps` call at `:2129`) |
| `facade.rs:1063-1070` (`load_wasm` signature) | **`:1915-1921`**, and it now delegates to `load_wasm_with_caps` |
| `loader.rs:56-59` coarse trust check | not re-verified this pass |
| `FsCaps::with_fs_root` has zero callers | **symbol does not exist** — grep above |

**Severity:** **no.** Strike the header; the register should no longer carry a critical here.

**Replacement text** (header line):

**~~UW-13 · `ExtensionManifest.capabilities` is parsed and never read — the per-extension WASM sandbox grant is inert~~** — ~~***critical***~~ **FIXED 2026-08-13** · = area 06 `EXT-054` (+ `EXT-055`) · re-verified at `14e6c56` 2026-09-22

and append to the FIXED blockquote:

> **Re-verified 2026-09-22 at `14e6c56`.** `load_discovered` passes `&disc.manifest.capabilities` into `load_wasm_with_caps` at `facade.rs:2129` (definition `:1938`); `load_wasm` (`:1915`) survives as the manifest-less host-internal entry and delegates with `Capabilities::host_granted()` at `:1921`. **Deny-by-default held**: both loader synthesis sites are now the EMPTY grant — `loader.rs:345` and `:426` are `Capabilities::none()`, not `Default::default()`. The manifest field is `manifest.rs:46`, the shape `:62`. **One citation in the body below names a symbol that no longer exists**: `FsCaps::with_fs_root` is gone workspace-wide (`grep -rn 'with_fs_root' crates/ --include=*.rs` → no output); `ext-fs` roots now come from `FsCaps::single` (`host/services.rs:1782`) and `FsCaps::from_grants` (`:1800`), with the grant refusals at `:1446`, `:1469`, `:1473`.

---

## UW-14 · SINGLE-mode `outputSchema` is unadvertised

**Verdict:** **CLOSED.** Both halves of the row are dead at HEAD: the property IS advertised on the
top-level schema, and neither constructor pins `None`. The file the row cites no longer exists.

**Evidence — advertised:**
```
$ grep -n 'outputSchema' crates/cyrup-ext-subagents/src/extension/tool/schema.rs
592:    // SUBA-043 / pi `extension/schemas.ts:351` @v0.43.0 — `outputSchema:
598:    // outputSchema})` parsed (the root schema is `additionalProperties: true`), dropped the schema
602:    props.insert("outputSchema".to_string(), {
1629:        for name in ["outputSchema", "toolBudget"] {     ← the guard test
```
`schema.rs:602` inserts `outputSchema` into the **top-level** props with a description
("JSON Schema the child's final output must satisfy; the result then carries typed JSON."), and the
comment block at `:592-601` narrates this very row's defect in the past tense.

**Evidence — the constructors carry the real value:**
```
$ sed -n '1179,1182p' crates/cyrup-ext-subagents/src/extension/executor/foreground.rs
            // SUBA-043 / pi `runSinglePath` (`subagent-executor.ts:3651,3671` @v0.43.0): the
            // top-level `outputSchema` param reaches the SINGLE run here. Pinned `None` until now,
            // which is what made SUBA-S01's capture machinery unreachable from the single surface.
            structured_output_schema: overrides.output_schema.clone(),
$ sed -n '234,238p' crates/cyrup-ext-subagents/src/extension/executor/background.rs
            // SUBA-043 / pi `params.outputSchema` (`extension/schemas.ts:351` @v0.43.0). Pinned
            // `None` here until now, which is the async half of the same unreachable-capability
            // defect the foreground `RunOptions` carried: ...
            structured_output_schema,
```
`foreground.rs:1182` sits inside `run_foreground_impl` (`:299`), production, with no `#[cfg(test)]`
boundary before it. Parse side: `extension/executor/requests.rs:115` and `:214`. Pinned by
`extension/executor/background.rs:1563` and `extension/tool/routing_tests.rs:621-631`.

**Evidence — the row's cyrup citations point at a deleted file:**
```
$ ls crates/cyrup-ext-subagents/src/extension.rs
ls: cannot access '...': No such file or directory
```
So `extension.rs:6543-6690`, `:1934` and `:2295` cannot be checked and cannot be corrected — the
crate was split into `extension/`.

**Severity:** the row should not carry one. It is closed.

**Replacement text** (replace the header and prepend a closure bullet; keep the existing body as
history):

**~~UW-14 · SINGLE-mode `outputSchema` is unadvertised, so the structured-output channel is unreachable~~** — ~~*high*~~ **CLOSED** · = area 09 `SUBA-043`
- **CLOSED, re-greped 2026-09-22 at `14e6c56`, and both halves are dead.** `outputSchema` is advertised **top-level**: `extension/tool/schema.rs:602` inserts it into the root props with a `[CYRUP-DELTA]` description, and the comment block at `:592-601` narrates this row's own defect in the past tense. Both constructors now carry the real value instead of a pinned `None` — `extension/executor/foreground.rs:1182` (`structured_output_schema: overrides.output_schema.clone()`, inside the production `run_foreground_impl`, `:299`) and `extension/executor/background.rs:238`; the param is parsed at `extension/executor/requests.rs:115`/`:214` and pinned by `background.rs:1563` plus `extension/tool/routing_tests.rs:621-631`. The general fix this row asked area 09 for also landed: `schema.rs:1629` is the advertised-property-has-a-consumer guard, iterating `["outputSchema", "toolBudget"]`. **The `extension.rs:6543-6690` / `:1934` / `:2295` citations below cannot be refreshed — that file no longer exists; the crate was split into `extension/`.** **The body below is the original filing and is kept as history.**

---

## UW-15 · Concurrent-duplicate ask collapse never wired — CLOSED

**Verdict:** CLOSED, citations drifted (trivially).

**Evidence:**
```
$ grep -n 'DedupCache' crates/cyrup-permission-system/src/extension/construct.rs
14:use crate::dedup::DedupCache;
198:            dedup: Mutex::new(DedupCache::new()),
```
The row cites `construct.rs:14` as the construction site; `:14` is the `use`, the construction is
`:198`.

**Replacement text** (closure bullet, one clause):

> `crate::dedup::DedupCache` is constructed at `crates/cyrup-permission-system/src/extension/construct.rs:198` (imported `:14`)

---

## UW-16 · Implemented-and-unadvertised, subagents edition

**Verdict:** **CLOSED.** The row states "**`SUBA-054` … This is the whole of what UW-16 still owes**"
— and `SUBA-054` has shipped. `defaultReads` now reaches a SINGLE run and the `[Read from: …]`
instruction is prepended on the production task-text path.

**Evidence — the persona's `defaultReads` reaches the single run:**
```
$ sed -n '1174,1179p' crates/cyrup-ext-subagents/src/extension/executor/foreground.rs
            // `reads` entries at `:144`, `:174` and `:204` are all per-ITEM), so the persona's own
            // `defaultReads` IS the whole precedence chain here. Before this it never left
            // frontmatter: the bundled `reviewer` shipped `defaultReads: plan.md, progress.md` and
            // was never told to read either file.
            reads: agent.default_reads.clone(),
```
This is inside `run_foreground_impl` (`foreground.rs:299`), production — the first `#[cfg(test)]` in
the file is well past it.

**Evidence — the instruction is actually composed, in production:**
```
$ sed -n '1414,1426p' crates/cyrup-ext-subagents/src/exec/spawn_plan.rs
    // SUBA-054 / pi `task = readsInstruction + task` (`subagent-executor.ts:3873`), which runs
    ...
    let reads_instruction = opts.reads.as_deref().map_or_else(String::new, |reads| {
        crate::spawn::chain_graph::build_single_reads_instruction(reads, &opts.cwd)
    });
    ...
    format!("{reads_instruction}{body}")
```
Pinned by `spawn_plan.rs:1991` (`build_task_text_prepends_the_default_reads_instruction_for_a_single_run`,
whose own header records "RED before the fix: `RunOptions` had no `reads` field at all") and by the
missing-file variant at `:2012-2032`.

**Severity:** the row should not carry one. It is closed.

**Replacement text** (header + the `SUBA-054` bullet; the rest stays as history):

**~~UW-16 · Implemented-and-unadvertised, subagents edition~~** — ~~*medium*~~ **CLOSED 2026-09-22 — all three shipped**
- **`SUBA-054` CLOSED, re-greped 2026-09-22 at `14e6c56`** — the last third, and the whole of what this row still owed. The persona's `defaultReads` now reaches a SINGLE run: `extension/executor/foreground.rs:1178` sets `reads: agent.default_reads.clone()` inside the production `run_foreground_impl` (`:299`), and `exec/spawn_plan.rs:1414-1426` composes pi's `task = readsInstruction + task` (`subagent-executor.ts:3873`) through `build_single_reads_instruction`, so `[Read from: …]` is prepended outside chains. Pinned by `spawn_plan.rs:1991` — whose header records "RED before the fix: `RunOptions` had no `reads` field at all" — and by the missing-file variant at `:2012-2032`. **Area 09's table still carries `SUBA-054` as one of its three open mediums; that is now stale and is the only thing left to correct.**

---

## UW-17 · Extension widgets, headers and footers reach the TUI and are stored where nothing renders them — CLOSED

**Verdict:** CLOSED. The row admits "**Not independently re-greped by this pass**"; it is now
independently verified, so that caveat can be struck.

**Evidence:**
```
$ grep -n 'render_extension_widgets\|extension_widgets' crates/cyrup-tui/src/app/render.rs
155:        render_extension_widgets(frame, wabove_area, state, false);
158:        render_extension_widgets(frame, wbelow_area, state, true);
233:/// Draw one placement's extension widgets, in mount order — Pi's `renderWidgets` re-adds every
236:pub(crate) fn render_extension_widgets(
243:        .extension_widgets
```
There is a real draw path over the stored values, in both placements, in mount order.

**Replacement text** (closure bullet, replace the caveat sentence):

> **Independently re-greped 2026-09-22 at `14e6c56`, so the area-file-only caveat is discharged for the widget half**: `cyrup-tui/src/app/render.rs:236` (`render_extension_widgets`) draws the stored values in mount order, called for both placements at `:155` and `:158`.

---

## UW-18 · Settings rows that toggle values nothing reads

**Verdict:** STILL OPEN (partially, as filed), citations drifted. The `CFG-015` half is confirmed,
and the confirmation is sharper than the row states — it is exactly the "a `/settings` row is not a
consumer" shape the section header defines.

**Evidence — `doubleEscapeAction` closure holds, numbers moved:**
```
$ grep -n 'double_escape_action' crates/cyrup-tui/src/app/input.rs crates/cyrup-tui/src/app/state.rs
app/input.rs:342:                    let action = self.state.double_escape_action.clone();
app/state.rs:199:    pub double_escape_action: String,
app/state.rs:491:            double_escape_action: "tree".to_string(),
```
Row says `input.rs:336-342` (the read is at `:342` ✓, the window opens slightly earlier) and
`state.rs:196-199` (field at `:199` ✓, default at `:491`).

**Evidence — `CFG-015` still open, and precisely how:**
```
$ grep -rn 'last_changelog_version' crates/ --include=*.rs
crates/cyrup-config/src/settings/effective.rs:706:    pub fn last_changelog_version(&self) -> Option<String> {
   ← the definition and NOTHING else, workspace-wide
$ grep -rn 'collapse_changelog' crates/ --include=*.rs
crates/cyrup-config/src/settings/effective.rs:711:    pub fn collapse_changelog(&self) -> bool {
crates/cyrup-tui/src/app/settings_rows.rs:181:        SettingRow::toggle("collapseChangelog", "Collapse changelog", eff.collapse_changelog())
   ← one caller, and it is a /settings row
```
`last_changelog_version` has **zero** callers of any kind. `collapse_changelog`'s only caller is the
settings row that renders it — the textbook case §2's header paragraph defines.

**Severity:** `low` still right.

**Replacement text** (replace the first and second bullets):

- **`CFG-045` CLOSED, re-greped 2026-09-22 at `14e6c56`:** `doubleEscapeAction` is no longer a settings row nothing reads — `crates/cyrup-tui/src/app/input.rs:342` is the double-Escape window and reads `self.state.double_escape_action`, the field being `app/state.rs:199` with its default at `:491`. **This matters beyond the row**: `doubleEscapeAction` is the example §2's own header paragraph uses to define "a `/settings` row is not a consumer", and that example is now stale as an example while remaining correct as a lesson
- **`CFG-015` STILL OPEN** (area 05, `low`), re-greped 2026-09-22 and the two headline accessors are worse than "unconsumed" in different ways: `last_changelog_version` (`cyrup-config/src/settings/effective.rs:706`) has **zero callers of any kind workspace-wide**, and `collapse_changelog` (`:711`) has exactly **one** — `cyrup-tui/src/app/settings_rows.rs:181`, a `/settings` row, which is the very thing this section's header says does not count. `collapseChangelog` is PB-6's home

---

## UW-19 · `keybindings.json` is read exactly once, at boot — CLOSED

**Verdict:** CLOSED. The row admits "**Not independently re-greped by this pass**"; verified now.

**Evidence — `/reload` DOES re-read at HEAD (the `TUI-051` half):**
```
$ grep -n 'reload_keybindings_in' crates/cyrup-tui/src/app/execute_session.rs
316:                                reload_keybindings_in: Some(agent_dir),
$ sed -n '268,282p' crates/cyrup-tui/src/app/shell.rs
        let path = agent_dir.join("keybindings.json");
        let json = match std::fs::read_to_string(&path) { ... };
        self.state.keymap = Keymap::default();
        ... (select/tree/session/models keymaps + editor reset) ...
        self.load_keybindings_json(&json)
$ grep -n 'load_keybindings_json' crates/cyrup-tui/src/app/shell.rs
231:    pub fn load_keybindings_json(&mut self, json: &str) -> ...
281:        self.load_keybindings_json(&json)
```
Two production readers now, not one: the boot path (`crates/cyrup/src/interactive.rs:231`) and the
`/reload` path (`execute_session.rs:316` → `shell.rs:281`), which resets all five keymaps first. The
row's "one non-test caller — `crates/cyrup/src/main.rs:1626`, at boot" is stale twice over: the boot
caller moved to `interactive.rs:231`, and it is no longer the only one.

**Replacement text** (closure bullet, replace the caveat sentence):

> **Independently re-greped 2026-09-22 at `14e6c56` for the `TUI-051` half, so that caveat is discharged**: there are now TWO production readers, not one — boot (`crates/cyrup/src/interactive.rs:231`) and `/reload` (`cyrup-tui/src/app/execute_session.rs:316` sets `reload_keybindings_in: Some(agent_dir)`, driving `app/shell.rs:268-281`, which resets all five keymaps plus the editor before calling `load_keybindings_json` at `:281`). The body's "`crates/cyrup/src/main.rs:1626`, at boot" is stale on both counts. `SEAM-067` and `CFG-048` remain on area-file evidence.

---

## UW-20 · The faithful fuzzy matcher is ported and unused — CLOSED

**Verdict:** CLOSED. The row admits "**Not independently re-greped by this pass**"; verified now.

**Evidence:**
```
$ grep -n 'fuzzy' crates/cyrup/src/actions.rs
101:/// by provider then id, fuzzy-filtered by `search` — with Pi's `No models matching "x"` empty message.
110:    // Pi: `fuzzyFilter(models, searchPattern, (m) => `${m.provider} ${m.id}`)` (list-models.ts:45
114:    // `packages/tui/src/fuzzy.ts:75-92`) was absent. `cyrup_tui::fuzzy_filter` is the faithful port
122:        cyrup_tui::fuzzy_filter(&keys, search, |k| k.as_str())
233:    /// SEAM-068 — `--list-models <search>` now runs pi's `fuzzyFilter` over `"{provider} {id}"`
255:    fn list_models_search_uses_pis_fuzzy_filter() {
```
`actions.rs:122` is the production call on the `--list-models <search>` path (`list_models`,
`:102`, reached from `list_models_action`, `:83`), pinned at `:255`.

**Replacement text** (closure bullet, replace the caveat sentence):

> **Independently re-greped 2026-09-22 at `14e6c56`, so that caveat is discharged**: `crates/cyrup/src/actions.rs:122` calls `cyrup_tui::fuzzy_filter` on the production `--list-models <search>` path (`list_models`, `:102`, from `list_models_action`, `:83`), pinned by `list_models_search_uses_pis_fuzzy_filter` at `:255`. **`SEAM-020` is STILL OPEN** (area 08, `low`) — not re-greped this pass.

---

## UW-21 · The whole `async_status_snapshot` subsystem has no production caller — CLOSED

**Verdict:** CLOSED, citations drifted, and one cross-reference in the closure is now false.

**Evidence — both production callers are live:**
```
$ grep -rn 'build_async_status_snapshot_for_state\|encode_async_status_snapshot_widget' crates/ --include=*.rs | grep -v 'async_status_snapshot/'
extension/rpc/mod.rs:393:    /// **This is where UW-21 closes.** ...
extension/rpc/mod.rs:429:            crate::background::async_status_snapshot::build_async_status_snapshot_for_state(
extension/host/slash.rs:214:            async_status_snapshot_jobs_for_state, encode_async_status_snapshot_widget,
extension/host/slash.rs:248:        let lines = encode_async_status_snapshot_widget(
tests/rpc_bridge_integration.rs:612 (the pinning test)
```

**Evidence — citations that drifted:**

| row says | current |
|---|---|
| `extension/rpc/mod.rs:429` | **`:429`** ✓ |
| `extension/host/slash.rs:236` | **`:248`** (imported `:214`) |
| `HostServices::set_widget` at `cyrup-ext/src/host/services.rs:376` | **`:401`** (trait default); the `LiveHostServices` side is `:2193` |

**Evidence — a false cross-reference:** the closure's last bullet says the widget half depends on
"a `set_widget` capability on `cyrup-ext`'s native-extension seam — **the same seam family as UW-7
and area 06 `EXT-021`**". `EXT-021` is the raw-terminal-input seam and it has **landed** (see UW-7's
evidence block). The sentence names a shipped seam as a blocker family.

**Replacement text** (two clauses):

- **The `set_widget` blocker this row named was not real.** `HostServices::set_widget` takes the key as its first argument (`cyrup-ext/src/host/services.rs:401`; the live implementation is `:2193`), so the capability existed; what was missing was a caller
- `encode_async_status_snapshot_widget` is called from `extension/host/slash.rs:248` (imported `:214`), the `ExtMode::Rpc` branch of `refresh_fleet_status_widget` — pi `renderWidget`'s RPC arm (`tui/render.ts:2999-3002 @v0.68.0`)
- **Depends on:** nothing any more — both halves landed. *(The old "the same seam family as UW-7 and area 06 `EXT-021`" clause is struck 2026-09-22: `EXT-021` shipped — `InitApi::subscribe_terminal_input` `cyrup-ext/src/native.rs:425`, dispatch `facade.rs:1472-1522` — and UW-7 is now blocked on call sites, not on a seam.)*

---

## Could not verify

- **`SESS-041` / `SESS-042`** (UW-12's two adjacent defects: `abort_compaction()` never cancels an AUTO compaction; no abort re-check before `append_compaction`). I read the surrounding comments at `cyrup-session-svc/src/session/compaction.rs:330` and `:403` and `session/auto_compaction.rs:186`, which discuss both, but tracing whether the re-check now exists needs a control-flow read I did not do. Stated as unknown, not as open.
- **`UW-9`'s decision rationale** (`extension/command.rs:57-60`, `yolo_api.rs:22-24`) — spot-checked by symbol only. The closure is a decision, so a zero-caller grep neither confirms nor refutes it.
- **`UW-19`'s `SEAM-067` and `CFG-048` halves**, and **`UW-20`'s `SEAM-020`** — still on area-file evidence only; I verified the `TUI-051` and `SEAM-068` halves and nothing else.
- **`UW-13`'s `loader.rs:56-59`** coarse trust check — not re-read; the row's claim about it is unaffected by the closure either way.
- **`UW-2`'s live pty run** — still the outstanding evidence the row names, and a grep cannot supply it.

## Believed but unproven

- **`UW-3`'s observable** ("a child mid-watchdog-review when its agent settles is killed by the ordinary final-drain timer"). The zero-caller fact is proven; the *consequence* is inferred from the upstream fold's shape at v0.43.0 and from `child_status.rs:471-485`'s own in-tree statement. I did not trace cyrup's final-drain timer to confirm there is no second mechanism holding the run open.
- **`UW-7`'s observable** ("pressing ↓ or ← … falls through to the editor"). The two missing call sites are proven; that the keys then reach the editor rather than being swallowed elsewhere is inferred, not measured. Per the standing TUI rule this wants a live terminal run.
