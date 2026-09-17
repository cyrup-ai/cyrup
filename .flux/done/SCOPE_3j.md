---
stage: qa
status: completed
updated: 2026-09-14 06:00
---

# SCOPE_3j — model-exclusion registry + `recordRetryableModelFailure`

OBJECTIVE: port
[`runs/shared/model-exclusions.ts`](../../../workspace/pi-subagents/src/runs/shared/model-exclusions.ts)
(**374 LOC**) and the `model-fallback.ts` half that writes to it — `recordRetryableModelFailure`
(`:617-623`) and the candidate filter (`:516-519`).

**Discovered while augmenting `SCOPE_3b`** (landed; task file since deleted). It is `is_context_overflow`'s third call
site, and it is unscheduled anywhere in SCOPE_2–16:

```
$ grep -rn "model_exclusion\|ModelExclusion\|record_model_failure" crates/cyrup-ext-subagents/src
# 0 matches
$ grep -l "model-exclusions\|recordModelFailure" ~/.flux/*/todo/SCOPE_*.md
# none
```

> ## ⚡ 3b LANDED FIRST, SO THIS TASK GOT SMALLER — [`SCOPE_3.md`](SCOPE_3.md) §A
>
> Read §A first; binding. **The dependency inverted:** SCOPE_3 §1 used to order 3j *before* 3b so
> that 3b could consume `record_retryable_model_failure`. It is now the other way round.
>
> **Why that is strictly better.** This task's original §0 carried a cross-task constraint reading
> roughly *"3j must hoist the classification above `fallback.rs:1456`, not bolt it onto the existing
> `:1462` call, or a last-candidate retryable failure never records an exclusion."* That instruction
> exists only because the ladder's precedence was statement order across a 90-line loop. After 3b
> part 2 (§A.1), the insertion point is **one line in `classify_attempt`**, a pure function, marked
> in place:
>
> ```rust
> // ── SCOPE_3j inserts `record_retryable_model_failure` HERE ────────────────────────
> // pi calls it at `:1407`, immediately BEFORE `isContextOverflow` at `:1408`.
> ```
>
> **Delete the hoist instruction from this file's §0 when executing** — it describes a problem that
> no longer exists, and leaving it will send someone editing line numbers.
>
> **One caveat the inversion does not remove:** `classify_attempt` is *pure*, and
> `record_retryable_model_failure` **writes to a persisted exclusion registry**
> (`model-exclusions.ts` — `flushPersist` `:103`, `EXCLUSIONS_PATH_ENV` `:7`). A recording side
> effect cannot go inside it. §A.1 row 5 gives the shape: `classify_attempt` returns the decision,
> and the **shell** performs the record — so `LadderStep::Settle`/`AdvanceModel` may need to carry a
> `record_exclusion: Option<ModelExclusion>` payload, or the shell re-asks a pure predicate. Decide
> that when implementing; do not smuggle a file write into the pure core.
>
> **Also consumes:** `AttemptNote` (§A) if any exclusion surfaces to the operator, and `ModelId`
> rather than `String` for exclusion targets — `parseModelKey` (`model-fallback.ts:620`) splits
> provider/model, which is what `cyrup_core::ModelId` already represents.

Read [`SCOPE_3.md`](SCOPE_3.md) §0 first. Depends on nothing. Blocks 3b.

---

## 0.0 ⚠ RE-AUGMENTED 2026-09-14 — corrected anchors, three broken premises, one dead link scheme

Everything below §0 is preserved. This section **adds** precision and flags what is now false. Where
a prescribed mechanism cannot work as written, the working mechanism is recorded **next to** the
original requirement, not in place of it.

### 0.0.1 The oracle is on disk, and every `../../../workspace/…` link in this file is dead

