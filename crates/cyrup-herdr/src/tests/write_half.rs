//! The write half: what the status bridge puts on the wire, and what it refuses to call success.
//!
//! Every assertion here is on the **bytes the fake received**, not on the client's return value.
//! A write verb answers `{"type":"ok"}` whatever it did, so a client that spelled a field wrong
//! cannot tell from the reply — only the request line can say. That is the whole reason
//! [`super::fake_server::FakeHerdr`] records what it reads.

use std::collections::BTreeMap;
use std::time::Duration;

use super::fake_server::{FakeHerdr, error_for, request_method, request_params, result_for};
use crate::client::HerdrClient;
use crate::error::{ApiErrorCode, HerdrError};
use crate::schema::common::PaneAgentState;
use crate::schema::panes::{
    PaneClearAgentAuthorityParams, PaneReleaseAgentParams, PaneReportAgentParams,
    PaneReportAgentSessionParams, PaneReportMetadataParams,
};
use crate::schema::tabs::TabRenameParams;

fn client(fake: &FakeHerdr) -> HerdrClient {
    HerdrClient::new(fake.path().to_path_buf()).with_timeout(Duration::from_secs(2))
}

/// A fake that answers `{"type":"ok"}` to anything — herdr's acknowledgement for every write verb
/// (`tmp/herdr/src/api/schema/response.rs:307`).
fn acknowledging() -> FakeHerdr {
    FakeHerdr::start_with(|request| result_for(request, r#"{"type":"ok"}"#))
}

/// AUG §8.14. The keys and the value spellings of `pane.report_agent`, asserted on the wire.
///
/// Three distinct ways to get this wrong, all invisible from the reply:
/// - the field is `state`, not `status` (`tmp/herdr/src/api/schema/panes.rs:451`);
/// - the value is `"working"`, not `"Working"` — `PaneAgentState` is `rename_all = "snake_case"`
///   (`tmp/herdr/src/api/schema/common.rs:149-151`);
/// - the four optional fields are **omitted**, not sent as `null`, because herdr's own
///   `skip_serializing_if = "Option::is_none"` (`panes.rs:452-459`) is what keeps a minimal report
///   minimal.
#[tokio::test]
async fn report_agent_sends_exactly_herdrs_field_names() {
    let fake = acknowledging();

    client(&fake)
        .report_agent(PaneReportAgentParams::new(
            "w1:p1",
            "cyrup",
            "cyrup",
            PaneAgentState::Working,
        ))
        .await
        .expect("the fake acknowledges");

    let received = fake.received();
    let line = received.first().expect("one request line");
    assert_eq!(request_method(line), "pane.report_agent");

    let params = request_params(line);
    let object = params.as_object().expect("params is an object");
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["agent", "pane_id", "source", "state"],
        "only the four required fields belong on a minimal report: {line}"
    );
    assert_eq!(
        object.get("state").and_then(|v| v.as_str()),
        Some("working")
    );
    assert_eq!(
        object.get("pane_id").and_then(|v| v.as_str()),
        Some("w1:p1")
    );
    assert_eq!(object.get("source").and_then(|v| v.as_str()), Some("cyrup"));
    assert_eq!(object.get("agent").and_then(|v| v.as_str()), Some("cyrup"));
}

/// The optional half, sent this time: `message`, `seq` and the two session references must appear
/// under herdr's own names when set.
#[tokio::test]
async fn report_agent_carries_the_session_reference_and_the_ordering_token() {
    let fake = acknowledging();

    let mut params = PaneReportAgentParams::new("w1:p1", "cyrup", "cyrup", PaneAgentState::Blocked);
    params.message = Some("waiting for review".to_owned());
    params.seq = Some(42);
    params.agent_session_id = Some("sess-7".to_owned());
    params.agent_session_path = Some("/runs/sess-7.jsonl".to_owned());

    client(&fake)
        .report_agent(params)
        .await
        .expect("the fake acknowledges");

    let received = fake.received();
    let line = received.first().expect("one request line");
    assert_eq!(
        request_params(line),
        serde_json::json!({
            "pane_id": "w1:p1",
            "source": "cyrup",
            "agent": "cyrup",
            "state": "blocked",
            "message": "waiting for review",
            "seq": 42,
            "agent_session_id": "sess-7",
            "agent_session_path": "/runs/sess-7.jsonl",
        }),
        "every optional field must use herdr's own name: {line}"
    );
}

/// `pane.report_metadata`'s `tokens` is a **patch**, not a replacement: `Some` sets and `null`
/// clears (`HashMap<String, Option<String>>`, `tmp/herdr/src/api/schema/panes.rs:123`). A client
/// that skips the `None` entries can set a token and can never unset one, and the reply is
/// `{"type":"ok"}` either way.
///
/// The three `clear_*` flags are pinned in the same line because they are how a *field* is
/// removed — `title: None` means "leave it alone", not "clear it".
#[tokio::test]
async fn report_metadata_sends_the_token_patch_with_null_for_a_clear() {
    let fake = acknowledging();

    let mut params = PaneReportMetadataParams::new("w1:p1", "cyrup");
    params.title = Some("build".to_owned());
    params.tokens = BTreeMap::from([
        ("run".to_owned(), Some("r-9".to_owned())),
        ("stale".to_owned(), None),
    ]);
    params.state_labels = BTreeMap::from([("working".to_owned(), "building".to_owned())]);
    params.clear_display_agent = true;
    params.ttl_ms = Some(60_000);

    client(&fake)
        .report_metadata(params)
        .await
        .expect("the fake acknowledges");

    let received = fake.received();
    let line = received.first().expect("one request line");
    assert_eq!(
        request_params(line),
        serde_json::json!({
            "pane_id": "w1:p1",
            "source": "cyrup",
            "title": "build",
            "state_labels": {"working": "building"},
            "tokens": {"run": "r-9", "stale": null},
            "clear_title": false,
            "clear_display_agent": true,
            "clear_state_labels": false,
            "ttl_ms": 60_000,
        }),
        "a null token is the clear; dropping it silently keeps a stale token forever: {line}"
    );
}

/// `pane.release_agent` requires `source` **and** `agent`
/// (`"required": ["pane_id","source","agent"]`), while `pane.clear_agent_authority` requires
/// neither and must omit an unset `source` rather than send `null`
/// (`tmp/herdr/src/api/schema/panes.rs:509-516`).
///
/// The asymmetry is the point: a release names whose agent is leaving, so one reporter cannot
/// retire another's; a clear surrenders whatever authority the pane holds.
#[tokio::test]
async fn release_names_its_agent_and_a_bare_clear_omits_its_source() {
    let fake = acknowledging();
    let client = client(&fake);

    client
        .release_agent(PaneReleaseAgentParams::new("w1:p1", "cyrup", "cyrup"))
        .await
        .expect("the fake acknowledges");
    client
        .clear_agent_authority(PaneClearAgentAuthorityParams::new("w1:p1"))
        .await
        .expect("the fake acknowledges");

    let received = fake.received();
    assert_eq!(
        received.len(),
        2,
        "one request per connection: {received:?}"
    );

    let release = received.first().expect("the release line");
    assert_eq!(request_method(release), "pane.release_agent");
    assert_eq!(
        request_params(release),
        serde_json::json!({"pane_id": "w1:p1", "source": "cyrup", "agent": "cyrup"})
    );

    let clear = received.get(1).expect("the clear line");
    assert_eq!(request_method(clear), "pane.clear_agent_authority");
    assert_eq!(
        request_params(clear),
        serde_json::json!({"pane_id": "w1:p1"}),
        "an unset source is omitted, never sent as null: {clear}"
    );
}

/// `pane.report_agent_session` binds a pane to a resumable session without asserting a state — so
/// it carries no `state` key at all, and it is the only write verb with
/// `session_start_source`.
#[tokio::test]
async fn report_agent_session_carries_no_state_and_names_its_start_source() {
    let fake = acknowledging();

    let mut params = PaneReportAgentSessionParams::new("w1:p1", "cyrup", "cyrup");
    params.agent_session_path = Some("/runs/sess-7.jsonl".to_owned());
    params.session_start_source = Some("resume".to_owned());

    client(&fake)
        .report_agent_session(params)
        .await
        .expect("the fake acknowledges");

    let received = fake.received();
    let line = received.first().expect("one request line");
    assert_eq!(request_method(line), "pane.report_agent_session");
    assert_eq!(
        request_params(line),
        serde_json::json!({
            "pane_id": "w1:p1",
            "source": "cyrup",
            "agent": "cyrup",
            "agent_session_path": "/runs/sess-7.jsonl",
            "session_start_source": "resume",
        })
    );
}

/// AUG §8.2, on the write half where it bites hardest.
///
/// herdr acknowledges every write with `{"type":"ok"}`. A newer herdr answering something richer —
/// or an older one answering something else entirely — must be an error, because "the call
/// returned" and "the write landed" are only the same statement while the acknowledgement is the
/// one this client asked for.
#[tokio::test]
async fn only_an_ok_is_a_successful_write() {
    for answer in [
        r#"{"type":"martian"}"#,
        r#"{"type":"pane_info","pane":{"pane_id":"w1:p1","terminal_id":"t1","workspace_id":"w1",
           "tab_id":"w1:t1","focused":true,"agent_status":"idle","revision":1}}"#,
        r#"{"type":"pong","version":"0.9.1","protocol":22}"#,
    ] {
        let owned = answer.to_owned();
        let fake = FakeHerdr::start_with(move |request| result_for(request, &owned));

        let failure = client(&fake)
            .report_agent(PaneReportAgentParams::new(
                "w1:p1",
                "cyrup",
                "cyrup",
                PaneAgentState::Working,
            ))
            .await
            .expect_err("a non-ok answer is not a successful write");

        match failure {
            HerdrError::UnexpectedResult { method, want, .. } => {
                assert_eq!(method, "pane.report_agent");
                assert_eq!(want, "ok");
            }
            other => panic!("expected UnexpectedResult for {answer}, got {other:?}"),
        }
    }
}

