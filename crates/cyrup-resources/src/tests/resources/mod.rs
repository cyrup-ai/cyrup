//! Conformance tests for cyrup-resources (A-09-1..10, func-09).
//!
//! Tempdir fixtures only; hermetic (no network) by default. The git CLONE / ref-CHECKOUT /
//! PULL-on-update paths are exercised against a LOCAL `file://` git repo created in-test via gix's
//! real clone machinery (skipped gracefully if the `git` CLI is unavailable); local-path install +
//! manifest parsing + pin/update are exercised unconditionally. One true-network https clone test
//! is `#[ignore]`d and additionally gated on `CYRUP_GIT_NETWORK_TESTS=1`.
//!
//! Split from a single 4,450-line file; assertions are unchanged, only their module moved. The
//! `#![allow]` below is inherited by every submodule declared here — the crate's
//! `[lints] workspace = true` denies `unwrap_used`/`expect_used`/`panic`/`indexing_slicing` for the
//! whole compilation, `#[cfg(test)]` code included.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod autoload_delta;
mod discovery;
mod fixtures;
mod git_clone;
mod git_url;
mod install;
mod manifest;
mod precedence;
mod prompt_namespaces;
mod prompts;
mod settings_packages;
mod skills;
mod system_prompt;
mod themes;
