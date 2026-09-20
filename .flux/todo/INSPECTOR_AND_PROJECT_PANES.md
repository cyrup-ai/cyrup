---
stage: new
status: pending
updated: 2026-09-19
---

# `inspector.*` + `project.*` — a faithful port of the herdr/ghostty inspector surface

OBJECTIVE: port pi's inspector plugin layer and BOTH built-in backends (herdr, ghostty), plus the
herdr project-pane manager, so all seven remaining verbs land: `inspector.{open,command,status,close}`
and `project.{open,status,close}`. `SUBAGENT_ACTIONS` 52 → 57 — the LAST of the verb gap, once the
sibling batch (`children.list`, `debug.run`) has merged.

**User decision, recorded**: "port faithfully." Not a cyrup-native backend; not a deliberate
non-port. The plugin layer plus herdr plus ghostty, as upstream ships them.

## What this feature IS — read before sizing

An **inspector** is a terminal pane opened next to the user's session that runs a separate process
— `inspector-runner` — which clears the screen, renders a lifecycle dashboard for ONE async run every
`refreshMs` (1 500 ms default), and reads control lines from stdin: `status`, `stop`, `steer <msg>`,
or plain guidance. It is a *mirror*: closing it does not stop the run (`inspector-runner.ts:33`).

The verbs:
- `inspector.command` — returns the shell command that would launch the runner. **Needs no backend.**
- `inspector.open` — picks a plugin whose `available()` and `owns()` say yes, and asks it to open a
  pane running the launch. Writes a **binding** file next to the run so later calls find the pane.
- `inspector.status` / `inspector.close` — through the plugin that owns the binding.
- `project.{open,status,close}` — herdr **project panes**: a pane per project root, with its own
  binding, a snapshot restorer on session start, and a trust status of
  `"human-verification-required"` (`project-panes.ts:17`).

**Honest degradation is upstream's, not ours.** herdr's `available()` is
`env.HERDR_ENV === "1" && env.HERDR_PANE_ID` — i.e. pi is itself running *inside* a herdr pane.
ghostty's needs macOS + `osascript` + Ghostty 1.3+ with Automation permission. With neither,
`inspector.open` answers that no backend is available — pi's own sentence — and `inspector.command`
still works. Port those refusals verbatim; they are the feature behaving correctly, not a stub.

## Upstream, pinned at v0.68.0 — `src/inspectors/` (~1 590 lines) + dispatch

| file | lines | what |
|---|---:|---|
| `types.ts` | 51 | `INSPECTOR_ACTIONS`, `InspectorParams`, `InspectorTarget`, `InspectorContext`, `InspectorLaunch`, **`InspectorPlugin { name, available, owns, open, status?, close? }`** |
| `plugins.ts` | 8 | `createBuiltinInspectorPlugins()` → `[herdr, ghostty]`, in host-preference order |
| `actions.ts` | 148 | `handleInspectorAction:120`; `resolveTarget:49`; `trustedDir:34` (path containment); `missionFor:79`; **`launchFor:90`** — builds the `InspectorLaunch` |
| `inspector-runner.ts` | 153 | `RunnerOptions:14`, `formatInspectorDashboard:30`, `submitInspectorControl:99`, `parseArgs`, `queueInspectorSteer`, **`runInspector:120`** — the in-pane process |
| `session-roots-codec.ts` | 31 | base64-JSON for `--session-roots` (the PowerShell-quoting rationale in its doc is the reason it exists — keep it) |
| `shell-command.ts` | 16 | `formatShellCommand` with win32/nushell branches |
| `herdr/plugin.ts` | 20 | the plugin; `available` = `HERDR_ENV=1 && HERDR_PANE_ID` |
| `herdr/client.ts` | 130 | `HerdrClient.run(args)` over `spawn(HERDR_BIN ?? "herdr")`; `detectHerdr` (`--version`, needs **0.7.5+** for `supportsRawPanes`); typed `HerdrErrorCode` |
| `herdr/actions.ts` | 155 | `HerdrInspectorBinding:9` `{schemaVersion:1, kind:"herdr-inspector", runId, asyncDir, paneId, openedAt, command, childIndex?}`; `bindingPath:24`; `readHerdrInspectorBindingForTarget:51` (realpath-compares `asyncDir`); **`openHerdrInspector:88`** — `pane split --current --direction right --cwd <cwd> --focus/--no-focus` → `pane run <id> <displayCommand>` → on failure `pane close <id>` → `writeAtomicJson(binding)`; `statusHerdrInspector`, `closeHerdrInspector` |
| `herdr/focus.ts` | 55 | focus handling |
| `herdr/project-panes.ts` | 730 | `HERDR_PROJECT_PANE_ACTIONS:12`, `HerdrProjectPaneBinding:19`, `ProjectPaneManager:128`, `projectPaneBindingPath:176`, `listHerdrProjectPaneRoots:192`, `readHerdrProjectPaneBinding:264`, **`restoreHerdrProjectPaneSnapshots:302`** (session-start restore), `PROJECT_PANE_TRUST_STATUS:17` |
| `ghostty/plugin.ts` | 17 | the plugin |
| `ghostty/actions.ts` | 74 | `GHOSTTY_APPLESCRIPT:7`; `execFile("/usr/bin/osascript", …)`; the failure hint at `:52` |

Dispatch: `subagent-executor.ts` — `HERDR_PROJECT_PANE_ACTIONS` arm and `INSPECTOR_ACTIONS` arm
(both child-safe-gated for the mutating members; `plugins: createBuiltinInspectorPlugins()`), and the
authority consult just above them: `inspector.open → inspectorOpen`, `project.open → projectOpen`.
`MUTATING_MANAGEMENT_ACTIONS:213` contains `inspector.open`, `inspector.close`, `project.open`,
`project.close` — so `inspector.command`, `inspector.status`, `project.status` are read-only.

Other production consumers of `inspectors/` (the augment must map each): `slash/slash-commands.ts`,
`tui/fleet.ts` (the `H` key), `api/project-panes.ts` (a public API module), `extension/index.ts`
(which also calls `restoreHerdrProjectPaneSnapshots` on session start — find where).

**OUT OF SCOPE, and say so in the port**: `integrations/herdr-status.ts` (400 lines, the
session-start status bridge `index.ts:866` runs — imported only by `index.ts`, not by the verbs) and
`runs/shared/herdr-connection.ts` (SSH to remote machines — used by `herdr-machine.ts`,
`herdr-placed-run.ts`, `herdr-external-adapters.ts`, i.e. REMOTE PLACED RUNS, a different feature).
`project-panes.ts` does not import either.

## cyrup seams — verified

- **THE CROSS-CRATE PIECE.** `InspectorLaunch.executable` is `resolveNodeExecutable()` and argv is
  `[inspector-runner.mjs, --async-dir, --run-id, --allow-steer, --allow-stop, --session-roots,
  --index?, --mission-path?]`. In cyrup the runner is a **subcommand on the `cyrup` binary**, and
  the precedent is exact: `crates/cyrup/src/predispatch.rs` dispatches the internal
  `__subagent-runner --config <path>` hop (`:35-37`, `:69` `subagent_runner_cmd::is_selected`),
  launched from `background/spawn_detached.rs:213-218` as
  `Command::new(&spawn_command.binary).arg("__subagent-runner").arg("--config").arg(path)`.
  Add `__subagent-inspector` the same way. This batch touches `crates/cyrup`, not only the extension.
- **The dashboard's inputs already exist**: `formatAsyncRunTranscript(status, asyncDir, {index,
  lines: 60, sessionRoots})` → cyrup's `background/fleet_view.rs` + `tui/fleet_transcript.rs`
  (the reader that #144 gave a writer); `parseMissionRecord` → `missions/store.rs` (whose doc
  already mentions `inspector-runner`); `requestAsyncSteer`/`requestAsyncStop` → the control
  channel under `background/`; `steeringReceipt` → check `exec/` or `background/`.
- **Authority**: `registration/authority.rs:44-46` omits `inspectorOpen`/`projectOpen` with a
  reason that is TRUE today — *"the verbs they gate are not ported"* — and becomes FALSE the moment
  this lands. Add both (upstream `policy/authority.ts:1-10` has eight; cyrup will match) and
  **delete that delta**, do not reword it.
- **The fleet `H` key**: `tui/fleet.rs:1766` answers pi's own *"Herdr inspector controls are
  unavailable in this context."* and `tui/fleet_overlay.rs` carries `has_inspect: false` ("there is
  no Herdr inspector to route `H` to"). Route `H` to `inspector.open` for the selected run and
  delete both deltas.
- **Process spawning**: cyrup's child-process idiom is `tokio::process::Command` (see
  `spawn_detached.rs`, `spawn/worktree.rs`'s `run_git`). The herdr client is that plus a timeout
  and a typed error; the ghostty runner is the same over `/usr/bin/osascript`.
- **Atomic JSON for bindings**: `background/atomic.rs:75` `write_atomic_json`.

## Rust shape

- `InspectorPlugin` is a **trait** (`async fn available/owns/open/status/close`), the two backends
  are structs implementing it, and `builtin_inspector_plugins()` returns `Vec<Box<dyn InspectorPlugin>>`
  in host-preference order.
- `HerdrErrorCode`, the binding kinds, and the pane actions are **enums**. `HerdrInspectorBinding`
  and `HerdrProjectPaneBinding` are `#[serde(rename_all = "camelCase", deny_unknown_fields)]` with a
  unit-struct schema version — **the binding files are shared with pi**, so on-disk keys do not
  change.
- `HerdrClient` is a trait so tests inject a fake binary; the real one shells out.
- The runner's argv parser is a small hand parser matching `parseArgs` exactly (it is pairwise
  `--key value`, refuses unknown shapes, `--refresh-ms >= 250`).
- ghostty is `#[cfg(target_os = "macos")]`-gated at the *availability* check, not compiled out —
  `available()` returns false elsewhere with the same hint text.

## Definition of done

