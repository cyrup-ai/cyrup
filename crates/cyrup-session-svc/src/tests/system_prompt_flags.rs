//! PROMPT-001 — `--system-prompt` and `--append-system-prompt` reach the provider request.
//!
//! The session writes its prompt as `sections` on a system row and builds the agent with no prompt
//! of its own (`CODE-014`), so what the model reads is the replay of the transcript. A request that
//! carried an empty `Context::system_prompt` (the regression) passed every test that replayed the
//! transcript itself. These tests read the field the adapters read, on the request the provider
//! actually received, through the real `SessionBuilder`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::{Arc, Mutex};

use crate::{SessionBuilder, SessionConfig};
use cyrup_core::StopReason;
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, FauxResponseStep, faux_assistant_message, faux_text};
use tempfile::TempDir;

struct Sent {
    prompt: Option<String>,
    system_rows: usize,
}

/// A faux provider answering `n` times and recording the prompt of every request it receives.
fn recording(n: usize) -> (Arc<FauxProvider>, Arc<Mutex<Vec<Sent>>>) {
    let seen: Arc<Mutex<Vec<Sent>>> = Arc::new(Mutex::new(Vec::new()));
    let steps: Vec<FauxResponseStep> = (0..n)
        .map(|_| {
            let seen = Arc::clone(&seen);
            FauxResponseStep::factory(move |ctx, _opts, _state, _model| {
                seen.lock().unwrap().push(Sent {
                    prompt: ctx.system_prompt.clone(),
                    system_rows: ctx
                        .messages
                        .iter()
                        .filter(|m| matches!(m, cyrup_core::Message::System(_)))
                        .count(),
                });
                faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)
            })
        })
        .collect();
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(steps);
    (faux, seen)
}

fn config(tmp: &TempDir) -> SessionConfig {
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    cfg
}

async fn two_prompts(cfg: SessionConfig, faux: Arc<FauxProvider>) {
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, cfg).build().await.unwrap();
    for text in ["first", "second"] {
        let _ = session.prompt(text).await.unwrap();
        session.wait_for_idle().await;
    }
}

/// With no flags the request still carries the whole built prompt, on the first request and the
/// second, and it is never empty.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_built_prompt_reaches_every_request() {
    let tmp = TempDir::new().unwrap();
    let (faux, seen) = recording(2);
    two_prompts(config(&tmp), faux).await;

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    for (n, sent) in seen.iter().enumerate() {
        let prompt = sent.prompt.as_deref().unwrap_or_default();
        assert!(prompt.contains("<tools>"), "request {n}: {prompt:?}");
        assert!(prompt.contains("<rules>"), "request {n}: {prompt:?}");
        assert_eq!(
            sent.system_rows, 0,
            "request {n}: rows are folded into the prompt"
        );
    }
    assert_eq!(
        seen[0].prompt, seen[1].prompt,
        "nothing changed between the two prompts"
    );
}

/// `--system-prompt` replaces the built prompt and `--append-system-prompt` follows it; each
/// reaches the provider exactly once, on every request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn system_prompt_flags_reach_every_request_once() {
    let tmp = TempDir::new().unwrap();
    let mut cfg = config(&tmp);
    cfg.system_prompt = Some("SENTINEL-REPLACEMENT-PROMPT".to_string());
    cfg.append_system_prompt = Some("APPENDED-SENTINEL-TEXT".to_string());
    let (faux, seen) = recording(2);
    two_prompts(cfg, faux).await;

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    for (n, sent) in seen.iter().enumerate() {
        let prompt = sent.prompt.as_deref().unwrap_or_default();
        assert_eq!(
            prompt.matches("SENTINEL-REPLACEMENT-PROMPT").count(),
            1,
            "request {n}: {prompt:?}"
        );
        assert_eq!(
            prompt.matches("APPENDED-SENTINEL-TEXT").count(),
            1,
            "request {n}: {prompt:?}"
        );
        // A custom prompt replaces the preamble, tools and rules sections, and keeps the addendum
        // and the working directory (`buildSystemPromptSections`, custom-prompt branch).
        assert!(prompt.contains("<addendum>"), "request {n}: {prompt:?}");
        assert!(prompt.contains("<cwd>"), "request {n}: {prompt:?}");
        assert!(!prompt.contains("<tools>"), "request {n}: {prompt:?}");
    }
}
