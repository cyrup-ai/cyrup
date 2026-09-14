//! The on-disk format, the env override, the validating read and the atomic write.
//!
//! Ports pi `getExclusionsFilePath` (`model-exclusions.ts:93-97`), `flushPersist` (`:104-117`),
//! the parse half of `ensureLoaded` (`:129-158`) and `readPersistedExclusion` (`:169-185`).
//!
//! # Why the read is validating rather than `serde`-derived
//!
//! Upstream drops a malformed entry, logs it, and keeps going — a corrupt exclusions file must
//! never be able to fail a run, because the file is a CACHE of failures, not run state. A derived
//! `Deserialize` on a `Vec<ModelExclusion>` fails the whole array on the first bad element, so the
//! read walks `serde_json::Value` and applies upstream's thirteen rules per entry, reproducing its
//! per-entry message.

use std::path::{Path, PathBuf};

use cyrup_core::{ModelId, ProviderId};

use crate::exec::model_exclusions::entry::{
    MAX_DATE_TIMESTAMP_MS, ModelExclusion, ModelExclusionTarget,
};

/// pi `EXCLUSIONS_PATH_ENV` (`model-exclusions.ts:7`) — upstream's `PI_`-prefixed spelling of this
/// same knob — under this crate's own prefix exactly as
/// [`crate::exec::thinking_ceiling::THINKING_CEILING_ENV`] is.
///
/// The legacy `PI_` spelling is deliberately NOT read — hard rename, R-07-028
/// (`cyrup-config/src/env.rs:119`, asserted by `drift050_offline_and_skip_version_check_stay_two_state`
/// at `:648-668`, whose message is "the dropped `PI_*` spelling must be inert"). Every env const in
/// this crate is a single `CYRUP_SUBAGENT_*` name with no fallback; a pair here would be the only
/// exception and would quietly re-introduce the spelling that regression test exists to keep dead.
/// [`exclusions_file_path_from`]'s `this_const_is_the_only_env_key_consulted` test states the
/// stronger property that follows: NO other key can move this path, whatever it is spelled.
pub const MODEL_EXCLUSIONS_PATH_ENV: &str = "CYRUP_SUBAGENT_MODEL_EXCLUSIONS_PATH";

/// The persisted store's file name under the run-scratch root — pi's
/// `path.join(TEMP_ROOT_DIR, "model-exclusions.json")` (`model-exclusions.ts:96`).
pub(crate) const EXCLUSIONS_FILE_NAME: &str = "model-exclusions.json";

/// pi `getExclusionsFilePath` (`model-exclusions.ts:93-97`) with its two inputs injected.
///
/// The env value is TRIMMED and a set-but-blank value falls through to the default, which is
/// upstream's own `typeof envPath === "string" && envPath.trim()` rule (`:95`) and the same rule
/// [`crate::background::artifact_roots::temp_root_dir_from`] applies for the reason stated there:
/// `PathBuf::from("")` is the RELATIVE empty path, so a blank override would root the store at the
/// process working directory rather than anywhere a later run could find it.
#[must_use]
pub(crate) fn exclusions_file_path_from(
    env: crate::paths::EnvLookup<'_>,
    run_scratch: &Path,
) -> PathBuf {
    if let Some(raw) = env(MODEL_EXCLUSIONS_PATH_ENV)
        && let Some(trimmed) = raw.to_str().map(str::trim).filter(|s| !s.is_empty())
    {
        return PathBuf::from(trimmed);
    }
    run_scratch.join(EXCLUSIONS_FILE_NAME)
}

/// [`exclusions_file_path_from`] against the process environment and a resolved
/// [`crate::paths::Roots`].
///
/// `Roots::run_scratch()` rather than `crate::background::temp_root_dir()` directly: the roots
/// value is the injectable form this crate threads everywhere, so a store built from
/// [`crate::paths::Roots::sandboxed`] is isolated by construction instead of needing the env
/// override every test would otherwise have to set.
#[must_use]
pub fn exclusions_file_path(roots: &crate::paths::Roots) -> PathBuf {
    exclusions_file_path_from(&|key| std::env::var_os(key), roots.run_scratch())
}

/// The wire shape of one entry — pi's `{modelId?, provider?, reason?, recordedAt, expiresAt}`
/// (`model-exclusions.ts:179-184`). Write-side only; the read side validates
/// [`serde_json::Value`] so one bad entry cannot fail the whole array.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistedExclusion<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    model_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'a str>,
    recorded_at: i64,
    expires_at: i64,
}

