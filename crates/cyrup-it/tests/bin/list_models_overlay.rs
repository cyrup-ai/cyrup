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
    home: std::path::PathBuf,
}

/// A project + agent dir with a **stored `groq` API key**.
///
/// The credential is not decoration. `--list-models` renders `modelRuntime.getAvailable()` — the
/// models whose provider has a RESOLVABLE credential (pi `cli/list-models.ts:35` @v0.83.0) — so a
/// `groq` overlay row can only ever appear if `groq` is configured. This fixture used to write no
/// credential at all, which made the whole file pass or fail on whether the developer happened to
/// export `GROQ_API_KEY`: it was green on the authoring machine and red here, with the overlay row
/// silently filtered out and the product blamed. That is exactly the ambient-credential dependence
/// docs/TEST-ARCHITECTURE.md §4 R5 exists to forbid, and it is why [`list_models`] below is
/// hermetic — with an inherited environment the developer's `HF_TOKEN` alone moves the baseline row
/// count from 8 to 50.
fn fixture() -> Fx {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        agent_dir.join("auth.json"),
        serde_json::to_string(&serde_json::json!({
            "groq": { "type": "api_key", "key": "sk-groq-fixture" }
        }))
        .unwrap(),
    )
    .unwrap();
    Fx {
        _tmp: tmp,
        cwd,
        agent_dir,
        home,
    }
}

/// Run the REAL binary with **no inherited environment** beyond the tiny allowlist
/// (`support::env::hermetic`). `CYRUP_OFFLINE=1` guarantees the revalidation phase cannot reach the
/// network; the temp `CYRUP_AGENT_DIR` isolates the store, settings and auth from the developer's;
/// and clearing the environment is what makes the row counts below a property of the fixture rather
/// than of whichever provider keys happen to be exported on this machine.
fn list_models(fx: &Fx, search: &str) -> String {
    let mut cmd = crate::support::env::hermetic(crate::support::bins::cyrup(), &fx.home);
    cmd.arg("--list-models")
        .arg(search)
        .current_dir(&fx.cwd)
        .env("CYRUP_AGENT_DIR", &fx.agent_dir)
        .env("CYRUP_OFFLINE", "1");
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
    seed_store(
        &fx.agent_dir,
        vec![groq_model_json("overlay-only-model", 777_777)],
    );

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
    // The hermetic fixture configures exactly ONE provider, so the baseline is groq's embedded
    // catalog plus the header row — a handful, not the ~50 an inherited environment produced by
    // pulling in whatever else the developer has a key for. Assert the floor is a real listing
    // (header + several rows) so a baseline that collapsed to "No models available" cannot make the
    // `before + 1` comparison below vacuously true.
    assert!(
        before_lines > 3
            && before
                .lines()
                .next()
                .is_some_and(|l| l.starts_with("provider")),
        "baseline must be a real groq listing with a header; got {before_lines} lines:\n{before}"
    );
    assert!(
        before.lines().skip(1).all(|l| l.starts_with("groq")),
        "only the configured provider may appear, or the baseline is env-dependent again:\n{before}"
    );

    seed_store(
        &fx.agent_dir,
        vec![groq_model_json("overlay-only-model", 777_777)],
    );

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
