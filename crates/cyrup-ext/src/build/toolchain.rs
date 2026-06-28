//! Toolchain detection for the Tier-1 build loop (arch-08 §6.4, R-ARCH-EXT-015). Detects `cargo`,
//! the `wasm32-wasip2` target, and the componentization tooling. On a miss, the loop surfaces an
//! ACTIONABLE message (e.g. "run: rustup target add wasm32-wasip2") and STOPS cleanly — the host
//! binary itself never requires the wasm toolchain to build (arch-00 Appendix B).

use crate::error::ExtError;
use std::process::Command;

/// What the Tier-1 build loop needs and whether it is present.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolchainStatus {
    /// Everything needed to build + componentize a guest is present.
    Ready,
    /// `cargo` is missing.
    NoCargo,
    /// `cargo` is present but the `wasm32-wasip2` target is not installed.
    NoWasmTarget,
    /// Target present but componentization tooling (cargo-component / wasm-tools) is missing.
    NoComponentTooling,
}

impl ToolchainStatus {
    pub fn is_ready(&self) -> bool {
        matches!(self, ToolchainStatus::Ready)
    }

    /// An actionable, user-facing message for a missing-toolchain condition (R-ARCH-EXT-015).
    pub fn actionable(&self) -> Option<String> {
        match self {
            ToolchainStatus::Ready => None,
            ToolchainStatus::NoCargo => {
                Some("cargo not found: install the Rust toolchain (https://rustup.rs)".into())
            }
            ToolchainStatus::NoWasmTarget => {
                Some("run: rustup target add wasm32-wasip2".into())
            }
            ToolchainStatus::NoComponentTooling => Some(
                "componentization tooling missing: install with `cargo install cargo-component` \
                 and `cargo install wasm-tools`"
                    .into(),
            ),
        }
    }
}

/// A resolved toolchain identity, folded into the cache key so a toolchain change busts the cache.
#[derive(Clone, Debug)]
pub struct Toolchain {
    pub status: ToolchainStatus,
    /// e.g. `rustc 1.96.0 (...)` — part of the cache `toolchain-id`.
    pub rustc_version: String,
    pub target: &'static str,
}

impl Toolchain {
    /// The cache `toolchain-id` component (arch-08 §4.2).
    pub fn id(&self) -> String {
        format!("{}::{}", self.rustc_version.trim(), self.target)
    }
}

/// Detect the build toolchain (arch-08 §6.4). Never errors on a missing toolchain — it reports
/// status so the caller can surface an actionable message and stop cleanly.
pub fn detect_toolchain() -> Toolchain {
    let target = "wasm32-wasip2";
    let rustc_version = run_capture("rustc", &["--version"]).unwrap_or_default();

    let status = if run_capture("cargo", &["--version"]).is_none() {
        ToolchainStatus::NoCargo
    } else if !wasm_target_installed() {
        ToolchainStatus::NoWasmTarget
    } else if !component_tooling_present() {
        ToolchainStatus::NoComponentTooling
    } else {
        ToolchainStatus::Ready
    };

    Toolchain { status, rustc_version, target }
}

fn wasm_target_installed() -> bool {
    match run_capture("rustup", &["target", "list", "--installed"]) {
        Some(out) => out.lines().any(|l| l.trim() == "wasm32-wasip2"),
        // No rustup: fall back to checking the sysroot is irrelevant; assume not installed.
        None => false,
    }
}

fn component_tooling_present() -> bool {
    // The `wasm32-wasip2` target produces a component directly via the bundled `wasm-component-ld`
    // linker, so cargo-component is not strictly required; we still check for `wasm-tools` for
    // validation/inspection in the build loop.
    run_capture("wasm-tools", &["--version"]).is_some()
        || run_capture("cargo-component", &["--version"]).is_some()
}

/// Run a command and capture stdout, returning `None` if it cannot be spawned or exits non-zero.
fn run_capture(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        None
    }
}

/// Guard used by the Tier-1 loader: returns the actionable error if the toolchain is not ready
/// (R-ARCH-EXT-015) so the live build path is gated cleanly.
pub fn require_ready(tc: &Toolchain) -> Result<(), ExtError> {
    if tc.status.is_ready() {
        Ok(())
    } else {
        Err(ExtError::Toolchain(
            tc.status.actionable().unwrap_or_else(|| "toolchain not ready".into()),
        ))
    }
}
