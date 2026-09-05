use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use cyrup_config::AppMode;
use cyrup_sdk::core::ModelThinkingLevel;
use cyrup_session_svc::{ExtensionFlagValue as SvcExtensionFlagValue, NoTools, SessionTarget};

use crate::diagnostics::DiagnosticLevel;

use super::*;

#[test]
fn trust_override_maps_approve_flags() {
    assert_eq!(parse(&["--approve"]).trust_override(), Some(true));
    assert_eq!(parse(&["--no-approve"]).trust_override(), Some(false));
    assert_eq!(parse(&["-na"]).trust_override(), Some(false));
    assert_eq!(parse(&[]).trust_override(), None);
    assert_eq!(
        parse(&["--approve", "--no-approve"]).trust_override(),
        Some(true)
    );
}

#[test]
fn tool_flags_map_to_no_tools_modes_and_lists() {
    assert_eq!(parse(&["--no-tools"]).no_tools_mode(), Some(NoTools::All));
    assert_eq!(
        parse(&["--no-builtin-tools"]).no_tools_mode(),
        Some(NoTools::Builtin)
    );
    // --no-tools wins when both are present.
    assert_eq!(
        parse(&["--no-tools", "--no-builtin-tools"]).no_tools_mode(),
        Some(NoTools::All)
    );
    let cli = parse(&["--tools", "read,grep,find", "--exclude-tools", "bash"]);
    assert_eq!(
        cli.tools,
        vec!["read".to_string(), "grep".to_string(), "find".to_string()]
    );
    assert_eq!(cli.exclude_tools, vec!["bash".to_string()]);
}

#[test]
fn name_is_trimmed_and_empty_is_rejected() {
    assert_eq!(
        parse(&["--name", "  hi  "])
            .validated_name()
            .unwrap()
            .as_deref(),
        Some("hi")
    );
    assert!(parse(&["--name", "   "]).validated_name().is_err());
    assert_eq!(parse(&[]).validated_name().unwrap(), None);
}

#[test]
fn relative_resource_paths_resolve_to_absolute_keeping_specs() {
    let cwd = std::path::Path::new("/work");
    let out = resolve_cli_paths(
        cwd,
        &[
            PathBuf::from("rel/x.ts"),
            PathBuf::from("/abs/y.ts"),
            PathBuf::from("npm:@a/b"),
        ],
    );
    assert_eq!(out[0], PathBuf::from("/work/rel/x.ts"));
    assert_eq!(out[1], PathBuf::from("/abs/y.ts"));
    assert_eq!(out[2], PathBuf::from("npm:@a/b"));
}

#[test]
fn config_mapping_carries_flags_and_persistence() {
    let d = dirs();
    let cli = parse(&[
        "--model",
        "faux/faux-1",
        "--system-prompt",
        "be terse",
        "--append-system-prompt",
        "cite sources",
        "--append-system-prompt",
        "stay calm",
        "--thinking",
        "low",
        "--no-tools",
        "--exclude-tools",
        "bash",
        "--no-context-files",
        "--no-skills",
        "--no-approve",
        "hello",
    ]);
    let config = cli.to_session_config(&d, AppMode::Print);
    assert_eq!(config.cwd, PathBuf::from("/work"));
    assert_eq!(config.model_pattern.as_deref(), Some("faux/faux-1"));
    assert_eq!(config.system_prompt.as_deref(), Some("be terse"));
    // CFG-S01: Pi joins the `--append-system-prompt` entries with a BLANK LINE
    // (`loaderAppendSystemPrompt.join("\n\n")`, agent-session.ts:1039-1040); cyrup used `\n`.
    assert_eq!(
        config.append_system_prompt.as_deref(),
        Some("cite sources\n\nstay calm")
    );
    assert_eq!(config.thinking_level, Some(ModelThinkingLevel::Low));
    assert_eq!(config.no_tools, Some(NoTools::All));
    assert_eq!(config.exclude_tools, vec!["bash".to_string()]);
    assert!(config.no_context_files);
    assert!(config.no_skills);
    assert_eq!(config.trust_override, Some(false));
    assert!(!config.persist);
    assert!(matches!(config.target, SessionTarget::New));

    // Interactive persists; resume persists even in PRINT; --no-session forces ephemeral.
    assert!(cli.to_session_config(&d, AppMode::Interactive).persist);
    let resume = parse(&["--continue"]).to_session_config(&d, AppMode::Print);
    assert!(resume.persist);
    let ephemeral = parse(&["--no-session"]).to_session_config(&d, AppMode::Interactive);
    assert!(!ephemeral.persist);
}

