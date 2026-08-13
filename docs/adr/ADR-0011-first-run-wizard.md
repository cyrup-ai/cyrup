# ADR-0011 — The first-run wizard is a finished port with no caller: wire it

**Status** accepted (decided by default under the parity rule — overridable)
**Date** 2026-08-13
**Decides** OQ-9 (`docs/PARITY-PLAN.md:1492-1498`; the item is `UW-2`, `docs/gap-analysis/PARITY-GAPS.md:505-508`)
**Blocks released** Batch 14's `UW-2` implementation line (`PARITY-PLAN.md:711-714`); batch 2's OQ-9 row (`:246-247`); move 1's `UW-2` repro row (`:80`); the known-traps paragraph in `docs/gap-analysis/README.md:168-172`

## Context

The question offers three worlds. **World 2 is true, and it is not close.** pi ships a first-run
wizard, invokes it from `main.ts`, and tests it; cyrup contains a complete, faithful, unit-tested
port of every part of it *except the one line that calls it*; and the "deliberately unreachable
first-run wizard" trap listing has been telling every analysis pass not to report that.

Citations below are `pi` at **v0.83.0** (with the v0.84.1 delta stated explicitly) and cyrup at
**HEAD `72cd292`**, whose last code commit is `04c1ba2` — `72cd292` and `a9000b1` are docs-only, so
the Rust cited here is the Rust at `04c1ba2`.

### pi has it, and it is live @v0.83.0

- `packages/coding-agent/src/cli/startup-ui.ts:115-133` — `shouldRunFirstTimeSetup(settingsPath = getSettingsPath())`,
  four clauses in order: official distribution, `areExperimentalFeaturesEnabled()`, no
  `process.env[ENV_AGENT_DIR]`, `!existsSync(settingsPath)`.
- `:26-28` the `OFFICIAL_*` triple (`@earendil-works/pi-coding-agent` / `pi` / `.pi`); `:36-42`
  `isOfficialDistribution` comparing the *running* build's triple against it.
- `:166-207` `showFirstTimeSetup(settingsManager)` — mounts `FirstTimeSetupComponent`, and on submit
  runs `setTheme` → `setEnableAnalytics` → `await settingsManager.flush()` (`:176-181`); on cancel
  (`finish(undefined)`, `:189`/`:197`) persists nothing.
- `modes/interactive/components/first-time-setup.ts:19-27` (theme + analytics options), `:29` (logo),
  `:48-83` (copy), `:112-123` (`moveSelection`, and the `onThemePreview` live recolour at `:117`),
  `:125-144` (the two-step `handleInput` machine: confirm on `theme` advances, confirm on
  `analytics` submits, cancel at either step abandons).
- **The call site — `main.ts:615-617`:**
  `if (appMode === "interactive" && !parsed.help && parsed.listModels === undefined && shouldRunFirstTimeSetup()) { await showFirstTimeSetup(startupSettingsManager); time("firstTimeSetup"); }`,
  sitting between `const startupSettingsManager = SettingsManager.create(cwd, agentDir)` (`:610`)
  and the `sessionDir` tier chain (`:625-630`), with the comment at `:613-614` stating the reason
  for that position: *"Runs before any runtime services are created so the chosen settings apply
  everywhere."*
- Upstream tests exist and cyrup's own test file names them: `test/first-time-setup.test.ts:36-55`
  (the four gate cases) and `first-time-setup-fork.test.ts:34-36` (the fork case).

**Drift check.** Unchanged at v0.84.1: the call site is `main.ts:663-664`, byte-identical condition;
`git diff v0.83.0 v0.84.1 -- packages/coding-agent/src/modes/interactive/components/first-time-setup.ts`
is **empty**; `startup-ui.ts`'s only diff in that window is `new TUI(...)` → `new TuiMainScreen(...)`
(`:1`, `:82`), a renderer-class change that is ADR-0001 substrate and touches no behaviour cyrup
must carry. So this is not a version-lag item under any reading.

### cyrup has all of it except the call

- `crates/cyrup/src/startup.rs:30-34` — `OFFICIAL_PACKAGE_NAME = "cyrup"`, `OFFICIAL_APP_NAME = "cyrup"`,
  `OFFICIAL_CONFIG_DIR_NAME = ".cyrup"`.
- `:36-43` — the running build's triple: `PACKAGE_NAME = env!("CARGO_PKG_NAME")`, and
  `crates/cyrup/Cargo.toml:2` is `name = "cyrup"`; `APP_NAME = "cyrup"`; `CONFIG_DIR_NAME = ".cyrup"`.
