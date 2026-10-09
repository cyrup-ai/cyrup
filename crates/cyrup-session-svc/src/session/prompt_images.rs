//! Prompt-image normalization — Pi `AgentSession._normalizePromptImages`
//! (`packages/coding-agent/src/core/agent-session.ts:1923-1943` @v1.0.4).
//!
//! # The gap this closes (SEAM-128)
//!
//! Every image that enters a prompt from anywhere EXCEPT the CLI `@file` boundary used to reach the
//! provider verbatim: the RPC `prompt` command builds its `UserInput` straight off the wire
//! (`cyrup-modes/src/rpc/mod.rs`'s `prompt` arm), the SDK/session API's four `prompt*` entry points
//! hand their `images` through unchanged, and so do ACP and the TUI. `UserInput::into_agent_message`
//! then copied them into the user message byte-for-byte. So the same screenshot cost a different
//! amount depending on how it arrived, and an undecodable one errored the whole request where
//! upstream turns it into a text hint.
//!
//! # The contract
//!
//! Per image: base64-decode, hand the raw bytes to the ONE shared resizer
//! ([`cyrup_tools::image_proc::process_image`]) with
//!
//! * `auto_resize_images` = the `images.autoResize` setting (Pi
//!   `this.settingsManager.getImageAutoResize()`, `:1931`), and
//! * `resize` = the REQUEST MODEL's `inputLimits.images.resize`, read off
//!   [`crate::AgentSession::limits_model`] (Pi `this._limitsModel()?.inputLimits?.images?.resize`,
//!   `:1932`).
//!
//! A `Failed` image is DROPPED and its message becomes a hint (Pi's `continue` at `:1937-1939`); a
//! SUCCESSFUL one contributes its own hints too, because a resize hints on success (the conversion
//! note and the `[Image: original WxH, displayed at WxH …]` dimension note). The caller folds the
//! hints into the user TEXT (`:2062`), which is why they come back as a separate vector rather than
//! being written anywhere here.
//!
//! # The decode is deliberately FORGIVING, because `Buffer.from` is
//!
//! Upstream decodes with `Buffer.from(image.data, "base64")` (`:1930`), and Node's base64 decoder
//! SKIPS characters outside the alphabet and tolerates missing padding rather than failing. Rust's
//! strict `STANDARD.decode` rejects both. Decoding strictly and inventing a failure message for the
//! rejects would put a message in the prompt that upstream never produces, and would produce the
//! WRONG one: for a supported MIME `normalizeImage` returns the bytes without decoding them
//! (`image-process.ts:49-53`), so a garbage payload fails at the RESIZE stage with
//! `"[Image omitted: could not be resized below the inline image size limit.]"`, not at the convert
//! stage. [`decode_wire_base64`] therefore mirrors Node, and the message is left to
//! `process_image`, which is the one place that knows which stage failed.

use cyrup_core::Content;

use crate::AgentSession;

/// The settled normalization: the images to attach, and the hints to fold into the user text.
///
/// Pi returns `{ images, hints }` (`agent-session.ts:1924-1925`); the hints are deliberately NOT
/// applied to anything here — the caller joins them onto the expanded text, which is the one place
/// upstream puts them (`:2062`).
pub(crate) struct NormalizedPromptImages {
    pub(crate) images: Vec<Content>,
    pub(crate) hints: Vec<String>,
}

