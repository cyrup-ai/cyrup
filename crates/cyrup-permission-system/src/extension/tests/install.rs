//! The DI-5 install probe: which of the policy file, the agent-markdown frontmatter and the
//! opt-in env var attach the gate, and which do not.
#![allow(clippy::panic)]

use crate::ext_config::ExtensionConfig;

use super::support::*;
use crate::extension::paths::{
    CONFIG_DIR, CONFIG_FILE, POLICY_FILE, PROJECT_AGENT_SUBDIR, policy_agent_dir,
};
use crate::extension::{
    POLICY_AGENT_DIR_ENV_KEY, PermissionSystemExtension, is_installed, permission_extension_for_env,
};

#[test]
fn not_installed_without_policy_or_env_returns_none() {
    // No policy file, env not set → DI-5 zero gating. `INSTALL_ENV_VAR` is sandboxed (and,
    // crucially, LOCKED) by [`without_install_env`]: it is the same opt-in env var
    // `permission_extension_for_env` reads in production, and a developer/CI shell that has
    // genuinely opted in workspace-wide (exactly as this crate's own module doc documents,
    // "opt-in per DI-5") would otherwise make this "no opt-in" case flake on ambient state that
    // has nothing to do with the code path under test.
    //
    // This test used to save/clear/restore the variable inline with NO lock, on the stated
    // grounds that "no other test in this crate reads or writes `INSTALL_ENV_VAR`". That is
    // false — the PERM-002/v0.8.0 tests below all do, via `without_install_env`. A mutex only
    // serializes the parties that take it, so an unlocked mutator races every locked one in
    // both directions: it can clear the variable out from under a sibling, and a sibling's
    // restore can set it back mid-assertion here.
    without_install_env(|| {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_installed(dir.path(), dir.path()));
    });
}

#[test]
fn installed_when_policy_file_present() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(POLICY_FILE), "{}").unwrap();
    assert!(is_installed(dir.path(), dir.path()));
}

// ---------------------------------------------------------------- PERM-023: the install probe
// must see the agent-markdown policy layer the manager ENFORCES.

/// PERM-023 (RED before the fix). `manager_paths_for` wires `agents_dir` and
/// `PermissionManager::load_agent_permissions` reads `<agents_dir>/<agent>.md` frontmatter as an
/// enforced layer (pi `loadAgentPermissionsFrom`, `permission-manager.ts:715-745` @v0.8.0), but
/// `is_installed` looked only at the env var, the two `.jsonc` files and `config.json`. An
/// operator whose ONLY policy artifact is a persona's frontmatter therefore got no extension
/// attached and their `permission:` deny rules were silently inert — a fail-open.
#[test]
fn agent_markdown_frontmatter_alone_installs_the_gate() {
    without_install_env(|| {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join("agent");
        let cwd = dir.path().join("work");
        std::fs::create_dir_all(&cwd).unwrap();
        // No policy file, no config.json, no env var — before the fix this is `false`.
        assert!(
            !is_installed(&agent_dir, &cwd),
            "control: nothing authored yet"
        );

        let agents = agent_dir.join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        // An EMPTY agents dir is not an authored policy.
        assert!(
            !is_installed(&agent_dir, &cwd),
            "an empty agents/ is not an install signal"
        );

        std::fs::write(
            agents.join("coder.md"),
            "---\npermission:\n  tools:\n    bash: deny\n---\n\nYou are a coder.\n",
        )
        .unwrap();
        assert!(
            is_installed(&agent_dir, &cwd),
            "agent-scoped `permission:` frontmatter is an ENFORCED layer, so it must install"
        );
    });
}

/// The project-scoped half of the same signal: `<cwd>/.cyrup/agent/agents/` is wired as
/// `project_agents_dir` and enforced identically.
#[test]
fn project_scoped_agent_markdown_also_installs_the_gate() {
    without_install_env(|| {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join("agent");
        let cwd = dir.path().join("work");
        let project_agents = PROJECT_AGENT_SUBDIR
            .iter()
            .fold(cwd.clone(), |acc, seg| acc.join(seg))
            .join("agents");
        std::fs::create_dir_all(&project_agents).unwrap();
        assert!(!is_installed(&agent_dir, &cwd));
        std::fs::write(
            project_agents.join("reviewer.md"),
            "---\npermission: {}\n---\n",
        )
        .unwrap();
        assert!(is_installed(&agent_dir, &cwd));
    });
}

// ------------------------------------------------ PERM-025: the relocatable global policy root

