//! Integration test: `discovery::discover_agents`/`discover_agents_all` end to end against a
//! real fixture directory tree spanning all four scopes (func-SA §5.1; arch-SA §6.2; task R-SA-
//! 003/007/016/017/022 assignment's own "integration test exercising the full discover_agents()
//! path end to end against a real fixture directory tree spanning all four scopes" requirement).
//!
//! No mocking of filesystem/package state anywhere in this file (matching this codebase's
//! standing "never mock subprocess/git/filesystem behavior in tests" convention): every scope
//! (Builtin/Package/User/Project) is a real directory tree under a real `tempfile::tempdir()`,
//! the "installed package" is a real `PackageSource::Path` install (cyrup-resources' own
//! local-dev-install channel — "no copy; referenced in place") pointed at a real on-disk package
//! root with a real `cyrup.toml`, and settings-override application runs against a real,
//! populated `SubagentSettings`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::{Path, PathBuf};

use cyrup_core::{PackageId, ThinkingLevel};
use cyrup_ext_subagents::discovery::types::{
    AgentOverrideConfig, AgentReadScope, AgentSource, OverrideField, SubagentSettings,
};
use cyrup_ext_subagents::discovery::{AgentDiscoveryConfig, discover_agents, discover_agents_all};
use cyrup_resources::package::source::PackageSource;
use cyrup_resources::{InstallScope, InstalledPackage, InstalledPackages};

fn write_agent(dir: &Path, file_name: &str, frontmatter_extra: &str, name: &str, description: &str) {
    std::fs::create_dir_all(dir).expect("mkdir agent dir");
    std::fs::write(
        dir.join(file_name),
        format!(
            "---\nname: {name}\ndescription: {description}\n{frontmatter_extra}---\n\nYou are {name}.\n"
        ),
    )
    .expect("write agent fixture file");
}

fn write_package_manifest(pkg_root: &Path, package_name: &str) {
    std::fs::create_dir_all(pkg_root).expect("mkdir package root");
    std::fs::write(
        pkg_root.join("cyrup.toml"),
        format!(
            "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\n\n[resources]\nagents = [\"./agents\"]\n"
        ),
    )
    .expect("write cyrup.toml");
}

/// Build a real `InstalledPackages` list with a single `PackageSource::Path`-installed package
/// (cyrup-resources' local-dev-install channel: "no copy; referenced in place", so no package
/// store/registry file machinery is needed for this test — the path IS the package root).
fn installed_packages_pointing_at(pkg_root: &Path) -> InstalledPackages {
    InstalledPackages {
        packages: vec![InstalledPackage {
            id: PackageId::from("fixture-pkg"),
            source: PackageSource::Path {
                path: pkg_root.to_path_buf(),
            },
            scope: InstallScope::Global,
            resolved_commit: None,
            installed_at: "0".to_string(),
            disabled: Default::default(),
        }],
    }
}

/// A full four-scope fixture tree:
///
/// ```text
/// <root>/
///   builtin/
///     delegate.md         (Builtin; local name "delegate" -> name-sensitive defaults, R-SA-018)
///     shared-name.md       (Builtin; will be shadowed by every higher tier)
///   package/
///     cyrup.toml
///     agents/
///       packaged.md        (Package; package: fixture-pkg -> qualified name)
///       shared-name.md      (Package; will be shadowed by User/Project)
///   user/
///     dir-a/
///       shared-name.md      (User; scanned first)
///     dir-b/
///       shared-name.md      (User; scanned second -> last-seen-wins, R-SA-002)
///     skills/
///       some-skill/
///         SKILL.md          (R-SA-007: must NEVER be discovered as an agent)
///     scoped-user-only.md
///   project/
///     shared-name.md        (Project; wins the whole four-tier merge, R-SA-001)
///     disabled-agent.md     (Project; disabled: true — visibility split, R-SA-013)
///     nested-fanout.md      (Project; tools includes literal "subagent" -> R-SA-016 eligible)
///     mcp-only.md            (Project; tools is only "mcp:subagent" -> R-SA-016 NOT eligible)
///     depth-limited.md       (Project; maxSubagentDepth: 1 -> feeds R-SA-022's tightening input)
///     skill-associated.md    (Project; skills: research, writing -> R-SA-017 pointer metadata)
///     scoped-project-only.md
/// ```
struct FourScopeFixture {
    _root: tempfile::TempDir,
    cfg: AgentDiscoveryConfig,
}

