//! One-time startup migrations (Pi `migrations.ts`). A faithful port of `runMigrations(cwd)`
//! (migrations.ts:296-315): migrate legacy `oauth.json`/`settings.json` API keys into `auth.json`,
//! move stray `*.jsonl` sessions out of the agent root into their per-cwd session dir, move managed
//! `fd`/`rg` binaries from `tools/` to `bin/`, rename legacy keybinding ids in
//! `<agent_dir>/keybindings.json`, migrate a `commands/` resource dir to `prompts/`, and
//! collect deprecation warnings for legacy `hooks/`/`tools/` dirs (surfaced in interactive mode by
//! [`show_deprecation_warnings`], Pi `showDeprecationWarnings`, migrations.ts:277-298 @v0.83.0 —
//! which BLOCKS startup on a keypress, so the notice cannot be painted over by the first TUI frame).
//!
//! Each step is best-effort and never fatal — a malformed/locked file is skipped exactly as Pi's
//! `try { … } catch {}` arms do. The keybindings step is Pi's `migrateKeybindingsConfigFile()`
//! (migrations.ts:157-174, called at `:312`); its mechanism lives in
//! [`cyrup_config::migrate_keybindings_config_file`] rather than in this file because Pi applies the
//! same rename table a SECOND time on every read (`core/keybindings.ts:366`), and cyrup's read-time
//! consumer (`cyrup-tui`'s `keymap.rs`) shares no ancestor with this binary other than
//! `cyrup-config` — the same argument that puts `migrate_settings` there.

use std::path::Path;

