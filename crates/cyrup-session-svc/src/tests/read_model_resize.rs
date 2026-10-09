//! The `read` tool resizes against the ACTIVE model's `inputLimits.images.resize` (PROV-134's
//! read-tool clause, and CFG-085's Verify line).
//!
//! Pi reads the profile PER CALL off the context it threads into every tool:
//! `resizeOptions: ctx?.model?.inputLimits?.images?.resize ?? fallbackResizeOptions`
//! (`core/tools/read.ts:137-138` @v1.0.4, re-read via `git -C tmp/pi show`), and `ctx.model` is the
//! agent's live state model — so a mid-session `/model` switch changes the very next `read`.
//!
//! cyrup's `read` resolved the shared 2000px default for every model: the profile existed on the
//! catalog row and in a user `models.json`, and the one tool whose whole job is putting images into
//! conversation history could not see it. `ReadOpts::image_resize` alone would not have fixed it —
//! it is baked at construction, so a `/model` switch would keep the startup model's profile — which
//! is why this goes through `ModelResizeHandle`, the twin of the `ModelVisionHandle` the non-vision
//! warning already uses.
//!
//! Asserted by DECODING the `read` result's image block and measuring its real pixels, through a
//! real `SessionBuilder::build()` with a real settings store and a faux provider issuing a real
//! `read` call — a unit test on `ReadOpts` would pass against the unwired build, which is the bug.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::Arc;

use crate::{AgentSession, SessionBuilder, SessionConfig};
use base64::Engine;
use cyrup_core::message::{Content, Message};
use cyrup_core::{ModelId, ProviderId, StopReason};
use cyrup_provider::faux::{
    FauxConfig, FauxModelDefinition, FauxProvider, FauxResponseStep, faux_assistant_message,
    faux_text, faux_tool_call,
};
use cyrup_provider::{ModelImageInputLimits, ModelImageResizeOptions, ModelInputLimits, Provider};
use serde_json::json;
use tempfile::TempDir;

/// Well above both profiles under test, so whichever one binds is unambiguous.
const FIXTURE_W: u32 = 2600;
const FIXTURE_H: u32 = 1800;

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    // `{}` leaves `images.autoResize` at pi's `?? true` default, so the resize branch runs.
    std::fs::write(agent_dir.join("settings.json"), "{}").unwrap();

    let img: image::ImageBuffer<image::Rgb<u8>, Vec<u8>> =
        image::ImageBuffer::from_fn(FIXTURE_W, FIXTURE_H, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 241) as u8, 0])
        });
    img.save_with_format(&cwd.join("big.png"), image::ImageFormat::Png)
        .unwrap();

    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

fn square_profile(edge: u32) -> ModelInputLimits {
    ModelInputLimits {
        max_request_bytes: None,
        images: Some(ModelImageInputLimits {
            resize: Some(ModelImageResizeOptions {
                max_width: Some(edge),
                max_height: Some(edge),
                max_bytes: None,
                jpeg_quality: None,
            }),
            max_per_message: None,
            max_per_request: None,
        }),
    }
}

/// Two models: the default declares `edge_default`, and `faux-tiny` declares 300px, so a `/model`
/// switch between them is observable in the next `read`.
fn faux_reading_the_image(edge_default: u32) -> Arc<FauxProvider> {
    let mut default = FauxModelDefinition::new("faux-1");
    default.input_limits = Some(square_profile(edge_default));
    let mut tiny = FauxModelDefinition::new("faux-tiny");
    tiny.input_limits = Some(square_profile(300));

    let faux = Arc::new(FauxProvider::with_config(FauxConfig {
        models: vec![default, tiny],
        ..FauxConfig::default()
    }));
    // One `read` call then a terminal turn, repeated, so two prompts can each issue a read.
    let step = || {
        vec![
            FauxResponseStep::factory(|_c, _o, _s, _m| {
                faux_assistant_message(
                    vec![faux_tool_call("read", json!({ "path": "big.png" }))],
                    StopReason::ToolUse,
                )
            }),
            FauxResponseStep::factory(|_c, _o, _s, _m| {
                faux_assistant_message(vec![faux_text("looked")], StopReason::Stop)
            }),
        ]
    };
    let mut steps = step();
    steps.extend(step());
    steps.extend(step());
    faux.set_response_steps(steps);
    faux
}

