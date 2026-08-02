//! The `cyrup:ext` WIT world has TWO on-disk copies — `crates/cyrup-ext/wit/world.wit` (consumed by
//! the host's `wasmtime::component::bindgen!`) and `crates/cyrup-ext-sdk/wit/world.wit` (consumed by
//! the guest's `wit-bindgen`). Nothing in the build enforces that they agree: if they drift, the host
//! links against one shape and the guest exports another, and the failure surfaces as a raw wasmtime
//! instantiation error at test time rather than a compile error.
//!
//! This is that enforcement.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

#[test]
fn the_host_and_guest_wit_world_copies_are_identical() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let host = root.join("wit/world.wit");
    let guest = root.join("../cyrup-ext-sdk/wit/world.wit");

    let host_src = std::fs::read_to_string(&host)
        .unwrap_or_else(|e| panic!("read {}: {e}", host.display()));
    let guest_src = std::fs::read_to_string(&guest)
        .unwrap_or_else(|e| panic!("read {}: {e}", guest.display()));

    if host_src != guest_src {
        let first_diff = host_src
            .lines()
            .zip(guest_src.lines())
            .enumerate()
            .find(|(_, (a, b))| a != b)
            .map(|(i, (a, b))| format!("line {}:\n  host : {a}\n  guest: {b}", i + 1))
            .unwrap_or_else(|| {
                format!(
                    "line counts differ: host {} vs guest {}",
                    host_src.lines().count(),
                    guest_src.lines().count()
                )
            });
        panic!(
            "the host and guest WIT world copies have drifted — change BOTH:\n  {}\n  {}\n{first_diff}",
            host.display(),
            guest.display()
        );
    }
}
