---
stage: qa
status: completed
updated: 2026-09-15 18:00
---

# SCOPE_15 — scheduled runs, part A: store and schedule model

OBJECTIVE: build the persistence half of `background/scheduled-runs.ts` (979 LOC @ `7fe9dee1`) — the
schedule record, its on-disk store, and the capability-ceiling gate that governs whether a schedule
may be persisted at all. **Unwired**: no trigger loop, no tool surface. SCOPE_16 adds those.

**Status after augmentation: NOT implemented.** `grep -rn "scheduled_run\|ScheduledRun"
crates/cyrup-ext-subagents/src --include=*.rs` returns zero hits. The only pre-existing traces of the
feature in the tree are three *anticipatory* seams that this task does not change:

* `registration/authority.rs:32,43,56,73` — `AuthorityAction::ScheduleCreate` and its
  `for_tool_action("schedule.create")` mapping already exist, with the doc at `:66-67` saying
  outright *"`schedule.create` is mapped here even though cyrup does not dispatch it yet
  (SUBA-016), so the verb arrives already gated rather than needing a second change to gate it."*
  **SCOPE_16 consumes this. Part A must not touch `registration/authority.rs`.**
* `registration/tool_description.rs:93-97,713-722` — the four `schedule*` verbs are deliberately
  DELETED from the tool description, with the reason `"scheduledRuns` is unported (SUBA-016)"`.
  **SCOPE_16 restores that line. Part A leaves it deleted — the module is unwired.**
* `exec/capability_ceiling.rs` — the ceiling this task's gate wires to (see SUBTASK3).

---

## 0. UPSTREAM PIN — read, verified, and drifting

Read with `git -C /home/user/cyrup/tmp/pi-subagents show <rev>:src/runs/background/scheduled-runs.ts`.

| rev | kind | LOC | status |
|---|---|---|---|
| `7fe9dee1` | commit, `2026-09-05`, *"fix(runtime): keep unconfigured prompt-runtime loads inert (#1984)"* | **979** | the draft's pin; `git describe` = `v0.65.1-50-g7fe9dee1`, i.e. BETWEEN tags |
| `v0.66.0` / `v0.67.0` | tags containing `7fe9dee1` | **1006** | same shapes, drifted line numbers |

`src/runs/shared/capability-ceiling.ts` — the file supplying `ResolvedSubagentCapabilityCeiling` —
**exists at both revs** (`git cat-file -e` succeeds). It is already ported; see §1.4.

**`7fe9dee1` is not a tag.** Every citation in this spec that carries a bare `:NNN` is against
`7fe9dee1` and is only valid there. The drift that matters:

| what | `7fe9dee1` | `v0.67.0` |
|---|---|---|
| `resolveCapabilityCeiling?:` dep field | `:86` | `:87` |
| `scheduledRunStorePath` | `:98` | `:99` (identical body) |
| `getSessionId() ?? "unknown"` | `:619` | `:623` |
| the refusal `textResult(...)` | `:620` | `:624` |
| the session-proxy `getSessionId` capture (SCOPE_16's `:466-470`) | `:466-470` | shifted |
| `ScheduleRecord` | `:45-62` | `:46-64`, **plus a new `quiet?: boolean` after `ownerSessionFile`** |

The draft's `:86`, `:98`, `:466-470`, `:619-620` are all **confirmed correct at `7fe9dee1`**. The only
upstream shape change between the pin and the newest tag is `quiet?: boolean` on `ScheduleRecord`.
**Port the `7fe9dee1` shape.** Do not add `quiet` — it is consumed only by the tool surface's
result-text suppression, which is SCOPE_16's and post-dates the pin.

Verbatim, from `7fe9dee1`:

```ts
// :86
resolveCapabilityCeiling?: (sessionId: string) => ResolvedSubagentCapabilityCeiling | undefined;

// :98-102
export function scheduledRunStorePath(cwd: string, _sessionId?: string, root?: string): string {
	if (!root) return path.join(getProjectSubagentsDir(path.resolve(cwd)), "schedules");
	const projectKey = createHash("sha256").update(path.resolve(cwd)).digest("hex").slice(0, 20);
	return path.join(root, projectKey);
}

// :619-620
const sessionId = ctx.sessionManager.getSessionId() ?? "unknown";
if (this.deps.resolveCapabilityCeiling?.(sessionId)) return textResult("Cannot persist a schedule while a capability ceiling is active.", undefined, undefined, true);
```

---

## 1. SEAM INVENTORY — every touched surface, re-opened and re-read

Each of the following was opened in the working tree on this branch (`claude/subagents-scope-3`,
cut from `b2fdc7e`, i.e. after PR #137/#139/#134). Nothing here is carried forward from the draft
unchecked.

### 1.1 `crate::artifacts::project_subagents_dir` — the `getProjectSubagentsDir` port EXISTS

`crates/cyrup-ext-subagents/src/artifacts.rs:153-157`:

```rust
/// `<cwd>/.cyrup-subagents` (pi `getProjectSubagentsDir`, `shared/artifacts.ts:133-135`).
#[must_use]
pub fn project_subagents_dir(cwd: &Path) -> PathBuf {
    cwd.join(PROJECT_ARTIFACT_ROOT)
}
```

with `const PROJECT_ARTIFACT_ROOT: &str = ".cyrup-subagents";` at `artifacts.rs:36`. Its two
siblings — `project_artifacts_dir` (`:161`) and `project_chain_runs_dir` (`:167`) — are the exact
`getProjectArtifactsDir`/`getProjectChainRunsDir` ports, so the `<cwd>/.cyrup-subagents/<leaf>`
family already exists and `schedules` is a fourth leaf on it. Upstream `getProjectSubagentsDir` was
re-read at `7fe9dee1` (`src/shared/artifacts.ts:129-131`) and is a bare `path.join(cwd,
PROJECT_SUBAGENTS_RELATIVE_DIR)` — the port is faithful.

**Use this function. Do not re-derive `.cyrup-subagents`.**

### 1.2 The run-scratch root family — the WRONG home for a schedule, and why

`background/artifact_roots.rs` owns the per-cwd scratch layout:

| item | line | resolves to |
|---|---|---|
| `temp_root_dir_from(env, os_temp_dir)` | `:237-250` | `CYRUP_HOME`/`.cyrup/subagents`, **else `<os-temp>/cyrup-subagents-<scope>`** |
| `cwd_key(cwd) -> String` | `:263-269` | `DefaultHasher` of the path, `{:016x}` — **not** sha256 |
| `run_artifact_roots_in(roots, cwd)` | `:318-326` | `<run_scratch>/async/<key>`, `<run_scratch>/results/<key>` |
| `wait_subscriptions_dir_in(roots, cwd)` | `:334-341` | `<run_scratch>/wait-subscriptions/<cwd_key>` |
| `attempt_scratch_dir_in(roots, cwd)` | `:365-368` | `<run_scratch>/scratch/<cwd_key>` |
| subdir consts | `:27,35,47,64` | `async`, `results`, `scratch`, `wait-subscriptions` |

`background/wait_subscriptions/mod.rs:57-82` argues at length for adding *"a fourth cwd-keyed
sibling"* under the run-scratch root. **That precedent must NOT be followed here, and the reason is
the first-order design fact of this task:**

> `temp_root_dir_from`'s own doc (`artifact_roots.rs:227-235`) calls that tree *"reboot-disposable
> run scratch"*, and with `CYRUP_HOME` unset — *"its only state outside tests"* — it resolves under
> `std::env::temp_dir()`. A schedule whose whole purpose is to fire in six hours, tomorrow, or next
> week cannot live in a directory the OS is entitled to clear on reboot or by a tmp reaper.

Upstream reached the same conclusion first: `scheduledRunStorePath`'s default branch is the ONLY
store in `pi-subagents` that lands in the PROJECT tree rather than `TEMP_ROOT_DIR`, and every other
store in that file's neighbourhood (`ASYNC_DIR`, `RESULTS_DIR`, `wait-subscriptions`,
`model-exclusions.json`) is temp-rooted. That asymmetry is deliberate and is the second half of
SUBTASK1's lesson. **Record it in `scheduled_runs/mod.rs`'s module doc** next to the cwd-keying
note, because the same reader who would session-key the store would also "tidy" it into
`<run_scratch>/schedules/<cwd_key>` for consistency with its four siblings.

### 1.3 The version-tolerance pattern — `IndexVersion`

`background/result_index/entry.rs:10-45`, re-read in full:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct IndexVersion;
impl IndexVersion { pub const VALUE: u32 = 1; }
impl serde::Serialize for IndexVersion { /* serialize_u32(Self::VALUE) */ }
impl<'de> serde::Deserialize<'de> for IndexVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = u32::deserialize(deserializer)?;
        if raw == Self::VALUE { Ok(Self) } else {
            Err(serde::de::Error::custom(format!(
                "unsupported result index version {raw} (this build reads version {})", Self::VALUE)))
        }
    }
}
```

The doc at `:15-18` states the property this task inherits: *"A future version 2 record deserializes
to `Err` here, which every caller already handles as 'ignore this index file' — the same outcome pi
reaches by returning `undefined`, and specifically **not** a panic."*

`background/wait_subscriptions/record.rs:29` (`SubscriptionVersion`) is the same type again, and its
`parse_record` at `:245-247` collapses everything to `Option` (*"Tolerant by contract"*). **Note the
divergence in §3.3: upstream's `parseSchedule` THROWS, it does not return `undefined`.**

### 1.4 `exec/capability_ceiling.rs` — the real signatures

836 lines. The two items this task consumes:

```rust
// :74-91
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedCapabilityCeiling {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub allowed_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub allowed_agents: Option<Vec<String>>,
    pub deny_extensions: bool,
    pub sources: Vec<String>,
}

