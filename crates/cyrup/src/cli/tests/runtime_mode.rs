use cyrup_config::AppMode;

use super::*;

#[test]
fn mode_flag_takes_precedence_over_tty() {
    assert_eq!(
        resolve_app_mode(&parse(&["--mode", "rpc"]), true, true),
        AppMode::Rpc
    );
    assert_eq!(
        resolve_app_mode(&parse(&["--mode", "json"]), true, true),
        AppMode::Json
    );
    assert_eq!(
        resolve_app_mode(&parse(&["-p"]), true, true),
        AppMode::Print
    );
    // `--mode text` is the default — interactive with a full TTY.
    assert_eq!(
        resolve_app_mode(&parse(&["--mode", "text"]), true, true),
        AppMode::Interactive
    );
}

/// ACP-002 — the table test the unit's *Verify* line names. Both spellings, and — the point of the
/// unit — with **both ends piped**, which is how an editor actually launches the agent. Before the
/// ACP branch was hoisted to the front of `resolve_app_mode`, this resolved `Print` and the host
/// ate the client's first JSON-RPC frame as a chat prompt.
#[test]
fn acp_wins_over_the_non_tty_print_fallback() {
    for argv in [vec!["--acp"], vec!["--mode", "acp"]] {
        let cli = parse(&argv);
        assert_eq!(
            resolve_app_mode(&cli, false, false),
            AppMode::Acp,
            "{argv:?} with pipes on both ends must not resolve Print"
        );
        assert_eq!(resolve_app_mode(&cli, true, true), AppMode::Acp, "{argv:?}");
    }
    // ACP-002 — the ACP branch is FIRST, so it also wins over an explicitly-passed sibling mode.
    assert_eq!(
        resolve_app_mode(&parse(&["--acp", "--mode", "rpc"]), false, false),
        AppMode::Acp
    );
    assert_eq!(
        resolve_app_mode(&parse(&["--acp", "--mode", "json", "-p"]), false, false),
        AppMode::Acp
    );
    // The other four modes are byte-identical to what they were before the variant existed.
    assert_eq!(
        resolve_app_mode(&parse(&["--mode", "rpc"]), false, false),
        AppMode::Rpc
    );
    assert_eq!(
        resolve_app_mode(&parse(&["--mode", "json"]), false, false),
        AppMode::Json
    );
    assert_eq!(resolve_app_mode(&parse(&[]), false, false), AppMode::Print);
    assert_eq!(
        resolve_app_mode(&parse(&[]), true, true),
        AppMode::Interactive
    );
    // ACP-002 — `should_take_over_stdout` needs no change and must NOT gain an exemption: the ACP
    // host writes JSON-RPC frames to stdout and a stray library line would corrupt them.
    assert!(should_take_over_stdout(&parse(&["--acp"]), AppMode::Acp));
}

#[test]
fn tty_probing_selects_interactive_or_print() {
    let cli = parse(&[]);
    assert_eq!(resolve_app_mode(&cli, true, true), AppMode::Interactive);
    assert_eq!(resolve_app_mode(&cli, false, true), AppMode::Print);
    assert_eq!(resolve_app_mode(&cli, true, false), AppMode::Print);
}

#[test]
fn stdout_takeover_decision_matches_pi() {
    // Plain metadata commands (help / list-models without --print/--mode) are NOT guarded.
    assert!(is_plain_runtime_metadata_command(&parse(&["--help"])));
    assert!(is_plain_runtime_metadata_command(&parse(&[
        "--list-models"
    ])));
    assert!(!is_plain_runtime_metadata_command(&parse(&[
        "-p",
        "--list-models"
    ])));
    // Print/JSON/RPC (non-interactive, non-metadata) ARE guarded; interactive never is.
    assert!(should_take_over_stdout(
        &parse(&["-p", "hi"]),
        AppMode::Print
    ));
    assert!(should_take_over_stdout(
        &parse(&["--mode", "json"]),
        AppMode::Json
    ));
    assert!(!should_take_over_stdout(
        &parse(&["--help"]),
        AppMode::Print
    ));
    assert!(!should_take_over_stdout(&parse(&[]), AppMode::Interactive));
}

/// SEAM-057 — `--json`, `--rpc` and `--output-format` are not pi flags (pi `parseArgs` has no arm
/// for any of them, `cli/args.ts` @v0.87.1), so they take pi's unknown-long-flag arm: captured as
/// extension flags, never changing the mode. An extension that registers `json` receives it; with
/// none, the runtime's reconciliation reports `Unknown option` and exits 1, exactly as pi does.
#[test]
fn the_removed_mode_aliases_are_extension_flags_as_in_pi() {
    let json = parse_like_main(&["--json", "hello"]);
    assert_eq!(
        json.extension_flags,
        vec![ExtensionFlag {
            name: "json".into(),
            value: ExtFlagValue::Str("hello".into()),
        }],
        "pi's capture takes the next non-flag token as the value (args.ts:226-240)"
    );
    assert_eq!(resolve_app_mode(&json, true, true), AppMode::Interactive);

    let rpc = parse_like_main(&["--rpc"]);
    assert_eq!(
        rpc.extension_flags,
        vec![ExtensionFlag {
            name: "rpc".into(),
            value: ExtFlagValue::Bool(true),
        }]
    );
    assert_eq!(resolve_app_mode(&rpc, true, true), AppMode::Interactive);

    let fmt = parse_like_main(&["--output-format", "json"]);
    assert_eq!(
        fmt.extension_flags,
        vec![ExtensionFlag {
            name: "output-format".into(),
            value: ExtFlagValue::Str("json".into()),
        }]
    );
    assert_eq!(resolve_app_mode(&fmt, false, false), AppMode::Print);
}

/// MIRROR: `--acp` is the one cyrup-only mode flag kept (its CYRUP-DELTA names the ACP clients'
/// `"args": ["--acp"]` launch contract), so it still reaches clap and selects the ACP host.
#[test]
fn acp_is_still_a_known_flag() {
    let cli = parse_like_main(&["--acp"]);
    assert!(cli.extension_flags.is_empty(), "{:?}", cli.extension_flags);
    assert_eq!(resolve_app_mode(&cli, false, false), AppMode::Acp);
}