1. All seven verbs advertised AND dispatched; authority-gated as pi gates them; the four mutators
   refused in child-safe fanout; `inspector.command`, `inspector.status`, `project.status` reachable there.
2. `cyrup __subagent-inspector --async-dir … --run-id …` runs the dashboard loop against a real
   on-disk run, refreshes, and a `steer hello` line on stdin lands a real steer request on the run's
   control channel (assert the file), `stop` lands a stop request, and a terminal run stops the timer.
3. `inspector.command` returns the exact launch command (with the base64 `--session-roots`) and
   works with no backend installed.
4. With a **fake herdr client** injected: `inspector.open` performs the `pane split` → `pane run` →
   binding-write sequence, `inspector.status` reads it back, `inspector.close` runs `pane close` and
   removes the binding; a `pane run` failure closes the pane it just opened and writes no binding.
5. With no backend available, `inspector.open` refuses with upstream's sentence and writes nothing.
6. `project.open/status/close` through the same fake client; `restoreHerdrProjectPaneSnapshots`
   runs on session start and is pinned.
7. The fleet `H` key opens an inspector; both "unavailable" deltas are gone.
8. `registration/authority.rs` has eight actions and its omission-delta is gone.

## Rules
- Upstream reads ONLY via `git -C /home/user/cyrup/tmp/pi-subagents show v0.68.0:<path>`.
- Every `[CYRUP-DELTA]` states a TRUE reason. Three deltas in the tree become false when this lands
  (`authority.rs:44-46`, `fleet.rs` delta 2, `fleet_overlay.rs`); delete them.
- No `allow(dead_code)`, no stub, no narrowing-and-reporting-done.
- **Do not start execution until the `children.list`/`debug.run` batch has merged** — both edit
  `SUBAGENT_ACTIONS` and `route_action`.
- Gates: fmt; clippy `--workspace --all-targets --features test-fixtures -- -D warnings`;
  `nextest run --workspace --features test-fixtures`; `nextest run -p cyrup-it --features it`;
  clippy on `cyrup-it` and the wasm sdk. Baselines are whatever the sibling batch leaves.

---

## [AUG — inspector]

READ-ONLY research pass. Upstream read exclusively via `git -C /home/user/cyrup/tmp/pi-subagents show
v0.68.0:<path>`. Every claim below was re-derived; the seed's claims were treated as leads.

### 0. The seed's anchors, re-verified

**Correct as written** — `types.ts` 51 lines; `plugins.ts` 8; `actions.ts` 148 with `trustedDir:34`,
`resolveTarget:49`, `missionFor:79`, `launchFor:90`, `handleInspectorAction:120`;
`inspector-runner.ts` 153 with `RunnerOptions:14`, `formatInspectorDashboard:30`,
`submitInspectorControl:99`, `runInspector:120`; `session-roots-codec.ts` 31; `shell-command.ts` 16;
`herdr/plugin.ts` 20; `herdr/client.ts` 130; `herdr/focus.ts` 55; `herdr/actions.ts` 155 with
`HerdrInspectorBinding:9`, `bindingPath:24`, `readHerdrInspectorBindingForTarget:51`,
`openHerdrInspector:88`; `herdr/project-panes.ts` 730 with `HERDR_PROJECT_PANE_ACTIONS:12`,
`PROJECT_PANE_TRUST_STATUS:17`, `HerdrProjectPaneBinding:19`, `ProjectPaneManager:128`,
`projectPaneBindingPath:176`, `listHerdrProjectPaneRoots:192`, `readHerdrProjectPaneBinding:264`,
`restoreHerdrProjectPaneSnapshots:302`; `ghostty/plugin.ts` 17; `ghostty/actions.ts` 74 with
`GHOSTTY_APPLESCRIPT:7` and the failure hint at `:52`. Sum 1 588 lines ≈ the seed's "~1 590".
`MUTATING_MANAGEMENT_ACTIONS` at `:213` ✔. `crates/cyrup/src/predispatch.rs:35-37` (the
`Internal::SubagentRunner` variant doc) and `:69` (`subagent_runner_cmd::is_selected(raw)`) ✔.
`background/atomic.rs:75` `write_atomic_json` ✔ (but it is `pub async fn`, not sync).

**STALE OR WRONG — corrections are load-bearing:**

| seed says | truth |
|---|---|
| `subagent-executor.ts` | the file is `src/runs/foreground/subagent-executor.ts` (7 511 lines). There is no `src/subagent-executor.ts` at v0.68.0. |
| `SUBAGENT_ACTIONS` 52 → **57** | **52 → 59.** Seven verbs land, not five. cyrup's list (`extension/tool/text.rs:239-366`) is exactly 52 today; upstream's (`shared/types.ts:2801`) is 58. |
| `inspector.open` "picks a plugin whose `available()` **and** `owns()` say yes" | **Only `available()`.** `actions.ts:133-134` iterates plugins and calls `plugin.open` on the first whose `available()` is true. `owns()` is consulted **only** for `inspector.status`/`inspector.close` (`:138`). Porting the seed's rule would make `inspector.open` permanently unreachable — the first open has no binding, so `owns()` is false. |
| ghostty `available()` "needs macOS + `osascript` + Ghostty 1.3+ with Automation permission" | `ghostty/plugin.ts:13` is `platform === "darwin" && context.env.TERM_PROGRAM?.toLowerCase() === "ghostty"` — two env/platform reads, no probe. The osascript/1.3/Automation sentence is the **failure hint** (`ghostty/actions.ts:52`), appended after a *failed run attempt*. |
| `tui/fleet.rs:1766` answers the "unavailable" message | `tui/fleet.rs:1820-1826`; the string literal is at **`:1825`**, guard at `:1822`. |
| `registration/authority.rs:44-46` carries the omission delta | the delta paragraph is **`:43-46`**; `AUTHORITY_ACTIONS` is `:47-55`. And there are **three** sites, not one — see §3. |
| `spawn_detached.rs:213-218` is where `SUBAGENT_RUNNER_SUBCOMMAND = "__subagent-runner"` lives | the constant is at **`spawn_detached.rs:86`**. `:214-219` is the `tokio::process::Command::new(&spawn_command.binary).args(base_args).arg(SUBAGENT_RUNNER_SUBCOMMAND).arg(CONFIG_FLAG).arg(cfg_path)` chain inside `spawn_detached_runner_with_command` (`:203`). |
| `tui/fleet_overlay.rs` "carries `has_inspect: false`" | `fleet_overlay.rs` only *documents* it (`:37`). The literal `false` is the **seventh positional argument at `extension/host/slash.rs:86`**, with the delta-2 back-reference at `:83-84`. That is the production site to change. |
| "Route `H` to `inspector.open` for the selected run" | upstream passes **`focus: true`** (`fleet.ts:1420`), resolves the target through a **separate `selectedInspectAction()`** (`fleet.ts:947-959`, which cyrup has no counterpart for), and binds **`["return", "H"]`** (`fleet.ts:45`), not `H` alone. |
| binding structs get `deny_unknown_fields` | **NO.** Upstream's `parse` (`herdr/actions.ts:28-40`) and `parseBinding` (`project-panes.ts:225-237`) validate only the *required* keys and pass unknown keys through untouched. These files are shared with pi; `deny_unknown_fields` would make a pi-written binding with a newer key unreadable by cyrup — a divergence, not a tightening. Use `#[serde(rename_all = "camelCase")]` with `#[serde(flatten)] extra: serde_json::Map<String, Value>` or a plain tolerant struct. |
| `steeringReceipt` → "check `exec/` or `background/`" | **It does not exist anywhere in cyrup.** `rg 'steering_message_preview\|preview_display_text\|Message sent:'` over `crates/` is zero-hit. It is a genuine (small) port: `runs/background/steering.ts:22-24` + `:41-46`. `redact_secret_values` **does** exist (`watchdog/permission_arbiter.rs:247`); `previewDisplayText` does not. |
| `formatAsyncRunTranscript(status, asyncDir, {index, lines: 60, sessionRoots})` maps to `background/fleet_view.rs` | correct module, different signature: `pub fn format_async_run_transcript(status: &RunStatus, paths: &RunPaths, index: Option<usize>, lines_param: Option<i64>, session_roots: &[PathBuf]) -> Result<String, String>` at **`background/fleet_view.rs:1034`**. It takes a `&RunPaths`, not a bare dir. |
| `inspector-runner.ts:33` for "closing it does not stop the run" | the sentence is at **`:34`**, inside `formatInspectorDashboard`'s header array. |

**Out of scope confirmed, no drift.** `git show v0.68.0:src/inspectors/herdr/project-panes.ts | grep
herdr-status` and the same for `herdr-connection` are both zero-hit. The only files that import
`src/inspectors/*` at v0.68.0 are `api/project-panes.ts:32`, `extension/index.ts:35,61`,
`runs/foreground/subagent-executor.ts:120-122`, `slash/slash-commands.ts:33`, `tui/fleet.ts:21-22`.

### 1. THE `__subagent-inspector` SUBCOMMAND — the exact cross-crate seam

**What upstream actually launches.** `launchFor` (`actions.ts:90-104`) builds
`executable = resolveNodeExecutable()` and
`argv = [runnerPath, "--async-dir", <dir>, "--run-id", <id>, "--allow-steer", "true|false",
"--allow-stop", "true|false", "--session-roots", <base64>]`, then optionally `"--index", <n>` and
`"--mission-path", <path>`, and `displayCommand = formatShellCommand(executable, argv)`. Nobody in
`inspectors/` ever spawns it: `openHerdrInspector` hands `launch.displayCommand` to
`herdr pane run <paneId> <displayCommand>` (`herdr/actions.ts:111`) and ghostty hands it to
AppleScript as `command of surfaceConfiguration` (`ghostty/actions.ts:15,61`). **The inspector
process is started by the terminal host, never by pi.** `inspector.command` just returns that string
(`actions.ts:130`).

So cyrup does **not** reuse `spawn_detached`. It reuses the *selector precedent* only.

**The precedent, verified end to end:**