/// ACP-213 — `AppMode::Acp` must persist sessions, and `--no-session` must still win.
///
/// The second half is the foot-gun half: `config.persist` used to be computed by the same
/// expression written out twice, in `Cli::to_session_config` and again in
/// `crate::prelaunch::resolve_session`, so this asserts the shared rule
/// (`crate::cli::persists`) directly for all six mode/flag combinations rather than only the
/// path `to_session_config` happens to take.
#[test]
fn acp_persists_sessions_unless_no_session() {
    let d = dirs();
    assert!(
        parse(&[]).to_session_config(&d, AppMode::Acp).persist,
        "an ACP session with no JSONL is invisible to session/list and unloadable"
    );
    assert!(
        !parse(&["--no-session"])
            .to_session_config(&d, AppMode::Acp)
            .persist
    );
    // The one rule, both call sites. `explicit_session` is the `--session`/`--fork`/`--continue`
    // leg `prelaunch::resolve_session` recomputes after the target settles.
    for (no_session, explicit, mode, expected) in [
        (false, false, AppMode::Acp, true),
        (true, false, AppMode::Acp, false),
        (false, true, AppMode::Acp, true),
        (false, false, AppMode::Interactive, true),
        (false, false, AppMode::Print, false),
        (false, true, AppMode::Print, true),
        (false, false, AppMode::Rpc, false),
        (false, false, AppMode::Json, false),
    ] {
        assert_eq!(
            crate::cli::persists(no_session, explicit, mode),
            expected,
            "persists({no_session}, {explicit}, {mode:?})"
        );
    }
}

/// PROV-002 (pi `test/max-thinking.test.ts`, "is accepted by CLI"): `--thinking max` must
/// parse, and a `model:max` suffix must split off the model id. Before the fix clap rejected
/// `max` with a usage error and `split_model_level` left `:max` glued to the id.
#[test]
fn thinking_max_is_accepted_by_the_cli() {
    assert_eq!(ThinkingArg::Max.to_level(), ModelThinkingLevel::Max);
    assert_eq!(
        ThinkingArg::from_str("max", true).expect("clap accepts `max`"),
        ThinkingArg::Max
    );

    let d = dirs();
    let cli = parse(&["--thinking", "max"]);
    let config = cli.to_session_config(&d, AppMode::Print);
    assert_eq!(config.thinking_level, Some(ModelThinkingLevel::Max));

    // `model:max` splits (Pi `resolveModelScope`).
    let (base, level) = split_model_level("anthropic/claude-opus-4-6:max");
    assert_eq!(base, "anthropic/claude-opus-4-6");
    assert_eq!(level, Some(ThinkingArg::Max));
    // A non-level suffix is still left alone.
    let (base, level) = split_model_level("anthropic/claude-opus-4-6");
    assert_eq!(base, "anthropic/claude-opus-4-6");
    assert_eq!(level, None);
}

#[test]
fn to_session_config_threads_real_home_not_agent_dir() {
    // G1: the real `$HOME` (Pi `getHomeDir()`, package-manager.ts:217) must flow onto
    // `SessionConfig.home`, distinct from the agent dir, so the resources ancestor-walk dedup
    // (`~/.agents/skills`) and the trust-requiring-resource walk resolve against the real home.
    let d = dirs();
    let config = parse(&[]).to_session_config(&d, AppMode::Print);
    assert_eq!(config.home, d.home);
    assert_eq!(config.home, PathBuf::from("/home/user"));
    // The gap was `home` silently equalling the agent dir (the `SessionConfig::new` default).
    assert_ne!(config.home, config.agent_dir);
}

#[test]
fn extension_flags_thread_into_config() {
    let d = dirs();
    // Explicit `-e` paths resolve to absolute vs cwd; `-ne` sets the discovery-disable flag.
    let cli = parse(&["--extension", "ext-a.ts", "-e", "/abs/ext-b", "-ne"]);
    let config = cli.to_session_config(&d, AppMode::Print);
    assert!(config.no_extensions);
    assert_eq!(
        config.extra_extension_paths,
        vec![PathBuf::from("/work/ext-a.ts"), PathBuf::from("/abs/ext-b")]
    );
    // Default: no discovery-disable, no explicit paths.
    let bare = parse(&[]).to_session_config(&d, AppMode::Print);
    assert!(!bare.no_extensions);
    assert!(bare.extra_extension_paths.is_empty());
}

