//! Read-modify-**write** of the `subagents.agentOverrides.<name>` block of a `settings.json`
//! (SUBA-005) — a port of pi-subagents' three settings writers
//! (`src/agents/agents.ts:1276-1371` at v0.34.0):
//!
//! | pi | here |
//! |---|---|
//! | `mergeBuiltinAgentOverride(cwd, name, scope, fields)` | [`merge_builtin_agent_override`] |
//! | `removeBuiltinAgentOverride(cwd, name, scope)` | [`remove_builtin_agent_override`] |
//! | `removeBuiltinAgentOverrideFields(cwd, name, scope, fields)` | [`remove_builtin_agent_override_fields`] |
//!
//! pi resolves the target path from `(cwd, scope)` internally via
//! `getUserAgentSettingsPath()`/`getProjectAgentSettingsPath(cwd)`; cyrup already carries both
//! resolved paths on [`crate::discovery::types::LayeredOverrideSettings`] (`user_settings_path` /
//! `project_settings_path`), so these functions take the path directly and the scope→path decision
//! stays in the one place that already owns it. That also keeps this module free of any
//! directory-resolution logic of its own, matching `discovery/mod.rs`'s standing rule.
//!
//! **Everything here goes through [`serde_json::Value`], never the typed
//! [`crate::discovery::types::SubagentSettings`]** — and that is load-bearing, not stylistic. A
//! `settings.json` is cyrup's *whole* settings document (`subagents` is one key among many, and the
//! `subagents` block itself may carry keys this crate's version does not know). Round-tripping it
//! through the typed struct would serialize back only the fields that struct declares, silently
//! deleting every unrelated key in the file — turning "disable one agent" into "wipe the user's
//! settings". The untyped path shallow-merges into, or deletes from, exactly one nested object and
//! preserves every sibling byte-for-byte modulo re-serialization.
//!
//! Empty-container pruning mirrors pi exactly: removing the last field of an override entry drops
//! the entry, dropping the last entry drops `agentOverrides`, and dropping the last `subagents` key
//! drops `subagents` — so an enable/reset round-trip leaves a settings file that is byte-identical
//! in structure to one that never had the override (no `"subagents": {}` residue).

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::error::SubagentError;

/// pi `readSettingsFileStrict` (`agents.ts:683-704`): an **absent** file reads as the empty object
/// (the common "no settings yet" case, not an error); an unreadable file, a file that does not
/// parse as JSON, and a file whose top level is not a JSON object each abort with that function's
/// verbatim message. The messages are shared with `read_subagent_settings_file`'s reader-side
/// equivalents on purpose — a malformed settings file must read the same whether discovery or a
/// management write is the one that noticed.
fn read_settings_file_strict(path: &Path) -> Result<Map<String, Value>, SubagentError> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => {
            return Err(SubagentError::MalformedSettings(format!(
                "Failed to read settings file '{}': {e}",
                path.display()
            )));
        }
    };
    let parsed: Value = serde_json::from_str(&raw).map_err(|e| {
        SubagentError::MalformedSettings(format!(
            "Failed to parse settings file '{}': {e}",
            path.display()
        ))
    })?;
    match parsed {
        Value::Object(map) => Ok(map),
        _ => Err(SubagentError::MalformedSettings(format!(
            "Settings file '{}' must contain a JSON object.",
            path.display()
        ))),
    }
}