- Therefore `is_official_distribution_of` (`:63-68`) compares three pairs of **identical literals**
  and `is_official_distribution()` (`:71-73`) is a compile-time **`true`** for this build. The claim
  the trap list rests on — that the predicate is a compile-time constant `false` — is not merely
  unproven, it is **inverted**.
- The gate is a complete port: `should_run_first_time_setup` (`:91-97`) and its injectable twin
  `should_run_first_time_setup_with` (`:102-115`), four clauses, pi's order, pi's strict
  `*_EXPERIMENTAL == "1"` (`:76-84`).
- The wizard is a complete port: `FirstTimeSetupResult` (`:124-129`), logo/options/blurb (`:132-146`),
  `first_time_setup_theme_step` (`:169-188`), `first_time_setup_analytics_step` (`:190-217`),
  `parse_theme_choice` / `parse_analytics_choice` (`:210`, `:219`), persistence
  `apply_first_time_setup` (`:235-242`, `set(Global,"theme")` then `set_enable_analytics`, which mints
  `trackingId` on first opt-in — `cyrup-config/src/settings.rs:1523-1543`), and the driver
  `run_first_time_setup` (`:256-291`).
- **The call site — `crates/cyrup/src/main.rs:215-223` — has an empty body:** the `if` evaluates
  `should_run_first_time_setup(&dirs.settings_path(), env.agent_dir.is_some())` and the block
  (`:221-222`) contains only a comment. `rg 'run_first_time_setup' crates/` returns `startup.rs`
  (definition + doc), `tests/first_time_setup.rs` (a doc line saying it is *not* exercised), and
  nothing else. Zero production callers.
- The comment above that gate (`main.rs:215-217`) makes three false claims in three lines: that the
  predicate is *"Faithfully `false` for the cyrup rebrand (not the official distribution)"* — the
  constants at `startup.rs:30-43` say `true`; that the wizard UI is *"the ext-UI dialog host, an
  outer layer"* — `startup.rs:256` is the wizard, in this crate, complete; and it cites
  `Pi main.ts:557` for a gate that is at `main.ts:615`.
- **The test suite proves the port and cannot see the hole.** `crates/cyrup/tests/first_time_setup.rs`
  is 1:1 with upstream's fixtures — the four gate cases (`:39-77`), the strict experimental flag
  (`:86-115`), the fork case (`:140-161`), both steps' copy and preselection (`:185-238`), the
  confirm mapping (`:240-259`) — and at `:124-134`
  `the_running_build_is_the_official_distribution` **asserts `is_official_distribution()` is true**.
  Its own header (`:10-12`) records that `run_first_time_setup` "is not exercised here". The suite
  that would have caught the dead call site instead asserts the fact that makes the trap listing
  false, and nobody read the two together.

### Two facts a wiring patch must not miss

1. **The condition is short by one conjunct, not zero.** pi has four; cyrup's site has one
   (`mode == AppMode::Interactive`). `!parsed.help` is structurally covered — cyrup prints help and
   returns at `main.rs:140-143`, upstream of the gate. `parsed.listModels === undefined` is **not**:
   `resolve_app_mode` (`crates/cyrup/src/cli.rs:571-585`) returns `Interactive` for
   `cyrup --list-models gpt` on a TTY, and the listing exit is downstream at `main.rs:283-285`.
   Wiring the body as-is would mount a full-screen wizard on a command pi answers with a model list.
2. **Position.** pi runs the wizard *before* the `sessionDir` tier-3 read (`main.ts:615` vs
   `:625-630`); cyrup's dead gate sits *after* `apply_settings_session_dir` (`main.rs:213`). No
   observable difference today (the wizard writes `theme` and `enableAnalytics`, never `sessionDir`),
   but pi's stated reason for the position is that the chosen settings apply to everything built
   afterwards, so the wiring should restore the order rather than preserve an accident.

### The one behaviour the current port cannot carry

pi's `onThemePreview` recolours the whole UI as the highlight moves (`startup-ui.ts:184-187` →
`first-time-setup.ts:117`). cyrup's `run_first_time_setup` builds its steps with
`ListSelector::prompt` (`startup.rs:264`, `:274`), which constructs with `preview: false`
(`cyrup-tui/src/selector.rs:567-572` → `:504-509`), and `run_startup_selector` treats
`SelectorOutcome::Preview` as a no-op (`cyrup-tui/src/startup_selector.rs:100-104`).
`startup.rs:250-255` states this honestly in-tree. Under the standing rule this is a **cost of a
mechanism difference, so it stays on the backlog as work** — it is not an accepted divergence, and
no gap id owns it today.

