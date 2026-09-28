use std::path::PathBuf;

use super::*;

#[test]
fn version_short_is_v_not_verbose() {
    // SEAM-052: `-v`/`--version` is a plain flag on `Cli`, NOT clap's `Version` action, so the
    // parse SUCCEEDS and `main` reports pi's parse diagnostics first (`main.ts:562-570`) before
    // printing the bare semver (`:573-576`). It used to exit from inside `Cli::parse_from`,
    // which made `cyrup -x --version` exit 0 where `pi -x --version` exits 1.
    assert!(parse(&["-v"]).version);
    assert!(parse(&["--version"]).version);
    assert!(!parse(&[]).version);
    // `--verbose` is a distinct boolean with no short.
    assert!(parse(&["--verbose"]).verbose);
    assert!(!parse(&["--verbose"]).version);
}

#[test]
fn provider_api_key_thinking_and_models_parse() {
    let cli = parse(&[
        "--provider",
        "openai",
        "--model",
        "openai/gpt-4o",
        "--api-key",
        "sk-test",
        "--thinking",
        "high",
        "--models",
        "claude-sonnet,gpt-4o:low",
    ]);
    assert_eq!(cli.provider.as_deref(), Some("openai"));
    assert_eq!(cli.model.as_deref(), Some("openai/gpt-4o"));
    assert_eq!(cli.api_key.as_deref(), Some("sk-test"));
    assert_eq!(cli.thinking, Some(ThinkingArg::High));
    assert_eq!(
        cli.models,
        vec!["claude-sonnet".to_string(), "gpt-4o:low".to_string()]
    );
}

#[test]
fn resource_flags_repeat_and_negate() {
    let cli = parse(&[
        "--extension",
        "a.ts",
        "-e",
        "b.ts",
        "--skill",
        "s1",
        "--theme",
        "t1",
        "--prompt-template",
        "p1",
        "--no-themes",
    ]);
    assert_eq!(
        cli.extension,
        vec![PathBuf::from("a.ts"), PathBuf::from("b.ts")]
    );
    assert_eq!(cli.skill, vec![PathBuf::from("s1")]);
    assert_eq!(cli.theme, vec![PathBuf::from("t1")]);
    assert_eq!(cli.prompt_template, vec![PathBuf::from("p1")]);
    assert!(cli.no_themes);
}

#[test]
fn list_models_optional_search_and_export() {
    assert_eq!(parse(&["--list-models"]).list_models.as_deref(), Some(""));
    assert_eq!(
        parse(&["--list-models", "sonnet"]).list_models.as_deref(),
        Some("sonnet")
    );
    assert_eq!(parse(&[]).list_models, None);
    assert_eq!(
        parse(&["--export", "s.jsonl"]).export,
        Some(PathBuf::from("s.jsonl"))
    );
}

#[test]
fn model_flag_is_parsed_regardless_of_position() {
    // A `--model` placed AFTER the bare prompt must still be parsed as the model flag.
    let after = parse(&[
        "-p",
        "Reply with pong",
        "--model",
        "together/moonshotai/Kimi-K2.6",
    ]);
    assert_eq!(
        after.model.as_deref(),
        Some("together/moonshotai/Kimi-K2.6")
    );
    assert_eq!(after.positionals, vec!["Reply with pong".to_string()]);
}

/// SEAM-119 — `--use-theme <name>` (pi `cli/args.ts:190-197` @v0.87.1) is a real flag: it reaches
/// `Cli::use_theme` through the whole `main.rs` pipeline instead of being captured as an unknown
/// extension flag that swallows its value and exits 1 with `Unknown option: --use-theme`.
#[test]
fn use_theme_is_a_known_flag_carrying_its_name() {
    let cli = parse_like_main(&["--use-theme", "light", "hello"]);
    assert_eq!(cli.use_theme.as_deref(), Some("light"));
    assert!(
        cli.extension_flags.is_empty(),
        "not captured as an extension flag: {:?}",
        cli.extension_flags
    );
    assert_eq!(cli.positionals, vec!["hello".to_string()]);
    // `name[/name]`: an auto pair is one value.
    let pair = parse_like_main(&["--use-theme", "light/dark"]);
    assert_eq!(pair.use_theme.as_deref(), Some("light/dark"));
    // pi assigns `result.useTheme`, so the last occurrence stands.
    let twice = parse_like_main(&["--use-theme", "light", "--use-theme", "dark"]);
    assert_eq!(twice.use_theme.as_deref(), Some("dark"));
    assert_eq!(parse_like_main(&[]).use_theme, None);
}

/// pi's one diagnostic for both unusable shapes — no next token, or a `-`-leading one — and in
/// the second case the next token is NOT consumed, so `--use-theme --print` still sets `--print`.
#[test]
fn use_theme_without_a_name_is_pis_error_and_keeps_the_next_flag() {
    let run = |args: &[&str]| {
        let raw = normalize_short_aliases(args.iter().map(|s| s.to_string()));
        crate::diagnostics::apply_arg_leniency(&raw)
    };
    let expected = vec![crate::Diagnostic::error(
        "--use-theme requires a theme name",
    )];

    let (clean, diags) = run(&["--use-theme"]);
    assert_eq!(diags, expected);
    assert!(clean.is_empty(), "{clean:?}");

    let (clean, diags) = run(&["--use-theme", "--print", "hi"]);
    assert_eq!(diags, expected);
    assert_eq!(clean, vec!["--print".to_string(), "hi".to_string()]);

    let (_, diags) = run(&["--use-theme="]);
    assert_eq!(diags, expected);

    let (clean, diags) = run(&["--use-theme", "nord"]);
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(clean, vec!["--use-theme".to_string(), "nord".to_string()]);
}
