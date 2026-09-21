---
stage: done
status: completed
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

---

## [AUG — verbs]

READ-ONLY pass at tree `af5337a` (working tree of `69e2087` + the seed commit). herdr read ONLY at
`git -C tmp/herdr show d59d060:<path>` / the pinned working tree; pi ONLY at
`git -C tmp/pi-subagents show v0.68.0:<path>`. Second client cross-checked:
`tmp/code_puppy_core_plugins/code_puppy_core_plugins/herdr/`.

**Scope of THIS section.** The `[AUG — inspector]` section above is mostly sound and I do not repeat
it. What follows is (§A) every citation in it that is now STALE or WRONG, (§B) herdr's own protocol
with line numbers and the six places pi's CLI-shaped usage loses something herdr offers, (§C) the
three SETTLE items, (§D) the corrections to its Rust shape / call sites / tests, and (§E) sizing.
Where §A and the section above disagree, **§A wins** — it was re-derived after #146/#147 merged.

---

### §A. Re-verification of `[AUG — inspector]` at `69e2087`

#### A.0 What still HOLDS (spot-checked, do not re-verify)

All thirteen `src/inspectors/` line counts (sum **1 588**); `actions.ts` `trustedDir:34`
`resolveTarget:49` `missionFor:79` `launchFor:90` `handleInspectorAction:120` `:130` `:132-137`
`:138-139` `:141-147`; `inspector-runner.ts` `RunnerOptions:14` `formatInspectorDashboard:30`
`parseArgs:50` `submitInspectorControl:99` `runInspector:120` and the mirror sentence at `:34`;
`client.ts` `normalizeCode:35-41` `:44` `:54-56`/`:84-86` `supportsRawPanes:118-120`
`detectHerdr:122-129`; `herdr/actions.ts` `HerdrInspectorBinding:9` `bindingPath:24` `parse:28-40`
`readHerdrInspectorBindingForTarget:51` `errorText:70-72` `openHerdrInspector:88` `:100` `:103-107`
`:111` `:113` `:122-126` `:131` `statusHerdrInspector:134` `closeHerdrInspector:146` `:150-154`;
`herdr/plugin.ts:13-15`; `ghostty/plugin.ts:13-14`; `ghostty/actions.ts` `GHOSTTY_APPLESCRIPT:7`
`:52` `:61-66` `:69`; `project-panes.ts` `:12 :17 :19 :128 :168-170 :176 :180-182 :192 :225-237
:264 :302 :427-450 :486-508 :529-531 :550 :552-576 :695 :709`; `fleet.ts` `:45 :100 :926 :947-959
:961-965 :963 :1417-1430` and zero `Herdr` hits in 1 448 lines; `subagent-executor.ts`
`:120 :122 :213 :6294 :6296-6301 :6312 :6313 :6319 :6320 :6329`; `extension/index.ts:980-981`
verbatim; `MUTATING_MANAGEMENT_ACTIONS` is exactly **31** entries. cyrup: `authority.rs:43-46`
(delta), `:47-55` (const), `:92-106` (`for_tool_action`), `:108-112`, `:263-268`; `fleet.rs:8`,
`:58-61`, `:1468`, `:1822`, `:1825`; `fleet_overlay.rs:271-274`, `:275-278`; `text.rs:239-366`
(**52** entries), `:332-334`, `:368-375`, `:384-385`; `schema.rs:987-990`, `:1324`;
`subagent_runner_cmd.rs:28-48 :61 :73 :88`; `predispatch.rs:34 :37 :68 :69`;
`spawn_detached.rs:82-85 :86 :203`; `atomic.rs:75`; `fleet_view.rs:244 :1034`;
`spawn/mod.rs:229 :280`; `artifacts.rs:162`; `paths.rs:23 :29 :196`; `permission_arbiter.rs:247`;
`missions/store.rs:460` (the false citation) `:466 :695 :734 :962`; `missions/lifecycle.rs:812`;
`fleet_status.rs:161-179` (no `surface`/`project_pane` field) `:367`;
`routing.rs:1085 :1096 :1105 :1478`.

#### A.1 STALE OR WRONG — corrections are load-bearing

| # | `[AUG — inspector]` says | truth at `69e2087` / `v0.68.0` / `d59d060` |
|---|---|---|
| V1 | "cyrup's list is exactly 52 today; upstream's (`shared/types.ts:2801`) is **58**" | upstream's is **57** (counted programmatically). The 52 → **59** target is still right, but for a different reason: cyrup carries two verbs upstream's `SUBAGENT_ACTIONS` does not (`append-step` at index 21, `inspect` at 22), so 57 − 7 = 50, 50 + 2 = 52, 52 + 7 = 59. Say *that*, not "58". |
| V2 | §6.3's insertion quote `… "refine.rollback", "inspector.open", …, "project.close", "status", …` | that is UPSTREAM's neighbourhood. **cyrup's list is not in upstream's order**: cyrup's `"status"` is at index 13 (`text.rs:270`), long before `mission.*`. In cyrup the seven go between `"refine.rollback"` (`text.rs:344`) and `"watchdog.status"` (`:345`). An exec following the AUG's quote will hunt for a `"status"` that is not there. Same for `schema.rs` (between `:993` and `:994`). |
| V3 | `background/control.rs:1258` `request_async_steer`, `:1092` `request_async_stop`, `:320` `read_status_file`, `:1283` `request_async_steer_with_mode` | **all four drifted +4** when #147 landed: `:1262`, `:1096`, `:324`, `:1287`. |
| V4 | `spawn_detached.rs:214-219` is the `Command` chain | it is `:213-218` — the SEED was right and the AUG's "correction" introduced the error. `:213` is `let mut command = tokio::process::Command::new(&spawn_command.binary);`. |
| V5 | `background/mod.rs:147-150` re-exports the run-id resolver | `:149-152`. `:145-147` is the `run_history` re-export. |
| V6 | `extension/host/native_impl.rs:314` is the `SessionStart` arm | `:315`. `reset_spawn_budget` at `:359` ✔. |
| V7 | D2 is `fleet_overlay.rs:36-38` | the **herdr half** is `:37-38` only. `:34-36` is the steer-delivery-mode half, which STAYS. Deleting `:36-38` truncates a surviving sentence mid-clause. |
| V8 | D4 is `fleet_overlay.rs:618-635` | the test spans `:618-639` (`#[tokio::test]` at `:618`, closing `}` at `:639`). |
| V9 | D9 is `routing.rs:1433-1437` | `:1433-1436`. `:1437` is `` `AuthorityAction::for_tool_action` therefore keeps returning `None` ``, which stays. (The AUG's catch that the arm's own `:47-54` should be `:47-55` is correct.) |
| V10 | D10 is `text.rs:160-165` | the clause to delete is `:162-165` (`:160-161` is the `MUTATING_MANAGEMENT_ACTIONS`-is-deliberately-not-extended sentence, which stays true). |
| V11 | "`text.rs:186-190`'s exhaustiveness claim" | the sentence is one line, `:187`. |
| V12 | "the `text.rs` list assertion (`:653`)" | the `assert_eq!` opens at `:651`; `:653` is inside it. |
| V13 | T-RUN-2: "`subcommands::SUBCOMMANDS` contains neither" | `SUBCOMMANDS` is **private** (`crates/cyrup/src/subcommands.rs:36`, `const`, no `pub`). The testable surface is `pub fn first_subcommand(argv) -> Option<&str>` (`:47-53`). Assert `first_subcommand(&["__subagent-inspector".into()]).is_none()`. |
| V14 | "`crates/cyrup-it/tests/subagents/main.rs`'s **40** `mod` lines" | **42** after #146/#147. |
| V15 | T-OPEN-1's fake scripts `pane split → {"pane_id":"p1"}` | that only passes because of `paneId()`'s fallback rung. herdr's REAL CLI output is the whole envelope, `{"id":"cli:pane:split","result":{"pane":{"pane_id":"w1:p2",…}}}` (`herdr src/cli/runtime.rs:109-111` → `print_method_response`; handler returns `ResponseResult::PaneInfo{pane}`, `herdr src/app/api/panes.rs:129`), and pi's client strips `envelope.result` (`client.ts:97-98`) leaving `{"pane":{…}}`. **The fake MUST emit `{"pane":{"pane_id":…,"tab_id":…,"workspace_id":…,"agent_status":…,"cwd":…}}`**, or the test pins a shape herdr never sends. Same for `pane get`. |
| V16 | T-PROJ-1's idle refusal `"Herdr project pane '{id}' is '{status}', not explicitly idle."` | upstream's literal is `Project pane '{paneId}' is '{agentStatus}', not explicitly idle.` (`project-panes.ts:650`) — no "Herdr" inside. The model sees it wrapped: `Herdr project pane error (PANE_NOT_IDLE): Project pane 'w1:p2' is 'working', not explicitly idle.` |
| V17 | T-DASH-1: "degrading to `Controls: steer <message> \| status` when `allow_stop == false`" | wrong for a `single`-mode run. `inspector-runner.ts:44-45`: `type guidance` is dropped by `allowSteer === false` **or** `!acceptsPlainGuidance`; `stop` only by `allowStop === false`. So `single` + `allow_stop=false` → `Controls: type guidance \| steer <message> \| status`. The four cases the table must cover are (steer,stop,single) × the aggregate case. |
| V18 | T-CTL-2 lists five control refusals | upstream has **nine**, and four are missing: `"Authority policy does not allow steer from this inspector."` (`:86`), `"No running child is available to steer. Open a child-specific inspector for a pending child."` (`:90`), `"Run '{id}' is {state} and cannot be stopped."` (`:106`), `"Lifecycle status for run '{id}' is unavailable."` (`:103`). Plus the two non-error replies `"Status refreshed."` (`:101`) and `"Stop requested for run {id}."` (`:108`). |
| V19 | T-TRUST-1 tests only the containment rung | `trustedDir` (`actions.ts:34-48`) has **two** rungs and the FIRST is a live-job match: any dir whose `realpathSync` equals a `state.asyncJobs`/`state.fleetJobs` entry's `asyncDir` is trusted **regardless of the root** (`:38-44`). Only if that misses does it fall to `existsSync(root) && pathWithin(root, dir) && pathWithin(realpath(root), real)` (`:46`) — **`pathWithin` applied twice, once literal and once realpath'd**. Dropping either rung is a behaviour change: rung 1 dropped → a run under a configured non-default root is refused; rung 2's realpath leg dropped → a symlinked root escapes. |
| V20 | "the missing `status`/`close` methods drive upstream's `… does not support …` messages" | true, but **unreachable through `builtin_inspector_plugins()`**: herdr supplies both (`herdr/plugin.ts:17-18`) and ghostty's `owns` is `() => false` (`ghostty/plugin.ts:14`), so no owner is ever a plugin lacking the method. Port the arms (a third-party plugin could), but do not claim a reachability test for them; the reachable assertion is T-STAT-2's `No inspector plugin owns this binding…`. |
| V21 | `[AUG — inspector]` §4: "`projectPaneEntries` at `fleet-status.ts:351-365` is the **only remaining production reader** of the restored map" | **now false, and so is the SCOPE CORRECTION's reversal of it.** `git grep herdrProjectPanes v0.68.0 -- src/` gives **two** readers: `fleet-status.ts:352` (the roster rows, `surface: "project-pane"` at `:55`, `projectPane` at `:67`, folded in at `:526`, rendered at `:764-765 :781 :846 :865-866`) and `extension/index.ts:864`'s `getProjectPaneCount` closure, which `herdr-status.ts:162-163` folds into the pane label as `" · N panes"`. See §C.4 — this decides what the bridge batch owes and what this batch owes. |
| V22 | seed §0: "the session-start status bridge `index.ts:866` runs" | `registerHerdrStatusBridge` is called at `extension/index.ts:861`. |
| V23 | (not in the AUG at all) cyrup's fleet footer | `fleet.rs:2208` emits `" ↑↓/jk agent · H Herdr · s steer · D stop · x/Ctrl+O tools · r refresh · Esc close · "` and `:3505` asserts it. Upstream's footer at `fleet.ts:1368` reads `… agent · p Prompt Audit · {inspect} Inspect · {steer} steer · …` — **"Inspect", not "Herdr"**. Two more sites this batch must touch, and a third literal at `fleet.rs:3398` asserting the old unavailable string. |
| V24 | (not in the AUG) | `routing.rs:1475` cites `DESTRUCTIVE_MANAGEMENT_ACTIONS` at `text.rs:299-312`; it is at `text.rs:376-390`. An already-false in-tree citation in the arm this batch edits — fix in passing. |
| V25 | (not in the AUG) | `predispatch.rs:11` says "**The three** internal hops" and `:29` "None of **the three** appears in `--help`". A fifth `Internal` variant makes both false. They become "four". `:59` and `:78` say "LAST of the four" → "LAST of the five". |
| V26 | (not in the AUG) | `ProjectPaneManager` has a **fourth** method, `focus` (`project-panes.ts:131`, body `:511-544`), reached by `project.open` when a pane is already open and `params.focus` is set (`:720-726`). It is not optional: without it, `project.open --focus` on an open pane silently does not focus. |
| V27 | the mandate's worked example "herdr v0.9.1 exposes **99** socket methods" | at `d59d060` the `Method` enum has **105** `#[serde(rename=…)]` variants (`tmp/herdr/src/api/schema.rs`). The doc's own table (`docs/versions/0.9.1/…/socket-api.mdx:96-113`) lists 88; the other 17 are undocumented-but-live, and **`pane.focus` is one of them** (§B.2). |

