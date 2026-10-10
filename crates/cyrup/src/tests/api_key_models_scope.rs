//! SEAM-147 — `--api-key` and the fresh-session default launch read the RESOLVED model and scope,
//! not whether `--models` was typed.
//!
//! pi @f1b2e77f5, `packages/coding-agent/src/main.ts`:
//!
//! * `:812-815` — `modelPatterns = parsed.models ?? settingsManager.getEnabledModels()`, resolved
//!   by `resolveModelScope` against `modelRuntime.getAvailable()` (`core/model-resolver.ts:364-370`)
//!   only when the list is non-empty;
//! * `:499-519` (`buildSessionOptions`) — with no `--model`, a non-empty scope on a session with no
//!   messages sets `options.model` (the saved default when it is in scope, else the first entry);
//! * `:830-838` — `if (parsed.apiKey) { if (!sessionOptions.model) <error "--api-key requires a
//!   model to be specified via --model, --provider/--model, or --models"> else
//!   setRuntimeApiKey(sessionOptions.model.provider, …) }`, an error diagnostic, so exit 1 at
//!   `:902-908`;
//! * with `options.model` unset, `createAgentSession` falls through to `findInitialModel`
//!   (`core/sdk.ts:234-243`), so `--models ""` or a scope matching nothing still launches on the
//!   saved / provider default.
//!
//! Driven through `main.rs`'s own sequence — `select_provider`, the
//! [`crate::bootstrap::resolve_default_launch_model`] upgrade, [`crate::session_launch::build_factory`]
//! and [`crate::session_launch::launch`] — over a hermetic agent dir holding `anthropic` and
//! `openai` keys and a saved `anthropic` default.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::ops::ControlFlow;
use std::sync::Arc;

use clap::Parser;
use cyrup_config::{AuthStore, ConfigDirs, ModelFile};
use cyrup_sdk::core::ProviderId;
use cyrup_session_svc::AppMode;

use crate::bootstrap::resolve_default_launch_model;
use crate::cli::Cli;
use crate::provider::select_provider;
use crate::session_launch::{Launched, PostBuild, build_factory, launch};
use crate::startup::file_settings_store;

const KEY: &str = "sk-runtime";

struct Fx {
    _tmp: tempfile::TempDir,
    dirs: ConfigDirs,
    anthropic_default: String,
}

/// A hermetic home: stored `anthropic` + `openai` keys, the saved default an `anthropic` model,
/// and `extra` merged into `settings.json`.
fn fixture(extra: serde_json::Value) -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let cwd = root.join("project");
    let agent_dir = root.join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("auth.json"),
        r#"{"anthropic":{"type":"api_key","key":"sk-a"},"openai":{"type":"api_key","key":"sk-o"}}"#,
    )
    .unwrap();
    let anthropic_default = crate::provider::all_available_models(&ModelFile::default())
        .into_iter()
        .find(|m| m.provider.as_str() == "anthropic")
        .expect("the built-in registry carries anthropic")
        .id
        .as_str()
        .to_string();
    let mut settings = serde_json::json!({
        "defaultProvider": "anthropic",
        "defaultModel": anthropic_default,
    });
    if let (Some(into), Some(from)) = (settings.as_object_mut(), extra.as_object()) {
        into.extend(from.clone());
    }
    std::fs::write(agent_dir.join("settings.json"), settings.to_string()).unwrap();
    let dirs = ConfigDirs {
        agent_dir: agent_dir.clone(),
        session_dir: agent_dir.join("sessions"),
        session_dir_explicit: false,
        package_dir: agent_dir.join("packages"),
        cwd,
        home: root.to_path_buf(),
    };
    Fx {
        _tmp: tmp,
        dirs,
        anthropic_default,
    }
}

fn cli(args: &[&str]) -> Cli {
    let mut argv = vec!["cyrup", "--no-session"];
    argv.extend_from_slice(args);
    let mut cli = Cli::try_parse_from(argv).expect("parses");
    cli.normalize_list_flags();
    cli
}

