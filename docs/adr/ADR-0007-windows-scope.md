# ADR-0007 — Windows is a supported platform

**Status** accepted (decided by default under the parity rule — overridable)
**Date** 2026-08-13
**Decides** OQ-5 (`docs/PARITY-PLAN.md:1446-1451`)
**Blocks released** batch 2 (OQ-5 itself); batch 9's `TOOL-036` / `TOOL-038` / `DRIFT-046`
(`PARITY-PLAN.md:185`); batch 24's `PB-19` = `ICOM-015`; the "moot if Windows is out of scope"
hold recorded at `12-upstream-drift-pi-core.md:999`; the "Prerequisite decision" hold recorded at
`04-cyrup-tools.md:593` and `04-cyrup-tools.md:733`.

## Context

### The question as posed rests on a bad measurement

`PARITY-PLAN.md:1447` and `PARITY-GAPS.md:830` both state the position as **"161 `cfg(unix)` sites
against 6 `cfg(windows)`"** and infer from it that a "yes" makes Windows a port-wide programme. Both
halves of that inference are wrong, and the ADR has to correct them before it can decide anything.

Measured at cyrup HEAD `72cd292` over `crates/`, `*.rs` only:

| form | count | src only |
|---|---|---|
| `cfg(unix)` / `cfg(all(unix …` / `target_family = "unix"` | 162 | 132 |
| `cfg(windows)` (the attribute — the figure the plan quotes) | 6 | 6 |
| `cfg!(windows)` (the **runtime** macro) | 30 | 28 |
| `cfg(not(unix))` + `cfg_attr(not(unix) …` | 26 | 26 |
| `target_os = "windows"` | 0 | 0 |

The "6" counts only the attribute spelling. cyrup's Windows branching is written predominantly in
the **runtime `cfg!(windows)`** form — `crates/cyrup-ext-subagents/src/spawn/worktree.rs` (6),
`crates/cyrup-config/src/config_value.rs` (5), `crates/cyrup-permission-system/src/wildcard.rs` (4),
`crates/cyrup-tools/src/path.rs:80`, `crates/cyrup-tools/src/ops/shell.rs:167` — plus 26 explicit
`cfg(not(unix))` fallback arms. The true Windows-aware surface in `crates/` is **62 sites, not 6**.
One of the six attribute hits is not even code: `crates/cyrup-tui/src/app.rs:2051` is a doc comment.

### What cyrup actually does on Windows today: it very nearly builds

This was never established in any gap file. It is cheap to establish, so this ADR establishes it.
`cargo check --target x86_64-pc-windows-gnu -p <crate>` at HEAD `72cd292`, per crate (`x86_64-pc-windows-gnu`
is the Windows target installed on this workstation; `cargo check` does not link, so no Windows host
is involved):

- **exit 0, zero errors (15 crates):** `cyrup-core`, `cyrup-provider`, `cyrup-tools`, `cyrup-config`,
  `cyrup-resources`, `cyrup-session`, `cyrup-ext`, `cyrup-tui`, `cyrup-permission-system`,
  `cyrup-ext-subagents`, `cyrup-agent`, `cyrup-modes`, `cyrup-sdk`, `cyrup-session-svc`,
  `cyrup-test-support`.
- **fails (1 crate):** `cyrup-intercom`, 6 errors, **all in the broker**:
  - `crates/cyrup-intercom/src/broker/mod.rs:24` — `unresolved import tokio::net::UnixListener`
  - `crates/cyrup-intercom/src/broker/mod.rs:25` — `unresolved import tokio::net::unix`
  - `crates/cyrup-intercom/src/broker/mod.rs:1177` — `cannot find type UnixStream in module tokio::net`
  - `crates/cyrup-intercom/src/broker/mod.rs:1255` (×2) — `cannot find unix in signal`
  - `crates/cyrup-intercom/src/broker/runtime_claim.rs:110` — `cannot find errno in nix`
- **fails transitively (1 crate):** `cyrup` (the binary) — 7 errors, every one of them the six above
  plus `could not compile cyrup-intercom`.

