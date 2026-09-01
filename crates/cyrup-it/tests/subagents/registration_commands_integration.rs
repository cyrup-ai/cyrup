//! Integration test: closing gap R-SA-130 for the 5 registration-surface slash commands —
//! `/subagents-models`, `/subagents-refresh-provider-models`, `/subagents-generate-profiles`,
//! `/subagents-check-profile` — proving each now routes through REAL
//! execution (the built-in model registry in `cyrup_provider::catalog`, and `registration::profiles`'
//! real on-disk profile read/write primitives) rather than the "recognized, not yet executing"
//! stub arm `extension.rs::dispatch_slash` used to fall into.
//!
//! `/subagents-refresh-provider-models` and `/subagents-generate-profiles` (and, transitively,
//! `/subagents-check-profile`'s report) DO spawn a real child OS subprocess per candidate model —
//! `extension.rs::probe_model` (pi `probeModel`, profiles.ts:318-335), always invoked (this port
//! never exposes a `--no-probe` flag, matching every real pi call site). Gated on the
//! `test-fixtures` Cargo feature (matching every other fixture-dependent integration test in this
//! crate) so those probes resolve to the deterministic, network-free `cyrup-subagent-fixture`
//! test-double via `CYRUP_SUBAGENT_BINARY` (R-SA-045 tier 1) instead of self-reexecing this very
//! test binary (which — lacking any CLI-arg parsing of its own — degrades every probe to a
//! spurious `error`, exactly pi's own test suite's reason for stubbing `pi.exec` with a canned
//! `{code: 0, stdout: "OK"}` result rather than letting `probeModel` hit a real network call).
//!
//! Lives in `tests/` (a separate compilation unit from this crate's own `lib.rs`) for the same
//! reason every other fixture-based integration test in this crate does: these tests need to
//! mutate `CYRUP_HOME`/`CYRUP_SUBAGENT_BINARY` (process-global state) via
//! (historically) `std::env::set_var`/`remove_var`, which Rust 2024 requires `unsafe` for — a `#[cfg(test)]`
//! module inside `src/` cannot do this because this crate's own `#![forbid(unsafe_code)]`
//! (`src/lib.rs`) applies even to its own test code (a HARD forbid, unlike `deny`, cannot be
//! locally `#[allow(...)]`-ed away).
//!
//! No mocking: every dispatch below drives the REAL `SubagentsExtension::execute_command` (the
//! `cyrup_ext::native::NativeExtension` trait method), which reads the REAL built-in model
//! registry (`cyrup_provider::catalog::builtin_catalog()` — every registered provider's embedded
//! catalog, PROV-007) and writes/reads REAL files under a temporary `CYRUP_HOME`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]



use cyrup_ext::native::{ExtMode, HostCtx, NativeExtension};
use cyrup_ext_subagents::paths::Roots;
use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::registration::profiles::{load_profile, NamedProfile};
use cyrup_ext_subagents::registration::SubagentExtensionConfig;
use cyrup_ext_subagents::spawn::SpawnCommand;



/// RAII guard installing `CYRUP_HOME` at a temp dir for the life of one test.
/// The config one test runs under: its own `CYRUP_HOME` root, and
/// `resolve_spawn_command`'s tier-1 override pointed at the scripted-NDJSON
/// `cyrup-subagent-fixture` test-double (arch-SA §11), so every `probe_model` call this test's
/// `execute_command` dispatch triggers spawns that deterministic, network-free real child.
///
/// With NO `--fixture-script` argv the fixture degrades to its documented no-script default (emit
/// nothing, exit 0 immediately), which `probe_model_with` classifies as `ProbeStatus::Ok` ("Probe
/// succeeded.") — exactly like pi's own suite's canned `{code: 0, stdout: "OK"}` stub result
/// (`profiles.test.ts`).
///
/// Both were RAII env guards; neither moves process-global state now, so no lock is needed.
fn sandboxed_config(home: &std::path::Path) -> SubagentExtensionConfig {
    SubagentExtensionConfig {
        roots: Roots::sandboxed(home),
        spawn_command: Some(SpawnCommand {
            binary: crate::support::bins::subagent_fixture(),
            base_args: Vec::new(),
        }),
        ..SubagentExtensionConfig::default()
    }
}

