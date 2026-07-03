//! One-time startup migrations (Pi `migrations.ts`). A faithful port of `runMigrations(cwd)`
//! (migrations.ts:296-315): migrate legacy `oauth.json`/`settings.json` API keys into `auth.json`,
//! move stray `*.jsonl` sessions out of the agent root into their per-cwd session dir, move managed
//! `fd`/`rg` binaries from `tools/` to `bin/`, migrate a `commands/` resource dir to `prompts/`, and
//! collect deprecation warnings for legacy `hooks/`/`tools/` dirs (surfaced in interactive mode by
//! [`format_deprecation_warnings`], Pi `showDeprecationWarnings`, migrations.ts:265-289).
//!
//! Each step is best-effort and never fatal — a malformed/locked file is skipped exactly as Pi's
//! `try { … } catch {}` arms do. The keybindings-config migration is intentionally NOT ported here:
//! cyrup's keybindings store (`cyrup-tui`) has no legacy on-disk shape to migrate from.

use std::path::Path;

use cyrup_config::ConfigDirs;
use serde_json::{Map, Value};

/// The result of a migration run (Pi `runMigrations` return), threaded into startup: provider names
/// whose credentials were migrated, plus the deprecation warnings to show in interactive mode.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MigrationResult {
    pub migrated_auth_providers: Vec<String>,
    pub deprecation_warnings: Vec<String>,
}

/// Run all startup migrations against the resolved dirs + cwd (Pi `runMigrations`, migrations.ts:296).
pub fn run_migrations(dirs: &ConfigDirs) -> MigrationResult {
    let migrated_auth_providers = migrate_auth_to_auth_json(&dirs.agent_dir);
    migrate_sessions_from_agent_root(&dirs.agent_dir);
    migrate_tools_to_bin(&dirs.agent_dir, &dirs.bin_dir());
    let deprecation_warnings = migrate_extension_system(&dirs.agent_dir, &dirs.cwd);
    MigrationResult {
        migrated_auth_providers,
        deprecation_warnings,
    }
}

/// Migrate legacy `oauth.json` + `settings.json.apiKeys` into `auth.json` (Pi
/// `migrateAuthToAuthJson`, migrations.ts:22-77). Skips entirely if `auth.json` already exists.
fn migrate_auth_to_auth_json(agent_dir: &Path) -> Vec<String> {
    let auth_path = agent_dir.join("auth.json");
    if auth_path.exists() {
        return Vec::new();
    }
    let oauth_path = agent_dir.join("oauth.json");
    let settings_path = agent_dir.join("settings.json");

    let mut migrated: Map<String, Value> = Map::new();
    let mut providers: Vec<String> = Vec::new();

    // oauth.json → { provider: { type: "oauth", ...cred } }
    if oauth_path.exists()
        && let Ok(text) = std::fs::read_to_string(&oauth_path)
        && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(&text)
    {
        for (provider, cred) in obj {
            let mut entry = Map::new();
            entry.insert("type".to_string(), Value::String("oauth".to_string()));
            if let Value::Object(cred_obj) = cred {
                for (k, v) in cred_obj {
                    entry.insert(k, v);
                }
            }
            migrated.insert(provider.clone(), Value::Object(entry));
            providers.push(provider);
        }
        let _ = std::fs::rename(&oauth_path, oauth_path.with_extension("json.migrated"));
    }

    // settings.json.apiKeys → { provider: { type: "api_key", key } }, then strip apiKeys.
    if settings_path.exists()
        && let Ok(text) = std::fs::read_to_string(&settings_path)
        && let Ok(mut settings) = serde_json::from_str::<Value>(&text)
        && let Some(Value::Object(api_keys)) =
            settings.as_object().and_then(|o| o.get("apiKeys")).cloned()
    {
        for (provider, key) in api_keys {
            if !migrated.contains_key(&provider)
                && let Value::String(key) = key
            {
                let mut entry = Map::new();
                entry.insert("type".to_string(), Value::String("api_key".to_string()));
                entry.insert("key".to_string(), Value::String(key));
                migrated.insert(provider.clone(), Value::Object(entry));
                providers.push(provider);
            }
        }
        if let Some(obj) = settings.as_object_mut() {
            obj.remove("apiKeys");
        }
        if let Ok(serialized) = serde_json::to_string_pretty(&settings) {
            let _ = std::fs::write(&settings_path, serialized);
        }
    }

    if !migrated.is_empty() {
        let _ = std::fs::create_dir_all(agent_dir);
        if let Ok(serialized) = serde_json::to_string_pretty(&Value::Object(migrated)) {
            let _ = std::fs::write(&auth_path, serialized);
        }
    }
    providers
}

