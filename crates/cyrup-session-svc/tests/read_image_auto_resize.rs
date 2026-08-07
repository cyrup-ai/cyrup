//! `images.autoResize` must reach the `read` tool (pi `agent-session.ts:2553,2564`).
//!
//! Pi's `_buildRuntime` reads `settingsManager.getImageAutoResize()` and hands it to
//! `createAllToolDefinitions(cwd, { read: { autoResizeImages } })`; `read` forwards it to
//! `processImage`, whose false branch skips `resizeImage` entirely and inlines the NORMALIZED
//! original bytes with no `[Image: original …, displayed at …]` note.
//!
//! cyrup had the setting (`settings.rs::image_auto_resize`) and a settings-panel toggle for it, and
//! NO consumer anywhere: `read` always ran the 2000px downscale. This file asserts the WIRING on the
//! path a user actually hits — a real `SessionBuilder::build()` over a real `FileSettingsStore`,
//! with the faux provider issuing a real `read` tool call, asserted on the `ToolResult` message that
//! lands in the session transcript. A unit test on `ReadOpts` would pass against the unwired build.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine;
use cyrup_core::message::{Content, Message};
use cyrup_core::StopReason;
use cyrup_provider::faux::{
    FauxProvider, FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use cyrup_provider::Provider;
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig};
use serde_json::json;
use tempfile::TempDir;

/// The fixture image is deliberately WIDER than `ReadOpts::max_image_dim` (2000) so the resize path
/// is unambiguously reachable, and a flat gradient so its PNG stays far below the 4.5MB base64 cap —
/// the byte ladder must not be what moves, only the `autoResize` branch.
const FIXTURE_W: u32 = 2600;
const FIXTURE_H: u32 = 800;

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
    /// The raw bytes of the on-disk PNG, i.e. exactly what `autoResize: false` must inline verbatim.
    png_bytes: Vec<u8>,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();

    let img: image::ImageBuffer<image::Rgb<u8>, Vec<u8>> =
        image::ImageBuffer::from_fn(FIXTURE_W, FIXTURE_H, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 241) as u8, 0])
        });
    let path = cwd.join("big.png");
    img.save_with_format(&path, image::ImageFormat::Png).unwrap();
    let png_bytes = std::fs::read(&path).unwrap();

    Fixture { _tmp: tmp, cwd, agent_dir, png_bytes }
}

/// Write `{"images":{"autoResize":<v>}}` into the GLOBAL settings file the session will read.
/// Omitting the key entirely exercises pi's `?? true` default instead.
fn write_setting(fx: &Fixture, value: Option<bool>) {
    let body = match value {
        Some(v) => format!("{{\"images\":{{\"autoResize\":{v}}}}}"),
        None => "{}".to_string(),
    };
    std::fs::write(fx.agent_dir.join("settings.json"), body).unwrap();
}

/// A provider that calls `read` on the fixture image, then stops. Two steps, because the loop needs
/// a terminal turn after the tool result comes back.
fn faux_reading_the_image() -> Arc<FauxProvider> {
    let call = FauxResponseStep::factory(|_ctx, _o, _s, _m| {
        faux_assistant_message(
            vec![faux_tool_call("read", json!({ "path": "big.png" }))],
            StopReason::ToolUse,
        )
    });
    let done = FauxResponseStep::factory(|_ctx, _o, _s, _m| {
        faux_assistant_message(vec![faux_text("looked at it")], StopReason::Stop)
    });
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![call, done]);
    faux
}

async fn session_for(fx: &Fixture) -> AgentSession {
    let provider: Arc<dyn Provider> = faux_reading_the_image() as Arc<dyn Provider>;
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    SessionBuilder::new(provider, cfg)
        .settings_store(Arc::new(cyrup_config::FileSettingsStore::new(
            fx.agent_dir.join("settings.json"),
            fx.cwd.join(".cyrup/settings.json"),
        )))
        .build()
        .await
        .expect("build")
}

/// The `read` tool result as `(note text, image base64)`. Panics loudly rather than skipping — a
/// silent skip branch is how a test ends up green against unfixed code.
async fn read_tool_result(session: &AgentSession) -> (String, String) {
    let messages = session.messages().await;
    let content = messages
        .iter()
        .find_map(|m| match m {
            Message::ToolResult { tool_name, content, .. } if tool_name == "read" => Some(content),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!("the run must contain a `read` tool result; got {messages:#?}");
        });
    let text = content
        .iter()
        .find_map(|c| match c {
            Content::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("`read` always emits its note text; got {content:#?}"));
    let data = content
        .iter()
        .find_map(|c| match c {
            Content::Image { data, .. } => Some(data.clone()),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!("`read` must attach the image block (pi read.ts:255-262); got {content:#?}")
        });
    (text, data)
}

async fn run_read(fx: &Fixture) -> (String, String) {
    let session = session_for(fx).await;
    // The returned stream is the caller's optional event view; the run itself is driven by the
    // session (same idiom as `tests/compaction_tokens_after.rs`), so settle on `wait_for_idle`.
    let _events = session.prompt("look at big.png").await.expect("prompt");
    session.wait_for_idle().await;
    read_tool_result(&session).await
}

/// THE WIRING PROOF. `images.autoResize: false` in the settings file the session loads ⇒ `read`
/// inlines the file's own bytes, unresized and undecorated.
///
/// Before the fix this failed on BOTH assertions: the note carried
/// `[Image: original 2600x800, displayed at 2000x615. Multiply coordinates by 1.30 …]` and the
/// attached base64 was a freshly re-encoded 2000px PNG.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_resize_false_reaches_the_read_tool() {
    let fx = fixture();
    write_setting(&fx, Some(false));
    let (text, data) = run_read(&fx).await;

    assert!(
        !text.contains("displayed at"),
        "autoResize:false skips resizeImage, so `formatDimensionNote` never runs (pi \
         image-process.ts final block). Got note: {text}"
    );
    assert_eq!(
        data,
        base64::engine::general_purpose::STANDARD.encode(&fx.png_bytes),
        "autoResize:false inlines the NORMALIZED original bytes verbatim — a PNG is already a \
         supported inline mime, so normalizeImage is the identity and the attachment must be \
         byte-identical to the file on disk"
    );
}

/// The mirror case, so the test above cannot pass vacuously: with the setting ON (pi's `?? true`
/// default, expressed here by writing an empty settings file) the SAME fixture through the SAME
/// path is downscaled and annotated.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_resize_default_still_downscales() {
    let fx = fixture();
    write_setting(&fx, None);
    let (text, data) = run_read(&fx).await;

    assert!(
        text.contains(&format!("[Image: original {FIXTURE_W}x{FIXTURE_H}, displayed at 2000x")),
        "the default (autoResize on) still runs the 2000px downscale + dimension note. Got: {text}"
    );
    assert_ne!(
        data,
        base64::engine::general_purpose::STANDARD.encode(&fx.png_bytes),
        "a downscaled image is re-encoded, so it cannot equal the source bytes"
    );
}

/// And the setting is read from the settings FILE, not from a compiled-in default: an explicit
/// `true` behaves exactly like the absent key.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_resize_explicit_true_matches_the_default() {
    let fx = fixture();
    write_setting(&fx, Some(true));
    let (text, _) = run_read(&fx).await;
    assert!(text.contains("displayed at 2000x"), "got: {text}");
}
