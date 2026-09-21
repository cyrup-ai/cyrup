//! The read half: the whole `session.snapshot` record closure, the pane and agent verbs, and the
//! two places where "it decoded" and "it is the right thing" are different statements.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use super::fake_server::{
    FakeHerdr, error_for, request_method, request_params, result_after, result_for,
};
use crate::client::{HerdrClient, WAIT_GRACE};
use crate::error::{ApiErrorCode, HerdrError};
use crate::schema::agents::AgentSessionRefKind;
use crate::schema::common::{AgentStatus, ReadFormat, ReadSource, SplitDirection};
use crate::schema::events::{OutputMatch, PaneWaitForOutputParams};
use crate::schema::panes::{
    PaneCurrentParams, PaneListParams, PaneProcessInfoParams, PaneReadParams, PaneSplitParams,
};

fn client(fake: &FakeHerdr) -> HerdrClient {
    HerdrClient::new(fake.path().to_path_buf()).with_timeout(Duration::from_secs(2))
}

/// One pane record with **every** optional field populated, so a mis-spelled field name shows up
/// as a `None` rather than passing unnoticed.
const FULL_PANE: &str = r#"{
    "pane_id":"w1:p1","terminal_id":"t1","workspace_id":"w1","tab_id":"w1:t1","focused":true,
    "cwd":"/home/u/proj","foreground_cwd":"/home/u/proj/crates","label":"build","agent":"cyrup",
    "title":"cargo build","terminal_title":"\u001b]0;cargo","terminal_title_stripped":"cargo",
    "display_agent":"cyrup","agent_status":"working",
    "state_labels":{"working":"building"},"tokens":{"run":"r-9"},
    "agent_session":{"source":"cyrup","agent":"cyrup","kind":"path","value":"/runs/s7.jsonl"},
    "scroll":{"offset_from_bottom":3,"max_offset_from_bottom":900,"viewport_rows":40},
    "revision":17
}"#;

