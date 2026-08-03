//! `cyrup --list-models` must render the persisted runtime catalog overlay (DRIFT-007).
//!
//! This is DRIFT-007's own acceptance criterion at the BINARY seam: a model present only in the
//! pi.dev overlay that a previous run cached must appear in the listing.
//!
//! Pi gets this for free from ordering. The cache-only restore is part of runtime CREATION —
//! `agent-session-services.ts:180` `await modelRuntime.refresh({ allowNetwork: false })`, reached
//! from `createAgentSessionRuntime` at `main.ts:793` — and the `--list-models` exit is downstream of
//! it at `main.ts:816`. The NETWORK refresh is downstream of the exit instead (`main.ts:863-866`),
//! so `pi --list-models` shows the cached overlay and issues no request. cyrup must land in the same
//! place: the disk-only restore before the early return, the detached revalidation after it.
//!
//! **No network.** The store is written directly, exactly as a completed refresh would have left it,
//! and the child runs with `CYRUP_OFFLINE=1` so the revalidation phase is gated off in any case. The
//! `--list-models` path itself never opens a socket.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

/// A well-formed remote catalog entry for `groq` (a provider that ships an embedded catalog, so the
/// overlay is provably an ADDITION to it rather than a listing that only exists because of it).
fn groq_model_json(id: &str, context_window: u64) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": id,
        "api": "openai-completions",
        "provider": "groq",
        "baseUrl": "https://api.groq.com/openai/v1",
        "reasoning": false,
        "input": ["text"],
        "cost": {"input": 1.0, "output": 2.0, "cacheRead": 0.0, "cacheWrite": 0.0},
        "contextWindow": context_window,
        "maxTokens": 8192
    })
}

/// Write `<agent_dir>/models-store.json` the way a completed background refresh would, with a
/// `lastModified` strictly newer than the built-in catalog manifest so the staleness guard keeps it.
fn seed_store(agent_dir: &Path, models: Vec<serde_json::Value>) {
    let newer = cyrup_provider::builtin_model_data_generated_at().unwrap() + 1;
    let entry = serde_json::json!({
        "groq": {
            "models": models,
            "lastModified": newer,
            "checkedAt": newer,
            "etag": "\"v1\""
        }
    });
    std::fs::write(
        agent_dir.join(cyrup_config::models_store::MODELS_STORE_FILE_NAME),
        serde_json::to_string_pretty(&entry).unwrap(),
    )
    .unwrap();
}

struct Fx {
    _tmp: TempDir,
    cwd: std::path::PathBuf,
    agent_dir: std::path::PathBuf,
}

fn fixture() -> Fx {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fx {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

/// Run the REAL binary. `CYRUP_OFFLINE=1` guarantees the revalidation phase cannot reach the
/// network; the temp `CYRUP_AGENT_DIR` isolates the store, settings and auth from the developer's.
fn list_models(fx: &Fx, search: &str) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cyrup"));
    cmd.arg("--list-models")
        .arg(search)
        .current_dir(&fx.cwd)
        .env("CYRUP_AGENT_DIR", &fx.agent_dir)
        .env("CYRUP_OFFLINE", "1")
        .env_remove("PI_CODING_AGENT_DIR")
        .env_remove("PI_OFFLINE");
    let out = cmd.output().expect("the cyrup binary runs");
    assert!(
        out.status.success(),
        "--list-models exited {:?}: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// DRIFT-007 acceptance: a model present only in the remote overlay appears in `--list-models`.
#[test]
fn list_models_renders_the_persisted_remote_overlay() {
    let fx = fixture();

    // Cold: the model does not exist anywhere in the embedded catalogs.
    let cold = list_models(&fx, "overlay-only-model");
    assert!(
        cold.contains("No models matching"),
        "sanity: the remote-only model must not be embedded, got:\n{cold}"
    );

    // A background refresh completes and writes the cache...
    seed_store(&fx.agent_dir, vec![groq_model_json("overlay-only-model", 777_777)]);

    // ...and the very next `--list-models` renders it, under its own provider.
    let warm = list_models(&fx, "overlay-only-model");
    assert!(
        warm.lines()
            .any(|l| l.contains("groq") && l.contains("overlay-only-model")),
        "the persisted overlay did not reach --list-models, got:\n{warm}"
    );
}

/// The overlay is an ADDITION, never a replacement: seeding a one-model `groq` overlay must not
/// shrink the listing. This is the floor invariant proved at the binary seam.
#[test]
fn the_overlay_never_shrinks_the_listing() {
    let fx = fixture();
    let before = list_models(&fx, "");
    let before_lines = before.lines().count();
    assert!(before_lines > 10, "the embedded catalogs list many models");

    seed_store(&fx.agent_dir, vec![groq_model_json("overlay-only-model", 777_777)]);

    let after = list_models(&fx, "");
    assert_eq!(
        after.lines().count(),
        before_lines + 1,
        "a one-model overlay must ADD exactly one row, never replace the groq catalog"
    );
    for line in before.lines() {
        assert!(
            after.contains(line),
            "the overlay removed an embedded listing row: {line}"
        );
    }
}