`/home/user/workspace` does not exist on this machine. The read-only upstream oracle is
**`/home/user/cyrup/tmp/pi-subagents`** (same correction as `SCOPE_3.md` §4's 2026-09-14 sweep).
Read `tmp/pi-subagents/src/runs/shared/model-exclusions.ts` and
`tmp/pi-subagents/src/runs/shared/model-fallback.ts`.

### 0.0.2 Upstream DRIFTED — `model-exclusions.ts` is 385 LOC, not 374

Every `:NNN` in this file's §1 table and prose is off by up to +12 in the back half. Corrected,
verified by `sed -n` on the oracle at the paths above:

| symbol | THIS FILE says | ACTUAL (`model-exclusions.ts`, 385 LOC) |
|---|---|---|
| `EXCLUSIONS_PATH_ENV` | `:7` | `:7` ✓ (`"PI_MODEL_EXCLUSIONS_PATH"`) |
| `ModelExclusionTarget` (the union) | `:9` | `:9` ✓ |
| `ModelExclusion` | `:11-15` | `:11-15` ✓ |
| `RecordModelFailureOptions` | *unlisted* | `:17-21` — has a `preserveExisting?: boolean` this file never mentions |
| `DEFAULT_MODEL_EXCLUSION_TTL_MS` | `:25` | `:26` (`24 * 60 * 60_000`) |
| `MAX_MODEL_EXCLUSION_TTL_MS` | `:27` | `:28` (`8_000_000_000_000_000`) |
| `MAX_DATE_TIMESTAMP_MS` | *unlisted* | `:29` (`8_640_000_000_000_000`) — the **persisted-entry** ceiling, distinct from the TTL ceiling |
| `AUTH_FAILURE_PATTERNS` | `:34-41` | `:35-41` |
| `isAuthModelExclusion` | — | `:44-47` |
| `getAuthStoreMtimeMs` | — | `:49-60` |
| `invalidateAuthExclusions` | `:60-78` | `:62-69` |
| `setDefaultTTL` | `:78` / validation `:78-88` | `:79-86` |
| `getExclusionsFilePath` | `:92` / `:92-97` | `:93-97` |
| `flushPersist` | `:103` / write `:105-115` / wire `:107-110` | `:104-117`; wire format `:108-111`; tmp name `${file}.${pid}.${seq++}.tmp` at `:109` |
| `schedulePersist` | *unlisted* | `:119-127` — **debounce is 5000 ms**, `timer.unref?.()` |
| `ensureLoaded` | *unlisted* | `:129-158` — the load-time prune/shorten/dedupe/`invalidateAuthExclusions` pipeline |
| `readPersistedExclusion` | *unlisted* | `:169-185` — **13 validation rules** this file never mentions (see §0.0.6) |
| `dedupKey` | *unlisted* | `:187-189` — `provider ?? ""` + `"\|"` + `modelId ?? ""` |
| `deduplicate` | *unlisted* | `:191-202` — `Map` keyed by `dedupKey`, keeps the entry with the **greater `recordedAt`**, returns `Array.from(map.values())` (first-seen key order) |
| `recordModelFailure` | `:208` / cap+dedupe `:221-224` | `:209-235`; `unshift` `:231`, `deduplicate` `:232`, `length > 200 → length = 200` `:233`, **`flushPersist()` (NOT `schedulePersist`) `:234`** |
| `clearExpiredExclusions` | `:230` | `:237-242` |
| `clearExclusions` | `:240` | `:247-251` |
| `entryMatches` | `:256-265` | `:263-270` (doc `:253-262`) |
| `isExcluded` | `:268` | `:275-279` — signature is `(modelId, provider)`, **already-split parts, not a full id** |
| `findModelExclusion` | `:280` | `:287-295` — also takes `{ now?, ignoreExclusion? }` |
| `getExcludedCount` | `:290` | `:301-305` — **mutates**: calls `clearExpiredExclusions()` first |
| `parseModelKey` | `:306` / `:306-312` | `:317-322` (doc `:307-316`) |
| `filterFallbackCandidates` | `:317` / `:317-344` | `:328-354` |
| `reloadFromDisk` | `:345` | `:356-360` |
| `prune` | *unlisted* | `:362-371` |
| `shortenExclusionsToTTL` | *unlisted* | `:373-385` — sets `expiresAt = min(expiresAt, recordedAt + ttlMs)`, then prunes |

And in `model-fallback.ts` (678 LOC):

| symbol | THIS FILE says | ACTUAL |
|---|---|---|
| `MODEL_EXCLUSION_DIAGNOSTIC_MAX_LENGTH` | *unlisted* | `:293` = **240** |
| `MODEL_EXCLUSION_DIAGNOSTIC_MAX_ENTRIES` | *unlisted* | `:294` = **20** |
| `sanitizeModelExclusionDiagnostic` | "part of that path" | `:296-301` |
| `formatModelExclusionExpiry` | "part of that path" | `:303-307` |
| `formatExcludedCandidateEvidence` | `:311-332` | `:309-316` |
| `MODEL_UNAVAILABLE_PATTERN` | *unlisted* | `:318` = `/(?:model.*(?:not found\|unavailable\|disabled)\|unknown model)/i` |
| `ignoreStaleModelUnavailableExclusion` | `:333-345` | `:320-324` |
| `throwForExplicitModelExclusion` | **MISSING from §1** | `:326-334` — a **fourth** consumer (see §0.0.13) |
| `ZERO_USABLE_MODEL_CANDIDATES_ERROR` | `:522-530` | declared `:426-427`; thrown `:519` |
| the filter call | `:516-519` | `:506-509` |
| the zero-candidates evidence block | `:522-530` | `:512-521` |
| `isTransientNoOutputFailure` | **MISSING from §1** | `:592-595` — a **fourth guard** on `recordRetryableModelFailure` |
| `REQUEST_SHAPE_FAILURE_PATTERN` | `:616` | `:608`; upstream's note at `:606-607` |
| `PROVISIONING_FAILURE_PATTERNS` / `_TTL_MS` / `isProvisioningFailure` | **MISSING entirely** | `:620-625` / `:627` (`15 * 60_000`) / `:629-631` |
| `recordRetryableModelFailure` | `:617-623` | `:633-642` |
| call site (foreground) | `execution.ts:1976` | **`runs/foreground/execution.ts:2066`** |
| call site (background) | `subagent-runner.ts:1407` | **`runs/background/subagent-runner.ts:1555`** |
| `setDefaultTTL` config wiring | `extension/config.ts:9` | `:9` ✓ (the import); `resolveModelExclusionTTL` `:227-229`; `applyModelExclusionsConfig` `:237-240`; **validation** `:106-107`; config key is `modelExclusions.defaultTtlMs` (`shared/types.ts:2560-2563`, field `:2628`) |

### 0.0.3 SCOPE_3b HAS LANDED. The marker is real and this file's ⚡ block is now history, not plan

Verified in the working tree (`crates/cyrup-ext-subagents/src/exec/fallback.rs`, 4083 lines):

| thing | line | note |
|---|---|---|
| `pub fn build_model_candidates` | `:146-174` (doc `:106-145`) | **still `-> Vec<ModelId>`, infallible**; doc at `:140-144` says *"never fails and never panics… an empty result is a legitimate, representable outcome"* |
| `pub fn provider_of(&ModelId) -> Option<ProviderId>` | `:184-191` | splits on the FIRST `/`, rejects empty halves |
| `pub fn qualify_model_candidate` | `:204-215` | |
| `pub fn build_model_candidates_scoped` | `:235-265` | returns `(Vec<ModelId>, Vec<ModelScopeViolation>)` — **the established "ladder + diagnostics" return shape**; copy it |
| the "silent downgrade" doc | **`:229-231`**, not `:229-234` | it is `build_model_candidates_scoped`'s doc, about *scope warnings*, not about an exclusion filter. A second, unrelated "silent downgrade" note is at `:325` and a third at `:2347` |
| `pub(crate) struct LowerLiteral` | `:490` | `lower!()` const-checks lowercase |
| `enum RetryPattern` | **`:515-548`**, not `:484+` | 8 variants; there is **no** `WordAlternatives` yet |
| `const RETRYABLE_MODEL_FAILURE_PATTERNS` | **`:551-605`**, not `:484+` | |
| `const CONTEXT_OVERFLOW_PATTERNS` | `:628-655` | |
| `fn matches_word_number` | `:678-693` | the `\b` implementation to reuse |
| `pub(crate) struct LoweredLine<'a>` | `:708` | |
| `fn line_matches` | `:729-826` | **private** |
| `pub fn is_retryable_model_failure` | `:899-918` | |
| `pub fn is_context_overflow` | **`:931-956`** | SCOPE_3b's landed port |
| `fn is_empty_output_sentinel` | **`:967-973`** | **this IS `isTransientNoOutputFailure`** — the missing fourth guard already exists, private, same file |
| `pub fn is_retryable_model_failure_attempt` | `:1001-1028` | |
| `pub enum AttemptNote` | `:1038-1064` | `ModelAdvance` / `ContextOverflow` / `StartupRetry` |
| `pub struct AttemptSignal` | `:1211` | |
| `pub(crate) enum LadderStop` | `:1648-1664` | |
| `pub(crate) enum LadderStep` | `:1667-1681` | `Settle(LadderStop)` / `RetryStartup{delay_ms}` / `StartupExhausted` / `AdvanceModel` — **all four variants are field-light and the enum derives `Copy`** (`:1666`) |
| `fn classify_attempt` | `:1699-1747` | pure, `(signal, is_last_candidate, startup_attempt_index) -> LadderStep` |
| **the SCOPE_3j marker** | **`:1721-1725`** | verbatim below |
| `enum LadderControl` | `:1749-1757` | |
| `pub async fn run_fallback_ladder` | `:1758` | the shell; the `match` on `LadderStep` is at `:1837` |

The marker in the tree reads:

```rust
// ── SCOPE_3j inserts `record_retryable_model_failure` HERE ──────────────────────────────
// pi calls it at `:1407`, immediately BEFORE `isContextOverflow` at `:1408`. Note the
// recording is a FILE WRITE (`model-exclusions.ts`'s persisted registry), so it cannot live
// inside this pure function: 3j must carry the decision out on a `LadderStep` payload and let
// the shell perform it.
```

(Its own `:1407`/`:1408` citations are the stale ones — the real upstream lines are
`subagent-runner.ts:1555` / `:1556` and `execution.ts:2066` / `:2067`.)

**Consequences for this file's ⚡ block:**

* "**Blocks 3b**" (end of the header) is **stale** — 3b landed. 3j now blocks nothing.
* "*Delete the hoist instruction from this file's §0 when executing*" is **already done** — §0 as it
  now stands ("What is missing and why it matters") carries no hoist instruction. Nothing to delete.
* `LadderStep` derives `Copy`. Adding `record_exclusion: Option<ModelExclusion>` to `Settle`/
  `AdvanceModel` **removes `Copy`** and breaks `:1837`'s by-value `match` and the
  `stop == LadderStop::ContextOverflow` compare inside it. See §0.0.8 for the shape that does not.

### 0.0.4 ⛔ `PI_MODEL_EXCLUSIONS_PATH` IS FORBIDDEN — this file's SUBTASK1 requirement is wrong

SUBTASK1/`persist.rs` says: *"Add the `PI_MODEL_EXCLUSIONS_PATH` lower-precedence fallback, matching
the twelve pairs already listed at `cyrup-config/src/env.rs:68-91` — that is this workspace's
standing rename convention, not an optional extra."*

**Every clause of that is false.** `cyrup-config/src/env.rs:68-91` is `fn truthy` plus the head of
`EnvVars::from_lookup`; there is no list of pairs there or anywhere. The workspace convention is the
exact opposite — a **hard rename** (R-07-028):

* `env.rs:25` — *"Typed view over the `CYRUP_*` environment surface (R-07-028; the legacy `PI_*`
  spellings are…)"*; `env.rs:119` — *"The upstream `PI_CODING_AGENT_DIR` spelling is **NOT honoured**
  (hard rename, R-07-028)."*
