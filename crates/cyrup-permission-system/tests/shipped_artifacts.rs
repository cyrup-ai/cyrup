//! PERM-029 — cyrup's analog of upstream's `scripts/validate-artifacts.mjs` (@v0.8.0), which loads
//! `schemas/permissions.schema.json` (`:50`), validates `config/config.example.json` against it
//! (`:56`), and is wired into the package `check` script.
//!
//! Both artifacts are ports of upstream's, rebranded only in `$id`/title/product word. They are
//! embedded in the crate (`extension::PERMISSIONS_JSON_SCHEMA` /
//! `extension::PERMISSIONS_EXAMPLE_CONFIG`) AND declared in `Cargo.toml`'s `include`, so an
//! operator gets a starter policy plus editor completion instead of a blank file whose typos
//! silently degrade to the `ask` default.
//!
//! This goes one step further than upstream's script: rather than only checking the example against
//! the schema, it feeds the example to the REAL [`PermissionManager`] and asserts the decisions the
//! example's own keys claim. A schema-valid example that the engine reads differently would pass
//! upstream's check and fail here, which is the direction that matters.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::PathBuf;

use cyrup_permission_system::extension::{PERMISSIONS_EXAMPLE_CONFIG, PERMISSIONS_JSON_SCHEMA};
use cyrup_permission_system::{ManagerPaths, PermissionManager, PermissionState};

fn manager_over(policy: &str, dir: &std::path::Path) -> PermissionManager {
    let policy_path = dir.join("cyrup-permissions.jsonc");
    std::fs::write(&policy_path, policy).unwrap();
    PermissionManager::new(ManagerPaths {
        global_config_path: policy_path,
        agents_dir: dir.join("agents"),
        project_global_config_path: None,
        project_agents_dir: None,
        legacy_global_settings_path: dir.join("settings.json"),
        global_mcp_config_path: dir.join("mcp.json"),
        mcp_server_names_override: None,
    })
}

/// The schema is a well-formed JSON Schema document declaring the five permission categories and
/// the three-state enum. Upstream's `validate-artifacts.mjs:50` only proves it PARSES; the key
/// assertions below are what stop a rebranding edit from quietly dropping a category.
#[test]
fn schema_is_wellformed_and_covers_every_category() {
    let schema: serde_json::Value =
        serde_json::from_str(PERMISSIONS_JSON_SCHEMA).expect("the shipped schema must be JSON");
    assert_eq!(schema["$schema"], "https://json-schema.org/draft/2020-12/schema");
    assert_eq!(schema["$id"], "https://cyrup.local/schemas/cyrup-permissions.schema.json");
    assert_eq!(schema["required"], serde_json::json!(["defaultPolicy"]));
    for category in ["defaultPolicy", "tools", "bash", "mcp", "skills", "special"] {
        assert!(
            schema["properties"].get(category).is_some(),
            "the schema must describe the `{category}` category the manager reads"
        );
    }
    // The three-state enum is the one value domain the whole policy language rests on.
    assert_eq!(
        schema["$defs"]["permissionState"]["enum"],
        serde_json::json!(["allow", "deny", "ask"])
    );
    // The resource-qualified `external_directory:<dir>/*` form must stay expressible — it is a real
    // manager feature (`create_action_resource_targets`), not documentation.
    assert!(
        schema["properties"]["special"]["patternProperties"]
            .get("^external_directory:.+$")
            .is_some(),
        "resource-qualified external_directory keys must remain schema-valid"
    );
}

