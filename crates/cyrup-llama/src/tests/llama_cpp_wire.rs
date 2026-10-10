//! Conformance: the llama.cpp router wire, through the real client (EXT-100), and this crate's
//! fake server's drift guard (EXT-108).
//!
//! The wire itself is no longer defined here. EXT-108 lifted it, unchanged in shape and with every
//! citation re-derived against llama.cpp `b11436`, into the test-only crate `cyrup-llama-cpp-wire`
//! (`crates/cyrup-llama-cpp-wire/src/router.rs`), so the OTHER fakes in this workspace — the seam
//! suite's (`cyrup-it/tests/llama/fake.rs`) and the classifier api's — can answer from the same
//! definition instead of keeping copies. Read that crate's docs for the pin, the floor and why it
//! is a crate of its own.
//!
//! The glob re-export keeps the names this module and [`super::fake_server`] have always used.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

pub use cyrup_llama_cpp_wire::router::*;

// ================================================================================================
// Conformance: the real shapes above, through the real client.
// ================================================================================================

#[cfg(test)]
mod conformance {
    use std::time::Duration;

    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    use super::{
        child_props, download_settled_event, downloading_entry, failed_entry, invalid_request,
        load_progress, loaded_entry, mmproj_load_progress, router_props, sse_event,
        status_change_event, unloaded_preset_entry,
    };
    use crate::client::{
        LlamaClient, LlamaModelStatus, ProgressField, parse_download_progress, parse_load_progress,
    };
    use crate::model::model_is_selectable;
    use crate::tests::fake_server::{FakeLlamaServer, Reply};

    fn never() -> CancellationToken {
        CancellationToken::new()
    }

    async fn client(server: &FakeLlamaServer) -> LlamaClient {
        LlamaClient::new(server.url(), None, None)
            .await
            .expect("a client")
            .with_request_timeout(Duration::from_secs(5))
    }

    /// A whole real catalog — every field `get_router_models` sends, for a loaded model, an
    /// unloaded preset, a failed entry and a downloading one — parses, and nothing the client
    /// ignores disturbs what it reads.
    ///
    /// The existing tests feed `{"id":…,"status":{"value":…}}`; this one feeds what the router
    /// actually writes, so a field the server adds in a shape the deserializer cannot take is
    /// caught here.
    #[tokio::test]
    async fn the_whole_real_catalog_parses() {
        let server = FakeLlamaServer::start().await;
        server.set_models(vec![
            loaded_entry("qwen", &["--ctx-size", "16384"]),
            unloaded_preset_entry("preset-model", &["-c", "8192"]),
            failed_entry("broken", 3),
            downloading_entry("owner/repo:Q4_K_M"),
        ]);
        let models = client(&server).await.list(false, &never()).await.unwrap();
        assert_eq!(models.len(), 4, "{models:?}");

        let loaded = &models[0];
        assert_eq!(loaded.id, "qwen");
        assert_eq!(loaded.status.value, LlamaModelStatus::Loaded);
        assert_eq!(
            loaded.status.args.as_deref(),
            Some(["--ctx-size".to_string(), "16384".to_string()].as_slice())
        );
        assert_eq!(loaded.source.as_deref(), Some("models_dir"));
        assert_eq!(
            loaded.meta.as_ref().and_then(|meta| meta.n_ctx),
            Some(32_768),
            "the child's `meta` reaches the catalog through the loaded_info merge"
        );
        assert_eq!(
            loaded.meta.as_ref().and_then(|meta| meta.n_ctx_train),
            Some(262_144)
        );
        assert_eq!(
            loaded
                .architecture
                .as_ref()
                .and_then(|a| a.input_modalities.as_deref()),
            Some(["text".to_string()].as_slice())
        );

        let preset = &models[1];
        assert_eq!(preset.status.value, LlamaModelStatus::Unloaded);
        assert_eq!(preset.source.as_deref(), Some("preset"));
        assert!(
            preset.meta.is_none(),
            "an unloaded entry carries no child meta; got {:?}",
            preset.meta
        );
        assert!(
            model_is_selectable(preset, true),
            "an unloaded `preset` entry is the autoload candidate"
        );

        let failed = &models[2];
        assert_eq!(failed.status.failed, Some(true));
        assert!(failed.status.exit_code.is_some(), "{failed:?}");
        assert!(!model_is_selectable(failed, true));

        let downloading = &models[3];
        assert_eq!(downloading.status.value, LlamaModelStatus::Downloading);
        assert!(
            downloading.status.progress.is_none(),
            "{}",
            super::THE_CATALOG_CARRIES_NO_DOWNLOAD_PROGRESS
        );
        assert!(!model_is_selectable(downloading, true));
    }

