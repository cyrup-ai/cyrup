# ADR-0003 — What `bash` is: the interpreter, and what the model may do through it

**Status** accepted (decided by default under the parity rule — overridable)
**Date** 2026-08-13
**Decides** OQ-1 (`PARITY-PLAN.md:1404-1415`) — `TOOL-039` + `TOOL-007` as ONE shell-surface decision
**Blocks released** Batch 9 in full — 14 items: `TOOL-039`, `TOOL-007`, `TOOL-038`, `TOOL-036` (+`DRIFT-046`), `TOOL-031`, `TOOL-040`, `TOOL-037`, `TOOL-020`, `TOOL-026`, `TOOL-030`, `SEAM-015` (+`DRIFT-004`), `DRIFT-029` — and transitively batch 10 (15 items in the same crate, which cannot open while batch 9 is open)

---

## Context

Two questions are entangled in one surface, which is why the plan refuses to take them separately:
**who chooses the interpreter that runs a model-issued command**, and **what, if anything, cyrup
prevents the model from reaching once it has a shell**. Shipped cyrup answers them incompatibly:
`TOOL-007` concedes the protected-path guard is theatre *because* `bash` is undecorated, while
`TOOL-039` shows that the same undecorated `bash` runs under whatever the ambient environment names.

Read on both sides: **cyrup at HEAD `72cd292`** (branch `david/cyrup`; last code-bearing commit
`04c1ba2`, the two commits above it are docs-only) and **pi at v0.83.0**, cyrup's ported tag, with the
v0.84.1 delta checked where it could matter.

### 1. Who chooses the interpreter — upstream

`getShellConfig` (pi v0.83.0 `packages/coding-agent/src/utils/shell.ts:67-120`) takes exactly one
input, `customShellPath`, and reads **no environment variable as a shell selector**:

- `:69-74` — an explicit path is used if it exists, else `throw new Error("Custom shell path not
  found: ${customShellPath}")`. Validation, not silent acceptance.
- `:76-106` — win32: `%ProgramFiles%\Git\bin\bash.exe`, `%ProgramFiles(x86)%\…`, then `where
  bash.exe` (`findBashOnPath`, `:24-43`), then a **throw** carrying a three-option repair recipe
  (`:100-106`).
- `:109-119` — unix: `/bin/bash`, then `which bash` (`:45-57`), then `{ shell: "sh", args: ["-c"] }`.

The only `process.env` reads in the entire file are `ProgramFiles` / `ProgramFiles(x86)` at `:79` and
`:83` — Windows *installation-location* lookups that build the candidate list — and the `PATH`
handling in `getShellEnv` (`:122-134`). The file is **byte-identical at v0.84.1**: `git -C pi diff
v0.83.0 v0.84.1 -- packages/coding-agent/src/utils/shell.ts` is empty, so these offsets hold at either
tag and no drift argument is available.

`customShellPath` has exactly one source upstream: the `shellPath` setting.
`createLocalBashOperations({ shellPath })` → `getShellConfig(options?.shellPath)` (v0.83.0
`core/tools/bash.ts:82`, `:89`), constructed at `bash.ts:320` from `BashToolOptions.shellPath`
(`:192`), which is fed from `getShellPath()` (`core/settings-manager.ts:878-880`, tilde-expanded via
`normalizePath`), declared at `settings-manager.ts:98` as *"Custom shell path (e.g., for Cygwin users
on Windows); supports leading ~ expansion"*.

This is not an accidental gap in upstream's env surface. pi **publishes** its complete
process-configuration environment contract — v0.83.0 `packages/coding-agent/docs/environment-variables.md:70-86`
lists `PI_CODING_AGENT_DIR`, `PI_CODING_AGENT_SESSION_DIR`, `PI_PACKAGE_DIR`, `PI_OFFLINE`,
`PI_SKIP_VERSION_CHECK`, `PI_TELEMETRY`, `PI_CACHE_RETENTION`, `PI_SHARE_VIEWER_URL`,
`PI_HARDWARE_CURSOR`, `VISUAL`/`EDITOR`, `HTTP_PROXY`/`HTTPS_PROXY`. **No interpreter variable is on
that list**, and the bash-tool section (`:15-44`) describes the `PI_*` family as data pi *injects
into* the child, never as knobs the child's parent may use to steer pi. Upstream does read env vars
freely elsewhere (`pi-subagents` v0.43.0 `src/shared/types.ts:1939,1951,1971`; `pi-permission-system`
v0.8.0 `src/permission-manager.ts:32`), so the rule this ADR applies is not "no env vars" — it is that
**no upstream env var selects the interpreter, and cyrup invented one that does**.