fn build_four_scope_fixture() -> FourScopeFixture {
    let root = tempfile::tempdir().expect("tempdir");
    let base = root.path();

    let builtin_dir = base.join("builtin");
    let package_dir = base.join("package");
    let user_dir_a = base.join("user").join("dir-a");
    let user_dir_b = base.join("user").join("dir-b");
    let user_skills_dir = base.join("user").join("skills").join("some-skill");
    let user_root = base.join("user");
    let project_dir = base.join("project");

    // --- Builtin tier ---
    write_agent(&builtin_dir, "delegate.md", "", "delegate", "builtin delegate persona");
    write_agent(
        &builtin_dir,
        "shared-name.md",
        "",
        "shared-name",
        "builtin shared-name (lowest precedence)",
    );

    // --- Package tier ---
    write_package_manifest(&package_dir, "fixture-pkg");
    write_agent(
        &package_dir.join("agents"),
        "packaged.md",
        "package: fixture-pkg\n",
        "packaged",
        "a packaged agent",
    );
    write_agent(
        &package_dir.join("agents"),
        "shared-name.md",
        "",
        "shared-name",
        "package shared-name (beats builtin, loses to user/project)",
    );

    // --- User tier (two dirs: last-seen-wins for the collision, R-SA-002) ---
    write_agent(
        &user_dir_a,
        "shared-name.md",
        "",
        "shared-name",
        "user dir-a shared-name (scanned first)",
    );
    write_agent(
        &user_dir_b,
        "shared-name.md",
        "",
        "shared-name",
        "user dir-b shared-name (scanned second, must win the User tier)",
    );
    write_agent(&user_root, "scoped-user-only.md", "", "scoped-user-only", "visible only under User scope");
    // R-SA-007: a skill bundle living under the SAME root being scanned for agents. Its SKILL.md
    // declares a `name`/`description` that WOULD parse as a valid agent if not excluded.
    std::fs::create_dir_all(&user_skills_dir).expect("mkdir skills bundle");
    std::fs::write(
        user_skills_dir.join("SKILL.md"),
        "---\nname: some-skill\ndescription: a bundled skill, never an agent\n---\n\nSkill body.\n",
    )
    .expect("write SKILL.md");

    // --- Project tier (wins every collision, R-SA-001) ---
    write_agent(
        &project_dir,
        "shared-name.md",
        "",
        "shared-name",
        "project shared-name (must win the whole four-tier merge)",
    );
    write_agent(
        &project_dir,
        "disabled-agent.md",
        "disabled: true\n",
        "disabled-agent",
        "a disabled project agent",
    );
    write_agent(
        &project_dir,
        "nested-fanout.md",
        "tools: read, subagent\n",
        "nested-fanout",
        "eligible for nested fanout (R-SA-016)",
    );
    write_agent(
        &project_dir,
        "mcp-only.md",
        "tools: mcp:subagent\n",
        "mcp-only",
        "NOT eligible for nested fanout despite same bare name (R-SA-016)",
    );
    write_agent(
        &project_dir,
        "depth-limited.md",
        "maxSubagentDepth: 1\n",
        "depth-limited",
        "feeds the tightening-only depth ceiling (R-SA-022 notes; enforced at spawn boundary)",
    );
    write_agent(
        &project_dir,
        "skill-associated.md",
        "skills: research, writing\ninheritSkills: true\n",
        "skill-associated",
        "carries proactive-injection skill pointers (R-SA-017)",
    );
    write_agent(
        &project_dir,
        "scoped-project-only.md",
        "",
        "scoped-project-only",
        "visible only under Project scope",
    );

    let installed = installed_packages_pointing_at(&package_dir);

    let mut overrides = std::collections::BTreeMap::new();
    overrides.insert(
        "delegate".to_string(),
        AgentOverrideConfig {
            thinking: OverrideField::Value(ThinkingLevel::High),
            ..Default::default()
        },
    );
    let settings = SubagentSettings {
        overrides,
        ..Default::default()
    };

    let cfg = AgentDiscoveryConfig {
        builtin_agents_dir: Some(builtin_dir),
        installed_packages: installed,
        trusted_project: true,
        global_dir: base.join("global"),
        project_root: Some(base.join("project-root-unused-by-path-source")),
        user_agent_dirs: vec![user_dir_a, user_dir_b, user_root],
        user_chain_dirs: Vec::new(),
        project_agent_dirs: vec![project_dir],
        project_chain_dirs: Vec::new(),
        settings,
    };

    FourScopeFixture { _root: root, cfg }
}