    /// `"downloaded"` is a status the router emits (`server-models.h:52-62`) and pi's union does
    /// not name (`client.ts:1`). It must survive as an unknown status rather than collapse onto a
    /// named one, or a transient post-download entry would be offered as a model.
    #[tokio::test]
    async fn every_real_status_string_round_trips_and_only_three_are_selectable() {
        let server = FakeLlamaServer::start().await;
        server.set_models(
            super::STATUS_VALUES
                .iter()
                .map(|status| {
                    json!({
                        "id": *status,
                        "status": { "value": *status, "args": [] },
                        "source": "preset",
                    })
                })
                .collect(),
        );
        let models = client(&server).await.list(false, &never()).await.unwrap();
        for (model, status) in models.iter().zip(super::STATUS_VALUES) {
            assert_eq!(
                model.status.value.as_str(),
                status,
                "{status} must survive the round trip"
            );
        }
        assert_eq!(
            models
                .iter()
                .filter(|model| model_is_selectable(model, true))
                .map(|model| model.id.clone())
                .collect::<Vec<_>>(),
            vec![
                "unloaded".to_string(),
                "loaded".to_string(),
                "sleeping".to_string()
            ],
            "`downloaded` and `downloading` are not offerable; `unloaded` is, as an autoload preset"
        );
    }

    /// The ROUTER's `/props` (no `model`) yields `models_autoload`; the CHILD's (`?model=`, a
    /// proxied answer) yields `chat_template` and NO `models_autoload`, because that field is
    /// router-only (`server-models.cpp:1996-2002` vs `server-context.cpp:4969-4997`).
    #[tokio::test]
    async fn router_props_and_child_props_each_yield_their_own_fields() {
        let server = FakeLlamaServer::start().await;
        server.set_props(router_props(true));
        let client = client(&server).await;
        let router = client.props(None, &never()).await.unwrap();
        assert_eq!(router.models_autoload, Some(true));
        assert_eq!(
            router.chat_template, None,
            "the router's props carry no chat template"
        );

        server.set_props(child_props("{%- if enable_thinking %}<think>{%- endif %}"));
        let child = client.props(Some("qwen"), &never()).await.unwrap();
        assert_eq!(
            child.models_autoload, None,
            "a child's props have no models_autoload"
        );
        assert!(
            child
                .chat_template
                .as_deref()
                .is_some_and(|template| template.contains("enable_thinking")),
            "{child:?}"
        );
        let asked = server.requests_to("GET", "/props");
        assert!(
            asked
                .last()
                .and_then(|request| request.query().map(str::to_string))
                .is_some_and(
                    |query| query.contains("model=qwen") && query.contains("autoload=false")
                ),
            "a per-model props asks the router to proxy WITHOUT loading; got {asked:?}"
        );
    }

    /// A router error body is `{"error":{code,message,type}}` with the status equal to `code`
    /// (`server-common.cpp:36-78`, `server-models.cpp:1915-1918`), and the client surfaces
    /// `error.message` verbatim — not its `llama.cpp returned HTTP <n>` fallback.
    #[tokio::test]
    async fn a_real_router_error_surfaces_its_message() {
        let server = FakeLlamaServer::start().await;
        // `post_router_models_unload` for a model that is not running (`server-models.cpp:2156`).
        let (status, body) = invalid_request("model is not running");
        server.respond("POST", "/models/unload", Reply::Json(status, body));
        let error = client(&server)
            .await
            .unload("qwen", &never())
            .await
            .expect_err("a 400 fails");
        assert_eq!(error.to_string(), "model is not running", "{error:?}");
    }

