//! Every ported method's wire name, checked against herdr's own `#[serde(rename = …)]`.
//!
//! This is the cheapest test in the crate and the one that catches the most expensive mistake. A
//! misspelled method name does not fail to compile and does not fail to serialise: it produces a
//! perfectly well-formed line that herdr answers with `invalid_request`
//! (`tmp/herdr/src/api/server.rs:177-204`, because `Method` is a tagged enum and an unknown tag is
//! a serde error), which this client then reports as *"this herdr build does not have that
//! method"* — degrading a feature that in fact works.
//!
//! Two things are asserted per method, because either alone can drift:
//!
//! 1. the name on the wire is herdr's spelling, checked against `tmp/herdr/src/api/schema.rs`;
//! 2. [`Method::name`] — which is what every [`crate::HerdrError`] reports and what the caller
//!    reads in a log — agrees with it.

use std::collections::BTreeMap;

use crate::schema::agents::{AgentInfo, AgentViewClearParams, AgentViewSetParams};
use crate::schema::common::{
    AgentStatus, AgentTarget, EmptyParams, PaneAgentState, PaneTarget, ReadFormat, ReadSource,
    SplitDirection, TabTarget,
};
use crate::schema::events::{
    EventsSubscribeParams, OutputMatch, PaneWaitForOutputParams, Subscription,
};
use crate::schema::panes::{
    PaneClearAgentAuthorityParams, PaneCurrentParams, PaneListParams, PaneProcessInfoParams,
    PaneReadParams, PaneReleaseAgentParams, PaneReportAgentParams, PaneReportAgentSessionParams,
    PaneReportMetadataParams, PaneSendInputParams, PaneSplitParams,
};
use crate::schema::tabs::TabRenameParams;
use crate::schema::{Method, PingParams, Request, TabInfo};

/// Every variant this crate ships, paired with the spelling herdr declares and the
/// `tmp/herdr/src/api/schema.rs` line it is declared on.
fn every_method() -> Vec<(Method, &'static str)> {
    vec![
        // schema.rs:48
        (Method::Ping(PingParams {}), "ping"),
        // schema.rs:74
        (Method::SessionSnapshot(EmptyParams {}), "session.snapshot"),
        // schema.rs:106
        (Method::TabGet(TabTarget::new("w1:t1")), "tab.get"),
        // schema.rs:110
        (
            Method::TabRename(TabRenameParams::new("w1:t1", "label")),
            "tab.rename",
        ),
        // schema.rs:116
        (Method::AgentList(EmptyParams {}), "agent.list"),
        // schema.rs:118
        (Method::AgentGet(AgentTarget::new("t1")), "agent.get"),
        // schema.rs:128
        (
            Method::AgentViewSet(AgentViewSetParams::new("cyrup:subagents")),
            "agent.view.set",
        ),
        // schema.rs:130
        (
            Method::AgentViewClear(AgentViewClearParams::owned_by("cyrup:subagents")),
            "agent.view.clear",
        ),
        // schema.rs:140
        (
            Method::PaneSplit(PaneSplitParams::new(SplitDirection::Right)),
            "pane.split",
        ),
        // schema.rs:150
        (
            Method::PaneProcessInfo(PaneProcessInfoParams::default()),
            "pane.process_info",
        ),
        // schema.rs:178
        (Method::PaneList(PaneListParams::default()), "pane.list"),
        // schema.rs:180
        (
            Method::PaneCurrent(PaneCurrentParams::default()),
            "pane.current",
        ),
        // schema.rs:182
        (Method::PaneGet(PaneTarget::new("w1:p1")), "pane.get"),
        // schema.rs:184
        (Method::PaneFocus(PaneTarget::new("w1:p1")), "pane.focus"),
        // schema.rs:198
        (
            Method::PaneSendInput(PaneSendInputParams::run("w1:p1", "ls")),
            "pane.send_input",
        ),
        // schema.rs:200
        (
            Method::PaneRead(PaneReadParams::new("w1:p1", ReadSource::Visible)),
            "pane.read",
        ),
        // schema.rs:223
        (
            Method::PaneReportAgent(PaneReportAgentParams::new(
                "w1:p1",
                "cyrup",
                "cyrup",
                PaneAgentState::Working,
            )),
            "pane.report_agent",
        ),
        // schema.rs:225
        (
            Method::PaneReportAgentSession(PaneReportAgentSessionParams::new(
                "w1:p1", "cyrup", "cyrup",
            )),
            "pane.report_agent_session",
        ),
        // schema.rs:227
        (
            Method::PaneReportMetadata(PaneReportMetadataParams::new("w1:p1", "cyrup")),
            "pane.report_metadata",
        ),
        // schema.rs:229
        (
            Method::PaneClearAgentAuthority(PaneClearAgentAuthorityParams::new("w1:p1")),
            "pane.clear_agent_authority",
        ),
        // schema.rs:231
        (
            Method::PaneReleaseAgent(PaneReleaseAgentParams::new("w1:p1", "cyrup", "cyrup")),
            "pane.release_agent",
        ),
        // schema.rs:233
        (Method::PaneClose(PaneTarget::new("w1:p1")), "pane.close"),
        // schema.rs:241
        (
            Method::PaneWaitForOutput(PaneWaitForOutputParams::new(
                "w1:p1",
                ReadSource::Recent,
                OutputMatch::substring("done"),
            )),
            "pane.wait_for_output",
        ),
        // schema.rs:237
        (
            Method::EventsSubscribe(EventsSubscribeParams {
                subscriptions: vec![Subscription::PaneCreated {}],
            }),
            "events.subscribe",
        ),
    ]
}

