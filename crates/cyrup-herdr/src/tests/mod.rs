//! Crate-internal tests.
//!
//! These live here rather than under `tests/` because Cargo compiles every file in `tests/` into
//! its own binary and its own process, and nothing here needs that isolation: no test in this crate
//! touches the process environment — the resolution ladder takes an [`crate::EnvSource`] map for
//! exactly that reason, and the [`cli`] tests that do spawn a process name its path outright rather
//! than going through `PATH`. The one genuine seam — driving a production host event through to a
//! real socket — belongs in `crates/cyrup-it/`, per `docs/TEST-ARCHITECTURE.md` §2.
//!
//! Every socket test drives [`fake_server::FakeHerdr`], a real `UnixListener` in a per-test
//! tempdir, and every CLI test drives a shell script in one that prints herdr's own bytes.
//! **The suite is green with neither `herdr` nor ghostty installed**, which is the state of this
//! container and of CI: no test requires the `herdr` binary, and the one that needs its *absence*
//! ([`cli`]) points at a path inside its own tempdir.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[cfg(unix)]
mod fake_server;

#[cfg(unix)]
mod agent_view;
#[cfg(unix)]
mod bootstrap;
mod cli;
#[cfg(unix)]
mod client_contract;
mod discovery;
#[cfg(unix)]
mod machine_remote;
mod method_names;
#[cfg(unix)]
mod probe_round_trip;
#[cfg(unix)]
mod read_half;
#[cfg(unix)]
mod reconnect;
#[cfg(unix)]
mod relay;
mod socket_path;
#[cfg(unix)]
mod stream;
#[cfg(unix)]
mod transport_bounds;
#[cfg(unix)]
mod write_half;
