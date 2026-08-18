# SUBA-072 — `.cyrup-subagent-scratch/` is written into the PROJECT working tree instead of the scoped `~/.cyrup` (or OS temp) root

> **Status** — OPEN. **Kind** `port-bug` · **Severity** medium · **Effort** S.
> Filed 2026-08-18 from a real project checkout: `git status` on `cyrup/` itself showed
> `.cyrup-subagent-scratch/` and `.cyrup-subagents/` as untracked directories after ordinary
> `/flux/*` subagent-tool use in this very repo.

---

## 1. Two directories, two different verdicts — do not conflate them

The report that prompted this filing named both `.cyrup-subagent-scratch/` and `.cyrup-subagents/`
as suspicious. They are **not** the same kind of thing, and only one is a bug:

| directory | verdict | why |
|---|---|---|
| `.cyrup-subagents/` (missions, project-scoped artifacts) | **NOT a bug — working as designed** | This is the direct, faithful port of pi-subagents' own `PROJECT_ARTIFACT_ROOT = ".pi-subagents"` (`pi-subagents/src/shared/artifacts.ts:6`), which pi's own `ArtifactConfig.artifactDir` doc calls the **default**: *"Defaults to `project` (cwd/.pi-subagents)"* (`shared/types.ts:1816`). Mission storage matches: `pi-subagents/src/missions/store.ts:262` resolves `path.join(projectRoot, ".pi-subagents", "missions")` — project-scoped by design, not accidentally. cyrup's `PROJECT_ARTIFACT_ROOT` const (`crates/cyrup-ext-subagents/src/artifacts.rs:35`, `".cyrup-subagents"`) and `missions/store.rs`'s resolution are correct, 1:1 ports. Upstream itself tells users to `.npmignore`/`.gitignore` it (`shared/artifacts.ts:130`: *"Add '.pi-subagents/' to .npmignore…"*) — the fix here is a `.gitignore` entry, not a code change (see §4).
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
([`crates/cyrup-ext-subagents/src/extension.rs:2380-2394`](../../../crates/cyrup-ext-subagents/src/extension.rs#L2380)):

> *"the per-attempt raw-stdout tee `run_sync` writes to `<cwd>/.cyrup-subagent-scratch/attempt-<n>.jsonl`
> is this run's persisted, observable child record... This mirrors pi, which likewise never deletes
> its persisted child NDJSON stream — pi only cleans the transient per-spawn prompt/task-overflow dir
> it creates under `os.tmpdir()` (`pi-subagents/src/runs/shared/pi-args.ts:143-158` build it, `:233-236`
> `cleanupTempDir` removes it, invoked from `pi-subagents/src/runs/foreground/execution.ts:1109`),
> **a dir that lives OUTSIDE the working tree** and never holds the event stream."*

That comment is correct about what pi does and correct that cyrup's tee must not be deleted — but it
elides the actual gap: **pi's transient dir lives under `os.tmpdir()`; cyrup's persisted tee lives
under the project's own working tree.** Verified directly against upstream:

- [`pi-subagents/src/runs/shared/pi-args.ts:571`](../../../pi-subagents/src/runs/shared/pi-args.ts) (and `:590`, `:604`, `:613`):
  `tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "pi-subagent-"))` — every one of pi's per-spawn
  scratch artifacts (prompt spill, task-overflow spill, tool-diagnostic path, permission-audit
  fallback) is created under the OS temp directory, never under the project `cwd`.
- `pi-subagents/src/runs/shared/pi-args.ts:171-176`'s `supervisorChannelDir` likewise resolves under
  `TEMP_ROOT_DIR` (`shared/types.ts:1902`, `path.join(os.tmpdir(), \`pi-subagents-${resolveTempScopeId()}\`)`)
  — **the same root the CLAUDE.md-documented `04c1ba2` fix already ports faithfully** for cyrup's own
  async/results/chain-runs/artifacts roots.

**cyrup already has the correct, established resolution function** — it is simply not used here:
[`crates/cyrup-ext-subagents/src/background/mod.rs:1425-1436`](../../../crates/cyrup-ext-subagents/src/background/mod.rs#L1425):

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
(`crates/cyrup-ext-subagents/src/artifacts.rs:18-20`). `exec/mod.rs:3858` is the one place in the
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
the same root.) Update the three tests that assert on `.cyrup-subagent-scratch` living under a
**tempdir-based project root** (`exec/mod.rs:7770,7791,7845`, `background/runner_main.rs:3710`,
`extension.rs:13778`) to assert on the new global-root path instead — they already construct their
project dir with `tempfile::TempDir`, so only the joined suffix path changes, not the fixture shape.

**Do not touch**: the retention behavior itself (the tee must survive the run, per the in-source
comment at `extension.rs:2380-2394` — this fix only relocates where the surviving file lives, it does
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
* All existing scratch-dir assertions in `exec/mod.rs`, `background/runner_main.rs` and
  `extension.rs` are updated to the new path and stay green.

## Verify

```bash
cd /tmp/some-project && git init -q
CYRUP_HOME=/tmp/cyrup-home cyrup ... # run any /run or /subagents-* single that spawns a real child
ls -la /tmp/some-project | grep -c cyrup-subagent-scratch   # expect 0
ls -la /tmp/cyrup-home/.cyrup/subagents/scratch             # expect the run's attempt-N.jsonl here instead
```

## Cross-references

- Same root cause class as the `~/.cyrup` leak this workspace's `CLAUDE.md` already documents as
  fixed in `04c1ba2` — that fix taught `temp_root_dir`/async/results/artifacts roots to agree; this
  item is the one remaining call site (`exec/mod.rs:3858`) that predates or bypassed that
  consolidation.
- Not a residual of any existing `SUBA-*` row — `grep -rn "scratch" docs/gap-analysis/09-cyrup-ext-subagents.md`
  turns up only an unrelated 0600-permissions item (`SUBA-030`, closed) and an unrelated diagnostic-path
  Fix note; this is a new, previously unfiled item.
