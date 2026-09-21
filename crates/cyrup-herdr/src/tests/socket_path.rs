//! §5.1 — herdr's own socket-path ladder, replicated rung for rung.
//!
//! Source of the order: `active_api_socket_path` (`tmp/herdr/src/session.rs:173-181`) →
//! `api_socket_path_for` (`:169-171`) → `data_dir_for` (`:161-167`) → `config_dir`
//! (`tmp/herdr/src/config/io.rs:30-35`), documented at `socket-api.mdx:686-692`.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::env::{HERDR_SESSION, HERDR_SOCKET_PATH, resolve_socket_path};

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

/// One row of the resolution table: why it is here, the environment, the path herdr would resolve.
struct Rung {
    why: &'static str,
    env: &'static [(&'static str, &'static str)],
    expected: &'static str,
}

/// The table test over every rung, including the two filters that are easy to drop.
#[test]
fn the_socket_path_resolution_order_is_herdrs_own() {
    let cases: &[Rung] = &[
        Rung {
            why: "rung 2 — HERDR_SOCKET_PATH wins over everything below it",
            env: &[
                (HERDR_SOCKET_PATH, "/run/user/1000/herdr-explicit.sock"),
                (HERDR_SESSION, "work"),
                ("XDG_CONFIG_HOME", "/xdg"),
                ("HOME", "/home/u"),
            ],
            expected: "/run/user/1000/herdr-explicit.sock",
        },
        Rung {
            why: "rung 3 — a named session lands under sessions/<name>/ (session.rs:161-171)",
            env: &[
                (HERDR_SESSION, "work"),
                ("XDG_CONFIG_HOME", "/xdg"),
                ("HOME", "/home/u"),
            ],
            expected: "/xdg/herdr/sessions/work/herdr.sock",
        },
        Rung {
            why: "rung 3 — HERDR_SESSION=default is IGNORED (session.rs:99), not a directory name",
            env: &[
                (HERDR_SESSION, "default"),
                ("XDG_CONFIG_HOME", "/xdg"),
                ("HOME", "/home/u"),
            ],
            expected: "/xdg/herdr/herdr.sock",
        },
        Rung {
            why: "rung 3 — a name validate_name rejects falls through (session.rs:100, :452-472)",
            env: &[
                (HERDR_SESSION, "../../etc"),
                ("XDG_CONFIG_HOME", "/xdg"),
                ("HOME", "/home/u"),
            ],
            expected: "/xdg/herdr/herdr.sock",
        },
        Rung {
            why: "rung 4 — XDG_CONFIG_HOME/herdr/herdr.sock (config/io.rs:30-35)",
            env: &[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/home/u")],
            expected: "/xdg/herdr/herdr.sock",
        },
        Rung {
            why: "rung 4 — without XDG_CONFIG_HOME, $HOME/.config/herdr (config/io.rs:62-68)",
            env: &[("HOME", "/home/u")],
            expected: "/home/u/.config/herdr/herdr.sock",
        },
    ];

    for rung in cases {
        assert_eq!(
            resolve_socket_path(&env(rung.env)),
            PathBuf::from(rung.expected),
            "{}",
            rung.why
        );
    }
}

/// herdr's `app_dir_name` answers `"herdr-dev"` under `cfg!(debug_assertions)`
/// (`tmp/herdr/src/config/io.rs:22-28`) — but that reads **herdr's** build profile, and this code
/// is compiled into **cyrup**. Deriving the directory from cyrup's own profile would make a debug
/// cyrup look in `~/.config/herdr-dev` while the user's released herdr listens on
/// `~/.config/herdr`: a client that works in release and silently finds nothing in debug.
///
/// This test runs under `cargo test`, i.e. with `debug_assertions` on, so it is exactly the case
/// that would break.
#[test]
fn the_config_directory_is_herdrs_release_name_whatever_cyrups_build_profile_is() {
    // `cargo test` builds with `debug_assertions` on, which is exactly the configuration in which
    // reading cyrup's own profile would resolve `herdr-dev` and find nothing.
    const {
        assert!(cfg!(debug_assertions));
    }
    assert_eq!(
        resolve_socket_path(&env(&[("XDG_CONFIG_HOME", "/xdg")])),
        PathBuf::from("/xdg/herdr/herdr.sock")
    );
}