/// AUG §8, the read half's anchor test: the whole `session.snapshot` closure —
/// `WorkspaceInfo` (with its `worktree` block), `TabInfo`, `PaneInfo` (with `agent_session` and
/// `scroll`), `PaneLayoutSnapshot` (with its rects and splits) and `AgentInfo` — decoded from one
/// reply, with every nested field read back.
///
/// The snapshot is **flat**: workspaces do not contain tabs and tabs do not contain panes
/// (`tmp/herdr/src/api/schema/session.rs:18-22`). The tree is rebuilt from `workspace_id`,
/// `tab_id` and `pane_id`, and this test walks that reconstruction so a mirror that quietly
/// renamed one of those three keys cannot pass.
#[tokio::test]
async fn the_whole_snapshot_record_closure_decodes() {
    let snapshot = format!(
        r#"{{"type":"session_snapshot","snapshot":{{
            "version":"0.9.1","protocol":22,
            "focused_workspace_id":"w1","focused_tab_id":"w1:t1","focused_pane_id":"w1:p1",
            "workspaces":[{{"workspace_id":"w1","number":1,"label":"cyrup","focused":true,
                "pane_count":2,"tab_count":1,"active_tab_id":"w1:t1","agent_status":"working",
                "tokens":{{"lane":"a"}},
                "worktree":{{"repo_key":"k","repo_name":"cyrup","repo_root":"/src/cyrup",
                    "checkout_path":"/src/wt/a","is_linked_worktree":true}}}}],
            "tabs":[{{"tab_id":"w1:t1","workspace_id":"w1","number":1,"label":"main",
                "focused":true,"pane_count":2,"agent_status":"working"}}],
            "panes":[{FULL_PANE}],
            "layouts":[{{"workspace_id":"w1","tab_id":"w1:t1","zoomed":false,
                "area":{{"x":0,"y":0,"width":200,"height":50}},"focused_pane_id":"w1:p1",
                "panes":[{{"pane_id":"w1:p1","focused":true,
                    "rect":{{"x":0,"y":0,"width":100,"height":50}}}}],
                "splits":[{{"id":"s1","direction":"right","ratio":0.5,
                    "rect":{{"x":0,"y":0,"width":200,"height":50}}}}]}}],
            "agents":[{{"terminal_id":"t1","name":"builder","agent":"cyrup","title":"cargo build",
                "terminal_title":"cargo","terminal_title_stripped":"cargo","display_agent":"cyrup",
                "agent_status":"working","screen_detection_skipped":true,
                "state_labels":{{"working":"building"}},"tokens":{{"run":"r-9"}},
                "agent_session":{{"source":"cyrup","agent":"cyrup","kind":"id","value":"s7"}},
                "workspace_id":"w1","tab_id":"w1:t1","pane_id":"w1:p1","focused":true,
                "launch_pending":false,"interactive_ready":true,"state_change_seq":5,
                "cwd":"/home/u/proj","foreground_cwd":"/home/u/proj/crates","revision":17}}]
        }}}}"#
    );
    let fake = FakeHerdr::start_with(move |request| result_for(request, &snapshot));

    let snapshot = client(&fake)
        .session_snapshot()
        .await
        .expect("the fake answers a snapshot");

    assert_eq!(snapshot.version, "0.9.1");
    assert_eq!(snapshot.protocol, 22);
    assert_eq!(snapshot.focused_pane_id.as_deref(), Some("w1:p1"));

    let workspace = snapshot.workspaces.first().expect("one workspace");
    assert_eq!(workspace.workspace_id, "w1");
    assert_eq!(workspace.tab_count, 1);
    assert_eq!(workspace.active_tab_id, "w1:t1");
    assert_eq!(workspace.agent_status, AgentStatus::Working);
    assert_eq!(workspace.tokens.get("lane").map(String::as_str), Some("a"));
    let worktree = workspace.worktree.as_ref().expect("the worktree block");
    assert_eq!(worktree.checkout_path, "/src/wt/a");
    assert!(worktree.is_linked_worktree);

    // The flat form, walked as a tree.
    let tab = snapshot
        .tabs
        .iter()
        .find(|tab| tab.workspace_id == workspace.workspace_id)
        .expect("the workspace's tab, found by workspace_id");
    assert_eq!(tab.tab_id, "w1:t1");
    let pane = snapshot
        .panes
        .iter()
        .find(|pane| pane.tab_id == tab.tab_id)
        .expect("the tab's pane, found by tab_id");
    let agent = snapshot
        .agents
        .iter()
        .find(|agent| agent.pane_id == pane.pane_id)
        .expect("the pane's agent, found by pane_id");

    assert_eq!(pane.terminal_id, "t1");
    assert_eq!(pane.foreground_cwd.as_deref(), Some("/home/u/proj/crates"));
    assert_eq!(pane.display_agent.as_deref(), Some("cyrup"));
    assert_eq!(pane.agent_status, AgentStatus::Working);
    assert_eq!(pane.tokens.get("run").map(String::as_str), Some("r-9"));
    assert_eq!(
        pane.state_labels.get("working").map(String::as_str),
        Some("building")
    );
    let session = pane.agent_session.as_ref().expect("the session block");
    assert_eq!(session.kind, AgentSessionRefKind::Path);
    assert_eq!(session.value, "/runs/s7.jsonl");
    let scroll = pane.scroll.as_ref().expect("the scroll block");
    assert_eq!(scroll.offset_from_bottom, 3);
    assert_eq!(scroll.viewport_rows, 40);
    assert_eq!(pane.revision, 17);

    let layout = snapshot.layouts.first().expect("one layout");
    assert_eq!(layout.tab_id, "w1:t1");
    assert!(!layout.zoomed);
    assert_eq!(layout.area.width, 200);
    assert_eq!(layout.focused_pane_id, "w1:p1");
    let layout_pane = layout.panes.first().expect("one laid-out pane");
    assert_eq!(layout_pane.rect.width, 100);
    let split = layout.splits.first().expect("one split");
    assert_eq!(split.direction, SplitDirection::Right);
    assert!((split.ratio - 0.5).abs() < f32::EPSILON);

    assert_eq!(agent.terminal_id, "t1");
    assert_eq!(agent.name.as_deref(), Some("builder"));
    assert!(agent.screen_detection_skipped);
    assert!(agent.interactive_ready);
    assert!(!agent.launch_pending);
    assert_eq!(agent.state_change_seq, 5);
    assert_eq!(
        agent
            .agent_session
            .as_ref()
            .expect("the agent's session block")
            .kind,
        AgentSessionRefKind::Id
    );

    let received = fake.received();
    let line = received.first().expect("one request line");
    assert_eq!(request_method(line), "session.snapshot");
    assert_eq!(request_params(line), serde_json::json!({}));
}

