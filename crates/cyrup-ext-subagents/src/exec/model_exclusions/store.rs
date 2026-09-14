//! The registry itself: in-process state, the TTL, dedupe, the 200-entry cap, and the load/persist
//! seams the other files in this module supply.
//!
//! Ports pi `model-exclusions.ts`' mutable module state (`:23-33`) and every function that reads or
//! writes it: `setDefaultTTL` (`:79-86`), `ensureLoaded` (`:129-158`), `deduplicate` (`:191-202`),
//! `recordModelFailure` (`:209-235`), `clearExpiredExclusions` (`:237-242`), `clearExclusions`
//! (`:247-251`), `isExcluded` (`:275-279`), `findModelExclusion` (`:287-295`), `getExcludedCount`
//! (`:301-305`), `reloadFromDisk` (`:356-360`), `prune` (`:362-371`) and `shortenExclusionsToTTL`
//! (`:373-385`).
//!
//! # Why this is a value and not a `static`
//!
//! Upstream is module-global mutable state (`let exclusions: ModelExclusion[]`, `let loaded`), and
//! that shape is exactly what `reloadFromDisk` and `EXCLUSIONS_PATH_ENV` exist to work around when
//! a test needs isolation. This crate already settled the same question the other way for
//! `completion_bus` and `workflow_resources` ([`crate::extension::executor`], whose own doc states
//! the rule: *"a `static` registry cannot be reset between sessions, and pi's own store is scoped
//! to the extension host, not the process"*). So the store is OWNED — by the executor in the
//! foreground, by the detached runner process in the background — and reaches the ladder on
//! [`crate::exec::RunOptions`], the same way `usage_budget` does. `reload_from_disk` and
//! `clear_exclusions` are then ordinary methods rather than global resets.
//!
//! # Which half is async
//!
//! Upstream is synchronous throughout (`readFileSync`/`writeFileSync`). Here the READ stays
//! synchronous — it is one small local file on the pre-spawn path, and making it async would force
//! [`crate::exec::fallback::build_model_candidates_scoped`], a `pub` synchronous function, to
//! become async for a syscall measured in microseconds — while the WRITE goes through this crate's
//! one atomic-JSON writer, which is `async`. That split is why `record_model_failure` is `async`
//! and every read path is not: the write already happens on the ladder's own async shell
//! ([`crate::exec::fallback::run_fallback_ladder`]), which is the only place that records.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use cyrup_core::{ModelId, ProviderId};

use crate::exec::model_exclusions::auth::invalidate_auth_exclusions;
use crate::exec::model_exclusions::entry::{ModelExclusion, ModelExclusionTarget, parse_model_key};
use crate::exec::model_exclusions::persist;

/// pi `DEFAULT_MODEL_EXCLUSION_TTL_MS` (`model-exclusions.ts:26`) — 24 h.
pub const DEFAULT_MODEL_EXCLUSION_TTL_MS: i64 = 24 * 60 * 60_000;

/// pi `MAX_MODEL_EXCLUSION_TTL_MS` (`model-exclusions.ts:28`).
///
/// Deliberately BELOW [`super::entry::MAX_DATE_TIMESTAMP_MS`] (`8e15` against `8.64e15`): a TTL is
/// added to `now`, and the gap is what keeps the sum a representable timestamp rather than one
/// [`super::filter::format_model_exclusion_expiry`] has to render as `"unknown"`.
pub const MAX_MODEL_EXCLUSION_TTL_MS: i64 = 8_000_000_000_000_000;

/// pi's `if (exclusions.length > 200) exclusions.length = 200` (`model-exclusions.ts:233`).
const MAX_STORED_EXCLUSIONS: usize = 200;

/// pi `recordModelFailure`'s `options.reason ?? "runtime-failure"` (`model-exclusions.ts:224`), and
/// the same literal [`super::filter::format_excluded_candidate_evidence`] substitutes for a missing
/// one.
pub const DEFAULT_EXCLUSION_REASON: &str = "runtime-failure";