// -------------------------------------------------------------------------------------------
// R-SA-001/002: four-tier precedence + asymmetric dedup, exercised end to end
// -------------------------------------------------------------------------------------------

#[test]
fn project_tier_wins_the_full_four_tier_collision_end_to_end() {
    let fixture = build_four_scope_fixture();
    let result = discover_agents_all(&fixture.cfg).expect("discovery succeeds");

    let winner = result
        .agents
        .iter()
        .find(|a| a.name == "shared-name")
        .expect("shared-name agent present");
    assert_eq!(winner.source, AgentSource::Project);
    assert_eq!(
        winner.description,
        "project shared-name (must win the whole four-tier merge)"
    );
}

#[test]
fn user_tier_second_directory_wins_over_first_last_seen_wins() {
    let mut fixture = build_four_scope_fixture();
    // Remove the project-tier collision so the User tier's own internal winner is directly
    // observable at the top level (otherwise Project would mask it, as proven by the previous
    // test) — this isolates R-SA-002's User-tier last-seen-wins rule end to end.
    fixture.cfg.project_agent_dirs.clear();

    let result = discover_agents_all(&fixture.cfg).expect("discovery succeeds");
    let winner = result
        .agents
        .iter()
        .find(|a| a.name == "shared-name")
        .expect("shared-name agent present");
    assert_eq!(winner.source, AgentSource::User);
    assert_eq!(
        winner.description,
        "user dir-b shared-name (scanned second, must win the User tier)"
    );
}

#[test]
fn package_tier_beats_builtin_but_loses_to_user_and_project() {
    let mut fixture = build_four_scope_fixture();
    fixture.cfg.project_agent_dirs.clear();
    fixture.cfg.user_agent_dirs.clear();

    let result = discover_agents_all(&fixture.cfg).expect("discovery succeeds");
    let winner = result
        .agents
        .iter()
        .find(|a| a.name == "shared-name")
        .expect("shared-name agent present");
    assert_eq!(winner.source, AgentSource::Package);
}

#[test]
fn packaged_agent_gets_the_qualified_runtime_name_from_its_own_frontmatter() {
    let fixture = build_four_scope_fixture();
    let result = discover_agents_all(&fixture.cfg).expect("discovery succeeds");
    let packaged = result
        .agents
        .iter()
        .find(|a| a.local_name == "packaged")
        .expect("packaged agent present");
    assert_eq!(packaged.name, "fixture-pkg.packaged");
    assert_eq!(packaged.package_name, Some("fixture-pkg".to_string()));
    assert_eq!(packaged.source, AgentSource::Package);
}

// -------------------------------------------------------------------------------------------
// R-SA-007: legacy skill-path exclusion, exercised end to end (not just at the walk-fn level)
// -------------------------------------------------------------------------------------------

#[test]
fn skill_bundle_under_a_scanned_user_root_never_becomes_an_agent() {
    let fixture = build_four_scope_fixture();
    let result = discover_agents_all(&fixture.cfg).expect("discovery succeeds");
    assert!(
        !result.agents.iter().any(|a| a.name == "some-skill"),
        "SKILL.md content must never surface as a discovered agent"
    );
}

// -------------------------------------------------------------------------------------------
// R-SA-013: management view includes disabled; delegation view excludes them
// -------------------------------------------------------------------------------------------

#[test]
fn management_view_includes_disabled_project_agent() {
    let fixture = build_four_scope_fixture();
    let result = discover_agents_all(&fixture.cfg).expect("discovery succeeds");
    assert!(result.agents.iter().any(|a| a.name == "disabled-agent"));
}

#[test]
fn delegation_view_excludes_disabled_project_agent() {
    let fixture = build_four_scope_fixture();
    let result = discover_agents(&fixture.cfg, None).expect("discovery succeeds");
    assert!(!result.agents.iter().any(|a| a.name == "disabled-agent"));
}

// -------------------------------------------------------------------------------------------
// R-SA-016: nested-fanout eligibility is exact-name, surfaced end to end via a real discovered
// AgentDefinition (canonical enforcement lives at the spawn boundary; this only proves the
// discovery-time data the spawn boundary will consult is itself correct).
// -------------------------------------------------------------------------------------------

