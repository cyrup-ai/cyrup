//! Tool-result images are normalized as they enter history (pi `normalizeToolResultImages`,
//! `utils/tool-result-images.ts`, called from `core/agent-session.ts:693-698` @v1.0.4).
//!
//! `inputLimits.images.resize` has THREE consumers upstream. The `read` tool reads its own and the
//! prompt path reads one; this is the third, and the one that matters most per image, because a
//! tool result is PERSISTED: an unnormalized 6000px screenshot from an MCP bridge was re-sent at
//! 6000px on every subsequent request in the session, which is why upstream's doc comment says
//! *"oversized images make the provider reject the whole conversation, not just the offending
//! turn"*.
//!
//! Observed at the PROVIDER BOUNDARY: a tool call means a second provider request, and that
//! request's `toolResult` message is the history the provider actually sees. Dimensions are
//! measured by DECODING the base64, never read off a hint.
//!
//! The failure arm here is the OPPOSITE of the prompt path's. A prompt image that cannot be
//! processed is dropped and hinted; a tool-result image is KEPT VERBATIM, because *"the tool
//! already produced this image and the failure may just be an unavailable image backend"*. Both
//! arms are pinned below.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::{SessionBuilder, SessionConfig};
use base64::Engine;
use cyrup_core::message::Message;
use cyrup_core::{
    CancelToken, Content, ExtensionId, StopReason, Tool, ToolCallId, ToolError, ToolResult,
    ToolUpdateSink,
};
use cyrup_ext::{
    EventKind, EventPatch, ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension,
};
use cyrup_provider::faux::{
    FauxConfig, FauxModelDefinition, FauxProvider, FauxResponseStep, faux_assistant_message,
    faux_text, faux_tool_call,
};
use cyrup_provider::{ModelImageInputLimits, ModelImageResizeOptions, ModelInputLimits, Provider};
use serde_json::{Value, json};
use tempfile::TempDir;

const BIG_W: u32 = 6000;
const BIG_H: u32 = 4000;

// --------------------------------------------------------------------------------------------
// Fixtures. Images are BUILT, never committed.
// --------------------------------------------------------------------------------------------

fn gradient_png(w: u32, h: u32) -> Vec<u8> {
    let img: image::ImageBuffer<image::Rgb<u8>, Vec<u8>> =
        image::ImageBuffer::from_fn(w, h, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 241) as u8, 0])
        });
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The REAL pixel size of a base64 payload — decoded and probed, not read off a hint.
fn dims(data: &str) -> (u32, u32) {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(data)
        .expect("the attachment must be valid base64");
    let img = image::load_from_memory(&raw).expect("the attachment must decode as an image");
    (img.width(), img.height())
}

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
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

fn write_setting(fx: &Fixture, value: Option<bool>) {
    let body = match value {
        Some(v) => format!("{{\"images\":{{\"autoResize\":{v}}}}}"),
        None => "{}".to_string(),
    };
    std::fs::write(fx.agent_dir.join("settings.json"), body).unwrap();
}

fn resize_profile(max_width: Option<u32>, max_height: Option<u32>) -> ModelInputLimits {
    ModelInputLimits {
        max_request_bytes: None,
        images: Some(ModelImageInputLimits {
            resize: Some(ModelImageResizeOptions {
                max_width,
                max_height,
                max_bytes: None,
                jpeg_quality: None,
            }),
            max_per_message: None,
            max_per_request: None,
        }),
    }
}

/// What the `shot` tool hands back.
#[derive(Clone)]
enum Shot {
    /// A decodable PNG of the given size.
    Png(u32, u32),
    /// Bytes that are not an image at all — the KEEP-on-failure arm.
    Corrupt,
}

/// A tool that produces an image itself, like an MCP screenshot bridge. It also returns structured
/// content, because cyrup's fold DROPS the structured half when `content` is replaced without it —
/// so an image-only rewrite must carry it explicitly or this tool loses its structured output.
struct ShotTool {
    params: Value,
    shot: Shot,
}

#[async_trait::async_trait]
impl Tool for ShotTool {
    fn name(&self) -> &str {
        "shot"
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let data = match &self.shot {
            Shot::Png(w, h) => b64(&gradient_png(*w, *h)),
            Shot::Corrupt => b64(b"\x89PNG\r\n\x1a\n not actually a png at all"),
        };
        Ok(ToolResult {
            content: vec![
                Content::text("took a screenshot"),
                Content::Image {
                    data,
                    mime_type: "image/png".to_string(),
                },
            ],
            structured_content: Some(json!({ "ok": true })),
            ..Default::default()
        })
    }
}

/// A `tool_result` handler that REPLACES the content with an oversized image of its own. Upstream
/// normalizes after the extension hook *"so images injected or replaced by extensions are
/// normalized too"* (`agent-session.ts:692`); this is the subscriber that proves the order.
struct InjectingExt;

