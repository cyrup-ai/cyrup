//! Prompt images are normalized IN THE SESSION, against the request model's profile (SEAM-128).
//!
//! Pi normalizes every prompt image inside `AgentSession`, between the `before_agent_start` dispatch
//! and the construction of the user message (`core/agent-session.ts:2045-2067` @v1.0.4, re-read via
//! `git -C tmp/pi show`), with the resize profile read off `_limitsModel()?.inputLimits?.images?.resize`
//! (`:1932`). cyrup carried prompt images VERBATIM from every entry point except the CLI `@file` one:
//! the RPC `prompt` arm built its `UserInput` straight off the wire, the SDK's four `prompt*` fns and
//! ACP and the TUI all handed `images` through unchanged, and `UserInput::into_agent_message` copied
//! them into the user message. So an oversized screenshot through the SDK was either paid for in
//! full or rejected by the provider, and an undecodable one errored the whole request.
//!
//! Every assertion here is taken at the PROVIDER BOUNDARY — the `Context` a `FauxResponseStep`
//! factory is handed is the request as the provider sees it — and output dimensions are measured by
//! DECODING the base64 for real, never inferred from a hint string. Asserting on the transcript
//! would not prove the bytes reached the wire, and asserting on a hint would not prove any pixel
//! moved.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::{AgentSession, InputSource, SessionBuilder, SessionConfig, UserInput};
use base64::Engine;
use cyrup_core::message::{Content, Message};
use cyrup_core::{ExtensionId, StopReason};
use cyrup_ext::{
    ControlOp, EventKind, ExtError, HookOutcome, HostCtx, HostEvent, HostServices, InitApi,
    NativeExtension,
};
use cyrup_provider::faux::{
    FauxConfig, FauxModelDefinition, FauxProvider, FauxResponseStep, faux_assistant_message,
    faux_text,
};
use cyrup_provider::{ModelImageInputLimits, ModelImageResizeOptions, ModelInputLimits, Provider};
use tempfile::TempDir;

/// Deliberately far above the 2000px default clamp, so a surviving verbatim copy is unmistakable.
const BIG_W: u32 = 6000;
const BIG_H: u32 = 4000;

// ---------------------------------------------------------------------------------------------
// Fixtures. Images are BUILT here, never committed, following `crates/cyrup-tools/src/tests/
// image_proc.rs`. The noise generator is fixed-seed and deliberately incompressible so a PNG of a
// given size really is that large — a smooth gradient would compress away and a byte-cap assertion
// would then be satisfied at the original dimensions.
// ---------------------------------------------------------------------------------------------

fn pixel_hash(x: u32, y: u32) -> u32 {
    let mut h = x
        .wrapping_mul(374_761_393)
        .wrapping_add(y.wrapping_mul(668_265_263));
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    h ^ (h >> 16)
}

/// A smooth-gradient PNG of the given size: large in pixels, small in bytes, so only the DIMENSION
/// clamps can be what moves it.
fn gradient_png(w: u32, h: u32) -> Vec<u8> {
    let img: image::ImageBuffer<image::Rgb<u8>, Vec<u8>> =
        image::ImageBuffer::from_fn(w, h, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 241) as u8, 0])
        });
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

