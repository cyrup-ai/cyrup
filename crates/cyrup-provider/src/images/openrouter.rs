//! The `openrouter-images` wire protocol (1:1 port of Pi `api/openrouter-images.ts`).
//!
//! OpenRouter exposes image generation through the OpenAI `chat/completions` shape with
//! `modalities: ["image"]` (plus `"text"` when the model also emits text). cyrup speaks it directly
//! over `reqwest` (no OpenAI SDK): a single non-streaming POST to `{baseUrl}/chat/completions`,
//! Bearer-keyed, whose response carries assistant `images[]` as `data:` URLs that are decoded into
//! [`Content::Image`] blocks (Pi `openrouter-images.ts:86-97`).

use super::{AssistantImages, ImagesApiImpl, ImagesContext, ImagesModel, ImagesOptions};
use crate::stream::ProviderResponse;
use crate::stream::sse::build_client_for_target;
use cyrup_core::{ApiId, Content, Usage};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

/// Build the `openrouter-images` [`ImagesApiImpl`] (Pi `openrouterImagesApi()`).
pub fn factory() -> Arc<dyn ImagesApiImpl> {
    Arc::new(OpenRouterImages {
        api: ApiId::from(super::OPENROUTER_IMAGES),
    })
}

struct OpenRouterImages {
    api: ApiId,
}

#[async_trait::async_trait]
impl ImagesApiImpl for OpenRouterImages {
    fn api(&self) -> &ApiId {
        &self.api
    }

    async fn generate_images(
        &self,
        model: &ImagesModel,
        context: &ImagesContext,
        options: &ImagesOptions,
    ) -> AssistantImages {
        match run(model, context, options).await {
            Ok(out) => out,
            Err(GenError::Aborted) => AssistantImages::errored(model, "aborted", true),
            Err(GenError::Message(m)) => AssistantImages::errored(model, m, false),
        }
    }
}

/// Internal failure channel — `Aborted` becomes `stop_reason: aborted`, every other failure
/// `stop_reason: error` (Pi `output.stopReason = options?.signal?.aborted ? "aborted" : "error"`,
/// openrouter-images.ts:101).
enum GenError {
    Aborted,
    Message(String),
}