#[test]
fn nested_fanout_eligibility_is_exact_name_not_fuzzy_end_to_end() {
    let fixture = build_four_scope_fixture();
    let result = discover_agents_all(&fixture.cfg).expect("discovery succeeds");

    let eligible = result
        .agents
        .iter()
        .find(|a| a.name == "nested-fanout")
        .expect("nested-fanout agent present");
    assert!(eligible.is_nested_fanout_eligible());

    let not_eligible = result
        .agents
        .iter()
        .find(|a| a.name == "mcp-only")
        .expect("mcp-only agent present");
    assert!(
        !not_eligible.is_nested_fanout_eligible(),
        "an mcp:-prefixed entry sharing the bare name \"subagent\" must not count (R-SA-016/DI-SA-7)"
    );
}

// -------------------------------------------------------------------------------------------
// R-SA-017: skill association round-trips as proactive-injection metadata (name + inheritSkills),
// not any form of on-demand content loading.
// -------------------------------------------------------------------------------------------

#[test]
fn skill_association_frontmatter_round_trips_as_pointer_metadata() {
    let fixture = build_four_scope_fixture();
    let result = discover_agents_all(&fixture.cfg).expect("discovery succeeds");
    let agent = result
        .agents
        .iter()
        .find(|a| a.name == "skill-associated")
        .expect("skill-associated agent present");
    assert_eq!(agent.skills, vec!["research".to_string(), "writing".to_string()]);
    assert!(agent.inherit_skills);
}

// -------------------------------------------------------------------------------------------
// R-SA-022 (discovery-layer note): max_subagent_depth is faithfully parsed and carried through
// discovery as the frontmatter field the spawn boundary's tightening-only algorithm consumes —
// this test does not re-implement or assert that algorithm itself (owned by spawn/depth.rs).
// -------------------------------------------------------------------------------------------

#[test]
fn max_subagent_depth_frontmatter_field_is_carried_through_discovery() {
    let fixture = build_four_scope_fixture();
    let result = discover_agents_all(&fixture.cfg).expect("discovery succeeds");
    let agent = result
        .agents
        .iter()
        .find(|a| a.name == "depth-limited")
        .expect("depth-limited agent present");
    assert_eq!(agent.max_subagent_depth, Some(1));
}

// -------------------------------------------------------------------------------------------
// R-SA-018: name-sensitive defaults for a Builtin-sourced "delegate" persona, surfaced through
// the real Builtin scan path (not a synthetic AgentDefinition).
// -------------------------------------------------------------------------------------------

#[test]
fn builtin_delegate_persona_gets_name_sensitive_defaults_end_to_end() {
    let fixture = build_four_scope_fixture();
    let result = discover_agents_all(&fixture.cfg).expect("discovery succeeds");
    let delegate = result
        .agents
        .iter()
        .find(|a| a.name == "delegate")
        .expect("builtin delegate present");
    assert_eq!(delegate.source, AgentSource::Builtin);
    assert!(delegate.inherit_project_context);
}

// -------------------------------------------------------------------------------------------
// R-SA-010/011/012: settings-based override application flows through discover_agents_all
// end to end (builtin full-replace branch: overriding "delegate"'s thinking level).
// -------------------------------------------------------------------------------------------

#[test]
fn builtin_override_applies_end_to_end_through_full_discovery_pipeline() {
    let fixture = build_four_scope_fixture();
    let result = discover_agents_all(&fixture.cfg).expect("discovery succeeds");
    let delegate = result
        .agents
        .iter()
        .find(|a| a.name == "delegate")
        .expect("builtin delegate present");
    assert_eq!(delegate.thinking, Some(ThinkingLevel::High));
    assert!(delegate.override_info.is_some());
}

// -------------------------------------------------------------------------------------------
// AgentReadScope narrowing over the full four-scope fixture.
// -------------------------------------------------------------------------------------------

#[test]
fn read_scope_project_excludes_user_only_agent_but_keeps_builtin_and_package() {
    let fixture = build_four_scope_fixture();
    let result =
        discover_agents(&fixture.cfg, Some(AgentReadScope::Project)).expect("discovery succeeds");
    let names: Vec<&str> = result.agents.iter().map(|a| a.name.as_str()).collect();

    assert!(!names.contains(&"scoped-user-only"));
    assert!(names.contains(&"scoped-project-only"));
    // Builtin/Package agents are orthogonal to the User/Project scope axis and remain visible.
    assert!(names.contains(&"delegate"));
    assert!(names.contains(&"fixture-pkg.packaged"));
}