---

### §B. HERDR IS THE AUTHORITY — the protocol, and six things pi's CLI shape loses

#### B.1 What pi actually sends, and what herdr actually does with it

pi drives exactly **four** CLI invocations across both features
(`herdr/actions.ts:85,103-107,111,113,149`; `project-panes.ts:402,577-579,585,587,654`;
`focus.ts:33,40,44`), plus `herdr --version` (`client.ts:123`):

| pi's argv | herdr CLI entry | socket method | what herdr does |
|---|---|---|---|
| `pane split --current --direction right --cwd <p> --focus\|--no-focus` | `src/cli/pane.rs:33` → `:617-628`, parser `:630-740` | `pane.split` (`src/api/schema.rs`, `PaneSplit(PaneSplitParams)`) | `src/app/api/panes.rs:34-135`. `--current` **requires `HERDR_PANE_ID`** (`src/cli/pane.rs:660-667`) — which is exactly why pi's `available()` gates on it. Response: `ResponseResult::PaneInfo{pane}` (`:134`), printed whole by `print_method_response` (`src/cli/runtime.rs:109-111`). |
| `pane run <id> <command>` | `src/cli/pane.rs:43` → `:1039-1053` | **`pane.send_input`** — there is no `pane.run` method | `panes.rs:1835-1859`. **It types the command into the pane's shell and presses Enter** (`text: args[1..].join(" "), keys: ["Enter"]`, `src/cli/pane.rs:1046-1052`). It does NOT spawn a process. Sent with `send_ok_request` (`src/cli.rs:755-767`), which prints **nothing** on success. |
| `pane get <id>` | `src/cli/pane.rs:22` | `pane.get` (`PaneGet(PaneTarget)`) | `panes.rs:159-168`, returns the full `PaneInfo` (`src/api/schema/panes.rs:527-560`). |
| `pane close <id>` | `src/cli/pane.rs:35` | `pane.close` (`PaneClose(PaneTarget)`) | `panes.rs:1861-1867` → `close_pane:1869`. |
| `tab focus <id>` / `workspace focus <id>` | `focus.ts:40,44` | `tab.focus` / `workspace.focus` | the two-hop focus dance. |

**Error path, verified end to end.** `print_response` (`src/cli.rs:745-753`) writes the error
envelope to **stderr** and exits **1**; pi's client then parses stderr (`client.ts:89`), finds
`"error"`, and runs `normalizeCode` (`:35-41`). herdr's actual codes are `pane_not_found`
(`panes.rs:1987-1990`), `pane_split_failed` (`:103`), `pane_send_failed` (`:1830,:1856`),
`pane_clear_failed`, `invalid_key`, `pane_layout_unavailable`. So `NOT_FOUND` is reachable
(`pane_not_found` contains `not_found`) and everything else collapses to `VALIDATION_ERROR`.
**`PANE_GONE` is never produced by herdr** — the only source is pi's own literal at
`herdr/actions.ts:110`. Port `from_wire_lossy` anyway (a third-party/forward code could say
`gone`), but the test that matters is `pane_not_found → NotFound`.

#### B.2 Where pi's CLI-shaped usage LOSES something the socket offers

**`[CYRUP-EXCEEDS-UPSTREAM]` candidates, in descending value. Each one is VALUABLE, not merely
available; two of them delete a pi refusal sentence that is factually false against herdr 0.9.1.**

1. **`pane.focus` exists, and pi's "Herdr cannot refocus an arbitrary raw pane id" is FALSE.**
   `openHerdrInspector:100` tells the user *"Herdr cannot refocus an arbitrary raw pane id; select
   it in the Herdr UI."* herdr has had `#[serde(rename = "pane.focus")] PaneFocus(PaneTarget)` and
   `herdr pane focus <pane_id>` (`src/cli/pane.rs:30` → `:201-211`) all along; the handler
   (`src/app/api/panes.rs:484-500`) focuses the pane **across tabs and workspaces in one call**
   (test `api_pane_focus_focuses_direct_target_across_tabs_and_workspaces`, `panes.rs:4152`) and
   sets `mode = Terminal`. pi's `focus.ts:32-55` instead does `pane get` → `tab focus` **or**
   `workspace focus` — two round-trips that focus the *tab*, leaving the pane unfocused inside it,
   and a `PANE_FOCUS_UNSUPPORTED` refusal (`:50-51`) that is unreachable against 0.9.1 because
   `PaneInfo` always carries `tab_id` and `workspace_id` (`src/api/schema/panes.rs:531`).
   **cyrup: one `pane focus <id>` call.** Keep `HerdrFocusErrorCode::PaneFocusUnsupported` in the
   enum for a pre-0.9 herdr, but stop emitting pi's `:100` sentence — with a real focus available,
   the already-open reply is `Herdr inspector pane {id} is already open for async run {runId}.` plus
   the focus outcome. **This is the one place the port's message text may diverge, and it must be
   marked `[CYRUP-EXCEEDS-UPSTREAM]` with the `panes.rs:484-500` citation as the premise.**
2. **`pane.split` takes `env` and `ratio`; pi passes neither.** `PaneSplitParams`
   (`src/api/schema/panes.rs:27-43`) has `env: HashMap<String,String>` and `ratio: Option<f32>`,
   both exposed on the CLI as `--env KEY=VALUE` (repeatable, `src/cli/pane.rs:711-719`) and
   `--ratio FLOAT` (`:676-688`). **Why it matters:** `pane run` is a shell keystroke injection, so
   every argv token in `launch.displayCommand` is re-parsed by whatever shell the pane happens to
   run — which is the entire reason `session-roots-codec.ts` exists. Passing the inspector's
   session roots as `--env CYRUP_INSPECTOR_SESSION_ROOTS=<b64>` at split time puts them in the
   pane's environment before the shell sees anything, and the runner reads the env when the flag is
   absent. Keep the base64 `--session-roots` flag as the primary (it is what `inspector.command`
   must print, and it is what a user pasting the command by hand needs), and add the env as a
   belt-and-braces second source. `--ratio` gives the inspector a sane default width instead of 50 %.
3. **`pane.wait_for_output` turns the blind `pane run` into a verified launch.**
   `pane.wait_for_output` (method table, `socket-api.mdx:106`; CLI
   `herdr pane wait-output <pane_id> (--match TEXT | --regex PATTERN) [--source visible|recent|recent-unwrapped] [--lines N] [--timeout MS] [--raw]`,
   `src/cli/pane.rs:1054-1130`, help at `:1689`). Today `pane run` returning ok means only "keys
   were delivered to a PTY". If the pane's shell was mid-command, or the binary is missing, pi
   writes a binding pointing at a pane that never started the inspector — and `inspector.status`
   then reports it open forever. **cyrup: after `pane run`, `pane wait-output <id> --match
   "cyrup-inspector for " --timeout 5000`** (the dashboard's own header line,
   `inspector-runner.ts:33`), and on timeout run upstream's existing failure path —
   `pane close <id>`, no binding (`herdr/actions.ts:112-115`). This converts T-OPEN-2 from "a
   scripted `pane run` failure" into a real guarantee.