fn command_ctx(cwd: &std::path::Path) -> HostCtx {
    HostCtx::command(ExtMode::Tui, false, cwd.to_path_buf())
}

/// A registered built-in provider that the retired 2-model `seed_catalog()` stub could NOT serve
/// (it held only `anthropic/claude-sonnet-4-5` and `openai/gpt-4o`), chosen small so the REAL
/// per-model probe subprocess fan-out stays fast (PROV-007).
const REGISTRY_ONLY_PROVIDER: &str = "deepseek";

/// How many models the REAL built-in registry lists for `provider`.
fn registry_model_count(provider: &str) -> usize {
    cyrup_provider::catalog::builtin_catalog()
        .iter()
        .filter(|m| m.provider.as_str() == provider)
        .count()
}

// =====================================================================================================
// /subagents-models
// =====================================================================================================

#[tokio::test]
async fn subagents_models_command_reports_the_runtime_builtin_model_mapping() {
    // pi `/subagents-models` (slash-commands.ts:802-823 -> `handleModels`) reports the RUNTIME
    // builtin-agent -> model mapping, NOT a dump of the static provider catalog. This asserts the
    // mapping's header/shape and that it no longer dumps the catalog.
    let work_dir = tempfile::tempdir().expect("real tempdir");
    let ext = SubagentsExtension::with_config_and_cwd(
        sandboxed_config(work_dir.path()),
        work_dir.path().to_path_buf(),
    );
    let ctx = command_ctx(work_dir.path());

    let output = ext
        .execute_command("subagents-models", "", &ctx)
        .await
        .expect("execute_command does not error")
        .expect("subagents-models produces textual output");

    assert!(
        !output.contains("recognized by the subagents extension"),
        "the stub placeholder text must be gone: {output}"
    );
    assert!(
        output.starts_with("Builtin subagent models\n"),
        "the report must be the runtime builtin->model mapping, not a catalog dump: {output}"
    );
    assert!(
        output.contains("Current session model:"),
        "the runtime mapping reports the current session model line: {output}"
    );
    assert!(
        !output.contains("reasoning="),
        "the report must NOT dump the static provider catalog's per-model context/reasoning rows: {output}"
    );
}

#[tokio::test]
async fn subagents_models_command_rejects_more_than_one_positional_token() {
    let work_dir = tempfile::tempdir().expect("real tempdir");
    let ext = SubagentsExtension::with_config_and_cwd(
        sandboxed_config(work_dir.path()),
        work_dir.path().to_path_buf(),
    );
    let ctx = command_ctx(work_dir.path());

    let output = ext
        .execute_command("subagents-models", "one two", &ctx)
        .await
        .expect("execute_command wraps SubagentError as rendered text, never ExtError")
        .expect("a rendered error message");
    assert!(
        output.contains("subagent command failed") || output.contains("Usage:"),
        "a malformed call must surface a real error, not a silent success: {output}"
    );
}

// =====================================================================================================
// /subagents-refresh-provider-models
// =====================================================================================================

#[tokio::test]
async fn subagents_refresh_provider_models_writes_a_real_catalog_cache_file() {
    let home_dir = tempfile::tempdir().expect("home tempdir");
    let home = home_dir.path().to_path_buf();

    let work_dir = tempfile::tempdir().expect("real tempdir");
    let ext = SubagentsExtension::with_config_and_cwd(
        sandboxed_config(&home),
        work_dir.path().to_path_buf(),
    );
    let ctx = command_ctx(work_dir.path());

    // PROV-007: a provider that is NEITHER anthropic NOR openai — i.e. one the retired 2-model
    // seed stub could not answer for at all (it hard-errored "No models found in the current
    // registry for provider '<p>'"). Every registered provider must now be servable.
    let provider = REGISTRY_ONLY_PROVIDER.to_string();
    let registry_count = registry_model_count(&provider);
    assert!(
        registry_count >= 2,
        "the built-in registry must carry {provider}'s models, got {registry_count}"
    );

    let output = ext
        .execute_command("subagents-refresh-provider-models", &provider, &ctx)
        .await
        .expect("execute_command does not error")
        .expect("refresh produces textual output");

    assert!(!output.contains("recognized by the subagents extension"), "got: {output}");
    assert!(output.contains("refreshed catalog cache"), "got: {output}");

    let cache_path = home.join(".cyrup").join("subagents").join("provider-catalog-cache.json");
    let contents = tokio::fs::read_to_string(&cache_path)
        .await
        .expect("the refresh command must genuinely write the cache file");
    let parsed: serde_json::Value = serde_json::from_str(&contents).expect("valid json");
    assert_eq!(parsed["provider"], serde_json::json!(provider));
    // Every model the registry lists for that provider was probed and written — pi's
    // `availableModels` for the provider, not a hand-seeded subset (profiles.ts:529).
    assert_eq!(
        parsed["modelCount"].as_u64().unwrap_or(0),
        registry_count as u64,
        "got: {parsed}"
    );
}