#[test]
fn every_method_serialises_under_herdrs_own_name() {
    for (method, herdr_name) in every_method() {
        assert_eq!(
            method.name(),
            herdr_name,
            "Method::name disagrees with herdr's #[serde(rename)]"
        );

        let request = Request {
            id: "req_1".to_owned(),
            method,
        };
        let line = serde_json::to_string(&request).expect("a request serialises");
        let value: serde_json::Value = serde_json::from_str(&line).expect("valid JSON");

        assert_eq!(
            value.get("method").and_then(serde_json::Value::as_str),
            Some(herdr_name),
            "the wire `method` is not herdr's spelling: {line}"
        );
        assert_eq!(
            value.get("id").and_then(serde_json::Value::as_str),
            Some("req_1"),
            "`id` must sit beside `method`, not inside `params`: {line}"
        );
        assert!(
            value.get("params").is_some(),
            "`params` is mandatory on EVERY method — herdr's Method is adjacently tagged \
             (tmp/herdr/src/api/schema.rs:41) and rejects a line without it: {line}"
        );
    }
}

/// herdr splits one vocabulary in two and the split is load-bearing: what a client *reports*
/// (`PaneAgentState`, `tmp/herdr/src/api/schema/common.rs:149-156`) has **four** values, while
/// what herdr *publishes* (`AgentStatus`, `:158-166`) has five. `done` exists only on the second.
///
/// Sending `"done"` on a `pane.report_agent` is `invalid_request`; reading a `"done"` off a
/// `PaneInfo` and failing to decode it is a broken roster. Both directions are pinned here.
///
/// The asymmetry in forward-compatibility is pinned with them. `PaneAgentState` is `Serialize`
/// only — this client constructs every value it sends, so a closed vocabulary is exactly right.
/// `AgentStatus` is decoded off a REQUIRED field of four record types, so it carries
/// [`AgentStatus::Unrecognised`]; see its type doc for what a closed one cost.
#[test]
fn the_reported_state_has_four_values_and_the_published_status_has_five() {
    let reported = [
        (PaneAgentState::Idle, "\"idle\""),
        (PaneAgentState::Working, "\"working\""),
        (PaneAgentState::Blocked, "\"blocked\""),
        (PaneAgentState::Unknown, "\"unknown\""),
    ];
    for (state, wire) in reported {
        assert_eq!(
            serde_json::to_string(&state).expect("a state serialises"),
            wire,
            "PaneAgentState must be snake_case on the wire"
        );
    }

    for wire in ["idle", "working", "blocked", "done", "unknown"] {
        let status: AgentStatus = serde_json::from_str(&format!("\"{wire}\""))
            .unwrap_or_else(|err| panic!("AgentStatus must decode {wire:?}: {err}"));
        assert_eq!(
            format!("{status:?}").to_lowercase(),
            wire,
            "AgentStatus decoded {wire:?} as the wrong variant"
        );
    }

    // A status herdr 0.9.1 does not publish decodes into its own arm, carrying the exact
    // spelling — it neither fails the line (which took the whole `session.snapshot` with it) nor
    // folds into a status herdr DOES publish (which would be a fabricated reading).
    let martian: AgentStatus = serde_json::from_str("\"martian\"")
        .expect("an unknown status is decoded, not a failed line");
    assert_eq!(martian, AgentStatus::Unrecognised("martian".to_owned()));
    assert_ne!(martian, AgentStatus::Unknown);
    assert_eq!(
        serde_json::to_string(&martian).expect("it round-trips"),
        "\"martian\"",
        "the spelling herdr sent is the spelling this client would send back"
    );
    // Only a JSON string reaches that arm: a number is still malformed, not a status.
    assert!(serde_json::from_str::<AgentStatus>("42").is_err());
}