/// Move stray `*.jsonl` session files from the agent root into their correct per-cwd session dir
/// (Pi `migrateSessionsFromAgentRoot`, migrations.ts:84-130). The target dir is derived from the
/// session header's `cwd` with the same `--<encoded>--` encoding as the session manager.
fn migrate_sessions_from_agent_root(agent_dir: &Path) {
    let entries = match std::fs::read_dir(agent_dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    let files: Vec<std::path::PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    for file in files {
        let content = match std::fs::read_to_string(&file) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let first_line = content.split('\n').next().unwrap_or("").trim();
        if first_line.is_empty() {
            continue;
        }
        let header: Value = match serde_json::from_str(first_line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if header.get("type").and_then(Value::as_str) != Some("session") {
            continue;
        }
        let cwd = match header.get("cwd").and_then(Value::as_str) {
            Some(c) if !c.is_empty() => c,
            _ => continue,
        };
        let safe = encode_cwd(cwd);
        let correct_dir = agent_dir.join("sessions").join(&safe);
        if !correct_dir.exists() && std::fs::create_dir_all(&correct_dir).is_err() {
            continue;
        }
        if let Some(name) = file.file_name() {
            let new_path = correct_dir.join(name);
            if new_path.exists() {
                continue;
            }
            let _ = std::fs::rename(&file, &new_path);
        }
    }
}

/// Encode a cwd into the session-dir folder name `--<cwd>--` (Pi: strip a leading separator, replace
/// `/`/`\`/`:` with `-`, wrap in `--…--`).
fn encode_cwd(cwd: &str) -> String {
    let trimmed = cwd.trim_start_matches(['/', '\\']);
    let replaced: String = trimmed
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') {
                '-'
            } else {
                c
            }
        })
        .collect();
    format!("--{replaced}--")
}

/// Move managed `fd`/`rg` binaries from `tools/` to `bin/` (Pi `migrateToolsToBin`,
/// migrations.ts:175-220).
fn migrate_tools_to_bin(agent_dir: &Path, bin_dir: &Path) {
    let tools_dir = agent_dir.join("tools");
    if !tools_dir.exists() {
        return;
    }
    for bin in ["fd", "rg", "fd.exe", "rg.exe"] {
        let old_path = tools_dir.join(bin);
        let new_path = bin_dir.join(bin);
        if old_path.exists() {
            if !bin_dir.exists() {
                let _ = std::fs::create_dir_all(bin_dir);
            }
            if new_path.exists() {
                let _ = std::fs::remove_file(&old_path);
            } else {
                let _ = std::fs::rename(&old_path, &new_path);
            }
        }
    }
}

/// Migrate a `commands/` resource dir to `prompts/` for global + project bases, then collect
/// deprecation warnings for legacy `hooks/`/`tools/` dirs (Pi `migrateExtensionSystem`,
/// migrations.ts:228-246).
fn migrate_extension_system(agent_dir: &Path, cwd: &Path) -> Vec<String> {
    let project_dir = cwd.join(".cyrup");
    migrate_commands_to_prompts(agent_dir, "Global");
    migrate_commands_to_prompts(&project_dir, "Project");
    let mut warnings = check_deprecated_extension_dirs(agent_dir, "Global");
    warnings.extend(check_deprecated_extension_dirs(&project_dir, "Project"));
    warnings
}

/// Rename `commands/` → `prompts/` when the former exists and the latter does not (Pi
/// `migrateCommandsToPrompts`, migrations.ts:135-158).
fn migrate_commands_to_prompts(base_dir: &Path, label: &str) -> bool {
    let commands_dir = base_dir.join("commands");
    let prompts_dir = base_dir.join("prompts");
    if commands_dir.exists() && !prompts_dir.exists() {
        match std::fs::rename(&commands_dir, &prompts_dir) {
            Ok(()) => {
                // Routed through the stdout guard (Pi `console.log` under `takeOverStdout`): during a
                // non-interactive PRINT/JSON/RPC run this notice is rerouted to stderr so it can never
                // corrupt the machine-readable stream on stdout (Pi migrations.ts:144, main.ts:537).
                crate::output_guard::emit_stray_line(&format!("Migrated {label} commands/ → prompts/"));
                return true;
            }
            Err(err) => {
                crate::output_guard::emit_stray_line(&format!(
                    "Warning: Could not migrate {label} commands/ to prompts/: {err}"
                ));
            }
        }
    }
    false
}

/// Warn about deprecated `hooks/`/`tools/` dirs (Pi `checkDeprecatedExtensionDirs`,
/// migrations.ts:251-289). A `tools/` dir warns only if it contains non-`fd`/`rg` entries.
fn check_deprecated_extension_dirs(base_dir: &Path, label: &str) -> Vec<String> {
    let mut warnings = Vec::new();
    if base_dir.join("hooks").exists() {
        warnings.push(format!(
            "{label} hooks/ directory found. Hooks have been renamed to extensions."
        ));
    }
    let tools_dir = base_dir.join("tools");
    if tools_dir.exists()
        && let Ok(entries) = std::fs::read_dir(&tools_dir)
    {
        let has_custom = entries.filter_map(|e| e.ok()).any(|e| {
            let name = e.file_name().to_string_lossy().to_ascii_lowercase();
            !matches!(name.as_str(), "fd" | "rg" | "fd.exe" | "rg.exe") && !name.starts_with('.')
        });
        if has_custom {
            warnings.push(format!(
                "{label} tools/ directory contains custom tools. Custom tools have been merged into extensions."
            ));
        }
    }
    warnings
}