/// AUG §8.3, on a write verb. herdr's `code` and `message` reach the caller unchanged, and a
/// failed write is a failure — never an `Ok(())` with the reason logged and dropped.
///
/// `is_unsupported_method` must be **false** here: `pane_not_found` says the pane is gone, not
/// that this herdr build lacks the method, and confusing the two turns a transient miss into a
/// permanently disabled feature.
#[tokio::test]
async fn a_refused_write_carries_herdrs_own_code_and_message() {
    let fake = FakeHerdr::start_with(|request| {
        error_for(request, "pane_not_found", "pane w1:p9 not found")
    });

    let failure = client(&fake)
        .report_agent(PaneReportAgentParams::new(
            "w1:p9",
            "cyrup",
            "cyrup",
            PaneAgentState::Idle,
        ))
        .await
        .expect_err("a refused write must not report success");

    assert!(
        !failure.is_unsupported_method(),
        "pane_not_found is not a missing method"
    );
    match failure {
        HerdrError::Api { method, source } => {
            assert_eq!(method, "pane.report_agent");
            assert_eq!(source.code, ApiErrorCode::PaneNotFound);
            assert_eq!(
                source.message, "pane w1:p9 not found",
                "herdr's message is reproduced byte for byte"
            );
        }
        other => panic!("expected Api, got {other:?}"),
    }
}

