//! Resource/package error vocabulary (arch-09 §8).
//!
//! Discovery never fails on a single bad file: malformed skills/themes/manifests degrade to a
//! [`ResourceWarning`] and the offending resource is skipped, so the registry still builds
//! (func-00 R-00-009). `discover()` only returns `Err` on a hard fault.

use std::path::PathBuf;

/// The four resource kinds plus packages, used by warnings/diagnostics.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ResourceKind {
    Skill,
    Prompt,
    Theme,
    Package,
}

/// Hard error for resource/package operations (arch-09 §8).
#[derive(thiserror::Error, Debug)]
pub enum ResourceError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("toml: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("front-matter in {path}: {reason}")]
    FrontMatter { path: PathBuf, reason: String },
    #[error("malformed skill {path}: {reason}")]
    Skill { path: PathBuf, reason: String },
    #[error("malformed theme {path}: {reason}")]
    Theme { path: PathBuf, reason: String },
    #[error("package manifest: {0}")]
    Manifest(String),
    #[error("git: {0}")]
    Git(String),
    #[error("project not trusted; refused project-scoped resource: {0}")]
    Untrusted(PathBuf),
    #[error("unsupported source (OCI deferred)")]
    Unsupported,
    #[error("cancelled")]
    Cancelled,
    #[error(transparent)]
    Core(#[from] cyrup_core::CoreError),
}

/// A soft, non-fatal problem found during discovery. Surfaced to the user (startup header /
/// `/reload` output) while the offending resource is skipped.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceWarning {
    pub path: PathBuf,
    pub kind: ResourceKind,
    pub reason: String,
}

impl ResourceWarning {
    pub fn new(kind: ResourceKind, path: impl Into<PathBuf>, reason: impl Into<String>) -> Self {
        Self { kind, path: path.into(), reason: reason.into() }
    }
}