* `env.rs:648-668` is a **regression test**
  (`drift050_offline_and_skip_version_check_stay_two_state`) asserting that
  `("CYRUP_OFFLINE",""),("PI_OFFLINE","1")` leaves `offline == false`, with the message
  *"the dropped `PI_*` spelling must be inert"*.
* Every one of this crate's env consts is a single `CYRUP_SUBAGENT_*` name with a doc comment citing
  the pi original and **no** fallback: `THINKING_CEILING_ENV` (`exec/thinking_ceiling.rs:32`),
  `RUN_FANOUT_BUDGET_ENV` (`exec/run_fanout_budget.rs:70`), `MAX_SPAWNS_PER_RUN_ENV` (`:74`),
  `MAX_SPAWNS_PER_SESSION_ENV` (`exec/spawn_budget.rs:41`), `TOOL_BUDGET_ENV`
  (`exec/tool_budget.rs:29`), `CAPABILITY_CEILING_ENV` (`exec/capability_ceiling.rs:60`),
  `STRUCTURED_OUTPUT_SCHEMA_ENV` (`exec/structured.rs:149`), `CHILD_TOOL_DIAGNOSTIC_PATH_ENV`
  (`exec/tool_availability.rs:40`), and the six in `spawn/nested_events.rs:35-46`.

**Do this instead — the original requirement (env-overridable path) preserved, mechanism corrected:**

```rust
/// pi `EXCLUSIONS_PATH_ENV = "PI_MODEL_EXCLUSIONS_PATH"` (`model-exclusions.ts:7`), under this
/// crate's own prefix exactly as [`crate::exec::thinking_ceiling::THINKING_CEILING_ENV`] is. The
/// legacy `PI_` spelling is NOT read — hard rename, R-07-028 (`cyrup-config/src/env.rs:119`,
/// asserted at `:664-668`).
pub const MODEL_EXCLUSIONS_PATH_ENV: &str = "CYRUP_SUBAGENT_MODEL_EXCLUSIONS_PATH";
```

Note the `SUBAGENT_` infix — this file's proposed `CYRUP_MODEL_EXCLUSIONS_PATH` omits it and would
be the only const in the crate that does. Upstream's trim rule is `typeof envPath === "string" &&
envPath.trim()` then `envPath.trim()` (`:94-96`); a set-but-blank value falls through, which is also
`background/artifact_roots.rs:226-229`'s stated rule and the reason it is load-bearing there.

### 0.0.5 The path / auth / write seams: use `crate::paths::Roots`, not the free functions

* **default exclusions path.** Upstream is `path.join(TEMP_ROOT_DIR, "model-exclusions.json")`
  (`:96`). cyrup's `TEMP_ROOT_DIR` is `crate::background::temp_root_dir()`
  (`background/artifact_roots.rs:203`, `pub(crate)`; pure core `temp_root_dir_from` at `:220`) —
  **but** the resolved, injectable form this crate threads everywhere is
  `crate::paths::Roots::run_scratch()` (`paths.rs:305`), which `Roots::from_lookup`
  (`paths.rs:231-243`) fills from exactly that function. A store that takes a `&Roots` is
  sandboxable by every existing test; one that calls `temp_root_dir()` is not.
* **auth store.** Upstream is `path.join(getAgentDir(), "auth.json")` (`:50`). `crate::paths` has
  **no** auth helper — its free functions are `home_dir` `:89`, `home_dir_from` `:101`,
  `resolve_agent_dir` `:121`, `resolve_agent_dir_from` `:135`, `agent_dir` `:141`,
  `project_config_dir` `:148`. Use **`roots.agent_dir().join("auth.json")`** (`paths.rs:299`);
  `cyrup-tui/src/login_dialog.rs:611` and `cyrup-tui/src/app/login.rs:344,458` independently confirm
  `<agent_dir>/auth.json` is cyrup's spelling. `ConfigDirs::auth_path`
  (`cyrup-config/src/env.rs:350`) is the binary-side twin of the same answer.
* **mtime → epoch ms.** `crate::time::epoch_millis(SystemTime)` (`time.rs:26`) converts a
  `metadata.modified()`; `crate::time::now_epoch_millis()` (`:18`) is `Date.now()`. Both return
  `i64` and never panic — that is the crate's single clock (`time.rs:1-12` says why). Do not add a
  second.
* **atomic write.** `crate::background::atomic::write_atomic_json` is at `background/atomic.rs:75`,
  is **`async`** (`tokio::fs`), `pub`, and does **not** create the parent directory. Upstream does
  `fs.mkdirSync(path.dirname(file), {recursive:true})` first (`:106`), so use
  `write_atomic_json_creating_parent` (`atomic.rs:114`, `pub(crate)`) — it exists for exactly this
  mismatch and its doc names it. (`write_private_atomic_json_blocking` is the `0600` sibling; the
  exclusions file is not a credential, so the ordinary writer is right.)

### 0.0.6 What §1's store-surface list OMITS — port these too

§1 lists the `export`ed surface only. Five private pieces are load-bearing and appear nowhere in
this file:

1. **`readPersistedExclusion` (`:169-185`) — 13 validation rules.** `modelId`/`provider`, when
   present, must be non-empty strings; `reason`, when present, a string; `recordedAt`/`expiresAt`
   must be finite numbers in `(0, MAX_DATE_TIMESTAMP_MS]`; an entry with **neither** `modelId` nor
   `provider` is rejected with `"must include modelId or provider"`. Each rejection logs
   `Model exclusion store entry {index} {message}.` and the entry is dropped — never fatal. In Rust
   most of this is free: `ModelExclusionTarget` as an enum makes the last rule unrepresentable,
   which is §0's own argument. **The numeric range checks are not free** and a corrupt file must not
   produce an `expires_at` that overflows the clock arithmetic.
2. **`ensureLoaded` (`:129-158`) order, which is not obvious:** parse → drop `expiresAt <= now` →
   `shortenExclusionsToTTL` if a ceiling is set → `deduplicate` → `flushPersist()` **if anything was
   invalid**, else `schedulePersist()` if anything was shortened → then `invalidateAuthExclusions()`.
   A file whose `version !== 1` is silently ignored (no throw, no clear). A read error that is not
   `ENOENT` logs and leaves the store empty.
3. **`deduplicate` (`:191-202`) keeps the entry with the GREATER `recordedAt`** and returns `Map`
   insertion order. Combined with `unshift`, a freshly recorded entry is the first-seen key and so
   stays at index 0. This file's *"newest-first `unshift`, deduplicate, truncate at 200"* is right
   about the order and silent about the tie-break.
4. **`recordModelFailure`'s TTL is CAPPED, not merely defaulted:**
   `Math.min(options.ttlMs ?? defaultTTLMs, defaultTTLMs)` (`:210`). A caller cannot exceed the
   configured default. `reason` defaults to the literal **`"runtime-failure"`** (`:229`).
5. **`preserveExisting` (`:217`, `:224-228`):** when set, an unexpired entry under the same
   `dedupKey` makes the whole call a no-op. Its only caller is the provisioning arm of
   `recordRetryableModelFailure` (§0.0.7) — omit it and every npm/preflight failure re-stamps a
   fresh 15-minute window, which is the behaviour the flag exists to prevent.

`schedulePersist`'s debounce is **5000 ms** with `unref` — *"Never hold the process open just to
flush exclusions"* (`:125`). In tokio the equivalent is a dirty-flag plus a `tokio::time::sleep`
task the runtime does not wait on at shutdown; `flush_persist` must stay directly callable, as this
file already requires.

### 0.0.7 `record_retryable_model_failure` has FOUR guards and a TTL override, not two

SUBTASK3's sketch is materially incomplete. The real function (`model-fallback.ts:633-642`):

```ts
export function recordRetryableModelFailure(model: string | undefined, error: string | undefined): void {
	if (!model || !error || !isRetryableModelFailure(error) || isContextOverflow(error)) return;
	if (REQUEST_SHAPE_FAILURE_PATTERN.test(error) || isTransientNoOutputFailure(error)) return;
	const { provider, modelId } = parseModelKey(model);
	recordModelFailure({
		modelId,
		reason: error,
		...(provider ? { provider } : {}),
		...(isProvisioningFailure(error) ? { ttlMs: PROVISIONING_FAILURE_TTL_MS, preserveExisting: true } : {}),
	});
}
```

* **Guard 3 — `isTransientNoOutputFailure` (`:592-595`)** is already ported as
  `is_empty_output_sentinel` (`exec/fallback.rs:967`, private, same file). A cold-start empty
  response must not mark a model unhealthy. **Reuse it; do not re-derive.** It is the same predicate
  `is_retryable_model_failure_attempt` consults at `:1012`.
* **The provisioning arm (`:620-631`) is absent from this task file entirely.**
  `PROVISIONING_FAILURE_PATTERNS` = `/^npm (?:install|ci|uninstall|exec|run)\b/i`,
  `/\bnpm\b[^\n]*failed with code \d+/i`, `/\bnpm\b[^\n]*exited with code \d+/i`, `/\bpreflight\b/i`;
  `PROVISIONING_FAILURE_TTL_MS = 15 * 60_000`. Upstream's reason (`:610-619`): a package name
  containing the substring `model` next to a sentence containing `failed` trips
  `RETRYABLE_MODEL_FAILURE_PATTERNS` even though no model request ever happened, so the record is
  kept *only long enough to damp repeated attempts* instead of blaming the model for 24 h.
  **Decide explicitly whether to port it; do not port it by accident or drop it silently.** Note the
  first pattern is `^`-anchored and two need `\d+` — none is expressible in today's `RetryPattern`,
  so this arm costs more than one new alternation variant.
* **The call site is guarded by the already-computed per-attempt flag**, identically in both
  upstream loops (`execution.ts:2065-2067`, `subagent-runner.ts:1554-1556`):
  ```ts
  const retryableModelFailure = isRetryableModelFailureAttempt({ error, messages, toolCount });
  if (retryableModelFailure) recordRetryableModelFailure(candidate ?? run.model ?? step.model, error);
  if (isContextOverflow(error)) { … }
  ```
  So `isRetryableModelFailure(error)` *inside* the function is a **second, broader** check that runs
  after the per-attempt one — keep both; they are not redundant (`…Attempt` also consults
  `tool_count`/`message_count`, which the text classifier cannot see).
  **In cyrup, `classify_attempt` does NOT compute that flag before the marker** — it reaches
  `is_retryable_model_failure_attempt(signal)` only at `:1743`, *below* the context-overflow branch.
  Reproducing upstream's order means hoisting that call into a `let` above the marker (it is pure,
  so this is free) and reusing the binding at `:1743`, not calling it twice.
* **`REQUEST_SHAPE_FAILURE_PATTERN` — the `RetryPattern` extension.** This file's
  `WordAlternatives(&[&str])` proposal is sound and remains the requirement.
  `matches_word_number` (`fallback.rs:678-693`) already implements `\b` over `is_word_byte`
  (`:662-665`) — **generalise it from ASCII digits to any ASCII literal** rather than writing a
  second boundary scanner. The regex expands to five `\b`-bounded literals: `bad request`,
  `bad_request`, `invalid argument`, `invalid_argument`, `invalid_request_error`. `line_matches`
  (`:729`) is private and its `match` is exhaustive over `RetryPattern`, so a new variant is a
  compile error until handled — the intended §A.1 property.

### 0.0.8 ⛔ Two prescribed mechanisms cannot work as written. Here is what can.

**(a) `build_model_candidates` cannot raise the zero-candidates error.** It is
`#[must_use] -> Vec<ModelId>` (`fallback.rs:146`) whose doc explicitly promises *"never fails and
never panics"*, and it is called from `resolve_model_candidates` (`exec/mod.rs:798-810`) and
`build_model_candidates_scoped` (`fallback.rs:235`) — both `pub` — plus
`exec/spawn_plan.rs:5065` and three in-crate tests (`exec/mod.rs:2014,2042,2047`). Turning it into
`Result` touches all of them. Upstream throws because it is TS. cyrup already has the seam:

