//! Where a manifest lives — pi `parallelHandoffPath` (`parallel-handoff.ts:614-616` @v0.68.0).
//!
//! Upstream folds two shapes into one function via an optional `runId`:
//! `runId ? <base>/handoffs/<runId>.json : <base>/handoff.json`. cyrup splits them, because the
//! two have different owners and neither caller ever wants the other branch:
//!
//! * the ASYNC shape (`<run_dir>/handoff.json`) belongs to the run directory, so it is
//!   [`crate::background::RunDir::handoff`] — the same accessor family as `status()`/`events()`,
//!   and the one place the `handoff.json` literal is spelled (the retention reader used to spell
//!   its own copy);
//! * the FOREGROUND shape lives under the artifacts directory and is [`handoff_manifest_path`]
//!   below.
//!
//! Splitting them makes the async caller's path un-mistakable for the foreground one, which a
//! single `Option<&RunId>` parameter would not.

use std::path::{Path, PathBuf};

use crate::background::RunId;

/// pi `parallelHandoffPath(baseDir, runId)` (`:615`) — `<base>/handoffs/<run_id>.json`.
///
/// The `handoffs/` directory will not exist on a first write; writes therefore go through
/// [`crate::background::atomic::write_atomic_json_creating_parent`], which is what pi's own
/// `writeAtomicJson` does implicitly (`shared/atomic-json.ts:56-58`).
#[must_use]
pub fn handoff_manifest_path(base_dir: &Path, run_id: &RunId) -> PathBuf {
    base_dir
        .join("handoffs")
        .join(format!("{}.json", run_id.as_str()))
}