/// `pane.split` is the one verb where the request bytes carry cyrup's whole reason for using the
/// socket rather than `herdr pane split`: `env` — how a spawned pane learns which cyrup run it
/// belongs to — and `ratio`, which herdr honours only when present
/// (`tmp/herdr/src/app/api/panes.rs:73-100`).
///
/// The reply is asserted too, because `pane.split` answers the **new** pane
/// (`panes.rs:126-134`), not the one that was split.
#[tokio::test]
async fn pane_split_sends_env_and_ratio_and_answers_the_new_pane() {
    let fake = FakeHerdr::start_with(|request| {
        result_for(
            request,
            &format!(r#"{{"type":"pane_info","pane":{FULL_PANE}}}"#),
        )
    });

    let mut params = PaneSplitParams::new(SplitDirection::Down);
    params.target_pane_id = Some("w1:p1".to_owned());
    params.ratio = Some(0.25);
    params.cwd = Some("/src/wt/a".to_owned());
    params.focus = true;
    params.env = BTreeMap::from([
        ("CYRUP_RUN".to_owned(), "r-9".to_owned()),
        ("CYRUP_LANE".to_owned(), "a".to_owned()),
    ]);

    let pane = client(&fake)
        .pane_split(params)
        .await
        .expect("the fake answers a pane");
    assert_eq!(pane.pane_id, "w1:p1");
    assert_eq!(pane.revision, 17);

    let received = fake.received();
    let line = received.first().expect("one request line");
    assert_eq!(request_method(line), "pane.split");
    assert_eq!(
        request_params(line),
        serde_json::json!({
            "target_pane_id": "w1:p1",
            "direction": "down",
            "ratio": 0.25,
            "cwd": "/src/wt/a",
            "focus": true,
            "right_click": "herdr",
            "env": {"CYRUP_LANE": "a", "CYRUP_RUN": "r-9"},
        }),
        "env and ratio are the whole reason this is a socket call: {line}"
    );
}

/// `pane.get`, `pane.list`, `pane.current` and `pane.focus`, in one pass: the three targeting
/// shapes and the one field that matters for a background task.
///
/// `pane.current`'s param is `caller_pane_id`, not `pane_id`
/// (`tmp/herdr/src/api/schema/panes.rs:320-326`). Set, it means *the pane this process is in*;
/// unset, herdr answers whichever pane the **user** has focused
/// (`tmp/herdr/src/app/api/panes.rs:145-149`) — a different question, and the wrong one for a
/// bridge reporting on itself.
#[tokio::test]
async fn the_three_pane_targeting_shapes_each_send_their_own_key() {
    let fake = FakeHerdr::start_with(|request| {
        let result = match request_method(request).as_str() {
            "pane.list" => format!(r#"{{"type":"pane_list","panes":[{FULL_PANE}]}}"#),
            "pane.current" => format!(r#"{{"type":"pane_current","pane":{FULL_PANE}}}"#),
            _ => format!(r#"{{"type":"pane_info","pane":{FULL_PANE}}}"#),
        };
        result_for(request, &result)
    });
    let client = client(&fake);

    assert_eq!(
        client.pane_get("w1:p1").await.expect("get").pane_id,
        "w1:p1"
    );
    assert_eq!(
        client
            .pane_list(PaneListParams {
                workspace_id: Some("w1".to_owned()),
            })
            .await
            .expect("list")
            .len(),
        1
    );
    assert_eq!(
        client
            .pane_current(PaneCurrentParams {
                caller_pane_id: Some("w1:p1".to_owned()),
            })
            .await
            .expect("current")
            .pane_id,
        "w1:p1"
    );
    assert!(client.pane_focus("w1:p1").await.expect("focus").focused);

    let received = fake.received();
    let sent: Vec<(String, serde_json::Value)> = received
        .iter()
        .map(|line| (request_method(line), request_params(line)))
        .collect();
    assert_eq!(
        sent,
        vec![
            (
                "pane.get".to_owned(),
                serde_json::json!({"pane_id": "w1:p1"})
            ),
            (
                "pane.list".to_owned(),
                serde_json::json!({"workspace_id": "w1"})
            ),
            (
                "pane.current".to_owned(),
                serde_json::json!({"caller_pane_id": "w1:p1"}),
            ),
            (
                "pane.focus".to_owned(),
                serde_json::json!({"pane_id": "w1:p1"})
            ),
        ],
        "each targeting shape has its own key and they are not interchangeable"
    );
}

/// `pane_info` and `pane_current` carry the same payload shape and mean different things: "the
/// pane you named" versus "the pane herdr chose for you". herdr gives them distinct `type` tags
/// (`tmp/herdr/src/api/schema/response.rs:117-125`) and this client refuses to swap them, so a
/// server answering the wrong one cannot hand a caller a pane it did not ask about.
#[tokio::test]
async fn a_pane_current_is_not_a_pane_info_and_the_reverse() {
    let fake = FakeHerdr::start_with(|request| {
        let result = if request_method(request) == "pane.current" {
            format!(r#"{{"type":"pane_info","pane":{FULL_PANE}}}"#)
        } else {
            format!(r#"{{"type":"pane_current","pane":{FULL_PANE}}}"#)
        };
        result_for(request, &result)
    });
    let client = client(&fake);

    match client.pane_get("w1:p1").await.expect_err("swapped tag") {
        HerdrError::UnexpectedResult { method, want, got } => {
            assert_eq!(
                (method, want, got.as_str()),
                ("pane.get", "pane_info", "pane_current")
            );
        }
        other => panic!("expected UnexpectedResult, got {other:?}"),
    }
    match client
        .pane_current(PaneCurrentParams::default())
        .await
        .expect_err("swapped tag")
    {
        HerdrError::UnexpectedResult { method, want, got } => {
            assert_eq!(
                (method, want, got.as_str()),
                ("pane.current", "pane_current", "pane_info")
            );
        }
        other => panic!("expected UnexpectedResult, got {other:?}"),
    }
}

/// `agent.list` and `agent.get`. The roster is keyed on `terminal_id`, and `agent.get`'s param is
/// `target` — a terminal id, a pane id, or a name
/// (`tmp/herdr/src/api/schema/common.rs:54-57`).
///
/// The ambiguity error is kept **verbatim**: herdr's message enumerates every candidate with its
/// terminal id, pane id, workspace, tab, cwd and status
/// (`tmp/herdr/src/app/agents.rs:302-322`). That list is the whole value of the error — pi's
/// five-way normalisation (`src/inspectors/herdr/client.ts:35-41` @v0.68.0) would fold it onto
/// `VALIDATION_ERROR` and throw the candidates away.
#[tokio::test]
async fn agent_list_and_agent_get_keep_herdrs_ambiguity_message_intact() {
    const AMBIGUOUS: &str = "agent target cyrup is ambiguous; candidates: \
        terminal_id=t1 pane_id=w1:p1 workspace_id=w1 tab_id=w1:t1 cwd=/a status=Working; \
        terminal_id=t2 pane_id=w1:p2 workspace_id=w1 tab_id=w1:t1 cwd=/b status=Idle";

    let fake = FakeHerdr::start_with(|request| {
        if request_method(request) == "agent.list" {
            result_for(
                request,
                r#"{"type":"agent_list","agents":[
                    {"terminal_id":"t1","agent_status":"working","workspace_id":"w1",
                     "tab_id":"w1:t1","pane_id":"w1:p1","focused":true,"revision":3},
                    {"terminal_id":"t2","agent_status":"idle","workspace_id":"w1",
                     "tab_id":"w1:t1","pane_id":"w1:p2","focused":false,"revision":4}]}"#,
            )
        } else {
            error_for(request, "agent_target_ambiguous", AMBIGUOUS)
        }
    });
    let client = client(&fake);

    let agents = client.agent_list().await.expect("the roster");
    assert_eq!(agents.len(), 2);
    assert_eq!(agents.first().expect("first").terminal_id, "t1");
    assert_eq!(
        agents.get(1).expect("second").agent_status,
        AgentStatus::Idle
    );

    let failure = client
        .agent_get("cyrup")
        .await
        .expect_err("an ambiguous target is an error");
    match failure {
        HerdrError::Api { method, source } => {
            assert_eq!(method, "agent.get");
            assert_eq!(source.code, ApiErrorCode::AgentTargetAmbiguous);
            assert_eq!(
                source.message, AMBIGUOUS,
                "the candidate list is the error's whole value"
            );
        }
        other => panic!("expected Api, got {other:?}"),
    }

    let received = fake.received();
    assert_eq!(
        request_params(received.first().expect("the list line")),
        serde_json::json!({})
    );
    assert_eq!(
        request_params(received.get(1).expect("the get line")),
        serde_json::json!({"target": "cyrup"}),
        "agent.get's key is `target`, not `agent_id`"
    );
}

/// `pane.send_input` **is** `herdr pane run`: the CLI verb sends
/// `{text, keys:["Enter"]}` in one call (`tmp/herdr/src/cli/pane.rs:1039-1052`), not a send
/// followed by a separate key press. `pane.close` acknowledges rather than answering a record.
#[tokio::test]
async fn pane_send_input_is_herdr_pane_run_and_close_acknowledges() {
    let fake = FakeHerdr::start_with(|request| result_for(request, r#"{"type":"ok"}"#));
    let client = client(&fake);

    client
        .pane_send_input(crate::schema::panes::PaneSendInputParams::run(
            "w1:p1",
            "cargo build",
        ))
        .await
        .expect("the fake acknowledges");
    client
        .pane_send_input(crate::schema::panes::PaneSendInputParams::text(
            "w1:p1", "partial",
        ))
        .await
        .expect("the fake acknowledges");
    client
        .pane_close("w1:p1")
        .await
        .expect("close acknowledges");

    let received = fake.received();
    assert_eq!(
        request_params(received.first().expect("the run line")),
        serde_json::json!({"pane_id": "w1:p1", "text": "cargo build", "keys": ["Enter"]}),
        "`run` is one call: the text and the Enter together"
    );
    assert_eq!(
        request_params(received.get(1).expect("the text line")),
        serde_json::json!({"pane_id": "w1:p1", "text": "partial"}),
        "an empty `keys` is omitted, never sent as []"
    );
    assert_eq!(
        request_params(received.get(2).expect("the close line")),
        serde_json::json!({"pane_id": "w1:p1"})
    );
}

/// `pane.read`'s `revision` is hard-coded to `0` by herdr
/// (`tmp/herdr/src/app/api/panes.rs:1540`) — and so is every other `PaneReadResult` at this pin,
/// because every producer of one routes through that same dispatch. (`PaneInfo::revision` is a
/// different field and a live counter; see its doc.) A consumer that builds cache invalidation on
/// the read's `revision` invalidates nothing, forever — so the value is read back here exactly as
/// herdr sends it, and the type doc says why.
#[tokio::test]
async fn pane_read_answers_herdrs_own_zero_revision() {
    let fake = FakeHerdr::start_with(|request| {
        result_for(
            request,
            r#"{"type":"pane_read","read":{"pane_id":"w1:p1","workspace_id":"w1",
               "tab_id":"w1:t1","source":"recent_unwrapped","format":"ansi",
               "text":"line one\nline two","revision":0,"truncated":true}}"#,
        )
    });

    let read = client(&fake)
        .pane_read(PaneReadParams::new("w1:p1", ReadSource::RecentUnwrapped))
        .await
        .expect("the fake answers a read");

    assert_eq!(read.source, ReadSource::RecentUnwrapped);
    assert_eq!(read.format, ReadFormat::Ansi);
    assert_eq!(read.text, "line one\nline two");
    assert_eq!(read.revision, 0, "herdr hard-codes this; see the type doc");
    assert!(read.truncated);

    let received = fake.received();
    assert_eq!(
        request_params(received.first().expect("the read line")),
        serde_json::json!({
            "pane_id": "w1:p1",
            "source": "recent_unwrapped",
            "format": "text",
            "strip_ansi": true,
        }),
        "herdr's own defaults are sent explicitly, not left to the server"
    );
}

/// `pane.process_info` is best effort by construction: herdr fills it from a platform probe
/// (`tmp/herdr/src/app/api/panes.rs:536-538`) that can legitimately see nothing. An answer with
/// no foreground processes must decode to an empty list — *herdr could not tell* — and not fail
/// the line.
#[tokio::test]
async fn pane_process_info_decodes_both_a_full_and_an_empty_answer() {
    let fake = FakeHerdr::start_with(|request| {
        let known = request_params(request)
            .get("pane_id")
            .and_then(|id| id.as_str())
            .is_some();
        let result = if known {
            r#"{"type":"pane_process_info","process_info":{"pane_id":"w1:p1","shell_pid":4242,
               "foreground_process_group_id":4250,"tty":"/dev/pts/3",
               "foreground_processes":[{"pid":4250,"name":"cargo","argv0":"cargo",
                 "argv":["cargo","build"],"cmdline":"cargo build","cwd":"/src"}]}}"#
        } else {
            r#"{"type":"pane_process_info","process_info":{"pane_id":"w1:p9"}}"#
        };
        result_for(request, result)
    });
    let client = client(&fake);

    let full = client
        .pane_process_info(PaneProcessInfoParams {
            pane_id: Some("w1:p1".to_owned()),
        })
        .await
        .expect("a populated answer");
    assert_eq!(full.shell_pid, Some(4242));
    assert_eq!(full.tty.as_deref(), Some("/dev/pts/3"));
    let process = full.foreground_processes.first().expect("one process");
    assert_eq!(process.name, "cargo");
    assert_eq!(
        process.argv.as_deref(),
        Some(["cargo".to_owned(), "build".to_owned()].as_slice())
    );

    let empty = client
        .pane_process_info(PaneProcessInfoParams::default())
        .await
        .expect("an answer herdr could not fill must still decode");
    assert_eq!(empty.pane_id, "w1:p9");
    assert!(empty.shell_pid.is_none());
    assert!(
        empty.foreground_processes.is_empty(),
        "empty means herdr could not tell, not that the pane is idle"
    );

    let received = fake.received();
    assert_eq!(
        request_params(received.get(1).expect("the default line")),
        serde_json::json!({}),
        "an unset pane_id is omitted; herdr then uses the focused pane"
    );
}

/// `pane.wait_for_output` holds the connection open and answers once
/// (`tmp/herdr/src/api/wait.rs:52-110`), so this client's deadline must be **derived from the wait
/// it asked for**, not inherited from the per-call default.
///
/// The test makes the two visibly different: the client's own deadline is 20 ms, herdr's wait is
/// 50 ms, and the fake answers after 250 ms. A client that used `self.timeout` reports
/// [`HerdrError::Timeout`] at 20 ms; one that used `timeout_ms + WAIT_GRACE` waits 5.05 s and
/// gets the match.
///
/// The `revision` is checked on the way past, and the fixture's `91` is deliberately a value **no
/// herdr at this pin ever sends**: `wait.rs:98` copies the field out of the `pane.read` the wait
/// performed, and that dispatch hard-codes `revision: 0`
/// (`tmp/herdr/src/app/api/panes.rs:1540`), so every real `output_matched` carries `0`. A fixture
/// that also said `0` would pass against a client that dropped the field, defaulted it, or
/// hard-coded the same literal; `91` is the only way to prove the number is read off the wire.
/// That is what the assertion pins — the decode, not a herdr behaviour.
#[tokio::test]
async fn a_wait_deadline_is_the_herdr_wait_plus_the_poll_grace() {
    let fake = FakeHerdr::start_with(|request| {
        result_after(
            request,
            Duration::from_millis(250),
            r#"{"type":"output_matched","pane_id":"w1:p1","revision":91,
               "matched_line":"BUILD DONE","read":{"pane_id":"w1:p1","workspace_id":"w1",
               "tab_id":"w1:t1","source":"recent","format":"text",
               "text":"...\nBUILD DONE","revision":91,"truncated":false}}"#,
        )
    });

    let client =
        HerdrClient::new(fake.path().to_path_buf()).with_timeout(Duration::from_millis(20));
    let mut params = PaneWaitForOutputParams::new(
        "w1:p1",
        ReadSource::Recent,
        OutputMatch::regex("BUILD (DONE|FAILED)"),
    );
    params.timeout_ms = Some(50);
    params.lines = Some(200);

    let started = Instant::now();
    let matched = client
        .pane_wait_for_output(params)
        .await
        .expect("the derived deadline outlives the 20 ms per-call default");
    let elapsed = started.elapsed();

    assert!(
        elapsed >= Duration::from_millis(200),
        "the answer arrived at 250 ms; a 20 ms deadline could not have seen it ({elapsed:?})"
    );
    assert!(
        WAIT_GRACE >= Duration::from_secs(5),
        "the grace must cover herdr's 100 ms poll plus its 5 s per-poll read"
    );
    assert_eq!(matched.pane_id, "w1:p1");
    assert_eq!(matched.matched_line.as_deref(), Some("BUILD DONE"));
    assert_eq!(
        matched.revision, 91,
        "the wire value, not a default and not a client-side literal — herdr itself always sends \
         0 here (panes.rs:1540 -> wait.rs:98), which is why the fixture does not"
    );
    assert_eq!(matched.read.text, "...\nBUILD DONE");

    let received = fake.received();
    assert_eq!(
        request_params(received.first().expect("the wait line")),
        serde_json::json!({
            "pane_id": "w1:p1",
            "source": "recent",
            "lines": 200,
            "match": {"type": "regex", "value": "BUILD (DONE|FAILED)"},
            "timeout_ms": 50,
            "strip_ansi": true,
        }),
        "the field is `match`, and OutputMatch is internally tagged"
    );
}

/// The other half of the same rule. `timeout_ms` absent does **not** mean "herdr's default": the
/// deadline is built with `params.timeout_ms.map(…)` (`tmp/herdr/src/api/wait.rs:30-32`) and every
/// later check is `deadline.is_some_and(…)` (`:113`), so herdr waits forever.
///
/// The client's own deadline is therefore applied rather than waived, because the alternative is a
/// hang with no herdr-side rescue.
#[tokio::test]
async fn a_wait_with_no_herdr_deadline_still_ends() {
    let fake = FakeHerdr::start(super::fake_server::Reply::Silence);

    let client =
        HerdrClient::new(fake.path().to_path_buf()).with_timeout(Duration::from_millis(150));
    let params =
        PaneWaitForOutputParams::new("w1:p1", ReadSource::Recent, OutputMatch::substring("never"));
    assert!(
        params.timeout_ms.is_none(),
        "the constructor leaves herdr's own deadline unset"
    );

    let started = Instant::now();
    let failure = client
        .pane_wait_for_output(params)
        .await
        .expect_err("a wait with no deadline on either side would hang forever");
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(2),
        "the client's own deadline must apply ({elapsed:?})"
    );
    match failure {
        HerdrError::Timeout { method, timeout } => {
            assert_eq!(method, "pane.wait_for_output");
            assert_eq!(timeout, Duration::from_millis(150));
        }
        other => panic!("expected Timeout, got {other:?}"),
    }
}