/// SUBA-029 — the RAII cross-process lock held across the whole read-modify-write of one
/// `settings.json`.
///
/// This is a `cyrup-original` finding: pi's `agents.ts` is likewise unlocked, so the bar being
/// missed is **cyrup's own** — `cyrup-config`'s `FileSettingsStore::with_lock` takes a `FileLock`
/// plus `write_atomic` for settings files. Without it two concurrent `disable`/`enable`/`reset`
/// actions can lose one another's write (both read the same base, the second write wins), or leave
/// a truncated `settings.json` if the process dies mid-`fs::write` — disabling every agent until
/// the file is hand-repaired.
///
/// The lock lives on a sidecar `<path>.lock`, so it survives the atomic rename over the target.
/// SUBA-171: every caller passes the PHYSICAL write target ([`settings_write_target`]), as pi's
/// `withSettingsFileLease` resolves the target before it takes the lease
/// (`src/shared/settings-file-lease.ts:33-36` @ad11b7ab), so a symlinked `settings.json` is locked
/// on its target's sidecar. `FileSettingsStore::with_lock` locks the path it is given, so for a
/// symlinked file the two take DIFFERENT sidecars and do not exclude each other; for a regular file
/// they are the same lock.
async fn lock_settings_file(path: &Path) -> Result<cyrup_config::lock::FileLock, SubagentError> {
    // `None`: this crate holds no `CancelToken` at these sites, and every non-`models_store`
    // caller of `FileLock::acquire` passes `None` for the same reason.
    cyrup_config::lock::FileLock::acquire(path, None)
        .await
        .map_err(|e| {
            SubagentError::MalformedSettings(format!(
                "Failed to lock settings file '{}': {e}",
                path.display()
            ))
        })
}

/// SUBA-171 — pi `resolveSettingsWriteTarget` (`src/shared/settings-file-lease.ts:6-30`
/// @ad11b7ab): the physical file a settings path writes to, following file and directory links,
/// including a dangling file link (whose link text names the file to create). A missing target is
/// allowed only when its physical parent exists; a path ending in a separator cannot name a new
/// settings file. The atomic rename then lands on the link's TARGET and the link stays in place.
fn resolve_settings_write_target(file_path: &Path) -> std::io::Result<PathBuf> {
    let mut target = file_path.to_path_buf();
    loop {
        match std::fs::canonicalize(&target) {
            Ok(resolved) => return Ok(resolved),
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            Err(e) => {
                let text = target.as_os_str().to_string_lossy();
                if text.ends_with('/') || text.ends_with(std::path::MAIN_SEPARATOR) {
                    return Err(e);
                }
            }
        }
        let parent = match target.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("."),
        };
        let parent_path = std::fs::canonicalize(parent)?;
        let Some(base) = target.file_name() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("settings path '{}' names no file", target.display()),
            ));
        };
        let unresolved = parent_path.join(base);
        let link_text = match std::fs::read_link(&unresolved) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(unresolved),
            Err(e) => return Err(e),
        };
        // Keep the link text intact (no normalization) so the filesystem follows directory links
        // before any `..`, as upstream's string join does.
        target = if link_text.is_absolute() {
            link_text
        } else {
            parent_path.join(link_text)
        };
    }
}

/// SUBA-171 — `mkdir -p` the parent of `path`, then resolve the physical write target. pi does
/// both before it takes the settings lease (`withSettingsFileLease`,
/// `src/shared/settings-file-lease.ts:33-36` @ad11b7ab), so the lease, the read-modify-write and
/// the rename all act on the same physical file whether it is reached directly or through a link.
fn settings_write_target(path: &Path) -> Result<PathBuf, SubagentError> {
    let fail = |e: std::io::Error| {
        SubagentError::MalformedSettings(format!(
            "Failed to write settings file '{}': {e}",
            path.display()
        ))
    };
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(fail)?;
    }
    resolve_settings_write_target(path).map_err(fail)
}

