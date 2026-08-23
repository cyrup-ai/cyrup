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