#[async_trait::async_trait]
impl NativeExtension for InjectingExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("image-injecting-ext")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::ToolResult]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if !matches!(ev, HostEvent::ToolResult { .. }) {
            return HookOutcome::Noop;
        }
        HookOutcome::Mutate(EventPatch::ToolResult {
            content: Some(vec![
                Content::text("injected by the extension"),
                Content::Image {
                    data: b64(&gradient_png(BIG_W, BIG_H)),
                    mime_type: "image/png".to_string(),
                },
            ]),
            details: None,
            structured_content: None,
            is_error: None,
            usage: None,
            terminate: None,
        })
    }
}

// --------------------------------------------------------------------------------------------
// The provider boundary: the SECOND request's `toolResult` message.
// --------------------------------------------------------------------------------------------

type Captured = Arc<Mutex<Vec<Message>>>;

/// Drive one model-issued `shot` call and return the `toolResult` content the provider saw on the
/// follow-up request, plus the settled tool-result message from history.
async fn run(
    shot: Shot,
    profile: Option<ModelInputLimits>,
    auto_resize: Option<bool>,
    ext: Option<Arc<dyn NativeExtension>>,
) -> (Vec<Content>, cyrup_agent::ToolResultMessage, Value) {
    let fx = fixture();
    write_setting(&fx, auto_resize);

    let mut default = FauxModelDefinition::new("faux-1");
    default.name = Some("Faux 1".into());
    default.input_limits = profile;
    let faux = Arc::new(FauxProvider::with_config(FauxConfig {
        models: vec![default],
        ..FauxConfig::default()
    }));
    let captured: Captured = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&captured);
    faux.set_response_steps(vec![
        FauxResponseStep::factory(|_ctx, _o, _s, _m| {
            faux_assistant_message(
                vec![faux_tool_call("shot".to_string(), json!({}))],
                StopReason::ToolUse,
            )
        }),
        FauxResponseStep::factory(move |ctx, _o, _s, _m| {
            if let Ok(mut g) = sink.lock() {
                *g = ctx.messages.clone();
            }
            faux_assistant_message(vec![faux_text("done")], StopReason::Stop)
        }),
    ]);

    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    cfg.custom_tools = vec![Arc::new(ShotTool {
        params: json!({ "type": "object" }),
        shot,
    }) as Arc<dyn Tool>];
    let mut builder = SessionBuilder::new(faux as Arc<dyn Provider>, cfg).settings_store(Arc::new(
        cyrup_config::FileSettingsStore::new(
            fx.agent_dir.join("settings.json"),
            fx.cwd.join(".cyrup/settings.json"),
        ),
    ));
    if let Some(ext) = ext {
        builder = builder.with_native_extension(ext);
    }
    let session = builder.build().await.unwrap().into_shared();
    let mut names = session.active_tool_names();
    names.push("shot".to_string());
    session.set_active_tools_by_name(&names).await;

    let ends: Arc<Mutex<Vec<Value>>> = Arc::default();
    let mut stream = session.subscribe();
    let end_sink = Arc::clone(&ends);
    tokio::spawn(async move {
        use futures::StreamExt as _;
        while let Some(event) = stream.next().await {
            if let crate::AgentSessionEvent::ToolExecutionEnd { result, .. } = event {
                end_sink.lock().unwrap().push(result);
            }
        }
    });

    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;
    let started = std::time::Instant::now();
    while ends.lock().unwrap().is_empty() && started.elapsed() < std::time::Duration::from_secs(10)
    {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let end = ends.lock().unwrap().first().cloned().expect("an end event");

    let messages = captured.lock().unwrap().clone();
    let wire = messages
        .iter()
        .find_map(|m| match m {
            Message::ToolResult { content, .. } => Some(content.clone()),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!("the follow-up request must carry a toolResult; got {messages:#?}")
        });
    let settled = session
        .agent_messages()
        .await
        .into_iter()
        .find_map(|m| match m {
            cyrup_agent::AgentMessage::ToolResult(t) if t.tool_name == "shot" => Some(t),
            _ => None,
        })
        .expect("a shot result");
    (wire, settled, end)
}

fn images(content: &[Content]) -> Vec<String> {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Image { data, .. } => Some(data.clone()),
            _ => None,
        })
        .collect()
}