/// The decoder's diagnostic, which is `[CYRUP-EXCEEDS-UPSTREAM]` and exists for exactly one day:
/// the day a newer herdr adds a required field to a record this crate mirrors.
///
/// herdr's own client decodes the answer envelope with `#[serde(untagged)]`
/// (`tmp/herdr/src/api/client.rs:231-236`), and serde discards each arm's real error before
/// reporting an untagged failure — so *every* decode failure reads
/// `"data did not match any variant of untagged enum WireResponse"` at `line: 0, column: 0`,
/// whether the line was truncated, corrupt, from another protocol, or a perfectly good response
/// carrying one field this client has not caught up with.
///
/// [`crate::schema::response::WireResponse::decode`] dispatches on `error`-versus-`result` first
/// and then decodes one arm, so serde's own message survives and **names the field**. This test
/// is what says so.
#[tokio::test]
async fn a_missing_field_is_named_in_the_error_not_swallowed() {
    let fake = FakeHerdr::start_with(|request| {
        result_for(
            request,
            r#"{"type":"tab_info","tab":{"tab_id":"w1:t1","workspace_id":"w1","number":1,
               "focused":true,"pane_count":2,"agent_status":"idle"}}"#,
        )
    });

    let failure = client(&fake)
        .tab_get("w1:t1")
        .await
        .expect_err("a record missing a required field is not a tab");

    match failure {
        HerdrError::Malformed { method, source } => {
            assert_eq!(method, "tab.get");
            let reported = source.to_string();
            assert!(
                reported.contains("label"),
                "the missing field must be named, or a herdr upgrade is undiagnosable: {reported}"
            );
            assert!(
                !reported.contains("untagged"),
                "serde's untagged error names nothing: {reported}"
            );
        }
        other => panic!("expected Malformed, got {other:?}"),
    }
}

