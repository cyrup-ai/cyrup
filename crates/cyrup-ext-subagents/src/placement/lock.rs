//! `withHerdrPaneAllocationLock(key, action)` (`src/runs/shared/herdr-placed-run.ts:60-110`
//! @v0.68.0): two launches onto the same machine, session and cwd must not both decide "there is
//! no owned workspace yet" and create two. The decision and the create run under one exclusive,
//! cross-process lock keyed by `[target, session, cwd]`.
//!
//! **`[CYRUP-DELTA]` — the mechanism, not the contract.** Upstream builds its lock out of
//! `link(2)` publication, a JSON owner record, pid liveness and a recovery file, because node has
//! no advisory lock. Rust has one: [`std::fs::File::try_lock`] (`flock(2)` on unix), which the
//! kernel releases when the holder dies, so the whole stale-owner recovery protocol collapses to
//! "the lock is free". Kept from upstream: the private mode-0700 directory under the agent dir,
//! the sha-256 file name, the 10 s deadline and 25 ms poll, and the timeout sentence.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sha2::Digest;

/// `HERDR_ALLOCATION_LOCK_TIMEOUT_MS` (`herdr-placed-run.ts:57`).
pub const ALLOCATION_LOCK_TIMEOUT: Duration = Duration::from_secs(10);
/// `HERDR_ALLOCATION_LOCK_POLL_MS` (`herdr-placed-run.ts:57`).
pub const ALLOCATION_LOCK_POLL: Duration = Duration::from_millis(25);

/// `herdrPaneAllocationKey(target, session, cwd)` (`herdr-placed-run.ts:58`) —
/// `JSON.stringify([target, session, cwd])`.
#[must_use]
pub fn pane_allocation_key(target: &str, session: Option<&str>, cwd: &str) -> String {
    serde_json::json!([target, session, cwd]).to_string()
}

/// `<agent dir>/subagents/herdr-allocation-locks`, each level created mode 0700.
fn lock_root(agent_dir: &Path) -> std::io::Result<PathBuf> {
    let root = agent_dir.join("subagents").join("herdr-allocation-locks");
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&root)?;
    Ok(root)
}

/// A held allocation lock. Dropping it releases the lock (and the kernel does, if the holder
/// dies first).
#[derive(Debug)]
pub struct AllocationLock {
    file: std::fs::File,
}

impl Drop for AllocationLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Take the allocation lock for `key`, polling until [`ALLOCATION_LOCK_TIMEOUT`].
///
/// # Errors
/// `"Timed out waiting for the Herdr pane allocation lock."`, or the I/O failure creating it.
pub async fn acquire_pane_allocation_lock(
    agent_dir: &Path,
    key: &str,
) -> Result<AllocationLock, String> {
    let root = lock_root(agent_dir)
        .map_err(|error| format!("Herdr allocation lock directory is unusable: {error}"))?;
    let digest = sha2::Sha256::digest(key.as_bytes());
    let name: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    let path = root.join(format!("{name}.lock"));
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(false).write(true).read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(&path)
        .map_err(|error| format!("Herdr allocation lock is unusable: {error}"))?;
    let deadline = Instant::now() + ALLOCATION_LOCK_TIMEOUT;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(AllocationLock { file }),
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(format!("Herdr allocation lock is unusable: {error}"));
            }
        }
        if Instant::now() >= deadline {
            return Err("Timed out waiting for the Herdr pane allocation lock.".to_string());
        }
        tokio::time::sleep(ALLOCATION_LOCK_POLL).await;
    }
}