```rust
// exec/mod.rs:379-384 — the EXISTING empty-ladder site
let candidates = resolve_model_candidates(agent, opts);
if candidates.is_empty() {
    let error = "no candidate model available for this subagent run (empty fallback ladder)"
        .to_string();
    return pre_spawn_failure(agent, task, error);   // pre_spawn_failure: exec/mod.rs:965
}
```

**The mechanism that works:** give the filter the shape `build_model_candidates_scoped` already
established — `(Vec<ModelId>, Vec<ExcludedCandidate>)` — and render the bounded evidence into that
existing `error` string at `exec/mod.rs:380-383`. One `pub` signature changes
(`build_model_candidates_scoped`, whose non-test caller is `exec/mod.rs:799`),
`build_model_candidates` keeps its documented contract, and the diagnostic reaches the operator
through the path that already exists. **Port upstream's precondition exactly** (`model-fallback.ts:514`):
the error is raised only when `candidates.length > 0` *before* filtering — an already-empty ladder
returns empty and falls to cyrup's existing message rather than gaining a bogus "excluded"
diagnostic.

**(b) `LadderStep` carrying a `record_exclusion: Option<ModelExclusion>` payload costs `Copy`.**
`LadderStep` derives `Copy` at `fallback.rs:1666` and the shell consumes it by value at `:1837`.
Two shapes keep the pure/shell split without that cost, in preference order:

1. **The shell re-asks a pure predicate.** `classify_attempt` stays byte-identical; the shell, on
   the `Settle(ContextOverflow)` / `Settle(ModelFailure)` / `AdvanceModel` arms, calls a pure
   `should_record_retryable_model_failure(&AttemptSignal) -> bool` (all four guards, no I/O) and
   performs the store write itself. This is §A.1 row 5 satisfied exactly — a decision tangled with
   IO becomes a pure `fn` returning a decision, executed by a shell — and it is the marker's own
   "carry the decision out … and let the shell perform it" in its cheapest form.
2. A separate `#[derive(Clone, Copy)] struct AttemptDecision { step: LadderStep, record: bool }`
   returned by `classify_attempt`, if the ordering must be visible in one place. Still `Copy`.

Do **not** hang `Option<ModelExclusion>` (which owns a `String` reason) off `LadderStep`:
`SCOPE_3.md` §3's type-contract gate 4 greps
`match .*(LadderStep|LadderStop|AttemptNote|RunMode)` for `_ =>` arms, and the shell's `match` at
`:1837` is the one it reads.

### 0.0.9 `cyrup_core::ModelId` does NOT represent a provider/model split

This file's ⚡ block says *"`ModelId` rather than `String` for exclusion targets — `parseModelKey`
splits provider/model, which is what `cyrup_core::ModelId` already represents."* **It does not.**
`ModelId` is `str_id!(ModelId)` (`cyrup-core/src/lib.rs:92`, macro `:54-86`): a
`#[serde(transparent)] pub struct ModelId(pub Arc<str>)` holding the **joined** `"provider/id"`
string — `fallback.rs:184-191`'s `provider_of` exists precisely because the split has to be
recomputed off it, and its doc says so. The split type in `cyrup-core` is
`ModelRef { provider: ProviderId, api: Option<ApiId>, model: ModelId }` (`lib.rs:102-106`), whose
mandatory `provider` and irrelevant `api` make it a poor fit.