impl AgentSession {
    /// Pi `_normalizePromptImages` (`agent-session.ts:1923-1943`).
    ///
    /// The profile is resolved to an OWNED value BEFORE the blocking hop, so no lock and no borrow
    /// of `self` crosses it. The decode + EXIF bake + clamp + JPEG ladder is CPU-bound — the `read`
    /// tool already runs it under `spawn_blocking` for exactly this reason — so it must not run
    /// inline on the async worker.
    pub(crate) async fn normalize_prompt_images(
        &self,
        images: Vec<Content>,
    ) -> NormalizedPromptImages {
        // Pi's `if (!images) return { images: [], hints: [] }` (`:1926`). Short-circuiting here also
        // keeps a text-only prompt off `limits_model()`, which takes a transcript snapshot.
        if images.is_empty() {
            return NormalizedPromptImages {
                images,
                hints: Vec::new(),
            };
        }

        // `_limitsModel()?.inputLimits?.images?.resize` (`:1932`). `limits_model` is pi's
        // `this.routedModel?.model ?? this.model` verbatim: under a virtual selection it is the
        // PHYSICAL model that answered, which is whose limits the request actually has. `None` all
        // the way down is correct and not a fallback to invent here — the resizer resolves every
        // absent key against `cyrup_core::DEFAULT_IMAGE_RESIZE`, which is byte-for-byte upstream's
        // `DEFAULT_OPTIONS` (`image-resize-core.ts:24-29`), so an unstamped model and a stamped one
        // resolve the same profile.
        let profile = self
            .limits_model()
            .await
            .and_then(|m| m.image_resize_profile());
        // `this.settingsManager.getImageAutoResize()` (`:1931`), read PER PROMPT as upstream does —
        // not baked at session build the way the `read` tool's copy of the flag is
        // (`builder.rs`'s `ReadOpts::auto_resize_images`, pi `agent-session.ts:2553`).
        let auto_resize_images = self.services.settings.effective().image_auto_resize();

        let processed = tokio::task::spawn_blocking(move || {
            normalize_blocking(images, auto_resize_images, profile.as_ref())
        })
        .await;
        match processed {
            Ok(out) => out,
            // A panic inside the resizer is the only way this arm is reached (the task is never
            // cancelled — nothing holds its handle but this await). Dropping the images and saying
            // so is strictly better than propagating a panic through the prompt path, and it keeps
            // the run going, which is what upstream's per-image failure arm does too.
            Err(_) => NormalizedPromptImages {
                images: Vec::new(),
                hints: vec![
                    "[Image omitted: could not be converted to a supported inline image format.]"
                        .to_string(),
                ],
            },
        }
    }
}

/// The blocking half: the per-image loop, verbatim from `agent-session.ts:1929-1942`.
fn normalize_blocking(
    images: Vec<Content>,
    auto_resize_images: bool,
    profile: Option<&cyrup_core::ModelImageResizeOptions>,
) -> NormalizedPromptImages {
    use cyrup_tools::image_proc::{ProcessImageOptions, Processed, process_image};

    let mut normalized = Vec::with_capacity(images.len());
    let mut hints: Vec<String> = Vec::new();
    for item in images {
        // Pi's loop is over `ImageContent[]`, a typed array that cannot hold anything else. cyrup's
        // `UserInput::images` is `Vec<Content>`, so a non-image block is structurally possible;
        // passing it through untouched is the only non-lossy answer and matches what the previous
        // verbatim copy did with it.
        let Content::Image { data, mime_type } = &item else {
            normalized.push(item);
            continue;
        };
        let raw = decode_wire_base64(data);
        match process_image(
            &raw,
            mime_type,
            ProcessImageOptions {
                auto_resize_images,
                resize: profile,
            },
        ) {
            Processed::Ok {
                data,
                mime,
                hints: image_hints,
            } => {
                normalized.push(Content::Image {
                    data,
                    mime_type: mime,
                });
                // `hints.push(...processed.hints)` (`:1939`) — a SUCCESSFUL resize still hints, and
                // dropping these would lose the coordinate-remap note a model needs to map a click
                // on the downscaled image back to the original.
                hints.extend(image_hints);
            }
            // `if (!processed.ok) { hints.push(processed.message); continue; }` (`:1936-1938`): the
            // image is DROPPED and its message rides the user text instead, which is the whole of
            // "a corrupt image becomes a text hint on the prompt instead of erroring the request".
            Processed::Failed { message } => hints.push(message),
        }
    }
    NormalizedPromptImages {
        images: normalized,
        hints,
    }
}