/// PERM-025 (RED before the fix — `POLICY_AGENT_DIR_ENV_KEY` had zero occurrences anywhere in
/// cyrup). pi `defaultPolicyAgentDir()` (`permission-manager.ts:31-33` @v0.8.0) relocates all
/// four global policy artifacts, and `createPermissionManagerForCwd` (`index.ts:1287-1301`)
/// supplies only the PROJECT paths, so in a live pi session every global path comes from that
/// override. Both the probe and the engine must consult it, or they inspect different trees.
#[test]
fn the_policy_agent_dir_override_moves_both_the_probe_and_the_engine() {
    without_install_env(|| {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join("agent");
        let elsewhere = dir.path().join("elsewhere");
        let cwd = dir.path().join("work");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(elsewhere.join(POLICY_FILE), r#"{"tools":{"bash":"deny"}}"#).unwrap();

        // Control: the policy lives somewhere the un-overridden probe cannot see.
        assert!(!is_installed(&agent_dir, &cwd));
        assert_eq!(
            PermissionSystemExtension::manager_paths_for(&agent_dir, &cwd).global_config_path,
            agent_dir.join(POLICY_FILE)
        );

        // A [`crate::envx`] pin, not a process mutation: only this thread sees the override.
        let (installed, paths) = {
            let _pin = crate::envx::pin(
                POLICY_AGENT_DIR_ENV_KEY,
                Some(&elsewhere.display().to_string()),
            );
            (
                is_installed(&agent_dir, &cwd),
                PermissionSystemExtension::manager_paths_for(&agent_dir, &cwd),
            )
        };

        assert!(
            installed,
            "the probe must follow the override, or it fails OPEN"
        );
        assert_eq!(paths.global_config_path, elsewhere.join(POLICY_FILE));
        assert_eq!(paths.agents_dir, elsewhere.join("agents"));
        assert_eq!(
            paths.legacy_global_settings_path,
            elsewhere.join("settings.json")
        );
        assert_eq!(paths.global_mcp_config_path, elsewhere.join("mcp.json"));
        // The PROJECT paths are supplied explicitly upstream too and must NOT be relocated.
        let project = PROJECT_AGENT_SUBDIR
            .iter()
            .fold(cwd.clone(), |acc, seg| acc.join(seg));
        assert_eq!(
            paths.project_global_config_path,
            Some(project.join(POLICY_FILE))
        );
    });
}

/// pi's precedence detail: `process.env[KEY]?.trim()` and then a JS truthiness test, so a value
/// that trims to `""` is NOT an override.
#[test]
fn a_blank_policy_agent_dir_override_is_not_an_override() {
    let dir = tempfile::tempdir().unwrap();
    // A [`crate::envx`] pin, not a process mutation: only this thread sees the override.
    let resolved = {
        let _pin = crate::envx::pin(POLICY_AGENT_DIR_ENV_KEY, Some("   "));
        policy_agent_dir(dir.path())
    };
    assert_eq!(
        resolved,
        dir.path(),
        "a whitespace-only value is falsy in pi and inert here"
    );
}

// ----------------------------------------------- PERM-028: `decisionScope` trims like pi's

// ============================================================================================
// PERM-002 / PERM-003 regression tests.
// ============================================================================================

/// PERM-002. Merely CONSTRUCTING the extension materializes
/// `<agent_dir>/cyrup-permission-system/config.json` on disk (`ExtensionConfig::ensure_on_disk`),
/// and `is_installed` used to accept that file's bare existence as an install signal. So one
/// `CYRUP_PERMISSION_SYSTEM=1` run permanently latched the gate on for every later run in that
/// agent dir, with no way to turn it back off — deleting the file did not help either, because
/// the next construction re-created it.
///
/// Observable contract, all three directions in one test:
///  1. after a full construct-and-materialize cycle with no env opt-in and no policy file,
///     `is_installed` is false again and `permission_extension_for_env` attaches NOTHING;
///  2. an operator-edited `config.json` still installs (the fix cannot silently disable a gate
///     whose only install signal was a hand-written config);
///  3. a policy file still installs, unaffected.
#[test]
fn auto_materialized_config_does_not_latch_the_gate_on() {
    without_install_env(|| {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join("agent");
        let cwd = dir.path().join("project");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        let config_path = agent_dir.join(CONFIG_DIR).join(CONFIG_FILE);

        assert!(
            !is_installed(&agent_dir, &cwd),
            "clean agent dir must not be installed"
        );

        // The opt-in run: build the extension exactly as the binary wiring does. This is the
        // step that writes `config.json`.
        let installed = PermissionSystemExtension::new(agent_dir.clone(), cwd.clone());
        drop(installed);
        assert!(
            config_path.exists(),
            "constructing the extension must still materialize the editable config template"
        );

        // The NEXT run, with the env opt-in gone and no policy file anywhere. The leftover
        // template is the extension's own footprint, not an operator decision.
        assert!(
            !is_installed(&agent_dir, &cwd),
            "the auto-written config template must not latch the gate on for every later run"
        );
        assert!(
            permission_extension_for_env(agent_dir.clone(), cwd.clone()).is_none(),
            "an un-opted-in run must attach no gate at all"
        );

        // An operator who configures the extension by hand IS opting in — that signal must
        // survive, or un-latching would have become a way to silently disable a real gate.
        std::fs::write(&config_path, "{\n  \"yoloMode\": true\n}\n").unwrap();
        assert!(
            is_installed(&agent_dir, &cwd),
            "a hand-edited config.json must still install the gate"
        );

        // ...and reverting it to the pristine template turns it back off: the switch is
        // two-way, which is the whole point.
        std::fs::write(&config_path, ExtensionConfig::default_config_content()).unwrap();
        assert!(
            !is_installed(&agent_dir, &cwd),
            "reverting the config must turn the gate off"
        );

        // A policy file remains an install signal regardless of the config file.
        write_file(
            &agent_dir.join(POLICY_FILE),
            r#"{ "bash": { "*": "deny" } }"#,
        );
        assert!(
            is_installed(&agent_dir, &cwd),
            "a policy file must still install the gate"
        );
    });
}

// ------------------------------------------------------- CFG-074: the two cyrup-ORIGINAL env vars

/// CFG-074 — `CYRUP_PERMISSION_SYSTEM` is cyrup-only (`pi-permission-system` v0.8.0 has no install
/// flag), and the property that makes the delta safe is that it is **ADDITIVE ONLY**: it can turn
/// the gate ON in addition to upstream's policy-artifact rule, and there is no spelling of it that
/// turns the gate OFF.
///
/// Killing mutation: any rewrite of `is_installed` that makes the flag authoritative — e.g.
/// `env_truthy(INSTALL_ENV_VAR) && has_policy(..)`, or an `is_some() && !truthy -> false` early
/// return. Each would let a falsy value disable a REAL policy file, which is the fail-open this
/// asserts cannot exist.
#[test]
fn the_install_flag_is_additive_only_and_can_never_disable_a_real_policy() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path().join("agent");
    let cwd = dir.path().join("work");
    std::fs::create_dir_all(&cwd).unwrap();

    // (a) The flag alone installs, with no policy artifact anywhere — the one thing it is for.
    for truthy in ["1", "true", "on", "yes", " true "] {
        let _pin = crate::envx::pin(crate::extension::INSTALL_ENV_VAR, Some(truthy));
        assert!(
            is_installed(&agent_dir, &cwd),
            "{truthy:?} must force-install the gate"
        );
    }

    // (b) Anything else is simply "not opted in" — never "opted out".
    for falsy in ["", "0", "false", "off", "no", "maybe"] {
        let _pin = crate::envx::pin(crate::extension::INSTALL_ENV_VAR, Some(falsy));
        assert!(
            !is_installed(&agent_dir, &cwd),
            "{falsy:?} must not install a gate with no policy artifact"
        );
    }

    // (c) With a real policy file present, NO value of the flag can take the gate away. This is the
    //     assertion that pins "additive only"; (b) alone would still pass a gating implementation.
    let policy_dir = policy_agent_dir(&agent_dir);
    std::fs::create_dir_all(&policy_dir).unwrap();
    std::fs::write(
        policy_dir.join(POLICY_FILE),
        r#"{ "bash": { "*": "deny" } }"#,
    )
    .unwrap();
    for value in ["", "0", "false", "off", "no", "maybe", "1", "true"] {
        let _pin = crate::envx::pin(crate::extension::INSTALL_ENV_VAR, Some(value));
        assert!(
            is_installed(&agent_dir, &cwd),
            "a real policy file must install the gate whatever {value:?} says"
        );
    }
}