/// Format the deprecation warnings for interactive display (Pi `showDeprecationWarnings`,
/// migrations.ts:265-289) — minus the keypress wait (handled by the interactive front-end). Returns
/// an empty string when there are no warnings.
pub fn format_deprecation_warnings(warnings: &[String]) -> String {
    if warnings.is_empty() {
        return String::new();
    }
    const MIGRATION_GUIDE_URL: &str = "https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/CHANGELOG.md#extensions-migration";
    const EXTENSIONS_DOC_URL: &str = "https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/docs/extensions.md";
    let mut out = String::new();
    for warning in warnings {
        out.push_str(&format!("Warning: {warning}\n"));
    }
    out.push_str("\nMove your extensions to the extensions/ directory.\n");
    out.push_str(&format!("Migration guide: {MIGRATION_GUIDE_URL}\n"));
    out.push_str(&format!("Documentation: {EXTENSIONS_DOC_URL}\n"));
    out
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn dirs(root: &Path) -> ConfigDirs {
        ConfigDirs {
            agent_dir: root.join("agent"),
            session_dir: root.join("agent/sessions"),
            session_dir_explicit: false,
            package_dir: root.join("agent/packages"),
            cwd: root.join("work"),
            home: root.to_path_buf(),
        }
    }

    #[test]
    fn encodes_cwd_like_session_manager() {
        assert_eq!(encode_cwd("/Users/x/proj"), "--Users-x-proj--");
        // Each of `:` and `\` is replaced individually (Pi `/[/\\:]/g`), yielding `--C--a-b--`.
        assert_eq!(encode_cwd("C:\\a\\b"), "--C--a-b--");
    }

    #[test]
    fn migrates_oauth_and_settings_api_keys_into_auth_json() {
        let tmp = tempfile::tempdir().unwrap();
        let d = dirs(tmp.path());
        std::fs::create_dir_all(&d.agent_dir).unwrap();
        std::fs::write(
            d.agent_dir.join("oauth.json"),
            r#"{"anthropic":{"access":"tok","refresh":"r"}}"#,
        )
        .unwrap();
        std::fs::write(
            d.agent_dir.join("settings.json"),
            r#"{"theme":"dark","apiKeys":{"openai":"sk-123"}}"#,
        )
        .unwrap();

        let result = run_migrations(&d);
        assert!(
            result
                .migrated_auth_providers
                .contains(&"anthropic".to_string())
        );
        assert!(
            result
                .migrated_auth_providers
                .contains(&"openai".to_string())
        );

        let auth: Value =
            serde_json::from_str(&std::fs::read_to_string(d.agent_dir.join("auth.json")).unwrap())
                .unwrap();
        assert_eq!(auth["anthropic"]["type"], "oauth");
        assert_eq!(auth["anthropic"]["access"], "tok");
        assert_eq!(auth["openai"]["type"], "api_key");
        assert_eq!(auth["openai"]["key"], "sk-123");

        // apiKeys stripped from settings.json; oauth.json renamed away.
        let settings: Value = serde_json::from_str(
            &std::fs::read_to_string(d.agent_dir.join("settings.json")).unwrap(),
        )
        .unwrap();
        assert!(settings.get("apiKeys").is_none());
        assert_eq!(settings["theme"], "dark");
        assert!(!d.agent_dir.join("oauth.json").exists());

        // Idempotent: a second run does nothing (auth.json already present).
        let again = run_migrations(&d);
        assert!(again.migrated_auth_providers.is_empty());
    }

    #[test]
    fn migrates_stray_session_into_per_cwd_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let d = dirs(tmp.path());
        std::fs::create_dir_all(&d.agent_dir).unwrap();
        let stray = d.agent_dir.join("abc123.jsonl");
        std::fs::write(
            &stray,
            "{\"type\":\"session\",\"cwd\":\"/home/u/p\"}\n{\"type\":\"message\"}\n",
        )
        .unwrap();
        run_migrations(&d);
        assert!(!stray.exists());
        let moved = d
            .agent_dir
            .join("sessions")
            .join("--home-u-p--")
            .join("abc123.jsonl");
        assert!(moved.exists());
    }

    #[test]
    fn migrates_commands_dir_and_warns_on_legacy_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let d = dirs(tmp.path());
        std::fs::create_dir_all(d.agent_dir.join("commands")).unwrap();
        std::fs::create_dir_all(d.agent_dir.join("hooks")).unwrap();
        let result = run_migrations(&d);
        assert!(d.agent_dir.join("prompts").exists());
        assert!(!d.agent_dir.join("commands").exists());
        assert!(
            result
                .deprecation_warnings
                .iter()
                .any(|w| w.contains("hooks/"))
        );
        let formatted = format_deprecation_warnings(&result.deprecation_warnings);
        assert!(formatted.contains("Warning:"));
        assert!(formatted.contains("extensions/ directory"));
    }
}
