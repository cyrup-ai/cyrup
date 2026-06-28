//! cyrup-resources — skills, prompt templates, themes, packages (arch-09; conformance: func-09).
//!
//! Agent Skills (`SKILL.md`) discovery, prompt templates, themes (hot-reload), and the package
//! model (`cyrup.toml` native manifest + git/registry install).
//!
//! Scaffold stub.

/// Resource/package error (arch-09 §8). Scaffold placeholder.
#[derive(Debug, thiserror::Error)]
pub enum ResourceError {
    #[error("not yet implemented: {0}")]
    Unimplemented(&'static str),
}
