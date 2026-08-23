use clap::Parser;
use cyrup_config::AppMode;

use super::*;

/// SEAM-105 end-to-end — the divergence was only ever visible through the REPEATED form, and
/// only after clap had appended (pi `result.tools = …`, args.ts:121-124).
#[test]
fn repeated_list_flags_resolve_to_the_last_occurrence() {
    // Presence before absence: the comma form keeps both, under both spellings.
    assert_eq!(
        parse_like_main(&["--tools", "read,bash"]).tools,
        vec!["read".to_string(), "bash".to_string()]
    );
    assert_eq!(
        parse_like_main(&["-t", "read,bash"]).tools,
        vec!["read".to_string(), "bash".to_string()]
    );
    // …and the repeated form keeps only the last.
    assert_eq!(
        parse_like_main(&["--tools", "read", "--tools", "bash"]).tools,
        vec!["bash".to_string()]
    );
    assert_eq!(
        parse_like_main(&["--models", "a", "--models", "b"]).models,
        vec!["b".to_string()]
    );
    assert_eq!(
        parse_like_main(&["--exclude-tools", "x", "-xt", "y"]).exclude_tools,
        vec!["y".to_string()]
    );
}

/// SEAM-107 end-to-end — `-p ---weird` must send `---weird` as the PROMPT and register no
/// extension flag (pi args.ts:140-146). Before the fix the token reached
/// [`partition_extension_flags`] and became the flag `-weird`, which the unknown-flag gate then
/// killed the run over.
#[test]
fn print_escape_hatch_makes_a_dashed_token_the_prompt() {
    let cli = parse_like_main(&["-p", "---weird"]);
    assert!(cli.print);
    assert_eq!(cli.positionals, vec!["---weird".to_string()]);
    assert!(cli.extension_flags.is_empty(), "{:?}", cli.extension_flags);
    // The marker never survives into the prompt.
    assert!(!cli.positionals[0].contains('\0'));
    // It keeps its POSITION among the messages rather than being pushed to the end.
    let cli = parse_like_main(&["-p", "---weird", "and", "more"]);
    assert_eq!(
        cli.positionals,
        vec![
            "---weird".to_string(),
            "and".to_string(),
            "more".to_string()
        ]
    );
    // Presence before absence: a genuine unknown long flag is STILL captured.
    let cli = parse_like_main(&["-p", "--weird"]);
    assert_eq!(cli.extension_flags.len(), 1);
    assert_eq!(cli.extension_flags[0].name, "weird");
}

/// SEAM-103 end-to-end — `--list-models @foo` lists the catalog (an empty search) and leaves
/// `@foo` in the file args (pi args.ts:171-177).
#[test]
fn list_models_leaves_a_following_file_arg_alone() {
    let cli = parse_like_main(&["--list-models", "@notes.md"]);
    assert_eq!(cli.list_models.as_deref(), Some(""));
    assert_eq!(cli.positionals, vec!["@notes.md".to_string()]);
    assert_eq!(
        crate::split_positionals(&cli.positionals).0,
        vec!["notes.md".to_string()]
    );
    // Presence before absence: a real pattern still filters.
    assert_eq!(
        parse_like_main(&["--list-models", "gpt"])
            .list_models
            .as_deref(),
        Some("gpt")
    );
}

#[test]
fn multi_char_short_aliases_normalize_to_longs() {
    assert!(parse(&["-nt"]).no_tools);
    assert!(parse(&["-nbt"]).no_builtin_tools);
    assert_eq!(
        parse(&["-xt", "ask"]).exclude_tools,
        vec!["ask".to_string()]
    );
    assert!(parse(&["-ne"]).no_extensions);
    assert!(parse(&["-ns"]).no_skills);
    assert!(parse(&["-np"]).no_prompt_templates);
    assert!(parse(&["-nc"]).no_context_files);
}

#[test]
fn unknown_flags_are_captured_as_extension_flags() {
    // `--plan` bare, `--mode=k=v` style with `=`, and a value form; known flags + their values
    // pass through to clap untouched.
    let (clean, flags) = partition_extension_flags(&[
        "--plan".to_string(),
        "--model".to_string(),
        "openai/gpt-4o".to_string(),
        "--reviewer=alice".to_string(),
        "--limit".to_string(),
        "5".to_string(),
        "hello".to_string(),
    ]);
    assert_eq!(
        clean,
        vec![
            "--model".to_string(),
            "openai/gpt-4o".to_string(),
            "hello".to_string()
        ]
    );
    assert_eq!(
        flags,
        vec![
            ExtensionFlag {
                name: "plan".into(),
                value: ExtFlagValue::Bool(true)
            },
            ExtensionFlag {
                name: "reviewer".into(),
                value: ExtFlagValue::Str("alice".into())
            },
            ExtensionFlag {
                name: "limit".into(),
                value: ExtFlagValue::Str("5".into())
            },
        ]
    );
    // The clean argv still parses under clap with the unknowns removed.
    let mut full = vec!["cyrup".to_string()];
    full.extend(clean);
    let cli = Cli::try_parse_from(full).expect("clean argv parses");
    assert_eq!(cli.model.as_deref(), Some("openai/gpt-4o"));
    assert_eq!(cli.positionals, vec!["hello".to_string()]);
}