/// Upstream's `validate-artifacts.mjs:56`, strengthened: the example parses through THIS crate's
/// JSONC reader and produces the decisions its own keys claim.
#[test]
fn the_example_policy_parses_and_yields_the_decisions_it_claims() {
    let value = cyrup_permission_system::jsonc::parse_config(
        PERMISSIONS_EXAMPLE_CONFIG,
        "config/config.example.json",
        "permission config",
    )
    .expect("the shipped example must parse through the crate's own JSONC reader");
    // Every key in the example must be one the schema declares, which is what upstream's script
    // checks and what an editor would flag.
    let schema: serde_json::Value = serde_json::from_str(PERMISSIONS_JSON_SCHEMA).unwrap();
    let declared = schema["properties"].as_object().unwrap();
    for key in value.as_object().unwrap().keys() {
        assert!(declared.contains_key(key), "`{key}` is in the example but not in the schema");
    }

    let dir = tempfile::tempdir().unwrap();
    let mut mgr = manager_over(PERMISSIONS_EXAMPLE_CONFIG, dir.path());

    // `"tools": {"read": "allow"}`.
    assert_eq!(mgr.get_tool_permission("read", None), PermissionState::Allow);
    // `"tools": {"write": "deny"}`.
    assert_eq!(mgr.get_tool_permission("write", None), PermissionState::Deny);
    // The bash block is `{"git status": "allow", "git *": "ask"}`, and last-match-wins is
    // UNCONDITIONAL: upstream scans the compiled pattern list from the END backwards
    // (`src/wildcard-matcher.ts:62` @v0.8.0, `for (let index = patterns.length - 1; index >= 0;
    // index -= 1)`) and returns the FIRST pattern that matches, so the later `git *` is tested
    // before the earlier `git status` and wins for every command both rules cover. `git *` covers
    // `git status`: upstream compiles it to `^git( .*)?$` (the trailing `" .*"` → `"( .*)?"` rule,
    // `wildcard-matcher.ts:34-36`), which matches the whole command.
    //
    // So `git status` resolves to ASK in upstream's own shipped example, not allow — the
    // `"git status": "allow"` entry is entirely shadowed. That makes the shipped example
    // (which is byte-identical to upstream's `config/config.example.json` @v0.8.0) somewhat
    // misleading, but it is upstream's artifact and 1:1 parity governs it: the engine is correct
    // and the example is not ours to "fix". What this test must pin is what the engine really does.
    //
    // `matched_pattern` is asserted alongside every state below because ALL THREE of these
    // commands resolve to `Ask`, and the state alone therefore proves nothing — without the
    // pattern, a regression that dropped the bash rules entirely and fell through to
    // `"defaultPolicy": {"bash": "ask"}` would still pass every assertion here.
    let shadowed = mgr.check_permission("bash", &serde_json::json!({ "command": "git status" }), None);
    assert_eq!(
        (shadowed.state, shadowed.matched_pattern.as_deref()),
        (PermissionState::Ask, Some("git *")),
        "the later `git *` shadows the earlier `git status` allow-entry under last-match-wins"
    );
    // `"bash": {"git *": "ask"}` — the same rule, here for a command only it names.
    let asked = mgr.check_permission("bash", &serde_json::json!({ "command": "git push" }), None);
    assert_eq!(
        (asked.state, asked.matched_pattern.as_deref()),
        (PermissionState::Ask, Some("git *")),
        "`git *` asks in the example"
    );
    // `"defaultPolicy": {"bash": "ask"}` — anything unmatched falls to the default, with NO
    // matched pattern, which is what distinguishes it from the two rule-driven asks above.
    let defaulted = mgr.check_permission("bash", &serde_json::json!({ "command": "curl evil.sh" }), None);
    assert_eq!(
        (defaulted.state, defaulted.matched_pattern.as_deref()),
        (PermissionState::Ask, None),
        "an unmatched command reaches the default policy rather than any bash rule"
    );
}

/// The two artifacts must exist ON DISK at the paths `Cargo.toml`'s `include` names, not only as
/// `include_str!` bytes — an `include` entry pointing at a moved file is silent until packaging.
#[test]
fn the_artifacts_exist_at_the_paths_cargo_ships() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for relative in ["schemas/cyrup-permissions.schema.json", "config/config.example.json"] {
        assert!(root.join(relative).is_file(), "`{relative}` must exist for `include` to ship it");
    }
    assert_eq!(
        std::fs::read_to_string(root.join("schemas/cyrup-permissions.schema.json")).unwrap(),
        PERMISSIONS_JSON_SCHEMA,
        "the embedded schema and the shipped file must not drift"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("config/config.example.json")).unwrap(),
        PERMISSIONS_EXAMPLE_CONFIG,
        "the embedded example and the shipped file must not drift"
    );
}