**The right representation is this file's own `ModelExclusionTarget` enum**, with `ProviderId` for
the provider half (`str_id!`, `lib.rs:91`) and `ModelId` for the model half. Both derive
`Serialize`/`Deserialize` transparently, so the persisted `{"modelId":…,"provider":…}` wire format
is unchanged and `entry.rs`'s §0 argument stands untouched. Only the sentence about `ModelId`
"already representing" the split is wrong.

### 0.0.10 `parse_model_key`: the two thinking-suffix splitters are NOT interchangeable

`parseModelKey` (`:317-322`) uses **`splitKnownThinkingSuffix`** (`shared/model-info.ts:39-47`),
while `ignoreStaleModelUnavailableExclusion` (`:320-324`) uses the *other* one,
**`splitThinkingSuffix`** (`model-fallback.ts:13-19`). cyrup has both:

* `crate::exec::spawn_plan::split_known_thinking_suffix` — **`pub`**, `exec/spawn_plan.rs:173-185`,
  returns `(base, suffix_including_colon)`, splits on the last `:` **only if** the tail is a
  `THINKING_LEVELS` entry. **This is the one `parse_model_key` needs** (this file's
  `grep -rn "thinking_suffix"` hint resolves here). Its doc at `:163-171` records the distinction
  explicitly.
* `split_thinking_suffix` — the unconditional last-`:` split — exists **twice and privately**:
  `extension/models/mod.rs:22` and `watchdog/model_selection.rs:323`. (`spawn_plan.rs:167` still
  cites its old home as `extension.rs`.) If `ignore_exclusion` needs the unconditional form, note
  that substituting the strict one is a **narrowing**: `openai/gpt-5:preview` keeps its suffix and
  would then fail to match an `available_models` entry of `openai/gpt-5`. Decide and document; do
  not add a third copy.

`available_models` in cyrup is `&[ModelId]` (`RunOptions::available_models`), so upstream's
`availableModels?.some(e => e.fullId === baseModel)` is
`available_models.iter().any(|m| m.as_str() == base)`.

### 0.0.11 Store ownership: `RunOptions` has no `Default`, so a new field touches 31 sites

The *"thread it through `RunOptions` the way `usage_budget` already is
(`runner_main/executor.rs:566-570`)"* citation is stale. Corrected:

* `pub struct RunOptions` — **`exec/agent_config.rs:396`** (doc `:391-395`),
  `#[derive(Debug, Clone)]`, **no `Default` impl**.
* `usage_budget` field — `exec/agent_config.rs:671` (doc `:666-670`).
* Production construction sites: **`background/runner_main/executor.rs:624`** (the literal;
  `usage_budget: self.usage_budget` at `:635`) and **`extension/executor/foreground.rs:749`**
  (`build_foreground_run_options`). `executor.rs:553` is only `build_step_run_options`' return type
  — the line range this file cites (`:566-570`) is an interrupt-token comment.
* The run-level source it is carried from: `runner_main/config.rs:225` →
  `runner_main/turn_loop.rs:411`.
* `grep -rn 'RunOptions {' crates --include=*.rs | wc -l` = **31**, spanning
  `exec/testsupport.rs:57` and twelve `cyrup-it/tests/**` fixtures: `base_run_options` in
  `subagents/exec_run_sync_integration.rs:87`, `subagents/child_protocol_stream_integration.rs:114`,
  `subagents/companions_wiring_proof.rs:135`,
  `subagents/startup_retry_lifecycle_integration.rs:126`,
  `subagents/child_stderr_drain_integration.rs:114`,
  `subagents/subagent_persona_and_depth_integration.rs:498`,
  `subagents/run_state_signal_and_stop_parity.rs:155`, plus
  `subagents/read_only_acceptance_inference.rs:121`,
  `subagents/acceptance_parser_state_model_interaction.rs:103`,
  `subagents/child_written_output_authorship.rs:110`,
  `intercom/child_bridge_activation.rs:134` and `permission/forwarding_spawn_env.rs:181`.
  **This is why `--workspace` is mandatory, and it is a real edit cost — budget for it.**

`usage_budget` is `Copy`; an `Option<Arc<ModelExclusionStore>>` is not, so the `executor.rs:624`
literal needs an explicit `.clone()`. The executor-owned precedent this file cites is real and its
rationale belongs quoted into `store.rs`'s doc: `extension/executor/mod.rs:69` (`completion_bus`,
doc `:61-68`), constructed at `:267`; the *"a `static` registry cannot be reset between sessions"*
argument is stated verbatim at **`:245-250`** for `workflow_resources` — a SCOPE_3d sibling that
made exactly this call.

`grep -rn "static .*EXCLUSION" crates/cyrup-ext-subagents/src` is empty today, and
`grep -rn "model_exclusion\|ModelExclusion\|record_model_failure" crates --include=*.rs` returns
**exactly one hit** — the marker comment at `exec/fallback.rs:1721`. §0's "0 matches" premise holds
and **the objective is NOT implemented**.

### 0.0.12 Reusables that are currently PRIVATE and will need widening

| needed | where | visibility today |
|---|---|---|
| `is_empty_output_sentinel` (guard 3) | `exec/fallback.rs:967` | `fn` — fine if `record_retryable_model_failure` lives in `fallback.rs`; needs `pub(crate)` otherwise |
| `line_matches` (to evaluate `AUTH_FAILURE_PATTERNS`) | `exec/fallback.rs:729` | `fn` — **private**. `RetryPattern` (`:515`) is private too; `LoweredLine` (`:708`) and `LowerLiteral` (`:490`) are `pub(crate)` |
| `redact_secret_values` (for `sanitizeModelExclusionDiagnostic`) | `watchdog/permission_arbiter.rs:230` | `fn` — **private**; this is the port of upstream's `redactSecretValues`. Widen it rather than writing a second redactor — `:896-897` pins its `sk-` behaviour |
| `split_known_thinking_suffix` | `exec/spawn_plan.rs:173` | `pub` ✓ |
| `provider_of` / `qualify_model_candidate` | `exec/fallback.rs:184` / `:204` | `pub` ✓ |
| `temp_root_dir` / `temp_root_dir_from` | `background/artifact_roots.rs:203` / `:220` | `pub(crate)` ✓ |
| `write_atomic_json` / `…_creating_parent` | `background/atomic.rs:75` / `:114` | `pub` / `pub(crate)` ✓ |
| `now_epoch_millis` / `epoch_millis` | `time.rs:18` / `:26` | `pub` ✓ |

`AUTH_FAILURE_PATTERNS` (`:35-41`) is `/auth(?:entication)?/i`, `/unauthori[sz]ed/i`,
`/forbidden/i`, `/api key/i`, `/token expired/i`, `/invalid key/i` — all six are already rows of
`RETRYABLE_MODEL_FAILURE_PATTERNS`: `Contains("auth")` `:557`, `Contains("unauthorized")` `:558`,
`Contains("unauthorised")` `:559`, `Contains("forbidden")` `:560`, `Contains("api key")` `:561`,
`Contains("token expired")` `:562`, `Contains("invalid key")` `:563`. Note upstream's single
`unauthori[sz]ed` is **two** cyrup rows, so the "six patterns" count in this file's `auth.rs`
section is **seven** rows here. Build the auth subset as a second
`const AUTH_FAILURE_PATTERNS: &[RetryPattern]` over the same `lower!()` literals; §1's "strict
subset of `RETRYABLE_MODEL_FAILURE_PATTERNS`" relation holds and is worth asserting in a test.