## Decision

**Wire `startup.rs:256`. Delete nothing.** Effort S; owner batch 14, which already owns `main.rs`'s
startup block.

1. **Move the gate to pi's position** in `crates/cyrup/src/main.rs`: immediately after
   `report_diagnostics(&collect_settings_diagnostics(&mut startup_settings, "startup session lookup"))`
   (`:200-203`) and **before** `let dirs = cyrup::apply_settings_session_dir(dirs, &startup_settings);`
   (`:213`) — mirroring `main.ts:610 → 615 → 625`. The gate reads `dirs.settings_path()`, which is
   unaffected by the tier-3 application.
2. **Complete the condition** to pi's conjuncts:
   `mode == AppMode::Interactive && cli.list_models.is_none() && cyrup::should_run_first_time_setup(&dirs.settings_path(), env.agent_dir.is_some())`.
   Add a one-line comment recording that `!parsed.help` needs no conjunct because `main.rs:140-143`
   already returned — the structural equivalent, stated so the next reader does not "fix" it.
3. **Fill the body** with `cyrup::run_first_time_setup(&theme, &mut startup_settings, detected)?;`,
   discarding the returned `Option` (pi's `showFirstTimeSetup` returns `void`; a cancel at either
   step persists nothing, `startup.rs:244-247`). Propagate the error — `run_first_time_setup` already
   maps a persistence failure to `anyhow` (`:287-288`); do not swallow it.
4. **`detected`** is pi's detection, not `TerminalTheme::detect()`: call
   `cyrup_tui::detect_terminal_theme_for_auto(&StdinTerminalProbe, Duration::from_millis(100), &std::env::var("COLORFGBG").unwrap_or_default())`
   (`cyrup-tui/src/theme.rs:1334-1343`), the port of `detectTerminalThemeForAuto({ ui, timeoutMs: 100 })`
   (`startup-ui.ts:180`). The 100 ms bound is pi's; keep it.
