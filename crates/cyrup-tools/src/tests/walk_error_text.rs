//! `LocalFs::walk`'s error text must read the way `rg` 14.1.0 prints it — `{path}: {io error}`
//! with the path stated ONCE.
//!
//! At the pinned `ignore` 0.4.26 + `walkdir` 2.5.0, `ignore::Error`'s own `Display` does not.
//! `Error::from_walkdir` (`ignore-0.4.26/src/lib.rs:296-301`) stores
//! `WithPath { path, err: Io(io::Error::from(walkdir_err)) }`, and walkdir 2.5.0's
//! `From<Error> for io::Error` (`walkdir-2.5.0/src/error.rs:253-261`) is
//! `io::Error::new(kind, walk_err)` — a CUSTOM io error whose `Display`
//! (`error.rs:224-229`) re-states the path as `IO error for operation on {path}: {err}`.
//! Under `WithPath`'s `{path}: {err}` (`lib.rs:333-335`) that composes to the path twice:
//!
//! ```text
//! rg: /proc/1/task/1/fdinfo: IO error for operation on /proc/1/task/1/fdinfo: Permission denied (os error 13)
//! ```
//!
//! against ripgrep's actual
//! `rg: /proc/1/task/1/fdinfo: Permission denied (os error 13)`.
//! (walkdir 2.3.x returned the inner `io::Error` unchanged from `From`, which is why the real
//! `rg` binary is unaffected.)
//!
//! These tests are HERMETIC: no `/proc`, no root, no `chmod`. `ignore::Error`'s variants and
//! fields are public, and the only thing `walk_error_message` needs from walkdir is the SHAPE
//! of its wrapper — a custom `io::Error` whose boxed payload has a `source()` — which
//! [`WalkdirShaped`] reproduces exactly.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::ops::local::fs::walk_error_message;
use std::path::{Path, PathBuf};

/// Stand-in for `walkdir::Error` with the same two load-bearing properties: its `Display`
/// prepends `IO error for operation on {path}: `, and its `source()` is the ORIGINAL
/// `io::Error` (`walkdir-2.5.0/src/error.rs:212-217`).
#[derive(Debug)]
struct WalkdirShaped {
    path: PathBuf,
    err: std::io::Error,
}

impl std::fmt::Display for WalkdirShaped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "IO error for operation on {}: {}",
            self.path.display(),
            self.err
        )
    }
}

impl std::error::Error for WalkdirShaped {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.err)
    }
}

/// `io::Error::from(walkdir::Error)` — the custom repr whose payload owns the real errno.
fn walkdir_wrapped(path: &str, errno: i32) -> std::io::Error {
    let err = std::io::Error::from_raw_os_error(errno);
    let kind = err.kind();
    std::io::Error::new(
        kind,
        WalkdirShaped {
            path: PathBuf::from(path),
            err,
        },
    )
}

/// The regression itself: `WithPath { Io(<walkdir wrapper over EACCES>) }`.
#[test]
fn with_path_over_walkdir_wrapper_states_the_path_once() {
    let err = ignore::Error::WithPath {
        path: PathBuf::from("/srv/locked"),
        err: Box::new(ignore::Error::Io(walkdir_wrapped("/srv/locked", 13))),
    };

    // What `Display` gives today — the bug, asserted so the premise of this test is checked
    // rather than assumed.
    assert_eq!(
        err.to_string(),
        "/srv/locked: IO error for operation on /srv/locked: Permission denied (os error 13)"
    );

    let msg = walk_error_message(&err);
    assert_eq!(msg, "/srv/locked: Permission denied (os error 13)");
    assert!(
        !msg.contains("IO error for operation on"),
        "walkdir's wording leaked through: {msg}"
    );
    assert_eq!(
        msg.matches("/srv/locked").count(),
        1,
        "path stated more than once: {msg}"
    );
}

/// A plain OS-repr `io::Error` (`get_ref()` is `None`) is passed through untouched.
#[test]
fn with_path_over_plain_os_error_is_unchanged() {
    let err = ignore::Error::WithPath {
        path: PathBuf::from("/nope/does/not/exist"),
        err: Box::new(ignore::Error::Io(std::io::Error::from_raw_os_error(2))),
    };

    assert_eq!(
        walk_error_message(&err),
        "/nope/does/not/exist: No such file or directory (os error 2)"
    );
    assert_eq!(walk_error_message(&err), err.to_string());
}

/// No `WithPath` => no prefix, and certainly no empty `": "` prefix (DoD 4).
#[test]
fn bare_io_error_has_no_path_prefix() {
    let err = ignore::Error::Io(std::io::Error::from_raw_os_error(13));
    let msg = walk_error_message(&err);

    assert_eq!(msg, "Permission denied (os error 13)");
    assert!(!msg.starts_with(": "), "empty prefix: {msg}");
}

/// `from_walkdir` returns `WithDepth { Loop }` with NO `WithPath` (`lib.rs:286-295`), so `rg`
/// prints a loop with no path prefix at all. Verbatim capture from `rg 14.1.0 --no-config -nL`.
#[test]
fn symlink_loop_keeps_ripgreps_unprefixed_wording() {
    let err = ignore::Error::WithDepth {
        depth: 1,
        err: Box::new(ignore::Error::Loop {
            ancestor: PathBuf::from("/w"),
            child: PathBuf::from("/w/a/back"),
        }),
    };

    let msg = walk_error_message(&err);
    assert_eq!(
        msg,
        "File system loop found: /w/a/back points to an ancestor /w"
    );
    assert_eq!(msg, err.to_string());
    assert!(!msg.starts_with('/'), "a path prefix was invented: {msg}");
}

/// Every non-io-leaf shape must be a byte-identical reimplementation of `Display` (DoD 6).
#[test]
fn ignore_file_parse_errors_match_display_exactly() {
    let path = Path::new("/w/.gitignore");
    let err = ignore::Error::Partial(vec![ignore::Error::WithPath {
        path: path.to_path_buf(),
        err: Box::new(ignore::Error::WithLineNumber {
            line: 3,
            err: Box::new(ignore::Error::Glob {
                glob: Some("[".to_string()),
                err: "unclosed character class".to_string(),
            }),
        }),
    }]);

    assert_eq!(
        walk_error_message(&err),
        "/w/.gitignore: line 3: error parsing glob '[': unclosed character class"
    );
    assert_eq!(walk_error_message(&err), err.to_string());
}

/// `ignore` builds a few `io::Error::new(kind, "<literal>")` values (`walk.rs:175-179`, `:377`,
/// `:427`). Those DO have a boxed payload, so `get_ref()` is `Some` — only the `source()` half
/// of the guard keeps them from being over-peeled.
#[test]
fn string_payload_io_errors_are_not_over_peeled() {
    let err = ignore::Error::Io(std::io::Error::other("boom"));

    assert!(std::io::Error::other("boom").get_ref().is_some());
    assert_eq!(walk_error_message(&err), "boom");
    assert_eq!(walk_error_message(&err), err.to_string());
}