// :406-424  — pi `resolveSubagentCapabilityCeiling` (`capability-ceiling.ts:159-166`)
#[must_use]
pub fn resolve_capability_ceiling(
    session_id: Option<&str>,
    inherited: Option<ResolvedCapabilityCeiling>,
) -> Option<ResolvedCapabilityCeiling>;

// :429-437 — the same, with `inherited` read from CAPABILITY_CEILING_ENV
pub fn resolve_current_capability_ceiling(
    session_id: Option<&str>,
) -> Result<Option<ResolvedCapabilityCeiling>, String>;
```

Facts the implementor must not re-derive:

* `resolve_capability_ceiling` takes **`Option<&str>`**, NOT `&str`. The `?? "unknown"` therefore has
  to be applied *before* the call — see §4.2, this is exactly where the instinct to "just pass
  `None`" bites.
* `resolve_current_capability_ceiling` returns **`Result`**, not `Option`. Its doc at `:425-428`:
  *"a MALFORMED ceiling must fail loudly rather than degrade to 'unbounded', which would invert the
  guarantee."* The gate must decide what `Err` means (§4.3 — fail closed).
* The registry is a process-local `Mutex<HashMap<String, HashMap<u64, ResolvedCapabilityCeiling>>>`
  keyed by session-id string (`registry()`, used at `:286`, `:301-308`, `:352`, `:415`). It is
  registered through `register_capability_ceiling(session_id, source, ceiling)` at `:332-336`,
  which returns a `CapabilityCeilingHandle` (`:264-269`) that **disposes on `Drop`** (`:321-325`).
  A test arming a ceiling must therefore keep the handle alive in a binding — `let _ = register(...)`
  drops it immediately and the ceiling never exists. This is the single most likely way
  `a_schedule_cannot_be_persisted_under_an_active_capability_ceiling` silently passes-by-not-testing.
* `CAPABILITY_CEILING_ENV = "CYRUP_SUBAGENT_CAPABILITY_CEILING_V1"` (`:60`).

### 1.5 The writers

| primitive | file:line | shape |
|---|---|---|
| `write_atomic_json<T: Serialize + Sync>(&Path, &T) -> io::Result<()>` | `background/atomic.rs:75` | **async**, temp+rename, bounded backoff, does NOT create parent |
| `write_private_atomic_json_blocking<T: Serialize>(&Path, &T) -> io::Result<()>` | `background/atomic.rs:209` | **blocking**, creates parent, chmods `0600` before rename — the direct `writePrivateAtomicJson` port (`atomic-json.ts:62`), *"the writer the whole `missions/` subtree persists through"* |
| `write_private_atomic_json` (async twin) | `background/atomic.rs:~145` | async, creates parent, `0600` — used by `completion_replay/store.rs:80` |
| `BoundedJsonlWriter::create(&Path) -> io::Result<Self>` / `.write_line(&str)` | `jsonl.rs:86,115` | **async**, 50 MB lifetime cap (`DEFAULT_JSONL_CAP_BYTES`, `:51`), over-cap lines are **silently dropped, never an error** |

Upstream uses `writePrivateAtomicJson` for `schedule.json`/`history.json`/`runs/<id>.json`
(`:354`, `:371`, `:373`) and a raw `fs.appendFileSync(..., { mode: 0o600 })` for `events.jsonl`
(`:375`, `:381`).

**Gap to close, not to discover at exec time:** `BoundedJsonlWriter::create_with_cap`
(`jsonl.rs:98-111`) opens with `tokio::fs::OpenOptions::new().create(true).append(true)` and
**never sets a mode** — the file lands at `0666 & !umask`, typically `0644`. Upstream's schedule
events log is explicitly `0600`. `run_paths.rs:90-93` states the rule *"Any writer appending to
this path MUST go through `crate::jsonl::BoundedJsonlWriter`"* for a run's `events.jsonl`, and the
same primitive is the right one here — so the fix is a `set_permissions(0o600)` on the schedule
directory's log after create (unix-gated), or a `mode()` option added to `create_with_cap`. Pick
one and say which in the module doc; do **not** hand-roll a second append path.

### 1.6 Session identity

* `extension/executor/session_state.rs:55-59`
  `pub fn current_session_id(&self) -> Option<String>` — the port of
  `ctx.sessionManager.getSessionId()`; doc at `:52-53`: *"`None`/empty (headless, unpersisted, or no
  host services bound) means 'no session identity'."* It already `.filter(|id| !id.is_empty())`.
* `identity/session_id.rs:41-67` — `SessionId(Arc<str>)`, `parse` rejects **only** the empty string
  (`:47-55`), does not trim (`:33-37`), and its `Deserialize` (`:88-93`) is a hard error on empty.
* `crates/cyrup-ext/src/host/services.rs:475-477`
  `fn session_file(&self) -> Option<PathBuf>` — the `sessionManager.getSessionFile()` seam that
  `ScheduleRecord::owner_session_file` needs. It exists; `:435` is the `session_id` twin.

### 1.7 Config and misc

* `registration/mod.rs:679-687` — `ModelExclusionsConfig`, the house pattern for a nested
  `{ … }` config object (`#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]`,
  `#[serde(rename_all = "camelCase", default)]`), with its `Option<…>` field on
  `SubagentExtensionConfig` at `:137` and its seed at `:475`. **`ScheduledRunsConfig` (`enabled`,
  `maxPending`) is SCOPE_16's — see §5.** Do not add it here; `registration/mod.rs` is SCOPE_16's
  file and a part-A edit there is a merge collision for nothing.
