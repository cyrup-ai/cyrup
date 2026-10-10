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
        Some(vec!["read".to_string(), "bash".to_string()])
    );
    assert_eq!(
        parse_like_main(&["-t", "read,bash"]).tools,
        Some(vec!["read".to_string(), "bash".to_string()])
    );
    // …and the repeated form keeps only the last.
    assert_eq!(
        parse_like_main(&["--tools", "read", "--tools", "bash"]).tools,
        Some(vec!["bash".to_string()])
    );
    assert_eq!(
        parse_like_main(&["--models", "a", "--models", "b"]).models,
        Some(vec!["b".to_string()])
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

    // Bad --mode: SEAM-120 — an exit-1 error diagnostic (pi args.ts:104-108 @v0.87.1), and clap
    // never sees the value it would reject with its own exit-2 text.
    let (cli, diags) = pipeline(&["--mode", "bogus", "hi"]);
    assert_eq!(cli.mode, None);
    assert_eq!(cli.positionals, vec!["hi".to_string()]);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].level, DiagnosticLevel::Error);

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
        Some(vec![
            "read".to_string(),
            "grep".to_string(),
            "find".to_string()
        ]),
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

    // `--models` (`args.ts:141-145` @v1.0.1): trim AND drop empty entries (SEAM-147).
    let mut m = parse(&["--models", " claude-sonnet , , gpt-4o:low ,"]);
    m.normalize_list_flags();
    assert_eq!(
        m.models,
        Some(vec!["claude-sonnet".to_string(), "gpt-4o:low".to_string()])
    );
}

/// SEAM-147 — pi `9b3c19da5` (fixes #10334), `cli/args.ts:141-145` @v1.0.1 (unchanged
/// @f1b2e77f5): `--models` drops empty entries, and pi's own test is
/// `parseArgs(["--models", "gpt-4o, ,claude-sonnet,"])` → `["gpt-4o", "claude-sonnet"]`. The flag
/// stays SUPPLIED when every entry is empty: `--models ""` is pi's truthy `[]`, so
/// `parsed.models ?? getEnabledModels()` (`main.ts:812`) does not fall back. RED before the fix:
/// the trailing comma kept `""` and `--models ""` stayed `[""]`.
#[test]
fn models_drops_empty_entries_but_stays_supplied() {
    assert_eq!(
        parse_like_main(&["--models", "gpt-4o, ,claude-sonnet,"]).models,
        Some(vec!["gpt-4o".to_string(), "claude-sonnet".to_string()])
    );
    assert_eq!(
        parse_like_main(&["--models", "faux/*,"]).models,
        parse_like_main(&["--models", "faux/*"]).models
    );
    assert_eq!(parse_like_main(&["--models", ""]).models, Some(Vec::new()));
    assert_eq!(parse_like_main(&["--models", ","]).models, Some(Vec::new()));
    assert_eq!(parse_like_main(&[]).models, None);
}

/// The full pre-clap → clap pipeline with the diagnostics kept, for the rows below.
fn pipeline_with_diags(args: &[&str]) -> (Cli, Vec<crate::diagnostics::Diagnostic>) {
    let norm = normalize_short_aliases(args.iter().map(|s| s.to_string()));
    let (lenient, diags) = crate::diagnostics::apply_arg_leniency(&norm);
    let (clean, ext) = partition_extension_flags(&lenient);
    let mut full = vec!["cyrup".to_string()];
    full.extend(clean);
    let mut cli = Cli::try_parse_from(full).expect("lenient argv parses under clap");
    cli.extension_flags = ext;
    cli.normalize_list_flags();
    cli.restore_escaped_positionals();
    (cli, diags)
}

