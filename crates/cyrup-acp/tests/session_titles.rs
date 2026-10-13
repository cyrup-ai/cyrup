//! What an editor client can see about a session: its title and its context window
//! (`ACP-299`…`ACP-302`), over the real connection and a scripted provider.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod support;

use std::path::{Path, PathBuf};

use cyrup_core::StopReason;
use cyrup_provider::faux::{
    FauxConfig, FauxModelDefinition, FauxProvider, faux_assistant_message, faux_text,
};
use cyrup_session::layout::encode_cwd;
use serde_json::{Value, json};
use support::{Harness, dump, in_local_set, index_of_response, is_response_to, updates_of};

fn ok() -> cyrup_core::AssistantMessage {
    faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)
}

fn text(t: &str) -> Value {
    json!({"type": "text", "text": t})
}

/// The `session_info_update`s that carry a title. The turn also sends one whose only payload is
/// the queue-depth `_meta` (`ACP-124`); that one is not a title.
fn titles(frames: &[Value]) -> Vec<&Value> {
    updates_of(frames, "session_info_update")
        .into_iter()
        .filter(|f| f["params"]["update"].get("title").is_some())
        .collect()
}

fn title_values(frames: &[Value]) -> Vec<Value> {
    titles(frames)
        .into_iter()
        .map(|f| f["params"]["update"]["title"].clone())
        .collect()
}

async fn prompt(h: &mut Harness, session: &str, blocks: Vec<Value>) -> Vec<Value> {
    let id = h
        .client
        .request(
            "session/prompt",
            json!({"sessionId": session, "prompt": blocks}),
        )
        .await;
    let frames = h.client.drain_until(|v| is_response_to(v, id)).await;
    let response = frames.iter().find(|v| is_response_to(v, id)).unwrap();
    assert!(
        response.get("error").is_none(),
        "the prompt failed: {response}"
    );
    frames
}

/// The session file the harness wrote for `id`.
fn session_file(h: &Harness, id: &str) -> PathBuf {
    let mut found = None;
    for dir in std::fs::read_dir(h.root().path()).unwrap() {
        for file in std::fs::read_dir(dir.unwrap().path()).unwrap() {
            let path = file.unwrap().path();
            if path.to_string_lossy().contains(id) {
                found = Some(path);
            }
        }
    }
    found.unwrap_or_else(|| panic!("no session file for {id}"))
}

/// Every `session_info` entry's `name`, in file order.
fn stored_names(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e["type"] == "session_info")
        .map(|e| e["name"].clone())
        .collect()
}

/// A transcript of `entries` (each `("user" | "assistant" | "info", text)`), one branch.
fn seed(h: &Harness, id: &str, entries: &[(&str, Option<&str>)]) -> PathBuf {
    let cwd = h.cwd.path();
    let dir = h.root().path().join(encode_cwd(cwd));
    std::fs::create_dir_all(&dir).unwrap();
    let ts = "2026-01-01T00:00:00.000Z";
    let usage = json!({
        "input": 10, "output": 2, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 12,
        "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
    });
    let mut lines = vec![
        json!({"type": "session", "version": 3, "id": id, "timestamp": ts, "cwd": cwd}).to_string(),
    ];
    let mut parent = Value::Null;
    for (n, (kind, body)) in entries.iter().enumerate() {
        let eid = format!("e{n:07}");
        let entry = match *kind {
            "info" => json!({"type": "session_info", "id": eid, "parentId": parent,
                             "timestamp": ts, "name": body}),
            "user" => json!({"type": "message", "id": eid, "parentId": parent, "timestamp": ts,
                             "message": {"role": "user", "content": [{"type": "text", "text": body}],
                                         "timestamp": 1_767_600_000_000u64}}),
            _ => json!({"type": "message", "id": eid, "parentId": parent, "timestamp": ts,
                        "message": {"role": "assistant", "content": [{"type": "text", "text": body}],
                                    "api": "anthropic-messages", "provider": "anthropic",
                                    "model": "claude", "usage": usage, "stopReason": "stop",
                                    "timestamp": 1_767_600_000_000u64}}),
        };
        lines.push(entry.to_string());
        parent = Value::String(eid);
    }
    let path = dir.join(format!("2026-01-01T00-00-00-000Z_{id}.jsonl"));
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    path
}