    /// The two load-progress payloads a child really sends, through the real SSE framing. The
    /// `mmproj` one carries `stage` alone — no `stages`, no `value` — which is why the parser
    /// falls back from `current` to `stage`.
    #[test]
    fn both_real_load_progress_payloads_are_understood() {
        let staged = status_change_event(
            "qwen",
            "loading",
            Some(load_progress(
                &["text_model", "mmproj_model"],
                "text_model",
                0.5,
            )),
        );
        let data = staged.get("data").expect("data");
        let parsed = parse_load_progress(data).expect("staged progress");
        assert_eq!(parsed.message, "Loading text model");
        assert_eq!(
            parsed.ratio,
            ProgressField::Set(0.25),
            "stage 0 of 2, half done"
        );

        let mmproj = status_change_event("qwen", "loading", Some(mmproj_load_progress()));
        let data = mmproj.get("data").expect("data");
        let parsed = parse_load_progress(data).expect("mmproj progress");
        assert_eq!(
            parsed.message, "Loading mmproj model",
            "the child names the stage in `stage`, not `current` (server-context.cpp:1270-1272)"
        );
    }

    /// `download_progress`'s `data` is the whole `loaded_info`, so the per-file map is NESTED under
    /// `progress` (`server-models.cpp:1389-1396`) — the branch of `parseDownloadProgress` that
    /// looks one level down.
    #[test]
    fn the_real_download_progress_event_is_the_nested_form() {
        let event = super::download_progress_event(
            "owner/repo",
            &[
                ("https://example/a.gguf", 100, 400),
                ("https://example/b.gguf", 100, 400),
            ],
        );
        let data = event.get("data").expect("data");
        assert!(
            data.get("progress").is_some(),
            "the real event nests its file map under `progress`: {data}"
        );
        let parsed = parse_download_progress(data).expect("download progress");
        assert_eq!(parsed.ratio, ProgressField::Set(0.25));
        assert_eq!(
            parsed.detail,
            ProgressField::Set("200 B / 800 B".to_string())
        );
    }

    /// The three real SSE payloads, through the real reader on a real socket: a rescan broadcasts
    /// `models_reload` for the pseudo-model `"*"` with NO `data` key at all
    /// (`server-models.cpp:1026`, `:686-696`), and a settled download broadcasts an EMPTY `data`
    /// object (`:1400-1402`). The decoder must take all three; a `data`-less frame in particular is
    /// the one upstream's `isModelEvent` guard could reject.
    #[tokio::test]
    async fn the_real_sse_payloads_decode_including_the_one_without_data() {
        let server = FakeLlamaServer::start().await;
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = std::sync::Arc::clone(&seen);
        let client = client(&server).await;
        let cancel = never();
        let watching = tokio::spawn({
            let cancel = cancel.clone();
            async move {
                client
                    .watch(
                        move |event| {
                            sink.lock().expect("the sink").push((
                                event.model,
                                event.event,
                                event.data,
                            ));
                        },
                        &cancel,
                    )
                    .await
            }
        });
        server.wait_for_sse(1).await;
        server.run(vec![
            crate::tests::fake_server::Step::Sse(sse_event("*", "models_reload", None)),
            crate::tests::fake_server::Step::Sse(download_settled_event("owner/repo", true)),
            crate::tests::fake_server::Step::Sse(download_settled_event("owner/repo", false)),
            crate::tests::fake_server::Step::CloseSse,
        ]);
        watching.await.expect("the watcher").expect("a clean end");
        let seen = seen.lock().expect("the sink").clone();
        assert_eq!(
            seen,
            vec![
                ("*".to_string(), "models_reload".to_string(), None),
                (
                    "owner/repo".to_string(),
                    "download_finished".to_string(),
                    Some(json!({}))
                ),
                (
                    "owner/repo".to_string(),
                    "download_failed".to_string(),
                    Some(json!({}))
                ),
            ],
            "a `models_reload` frame has no `data` key, and a settled download has an empty one"
        );
    }