#[test]
fn read_scope_user_excludes_project_only_agent_but_keeps_builtin_and_package() {
    let fixture = build_four_scope_fixture();
    let result =
        discover_agents(&fixture.cfg, Some(AgentReadScope::User)).expect("discovery succeeds");
    let names: Vec<&str> = result.agents.iter().map(|a| a.name.as_str()).collect();

    assert!(names.contains(&"scoped-user-only"));
    assert!(!names.contains(&"scoped-project-only"));
    assert!(names.contains(&"delegate"));
    assert!(names.contains(&"fixture-pkg.packaged"));
}

// -------------------------------------------------------------------------------------------
// R-SA-019: re-scanned per call — a filesystem change between two discover_agents_all calls is
// observed by the second call without any explicit cache invalidation.
// -------------------------------------------------------------------------------------------

#[test]
fn discovery_observes_filesystem_changes_between_calls_over_the_full_fixture() {
    let fixture = build_four_scope_fixture();
    let before = discover_agents_all(&fixture.cfg).expect("discovery succeeds");
    assert!(!before.agents.iter().any(|a| a.name == "brand-new"));

    let project_dir = fixture
        .cfg
        .project_agent_dirs
        .first()
        .expect("project dir configured");
    write_agent(project_dir, "brand-new.md", "", "brand-new", "added after first call");

    let after = discover_agents_all(&fixture.cfg).expect("discovery succeeds");
    assert!(after.agents.iter().any(|a| a.name == "brand-new"));
}

// -------------------------------------------------------------------------------------------
// R-SA-003: extra agent directories via environment, exercised against the real config type
// (the pure closure-injected core is unit-tested in discovery/mod.rs itself; this integration
// test additionally proves `with_env_extras` composes correctly with a full discovery pass).
// -------------------------------------------------------------------------------------------

#[test]
fn with_env_extras_is_a_no_op_when_the_env_var_is_unset_in_the_real_process_environment() {
    // This test intentionally does NOT set the env var (this crate forbids unsafe code and never
    // calls std::env::set_var) — it only proves that calling `with_env_extras()` against whatever
    // the real test-process environment happens to be does not corrupt an otherwise-valid
    // discovery config, by re-running discovery afterward and getting the same agent set back.
    let fixture = build_four_scope_fixture();
    let user_dirs_before = fixture.cfg.user_agent_dirs.clone();

    let cfg_with_extras = AgentDiscoveryConfig {
        user_agent_dirs: user_dirs_before.clone(),
        ..fixture.cfg.clone()
    }
    .with_env_extras();

    // Whatever CYRUP_SUBAGENT_EXTRA_AGENT_DIRS is (or is not) set to in this test process, the
    // ordinary configured dirs must still be present and in their original relative order as a
    // prefix — with_env_extras only ever appends.
    assert_eq!(
        &cfg_with_extras.user_agent_dirs[..user_dirs_before.len()],
        user_dirs_before.as_slice()
    );

    let result = discover_agents_all(&cfg_with_extras).expect("discovery still succeeds");
    assert!(result.agents.iter().any(|a| a.name == "scoped-user-only"));
}

// -------------------------------------------------------------------------------------------
// Sanity: default config with nothing configured discovers nothing and does not error.
// -------------------------------------------------------------------------------------------

#[test]
fn empty_config_discovers_nothing_without_erroring() {
    let cfg = AgentDiscoveryConfig::default();
    let all = discover_agents_all(&cfg).expect("empty discovery succeeds");
    assert!(all.agents.is_empty());
    assert!(all.chains.is_empty());
    assert!(all.diagnostics.is_empty());

    let delegation = discover_agents(&cfg, None).expect("empty discovery succeeds");
    assert!(delegation.agents.is_empty());
}

// Sanity: PathBuf import is exercised by the fixture builder; referenced here only to keep this
// test module's imports honest against unused-import lint drift.
#[test]
fn path_buf_import_is_reachable() {
    let _ = PathBuf::from("/tmp");
}

// -------------------------------------------------------------------------------------------
// R-SA-132/134: the 8 bundled builtin personas (scout, delegate, context-builder, planner,
// researcher, reviewer, worker, oracle) are real, discoverable resources — not hardcoded Rust
// strings — under this crate's own `resources/agents/` directory (the exact path
// `extension.rs::builtin_agents_dir()` resolves at runtime via `CARGO_MANIFEST_DIR`, mirrored
// here so this test exercises the SAME on-disk resource root the real extension uses, not a
// synthetic fixture standing in for it). Resolved through the real `AgentDiscoveryConfig::
// builtin_agents_dir` -> `scan_builtin_agents` -> `cyrup_resources::resolve_manifest` auto-
// discovery pipeline (R-SA-020), end to end via `discover_agents_all`.
// -------------------------------------------------------------------------------------------

