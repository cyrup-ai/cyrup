//! The v0.8.0 `enabled` master switch (pi `extension-config.ts:11-12,88` → `index.ts:1473-1477`):
//! `enabled: false` skips every registration, and the probe and the switch must read the SAME
//! resolved config.

use std::path::Path;

use crate::ext_config::ExtensionConfig;

use super::support::*;
use crate::extension::paths::{CONFIG_DIR, CONFIG_FILE, POLICY_FILE};
use crate::extension::{is_installed, permission_extension_for_env};

/// pi v0.8.0 added an `enabled` master switch: "When false, the extension skips all
/// registrations and startup work" (`extension-config.ts:11-12`), enforced by a bare early
/// return out of the extension entry point before any registration happens
/// (`index.ts:1473-1477`). cyrup's analog is [`permission_extension_for_env`] returning `None`,
/// so the binary attaches no `NativeExtension` at all.
///
/// The switch must beat a REAL install signal, which is the whole point of a master switch —
/// so this test arms the gate with a policy file first (the strongest signal, untouched by the
/// PERM-002 pristine logic) and then turns it off with the config key alone.
#[test]
fn enabled_false_attaches_nothing_even_with_a_policy_file_present() {
    without_install_env(|| {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join("agent");
        let cwd = dir.path().join("project");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        let config_path = agent_dir.join(CONFIG_DIR).join(CONFIG_FILE);

        // An unambiguous, operator-authored install signal.
        write_file(&agent_dir.join(POLICY_FILE), r#"{ "bash": { "*": "deny" } }"#);
        assert!(
            permission_extension_for_env(agent_dir.clone(), cwd.clone()).is_some(),
            "precondition: a policy file installs the gate"
        );

        // The master switch off.
        write_file(&config_path, "{\n  \"enabled\": false\n}\n");
        assert!(
            is_installed(&agent_dir, &cwd),
            "`enabled` is NOT the install probe — an edited config still reads as installed; \
             the switch has to be enforced downstream of it"
        );
        assert!(
            permission_extension_for_env(agent_dir.clone(), cwd.clone()).is_none(),
            "`\"enabled\": false` must attach no extension at all (pi `index.ts:1473-1477`)"
        );

        // MIRROR (must stay green): the switch is not over-broad. Only the literal `false`
        // disables (pi `record.enabled !== false`, `extension-config.ts:88`) — an explicit
        // `true`, a non-boolean, and a file with no `enabled` key at all (i.e. every config
        // written before v0.8.0) all keep the gate attached.
        for still_enabled in
            ["{\n  \"enabled\": true\n}\n", "{\n  \"enabled\": 0\n}\n", "{\n  \"yoloMode\": true\n}\n"]
        {
            write_file(&config_path, still_enabled);
            assert!(
                permission_extension_for_env(agent_dir.clone(), cwd.clone()).is_some(),
                "config {still_enabled:?} must NOT disable the gate"
            );
        }
    });
}

/// The upgrade hazard that comes with adding a fourth key to the auto-materialized template.
///
/// [`ExtensionConfig::is_pristine_default_file`] is a BYTE-EXACT compare and it is the third
/// install signal in [`is_installed`] (see that function's doc / PERM-002). Every cyrup build
/// before `enabled` existed wrote a three-key `config.json`, and those files are sitting on
/// disk. If the probe only ever accepted the CURRENT template, every one of them would stop
/// reading as pristine the moment this key landed — silently re-arming the permission gate, on
/// upgrade, for exactly the population PERM-002 was fixed for: people who opted in once and
/// then opted back out.
///
/// So the probe accepts a SET of exact templates, and this test pins the legacy member of it.
#[test]
fn a_legacy_three_key_config_template_still_reads_as_pristine() {
    without_install_env(|| {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join("agent");
        let cwd = dir.path().join("project");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        let config_path = agent_dir.join(CONFIG_DIR).join(CONFIG_FILE);

        // Byte-for-byte what an older cyrup build left behind. Written as a literal, not via
        // the constant, so this test still fails if the constant itself is edited.
        write_file(
            &config_path,
            "{\n  \"debug\": false,\n  \"yoloMode\": false,\n  \"forwardedPromptTimeoutSeconds\": 30\n}\n",
        );
        assert!(
            ExtensionConfig::is_pristine_default_file(&config_path),
            "a config.json written by a pre-`enabled` cyrup build is still the crate's own \
             footprint, not an operator decision"
        );
        assert!(
            !is_installed(&agent_dir, &cwd),
            "upgrading must not re-arm the gate for a user whose only leftover is the old \
             auto-written template"
        );
        assert!(permission_extension_for_env(agent_dir.clone(), cwd.clone()).is_none());

        // MIRROR (must stay green): the CURRENT template reads as pristine too — accepting the
        // legacy bytes is additive, it does not replace the live compare.
        write_file(&config_path, &ExtensionConfig::default_config_content());
        assert!(ExtensionConfig::is_pristine_default_file(&config_path));
        assert!(!is_installed(&agent_dir, &cwd));

        // MIRROR (must stay green): the probe did NOT get looser. A file an operator actually
        // touched still reads as configured and still installs — including one that differs
        // from the legacy template by a single character, and one that is a strict subset of
        // the known keys (the semantic "does it normalize to the default" check that was
        // rejected would have wrongly accepted this second one and disabled a real gate).
        for edited in [
            "{\n  \"debug\": true,\n  \"yoloMode\": false,\n  \"forwardedPromptTimeoutSeconds\": 30\n}\n",
            "{\n  \"yoloMode\": false\n}\n",
        ] {
            write_file(&config_path, edited);
            assert!(
                !ExtensionConfig::is_pristine_default_file(&config_path),
                "hand-edited config {edited:?} must not read as pristine"
            );
            assert!(is_installed(&agent_dir, &cwd), "...and must still install the gate");
        }
    });
}

/// Point `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH` at `path` for the duration of `body`, restoring
/// the ambient value after.
///
/// MUST be called from inside [`without_install_env`], which already holds
/// [`crate::ext_config::env_lock`]; this helper deliberately does NOT take that lock itself,
/// because `std::sync::Mutex` is not reentrant and re-taking it here would deadlock.
fn with_config_path_override<T>(path: &Path, body: impl FnOnce() -> T) -> T {
    let key = crate::ext_config::CONFIG_PATH_ENV_KEY;
    let previous = std::env::var(key).ok();
    // SAFETY: serialized by `env_lock` (held by the enclosing `without_install_env`), and
    // restored below.
    unsafe { std::env::set_var(key, path) };
    let out = body();
    // SAFETY: same scope/serialization; restores whatever the ambient shell had.
    unsafe {
        match previous {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }
    out
}

/// G130(b). The install probe and the `enabled` master switch must read the SAME file.
///
/// `is_installed`'s pristine-template probe read the RAW `config_path_for(agent_dir)` with no
/// env consultation, while the `enabled` check goes through `ExtensionConfig::load` →
/// `resolve_config_path`, which honours `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH`. With the
/// override set, the two gates inspected DIFFERENT files, so "is this installed?" and "is it
/// switched on?" were answered about two different operator intentions. Upstream has one
/// accessor, `getPermissionSystemConfigPath()` (`extension-config.ts:51-53`), and every
/// consumer — `loadPermissionSystemConfig` (`:117`), `savePermissionSystemConfig` (`:240`), the
/// modal's displayed path (`index.ts:1509`) — defaults to it.
///
/// The case neither `enabled` test covered: the override points at a file whose `enabled`
/// differs from the pristine template sitting at the default path.
#[test]
fn the_install_probe_reads_the_same_resolved_config_as_the_enabled_switch() {
    without_install_env(|| {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join("agent");
        let cwd = dir.path().join("project");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();

        // The DEFAULT path holds the pristine, crate-written template — the extension's own
        // footprint, therefore NOT an install signal (PERM-002), and `enabled: true`.
        let default_path = agent_dir.join(CONFIG_DIR).join(CONFIG_FILE);
        write_file(&default_path, &ExtensionConfig::default_config_content());

        let override_path = dir.path().join("ops").join("permissions.json");
        with_config_path_override(&override_path, || {
            // The operator's own file, at the override path, with the master switch OFF — the
            // opposite of what the default path says.
            write_file(&override_path, "{\n  \"enabled\": false\n}\n");
            assert!(
                is_installed(&agent_dir, &cwd),
                "the install probe must read the OVERRIDE file (hand-authored ⇒ installed), \
                 not the pristine template still sitting at the default path"
            );
            assert!(
                permission_extension_for_env(agent_dir.clone(), cwd.clone()).is_none(),
                "...and that same file's `\"enabled\": false` is what then declines to attach"
            );

            // Same file, switch ON: the two gates agree in the other direction too.
            write_file(&override_path, "{\n  \"enabled\": true,\n  \"yoloMode\": true\n}\n");
            assert!(is_installed(&agent_dir, &cwd));
            assert!(
                permission_extension_for_env(agent_dir.clone(), cwd.clone()).is_some(),
                "an override file that installs AND enables must attach the gate"
            );

            // The default-path template is inert while the override is in force: nothing reads
            // it and nothing rewrote it.
            assert_eq!(
                std::fs::read_to_string(&default_path).unwrap(),
                ExtensionConfig::default_config_content()
            );
        });

        // MIRROR (must stay green): with NO override in force, both gates read the default
        // path exactly as before, and the pristine template there is still not an install
        // signal — resolving the path did not make the probe looser.
        assert!(!is_installed(&agent_dir, &cwd));
        assert!(permission_extension_for_env(agent_dir.clone(), cwd.clone()).is_none());
        write_file(&default_path, "{\n  \"yoloMode\": true\n}\n");
        assert!(is_installed(&agent_dir, &cwd));
        assert!(permission_extension_for_env(agent_dir.clone(), cwd.clone()).is_some());
    });
}

/// G130(a). Building the gate reads `config.json` ONCE.
///
/// The `enabled` switch landed as its own `ExtensionConfig::load` in
/// [`permission_extension_for_env`], and the constructor immediately loaded the SAME file
/// again. `load` `eprintln!`s on a malformed or unreadable config, so an operator with a
/// corrupt `config.json` saw the identical warning twice per session build where pi — which
/// holds one `extensionConfig` populated by one `loadExtensionConfigState()` at
/// `index.ts:1473` — prints it once.
///
/// Counted rather than observed on stderr: `eprintln!` cannot be captured from inside the same
/// process without redirecting fd 2. See `crate::ext_config::LOAD_COUNT`.
#[test]
fn attaching_the_gate_loads_the_extension_config_exactly_once() {
    without_install_env(|| {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join("agent");
        let cwd = dir.path().join("project");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        // An install signal that is NOT the config file, so the probe itself performs no load.
        write_file(&agent_dir.join(POLICY_FILE), r#"{ "bash": { "*": "deny" } }"#);

        crate::ext_config::reset_load_count();
        let attached = permission_extension_for_env(agent_dir.clone(), cwd.clone());
        let loads = crate::ext_config::load_count();
        assert!(attached.is_some(), "precondition: the policy file installs the gate");
        assert_eq!(
            loads, 1,
            "the session build must read config.json once, not once for the `enabled` switch \
             and again inside the constructor"
        );

        // MIRROR (must stay green): declining to attach still reads it once — the `enabled`
        // switch has to open the file to answer at all, and the constructor never runs.
        write_file(&agent_dir.join(CONFIG_DIR).join(CONFIG_FILE), "{\n  \"enabled\": false\n}\n");
        crate::ext_config::reset_load_count();
        assert!(permission_extension_for_env(agent_dir.clone(), cwd.clone()).is_none());
        assert_eq!(crate::ext_config::load_count(), 1);

        // MIRROR (must stay green): a NOT-installed dir pays no config load at all, and so
        // never materializes the template as a side effect of deciding not to attach.
        let clean = tempfile::tempdir().unwrap();
        crate::ext_config::reset_load_count();
        assert!(permission_extension_for_env(clean.path().to_path_buf(), cwd.clone()).is_none());
        assert_eq!(crate::ext_config::load_count(), 0);
        assert!(!clean.path().join(CONFIG_DIR).join(CONFIG_FILE).exists());
    });
}