/// SEAM-123 — pi args.ts:82-91 @v0.87.1: `--` ends option parsing, as the FIRST arm of pi's loop.
/// RED twice over before the fix: `apply_arg_leniency`'s SEAM-104 arm errored `Unknown option: -
/// Summarize these points`, and `partition_extension_flags` captured an extension flag named `""`
/// (`strip_prefix("--")` on the bare token yields the empty string) which also swallowed the next
/// token as its value.
#[test]
fn double_dash_ends_option_parsing() {
    let (cli, diags) = pipeline_with_diags(&["-p", "--", "- Summarize these points"]);
    assert!(diags.is_empty(), "{diags:?}");
    assert!(cli.print);
    assert!(cli.extension_flags.is_empty(), "{:?}", cli.extension_flags);
    assert_eq!(
        cli.positionals,
        vec!["- Summarize these points".to_string()]
    );

    // Everything after `--` is a message/file verbatim — a real flag spelling included.
    let (cli, diags) = pipeline_with_diags(&["-p", "--", "@notes.md", "--model", "x"]);
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(cli.model, None, "`--model` after `--` is not a flag");
    assert!(cli.extension_flags.is_empty(), "{:?}", cli.extension_flags);
    assert_eq!(
        cli.positionals,
        vec![
            "@notes.md".to_string(),
            "--model".to_string(),
            "x".to_string()
        ]
    );

    // The short-alias PRE-PASS honours `--` too. `normalize_short_aliases` runs over the whole
    // argv in `main.rs` before anything else, so without its own `--` latch the tail arrived at
    // clap already rewritten: `["-p","--","-nc","-na","-nt","hi"]` yielded positionals
    // `["--no-context-files","--no-approve","--no-tools","hi"]` with NO diagnostic — a prompt
    // beginning with a dash silently became different text, which is the exact case this row
    // exists for. pi's `--` arm (args.ts:82-91 @v0.87.1) precedes and `break`s past every alias
    // arm (args.ts:141-183), so its `messages` are `["-nc","-na","-nt","hi"]`.
    let (cli, diags) = pipeline_with_diags(&["-p", "--", "-nc", "-na", "-nt", "hi"]);
    assert!(diags.is_empty(), "{diags:?}");
    assert!(cli.print);
    assert!(cli.extension_flags.is_empty(), "{:?}", cli.extension_flags);
    assert!(
        !cli.no_context_files && !cli.no_approve && !cli.no_tools,
        "a post-`--` alias spelling must not set its flag"
    );
    assert_eq!(
        cli.positionals,
        vec![
            "-nc".to_string(),
            "-na".to_string(),
            "-nt".to_string(),
            "hi".to_string()
        ]
    );
    // Presence before absence: the same aliases BEFORE `--` are still rewritten and still set
    // their flags, and only the tail stays verbatim.
    let (cli, diags) = pipeline_with_diags(&["-nc", "-nt", "-p", "--", "-na"]);
    assert!(diags.is_empty(), "{diags:?}");
    assert!(cli.no_context_files && cli.no_tools);
    assert!(
        !cli.no_approve,
        "the post-`--` `-na` is a message, not a flag"
    );
    assert_eq!(cli.positionals, vec!["-na".to_string()]);
    // And the pre-pass alone, without the rest of the pipeline.
    assert_eq!(
        normalize_short_aliases(["cyrup", "-nc", "--", "-nc"].map(String::from)),
        vec![
            "cyrup".to_string(),
            "--no-context-files".to_string(),
            "--".to_string(),
            "-nc".to_string()
        ]
    );

    // Presence before absence: `--` still advertised in the help body (args.ts:275,329,355).
    let help = crate::cli::render_help(&[]);
    assert!(help.contains("[options] [--] [@files...] [messages...]"));
    assert!(help.contains(
        "  --                             End option parsing; treat remaining arguments as \
         messages/files"
    ));
    assert!(help.contains("-p -- \"- Summarize these points\""));
}