4. **`pane.report_agent` / `pane.report_metadata` make the inspector visible in herdr's own UI.**
   `PaneReportAgentParams { pane_id, source, agent, state: PaneAgentState, message, seq, … }`
   (`src/api/schema/panes.rs:448-461`); `AgentStatus` is `Idle|Working|Blocked|Done|Unknown`
   (`src/api/schema/common.rs:160-166`); the doc's own contract is at `socket-api.mdx:701-733`.
   `state` "affects waits, notifications, and rollups" (`:715-716`). The inspector pane already
   knows the run's lifecycle state every `refreshMs`; one `pane report-agent <id> --source
   cyrup:inspector --agent <runId> --state working|idle|blocked` per transition makes the run show
   up in herdr's sidebar, its rollups and `agent.wait`. pi never calls it from `inspectors/` at all
   (`herdr-status.ts` reports the *parent* pane, not the inspector pane). **This is the single
   largest capability gap and it costs one call per state change.** `pane report-metadata --token
   summary=<…>` additionally populates the exact key pi's own `paneSummary` probes for (§B.3).
5. **`pane.read` is a real transcript source pi never uses.** `PaneRead(PaneReadParams)`
   (`panes.rs:1505-1546`) returns `{text, truncated, source, format, revision}` with
   `source: visible|recent|recent-unwrapped`. `project.status` today reports only "is open"; with
   `pane read` it can report what the project pane is actually doing. Optional for v1, but it is the
   answer to "why is this pane `working`".
6. **`pane.process_info` is a stronger ownership proof than `cwd`.** `projectPaneOwnership`
   (`project-panes.ts:420-425`) compares `runtime.cwd` to the project root, and `cwd` is documented
   as "the pane/workspace cwd used for labels … and restored session state", **not** the running
   process's cwd (`socket-api.mdx:750-752`). `PaneProcessInfo { shell_pid, foreground_process_group_id, tty, foreground_processes[{pid,name,argv0}] }`
   (`src/api/schema/panes.rs:570-588`, handler `panes.rs:519-566`) and the `foreground_cwd` field on
   `PaneInfo` (`:536`) are the real ones. pi already reads `foreground_cwd` into
   `ProjectPaneRuntime.foregroundCwd` (`project-panes.ts:346`) **and then never consults it in
   `projectPaneOwnership`**. cyrup: `Verified` if `cwd` OR `foreground_cwd` canonicalises to the
   root; `Mismatch` only when both disagree. Strictly fewer false `mismatch` refusals, identical
   `verified` set.

#### B.3 Where pi MIS-DESCRIBES herdr (report as `anchorsWrong`, do not port the mistake)

- **`paneSummary` (`project-panes.ts:323-331`) is dead against herdr 0.9.1.** It probes
  `summary`, `state_text`, `stateText`, `token_summary`, `tokens.summary`, `metadata.summary`,
  `metadata.tokens.summary`. `PaneInfo` (`src/api/schema/panes.rs:527-560`) has **none** of
  `summary`, `state_text`, `token_summary` or a `metadata` object. Only `tokens.summary` can ever
  hit, and only if something called `pane.report_metadata --token summary=…`. The real presentation
  fields are `state_labels: HashMap<String,String>` (`:551`), `display_agent` (`:548`), `label`
  (`:538`), `title` (`:542`) and `terminal_title_stripped` (`:546`). **cyrup: keep the seven probe
  rungs for cross-version tolerance, and ADD `state_labels[agent_status]` → `display_agent` →
  `label` ahead of them.** Without this, `FleetStatusEntry.project_pane.summary` is always `None` and
  `fleet-status.ts:866`'s `summary ?? "—"` renders a dash forever.
- **`focus.ts`'s `PANE_FOCUS_UNSUPPORTED` premise** — see B.2.1.
- **`openHerdrInspector:100`'s refusal sentence** — see B.2.1. This is the one upstream string this
  batch is permitted to change, because its premise is false.
- **The version floor is pi's, not herdr's.** `supportsRawPanes` demands ≥ 0.7.5
  (`client.ts:118-120`). Nothing in herdr calls a pane "raw"; `pane split` + `pane run` + `pane get`
  + `pane close` are all present well before that and unchanged at 0.9.1. Keep the floor (it is
  upstream's stated contract and its message is model-visible) but do not repeat "raw inspector
  panes" as if it were a herdr concept — say in the doc that it is pi's own floor.

---

### §C. The three SETTLE items

#### C.1 SETTLED — the `__subagent-inspector` subcommand seam

The `[AUG — inspector]` §1 design is **correct and I adopt it**, with V4/V25 applied and three
additions.

The precedent, re-verified: `subagent_runner_cmd.rs:61` (`pub const SUBCOMMAND`), `:73`
(`is_selected` = exact `argv[1]`), `:88` (`parse_config_flag`, the hand-parser precedent
`parse_args` needs, with its stated rationale at `:83-87`); `predispatch.rs:34` (`pub enum
Internal`), `:68-85` (`classify_internal`, first arm `:69`), module doc `:11-17`;
`main.rs:266` (the `match`), `:267-270` (the `SubagentRunner` arm), `:276-280` (the `PR_SET_NAME`
truncation note); `spawn_detached.rs:82-85` (the two-independent-literals convention), `:86`,
`:213-218`. **Nothing here spawns the inspector** — `openHerdrInspector` hands
`launch.displayCommand` to `herdr pane run` (`herdr/actions.ts:111`) and ghostty to AppleScript
(`ghostty/actions.ts:61`); the terminal host starts the process. So `spawn_detached` is NOT reused,
only the *selector* precedent.

Seven edits, in this order:

1. `crates/cyrup-ext-subagents/src/inspectors/runner.rs` — `pub const INSPECTOR_SUBCOMMAND: &str = "__subagent-inspector";` plus `RunnerOptions`, `parse_args`, `format_inspector_dashboard`, `submit_inspector_control`, `run_inspector`. **`run_inspector` takes injected argv + an injected `impl AsyncBufRead` stdin + `impl Write` stdout**, so T-DASH/T-CTL/T-ARGV run with no TTY.
2. `crates/cyrup/src/subagent_inspector_cmd.rs` — the second independent literal, `is_selected`, `dispatch(&raw).await -> i32`.
3. `crates/cyrup/src/predispatch.rs` — `Internal::SubagentInspector`, classified **after** `SubagentRunner` and **before** `AcpTerminalLogin`; **and the module doc at `:11` and `:29` retyped from "three" to "four", and `:59`/`:78`'s "LAST of the four" to "of the five"** (V25).
4. `crates/cyrup/src/main.rs` — the arm, `set_process_name("cyrup-inspector")` (15 bytes + NUL = 16, survives `PR_SET_NAME` intact; say so as `:276-280` does for the truncated one).
5. `crates/cyrup/src/lib.rs` — `pub mod subagent_inspector_cmd;` between `subagent_config` (`:39`) and `subagent_runner_cmd` (`:40`).
6. `inspectors/actions.rs::launch_for` — `executable = resolve_spawn_command().binary`, `argv = base_args ++ [INSPECTOR_SUBCOMMAND, "--async-dir", …]` (`spawn/mod.rs:229,:280`).
7. `subagent_runner_cmd.rs:28-48` — the `(SEAM-109)` delta's premise is still TRUE and is now true of **two** tokens. **EXTEND with one sentence naming `__subagent-inspector`; do not copy the block.** Its own citations are pinned to `pi-subagents HEAD 30c6080` / `v0.83.0` — leave them, they are labelled; add none in that style.

**The failure mode this seam has, and the only test that catches it:** classification and dispatch
live in different crates. A `predispatch` arm without a `main.rs` arm falls through to clap, which
rejects `--async-dir` with a usage error and exit 2 — a silent half-wiring. **T-RUN-1 (IT) must be
written before the implementation.**

#### C.2 SETTLED — NO herdr and NO ghostty (this container, and every Linux CI box)

Upstream's degradation is already quoted correctly in `[AUG — inspector]` §2 and I confirm every
sentence. Three points it does not make, all load-bearing:

- **`available()` on both plugins is a pure env/platform read — no probe, no spawn.** herdr:
  `env.HERDR_ENV === "1" && Boolean(env.HERDR_PANE_ID?.trim())` (`herdr/plugin.ts:13-14`); ghostty:
  `platform === "darwin" && env.TERM_PROGRAM?.toLowerCase() === "ghostty"` (`ghostty/plugin.ts:13`).
  With neither, `inspector.open` refuses in microseconds and **cannot hang** — there is no binary to
  time out on. This is why `HerdrClient` must never be constructed eagerly in `builtin_inspector_plugins()`.
- **`launchFor` is called INSIDE the loop body (`actions.ts:134`), not before it.** With zero
  available plugins the mission lookup never runs and nothing touches disk. T-OPEN-0's
  "`<asyncDir>/inspectors/` does not exist afterwards" is the assertion that pins it; add
  "and `missions/` was not read" if the mission store is instrumentable.
- **`inspector.command` returns before `deps.plugins` is even read (`actions.ts:130` vs `:131`).**
  In the not-installed state it is the ONLY verb of the seven that does real work, and it is not an
  error. That is the feature behaving correctly.

Complete not-installed behaviour table — **this is what a reviewer on this container will see**:

| verb | with no herdr, no ghostty | `is_error` |
|---|---|---|
| `inspector.command` | the full launch string incl. base64 `--session-roots` | false |
| `inspector.open` | `No inspector plugin is available. Start a supported inspector host, or use inspector.command for a standalone command.` | **true** |
| `inspector.status` | `No inspector plugin owns this binding for async run {runId}.` | **false** |
| `inspector.close` | same sentence | **false** |
| `project.status` | `No Herdr project pane binding exists for {projectRoot}.` (`project-panes.ts:695`) | **false** |
| `project.close` | same sentence (`:709`) — **but `removeHerdrProjectPaneRoot(ownerRoot, root)` runs FIRST (`:706`), before the absent check.** A faithful port mutates the root index even on the absent path. | **false** |
| `project.open` | `Herdr project pane error (HERDR_UNAVAILABLE): Herdr is not installed or is not on PATH. Install Herdr 0.7.5+ or set HERDR_BIN.` | **true** |

`inspector.status`/`close`/`project.status`/`project.close` being **non-errors** is the single
easiest thing to get wrong: the natural `Result` shape flips `is_error`. T-STAT-2 and T-PROJ-2 exist
for exactly that and must assert `is_error == false` explicitly, not merely check the text.

**How it is tested without herdr:** every one of those rows is a plain unit test with an env map
carrying neither `HERDR_ENV` nor `TERM_PROGRAM` and a `HerdrClient` fake that **records zero calls**
— asserting the recorder is empty is what proves no probe happened. The three `project.*` rows and
all four `inspector.*` rows run green on this container today. Only T-RUN-1/T-RESTORE-1 need the
real `cyrup` binary, and neither needs herdr.

#### C.3 SETTLED — the fleet `H` key, the authority additions, and the exact deltas

`[AUG — inspector]` §3's D1-D12 are correct in substance. Apply V7/V8/V9/V10/V11 to their spans, and
add three sites it missed:

**Deleted, with the corrected span:**

| # | site | span |
|---|---|---|
| D1 | `tui/fleet.rs:58-61` — delta 2 in full; renumber 3→2, 4→3; retype the module-doc line `:8` to "inspector" | `:58-61`, `:8` |
| D2 | `tui/fleet_overlay.rs` — **`:37-38` only** (the herdr half). `:34-36` stays; retype "Both are inherited" → "One is inherited" | `:37-38` |
| D3 | `tui/fleet_overlay.rs:271-274` — the unreachability note; `:275-278` becomes a real `inspector_open(cwd, target, /*focus*/ true)` keeping the fallback text | `:271-274` |
| D4 | `tui/fleet_overlay.rs:618-639` — `an_inspect_action_answers_upstreams_herdr_failure_text`, replaced by T-FLEET-1's driver | `:618-639` |
| D5 | `extension/host/slash.rs:83-84` (the comment) and `:86` (`false` → `true`) | `:83-84`, `:86` |
| D6 | `registration/authority.rs:43-46` | ✔ |
| D7 | `registration/authority.rs:108-112` → the straight port note: four `confirm` (`discardWorktree`, `destructiveCleanup`, `spawnBudgetGrant`, `projectOpen`), four `auto` (`scheduleCreate`, `stopRun`, `steerRun`, `inspectorOpen`), per `policy/authority.ts:16-25` | ✔ |
| D8 | `registration/authority.rs:263-268` — the parenthetical and "six"/"three and three" | ✔ |
| D9 | `registration/authority.rs` → `routing.rs:1433-1436`; `:1437` survives | **`:1433-1436`** |
| D10 | `extension/tool/text.rs:162-165` | **`:162-165`** |
| D11 | `extension/tool/text.rs:332-334` | ✔ |
| D12 | `extension/tool/schema.rs:987-990` | ✔ |
| **D13** | `tui/fleet.rs:2208` — `H Herdr` → `H Inspect` in the footer, and the `:3505` assertion with it. Upstream is `fleet.ts:1368`'s `{inspect} Inspect`. | new |
| **D14** | `tui/fleet.rs:3398` — the test literal `"Herdr inspector controls are unavailable in this context."` → upstream's `"Inspector controls are unavailable in this context."` (`fleet.ts:963`) | new |
| **D15** | `extension/tool/routing.rs:1475` — `DESTRUCTIVE_MANAGEMENT_ACTIONS` is at `text.rs:376-390`, not `:299-312`. Already false; this batch edits the arm, so it owns the fix. | new |

**Also already false, fix in passing:** `missions/store.rs:460`'s
`inspectors/herdr/inspector-runner.ts:24` (the file is `src/inspectors/inspector-runner.ts`, import
`:5`, call `:27`; `git show v0.68.0:src/inspectors/herdr/inspector-runner.ts` → `does not exist`) —
this batch gives that reference its Rust counterpart, so it owns the correction.

**The `H` key itself.** `fleet.ts:45` binds `inspect: ["return", "H"]` — **Enter as well as H**.
cyrup's `FleetKey::Enter` is bound only inside the steer draft (`fleet.rs:1693`) and the confirm
prompt (`:1747`); at top level it is free. Add the top-level `FleetKey::Enter` arm beside
`FleetKey::Char('H')` (`:1820`), both calling one `fn inspect_selected(&mut self)`.
`selectedInspectAction` (`fleet.ts:947-959`) is **not** `selectedAsyncAction` (`:926-933`) and
carries four refusals cyrup has no counterpart for: `"No child is selected."` (`:949`),
`"External jobs are display-only and have no inspector controls."` (`:950`),
`` `Selected child is ${item.state}; controls require a running or queued async child.` `` (`:952`),
`"The parent workflow is no longer available for inspection."` (`:957`) — and on the
`foreground-active` path it returns the **parent's** `asyncId` (`:958`), not the child's `runId`.
Add `fn selected_inspect_action(&self) -> Result<FleetActionTarget, String>` beside
`selected_async_action` (`fleet.rs:1650`).
Upstream passes `focus: true` (`fleet.ts:1420`) and its fallback text is
`` `Failed to open inspector for async run ${runId}.` `` — cyrup's `fleet_overlay.rs:275-277`
already emits exactly that string, so D3 preserves it verbatim.

**Authority.** `AUTHORITY_ACTIONS` gains `"inspectorOpen"`, `"projectOpen"` in upstream's order
(`policy/authority.ts:1-10`); `AuthorityAction` gains the two variants; `as_str`, `default_decision`
(`inspectorOpen → Auto`, `projectOpen → Confirm`) and `for_tool_action` (`"inspector.open"`,
`"project.open"` — **and nothing else; the other five verbs must keep returning `None`**) all
extend. The `text.rs:187` exhaustiveness claim must gain a bullet naming where the four mutators are
refused in child-safe mode: **inline in each new `route_action` arm**, exactly as
`subagent-executor.ts:6313` and `:6320` do, and **NOT** by extending
`discovery::management::MUTATING_MANAGEMENT_ACTIONS` — `text.rs:158-161` says why and that half
stays true.

#### C.4 The `herdr-status.ts` SCOPE CORRECTION — what it actually changes for this batch

The correction reverses the exclusion; I accept that. But its stated *reason* is wrong and the
design consequence is the opposite of what it says (V21).

`herdr-status.ts` **never reads `state.herdrProjectPanes`.** It reads `HERDR_PANE_ID`/`HERDR_ENV`
(`:130-131`) and one injected closure, `options.getProjectPaneCount`, which it folds into the pane
label as `" · N panes"` (`:162-163`). The closure is supplied by the extension
(`extension/index.ts:864`: `[...(state.herdrProjectPanes?.values() ?? [])].filter(p => p.state === "open").length`).

So:
- The bridge consumes a **count**, not the snapshots. That is enough to make
  `restore_herdr_project_pane_snapshots` non-dead the moment the bridge lands.
- It is **not** enough to make `FleetStatusEntry`'s new `surface` / `project_pane` fields live.
  Those have exactly one reader, `fleet-status.ts:351-365` → `:526` → `:764-765,:781,:846,:865-866`,
  and it is in **this** batch (§D.3 item 10).
- **Therefore the `[AUG — inspector]` §4 workaround stands, restated:** this batch still writes
  `project_pane_entries` and the two `FleetStatusEntry` fields, and T-RESTORE-1 is still the test
  that forces them. What the correction changes is only the *justification paragraph*: cyrup no
  longer needs to argue "without this the restore writes state nothing reads", because the bridge
  batch is adding a second reader. **The seam this batch owes the bridge batch** is one
  `pub fn open_project_pane_count(state: &FleetState) -> usize` beside
  `restore_herdr_project_pane_snapshots`, matching `index.ts:864`'s filter exactly. Name it in the
  doc and point at the bridge spec; do not implement the bridge here.
- Watch the name collision: `tui/fleet_status.rs` **already** has an `inspector_open` field
  (`:582`, `:657`) — that is pi's fleet-OVERLAY-is-open flag, nothing to do with the herdr
  inspector. Do not reuse or shadow it.

---

### §D. Corrections to the Rust shape, call sites and tests

#### D.1 Shape (additive to `[AUG — inspector]` §5, which is otherwise adopted)

- `InspectorPlugin::available` must take `&InspectorContext` and be `async`, but **must not
  construct a client**; `SpawnedHerdrClient` resolves `HERDR_BIN` lazily at first `run` (`client.ts:44`).
- `HerdrErrorCode::from_wire_lossy` ports `normalizeCode:35-41` as a lowercase-contains ladder in
  upstream's order — `timeout`/`timed_out` → `Timeout`; `gone` → `PaneGone`;
  `not_found`/`not-found`/exact `no_such_pane` → `NotFound`; else `ValidationError`.
- `parse_last_json` (`client.ts:25-33`) is required, not optional: it tries the whole trimmed blob,
  then **each line from last to first**. herdr's CLI prints one JSON line, but a warning line ahead
  of it would break a naive whole-buffer parse.
- The `textOk` flag (`client.ts:99`) is what makes `--version` return the raw string. Model it as
  `HerdrRunOptions { timeout: Duration, text_ok: bool }` with `Duration::from_secs(15)` default and
  3 s for `--version` (`client.ts:76,:123`).
- Bindings: **no `deny_unknown_fields`**, `#[serde(flatten)] extra` to preserve pi-written keys,
  `skip_serializing_if = "Option::is_none"` on every optional (a `"childIndex": null` fails pi's own
  `parse` at `herdr/actions.ts:33`, because `null !== undefined`). Confirmed against `parse:28-40`
  and `parseBinding:225-237`, which validate required keys only.