/// pi `RecordModelFailureOptions` (`model-exclusions.ts:17-21`).
#[derive(Debug, Clone)]
pub struct RecordModelFailure {
    pub target: ModelExclusionTarget,
    pub reason: Option<String>,
    /// pi `ttlMs` — a per-record override that is **capped** by the configured default, never
    /// merely defaulted to it (`:211`'s `Math.min(options.ttlMs ?? defaultTTLMs, defaultTTLMs)`).
    /// A caller cannot exclude a model for longer than the operator allowed.
    pub ttl_ms: Option<i64>,
    /// pi `preserveExisting` (`:217`, `:224-228`): when set, an unexpired entry under the same
    /// dedup key makes the whole call a no-op.
    ///
    /// Its only caller is the provisioning arm of
    /// [`crate::exec::fallback::record_retryable_model_failure`] — omit it and every npm/preflight
    /// failure re-stamps a fresh window, which is the behaviour the flag exists to prevent.
    pub preserve_existing: bool,
}

impl RecordModelFailure {
    /// The ordinary record: this target, this reason, the configured TTL, no preservation.
    #[must_use]
    pub fn new(target: ModelExclusionTarget, reason: Option<String>) -> Self {
        Self {
            target,
            reason,
            ttl_ms: None,
            preserve_existing: false,
        }
    }
}

#[derive(Debug)]
struct StoreState {
    exclusions: Vec<ModelExclusion>,
    loaded: bool,
    default_ttl_ms: i64,
    /// pi `loadedTTLCeilingMs` (`model-exclusions.ts:31`): `Some` only when the operator explicitly
    /// set a TTL **and** asked for existing entries to be shortened.
    loaded_ttl_ceiling_ms: Option<i64>,
    /// pi `schedulePersist`'s debounced write (`model-exclusions.ts:119-127`), as a flag.
    ///
    /// Upstream's 5000 ms `setTimeout` carries `timer.unref?.()` — *"Never hold the process open
    /// just to flush exclusions"* (`:125`). A `tokio` timer with that property is a detached task
    /// the runtime does not join, which is the same thing as "write it at the next flush point"
    /// with one fewer way to lose the write at shutdown. [`ModelExclusionStore::flush_persist`]
    /// stays directly callable, which is what upstream exposes `flushPersist` for.
    dirty: bool,
}

/// The model-exclusion registry — pi `model-exclusions.ts`'s module state, owned.
#[derive(Debug)]
pub struct ModelExclusionStore {
    exclusions_path: PathBuf,
    /// `<agent_dir>/auth.json` — pi `path.join(getAgentDir(), "auth.json")` (`:50`), resolved
    /// through [`crate::paths::Roots::agent_dir`] rather than a literal join, so a sandboxed roots
    /// value isolates the auth-mtime rule along with everything else.
    auth_store_path: PathBuf,
    state: Mutex<StoreState>,
}

impl ModelExclusionStore {
    /// The store rooted on a resolved [`crate::paths::Roots`].
    #[must_use]
    pub fn new(roots: &crate::paths::Roots) -> Self {
        Self::with_paths(
            persist::exclusions_file_path(roots),
            roots.agent_dir().join("auth.json"),
        )
    }

    /// The store rooted on the process environment — the detached background runner's constructor,
    /// which holds no executor to inherit one from and reaches the SAME file the foreground wrote.
    #[must_use]
    pub fn from_env() -> Self {
        Self::new(&crate::paths::Roots::from_env())
    }

    /// Both paths given explicitly.
    #[must_use]
    pub fn with_paths(exclusions_path: PathBuf, auth_store_path: PathBuf) -> Self {
        Self {
            exclusions_path,
            auth_store_path,
            state: Mutex::new(StoreState {
                exclusions: Vec::new(),
                loaded: false,
                default_ttl_ms: DEFAULT_MODEL_EXCLUSION_TTL_MS,
                loaded_ttl_ceiling_ms: None,
                dirty: false,
            }),
        }
    }

    /// Where this store persists — pi `getExclusionsFilePath` (`model-exclusions.ts:93-97`).
    #[must_use]
    pub fn exclusions_path(&self) -> &Path {
        &self.exclusions_path
    }