/// An incompressible PNG, for the byte-cap ladder.
fn noise_png(w: u32, h: u32) -> Vec<u8> {
    let img: image::ImageBuffer<image::Rgb<u8>, Vec<u8>> =
        image::ImageBuffer::from_fn(w, h, |x, y| {
            let v = pixel_hash(x, y);
            image::Rgb([v as u8, (v >> 8) as u8, (v >> 16) as u8])
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

/// `{"images":{"autoResize":<v>}}` in the GLOBAL settings file the session loads; `None` writes an
/// empty object, exercising pi's `?? true` default.
fn write_setting(fx: &Fixture, value: Option<bool>) {
    let body = match value {
        Some(v) => format!("{{\"images\":{{\"autoResize\":{v}}}}}"),
        None => "{}".to_string(),
    };
    std::fs::write(fx.agent_dir.join("settings.json"), body).unwrap();
}

// ---------------------------------------------------------------------------------------------
// The provider boundary.
// ---------------------------------------------------------------------------------------------

/// The user message as the PROVIDER received it, captured from inside the response factory.
type Captured = Arc<Mutex<Vec<Message>>>;

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

/// A faux provider with two models — the default carrying `profile_default`, plus a `small` one
/// clamped to 512px for the extension-selected-model case — that records the request context of
/// every call it serves.
fn faux_with(profile_default: Option<ModelInputLimits>) -> (Arc<FauxProvider>, Captured) {
    let mut default = FauxModelDefinition::new("faux-1");
    default.name = Some("Faux 1".into());
    default.input_limits = profile_default;
    let mut small = FauxModelDefinition::new("faux-small");
    small.input_limits = Some(resize_profile(Some(512), Some(512)));

    let faux = Arc::new(FauxProvider::with_config(FauxConfig {
        models: vec![default, small],
        ..FauxConfig::default()
    }));
    let captured: Captured = Arc::new(Mutex::new(Vec::new()));
    let sink = captured.clone();
    faux.set_response_steps(vec![FauxResponseStep::factory(move |ctx, _o, _s, _m| {
        if let Ok(mut g) = sink.lock() {
            *g = ctx.messages.clone();
        }
        faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)
    })]);
    (faux, captured)
}

/// The `(text, image payloads)` of the user message the provider was sent.
fn user_turn(captured: &Captured) -> (String, Vec<String>) {
    let messages = captured.lock().unwrap().clone();
    let content = messages
        .iter()
        .find_map(|m| match m {
            Message::User { content, .. } => Some(content.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the request must carry a user message; got {messages:#?}"));
    let text = content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let images = content
        .iter()
        .filter_map(|c| match c {
            Content::Image { data, .. } => Some(data.clone()),
            _ => None,
        })
        .collect();
    (text, images)
}

async fn session_with(
    fx: &Fixture,
    faux: Arc<FauxProvider>,
    ext: Option<Arc<dyn NativeExtension>>,
) -> AgentSession {
    let provider: Arc<dyn Provider> = faux;
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    let mut builder = SessionBuilder::new(provider, cfg).settings_store(Arc::new(
        cyrup_config::FileSettingsStore::new(
            fx.agent_dir.join("settings.json"),
            fx.cwd.join(".cyrup/settings.json"),
        ),
    ));
    if let Some(ext) = ext {
        builder = builder.with_native_extension(ext);
    }
    builder.build().await.expect("build")
}

/// Submit `text` + one image the way the RPC `prompt` arm does — a `UserInput` whose `images` came
/// straight off the wire, with `InputSource::Rpc`. This is the shape the row's Verify line is about.
fn rpc_input(text: &str, data: String, mime_type: &str) -> UserInput {
    UserInput {
        text: text.to_string(),
        images: vec![Content::Image {
            data,
            mime_type: mime_type.to_string(),
        }],
        source: InputSource::Rpc,
        expand_templates: true,
    }
}

async fn run(session: &AgentSession, input: UserInput) {
    session
        .prompt_accepted(input)
        .await
        .expect("prompt accepted");
    session.wait_for_idle().await;
}

// ---------------------------------------------------------------------------------------------
// THE ROW'S VERIFY LINE.
// ---------------------------------------------------------------------------------------------

/// SEAM-128 Verify, first half: an RPC `prompt` carrying a 6000px PNG reaches the provider at
/// ≤2000px.
///
/// The model declares NO profile, so this also covers "a model with no profile still gets the fixed
/// default": `None` resolves per key against `cyrup_core::DEFAULT_IMAGE_RESIZE`, which is upstream's
/// `DEFAULT_OPTIONS` (`image-resize-core.ts:24-29`) — 2000 / 2000 / 4.5 MiB / quality 80.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_prompt_image_reaches_the_provider_downscaled() {
    let fx = fixture();
    write_setting(&fx, None);
    let (faux, captured) = faux_with(None);
    let session = session_with(&fx, faux, None).await;

    let png = gradient_png(BIG_W, BIG_H);
    run(&session, rpc_input("what is this", b64(&png), "image/png")).await;

    let (text, images) = user_turn(&captured);
    assert_eq!(images.len(), 1, "the image must still be attached");
    let (w, h) = dims(&images[0]);
    assert!(
        w <= 2000 && h <= 2000,
        "a model with no `inputLimits.images.resize` resolves the 2000px default, so a \
         {BIG_W}x{BIG_H} PNG must reach the provider at <=2000px; got {w}x{h}"
    );
    // A SUCCESSFUL resize still hints (`agent-session.ts:1939`) and the hints are folded into the
    // user TEXT (`:2062`) — the coordinate-remap note a model needs to map a click on the
    // downscaled image back to the original.
    assert!(
        text.starts_with("what is this\n\n[Image: original 6000x4000, displayed at "),
        "the hints are appended to the expanded text, separated by a blank line. Got: {text:?}"
    );
}

/// SEAM-128 Verify, second half: a corrupt PNG yields a prompt whose text ENDS with the processing
/// hint and which carries NO image block.
///
/// Upstream drops the image and pushes `processed.message` as a hint (`:1936-1939`). Before this,
/// the undecodable bytes were forwarded and the provider errored the whole request.
///
/// The message is the RESIZE one, not the convert one, and that is upstream's behaviour rather than
/// an accident: `image/png` is already a supported inline MIME, so `normalizeImage` returns the
/// bytes WITHOUT decoding them (`image-process.ts:49-53`) and the first decode attempt is the
/// resize ladder's. `a_corrupt_bmp_fails_at_the_convert_stage` covers the other arm.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_corrupt_image_becomes_a_hint_and_no_image_block() {
    let fx = fixture();
    write_setting(&fx, None);
    let (faux, captured) = faux_with(None);
    let session = session_with(&fx, faux, None).await;

    let mut corrupt = b"\x89PNG\r\n\x1a\n".to_vec();
    corrupt.extend(std::iter::repeat_n(0xAB_u8, 4096));
    run(
        &session,
        rpc_input("describe this", b64(&corrupt), "image/png"),
    )
    .await;

    let (text, images) = user_turn(&captured);
    assert!(
        images.is_empty(),
        "an unprocessable image is DROPPED, not forwarded; got {} block(s)",
        images.len()
    );
    assert_eq!(
        text,
        "describe this\n\n[Image omitted: could not be resized below the inline image size \
         limit.]",
        "the message rides the user text instead of failing the request"
    );
}

/// A prompt whose ONLY content was a failing image: upstream has no empty-text guard, so
/// `${expandedText}\n\n${hints}` with an empty `expandedText` is a leading blank line, byte for
/// byte. The run still happens.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_image_only_prompt_whose_image_fails_still_runs_and_both_messages_are_reachable() {
    let fx = fixture();
    write_setting(&fx, None);
    let (faux, captured) = faux_with(None);
    let session = session_with(&fx, faux, None).await;

    let mut corrupt = b"\x89PNG\r\n\x1a\n".to_vec();
    corrupt.extend(std::iter::repeat_n(0x11_u8, 2048));
    run(&session, rpc_input("", b64(&corrupt), "image/png")).await;

    let (text, images) = user_turn(&captured);
    assert!(images.is_empty());
    assert_eq!(
        text,
        "\n\n[Image omitted: could not be resized below the inline image size limit.]"
    );

    // Both of pi's two failure messages must be reachable, or a port that collapsed them into one
    // would pass every test above. An unsupported MIME takes `normalizeImage`'s conversion arm
    // (`image-process.ts:55-58`), which is the one that can answer `null` → "could not be
    // converted".
    let (faux2, captured2) = faux_with(None);
    let session2 = session_with(&fx, faux2, None).await;
    let mut not_a_bmp = b"BM".to_vec();
    not_a_bmp.extend(std::iter::repeat_n(0x22_u8, 512));
    run(
        &session2,
        rpc_input("and this", b64(&not_a_bmp), "image/bmp"),
    )
    .await;
    let (text2, images2) = user_turn(&captured2);
    assert!(images2.is_empty());
    assert_eq!(
        text2,
        "and this\n\n[Image omitted: could not be converted to a supported inline image format.]"
    );
}

/// THE PROFILE COMES FROM THE MODEL, not from a constant: the same 6000px PNG against a model whose
/// row declares `inputLimits.images.resize.maxWidth/maxHeight = 512` arrives at ≤512px.
///
/// This is what distinguishes "resizes" from "resizes to the REQUEST MODEL's profile". Deleting the
/// `limits_model()` lookup and passing `None` leaves
/// `rpc_prompt_image_reaches_the_provider_downscaled` green and fails only this one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_models_own_profile_decides_the_size() {
    let fx = fixture();
    write_setting(&fx, None);
    let (faux, captured) = faux_with(Some(resize_profile(Some(512), Some(512))));
    let session = session_with(&fx, faux, None).await;

    let png = gradient_png(BIG_W, BIG_H);
    run(&session, rpc_input("hi", b64(&png), "image/png")).await;

    let (_text, images) = user_turn(&captured);
    let (w, h) = dims(&images[0]);
    assert!(
        w <= 512 && h <= 512,
        "the model's own 512px profile must bind, not the 2000px default; got {w}x{h}"
    );
}

/// `images.autoResize: false` reaches the PROMPT path too, and is read PER PROMPT (pi
/// `agent-session.ts:1931` calls `getImageAutoResize()` inside `_normalizePromptImages`, not at
/// build time the way the `read` tool's copy of the flag is baked in). With it off the 6000px PNG
/// is forwarded verbatim, which is also the mirror that keeps every assertion above non-vacuous.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_resize_off_forwards_the_image_verbatim() {
    let fx = fixture();
    write_setting(&fx, Some(false));
    let (faux, captured) = faux_with(None);
    let session = session_with(&fx, faux, None).await;

    let png = gradient_png(BIG_W, BIG_H);
    run(&session, rpc_input("hi", b64(&png), "image/png")).await;

    let (text, images) = user_turn(&captured);
    assert_eq!(
        images[0],
        b64(&png),
        "autoResize:false normalizes only, and a PNG is already a supported inline mime, so \
         `normalizeImage` is the identity"
    );
    assert_eq!(
        text, "hi",
        "no resize ⇒ no dimension note ⇒ nothing folded into the text"
    );
}

/// The byte-cap ladder runs HERE now. A dense 1500×1500 PNG is under every dimension clamp but over
/// pi's 4.5 MiB base64 cap, so only `maxBytes` can move it — this is the assertion the deleted
/// `crates/cyrup/src/tests/image_bytecap.rs` used to make at the `@file` boundary, relocated to
/// where the resize actually happens.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_byte_cap_ladder_runs_in_the_session() {
    /// Pi `DEFAULT_MAX_BYTES` (`image-resize-core.ts:22`).
    const MAX_B64: usize = 4_718_592;

    let fx = fixture();
    write_setting(&fx, None);
    let (faux, captured) = faux_with(None);
    let session = session_with(&fx, faux, None).await;

    let png = noise_png(1500, 1500);
    let source = b64(&png);
    assert!(
        source.len() > MAX_B64,
        "fixture must exceed the cap ({} base64 bytes)",
        source.len()
    );
    run(&session, rpc_input("hi", source.clone(), "image/png")).await;

    let (text, images) = user_turn(&captured);
    assert!(
        images[0].len() < MAX_B64,
        "the attached payload must be under pi's 4.5MB cap; got {} from {} source bytes",
        images[0].len(),
        source.len()
    );
    assert!(
        text.contains("Multiply coordinates by"),
        "a byte-cap re-encode is a resize, so the dimension note is emitted. Got: {text}"
    );
}