async fn run(
    model: &ImagesModel,
    context: &ImagesContext,
    options: &ImagesOptions,
) -> Result<AssistantImages, GenError> {
    // `if (!apiKey) throw new Error("No API key for provider: …")` (openrouter-images.ts:53-56).
    let Some(api_key) = options.api_key.clone() else {
        return Err(GenError::Message(format!(
            "No API key for provider: {}",
            model.provider
        )));
    };

    // `buildParams(model, context)` (openrouter-images.ts:124-151).
    let mut params = build_params(model, context);
    // `options?.onPayload?.(params, model)` — a returned value replaces the payload (openrouter-images.ts:59-62).
    if let Some(hook) = &options.on_payload
        && let Some(next) = hook(&params, model)
    {
        params = next;
    }

    let url = format!("{}/chat/completions", model.base_url);
    // PROXY MODEL — DOCUMENTED DEFERRAL (residual #4). cyrup routes the image client through the
    // shared per-target resolver (`build_client_for_target` → Pi `resolveHttpProxyUrlForTarget`,
    // node-http-proxy.ts:92-112), which honors `http(s)_proxy` + `all_proxy` + `no_proxy` and
    // REJECTS SOCKS/PAC. Pi's image path does NOT use that resolver: `createClient`
    // (openrouter-images.ts:107-119) builds a bare OpenAI SDK client whose global fetch is proxied by
    // undici's GLOBAL `EnvHttpProxyAgent` (http-dispatcher.ts `configureHttpDispatcher` →
    // `setGlobalDispatcher`), which reads HTTP_PROXY/HTTPS_PROXY/NO_PROXY ONLY — it never reads
    // `all_proxy` and silently ignores SOCKS instead of rejecting it.
    //
    // Choice: KEEP cyrup's unified resolver (deliberate broader/safer delta), do NOT regress.
    // Matching Pi exactly is not clean: it would require either (a) a bare client that honors NO
    // proxy at all (strictly worse than Pi, which DOES honor HTTP(S)_PROXY), or (b) a third,
    // image-only proxy mode replicating undici's `EnvHttpProxyAgent` (HTTP(S)_PROXY/NO_PROXY only,
    // no `all_proxy`, silent SOCKS) — extra surface that is strictly LESS safe than rejecting SOCKS.
    // The delta only surfaces in the rare `all_proxy`-set / SOCKS-on-image edge; the SOCKS-rejection
    // test below pins the current (broader) behavior. See spec/gap-analysis/00-residual-ledger.md #4.
    // PROV-006: `ImagesOptions.timeout_ms` is Pi's `{ timeout: options.timeoutMs }` on the images
    // SDK client (openrouter-images.ts:67); `None` falls back to the process-global idle timeout.
    let client = build_client_for_target(
        &url,
        &crate::auth::types::EnvAuthContext,
        options.env.as_ref(),
        options.timeout_ms,
    )
    .await
    .map_err(|e| GenError::Message(e.to_string()))?;
    let mut builder = client.post(&url).bearer_auth(&api_key).json(&params);
    // `defaultHeaders: providerHeadersToRecord({ ...model.headers, ...optionsHeaders })`
    // (openrouter-images.ts:116). A `None` value suppresses a default; on a fresh request there is no
    // default to suppress, so only present values are applied (matching cyrup's header overlay).
    for (name, value) in merged_headers(model, options) {
        if let Some(value) = value {
            builder = builder.header(name, value);
        }
    }
    if let Some(ms) = options.timeout_ms {
        builder = builder.timeout(Duration::from_millis(ms));
    }

    // Race the request against cancellation (Pi `signal`, openrouter-images.ts:64).
    let response = match &options.cancel {
        Some(cancel) => tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(GenError::Aborted),
            sent = builder.send() => sent,
        },
        None => builder.send().await,
    }
    .map_err(|e| GenError::Message(e.to_string()))?;

    let status = response.status();
    let resp_headers = header_record(response.headers());
    // `options?.onResponse?.({ status, headers }, model)` (openrouter-images.ts:71).
    if let Some(hook) = &options.on_response {
        hook(
            &ProviderResponse {
                status: status.as_u16(),
                headers: resp_headers,
            },
            model,
        );
    }

    if !status.is_success() {
        // The OpenAI SDK throws on a non-2xx response; Pi catches it and surfaces the body text.
        let body = response.text().await.unwrap_or_default();
        return Err(GenError::Message(format!(
            "http {}: {}",
            status.as_u16(),
            body
        )));
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| GenError::Message(e.to_string()))?;

    Ok(parse_response(model, &body))
}

/// `buildParams` (openrouter-images.ts:124-151): a single user turn whose content mirrors the input,
/// `stream:false`, and `modalities` driven by the model's output modalities.
fn build_params(model: &ImagesModel, context: &ImagesContext) -> serde_json::Value {
    let content: Vec<serde_json::Value> = context
        .input
        .iter()
        .filter_map(|item| match item {
            // `sanitizeSurrogates(item.text)` is a structural no-op in Rust (well-formed UTF-8).
            Content::Text { text, .. } => Some(json!({ "type": "text", "text": text })),
            Content::Image { data, mime_type } => Some(json!({
                "type": "image_url",
                "image_url": { "url": format!("data:{mime_type};base64,{data}") },
            })),
            // Pi's `ImagesInputContent` is Text|Image only; other variants cannot appear.
            Content::Thinking { .. } | Content::ToolCall(_) => None,
        })
        .collect();

    let modalities = if model.outputs_text() {
        json!(["image", "text"])
    } else {
        json!(["image"])
    };

    json!({
        "model": model.id,
        "messages": [{ "role": "user", "content": content }],
        "stream": false,
        "modalities": modalities,
    })
}