#[test]
fn captured_extension_flag_values_thread_into_config() {
    // Pi `extensionFlagValues: parsed.unknownFlags` (main.ts:634): the unknown `--flag[=val]`
    // tokens partitioned out before clap must reach `SessionConfig` (and thence the services), so
    // a loaded extension can read them. The bin sets `extension_flags` after clap; verify the
    // mapping from the bin's `ExtFlagValue` to the svc `ExtensionFlagValue` is faithful.
    let d = dirs();
    let (clean, flags) = partition_extension_flags(&[
        "--plan".to_string(),
        "--reviewer=alice".to_string(),
        "hi".to_string(),
    ]);
    let mut full = vec!["cyrup".to_string()];
    full.extend(clean);
    let mut cli = Cli::try_parse_from(full).expect("clap parse of the cleaned argv");
    cli.extension_flags = flags;
    let config = cli.to_session_config(&d, AppMode::Print);
    assert_eq!(
        config.extension_flag_values,
        vec![
            ("plan".to_string(), SvcExtensionFlagValue::Bool(true)),
            (
                "reviewer".to_string(),
                SvcExtensionFlagValue::Str("alice".to_string())
            ),
        ]
    );
    // No unknown flags ⇒ an empty threaded set (the live path carries nothing extra).
    assert!(
        parse(&["hi"])
            .to_session_config(&d, AppMode::Print)
            .extension_flag_values
            .is_empty()
    );
}

#[test]
fn resource_flags_thread_into_config() {
    let d = dirs();
    let cli = parse(&[
        "--skill",
        "s1",
        "--skill",
        "s2",
        "--theme",
        "t1",
        "--prompt-template",
        "p1",
        "--no-themes",
        "--no-prompt-templates",
    ]);
    let config = cli.to_session_config(&d, AppMode::Print);
    // Relative resource paths are resolved to absolute vs the cwd (`/work`) before threading.
    assert_eq!(
        config.extra_skill_paths,
        vec![PathBuf::from("/work/s1"), PathBuf::from("/work/s2")]
    );
    assert_eq!(config.extra_theme_paths, vec![PathBuf::from("/work/t1")]);
    assert_eq!(config.extra_prompt_paths, vec![PathBuf::from("/work/p1")]);
    assert!(config.no_themes);
    assert!(config.no_prompt_templates);
}

// ===================================================== CFG-S01: --system-prompt is path-or-text

/// THE bug: `--system-prompt <path>` used the PATH as the prompt text. Pi
/// (`resolvePromptInput`, resource-loader.ts:53-68 → applied at :526) reads the file when the
/// token names something that exists.
#[test]
fn system_prompt_reads_the_file_when_the_token_is_an_existing_path() {
    let tmp = tempfile::TempDir::new().unwrap();
    let file = tmp.path().join("persona.md");
    std::fs::write(&file, "You are a rigorous reviewer.\n").unwrap();

    // Absolute form.
    let cli = parse(&["--system-prompt", file.to_str().unwrap()]);
    let cfg = cli.to_session_config(&dirs_at(tmp.path()), AppMode::Print);
    assert_eq!(
        cfg.system_prompt.as_deref(),
        Some("You are a rigorous reviewer.\n")
    );

    // Relative form, resolved against the cwd (Pi's bare `existsSync(input)`).
    let cli = parse(&["--system-prompt", "persona.md"]);
    let cfg = cli.to_session_config(&dirs_at(tmp.path()), AppMode::Print);
    assert_eq!(
        cfg.system_prompt.as_deref(),
        Some("You are a rigorous reviewer.\n")
    );
}

/// The other half of the rule, and the reason the fix cannot be "always treat it as a path":
/// a token that does not exist is LITERAL prompt text, with no diagnostic.
#[test]
fn system_prompt_keeps_literal_text_when_no_such_file_exists() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cli = parse(&["--system-prompt", "be terse and never apologise"]);
    let (cfg, diags) = cli.to_session_config_with_diagnostics(&dirs_at(tmp.path()), AppMode::Print);
    assert_eq!(
        cfg.system_prompt.as_deref(),
        Some("be terse and never apologise")
    );
    assert!(
        diags.is_empty(),
        "a non-path token is not a failure: {diags:?}"
    );
}

