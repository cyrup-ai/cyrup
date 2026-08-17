//! cyrup-flux — Flux: a structured, file-persisted AI development pipeline
//! (`new → ask → split → aug → exec → qa → tests → commit → create-pr`), ported from
//! code-puppy's `flux_bootstrap` plugin onto cyrup as a single
//! `cyrup_ext::native::NativeExtension`.
//!
//! Two mechanisms, both cyrup-native replacements for upstream machinery:
//!   * the 15 pipeline commands are cyrup PROMPT TEMPLATES bundled in `resources/prompts/flux/`
//!     and contributed to every session via `ResourcesDiscover` — replacing code-puppy's
//!     `customizable_commands` dispatcher and its `installer.py` copy-into-`~/.code_puppy` step;
//!   * `status` / `cheatsheet` / `about` are NATIVE COMMANDS — replacing code-puppy's
//!     `exec:` frontmatter directive, which cyrup deliberately does not support.
//!
//! State on disk (`~/.flux/<flattened-cwd>/`) stays byte-identical to code-puppy's so one
//! project's task tree is readable by both harnesses.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
#![forbid(unsafe_code)]

pub mod ask_tool;
pub mod extension;
pub mod overlay;
pub mod render_about;
pub mod render_cheatsheet;
pub mod render_status;
pub mod resources;
pub mod state;

use std::sync::{Arc, OnceLock};

/// The subagent-child signal, mirroring
/// `cyrup_ext_subagents::spawn::nested_events::CHILD_ENV` (`nested_events.rs:35`). Declared
/// locally rather than imported: this crate needs one `&str`, not a dependency on the
/// workspace's largest crate.
const SUBAGENT_CHILD_ENV: &str = "CYRUP_SUBAGENT_CHILD";

/// Construct the flux extension.
#[must_use]
pub fn flux_extension() -> Arc<extension::FluxExtension> {
    Arc::new(extension::FluxExtension {
        id: cyrup_core::ExtensionId::from("cyrup-flux"),
        host_services: Arc::new(OnceLock::new()),
    })
}

/// `Some(flux_extension())` at the top level; `None` inside a subagent CHILD (a child re-execs
/// this binary in Print/Json mode, and contributing 15 templates plus a skill to every child
/// would put the skill into every child's system prompt for a pipeline the child is not
/// running).
#[must_use]
pub fn flux_extension_for_env() -> Option<Arc<extension::FluxExtension>> {
    if std::env::var(SUBAGENT_CHILD_ENV).ok().as_deref() == Some("1") {
        return None;
    }
    Some(flux_extension())
}