    /// pi `setDefaultTTL` (`model-exclusions.ts:79-86`).
    ///
    /// `shorten_existing` retroactively lowers every loaded entry's expiry to
    /// `recorded_at + ms`; without it, lowering the TTL has no effect until every existing entry
    /// expires under the old one. It is wired to *"the operator set the key"*, never to a bare
    /// `true` — pi's condition is `config.modelExclusions?.defaultTtlMs !== undefined`
    /// (`extension/config.ts:239`), so the built-in default never shortens anything.
    ///
    /// # Errors
    ///
    /// Upstream's message verbatim (`:81`) for a non-positive TTL or one above
    /// [`MAX_MODEL_EXCLUSION_TTL_MS`].
    pub fn set_default_ttl(&self, ms: i64, shorten_existing: bool) -> Result<(), String> {
        validate_model_exclusion_ttl(ms)?;
        let Ok(mut state) = self.state.lock() else {
            return Ok(());
        };
        state.default_ttl_ms = ms;
        state.loaded_ttl_ceiling_ms = shorten_existing.then_some(ms);
        if state.loaded
            && let Some(ceiling) = state.loaded_ttl_ceiling_ms
        {
            let now = crate::time::now_epoch_millis();
            if shorten_exclusions_to_ttl(&mut state.exclusions, ceiling, now) {
                state.dirty = true;
            }
        }
        Ok(())
    }

    /// The TTL a new record gets when it names none.
    #[must_use]
    pub fn default_ttl_ms(&self) -> i64 {
        self.state
            .lock()
            .map_or(DEFAULT_MODEL_EXCLUSION_TTL_MS, |state| state.default_ttl_ms)
    }