/// SEAM-120 — pi args.ts:95-110 @v0.87.1. cyrup silently dropped an invalid `--mode` value AND
/// consumed the following token unconditionally. pi does neither: a missing or `-`-leading value is
/// an exit-1 error that consumes NOTHING, and a present-but-invalid value is a *different* exit-1
/// error.
#[test]
fn bad_mode_is_an_error_not_a_silent_drop() {
    use crate::diagnostics::DiagnosticLevel;

    // (1) present but invalid → `Invalid mode "…"`, flag and value both withheld from clap.
    let (cli, diags) = pipeline_with_diags(&["--mode", "bogus", "hi"]);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].level, DiagnosticLevel::Error);
    assert_eq!(
        diags[0].message,
        "Invalid mode \"bogus\". Valid values: text, json, rpc"
    );
    assert_eq!(cli.mode, None);
    assert_eq!(cli.positionals, vec!["hi".to_string()]);

    // (2) next token is a flag → `--mode requires …`, and the flag is NOT consumed (pi does not
    // `i++` on this branch), so `--model m` still reaches clap.
    let (cli, diags) = pipeline_with_diags(&["--mode", "--model", "m", "hi"]);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].level, DiagnosticLevel::Error);
    assert_eq!(diags[0].message, "--mode requires text, json, or rpc");
    assert_eq!(cli.model.as_deref(), Some("m"));
    assert_eq!(cli.positionals, vec!["hi".to_string()]);

    // (3) trailing `--mode` with no value at all → the same error, never a clap exit-2.
    let (_cli, diags) = pipeline_with_diags(&["--mode"]);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].message, "--mode requires text, json, or rpc");

    // (4) CYRUP-DELTA: `acp` is cyrup's fifth mode and still parses with no diagnostic.
    let (cli, diags) = pipeline_with_diags(&["--mode", "acp", "hi"]);
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(cli.mode, Some(Mode::Acp));
}

/// `cyrup intercom …` reaches `cyrup_intercom::cli` with its argv verbatim: a `--text`/`--to`/
/// `--name` VALUE spelled like a pi multi-letter alias (`-nc`, `-na`) is message text, not a flag,
/// so the process pre-pass must not rewrite it. Every other argv is still normalized.
#[test]
fn intercom_subcommand_argv_skips_short_alias_normalization() {
    let intercom = [
        "cyrup", "intercom", "send", "--to", "-na", "--text", "-nc", "--name", "-nt",
    ]
    .map(String::from);
    assert_eq!(
        crate::cli::normalize_process_argv(intercom.clone()),
        intercom.to_vec()
    );
    // Presence before absence: the same tokens outside `intercom` are still rewritten…
    assert_eq!(
        crate::cli::normalize_process_argv(["cyrup", "-nc", "intercom"].map(String::from)),
        vec![
            "cyrup".to_string(),
            "--no-context-files".to_string(),
            "intercom".to_string()
        ]
    );
    // …and a non-intercom subcommand keeps the pre-pass (`install -na` ⇒ `--no-approve`).
    assert_eq!(
        crate::cli::normalize_process_argv(["cyrup", "install", "-na"].map(String::from)),
        vec![
            "cyrup".to_string(),
            "install".to_string(),
            "--no-approve".to_string()
        ]
    );
}

/// The whole pre-clap pipeline, keeping clap's verdict instead of panicking on it.
fn pipeline_outcome(args: &[&str]) -> (Result<Cli, String>, Vec<crate::diagnostics::Diagnostic>) {
    let norm = normalize_short_aliases(args.iter().map(|s| s.to_string()));
    let (lenient, diags) = crate::diagnostics::apply_arg_leniency(&norm);
    let (clean, ext) = partition_extension_flags(&lenient);
    let mut full = vec!["cyrup".to_string()];
    full.extend(clean);
    let cli = Cli::try_parse_from(full).map(|mut cli| {
        cli.extension_flags = ext;
        cli.normalize_list_flags();
        cli.restore_escaped_positionals();
        cli
    });
    (cli.map_err(|e| e.to_string()), diags)
}