* `crates/cyrup/src/subagent_runner_cmd.rs:61` — `pub const SUBCOMMAND: &str = "__subagent-runner";`
* `:72-74` — `pub fn is_selected(argv: &[String]) -> bool { argv.get(1).map(String::as_str) == Some(SUBCOMMAND) }`
* `:88` — `fn parse_config_flag(rest: &[String]) -> Result<PathBuf, String>`, a deliberate hand parser ("a hand-rolled scan is clearer and lighter than pulling `clap` in for a one-flag internal contract never shown to a user") — **exactly the precedent `parseArgs` needs.**
* `crates/cyrup/src/predispatch.rs:33-63` — `pub enum Internal { SubagentRunner, IntercomBroker, McpKeyringHelper, AcpTerminalLogin }`, the `SubagentRunner` doc at `:35-37`.
* `predispatch.rs:68-85` — `classify_internal`, first arm `subagent_runner_cmd::is_selected(raw)` at `:69`. Module doc `:11-17` states *why* classification and dispatch are split: `set_process_name` needs `unsafe`, and `crates/cyrup` is the only crate that may hold it.
* `crates/cyrup/src/main.rs:266-270` — `Some(Internal::SubagentRunner) => { set_process_name("cyrup-subagent"); return Ok(cyrup::subagent_runner_cmd::dispatch(&raw).await); }`
* `crates/cyrup-ext-subagents/src/background/spawn_detached.rs:86` — the mirrored constant, with the doc at `:82-85` stating the two-independent-literals convention ("`cyrup` is the one binary crate that owns CLI-subcommand dispatch, this crate is a pure library the subcommand handler calls into").

**The seam to add, mirroring it exactly:**

1. `crates/cyrup-ext-subagents/src/inspectors/runner.rs` — `pub const INSPECTOR_SUBCOMMAND: &str =
   "__subagent-inspector";` plus `RunnerOptions`, `parse_args`, `format_inspector_dashboard`,
   `submit_inspector_control`, `run_inspector`. The library half: it must be callable with an
   injected argv and an injected stdin/stdout so it is unit-testable without a TTY.
2. `crates/cyrup/src/subagent_inspector_cmd.rs` — `pub const SUBCOMMAND: &str =
   "__subagent-inspector";` (the second independent literal, per `spawn_detached.rs:82-85`'s stated
   convention), `pub fn is_selected(argv: &[String]) -> bool`, `pub async fn dispatch(argv: &[String]) -> i32`
   calling `cyrup_ext_subagents::inspectors::runner::run_inspector`.
3. `crates/cyrup/src/predispatch.rs` — a fifth `Internal::SubagentInspector` variant, classified
   **after** `SubagentRunner` and **before** `AcpTerminalLogin` (the module doc's ordering rule:
   `--terminal-login` is membership-anywhere and must stay last).
4. `crates/cyrup/src/main.rs` — `Some(Internal::SubagentInspector) => { set_process_name("cyrup-inspector"); return Ok(cyrup::subagent_inspector_cmd::dispatch(&raw).await); }`.
   (`PR_SET_NAME` caps at 16 bytes incl. NUL; `cyrup-inspector` is 15 — it survives intact, unlike
   `cyrup-mcp-keyring`. Say so in the arm, as `main.rs:275-279` does for that one.)
5. `crates/cyrup/src/lib.rs` — `pub mod subagent_inspector_cmd;`.
6. `crates/cyrup-ext-subagents/src/inspectors/actions.rs` — `launch_for` builds the launch from
   `crate::spawn::resolve_spawn_command()` (`spawn/mod.rs:280`, returning
   `SpawnCommand { binary: PathBuf, base_args: Vec<String> }`), i.e.
   `executable = spawn_command.binary`, `argv = base_args ++ [INSPECTOR_SUBCOMMAND, "--async-dir", …]`.
   This is upstream's `resolveNodeExecutable()` + `runnerPath` collapsed into one binary + one token,
   which is *precisely* the delta `subagent_runner_cmd.rs:28-48` already records.

**The existing `[CYRUP-DELTA] (SEAM-109)` at `subagent_runner_cmd.rs:28-48` stays TRUE and must be
EXTENDED, not copied.** Its premise — "pi has NO argv verbs … cyrup ships one compiled binary with no
interpreter to hand a script to, so the same mechanism is expressed as a re-exec under a reserved
argv token" — is now true of *two* tokens. Add one sentence naming `__subagent-inspector` and the
same `--help`-absent/`SUBCOMMANDS`-absent undiscoverability argument. **Its own citations are from a
different pin** (`pi-subagents HEAD 30c6080`, `v0.83.0`) than this repo's v0.68.0 pin — leave them,
they are explicitly labelled with their pin, but do not add new ones in that style.

**Cross-crate consequence the exec must plan for:** this is the only batch in the set that edits
`crates/cyrup`. `cargo clippy --workspace --all-targets` covers it; the `cyrup-it` gate gains a real
end-to-end case (§6, T-RUN-1) because `cyrup-it` can spawn the real binary.

### 2. NO HERDR AND NO GHOSTTY — upstream's own degradation, verbatim

This is the common case on every Linux CI box and every non-Ghostty macOS terminal. Upstream's
answers, quoted:

**`inspector.command`** — `actions.ts:130`: `if (action === "inspector.command") return
result(launchFor(target, deps).displayCommand);` — **runs before `deps.plugins` is even read**
(`:131`). It needs no backend, ever, and it is not an error. Port that ordering literally.

**`inspector.open` with no plugin available** — `actions.ts:132-137`:

```ts
if (action === "inspector.open") {
    for (const plugin of plugins) {
        if (await plugin.available(context)) return plugin.open(context, launchFor(target, deps), params);
    }
    return result("No inspector plugin is available. Start a supported inspector host, or use inspector.command for a standalone command.", true);
}
```

The sentence is exactly `"No inspector plugin is available. Start a supported inspector host, or use
inspector.command for a standalone command."`, `isError: true`, **and nothing is written to disk** —
`launchFor` is only called *inside* the loop body, so with zero available plugins not even the
mission lookup runs. herdr's `available` is `env.HERDR_ENV === "1" && Boolean(env.HERDR_PANE_ID?.trim())`
(`herdr/plugin.ts:13-14`); ghostty's is `platform === "darwin" && env.TERM_PROGRAM?.toLowerCase() === "ghostty"`
(`ghostty/plugin.ts:13`). Both are pure env/platform reads — **no binary probe**, so the refusal is
instant and cannot hang.

**`inspector.status` / `inspector.close` with no binding** — `actions.ts:138-139`:
`const owner = plugins.find((plugin) => plugin.owns(context)); if (!owner) return result(\`No
inspector plugin owns this binding for async run ${target.runId}.\`);` — note the **missing second
argument**: this is `isError` *false*. A status query with no inspector open is a normal answer, not
a failure. ghostty's `owns` is `() => false` (`ghostty/plugin.ts:14`), so ghostty is never an owner
and its own open message says so: *"Status and close are unavailable because this plugin writes no
binding."* (`ghostty/actions.ts:69`).

**`project.status` / `project.close` with no herdr installed** — neither calls `detectHerdr`.
`manager.status` → `bindingForManager` → `readBinding` → `ENOENT` → `{state:"absent"}` →
`toolResult(\`No Herdr project pane binding exists for ${projectRoot}.\`)` (`project-panes.ts:695`),
`isError` false. `manager.close` with no binding → `disposition: "absent"` → the same sentence
(`:709`). **Both are answerable with no `herdr` binary on PATH at all.**

**`project.open` with no herdr binary** — `:550` calls `detectHerdr` first. The spawn fails `ENOENT`
→ `client.ts:54-55` → `HERDR_UNAVAILABLE` with `"Herdr is not installed or is not on PATH. Install
Herdr 0.7.5+ or set HERDR_BIN."` → `formatProjectPaneError` (`:168-170`) →
`"Herdr project pane error (HERDR_UNAVAILABLE): Herdr is not installed or is not on PATH. Install
Herdr 0.7.5+ or set HERDR_BIN."`, `isError: true`.

**`inspector.open` inside a herdr pane but with no herdr binary** — `openHerdrInspector:94-95` →
`"Herdr inspector error (HERDR_UNAVAILABLE): Herdr is not installed or is not on PATH. Install Herdr
0.7.5+ or set HERDR_BIN."` (`errorText` at `:70-72` is `Herdr inspector error (${code}): ${message}`).

**Version floor** — `detectHerdr` (`client.ts:122-129`) runs `herdr --version` with `timeoutMs: 3_000,
textOk: true`, parses `/(\d+)\.(\d+)\.(\d+)/`, and refuses below 0.7.5
(`supportsRawPanes`, `:118-120`: `major > 0 || minor > 7 || (minor === 7 && patch >= 5)`) with
`HERDR_UNSUPPORTED_VERSION` and `"Herdr ${versionText} does not support raw inspector panes. Upgrade
to Herdr 0.7.5 or newer."`. An unparseable version is `VALIDATION_ERROR`:
`"Could not parse the Herdr version from '${versionText}'."`

**Every one of these sentences is a test assertion in §6.** No cyrup-invented refusal text anywhere.

### 3. THE FLEET `H` KEY AND THE AUTHORITY ADDITIONS — exact deltas to delete

**Upstream at v0.68.0 no longer says "Herdr" anywhere in `tui/fleet.ts`** — `git show
v0.68.0:src/tui/fleet.ts | grep -n Herdr` is zero-hit (1 448 lines). The message is
`fleet.ts:963`: `"Inspector controls are unavailable in this context."` cyrup's tree emits
`"Herdr inspector controls are unavailable in this context."` and cites `fleet.ts:692` — **an older
pin.** At v0.68.0 the anchors are: `inspect?` optional handler `:100`; `DEFAULT_FLEET_KEYBINDINGS.inspect
= ["return", "H"]` `:45`; `selectedAsyncAction` `:926`; `selectedInspectAction` `:947-959`;
`inspectSelected` `:961-965`; `handleInput` `:1021`; the `inspect` handler construction `:1417-1430`.
The cyrup in-tree citations `fleet.ts:51`, `:547-553`, `:606-713`, `:620,647,680,692,698`,
`:324-328`, `:458-465`, `:476-841`, `:520`, `:844-846`, `:876-878` are all from that older pin.
Re-pin the ones this change touches; leave the rest alone (out of scope, and mass re-citation would
collide with the sibling batches).