    /// pi `recordModelFailure` (`model-exclusions.ts:209-235`).
    ///
    /// Order matters and is upstream's: `unshift` → `deduplicate` → truncate at 200. Deduping
    /// BEFORE truncating is what keeps a duplicate from evicting a distinct entry, and `unshift`
    /// plus `deduplicate`'s first-seen-key ordering is what keeps the freshly recorded entry at
    /// index 0.
    ///
    /// Persists IMMEDIATELY rather than through the debounce — upstream calls `flushPersist()` here
    /// (`:234`), not `schedulePersist()`, because the whole point of the record is that the NEXT
    /// process must see it.
    pub async fn record_model_failure(&self, options: RecordModelFailure) {
        let snapshot = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            self.ensure_loaded_locked(&mut state);

            let now = crate::time::now_epoch_millis();
            // `Math.min(options.ttlMs ?? defaultTTLMs, defaultTTLMs)` (`:211`) — a cap, not a
            // default: a caller may shorten its own record but never extend it past the operator's
            // configured ceiling.
            let ttl = options
                .ttl_ms
                .map_or(state.default_ttl_ms, |ttl| ttl.min(state.default_ttl_ms));
            let dedup_key = options.target.dedup_key();

            if options.preserve_existing
                && state
                    .exclusions
                    .iter()
                    .any(|entry| entry.target.dedup_key() == dedup_key && entry.expires_at > now)
            {
                return;
            }

            state.exclusions.insert(
                0,
                ModelExclusion {
                    target: options.target,
                    reason: Some(
                        options
                            .reason
                            .unwrap_or_else(|| DEFAULT_EXCLUSION_REASON.to_string()),
                    ),
                    recorded_at: now,
                    // `saturating_add` rather than `+`: a corrupt clock plus the maximum TTL is the
                    // one input that could overflow, and this crate's clock policy is clamp,
                    // never panic (`crate::time`).
                    expires_at: now.saturating_add(ttl),
                },
            );
            let deduplicated = deduplicate(std::mem::take(&mut state.exclusions));
            state.exclusions = deduplicated;
            state.exclusions.truncate(MAX_STORED_EXCLUSIONS);
            state.dirty = false;
            state.exclusions.clone()
        };
        self.persist_snapshot(snapshot).await;
    }

    /// pi `flushPersist` (`model-exclusions.ts:104-117`) — write now, whatever the debounce thinks.
    ///
    /// A no-op when nothing is pending, so the ladder can call it unconditionally at its boundary.
    /// A failure is logged and swallowed exactly as upstream's is (`:114-116`): the store is a cache
    /// of failures, and being unable to write it must never fail a run.
    pub async fn flush_persist(&self) {
        let snapshot = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            if !state.dirty {
                return;
            }
            state.dirty = false;
            state.exclusions.clone()
        };
        self.persist_snapshot(snapshot).await;
    }

    async fn persist_snapshot(&self, snapshot: Vec<ModelExclusion>) {
        // `deduplicate` again on the way out, exactly as `flushPersist` does (`:111`): the on-disk
        // document is canonical even if some future in-memory path forgets to dedupe.
        if let Err(error) = persist::persist(&self.exclusions_path, &deduplicate(snapshot)).await {
            tracing::warn!(
                path = %self.exclusions_path.display(),
                %error,
                "[model-exclusions] Failed to persist exclusions"
            );
        }
    }

    /// pi `clearExpiredExclusions` (`model-exclusions.ts:237-242`).
    pub fn clear_expired_exclusions(&self) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        self.ensure_loaded_locked(&mut state);
        if invalidate_auth_exclusions(&mut state.exclusions, &self.auth_store_path) {
            state.dirty = true;
        }
        let now = crate::time::now_epoch_millis();
        state.exclusions.retain(|entry| entry.expires_at > now);
        state.dirty = true;
    }

    /// pi `clearExclusions` (`model-exclusions.ts:247-251`) — every entry, e.g. after the operator
    /// fixes credentials by hand rather than by touching the auth store.
    pub fn clear_exclusions(&self) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        self.ensure_loaded_locked(&mut state);
        state.exclusions.clear();
        state.dirty = true;
    }

    /// pi `isExcluded` (`model-exclusions.ts:275-279`). Note the signature: ALREADY-SPLIT parts,
    /// not a full id — [`parse_model_key`] is the caller's job, so the split happens once.
    #[must_use]
    pub fn is_excluded(&self, model_id: &str, provider: Option<&str>) -> bool {
        let now = crate::time::now_epoch_millis();
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        self.ensure_loaded_locked(&mut state);
        if invalidate_auth_exclusions(&mut state.exclusions, &self.auth_store_path) {
            state.dirty = true;
        }
        state
            .exclusions
            .iter()
            .any(|entry| entry.matches(model_id, provider, now))
    }

    /// pi `findModelExclusion` (`model-exclusions.ts:287-295`) — the active exclusion matching a
    /// FULL id, for hard-fail diagnostics.
    #[must_use]
    pub fn find_model_exclusion(
        &self,
        full_id: &ModelId,
        now: Option<i64>,
        ignore_exclusion: Option<crate::exec::model_exclusions::filter::ExclusionOverride<'_>>,
    ) -> Option<ModelExclusion> {
        let now = now.unwrap_or_else(crate::time::now_epoch_millis);
        self.find_active_exclusion(full_id, now, ignore_exclusion)
    }

    /// The shared core of [`Self::find_model_exclusion`] and
    /// [`super::filter::filter_fallback_candidates`]: one lock, one auth sweep, one lookup.
    ///
    /// `pub(crate)` rather than private because the filter lives in its own file for the same
    /// reason upstream splits it out — but the two MUST read the store identically, so they read it
    /// through one function rather than two that happen to agree today.
    pub(crate) fn find_active_exclusion(
        &self,
        candidate: &ModelId,
        now: i64,
        ignore_exclusion: Option<crate::exec::model_exclusions::filter::ExclusionOverride<'_>>,
    ) -> Option<ModelExclusion> {
        let key = parse_model_key(candidate.as_str());
        let Ok(mut state) = self.state.lock() else {
            return None;
        };
        self.ensure_loaded_locked(&mut state);
        if invalidate_auth_exclusions(&mut state.exclusions, &self.auth_store_path) {
            state.dirty = true;
        }
        state
            .exclusions
            .iter()
            .find(|entry| {
                entry.matches(
                    key.model_id.as_str(),
                    key.provider.as_ref().map(ProviderId::as_str),
                    now,
                ) && !ignore_exclusion.is_some_and(|ignore| ignore(candidate, entry))
            })
            .cloned()
    }

    /// pi `getExcludedCount` (`model-exclusions.ts:301-305`). **Mutates**, exactly as upstream does:
    /// it prunes first, so the number reported is live rather than historical.
    #[must_use]
    pub fn excluded_count(&self) -> usize {
        self.clear_expired_exclusions();
        self.state.lock().map_or(0, |state| state.exclusions.len())
    }

    /// pi `reloadFromDisk` (`model-exclusions.ts:356-360`) — for tests and config hot-reload.
    /// Discards any in-memory-only exclusion that was not yet persisted.
    pub fn reload_from_disk(&self) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.loaded = false;
        state.exclusions.clear();
        self.ensure_loaded_locked(&mut state);
    }

    /// pi `ensureLoaded` (`model-exclusions.ts:129-158`), whose ORDER is not obvious and is ported
    /// exactly: parse → drop everything already expired → shorten to the configured ceiling if one
    /// is set → deduplicate → mark for persist → `invalidateAuthExclusions`.
    ///
    /// [CYRUP-DELTA] upstream distinguishes the two persist urgencies here —
    /// `flushPersist()` when an entry was INVALID, `schedulePersist()` when one was merely shortened
    /// (`:150-151`) — because it can write synchronously. Both land on the same dirty flag here,
    /// since the writer is `async` and this function is on the synchronous read path (see the module
    /// doc). The distinction only ever affected WHEN the rewrite happened, never whether: the next
    /// [`Self::flush_persist`] or [`Self::record_model_failure`] writes the cleaned document either
    /// way, and a corrupt entry is dropped from memory the instant it is read regardless.
    fn ensure_loaded_locked(&self, state: &mut StoreState) {
        if state.loaded {
            return;
        }
        state.loaded = true;
        if let Some(loaded) = persist::load_from_disk(&self.exclusions_path) {
            let now = crate::time::now_epoch_millis();
            let mut entries = loaded.entries;
            entries.retain(|entry| entry.expires_at > now);
            let shortened = state
                .loaded_ttl_ceiling_ms
                .is_some_and(|ceiling| shorten_exclusions_to_ttl(&mut entries, ceiling, now));
            state.exclusions = deduplicate(entries);
            if loaded.dropped_invalid || shortened {
                state.dirty = true;
            }
        }
        if invalidate_auth_exclusions(&mut state.exclusions, &self.auth_store_path) {
            state.dirty = true;
        }
    }
}

