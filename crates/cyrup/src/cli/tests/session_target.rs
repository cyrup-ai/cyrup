use std::path::PathBuf;

use cyrup_session_svc::SessionTarget;

use super::*;

#[test]
fn session_target_precedence_and_validation() {
    let d = dirs();
    assert!(matches!(
        parse(&["-c"]).session_target(&d.session_dir),
        SessionTarget::Continue
    ));
    assert!(matches!(
        parse(&[]).session_target(&d.session_dir),
        SessionTarget::New
    ));
    // A bare id resolves under the session dir with `.jsonl`.
    match parse(&["--session", "abc123"]).session_target(&d.session_dir) {
        SessionTarget::Resume(p) => {
            assert_eq!(p, PathBuf::from("/agent/sessions/abc123.jsonl"))
        }
        other => panic!("expected resume, got {other:?}"),
    }
    // --fork wins over --continue (and conflicts are reported).
    assert!(
        parse(&["--fork", "x", "--continue"])
            .validate_session_flags()
            .is_err()
    );
    assert!(
        parse(&["--session", "a", "--session-id", "valid"])
            .validate_session_flags()
            .is_err()
    );
    // `--no-session --continue` is NOT a conflict in Pi (no-session just goes in-memory).
    assert!(
        parse(&["--no-session", "--continue"])
            .validate_session_flags()
            .is_ok()
    );
    // `--fork --session-id` is allowed (fork into a new id, Pi createSessionManager).
    assert!(
        parse(&["--fork", "x", "--session-id", "newid"])
            .validate_session_flags()
            .is_ok()
    );
    assert!(parse(&["--continue"]).validate_session_flags().is_ok());
}

#[test]
fn session_id_format_is_validated() {
    assert!(assert_valid_session_id("abc-123_x.y").is_ok());
    assert!(assert_valid_session_id("a").is_ok());
    assert!(assert_valid_session_id("").is_err());
    assert!(assert_valid_session_id("-bad").is_err());
    assert!(assert_valid_session_id("bad-").is_err());
    assert!(assert_valid_session_id("bad/slash").is_err());
    // Threaded through the flag validator (a value clap accepts but the grammar rejects).
    assert!(
        parse(&["--session-id", "bad."])
            .validate_session_flags()
            .is_err()
    );
    assert!(
        parse(&["--session-id", "ok.id-1"])
            .validate_session_flags()
            .is_ok()
    );
}