- `ProjectPaneManager` is one struct with a `ToolCompatibility` flag (`project-panes.ts:135-138`) and
  **four** methods — `open`, `status`, `focus`, `close` (V26).
- `Platform` for `format_shell_command` is injectable; the Unix branch's nushell workaround
  (`shell-command.ts:13-14`) is `sh -c 'exec "$0" "$@"' '<exe>'` only when
  `!isBareExecutable(exe)` i.e. `^[\w./@:-]+$` fails.

#### D.2 The runner's steer targeting — a real gap `[AUG — inspector]` does not name

`queueInspectorSteer` (`inspector-runner.ts:88-95`) computes
`runningIndexes` from `status.steps`, sets `targetIndex = options.index ?? (mode === "single" ? 0 : undefined)`,
and when that is `undefined` passes **`targetIndexes: runningIndexes`** — a PLURAL field.
cyrup's `SteerRequest` has only `target_index: Option<usize>` (`background/control.rs:1185`), whose
doc already states `None` means "every currently running child, which is what the runner fans it
out to". So the plural collapses to `None` — **but the semantics differ**: upstream snapshots the
running set at request time, cyrup resolves it at drain time. cyrup's is the better answer (no stale
snapshot) and is a `[CYRUP-DELTA: resolution time, not the wire shape]`. **What must NOT be lost:**
upstream still refuses when `targetIndex === undefined && runningIndexes.length === 0` with
`"No running child is available to steer. Open a child-specific inspector for a pending child."`
(`:90`). `None` would otherwise succeed silently into an empty fanout. Reproduce the refusal by
computing `running_indexes` in the runner purely for the guard.

#### D.3 Production call sites — deltas to `[AUG — inspector]` §6

Its eleven sites are right. Apply V2 (the `text.rs`/`schema.rs` insertion point is between
`refine.rollback` and `watchdog.status`, **not** before `status`), V23/D13-D14 (three more
`fleet.rs` literals), D15 (`routing.rs:1475`), V25 (`predispatch.rs` doc counts), and add:
- both new `route_action` arms must set the session id first, as `subagent-executor.ts:6316` and
  `:6323` do (`deps.state.currentSessionId = resolveCurrentSessionId(...)`);
- `routing.rs` arm ORDER mirrors upstream: **project panes (`:6312`) before inspector (`:6319`)**,
  with the authority consult above both (`:6294-6311`) and the child-safe refusal **first inside it**
  (`:6296-6301`) — port that comment verbatim;
- `extension/host/slash.rs`'s `false` at `:86` is the argument the `fleet_overlay` reads as
  `has_inspect`; flipping it is what makes `H` reachable at all.

#### D.4 Tests — deltas to `[AUG — inspector]` §7

Adopt all 28 with these corrections and seven additions.

Corrections: **T-OPEN-1/T-OPEN-3/T-STAT-1/T-PROJ-\*** fakes must speak herdr's real envelope
(V15). **T-PROJ-1**'s idle sentence is V16's. **T-DASH-1**'s controls table is V17's four cases.
**T-CTL-2** gains V18's four refusals and two non-error replies. **T-TRUST-1** gains V19's live-job
rung and the double `path_within`. **T-RUN-2** asserts through `first_subcommand` (V13). Drop the
claim that the "does not support status/close" arms are reachable (V20).

Additions:

- **T-TGT-1 `resolve_target_refuses_with_each_of_upstreams_seven_sentences`** — table over
  `actions.ts:55,57,58,61,64,65,73,74`. *Gutted* → a single generic "run not found" passes a shallow
  test and fails all eight rows; these are the messages a model actually reads when it mistypes an id.
- **T-CTL-3 `an_aggregate_steer_with_no_running_child_is_refused`** — §D.2's guard.
  *Gutted* → dropping it turns a no-op fanout into a silent success and the user's guidance vanishes.
- **T-FOCUS-1 `focus_uses_one_pane_focus_call`** — assert the recorded argv is exactly
  `["pane","focus","w1:p2"]` and that **no** `tab focus`/`workspace focus` was issued. *Gutted* →
  reverting to pi's two-hop is invisible to every other test; only this one sees it, and it is the
  `[CYRUP-EXCEEDS-UPSTREAM]` premise.
