---
stage: aug
status: done
updated: 2026-09-06 14:20
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

`extension/config.ts:9` imports `DEFAULT_MODEL_EXCLUSION_TTL_MS`, `MAX_MODEL_EXCLUSION_TTL_MS` and
`setDefaultTTL`, so the TTL is operator-configurable. Wire the same into cyrup's subagents config
(`extension/`), with `set_default_ttl`'s validation ported exactly (`:78-88`): finite, positive, and
`<= MAX_MODEL_EXCLUSION_TTL_MS`, rejected with upstream's message. The `shorten_existing` option
retroactively shortens loaded entries — port it; without it, lowering the TTL has no effect until
every existing entry expires under the old one.

---

## Definition of done

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