/// pi `setDefaultTTL`'s guard (`model-exclusions.ts:80-82`), split out so the config layer can
/// refuse at the tool boundary with the same shape [`crate::exec::usage_budget`] uses — upstream
/// validates in BOTH places (`extension/config.ts:106-107` and again inside `setDefaultTTL`), and
/// the double check is deliberate: the config path is not the only caller.
///
/// # Errors
///
/// Upstream's message verbatim, including its trailing period.
pub fn validate_model_exclusion_ttl(ms: i64) -> Result<(), String> {
    if ms <= 0 || ms > MAX_MODEL_EXCLUSION_TTL_MS {
        return Err(format!(
            "Default model exclusion TTL must be a finite positive number no greater than {MAX_MODEL_EXCLUSION_TTL_MS}."
        ));
    }
    Ok(())
}

/// pi `validateModelExclusionsConfig` (`extension/config.ts:101-109`) — the CONFIG-layer guard,
/// which runs before [`ModelExclusionStore::set_default_ttl`] ever sees the value.
///
/// Upstream validates in both places and so does this port: the config path is not the only caller,
/// and the two messages are deliberately different — this one names the config key and, unlike
/// [`validate_model_exclusion_ttl`], carries **no trailing period**. That asymmetry is upstream's
/// (`config.ts:107` against `model-exclusions.ts:81`); reproducing it keeps an operator's search for
/// the exact sentence they saw landing on the right layer.
///
/// # Errors
///
/// Upstream's message verbatim for a non-positive TTL or one above [`MAX_MODEL_EXCLUSION_TTL_MS`].
pub fn validate_model_exclusions_config(
    config: Option<&crate::registration::ModelExclusionsConfig>,
) -> Result<(), String> {
    let Some(default_ttl_ms) = config.and_then(|config| config.default_ttl_ms) else {
        return Ok(());
    };
    if default_ttl_ms <= 0 || default_ttl_ms > MAX_MODEL_EXCLUSION_TTL_MS {
        return Err(format!(
            "config.modelExclusions.defaultTtlMs must be a finite positive number no greater than {MAX_MODEL_EXCLUSION_TTL_MS}"
        ));
    }
    Ok(())
}