/// pi `writeSettingsFile` (`src/agents/agents.ts:950-978` @ad11b7ab): write
/// `JSON.stringify(settings, null, 2) + "\n"` atomically. The two-space indent and the trailing
/// newline are matched exactly so a cyrup-written settings file is diff-clean against a pi-written
/// one.
///
/// SUBA-029: the delivery is temp-then-rename, so an interrupted save leaves the previous settings
/// readable. SUBA-171: `target` is the PHYSICAL file ([`settings_write_target`]), so a symlinked
/// `settings.json` keeps its link and the save lands on the link's target; an existing file's mode
/// (`stat().mode & 0o7777`) is checked for write access (pi `accessSync(W_OK)`) and carried onto
/// the temp before the rename, so a `0600` file stays `0600` and a read-only file is refused
/// instead of replaced. A new file gets the umask default, as pi's does.
fn write_settings_file(
    path: &Path,
    target: &Path,
    settings: &Map<String, Value>,
) -> Result<(), SubagentError> {
    let fail = |e: &dyn std::fmt::Display| {
        SubagentError::MalformedSettings(format!(
            "Failed to write settings file '{}': {e}",
            path.display()
        ))
    };
    let existing_mode = match std::fs::metadata(target) {
        Ok(meta) => Some(file_mode(&meta)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(fail(&e)),
    };
    if existing_mode.is_some() {
        // pi `fs.accessSync(targetPath, W_OK)`. This crate forbids `unsafe`, so the probe is an
        // open-for-write that neither truncates nor creates: it is refused exactly when `access(2)`
        // with `W_OK` would be for a regular file the caller owns or is permitted to write.
        std::fs::OpenOptions::new()
            .write(true)
            .open(target)
            .map_err(|e| fail(&e))?;
    }
    let mut body = serde_json::to_string_pretty(&Value::Object(settings.clone()))
        .map_err(|e| SubagentError::Spawn(std::io::Error::other(e)))?;
    body.push('\n');
    cyrup_config::lock::write_atomic_with_mode(target, body.as_bytes(), existing_mode)
        .map_err(|e| fail(&e))
}

/// `stat().mode & 0o7777` on unix; `None`-equivalent elsewhere, where a file has no mode bits and
/// `write_atomic_with_mode` ignores the value.
#[cfg(unix)]
fn file_mode(meta: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o7777
}

#[cfg(not(unix))]
fn file_mode(_meta: &std::fs::Metadata) -> u32 {
    0
}

/// Borrow `settings.subagents.agentOverrides` as an object, if all three levels are objects.
fn overrides_of(settings: &Map<String, Value>) -> Option<&Map<String, Value>> {
    settings
        .get("subagents")?
        .as_object()?
        .get("agentOverrides")?
        .as_object()
}

/// Re-attach a (possibly now-empty) `agentOverrides` map under `subagents`, pruning empties exactly
/// as pi does: an empty `agentOverrides` is **deleted** rather than written as `{}`, and a
/// `subagents` block left with no keys at all is deleted in turn.
fn store_overrides(settings: &mut Map<String, Value>, next_overrides: Map<String, Value>) {
    let mut subagents = settings
        .get("subagents")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if next_overrides.is_empty() {
        subagents.remove("agentOverrides");
    } else {
        subagents.insert("agentOverrides".to_string(), Value::Object(next_overrides));
    }
    if subagents.is_empty() {
        settings.remove("subagents");
    } else {
        settings.insert("subagents".to_string(), Value::Object(subagents));
    }
}

/// pi `mergeBuiltinAgentOverride` (`agents.ts:1301-1327`): shallow-merge `fields` into
/// `subagents.agentOverrides.<name>` in the settings file at `path`, creating every missing level,
/// and return that path.
///
/// "Shallow" is pi's semantics verbatim: an existing entry's other fields survive, a same-named
/// field is replaced outright (no recursive merge into a nested value). A non-object value already
/// sitting at `subagents`, `agentOverrides`, or the entry itself is treated as absent and replaced
/// — pi's `typeof x === "object" && !Array.isArray(x)` guards do the same, and discovery already
/// tolerates those shapes rather than erroring on them.
///
/// # Errors
///
/// [`SubagentError::MalformedSettings`] if the file exists but is unreadable / not JSON / not a JSON
/// object; [`SubagentError::Spawn`] on a genuine write failure.
pub async fn merge_builtin_agent_override(
    path: &Path,
    name: &str,
    fields: &Map<String, Value>,
) -> Result<(), SubagentError> {
    // SUBA-029: ONE lock spans the read AND the write, so the read-modify-write is atomic against
    // a concurrent management action on the same file rather than three independent operations.
    let target = settings_write_target(path)?;
    let _lock = lock_settings_file(&target).await?;
    let mut settings = read_settings_file_strict(path)?;
    let mut next_overrides = overrides_of(&settings).cloned().unwrap_or_default();
    let mut entry = next_overrides
        .get(name)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for (key, value) in fields {
        entry.insert(key.clone(), value.clone());
    }
    next_overrides.insert(name.to_string(), Value::Object(entry));
    store_overrides(&mut settings, next_overrides);
    write_settings_file(path, &target, &settings)
}

/// pi `removeBuiltinAgentOverride` (`agents.ts:1276-1299`): delete the WHOLE
/// `subagents.agentOverrides.<name>` entry. Returns whether anything was actually removed — the
/// caller (`reset`) reports "Removed &lt;scope&gt; settings override at &lt;path&gt;" only on `true`,
/// and pi distinguishes the same way.
///
/// An absent file, an absent/non-object `subagents`, an absent/non-object `agentOverrides`, or an
/// absent entry all return `false` **without writing anything** — a no-op reset must not create or
/// rewrite a settings file.
///
/// # Errors
///
/// As [`merge_builtin_agent_override`].
pub async fn remove_builtin_agent_override(path: &Path, name: &str) -> Result<bool, SubagentError> {
    // SUBA-029: held across read+write; see `lock_settings_file`.
    let target = settings_write_target(path)?;
    let _lock = lock_settings_file(&target).await?;
    let mut settings = read_settings_file_strict(path)?;
    let Some(overrides) = overrides_of(&settings) else {
        return Ok(false);
    };
    if !overrides.contains_key(name) {
        return Ok(false);
    }
    let mut next_overrides = overrides.clone();
    next_overrides.remove(name);
    store_overrides(&mut settings, next_overrides);
    write_settings_file(path, &target, &settings)?;
    Ok(true)
}

/// SUBA-100 — pi `removeBuiltinAgentOverride(cwd, name, scope, { preserveMachine: true })`
/// (`agents.ts:1631-1658` @v0.68.0), which is what `reset` calls (`agent-management.ts:1387`):
/// the entry is cleared, EXCEPT a stated `machine` (a non-blank string, or `false`), which is kept
/// as the entry's only field — machine placement is where an agent runs, not a customization of
/// what it is. Returns `(removed, machine_preserved)`; `removed == false` wrote nothing.
///
/// # Errors
///
/// As [`merge_builtin_agent_override`].
pub async fn reset_builtin_agent_override_preserving_machine(
    path: &Path,
    name: &str,
) -> Result<(bool, bool), SubagentError> {
    let target = settings_write_target(path)?;
    let _lock = lock_settings_file(&target).await?;
    let mut settings = read_settings_file_strict(path)?;
    let Some(overrides) = overrides_of(&settings) else {
        return Ok((false, false));
    };
    let Some(current) = overrides.get(name) else {
        return Ok((false, false));
    };
    let machine = current.get("machine").filter(|value| {
        **value == Value::Bool(false) || value.as_str().is_some_and(|s| !s.trim().is_empty())
    });
    let mut next_overrides = overrides.clone();
    let preserved = match machine {
        Some(machine) => {
            let mut entry = serde_json::Map::new();
            entry.insert("machine".to_string(), machine.clone());
            next_overrides.insert(name.to_string(), Value::Object(entry));
            true
        }
        None => {
            next_overrides.remove(name);
            false
        }
    };
    store_overrides(&mut settings, next_overrides);
    write_settings_file(path, &target, &settings)?;
    Ok((true, preserved))
}

/// pi `removeBuiltinAgentOverrideFields` (`agents.ts:1329-1371`): delete only the named `fields`
/// from `subagents.agentOverrides.<name>`, leaving the entry's other fields intact — and delete the
/// entry entirely if that emptied it. Returns whether any field was actually present and removed;
/// `false` means nothing was written.
///
/// This is what `enable` uses (removing just `disabled`), so an agent carrying a
/// `{ disabled: true, model: "…" }` override keeps its model override when re-enabled — the
/// distinguishing behavior versus [`remove_builtin_agent_override`], which `reset` uses to clear the
/// entry wholesale.
///
/// # Errors
///
/// As [`merge_builtin_agent_override`].
pub async fn remove_builtin_agent_override_fields(
    path: &Path,
    name: &str,
    fields: &[&str],
) -> Result<bool, SubagentError> {
    // SUBA-029: held across read+write; see `lock_settings_file`.
    let target = settings_write_target(path)?;
    let _lock = lock_settings_file(&target).await?;
    let mut settings = read_settings_file_strict(path)?;
    let Some(overrides) = overrides_of(&settings) else {
        return Ok(false);
    };
    let Some(entry) = overrides.get(name).and_then(Value::as_object) else {
        return Ok(false);
    };
    let mut next_entry = entry.clone();
    let mut removed = false;
    for field in fields {
        if next_entry.remove(*field).is_some() {
            removed = true;
        }
    }
    if !removed {
        return Ok(false);
    }
    let mut next_overrides = overrides.clone();
    if next_entry.is_empty() {
        next_overrides.remove(name);
    } else {
        next_overrides.insert(name.to_string(), Value::Object(next_entry));
    }
    store_overrides(&mut settings, next_overrides);
    write_settings_file(path, &target, &settings)?;
    Ok(true)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    /// SUBA-029 — the read-modify-write must be serialized and the write must be atomic.
    ///
    /// THE USER ACTION: two management actions land at once (a `/subagents` disable while a
    /// background reset finishes, or two sessions in the same repo). Before the fix
    /// `write_settings_file` was `create_dir_all` + a bare `std::fs::write`, and
    /// `read_settings_file_strict` was a separate unlocked call — so both readers saw the same
    /// base document and the second writer silently discarded the first's change, and a crash
    /// mid-write left a truncated `settings.json` that disables every agent until hand-repaired.
    /// cyrup's own bar (`FileSettingsStore::with_lock`) is `FileLock` + `write_atomic`.
    ///
    /// Red before the fix: with the bare `fs::write` both writers read `{}` and the last write
    /// wins, so exactly ONE override survives.
    ///
    /// The writers are `tokio::spawn`ed onto a two-worker runtime rather than `std::thread::scope`d
    /// because `merge_builtin_agent_override` is `async` since the two-layer `FileLock` landed, and
    /// a plain thread closure cannot await it. `worker_threads = 2` is load-bearing for the RED
    /// case specifically: with the lock removed this function's body is entirely synchronous, so on
    /// one worker each task would run to completion in a single poll and neither could read the
    /// other's stale document. Two workers make the overlap POSSIBLE, not certain — like the
    /// `thread::scope` form it replaces this is a probabilistic race detector, measured to fire at
    /// least as often as that form did.
    ///
    /// What it contends on is `FileLock`'s layer 1, the per-path async mutex — NOT the `flock`.
    /// Layer 1 admits one task per path per process, so the second writer reaches `flock(2)` only
    /// after the first has already released it. That is DIFFERENT coverage from the pre-split
    /// thread version, not less: layer 1 is the layer two writers in ONE process actually contend
    /// on, and it is what serializes the read-modify-write this test is about. In-process `flock`
    /// contention stopped being reachable when layer 1 was put in front of it, not when this test
    /// was rewritten — nothing reachable was given up here. Exercising the `flock` needs a second
    /// process, which no unit test in this crate is.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn two_concurrent_overrides_on_one_settings_file_both_survive() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");

        let mut writers = Vec::new();
        for name in ["scout", "worker"] {
            let path = path.clone();
            writers.push(tokio::spawn(async move {
                let mut fields = Map::new();
                fields.insert("disabled".to_string(), Value::Bool(true));
                merge_builtin_agent_override(&path, name, &fields)
                    .await
                    .expect("each concurrent override must succeed");
            }));
        }
        for w in writers {
            w.await.expect("writer task must not panic");
        }

        let settings = read_settings_file_strict(&path).expect("settings parse");
        let overrides = overrides_of(&settings).expect("agentOverrides written");
        assert!(
            overrides.contains_key("scout") && overrides.contains_key("worker"),
            "both concurrent writes must persist — one losing the other is the lost-update bug; \
             got {overrides:?}"
        );
    }

    /// SUBA-029's format half: the lock/atomic change must not alter a single byte of the document
    /// pi writes (two-space indent + trailing newline, `agents.ts:706-709`), or a cyrup-written
    /// settings file stops being diff-clean against a pi-written one.
    #[tokio::test]
    async fn the_written_settings_document_is_still_pi_byte_shaped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("settings.json");
        let mut fields = Map::new();
        fields.insert("disabled".to_string(), Value::Bool(true));
        merge_builtin_agent_override(&path, "scout", &fields)
            .await
            .expect("write");

        let raw = std::fs::read_to_string(&path).expect("written");
        assert!(raw.ends_with("}\n"), "trailing newline required: {raw:?}");
        assert!(
            raw.contains("\n  \"subagents\": {"),
            "two-space indent required: {raw}"
        );
        // The parent directory is created on demand, as `mkdir -p` does.
        assert!(path.parent().is_some_and(std::path::Path::is_dir));
    }

    fn field(key: &str, value: Value) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert(key.to_string(), value);
        m
    }

    fn read(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[tokio::test]
    async fn merge_creates_every_missing_level_in_an_absent_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join("settings.json");
        merge_builtin_agent_override(&path, "scout", &field("disabled", Value::Bool(true)))
            .await
            .unwrap();
        assert_eq!(
            read(&path)["subagents"]["agentOverrides"]["scout"]["disabled"],
            Value::Bool(true)
        );
    }

    #[tokio::test]
    async fn merge_preserves_every_unrelated_key_in_the_document() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"theme":"dark","mcpServers":{"a":{"command":"x"}},"subagents":{"defaultModel":"anthropic/m","unknownFutureKey":42,"agentOverrides":{"worker":{"model":"anthropic/w"}}}}"#,
        )
        .unwrap();
        merge_builtin_agent_override(&path, "scout", &field("disabled", Value::Bool(true)))
            .await
            .unwrap();
        let after = read(&path);
        // The whole rest of the document survives — this is the failure mode a typed round-trip has.
        assert_eq!(after["theme"], Value::String("dark".to_string()));
        assert_eq!(
            after["mcpServers"]["a"]["command"],
            Value::String("x".to_string())
        );
        assert_eq!(
            after["subagents"]["defaultModel"],
            Value::String("anthropic/m".to_string())
        );
        assert_eq!(
            after["subagents"]["unknownFutureKey"],
            Value::Number(42.into())
        );
        assert_eq!(
            after["subagents"]["agentOverrides"]["worker"]["model"],
            Value::String("anthropic/w".to_string())
        );
        assert_eq!(
            after["subagents"]["agentOverrides"]["scout"]["disabled"],
            Value::Bool(true)
        );
    }

    #[tokio::test]
    async fn merge_is_shallow_and_keeps_the_entrys_other_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"subagents":{"agentOverrides":{"scout":{"model":"anthropic/x"}}}}"#,
        )
        .unwrap();
        merge_builtin_agent_override(&path, "scout", &field("disabled", Value::Bool(true)))
            .await
            .unwrap();
        let entry = &read(&path)["subagents"]["agentOverrides"]["scout"];
        assert_eq!(entry["model"], Value::String("anthropic/x".to_string()));
        assert_eq!(entry["disabled"], Value::Bool(true));
    }

    #[tokio::test]
    async fn remove_fields_keeps_siblings_and_prunes_only_what_it_emptied() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"subagents":{"agentOverrides":{"scout":{"disabled":true,"model":"anthropic/x"}}}}"#,
        )
        .unwrap();
        assert!(
            remove_builtin_agent_override_fields(&path, "scout", &["disabled"])
                .await
                .unwrap()
        );
        let entry = &read(&path)["subagents"]["agentOverrides"]["scout"];
        assert_eq!(entry["model"], Value::String("anthropic/x".to_string()));
        assert!(entry.get("disabled").is_none());
    }

    #[tokio::test]
    async fn remove_last_field_prunes_entry_overrides_and_subagents() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"theme":"dark","subagents":{"agentOverrides":{"scout":{"disabled":true}}}}"#,
        )
        .unwrap();
        assert!(
            remove_builtin_agent_override_fields(&path, "scout", &["disabled"])
                .await
                .unwrap()
        );
        let after = read(&path);
        assert_eq!(after["theme"], Value::String("dark".to_string()));
        assert!(
            after.get("subagents").is_none(),
            "empty subagents block must be pruned: {after}"
        );
    }

    #[tokio::test]
    async fn remove_entry_prunes_but_leaves_sibling_overrides() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"subagents":{"agentOverrides":{"scout":{"disabled":true},"worker":{"model":"m"}}}}"#,
        )
        .unwrap();
        assert!(remove_builtin_agent_override(&path, "scout").await.unwrap());
        let overrides = &read(&path)["subagents"]["agentOverrides"];
        assert!(overrides.get("scout").is_none());
        assert_eq!(overrides["worker"]["model"], Value::String("m".to_string()));
    }

    #[tokio::test]
    async fn removals_are_no_ops_when_nothing_matches_and_never_create_a_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        assert!(!remove_builtin_agent_override(&path, "scout").await.unwrap());
        assert!(
            !remove_builtin_agent_override_fields(&path, "scout", &["disabled"])
                .await
                .unwrap()
        );
        assert!(
            !path.exists(),
            "a no-op removal must not create a settings file"
        );

        std::fs::write(
            &path,
            r#"{"subagents":{"agentOverrides":{"scout":{"model":"m"}}}}"#,
        )
        .unwrap();
        assert!(
            !remove_builtin_agent_override_fields(&path, "scout", &["disabled"])
                .await
                .unwrap()
        );
        assert_eq!(
            read(&path)["subagents"]["agentOverrides"]["scout"]["model"],
            Value::String("m".to_string())
        );
    }

    #[tokio::test]
    async fn a_malformed_settings_file_aborts_rather_than_being_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, "{ not json").unwrap();
        let err =
            merge_builtin_agent_override(&path, "scout", &field("disabled", Value::Bool(true)))
                .await
                .expect_err("malformed settings must abort");
        assert!(matches!(err, SubagentError::MalformedSettings(_)), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    }

    /// SUBA-171 — a save through a symlinked `settings.json` (a dotfile manager's layout) updates
    /// the link's target and leaves the link in place. Red before: `write_atomic` renamed the temp
    /// over the link itself, so the link became a plain file and the target kept the old content.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_save_through_a_symlink_updates_the_target_and_keeps_the_link() {
        let tmp = tempfile::tempdir().unwrap();
        let dotfiles = tmp.path().join("dotfiles");
        std::fs::create_dir(&dotfiles).unwrap();
        let real = dotfiles.join("settings.json");
        std::fs::write(&real, r#"{"theme":"dark"}"#).unwrap();
        let link = tmp.path().join("settings.json");
        std::os::unix::fs::symlink("dotfiles/settings.json", &link).unwrap();

        merge_builtin_agent_override(&link, "scout", &field("disabled", Value::Bool(true)))
            .await
            .unwrap();

        let meta = std::fs::symlink_metadata(&link).unwrap();
        assert!(
            meta.file_type().is_symlink(),
            "the link must survive the save"
        );
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            Path::new("dotfiles/settings.json"),
            "the link text is untouched"
        );
        let after = read(&real);
        assert_eq!(after["theme"], Value::String("dark".to_string()));
        assert_eq!(
            after["subagents"]["agentOverrides"]["scout"]["disabled"],
            Value::Bool(true)
        );
    }

    /// SUBA-171 — a dangling link names the file to create (pi `resolveSettingsWriteTarget`'s
    /// readlink arm): the save creates the target and the link then resolves to it.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_save_through_a_dangling_symlink_creates_the_link_target() {
        let tmp = tempfile::tempdir().unwrap();
        let dotfiles = tmp.path().join("dotfiles");
        std::fs::create_dir(&dotfiles).unwrap();
        let link = tmp.path().join("settings.json");
        std::os::unix::fs::symlink(dotfiles.join("settings.json"), &link).unwrap();

        merge_builtin_agent_override(&link, "scout", &field("disabled", Value::Bool(true)))
            .await
            .unwrap();

        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            read(&dotfiles.join("settings.json"))["subagents"]["agentOverrides"]["scout"]["disabled"],
            Value::Bool(true)
        );
    }

    /// SUBA-171 — the existing mode survives the atomic rename exactly (`stat().mode & 0o7777`,
    /// pi `agents.ts:955,971` @ad11b7ab), for a restricted file and for a file whose mode the
    /// umask would never produce. Red before: the temp was created at the umask default, so a
    /// `0600` file came back `0644`.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_save_keeps_the_existing_file_mode_exactly() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        for mode in [0o600, 0o660, 0o640] {
            let path = tmp.path().join(format!("settings-{mode:o}.json"));
            std::fs::write(&path, "{}").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();

            merge_builtin_agent_override(&path, "scout", &field("disabled", Value::Bool(true)))
                .await
                .unwrap();
            assert!(remove_builtin_agent_override(&path, "scout").await.unwrap());

            let after = std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777;
            assert_eq!(after, mode, "mode {mode:o} must survive the save");
        }
    }

    /// SUBA-171 — the mode of a symlink's TARGET is the one kept (the link's own mode is
    /// meaningless on Linux).
    #[cfg(unix)]
    #[tokio::test]
    async fn a_save_through_a_symlink_keeps_the_targets_mode() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real.json");
        std::fs::write(&real, "{}").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = tmp.path().join("settings.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        merge_builtin_agent_override(&link, "scout", &field("disabled", Value::Bool(true)))
            .await
            .unwrap();

        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::metadata(&real).unwrap().permissions().mode() & 0o7777,
            0o600
        );
    }

    /// SUBA-171 — a read-only settings file is refused (pi `accessSync(W_OK)`), not replaced, and
    /// its content and mode are untouched. Red before: the rename replaced it regardless.
    ///
    /// `root` passes every `W_OK` check (pi's `accessSync` included), so when the probe below can
    /// open a `0444` file for writing the refusal is unobservable and the test stops there.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_read_only_settings_file_is_refused_and_left_untouched() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, r#"{"theme":"dark"}"#).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        if std::fs::OpenOptions::new().write(true).open(&path).is_ok() {
            eprintln!("skipping: this user bypasses file permissions (root)");
            return;
        }

        let err =
            merge_builtin_agent_override(&path, "scout", &field("disabled", Value::Bool(true)))
                .await
                .expect_err("a read-only settings file must be refused");
        assert!(
            err.to_string().contains("Failed to write settings file"),
            "{err}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"theme":"dark"}"#
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
            0o444
        );
    }

    /// SUBA-171 — the write-access probe as such: with the probe pointed at a target that cannot
    /// be opened for writing, `write_settings_file` refuses before any temp or rename. A directory
    /// stands in for the read-only file so this half is observable as `root` too (where the test
    /// above cannot refuse anything). Without the probe the call still fails — the rename cannot
    /// replace a non-empty directory — but only AFTER `write_atomic_with_mode` has written its
    /// `settings.json.tmp.<pid>` beside the target, which it does not clean up; so "no temp left in
    /// the parent" is what proves the refusal came first.
    #[tokio::test]
    async fn the_write_access_probe_refuses_before_any_rename() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("settings.json");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("keep"), "x").unwrap();
        let err = write_settings_file(&target, &target, &Map::new())
            .expect_err("an unwritable target must be refused");
        assert!(
            err.to_string().contains("Failed to write settings file"),
            "{err}"
        );
        assert!(target.join("keep").is_file(), "the target is untouched");
        let temps: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("settings.json.tmp."))
            .collect();
        assert!(
            temps.is_empty(),
            "no temp was written before the refusal: {temps:?}"
        );
    }

    /// pi `resolveSettingsWriteTarget`: a path ending in a separator cannot name a new file.
    #[test]
    fn a_missing_path_with_a_trailing_separator_is_not_a_write_target() {
        let tmp = tempfile::tempdir().unwrap();
        let mut path = tmp.path().join("missing").into_os_string();
        path.push("/");
        assert!(resolve_settings_write_target(Path::new(&path)).is_err());
    }

    #[tokio::test]
    async fn written_file_uses_two_space_indent_and_a_trailing_newline() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        merge_builtin_agent_override(&path, "scout", &field("disabled", Value::Bool(true)))
            .await
            .unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.ends_with("}\n"), "{raw:?}");
        assert!(raw.contains("\n  \"subagents\""), "{raw:?}");
    }
}