**Upstream's `H` is not what cyrup's `H` is.** `fleet.ts:1417-1430` constructs the `inspect` handler
**unconditionally** in the action bundle, so at v0.68.0 `!actions?.inspect` is only reachable when
`actions` itself is absent. And the handler calls:

```ts
inspect: async (input) => firstToolResultText(await handleInspectorAction("inspector.open", {
    id: input.runId, dir: input.asyncDir, focus: true,
    ...(input.index !== undefined ? { index: input.index } : {}),
}, { state, sessionRoots: state.trustedSessionRoots, cwd: state.baseCwd,
     ...(state.authorityPolicy ? { authorityPolicy: state.authorityPolicy } : {}),
     ...(state.missionStoreConfig ? { missions: state.missionStoreConfig } : {}),
     ...(options.inspectorPlugins ? { plugins: options.inspectorPlugins } : {}),
     ...(options.inspectorEnv ? { env: options.inspectorEnv } : {}) }),
  `Failed to open inspector for async run ${input.runId}.`),
```

Note `focus: true`, and note the fallback sentence `"Failed to open inspector for async run
${runId}."` — which is **exactly the string `fleet_overlay.rs:275-277` already emits**, so that arm
becomes a genuine call with its existing fallback preserved.

`selectedInspectAction` (`fleet.ts:947-959`) is **not** `selectedAsyncAction`. It additionally
accepts a `foreground-active` item with a `parentWorkflowRunId`, mapping to the parent workflow's
async run, and it carries two refusal sentences cyrup does not have:
`"External jobs are display-only and have no inspector controls."` (`:950`) and
`"The parent workflow is no longer available for inspection."` (`:957`). cyrup's `H` currently
funnels through `selected_async_action` (`fleet.rs:1650`), which refuses every foreground item with
`"Fleet controls are available for current-session top-level async runs only."` — a behavioural gap
this batch closes by adding `fn selected_inspect_action(&self) -> Result<FleetActionTarget, String>`
beside it.

`inspect: ["return", "H"]` (`fleet.ts:45`) — upstream binds **Enter as well as H** at top level.
cyrup's `FleetKey::Enter` is handled only inside the steer draft (`fleet.rs:1693`) and the confirm
prompt (`:1747`); at top level it is unbound. Add the top-level `FleetKey::Enter` arm next to
`FleetKey::Char('H')` (`fleet.rs:1820`), sharing one `fn inspect_selected(&mut self)`.

**Deltas DELETED (never reworded), each with its grep:**

| # | site | what goes |
|---|---|---|
| D1 | `crates/cyrup-ext-subagents/src/tui/fleet.rs:58-61` | delta 2 **in full** — *"**No Herdr inspector.** `handleHerdrInspectorAction` lives in `src/inspectors/herdr/`, a subtree this crate does not port…"*. The subtree IS ported. Renumber deltas 3→2 and 4→3. Also update the module-doc line `:8` (`` `H` opens the Herdr inspector ``) to upstream's neutral "inspector". |
| D2 | `crates/cyrup-ext-subagents/src/tui/fleet_overlay.rs:36-38` | the second half of the inherited-deltas note — *"…and there is no Herdr inspector to route `H` to (`fleet.rs` delta 2), so `has_inspect` is `false` and `H` takes pi's own "unavailable in this context" branch (`fleet.ts:692`)."* The steer-delivery-mode half (`:34-36`) stays true; keep it and drop the `Both are inherited` framing to `One is inherited`. |
| D3 | `crates/cyrup-ext-subagents/src/tui/fleet_overlay.rs:271-274` | the four-line *"…so this is unreachable from `handle_input` while `has_inspect` is false. Answered with pi's own message rather than a panic, because "unreachable" is a property of the caller, not of this function."* The `FleetPendingAction::Inspect` arm (`:275-278`) becomes a real `executor.inspector_open(cwd, target, /* focus */ true)` call whose `Err` keeps the existing fallback text. |
| D4 | `crates/cyrup-ext-subagents/src/tui/fleet_overlay.rs:618-635` | the test `an_inspect_action_answers_upstreams_herdr_failure_text` and its *"`has_inspect` is false, so `handle_input` never emits this"* comment. Replaced by a test that drives the arm and asserts it reached `inspector.open`. |
| D5 | `crates/cyrup-ext-subagents/src/extension/host/slash.rs:83-86` | the comment *"the Herdr inspector does not — see `tui/fleet.rs`'s delta 2"* and the `false` argument. Becomes `true` with a one-line note naming `builtin_inspector_plugins()`. |
| D6 | `crates/cyrup-ext-subagents/src/registration/authority.rs:43-46` | *"A SUBSET, not a copy: upstream declares EIGHT and this is the first six. `inspectorOpen` and `projectOpen` are omitted because the `inspector.*` / `project.*` verbs they gate are not ported…"* — the whole paragraph. |
| D7 | `crates/cyrup-ext-subagents/src/registration/authority.rs:108-112` | the `default_decision` doc's *"restricted to the six actions [`AUTHORITY_ACTIONS`] ports … Upstream's map has eight entries because it also carries `inspectorOpen: "auto"` and `projectOpen: "confirm"`, so the three/three split is a statement about THIS list, not about upstream's."* Replace with a straight port note: four `confirm` (`discardWorktree`, `destructiveCleanup`, `spawnBudgetGrant`, `projectOpen`), four `auto` (`scheduleCreate`, `stopRun`, `steerRun`, `inspectorOpen`), per `policy/authority.ts:16-25`. |
| D8 | `crates/cyrup-ext-subagents/src/registration/authority.rs:263-268` | the test doc's parenthetical *"(upstream's other two, `inspectorOpen`/`projectOpen`, gate unported verbs and are not in [`AUTHORITY_ACTIONS`])"* and the "six actions"/"three and three" wording. The test body gains two `assert_eq!`s. |
| D9 | `crates/cyrup-ext-subagents/src/extension/tool/routing.rs:1433-1437` | *"cyrup's `AUTHORITY_ACTIONS` (`registration/authority.rs:47-54`) is a closed SIX-entry list and upstream's (`policy/authority.ts:1-10` @v0.68.0) a closed EIGHT — cyrup omits `inspectorOpen`/`projectOpen`, whose verbs it has not ported —"*. The surviving claim (neither list has a member for any `refine*` verb) stays; the six/eight clause goes. (Its own anchor `:47-54` is off by one: the const spans `:47-55`.) |
| D10 | `crates/cyrup-ext-subagents/src/extension/tool/text.rs:160-165` | *"upstream's set … names actions this crate has not ported — exactly four of them, `inspector.open`, `inspector.close`, `project.open` and `project.close` — and grafting one of those onto a 7-entry port would make the runtime denylist message advertise a verb with no handler."* All four now have handlers. |
| D11 | `crates/cyrup-ext-subagents/src/extension/tool/text.rs:332-334` | *"cyrup still omits `inspector.*`/`project.*`, so the band from `worktree.discard` to `refine.rollback` is the …"* — the band is no longer what is contiguous. |
| D12 | `crates/cyrup-ext-subagents/src/extension/tool/schema.rs:987-990` | *"cyrup omits `inspector.*`/`project.*`, so the band from `worktree.discard` through `refine.rollback` is what is contiguous here now — re-derived from the v0.68.0 list, not patched."* Same reason. |

**Already false today, unrelated to this batch's landing — fix in passing (each is a one-line
citation), because the bar says a doc comment must state a TRUE premise:**

* `crates/cyrup-ext-subagents/src/missions/store.rs:460` — *"Exported: `inspectors/herdr/inspector-runner.ts:24`"*. **Two errors.** The file is `src/inspectors/inspector-runner.ts` (not under `herdr/`); the import is at `:5` and the call at `:27`. Grep: `git -C tmp/pi-subagents show v0.68.0:src/inspectors/herdr/inspector-runner.ts` → `fatal: path … does not exist`. This batch is the one that gives that reference a real Rust counterpart, so it owns the correction.
* `crates/cyrup-ext-subagents/src/tui/fleet.rs:59-60` cites `fleet.ts:51` for the optional `inspect` handler; at v0.68.0 it is `:100`. Dies with D1 anyway.