    /// Because `download_failed` carries an empty `data` (`server-models.cpp:1400-1402`), the
    /// failure a real router produces has NO message of its own, and what the user sees is the
    /// client's own `"Download failed"` (`client.rs:1289`) — not a server string.
    #[tokio::test]
    async fn a_real_download_failure_reports_the_clients_own_message() {
        let server = FakeLlamaServer::start().await;
        server.on_download(
            "owner/repo",
            vec![
                crate::tests::fake_server::Step::WaitForSse(1),
                crate::tests::fake_server::Step::Sse(download_settled_event("owner/repo", false)),
            ],
        );
        let error = client(&server)
            .await
            .download_and_wait("owner/repo", &|_| {}, &never())
            .await
            .expect_err("a failed download fails");
        assert_eq!(
            error.to_string(),
            "Download failed",
            "the router says nothing, so the message is the client's"
        );
    }

    /// Send one raw HTTP/1.1 request and read the whole answer: the fake closes every connection,
    /// so the close delimits the body. Raw, not through `LlamaClient` or `reqwest`, because the
    /// guard below is about the exact BYTES the fake writes.
    async fn raw(url: &str, method: &str, path: &str, extra: &str, body: &str) -> (u16, String) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let addr = url.trim_start_matches("http://");
        let mut socket = tokio::net::TcpStream::connect(addr).await.expect("connect");
        let request = format!(
            "{method} {path} HTTP/1.1\r\nhost: {addr}\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(request.as_bytes()).await.expect("write");
        let mut answer = String::new();
        socket.read_to_string(&mut answer).await.expect("read");
        let (head, body) = answer.split_once("\r\n\r\n").expect("a head");
        let status = head
            .split(' ')
            .nth(1)
            .and_then(|status| status.parse().ok())
            .expect("a status");
        (status, body.to_string())
    }