/// The document — pi `{ version: 1, exclusions: [...] }` (`model-exclusions.ts:109-112`).
#[derive(serde::Serialize)]
struct PersistedStore<'a> {
    version: u32,
    exclusions: Vec<PersistedExclusion<'a>>,
}

/// The only version this store writes, and the only one it reads. A file carrying any other value
/// is IGNORED rather than cleared or rejected (pi `:135`'s `if (data.version === 1)` has no `else`)
/// — a newer cyrup's store must survive being opened by an older one.
pub(crate) const STORE_VERSION: u32 = 1;

/// What a load produced. `dropped_invalid` drives pi's `flushPersist()`-vs-`schedulePersist()`
/// choice at `:150-151`: a file that contained garbage is rewritten IMMEDIATELY so the garbage
/// cannot be re-parsed (and re-logged) by every later process, whereas a merely shortened one can
/// wait for the debounce.
pub(crate) struct LoadedStore {
    pub(crate) entries: Vec<ModelExclusion>,
    pub(crate) dropped_invalid: bool,
}

/// pi `readPersistedExclusion` (`model-exclusions.ts:169-185`) — thirteen rules, one message each.
///
/// Three of them are unrepresentable in Rust and so are absent by construction rather than by
/// omission: `reason` is a `String` or nothing, and "must include modelId or provider" is
/// [`ModelExclusionTarget`]'s whole reason for being an enum. The NUMERIC range checks are not
/// free and are ported literally: an `expiresAt` outside `(0, MAX_DATE_TIMESTAMP_MS]` would make
/// every later `expires_at <= now` comparison meaningless, and a `recordedAt` outside it breaks the
/// auth-mtime rule that compares the two.
fn read_persisted_exclusion(
    entry: &serde_json::Value,
    index: usize,
) -> Result<ModelExclusion, String> {
    let invalid = |message: &str| Err(format!("Model exclusion store entry {index} {message}."));

    let Some(object) = entry.as_object() else {
        return invalid("must be an object");
    };

    let model_id = match object.get("modelId") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => match value.as_str().filter(|s| !s.is_empty()) {
            Some(model_id) => Some(model_id),
            None => return invalid("has an invalid modelId"),
        },
    };
    let provider = match object.get("provider") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => match value.as_str().filter(|s| !s.is_empty()) {
            Some(provider) => Some(provider),
            None => return invalid("has an invalid provider"),
        },
    };
    let reason = match object.get("reason") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => match value.as_str() {
            Some(reason) => Some(reason.to_string()),
            None => return invalid("has an invalid reason"),
        },
    };

    let Some(recorded_at) = finite_timestamp(object.get("recordedAt")) else {
        return invalid("has an invalid recordedAt");
    };
    let Some(expires_at) = finite_timestamp(object.get("expiresAt")) else {
        return invalid("has an invalid expiresAt");
    };

    let target = match (model_id, provider) {
        (Some(model_id), provider) => ModelExclusionTarget::Model {
            model_id: ModelId::from(model_id),
            provider: provider.map(ProviderId::from),
        },
        (None, Some(provider)) => ModelExclusionTarget::Provider {
            provider: ProviderId::from(provider),
        },
        // pi `:181` — the one shape the enum above makes unrepresentable everywhere else.
        (None, None) => return invalid("must include modelId or provider"),
    };

    Ok(ModelExclusion {
        target,
        reason,
        recorded_at,
        expires_at,
    })
}

/// `typeof v === "number" && Number.isFinite(v) && v > 0 && v <= MAX_DATE_TIMESTAMP_MS`
/// (`model-exclusions.ts:177-178`), landed on this crate's `i64` millisecond width.
///
/// A JSON float is accepted and truncated the way `Date` arithmetic on a fractional millisecond
/// would be, but `NaN`/`Infinity` cannot survive `serde_json`'s number type at all and a value too
/// large for `i64` fails the range check rather than wrapping.
fn finite_timestamp(value: Option<&serde_json::Value>) -> Option<i64> {
    let number = value?.as_f64()?;
    if !number.is_finite() {
        return None;
    }
    // `as` on a non-finite or out-of-range float saturates in Rust rather than being UB, and the
    // range check below rejects both saturation endpoints.
    let millis = number.trunc();
    if millis <= 0.0 || millis > MAX_DATE_TIMESTAMP_MS as f64 {
        return None;
    }
    Some(millis as i64)
}

