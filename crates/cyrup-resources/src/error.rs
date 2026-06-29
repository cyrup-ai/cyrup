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
        Self {
            kind,
            path: path.into(),
            reason: reason.into(),
        }
    }
}

/// Diagnostic severity, 1:1 with Pi's `ResourceDiagnostic.type` (diagnostics.ts;
/// resource-loader.ts:8). `collision` carries winner/loser detail in [`ResourceDiagnostic::collision`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DiagnosticType {
    Warning,
    Error,
    Collision,
}

/// Winner/loser detail for a same-name `collision` diagnostic (skills.ts:415-424;
/// resource-loader.ts:939-964). `winner_path` is the resource kept; `loser_path` is shadowed.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Collision {
    pub resource_type: ResourceKind,
    pub name: String,
    pub winner_path: PathBuf,
    pub loser_path: PathBuf,
}

/// A structured diagnostic, mirroring Pi's `ResourceDiagnostic` (warning | error | collision).
///
/// Richer than [`ResourceWarning`]: distinguishes `error` (e.g. a configured path that does not
/// exist) from `warning`, and carries structured `collision` detail so the UI can print the
/// `name "X" collision (winner/loser)` feedback Pi shows at startup and on `/reload`.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceDiagnostic {
    #[serde(rename = "type")]
    pub diagnostic_type: DiagnosticType,
    pub message: String,
    pub path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collision: Option<Collision>,
}

impl ResourceDiagnostic {
    pub fn warning(
        kind: ResourceKind,
        path: impl Into<PathBuf>,
        message: impl Into<String>,
    ) -> Self {
        let _ = kind;
        Self {
            diagnostic_type: DiagnosticType::Warning,
            message: message.into(),
            path: path.into(),
            collision: None,
        }
    }

    pub fn error(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self {
            diagnostic_type: DiagnosticType::Error,
            message: message.into(),
            path: path.into(),
            collision: None,
        }
    }

    /// A `collision` diagnostic: `loser` was shadowed by `winner` (first-wins, skills.ts:410-427).
    pub fn collision(
        resource_type: ResourceKind,
        name: impl Into<String>,
        winner_path: impl Into<PathBuf>,
        loser_path: impl Into<PathBuf>,
    ) -> Self {
        let name = name.into();
        let loser = loser_path.into();
        Self {
            diagnostic_type: DiagnosticType::Collision,
            message: format!("name \"{name}\" collision"),
            path: loser.clone(),
            collision: Some(Collision {
                resource_type,
                name,
                winner_path: winner_path.into(),
                loser_path: loser,
            }),
        }
    }
}
