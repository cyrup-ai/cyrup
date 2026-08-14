//! Helpers shared by this target's modules, de-duplicated as the 18 files landed.
//!
//! Every item below was lifted VERBATIM from the migrated files after checking the copies were
//! byte-identical (md5 over the extracted block, per file). Nothing here was rewritten, retuned or
//! merged from differing variants — where variants differed they stayed module-local, so this file
//! carries no behavioural change of its own:
//!
//! | item                     | identical copies collapsed | left behind, and why                                    |
//! |--------------------------|----------------------------|---------------------------------------------------------|
//! | [`Broker`]               | 6                          | —                                                        |
//! | [`registration`]         | 9                          | the 4 `protocol_*` no-arg `registration()`s take no name |
//! | [`spawn_broker`]         | 6                          | `broker_runtime_claim`'s `spawn_broker_capturing` pipes  |
//! | [`within`]               | 5                          | `dismiss_incoming_ask`'s `wait_until` polls at 20ms      |
//! | [`write_broker_command`] | 4                          | `reconnect`'s takes an explicit `command` path           |
//!
//! The six `RawClient`s and the four `HostileBroker`s are deliberately NOT here: their method sets
//! genuinely differ per file (six distinct md5s and three distinct md5s respectively), so unifying
//! them would mean merging behaviour, not relocating it. See the `unresolved` note from the
//! migration.
//!
//! This module lives in `tests/intercom/`, not in `tests/support/`, because everything in it is
//! about the intercom broker seam specifically — `tests/support/mod.rs` states the rule.

// Not every module uses every helper; the compiler cannot see that a sibling module does.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use cyrup_intercom::config::config_path;
use cyrup_intercom::transport::protocol::{SessionRegistration, now_ms};
use cyrup_intercom::transport::spawn::wait_for_broker;

/// The real `cyrup-intercom-broker` binary.
///
/// Was `PathBuf::from(env!("CARGO_BIN_EXE_cyrup-intercom-broker"))` in every one of these files.
/// That `env!` is set only for test targets in the SAME package as the binary, so it does not
/// compile here; `build.rs` resolves the path instead and `support::bins` owns the env-var name.
pub fn broker_bin() -> PathBuf {
    crate::support::bins::intercom_broker()
}

/// A live broker child process + its socket path. Killed on drop.
pub struct Broker {
    pub _dir: tempfile::TempDir,
    pub socket: PathBuf,
    pub child: tokio::process::Child,
}

impl Broker {
    pub async fn start() -> Self {
        let bin = broker_bin();
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("intercom").join("broker.sock");
        let child = tokio::process::Command::new(&bin)
            .env("CYRUP_CODING_AGENT_DIR", dir.path())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn the real intercom broker subprocess");
        wait_for_broker(&socket, Duration::from_secs(5)).await.expect("broker is health-connectable");
        Self { _dir: dir, socket, child }
    }
}

impl Drop for Broker {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

pub fn registration(name: &str) -> SessionRegistration {
    SessionRegistration {
        name: Some(name.to_string()),
        cwd: "/tmp/work".to_string(),
        model: "test-model".to_string(),
        pid: std::process::id().into(),
        started_at: now_ms().into(),
        last_activity: now_ms().into(),
        status: None,
        extra: Default::default(),
    }
}

/// A long-lived broker: stdio discarded, so a pipe nobody drains can never stall it.
pub fn spawn_broker(agent_dir: &Path) -> tokio::process::Child {
    tokio::process::Command::new(broker_bin())
        .env("CYRUP_CODING_AGENT_DIR", agent_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn the real intercom broker subprocess")
}

/// Poll `predicate` until it holds or `budget` elapses.
pub async fn within<F: FnMut() -> bool>(budget: Duration, mut predicate: F) -> bool {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        if predicate() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// `ensure_broker` (pi `spawnBrokerIfNeeded` inside `ensureConnected`, `index.ts:828`) launches a
/// genuine broker instead of re-execing the test harness. This is also what makes the test robust
/// under CPU contention: even if the 1 s health probe on the pre-spawned broker times out, the
/// fallback spawn produces a working broker rather than a spurious failure.
pub fn write_broker_command(intercom_dir: &Path) {
    std::fs::create_dir_all(intercom_dir).expect("create intercom dir");
    let body = serde_json::json!({
        "brokerCommand": broker_bin().to_string_lossy(),
        "brokerArgs": [],
    });
    std::fs::write(config_path(intercom_dir), serde_json::to_string(&body).expect("serialize config"))
        .expect("write config.json");
}