/// pi `resolveModelExclusionTTL` (`extension/config.ts:227-229`): the configured TTL, or the
/// built-in 24 h.
#[must_use]
pub fn resolve_model_exclusion_ttl(
    config: Option<&crate::registration::ModelExclusionsConfig>,
) -> i64 {
    config
        .and_then(|config| config.default_ttl_ms)
        .unwrap_or(DEFAULT_MODEL_EXCLUSION_TTL_MS)
}

/// pi `applyModelExclusionsConfig` (`extension/config.ts:237-240`).
///
/// `shorten_existing` is `config.modelExclusions?.defaultTtlMs !== undefined` — **the operator set
/// the key**, never a bare `true`. Without that condition the built-in default would retroactively
/// rewrite every entry on disk on every start; with it, only an operator lowering the TTL does.
///
/// # Errors
///
/// [`validate_model_exclusions_config`]'s message, so a malformed value is refused at the boundary
/// rather than silently ignored.
pub fn apply_model_exclusions_config(
    store: &ModelExclusionStore,
    config: Option<&crate::registration::ModelExclusionsConfig>,
) -> Result<(), String> {
    validate_model_exclusions_config(config)?;
    let configured = config.and_then(|config| config.default_ttl_ms);
    store.set_default_ttl(resolve_model_exclusion_ttl(config), configured.is_some())
}

/// pi `deduplicate` (`model-exclusions.ts:191-202`): one entry per [`ModelExclusionTarget::
/// dedup_key`], keeping the one with the GREATER `recorded_at`, in first-seen key order.
///
/// Both halves are load-bearing. The tie-break is why a fresh record wins over a stale one for the
/// same model; the ordering is why — combined with `record_model_failure`'s `insert(0, …)` — that
/// fresh record stays at index 0 and therefore survives the 200-entry truncate.
fn deduplicate(items: Vec<ModelExclusion>) -> Vec<ModelExclusion> {
    let mut keys: Vec<String> = Vec::with_capacity(items.len());
    let mut out: Vec<ModelExclusion> = Vec::with_capacity(items.len());
    for entry in items {
        let key = entry.target.dedup_key();
        match keys.iter().position(|existing| *existing == key) {
            Some(index) => {
                if let Some(existing) = out.get_mut(index)
                    && entry.recorded_at > existing.recorded_at
                {
                    *existing = entry;
                }
            }
            None => {
                keys.push(key);
                out.push(entry);
            }
        }
    }
    out
}