**`text.rs:186-190`'s exhaustiveness claim must be honoured.** It reads *"That list is exhaustive
against `:213`; anything appearing there and not here is a gap."* After this batch the list must gain
a bullet naming where `inspector.open`/`inspector.close`/`project.open`/`project.close` are refused
in child-safe mode — upstream refuses them in **two** places (`subagent-executor.ts:6299` inside the
`policyAction` block, then again at `:6313` and `:6320` inside each family's own arm), so cyrup's
arms carry the same inline `!self.allow_mutating_management && verb.is_mutating()` check the
`mission.*`/`lane.*`/`schedule.*`/`refine*` arms use. Do **not** extend
`discovery::management::MUTATING_MANAGEMENT_ACTIONS` (a 7-entry set scoped to
`route_management_action`'s CRUD, which these verbs do not route through) — the note at
`text.rs:158-160` says exactly that and *that* half stays true.

**`DESTRUCTIVE_MANAGEMENT_ACTIONS` already carries `inspector.close` and `project.close`**
(`text.rs:384-385`), ported verbatim ahead of dispatch with a doc (`:368-375`) that predicted this
day. Nothing to change there — and its premise stays true.

### 4. What cyrup ALREADY has (grep hard — most of the plumbing exists)

* `background/atomic.rs:75` `pub async fn write_atomic_json<T: Serialize + Sync>` — both binding writers.
* `background/control.rs:1258` `pub async fn request_async_steer(run_dir, message, target_index, source)` and `:1092` `pub async fn request_async_stop(run_dir, request: StopRequest)` — the runner's two control verbs. `:1283` `request_async_steer_with_mode` if a mode is ever needed.
* `background/control.rs:320` `pub(crate) async fn read_status_file(path) -> Result<Option<RunStatus>, SubagentError>` — upstream's `readStatus`.
* `background/fleet_view.rs:1034` `format_async_run_transcript` — the dashboard body. `:244` `fn path_within` — upstream's `pathWithin`, the exact helper `trustedDir` needs (also at `tui/fleet_transcript.rs:367`, `background/scheduled_runs/store.rs:178`; pick one and `pub(crate)` it rather than adding a fourth).
* `background/run_id_resolver` (re-exported `background/mod.rs:147-150`): `resolve_async_run_id`, `resolve_async_run_dir`, `AsyncRunLocation`, `ResolveRunIdError`, `find_async_run_prefix_matches` — upstream's `resolveSubagentRunId`.
* `missions/store.rs:466` `parse_mission_record`, `:695` `resolve_mission_store_location`, `:734` `mission_record_path`, `:962` `list_missions`; `missions/lifecycle.rs:812` `read_mission_binding` — every input `missionFor` needs.
* `registration/authority.rs` — `AUTHORITY_ACTIONS`, `AuthorityAction`, `AuthorityDecision`, `resolve_authority_decision`, `for_tool_action` (`:92-106`), `forbidden_message`/`no_ui_message`/`confirm_prompt`/`confirm_message`/`declined_message`. `launchFor`'s `allowSteer`/`allowStop` are `resolve_authority_decision(SteerRun|StopRun, policy) == Auto`, already available.
* `spawn/mod.rs:229` `SpawnCommand { binary, base_args }`, `:280` `resolve_spawn_command()` — both `launch_for`'s executable and `projectPaneCommand`'s `getPiSpawnCommand`.
* `artifacts.rs:162` `project_subagents_dir(cwd) -> <cwd>/.cyrup-subagents` — `projectPaneDir`'s base.
* `extension/executor/paths.rs:23` `default_async_root_in`, `:29` `default_results_dir_in`, `:196` `trusted_session_roots` — `DIRS.async`, `DIRS.results`, `state.trustedSessionRoots`.
* `watchdog/permission_arbiter.rs:247` `redact_secret_values` — half of `steeringMessagePreview`.
* `extension/tool/text.rs:376-390` `DESTRUCTIVE_MANAGEMENT_ACTIONS` **already contains `inspector.close` and `project.close`**.
* `tui/fleet.rs:1405-1409` `FleetPendingAction::Inspect { target }` and `:1467-1468` `has_inspect` — the plumbing is all there; only the handler and the flag are missing.
* `extension/host/native_impl.rs` `HostEvent::SessionStart` arm (`:314` onward, the `reset_spawn_budget` neighbourhood at `~:355-360`) — the session-start hook `restoreHerdrProjectPaneSnapshots` attaches to.
* `registration/guide.rs:290-300` `the_tool_reference_topic_names_every_dispatched_verb` — an **existing** mechanical gate: adding seven verbs to `SUBAGENT_ACTIONS` without documenting them in `resources/docs/tool-reference.md` turns it red. Free reachability.
* `crates/cyrup-it/tests/subagents/fleet_inspector_integration.rs` and `main.rs`'s 40 `mod` lines — the IT idiom and the registration point.

**Genuinely missing, must be written:** the whole `inspectors/` subtree; `steering_receipt` +
`steering_message_preview` + `preview_display_text`; `selected_inspect_action`; a
`ProjectPaneSnapshot` field on `FleetState` and a `project-pane` surface on `FleetStatusEntry`
(`tui/fleet_status.rs:161-179` has no `surface`/`project_pane` field at all, and upstream's
`projectPaneEntries` at `fleet-status.ts:351-365` is the **only remaining production reader** of the
restored map now that `herdr-status.ts` is out of scope — without it the restore writes state nothing
reads, which is exactly the dead-code the bar forbids).

### 5. Rust shape

```
crates/cyrup-ext-subagents/src/inspectors/
    mod.rs                  InspectorAction enum + INSPECTOR_ACTIONS + re-exports
    types.rs                InspectorParams, InspectorTarget, InspectorContext, InspectorLaunch,
                            trait InspectorPlugin
    actions.rs              handle_inspector_action, resolve_target, trusted_dir, mission_for,
                            launch_for, InspectorDispatcherDeps
    session_roots_codec.rs  encode_session_roots / decode_session_roots (+ the PowerShell doc,
                            ported verbatim — it is the reason the encoding exists)
    shell_command.rs        format_shell_command(exe, args, Platform)
    plugins.rs              builtin_inspector_plugins(...) -> Vec<Box<dyn InspectorPlugin>>
    runner.rs               INSPECTOR_SUBCOMMAND, RunnerOptions, parse_args,
                            format_inspector_dashboard, submit_inspector_control, run_inspector
    herdr/{mod,client,actions,focus,plugin,project_panes}.rs
    ghostty/{mod,actions,plugin}.rs
crates/cyrup/src/subagent_inspector_cmd.rs
```

**Enums and newtypes, per the directive:**

```rust
pub enum InspectorAction { Open, Command, Status, Close }   // from_wire / as_str / is_mutating
pub enum ProjectPaneAction { Open, Status, Close }          // ditto
pub enum HerdrErrorCode { Unavailable, UnsupportedVersion, PaneGone, NotFound, Timeout, ValidationError }
pub enum HerdrFocusErrorCode { Herdr(HerdrErrorCode), PaneFocusUnsupported, InvalidPaneResponse }
pub enum ProjectPaneErrorCode { Herdr(HerdrErrorCode), InvalidProjectRoot, InvalidPaneResponse,
    InvalidBinding, BindingReadFailed, BindingWriteFailed, BindingRemoveFailed,
    PaneFocusUnsupported, PaneNotIdle, PaneOwnershipUnverified }
pub enum PaneOwnership { Verified, Unknown, Mismatch }
pub enum ProjectPaneState { Absent, Open, Stale }
pub enum OpenDisposition { Opened, AlreadyOpen }
pub enum CloseDisposition { Closed, Absent, StaleBindingRemoved }
pub enum Platform { Win32, Unix }       // shell_command's two branches, injectable
```

`HerdrErrorCode::as_str` must emit upstream's SCREAMING_SNAKE spellings verbatim — they appear inside
`Herdr inspector error ({code}): {message}` and `Herdr project pane error ({code}): {message}`, which
are model-visible and are asserted in §6. `normalize_code` (`client.ts:35-41`) is a `&str`
lowercase-contains ladder; port it as a `fn from_wire_lossy(raw: &str) -> HerdrErrorCode` and keep
the fallthrough to `ValidationError`.

**Traits (dependency injection is what makes this testable without herdr or macOS):**

```rust
#[async_trait::async_trait]
pub trait HerdrClient: Send + Sync {
    async fn run(&self, args: &[&str], options: HerdrRunOptions) -> HerdrResult<serde_json::Value>;
}
#[async_trait::async_trait]
pub trait InspectorPlugin: Send + Sync {
    fn name(&self) -> &'static str;
    async fn available(&self, ctx: &InspectorContext) -> bool;
    fn owns(&self, ctx: &InspectorContext) -> bool;
    async fn open(&self, ctx: &InspectorContext, launch: &InspectorLaunch, params: &InspectorParams) -> ToolResult;
    async fn status(&self, ctx: &InspectorContext) -> Option<ToolResult>;   // None == upstream's absent method
    async fn close(&self, ctx: &InspectorContext) -> Option<ToolResult>;
}
#[async_trait::async_trait]
pub trait GhosttyRunner: Send + Sync {
    async fn run(&self, args: &[&str]) -> Result<CommandOutput, GhosttyError>;
}
```

`status`/`close` returning `Option<ToolResult>` is how the optional `status?`/`close?` methods port —
`None` drives upstream's `"Inspector plugin '{name}' does not support status for async run {runId}."`
/ `… does not support close …` messages (`actions.ts:141-147`, both `isError: true`). The crate
already uses `async_trait` (`exec/attempt_runner.rs:109`, `exec/fallback.rs:1738`), so this is the
house idiom.

**The real clients:** `SpawnedHerdrClient { bin: PathBuf }` over `tokio::process::Command` (the
crate's idiom — `spawn_detached.rs:214`, `spawn/worktree.rs`'s `run_git`) with
`tokio::time::timeout`, `kill_on_drop(true)`, `Stdio::piped()`, and `HERDR_BIN`-then-`"herdr"`
resolution (`client.ts:44`). `OsascriptRunner` is the same over `/usr/bin/osascript` with
`timeout: 15_000` and a 64 KiB stdout cap (`ghostty/actions.ts:61-66`).

**On-disk types — the files are SHARED with pi, so keys do not change and unknown keys survive:**

```rust
#[derive(Serialize, Deserialize)] #[serde(rename_all = "camelCase")]
pub struct HerdrInspectorBinding {
    pub schema_version: SchemaVersion1,          // validating newtype, serialises as `1`
    pub kind: HerdrInspectorKind,                // one variant, serialises as "herdr-inspector"
    pub run_id: String, pub async_dir: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")] pub child_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")] pub mission_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub mission_path: Option<PathBuf>,
    pub pane_id: String, pub opened_at: String,
    #[serde(skip_serializing_if = "Option::is_none")] pub last_focused_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub herdr_version: Option<String>,
    pub command: String,
    #[serde(flatten)] pub extra: serde_json::Map<String, serde_json::Value>,
}
```

**No `deny_unknown_fields`** — see §0. `#[serde(flatten)] extra` is what makes a round-trip through
cyrup preserve a key pi added; a plain tolerant struct that *drops* unknown keys is a silent data
loss on `focus` (which rewrites the binding, `project-panes.ts:529-531`). `skip_serializing_if` is
required, not cosmetic: upstream's conditional spreads (`herdr/actions.ts:122-126`) omit absent keys
entirely, and a `"childIndex": null` would fail pi's own `parse` check
(`binding.childIndex !== undefined && !Number.isInteger(...)` — `null` is not `undefined`).

`binding_path(async_dir, index)` → `<asyncDir>/inspectors/herdr{,-<index>}.json` (`:24-26`).
`project_pane_binding_path(root)` → `<root>/.cyrup-subagents/project-panes/herdr.json` (`:172-178`);
root index at `herdr-roots.json` (`:180-182`).

**Errors:** one `thiserror` enum per layer (`InspectorError`, `HerdrError`, `ProjectPaneError`) with
`Display` producing upstream's sentence, so a `Result` collapses to a `ToolResult` at exactly one
place per verb. The `ok/err` split maps to upstream's `isError` — **and note the three upstream
messages that are deliberately NOT errors** (§2): `No inspector plugin owns this binding…`,
`No Herdr project pane binding exists for …` (×2). Encode that as `ToolResult { is_error: false }`,
not as `Err`.

**The argv parser** (`runner.rs`) mirrors `parseArgs` (`inspector-runner.ts:50-79`) exactly: strictly
pairwise, `argv[i]` must start with `--` and `argv[i+1]` must exist or
`"Invalid inspector argument '{key}'."`; `--async-dir` and `--run-id` required or
`"Inspector requires --async-dir and --run-id."`; `--index` a non-negative integer or
`"--index must be a non-negative integer."`; `--refresh-ms` an integer `>= 250` or
`"--refresh-ms must be an integer >= 250."`, defaulting to 1 500; `--allow-steer`/`--allow-stop` are
`!= "false"` (i.e. anything but the literal `"false"` is true — port that literally, it is not a bool
parse); `--session-roots` through `decode_session_roots` or
`"--session-roots must be a base64-encoded JSON array of strings."`

**ghostty is `cfg`-gated at the availability check, not compiled out** — `available()` is
`cfg!(target_os = "macos") && env TERM_PROGRAM is "ghostty"` (case-insensitive). The AppleScript
constant, the runner trait and `open_ghostty_inspector` compile on every platform, so the whole
plugin is unit-testable on Linux CI with an injected runner. Upstream does exactly this
(`deps.platform ?? process.platform`, `ghostty/plugin.ts:10`).

### 6. Production call sites

1. **`extension/tool/routing.rs` `route_action`** (`:1085`, `match action` at `:1096`) — two new guard
   arms, shaped like the `schedule.*` arm at `:1478-1540` (guard → child-safe check → authority
   consult → dispatch), placed to mirror upstream's order (`subagent-executor.ts:6312` project panes,
   then `:6319` inspector):
   * `project_action if ProjectPaneAction::from_wire(..).is_some()` — child-safe refusal for
     `Open`/`Close` with the crate's existing sentence
     `"Action '{action}' is not available from child-safe subagent fanout mode."`, then
     `AuthorityAction::for_tool_action("project.open") == Some(ProjectOpen)` through the same
     three-arm gate, then `handle_herdr_project_pane_action`.
   * `inspector_action if InspectorAction::from_wire(..).is_some()` — same shape;
     `"inspector.open" → InspectorOpen`; `plugins: builtin_inspector_plugins(...)`.
   Upstream's authority consult sits **above** both arms (`:6294-6311`) and refuses child-safe
   **first** (`:6296-6301`: *"Refuse first, so the gate never prompts for an action that is going to
   be rejected anyway"*) — port that ordering comment and that ordering.
2. **`registration/authority.rs`** — `AUTHORITY_ACTIONS` gains `"inspectorOpen"`, `"projectOpen"`
   (upstream's order, `policy/authority.ts:1-10`); `AuthorityAction` gains `InspectorOpen`,
   `ProjectOpen`; `as_str`, `default_decision` (`inspectorOpen → Auto`, `projectOpen → Confirm`,
   `policy/authority.ts:23-24`) and `for_tool_action` (`"inspector.open"`, `"project.open"`) all
   extend. D6/D7/D8 die here.
3. **`extension/tool/text.rs:239-366`** — seven entries at pi's own indices: `… "refine.rollback",
   "inspector.open", "inspector.command", "inspector.status", "inspector.close", "project.open",
   "project.status", "project.close", "status", …` (`shared/types.ts:2801`). **52 → 59.**
   D10/D11 die here. `DESTRUCTIVE_MANAGEMENT_ACTIONS` needs no change.
4. **`extension/tool/schema.rs`** — the same seven in the `action` enum; D12 dies here; the
   `props["action"]["enum"]` assertion (`:1324`) and `text.rs`'s own list assertion (`:653`) both
   re-pin.
5. **`registration/guide.rs`** → `resources/docs/tool-reference.md` gains all seven, or
   `the_tool_reference_topic_names_every_dispatched_verb` (`:290-300`) goes red.
6. **`tui/fleet.rs`** — `fn selected_inspect_action`, `fn inspect_selected`, the `FleetKey::Enter`
   top-level arm, `self.has_inspect` retained but now genuinely true, the message corrected to
   upstream's `"Inspector controls are unavailable in this context."` D1 dies here.
7. **`tui/fleet_overlay.rs:275-278`** — `FleetPendingAction::Inspect` becomes
   `executor.inspector_open(cwd, target, /* focus */ true)` keeping the existing fallback sentence.
   D2/D3/D4 die here.
8. **`extension/host/slash.rs:86`** — `false` → `true`. D5 dies here.
9. **`extension/host/native_impl.rs`, `HostEvent::SessionStart`** — after `reset_spawn_budget`,
   `restore_herdr_project_pane_snapshots(state, roots)` where
   `roots = existing keys ∪ list_herdr_project_pane_roots(owner_root) ∪ {owner_root}`
   (`extension/index.ts:980-981`, verbatim).
10. **`tui/fleet_status.rs`** — `project_pane_entries` folded into
    `collect_fleet_status_entries` (`:367`), `FleetStatusEntry` (`:161-179`) gains the
    `surface`/`project_pane` fields upstream has (`fleet-status.ts:351-365`). This is the reader that
    makes #9's write live.
11. **`crates/cyrup/src/{predispatch,main,lib}.rs` + `subagent_inspector_cmd.rs`** — §1.

### 7. Reachability tests, and why each fails if the implementation is gutted

Unit (`--features test-fixtures`) unless marked **IT** (`cyrup-it --features it`).

* **T-CMD-1 `inspector_command_returns_the_launch_with_no_backend_installed`** — a real on-disk async
  run + an env with no `HERDR_ENV`/`TERM_PROGRAM`; `route_action("inspector.command")` returns a
  string containing the resolved binary, `__subagent-inspector`, `--async-dir <dir>`,
  `--run-id <id>`, `--allow-steer true`, `--allow-stop true` and a `--session-roots` value that
  `decode_session_roots` round-trips back to the seeded roots. *Gutted* → a `todo!()` panics; a
  stub returning `""` fails the substring asserts; **wiring `inspector.command` after the plugin loop
  (the natural mistake) makes it refuse with the no-plugin sentence and fails.**
* **T-CMD-2 `session_roots_survive_a_shell_round_trip`** — property-ish: roots containing `'`, `"`,
  `\`, a space and a non-ASCII char encode to a base64 string matching `^[A-Za-z0-9+/=]*$` and decode
  identically; `format_shell_command` on Win32 emits a leading `& ` and on Unix wraps a
  quote-needing exe as `sh -c 'exec "$0" "$@"' '<exe>'`. *Gutted* → a JSON-inline `--session-roots`
  passes the decode leg but fails the charset assert, which is the exact PowerShell bug
  `session-roots-codec.ts:1-12` exists to prevent.
* **T-OPEN-0 `inspector_open_refuses_with_upstreams_sentence_and_writes_nothing`** — env with neither
  backend; assert `is_error` **and** the byte-exact sentence *"No inspector plugin is available.
  Start a supported inspector host, or use inspector.command for a standalone command."* **and** that
  `<asyncDir>/inspectors/` does not exist afterwards. *Gutted* → any invented refusal text fails; a
  handler that writes a binding before checking availability fails the directory assert.
* **T-OPEN-1 `inspector_open_performs_split_run_write_through_a_fake_client`** — `FakeHerdrClient`
  scripted for `--version → 0.7.5`, `pane split … → {"pane_id":"p1"}`, `pane run p1 <cmd> → {}`;
  assert the recorded argv sequence is exactly
  `["--version"]`, `["pane","split","--current","--direction","right","--cwd",<cwd>,"--no-focus"]`,
  `["pane","run","p1",<displayCommand>]`; assert the binding file parses, `paneId == "p1"`,
  `kind == "herdr-inspector"`, `schemaVersion == 1`, `command == displayCommand`; assert the reply is
  *"Opened read-only Herdr inspector pane p1 for async run {id}. Closing the pane does not stop the
  run.\nControls inside the pane: steer <message>, stop, status."* *Gutted* → argv order or flag
  spelling drift fails; herdr rejects a wrong flag in production, so this is the only place the
  contract is pinned.
* **T-OPEN-2 `a_pane_run_failure_closes_the_pane_and_writes_no_binding`** — `pane run` scripted to
  fail; assert `["pane","close","p1"]` was issued and `binding_path` does not exist. *Gutted* →
  dropping the cleanup leaves an orphan pane in the user's terminal; dropping the write-skip leaves a
  binding pointing at a closed pane, which `inspector.status` then reports as open. Both fail here.
* **T-OPEN-3 `an_already_open_pane_is_reported_not_reopened`** — pre-seed a binding, script
  `pane get p1 → {"pane_id":"p1"}`; assert *"Herdr inspector pane p1 is already open for async run
  {id}."*, no `pane split` recorded, binding byte-identical. With `focus: true` the sentence gains
  *" Herdr cannot refocus an arbitrary raw pane id; select it in the Herdr UI."* (`:100`).
* **T-STAT-1 `status_and_close_round_trip_the_binding`** — `inspector.status` on the T-OPEN-1 binding
  returns *"Herdr inspector p1 is open for async run {id}.\nRun state: {state}\nBinding: {path}"*;
  `inspector.close` issues `["pane","close","p1"]`, removes the file, and returns *"Closed Herdr
  inspector pane p1 for async run {id}. The subagent run was not stopped."*
* **T-STAT-2 `status_with_no_binding_is_not_an_error`** — asserts `is_error == false` and *"No
  inspector plugin owns this binding for async run {id}."* *Gutted* → the natural `Err` shape flips
  `is_error` and fails. This is the single easiest behaviour to get wrong.
* **T-STAT-3 `close_tolerates_a_pane_that_is_already_gone`** — `pane close` → `NOT_FOUND`; assert the
  binding is still removed and the reply is the success sentence (`herdr/actions.ts:150-154`).
* **T-TRUST-1 `a_dir_outside_the_trusted_roots_is_refused`** — `params.dir` pointing at a tempdir
  outside the async root: *"Async run directory '{dir}' is outside trusted run roots."*; a symlink
  into the async root is **also** refused (`lstatSync(dir).isSymbolicLink()`, `actions.ts:36`);
  a real dir inside the root is accepted. *Gutted* → dropping the `lstat` symlink check is a
  path-escape and only this case catches it.
* **T-GHOST-1 `ghostty_opens_through_an_injected_runner_on_any_platform`** — plugin constructed with
  `Platform::Darwin` + `TERM_PROGRAM=Ghostty` + a fake runner returning `"term-7"`; assert the argv is
  `["-e", GHOSTTY_APPLESCRIPT, "--", <displayCommand>, <cwd>, "false"]` and the reply is *"Opened
  read-only Ghostty inspector terminal term-7 for async run {id}. Status and close are unavailable
  because this plugin writes no binding."*
* **T-GHOST-2 `an_empty_terminal_id_and_a_runner_error_both_carry_the_hint`** — both replies end with
  *"Ghostty inspector requires Ghostty 1.3+ and Automation permission for osascript."*
* **T-GHOST-3 `ghostty_owns_nothing_so_it_is_never_the_status_owner`** — with ghostty available and
  no herdr binding, `inspector.status` still answers T-STAT-2's sentence.
* **T-PROJ-1 `project_open_status_close_round_trip_through_a_fake_client`** — the `pane split
  --cwd <projectRoot>` → `pane run` → binding at `<root>/.cyrup-subagents/project-panes/herdr.json`
  → root index at `herdr-roots.json` sequence; then `status` reports open; then `close` **refuses**
  while `agent_status != "idle"` with *"Herdr project pane '{id}' is '{status}', not explicitly
  idle."* and succeeds once idle, removing both the binding and the root-index entry. *Gutted* →
  dropping the idle gate closes a pane running someone's live session.
* **T-PROJ-2 `project_status_and_close_answer_with_no_herdr_binary_at_all`** — a client whose every
  `run` returns `HERDR_UNAVAILABLE`; both verbs still answer *"No Herdr project pane binding exists
  for {root}."* with `is_error == false` and **never invoke the client**. *Gutted* → a
  `detect_herdr` call at the top of `status`/`close` (the symmetric-looking mistake) makes both fail.
* **T-PROJ-3 `project_open_with_no_herdr_binary_carries_upstreams_install_sentence`** —
  *"Herdr project pane error (HERDR_UNAVAILABLE): Herdr is not installed or is not on PATH. Install
  Herdr 0.7.5+ or set HERDR_BIN."*
* **T-PROJ-4 `herdr_below_0_7_5_is_refused`** — `--version → 0.7.4` → *"Herdr 0.7.4 does not support
  raw inspector panes. Upgrade to Herdr 0.7.5 or newer."*; `0.7.5` and `1.0.0` are accepted.
* **T-PROJ-5 `ownership_mismatch_refuses_close`** — `pane get` returns a `cwd` other than the project
  root → *"Project pane '{id}' ownership is 'mismatch' for '{root}'."*, binding untouched.
* **T-RESTORE-1 (IT) `a_restored_project_pane_appears_in_the_fleet_status_roster`** — write a binding
  and a root index on disk, drive a real `SessionStart` through the host, then
  `collect_fleet_status_entries` contains an entry keyed `project-pane:<root>` with the pane id in
  its `agent` field. *Gutted* → restoring into a map nothing reads passes a shallow "is the map
  populated" test but fails this one, which is the whole point: it forces the §6.10 reader to exist.
* **T-AUTH-1 `authority_has_eight_actions_with_pis_own_defaults`** — `AUTHORITY_ACTIONS` equals
  upstream's eight in upstream's order; `InspectorOpen.default_decision() == Auto`;
  `ProjectOpen.default_decision() == Confirm`; `for_tool_action("inspector.open")` and
  `("project.open")` resolve; `for_tool_action("inspector.command"|"inspector.status"|
  "inspector.close"|"project.status"|"project.close")` all return `None`. *Gutted* → adding the
  actions without the `for_tool_action` mapping leaves two policy keys an operator can set and
  nothing consults — which is precisely the sin D6 named.
* **T-AUTH-2 `project_open_forbidden_by_policy_is_refused_before_any_client_call`** — policy
  `{"projectOpen": "forbid"}` → `"Authority policy forbids action 'project.open'."` and the fake
  client recorded zero calls.
* **T-CHILD-1 `the_four_mutators_are_refused_in_child_safe_fanout_and_the_three_reads_are_not`** —
  over `inspector.open`, `inspector.close`, `project.open`, `project.close` assert
  *"Action '{action}' is not available from child-safe subagent fanout mode."*; over
  `inspector.command`, `inspector.status`, `project.status` assert a real answer. *Gutted* → gating
  the whole family (the easy shortcut) fails the second half.
* **T-CHILD-2 `the_child_safe_refusal_precedes_the_authority_prompt`** — child-safe registration +
  `{"projectOpen":"confirm"}` + a host with no UI: the reply is the child-safe sentence, **not**
  `"Authority policy requires user confirmation…"`. Pins `subagent-executor.ts:6296-6301`'s stated
  ordering.
* **T-DASH-1 `the_dashboard_header_and_controls_line_are_upstreams`** — `format_inspector_dashboard`
  over a seeded status emits `"pi-subagents inspector for {runId}"`-equivalent header, the mirror
  sentence *"This inspector mirrors lifecycle artifacts; closing it does not stop the run."*, and
  `Controls: type guidance | steer <message> | stop | status` for a `single`-mode run, degrading to
  `Controls: steer <message> | status` when `allow_stop == false` and to `Controls: status` when both
  are false. *Gutted* → a fixed controls string fails three of the four cases.
* **T-CTL-1 `steer_and_stop_from_stdin_land_real_requests_on_the_control_channel`** —
  `submit_inspector_control(opts, "steer hello")` produces a real steer request file under the run
  dir whose payload carries `source: "inspector-runner"` and the message, and returns a
  `steering_receipt`-formatted string containing a fenced `Message sent:` block; `"stop"` produces a
  real stop request. *Gutted* → a receipt-only implementation writes no file and fails.
* **T-CTL-2 `the_control_refusals_are_upstreams`** — `"steer "` (empty) →
  *"steer requires a message."*; `"reply x"` → *"Supervisor replies are owned by the parent Pi
  session; use subagent_supervisor/intercom there."*; plain guidance on a non-`single` aggregate with
  no `--index` → *"Plain guidance requires a child-specific inspector. Use steer <message> to target
  all running children from the aggregate inspector."*; `stop` with `allow_stop == false` →
  *"Authority policy does not allow stop from this inspector."*; `steer` on a terminal run →
  *"Run '{id}' is {state} and cannot be steered."*
* **T-ARGV-1 `the_runner_argv_parser_matches_parse_args_exactly`** — table-driven over each refusal
  in §5's parser paragraph, plus the defaults (`refresh_ms == 1500`, `allow_steer == true`), plus
  `--allow-steer FALSE` (uppercase) parsing as **true** (upstream is a literal `!== "false"`).
* **T-RUN-1 (IT) `cyrup___subagent_inspector_renders_a_real_run_and_a_steer_line_lands`** — spawn the
  real `cyrup` binary with `__subagent-inspector --async-dir <dir> --run-id <id> --refresh-ms 250
  --session-roots <b64>` against an on-disk run, feed `steer hello\n` on stdin, assert the run's
  control channel gained a steer request naming `inspector-runner`, feed `stop\n`, assert the stop
  request, then terminalise the run and assert the process's refresh loop stops. *Gutted* → a
  subcommand that classifies but does not dispatch exits non-zero with no output and fails; a
  `predispatch` arm added without the `main.rs` dispatch arm falls through to clap, which rejects
  `--async-dir` — this test is the only thing that catches that split.
* **T-RUN-2 (IT) `the_inspector_subcommand_is_undiscoverable`** — `cyrup --help` mentions neither
  `__subagent-runner` nor `__subagent-inspector`; `subcommands::SUBCOMMANDS` contains neither.
* **T-FLEET-1 `H_and_Enter_both_route_to_inspector_open_with_focus_true`** — drive the component with
  a selected actionable async child; both keys emit `FleetInputOutcome::RunAction(Inspect{target})`
  with the right target; the overlay's `run_fleet_action` reaches `inspector.open` with `focus: true`
  (recorded by a fake executor seam). *Gutted* → `has_inspect: false` left in place makes both keys
  emit the unavailable notice and this fails.
* **T-FLEET-2 `the_inspect_target_resolver_is_upstreams_not_the_async_one`** — an external item →
  *"External jobs are display-only and have no inspector controls."*; a `foreground-active` item with
  a live parent workflow → the **parent's** run id and async dir; the same item with a settled parent
  → *"The parent workflow is no longer available for inspection."* *Gutted* → reusing
  `selected_async_action` makes all three fail.
* **T-FLEET-3 `the_unavailable_message_is_upstreams_v0_68_0_wording`** — with `has_actions == false`,
  `H` answers *"Inspector controls are unavailable in this context."* — no `"Herdr"`. *Gutted* →
  leaving the old string fails; this is the assertion that makes D1's deletion a behaviour change
  rather than a comment edit.
* **T-DOC-1** — the existing `the_tool_reference_topic_names_every_dispatched_verb`
  (`registration/guide.rs:290-300`) and the existing `SUBAGENT_ACTIONS`/schema list assertions
  (`text.rs:653`, `schema.rs:1324`) all re-pin at 59 entries. No new test needed; they are already
  the gate.

### 8. Files touched

**New —** `crates/cyrup-ext-subagents/src/inspectors/{mod,types,actions,plugins,shell_command,
session_roots_codec,runner}.rs`; `.../inspectors/herdr/{mod,client,actions,focus,plugin,
project_panes}.rs`; `.../inspectors/ghostty/{mod,actions,plugin}.rs`;
`crates/cyrup/src/subagent_inspector_cmd.rs`;
`crates/cyrup-it/tests/subagents/inspector_verbs_integration.rs`;
`crates/cyrup-it/tests/subagents/inspector_runner_subcommand_integration.rs`.

**Edited —** `crates/cyrup-ext-subagents/src/lib.rs` (`pub mod inspectors;`);
`.../src/extension/tool/{routing,text,schema}.rs`; `.../src/registration/authority.rs`;
`.../src/registration/guide.rs` only if a topic name changes (it should not);
`.../resources/docs/tool-reference.md`; `.../src/tui/{fleet,fleet_overlay,fleet_state,fleet_status}.rs`;
`.../src/extension/host/{slash,native_impl}.rs`; `.../src/extension/executor/mod.rs` (the
`inspector_open`/`inspector_status`/`inspector_close`/`project_*` executor entry points the overlay
and routing call); `.../src/missions/store.rs:460` (the false citation); `.../src/background/mod.rs`
or `fleet_view.rs` (`pub(crate)` the one `path_within`); a new `steering_receipt` home
(`background/control.rs` or a new `background/steering.rs`);
`crates/cyrup/src/{lib,main,predispatch}.rs`; `crates/cyrup-it/tests/subagents/main.rs`.

### 9. Sizing — honest

**This is the largest remaining batch in the set, and it is not one shape.** Roughly 1 590 upstream
lines plus a cross-crate CLI seam plus eleven delta deletions plus ~28 tests. It does not decompose
into "small" and "large" evenly — it decomposes into four independent halves plus a tail, and the
executor should land them in this order because each later one depends on the earlier's types:

1. **The plugin-free core — small, and it ships a working verb on its own.** `types.rs`,
   `shell_command.rs` (16 lines), `session_roots_codec.rs` (31), `actions.rs`'s
   `resolve_target`/`trusted_dir`/`mission_for`/`launch_for`, the `InspectorAction` enum, the
   `route_action` arm, the `SUBAGENT_ACTIONS`/schema/tool-reference churn, and the authority
   additions with D6-D10/D12 deleted. **`inspector.command` works end to end after this step, on a
   box with no herdr and no ghostty, and T-CMD-1/2, T-OPEN-0, T-TRUST-1, T-AUTH-1/2, T-CHILD-1/2 and
   T-DOC-1 all pass.** ~600 Rust lines. Do this first; it is the step that converts the batch from
   "all or nothing" into "already useful".
2. **The runner + the cross-crate subcommand — medium, and the riskiest.** `runner.rs` (153 upstream
   lines, but the dashboard leans entirely on `format_async_run_transcript`, which exists), plus the
   `steering_receipt` port (~15 lines), plus the four-file `crates/cyrup` seam (§1), plus T-DASH-1,
   T-CTL-1/2, T-ARGV-1, T-RUN-1/2. The *risk* is not size — it is that classification and dispatch
   live in different crates and a half-wired seam fails silently by falling through to clap.
   T-RUN-1 is the only thing that catches it; write it before the implementation.
3. **herdr — large, and mechanically so.** `client.rs` (130), `focus.rs` (55), `actions.rs` (155),
   `plugin.rs` (20) = ~360 upstream lines → ~700 Rust with the enums, the trait, the fake client and
   T-OPEN-1/2/3, T-STAT-1/2/3, T-PROJ-3/4. No design decisions left; the shape is in §5.
4. **project panes — large, and the one with real design left.** `project-panes.ts` is 730 lines,
   ~45 % of the subtree, and it carries the `legacyToolCompatibility` double-standard (the model-
   facing verbs parse loosely and tolerate `INVALID_PANE_RESPONSE`; the public API parses strictly
   and refuses unverified ownership — `:427-450`, `:486-508`, `:552-576`). **Port both modes**, as
   one manager with a `ToolCompatibility` flag exactly as upstream does; collapsing them to one is
   the narrowing the bar forbids, and it would either make the tool verbs fail on a pane herdr
   reports oddly or make the API silently close someone else's pane. Plus the `FleetState` /
   `FleetStatusEntry` extension and T-RESTORE-1.
5. **The tail — small but fiddly.** ghostty (91 upstream lines, T-GHOST-1/2/3), the fleet `H`
   rewiring with D1-D5 deleted (T-FLEET-1/2/3), the `missions/store.rs:460` citation fix, and the
   session-start restore hook.

Steps 1 and 2 touch `crates/cyrup` and the shared action lists; steps 3-5 are confined to
`cyrup-ext-subagents`. **Nothing here is a blocker and nothing may be narrowed** — in particular, do
not land "`inspector.command` only" and call the batch done: the seed's Definition of Done is seven
verbs, and a verb advertised without a dispatch arm violates this crate's own
advertise-vs-dispatch invariant (`extension/tool/text.rs:122`). Land step 1 first *as a working
increment*, then keep going.

### 10. Blockers

**One, already named by the seed and now merged:** the `children.list`/`debug.run` batch (#146) edits
`SUBAGENT_ACTIONS` and `route_action`. It has landed — `children.list` sits at index 3 and
`debug.run` at index 14 of the 52-entry list, and `route_action` has a `"children.list"` arm at
`routing.rs:1105`. **No remaining dependency.** Every other input this batch needs
(`format_async_run_transcript`, `request_async_steer`/`_stop`, `write_atomic_json`,
`parse_mission_record`, `resolve_async_run_id`, `resolve_spawn_command`, `project_subagents_dir`,
`resolve_authority_decision`, `FleetPendingAction::Inspect`) exists today with the signatures quoted
in §4. `steering_receipt` and `preview_display_text` do not exist, but they are ~20 lines of this
batch's own work, not a missing dependency.


---

## [SCOPE CORRECTION — orchestrator, 2026-09-20] `herdr-status.ts` is BACK IN. And a whole feature is unfiled.

### 1. I excluded a file VL-S6's own row names. Reversing that.

The seed says *"OUT OF SCOPE: `integrations/herdr-status.ts` (400 lines, the status bridge)"*.
That exclusion was written by the orchestrator to keep the batch tractable and it is **wrong on
the ledger's own terms**: VL-S6's row (`PARITY-GAPS.md:1493-1497`) names exactly two upstream
paths, and `src/integrations/herdr-status.ts` is one of them. Excluding it means VL-S6 could not
close even if all seven verbs landed.

**`herdr-status.ts` (400 lines) is IN SCOPE**: `HerdrStatusBridgeEvents:23`,
`HerdrStatusRun:28`, `HerdrStatusBridgeOptions:38`, `HerdrStatusBridge:54`,
`registerHerdrStatusBridge:128`. Its one upstream consumer is `src/extension/index.ts`, so the
cyrup seam is the extension's own registration path. This is what makes herdr's runs VISIBLE in
the fleet rather than merely controllable by verb — without it the port is seven verbs over a
surface the user cannot see, which is precisely the half-usable shape this project does not ship.

The AUG's own §5 already noticed the consequence and worked around it: *"the restored map now
that `herdr-status.ts` is out of scope — without it the restore writes state nothing reads"*.
**That workaround is deleted.** With the bridge in scope, the restored project-pane map has a
real reader and `FleetStatusEntry`'s new `surface`/`project_pane` field is live state, not a
placeholder. Re-do that part of the design against the bridge.

`src/runs/shared/herdr-connection.ts` stays out of THIS batch for the reason in §2 below — not
because it is large, but because it belongs to a feature that deserves its own scoping rather
than being smuggled in as a dependency of the panes.

### 2. An entire feature is missing from the ledger: REMOTE SUBAGENT PLACEMENT

`grep -rn 'herdr-machine\|herdr-placed\|herdr-connection\|external-adapters\|herdr-pi-bridge'
docs/gap-analysis/*.md` returns **nothing**. Not one row. Meanwhile at v0.68.0:

| upstream file | lines | what it is |
|---|---|---|
| `src/runs/shared/herdr-machine.ts` | 279 | the remote machines a run can be placed on |
| `src/runs/shared/herdr-placed-run.ts` | 263 | a subagent run PLACED on one of them |
| `src/runs/shared/herdr-external-adapters.ts` | 169 | the external-runner adapters over that transport |
| `src/extension/herdr-pi-bridge.ts` | 160 | the extension-side bridge |
| `src/runs/shared/herdr-connection.ts` | 134 | the SSH connection to a remote machine |
| `src/runs/shared/herdr-pi-protocol.ts` | 59 | the wire protocol |
| | **1 064** | |

cyrup has **zero** of it (`grep -rli herdr crates/cyrup-ext-subagents/src` returns only files
that MENTION herdr in a comment or a refusal string). This is "run your subagents on other
machines" — a first-class capability for anyone delegating real work — and it has never been
written down as a gap, which is why no batch has ever been scoped for it.

**Action: file it as a new ledger row** (`VL-S14 · Remote subagent placement`), sized and
cited, so it is tracked rather than invisible. It is NOT smuggled into this batch: it is a
feature of its own and deserves its own augment, its own exec, its own PR. But it is also NOT
allowed to stay unfiled, and "we never noticed" is not a reason to keep not noticing.

### 3. The rule this section enforces

The orchestrator wrote the "out of scope" line in §0 of this seed, not an agent. The lesson is
the same either way: **before writing "out of scope", check what the ledger row actually
claims, and grep for what else exists next to it.** A batch scoped for tractability that cannot
close the row it was scoped for is not a batch, it is a detour.