- **T-OPEN-4 `a_pane_that_never_starts_the_inspector_is_closed_and_leaves_no_binding`** — §B.2.3:
  `pane run` ok, `pane wait-output` times out → `["pane","close","p1"]`, no binding. *Gutted* →
  dropping the wait leaves a binding pointing at a pane running nothing, which `inspector.status`
  then reports open forever.
- **T-AGENT-1 `the_inspector_pane_reports_its_run_state_to_herdr`** — after open, assert a
  `["pane","report-agent",<id>,"--source","cyrup:inspector","--agent",<runId>,"--state","working"]`
  call, and `--state idle` once the run settles. *Gutted* → the run disappears from herdr's sidebar
  and rollups; nothing else in the suite notices.
- **T-SUM-1 `a_project_pane_summary_comes_from_state_labels`** — a `pane get` returning
  `state_labels: {"working":"refactoring auth"}` and `agent_status: "working"` yields
  `summary == "refactoring auth"`. *Gutted* → reverting to pi's seven dead probes makes every
  project-pane roster row render `—` (`fleet-status.ts:866`) and this is the only test that sees it.
- **T-PROJ-6 `project_close_with_no_binding_still_prunes_the_root_index`** — `:706` runs before
  `:709`. *Gutted* → a root index that never shrinks, so session-start restore keeps re-reading
  dead roots forever.

#### D.5 Files touched — additions to `[AUG — inspector]` §8

Add to **Edited**: `crates/cyrup-ext-subagents/src/extension/tool/routing.rs` (D15 also),
`crates/cyrup/src/predispatch.rs` (V25 doc counts, beyond the new variant),
`crates/cyrup/src/subagent_runner_cmd.rs` (the SEAM-109 extension),
`crates/cyrup-ext-subagents/src/tui/fleet.rs` (`:2208`, `:3398`, `:3505` beyond the handler).
Everything else in §8 stands; `crates/cyrup-it/tests/subagents/main.rs` now carries 42 `mod` lines,
not 40.

---

### §E. Sizing, and the order to land it

`[AUG — inspector]` §9's five-step decomposition is right and I adopt it unchanged in shape. The
honest totals move: ~1 588 upstream lines → **~2 300-2 600 Rust** with the enums, the two traits,
the two fakes and ~35 tests; plus a five-file cross-crate seam; plus **15** delta deletions (D1-D15,
up from 12) and three already-false in-tree citations. This is the largest remaining batch in the
set and it must be SEQUENCED, not narrowed.

1. **The plugin-free core — small, and it ships a working verb on a box with no herdr.**
   `types.rs`, `shell_command.rs`, `session_roots_codec.rs`, `actions.rs`'s
   `resolve_target`/`trusted_dir`/`mission_for`/`launch_for`, `InspectorAction`, the `route_action`
   arm, the `SUBAGENT_ACTIONS`/schema/tool-reference churn at V2's insertion point, the authority
   additions, D6-D12 + D15 deleted. **T-CMD-1/2, T-OPEN-0, T-TRUST-1, T-TGT-1, T-AUTH-1/2,
   T-CHILD-1/2, T-DOC-1 all green, on this container, with nothing installed.** ~650 Rust lines.
   This is the step that turns the batch from all-or-nothing into already-useful.
2. **The runner + the cross-crate seam — medium, riskiest.** `runner.rs`, the `steering_receipt`
   port (~15 lines; `steering.ts:41-46` + `:22-24`, and `redact_secret_values` already exists at
   `permission_arbiter.rs:247` while `preview_display_text` does not), the five `crates/cyrup`
   edits, §D.2's steer-targeting guard, T-DASH-1, T-CTL-1/2/3, T-ARGV-1, T-RUN-1/2. **Write T-RUN-1
   before the implementation** — a half-wired seam fails silently into clap and nothing else catches it.
3. **herdr — large, mechanical.** `client.rs` + `focus.rs` + `actions.rs` + `plugin.rs`
   (~360 upstream lines → ~750 Rust), the real-envelope fake, T-OPEN-1/2/3/4, T-STAT-1/2/3,
   T-FOCUS-1, T-AGENT-1, T-PROJ-3/4. B.2.1 (`pane focus`), B.2.3 (`wait-output`) and B.2.4
   (`report-agent`) land **here**, each with its `[CYRUP-EXCEEDS-UPSTREAM]` premise and its citation.
4. **project panes — large, and the one with design left.** 730 upstream lines, both compatibility
   modes as one manager with a `ToolCompatibility` flag (`:135-138`), all four manager methods
   including `focus` (V26), B.2.6's ownership widening, B.3's `state_labels` summary, the
   `FleetState`/`FleetStatusEntry` extension and `open_project_pane_count` for the bridge batch,
   T-PROJ-1/2/5/6, T-SUM-1, T-RESTORE-1.
5. **The tail — small but fiddly.** ghostty (91 lines, T-GHOST-1/2/3), the `H`/`Enter` rewiring with
   D1-D5 + D13-D14 deleted (T-FLEET-1/2/3), `missions/store.rs:460`, `predispatch.rs`'s doc counts,
   the session-start restore hook.

Steps 1-2 touch `crates/cyrup` and the shared action lists; 3-5 are confined to
`cyrup-ext-subagents`. **Do not land step 1 and call the batch done**: a verb advertised without a
dispatch arm violates this crate's own advertise-vs-dispatch invariant (`extension/tool/text.rs:122`).

### §F. Blockers

**None.** #146 and #147 have merged (`children.list` at index 3, `debug.run` at 14 of the 52-entry
list; `route_action`'s `"children.list"` arm at `routing.rs:1105`). Every input exists today with
the signatures in §A.0 — including the four whose line numbers drifted (V3). `steering_receipt`,
`preview_display_text` and `selected_inspect_action` do not exist, but they are ~40 lines of this
batch's own work, not a missing dependency. `herdr` and `ghostty` are not installed in this
container and **that is not a blocker either**: §C.2's table is the whole not-installed contract and
every row of it is a unit test that runs here.

---

## [WRITE — verbs-core]

WRITE pass, concurrent with four siblings. Files written: ONLY
`crates/cyrup-ext-subagents/src/inspectors/{actions,shell_command,session_roots_codec}.rs`.
Nothing else was touched — not `types.rs`, not `plugins.rs`, not `mod.rs`, not any shared
registration file. Upstream read exclusively at `git -C tmp/pi-subagents show v0.68.0:<path>`;
herdr at `git -C tmp/herdr show d59d060:<path>`.

### What now works with NOTHING installed

`inspector.command` returns the full launch string — `<cyrup binary> __subagent-inspector
--async-dir <dir> --run-id <id> --allow-steer <bool> --allow-stop <bool> --session-roots <base64>
[--index <n>] [--mission-path <p>]`, platform-quoted — and it is **not** an error. It returns at
the port of `actions.ts:130`, before `deps.plugins` is read at `:131`. `inspector.open` with no
backend answers `No inspector plugin is available. Start a supported inspector host, or use
inspector.command for a standalone command.` as an `Err(ToolError)` and writes nothing —
`<async_dir>/inspectors/` is not created, and the mission lookup does not run, because
`launch_for` is called INSIDE the plugin loop exactly as `:134` does. `inspector.status` and
`inspector.close` with no owner answer `No inspector plugin owns this binding for async run
{runId}.` as `Ok(..)` — `:139` passes no second argument, and the port pins that with an explicit
non-error assertion rather than a text check.

### The three files

| file | what |
|---|---|
| `session_roots_codec.rs` | `encode_session_roots`/`decode_session_roots` + `SessionRootsDecodeError`. The PowerShell rationale (`session-roots-codec.ts:1-12`) is ported verbatim as the module doc — it is the reason the encoding exists. Standard padded base64 of the UTF-8 JSON array, byte-identical to `Buffer.from(JSON.stringify(roots)).toString("base64")`, pinned by an exact-literal test (`["/a","/b"]` → `WyIvYSIsIi9iIl0=`) so a pi-written launch string still decodes. |
| `shell_command.rs` | `format_shell_command(exe, args, Platform)` + `host_platform()`. Both branches: PowerShell's `&` call operator with `\"`-escaped double quotes, and POSIX `'…'` with the close/escape/reopen dance, including the nushell workaround `sh -c 'exec "$0" "$@"' '<exe>'` for an executable that fails `isBareExecutable` (`^[\w./@:-]+$`, i.e. ASCII alnum + `_ . / @ : -`, non-empty). `Platform` is a PARAMETER, so the Win32 branch is exercised on Linux CI. |
| `actions.rs` | `trusted_dir`, `resolve_target`, `mission_for`, `launch_for` (+ `launch_for_with_mission`), `handle_inspector_action`, `InspectorDispatcherDeps`, `InspectorRequest`, `LiveInspectorJob`, `ResolvedTarget`, `ResolvedMission`, `NO_INSPECTOR_PLUGIN_AVAILABLE`. |

### Fidelity points the exec/verify pass must not "simplify"

* **`trusted_dir` has TWO rungs** (`actions.ts:34-48`), both ported. Rung 1 is the live-job realpath
  match, which trusts a directory **regardless of the configured root** — drop it and a run under a
  non-default async root becomes uninspectable. Rung 2 applies `path_within` **twice**, once
  literal and once realpath'd — drop the realpath leg and a symlink planted inside the root
  escapes. The symlink/non-directory refusal at `:36` is ahead of both, and every filesystem error
  anywhere is `false`, never a throw.
* **`resolve_target` re-reads `status.json`** (`:72`) even on the `dir` branch that already read
  one. Ported as written; the second read is the snapshot the range check and the launch cwd come
  from.
* **`params.index` is `i64` on the request**, narrowed to `usize` only after the range check, so
  `Index -1 is out of range.` stays reachable (`:74`). A `usize` at the edge would lose it.
* **`mission_for`'s two rungs share ONE catch** (`:86-88`): a throw from `readMissionBinding` or
  from `missionRecordPath` returns `undefined` WITHOUT falling through to the store scan. An
  `if let Ok(..)` that dropped into the scan on error would let a corrupt binding silently resolve
  to an unrelated mission.
* **`inspector.open` consults `available()` and NOT `owns()`** (`:133-134`) — confirmed against
  `[AUG — inspector]` §0's correction. Gating on `owns()` would make the first open of any run
  permanently impossible.
* **`--allow-steer`/`--allow-stop` are `resolve_authority_decision(SteerRun|StopRun, policy) ==
  Auto`** (`:95-96`), stringified as `"true"`/`"false"` — not constants.