use cyrup_config::ConfigDirs;
use cyrup_session_svc::encode_cwd;
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
    // Pi discards the return too (`migrateToolsToBin();`, migrations.ts:311) — the notice is the
    // whole observable effect, and it is emitted inside the callee.
    let _moved_binaries = migrate_tools_to_bin(&dirs.agent_dir, &dirs.bin_dir());
    // migrations.ts:312 — between `migrateToolsToBin()` (`:311`) and `migrateExtensionSystem()`
    // (`:313`). Every failure mode is swallowed inside the callee, as Pi's `try { … } catch {}` does.
    cyrup_config::migrate_keybindings_config_file(&dirs.agent_dir);
    // CFG-054 — a cyrup-only migration (pi has no two-level package root to unwind): move Global
    // package working trees out of the doubled `<package_dir>/packages/` segment. Runs here so a
    // plain `cyrup` start repairs the layout, and again inside the package subcommands
    // (`subcommands::run`), which are dispatched BEFORE this function (main.rs:145-154).
    migrate_packages_root(&dirs.package_dir);
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

    if !migrated.is_empty()
        && let Ok(serialized) = serde_json::to_string_pretty(&Value::Object(migrated))
    {
        // `auth.json` carries OAuth access/refresh tokens and plaintext API keys, so it MUST be
        // owner-only — Pi writes it with `{ mode: 0o600 }` (migrations.ts:69) and creates its dir
        // 0700 (auth-storage.ts:55). A plain `fs::write` uses the ambient umask, which leaves the
        // credentials group- or world-readable on a permissive umask. `write_atomic(secret=true)` is
        // the same writer [`cyrup_config::AuthStore::save`] uses, so the migrated file lands with
        // exactly the mode the store would have given it. This branch only ever CREATES the file —
        // the fn returned early above when `auth.json` already existed — so no pre-existing file's
        // permissions are read, relaxed, or otherwise touched.
        let _ = cyrup_config::lock::write_atomic(&auth_path, serialized.as_bytes(), true);
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
        // SESS-044: the ONE encoder — `cyrup_session::layout::encode_cwd`, re-exported by
        // `cyrup-session-svc`. This function used to carry a private duplicate whose
        // `trim_start_matches` stripped ALL leading separators, while pi's first `replace` is
        // anchored and NOT global (`migrations.ts:112` @v0.83.0, byte-identical to
        // `session-manager.ts:479`). Once the canonical copy was corrected the two DISAGREED, so a
        // migration and the live layout could resolve a multi-separator cwd to different directory
        // names — this migration would then move sessions into a folder the session manager never
        // looks in.
        let safe = encode_cwd(std::path::Path::new(cwd));
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

/// Move managed `fd`/`rg` binaries from `tools/` to `bin/` (Pi `migrateToolsToBin`,
/// migrations.ts:177-216 @v0.83.0).
///
/// Returns Pi's `movedAny` (`:185`, set at `:198`) so the caller — and a test — can see whether the
/// filesystem changed. On `true` the notice at `:213-215` is emitted, routed through the stdout
/// guard exactly like the sibling `commands/ → prompts/` line: `console.log` under Pi's
/// `takeOverStdout` reaches stderr during a PRINT/JSON/RPC run, so it cannot corrupt the
/// machine-readable stream. **A "target already exists" delete is NOT a move** (`:203-208` sets the
/// flag only on `renameSync`), so a second run after a successful migration says nothing (CFG-050).
fn migrate_tools_to_bin(agent_dir: &Path, bin_dir: &Path) -> bool {
    let tools_dir = agent_dir.join("tools");
    if !tools_dir.exists() {
        return false;
    }
    let mut moved_any = false;
    for bin in ["fd", "rg", "fd.exe", "rg.exe"] {
        let old_path = tools_dir.join(bin);
        let new_path = bin_dir.join(bin);
        if old_path.exists() {
            if !bin_dir.exists() {
                let _ = std::fs::create_dir_all(bin_dir);
            }
            if new_path.exists() {
                let _ = std::fs::remove_file(&old_path);
            } else if std::fs::rename(&old_path, &new_path).is_ok() {
                moved_any = true;
            }
        }
    }
    if moved_any {
        crate::output_guard::emit_stray_line("Migrated managed binaries tools/ → bin/");
    }
    moved_any
}

/// CFG-054 — unwind the doubled `<package_dir>/packages/<id>` working-tree layout a pre-fix build
/// wrote, announcing it exactly once (the same `movedAny`-gated notice shape as
/// [`migrate_tools_to_bin`], Pi migrations.ts:213-215): after the fix the tree resolves at
/// `<package_dir>/<id>`, so without the move every installed git package's resources would silently
/// stop loading while its registry row — whose path never doubled — stayed put.
///
/// Idempotent and best-effort; `0` for every fresh install, which is one `read_dir` on a missing
/// path.
pub(crate) fn migrate_packages_root(package_dir: &Path) -> usize {
    let moved = cyrup_resources::migrate_legacy_doubled_packages_root(package_dir);
    if moved > 0 {
        crate::output_guard::emit_stray_line(&format!(
            "Migrated {moved} installed package(s) out of {}",
            package_dir.join("packages").display()
        ));
    }
    moved
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
                crate::output_guard::emit_stray_line(&format!(
                    "Migrated {label} commands/ → prompts/"
                ));
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

/// Pi `showDeprecationWarnings(warnings)` (migrations.ts:277-298 @v0.83.0) in full: print the
/// warning block, print `Press any key to continue...`, then **block startup** until a key arrives,
/// and finish with a blank line (`console.log()`, `:297`).
///
/// The gate is the point of the function. These warnings are the only signal that a legacy `hooks/`
/// or custom `tools/` directory has stopped being loaded — i.e. that every extension in it is now
/// silently doing nothing — and pi deliberately refuses to proceed until the user has seen them
/// (awaited from `main.ts:838-840`, BEFORE the interactive UI takes the terminal). cyrup printed the
/// same text microseconds before the first TUI frame painted over it (CFG-049).
///
/// Returns immediately when there is nothing to show, exactly like Pi's `if (warnings.length === 0)
/// return;` (`:278`) — so the common startup pays nothing.
///
/// **[CYRUP-DELTA]** on EOF: `process.stdin.once("data")` never fires when stdin is already closed,
/// so upstream hangs forever on a piped/closed stdin. A zero-length read returns here instead. The
/// same input reaches this function only in interactive mode, where a closed stdin means there is no
/// one to press a key.
pub fn show_deprecation_warnings(warnings: &[String]) {
    let text = deprecation_gate_block(warnings);
    if text.is_empty() {
        return;
    }
    // cyrup writes the block to stderr rather than Pi's `console.log`, which is the pre-existing
    // choice at this call site and the safe one: stdout may be a protocol stream.
    eprint!("{text}");
    wait_for_any_key();
    // `console.log()` (`:297`) — one blank line once the key has been pressed.
    eprintln!();
}

/// Everything [`show_deprecation_warnings`] writes BEFORE it blocks: the warning block
/// (migrations.ts:280-285) plus the prompt line (`:286`). Split out so a test can pin the strings —
/// the block itself cannot be asserted through the wait, and a test must never drive
/// [`wait_for_any_key`], which would hang the suite on a terminal stdin.
fn deprecation_gate_block(warnings: &[String]) -> String {
    let text = format_deprecation_warnings(warnings);
    if text.is_empty() {
        return text;
    }
    // `console.log(chalk.dim("\nPress any key to continue..."))` (`:286`) — a leading blank line,
    // then the prompt, then `console.log`'s own newline.
    format!("{text}\nPress any key to continue...\n")
}

/// Pi's `await new Promise(resolve => { process.stdin.setRawMode?.(true); process.stdin.resume();
/// process.stdin.once("data", () => { process.stdin.setRawMode?.(false); process.stdin.pause();
/// resolve(); }); })` (migrations.ts:288-296 @v0.83.0).
///
/// Raw mode is what makes it "any key" rather than "Enter"; `setRawMode?.` is an OPTIONAL call
/// upstream — undefined on a non-TTY — so a failure to enter raw mode is ignored here too, and the
/// read simply becomes line-buffered on that stdin. Raw mode is always restored, including on a read
/// error, because leaving it on would hand the user a terminal that echoes nothing.
fn wait_for_any_key() {
    use std::io::Read as _;
    let raw = cyrup_tui::crossterm::terminal::enable_raw_mode().is_ok();
    let mut byte = [0u8; 1];
    let _ = std::io::stdin().read(&mut byte);
    if raw {
        let _ = cyrup_tui::crossterm::terminal::disable_raw_mode();
    }
}

/// Format the deprecation warnings for interactive display (Pi `showDeprecationWarnings`,
/// migrations.ts:277-286 @v0.83.0) — the TEXT half only; [`show_deprecation_warnings`] is the whole
/// function, including the keypress gate at `:286-297`. Returns an empty string when there are no
/// warnings.
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

    /// SESS-044 — this migration and the live session layout must resolve the SAME cwd to the SAME
    /// directory, so it now calls the one shared encoder rather than a private copy.
    ///
    /// pi `migrations.ts:112` @v0.83.0 is byte-identical to `session-manager.ts:479`:
    /// ``--${cwd.replace(/^[/\\]/, "").replace(/[/\\:]/g, "-")}--``. The FIRST replace is anchored
    /// and carries no `g`, so exactly ONE leading separator goes.
    ///
    /// RED before this pass: the deleted duplicate used `trim_start_matches(['/', '\\'])`, which
    /// strips ALL leading separators — so `//net/x` encoded to `--net-x--` here while
    /// `cyrup_session::encode_cwd` (corrected earlier this cycle) produced `---net-x--`, and a
    /// migrated session landed in a folder the session manager does not list.
    #[test]
    fn encodes_cwd_like_session_manager() {
        let enc = |s: &str| encode_cwd(std::path::Path::new(s));
        assert_eq!(enc("/Users/x/proj"), "--Users-x-proj--");
        // Each of `:` and `\` is replaced individually (Pi `/[/\\:]/g`), yielding `--C--a-b--`.
        assert_eq!(enc("C:\\a\\b"), "--C--a-b--");
        // Exactly ONE leading separator is stripped; the second becomes a `-`.
        assert_eq!(enc("//net/x"), "---net-x--");
        assert_eq!(enc("\\\\srv\\share"), "---srv-share--");
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

    /// The migrated `auth.json` holds OAuth tokens and plaintext API keys, so it must land at 0600
    /// regardless of the ambient umask (Pi `writeFileSync(authPath, …, { mode: 0o600 })`,
    /// migrations.ts:69). Regression guard for CFG-032, which wrote it with the default umask.
    #[cfg(unix)]
    #[test]
    fn migrated_auth_json_is_written_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let d = dirs(tmp.path());
        std::fs::create_dir_all(&d.agent_dir).unwrap();
        std::fs::write(
            d.agent_dir.join("oauth.json"),
            r#"{"anthropic":{"access":"secret-access","refresh":"secret-refresh"}}"#,
        )
        .unwrap();

        run_migrations(&d);

        let auth_path = d.agent_dir.join("auth.json");
        let mode = std::fs::metadata(&auth_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "auth.json holds credentials and must be owner-read/write only, got {mode:o}"
        );
        // And the credential really is in there, so the assertion is about a live secret.
        assert!(
            std::fs::read_to_string(&auth_path)
                .unwrap()
                .contains("secret-refresh")
        );
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

    /// CFG-049 — Pi's `showDeprecationWarnings` does not just print: it prints `Press any key to
    /// continue...` (migrations.ts:286) and then blocks startup until a key arrives (`:288-296`),
    /// awaited from `main.ts:838-840` BEFORE the interactive UI takes the terminal.
    ///
    /// RED before this pass: `grep -rn 'Press any key' crates` returned ZERO hits workspace-wide —
    /// cyrup's `main.rs` printed the warning text and fell straight into TUI init, so the first
    /// frame painted over the only notice that a legacy `hooks/` dir has stopped loading.
    ///
    /// This pins the text and the early return. The BLOCK itself is deliberately not driven from a
    /// test: `wait_for_any_key` reads stdin, and a test harness whose stdin is a terminal would hang
    /// the suite. It needs the live-terminal run the item's Verify calls for.
    #[test]
    fn the_deprecation_gate_prints_pis_prompt_and_is_free_when_there_is_nothing_to_show() {
        assert_eq!(
            deprecation_gate_block(&[]),
            "",
            "no warnings must not reach the keypress gate (Pi `:278`)"
        );
        // The early return is what makes `show_deprecation_warnings` safe to call unconditionally.
        show_deprecation_warnings(&[]);

        let block = deprecation_gate_block(&["Global hooks/ directory found.".to_string()]);
        assert!(block.contains("Warning: Global hooks/ directory found."));
        assert!(block.contains("Move your extensions to the extensions/ directory."));
        assert!(
            block.ends_with("\nPress any key to continue...\n"),
            "the prompt is the LAST thing written before the block, got {block:?}"
        );
    }

    /// CFG-050 — Pi's `migrateToolsToBin` tracks `movedAny` (migrations.ts:185, set at `:198`) and
    /// announces the move at `:213-215`. cyrup moved the binaries silently, so a user pointing a
    /// script or a `PATH` entry at `~/.cyrup/agent/tools/rg` found it gone with nothing said.
    ///
    /// RED before this pass: `migrate_tools_to_bin` returned `()` and emitted nothing at all.
    /// The three assertions are Pi's three states: a real move (notice), a re-run with nothing left
    /// to move (silence), and a collision-only pass — `rmSync` on the stale source, which Pi does
    /// NOT count as a move (`:203-208`), so it must stay silent too.
    #[test]
    fn announces_a_managed_binary_move_exactly_once() {
        let tmp = tempfile::tempdir().unwrap();
        let d = dirs(tmp.path());
        let tools = d.agent_dir.join("tools");
        std::fs::create_dir_all(&tools).unwrap();
        std::fs::write(tools.join("rg"), b"#!/bin/sh\n").unwrap();

        assert!(
            migrate_tools_to_bin(&d.agent_dir, &d.bin_dir()),
            "a successful rename is Pi's `movedAny = true`"
        );
        assert!(d.bin_dir().join("rg").exists());
        assert!(!tools.join("rg").exists());

        assert!(
            !migrate_tools_to_bin(&d.agent_dir, &d.bin_dir()),
            "nothing left in tools/ — Pi says nothing on the second run"
        );

        // Both copies present: Pi deletes the stale source WITHOUT setting `movedAny`.
        std::fs::write(tools.join("rg"), b"#!/bin/sh\n").unwrap();
        assert!(
            !migrate_tools_to_bin(&d.agent_dir, &d.bin_dir()),
            "a collision delete is not a move"
        );
        assert!(
            !tools.join("rg").exists(),
            "the stale source is still removed"
        );
        assert!(d.bin_dir().join("rg").exists());
    }

    /// CFG-054 — `run_migrations` must unwind the doubled package root, or upgrading silently
    /// unloads every installed git package: `PackageStore::package_dir` now resolves
    /// `<package_dir>/<id>` while the tree an older build cloned still sits at
    /// `<package_dir>/packages/<id>`, and the registry row that names it — whose own path never
    /// doubled — keeps claiming the package is installed.
    #[test]
    fn migrates_installed_packages_out_of_the_doubled_root() {
        let tmp = tempfile::tempdir().unwrap();
        let d = dirs(tmp.path());
        let legacy = d
            .package_dir
            .join("packages")
            .join("git-github.com-acme-pack");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("SKILL.md"), "x").unwrap();

        run_migrations(&d);

        let moved = d.package_dir.join("git-github.com-acme-pack");
        assert!(
            moved.join("SKILL.md").exists(),
            "the tree must move up one level"
        );
        assert!(!d.package_dir.join("packages").exists());
        // Idempotent: the second run has nothing to move, so it says nothing.
        assert_eq!(migrate_packages_root(&d.package_dir), 0);
    }

    /// CFG-048 — Pi's FIFTH `runMigrations` call (`migrations.ts:312`, between `migrateToolsToBin()`
    /// `:311` and `migrateExtensionSystem()` `:313`) rewrites legacy keybinding ids in
    /// `<agent_dir>/keybindings.json`. RED before this pass: `run_migrations` made four calls and a
    /// legacy file was left untouched forever, so every legacy binding was silently inert.
    ///
    /// The table/format behaviour is covered exhaustively in `cyrup-config`'s `keybindings` module;
    /// what this test pins is that `run_migrations` REACHES it, and at Pi's position.
    #[test]
    fn migrates_legacy_keybinding_ids_in_the_agent_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let d = dirs(tmp.path());
        std::fs::create_dir_all(&d.agent_dir).unwrap();
        let path = d.agent_dir.join("keybindings.json");
        std::fs::write(
            &path,
            r#"{"cursorUp":"ctrl+p","interrupt":"ctrl+q","app.clear":"ctrl+k"}"#,
        )
        .unwrap();

        run_migrations(&d);

        // `${JSON.stringify(config, null, 2)}\n` in Pi's `KEYBINDINGS` declaration order.
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\n  \"tui.editor.cursorUp\": \"ctrl+p\",\n  \"app.interrupt\": \"ctrl+q\",\n  \"app.clear\": \"ctrl+k\"\n}\n"
        );

        // migrations.ts:168 — a clean file is not rewritten, so a second run is a no-op.
        let before = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        run_migrations(&d);
        assert_eq!(
            std::fs::metadata(&path).and_then(|m| m.modified()).ok(),
            before
        );
    }
}
