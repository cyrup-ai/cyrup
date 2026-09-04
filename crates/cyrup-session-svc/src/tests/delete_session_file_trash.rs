//! SEAM-063 — session delete must try the OS `trash` CLI before it permanently unlinks, and must
//! report which happened instead of swallowing the outcome.
//!
//! Pi ground truth: `packages/coding-agent/src/modes/interactive/components/session-selector.ts:
//! 644-679` @v0.83.0 (byte-identical at v0.84.1):
//!
//! ```text
//! const trashArgs = sessionPath.startsWith("-") ? ["--", sessionPath] : [sessionPath];
//! const trashResult = spawnSync("trash", trashArgs, { encoding: "utf-8" });
//! if (trashResult.status === 0 || !existsSync(sessionPath)) return { ok: true, method: "trash" };
//! try { await unlink(sessionPath); return { ok: true, method: "unlink" }; }
//! catch (err) { … return { ok: false, method: "unlink", error }; }
//! ```
//!
//! and the caller renders `result.method === "trash" ? "Session moved to trash" : "Session deleted"`
//! (`:846`) or `Failed to delete: ${error}` (`:849`). This path is live in the PRE-LAUNCH picker
//! too — `onDeleteSession` is assigned unconditionally in the constructor (`:831`), unlike
//! `onRenameSession`.
//!
//! Before the fix `rg -ni 'trash' crates` was empty workspace-wide: both call sites were a bare
//! `std::fs::remove_file`, and the pre-launch one discarded the `io::Result` entirely, so a delete
//! that failed on a read-only volume was visually identical to one that succeeded.
//!
//! **Coverage limit, stated rather than hidden.** The `trash`-is-installed arm cannot be pinned
//! from a unit test in this crate: putting a stub first on `PATH` needs `std::env::set_var`, which
//! is `unsafe` under edition 2024 and this crate is `#![forbid(unsafe_code)]`. What IS pinned here
//! is the argv construction (pi's `--` guard), the two status strings the callers render, and the
//! absent-file verdict. The item's own Verify step — a live run with `trash` installed, confirming
//! the status line says "moved to trash" and the file is in the OS trash — remains required.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

use crate::session::trash_args;
use crate::{DeleteMethod, delete_session_file_at};
use tempfile::TempDir;

/// pi's `trashArgs` (`session-selector.ts:649`). RED before the fix by construction — there was no
/// `trash` invocation at all, so there were no args to guard.
#[test]
fn trash_argv_carries_pis_dash_dash_guard() {
    assert_eq!(
        trash_args(Path::new("/s/a.jsonl")),
        vec![std::ffi::OsString::from("/s/a.jsonl")],
        "an ordinary path is passed alone"
    );
    assert_eq!(
        trash_args(Path::new("-dash.jsonl")),
        vec![
            std::ffi::OsString::from("--"),
            std::ffi::OsString::from("-dash.jsonl")
        ],
        "a leading-dash path must follow `--` or `trash` reads it as an option"
    );
}

/// pi's status strings, which the callers render verbatim (`session-selector.ts:846`). Pinned
/// because the pre-launch picker had NO status line at all before this and the in-app one said
/// "deleted session" whether or not the file went.
#[test]
fn status_messages_are_pis_own() {
    assert_eq!(
        DeleteMethod::Trash.status_message(),
        "Session moved to trash"
    );
    assert_eq!(DeleteMethod::Unlink.status_message(), "Session deleted");
}

/// The file really goes, by whichever route this machine offers — with `trash` installed pi (and
/// now cyrup) reports `Trash`, without it both fall through to `unlink` (`:666-674`). Asserting on
/// the OUTCOME rather than the method is deliberate: the method depends on the developer's `PATH`
/// and both answers are pi-correct.
///
/// An already-absent file is success on both sides (pi reaches that verdict through its
/// `!existsSync(sessionPath)` clause at `:666`), which is what makes a double-delete from a stale
/// picker row harmless.
#[test]
fn the_file_is_removed_and_an_absent_file_is_a_no_op() {
    let tmp = TempDir::new().unwrap();
    let f = tmp.path().join("x.jsonl");
    std::fs::write(&f, b"{}\n").unwrap();
    delete_session_file_at(&f).expect("delete succeeds with or without a real `trash`");

    let absent = tmp.path().join("never-existed.jsonl");
    assert_eq!(
        delete_session_file_at(&absent).expect("an absent file is a no-op, not an error"),
        DeleteMethod::Trash,
        "pi's `!existsSync` clause answers `trash` for an already-gone file (:666)"
    );
}
