//! OS-level sandbox backend — **DEFERRED placeholder** (R-12-013, A-12-9).
//!
//! # Tracked gap
//! arch-12 §6.4 specifies an in-process `SandboxedOps` decorator that hardens spawned children with
//! Landlock + seccompiler (Linux) or a Seatbelt profile (macOS). Those crates
//! (`landlock`/`seccompiler`/`libc sandbox_init`) are **not** pulled here: the concrete sandbox
//! technology is a deferred architecture decision (arch-12 §12) and is not testable on this CI host
//! (A-12-9). This module therefore ships only the *shape* — a [`SandboxPolicy`] config type, an
//! [`OsSandbox`] trait + [`SandboxKind`] tag, and a [`DeferredSandbox`] that reports `Unsupported`.
//! When the technology is chosen, a real `SandboxedOps` implementing [`crate::ops::ProcOps`] +
//! [`crate::ops::FsOps`] slots in behind a `cfg`/feature gate **without** new external crates leaking
//! into the default build.
//!
//! No silent downgrade (R-12-004): constructing a sandbox returns [`SandboxError::Unsupported`]
//! rather than quietly running unsandboxed.

use std::path::PathBuf;
use thiserror::Error;

/// OS-sandbox policy (mirrors Pi `sandbox.json`, arch-12 §4.2). Config-only here; enforcement is
/// deferred. Serde uses the workspace `camelCase` convention with forward-compatible defaults.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxPolicy {
    /// Paths whose reads are denied (e.g. `~/.ssh`, `~/.aws`).
    #[serde(default)]
    pub fs_read_deny: Vec<PathBuf>,
    /// Paths whose writes are allowed (e.g. `.`, `/tmp`).
    #[serde(default)]
    pub fs_write_allow: Vec<PathBuf>,
    /// Paths whose writes are denied (e.g. `.env`, `*.pem`).
    #[serde(default)]
    pub fs_write_deny: Vec<PathBuf>,
}

/// Which OS-sandbox technology a backend uses. `Unsupported` is the only variant realized today.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SandboxKind {
    /// No OS sandbox available in this build/host (the deferred default).
    Unsupported,
    /// Linux Landlock (paths) + seccompiler (syscalls) — planned (arch-12 §6.4).
    Landlock,
    /// macOS Seatbelt profile — planned (arch-12 §6.4).
    Seatbelt,
}

/// Errors from constructing/applying an OS sandbox.
#[derive(Error, Debug)]
pub enum SandboxError {
    /// The OS sandbox is not available in this build/host (deferred, R-12-013).
    #[error("os sandbox unsupported on this platform/build (deferred: R-12-013)")]
    Unsupported,
}

/// Marker trait for an OS-sandbox backend. A real implementation also implements
/// [`crate::ops::ProcOps`]/[`crate::ops::FsOps`] so it composes with the operations seam.
pub trait OsSandbox: Send + Sync {
    /// The sandbox technology in use.
    fn kind(&self) -> SandboxKind;

    /// True when this sandbox actually enforces restrictions (false for the deferred placeholder).
    fn enforces(&self) -> bool {
        self.kind() != SandboxKind::Unsupported
    }
}

/// The deferred placeholder backend: reports `Unsupported` and enforces nothing.
#[derive(Clone, Copy, Debug, Default)]
pub struct DeferredSandbox;

impl OsSandbox for DeferredSandbox {
    fn kind(&self) -> SandboxKind {
        SandboxKind::Unsupported
    }
}

/// The sandbox technology this build *would* use on the current target (documentation only; the
/// implementation is deferred). Lets callers surface the tracked gap instead of guessing.
pub fn planned_kind() -> SandboxKind {
    #[cfg(target_os = "linux")]
    {
        SandboxKind::Landlock
    }
    #[cfg(target_os = "macos")]
    {
        SandboxKind::Seatbelt
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        SandboxKind::Unsupported
    }
}

/// Attempt to build an OS sandbox for `_policy`. Always `Err(Unsupported)` until the deferred
/// implementation lands — never silently returns an unsandboxed backend (R-12-004).
pub fn build(_policy: &SandboxPolicy) -> Result<DeferredSandbox, SandboxError> {
    Err(SandboxError::Unsupported)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_does_not_enforce() {
        let s = DeferredSandbox;
        assert_eq!(s.kind(), SandboxKind::Unsupported);
        assert!(!s.enforces());
    }

    #[test]
    fn build_is_unsupported() {
        assert!(matches!(
            build(&SandboxPolicy::default()),
            Err(SandboxError::Unsupported)
        ));
    }

    #[test]
    fn policy_serde_camel_case_roundtrip() {
        let json = serde_json::json!({ "fsWriteDeny": [".env"], "fsReadDeny": ["/home/x/.ssh"] });
        let p: SandboxPolicy = serde_json::from_value(json).unwrap();
        assert_eq!(p.fs_write_deny, vec![PathBuf::from(".env")]);
        assert_eq!(p.fs_read_deny, vec![PathBuf::from("/home/x/.ssh")]);
        let back = serde_json::to_value(&p).unwrap();
        assert!(back.get("fsWriteDeny").is_some());
    }
}