async fn initialize(h: &mut Harness) {
    let id = h
        .client
        .request(
            "initialize",
            json!({"protocolVersion": 1, "clientCapabilities": {}}),
        )
        .await;
    h.client.response_to(id).await;
}

/// `session/load` of `id`; every frame up to and including the `available_commands_update` that
/// closes the follow-up, and the request id.
async fn load(h: &mut Harness, id: &str) -> (Vec<Value>, i64) {
    let cwd = h.cwd.path().to_string_lossy().to_string();
    let req = h
        .client
        .request(
            "session/load",
            json!({"sessionId": id, "cwd": cwd, "mcpServers": []}),
        )
        .await;
    let frames = h
        .client
        .drain_until(|v| {
            v["params"]["update"]["sessionUpdate"] == "available_commands_update"
                || (is_response_to(v, req) && v.get("error").is_some())
        })
        .await;
    (frames, req)
}

/// `session/new`; every frame up to the closing `available_commands_update`, and the session id.
async fn new_session(h: &mut Harness) -> (Vec<Value>, i64, String) {
    let cwd = h.cwd.path().to_string_lossy().to_string();
    let req = h
        .client
        .request("session/new", json!({"cwd": cwd, "mcpServers": []}))
        .await;
    let frames = h
        .client
        .drain_until(|v| v["params"]["update"]["sessionUpdate"] == "available_commands_update")
        .await;
    let response = frames.iter().find(|v| is_response_to(v, req)).unwrap();
    let id = response["result"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    (frames, req, id)
}

fn position(frames: &[Value], kind: &str) -> usize {
    frames
        .iter()
        .position(|v| v["params"]["update"]["sessionUpdate"] == kind)
        .unwrap_or_else(|| panic!("no {kind}:\n{}", dump(frames)))
}

// --- ACP-299 ------------------------------------------------------------------------------------

/// ACP-299 — the first prompt names a new session once, from its text blocks normalised; the name
/// is persisted, and a second prompt names nothing.
#[test]
fn acp299_the_first_prompt_titles_a_new_session_once() {
    in_local_set(|| async {
        let mut h = Harness::start(vec![ok(), ok()]).await;
        let session = h.open_session().await;

        let first = prompt(
            &mut h,
            &session,
            vec![text("  Fix\n\nthe   bug  "), text("now")],
        )
        .await;
        assert_eq!(
            title_values(&first),
            [json!("Fix the bug now")],
            "{}",
            dump(&first)
        );
        // Before the turn's own output: the client names the thread as soon as it starts.
        let title_at = first
            .iter()
            .position(|f| f["params"]["update"].get("title").is_some())
            .unwrap();
        assert!(
            title_at < position(&first, "agent_message_chunk"),
            "{}",
            dump(&first)
        );
        assert_eq!(
            stored_names(&session_file(&h, &session)).last(),
            Some(&json!("Fix the bug now"))
        );

        let second = prompt(&mut h, &session, vec![text("and something else")]).await;
        assert!(title_values(&second).is_empty(), "{}", dump(&second));
    });
}

/// ACP-299 — a session `/name`d before its first prompt keeps that name.
#[test]
fn acp299_a_name_set_before_the_first_prompt_is_kept() {
    in_local_set(|| async {
        let mut h = Harness::start(vec![ok()]).await;
        let session = h.open_session().await;

        let named = prompt(&mut h, &session, vec![text("/name keep me")]).await;
        assert_eq!(title_values(&named), [json!("keep me")]);

        let first = prompt(&mut h, &session, vec![text("Fix the bug")]).await;
        assert!(
            title_values(&first).iter().all(|t| t == "keep me"),
            "{}",
            dump(&first)
        );
        assert_eq!(
            stored_names(&session_file(&h, &session)).last(),
            Some(&json!("keep me"))
        );
    });
}

/// ACP-299 — an image-only first prompt produces no title and leaves the session waiting for one:
/// the next prompt with text names it. This is what makes the flag a flag and not "ran once".
#[test]
fn acp299_an_image_only_first_prompt_leaves_the_title_pending() {
    in_local_set(|| async {
        let mut h = Harness::start(vec![ok(), ok()]).await;
        let session = h.open_session().await;
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==";

        let image = prompt(
            &mut h,
            &session,
            vec![json!({"type": "image", "data": png, "mimeType": "image/png"})],
        )
        .await;
        assert!(title_values(&image).is_empty(), "{}", dump(&image));

        let words = prompt(&mut h, &session, vec![text("Now with words")]).await;
        assert_eq!(
            title_values(&words),
            [json!("Now with words")],
            "{}",
            dump(&words)
        );
    });
}

/// ACP-299 — a restored session is never auto-titled: it is not a `session/new` session.
#[test]
fn acp299_a_restored_session_is_never_auto_titled() {
    in_local_set(|| async {
        let mut h = Harness::start(vec![ok()]).await;
        initialize(&mut h).await;
        let path = seed(
            &h,
            "restored",
            &[("user", Some("old")), ("assistant", Some("a"))],
        );
        load(&mut h, "restored").await;

        let frames = prompt(&mut h, "restored", vec![text("brand new words")]).await;
        assert!(
            title_values(&frames).iter().all(|t| t != "brand new words"),
            "{}",
            dump(&frames)
        );
        assert!(!stored_names(&path).contains(&json!("brand new words")));
    });
}

// --- ACP-300 ------------------------------------------------------------------------------------

/// ACP-300 — `session/load` publishes the stored title, with no `updatedAt`, after the response
/// and before the command advertisement (the recorded placement).
#[test]
fn acp300_load_publishes_the_stored_title() {
    in_local_set(|| async {
        let mut h = Harness::start(vec![]).await;
        initialize(&mut h).await;
        seed(
            &h,
            "named",
            &[
                ("info", Some("saved")),
                ("user", Some("hi")),
                ("assistant", Some("a")),
            ],
        );
        let (frames, req) = load(&mut h, "named").await;

        let found = titles(&frames);
        assert_eq!(found.len(), 1, "{}", dump(&frames));
        let update = &found[0]["params"]["update"];
        assert_eq!(update["title"], "saved");
        assert!(update.get("updatedAt").is_none(), "{update}");
        let at = frames
            .iter()
            .position(|f| f["params"]["update"].get("title").is_some())
            .unwrap();
        assert!(index_of_response(&frames, req) < at, "{}", dump(&frames));
        assert!(
            at < position(&frames, "available_commands_update"),
            "{}",
            dump(&frames)
        );
    });
}

/// ACP-300 — the ladder: no name falls back to the first prompt, normalised; a cleared name falls
/// back to the first prompt and not to an older name; neither emits nothing.
#[test]
fn acp300_the_stored_title_falls_back_to_the_first_prompt() {
    in_local_set(|| async {
        let mut h = Harness::start(vec![]).await;
        initialize(&mut h).await;

        seed(
            &h,
            "unnamed",
            &[("user", Some("hello   world")), ("assistant", Some("a"))],
        );
        let (frames, _) = load(&mut h, "unnamed").await;
        assert_eq!(
            title_values(&frames),
            [json!("hello world")],
            "{}",
            dump(&frames)
        );

        seed(
            &h,
            "cleared",
            &[
                ("info", Some("old name")),
                ("user", Some("first ask")),
                ("assistant", Some("a")),
                ("info", None),
            ],
        );
        let (frames, _) = load(&mut h, "cleared").await;
        assert_eq!(
            title_values(&frames),
            [json!("first ask")],
            "{}",
            dump(&frames)
        );

        seed(&h, "empty", &[("assistant", Some("a"))]);
        let (frames, _) = load(&mut h, "empty").await;
        assert!(title_values(&frames).is_empty(), "{}", dump(&frames));
    });
}

/// ACP-300 — the prompt-path restore (no `session/load`) publishes the stored title too.
#[test]
fn acp300_a_prompt_that_restores_publishes_the_stored_title() {
    in_local_set(|| async {
        let mut h = Harness::start(vec![ok()]).await;
        initialize(&mut h).await;
        seed(
            &h,
            "named",
            &[
                ("info", Some("saved")),
                ("user", Some("hi")),
                ("assistant", Some("a")),
            ],
        );

        let frames = prompt(&mut h, "named", vec![text("again")]).await;
        assert_eq!(title_values(&frames), [json!("saved")], "{}", dump(&frames));
    });
}

// --- ACP-301 ------------------------------------------------------------------------------------

/// ACP-301 — a listing whose first user message is whitespace takes its title from the next one.
#[test]
fn acp301_a_whitespace_first_message_titles_from_the_next() {
    in_local_set(|| async {
        let mut h = Harness::start(vec![]).await;
        initialize(&mut h).await;
        seed(
            &h,
            "blank-first",
            &[
                ("user", Some("   ")),
                ("assistant", Some("a")),
                ("user", Some("real")),
                ("assistant", Some("b")),
            ],
        );
        let cwd = h.cwd.path().to_string_lossy().to_string();
        let id = h.client.request("session/list", json!({"cwd": cwd})).await;
        let listed = h.client.response_to(id).await;
        let rows = listed["result"]["sessions"].as_array().unwrap();
        let row = rows
            .iter()
            .find(|r| r["sessionId"] == "blank-first")
            .unwrap();
        assert_eq!(row["title"], "real", "{listed}");
    });
}

// --- ACP-302 ------------------------------------------------------------------------------------

/// ACP-302 — `session/new` publishes the context window after the response and before the command
/// advertisement.
#[test]
fn acp302_new_publishes_usage_before_the_commands() {
    in_local_set(|| async {
        let mut h = Harness::start(vec![]).await;
        initialize(&mut h).await;
        let (frames, req, _) = new_session(&mut h).await;
        let usage = updates_of(&frames, "usage_update");
        assert_eq!(usage.len(), 1, "{}", dump(&frames));
        assert_eq!(usage[0]["params"]["update"]["size"], 128_000);
        let at = position(&frames, "usage_update");
        assert!(index_of_response(&frames, req) < at, "{}", dump(&frames));
        assert!(
            at < position(&frames, "available_commands_update"),
            "{}",
            dump(&frames)
        );
    });
}

/// ACP-302 — `session/load` likewise.
#[test]
fn acp302_load_publishes_usage_before_the_commands() {
    in_local_set(|| async {
        let mut h = Harness::start(vec![]).await;
        initialize(&mut h).await;
        seed(
            &h,
            "loaded",
            &[("user", Some("hi")), ("assistant", Some("a"))],
        );
        let (frames, req) = load(&mut h, "loaded").await;
        let usage = updates_of(&frames, "usage_update");
        assert_eq!(usage.len(), 1, "{}", dump(&frames));
        let at = position(&frames, "usage_update");
        assert!(index_of_response(&frames, req) < at, "{}", dump(&frames));
        assert!(
            at < position(&frames, "available_commands_update"),
            "{}",
            dump(&frames)
        );
    });
}

/// ACP-302 — a model change publishes the NEW model's window.
#[test]
fn acp302_a_model_change_publishes_the_new_window() {
    in_local_set(|| async {
        let mut small = FauxModelDefinition::new("faux-small");
        small.context_window = 32_000;
        let faux = FauxProvider::with_config(FauxConfig {
            models: vec![FauxModelDefinition::new("faux-1"), small],
            ..FauxConfig::default()
        });
        let mut h = Harness::start_with(faux, vec![]).await;
        initialize(&mut h).await;
        let (frames, req, session) = new_session(&mut h).await;
        let response = frames.iter().find(|v| is_response_to(v, req)).unwrap();
        let model = response["result"]["configOptions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["id"] == "model")
            .unwrap();
        let value = model["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["value"].as_str().unwrap().to_string())
            .find(|v| v.ends_with("faux-small"))
            .unwrap_or_else(|| panic!("no faux-small option: {model}"));

        h.client
            .request(
                "session/set_config_option",
                json!({"sessionId": session, "configId": "model", "value": value}),
            )
            .await;
        let frames = h
            .client
            .drain_until(|v| v["params"]["update"]["sessionUpdate"] == "usage_update")
            .await;
        let usage = updates_of(&frames, "usage_update");
        assert_eq!(usage.len(), 1, "{}", dump(&frames));
        assert_eq!(usage[0]["params"]["update"]["size"], 32_000);
    });
}