#[tokio::test]
async fn subagents_refresh_provider_models_rejects_unsafe_provider_names() {
    let work_dir = tempfile::tempdir().expect("real tempdir");
    let ext = SubagentsExtension::with_config_and_cwd(
        sandboxed_config(work_dir.path()),
        work_dir.path().to_path_buf(),
    );
    let ctx = command_ctx(work_dir.path());

    let output = ext
        .execute_command("subagents-refresh-provider-models", "../../etc/passwd", &ctx)
        .await
        .expect("execute_command does not error")
        .expect("a rendered error message");
    assert!(
        output.contains("subagent command failed"),
        "a traversal-shaped provider name must be rejected, not silently accepted: {output}"
    );
}

// =====================================================================================================
// /subagents-generate-profiles
// =====================================================================================================

#[tokio::test]
async fn subagents_generate_profiles_writes_two_real_loadable_profiles() {
    let home_dir = tempfile::tempdir().expect("home tempdir");
    let home = home_dir.path().to_path_buf();

    let work_dir = tempfile::tempdir().expect("real tempdir");
    let ext = SubagentsExtension::with_config_and_cwd(
        sandboxed_config(&home),
        work_dir.path().to_path_buf(),
    );
    let ctx = command_ctx(work_dir.path());

    // PROV-007: same non-seed provider — profile generation must work for the whole registry.
    let provider = REGISTRY_ONLY_PROVIDER.to_string();

    let output = ext
        .execute_command("subagents-generate-profiles", &provider, &ctx)
        .await
        .expect("execute_command does not error")
        .expect("generation produces textual output");

    assert!(output.contains(&format!("{provider}.quota")), "got: {output}");
    assert!(output.contains(&format!("{provider}.quality")), "got: {output}");

    let profiles_dir = home.join(".cyrup").join("subagents").join("profiles");
    let quota: NamedProfile = load_profile(&profiles_dir, &format!("{provider}.quota"))
        .expect("the quota profile must be genuinely loadable from disk");
    let quality: NamedProfile = load_profile(&profiles_dir, &format!("{provider}.quality"))
        .expect("the quality profile must be genuinely loadable from disk");
    assert!(quota.subagents.default_model.is_some(), "got: {quota:?}");
    assert!(quality.subagents.default_model.is_some(), "got: {quality:?}");
}

// =====================================================================================================
// /subagents-check-profile
// =====================================================================================================