* `exec/model_exclusions/persist.rs:47-68` — the `<store>_path_from(env, root)` + thin
  `<store>_path(roots)` pair, the closest landed store-path precedent (SCOPE_3j, PR #137).
* `sha2 = "0.11.0"` is a **direct dependency** of this crate (`Cargo.toml`, with the comment naming
  `exec/mcp_direct_tools.rs`'s `computeMcpServerHash`). The hex helper to copy is
  `exec/mcp_direct_tools.rs:1009-1020` (`hex_sha256`). No new dependency is needed.
* `std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())` is this crate's `path.resolve()`
  idiom — `discovery/skills.rs:235-236`, `spawn/worktree.rs:255`, `tui/fleet.rs:1084`.
* Crate lints: `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic,
  clippy::indexing_slicing)]` + `#![forbid(unsafe_code)]` (`lib.rs:19-25`), and the same four are
  `deny` in `[workspace.lints.clippy]` (root `Cargo.toml:100-105`). Test modules in this crate open
  with the standard four-lint `#![allow(...)]` block (e.g. `result_index/entry.rs:111-116`).
* `background/mod.rs` — `pub mod` declarations at `:36-51` and `:69-79`, private modules at
  `:88-96`, re-exports at `:98-117`. `scheduled_runs` is a **public module with its own facade**
  (like `completion_replay`, `inspect_rpc`, `wait_subscriptions`), so it is one `pub mod
  scheduled_runs;` line inserted alphabetically into the `:69-79` block. No `pub use` re-export
  block is needed at `background::`.

---

## 2. SUBTASK1 — the store path, and its deliberate non-session keying

**Where:** new module `crates/cyrup-ext-subagents/src/background/scheduled_runs/`

### Original requirement (preserved verbatim)

```ts
export function scheduledRunStorePath(cwd: string, _sessionId?: string, root?: string): string;  // :98
```

The `_sessionId` parameter is **accepted and unused** — underscore-prefixed upstream. The store is
per-**cwd** by design: a schedule outlives the session that created it, which is the whole point of
scheduling.

**Port that, do not "fix" it.** Keep the parameter and the underscore, and document why: a reader
who has just absorbed this programme's session-scoping work will otherwise "correct" it into a
session-keyed store and silently break schedule persistence across sessions.

This is the counter-example that proves the rule is *scoping*, not *session-keying everything*.

### The exact Rust it becomes

```rust
// background/scheduled_runs/store.rs
/// pi `scheduledRunStorePath` (`scheduled-runs.ts:98-102` @ `7fe9dee1`).
#[must_use]
pub fn scheduled_run_store_path(
    cwd: &Path,
    _session_id: Option<&SessionId>,
    root: Option<&Path>,
) -> PathBuf
```

* The DoD's spelling `scheduled_run_store_path` (singular `run`) is the one to use — it is what the
  DoD checks and what SCOPE_16 will call.
* `_session_id: Option<&SessionId>`, not `Option<&str>`: the crate's parsed-identity discipline
  (`identity/session_id.rs:12-22`) applies even to a parameter that is discarded, because the
  *type* is half of what tells the next reader the omission is deliberate rather than a `&str` that
  someone forgot to thread. `#[allow(unused)]` is **not** needed — a leading underscore suffices and
  is the upstream-faithful spelling.
* A `#[must_use]` is required by house style for a pure path function (`artifact_roots.rs:305`,
  `:317`, `:333`, `:364`).
* The doc comment **must** state both reasons the parameter is dead: (a) a schedule outlives its
  session; (b) §1.2's reboot-disposability argument for why the default is project-local and not a
  fifth `<run_scratch>/…/<cwd_key>` sibling.

### The two branches, precisely

```rust
let resolved = std::path::absolute(cwd).unwrap_or_else(|_| cwd.to_path_buf());   // path.resolve(cwd)
match root {
    None => crate::artifacts::project_subagents_dir(&resolved).join(SCHEDULES_SUBDIR), // "schedules"
    Some(root) => root.join(project_key(&resolved)),
}
```

`project_key` is `createHash("sha256").update(path.resolve(cwd)).digest("hex").slice(0, 20)` —
**20 hexadecimal CHARACTERS, i.e. 10 bytes of digest**, not 20 bytes. Getting this wrong produces a
store that a differently-built process cannot find, with no error. Build it on
`exec/mcp_direct_tools.rs:1009`'s `hex_sha256` shape and truncate the *string*:

```rust
fn project_key(resolved_cwd: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(resolved_cwd.to_string_lossy().as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(20);
    for byte in digest.iter().take(10) { let _ = write!(hex, "{byte:02x}"); }
    hex
}
```

**Do not substitute `background::cwd_key`** (`artifact_roots.rs:263-269`). It is a `DefaultHasher`
16-hex-char key, it is `pub(crate)` and re-exported only as `pub(crate) use` at
`background/mod.rs:103`, and — decisively — `DefaultHasher` carries **no stability guarantee across
Rust releases**, which is tolerable for a reboot-disposable scratch key and is not tolerable for a
directory a schedule must still be found in next month. Say so in a comment, because "we already
have a cwd key" is the obvious wrong move here.

`to_string_lossy()` is the encoding boundary: upstream hashes a JS string. On a non-UTF-8 path this
is lossy and two distinct paths could collide. That is acceptable only because the `root` branch is
the injected test/sandbox seam (`deps.storeRoot`, `:85`), never the production default — state that
in the doc rather than leaving it implicit.

---

## 3. SUBTASK2 — the schedule record and store

### Original requirement (preserved verbatim)

The persisted schedule: what to run, when, with which agent/options. Round-trips through the store
with a version field, following the `ResultIndexEntry` pattern — a future version deserializes to
`None`, never a panic.

**Layout:** `scheduled_runs/{mod,store,schedule}.rs` for this task; SCOPE_16 adds `trigger.rs` and
`tool.rs`.

### 3.1 The records, at `7fe9dee1:45-76`

> **RE-VERIFIED AGAINST `v0.68.0` (latest), 2026-09-15.** The section below was written against
> `v0.66.0`/`v0.67.0`. The `v0.66.0..v0.68.0` window changes `scheduled-runs.ts` by +51/−18, and
> **exactly one of those changes is schema-level**: `ScheduleTarget` gained a REQUIRED `args` field.
>
> ```diff
> -export type ScheduleTarget = { workflowScript: string; baseRef?: string };
> +export type ScheduleTarget = { workflowScript: string; args: Record<string, unknown>; baseRef?: string };
> ```
>
> Everything else that this task owns is byte-identical at `v0.68.0`: `SCHEDULE_VERSION`, the
> store's directory layout, the record shape otherwise, and the parser's error strings.
>
> **THIS TASK MUST CARRY THE FIELD, even though it does not act on it.** `args` lands in a
> PERSISTED record. Adding it after schedules exist on disk is a migration; adding it now is a
> field. SCOPE_16 deliberately scopes OUT the *behaviour* that reads it
> (`normalizeWorkflowArgs` / `deepFreezeWorkflowArgs` in `parseScheduleTarget`) — see
> `SCOPE_16.md:36-37` — and that split is correct: part A reserves the shape, part B gives it
> meaning. Serialize it as an always-present object (default `{}`), never
> `skip_serializing_if`, so a record written today round-trips unchanged once SCOPE_16 lands.
> `a_schedule_round_trips_through_the_store` (§ tests) must populate it.

```ts
export type ScheduleTrigger =
	| { kind: "once"; at: string; nextRunAt?: string }
	| { kind: "interval"; every: string; everyMs: number; anchorAt: string; nextRunAt: string };
// @v0.67.0 — at v0.68.0 this is { workflowScript, args: Record<string, unknown>, baseRef? }
export type ScheduleTarget = { workflowScript: string; baseRef?: string };
export type ScheduleRunState = "running" | "skipped" | "missed" | "completed" | "failed_launch" | "failed_run";

export interface ScheduleRecord {
	schemaVersion: 1; id: string; name: string; cwd: string;
	trigger: ScheduleTrigger; target: ScheduleTarget;
	overlap: "skip"; catchUp: "none" | "latest";
	timeoutMs?: number; paused: boolean;
	sessionOnly?: boolean; ownerSessionFile?: string;
	createdAt: string; updatedAt: string;
	activeRunId?: string; lastRunId?: string;
}

export interface ScheduleRunRecord {
	schemaVersion: 1; id: string; scheduleId: string; plannedAt: string;
	dueReason: "timer" | "run-due" | "manual"; state: ScheduleRunState;
	startedAt?: string; completedAt?: string; asyncId?: string; asyncDir?: string; error?: string;
}
```

Notes that are load-bearing and easy to lose:

* **`createdAt`/`updatedAt`/`plannedAt`/`nextRunAt`/`anchorAt`/`startedAt`/`completedAt` are ISO-8601
  STRINGS on the wire**, produced by `timestamp(value) = new Date(value).toISOString()` (`:144-146`).
  `everyMs`/`timeoutMs` are numbers. This crate's clock is `crate::time::now_epoch_millis() -> i64`
  (`time.rs:18`, *"The crate's SINGLE epoch-millisecond clock"*) — so every record field is
  `String` on disk and derived from an `i64` in memory. Do **not** silently switch the wire format
  to epoch millis: SCOPE_16 sorts and compares `nextRunAt` lexicographically at `:657`
  (`localeCompare`), and that only works because the value is a `Z`-normalised ISO string. If the
  port needs an `i64` alongside, it is a computed accessor, not a second field.
* **`cwd` is stored in the record** even though the store is cwd-keyed. It is the cwd a fired run
  executes in (`executionParams`, `:456`), and the `root`-override branch's key is a one-way hash,
  so the record is the only way back to the path. Keep it.
* `overlap` has exactly one legal value (`"skip"`); `parseSchedule:304` rejects anything else. Model
  it as a unit-like serde type (the `IndexVersion` shape) or a one-variant enum, not a `String`.
* `sessionOnly: true` **requires** a non-empty `ownerSessionFile` (`:311`). Enforce it at parse.
* `activeRunId`/`lastRunId` are written only by the launch/finish state machine (SCOPE_16) but are
  **fields of the part-A record** — a part-A round-trip test that omits them is not testing the
  format SCOPE_16 will write.

### 3.2 File layout — what goes where

The draft prescribes `{mod,store,schedule}.rs`. That holds, with one addition stated next to it
rather than replacing it:

| file | contents |
|---|---|
| `mod.rs` | facade + module narrative: §1.2's project-local-not-scratch decision, SUBTASK1's dead parameter, the ceiling's place, the SCOPE_16 handoff (§5). Re-exports everything public. |
| `schedule.rs` | `ScheduleVersion`, `ScheduleId`, `ScheduleTrigger`, `ScheduleTarget`, `ScheduleRecord`, `ScheduleRunState`, `ScheduleRunRecord`, `ScheduleHistory`, the parsers and their verbatim error strings. |
| `store.rs` | `SCHEDULES_SUBDIR`, `scheduled_run_store_path`, `project_key`, `ScheduleStore` + its path-containment guard, read/write/list/delete, history read/write, the events append, the active-lock primitives. |
| `ceiling_gate.rs` | **added.** The SUBTASK3 gate, alone. |

**Why `ceiling_gate.rs` and not a function inside `store.rs`:** §4.4's trap. A gate that lives next
to `ScheduleStore::write` will, sooner or later, be called *from* `ScheduleStore::write` — and that
is a deadlock, not a tightening. Its own file with its own doc makes "this gates CREATE, and only
create" a structural statement. If exec prefers three files, put it at the top of `store.rs` under a
`// === the create-time gate: NOT a write-path guard ===` banner and say why in the doc; the
requirement is the separation, not the file count. `targetFiles` lists it either way.

`ScheduleId` is a parsed newtype in the `identity/` house style (`identity/run_dir_name.rs:37-70`,
`identity/result_name.rs:50-96` are the two models): `parse(&str) -> Option<Self>` enforcing
upstream's `/^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/` (`:35`), `as_str`, `Display`, and a
`Deserialize` that goes through `parse`. That is what makes `scheduleDir`'s escape checks
(`:258-273`) structural rather than a runtime re-check at three call sites — the identical argument
`result_index/entry.rs:54-60` already makes for `ResultFileName`.

### 3.3 Version discipline — the draft's `None` vs upstream's throw

**Stated plainly, next to the requirement it qualifies:** the draft says *"a future version
deserializes to `None`, never a panic"*, citing `ResultIndexEntry`. Upstream does **not** do that
here. `parseSchedule` (`:298-313`) `throw`s:

```ts
if (record.schemaVersion !== 1 || … ) throw new Error(`Schedule record '${file}' has invalid required fields.`);
```

and `ScheduleStore.find` (`:347-351`) propagates it, so `list()` (`:336-338`) throws too — the
`try/catch` in `handleToolCall` (`:556-558`) is what turns it into an error result.

Both halves of the draft's claim are satisfiable, and they are not the same claim:

* **"never a panic"** — honoured absolutely. `ScheduleVersion` is `IndexVersion` again
  (`result_index/entry.rs:19-45`): a `Deserialize` that returns `Err` for any value but `1`. No
  `unwrap`, no `expect`, no index — the crate's `#![deny]` block forbids them anyway.
* **"deserializes to `None`"** — this is a *store-API* choice, not a serde one, and it is the one
  genuine open question in this task (`unresolvedQuestions` #1). The recommendation:

  ```rust
  pub enum ScheduleStoreError { Io(std::io::Error), Invalid { file: PathBuf, reason: String } }

  impl ScheduleStore {
      pub async fn find(&self, id: &ScheduleId) -> Result<Option<ScheduleRecord>, ScheduleStoreError>;
      pub async fn get(&self, id: &ScheduleId)  -> Result<ScheduleRecord, ScheduleStoreError>;
      pub async fn list(&self) -> (Vec<ScheduleRecord>, Vec<ScheduleStoreError>);
  }
  ```

  `find`/`get` are upstream-faithful (a named, addressed record that is corrupt is an ERROR the
  caller must see — `Schedule record '<file>' has invalid required fields.`); `list` **skips and
  reports** rather than propagating, because one future-version record must not make every other
  schedule in the project unlistable, which is the precise failure mode a schema bump would
  otherwise cause on every existing install. That divergence from upstream is deliberate and must
  be written down in `store.rs`, not left as an accident of the signature.

  This is also what keeps the draft's test name honest: `a_future_schedule_version_is_ignored_not_a_panic`
  asserts *both* — `list()` omits it (ignored) and returns it in the diagnostics vector, and
  `get()` returns `Err(Invalid{..})` rather than unwinding.

### 3.4 Store surface — exactly what part A owns

Ported from `ScheduleStore` (`:315-383`) and the free functions it leans on:

| upstream | part A | notes |
|---|---|---|
| `validateScheduleId` `:148-151` | `ScheduleId::parse` | error verbatim: `Schedule id must be 1-64 characters and contain only letters, numbers, '.', '_', or '-'.` |
| `normalizedComparisonPath` `:153-164`, `samePath` `:184-186`, `pathWithin` `:166-169` | path helpers | the win32 arm is real; cyrup's `spawn/nested_events.rs:701-707` and `tui/fleet_transcript.rs:363-364` already have the `std::path::absolute` containment idiom — reuse the shape, do not import those functions (different trust roots) |
| `assertScheduleRoot` `:230-256` | `ScheduleStore::ensure_root` | the walk-up-to-the-first-existing-ancestor + realpath + `pathWithin(projectPath, …)` containment check |
| `scheduleDir` `:258-273` | `ScheduleStore::directory` | **lstat symlink rejection** (`:262-263`: `Schedule path '<dir>' must be a real directory.`) and the post-mkdir realpath re-check (`:271`: `Schedule path '<dir>' escapes the project schedule root.`) |
| `ScheduleStore.ids/list/get/find/write/delete` `:328-359` | same | `schedule.json` per id-directory |
| `ScheduleStore.history` `:361-367` | same | `history.json` = `{ schemaVersion: 1, runs: [...] }`, capped at `MAX_HISTORY = 100` (`:33`) |
| `ScheduleStore.writeRun` `:369-376` | same | `runs/<runId>.json` + rewritten `history.json` (newest first, de-duplicated by id, `.slice(0, MAX_HISTORY)`) + one `events.jsonl` line |
| `ScheduleStore.appendEvent` `:378-382` | same | `{schemaVersion:1, timestamp, event, scheduleId}` |
| `active.lock` `:825-844`, `:747`, `:899` | the primitive only | `open(…, "wx", 0o600)` = `OpenOptions::new().create_new(true)`; `EEXIST` is the overlap signal. **Part A owns create/remove/probe; the skip/steal/stale-recovery POLICY is SCOPE_16's.** |

**Explicitly NOT in part A** (upstream-resident but trigger-side): `resolveMaxPending` `:385-388`,
`hasPendingScheduleWork` `:390-392`, `nextAfter` `:394-399`, `nextRunAt` `:401-407`,
`duePlannedAt` `:409-413`, `parseScheduledRunTime` `:104-131`, `parseScheduleInterval` `:133-142`,
`sanitizeTarget` `:434-449`, `executionParams` `:451-462`, `snapshotContext` `:464-477`,
`scheduleBelongsToSession` `:498-503`, and the whole `ScheduledRunManager` class `:509-975`.

Borderline, and resolved deliberately: `parseScheduleTarget` (`:283-296`) **is** part A — it is
called from `parseSchedule`, so the record cannot round-trip without it. It calls
`normalizeWorktreeBaseRef`, which **has no cyrup port** (`grep -rn "base_ref"` finds only
`workflows/scripted/engine.rs:1502-1503`'s `valid_git_ref` predicate on a `baseRef` param). See
`unresolvedQuestions` #3.

### 3.5 Async or blocking

**Async.** `ScheduleStore`'s methods are `async` and write through
`background::atomic::write_private_atomic_json` (the async `0600` twin, the one
`completion_replay/store.rs:80` uses), because SCOPE_16's trigger loop and tool path are both tokio
and `spawn_blocking` around every store touch is a worse seam than an async store. The counter-case
is real and should be named in the doc: `missions/` is *"a synchronous subsystem end to end"*
(`atomic.rs:~200`) and uses the blocking writer; upstream is `readFileSync`/`writeFileSync`
throughout. `scheduled_run_store_path` and `project_key` stay **pure and synchronous** — they touch
nothing.

---

## 4. SUBTASK3 — the capability-ceiling gate

### Original requirement (preserved verbatim)

```ts
resolveCapabilityCeiling?: (sessionId: string) => ResolvedSubagentCapabilityCeiling | undefined;  // :86
const sessionId = ctx.sessionManager.getSessionId() ?? "unknown";                                  // :619
if (this.deps.resolveCapabilityCeiling?.(sessionId))                                               // :620
    return textResult("Cannot persist a schedule while a capability ceiling is active.", …, true);
```

**This is where session enters the file** — not in the store, but in the *authority to write to it*.
A session operating under a capability ceiling cannot persist a schedule, because a schedule would
outlive the ceiling and escape it.

Note `:619`'s `?? "unknown"` — upstream substitutes a literal string rather than refusing. Port
that fallback; do not turn it into an `Option`, or a headless host with no session gains the
ability to persist schedules that a ceiling-bound one lacks.

cyrup already has `exec/capability_ceiling.rs` (14 `session_id` references) — wire to it, do not
build a second ceiling notion.

The refusal string is contract: `Cannot persist a schedule while a capability ceiling is active.`

### 4.1 The `14 session_id references` claim, re-checked

`grep -n "session_id" exec/capability_ceiling.rs` returns **14 lines** (`:265, 286, 301, 308, 333,
337, 338, 352, 357, 409, 413, 415, 430, 434`). The draft's count is correct, and the file is 836
lines. Wiring to it is right and there is nothing to build twice.

### 4.2 The `"unknown"` fallback, in Rust

```rust
pub const UNKNOWN_SESSION_KEY: &str = "unknown";

/// pi `:619` — `ctx.sessionManager.getSessionId() ?? "unknown"`.
fn ceiling_lookup_key(session_id: Option<&SessionId>) -> &str {
    session_id.map_or(UNKNOWN_SESSION_KEY, SessionId::as_str)
}
```

and the call is `resolve_capability_ceiling(Some(ceiling_lookup_key(sid)), inherited)` — `Some(...)`
of a literal, never `None`.

**This is the whole point and it is one keystroke from being lost.** `resolve_capability_ceiling`
takes `Option<&str>` (§1.4). Passing `None` compiles, reads as "no session", and *skips the registry
lookup entirely* (`:413-416` is `if let Some(session_id) = session_id && …`). A host that has
registered a ceiling under the literal session id `"unknown"` — which is exactly what a headless or
unpersisted host does when it registers one — would then be unbound, while a named session is
bound. That is the inversion the draft's `a_host_with_no_session_uses_the_unknown_key` test exists
to catch, and it is why the test must assert the *registry* path, not just that the call returns
something.

Note the asymmetry with the rest of this crate, and write it down: `session_state.rs:52-53` says
`None` means *"no filter, no stamp"* everywhere else. **Here it means the opposite** — no session is
not an exemption, it is a key. One sentence in the gate's doc prevents a future "consistency"
refactor.

### 4.3 The injected seam, and what `Err` means

Upstream's `resolveCapabilityCeiling?:` is an optional dep (`:86`) — absent means *no gate at all*,
which is how its tests run without a registry. Port it as an explicit function seam so tests can
arm a ceiling without touching process-global state, defaulting to the real resolver:

```rust
// background/scheduled_runs/ceiling_gate.rs
pub type CeilingResolver<'a> = &'a dyn Fn(&str) -> Option<ResolvedCapabilityCeiling>;

pub const SCHEDULE_CEILING_REFUSAL: &str =
    "Cannot persist a schedule while a capability ceiling is active.";

/// pi `:619-620`. `Some(refusal)` means the caller MUST NOT persist.
#[must_use]
pub fn schedule_persistence_refusal(
    session_id: Option<&SessionId>,
    resolve: CeilingResolver<'_>,
) -> Option<&'static str>;

/// The production resolver: `resolve_current_capability_ceiling` (`capability_ceiling.rs:429`),
/// **fail-closed** on a malformed inherited ceiling.
#[must_use]
pub fn process_ceiling_resolver(session_key: &str) -> Option<ResolvedCapabilityCeiling>;
```

`resolve_current_capability_ceiling` returns `Result<Option<_>, String>` and its doc
(`capability_ceiling.rs:425-428`) says a malformed value *"must fail loudly rather than degrade to
'unbounded', which would invert the guarantee."* **Fail closed:** an `Err` is treated as "a ceiling
is active" and the schedule is refused. Degrading `Err` to `None` would let a corrupt
`CYRUP_SUBAGENT_CAPABILITY_CEILING_V1` value *buy* the ability to persist a schedule, which is
strictly worse than the refusal. The refusal text stays the contract string either way — a schedule
create is not the place to surface a decode diagnostic. (`unresolvedQuestions` #2 records the
alternative: a distinct second message. Recommendation is the single contract string.)

The check is `is_some()` on the resolved ceiling — **any** ceiling, of any shape, including one whose
`allowed_tools`/`allowed_agents` are both `None` and whose `deny_extensions` is `false`. Upstream's
`:620` is a bare truthiness test on the returned object; it does not inspect the axes. Do not
"improve" it into "only if the ceiling actually restricts something" — `intersect_capability_ceilings`
(`capability_ceiling.rs:371`) already returns `None` when there is nothing registered, so the
presence of a value IS the restriction.

### 4.4 The trap: this gates CREATE, not every write

Upstream calls the gate at exactly ONE site — `create`, `:620` — and at no other. `pause` (`:678`),
`remove` (`:715`), `launch` (`:819`, `:839`, `:849`, `:867`), `finishRun` (`:898`) and
`recordMissed` (`:916`) all write to the store with no ceiling check.

**If the gate is applied inside `ScheduleStore::write`, the store deadlocks**: a run fired before a
ceiling was registered completes while the ceiling is active, `finishRun` cannot clear
`activeRunId`, `active.lock` is never removed, and the schedule is permanently wedged in the
"running" state with `overlap: "skip"` refusing every future fire. The ceiling's purpose is to stop
new authority being *minted*, not to stop existing authority being *wound down*.

State this in `ceiling_gate.rs`'s doc as the reason the gate is a free function the caller invokes,
and add the named test `the_ceiling_gate_does_not_block_updating_an_existing_schedule`.

### 4.5 Sweep item D20

`exec/capability_ceiling.rs`'s module doc (`:6-20`, the *"What a ceiling is"* section) gains a
short note that the ceiling is consulted before a schedule may be persisted, with a
`[`crate::background::scheduled_runs`]` intra-doc link. `rustdoc::broken_intra_doc_links` is
`deny` at the workspace level (root `Cargo.toml:110`), so the link must resolve — which it will,
since the module is `pub`. This is a **docs-only** edit to that file; no code change.

---

## 5. THE SCOPE_16 HANDOFF — stated exactly

Part A ends, and part B begins, at these four lines:

1. **Part A owns the *file format and the directory*.** `ScheduleRecord`, `ScheduleRunRecord`,
   `ScheduleHistory`, the event-line shape, `schedule.json` / `history.json` / `runs/<id>.json` /
   `events.jsonl` / `active.lock` as *paths and primitives*, `scheduled_run_store_path`,
   `ScheduleId`, and the containment guard. **Part B writes no new on-disk format.**
2. **Part A owns the create-time ceiling gate as a callable.** `schedule_persistence_refusal` is
   `pub` and, in part A, **has no production caller** — SCOPE_16's `tool.rs` `create` action is the
   caller. It therefore lands with `#[cfg_attr(not(test), allow(dead_code))]` **only if** clippy
   objects; a `pub` item in a `pub` module is reachable and normally will not warn. Do **not**
   invent a caller to "wire it up" — the DoD says unwired, and SCOPE_8's
   `reconcile_detached_workflow_child_completion` (PR #139) is the landed precedent for a driver
   that ships with no cyrup-side hook yet.
3. **Part B owns every decision that consults a clock or a session.** The trigger algebra
   (`parseScheduledRunTime`, `parseScheduleInterval`, `nextAfter`, `nextRunAt`, `duePlannedAt`,
   `hasPendingScheduleWork`), the launch/skip/miss/finish state machine, the stale-launch-claim
   recovery (`STALE_LAUNCH_CLAIM_MS = 5 * 60_000`, `:34`), the pinned-session identity
   (`:466-470`), `scheduleBelongsToSession`, the `maxPending` cap, the tool actions, the
   `SCHEDULED_RUN_ACTIONS` list (`:19-29`), `registration/mod.rs`'s `ScheduledRunsConfig`, the
   `registration/tool_description.rs:713-722` restoration, and the shared staggered scheduler.
4. **Part B does not re-key the store.** If SCOPE_16 finds itself wanting a session in the path,
   that is the bug §2 exists to prevent.

---

## 6. Tests

All in-module `#[cfg(test)]` (this crate's convention; `src/tests/` holds only cross-module
integration suites). Each opens with the standard four-lint `#![allow(...)]` block
(`result_index/entry.rs:111-116`). Every one of these **fails to compile before the change** (the
module does not exist) — the fail-before/pass-after column therefore states the *behavioural*
failure each would exhibit against a plausible wrong implementation, which is the property that
actually pins anything.

| test | file | pins | fails before / passes after |
|---|---|---|---|
| `the_store_path_is_keyed_by_cwd_not_session` | `store.rs` | SUBTASK1 — two sessions, one cwd, one store | Fails if the path is threaded through `_session_id`. Assert `scheduled_run_store_path(cwd, Some(&sid_a), None) == scheduled_run_store_path(cwd, Some(&sid_b), None) == scheduled_run_store_path(cwd, None, None)` — all three, so "I only ignore it when it's `None`" also fails. |
| `the_default_store_path_is_project_local_and_never_run_scratch` | `store.rs` | §1.2 | **New, and the most valuable one here.** Asserts the `root: None` path starts with `artifacts::project_subagents_dir(cwd)` and ends in `schedules`, AND that it is NOT under `Roots::from_env().run_scratch()`. Fails the moment someone "aligns" this with `wait_subscriptions_dir_in`. |
| `an_explicit_root_keys_by_a_twenty_hex_char_sha256_of_the_resolved_cwd` | `store.rs` | `:100` | Asserts the leaf is 20 chars, all `[0-9a-f]`, equals the first 20 chars of the full hex sha256 of the absolute cwd, and differs for a different cwd. Fails against a 20-*byte* truncation or a `cwd_key` substitution. |
| `a_schedule_round_trips_through_the_store` | `store.rs` | persistence | Write then read a record with **every** optional field populated (`timeoutMs`, `sessionOnly`+`ownerSessionFile`, `activeRunId`, `lastRunId`, `target.baseRef`) and assert equality. Fails against a `skip_serializing_if` that drops a `false`/`0`, and against camelCase drift. |
| `a_future_schedule_version_is_ignored_not_a_panic` | `schedule.rs` + `store.rs` | version discipline (§3.3) | Write `{"schemaVersion":2,…}`; assert `get()` is `Err(Invalid{..})` with upstream's `Schedule record '<file>' has invalid required fields.`, that `list()` omits it and reports it, that a valid sibling schedule still lists, and that neither unwinds. |
| `a_schedule_id_that_escapes_the_store_root_is_refused` | `store.rs` | `:258-273` | `../evil`, `a/b`, an empty id, a 65-char id, an id starting with `.` or `-`. All `Err`, nothing created outside the root. Fails against `root.join(raw_id)`. |
| `a_symlinked_schedule_directory_is_refused` | `store.rs` | `:262-263` | unix-gated. Fails against a plain `is_dir()` check, which follows links. |
| `a_schedule_cannot_be_persisted_under_an_active_capability_ceiling` | `ceiling_gate.rs` | `:620` + exact string | Register a ceiling **holding the handle in a live binding**, assert `schedule_persistence_refusal(Some(&sid), &process_ceiling_resolver) == Some(SCHEDULE_CEILING_REFUSAL)` and that the constant is byte-identical to upstream's sentence. |
| `a_schedule_persists_normally_with_no_ceiling` | `ceiling_gate.rs` | the negative | Drop the handle, assert `None`, and assert a subsequent `store.write` succeeds. Fails against a gate that latches. |
| `a_host_with_no_session_uses_the_unknown_key` | `ceiling_gate.rs` | `:619`'s `?? "unknown"` | Register a ceiling under the literal session id `"unknown"`, call with `session_id: None`, assert refusal. **Fails against the natural `Option` pass-through**, which is the entire reason the test is named. |
| `the_ceiling_gate_does_not_block_updating_an_existing_schedule` | `ceiling_gate.rs` | §4.4 | With a ceiling active, `store.write` of an already-persisted record still succeeds. Fails if the gate is folded into the write path. |
| `the_store_survives_a_session_change` | `store.rs` | the reason it is cwd-keyed | Write under session A, construct a fresh store as session B over the same cwd, assert the record is found unchanged. |
| `a_run_record_and_its_history_round_trip` | `store.rs` | §3.4 | `write_run` twice for the same run id → one history entry, newest first; `MAX_HISTORY` truncation at 100; `runs/<id>.json` present. |
| `an_event_line_is_appended_privately` | `store.rs` | `:375`/`:381` + §1.5 | Two appends → two lines, each parseable as `{schemaVersion,timestamp,event,scheduleId[,runId,state]}`; unix-gated assertion that the file mode is `0o600`. **Fails today against `BoundedJsonlWriter` as written** — that is the §1.5 gap, and this test is what forces it closed. |
| `an_active_lock_is_exclusive` | `store.rs` | `:829` | First `create_new` succeeds, second returns `AlreadyExists`, remove then re-acquire succeeds. Policy stays in SCOPE_16; this pins only the primitive. |

## 7. Benchmarks

None. Schedule persistence is a user-triggered write of a small record.

## 8. Definition of done

- [ ] `scheduled_run_store_path` is cwd-keyed, accepts an unused `_session_id: Option<&SessionId>`
      parameter, and its doc says why that is correct rather than an oversight — **and** why the
      default branch is project-local rather than a fifth `<run_scratch>/…/<cwd_key>` sibling
      (§1.2). Checkable by reading `store.rs` and by
      `the_default_store_path_is_project_local_and_never_run_scratch`.
- [ ] Schedules round-trip with version tolerance: `ScheduleVersion` is the `IndexVersion` shape,
      `get` errors and `list` skips-and-reports on a future version, nothing panics
      (`a_schedule_round_trips_through_the_store`, `a_future_schedule_version_is_ignored_not_a_panic`).
- [ ] The capability-ceiling gate refuses persistence with upstream's exact string
      `Cannot persist a schedule while a capability ceiling is active.`, wired to
      `exec/capability_ceiling.rs`'s `resolve_current_capability_ceiling`, **fail-closed on `Err`**,
      and NOT invoked from the write path
      (`a_schedule_cannot_be_persisted_under_an_active_capability_ceiling`,
      `a_schedule_persists_normally_with_no_ceiling`,
      `the_ceiling_gate_does_not_block_updating_an_existing_schedule`).
- [ ] The `"unknown"` session fallback is ported verbatim and reaches the registry lookup as
      `Some("unknown")` (`a_host_with_no_session_uses_the_unknown_key`).
- [ ] The module is **unwired** — `grep -rn "scheduled_runs" crates/cyrup-ext-subagents/src
      --include=*.rs` outside `background/scheduled_runs/` matches only the single `pub mod
      scheduled_runs;` line in `background/mod.rs` and the D20 doc note in
      `exec/capability_ceiling.rs`. No trigger, no tool registration, no
      `registration/{mod,authority,tool_description}.rs` edit.
- [ ] `exec/capability_ceiling.rs`'s module doc carries the D20 note with a resolving intra-doc link.
- [ ] `cargo nextest run --workspace --no-fail-fast` → 0 failed; `cargo clippy --workspace
      --all-targets` → exit 0 (the four crate-level `deny`s plus the workspace table, root
      `Cargo.toml:100-105`).

## 9. Research notes

* Upstream: `pi-subagents/src/runs/background/scheduled-runs.ts` @ `7fe9dee1` (979 LOC;
  `git describe` = `v0.65.1-50-g7fe9dee1`; contained in tags `v0.66.0`, `v0.67.0` where the file is
  1006 LOC and every cited line has shifted — see §0). Draft's `:86`, `:98`, `:466-470`, `:619-620`
  all **verified correct at the pin**.
* Upstream `getProjectSubagentsDir`: `pi-subagents/src/shared/artifacts.ts:129-131` @ `7fe9dee1` —
  confirmed to exist; ported at `crates/cyrup-ext-subagents/src/artifacts.rs:153-157`.
* Upstream `capability-ceiling.ts`: confirmed present at both `7fe9dee1` and `v0.67.0`.
* Existing ceiling: `crates/cyrup-ext-subagents/src/exec/capability_ceiling.rs` (836 lines; 14
  `session_id` references — the draft's count is right).
* Version-tolerance pattern: `background/result_index/entry.rs:10-45` (`IndexVersion`), and its
  second instance `background/wait_subscriptions/record.rs:29` (`SubscriptionVersion`).
* Store-path precedent: `exec/model_exclusions/persist.rs:47-68` (SCOPE_3j, PR #137).
* Per-cwd durable-record-store precedent: `background/wait_subscriptions/` (SCOPE_11, PR #139) —
  structurally the closest sibling, and the one whose **directory choice must not be copied** (§1.2).
* Sweep item **D20**: `exec/capability_ceiling.rs`'s docs gain a note that the ceiling is consulted
  before a schedule may be persisted.
* `registration/authority.rs:66-73` already anticipates this feature and belongs to SCOPE_16.