5. **`theme`** is `UiTheme::dark()` / `UiTheme::light()` (`cyrup-tui/src/theme.rs:190`, `:200`) chosen
   from `detected` — **not** `UiTheme::default()` as the other pre-launch surfaces use
   (`main.rs:1089`). pi's `createStartupTui` resolves the theme *setting* first
   (`startup-ui.ts:77-84`), but on a first run there is no `settings.json` by definition — that is
   the gate's own fourth clause — so what pi actually resolves to is the detected terminal polarity,
   which `setTheme(detectedTheme)` at `:182` then makes explicit. (This is why `UW-2` does **not**
   wait on `SEAM-067`'s pre-launch-theme work: on this one screen there is no setting to read.)
6. **Timing mark:** `timings.mark("firstTimeSetup")` after the call (`time("firstTimeSetup")`,
   `main.ts:617`).
7. **Delete the comment at `main.rs:215-217`** and replace it with one that cites `main.ts:615-617`.
   Leaving it is not an option: it asserts the opposite of what `startup.rs:30-43` computes and it is
   the sentence the trap list was derived from.
8. **Extend `crates/cyrup/tests/first_time_setup.rs`** with the assertion its header admits it lacks,
   and add these to batch 14's mandatory live-terminal session (`PARITY-PLAN.md:725-737`), which
   already builds the harness they need:
   - `CYRUP_EXPERIMENTAL=1`, fresh agent dir, no `settings.json` → the wizard appears; choose
     **Light** then **Don't share** → `<agent_dir>/settings.json` has `"theme":"light"`,
     `"enableAnalytics":false`, and **no** `trackingId`; relaunch → no wizard.
   - Same conditions, choose **Share anonymous usage data** → `"enableAnalytics":true` **and** a
     non-empty `trackingId` (`settings.rs:1538-1543`).
   - Escape at the theme step, and Escape at the analytics step → **no `settings.json` is created**
     at all, and the next launch shows the wizard again.
   - `CYRUP_EXPERIMENTAL=1 cyrup --list-models gpt` on a TTY → the model list, **no wizard**.
   - `CYRUP_AGENT_DIR=/tmp/x CYRUP_EXPERIMENTAL=1 cyrup` → no wizard.
   - No `CYRUP_EXPERIMENTAL` → no wizard (the default path for every existing user).

**Correct the trap list** in the same ledger pass (that edit belongs to the doc phase, not to this
file): remove the wizard entry from `docs/gap-analysis/README.md:168-172` — it is a gap, not a trap —
and fix `PARITY-GAPS.md:508`, which escalates `UW-2` to a bare "**OQ-6**". That token is **ambiguous,
and wrong under either reading** — which is the point. Read against `PARITY-GAPS.md`'s own §6 list it
means q6, *the wizard question itself*, and is circular; read against `PARITY-PLAN.md` §7 it is the
`spec/` / `PERM-009` question. The two documents run **independent `OQ-N` namespaces**
(`PG §6 q3 = OQ-5` · `q4 ⊂ OQ-6` · **`q6 = OQ-9`** · `q7 = OQ-2` · `q8 = OQ-3` · `q9 = OQ-1`), and
ADR-0003, ADR-0004 and ADR-0005 each hit the same collision independently; the binding convention is
in `docs/adr/README.md`. The repair is the same under both readings: qualify it to
**`PARITY-PLAN` §7 OQ-9 / `PARITY-GAPS` §6 q6, decided by ADR-0011**. For the record, OQ-6 is the `spec/` / `PERM-009`
question (`PARITY-PLAN.md:1453-1461`), and `UW-2` is **OQ-9** (`:1492`).

## Consequences

**Item by item.**

- **`UW-2`** (`PARITY-GAPS.md:505-508`): kind changes from *contested / deliberately unreachable* to
  **not-ported (wiring)**, confirmed at HEAD, unblocked, batch 14. Effort stays **S**. Its **scope
  grows by one clause** — the missing `listModels` conjunct and the call-site position are part of
  the same fix, and a patch that only fills the `if` body ships a new divergence. Its evidence line
  should drop "Escalated to OQ-6" in favour of "decided by ADR-0011".
- **Batch 14** (`PARITY-PLAN.md:711-714`): the "**and UW-2's implementation**" line is now executable
  with no further decision; fold the six live checks above into the batch's mandatory terminal
  session at `:725-737`.
- **Move 1** (`PARITY-PLAN.md:80`): the repro row "*UW-2 (first run with `CYRUP_EXPERIMENTAL=1`)*"
  now has a stated expected result — **the wizard appears**. If it does not appear once the wiring
  lands, that is a *second* defect (settings-path resolution or the selector loop), not this one.
- **Batch 2** (`PARITY-PLAN.md:246-247`): OQ-9 closes with this ADR.
- **`docs/gap-analysis/README.md:168-172`**: the wizard entry is deleted from the known-traps
  paragraph. The README already hedged it as "contested by evidence"; this ADR settles it as
  **wrong**, and a wrong trap is not downgraded, it is removed.
- **New work no item covers:** the `onThemePreview` live recolour (`startup-ui.ts:184-187`,
  `first-time-setup.ts:117`). File it as a new `UW`-family row, *low*, **S** — set `preview: true` on
  the wizard's theme step and give `run_startup_selector` a repaint-on-`Preview` branch
  (`startup_selector.rs:100-104` currently no-ops it; the `on_apply` callback is the seam shape to
  copy). It belongs beside `SEAM-067`'s pre-launch-theme family. Nothing in the ledger owns it today
  — `startup.rs:250-255` admits it in-tree and no id cites it, which is the same "documented in the
  source, invisible to the backlog" pattern this ADR is about.

**The method defect — and it generalises.** A known-traps list exists to stop re-reporting; when an
entry is wrong it instead **converts a real finding into a non-finding across every pass**, and
passes are the only mechanism this project has for finding anything. This entry hid an **S**-effort
wiring gap on the first screen a new user ever meets, for at least three passes, behind a claim that
a five-line read refutes — and the repo's own test asserts the negation of that claim
(`tests/first_time_setup.rs:124-134`). Nobody read the trap and the test together, because a trap is
a reason *not* to look.

Two of the six entries are now known wrong: the wizard (this ADR) and the out-of-scope pi package
list (`PARITY-GAPS.md:788` — `packages/telemetry` is *inside* the dependency closure as of v0.84.1;
blind spot 6). **The remaining four are unaudited and must each get the same two-sided re-check
before the next pass consumes the list:**

1. **`loop_fn.rs` as a facade** (asserted as a trap at `02-cyrup-agent.md:1396`; the code is
   `crates/cyrup-agent/src/agent.rs` + `lib.rs`) — check the facade carries pi's agent-loop
   *behaviour*, not just its name.
2. **pi's two forked compaction implementations** — check which fork cyrup ported and that the other
   is genuinely unreachable upstream, rather than the one users hit.
3. **The provider `fleet!` macro "hiding ~20 registrations"** (`cyrup-provider/src/providers/fleet.rs:27`,
   `:43`; `anthropic.rs:104`) — **already smoking**: `PARITY-GAPS.md:787` records cyrup registering
   **35** of upstream's **40** built-ins, three of the five missing being port bugs. "The macro hides
   them, so don't count them" is precisely the reasoning shape that produced this ADR.
4. **`wasm-host` being default-on** (`crates/cyrup-ext/Cargo.toml:67-68` — the default *is* on) —
   check the trap's implied conclusion, that the feature-gated code is therefore live, against the
   dispatcher; `EXT-054` (critical) says capabilities never reach instantiation, which is the same
   "compiled ≠ wired" error in the other direction.

**The rule this should leave behind:** a trap entry carries the same evidence standard as a gap item
— two file:line citations, both trees, and the date they were read. An entry that cannot produce
them is deleted, not inherited.

## Rejected alternatives

1. **Delete the predicate and the wizard, and record it as `cyrup-original` dead code (world 1).**
   Cost: it rests on a false premise — pi ships the wizard and still invokes it at v0.84.1
   (`main.ts:663-664`). Taking it would delete a faithful, upstream-test-mirroring port
   (`startup.rs:124-291`, `tests/first_time_setup.rs` in full), create a real parity regression on
   the first screen a user sees, and buy nothing except a smaller `rg` result — the same code would
   have to be written again later from the same TypeScript. Rejected on evidence, not preference.
2. **Leave it and document why (world 3).** Cost: it preserves the state the backlog itself calls
   "the worst of both" (`PARITY-GAPS.md:833`) — dead code advertised as intentional, a comment
   asserting the opposite of the constants two files away, and a trap entry that keeps suppressing
   the finding. And there is no "something" to gate on: every dependency exists at HEAD — the
   file-backed settings store (`startup.rs:296-301`), the selector loop
   (`cyrup-tui/src/startup_selector.rs:36`), terminal-polarity detection (`theme.rs:1334`), and the
   persistence path with `trackingId` minting (`settings.rs:1523-1543`). World 3 is empty.
3. **Wire it, but keep the single-conjunct condition.** Cost: `CYRUP_EXPERIMENTAL=1 cyrup --list-models gpt`
   on a fresh machine mounts a full-screen wizard where pi prints a model list — a brand-new
   divergence introduced by the fix, in the same batch that exists to remove divergences.
4. **Keep the wizard but gate it behind a new cyrup-only setting or env var.** Cost: a
   cyrup-original configuration surface pi does not have. That is the "accepted divergence" category
   this project does not have, and it would need its own migration and `/settings` row forever.
5. **Ship the wizard without the analytics step** (theme only). Cost: silently changes what
   `settings.json` contains after a first run, so a cyrup-written settings file and a pi-written one
   diverge on `enableAnalytics`/`trackingId`, and `/privacy` (which the ported blurb tells the user
   to run, `startup.rs:147`) describes a state the wizard never establishes. If cyrup should not
   collect analytics, that is a separate decision about what the analytics setting *does*, not about
   whether the wizard exists.
6. **Defer the wiring until `SEAM-065` restructures the startup/trust flow.** Rejected as a
   *dependency*, accepted as *sequencing*: `SEAM-065` moves the **trust** prompt out of
   `resolve_startup_ui` (`main.rs:1083`, `:1157`), which is a different call site from
   `main.ts:615`'s. Do the wiring inside batch 14, but do not make it wait on `SEAM-065`; coupling
   an S-effort one-liner to an M-effort structural change is how S items become invisible.

## How to reverse this

**"cyrup is not the official distribution — change the identity triple so `is_official_distribution()`
is false, and drop the wizard."**

For that to hold, `startup.rs:30-34`'s `OFFICIAL_*` constants would have to stop naming the build
that ships from this tree (today they are `cyrup` / `cyrup` / `.cyrup`, and the running build's own
triple at `:36-43` is character-identical), and `tests/first_time_setup.rs:124-134` — which asserts
the running build **is** official, ported from pi's own fork fixture — would have to be inverted.
Note the cost of that reversal: it declares cyrup a fork *of itself* for gating purposes, which
silences not only this wizard but every future official-distribution-only surface pi adds behind the
same predicate. If the real intent is narrower — "cyrup ships no analytics" — the reversal does not
touch this decision at all: keep the wizard and the theme step, and change what the analytics answer
writes. That is a statable decision about telemetry, not about whether the first-run wizard exists.