#[tokio::test]
async fn subagents_check_profile_cross_references_the_real_model_registry() {
    let home_dir = tempfile::tempdir().expect("home tempdir");
    let home = home_dir.path().to_path_buf();

    let profiles_dir = home.join(".cyrup").join("subagents").join("profiles");
    tokio::fs::create_dir_all(&profiles_dir).await.expect("mkdir profiles dir");
    // PROV-007: a model id from a provider the retired seed stub never carried, so this really
    // exercises the whole-registry cross-reference (pi `findModelInfo` over
    // `ctx.modelRegistry.getAvailable()`).
    let known_model = cyrup_provider::catalog::builtin_catalog()
        .iter()
        .find(|m| m.provider.as_str() == REGISTRY_ONLY_PROVIDER)
        .expect("the registry must carry the probe provider")
        .id
        .as_str()
        .to_string();

    // pi `checkSubagentProfile`'s `entries` (profiles.ts:639-641) walks ONLY
    // `subagents.agentOverrides`, never `defaultModel` (this crate's own `render_profile_check_
    // report` doc comment says the same) — so a profile needs a real `overrides.<agent>.model`
    // entry to have anything to check at all; a `defaultModel`-only profile always renders "no
    // model references declared" (pi-faithful, not a bug).
    let mut overrides = std::collections::BTreeMap::new();
    overrides.insert(
        "researcher".to_string(),
        cyrup_ext_subagents::discovery::types::AgentOverrideConfig {
            model: cyrup_ext_subagents::discovery::types::OverrideField::Value(known_model.clone()),
            ..Default::default()
        },
    );
    let profile = NamedProfile {
        subagents: cyrup_ext_subagents::discovery::types::SubagentSettings {
            overrides,
            default_model: None,
            // G101 added these two; a profile declares neither.
            default_thinking: None,
            default_extensions: None,
            disable_builtins: None,
            disable_thinking: None,
            // SUBA-003 added this field; a profile declares no model-scope policy of its own.
            model_scope: None,
            // SUBA-078 added this field; a profile declares no reasoning ceiling of its own.
            max_thinking: None,
        },
    };
    tokio::fs::write(profiles_dir.join("mixed.json"), serde_json::to_vec_pretty(&profile).expect("serialize"))
        .await
        .expect("write profile");

    let mut bogus_overrides = std::collections::BTreeMap::new();
    bogus_overrides.insert(
        "researcher".to_string(),
        cyrup_ext_subagents::discovery::types::AgentOverrideConfig {
            model: cyrup_ext_subagents::discovery::types::OverrideField::Value(
                "definitely-not-a-real-model-id".to_string(),
            ),
            ..Default::default()
        },
    );
    let bogus_profile = NamedProfile {
        subagents: cyrup_ext_subagents::discovery::types::SubagentSettings {
            overrides: bogus_overrides,
            default_model: None,
            // G101 added these two; a profile declares neither.
            default_thinking: None,
            default_extensions: None,
            disable_builtins: None,
            disable_thinking: None,
            // SUBA-003 added this field; a profile declares no model-scope policy of its own.
            model_scope: None,
            // SUBA-078 added this field; a profile declares no reasoning ceiling of its own.
            max_thinking: None,
        },
    };
    tokio::fs::write(
        profiles_dir.join("bogus.json"),
        serde_json::to_vec_pretty(&bogus_profile).expect("serialize"),
    )
    .await
    .expect("write profile");

    let work_dir = tempfile::tempdir().expect("real tempdir");
    let ext = SubagentsExtension::with_config_and_cwd(
        sandboxed_config(&home),
        work_dir.path().to_path_buf(),
    );
    let ctx = command_ctx(work_dir.path());

    let ok_output = ext
        .execute_command("subagents-check-profile", "mixed", &ctx)
        .await
        .expect("execute_command does not error")
        .expect("check produces textual output");
    // pi's real rendering (slash-commands.ts:984 @v0.43.0): "<agent> -> <model> — registry
    // ok|missing; probe <status>(...)" — never the literal "OK"/"UNKNOWN" this test used to
    // assert on (pi's own real output never emits those uppercase tokens anywhere).
    assert!(ok_output.contains(&known_model), "got: {ok_output}");
    assert!(
        ok_output.contains("registry ok"),
        "a model resolvable against the model registry must report `inRegistry: true`: {ok_output}"
    );

    let bogus_output = ext
        .execute_command("subagents-check-profile", "bogus", &ctx)
        .await
        .expect("execute_command does not error")
        .expect("check produces textual output even for an unresolvable model reference");
    assert!(
        bogus_output.contains("registry missing"),
        "an unresolvable model reference must report `inRegistry: false`, never a fabricated match: {bogus_output}"
    );
}

#[tokio::test]
async fn subagents_check_profile_errors_for_a_nonexistent_profile() {
    let home_dir = tempfile::tempdir().expect("home tempdir");

    let work_dir = tempfile::tempdir().expect("real tempdir");
    let ext = SubagentsExtension::with_config_and_cwd(
        sandboxed_config(home_dir.path()),
        work_dir.path().to_path_buf(),
    );
    let ctx = command_ctx(work_dir.path());

    let output = ext
        .execute_command("subagents-check-profile", "does-not-exist", &ctx)
        .await
        .expect("execute_command does not error")
        .expect("a rendered error message");
    assert!(
        output.contains("subagent command failed"),
        "a nonexistent profile must surface as a real error, never a fabricated OK: {output}"
    );
}