### 2. Who chooses the interpreter — cyrup today

`crates/cyrup-tools/src/ops/shell.rs:100-105`:

```rust
pub fn detect() -> Self {
    // `CYRUP_SHELL` is a cyrup-specific override (honored through `get_bash_shell_config` so a
    // WSL-legacy override still selects stdin transport); it has no Pi analogue.
    if let Some(explicit) = std::env::var_os("CYRUP_SHELL") {
        return get_bash_shell_config(PathBuf::from(explicit));
    }
```

Five facts, each re-verified at source for this ADR:

1. **First in resolution order.** The arm at `:101-105` precedes the `/bin/bash` probe (`:109-110`),
   `find_bash_on_path` (`:112-113`) and the `sh -c` fallback (`:115-119`). It beats a healthy
   `/bin/bash`; it is not a fallback that fires only when detection fails.
2. **It is the default path.** `detect()` is what `ToolRegistry::with_builtins` calls
   (`crates/cyrup-tools/src/registry.rs:54`), what `Backend::default()` calls
   (`crates/cyrup-tools/src/ops/mod.rs:357-361`), what `impl Default for ShellConfig` calls
   (`ops/shell.rs:149-153`), and what the real session builder calls
   (`crates/cyrup-session-svc/src/builder.rs:635`). With no `shellPath` setting, this variable **is**
   the interpreter for every model-issued `bash` call.