/// `session.snapshot`, `agent.list` and `ping` all take an empty params object, and all three
/// still have to **send** it. `{"id":"x","method":"agent.list"}` is `invalid_request`.
#[test]
fn an_empty_params_method_still_sends_an_empty_object() {
    for method in [
        Method::SessionSnapshot(EmptyParams {}),
        Method::AgentList(EmptyParams {}),
        Method::Ping(PingParams {}),
    ] {
        let name = method.name();
        let line = serde_json::to_string(&Request {
            id: "req_1".to_owned(),
            method,
        })
        .expect("a request serialises");
        assert_eq!(
            line,
            format!(r#"{{"id":"req_1","method":"{name}","params":{{}}}}"#),
            "the empty params object must be on the wire"
        );
    }
}

/// herdr's own compatibility rule, made real rather than aspirational: *"JSON API clients should
/// ignore unknown fields"*
/// (`tmp/herdr/docs/preview/website/src/content/docs/socket-api.mdx:959`).
///
/// A newer herdr that adds a field to a record this client already decodes must not turn every
/// reply into a parse error. No type in `schema/` carries `#[serde(deny_unknown_fields)]`, and
/// this is what says so.
#[test]
fn an_unknown_field_on_a_record_is_ignored_not_an_error() {
    let tab: TabInfo = serde_json::from_str(
        r#"{"tab_id":"w1:t1","workspace_id":"w1","number":1,"label":"main","focused":true,
            "pane_count":2,"agent_status":"idle","a_field_from_a_newer_herdr":{"nested":[1,2]}}"#,
    )
    .expect("an unknown field must be ignored, not rejected");
    assert_eq!(tab.label, "main");

    let agent: AgentInfo = serde_json::from_str(
        r#"{"terminal_id":"t1","agent_status":"working","workspace_id":"w1","tab_id":"w1:t1",
            "pane_id":"w1:p1","focused":false,"revision":7,"something_new":"ignored"}"#,
    )
    .expect("an unknown field must be ignored, not rejected");
    assert_eq!(agent.terminal_id, "t1");
    assert_eq!(agent.revision, 7);
    assert_eq!(
        agent.tokens,
        BTreeMap::new(),
        "an omitted map must default to empty, not fail the line"
    );
    assert_eq!(agent.state_change_seq, 0);
    assert!(agent.agent_session.is_none());
}

/// `pane.read` answers a `ReadFormat` and a `ReadSource` back, so both have to survive the round
/// trip under herdr's `rename_all = "snake_case"` — including the two-word
/// `ReadSource::RecentUnwrapped`, whose wire spelling is `recent_unwrapped` and which is the one a
/// hand-written mirror gets wrong.
#[test]
fn the_read_vocabulary_round_trips_under_snake_case() {
    for (source, wire) in [
        (ReadSource::Visible, "visible"),
        (ReadSource::Recent, "recent"),
        (ReadSource::RecentUnwrapped, "recent_unwrapped"),
        (ReadSource::Detection, "detection"),
    ] {
        assert_eq!(
            serde_json::to_string(&source).expect("serialises"),
            format!("\"{wire}\"")
        );
        assert_eq!(
            serde_json::from_str::<ReadSource>(&format!("\"{wire}\"")).expect("decodes"),
            source
        );
    }

    for (format, wire) in [(ReadFormat::Text, "text"), (ReadFormat::Ansi, "ansi")] {
        assert_eq!(
            serde_json::to_string(&format).expect("serialises"),
            format!("\"{wire}\"")
        );
        assert_eq!(
            serde_json::from_str::<ReadFormat>(&format!("\"{wire}\"")).expect("decodes"),
            format
        );
    }
}