/// The five codes this batch newly made reachable, each keyed to the herdr line that produces it.
/// They are named variants rather than `Other` because a consumer branches on them: `invalid_key`
/// and `invalid_env` are caller bugs, `confirmation_required` is a prompt to show the user, and
/// the two agent-target codes distinguish "no such agent" from "say which one".
#[test]
fn the_codes_this_batch_made_reachable_are_named_not_other() {
    for (wire, expected) in [
        // tmp/herdr/src/app/api/panes.rs:1852 — pane.send_input
        ("invalid_key", ApiErrorCode::InvalidKey),
        // tmp/herdr/src/app/api/env.rs:3-31 — pane.split
        ("invalid_env", ApiErrorCode::InvalidEnv),
        // tmp/herdr/src/app/api/panes.rs:1880-1886 — pane.close
        ("confirmation_required", ApiErrorCode::ConfirmationRequired),
        // tmp/herdr/src/api/wait.rs:38-48 — pane.wait_for_output
        ("invalid_regex", ApiErrorCode::InvalidRegex),
        // tmp/herdr/src/app/agents.rs:298-301 — agent.get
        ("agent_not_found", ApiErrorCode::AgentNotFound),
        // tmp/herdr/src/app/agents.rs:302-322 — agent.get
        ("agent_target_ambiguous", ApiErrorCode::AgentTargetAmbiguous),
    ] {
        assert_eq!(ApiErrorCode::from_wire(wire), expected, "decoding {wire}");
        assert_eq!(expected.as_str(), wire, "round-tripping {wire}");
    }
}

/// `tab.rename` answers the renamed [`crate::schema::TabInfo`], not an acknowledgement
/// (`tmp/herdr/src/app/api/tabs.rs:169-171`) — so the pane-owned-tab-label pattern is read,
/// decide, rename, and keep what came back: two calls, not three.
///
/// A `{"type":"ok"}` to a `tab.rename` is therefore a **failure**, not a silent success with an
/// unknown label.
#[tokio::test]
async fn tab_get_and_tab_rename_both_answer_a_tab_record() {
    let fake = FakeHerdr::start_with(|request| {
        let label = if request_method(request) == "tab.rename" {
            "cyrup · building"
        } else {
            "main"
        };
        result_for(
            request,
            &format!(
                r#"{{"type":"tab_info","tab":{{"tab_id":"w1:t1","workspace_id":"w1","number":1,
                   "label":"{label}","focused":true,"pane_count":2,"agent_status":"working"}}}}"#
            ),
        )
    });
    let client = client(&fake);

    let before = client
        .tab_get("w1:t1")
        .await
        .expect("tab.get answers a tab");
    assert_eq!(before.label, "main");
    assert_eq!(before.pane_count, 2);

    let after = client
        .tab_rename(TabRenameParams::new("w1:t1", "cyrup · building"))
        .await
        .expect("tab.rename answers the renamed tab");
    assert_eq!(after.label, "cyrup · building");
    assert_eq!(after.tab_id, "w1:t1");

    let received = fake.received();
    assert_eq!(
        request_params(received.get(1).expect("the rename line")),
        serde_json::json!({"tab_id": "w1:t1", "label": "cyrup · building"})
    );
}

/// The other direction of the same rule: a bare acknowledgement to `tab.rename` must not be
/// upgraded into a fabricated [`crate::schema::TabInfo`].
#[tokio::test]
async fn an_acknowledgement_is_not_a_tab_record() {
    let fake = acknowledging();

    let failure = client(&fake)
        .tab_rename(TabRenameParams::new("w1:t1", "label"))
        .await
        .expect_err("an ok is not a tab");

    match failure {
        HerdrError::UnexpectedResult { method, want, got } => {
            assert_eq!(method, "tab.rename");
            assert_eq!(want, "tab_info");
            assert_eq!(got, "ok");
        }
        other => panic!("expected UnexpectedResult, got {other:?}"),
    }
}