3. **It is unscrubbed and inherited.** `session_env_scrub_keys()`
   (`crates/cyrup-tools/src/config.rs:41-48`) is built solely from `SESSION_ENV_SUFFIXES`
   (`:31-32` — `SESSION_ID`, `SESSION_FILE`, `PROVIDER`, `MODEL`, `REASONING_LEVEL`) crossed with the
   `CYRUP_`/`PI_` prefixes: ten keys, and `CYRUP_SHELL` **structurally cannot** be one of them. The
   inheritance path is live, not theoretical: a subagent run is a real re-exec of the `cyrup` binary
   (`crates/cyrup-ext-subagents/src/spawn/mod.rs:10`) and that spawn deliberately never clears the
   environment — `tokio::process::Command::new(&spec.command.binary)` at `:489` with `.envs(&spec.env_overlay)`
   at `:493`, guarded by the standing prohibition at `:220` and `:415-418` ("MUST NEVER call
   `env_clear()`"). One export at the top of a parent session governs every descendant.
4. **It is silent.** Nothing writes the resolved program to the transcript, the event stream, session
   start diagnostics or the `bash` result details; `get_bash_shell_config` (`ops/shell.rs:48-54`)
   returns a `ShellConfig` and no caller reports it.
5. **It is not validated.** Unlike the `shellPath` arm (`:89-95`, which reproduces pi's `Custom shell
   path not found` error from `shell.ts:73`), the env value goes straight into
   `get_bash_shell_config`. The substitute need not be a shell, need not exist, and need not be
   executable: `CYRUP_SHELL=/path/to/anything` makes that binary the executor of every model-issued
   command, with the command text as its argument.

`CYRUP_SHELL` appears in exactly one code location workspace-wide (`ops/shell.rs:101-105`); every other
hit is this backlog's own prose. **No test sets it, no CI job sets it, and no user-facing document
mentions it** — deleting it breaks no documented contract and no fixture.

The replacement already exists and works end to end: `shellPath` is read at
`crates/cyrup-config/src/settings.rs:733` (tilde-expanded per `settings-manager.ts:883-886`), threaded
at `crates/cyrup-session-svc/src/builder.rs:633,686` into `BashOpts::shell_path`, re-resolved per
exec at `crates/cyrup-tools/src/tools/bash.rs:224-227` (matching pi resolving inside `exec`,
`bash.ts:85-89`), and separately honoured on the immediate-bash RPC seam at
`crates/cyrup-session-svc/src/session.rs:4500-4505`. It is already covered by tests
(`crates/cyrup-tools/tests/tools.rs:1212`, `crates/cyrup-session-svc/tests/round3.rs:305`).

### 3. What the model may reach through `bash` — the guard

`crates/cyrup-session-svc/src/builder.rs:208` sets `protect_paths: true` in the default
`SessionConfig`; `:643-644` wraps the **fs** backend in `ProtectedFs::with_defaults`; `:646` then
builds `Backend { fs, proc: base.proc.clone() }` — the process seam is passed through untouched. So
`write`/`edit` to `.env`, `.git/` or `node_modules/` are refused
(`crates/cyrup-tools/src/isolation/protected.rs:30-32` defaults, `:51-58` component-equality match,
`:85-92` the `write to protected path denied: …` error), while `bash 'echo K=v >> .env'` succeeds.
`grep -rn protect_paths crates/` returns four lines — `builder.rs:152` (doc), `:153` (declaration),
`:208`, `:643` — so there is **no CLI flag, no settings key, no builder setter and no consumer**: the
default is the only reachable setting in the shipped binary. `crates/cyrup-tools/src/isolation/mod.rs:3-6`
asserts the opposite of that wiring ("by default nothing here is in the call path").

Upstream has no counterpart at the ported tag: pi v0.83.0 `core/tools/write.ts:195-225` resolves the
path and calls `ops.writeFile` with no path predicate of any kind, and `git grep -Ei
"protected|blockedPath" v0.83.0 -- core/tools/{write,edit}.ts` is empty. The nearest upstream concept,
`pi-permission-system` v0.8.0, is a rule-engine extension with no `.env`/`node_modules` defaults
anywhere in `src/permission-manager.ts`.

The model is never told the restriction exists (no description text, no `prompt_guidelines` entry), so
it routes around a refusal via `bash` — which works. The guard therefore costs a failed turn and buys
nothing, which is precisely `TOOL-007`'s own concession.

### 4. The contradiction, stated plainly

"cyrup constrains what the model may reach through `bash`" (TOOL-007's premise) and "cyrup does not
control which interpreter `bash` is" (TOOL-039's finding) cannot both be true. One of them has to go,
and under the standing rule — 1:1 behavioural parity with pi, no accepted-divergence category, a
decision not to port must rest on impossibility or a stated project constraint, never on effort —
**both of them go the same way**, because both are cyrup-original surfaces with no upstream behaviour
being faithfully ported. There is no parity argument for either, and neither is forced by Rust.

---

## Decision

**`bash` is pi's `bash`: the user chooses the interpreter, cyrup chooses nothing, and cyrup imposes no
ambient restriction on what the model may reach through it. The only things that may govern a
model-issued command are the ones pi has — the `shellPath` setting, the per-call `operations` backend
override, and the opt-in permission gate.** Option (i) of OQ-1, both halves.

Implement exactly this, in `cyrup-tools` first, as the opening commits of batch 9:

**D1 — Delete the env-var arm.** Remove `crates/cyrup-tools/src/ops/shell.rs:101-105` in full (the
two-line comment and the three-line `if let`). `detect()` then opens directly on `#[cfg(unix)]` and
reproduces `shell.ts:109-119` / `:76-106` in order, with no cyrup-specific input. Do not replace it
with a differently-named variable, a `CYRUP_SHELL_PATH`, a debug-only arm, or a `#[cfg(test)]` arm.

**D2 — Add nothing to compensate.** No `[CYRUP-DELTA]` stamp (there is no delta left to stamp), no
session-start interpreter report, no `shell` field in the `bash` result details, and no second scrub
group in `config::session_env_scrub_keys()`. All four limbs of option (ii) are struck: they exist only
to make an unported divergence survivable, and reporting an interpreter pi does not report is itself
new surface. `session_env_scrub_keys()` stays exactly as it is at `config.rs:41-48`, the faithful port
of `bash.ts:165-170`.

**D3 — `shellPath` is the sole override, and it must fail loudly.** Keep
`ShellConfig::resolve(Some(p))` (`ops/shell.rs:89-96`) as the only path that accepts a caller-supplied
interpreter, keeping its `Custom shell path not found: {p}` error verbatim (`shell.ts:73`). No new
setting, no new CLI flag, no env alias. Nothing else needs writing here — the setting is already wired
(`settings.rs:733` → `builder.rs:633,686` → `bash.rs:224` → `session.rs:4500`).

**D4 — Silent interpreter substitution is banned everywhere, not just on the env path.** The
`cmd.exe` arm at `ops/shell.rs:140-144` is the same defect wearing a Windows hat — cyrup selecting an
interpreter the user did not choose, with different quoting, redirection and `$VAR` semantics, and
telling no one. `TOOL-038` is therefore decided along with this ADR and in the same direction:
**replace that arm with pi's `No bash shell found. Options: …` error** (`shell.ts:100-106`, including
the searched-path list the probe block already holds at `:125-131`), and do **not** add a `cmd.exe`
opt-in setting — a settings key pi lacks is new divergence, not a mitigation. Implementation note: this
makes detection fallible, which `impl Default for ShellConfig` (`ops/shell.rs:149-153`) and `impl
Default for Backend` (`ops/mod.rs:357-361`) cannot express, so the fallible entry point must be a new
`try_detect() -> Result<ShellConfig, ToolError>` used by the real construction sites
(`registry.rs:54`, `builder.rs:635`, `session.rs:4500`) with the error surfaced at session
construction, not at first `bash` call. This holds **whatever OQ-5 decides about Windows**: if Windows
is in scope the arm is a live defect, and if it is out of scope the arm must still be an error rather
than a silent `cmd.exe`. **OQ-5 has since been decided — `docs/adr/ADR-0007-windows-scope.md` puts
Windows in scope — so the first branch is the live one**, and ADR-0007 independently reaches the same
instruction (its Decision 5). Two notes where the two ADRs are read together: the arm is the struct
literal at `ops/shell.rs:140-144` (ADR-0007 writes `:140-143`, which stops one line short of the
closing brace), and the `try_detect()` fallibility consequence above is this ADR's, not repeated
there — an implementer taking ADR-0007's instruction alone will not discover that
`impl Default for ShellConfig` can no longer express detection.

**D5 — `protect_paths` defaults to `false`.** Flip `crates/cyrup-session-svc/src/builder.rs:208` to
`protect_paths: false`. Default cyrup then matches pi: `write`/`edit` write whatever path they are
given (`write.ts:195-225`). Keep the `SessionConfig::protect_paths` field and the `ProtectedFs`
decorator as an **embedder-only, opt-in** composition point — it is dead by default, so it costs no
behaviour — and specifically do **not** promote it to a CLI flag or a `settings.json` key: pi has
neither, and adding user-visible configuration surface pi lacks is the divergence this rule exists to
prevent. The plan's phrase "behind a flag" is satisfied by the existing builder field; this is a
deliberate narrowing, recorded here with its reason.

**D6 — Make the guard's own documentation true.** Amend `builder.rs:152` to state that the field
decorates the **fs** seam only and that `bash` is not covered by it, and amend
`crates/cyrup-tools/src/isolation/mod.rs:3-6` so it describes the wiring that actually exists.
Stamp the field `[CYRUP-DELTA]` — it is cyrup-original API surface with no pi analogue, even though it
is inert by default. Do **not** decorate `ProcOps` to "fix" the bypass, and do **not** surface the
restriction in the `write`/`edit` descriptions or `prompt_guidelines`: both were limbs of the
keep-it-on branch, which is not taken. `confine_to_cwd` (`:209`) is already `false` and is unchanged.

**D7 — After this ADR, exactly three things may govern a model-issued command, all of them pi's.**
(a) the `shellPath` setting; (b) the per-call `operations` backend override — pi's
`BashToolOptions.operations` (`bash.ts:188`, consumed at `:320`), which is `SEAM-015`'s subject and is the
sanctioned, per-call, extension-supplied way to redirect execution that `CYRUP_SHELL` was pretending
to be; (c) the opt-in permission gate ported from `pi-permission-system`. Nothing ambient, nothing
process-global, nothing on by default. The dead `isolation/policy.rs` helpers (`dangerous_bash_rule`,
`is_dangerous_command`, `protected_path_rule`, zero production consumers) are **not** to be wired as
part of this work; they remain where the backlog put them, in `PARITY-GAPS` §5's deletion candidates.

**D8 — Tests that lock it down.** (1) `ShellConfig::detect()` ignores `CYRUP_SHELL`: set it to a
sentinel, assert the returned `program` is `/bin/bash` (or the `sh` fallback on a machine without it).
(2) A repo guard asserting the literal `CYRUP_SHELL` appears nowhere under `crates/`. (3) A subagent
re-exec launched from a parent with `CYRUP_SHELL` exported resolves its own interpreter unchanged —
the path the env scrub structurally cannot reach, so it must be pinned by test rather than by scrub.
(4) Default chain has no `ProtectedFs`: `write` to `.env` **and** `bash 'echo x >> .env'` both succeed
by default; with `protect_paths: true` set by an embedder, `write` is refused and `bash` still
succeeds — asserted explicitly, so the fs-only scope is executable documentation.
(5) `crates/cyrup-tools/tests/isolation.rs:53-69` (`default_bash_rm_rf_runs_without_any_gate`) is
correct as written and stays. The live check the plan already mandates for batch 9 — `CYRUP_SHELL=/bin/false`
exported into a real session, model-issued `bash` still running under `/bin/bash`, and the same through
a subagent re-exec — remains the acceptance gate.

---

## Consequences

**Batch 9 opens immediately.** Its blocking question is answered; its first two commits are D1+D8(1-3)
in `cyrup-tools` and D5+D6+D8(4) in `cyrup-session-svc`/`cyrup-tools`. **Batch 10 is unblocked
transitively** — it was waiting only on batch 9 vacating the crate.

Item by item, for the ledger:

- **`TOOL-039`** (04, `high`, cyrup-original, S) — decision recorded; scope collapses from "decide,
  then do one of two multi-limb branches" to a **five-line deletion at `ops/shell.rs:101-105` plus
  three tests**. Option (ii)'s limbs (a)-(d) are struck from the item text. Severity and kind stand
  until it lands; it closes with batch 9's first commit. The ledger's row 9
  (`00-residual-ledger.md:111`), `PARITY-GAPS.md:141` row 15 and `PARITY-GAPS.md:819` should be
  re-stated as *decided: option (i), ADR-0003*, and **`PARITY-GAPS` §6 q9** (`PARITY-GAPS.md:836`) is
  answered by this ADR, not open. *(Numbering note: `PARITY-GAPS.md` §6 carries its **own** nine-item
  open-question list whose numbers do not match `PARITY-PLAN.md` §7's. `PARITY-GAPS` §6 q9 **is**
  OQ-1 — the question this ADR decides — while `PARITY-PLAN` §7's OQ-9 is the first-run wizard, which
  is ADR-0011's. An earlier draft of this line read "§5 item 9 / OQ-9" and was wrong twice: the
  section is §6, and the plan-side number is OQ-1. The disambiguation convention is in
  `docs/adr/README.md`.)*
- **`TOOL-007`** (04, `medium`, cyrup-original) — **effort M → S**. Fix becomes: one-line default flip
  at `builder.rs:208`, two doc corrections (`builder.rs:152`, `isolation/mod.rs:3-6`), a
  `[CYRUP-DELTA]` stamp, and the D8(4) tests. The three "keep it on" limbs — surface it in
  `write`/`edit` descriptions, decorate `ProcOps`, add the prompt guideline — are struck; only the doc
  correction survives from that branch. The item is fully closed by this work: the divergence is gone
  at the default, and the residual ("an opt-in fs decorator does not cover the process seam") is
  documented rather than hidden, which is the whole of what was wrong with it.
- **`TOOL-038`** (04, `medium`, cyrup-original) — **scope fixed, not re-rated**: pi's error text, no
  `cmd.exe` escape-hatch setting, and detection becomes fallible via `try_detect` with the failure
  surfaced at session construction. Its dependence on OQ-5 is removed — the arm is deleted under
  either Windows answer. It ships in batch 9 next to `TOOL-039` because it is the same rule.
- **`TOOL-036` / `DRIFT-046`** (04 / 12, `low`) — **no severity change**, one scope note: with
  `CYRUP_SHELL` gone, `shellPath` is the *only* user-side interpreter lever, so the Windows path
  normalization is the only remaining way a Windows user's configured shell can be mis-resolved. Its
  ranking follows OQ-5, not this ADR — and **OQ-5 is now decided**: `ADR-0007` puts Windows in scope,
  refuses the strike, and keeps `TOOL-036` at `low` in batch 9. The "still follows OQ-5" phrasing here
  is discharged, not pending.
- **`SEAM-015`** (08, `medium`, not-ported) — **no severity change**, but its meaning sharpens: the
  per-call `operations` override is now the *only* sanctioned way anything other than the locally
  resolved shell executes a bash call, so it is the migration target for any user who was reaching for
  `CYRUP_SHELL`. Its wire-side work reads a settled interpreter rule instead of guessing at one.
- **`PERM-009`** (10, `critical`) — unchanged in severity or scope, but this ADR makes it
  **load-bearing**: after D5, the permission gate is the only thing that can stop a model-issued
  command, so a defeated `tools.bash: deny` is the last remaining control failing. Batch 7's deletion
  of the bash bypass must land; nothing in this ADR substitutes for it.
- **`isolation/policy.rs`'s dead helpers** — explicitly not resurrected by this decision; they stay
  `PARITY-GAPS` §5 deletion candidates and can never be reached by default under D5.
- **`PARITY-PLAN.md` §7 OQ-1 and §6 row for batch 9** — answered; batch 9's "Risk" paragraph about
  half-(ii) becomes moot, since (ii) is rejected entirely.

**Behavioural cost accepted, stated openly.** (1) Anyone relying on `CYRUP_SHELL` in a wrapper script
or CI job loses it and must move to `shellPath` in settings — nothing in the tree or in any user
document advertises the variable, so the exposure is limited to someone who read `ops/shell.rs`.
(2) After D5, cyrup no longer refuses `write`/`edit` to `.env`, `.git/` or `node_modules/`, which is
exactly pi's behaviour and exactly what `bash` could already do; users who want that protection use
the permission system, as pi's users do.

---

## Rejected alternatives

**Option (ii) — keep `CYRUP_SHELL` with all four limbs** (`[CYRUP-DELTA]` stamp; interpreter reported
at session start and in bash result details; a second, explicitly-named scrub group because it cannot
fit the `{CYRUP,PI}_<SUFFIX>` shape; `shell.ts:73` path validation). Cost: it keeps an
ambient-authority interpreter selector that upstream deliberately does not have, and pays for it with
**four permanent divergences instead of one** — a transcript field pi does not emit, a session-start
diagnostic line pi does not print, a scrub table that no longer derives from `SESSION_ENV_SUFFIXES`
(so every future session variable has two places to be registered), and a maintenance burden on every
subsequent `bash` change. It is more code, more surface and more drift than the thing it protects. The
only argument for it is convenience for a user who cannot edit `settings.json`, and that user is
served by `shellPath` — including on the exact use case the code comment cites, WSL-legacy stdin
transport, which `get_bash_shell_config` handles identically from the setting.

**Half of option (ii)** — the stamp without the reporting, or the scrub group without the validation.
Rejected as a matter of record: a reviewer must reject it. A stamped divergence that is still silent
and still inherited is the current defect with a comment attached.

**Keep `protect_paths: true` and close the bypass by decorating `ProcOps`.** Cost: it requires
deciding, from command text alone, whether an arbitrary shell command mutates a protected path — a
problem that has no correct solution (`sh -c 'e''cho x > .env'`, `python -c …`, `sed -i`, a script
that writes it) and whose failure modes are both false refusals on legitimate commands and a guard
users believe in that still does not hold. It also invents a restriction pi does not have, on by
default, that the model is never told about, so the first-order symptom (`TOOL-007`'s wasted turn)
survives the fix. cyrup already has the dead-code corpse of this approach in
`isolation/policy.rs::is_dangerous_command`, with zero consumers.

**Promote `protect_paths` to a CLI flag or a `settings.json` key.** Cost: new user-visible surface
with no pi analogue — a `--help` line and a settings key that diverge from upstream forever, and which
must then be documented, migrated and drift-checked. The embedder-facing `SessionConfig` field already
covers every real consumer of an opt-in isolation decorator.

**Delete `ProtectedFs` / `ProtectedPaths` outright.** Cost: it discards a composable, correctly-written
`FsOps` decorator that an SDK embedder can legitimately want, for no parity gain — once the default is
`false` it is not in any call path and creates no divergence. Its fate belongs to the `PARITY-GAPS` §5
deletion sweep with the rest of the unused isolation code, not here.

**Defer the decision to a maintainer.** Cost: 29 items across two batches stay blocked, and the two
contradictory behaviours keep shipping together — which is the failure mode this ADR batch exists to
end.

---

## How to reverse this

> *"Keep `CYRUP_SHELL` — I need to redirect the interpreter without touching `settings.json`."*

Then option (ii) applies in full and all four limbs are mandatory in the same change: a
`[CYRUP-DELTA]` stamp at `ops/shell.rs:101-105`; the resolved interpreter reported both at session
start and in every `bash` result's details; a second, explicitly-named scrub group in
`config::session_env_scrub_keys()` (`config.rs:41-48`) plus the D8(3) subagent test proving it is
scrubbed; and existence/executability validation matching `shell.ts:73`. Half of that is still not an
option, and D5/D6 are unaffected — the protected-path half reverses only on a separate instruction
(*"keep the protected-path block on by default"*), which would additionally require pi to grow a
protected-path concept, or an explicit acceptance that cyrup refuses writes pi permits.