/// Parse the OpenRouter chat-completion response into [`AssistantImages`] (openrouter-images.ts:73-99).
fn parse_response(model: &ImagesModel, body: &serde_json::Value) -> AssistantImages {
    let mut output = AssistantImages::new(model);
    output.response_id = body.get("id").and_then(|v| v.as_str()).map(String::from);
    if let Some(usage) = body.get("usage").filter(|u| u.is_object()) {
        output.usage = Some(parse_usage(model, usage));
    }

    let Some(choice) = body
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|c| c.first())
    else {
        return output;
    };
    let message = choice.get("message");

    // `if (typeof content === "string" && content.length > 0)` (openrouter-images.ts:82).
    if let Some(text) = message
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        && !text.is_empty()
    {
        output.output.push(Content::text(text));
    }

    // `for (const image of choice.message.images ?? [])` (openrouter-images.ts:86-96).
    let images = message
        .and_then(|m| m.get("images"))
        .and_then(|i| i.as_array());
    for image in images.into_iter().flatten() {
        let image_url = image.get("image_url");
        let url = match image_url {
            Some(serde_json::Value::String(s)) => Some(s.as_str()),
            Some(obj) => obj.get("url").and_then(|u| u.as_str()),
            None => None,
        };
        let Some(url) = url else { continue };
        if let Some((mime_type, data)) = parse_data_url(url) {
            output.output.push(Content::Image { data, mime_type });
        }
    }

    output
}

/// Decode a `data:` URL into `(mimeType, base64Data)` (Pi regex
/// `^data:([^;]+);base64,(.+)$`, openrouter-images.ts:88-90). A non-`data:` URL, a `mime` containing
/// `;`, or empty data yields `None` (skipped, like a non-matching Pi regex).
fn parse_data_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("data:")?;
    let (mime, data) = rest.split_once(";base64,")?;
    if mime.is_empty() || mime.contains(';') || data.is_empty() {
        return None;
    }
    Some((mime.to_string(), data.to_string()))
}

/// `parseUsage` (openrouter-images.ts:153-184): split reported cached tokens into read/write, derive
/// input, and price each component against the model's per-1e6-token rates.
fn parse_usage(model: &ImagesModel, raw: &serde_json::Value) -> Usage {
    let num = |v: &serde_json::Value, path: &[&str]| -> u64 {
        let mut cur = v;
        for key in path {
            match cur.get(key) {
                Some(next) => cur = next,
                None => return 0,
            }
        }
        cur.as_u64().unwrap_or(0)
    };

    let prompt_tokens = num(raw, &["prompt_tokens"]);
    let reported_cached = num(raw, &["prompt_tokens_details", "cached_tokens"]);
    let cache_write = num(raw, &["prompt_tokens_details", "cache_write_tokens"]);
    let cache_read = if cache_write > 0 {
        reported_cached.saturating_sub(cache_write)
    } else {
        reported_cached
    };
    let input = prompt_tokens
        .saturating_sub(cache_read)
        .saturating_sub(cache_write);
    let output = num(raw, &["completion_tokens"]);

    let mut usage = Usage {
        input,
        output,
        cache_read,
        cache_write,
        cache_write_1h: None,
        reasoning: None,
        total_tokens: 0,
        cost: cyrup_core::Cost::default(),
    };
    // `apply_cost` recomputes `total_tokens = input+output+cacheRead+cacheWrite` and prices each
    // component — byte-identical to Pi's inline `usage.cost` math (no 1h split here).
    crate::usage::apply_cost(&model.cost, &mut usage);
    usage
}