`MODEL_UNAVAILABLE_PATTERN` (`model-fallback.ts:318`) likewise decomposes into rows that already
exist: `Then("model","not found")` `:568`, `Then("model","unavailable")` `:566`,
`Then("model","disabled")` `:567`, `Contains("unknown model")` `:569`.

### 0.0.13 The diagnostic strings, verbatim (and the fourth consumer §1 omits)

```
No usable subagent models remain after registry, scope, and cached-exclusion filtering.
```
(`model-fallback.ts:426-427`), suffixed when there is evidence with
`" (excluded: " + entries.join("; ") + (omitted > 0 ? "; ... and " + omitted + " more" : "") + ")"`
(`:517`).

One evidence entry (`:315`), **em dash**, four labelled fields:
```
{candidate} — model: {modelId}; provider: {provider}; reason: {reason}; expires: {ISO-8601}
```
`"unknown"` for a missing candidate or model, `"unspecified"` for a missing provider,
`"runtime-failure"` for a missing reason, `"unknown"` for a non-finite expiry (`:303-307`).
`sanitizeModelExclusionDiagnostic` (`:296-301`) collapses C0 controls, DEL, U+2028 and U+2029 runs
to a single space, trims, substitutes the fallback when empty, **redacts, and only then truncates to
240** — in that order. Truncating before redacting is the exact regression
`crates/cyrup-ext-subagents/src/tests/verify_memo_and_redaction.rs:318`
(`redaction_runs_before_the_output_is_bounded_so_a_straddling_secret_cannot_leak`) already guards
elsewhere in this crate.

The per-skip warn (`:471`):
```
[pi-subagents] Skipping model '{candidate}' due to a cached exclusion (reason: {reason}; expires: {ISO}).
```

**`throwForExplicitModelExclusion` (`:326-334`) — the consumer §1's table omits.** When the primary
model's origin is `explicit`, upstream refuses the run outright rather than rotating to a fallback:
```
Requested subagent model '{model}' is excluded and cannot be replaced by a fallback (reason: {reason}{; expires: ISO}).
```
It re-implements the sanitiser inline (`:331`, controls → space, redact, slice 240 — note it does
**not** strip U+2028/29). cyrup's counterpart rung is `ModelOverride::as_model_id()`
(`fallback.rs:84-104`) / `resolve_model_inheritance` (`:336`); decide whether to port this arm and
say so — it is the difference between "explicit model silently downgraded" and "run refused".

