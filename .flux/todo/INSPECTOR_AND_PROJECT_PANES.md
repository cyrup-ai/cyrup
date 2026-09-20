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