impl AgentSession {
    /// Normalize `input.images` in place and fold the hints into `input.text` — Pi's two statements
    /// at `agent-session.ts:2061-2062`:
    ///
    /// ```text
    /// const normalized = await this._normalizePromptImages(currentImages);
    /// const userText = normalized.hints.length > 0 ? `${expandedText}\n\n${normalized.hints.join("\n")}` : expandedText;
    /// ```
    ///
    /// `input.text` IS upstream's `expandedText`: `prepare_and_assemble`'s step 1 already ran
    /// `expand_input_text` over it. And [`crate::UserInput::into_agent_message`] already pushes
    /// `Content::text(self.text)` first and the images after, so folding the hints into `text` here
    /// lands them in the same single LEADING text block as upstream's `userContent[0]` rather than
    /// in a second block the model would read as a separate turn fragment.
    ///
    /// It is one helper rather than two call-site copies because `assemble_run_inputs` has TWO
    /// exits (the `no_subscribers` fast path and the dispatch path) and both need it.
    pub(crate) async fn normalize_prompt_images_into(&self, input: &mut crate::UserInput) {
        let images = std::mem::take(&mut input.images);
        let normalized = self.normalize_prompt_images(images).await;
        input.images = normalized.images;
        if !normalized.hints.is_empty() {
            input.text = format!("{}\n\n{}", input.text, normalized.hints.join("\n"));
        }
    }
}

/// Decode a wire base64 payload the way `Buffer.from(data, "base64")` does: TRANSLATE the two
/// URL-safe characters into their standard twins, skip every other character outside the alphabet,
/// and do not require padding.
///
/// Node's decoder is forgiving, and the difference is observable. A payload carrying a stray
/// newline, a URL-safe `-`/`_`, or no `=` padding decodes upstream and reaches `processImage`; under
/// Rust's strict `STANDARD.decode` it would be rejected before the resizer ever saw it, and the
/// prompt would carry a failure hint for an image upstream processes fine. The filter also means
/// outright garbage still yields SOME bytes, so the stage that fails — and therefore which of pi's
/// two `[Image omitted: …]` messages the prompt carries — is decided by `process_image`, as it is
/// upstream, rather than guessed here.
///
/// `-` and `_` are MAPPED, not dropped. Node accepts the standard and URL-safe alphabets
/// interchangeably — `node -e 'console.log([...Buffer.from("_-8A","base64")])'` prints
/// `[255, 239, 0]`, i.e. `_` decoded as 63 and `-` as 62. Deleting them instead (which this
/// function did until the base64url fix) shifts every following 6-bit group, so a base64url-encoded
/// PNG — the natural choice for an RPC/SDK client whose payload also rides a URL or a JWT-shaped
/// envelope — arrived as garbage, failed in the resize ladder, and was dropped with
/// `[Image omitted: could not be resized below the inline image size limit.]` for an image upstream
/// sends unharmed. The strict fast path below cannot rescue it either: a base64url payload with no
/// `+`/`/` is still not valid STANDARD base64 once a `-` or `_` is present.
pub(crate) fn decode_wire_base64(data: &str) -> Vec<u8> {
    use base64::Engine;
    use base64::engine::{GeneralPurpose, GeneralPurposeConfig, general_purpose::STANDARD};
    // The fast path: a well-formed payload, which is every payload any cyrup front-end produces.
    if let Ok(raw) = STANDARD.decode(data) {
        return raw;
    }
    let filtered: String = data
        .chars()
        .filter_map(|c| match c {
            '-' => Some('+'),
            '_' => Some('/'),
            c if c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=' => Some(c),
            _ => None,
        })
        .collect();
    let forgiving = GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        GeneralPurposeConfig::new()
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent)
            .with_decode_allow_trailing_bits(true),
    );
    forgiving.decode(&filtered).unwrap_or_default()
}