/// A CLI `@file` image still reaches the provider downscaled — now via the session rather than at
/// startup. Without this, dropping the `@file` startup resize is a silent CLI regression with every
/// other test green: `crates/cyrup/src/tests/image_auto_resize_file_args.rs` only proves the
/// attachment leaves `build_inputs` at full size, not that something later shrinks it.
///
/// The `@file` path's product is exactly a `UserInput` whose `text` is the `<file>`-wrapped
/// reference and whose `images` carry the file's own bytes, so that is what is submitted here, with
/// `InputSource::Cli`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cli_file_image_still_reaches_the_provider_downscaled() {
    let fx = fixture();
    write_setting(&fx, None);
    let (faux, captured) = faux_with(None);
    let session = session_with(&fx, faux, None).await;

    let png = gradient_png(2600, 800);
    let mut input = rpc_input(
        "<file name=\"/tmp/shot.png\"></file>\n",
        b64(&png),
        "image/png",
    );
    input.source = InputSource::Cli;
    run(&session, input).await;

    let (_text, images) = user_turn(&captured);
    let (w, h) = dims(&images[0]);
    assert!(
        w <= 2000,
        "a 2600px @file image must still be downscaled before the provider sees it; got {w}x{h}"
    );
}

// ---------------------------------------------------------------------------------------------
// THE ORDER: the hook decides the model, the model decides the profile.
// ---------------------------------------------------------------------------------------------

