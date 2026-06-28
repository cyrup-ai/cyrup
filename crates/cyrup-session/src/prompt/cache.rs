//! Session-scoped context cache (arch-06 §3.4/§6.3, R-06-016/017).
//!
//! Context files + skill pointers are read **once** per session into an [`arc_swap::ArcSwap`]
//! [`ContextSnapshot`]; the per-turn build path reads it lock-free. Re-read happens only on session
//! start or `/reload` via [`ContextStore::reload`], which runs the owned blocking
//! [`ContextFileLoader`] off the reactor through `tokio::task::spawn_blocking` and races the work
//! against cancellation.

use std::sync::Arc;

use arc_swap::ArcSwap;
use cyrup_core::RunCancel;
use cyrup_resources::SkillPointer;

use super::context_files::{ContextDiagnostic, ContextFile, ContextFileLoader};
use super::overrides::ResolvedOverride;

/// Errors surfaced by context reload. Per-file issues are non-fatal diagnostics, not errors.
#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    #[error("cancelled")]
    Cancelled,
    #[error("reading skill pointers: {0}")]
    Resources(#[from] cyrup_resources::ResourceError),
    #[error("resolving override sources: {0}")]
    Config(#[from] cyrup_config::ConfigError),
    #[error(transparent)]
    Core(#[from] cyrup_core::CoreError),
}

/// Immutable per-session view, swapped atomically on reload.
#[derive(Clone, Debug, Default)]
pub struct ContextSnapshot {
    pub context_files: Arc<[ContextFile]>,
    pub skills: Arc<[SkillPointer]>,
    pub override_source: ResolvedOverride,
    pub diagnostics: Arc<[ContextDiagnostic]>,
}

/// Session-scoped cache holding the latest [`ContextSnapshot`].
pub struct ContextStore {
    snapshot: ArcSwap<ContextSnapshot>,
}

impl Default for ContextStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ContextStore {
    /// An empty store (no context files / skills) — the pre-`reload` state.
    pub fn new() -> Self {
        Self { snapshot: ArcSwap::from_pointee(ContextSnapshot::default()) }
    }

    /// Seed the store with a pre-built snapshot (e.g. in tests / restore).
    pub fn from_snapshot(snap: ContextSnapshot) -> Self {
        Self { snapshot: ArcSwap::from_pointee(snap) }
    }

    /// Lock-free read of the current snapshot (the build hot path).
    pub fn snapshot(&self) -> Arc<ContextSnapshot> {
        self.snapshot.load_full()
    }

    /// Atomically publish a new snapshot.
    pub fn store(&self, snap: ContextSnapshot) {
        self.snapshot.store(Arc::new(snap));
    }

    /// `/reload` or session start: re-discover context files off the reactor and atomically swap
    /// (R-06-016). The `skills` + `override_source` are resolved by the caller (arch-09/arch-07)
    /// and passed in so this layer never reaches into those crates' discovery.
    ///
    /// Cancellation aborts before any swap (`ContextError::Cancelled`), never a partial publish.
    pub async fn reload(
        &self,
        cancel: &RunCancel,
        loader: ContextFileLoader,
        skills: Arc<[SkillPointer]>,
        override_source: ResolvedOverride,
    ) -> Result<(), ContextError> {
        let load = async move {
            // Owned + BLOCKING discovery off the reactor (arch-00 §5).
            let (files, diags) = tokio::task::spawn_blocking(move || loader.load())
                .await
                .map_err(|_| ContextError::Cancelled)?;
            Ok::<ContextSnapshot, ContextError>(ContextSnapshot {
                context_files: Arc::from(files),
                skills,
                override_source,
                diagnostics: Arc::from(diags),
            })
        };
        let snap = cancel.run_until(load).await.ok_or(ContextError::Cancelled)??;
        self.snapshot.store(Arc::new(snap)); // atomic publish (R-06-016)
        Ok(())
    }
}