`setDefaultTTL`'s rejection (`model-exclusions.ts:81`):
```
Default model exclusion TTL must be a finite positive number no greater than 8000000000000000.
```
and the config-layer twin (`extension/config.ts:107`):
```
config.modelExclusions.defaultTtlMs must be a finite positive number no greater than 8000000000000000
```
(no trailing period on the second — that asymmetry is upstream's; reproduce it).

### 0.0.14 SUBTASK4's config home is `registration/`, not `extension/`

`SubagentExtensionConfig` is at **`crates/cyrup-ext-subagents/src/registration/mod.rs:79`**
(`#[serde(rename_all = "camelCase", default)]`, doc `:69-78`). `src/extension/` holds `executor/`,
`host/`, `models/`, `tool/`, `wait_tool.rs` — there is no config struct there. The nearest field
precedents for a nested optional knob are `parallel: Option<TopLevelParallelConfig>`,
`chain: Option<ExtensionChainConfig>` and `control: Option<ControlConfig>`, each
`#[serde(skip_serializing_if = "Option::is_none")]` — so
`model_exclusions: Option<ModelExclusionsConfig>` with `default_ttl_ms: Option<u64>` matches both
upstream's shape (`shared/types.ts:2560-2563`) and the struct's house style.
`max_subagent_spawns_per_run` (same struct, the `CFG-067` field) is the closest analogue for
"absent is NOT unlimited, absent falls to the next rung".

`shortenExisting` is wired as `config.modelExclusions?.defaultTtlMs !== undefined`
(`extension/config.ts:239`) — i.e. it is **true exactly when the operator set the key**, never for
the built-in default. Port that condition, not a bare `true`.

Validation lives beside upstream's at `extension/config.ts:106-107` (before `setDefaultTTL`), and
`setDefaultTTL` re-validates (`:80-82`). Port **both** — the double check is upstream's, and
`crate::exec::usage_budget::validate_usage_budget_config` (`exec/usage_budget.rs:202`) is this
crate's established pattern for a config validator that refuses at the tool boundary with
upstream's own text.

### 0.0.15 Gates

Per `SCOPE_3.md` §3 as amended 2026-09-14. Baseline on this commit is green — there are no
pre-existing failures to attribute anything to:

```bash
cargo fmt --all -- --check                              # clean (repo-wide no-op)
cargo clippy --workspace --all-targets -- -D warnings   # clean
cargo nextest run --workspace                           # 9913 run, 9913 passed, 9 skipped
```

Format only what you touch (`cargo fmt -p cyrup-ext-subagents`). `--workspace` is mandatory — see
§0.0.11's 31 `RunOptions` construction sites, twelve of them in `cyrup-it`. Check `df -h /`,
`df -h /tmp` and `pgrep -a cargo` first. **No `git` command at any stage.**

---

## 0. What is missing and why it matters

cyrup's ladder builds its candidate list in `build_model_candidates`
([`exec/fallback.rs:146-215`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/exec/fallback.rs))
— dedupe, in order, allowlist-filter. Upstream's equivalent ends with
(`model-fallback.ts:516-519`):

```ts
const resolved = filterFallbackCandidates(candidates, {
    onExcluded: warnCachedExclusion,
    ignoreExclusion: (candidate, exclusion) => ignoreStaleModelUnavailableExclusion(candidate, exclusion, availableModels),
});
```

**cyrup has no such filter and no such store.** Three consequences, all live today:

1. A model that failed with `429`/`quota`/`unauthorized` on the last run is retried immediately on
   the next, and on every run after that, until the underlying condition clears on its own. The
   ladder's entire purpose — spend attempts where they can succeed — is undercut by re-attempting a
   model already known to be down.
2. When every candidate is excluded, upstream raises `ZERO_USABLE_MODEL_CANDIDATES_ERROR` **with
   evidence** — which model, which reason, when the exclusion expires (`:522-530`). cyrup cannot
   produce that diagnostic because it has nothing to report.
3. `recordRetryableModelFailure`'s guard (`:617`) is
   `!isRetryableModelFailure(error) || isContextOverflow(error)` — a context overflow must **not**
   mark the model unhealthy, since the model is fine and the *input* was too large. That guard is
   SCOPE_3b's third consumer and cannot exist without this store.

---

## 1. Verified anchors

> **⚠ 2026-09-14: every `:NNN` below is STALE — see §0.0.2 for the corrected table.**
> §1 also OMITS four things: `throwForExplicitModelExclusion`, `isTransientNoOutputFailure`,
> the `PROVISIONING_FAILURE_*` arm, and the whole private half of the store (§0.0.6, §0.0.7, §0.0.13).

| upstream | LOC | cyrup |
|---|---|---|
| `model-exclusions.ts` | 374 | **absent** |
| `recordRetryableModelFailure` (`model-fallback.ts:617-623`) | 7 | **absent** |
| `formatExcludedCandidateEvidence` (`:311-332`) | 22 | **absent** |
| `ignoreStaleModelUnavailableExclusion` (`:333-345`) | 13 | **absent** |
| the filter call (`:516-519`) + zero-candidates evidence (`:522-530`) | — | `build_model_candidates` (`exec/fallback.rs:146-215`) has no filter step |
| `setDefaultTTL` config wiring (`extension/config.ts:9`) | — | **absent** |

Store surface (`model-exclusions.ts`): `EXCLUSIONS_PATH_ENV` (`:7`), `ModelExclusion` (`:11-15`),
`DEFAULT_MODEL_EXCLUSION_TTL_MS` = 24 h (`:25`), `MAX_MODEL_EXCLUSION_TTL_MS` (`:27`),
`setDefaultTTL` (`:78`), `getExclusionsFilePath` (`:92`), `flushPersist` (`:103`),
`recordModelFailure` (`:208`), `clearExpiredExclusions` (`:230`), `clearExclusions` (`:240`),
`isExcluded` (`:268`), `findModelExclusion` (`:280`), `getExcludedCount` (`:290`), `parseModelKey`
(`:306`), `filterFallbackCandidates` (`:317`), `reloadFromDisk` (`:345`).

---

## SUBTASK1 — `exec/model_exclusions/`

> **⚠ 2026-09-14: the `PI_MODEL_EXCLUSIONS_PATH` fallback required in `persist.rs` below is
> FORBIDDEN by R-07-028 — see §0.0.4 for the const that is correct. The `crate::paths` auth/temp
> seams are §0.0.5; the omitted store internals are §0.0.6; the `static`-vs-`RunOptions` cost is
> §0.0.11.**

**Where:** new module `crates/cyrup-ext-subagents/src/exec/model_exclusions/`

```text
model_exclusions/
  mod.rs      facade + the "why a failed model stays failed" narrative; no logic
  entry.rs    ModelExclusion, ModelExclusionTarget, entry_matches, parse_model_key
  store.rs    the in-process store + TTL + dedupe + the 200-entry cap
  persist.rs  the on-disk format, the env override, the atomic write, the debounce
  auth.rs     invalidate_auth_exclusions — the auth-store mtime rule
  filter.rs   filter_fallback_candidates / is_excluded / find_model_exclusion
```

House style is `background/result_index/`'s: facade `mod.rs` with the layout diagram and zero logic.

### `entry.rs` — model the target as an enum, not two `Option`s

```rust
/// What an exclusion applies to — pi `ModelExclusionTarget` (`model-exclusions.ts:9`).
///
/// A union upstream (`{modelId, provider?} | {provider, modelId?: never}`), and an enum here for
/// the reason upstream spells the `never` out: a record with NEITHER field is meaningless and a
/// record with both interpreted loosely would let `openai/gpt-4` exclude `github-copilot/gpt-4`.
/// Two `Option`s on a struct admit both mistakes; this admits neither.
pub enum ModelExclusionTarget {
    /// A specific model, optionally narrowed to one provider.
    Model { model_id: String, provider: Option<String> },
    /// Every model of one provider — what a quota or auth failure produces.
    Provider { provider: String },
}
```

`entry_matches` (`:256-265`) is the matching rule and its two branches are asymmetric on purpose —
port the doc comment with it:

* a **model** entry matches that `model_id`, and when *both* entry and candidate carry a provider
  the providers must agree — so `openai/gpt-4` does not exclude `github-copilot/gpt-4`;
* a **provider** entry matches every model of that provider;
* an expired entry (`expires_at <= now`) matches nothing.

`parse_model_key` (`:306-312`) strips a known thinking suffix first, then splits on the **first**
`/` — so `openrouter/google/gemini-flash` is provider `openrouter`, model `google/gemini-flash`.
Upstream's own doc says this *"MUST stay in lock-step with the matching inside `isExcluded`"*; keep
them in one file for that reason. cyrup already has the suffix splitter — reuse it rather than
re-deriving (`grep -rn "thinking_suffix" crates/cyrup-ext-subagents/src crates/cyrup-provider/src`).

### `store.rs` — no `static`

Upstream is module-global mutable state (`let exclusions: ModelExclusion[]`, `let loaded`). Do
**not** port that as a Rust `static`: this crate's established ownership pattern hangs shared
registries off `SubagentExecutor` (`extension/executor/mod.rs:61`'s `completion_bus`, with the
rationale stated there), and a `static` store cannot be isolated between runs — which is exactly
what upstream's `reloadFromDisk` (`:345`) and `EXCLUSIONS_PATH_ENV` exist to work around.

Own it as a `ModelExclusionStore` behind an `Arc`, threaded to the ladder through `RunOptions` the
way `usage_budget` already is (`runner_main/executor.rs:566-570`). `reload_from_disk` and
`clear_exclusions` then become ordinary methods rather than global resets.

Cap and dedupe (`:221-224`): newest-first `unshift`, deduplicate, truncate at **200**. Port the
order — dedupe before truncate, or a duplicate can evict a distinct entry.

### `persist.rs` — atomic, debounced, env-overridable

* path: `CYRUP_MODEL_EXCLUSIONS_PATH` over `<temp_root>/model-exclusions.json`
  (`getExclusionsFilePath`, `:92-97`). Add the `PI_MODEL_EXCLUSIONS_PATH` lower-precedence fallback,
  matching the twelve pairs already listed at `cyrup-config/src/env.rs:68-91` — that is this
  workspace's standing rename convention, not an optional extra.
* wire format: `{ version: 1, exclusions: [...] }` (`:107-110`).
* the write is tmp-plus-rename (`:105-115`). cyrup already has that primitive —
  `background/atomic.rs`'s `write_atomic_json` — use it rather than a second implementation.
* upstream **debounces** writes and exposes `flushPersist` for when durability matters. Port both;
  a `tokio` interval or a dirty-flag checked at the ladder boundary is sufficient, and
  `flush_persist` must be callable directly.

### `auth.rs` — the auth-store mtime rule, which is not obvious

`invalidateAuthExclusions` (`:60-78`) drops every auth-flavoured exclusion recorded **before** the
Pi auth store was last modified:

```ts
const retained = exclusions.filter((entry) => !isAuthModelExclusion(entry) || authStoreMtimeMs <= entry.recordedAt);
```

The reasoning is worth writing down because the code does not say it: a 401 excludes a provider for
24 h, but the user fixing their credentials should not have to wait it out. Touching the auth store
is the signal that the cause may be gone. `AUTH_FAILURE_PATTERNS` (`:34-41`) classifies by the
stored `reason` text; the six patterns are a strict subset of `RETRYABLE_MODEL_FAILURE_PATTERNS`
(`exec/fallback.rs:484+`) — reuse `RetryPattern` and `line_matches` rather than adding a matcher.

A stat failure that is not `ENOENT` **preserves** the exclusions and logs (`:53-58`). Do not degrade
to "drop them"; the fail-safe direction is to keep the exclusion.

cyrup's auth store is `<agent_dir>/auth.json` — resolve through `crate::paths`, not a literal join.

---

## SUBTASK2 — `filter.rs` and the ladder filter

> **⚠ 2026-09-14: `build_model_candidates` is infallible and CANNOT raise the zero-candidates error
> — §0.0.8(a) gives the mechanism that can, and the `candidates.length > 0` precondition this
> section omits. The `:229-234` "silent downgrade" doc is really `:229-231` and is about SCOPE
> warnings, not exclusions (§0.0.3). Diagnostic strings verbatim: §0.0.13.**

`filter_fallback_candidates` (`:317-344`) preserves order, drops duplicates, drops excluded, and
takes two callbacks:

* `on_excluded(candidate, exclusion)` — the diagnostic hook;
* `ignore_exclusion(candidate, exclusion) -> bool` — the escape hatch.

Port both as `Option<&dyn Fn(...)>` parameters rather than dropping them: `ignore_exclusion` is what
`ignoreStaleModelUnavailableExclusion` (`model-fallback.ts:333-345`) uses to ignore a stale
`model-unavailable` exclusion for a model that is once again in `availableModels`. Without it a
transient availability blip pins a model out for 24 h.

Wire into `build_model_candidates` (`exec/fallback.rs:146-215`) as the **last** step, after dedupe
and the allowlist filter — upstream's position (`:516`, after its own `seen`/scope loop). The
existing doc at `:229-234` warns that filtering in the wrong place is *"a silent downgrade"*; read it
before choosing the insertion point.

### The zero-candidates diagnostic

When the filter empties the list, upstream throws `ZERO_USABLE_MODEL_CANDIDATES_ERROR` with bounded
evidence (`:522-530`): up to `MODEL_EXCLUSION_DIAGNOSTIC_MAX_ENTRIES` entries formatted by
`formatExcludedCandidateEvidence` (`:311-332`), plus `; ... and N more`. Port the bound and the
overflow phrasing verbatim — an unbounded diagnostic over a 200-entry store is a wall of text at
exactly the moment an operator needs one line.

`sanitizeModelExclusionDiagnostic` and `formatModelExclusionExpiry` are part of that path; port them
with it. The sanitiser exists because the `reason` is raw provider error text and reaches a log.

---

## SUBTASK3 — `record_retryable_model_failure`

> **⚠ 2026-09-14: the sketch below shows TWO guards; upstream has FOUR plus a provisioning TTL
> override — §0.0.7. The call-site line numbers (`subagent-runner.ts:1407` / `execution.ts:1976`)
> are stale; the real ones are `:1555` / `:2066`. `LadderStep` is `Copy` and must stay so —
> §0.0.8(b).**

The `model-fallback.ts` half, and SCOPE_3b's third `is_context_overflow` consumer:

```rust
/// pi `recordRetryableModelFailure` (`model-fallback.ts:617-623`).
///
/// # The two guards, and why each is there
///
/// * `is_context_overflow(error)` — an overflow says the INPUT was too large, not that the model
///   is unhealthy. Excluding the model for 24 h over it would take a working model out of every
///   later ladder because one task was too big. This is [`crate::exec::fallback::is_context_overflow`]'s
///   third call site (SCOPE_3b).
/// * [`REQUEST_SHAPE_FAILURE_PATTERN`] — upstream's own note (`:615-616`): *"Request-shape failures
///   can match broad fallback signals such as 'upstream', but do not establish that the model is
///   unhealthy for subsequent requests."*
pub fn record_retryable_model_failure(store: &ModelExclusionStore, model: Option<&ModelId>, error: Option<&str>)
```

`REQUEST_SHAPE_FAILURE_PATTERN` is `/\b(?:bad[ _]request|invalid[ _]argument|invalid_request_error)\b/i`
(`:616`). Port it over `RetryPattern` — `bad[ _]request` is `OptionalCharBetween("bad", "request")`
restricted to space/underscore, which the enum does not express exactly; add a
`WordAlternatives(&[&str])` variant, or express it as three word-bounded `Contains` checks with the
`\b` handling `matches_word_number` already implements. **Prefer extending the enum**: a
word-bounded alternation is a shape upstream uses more than once, and open-coding it here guarantees
the next one is open-coded differently.

Call it from the ladder at pi's position — `subagent-runner.ts:1407` / `execution.ts:1976`, i.e.
immediately after the per-attempt retryable classification and **before** SCOPE_3b's
context-overflow branch. In cyrup that is `exec/fallback.rs`, just above the insertion point
SCOPE_3b names.

---

## SUBTASK4 — config wiring

> **⚠ 2026-09-14: the config struct is `registration/mod.rs:79`, NOT `extension/` — §0.0.14, which
> also gives `shorten_existing`'s real condition (`defaultTtlMs !== undefined`, not `true`) and
> both validation sites.**

`extension/config.ts:9` imports `DEFAULT_MODEL_EXCLUSION_TTL_MS`, `MAX_MODEL_EXCLUSION_TTL_MS` and
`setDefaultTTL`, so the TTL is operator-configurable. Wire the same into cyrup's subagents config
(`extension/`), with `set_default_ttl`'s validation ported exactly (`:78-88`): finite, positive, and
`<= MAX_MODEL_EXCLUSION_TTL_MS`, rejected with upstream's message. The `shorten_existing` option
retroactively shortens loaded entries — port it; without it, lowering the TTL has no effect until
every existing entry expires under the old one.

---

## Definition of done

> **⚠ 2026-09-14 — three bullets below are unachievable or wrong as written; the replacements are
> in §0.0 and the REQUIREMENT each serves is unchanged:**
>
> * *"honours `CYRUP_MODEL_EXCLUSIONS_PATH` with a `PI_MODEL_EXCLUSIONS_PATH` fallback"* →
>   honours **`CYRUP_SUBAGENT_MODEL_EXCLUSIONS_PATH`** and reads **no** `PI_*` spelling (§0.0.4).
>   `grep -rn '"PI_MODEL_EXCLUSIONS_PATH"' crates` MUST be empty.
> * *"`build_model_candidates` filters … and an empty result raises the zero-candidates error"* →
>   the filter is the last step of **`build_model_candidates_scoped`**, which returns the excluded
>   evidence alongside the ladder; the error text is rendered at the existing empty-ladder site
>   `exec/mod.rs:379-384`, and only when the pre-filter ladder was non-empty (§0.0.8a).
> * *"`record_retryable_model_failure` refuses a context overflow and a request-shape failure"* →
>   **and** an empty-output sentinel (`is_empty_output_sentinel`, `fallback.rs:967`), and its
>   provisioning arm is an explicit decision, not an omission (§0.0.7).
>
> Add: `grep -rn 'static .*EXCLUSION' crates/cyrup-ext-subagents/src` empty (already in the list),
> and `LadderStep` still derives `Copy` (`fallback.rs:1666`) — §0.0.8b.

* `exec/model_exclusions/` exists with the six-file layout, and every export in §1's list has a
  counterpart.
* The store is owned (an `Arc<ModelExclusionStore>` threaded through `RunOptions`), **not** a
  `static` — `grep -rn "static .*EXCLUSION" crates/cyrup-ext-subagents/src` is empty.
* `ModelExclusionTarget` is an enum; a record with neither a model nor a provider is
  unrepresentable.
* `entry_matches` implements both asymmetric branches: `openai/gpt-4` does not match
  `github-copilot/gpt-4`, and a provider-only entry matches every model of that provider.
* `parse_model_key` splits on the FIRST `/` after stripping a thinking suffix, so
  `openrouter/google/gemini-flash` parses as provider `openrouter`.
* Persistence is `{version:1,...}` through `background/atomic.rs`, honours
  `CYRUP_MODEL_EXCLUSIONS_PATH` with a `PI_MODEL_EXCLUSIONS_PATH` fallback, debounces, and exposes
  a direct `flush_persist`.
* Auth-flavoured exclusions recorded before the auth store's mtime are dropped; a non-`ENOENT` stat
  failure preserves them.
* The store caps at 200 entries, dedupes **before** truncating, and newest-first.
* `build_model_candidates` filters through `filter_fallback_candidates` as its last step, and an
  empty result raises the zero-candidates error with bounded evidence and the `; ... and N more`
  overflow phrasing.
* `ignore_exclusion` is wired to the stale-`model-unavailable` rule; a model back in
  `available_models` is not pinned out.
* `record_retryable_model_failure` refuses a context overflow and a request-shape failure, and the
  word-bounded alternation is a `RetryPattern` variant rather than open-coded.
* The TTL is configurable with upstream's validation and its `shorten_existing` semantics.

**Gates:** as [`SCOPE_3.md`](SCOPE_3.md) §3. `RunOptions` and `build_model_candidates` are `pub` and
cross crates — `--workspace` is mandatory.

---

/home/d0m17bw/.flux/-home-d0m17bw-workspace-cyrup/todo/SCOPE_3j.md