/// The parse half of pi `ensureLoaded` (`model-exclusions.ts:132-152`).
///
/// `None` means "nothing was loaded", which upstream reaches four ways and this function
/// distinguishes only in what it logs: the file is absent (silent — the overwhelmingly common
/// first-run case, pi's `ENOENT` arm at `:155`), unreadable, unparseable, or carries a `version`
/// this build does not know (silent by design, `:135`).
pub(crate) fn load_from_disk(path: &Path) -> Option<LoadedStore> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    path = %path.display(),
                    %error,
                    "[model-exclusions] Failed to load exclusions"
                );
            }
            return None;
        }
    };
    let document: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(document) => document,
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                %error,
                "[model-exclusions] Failed to load exclusions"
            );
            return None;
        }
    };
    if document.get("version").and_then(serde_json::Value::as_u64) != Some(u64::from(STORE_VERSION))
    {
        return None;
    }
    let Some(array) = document
        .get("exclusions")
        .and_then(serde_json::Value::as_array)
    else {
        tracing::warn!(
            path = %path.display(),
            "[model-exclusions] Model exclusion store version 1 must contain an exclusions array."
        );
        return None;
    };

    let mut entries = Vec::with_capacity(array.len());
    let mut dropped_invalid = false;
    for (index, value) in array.iter().enumerate() {
        match read_persisted_exclusion(value, index) {
            Ok(entry) => entries.push(entry),
            Err(message) => {
                dropped_invalid = true;
                tracing::warn!("[model-exclusions] Ignoring invalid exclusion: {message}");
            }
        }
    }
    Some(LoadedStore {
        entries,
        dropped_invalid,
    })
}

