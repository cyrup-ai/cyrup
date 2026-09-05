use cyrup_config::AppMode;

use super::*;

#[test]
fn mode_flag_and_aliases_take_precedence_over_tty() {
    assert_eq!(
        resolve_app_mode(&parse(&["--mode", "rpc"]), true, true),
        AppMode::Rpc
    );
    assert_eq!(
        resolve_app_mode(&parse(&["--rpc"]), true, true),
        AppMode::Rpc
    );
    assert_eq!(
        resolve_app_mode(&parse(&["--mode", "json"]), true, true),
        AppMode::Json
    );
    assert_eq!(
        resolve_app_mode(&parse(&["--json"]), true, true),
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
    // ACP-002 — the ACP branch is FIRST, so it also wins over an explicitly-passed sibling alias.
    assert_eq!(
        resolve_app_mode(&parse(&["--acp", "--rpc"]), false, false),
        AppMode::Acp
    );
    assert_eq!(
        resolve_app_mode(&parse(&["--acp", "--json", "-p"]), false, false),
        AppMode::Acp
    );
    // The other four modes are byte-identical to what they were before the variant existed.
    assert_eq!(resolve_app_mode(&parse(&["--rpc"]), false, false), AppMode::Rpc);
    assert_eq!(resolve_app_mode(&parse(&["--json"]), false, false), AppMode::Json);
    assert_eq!(resolve_app_mode(&parse(&[]), false, false), AppMode::Print);
    assert_eq!(resolve_app_mode(&parse(&[]), true, true), AppMode::Interactive);
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
    assert!(should_take_over_stdout(&parse(&["--json"]), AppMode::Json));
    assert!(!should_take_over_stdout(
        &parse(&["--help"]),
        AppMode::Print
    ));
    assert!(!should_take_over_stdout(&parse(&[]), AppMode::Interactive));
}