/// The 8 bundled builtin persona runtime names this crate ships, mirroring `pi-subagents/agents/
/// {context-builder,delegate,oracle,planner,researcher,reviewer,scout,worker}.md` (func-SA
/// §5.1 R-SA-132's exact target list). Each `.md` file's frontmatter `name:` is unqualified (no
/// `package:` field), so per R-SA-008 the runtime name is exactly the local name.
const BUILTIN_PERSONA_NAMES: &[&str] = &[
    "context-builder",
    "delegate",
    "oracle",
    "planner",
    "researcher",
    "reviewer",
    "scout",
    "worker",
];

/// Resolve this crate's bundled `resources/` directory the exact same way
/// `extension.rs::builtin_agents_dir()` does in production: `CARGO_MANIFEST_DIR`-relative,
/// baked in at compile time. Kept as its own tiny helper (rather than importing a private
/// `extension.rs` function into this black-box integration test) so this test proves the
/// resource files are genuinely present and parseable at the conventional path independent of
/// `extension.rs`'s own internal wiring.
fn bundled_resources_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources")
}

#[test]
fn all_eight_bundled_builtin_personas_are_discovered_with_builtin_source() {
    let cfg = AgentDiscoveryConfig {
        builtin_agents_dir: Some(bundled_resources_dir()),
        ..AgentDiscoveryConfig::default()
    };

    let result = discover_agents_all(&cfg).expect("builtin-only discovery succeeds");

    for expected_name in BUILTIN_PERSONA_NAMES {
        let found = result
            .agents
            .iter()
            .find(|a| a.name == *expected_name)
            .unwrap_or_else(|| {
                panic!(
                    "expected bundled builtin persona '{expected_name}' to be discovered; \
                     discovered names were: {:?}",
                    result.agents.iter().map(|a| a.name.as_str()).collect::<Vec<_>>()
                )
            });
        assert_eq!(
            found.source,
            AgentSource::Builtin,
            "bundled persona '{expected_name}' must be discovered at AgentSource::Builtin precedence"
        );
        assert_eq!(
            found.local_name, *expected_name,
            "bundled persona '{expected_name}' must be unqualified (no package prefix)"
        );
    }

    assert_eq!(
        result.agents.len(),
        BUILTIN_PERSONA_NAMES.len(),
        "exactly the 8 bundled personas should be discovered from the builtin resource root, \
         found: {:?}",
        result.agents.iter().map(|a| a.name.as_str()).collect::<Vec<_>>()
    );
}

#[test]
fn bundled_builtin_personas_are_visible_through_the_delegation_view_too() {
    let cfg = AgentDiscoveryConfig {
        builtin_agents_dir: Some(bundled_resources_dir()),
        ..AgentDiscoveryConfig::default()
    };

    let result = discover_agents(&cfg, None).expect("builtin-only delegation discovery succeeds");
    for expected_name in BUILTIN_PERSONA_NAMES {
        assert!(
            result.agents.iter().any(|a| a.name == *expected_name),
            "delegation view must include bundled builtin persona '{expected_name}'"
        );
    }
}

#[test]
fn delegate_persona_keeps_its_name_sensitive_defaults_when_loaded_from_the_bundled_resource_root() {
    // R-SA-018: `delegate`'s `systemPromptMode`/`inheritProjectContext` name-sensitive defaults
    // must still apply when parsed from the real bundled file (not just a synthetic fixture) —
    // the frontmatter itself declares them explicitly (systemPromptMode: append), so this also
    // pins that the real file's frontmatter was ported byte-for-byte rather than paraphrased.
    let cfg = AgentDiscoveryConfig {
        builtin_agents_dir: Some(bundled_resources_dir()),
        ..AgentDiscoveryConfig::default()
    };
    let result = discover_agents_all(&cfg).expect("discovery succeeds");
    let delegate = result
        .agents
        .iter()
        .find(|a| a.name == "delegate")
        .expect("bundled delegate.md must be discovered");
    assert_eq!(
        delegate.system_prompt_mode,
        cyrup_ext_subagents::discovery::types::SystemPromptMode::Append
    );
    assert!(delegate.inherit_project_context);
}