/// Pi resolves EVERY `--append-system-prompt` entry independently (resource-loader.ts:536-538),
/// mixing files and literals freely, then joins with a BLANK LINE (agent-session.ts:1039-1040).
/// cyrup joined with a single `\n` — a second, smaller divergence folded into the same fix.
#[test]
fn each_append_system_prompt_entry_resolves_independently_and_joins_with_a_blank_line() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(tmp.path().join("rules.md"), "Always cite sources.").unwrap();

    let cli = parse(&[
        "--append-system-prompt",
        "rules.md",
        "--append-system-prompt",
        "stay calm",
    ]);
    let cfg = cli.to_session_config(&dirs_at(tmp.path()), AppMode::Print);
    assert_eq!(
        cfg.append_system_prompt.as_deref(),
        Some("Always cite sources.\n\nstay calm"),
    );
}

/// An EXISTING but unreadable token (here: a directory) warns and falls back to the literal —
/// Pi's `catch` arm returns `input` rather than throwing (resource-loader.ts:60-63). This is the
/// case that must NOT be fatal.
#[test]
fn an_unreadable_prompt_file_warns_and_falls_back_to_the_literal() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("a-directory");
    std::fs::create_dir(&dir).unwrap();

    let cli = parse(&["--system-prompt", "a-directory"]);
    let (cfg, diags) = cli.to_session_config_with_diagnostics(&dirs_at(tmp.path()), AppMode::Print);

    assert_eq!(
        cfg.system_prompt.as_deref(),
        Some("a-directory"),
        "literal fallback"
    );
    assert_eq!(diags.len(), 1, "expected one warning, got {diags:?}");
    assert_eq!(diags[0].level, DiagnosticLevel::Warning, "never fatal");
    assert!(
        diags[0]
            .message
            .starts_with("Could not read system prompt file a-directory: "),
        "message: {}",
        diags[0].message
    );
}

/// An empty `--system-prompt ""` must not be probed as a path (joining `""` onto the cwd would
/// "exist" as the cwd itself and produce a bogus unreadable-file warning). Pi's
/// `if (!input) return undefined` guard.
#[test]
fn an_empty_prompt_token_is_never_probed_as_a_path() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (text, diags) = resolve_prompt_input(tmp.path(), "", "system prompt");
    assert_eq!(text, "");
    assert!(diags.is_empty(), "{diags:?}");
}

/// ACP-018 — the ACP host disables theme discovery, and **only** theme discovery.
///
/// The unit's verify has two halves, and the second is the one that catches an over-broad fix: it
/// is not enough that `no_themes` is true; extension, skill, prompt-template and context-file
/// discovery must be *untouched*, because every one of those IS observable over ACP (a skill is a
/// slash command in the client's palette, a prompt template expands server-side, an extension can
/// register both). Only the terminal's own rendering concern is dropped.
#[test]
fn the_acp_host_drops_themes_and_nothing_else() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dirs = dirs_at(tmp.path());

    let bare = parse(&[]);
    let acp = bare.to_session_config(&dirs, AppMode::Acp);
    assert!(
        acp.no_themes,
        "ACP-018: the ACP host disables theme discovery with no flag"
    );
    for (name, dropped) in [
        ("extensions", acp.no_extensions),
        ("skills", acp.no_skills),
        ("prompt templates", acp.no_prompt_templates),
        ("context files", acp.no_context_files),
    ] {
        assert!(
            !dropped,
            "ACP-018: {name} discovery must be untouched — it is observable over ACP"
        );
    }

    // Every other mode keeps following the flag, in both directions.
    for mode in [
        AppMode::Interactive,
        AppMode::Print,
        AppMode::Json,
        AppMode::Rpc,
    ] {
        assert!(
            !bare.to_session_config(&dirs, mode).no_themes,
            "{mode:?} without --no-themes must keep theme discovery"
        );
        assert!(
            parse(&["--no-themes"])
                .to_session_config(&dirs, mode)
                .no_themes,
            "{mode:?} with --no-themes must drop it"
        );
    }

    // And `--no-themes` under ACP is a no-op rather than a contradiction.
    assert!(
        parse(&["--no-themes"])
            .to_session_config(&dirs, AppMode::Acp)
            .no_themes
    );
}