/// SEAM-152 — pi's value-taking flags take the next token as their value whatever it looks like
/// (`args[++i]`, `cli/args.ts:115-131`, `:134-176`, `:184-201` @f1b2e77f5): only `--mode`,
/// `--use-theme`, `--tui-mode`, `--list-models` and `-p` inspect it. RED before the fix: the alias
/// pre-pass rewrote `-nc` to `--no-context-files` and clap then refused it (`a value is required for
/// '--name <NAME>'`); `-x`-style values were refused by clap or flagged `Unknown option`, and a
/// `--`-leading value after a short flag was captured as an extension flag.
#[test]
fn a_flag_shaped_value_is_the_value_as_in_pi() {
    let ok = |args: &[&str]| -> Cli {
        let (cli, diags) = pipeline_outcome(args);
        assert!(diags.is_empty(), "{args:?}: {diags:?}");
        let cli = cli.unwrap_or_else(|e| panic!("{args:?}: {e}"));
        assert!(
            cli.extension_flags.is_empty(),
            "{args:?}: {:?}",
            cli.extension_flags
        );
        cli
    };
    let cli = ok(&["--name", "-nc"]);
    assert_eq!(cli.name.as_deref(), Some("-nc"));
    assert!(!cli.no_context_files);
    assert_eq!(ok(&["-n", "-na"]).name.as_deref(), Some("-na"));
    assert!(!ok(&["-n", "-na"]).no_approve);
    assert_eq!(ok(&["-n", "--offline"]).name.as_deref(), Some("--offline"));
    assert_eq!(
        ok(&["-n", "--no-such-flag"]).name.as_deref(),
        Some("--no-such-flag")
    );
    assert_eq!(ok(&["--model", "-x"]).model.as_deref(), Some("-x"));
    assert_eq!(ok(&["--provider", "-x"]).provider.as_deref(), Some("-x"));
    assert_eq!(
        ok(&["--api-key", "-secret"]).api_key.as_deref(),
        Some("-secret")
    );
    assert_eq!(
        ok(&["--append-system-prompt", "-be terse"]).append_system_prompt,
        vec!["-be terse".to_string()]
    );
    assert_eq!(
        ok(&["--system-prompt", "--x"]).system_prompt.as_deref(),
        Some("--x")
    );
    assert_eq!(
        ok(&["-e", "-ext.wasm"]).extension,
        vec![std::path::PathBuf::from("-ext.wasm")]
    );
    assert_eq!(ok(&["--session", "-s"]).session.as_deref(), Some("-s"));
    assert_eq!(ok(&["--models", "-m"]).models, Some(vec!["-m".to_string()]));
    assert_eq!(ok(&["-xt", "-nt"]).exclude_tools, vec!["-nt".to_string()]);
    assert!(!ok(&["-xt", "-nt"]).no_tools);
    // Still modifiers / aliases where pi has them.
    assert_eq!(
        ok(&["--tools", "-bash"]).tools,
        Some(vec!["-bash".to_string()])
    );
    assert_eq!(ok(&["-t", "-bash"]).tools, Some(vec!["-bash".to_string()]));
    assert!(ok(&["-nc", "--name", "x"]).no_context_files);
    // A value is consumed once: the token after it is a flag again.
    let cli = ok(&["--name", "--model", "-nc"]);
    assert_eq!(cli.name.as_deref(), Some("--model"));
    assert!(cli.no_context_files);
    assert_eq!(cli.model, None);
}

/// SEAM-152 — a value-taking flag with nothing after it, as pi reports it. pi's guarded arms
/// (`… && i + 1 < args.length`) do not match, so a long flag falls into `unknownFlags`
/// (`args.ts:250-262`), reported at runtime as `Unknown option: --model`
/// (`agent-session-services.ts:120-123`), and a short one into `Unknown option: -t` (`:263-264`);
/// `--name`/`-n` has its own `--name requires a value` (`:126-131`).
#[test]
fn a_value_flag_at_the_end_of_argv_is_reported_as_pi_reports_it() {
    for flag in ["--name", "-n"] {
        let (_, diags) = pipeline_outcome(&[flag]);
        assert_eq!(
            diags.iter().map(|d| d.message.as_str()).collect::<Vec<_>>(),
            vec!["--name requires a value"],
            "{flag}"
        );
    }
    for flag in ["-t", "-e", "-xt"] {
        let (_, diags) = pipeline_outcome(&[flag]);
        assert_eq!(
            diags.iter().map(|d| d.message.as_str()).collect::<Vec<_>>(),
            vec![format!("Unknown option: {flag}")],
            "{flag}"
        );
    }
    for flag in [
        "--model",
        "--provider",
        "--models",
        "--tools",
        "--thinking",
        "--session",
    ] {
        let (cli, diags) = pipeline_outcome(&["-p", "hi", flag]);
        assert!(diags.is_empty(), "{flag}: {diags:?}");
        let cli = cli.unwrap_or_else(|e| panic!("{flag}: {e}"));
        assert_eq!(
            cli.extension_flags,
            vec![ExtensionFlag {
                name: flag.trim_start_matches('-').to_string(),
                value: ExtFlagValue::Bool(true),
            }],
            "{flag}"
        );
    }
}