#[test]
fn lenient_args_feed_clap_without_a_hard_error() {
    use crate::diagnostics::{DiagnosticLevel, apply_arg_leniency};

    // The full bin pipeline: normalize → leniency → partition → clap. A bad `--mode` and a bad
    // `--thinking` must NOT make clap exit-2; they are dropped/warned by the leniency layer.
    let pipeline = |args: &[&str]| -> (Cli, Vec<crate::diagnostics::Diagnostic>) {
        let norm = normalize_short_aliases(args.iter().map(|s| s.to_string()));
        let (lenient, diags) = apply_arg_leniency(&norm);
        let (clean, ext) = partition_extension_flags(&lenient);
        let mut full = vec!["cyrup".to_string()];
        full.extend(clean);
        let mut cli = Cli::try_parse_from(full).expect("lenient argv parses under clap");
        cli.extension_flags = ext;
        (cli, diags)
    };

    // Bad --mode: silently ignored ⇒ default text mode, no diagnostics.
    let (cli, diags) = pipeline(&["--mode", "bogus", "hi"]);
    assert_eq!(cli.mode, None);
    assert_eq!(cli.positionals, vec!["hi".to_string()]);
    assert!(diags.is_empty());

    // Bad --thinking: warns + continues, no thinking set.
    let (cli, diags) = pipeline(&["--thinking", "ultra", "go"]);
    assert_eq!(cli.thinking, None);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].level, DiagnosticLevel::Warning);

    // Unknown single-dash option: error diagnostic, the rest still parses.
    let (cli, diags) = pipeline(&["-x", "hello"]);
    assert_eq!(cli.positionals, vec!["hello".to_string()]);
    assert!(diags.iter().any(|d| d.level == DiagnosticLevel::Error));

    // A valid mode/thinking pair still parses normally.
    let (cli, diags) = pipeline(&["--mode", "json", "--thinking", "high"]);
    assert_eq!(cli.mode, Some(Mode::Json));
    assert_eq!(cli.thinking, Some(ThinkingArg::High));
    assert!(diags.is_empty());
}

#[test]
fn list_flags_trim_each_comma_split_segment_and_drop_empties() {
    // Pi `args.ts:120-129`: `--tools`/`--exclude-tools` split on ',' then trim + drop empties.
    // clap's `value_delimiter = ','` splits but never trims, so `"read, grep"` arrives as
    // `["read", " grep"]` — normalize_list_flags must trim the leading space so `grep` is kept
    // (not silently dropped by the exact tool-name match) and drop the empty middle segment.
    let mut cli = parse(&[
        "--tools",
        "read, grep ,, find",
        "--exclude-tools",
        " bash , ",
    ]);
    cli.normalize_list_flags();
    assert_eq!(
        cli.tools,
        vec!["read".to_string(), "grep".to_string(), "find".to_string()],
        "each tool trimmed; empty segments dropped"
    );
    assert_eq!(
        cli.exclude_tools,
        vec!["bash".to_string()],
        "exclude-tools trimmed; trailing empty dropped"
    );
    // The trimmed lists must reach the SessionConfig the session consumes (the exact seam the bin
    // threads via to_session_config): `grep` is enabled, not the silently-dropped `" grep"`.
    let config = cli.to_session_config(&dirs(), AppMode::Print);
    assert_eq!(
        config.tools,
        Some(vec![
            "read".to_string(),
            "grep".to_string(),
            "find".to_string()
        ])
    );
    assert_eq!(config.exclude_tools, vec!["bash".to_string()]);

    // `--models` (`args.ts:115`): trim only, empties KEPT (Pi does not `.filter`).
    let mut m = parse(&["--models", " claude-sonnet , gpt-4o:low "]);
    m.normalize_list_flags();
    assert_eq!(
        m.models,
        vec!["claude-sonnet".to_string(), "gpt-4o:low".to_string()]
    );
    let mut empty = parse(&["--models", ""]);
    empty.normalize_list_flags();
    assert_eq!(
        empty.models,
        vec![String::new()],
        "an empty --models value stays a single empty pattern, matching Pi's unfiltered split"
    );
}