fn text(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// --------------------------------------------------------------------------------------------
// The behaviour.
// --------------------------------------------------------------------------------------------

/// THE ROW'S POINT: a tool that produces a 6000px PNG has it in HISTORY, and therefore in every
/// later provider request, at the resolved default profile's 2000px.
///
/// Before this, the block was copied verbatim into history and re-sent at 6000px on every turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_oversized_tool_result_image_reaches_the_provider_downscaled() {
    let (wire, settled, end) = run(Shot::Png(BIG_W, BIG_H), None, None, None).await;

    let on_wire = images(&wire);
    assert_eq!(on_wire.len(), 1, "the image must still be there");
    let (w, h) = dims(&on_wire[0]);
    assert!(
        w <= 2000 && h <= 2000,
        "a model with no profile resolves the 2000px default, so a {BIG_W}x{BIG_H} tool-result \
         PNG must reach the provider at <=2000px; got {w}x{h}"
    );
    // The dimension note rides a text block appended after the image
    // (`tool-result-images.ts:58-61`) — a tool result has no user text to fold it into.
    assert!(
        text(&wire).contains("[Image: original 6000x4000, displayed at "),
        "the resize hint must reach the provider beside the image. Got: {:?}",
        text(&wire)
    );
    // And the normalization is what PERSISTED, not a per-request rewrite: history carries the
    // small image, which is what makes the cost saving hold across turns.
    let (sw, sh) = dims(&images(&settled.content)[0]);
    assert!(
        sw <= 2000 && sh <= 2000,
        "history must hold the normalized image, not the original; got {sw}x{sh}"
    );
    // cyrup-specific guard: the fold DROPS structured content when `content` is replaced without
    // it, so an image-only rewrite must carry the tool's structured half explicitly.
    assert_eq!(
        end["structuredContent"],
        json!({ "ok": true }),
        "normalizing the images must not delete the tool's structured output: {end}"
    );
}

/// THE PROFILE COMES FROM THE MODEL. The same 6000px tool-result PNG against a model whose row
/// declares `maxWidth/maxHeight = 512` arrives at <=512px.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_models_own_profile_decides_a_tool_result_images_size() {
    let (wire, _, _) = run(
        Shot::Png(BIG_W, BIG_H),
        Some(resize_profile(Some(512), Some(512))),
        None,
        None,
    )
    .await;
    let (w, h) = dims(&images(&wire)[0]);
    assert!(
        w <= 512 && h <= 512,
        "the row's own 512px clamp must win over the 2000px default; got {w}x{h}"
    );
}

/// The failure arm, and the one that differs from the prompt path: an image the resizer cannot
/// process is KEPT VERBATIM and NOT hinted.
///
/// Deleting a tool's output would make an unrelated backend failure look like the tool returning
/// nothing, which is why upstream passes the original block through
/// (`tool-result-images.ts:45-51`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_corrupt_tool_result_image_is_kept_verbatim_and_unhinted() {
    let (wire, _, _) = run(Shot::Corrupt, None, None, None).await;
    assert_eq!(
        images(&wire).len(),
        1,
        "the tool's own block must survive, unlike a failing PROMPT image which is dropped"
    );
    assert_eq!(
        images(&wire)[0],
        b64(b"\x89PNG\r\n\x1a\n not actually a png at all"),
        "kept byte-for-byte"
    );
    assert!(
        !text(&wire).contains("[Image omitted:"),
        "and no hint: this path has no user text to carry one. Got: {:?}",
        text(&wire)
    );
}

/// An image already inside the profile is re-emitted with its ORIGINAL bytes and NO extra text
/// block — upstream's `processed.data === block.data && … && hints.length === 0` short-circuit
/// (`:53-56`), which is the common case and must not add noise to every screenshot.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_result_image_within_the_profile_is_untouched_and_unhinted() {
    let small = gradient_png(64, 48);
    let (wire, _, _) = run(Shot::Png(64, 48), None, None, None).await;
    assert_eq!(images(&wire)[0], b64(&small), "byte-for-byte unchanged");
    assert_eq!(
        text(&wire),
        "took a screenshot",
        "no hint block may be added to an image that did not move"
    );
}

/// `images.autoResize: false` reaches this path too: the oversized image is forwarded verbatim.
/// This is the mirror that keeps every assertion above non-vacuous.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_resize_off_forwards_a_tool_result_image_verbatim() {
    let (wire, _, _) = run(Shot::Png(BIG_W, BIG_H), None, Some(false), None).await;
    let (w, h) = dims(&images(&wire)[0]);
    assert_eq!(
        (w, h),
        (BIG_W, BIG_H),
        "with the setting off the tool's own size stands"
    );
}

/// THE ORDERING. A `tool_result` handler that REPLACES the content with an oversized image of its
/// own has that image normalized, because the normalization runs AFTER the extension seam —
/// upstream's *"Runs after the extension hook so images injected or replaced by extensions are
/// normalized too"* (`agent-session.ts:692`).
///
/// Running the two in the other order leaves this test — and only this test — failing at 6000px.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_image_an_extension_injected_into_a_tool_result_is_normalized_too() {
    // The tool itself returns a SMALL image, so the only oversized bytes in play are the
    // extension's. If the normalization ran before the hook it would find nothing to do.
    let (wire, _, _) = run(Shot::Png(64, 48), None, None, Some(Arc::new(InjectingExt))).await;
    assert!(
        text(&wire).contains("injected by the extension"),
        "the handler's replacement must be what reached the provider. Got: {:?}",
        text(&wire)
    );
    let (w, h) = dims(&images(&wire)[0]);
    assert!(
        w <= 2000 && h <= 2000,
        "an image injected by a `tool_result` handler must be normalized too; got {w}x{h}"
    );
}