/// `main.rs`'s launch sequence for a print run of `args`; `fresh` is pi's `!hasExistingSession`.
async fn launched(
    fx: &Fx,
    args: &[&str],
    fresh: bool,
) -> (ControlFlow<i32, Launched>, Arc<AuthStore>) {
    let cli = cli(args);
    let models_json = Arc::new(ModelFile::default());
    let auth = Arc::new(AuthStore::at(fx.dirs.agent_dir.join("auth.json")));
    let credentials = || Some(cyrup_config::login::runtime_credentials(auth.clone()));
    let mut provider = select_provider(
        cli.provider.as_deref(),
        cli.model.as_deref(),
        None,
        &models_json,
        credentials(),
    )
    .unwrap();
    let mut config = cli.to_session_config(&fx.dirs, AppMode::Print);
    config.trust_override = Some(true);
    let store = file_settings_store(&fx.dirs);
    if let Some((launch_provider, pattern)) =
        resolve_default_launch_model(&cli, &fx.dirs, &config, &models_json, &store)
    {
        provider = select_provider(
            Some(&launch_provider),
            None,
            None,
            &models_json,
            credentials(),
        )
        .unwrap();
        config.model_pattern = Some(pattern);
    }
    let target = config.target.clone();
    let factory = build_factory(
        provider,
        config,
        store,
        auth.clone(),
        &fx.dirs,
        models_json,
        None,
    )
    .unwrap();
    let flow = launch(
        factory,
        target,
        PostBuild {
            session_name: None,
            cli: &cli,
            fresh,
            require_model: false,
            startup_diagnostics: &[],
            interactive: false,
        },
    )
    .await
    .unwrap();
    (flow, auth)
}

fn session_model(flow: &ControlFlow<i32, Launched>) -> Option<(String, String)> {
    match flow {
        ControlFlow::Continue(l) => l.session.model().map(|m| {
            (
                m.provider.as_str().to_string(),
                m.model.as_str().to_string(),
            )
        }),
        ControlFlow::Break(code) => panic!("the launch stopped with exit {code}"),
    }
}

async fn dispose(flow: ControlFlow<i32, Launched>) {
    if let ControlFlow::Continue(l) = flow {
        l.runtime.dispose().await;
    }
}

/// pi `main.ts:499` + `sdk.ts:234-243`: a supplied `--models` that resolves no scope leaves
/// `options.model` unset, and the session starts on the saved default exactly as with no flag.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_empty_or_unmatched_models_scope_launches_on_the_saved_default() {
    let fx = fixture(serde_json::json!({}));
    let expected = Some(("anthropic".to_string(), fx.anthropic_default.clone()));
    for args in [
        &[][..],
        &["--models", ""][..],
        &["--models", " , "][..],
        &["--models", "nosuchprovider/*"][..],
    ] {
        let (flow, _) = launched(&fx, args, true).await;
        assert_eq!(session_model(&flow), expected, "{args:?}");
        dispose(flow).await;
    }
}

/// pi `main.ts:499-519`: on a fresh session a non-empty scope picks the active model, resolved
/// over every available provider (`getAvailable()`), not only the default's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_models_scope_on_another_provider_picks_the_active_model() {
    let fx = fixture(serde_json::json!({}));
    let (flow, _) = launched(&fx, &["--models", "openai/*"], true).await;
    let (provider, _) = session_model(&flow).expect("a scoped launch has a model");
    assert_eq!(provider, "openai");
    dispose(flow).await;
}

/// pi `main.ts:830-834`: with no `--model` and no scope pick, `--api-key` is an exit-1 error —
/// for an empty `--models`, a scope matching nothing, and a non-empty scope on a resumed session
/// (`!hasExistingSession` is false, so `options.model` stays unset).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_key_without_a_resolved_model_exits_one() {
    let fx = fixture(serde_json::json!({}));
    for (args, fresh) in [
        (&["--api-key", KEY, "--models", ""][..], true),
        (
            &["--api-key", KEY, "--models", "nosuchprovider/*"][..],
            true,
        ),
        (&["--api-key", KEY, "--models", "openai/*"][..], false),
        (&["--api-key", KEY][..], true),
    ] {
        let (flow, auth) = launched(&fx, args, fresh).await;
        assert!(
            matches!(flow, ControlFlow::Break(1)),
            "{args:?} fresh={fresh}: expected exit 1"
        );
        for p in ["anthropic", "openai"] {
            assert_eq!(auth.runtime_api_key(&ProviderId::from(p)), None, "{args:?}");
        }
    }
}

/// pi `main.ts:836-837`: when the scope picks the model, the runtime key goes to THAT model's
/// provider — from `--models`, or from the `enabledModels` setting when `--models` is absent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_key_goes_to_the_scoped_models_provider() {
    let plain = fixture(serde_json::json!({}));
    let enabled = fixture(serde_json::json!({"enabledModels": ["openai/*"]}));
    for (fx, args) in [
        (&plain, &["--api-key", KEY, "--models", "openai/*"][..]),
        (&enabled, &["--api-key", KEY][..]),
    ] {
        let (flow, auth) = launched(fx, args, true).await;
        let (provider, _) = session_model(&flow).expect("a scoped launch has a model");
        assert_eq!(provider, "openai", "{args:?}");
        assert_eq!(
            auth.runtime_api_key(&ProviderId::from("openai")).as_deref(),
            Some(KEY),
            "{args:?}"
        );
        assert_eq!(
            auth.runtime_api_key(&ProviderId::from("anthropic")),
            None,
            "{args:?}"
        );
        dispose(flow).await;
    }
}