async fn session_for(fx: &Fixture, edge_default: u32) -> AgentSession {
    let provider: Arc<dyn Provider> = faux_reading_the_image(edge_default) as Arc<dyn Provider>;
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

/// The pixel size of the LAST `read` tool result's image block, decoded for real.
async fn last_read_dims(session: &AgentSession) -> (u32, u32) {
    let messages = session.messages().await;
    let content = messages
        .iter()
        .rev()
        .find_map(|m| match m {
            Message::ToolResult {
                tool_name, content, ..
            } if tool_name == "read" => Some(content.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the run must contain a `read` tool result; got {messages:#?}"));
    let data = content
        .iter()
        .find_map(|c| match c {
            Content::Image { data, .. } => Some(data.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("`read` must attach the image block; got {content:#?}"));
    let raw = base64::engine::general_purpose::STANDARD
        .decode(&data)
        .unwrap();
    let img = image::load_from_memory(&raw).unwrap();
    (img.width(), img.height())
}

/// CFG-085's Verify line, through the whole stack: a model whose row declares a 512px resize
/// profile makes `read` of a 2600px PNG return a ≤512px image.
///
/// RED before the handle: `read` resolved `DEFAULT_IMAGE_RESIZE` regardless of the row and the
/// result came back at 2000px.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_resizes_to_the_models_declared_profile() {
    let fx = fixture();
    let session = session_for(&fx, 512).await;
    let _ = session.prompt("look at big.png").await.expect("prompt");
    session.wait_for_idle().await;

    let (w, h) = last_read_dims(&session).await;
    assert!(
        w <= 512 && h <= 512,
        "the model's 512px profile must bind, not the 2000px default; got {w}x{h}"
    );
}

/// …and it is LIVE, not baked at construction: a model switch changes the very next `read` (pi
/// reads `ctx?.model?.inputLimits?.images?.resize` per call, `read.ts:138`).
///
/// BOTH switch paths are exercised, because they are two different functions and a push added to
/// only one of them passes a test that uses the other: `set_model` is the `/model` path
/// (`apply_model_change`) and `set_model_id` is the one an extension's queued `ControlOp::SetModel`
/// takes. The first version of this test used only `set_model_id`, and neutering
/// `apply_model_change`'s push left it green — which is exactly the half-wired state this pair
/// exists to catch.
///
/// RED against a static `ReadOpts::image_resize`, which keeps the startup model's profile forever —
/// the failure mode `ModelVisionHandle` already exists to avoid for the non-vision warning.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_model_switch_reaches_the_next_read() {
    let fx = fixture();
    let session = session_for(&fx, 2000).await;

    let _ = session.prompt("look at big.png").await.expect("prompt");
    session.wait_for_idle().await;
    let (before, _) = last_read_dims(&session).await;
    assert_eq!(
        before, 2000,
        "the startup model declares 2000px, so the first read lands there"
    );

    // Path 1: `/model` → `apply_model_change`.
    session.set_model("faux-tiny").await.expect("switch down");
    let _ = session.prompt("look again").await.expect("prompt");
    session.wait_for_idle().await;
    let (after, after_h) = last_read_dims(&session).await;
    assert!(
        after <= 300 && after_h <= 300,
        "the `/model` switch must reach the next read; expected <=300px from `faux-tiny`, got \
         {after}x{after_h}"
    );

    // Path 2: an extension's queued `SetModel` → `set_model_id`.
    session
        .set_model_id(ProviderId::from("faux"), ModelId::from("faux-1"))
        .await
        .expect("switch back");
    let _ = session.prompt("and again").await.expect("prompt");
    session.wait_for_idle().await;
    let (back, _) = last_read_dims(&session).await;
    assert_eq!(
        back, 2000,
        "`set_model_id` must re-push the profile too; a stale 300px here means only the \
         `/model` path pushes"
    );
}

/// CFG-085's Verify line, THROUGH A FILE ON DISK: a user `models.json` that patches `inputLimits`
/// makes `read` of a 2600px PNG return a ≤512px image.
///
/// Every other test in this file injects the profile through `FauxModelDefinition::input_limits`,
/// which never touches `ModelFile::compose` — so the config half and the session half were each
/// tested and nothing joined them. It was in fact BROKEN: the startup model was resolved against
/// the bare `provider.models()` while `/model` used the composed registry, so a `modelOverrides`
/// patch reached the model picker and not the model the session ran on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_user_models_json_override_decides_the_read_result_size() {
    let fx = fixture();
    std::fs::write(
        fx.agent_dir.join("models.json"),
        r#"{"providers":{"faux":{"modelOverrides":{"faux-1":{"inputLimits":{"images":{"resize":{"maxWidth":512,"maxHeight":512}}}}}}}}"#,
    )
    .unwrap();
    // The ROW declares 2000, i.e. the same value the shared default resolves to: only the file can
    // produce 512, so neither the row nor the default can satisfy the assertion by accident.
    let session = session_for(&fx, 2000).await;
    let _ = session.prompt("look").await.expect("prompt");
    session.wait_for_idle().await;

    let (w, h) = last_read_dims(&session).await;
    assert!(
        w <= 512 && h <= 512,
        "a `modelOverrides` patch on disk must reach the `read` tool's profile; got {w}x{h}"
    );
}