    /// EXT-108's DRIFT GUARD for this crate's fake: what [`FakeLlamaServer`] writes on a real
    /// socket is, byte for byte, the pinned llama.cpp answer in `cyrup_llama_cpp_wire::golden`.
    /// The fake answers from `cyrup_llama_cpp_wire::router`, so a change to any definition it
    /// serves — the unknown-route 404, the 401, `{"success":true}`, the catalog envelope (empty,
    /// and over EXT-110's decision entries), the router props, the SSE framing — fails this test until the golden moves with it.
    ///
    /// It replaces EXT-100's `the_fake_server_answers_with_these_definitions`, which drove the
    /// real client and so could only see what the client parses, and which asserted the
    /// definitions against literals rather than the fake against anything.
    #[tokio::test]
    async fn drift_guard_the_fake_writes_the_golden_bytes() {
        use cyrup_llama_cpp_wire::golden;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let server = FakeLlamaServer::start().await;
        let url = server.url().to_string();
        let answer = |status: u16, body: &str| (status, body.to_string());

        assert_eq!(
            raw(&url, "GET", "/no-such-route", "", "").await,
            answer(404, golden::FILE_NOT_FOUND),
            "an unknown route is llama.cpp's own 404 literal"
        );
        assert_eq!(
            raw(&url, "GET", "/models", "", "").await,
            answer(200, golden::EMPTY_MODELS)
        );
        assert_eq!(
            raw(&url, "GET", "/props", "", "").await,
            answer(200, golden::ROUTER_PROPS_AUTOLOAD)
        );
        // EXT-110's decision-model fixture, as the catalog the fake serves.
        server.set_models(vec![
            super::decision_entry("kev", "loaded"),
            super::decision_entry("laya", "unloaded"),
        ]);
        assert_eq!(
            raw(&url, "GET", "/models", "", "").await,
            answer(200, golden::MODELS_DECISION_KEV_LOADED_LAYA_UNLOADED)
        );
        server.set_models(Vec::new());
        for path in ["/models/load", "/models/unload", "/models"] {
            assert_eq!(
                raw(&url, "POST", path, "", r#"{"model":"qwen"}"#).await,
                answer(200, golden::SUCCESS),
                "POST {path}"
            );
        }

        // One SSE frame, read off an open `GET /models/sse`.
        let addr = url.trim_start_matches("http://").to_string();
        let mut stream = tokio::net::TcpStream::connect(&addr)
            .await
            .expect("connect");
        stream
            .write_all(format!("GET /models/sse HTTP/1.1\r\nhost: {addr}\r\n\r\n").as_bytes())
            .await
            .expect("write");
        server.wait_for_sse(1).await;
        server.broadcast(sse_event("*", "models_reload", None));
        let mut seen = String::new();
        loop {
            if let Some((_, frame)) = seen.split_once("\r\n\r\n")
                && frame.ends_with("\n\n")
            {
                assert_eq!(frame, golden::SSE_MODELS_RELOAD_FRAME);
                break;
            }
            let mut chunk = [0_u8; 1024];
            let read = stream.read(&mut chunk).await.expect("read");
            assert!(read > 0, "the stream closed before a frame: {seen:?}");
            seen.push_str(&String::from_utf8_lossy(&chunk[..read]));
        }

        server.require_bearer("sk-guard");
        assert_eq!(
            raw(&url, "GET", "/models", "", "").await,
            answer(401, golden::INVALID_API_KEY),
            "a missing key is the middleware's 401 literal"
        );
        assert_eq!(
            raw(
                &url,
                "GET",
                "/models",
                "authorization: Bearer sk-guard\r\n",
                ""
            )
            .await,
            answer(200, golden::EMPTY_MODELS)
        );
    }
}

// ================================================================================================
// EXT-110: decision models, through the real controller.
// ================================================================================================

#[cfg(test)]
mod decision_models {
    use std::sync::{Arc, Mutex};

    use serde_json::json;

    use cyrup_core::CancelToken;
    use cyrup_provider::auth::{Credential, InMemoryCredentialStore, ProviderEnv};
    use cyrup_provider::{AnyModel, Provider};

    use super::{decision_entry, loaded_entry};
    use crate::client::LlamaModelInfo;
    use crate::error::LlamaError;
    use crate::provider::{
        CatalogEntry, CatalogPublication, CatalogPublisher, LlamaController,
        LlamaControllerOptions, LlamaRefreshContext, RegisterProviderFn, SetCatalogOptions,
    };
    use crate::tests::fake_server::FakeLlamaServer;

    /// Persist, then update, then answer `true` — pi's test `publish`.
    #[derive(Default)]
    struct Publisher(Mutex<Option<CatalogEntry>>);

    #[async_trait::async_trait]
    impl CatalogPublisher for Publisher {
        async fn publish(&self, publication: CatalogPublication) -> Result<bool, LlamaError> {
            if let Some(entry) = publication.persist {
                *self.0.lock().unwrap() = Some(entry);
            }
            if let Some(update) = publication.update {
                update();
            }
            Ok(true)
        }
    }

    fn controller() -> LlamaController {
        let register: RegisterProviderFn = Arc::new(|_: Arc<dyn Provider>| Ok(()));
        LlamaController::new(LlamaControllerOptions::new(
            Arc::new(InMemoryCredentialStore::new()),
            register,
        ))
    }

    fn chat_ids(controller: &LlamaController) -> Vec<String> {
        controller
            .provider()
            .models()
            .iter()
            .map(|model| model.id.as_str().to_string())
            .collect()
    }

    fn classifier_ids(controller: &LlamaController) -> Vec<String> {
        controller
            .provider()
            .get_all_models()
            .iter()
            .filter(|model| matches!(model, AnyModel::Classifier(_)))
            .map(|model| model.id().to_string())
            .collect()
    }

