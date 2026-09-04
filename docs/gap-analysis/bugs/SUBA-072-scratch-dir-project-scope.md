# SUBA-072 — `.cyrup-subagent-scratch/` is written into the PROJECT working tree instead of the scoped `~/.cyrup` (or OS temp) root

> **Status** — ~~OPEN~~ **CLOSED 2026-09-04, cyrup `7791b26a`.** **Kind** `port-bug` · **Severity** medium · **Effort** S.
> Landed exactly as §3 prescribes, with the resolution given a name: `crate::background::attempt_scratch_dir(cwd)`
> (`background/mod.rs:1493`; pure core `attempt_scratch_dir_in(&Roots, cwd)` at `:1501`, leaf
> `SCRATCH_SUBDIR = "scratch"` at `:1237`) → `<Roots::run_scratch>/scratch/<cwd_key>`; the one call
> site is `exec/mod.rs:827` (`prepare_ladder`). Upstream anchors re-read at **v0.64.0** (ADR-0006
> parity target; the v0.43.0 anchors below remain correct for the baseline): per-spawn scratch
> `mkdtempSync(path.join(os.tmpdir(), "pi-subagent-"))` at `src/runs/shared/pi-args.ts:787`/`:802`/
> `:826`/`:841`/`:855`, `cleanupTempDir` at `:1052-1059`, `TEMP_ROOT_DIR` at `src/shared/types.ts:2689-2691`
> (v0.64.0 adds a `PI_SUBAGENTS_TEMP_ROOT` override at `:2688` — CFG-067's item, not this one).
> Pinned by `exec::tests::prepare_ladder_makes_the_scratch_dir_under_the_run_scratch_root_not_the_project_tree`
> (red against the pre-fix call site) and `background::tests::attempt_scratch_dir_is_a_cwd_keyed_leaf_of_the_run_scratch_root_never_the_project_tree`;
> the §3 consumer list was still short by three (`child_protocol_stream_integration`,
> `background_runner_main_integration`, intercom `child_bridge_activation`) — all eight `cyrup-it`
> readers now go through the public resolver. Residual: `.gitignore:20` is dead and can go.
> Filed 2026-08-18 from a real project checkout: `git status` on `cyrup/` itself showed
> `.cyrup-subagent-scratch/` and `.cyrup-subagents/` as untracked directories after ordinary
> `/flux/*` subagent-tool use in this very repo.
>
> **RE-VERIFIED 2026-08-19 at HEAD (`4fb5e40`) — STILL OPEN, unchanged.**
> `crates/cyrup-ext-subagents/src/exec/mod.rs:3858` is verbatim
> `let scratch_dir = opts.cwd.join(".cyrup-subagent-scratch");`, and
> `crate::background::temp_root_dir()` is still not called from it. **`e4c0a20` — the commit that
> filed this row — added `.gitignore:16`/`:20` and nothing else: the ignore entry hides the symptom
> and closes nothing.** The fix is still the one call-site change in §3. Upstream anchors below were
> re-read with `git -C ../pi-subagents show v0.43.0:<path>` (the ported baseline recorded in
> `09-cyrup-ext-subagents.md:5`), **not** from a working tree — that checkout is a SIBLING of this
> repo, so `../../../pi-subagents/…` resolved to nothing. Two upstream anchors were wrong (both in
> `shared/types.ts`), one markdown link was dead, and §3's test list was missing seven consumers in
> `cyrup-it`; each is corrected in place and marked.

---

## 1. Two directories, two different verdicts — do not conflate them

The report that prompted this filing named both `.cyrup-subagent-scratch/` and `.cyrup-subagents/`
as suspicious. They are **not** the same kind of thing, and only one is a bug:

| directory | verdict | why |
|---|---|---|
| `.cyrup-subagents/` (missions, project-scoped artifacts) | **NOT a bug — working as designed** | This is the direct, faithful port of pi-subagents' own `PROJECT_ARTIFACT_ROOT = ".pi-subagents"` (`pi-subagents/src/shared/artifacts.ts:6`), which pi's own `ArtifactConfig.artifactDir` doc calls the **default**: *"Defaults to `project` (cwd/.pi-subagents)"* (`shared/types.ts:1776`, the doc comment above the `artifactDir?: ArtifactDirPreference` field at `:1777` — **CORRECTED 2026-08-19 from `:1816`**, which at v0.43.0 is `env?: NodeJS.ProcessEnv;`). Mission storage matches: `pi-subagents/src/missions/store.ts:262` resolves `path.join(projectRoot, ".pi-subagents", "missions")` — project-scoped by design, not accidentally. cyrup's `PROJECT_ARTIFACT_ROOT` const (`crates/cyrup-ext-subagents/src/artifacts.rs:35`, `".cyrup-subagents"`) and `missions/store.rs`'s resolution are correct, 1:1 ports. Upstream itself tells users to `.npmignore`/`.gitignore` it (`shared/artifacts.ts:130`: *"Add '.pi-subagents/' to .npmignore…"*) — the fix here is a `.gitignore` entry, not a code change (see §4). **Drift note, 2026-08-19 — does not change this verdict, but a later pass must not read it as one:** upstream MOVED this directory after the ported baseline. `c386b25` (*"feat: move project storage under pi dir"*, 2026-08-11), first released in **v0.47.0**, renames the constant to `PROJECT_SUBAGENTS_RELATIVE_DIR = ".pi/subagents"` (`shared/artifacts.ts:6` @v0.47.0) and routes missions through `getProjectSubagentsDir(projectRoot)` (`missions/store.ts:294` @v0.47.0). cyrup's `.cyrup-subagents` is a faithful port **of v0.43.0**, which is what this row measures against; the v0.43.0→v0.47.x rename is a separate upstream-drift item for area 09, **not** part of this fix. |
| `.cyrup-subagent-scratch/` (per-attempt raw-stdout tee) | **BUG — genuine port divergence** | See §2. |

## 2. The bug: `.cyrup-subagent-scratch/` should never be `<cwd>/…`

**cyrup, at HEAD:**
[`crates/cyrup-ext-subagents/src/exec/mod.rs:3858`](../../../crates/cyrup-ext-subagents/src/exec/mod.rs#L3858):

```rust
let scratch_dir = opts.cwd.join(".cyrup-subagent-scratch");
if let Err(err) = std::fs::create_dir_all(&scratch_dir) {
    ...
}
```

`opts.cwd` is the **project working directory** — the same `cwd` every other artifact root in this
crate keys off of. This one line is the sole production call site that builds it.

**pi's own admission of the divergence is already sitting in cyrup's source**, immediately after the
run this scratch dir belongs to settles
([`crates/cyrup-ext-subagents/src/extension.rs:2381-2395`](../../../crates/cyrup-ext-subagents/src/extension.rs#L2381) — **line range corrected 2026-08-19**; `:2380` is blank):

> *"the per-attempt raw-stdout tee `run_sync` writes to `<cwd>/.cyrup-subagent-scratch/attempt-<n>.jsonl`
> is this run's persisted, observable child record... This mirrors pi, which likewise never deletes
> its persisted child NDJSON stream — pi only cleans the transient per-spawn prompt/task-overflow dir
> it creates under `os.tmpdir()` (`pi-subagents/src/runs/shared/pi-args.ts:143-158` build it, `:233-236`
> `cleanupTempDir` removes it, invoked from `pi-subagents/src/runs/foreground/execution.ts:1109`),
> **a dir that lives OUTSIDE the working tree** and never holds the event stream."*

That comment is correct about *what* pi does and correct that cyrup's tee must not be deleted —
**though two of its own upstream anchors are stale and should not be copied forward**: at v0.43.0 the
per-spawn temp dir is built at `pi-args.ts:571`/`:590`/`:604`/`:613` (not `:143-158`, which is a type
declaration), `cleanupTempDir` is defined at `pi-args.ts:791` (not `:233-236`), and the invocation is
`foreground/execution.ts:1110` (not `:1109`). The §2 citations below are the correct ones. What the
comment elides is the actual gap: **pi's transient dir lives under `os.tmpdir()`; cyrup's persisted tee lives
under the project's own working tree.** Verified directly against upstream:

- `pi-subagents/src/runs/shared/pi-args.ts:571` (and `:590`, `:604`, `:613`) @**v0.43.0** — read with
  `git -C ../pi-subagents show v0.43.0:src/runs/shared/pi-args.ts`, since that checkout is a SIBLING
  of this repo, not a subdirectory of it (the previous markdown link resolved to nothing):
  `tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "pi-subagent-"))` — every one of pi's per-spawn
  scratch artifacts (prompt spill, task-overflow spill, tool-diagnostic path, permission-audit
  fallback) is created under the OS temp directory, never under the project `cwd`.
- `pi-subagents/src/runs/shared/pi-args.ts:171-176`'s `supervisorChannelDir` likewise resolves under
  `TEMP_ROOT_DIR` (`shared/types.ts:1862` @v0.43.0 — **CORRECTED 2026-08-19 from `:1902`**, which is
  blank there; the four sibling roots follow it at `:1863-1866`),
  `path.join(os.tmpdir(), \`pi-subagents-${resolveTempScopeId()}\`)`
  — **the same root the CLAUDE.md-documented `04c1ba2` fix already ports faithfully** for cyrup's own
  async/results/chain-runs/artifacts roots.

**cyrup already has the correct, established resolution function** — it is simply not used here:
[`crates/cyrup-ext-subagents/src/background/mod.rs:1425-1437`](../../../crates/cyrup-ext-subagents/src/background/mod.rs#L1425) (`temp_root_dir` at `:1425`, its pure core `temp_root_dir_from` at `:1432`):

```rust
pub(crate) fn temp_root_dir() -> PathBuf {
    temp_root_dir_from(&|key| std::env::var(key).ok(), std::env::temp_dir())
}

fn temp_root_dir_from(env: &dyn Fn(&str) -> Option<String>, os_temp_dir: PathBuf) -> PathBuf {
    if let Some(sandbox) = env("CYRUP_HOME").filter(|v| !v.trim().is_empty()) {
        return PathBuf::from(sandbox).join(".cyrup").join("subagents");
    }
    os_temp_dir.join(format!("cyrup-subagents-{}", resolve_temp_scope_id()))
}
```

This is exactly the "global, not project" root the report expected — `<CYRUP_HOME|HOME>/.cyrup/subagents`
when `CYRUP_HOME` is set, else the OS temp dir keyed by scope, matching pi's `TEMP_ROOT_DIR` shape.
`crate::artifacts` already reuses it via `cwd_key` so a project's artifacts/chain-runs/async/results
roots all live together under **one per-`cwd` scope inside this global root**
(`crates/cyrup-ext-subagents/src/artifacts.rs:20-22` — **CORRECTED 2026-08-19 from `:18-20`**; `cwd_key` itself is `background/mod.rs:1450`). `exec/mod.rs:3858` is the one place in the
crate that bypasses this convention and writes straight into the working tree instead.

## 3. Fix

Replace the one call site:

```rust
// was:
let scratch_dir = opts.cwd.join(".cyrup-subagent-scratch");

// should be — reuse the crate's own established per-cwd scoping under the global root:
let scratch_dir = crate::background::temp_root_dir()
    .join("scratch")
    .join(crate::background::cwd_key(&opts.cwd));
```

(`temp_root_dir`/`cwd_key` are already `pub(crate)`, so no visibility change is needed; `"scratch"`
is a new leaf alongside the existing `"artifacts"`/`"chain-runs"`/`"async"`/`"results"` leaves under
the same root.) Update the **five in-crate absence assertions** — `exec/mod.rs:7770`, `:7791`,
`:7845`, `background/runner_main.rs:3710`, `extension.rs:13778` — to assert on the new global-root
path instead; they already construct their project dir with `tempfile::TempDir`, so only the joined
suffix path changes, not the fixture shape.

**AMENDED 2026-08-19 — the Fix's test list was incomplete, and the missing half is the harder half.**
`grep -rn 'cyrup-subagent-scratch' crates/` finds **seven more consumers in a DIFFERENT crate**, and
four of them do not merely assert absence — they *read the tee back* from
`<cwd>/.cyrup-subagent-scratch/attempt-0.jsonl` and are the crate's stated observation channel (the
in-source comment at `extension.rs:2385-2387` names two of them by name):
`crates/cyrup-it/tests/subagents/companions_wiring_proof.rs:183`,
`.../subagent_persona_and_depth_integration.rs:82` (+ absence at `:706`),
`.../artifacts_run_integration.rs:185`, `.../tool_parallel_chain_integration.rs:93` (+ absence at
`:319`), and `.../exec_run_sync_integration.rs:453` (absence). **Every one of those readers breaks
the moment the directory moves**, so the fix is a one-line change plus twelve assertion updates
across two crates — still `S`, but not "three tests".

**Do not touch**: the retention behavior itself (the tee must survive the run, per the in-source
comment at `extension.rs:2381-2395` — this fix only relocates where the surviving file lives, it does
not add or remove a cleanup path), `.cyrup-subagents/` (missions/artifacts — correct as-is, §1), and
`crate::artifacts`'s existing `PROJECT_ARTIFACT_ROOT` constant/behavior (unrelated, also correct).

## Definition of done

* `exec/mod.rs`'s scratch-dir construction resolves under `crate::background::temp_root_dir()`
  (global, `CYRUP_HOME`/OS-temp scoped), keyed by `cwd_key(&opts.cwd)`, never `opts.cwd.join(..)`
  directly.
* A fresh subagent run in a project checkout with `CYRUP_HOME` unset leaves **no**
  `.cyrup-subagent-scratch/` directory anywhere under the project's working tree.
* The persisted per-attempt NDJSON tee (`attempt-<n>.jsonl`) still survives the run exactly as
  before — only its parent directory moves.
* All existing scratch-dir assertions are updated to the new path and stay green — **in both
  crates**: `cyrup-ext-subagents` (`exec/mod.rs`, `background/runner_main.rs`, `extension.rs`) and
  `cyrup-it` (`tests/subagents/{companions_wiring_proof, subagent_persona_and_depth_integration,
  artifacts_run_integration, tool_parallel_chain_integration, exec_run_sync_integration}.rs`), whose
  tee READS are the crate's observation channel and would otherwise all fail.

## Verify

```bash
cd /tmp/some-project && git init -q
CYRUP_HOME=/tmp/cyrup-home cyrup ... # run any /run or /subagents-* single that spawns a real child
ls -la /tmp/some-project | grep -c cyrup-subagent-scratch   # expect 0
ls -R  /tmp/cyrup-home/.cyrup/subagents/scratch             # expect <cwd_key>/attempt-N.jsonl here instead
```

(`cwd_key` adds one level under `scratch/`, matching the existing `artifacts`/`chain-runs`/`async`/
`results` leaves — so the tee lands at `.cyrup/subagents/scratch/<cwd_key>/attempt-N.jsonl`, not
directly under `scratch/`.)

## Cross-references

- Same root cause class as the `~/.cyrup` leak this workspace's `CLAUDE.md` already documents as
  fixed in `04c1ba2` — that fix taught `temp_root_dir`/async/results/artifacts roots to agree; this
  item is the one remaining call site (`exec/mod.rs:3858`) that predates or bypassed that
  consolidation.
- Not a residual of any existing `SUBA-*` row — `grep -rn "scratch" docs/gap-analysis/09-cyrup-ext-subagents.md`
  turns up only an unrelated 0600-permissions item (`SUBA-030`, closed) and an unrelated diagnostic-path
  Fix note; this is a new, previously unfiled item.
- **`e4c0a20` is this row's filing commit, not its fix.** Its three-file diff is `.gitignore` (+10),
  `09-cyrup-ext-subagents.md` (+8, the cross-reference row) and this file (+137). No `crates/` file
  was touched. `.gitignore:16` (`.cyrup-subagents/`) is the correct, upstream-sanctioned treatment
  per §1; `.gitignore:20` (`.cyrup-subagent-scratch/`) only stops the divergence showing up as
  untracked noise — **the directory is still created in the user's working tree on every subagent
  run, which is what this row is about.**
