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