    /// pi `exposes decision models reported by the catalog as native System One classifiers`
    /// (`llama-extension.test.ts`, f6127a1bf), its catalog half: a real router catalog with a
    /// text model, a loaded and a sleeping decision model and a pre-0.6.0 entry with no
    /// `architecture`. The decision models are not chat models, their `/props` is never asked
    /// for, and they stay classifiers.
    #[tokio::test]
    async fn a_refresh_lists_decision_models_as_classifiers_only_and_never_reads_their_props() {
        let server = FakeLlamaServer::start().await;
        let mut legacy = loaded_entry("legacy", &[]);
        legacy.as_object_mut().unwrap().remove("architecture");
        server.set_models(vec![
            loaded_entry("qwen", &[]),
            decision_entry("kev", "loaded"),
            decision_entry("laya", "sleeping"),
            legacy,
        ]);
        server.set_props(json!({}));
        let mut env = ProviderEnv::new();
        env.insert("LLAMA_BASE_URL".to_string(), server.url().to_string());
        let credential = Credential::ApiKey {
            key: Some("local".to_string()),
            env: Some(env),
        };
        let publisher = Publisher::default();
        let controller = controller();
        let cancel = CancelToken::new();
        controller
            .provider()
            .refresh(&LlamaRefreshContext {
                credential: Some(&credential),
                stored: None,
                publisher: &publisher,
                allow_network: true,
                cancel: &cancel,
            })
            .await
            .unwrap();

        let mut props: Vec<String> = server
            .requests_to("GET", "/props")
            .iter()
            .map(|request| request.query().unwrap_or_default().to_string())
            .collect();
        props.sort();
        assert_eq!(
            props,
            [
                "model=legacy&autoload=false".to_string(),
                "model=qwen&autoload=false".to_string()
            ],
            "only the chat models' props are read; `kev` is loaded and still not asked"
        );
        assert_eq!(chat_ids(&controller), ["qwen", "legacy"]);
        assert_eq!(
            classifier_ids(&controller),
            ["qwen", "kev", "laya", "legacy"]
        );
        let persisted: Vec<(String, &'static str)> = publisher
            .0
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .models
            .iter()
            .map(|model| {
                let kind = match model {
                    AnyModel::Chat(_) => "chat",
                    AnyModel::Classifier(_) => "classifier",
                    AnyModel::Image(_) => "image",
                };
                (model.id().to_string(), kind)
            })
            .collect();
        assert_eq!(
            persisted,
            [
                ("qwen".to_string(), "chat"),
                ("legacy".to_string(), "chat"),
                ("qwen".to_string(), "classifier"),
                ("kev".to_string(), "classifier"),
                ("laya".to_string(), "classifier"),
                ("legacy".to_string(), "classifier"),
            ]
        );
    }

    /// pi `lists decision models that also output text for chat` (`llama-extension.test.ts`,
    /// f6127a1bf): `isChatModel` keeps a model that reports `"text"` beside `"decisions"`. No
    /// llama.cpp release emits that pair (`server-common.cpp:150-163` @b11436 answers one or the
    /// other), so this pins pi's predicate rather than a router shape.
    #[test]
    fn set_catalog_keeps_a_text_and_decisions_model_in_both_lists() {
        let catalog: Vec<LlamaModelInfo> = [
            json!({ "id": "decide", "status": { "value": "sleeping" },
                    "architecture": { "output_modalities": ["decisions"] } }),
            json!({ "id": "hybrid", "status": { "value": "loaded" },
                    "architecture": { "output_modalities": ["text", "decisions"] } }),
        ]
        .into_iter()
        .map(|entry| serde_json::from_value(entry).unwrap())
        .collect();
        let controller = controller();
        controller
            .set_catalog(
                &catalog,
                "http://localhost:8080",
                SetCatalogOptions::default(),
            )
            .unwrap();
        assert_eq!(chat_ids(&controller), ["hybrid"]);
        assert_eq!(classifier_ids(&controller), ["decide", "hybrid"]);
    }
}
