//! Integration test: [`crate::spawn::SpawnedChild::spawn`] must remove the
//! [`ChildSpawnSpec::temp_files`] it takes ownership of on its OWN failure paths (R-SA-067).
//!
//! Why this needs to exist as a separate file from `spawn/mod.rs`'s inline tests: the two cleanup
//! tests already in `src/spawn/mod.rs` (`finish_cleans_up_temp_files_on_the_success_path`,
//! `terminate_cleans_up_temp_files_on_the_failure_path`) both construct a live `SpawnedChild`
//! first and then exercise a method on it. Neither can reach the constructor's own early returns,
//! which are precisely where the list of temp-file paths used to be dropped un-unlinked — no
//! `SpawnedChild` was ever built, so `finish`/`terminate` were unreachable by construction.
//!
//! The production caller that makes this a real leak is `exec/mod.rs`'s attempt driver:
//!
//! ```ignore
//! let mut child = match SpawnedChild::spawn(plan.spec, &jsonl_path).await {
//!     Ok(child) => child,
//!     Err(err) => { return (AttemptSignal { success: false, error: Some(err.to_string()), .. }, ..); }
//! };
//! ```
//!
//! It converts the `Err` into a failed attempt record and moves on to the next model-fallback
//! attempt — `plan.spec` is gone and nothing downstream ever learns which files it had written.
//! A missing or non-executable `cyrup` binary (the ordinary cause) therefore leaked one task
//! temp file per attempt, forever, into the run's scratch directory.
//!
//! Upstream parity: pi-subagents v0.34.0 `src/runs/foreground/execution.ts` cleans the temp dir on
//! the spawn-error path exactly as it does on the normal close path —
//! `proc.on("error", (error) => { … cleanupTempDir(tempDir); … })`, matching the `cleanupTempDir`
//! call in `proc.on("close", …)`.
//!
//! No mocking: every test below drives the real `SpawnedChild::spawn`, with real files on a real
//! `tempfile::TempDir`, and asserts on the filesystem afterward rather than on any bookkeeping of
//! this crate's own.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::spawn::{ChildSpawnSpec, SpawnCommand, SpawnedChild};

/// Write a task temp file of the shape `ChildSpawnSpec::resolve_task_arg` produces (a real file
/// holding the child's prompt text) and return its path.
fn write_task_temp_file(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(
        &path,
        "the long task prompt that overflowed the argv inline threshold",
    )
    .unwrap();
    assert!(path.exists(), "precondition: the temp file was created");
    path
}

fn spec_with_temp_files(binary: PathBuf, cwd: &Path, temp_files: Vec<PathBuf>) -> ChildSpawnSpec {
    ChildSpawnSpec {
        command: SpawnCommand {
            binary,
            base_args: Vec::new(),
        },
        args: vec![
            "--print".to_string(),
            "--mode".to_string(),
            "json".to_string(),
        ],
        task_arg: format!("@{}", temp_files[0].display()),
        env_overlay: HashMap::new(),
        cwd: cwd.to_path_buf(),
        temp_files,
    }
}

/// The `command.spawn()` failure path — a binary that does not exist, which is exactly what a
/// mis-resolved `CYRUP_SUBAGENT_BINARY`/missing `cyrup` on `PATH` produces in production.
#[tokio::test]
async fn spawn_failure_removes_the_specs_temp_files() {
    let dir = tempfile::tempdir().unwrap();
    let temp_a = write_task_temp_file(dir.path(), "subagent-task-a.txt");
    let temp_b = write_task_temp_file(dir.path(), "subagent-prompt-override-b.txt");

    let missing_binary = dir.path().join("definitely-not-an-executable-binary");
    assert!(
        !missing_binary.exists(),
        "precondition: the binary is absent"
    );

    let spec = spec_with_temp_files(
        missing_binary,
        dir.path(),
        vec![temp_a.clone(), temp_b.clone()],
    );
    let jsonl_path = dir.path().join("attempt-0.jsonl");

    let err = SpawnedChild::spawn(spec, &jsonl_path)
        .await
        .err()
        .expect("spawning a non-existent binary must fail");
    // Sanity: this is the spawn failure we meant to exercise, not some other error.
    assert!(
        err.to_string().to_lowercase().contains("no such file")
            || err.to_string().to_lowercase().contains("not found"),
        "expected an ENOENT-shaped spawn failure, got: {err}"
    );

    assert!(
        !temp_a.exists(),
        "R-SA-067: {} leaked after the spawn failure",
        temp_a.display()
    );
    assert!(
        !temp_b.exists(),
        "R-SA-067: {} leaked after the spawn failure",
        temp_b.display()
    );
}

/// The `.jsonl`-tee creation failure path — the child DID spawn, but the artifact writer could not
/// be created (here: its parent directory does not exist). The temp files must still be unlinked,
/// and the now-unreachable child must not be left orphaned.
#[tokio::test]
async fn jsonl_writer_failure_removes_the_specs_temp_files() {
    let dir = tempfile::tempdir().unwrap();
    let temp_a = write_task_temp_file(dir.path(), "subagent-task-c.txt");

    // A real, always-present binary that exits immediately regardless of its argv, so this test
    // exercises the post-spawn failure path without depending on any fixture behavior.
    let binary = ["/bin/true", "/usr/bin/true"]
        .into_iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
        .expect("a `true` binary must exist on this platform");

    let spec = spec_with_temp_files(binary, dir.path(), vec![temp_a.clone()]);
    // Parent directory deliberately absent -> `BoundedJsonlWriter::create` fails.
    let jsonl_path = dir.path().join("no-such-subdir").join("attempt-0.jsonl");
    assert!(
        !jsonl_path.parent().unwrap().exists(),
        "precondition: the jsonl parent directory is absent"
    );

    let err = SpawnedChild::spawn(spec, &jsonl_path)
        .await
        .err()
        .expect("creating the jsonl tee under a missing directory must fail");
    assert!(
        !err.to_string().is_empty(),
        "the failure must carry a message"
    );

    assert!(
        !temp_a.exists(),
        "R-SA-067: {} leaked after the jsonl-writer failure",
        temp_a.display()
    );
}