/// One `agent_status` a newer herdr invented must not take the whole session down.
///
/// `agent_status` is REQUIRED and un-defaulted on `PaneInfo`, `TabInfo`, `AgentInfo` and
/// `WorkspaceInfo`, and `SessionSnapshot` carries a `Vec<PaneInfo>`. With a closed enum, a single
/// pane in a sixth status — a pane cyrup does not own and does not care about — failed the whole
/// `session.snapshot` line, and with it every verb that decodes one of those four records. It did
/// not degrade either: `bootstrap` failed, `ReconnectingEvents` read that as a restart, and
/// `rebootstrap` spent its whole budget re-fetching the same undecodable snapshot before ending
/// the stream for ever. One enum value took the fleet view dark for the life of the process.
///
/// So [`AgentStatus::Unrecognised`] exists. It is **loud** rather than folded into
/// [`AgentStatus::Unknown`] — "herdr cannot tell" is a status herdr publishes, and reporting a new
/// one as that would be a fabricated reading — and it carries herdr's own spelling, so a consumer
/// can log what it actually saw.
#[tokio::test]
async fn an_unknown_agent_status_decodes_instead_of_failing_the_whole_snapshot() {
    // `"waiting"` is not one of herdr 0.9.1's five (`tmp/herdr/src/api/schema/common.rs:158-166`,
    // pinned by its published artifact); it stands in for whatever 0.9.2 adds.
    let snapshot = r#"{"type":"session_snapshot","snapshot":{
        "version":"0.9.2","protocol":22,
        "focused_workspace_id":"w1","focused_tab_id":"w1:t1","focused_pane_id":"w1:p1",
        "workspaces":[{"workspace_id":"w1","number":1,"label":"cyrup","focused":true,
            "pane_count":2,"tab_count":1,"active_tab_id":"w1:t1","agent_status":"waiting"}],
        "tabs":[{"tab_id":"w1:t1","workspace_id":"w1","number":1,"label":"main",
            "focused":true,"pane_count":2,"agent_status":"waiting"}],
        "panes":[
            {"pane_id":"w1:p1","terminal_id":"t1","workspace_id":"w1","tab_id":"w1:t1",
             "focused":true,"agent_status":"waiting","revision":0},
            {"pane_id":"w1:p2","terminal_id":"t2","workspace_id":"w1","tab_id":"w1:t1",
             "focused":false,"agent_status":"working","revision":0}],
        "layouts":[],
        "agents":[{"terminal_id":"t1","name":"builder","agent_status":"waiting",
            "screen_detection_skipped":false,"workspace_id":"w1","tab_id":"w1:t1",
            "pane_id":"w1:p1","focused":true,"launch_pending":false,"interactive_ready":true,
            "state_change_seq":5,"revision":0}]
    }}"#;
    let fake = FakeHerdr::start_with(move |request| result_for(request, snapshot));

    let snapshot = client(&fake)
        .session_snapshot()
        .await
        .expect("a status this build does not name is not a reason to fail the line");

    let unknown = AgentStatus::Unrecognised("waiting".to_string());
    assert_eq!(
        snapshot
            .workspaces
            .first()
            .expect("a workspace")
            .agent_status,
        unknown
    );
    assert_eq!(snapshot.tabs.first().expect("a tab").agent_status, unknown);
    assert_eq!(
        snapshot.agents.first().expect("an agent").agent_status,
        unknown
    );

    // And the rest of the line is intact — that is the whole point: the pane cyrup DOES care
    // about is still readable next to the one it does not.
    let mine = snapshot
        .panes
        .iter()
        .find(|pane| pane.pane_id == "w1:p2")
        .expect("the pane in a status this build names is still there");
    assert_eq!(mine.agent_status, AgentStatus::Working);
    assert_eq!(
        snapshot
            .panes
            .iter()
            .find(|pane| pane.pane_id == "w1:p1")
            .expect("and so is the one that is not")
            .agent_status,
        unknown
    );

    // It is NOT silently folded into a status herdr publishes.
    assert_ne!(unknown, AgentStatus::Unknown);
    // And it round-trips herdr's own spelling, so it can be logged and re-sent as the narrowing
    // on `Subscription::PaneAgentStatusChanged` without becoming a different request.
    assert_eq!(
        serde_json::to_string(&unknown).expect("an untagged newtype variant is its inner string"),
        r#""waiting""#
    );
    // The five named values still take their own arms, not this one.
    for (wire, want) in [
        ("idle", AgentStatus::Idle),
        ("working", AgentStatus::Working),
        ("blocked", AgentStatus::Blocked),
        ("done", AgentStatus::Done),
        ("unknown", AgentStatus::Unknown),
    ] {
        let decoded: AgentStatus =
            serde_json::from_str(&format!("\"{wire}\"")).expect("a named status");
        assert_eq!(decoded, want, "{wire} fell through to the untagged arm");
        assert_eq!(
            serde_json::to_string(&want).expect("serialize"),
            format!("\"{wire}\"")
        );
    }
}