* `found.kind !== "async"` (`:65`'s first disjunct) has **no cyrup counterpart**: this crate's
  resolver is the async slice only (`background/run_id_resolver.rs`'s module doc), so a foreground
  id simply does not resolve and takes the `:64` arm. The second disjunct (resolved, no `asyncDir`)
  IS reproduced.

### The one structural delta (already recorded, extended)

`launch_for` builds `exe = resolve_spawn_command().binary` and puts
`runner::INSPECTOR_SUBCOMMAND` in the slot upstream's `inspector-runner.mjs` occupies. That is
`subagent_runner_cmd.rs:28-48`'s `(SEAM-109)` premise applied to its **second** token, per
`[AUG — verbs]` §C.1 item 6. **`actions.rs` references
`crate::inspectors::runner::INSPECTOR_SUBCOMMAND`; the runner writer owns defining it.** A
`cargo check` at the end of this pass reported exactly one error in these three files — that
missing constant — and nothing else.

### Contract gaps reported (orchestrator's call, not worked around locally)

1. `InspectorContext` has no `env`. herdr's `available()` is `HERDR_ENV=1 && HERDR_PANE_ID` and
   ghostty's is `TERM_PROGRAM=="ghostty"` (`herdr/plugin.ts:13-14`, `ghostty/plugin.ts:13`), both
   read off `context.env` upstream. Without the field a backend must read the real process
   environment, which makes `[AUG — verbs]` §C.2's "an env map carrying neither" untestable.
2. `InspectorContext` has no run state. `statusHerdrInspector` prints `Run state: {state}`
   (`herdr/actions.ts:143`) and repeats it on the failure branch (`:141`).
3. `InspectorParams` has no `id`/`run_id`/`dir`/`index`. Those four are upstream's
   `InspectorParams` too (`inspectors/types.ts:7-13`); only the dispatcher reads them, so this
   pass put them on a local `InspectorRequest` with a single `plugin_params()` conversion rather
   than redefining a shared type.
4. `InspectorContext.trusted_dir` is undocumented as to WHICH directory. It is filled here with
   upstream's `pane split --cwd` argument, `target.status.cwd ?? context.cwd`
   (`herdr/actions.ts:105`) — the only directory in the verb that is neither the async dir
   (already on the target) nor derivable by a backend. **The herdr writer must read it that way.**
5. `INSPECTOR_SUBCOMMAND` belongs in the frozen contract, not in one implementation module that
   another implementation module must reach into.
6. `path_within` is now a FOURTH private copy (`background/fleet_view.rs:244`,
   `tui/fleet_transcript.rs:367`, `background/scheduled_runs/store.rs:178`, and this one). All
   three existing copies are private to their module and promoting one is an edit to a file this
   writer does not own.

### Tests written (not run — this was a writing phase)

`session_roots_codec.rs`: exact-literal wire form; empty list; round trip through quote/backslash/
space paths; shell-inertness of the payload; the five-row refusal table on one stable sentence.
`shell_command.rs`: both Unix branches (bare + `sh -c`), the `'\''` escape, both Win32 branches,
the bare-executable character set, empty argv, `host_platform()`.
`actions.rs`: T-TGT-1 (all of upstream's refusal sentences incl. the negative index and the
resolver's own ambiguity message), the `:73` vanished-status arm, a valid child index, T-TRUST-1
(both rungs + the symlink escape), T-CMD-1 (answers with no backend), T-CMD-2 (base64
`--session-roots` round-trips out of the launch), the job's own session root joining the list,
T-AUTH-1 (both flags follow the policy), `--index` only for a child, `--mission-path` only with a
binding, T-OPEN-0 (exact sentence + nothing written), open-takes-available-not-owns, the plugin
receiving the run's own cwd + focus + launch, T-STAT-2 (status/close with no owner are `Ok`), the
owning-plugin routing, the `does not support status/close` arms, and an unresolvable target being
an error for all four verbs. Each carries its own GUT-this-line note in the doc comment.

### What the WIRE agent owes this module

See `needsWiring` in this pass's structured report: the seven `SUBAGENT_ACTIONS`/schema entries go
between `"refine.rollback"` and `"watchdog.status"` (`[AUG — verbs]` V2's CORRECTED insertion
point — cyrup's list is not in upstream's order and has no `"status"` there), plus the
`route_action` arms, the two `AUTHORITY_ACTIONS` additions and the `tool-reference.md` rows.

---

## [WRITE — verbs-runner]

WRITE pass, concurrent with four siblings. Files written: ONLY
`crates/cyrup-ext-subagents/src/inspectors/runner.rs`,
`crates/cyrup/src/subagent_inspector_cmd.rs` (new), `crates/cyrup/src/predispatch.rs`,
`crates/cyrup-it/tests/subagents/inspector_runner_subcommand_integration.rs` (new). Nothing else
was touched — not the frozen contract, not `main.rs`, not `lib.rs`, not the IT suite's `main.rs`,
not any shared registration file. Upstream read exclusively at
`git -C tmp/pi-subagents show v0.68.0:<path>`.

### What now works

`cyrup __subagent-inspector --async-dir <dir> --run-id <id> [--allow-steer <b>] [--allow-stop <b>]
[--session-roots <b64>] [--index <n>] [--mission-path <p>] [--refresh-ms <n>]` IS the inspector
pane: it clears the screen, renders a lifecycle dashboard for one async run on a timer, and reads
`status` / `stop` / `steer <message>` / plain guidance off stdin, landing REAL steering and stop
requests on that run's control channel with `source: "inspector-runner"`. It is a mirror — closing
it does not stop the run — and it is reachable today by pasting the string `inspector.command`
returns into any terminal, with no herdr and no ghostty installed.

### Fidelity points the exec/verify pass must not "simplify"

* **`is_terminal_for_inspector` is NOT `RunState::is_terminal`.** Upstream's inspector predicate is
  `state !== "queued" && state !== "running"` (`inspector-runner.ts:81-83`), which INCLUDES
  `paused`; this crate's `RunState::is_terminal` (`background/state.rs:155`) deliberately EXCLUDES
  it, because a paused run is resumable. Routing the pane through the crate's predicate leaves a
  paused run's timer spinning forever and accepts a steer with no live child to deliver it to.
  Pinned by `the_refresh_predicate_is_upstreams_not_run_state_is_terminal`.
* **`--allow-steer` / `--allow-stop` are `!== "false"`, not a bool parse** (`:76-77`). `FALSE`,
  `0` and `no` all mean TRUE. Ported literally; a table row asserts each.
* **The controls line degrades on two independent axes** (`:44-45`, `[AUG — verbs]` V17). A
  `single` run with `allow_stop == false` still offers `type guidance`. A fixed string gets three
  of the five cases wrong.
* **`status` and an empty line answer before `status.json` is read** (`:101`), so a pane whose run
  was swept still refreshes instead of refusing.
* **`reply …` is matched AFTER `steer …` and BEFORE the plain-guidance fallthrough** (`:110`,
  `:115`, `:116`), so `reply` never becomes guidance text.
* **The receipt's fence WIDENS** past the longest ```` `{3,} ```` run in the preview
  (`steering.ts:43-44`), and the preview is REDACTED before it is truncated (`:22-24`) so a
  secret cannot survive half-cut across the ellipsis.
* **`parse_args` is strictly pairwise** (`:52-55`): an odd-arity argv or a bare token refuses the
  WHOLE parse. It is not a lenient flag scanner.

### The one structural delta (recorded, with its grep)

`queue_inspector_steer` collapses upstream's plural `targetIndexes: runningIndexes` (`:93`) to
cyrup's `target_index: None`, whose doc already means "every currently running child, which is what
the runner fans it out to" (`background/control.rs:1183-1184`). A RESOLUTION-TIME difference, not a
wire-shape one, and the better answer — upstream's snapshot can name a child that finished between
the write and the drain. Grep that keeps the premise honest:
`rg -n 'target_index' crates/cyrup-ext-subagents/src/background/control.rs` → one `Option<usize>`
field, no plural sibling. **What the collapse does NOT take with it** is upstream's `:90` refusal:
`running_indexes` is still computed, purely to drive
`"No running child is available to steer. Open a child-specific inspector for a pending child."`
Without it, `None` succeeds into an empty fan-out and the user's guidance silently vanishes
(`an_aggregate_steer_with_no_running_child_is_refused`).

### `[CYRUP-EXCEEDS-UPSTREAM]` — a header the herdr backend can wait on

`INSPECTOR_HEADER_PREFIX = "cyrup-inspector for "` is `pub`, because
`[AUG — verbs]` §B.2.3's `herdr pane wait-output <id> --match …` needs a literal to match and the
dashboard's own header line is it (`inspector-runner.ts:33`). pi cannot do this at all: `pane run`
is `pane.send_input`, which returns as soon as keystrokes reach the PTY
(`herdr src/cli/pane.rs:1046-1052`), so upstream writes a binding for a pane that may never have
started the inspector. **The herdr writer must match on `INSPECTOR_HEADER_PREFIX`, not on a
copied string literal.**

### Corrections to `[AUG — inspector]` / `[AUG — verbs]` found while writing

1. **`preview_display_text` EXISTS.** `[AUG — inspector]` §0 and §10 both say it does not
   (*"`previewDisplayText` does not"*). It is at
   `crates/cyrup-ext-subagents/src/workflows/display_text.rs:201`, `pub` via
   `crate::workflows::preview_display_text` (`workflows/mod.rs:102-103`), a faithful port of
   `shared/display-text.ts:95-100` including the `<= 3` bare-truncation branch. Only
   `steering_receipt` and `steering_message_preview` were genuinely missing; both now live in
   `inspectors/runner.rs`, ~20 lines, as upstream's composition of two existing functions.
2. **`predispatch.rs` had FIVE false counts, not four.** Beyond V25's `:11`, `:29`, `:59`, `:78`,
   the module doc's opening line said *"Five gates run ahead of `parseArgs`"* over a five-item
   list that this hop makes six. All corrected in place.
3. **`crates/cyrup` has no `io-std` tokio feature of its own** (`crates/cyrup/Cargo.toml:108`,
   workspace features at `Cargo.toml:154`), yet `tokio::io::stdin()` compiles there today —
   `main.rs:829-830` and `input.rs:594` already use it, so the feature arrives by workspace
   unification. `subagent_inspector_cmd::dispatch` relies on the same unification. If the verify
   pass sees `tokio::io::stdin` unresolved, the fix is one feature on `crates/cyrup`'s tokio
   dependency, not a rewrite of the seam.

### Tests written (not run — this was a writing phase)

`inspectors/runner.rs`: T-ARGV-1 (the whole refusal table plus the `Number("") === 0`,
`--allow-steer FALSE` and repeated-key edges), T-DASH-1 (header, mirror sentence, and all five
controls cases), T-CTL-1 (steer AND stop assert the FILE, not the receipt), the aggregate
`target_index: None` case, T-CTL-3, T-CTL-2 (nine refusals including the paused rows), the
fence-widening and redaction receipts, the whole `run_inspector` loop over an injected
stdin/stdout, the `Control error:` notice path, the degraded no-status screen, and the refresh
predicate. `crates/cyrup/src/subagent_inspector_cmd.rs`: the two-independent-literals equality,
the exact-`argv[1]` predicate, and undiscoverability through `first_subcommand` (V13 — `SUBCOMMANDS`
is private). `crates/cyrup-it/.../inspector_runner_subcommand_integration.rs`: T-RUN-1 through the
REAL binary (the ONLY witness of the classification/dispatch split), the runner's-own-parser
refusal, and T-RUN-2. Every test carries its own GUT-this-line note in its doc comment.

### What the WIRE agent owes this module — the seam FAILS SILENTLY without it

`crates/cyrup` does not compile, and the pane is unreachable, until these three one-line edits
land. See `needsWiring` in this pass's structured report for the verbatim text.


---

## [WRITE — verbs-herdr]

WRITE pass, concurrent with four siblings. Files written: ONLY
`crates/cyrup-ext-subagents/src/inspectors/herdr/{mod,plugin,client,actions,focus,project_panes}.rs`
(6 files, ~3 000 lines with tests). Nothing else was touched — not `types.rs`, not `plugins.rs`,
not `inspectors/mod.rs`, not any shared registration file. Upstream read exclusively at
`git -C tmp/pi-subagents show v0.68.0:<path>`; herdr through `crates/cyrup-herdr`'s own source and
the citations it carries to `tmp/herdr` @ `d59d060`.

### There is ONE herdr client in this workspace, and this batch did not write a second

`client.rs` opens no socket and spawns no process. Every herdr operation ends at a typed
`cyrup_herdr::HerdrClient` method (`crates/cyrup-herdr/src/client.rs:93-532`). What `client.rs` is
is a **translator**: the frozen contract's seam is argv-shaped
(`inspectors/plugins.rs:52-56`), and `SocketHerdrClient::run` dispatches that argv to the matching
typed call, then renders the answer back into the `{"pane": {…}}` envelope herdr's own CLI prints
(`tmp/herdr/src/cli/runtime.rs:109-111`, stripped of `result` exactly as `client.ts:97-98` does).

Keeping the seam argv-shaped is not inertia. It is what lets every test in this subtree assert the
**exact sequence of herdr operations** a verb performs — the thing that actually breaks in
production, because herdr rejects a flag it does not know. Pinning the typed calls instead would
pin Rust structs, which cannot drift.

### What a person gets, on a box with herdr

* `inspector.open` splits a pane beside their work, starts the dashboard in it, **waits for the
  dashboard's own first line before writing the binding**, tells herdr the run is working so it
  appears in the sidebar, and writes the binding.
* `inspector.open --focus` on a pane that is already open **focuses it**, in one call.
* `inspector.status` / `inspector.close` read that binding back; `close` tolerates a pane the user
  already closed by hand.
* `project.open` runs a full cyrup session in a project root, indexes the root so a later session
  finds it, and `--focus` on an open pane focuses it (upstream's fourth manager method, which
  `[AUG — verbs]` V26 names and without which `--focus` is a silent no-op).
* `project.close` refuses while the pane is not explicitly idle and refuses when it cannot prove
  the pane is this project's.

### What a person gets on THIS container, with nothing installed

| verb | answer | `is_error` | herdr calls |
|---|---|---|---|
| `inspector.status` / `inspector.close` | `No Herdr inspector binding exists for async run {id}.` | **false** | **0** |
| `project.status` / `project.close` | `No Herdr project pane binding exists for {root}.` | **false** | **0** |
| `inspector.open` | `Herdr inspector error (HERDR_UNAVAILABLE): Herdr is not installed or is not on PATH. Install Herdr 0.7.5+ or set HERDR_BIN.` | true | 0 |
| `project.open` | the same sentence under `Herdr project pane error (…)` | true | 1 (`--version`) |

The zero-call rows are asserted by asserting the recorder is EMPTY, not by checking text — that is
what proves no probe happened. The gate is a pure environment read
(`HerdrPane::discover`, `crates/cyrup-herdr/src/env.rs:133-147` = `herdr/plugin.ts:13-14`), so
nothing can hang: there is no binary to time out on.

### `[CYRUP-EXCEEDS-UPSTREAM]` — six, each with a greppable premise

| # | what | premise | where |
|---|---|---|---|
| 1 | `pane focus` is ONE call across tabs AND workspaces | `tmp/herdr/src/app/api/panes.rs:484-500`, test `…across_tabs_and_workspaces` at `:4152` | `focus.rs` |
| 2 | pi's *"Herdr cannot refocus an arbitrary raw pane id"* (`actions.ts:100`) is **DELETED** | its premise is false against 0.9.1 — same citation | `actions.rs::already_open` |
| 3 | `pane run` is VERIFIED with `pane wait-output` before a binding is written | `pane run` is `pane.send_input`: keys into a shell (`tmp/herdr/src/cli/pane.rs:1046-1052`), so `ok` ≠ started | `actions.rs` |
| 4 | the split passes `--ratio` and `--env` | `PaneSplitParams.ratio`/`.env` (`crates/cyrup-herdr/src/schema/panes.rs:50,60`); pi passes neither | `actions.rs` |
| 5 | `pane report-agent` puts the run in herdr's sidebar/rollups/`agent.wait` | `socket-api.mdx:701-733`, `:715-716`; pi never calls it from `inspectors/` | `actions.rs` |
| 6 | ownership verifies on `cwd` **OR** `foreground_cwd`; the roster summary comes from `state_labels` first | herdr documents `cwd` as the LABEL cwd (`socket-api.mdx:750-752`); `PaneInfo` has none of pi's seven summary keys except `tokens` | `project_panes.rs` |

#5 is the largest capability gap in the whole port and costs one call per open. #6's second half is
why `fleet-status.ts:866`'s `summary ?? "—"` would otherwise render a dash forever.

**Exactly one upstream sentence changed** (#2), and only because its premise is false. Every other
model-visible string in these six files is upstream's, byte for byte.

### The `legacyToolCompatibility` DOUBLE STANDARD — ported as ONE manager with a flag

`ToolCompatibility::{Legacy, Strict}`. The seven rows where the two modes disagree are tabulated on
`project_panes.rs`'s module doc with upstream's line for each (`:428 :431 :438 :444 :492 :559
:566 :582`). Collapsing to strict makes `project.open` fail on a live pane herdr reports oddly;
collapsing to legacy makes the public API close someone else's pane. `close` is the one place
neither mode yields: **ownership verified, then explicitly idle**, in that order, in both modes.

### Fidelity points the exec/verify pass must not "simplify"

* **`project.status`/`project.close` never call `detect_herdr`.** Only `open` does (`:550`). The
  symmetric-looking fix breaks the two rows that work with nothing installed.
* **`project.close` prunes the root index BEFORE the absent check** (`:706` then `:709`). A
  faithful port mutates the index even on the absent path, or session-start restore re-reads dead
  roots forever.
* **`supports_raw_panes` is a three-term disjunction**, not a lexicographic compare
  (`client.ts:118-120`). Both agree on every input herdr can produce; only one is what upstream
  wrote.
* **`PANE_GONE` is never produced by herdr** (`[AUG — verbs]` §B.1). The only source is pi's own
  literal at `actions.ts:110`, and it is kept there and nowhere else.
* **The binding structs carry `#[serde(flatten)] extra` and NO `deny_unknown_fields`**, and every
  optional carries `skip_serializing_if`. A `"childIndex": null` fails pi's own `parse`
  (`actions.ts:33`) — `null` is not `undefined`. Both are pinned by a round-trip test.
* **`status`/`close` with no binding are `Ok`, not `Err`** (`:137`, `:148` pass no second
  argument). Pinned with an explicit `is_ok()` assertion, not a text check.
* **The one collapse made deliberately**: upstream uses 5 s for the *cleanup* `pane close`
  (`:113`) and 10 s for the *requested* one (`:149`). At the argv layer those are the same three
  tokens, so both get 10 s — the more patient, because a cleanup close that times out strands a
  pane in the user's terminal. Recorded on `client::timeout_for`.

### Contract gaps reported (the orchestrator's call, not worked around silently)

1. **`HerdrClient::run` returns `Result<Value, HerdrErrorCode>` — a bare code with NO message**,
   while every failure sentence in this subtree is `Herdr inspector error ({code}): {message}`
   with herdr's OWN message (`actions.ts:70-72`, forwarded by `client.ts:92`). `client::message_for`
   reconstructs the sentence from the code plus the argv, and every message these tests assert is
   reconstructible that way — but herdr's specific explanation is lost at the seam.
   `Result<Value, HerdrFailure>` is the fix.
2. `InspectorContext` has no **env** (re-reporting the core writer's gap). Worked around locally by
   making the environment injectable on `HerdrInspectorPlugin`, which is this module's own type.
3. `InspectorContext` has no **run state**. `statusHerdrInspector` prints `Run state: {state}`
   twice (`:141`, `:143`). Worked around by re-reading `<async_dir>/status.json` through
   `RunDir::status`, which is where the dispatcher's snapshot came from anyway.
4. **`HerdrProjectPaneBinding`, `HerdrProjectPaneSnapshot`, `ProjectPaneRuntime` and
   `ToolCompatibility` are NOT in the contract** and are defined here. That is right for three of
   them, but `HerdrProjectPaneSnapshot` has a consumer outside this module — `tui/fleet_status.rs`
   — so it arguably belongs in `types.rs` beside `HerdrInspectorBinding`.
5. **`format_shell_command` / `host_platform` are not in the contract** although two
   implementation modules call them (`project_panes::project_pane_command` here, and the
   dispatcher's `launch_for`). Consumed at the signature the `[WRITE — verbs-core]` section
   documents; `cargo check` confirms it.

### Tests written (not run — this was a writing phase)

`client.rs`: the version scanner (incl. a non-ASCII prefix), the three-term floor,
`detect_herdr`'s two refusal sentences and its two accepted versions, the install sentence, the
per-call deadline table, the repeated-flag argv helpers, `normalizeCode` over herdr's REAL codes,
and the no-client-outside-a-pane gate.
`focus.rs`: **T-FOCUS-1** (exactly `["pane","focus",id]`, and NO `tab`/`workspace` call),
`INVALID_PANE_RESPONSE`, the pre-0.9 `PANE_FOCUS_UNSUPPORTED` rung, the `NOT_FOUND`
short-circuit, and both pane-record shapes.
`actions.rs`: the binding path, **T-OPEN-1** (the full five-call argv sequence + the written
binding + the omitted-null assertion), **T-OPEN-2**, **T-OPEN-4**, **T-OPEN-3** (both with and
without focus), the `PANE_GONE` split literal, **T-STAT-1**, **T-STAT-3** (both tolerated codes
AND the non-tolerated one), the non-error no-binding pair, the three staleness checks, the
unknown-key round trip, and the session-roots lift.
`plugin.rs`: `available()`'s two variables, `available()` never touching the seam, `owns()` being
the binding file only, both optional methods being supplied, and the no-seam install sentence.
`project_panes.rs`: **T-PROJ-2** (incl. the empty-recorder assertion), **T-PROJ-3**, **T-PROJ-1**
(open → status → refused close → idle close, with the root index and the snapshot map),
**T-PROJ-5**, **T-PROJ-6**, **T-SUM-1**, the ownership widening, the double standard at both the
parse rung and the `INVALID_PANE_RESPONSE` rung, the stale path in both modes, **V26**'s focus,
the restore + `open_project_pane_count`, the root index's set/delete-when-empty contract,
`resolveProjectRoot`'s two refusals, the unknown-key round trip, and the strict blank-field rung.

Every test carries a GUT-this-line note in its doc comment naming what breaks for a USER when the
line goes.

### `cargo check`, once, at the end

`cargo check -p cyrup-ext-subagents` reported **one** error across all six files — a
`#[non_exhaustive]` match on `HerdrError` needing a catch-all, now present with the reason the
catch-all is `VALIDATION_ERROR` and not a tolerated code. Nothing else. In particular the two
signatures this pass could not read (`format_shell_command`, `host_platform`, owned by a sibling)
resolved.

---

## [WRITE — ghostty-fleet]

WRITE pass, concurrent with four siblings. Files written: ONLY
`crates/cyrup-ext-subagents/src/inspectors/ghostty/{mod,plugin,actions}.rs`,
`.../src/tui/{fleet,fleet_overlay,fleet_status}.rs`, `.../src/missions/store.rs`. Nothing else was
touched — not the frozen contract, not `inspectors/mod.rs`, not `tui/fleet_state.rs`, not
`extension/host/{slash,native_impl}.rs`, not `extension/executor/`, not any shared registration
file. Upstream read exclusively at `git -C tmp/pi-subagents show v0.68.0:<path>`.

### What a HUMAN can now do

Inside the fleet inspector, **`Enter` or `H` on a live async child opens an inspector pane for it**,
with `focus: true` — the pane takes the keyboard, because looking at the child is why the key was
pressed. The refusals are upstream's v0.68.0 sentences, and the one for "no handler" is now
`Inspector controls are unavailable in this context.` — **no "Herdr"**, because the dispatcher
picks whichever backend is available and may have opened ghostty's. The footer reads
`Enter/H Inspect`, which is what `bindingLabel(keybindings, "inspect")` renders for
`["return", "H"]` (`fleet.ts:45`, `:67-71`, `:1368`).

The fleet **status widget now carries project panes**: a `project panes` section under the agent
tree, each row `"<project> · <paneId>"` with its summary and how long ago herdr refreshed it, and a
collapsed line that counts them separately and flags the ones wanting a human —
`1 active agent · 2 panes (1 ⚠)`. That is the sidebar telling you which pane is blocked on you.

On macOS inside Ghostty, `inspector.open` opens a real read-only inspector terminal through
Ghostty 1.3's AppleScript. Everywhere else the backend says so instantly, with no probe and no
spawn.

### The three ghostty files (91 upstream lines, ported whole)

| file | what |
|---|---|
| `ghostty/actions.rs` | `GHOSTTY_APPLESCRIPT` byte-for-byte (`actions.ts:7-26`, pinned at 20 lines), `GHOSTTY_FAILURE_HINT` (`:52`), `ghostty_argv`, `open_ghostty_inspector` (`:54-74`), and `OsascriptRunner` — the real `/usr/bin/osascript` seam over `tokio::process::Command` with `:63`'s 15 s timeout, `:65`'s 64 KiB cap and `kill_on_drop`. |
| `ghostty/plugin.rs` | `GhosttyInspectorPlugin` with `with_runner`/`with_env`/`with_macos`. `available` is `platform === "darwin" && env.TERM_PROGRAM?.toLowerCase() === "ghostty"` (`plugin.ts:13`) and **nothing else** — no probe, so the not-installed refusal is instant and cannot hang. `owns` is a literal `false` (`:14`); `status`/`close` are `None`, because upstream's object has no such keys. |
| `ghostty/mod.rs` | Module doc: one verb, no binding, and why nothing here is `#[cfg]`-ed out. |

### Fidelity points the exec/verify pass must not "simplify"

* **`params.focus === true` is a STRICT comparison** (`actions.ts:61`). Absent ⇒ `"false"` ⇒ the
  AppleScript refocuses the SOURCE terminal. `!= Some(false)` would steal the keyboard on every
  agent-driven open.
* **The argv is positional**: `["-e", SCRIPT, "--", displayCommand, cwd, focus]`. `on run argv`
  reads items 1-3 in that order; a reorder opens the right terminal in the wrong directory running
  the wrong string, and only `ghostty_opens_through_an_injected_runner_on_any_platform` sees it.
* **The cwd is `ctx.trusted_dir`**, which the dispatcher fills with upstream's
  `context.target.status.cwd ?? context.cwd` (`[WRITE — verbs-core]`'s contract gap 4). Not the
  async dir.
* **A non-zero exit is a FAILURE.** Node's `execFile` rejects it, so upstream never reads a
  terminal id off a failed osascript. Dropping that check turns "not authorized to send Apple
  events" into a success sentence naming a terminal that does not exist.
* **ghostty is never a status owner.** `owns() == false` is what makes `inspector.status` on a
  ghostty-only host answer upstream's non-error *"No inspector plugin owns this binding…"*.

### `[CYRUP-DELTA: the frozen `GhosttyRunner` error channel carries a CODE, not a message]`

Upstream renders `Ghostty inspector error: ${cause.message}. ${hint}` (`actions.ts:70-72`) where
`cause` is Node's `execFile` rejection. The frozen seam is
`Result<CommandOutput, HerdrErrorCode>` — a bare code. So the port recovers the text where it still
exists and renders the code where it cannot: a **non-zero exit** (the realistic AppleScript failure)
carries osascript's own stderr in Node's `Command failed: /usr/bin/osascript\n…` shape, and a
**spawn failure or timeout** renders `HERDR_UNAVAILABLE` / `TIMEOUT`. Reported in `contractGaps`;
the fix is one message field on the seam and it is the orchestrator's call.

### The fleet `H` key — every line number re-verified, and three of the AUG's corrected

Verified at this tree: `fleet.rs:1820` (`Char('H')`), `:1822` (the `has_actions && has_inspect`
guard), `:1825` (the literal), `:1468` (`has_inspect`), `:1650` (`selected_async_action`),
`:2208` (the footer), `:3505` (the footer assertion); `fleet_overlay.rs:275-278` (the `Inspect`
arm), `:618-639` (the test). **Corrections:**

1. The task brief's *"tests at `:3392` and `:3403`"* — the tests are
   `herdr_is_unavailable_without_an_inspect_handler` at **`:3391-3399`** and
   `herdr_dispatches_when_an_inspect_handler_exists` at **`:3402-3416`**; `:3392` and `:3403` are
   their bodies' first lines, not the tests.
2. `[AUG — inspector]` §3 D2 and `[AUG — verbs]` V7 both say `fleet_overlay.rs`'s
   steer-delivery-mode half *"stays true"*. **It is FALSE.** SUBA-049 gave `control_steer` a `mode`
   parameter (`extension/executor/foreground_actions/steer.rs:82`) and `fleet_overlay.rs:222-243`
   already passes the `Tab`-selected mode into it. So `fleet.rs`'s delta 1 and `fleet_overlay.rs`'s
   `:34-36` are BOTH deleted, not just the herdr halves, and `fleet_overlay` now has no deltas at
   all.
3. D13 says the footer becomes `H Inspect`. Upstream's own footer renders
   `bindingLabel(keybindings, "inspect")`, and `bindingLabel` (`fleet.ts:67-71`) maps `return` to
   `Enter` and joins with `/` — so `["return", "H"]` renders **`Enter/H`**. The footer is
   `Enter/H Inspect`; `H Inspect` would be a cyrup invention.
4. `selectedInspectAction`'s external rung (`fleet.ts:950`) is **unreachable in cyrup**:
   `rg -n external crates/cyrup-ext-subagents/src/tui/fleet.rs` is zero-hit and `FleetItemKind` has
   three members, none of them `External`. Recorded in the function's doc as unreachable-not-
   dropped, with the arm named for the day an external surface lands.

`fn selected_inspect_action` is written beside `selected_async_action`, with the async rung, the
parent-workflow rung (`:956-958`, returning the PARENT's id and dir and **no index**) and both
refusal sentences. Its parent lookup also consults `history_jobs`, marked
`[CYRUP-EXCEEDS-UPSTREAM]`: pi looks only in two in-memory maps, and the actionable-state test is
applied identically either way, so the widening cannot admit a settled parent.

### `FleetStatusEntry` gains `surface` + `project_pane` — and a renderer

`tui/fleet_status.rs` now ports `projectPaneEntries` (`fleet-status.ts:351-365`),
`projectPaneNeedsAttention` (`:517-520`), `projectName` (`:522-524`), the fold at `:526`, the
collapsed-line pane count at `:764-765,:781`, the work/pane split of the agent tree at `:800-801`,
`renderProjectPaneSection` (`:845-853`) and `renderEntry`'s project-pane right column (`:865-866`).
`FleetStatusSurface` is a one-variant enum rather than an `Option<String>`, for the reason
`HerdrInspectorKind` is one.

`parse_iso8601_millis` (pi's `Date.parse(pane.openedAt) || pane.refreshedAt`, `:361`) is written
**locally** as the exact inverse of `crate::time::format_iso8601_millis`, because `crate::time` is
not this writer's file. It belongs beside its inverse; that is a one-function move for whoever owns
that file next.

### What the WIRE agent owes this module — `H` is INERT without the first two

See this pass's `needsWiring` for the verbatim edits. In summary: `extension/host/slash.rs:86`'s
`false` → `true` (without it `Enter`/`H` answer the unavailable notice forever, which is the whole
delta this batch deletes); a `SubagentExecutor::inspector_open` entry point shaped like
`control_stop`; `ForegroundControlView::parent_workflow_run_id` plus its one producer line (without
it `selected_inspect_action`'s parent rung does not compile — the entry already carries the value
at `extension/executor/notices.rs:73`); `FleetState::herdr_project_panes`; and the session-start
`restore_herdr_project_pane_snapshots` call in `extension/host/native_impl.rs`.

### Tests written (not run — this was a writing phase)

`ghostty/actions.rs`: T-GHOST-1 (exact argv + exact success sentence), the strict-`focus` table,
T-GHOST-2 (empty id, runner refusal and timeout all carrying the hint, all `Err`), the non-zero
exit and signalled-process arms, the AppleScript's own shape, the 64 KiB cap including a multi-byte
boundary, and the trusted-dir launch directory. `ghostty/plugin.rs`: the two-conjunct case-folded
`available`, `available` never touching the runner, T-GHOST-3 (`owns` false + both optional methods
absent), and `open` running the injected seam. `tui/fleet.rs`: T-FLEET-3 (upstream's wording),
T-FLEET-1 (`H` AND `Enter`, one handler), `Enter` still belonging to the steer draft, T-FLEET-2
(all three resolver rungs), the settled-child refusal, and the empty roster. `tui/fleet_overlay.rs`:
the `Inspect` arm REACHING `inspector.open` (the old literal could not produce a resolver sentence).
`tui/fleet_status.rs`: the open/stale roster fold, both `||` fallbacks, the collapsed pane count and
`⚠`, the section-not-tree rendering with the summary/refresh-age column, the substring attention
test, `project_name` over both separators, and the ISO-8601 parser against its own writer. Every
test carries its own GUT-this-line note in its doc comment.