/// `{ ...model.headers, ...options.headers }` (openrouter-images.ts:116): the model's default headers
/// overlaid by the per-request headers (request wins per key).
fn merged_headers(model: &ImagesModel, options: &ImagesOptions) -> crate::HeaderMap {
    let mut merged = crate::HeaderMap::new();
    if let Some(h) = &model.headers {
        merged.extend(h.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    if let Some(h) = &options.headers {
        merged.extend(h.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    merged
}

/// Flatten a `reqwest` header map into the string record Pi's `headersToRecord` produces.
fn header_record(
    headers: &reqwest::header::HeaderMap,
) -> std::collections::BTreeMap<String, String> {
    headers
        .iter()
        .filter_map(|(k, v)| {
            v.to_str()
                .ok()
                .map(|v| (k.as_str().to_string(), v.to_string()))
        })
        .collect()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::images::{ImagesStopReason, get_image_model, openrouter_image_models};
    use crate::model::Modality;

    fn nano_banana() -> ImagesModel {
        get_image_model("openrouter", "google/gemini-2.5-flash-image").expect("nano banana")
    }
    fn flux() -> ImagesModel {
        get_image_model("openrouter", "black-forest-labs/flux.2-flex").expect("flux")
    }

    #[test]
    fn build_params_encodes_modalities_and_content() {
        let model = nano_banana(); // emits text+image
        let ctx = ImagesContext {
            input: vec![
                Content::text("a cat"),
                Content::Image {
                    data: "QUJD".into(),
                    mime_type: "image/png".into(),
                },
            ],
        };
        let params = build_params(&model, &ctx);
        assert_eq!(params["model"], model.id);
        assert_eq!(params["stream"], false);
        assert_eq!(params["modalities"], json!(["image", "text"]));
        let content = &params["messages"][0]["content"];
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "a cat");
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(content[1]["image_url"]["url"], "data:image/png;base64,QUJD");

        // An image-only model omits "text" from modalities.
        let image_only = build_params(&flux(), &ctx);
        assert_eq!(image_only["modalities"], json!(["image"]));
    }

    #[test]
    fn parse_response_decodes_text_and_data_url_images() {
        let model = nano_banana();
        let body = json!({
            "id": "gen-123",
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 10,
                "prompt_tokens_details": { "cached_tokens": 20, "cache_write_tokens": 5 },
            },
            "choices": [{
                "message": {
                    "content": "here you go",
                    "images": [
                        { "image_url": { "url": "data:image/png;base64,QUJD" } },
                        { "image_url": "data:image/jpeg;base64,WFla" },
                        { "image_url": { "url": "https://example.test/not-data.png" } },
                    ],
                },
            }],
        });
        let out = parse_response(&model, &body);
        assert_eq!(out.response_id.as_deref(), Some("gen-123"));
        assert_eq!(out.stop_reason, ImagesStopReason::Stop);
        // text + two decoded images (the https URL is skipped).
        assert_eq!(out.output.len(), 3);
        assert!(matches!(&out.output[0], Content::Text { text, .. } if text == "here you go"));
        match &out.output[1] {
            Content::Image { data, mime_type } => {
                assert_eq!(mime_type, "image/png");
                assert_eq!(data, "QUJD");
            }
            other => panic!("expected image, got {other:?}"),
        }
        match &out.output[2] {
            Content::Image { mime_type, .. } => assert_eq!(mime_type, "image/jpeg"),
            other => panic!("expected image, got {other:?}"),
        }

        // Usage split: cacheRead = 20-5 = 15; input = 100-15-5 = 80; output = 10.
        let usage = out.usage.expect("usage");
        assert_eq!(usage.input, 80);
        assert_eq!(usage.output, 10);
        assert_eq!(usage.cache_read, 15);
        assert_eq!(usage.cache_write, 5);
        assert_eq!(usage.total_tokens, 80 + 10 + 15 + 5);
    }

    #[test]
    fn parse_data_url_matches_pi_regex_semantics() {
        assert_eq!(
            parse_data_url("data:image/png;base64,QUJD"),
            Some(("image/png".to_string(), "QUJD".to_string()))
        );
        // Non-data URL, empty data, and a non-base64 data URL are skipped.
        assert_eq!(parse_data_url("https://example.test/x.png"), None);
        assert_eq!(parse_data_url("data:image/png;base64,"), None);
        assert_eq!(parse_data_url("data:image/png,QUJD"), None);
    }

    #[test]
    fn every_catalog_model_emits_image_output() {
        assert!(
            openrouter_image_models()
                .iter()
                .all(|m| m.output.contains(&Modality::Image))
        );
    }

    /// Behaviour-check vs Pi: the image request path now runs through the shared proxy resolver
    /// (`build_client_for_target` → `resolveHttpProxyUrlForTarget`, node-http-proxy.ts:92-112),
    /// uniformly with the six text wire protocols. Proven by the SOCKS-rejection semantics: a
    /// `socks5://` proxy in `options.env` is surfaced as an error BEFORE any request is sent (Pi
    /// throws `UNSUPPORTED_PROXY_PROTOCOL_MESSAGE`, node-http-proxy.ts:106-108). The previous bare
    /// `build_client()` skipped the resolver entirely, so this error could never arise.
    #[tokio::test]
    async fn image_request_applies_proxy_resolver_and_rejects_socks() {
        let model = nano_banana();
        let ctx = ImagesContext {
            input: vec![Content::text("a small red circle")],
        };
        // Pin the whole proxy quartet in the overlay rather than inheriting any of it.
        //
        // `get_proxy_for_url` consults `{scheme}_proxy` BEFORE `all_proxy`, and an overlay only
        // wins where it supplies a non-empty value — an absent key falls through to the ambient env
        // (Pi's `env?.[n] || process.env[n]`; an empty overlay value falls through too, so emptiness
        // cannot mask). So on a host with an ambient `https_proxy` — this project's CI container has
        // one — the real proxy was selected before `all_proxy` was ever read, the request was sent
        // for real, and the SOCKS rejection this test exists to pin never happened. Setting
        // `https_proxy` too keeps SOCKS the resolved proxy whichever key wins, and `no_proxy` names
        // a host that is not the target so an ambient exemption cannot skip proxying altogether.
        let env: crate::auth::types::ProviderEnv = [
            ("all_proxy".to_string(), "socks5://proxy.local:1080".to_string()),
            ("https_proxy".to_string(), "socks5://proxy.local:1080".to_string()),
            ("no_proxy".to_string(), "never-matches.invalid".to_string()),
        ]
        .into_iter()
        .collect();
        let opts = ImagesOptions {
            api_key: Some("test-key".to_string()),
            env: Some(env),
            ..Default::default()
        };
        let err = run(&model, &ctx, &opts)
            .await
            .expect_err("socks proxy must be rejected before sending");
        match err {
            GenError::Message(m) => assert!(
                m.contains("SOCKS and PAC"),
                "expected SOCKS rejection from the resolver, got: {m}"
            ),
            GenError::Aborted => panic!("expected a SOCKS-rejection message, got Aborted"),
        }
    }

    /// Live smoke test against the real OpenRouter image API. Ignored by default; run with
    /// `OPENROUTER_API_KEY` set: `cargo test -p cyrup-provider -- --ignored live_openrouter_images`.
    #[tokio::test]
    #[ignore = "hits the real OpenRouter API; requires OPENROUTER_API_KEY"]
    async fn live_openrouter_images_returns_image() {
        let Ok(key) = std::env::var("OPENROUTER_API_KEY") else {
            eprintln!("skipping: OPENROUTER_API_KEY not set");
            return;
        };
        let model = nano_banana();
        let ctx = ImagesContext {
            input: vec![Content::text("a small red circle on white")],
        };
        let opts = ImagesOptions {
            api_key: Some(key),
            ..Default::default()
        };
        let out = factory().generate_images(&model, &ctx, &opts).await;
        assert_eq!(
            out.stop_reason,
            ImagesStopReason::Stop,
            "error: {:?}",
            out.error_message
        );
        assert!(
            out.output
                .iter()
                .any(|c| matches!(c, Content::Image { .. })),
            "expected an image, got {:?}",
            out.output
        );
    }
}