/// A `before_agent_start` subscriber that switches the model to `faux/faux-small` — whose row
/// declares a 512px profile — through the SAME `HostServices::control` seam a wasm guest's
/// `control.*` import reaches.
#[derive(Default)]
struct SwitchModelOnStart {
    services: Arc<Mutex<Option<Arc<dyn HostServices>>>>,
}

#[async_trait::async_trait]
impl NativeExtension for SwitchModelOnStart {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("switch-model-on-start")
    }

    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        if let Ok(mut g) = self.services.lock() {
            *g = Some(services);
        }
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::BeforeAgentStart]);
        Ok(())
    }

    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if !matches!(ev, HostEvent::BeforeAgentStart { .. }) {
            return HookOutcome::Noop;
        }
        if let Some(svc) = self.services.lock().ok().and_then(|g| g.clone()) {
            let _ = svc.control(ControlOp::SetModel(serde_json::json!("faux/faux-small")));
        }
        HookOutcome::Noop
    }
}

/// THE ORDERING TEST, and the one this step exists for. An extension that changes the model from
/// `before_agent_start` decides the resize profile: the 6000px PNG arrives at ≤512px, the
/// `faux-small` row's clamp, not 2000.
///
/// Upstream's comment says why this placement is load-bearing: *"Emit before_agent_start before
/// normalizing images so extension-driven model selection determines the resize profile used for the
/// request and history"* (`agent-session.ts:2045-2046`). That only buys something upstream because
/// `ctx.setModel` mutates `this.model` synchronously from the handler. In cyrup it is a QUEUED
/// `ControlOp::SetModel`, so the placement alone is not enough —
/// `AgentSession::apply_pending_agent_state_only` is what makes it true, and this is the only test
/// that fails without it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_extension_selected_model_decides_the_profile() {
    let fx = fixture();
    write_setting(&fx, None);
    // The DEFAULT model is deliberately profile-less, so 2000px is what a pre-hook read would give.
    let (faux, captured) = faux_with(None);
    let session = session_with(
        &fx,
        faux,
        Some(Arc::new(SwitchModelOnStart::default()) as Arc<dyn NativeExtension>),
    )
    .await;

    let png = gradient_png(BIG_W, BIG_H);
    run(&session, rpc_input("hi", b64(&png), "image/png")).await;

    let (_text, images) = user_turn(&captured);
    let (w, h) = dims(&images[0]);
    assert!(
        w <= 512 && h <= 512,
        "the model the hook selected must decide the profile — expected <=512px from \
         `faux-small`, got {w}x{h} (2000x1333 means the pre-hook model was read)"
    );
}