/// CFG-074's `Verify` line for this crate: each env var this crate INVENTED carries a
/// `[CYRUP-DELTA]` in the doc block that declares it, and the one that turned out to be a RENAME of
/// a real upstream variable cites the upstream name instead — so the next enumeration of the env
/// surface reads the answer at the declaration rather than re-filing it.
///
/// Killing mutation: deleting any of the three doc blocks.
#[test]
fn every_cyrup_only_env_var_in_this_crate_carries_a_cyrup_delta() {
    let env_rs = include_str!("../env.rs");
    let forwarding_rs = include_str!("../../forwarding.rs");
    for (source, decl) in [
        (env_rs, "pub const INSTALL_ENV_VAR"),
        (forwarding_rs, "pub const CHILD_WAIT_TIMEOUT_ENV"),
    ] {
        let doc = doc_block_above(source, decl);
        assert!(
            doc.contains("[CYRUP-DELTA]"),
            "{decl} is cyrup-original and must declare it:\n{doc}"
        );
    }
    // The two renames of real upstream variables must name what they rename, NOT claim a delta.
    for (source, decl, upstream) in [
        (
            env_rs,
            "pub const POLICY_AGENT_DIR_ENV_KEY",
            "PI_PERMISSION_SYSTEM_POLICY_AGENT_DIR",
        ),
        (
            forwarding_rs,
            "pub const FORWARDING_AGENT_DIR_ENV",
            "PERMISSION_FORWARDING_AGENT_DIR_ENV_KEY",
        ),
    ] {
        let doc = doc_block_above(source, decl);
        assert!(
            doc.contains(upstream),
            "{decl} renames {upstream} and must say so:\n{doc}"
        );
    }
}

/// The contiguous run of `///` lines immediately above the line starting with `decl`.
fn doc_block_above(source: &str, decl: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let at = lines
        .iter()
        .position(|line| line.trim_start().starts_with(decl))
        .unwrap_or_else(|| panic!("{decl} not found"));
    let mut start = at;
    while start > 0 && lines[start - 1].trim_start().starts_with("///") {
        start -= 1;
    }
    assert!(start < at, "{decl} has no doc block at all");
    lines[start..at].join("\n")
}
