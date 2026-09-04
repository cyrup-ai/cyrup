//! The pieces the two cross-process forwarding proofs
//! ([`super::forwarding_subprocess`] and [`super::forwarding_spawn_env`]) genuinely shared, byte for
//! byte, as two separate integration binaries.
//!
//! Collapsing them is the migration note this target's `main.rs` carries: the crate shipped TWO
//! copies of `wait_child` whose only difference was a poll cadence, which is exactly the shape open
//! item PERM-022 came out of. What is NOT collapsed is `spawn_child` — the two differ substantively
//! (one hand-arms the child env, the other takes it from the production spawn planner, which is the
//! whole point of PERM-001) and each keeps its own.
//!
//! ## The poll cadence, chosen rather than averaged
//!
//! `forwarding_subprocess.rs` polled every 40 ms, `forwarding_spawn_env.rs` every 25 ms. This takes
//! **25 ms**, the faster of the two, deliberately: the cadence is only the granularity at which a
//! finished child is noticed, it is bounded above by each caller's own `overall` wall clock (which is
//! passed in and NOT collapsed — 30 s and 40 s call sites keep their values), and a shorter poll can
//! only ever observe an exit sooner. Nothing asserts on it. It is not an average of 40 and 25.
//!
//! The child-side wait bounds (`child_wait_ms`: 20_000 / 8_000 / 1_200) are arguments at the call
//! sites and are untouched here — those ARE load-bearing, the 1_200 one especially, since
//! `forwarded_timeout_fail_closes_the_child` asserts the child really waited on it.

use std::path::Path;
use std::process::Child;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use cyrup_ext::{DialogOptions, HostServices, HumanInteractionLock, NotifyKind};
use serde_json::Value;

// ---- child exit codes (identical in both files before the migration) ----
/// The child's gate let the tool through.
pub const EXIT_ALLOWED: i32 = 0;
/// The child's gate blocked the tool (forwarded deny, or a fail-closed timeout).
pub const EXIT_BLOCKED: i32 = 3;
/// A test-setup failure in the child — never a decision.
pub const EXIT_UNEXPECTED: i32 = 4;

/// A scripted `HostServices` standing in for the parent's live TUI/RPC renderer: `session_id()` is the
/// parent inbox id the watcher addresses, `select()` returns a fixed dialog answer (and counts calls,
/// so a test can prove the watcher actually surfaced a forwarded prompt), and `human_interaction_lock()`
/// hands back the ONE session lock the forwarding decision acquires (C3).
pub struct ScriptedHost {
    pub session_id: String,
    pub answer: String,
    pub selects: Arc<AtomicUsize>,
    pub lock: Arc<HumanInteractionLock>,
}

impl HostServices for ScriptedHost {
    fn session_id(&self) -> Option<String> {
        Some(self.session_id.clone())
    }
    fn select(&self, _prompt: &str, _options: &Value, _opts: &DialogOptions) -> Option<String> {
        self.selects.fetch_add(1, Ordering::SeqCst);
        Some(self.answer.clone())
    }
    fn human_interaction_lock(&self) -> Option<Arc<HumanInteractionLock>> {
        Some(self.lock.clone())
    }
    fn notify(&self, _message: &str, _kind: NotifyKind) {}
}

/// Empty-tool default is ASK; make bash explicitly ASK so a `bash` call forwards.
pub fn write_policy(agent_dir: &Path) {
    std::fs::write(
        agent_dir.join("cyrup-permissions.jsonc"),
        r#"{ "bash": { "*": "ask" } }"#,
    )
    .expect("write policy");
}

/// Poll a spawned child to completion under an overall wall-clock bound (kills + returns `None` if it
/// overruns, so a test never hangs).
pub async fn wait_child(mut child: Child, overall: Duration) -> Option<i32> {
    let deadline = Instant::now() + overall;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.code(),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(_) => return None,
        }
    }
}