/// pi `shortenExclusionsToTTL` (`model-exclusions.ts:373-385`): clamp every expiry to
/// `recorded_at + ttl_ms`, then prune. `true` when anything changed — either an expiry moved or an
/// entry fell out of the pruned window — which is the caller's signal to persist.
fn shorten_exclusions_to_ttl(items: &mut Vec<ModelExclusion>, ttl_ms: i64, now: i64) -> bool {
    let mut changed = false;
    for entry in items.iter_mut() {
        let configured_expiry = entry.recorded_at.saturating_add(ttl_ms);
        if entry.expires_at > configured_expiry {
            entry.expires_at = configured_expiry;
            changed = true;
        }
    }
    let previous_length = items.len();
    items.retain(|entry| entry.expires_at > now);
    changed || items.len() != previous_length
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn sandbox() -> (tempfile::TempDir, ModelExclusionStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = ModelExclusionStore::with_paths(
            dir.path().join("model-exclusions.json"),
            dir.path().join("auth.json"),
        );
        (dir, store)
    }

    fn model_target(model_id: &str, provider: Option<&str>) -> ModelExclusionTarget {
        ModelExclusionTarget::Model {
            model_id: ModelId::from(model_id),
            provider: provider.map(ProviderId::from),
        }
    }

    fn entry(model_id: &str, recorded_at: i64, expires_at: i64) -> ModelExclusion {
        ModelExclusion {
            target: model_target(model_id, Some("openai")),
            reason: Some("429".to_string()),
            recorded_at,
            expires_at,
        }
    }

    #[tokio::test]
    async fn a_recorded_failure_excludes_the_model_and_survives_a_reload() {
        let (_dir, store) = sandbox();
        store
            .record_model_failure(RecordModelFailure::new(
                model_target("gpt-4", Some("openai")),
                Some("429 rate limit".to_string()),
            ))
            .await;
        assert!(store.is_excluded("gpt-4", Some("openai")));

        store.reload_from_disk();
        assert!(store.is_excluded("gpt-4", Some("openai")));
        assert!(!store.is_excluded("gpt-4", Some("anthropic")));
    }

    #[tokio::test]
    async fn an_absent_reason_is_stored_as_upstreams_runtime_failure_literal() {
        let (_dir, store) = sandbox();
        store
            .record_model_failure(RecordModelFailure::new(model_target("gpt-4", None), None))
            .await;
        let found = store
            .find_model_exclusion(&ModelId::from("gpt-4"), None, None)
            .unwrap();
        assert_eq!(found.reason.as_deref(), Some(DEFAULT_EXCLUSION_REASON));
    }

    #[tokio::test]
    async fn a_per_record_ttl_is_capped_by_the_default_never_extended_past_it() {
        let (_dir, store) = sandbox();
        store
            .record_model_failure(RecordModelFailure {
                ttl_ms: Some(MAX_MODEL_EXCLUSION_TTL_MS),
                ..RecordModelFailure::new(model_target("gpt-4", None), None)
            })
            .await;
        let found = store
            .find_model_exclusion(&ModelId::from("gpt-4"), None, None)
            .unwrap();
        assert!(
            found.expires_at - found.recorded_at <= DEFAULT_MODEL_EXCLUSION_TTL_MS,
            "a caller extended its own exclusion past the configured default"
        );
    }

    #[tokio::test]
    async fn preserve_existing_makes_a_repeat_record_a_no_op() {
        let (_dir, store) = sandbox();
        let options = || RecordModelFailure {
            ttl_ms: Some(60_000),
            preserve_existing: true,
            ..RecordModelFailure::new(model_target("gpt-4", None), Some("npm install".to_string()))
        };
        store.record_model_failure(options()).await;
        let first = store
            .find_model_exclusion(&ModelId::from("gpt-4"), None, None)
            .unwrap();
        store.record_model_failure(options()).await;
        let second = store
            .find_model_exclusion(&ModelId::from("gpt-4"), None, None)
            .unwrap();
        assert_eq!(
            first.expires_at, second.expires_at,
            "the window was re-stamped"
        );
    }

    #[test]
    fn deduplicate_keeps_the_newer_record_in_first_seen_key_order() {
        let deduped = deduplicate(vec![
            entry("gpt-4", 10, 100),
            entry("gpt-5", 5, 100),
            entry("gpt-4", 20, 200),
        ]);
        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped.first().map(|e| e.recorded_at), Some(20));
        assert_eq!(
            deduped.first().and_then(|e| e.target.model_id().cloned()),
            Some(ModelId::from("gpt-4"))
        );
        assert_eq!(deduped.get(1).map(|e| e.recorded_at), Some(5));
    }

    #[test]
    fn shortening_clamps_to_recorded_at_plus_ttl_and_prunes_what_that_expires() {
        let mut items = vec![entry("gpt-4", 0, 1_000_000), entry("gpt-5", 0, 10)];
        assert!(shorten_exclusions_to_ttl(&mut items, 100, 50));
        assert_eq!(items.len(), 1);
        assert_eq!(items.first().map(|e| e.expires_at), Some(100));
    }

    #[test]
    fn the_ttl_validator_uses_upstreams_message_verbatim() {
        assert_eq!(
            validate_model_exclusion_ttl(0).unwrap_err(),
            "Default model exclusion TTL must be a finite positive number no greater than 8000000000000000."
        );
        assert!(validate_model_exclusion_ttl(-1).is_err());
        assert!(validate_model_exclusion_ttl(MAX_MODEL_EXCLUSION_TTL_MS + 1).is_err());
        assert!(validate_model_exclusion_ttl(1).is_ok());
        assert!(validate_model_exclusion_ttl(MAX_MODEL_EXCLUSION_TTL_MS).is_ok());
    }

    #[tokio::test]
    async fn shorten_existing_retroactively_lowers_a_loaded_entrys_expiry() {
        let (_dir, store) = sandbox();
        store
            .record_model_failure(RecordModelFailure::new(model_target("gpt-4", None), None))
            .await;
        let before = store
            .find_model_exclusion(&ModelId::from("gpt-4"), None, None)
            .unwrap();
        assert!(before.expires_at - before.recorded_at > 60_000);

        store.set_default_ttl(60_000, true).unwrap();
        let after = store
            .find_model_exclusion(&ModelId::from("gpt-4"), None, None)
            .unwrap();
        assert_eq!(after.expires_at, after.recorded_at + 60_000);
    }

    #[tokio::test]
    async fn without_shorten_existing_a_lower_ttl_leaves_loaded_entries_alone() {
        let (_dir, store) = sandbox();
        store
            .record_model_failure(RecordModelFailure::new(model_target("gpt-4", None), None))
            .await;
        let before = store
            .find_model_exclusion(&ModelId::from("gpt-4"), None, None)
            .unwrap();
        store.set_default_ttl(60_000, false).unwrap();
        let after = store
            .find_model_exclusion(&ModelId::from("gpt-4"), None, None)
            .unwrap();
        assert_eq!(before.expires_at, after.expires_at);
    }

    #[tokio::test]
    async fn the_store_caps_at_two_hundred_entries_and_keeps_the_newest() {
        let (_dir, store) = sandbox();
        for index in 0..205 {
            store
                .record_model_failure(RecordModelFailure::new(
                    model_target(&format!("gpt-{index}"), None),
                    None,
                ))
                .await;
        }
        assert_eq!(store.excluded_count(), MAX_STORED_EXCLUSIONS);
        // The last record written is still index 0 and therefore still live.
        assert!(store.is_excluded("gpt-204", None));
        assert!(!store.is_excluded("gpt-0", None));
    }

    #[tokio::test]
    async fn touching_the_auth_store_invalidates_an_older_auth_exclusion() {
        let (dir, store) = sandbox();
        store
            .record_model_failure(RecordModelFailure::new(
                ModelExclusionTarget::Provider {
                    provider: ProviderId::from("openai"),
                },
                Some("401 unauthorized".to_string()),
            ))
            .await;
        assert!(store.is_excluded("gpt-4", Some("openai")));

        // The operator fixes their credentials: the auth store is rewritten AFTER the record.
        let auth = dir.path().join("auth.json");
        std::fs::write(&auth, b"{}").unwrap();
        filetime::set_file_mtime(
            &auth,
            filetime::FileTime::from_unix_time(i64::MAX / 4_000, 0),
        )
        .unwrap();

        assert!(
            !store.is_excluded("gpt-4", Some("openai")),
            "a fixed credential should not have to wait out the TTL"
        );
    }

    #[tokio::test]
    async fn a_rate_limit_exclusion_survives_the_auth_store_being_touched() {
        let (dir, store) = sandbox();
        store
            .record_model_failure(RecordModelFailure::new(
                ModelExclusionTarget::Provider {
                    provider: ProviderId::from("openai"),
                },
                Some("429 rate limit".to_string()),
            ))
            .await;
        let auth = dir.path().join("auth.json");
        std::fs::write(&auth, b"{}").unwrap();
        filetime::set_file_mtime(
            &auth,
            filetime::FileTime::from_unix_time(i64::MAX / 4_000, 0),
        )
        .unwrap();
        assert!(store.is_excluded("gpt-4", Some("openai")));
    }

    #[tokio::test]
    async fn clear_exclusions_empties_the_store() {
        let (_dir, store) = sandbox();
        store
            .record_model_failure(RecordModelFailure::new(model_target("gpt-4", None), None))
            .await;
        store.clear_exclusions();
        assert_eq!(store.excluded_count(), 0);
        assert!(!store.is_excluded("gpt-4", None));
    }

    #[tokio::test]
    async fn an_expired_entry_is_neither_matched_nor_counted() {
        let (dir, store) = sandbox();
        let path = dir.path().join("model-exclusions.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "exclusions": [{"modelId": "gpt-4", "recordedAt": 1, "expiresAt": 2}],
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(!store.is_excluded("gpt-4", None));
        assert_eq!(store.excluded_count(), 0);
    }
}