// =====================================================================================================
// /subagents-companions — REMOVED upstream
// =====================================================================================================

/// `/subagents-companions` no longer exists. Upstream deleted `src/extension/companion-suggestions.ts`
/// (359 lines) plus its `subagents-companions` `pi.registerCommand` block, its
/// `companionSuggestions` `ExtensionConfig` key and its `CompanionSuggestion*` types wholesale in
/// `3ac0ef5` ("Make supervisor coordination native", 2026-07-03) — three days BEFORE v0.34.0, the
/// tag this crate ported from, and it is absent from every tag through v0.43.0. Nothing replaced the
/// command: the same commit added `intercom/native-supervisor-channel.ts` and
/// `slash/prompt-workflows.ts`, neither of which registers a companions surface.
///
/// The user action: typing `/subagents-companions` (with any argument shape the old parser accepted)
/// must now be handled the way every other unknown command is — `SlashCommandName::from_str_exact`
/// returns `None`, so `execute_command` returns an `ExtError`, exactly as it does for a name this
/// extension never registered. Before the removal all four of these returned real, rendered output.
#[tokio::test]
async fn subagents_companions_is_no_longer_a_registered_command() {
    let work_dir = tempfile::tempdir().expect("real tempdir");
    let ext = SubagentsExtension::with_config_and_cwd(
        sandboxed_config(work_dir.path()),
        work_dir.path().to_path_buf(),
    );
    let ctx = command_ctx(work_dir.path());

    for args in ["", "status", "hide pi-intercom workspace", "show pi-intercom"] {
        let err = ext
            .execute_command("subagents-companions", args, &ctx)
            .await
            .expect_err("the removed command must not resolve to a handler");
        let rendered = err.to_string();
        assert!(
            rendered.contains("no handler for command `subagents-companions`"),
            "`/subagents-companions {args}` must be an unknown command, not a rendered report: {rendered}"
        );
    }

    // And it is gone from the registered descriptor table the command palette is built from, so it
    // cannot be offered for completion either.
    assert!(
        !cyrup_ext_subagents::registration::slash_commands::SLASH_COMMANDS
            .iter()
            .any(|d| d.name.as_str() == "subagents-companions"),
        "the descriptor table must not advertise a command with no dispatch arm"
    );
}

/// The other half of the removal: the `companionSuggestions` CONFIG KEY is gone from
/// `SubagentExtensionConfig` too. A removal that deletes the command but keeps parsing and
/// re-emitting the key is half-done — a user's `config.json` would keep round-tripping a setting
/// nothing reads, and the next `hide`-equivalent write would resurrect it.
///
/// Upstream deleted the field from `ExtensionConfig` and the three `CompanionSuggestion*` types from
/// `src/shared/types.ts` in the SAME commit (`3ac0ef5`, `shared/types.ts:1743-1789`).
///
/// The user action: a `~/.cyrup/subagents/config.json` that still carries the legacy block must load
/// without error (an old config must not brick a session) and must NOT be re-serialized with it.
#[test]
fn the_companion_suggestions_config_key_is_neither_parsed_nor_re_emitted() {
    let legacy = r#"{
      "asyncByDefault": true,
      "companionSuggestions": {
        "enabled": true,
        "packages": { "pi-intercom": { "dismissed": { "user": true } } }
      }
    }"#;
    let parsed: SubagentExtensionConfig =
        serde_json::from_str(legacy).expect("a legacy config must still load, not hard-fail");
    // The rest of the config is untouched by the removal.
    assert!(parsed.async_by_default);

    let re_emitted = serde_json::to_string(&parsed).expect("serializes");
    assert!(
        !re_emitted.contains("companionSuggestions"),
        "the removed key must not survive a load/save round trip: {re_emitted}"
    );
    let fresh = serde_json::to_string(&SubagentExtensionConfig::default()).expect("serializes");
    assert!(
        !fresh.contains("companionSuggestions"),
        "a default config must not advertise the removed key: {fresh}"
    );
}