`nix` and `libc` are not the obstacle anyone assumed: `nix` compiles to an empty crate on the Windows
target, and `cyrup-ext-subagents` — the crate with the most `cfg(unix)` sites of all (77) and an
unconditional `nix` dependency at `crates/cyrup-ext-subagents/Cargo.toml:50` — **cross-compiles
clean**. `cyrup-tools` already gates `libc` correctly at `crates/cyrup-tools/Cargo.toml:44`
(`[target."cfg(unix)".dependencies]`).

So the honest state is: **17 of 18 crates compile for Windows today, and the entire port is blocked
on one file.** That file is `PB-19`/`ICOM-015` — currently filed **low**.

The intercom crate is also the sharpest illustration that Windows was already being ported on
purpose. `crates/cyrup-intercom/src/transport/spawn.rs:154-169` carries a **complete** Windows
broker-spawn arm — `DETACHED_PROCESS | CREATE_NO_WINDOW` via `tokio::process::Command::creation_flags`
— with a written rationale for why it does not reproduce pi's `.vbs` + `wscript.exe` hidden launcher
(`pi-intercom` v0.9.2 `broker/spawn.ts:63-64,95,115-120,128-141,205-207`). The **client** transport is fully live
(`transport/target.rs:254`, `transport/stream.rs:64-90`, called from `transport/spawn.rs:226,299`).
Only the **listen** side was deferred, in-source, at `crates/cyrup-intercom/src/paths.rs:6-8`
("First cyrup milestone: **Unix domain socket only**") and `broker/mod.rs:1253-1254` ("this whole
entrypoint is inherently unix-only for this milestone"). Those two comments are the entire basis of
the deferral, and neither is a decision of record.

### Does pi support Windows? Unambiguously yes — and it ships binaries for it

Read at `v0.83.0` unless noted.

- **`package.json`** has no `os` field and no platform restriction; `engines` is `node >= 22.19.0`
  only. Nothing excludes Windows.
- **`scripts/build-binaries.sh:73,139,147-148`** builds `windows-x64` and `windows-arm64` with
  `bun build --compile --target=bun-windows-<arch> … --outfile …/pi.exe`, packaged as `.zip` at
  `:207-251`.
- **`.github/workflows/build-binaries.yml`** requires `pi-windows-x64.zip` and
  `pi-windows-arm64.zip` in both the `binary_assets` existence loop and the `expected_assets`
  validation loop, and in `SHA256SUMS`. **A release that fails to produce the Windows binaries
  fails.** Windows is a release gate upstream.
- **`packages/coding-agent/docs/windows.md`** is a user-facing setup document (bash discovery order:
  `settings.json` `shellPath` → Git Bash → `bash.exe` on `PATH`), linked from
  `packages/coding-agent/README.md:93` under "**Platform notes:** [Windows](docs/windows.md)".
- The public README documents Windows-specific *user* behaviour: `:165` "Ctrl+Enter on Windows
  Terminal", `:166` "Notepad on Windows", `:167` "Alt+V on Windows", `:229` the Alt+Enter remap note.
- **`packages/tui/native/win32/src/win32-console-mode.c`** with checked-in prebuilds for
  `win32-x64` and `win32-arm64`, loaded at `packages/tui/src/terminal.ts:339-366` to set
  `ENABLE_VIRTUAL_TERMINAL_INPUT`. pi wrote and ships **hand-written C** for Windows.
- **`packages/coding-agent/src/utils/windows-self-update.ts`** — a Windows-only self-update
  quarantine path, driven from `packages/coding-agent/src/main.ts:530,542`.
- **`packages/coding-agent/test/bash-close-hang-windows.test.ts:72`** —
  `describe.skipIf(process.platform !== "win32")`, a regression test written against a real Windows
  defect (`earendil-works/pi#5303`, cited at `packages/coding-agent/src/utils/child-process.ts:38-48`).
- **45 `win32` occurrences across 20 source files** at v0.83.0 (`packages/coding-agent/src` 35 in 18
  files, `packages/tui/src` 5 in 1, `packages/agent/src` 5 in 1), rising to **54** at v0.84.1 — the
  delta including `normalizeWindowsShellPath` (`packages/coding-agent/src/utils/paths.ts:67-73`
  @v0.84.1, called from `:84`; the function is absent at v0.83.0).
- `pi-intercom` v0.9.2 `broker/paths.ts:44-116` — `shouldUseWindowsTcpTransport`,
  `getBrokerSocketPath`'s `\\.\pipe\pi-intercom-…` named pipe, `getBrokerListenTarget`; consumed by
  `broker/broker.ts:25,233,240-269,401,481,1519`.

**And: pi has no Windows CI either.** All ten workflows at `v0.83.0/.github/workflows/` contain zero
`runs-on: windows` lines; `ci.yml` is `ubuntu-latest` only, and `build-binaries.yml` cross-builds the
Windows binaries *on Linux*. `pi-test.bat` and `pi-test.ps1` at the repo root are the manual smoke
harness that stands in for the CI pi does not have — `pi-test.bat` is a six-line shim that hands off
to `pi-test.ps1`, which mirrors `pi-test.sh`'s `--no-env` key-scrubbing behaviour.

cyrup has **no CI at all**: there is no `.github/` directory at HEAD; `.workflows/*.js` are agent
orchestration scripts, not CI.

### The behavioural holes on Windows that compile perfectly well

Of the 132 `cfg(unix)` sites in `crates/*/src/`, only 26 have an explicit non-unix arm. The other
~106 compile away to nothing. Sampled, with the pi counterpart:

| cyrup | today on Windows | pi @v0.83.0 |
|---|---|---|
| `cyrup-tui/src/app.rs:5018-5019` `copy_to_clipboard` | `#[cfg(not(unix))] fn copy_to_clipboard(_text: &str) {}` — silent no-op | `utils/clipboard.ts:85-87` `execSync("clip", …)` |
| ~~`cyrup-ext-subagents/src/spawn/signal.rs`~~ **CLOSED** | This row was wrong in both directions and is now moot. `send_sigkill` never "killed nothing" — it always fell back to `child.start_kill()` — but that is `TerminateProcess` against the DIRECT pid, so it reaped the child and **orphaned its whole descendant subtree**, where the unix arm `kill(-pgid, SIGKILL)`s the group on purpose. `send_sigint`/`send_sigterm` genuinely were empty, and the ladder still paid out both grace periods waiting for reactions to signals never sent. Fixed: stage 3 now runs pi's own `taskkill /F /T /PID` before `start_kill`, and the two graceful rungs report `false` so `terminate` skips their graces (the `cyrup_tools::ops::local::terminate_pid` `Ok(false)` convention). | `utils/shell.ts:200-212` `killProcessTree` → `taskkill /F /T /PID` |
| `cyrup-tui/src/drain.rs:189-193` | `consume_ready` returns `0` — the input drain does nothing | `packages/tui/src/terminal.ts:339-366` loads the win32 native VT-input helper |
| `cyrup-tui/src/terminal_query.rs:450-452` | `read_reply` returns `None`; background detection falls back to `COLORFGBG` | pi keeps reading stdin on Windows; `packages/tui/src/terminal.ts:339-366` exists precisely so it can |
| `cyrup-tools/src/path.rs:91-93` | `home_dir()` reads `$HOME` only, so the ported `~\` arm at `:80-84` never fires | `utils/paths.ts:66-72` `os.homedir()` |
| `cyrup-tui/src/keymap.rs:391` | `ctrl+z` → `Suspend` bound unconditionally; the action raises `SIGTSTP` | `core/keybindings.ts:69-71` `defaultKeys: []` on win32 |
| `cyrup-tui/src/keymap.rs:413-419` | binds **both** `ctrl+v` and `alt+v` | `core/keybindings.ts:112` binds exactly one per platform |
| `cyrup-tools/src/ops/shell.rs:140-144` | falls back to `cmd.exe /C` | `utils/shell.ts:100-107` **throws** `No bash shell found` |

Two of these are not Windows-conditional at all and do not wait on this ADR: `path.rs:91-93`'s
`$HOME`-only `home_dir` (already noted at `12-upstream-drift-pi-core.md:740`) and
`keymap.rs:413-419`'s double binding, which diverges from pi on *every* platform.

`arboard` is already a workspace dependency (`Cargo.toml:113`) and resolves `clipboard-win v5.4.1`
on the Windows target — the clipboard no-op is a choice, not a platform constraint.

### Mechanism differences that cost no behaviour

Two, and they are the good kind — state them and move on:

- **VT console mode.** pi ships a C native module (`packages/tui/native/win32/`) because Node cannot
  set `ENABLE_VIRTUAL_TERMINAL_INPUT`. cyrup uses ratatui + crossterm
  (`crates/cyrup-tui/Cargo.toml:50`), which does this itself. No counterpart is needed, and none
  should be written.
- **Hidden detached spawn.** pi writes a `.vbs` launcher and runs it through `wscript.exe` because
  Node cannot pass creation flags (`pi-intercom` v0.9.2 `broker/spawn.ts:63-64,95,115-120,128-141,205-207`). cyrup calls
  `Command::creation_flags` directly — already done, already documented in-source at
  `crates/cyrup-intercom/src/transport/spawn.rs:159-166`.

## Decision

**Windows is a supported platform for cyrup. Port the Windows behaviour, item by item, under the
same 1:1 rule as everything else.** Concretely:

1. **`x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc` are declared targets** — the two arches
   pi ships (`scripts/build-binaries.sh:139`). Record them in `rust-toolchain.toml` alongside the
   existing `wasm32-wasip2` note.

2. **Named prerequisite, and it is the gate on every Windows claim: a Windows CI target must exist
   before any Windows item may be marked verified.** Nothing in this workspace has ever executed a
   win32 branch, and this ADR does not pretend otherwise. Until a Windows runner exists, the
   *interim* gate — available today, at zero cost, on any host, because `cargo check` does not link
   — is:

   ```
   cargo check --target x86_64-pc-windows-msvc --workspace --all-targets
   cargo check --target aarch64-pc-windows-msvc --workspace --all-targets
   ```

   Add both to `cargo xtask` (batch 3) as a required check. This gate is **red today** and turns
   green the moment `cyrup-intercom`'s broker is fixed. A compile gate is not a behaviour gate; do
   not let it be reported as one.

3. **Fix `cyrup-intercom`'s broker first, and re-classify it.** `PB-19`/`ICOM-015` is the single
   thing standing between cyrup and a Windows build. Its fix is already written down and already
   correct (`11-cyrup-intercom.md` ICOM-015 **Fix**): call the already-ported
   `broker_listen_target` (`transport/target.rs:278`, currently with zero production callers) from
   `broker/mod.rs:1243`, cfg-gate the `UnixListener` import at `:24`, add the named-pipe and TCP
   listener arms, write `broker.port.json` on the TCP arm, and gate the
   `tokio::signal::unix::SignalKind::terminate()` at `:1255` and the `nix::errno` use at
   `runtime_claim.rs:110`. Effort **M** stands — the resolver, the three-arm enum, the discovery-file
   format and its validation ladder are all already written and tested.

4. **Do not strike `TOOL-036`, `TOOL-038` or `DRIFT-046`.** The "if Windows is out of scope by
   policy, strike this item" instruction at `04-cyrup-tools.md:593` and `:733`, and the "all moot"
   note at `12-upstream-drift-pi-core.md:999`, are discharged **against** striking. All three are
   ordinary work items on their scheduled batches.

5. **Where pi refuses, cyrup refuses.** `cyrup-tools/src/ops/shell.rs:140-144`'s `cmd.exe /C`
   fallback is deleted and replaced with pi's error text from `utils/shell.ts:100-107` — the
   three-option message naming Git for Windows, `PATH`, and `shellPath`. This is `TOOL-038` and it
   is a *cyrup-original* divergence, not a port gap: nothing forced it.

   **`docs/adr/ADR-0003-bash-scope.md` D4 reaches the identical instruction from the other side** —
   as a case of "cyrup never silently picks an interpreter the user did not choose" — and it is
   deliberately written to hold under *either* answer to OQ-5. This ADR supplies the answer that
   makes it a live defect rather than a hygiene fix. **Take ADR-0003's implementation note with it:**
   deleting the arm makes detection fallible, which `impl Default for ShellConfig`
   (`ops/shell.rs:149-153`) and `impl Default for Backend` (`ops/mod.rs:357-361`) cannot express, so
   a new `try_detect() -> Result<ShellConfig, ToolError>` is required at `registry.rs:54`,
   `builder.rs:635` and `session.rs:4500`, with the error surfaced at session construction. An
   implementer working from this ADR alone will not find that. ADR-0003 also deletes the
   `CYRUP_SHELL` arm in the same file and the same batch, so the two edits land together.

6. **Open a Windows area file, `docs/gap-analysis/13-windows-platform.md`**, and sweep the ~106
   `cfg(unix)` `src/` sites that have no non-unix arm against their pi counterparts. Four items do
   not cover this and never did; the table in §Context is a starting list, not the finding. The
   sweep's own acceptance is that every remaining armless `cfg(unix)` site in `src/` carries either a
   Windows arm or a one-line reason.

7. **A Windows behavioural claim requires a Windows run.** Same rule the TUI already lives under
   (`docs/gap-analysis/07-cyrup-tui.md`, and the standing "run it in a real terminal" constraint). A
   green cross-`check` closes item (2)'s prerequisite for *compilation* only.

## Consequences

### Ledger changes

| id | change |
|---|---|
| `PB-19` (`PARITY-GAPS.md:440`) = `ICOM-015` (`11-cyrup-intercom.md`) | **severity low → high.** The stated impact ("the broker binary does not build or run on Windows") is measurably an *understatement*: `cargo check --target x86_64-pc-windows-gnu -p cyrup` fails with six errors, all from this crate, so **the whole binary does not build**. `PARITY-GAPS.md:440`'s "(severity corrected down)" annotation is reversed. Kind stays `not-ported`/partial; effort stays M. Scope narrows to the listen half only — the spawn and client halves are done. |
| `PB-19` scheduling | **split out of batch 24 and moved forward.** It is a prerequisite for the compile gate in Decision (2), so it cannot sit behind ~20 other ICOM items. The rest of `ICOM-015`'s crate-mates stay in batch 24. |
| `TOOL-036` (`04-cyrup-tools.md:565`, low, parity-bug) | **unblocked** — the "Prerequisite decision" paragraph at `:593` is discharged: Windows is in scope, do not strike. Severity low holds. Its `~`/`os.homedir()` half (`cyrup-tools/src/path.rs:91-93`) was never conditional on this answer and stays in batch 9 exactly as `PARITY-GAPS.md:830` says. |
| `TOOL-038` (`04-cyrup-tools.md:343`, medium, cyrup-original) | **severity medium holds; scope confirmed as "delete the fallback, port the throw".** No longer contingent. Stays in batch 9. |
| `DRIFT-046` (`12-upstream-drift-pi-core.md:738`, low, upstream-drift) | unchanged as `duplicate-of: TOOL-036`; ported under `TOOL-036`. The "moot if Windows is not a declared target" hold at `12-…:999` is discharged **against** mootness. |
| `04-cyrup-tools.md:733` open lead 3 ("Windows is unexercised") | **closed by this ADR** for the scope half; the *verification* half survives as Decision (2)'s named prerequisite. |
| `PARITY-GAPS.md:830` item 3 | rewrite: PB-19 does **not** reduce to its second half; it grows. The "161 vs 6" figure is replaced by "162 unix sites vs 62 Windows-aware sites (6 attribute + 30 runtime `cfg!` + 26 `cfg(not(unix))`)". |
| `PARITY-PLAN.md:263` branch risk | **partially discharged.** "If OQ-5 returns Windows is in scope, the 161-vs-6 imbalance is a port-wide problem, not four items" — true in kind, but far smaller in size than feared: 17/18 crates already compile and 62 sites already branch. Re-size the branch, do not re-plan around it. |

### Batch by batch

- **Batch 2** — OQ-5 closes here. Update `PARITY-PLAN.md:1389`'s row 9 and `§7 OQ-5` to cite this ADR.
- **Batch 3 (`xtask`)** — gains one item: the two `cargo check --target *-pc-windows-msvc` gates as a
  required check. Small; it belongs with the other lint gates, not in a Windows batch. **Batch 3 is
  the collision point of five ADRs in this batch** — it also gains `cargo xtask lint-citations`
  (ADR-0008, carrying ADR-0002's `CYRUP-DELTA` check), `cargo xtask upstream-watch` plus the
  `check-citations.py` repair (ADR-0006), and the `CYRUP_SHELL` repo-guard test (ADR-0003 D8(2)).
  Consolidated list in `docs/adr/README.md`; size the batch against all of it, not against this line.
- **Batch 9 ("What `bash` is")** — unchanged membership. `TOOL-038`'s resolution is now specified
  (port the throw, delete the `cmd.exe` fallback) rather than contingent. `TOOL-036` +`DRIFT-046`
  land as one change and now include the `USERPROFILE` half of `home_dir()`.
- **Batch 24 (Intercom A)** — loses `PB-19`'s listen half to the forward-moved Windows bring-up;
  keeps the rest.
- **New: a Windows bring-up batch**, containing (a) `PB-19`'s listen half, (b) the two cross-`check`
  gates, (c) the `rust-toolchain.toml` target declaration. Its acceptance is "`cargo check` is green
  for both Windows targets across the workspace". Size **S–M**; it is one file plus a lint gate.
- **New: `docs/gap-analysis/13-windows-platform.md` + its own batch**, for the ~106-site sweep. Size
  unknown until the sweep runs — that is what a sweep is for, and this ADR does not guess it.

### What does not change

The four items named in `PARITY-PLAN.md:1389` were never the whole question and are not the whole
answer. But nothing that was scheduled gets cancelled, no severity is corrected *down*, and no batch
after 9 is re-ordered by this decision except by the insertion of one small bring-up batch.

## Rejected alternatives

**Out of scope — record it and close the four.** *Cost:* it contradicts the project's stated rule
directly. pi gates its own releases on producing two Windows binaries, ships a Windows setup doc
linked from its README, documents Windows-specific keybindings to end users, maintains a Windows-only
regression test, and ships hand-written C for the Windows console. Declaring Windows out of scope is
not a mechanism difference the language forces — Rust's Windows support is better than Node's — it is
a behaviour deletion, and the rule has no "accepted divergence" category to put it in. It would also
close `PB-19` as WONTFIX while it is, in fact, the reason the binary does not compile for a target
17 of 18 crates already build for: an absurd result. Concretely it would strike `TOOL-036`'s win32
leg, `TOOL-038`, `DRIFT-046` and `PB-19`'s first half, and permanently bless the six behavioural
no-ops in §Context — including a `Ctrl+C`-to-copy that silently does nothing and a subagent kill path
that terminates nothing.

**Tier-2 best-effort.** *Cost:* it is the status quo wearing a label, and the status quo is a crate
that does not compile with an in-source deferral comment standing in for a decision
(`crates/cyrup-intercom/src/paths.rs:6-8`). "Best-effort" gives a reviewer no test to fail and no
gate to go red, so the next `cfg(unix)`-with-no-arm lands unremarked — exactly how ~106 of them got
there. It also has no answer for `TOOL-038`: is a silent `cmd.exe` acceptable at tier 2? Either it is
a divergence or it is not. If the maintainer wants sequencing relief, the honest instrument is batch
order, not a tier — and this ADR already puts the sweep in its own late batch.

**In scope, but only the four named items.** *Cost:* it decides the question and then declines to
believe the answer. The four items do not include the clipboard no-op, the signal no-ops, the
`ctrl+z` binding, or the input drain, none of which any item covers today. Deciding "yes" without
opening the area file produces a port that compiles on Windows and misbehaves on it, with no ledger
row saying so — which is worse than "no", because it is undiscoverable.

**In scope, blocked until Windows CI exists.** *Cost:* neither cyrup nor pi has Windows CI today, and
pi ships Windows anyway from a Linux runner. Making CI a precondition rather than a prerequisite
would block `PB-19` — a six-error compile fix — behind procurement of a runner, and would leave the
whole binary uncompilable in the meantime. Decision (2) takes the useful half of this option (a real,
checkable gate) without the blocking half.

## How to reverse this

**"Windows is not a target for cyrup; strike `PB-19`'s first half, `TOOL-036`'s win32 leg,
`TOOL-038` and `DRIFT-046`, and stop cross-checking."** To be reversible on the evidence rather than
on preference, that would additionally require one of: pi dropping `pi-windows-x64.zip` /
`pi-windows-arm64.zip` from its release asset set in `.github/workflows/build-binaries.yml` and
deleting `packages/coding-agent/docs/windows.md`; or a stated project constraint that cyrup ships
only for the maintainer's own platforms, which no document in this workspace currently contains. A
partial reversal — "in scope, but the sweep never runs" — is the one shape to refuse: it produces a
binary that builds on Windows and quietly misbehaves there, which is the outcome this ADR exists to
prevent.