/// A `before_agent_start` subscriber that does nothing — the minimal condition that takes the
/// DISPATCH path instead of the `no_subscribers` fast path.
struct PassiveStartSubscriber;

#[async_trait::async_trait]
impl NativeExtension for PassiveStartSubscriber {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("passive-start-subscriber")
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::BeforeAgentStart]);
        Ok(())
    }

    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

/// BOTH EXITS. `assemble_run_inputs` returns early when nothing subscribes to `BeforeAgentStart`,
/// which is every install with no extension loaded — i.e. the common case, not the exotic one. A
/// normalization added only to the dispatch path would be inert for almost all users while every
/// extension-flavoured test above stayed green, which is exactly the inert-feature failure the
/// gap-analysis README warns about.
///
/// This pair is the proof: the same assertion with a passive subscriber (dispatch path) and with
/// none at all (fast path). The tests above already run on the fast path, so the subscriber arm is
/// what this adds — and the pair is what makes the coverage legible rather than accidental.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn both_exits_normalize() {
    for passive in [false, true] {
        let fx = fixture();
        write_setting(&fx, None);
        let (faux, captured) = faux_with(None);
        let ext: Option<Arc<dyn NativeExtension>> = if passive {
            Some(Arc::new(PassiveStartSubscriber))
        } else {
            None
        };
        let session = session_with(&fx, faux, ext).await;

        let png = gradient_png(BIG_W, BIG_H);
        run(&session, rpc_input("hi", b64(&png), "image/png")).await;

        let (_text, images) = user_turn(&captured);
        let (w, h) = dims(&images[0]);
        assert!(
            w <= 2000 && h <= 2000,
            "with passive subscriber = {passive} (fast path when false), the image must be \
             normalized; got {w}x{h}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// THE WIRE DECODE. Upstream's `Buffer.from(image.data, "base64")` accepts the standard and the
// URL-safe alphabets interchangeably; cyrup's decoder must too, or an SDK client that base64url-
// encodes its payload has every `-`/`_` deleted, every following 6-bit group shifted, and its
// image dropped with a failure hint for an image pi sends unharmed.
// ---------------------------------------------------------------------------------------------

/// `decode_wire_base64` is documented as Node-equivalent; this pins the three ways Node's decoder
/// is more forgiving than Rust's strict `STANDARD.decode`.
///
/// Reference values are Node's own:
/// `node -e 'console.log([...Buffer.from("_-8A","base64")])'` -> `[255, 239, 0]`.
#[test]
fn the_wire_decode_matches_node_on_padding_whitespace_and_the_url_safe_alphabet() {
    use crate::session::prompt_images::decode_wire_base64;

    // The payload Node is quoted on above: `_` is 63 and `-` is 62, not characters to drop.
    assert_eq!(
        decode_wire_base64("_-8A"),
        vec![255u8, 239, 0],
        "`-`/`_` must be TRANSLATED like Node, not filtered out"
    );

    let png = gradient_png(16, 9);
    let standard = b64(&png);

    // 1. URL-safe, unpadded — the base64url case.
    let url_safe = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&png);
    assert!(
        url_safe.contains('-') || url_safe.contains('_'),
        "the fixture must actually exercise the URL-safe alphabet, else this test is vacuous"
    );
    assert_eq!(
        decode_wire_base64(&url_safe),
        png,
        "a base64url payload must decode to the original bytes"
    );

    // 2. Embedded whitespace — a MIME-wrapped payload.
    let wrapped = standard
        .as_bytes()
        .chunks(76)
        .map(|c| String::from_utf8_lossy(c).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        decode_wire_base64(&wrapped),
        png,
        "newlines must be skipped, not rejected"
    );

    // 3. Padding stripped.
    assert_eq!(
        decode_wire_base64(standard.trim_end_matches('=')),
        png,
        "missing `=` padding must not reject the payload"
    );
}

/// The end-to-end consequence: a base64url-encoded 6000px PNG reaches the provider AS AN IMAGE,
/// downscaled, instead of being dropped with `[Image omitted: ...]`.
///
/// Against the pre-fix filter (`-`/`_` deleted rather than mapped) the decoded bytes are shifted
/// garbage, the resize ladder cannot decode them, and this test sees zero image blocks and a
/// failure hint.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_base64url_encoded_prompt_image_reaches_the_provider_as_an_image() {
    let fx = fixture();
    write_setting(&fx, None);
    let (faux, captured) = faux_with(None);
    let session = session_with(&fx, faux, None).await;

    let png = gradient_png(BIG_W, BIG_H);
    let url_safe = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&png);
    assert!(
        url_safe.contains('-') || url_safe.contains('_'),
        "the fixture must actually exercise the URL-safe alphabet, else this test is vacuous"
    );
    run(&session, rpc_input("what is this", url_safe, "image/png")).await;

    let (text, images) = user_turn(&captured);
    assert_eq!(
        images.len(),
        1,
        "a base64url payload must survive the decode and stay attached; text was {text:?}"
    );
    let (w, h) = dims(&images[0]);
    assert!(
        w <= 2000 && h <= 2000,
        "and be resized like any other image; got {w}x{h}"
    );
    assert!(
        !text.contains("[Image omitted:"),
        "no failure hint belongs on a payload pi decodes fine. Got: {text:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// THE CONFIG HALF, JOINED. CFG-085's Verify is about a USER `models.json` — a file on disk — and
// every other test here injects the profile through `FauxModelDefinition::input_limits`, which
// bypasses `ModelFile::compose` entirely. These two run the whole link: file ->
// `load_models_file_reporting` -> `compose`/`apply_models_json` -> the composed registry -> the
// running model -> `limits_model()` -> the resizer -> the pixels on the wire.
// ---------------------------------------------------------------------------------------------

/// `{...}` written to `<agent_dir>/models.json`, the path the builder reads (CFG-002).
fn write_models_json(fx: &Fixture, body: &str) {
    std::fs::write(fx.agent_dir.join("models.json"), body).unwrap();
}

/// A `modelOverrides` patch in a user `models.json` changes the pixels that reach the provider.
///
/// This is the one test that would catch a future `Model` rebuild dropping `input_limits` on the
/// composition path: `compose.rs`'s own tests prove the merge in isolation and every other test
/// here hands the profile in directly, so both halves can be green while the join is broken.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_user_models_json_override_decides_the_prompt_image_size() {
    let fx = fixture();
    write_setting(&fx, None);
    write_models_json(
        &fx,
        r#"{"providers":{"faux":{"modelOverrides":{"faux-1":{"inputLimits":{"images":{"resize":{"maxWidth":512,"maxHeight":512}}}}}}}}"#,
    );
    // NO profile on the row: the 512 clamp can only have come from the file.
    let (faux, captured) = faux_with(None);
    let session = session_with(&fx, faux, None).await;

    let png = gradient_png(2600, 800);
    run(&session, rpc_input("what is this", b64(&png), "image/png")).await;

    let (text, images) = user_turn(&captured);
    assert_eq!(
        images.len(),
        1,
        "the image must still be attached: {text:?}"
    );
    let (w, h) = dims(&images[0]);
    assert!(
        w <= 512 && h <= 512,
        "a `modelOverrides` patch on disk must decide the clamp; got {w}x{h} (the 2000px default \
         would also satisfy a naive <=2000 assertion, which is why this asserts 512)"
    );
}

/// The same through a DECLARED row rather than a patch: `models` upserts `faux-1` with its own
/// `inputLimits`, which `model_from_json` copies verbatim onto the composed model.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_user_models_json_declared_row_decides_the_prompt_image_size() {
    let fx = fixture();
    write_setting(&fx, None);
    write_models_json(
        &fx,
        r#"{"providers":{"faux":{"models":[{"id":"faux-1","name":"Faux 1","inputLimits":{"images":{"resize":{"maxWidth":256,"maxHeight":256}}}}]}}}"#,
    );
    let (faux, captured) = faux_with(None);
    let session = session_with(&fx, faux, None).await;

    let png = gradient_png(2600, 800);
    run(&session, rpc_input("what is this", b64(&png), "image/png")).await;

    let (text, images) = user_turn(&captured);
    assert_eq!(
        images.len(),
        1,
        "the image must still be attached: {text:?}"
    );
    let (w, h) = dims(&images[0]);
    assert!(
        w <= 256 && h <= 256,
        "a declared row's own `inputLimits` must decide the clamp; got {w}x{h}"
    );
}