/// pi `flushPersist` (`model-exclusions.ts:104-117`) — the `{version:1, exclusions:[…]}` document,
/// written tmp-plus-rename.
///
/// [`crate::background::atomic::write_atomic_json_creating_parent`] rather than the bare
/// `write_atomic_json`, because upstream `mkdirSync`s the parent first (`:107`) and the async cyrup
/// writer does not; the run-scratch root need not exist on a first run. The ordinary writer rather
/// than the `0600` sibling: an exclusions file records which models failed, not a credential.
///
/// # Errors
///
/// A directory-creation, write or rename failure. Upstream logs and swallows (`:114-116`); the
/// caller here does the same, so persistence failing can never fail a run.
pub(crate) async fn persist(path: &Path, entries: &[ModelExclusion]) -> std::io::Result<()> {
    let document = PersistedStore {
        version: STORE_VERSION,
        exclusions: entries
            .iter()
            .map(|entry| PersistedExclusion {
                model_id: entry.target.model_id().map(ModelId::as_str),
                provider: entry.target.provider().map(ProviderId::as_str),
                reason: entry.reason.as_deref(),
                recorded_at: entry.recorded_at,
                expires_at: entry.expires_at,
            })
            .collect(),
    };
    crate::background::atomic::write_atomic_json_creating_parent(path, &document).await
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::ffi::OsString;

    use super::*;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + use<> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |key: &str| {
            pairs
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn the_env_override_wins_and_is_trimmed() {
        let env = env_of(&[(MODEL_EXCLUSIONS_PATH_ENV, "  /tmp/custom.json  ")]);
        assert_eq!(
            exclusions_file_path_from(&env, Path::new("/scratch")),
            PathBuf::from("/tmp/custom.json")
        );
    }

    #[test]
    fn a_blank_env_override_falls_through_to_the_run_scratch_default() {
        let env = env_of(&[(MODEL_EXCLUSIONS_PATH_ENV, "   ")]);
        assert_eq!(
            exclusions_file_path_from(&env, Path::new("/scratch")),
            PathBuf::from("/scratch/model-exclusions.json")
        );
    }

    /// R-07-028's hard rename, stated as the property that actually matters: this const is the ONLY
    /// key the resolver reads. The lookup below answers with a path for EVERY key except it, so any
    /// fallback rung at all — the dropped `PI_*` spelling included — would move the answer.
    #[test]
    fn this_const_is_the_only_env_key_consulted() {
        let answer_everything_else = |key: &str| {
            (key != MODEL_EXCLUSIONS_PATH_ENV).then(|| OsString::from("/tmp/should-never-win.json"))
        };
        assert_eq!(
            exclusions_file_path_from(&answer_everything_else, Path::new("/scratch")),
            PathBuf::from("/scratch/model-exclusions.json")
        );
        assert_eq!(
            MODEL_EXCLUSIONS_PATH_ENV,
            "CYRUP_SUBAGENT_MODEL_EXCLUSIONS_PATH"
        );
    }

    #[test]
    fn every_rejection_carries_upstreams_own_message() {
        let cases: &[(serde_json::Value, &str)] = &[
            (
                serde_json::json!([]),
                "Model exclusion store entry 0 must be an object.",
            ),
            (
                serde_json::json!({"modelId": "", "recordedAt": 1, "expiresAt": 2}),
                "Model exclusion store entry 0 has an invalid modelId.",
            ),
            (
                serde_json::json!({"modelId": "m", "provider": 7, "recordedAt": 1, "expiresAt": 2}),
                "Model exclusion store entry 0 has an invalid provider.",
            ),
            (
                serde_json::json!({"modelId": "m", "reason": 7, "recordedAt": 1, "expiresAt": 2}),
                "Model exclusion store entry 0 has an invalid reason.",
            ),
            (
                serde_json::json!({"modelId": "m", "recordedAt": 0, "expiresAt": 2}),
                "Model exclusion store entry 0 has an invalid recordedAt.",
            ),
            (
                serde_json::json!({"modelId": "m", "recordedAt": 1}),
                "Model exclusion store entry 0 has an invalid expiresAt.",
            ),
            (
                serde_json::json!({"recordedAt": 1, "expiresAt": 2}),
                "Model exclusion store entry 0 must include modelId or provider.",
            ),
        ];
        for (value, expected) in cases {
            assert_eq!(read_persisted_exclusion(value, 0).unwrap_err(), *expected);
        }
    }

    #[test]
    fn a_timestamp_past_the_date_ceiling_is_rejected_rather_than_wrapped() {
        let beyond = serde_json::json!({
            "modelId": "m",
            "recordedAt": 1,
            "expiresAt": MAX_DATE_TIMESTAMP_MS as f64 + 1.0,
        });
        assert_eq!(
            read_persisted_exclusion(&beyond, 3).unwrap_err(),
            "Model exclusion store entry 3 has an invalid expiresAt."
        );
    }

    #[test]
    fn a_store_whose_version_is_not_one_is_ignored_not_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model-exclusions.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({"version": 2, "exclusions": []})).unwrap(),
        )
        .unwrap();
        assert!(load_from_disk(&path).is_none());
    }

    #[test]
    fn an_absent_store_loads_as_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_from_disk(&dir.path().join("absent.json")).is_none());
    }

    #[test]
    fn one_bad_entry_is_dropped_and_the_rest_survive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model-exclusions.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "exclusions": [
                    {"modelId": "good", "recordedAt": 1, "expiresAt": 2},
                    {"recordedAt": 1, "expiresAt": 2},
                    {"provider": "openai", "reason": "429", "recordedAt": 3, "expiresAt": 4},
                ],
            }))
            .unwrap(),
        )
        .unwrap();
        let loaded = load_from_disk(&path).unwrap();
        assert!(loaded.dropped_invalid);
        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(
            loaded.entries.first().map(|e| e.target.clone()),
            Some(ModelExclusionTarget::Model {
                model_id: ModelId::from("good"),
                provider: None,
            })
        );
    }

    #[tokio::test]
    async fn the_round_trip_preserves_every_field_and_the_wire_shape() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("model-exclusions.json");
        let entries = vec![
            ModelExclusion {
                target: ModelExclusionTarget::Model {
                    model_id: ModelId::from("gpt-4"),
                    provider: Some(ProviderId::from("openai")),
                },
                reason: Some("429 rate limit".to_string()),
                recorded_at: 10,
                expires_at: 20,
            },
            ModelExclusion {
                target: ModelExclusionTarget::Provider {
                    provider: ProviderId::from("anthropic"),
                },
                reason: None,
                recorded_at: 30,
                expires_at: 40,
            },
        ];
        persist(&path, &entries).await.unwrap();

        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            raw.get("version").and_then(serde_json::Value::as_u64),
            Some(1)
        );
        let array = raw
            .get("exclusions")
            .and_then(serde_json::Value::as_array)
            .unwrap();
        assert_eq!(array.len(), 2);
        // An absent half is OMITTED, never `null` — upstream spreads the key in conditionally.
        assert!(array.get(1).and_then(|e| e.get("modelId")).is_none());
        assert!(array.get(1).and_then(|e| e.get("reason")).is_none());

        assert_eq!(load_from_disk(&path).unwrap().entries, entries);
    }
}
