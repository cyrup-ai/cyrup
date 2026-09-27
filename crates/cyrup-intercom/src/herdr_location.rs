//! `herdr-location.ts` (`v0.14.0`, 137 lines, `0ffe1d5` / #129) — resolve every Herdr-hosted
//! session in a `list` roster against ONE live Herdr snapshot.
//!
//! Upstream's doc comment is the contract: "The registration-time pane id is only a stable join
//! key; workspace and tab are always taken from this snapshot and are never cached." The broker
//! calls [`resolve_current`] once per `list` request ([`crate::broker`]'s `handle_list`) and never
//! keeps the answer, so a moved pane is re-resolved by the next `list`.
//!
//! ## Herdr is consumed, not ported
//!
//! Upstream shells out to `herdr api snapshot` through `project-agent.ts`'s `createHerdrClient`.
//! cyrup reads the same `session.snapshot` over the socket client it already has
//! ([`cyrup_herdr::HerdrClient::session_snapshot`]), resolving the socket with Herdr's own ladder
//! ([`cyrup_herdr::resolve_socket_path`]) from the BROKER's environment — the same environment
//! upstream's `herdr` subprocess would inherit. What is ported is pi-intercom's join and its
//! failure vocabulary, below.
//!
//! [CYRUP-DELTA] the snapshot is TYPED here ([`SessionSnapshot`]), where upstream probes an untyped
//! object field by field. A reply that omits `panes`, `tabs` or `workspaces` — upstream's
//! `invalid_response` "Herdr snapshot omitted panes, tabs, or workspaces." — therefore arrives as a
//! decode failure of the client, and [`failure_reason`] maps that failure to the same
//! `invalid_response` reason, with the client's own message as the detail.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_herdr::schema::agents::AgentSessionRefKind;
use cyrup_herdr::schema::panes::PaneInfo;
use cyrup_herdr::schema::session::SessionSnapshot;
use cyrup_herdr::{HerdrClient, HerdrError};

use crate::transport::protocol::{
    HerdrLabelRef, HerdrLocation, HerdrUnavailableReason, SessionInfo, now_ms,
};

/// `client.run(["api", "snapshot"], { timeoutMs: 3_000 })` (`herdr-location.ts:11`, `:55`) — a
/// `list` must not wait on a wedged Herdr for the client default of 15 s.
pub const SNAPSHOT_TIMEOUT: Duration = Duration::from_millis(3_000);

/// The `agent_session.source` Herdr stamps on a pane running pi (`herdr-location.ts:92`). The
/// session-path join matches only panes reporting it.
pub const PI_AGENT_SESSION_SOURCE: &str = "herdr:pi";

/// A snapshot, or the reason and message it could not be read with.
pub type SnapshotOutcome = Result<SessionSnapshot, (HerdrUnavailableReason, String)>;

/// `let activeSnapshot` (`herdr-location.ts:7`): the ONE snapshot request in flight, shared by every
/// `list` that arrives while it runs. "Production calls share only an already-in-flight snapshot,
/// collapsing concurrent list bursts without caching a result for a later request."
type Flight = Arc<tokio::sync::OnceCell<SnapshotOutcome>>;
static ACTIVE_SNAPSHOT: Mutex<Option<Flight>> = Mutex::new(None);

/// `readCurrentSnapshot(client)` (`herdr-location.ts:9-14`): join the in-flight request or start
/// one, and clear the slot once it settles (`activeSnapshot.finally(() => activeSnapshot = undefined)`).
async fn read_current_snapshot() -> SnapshotOutcome {
    let flight = {
        let mut slot = ACTIVE_SNAPSHOT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(slot.get_or_insert_with(|| Arc::new(tokio::sync::OnceCell::new())))
    };
    let outcome = flight
        .get_or_init(|| async {
            let socket = cyrup_herdr::resolve_socket_path(&cyrup_herdr::ProcessEnv);
            HerdrClient::new(socket)
                .with_timeout(SNAPSHOT_TIMEOUT)
                .session_snapshot()
                .await
                .map_err(|e| (failure_reason(&e), e.to_string()))
        })
        .await
        .clone();
    let mut slot = ACTIVE_SNAPSHOT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if slot.as_ref().is_some_and(|f| Arc::ptr_eq(f, &flight)) {
        *slot = None;
    }
    outcome
}

/// Upstream's failure mapping (`herdr-location.ts:59-63`): `HERDR_UNAVAILABLE → herdr_unavailable`,
/// `HERDR_UNSUPPORTED_VERSION → unsupported`, anything else `command_failed`.
///
/// Over the socket the same three conditions are: no reachable server
/// ([`HerdrError::Unavailable`]); a Herdr that refuses the request shape
/// ([`HerdrError::is_unsupported_method`], an `invalid_request` — the build does not have
/// `session.snapshot`); and the rest. A reply that does not decode as a snapshot is
/// `invalid_response`, see the module doc.
#[must_use]
pub fn failure_reason(error: &HerdrError) -> HerdrUnavailableReason {
    if error.unavailable().is_some() {
        HerdrUnavailableReason::HerdrUnavailable
    } else if error.is_unsupported_method() {
        HerdrUnavailableReason::Unsupported
    } else if matches!(
        error,
        HerdrError::Malformed { .. } | HerdrError::UnexpectedResult { .. }
    ) {
        HerdrUnavailableReason::InvalidResponse
    } else {
        HerdrUnavailableReason::CommandFailed
    }
}

/// `resolveHerdrLocations(sessions, { sessionPaths })` in production (`herdr-location.ts:41-57`).
///
/// "Preserve the upstream roster byte-for-byte in installations where Herdr is not in use: no
/// subprocess and no additive rendering/state." — so a roster with no `herdrPaneId` is returned
/// untouched and Herdr is never contacted.
pub async fn resolve_current(
    sessions: Vec<SessionInfo>,
    session_paths: &HashMap<String, String>,
) -> Vec<SessionInfo> {
    if !any_hosted(&sessions) {
        return sessions;
    }
    let snapshot = read_current_snapshot().await;
    resolve_herdr_locations(sessions, &snapshot, session_paths, now_ms())
}

/// `sessions.filter((session) => session.herdrPaneId)` is non-empty — JS-truthy, so `""` is not
/// hosted.
#[must_use]
pub fn any_hosted(sessions: &[SessionInfo]) -> bool {
    sessions.iter().any(|s| hosted_pane(s).is_some())
}

fn hosted_pane(session: &SessionInfo) -> Option<&str> {
    session.herdr_pane_id.as_deref().filter(|p| !p.is_empty())
}

fn unavailable(pane_id: &str, reason: HerdrUnavailableReason, detail: &str) -> HerdrLocation {
    HerdrLocation::Unavailable {
        pane_id: pane_id.to_string(),
        reason,
        // `...(detail ? { detail } : {})`.
        detail: Some(detail.to_string()).filter(|d| !d.is_empty()),
    }
}

/// `stringField` (`herdr-location.ts:20-23`): a non-empty string, else absent.
fn non_empty(value: &str) -> Option<&str> {
    Some(value).filter(|v| !v.is_empty())
}

/// The join itself (`herdr-location.ts:58-136`), over a snapshot already read. Every session in the
/// roster gets a `herdrLocation`: `not_hosted` without a pane id, `current` when the pane resolves,
/// and `unavailable` with upstream's reason and detail otherwise.
#[must_use]
pub fn resolve_herdr_locations(
    sessions: Vec<SessionInfo>,
    snapshot: &SnapshotOutcome,
    session_paths: &HashMap<String, String>,
    refreshed_at: u64,
) -> Vec<SessionInfo> {
    let snapshot = match snapshot {
        Ok(snapshot) => snapshot,
        Err((reason, message)) => {
            return sessions
                .into_iter()
                .map(|mut session| {
                    session.herdr_location = Some(match hosted_pane(&session) {
                        Some(pane) => unavailable(pane, *reason, message),
                        None => HerdrLocation::NotHosted,
                    });
                    session
                })
                .collect();
        }
    };

    // `new Map(keyed(panes, "pane_id"))` — a later duplicate overwrites an earlier one, as a JS Map
    // built from entries does.
    let mut panes_by_id: HashMap<&str, &PaneInfo> = HashMap::new();
    // `null` marks a session path two panes advertise.
    let mut panes_by_session_path: HashMap<&str, Option<&PaneInfo>> = HashMap::new();
    for pane in &snapshot.panes {
        if let Some(id) = non_empty(&pane.pane_id) {
            panes_by_id.insert(id, pane);
        }
        let session_path = pane
            .agent_session
            .as_ref()
            .filter(|a| a.kind == AgentSessionRefKind::Path && a.source == PI_AGENT_SESSION_SOURCE)
            .and_then(|a| non_empty(&a.value));
        if let Some(path) = session_path {
            let seen = panes_by_session_path.contains_key(path);
            panes_by_session_path.insert(path, if seen { None } else { Some(pane) });
        }
    }
    let tabs_by_id: HashMap<&str, &str> = snapshot
        .tabs
        .iter()
        .filter_map(|t| Some((non_empty(&t.tab_id)?, t.label.as_str())))
        .collect();
    let workspaces_by_id: HashMap<&str, &str> = snapshot
        .workspaces
        .iter()
        .filter_map(|w| Some((non_empty(&w.workspace_id)?, w.label.as_str())))
        .collect();

    sessions
        .into_iter()
        .map(|mut session| {
            let location = match hosted_pane(&session) {
                None => HerdrLocation::NotHosted,
                Some(pane_id) => {
                    // "Pane ids are workspace-qualified launch aliases and change when a pane is
                    // moved. New clients register the Pi session file, which Herdr carries with
                    // the terminal across moves. Older clients fall back to a direct pane-id
                    // lookup and explicitly become unavailable after a move."
                    let pane = match session_paths.get(&session.id) {
                        Some(path) => panes_by_session_path.get(path.as_str()).copied(),
                        None => panes_by_id.get(pane_id).map(|p| Some(*p)),
                    };
                    locate(pane_id, pane, &tabs_by_id, &workspaces_by_id, refreshed_at)
                }
            };
            session.herdr_location = Some(location);
            session
        })
        .collect()
}

/// One hosted session's arm of the join (`herdr-location.ts:108-135`). `pane` is `None` when the
/// session is absent from the snapshot and `Some(None)` when two panes claim its identity.
fn locate(
    pane_id: &str,
    pane: Option<Option<&PaneInfo>>,
    tabs_by_id: &HashMap<&str, &str>,
    workspaces_by_id: &HashMap<&str, &str>,
    refreshed_at: u64,
) -> HerdrLocation {
    let pane = match pane {
        Some(Some(pane)) => pane,
        Some(None) => {
            return unavailable(
                pane_id,
                HerdrUnavailableReason::InvalidResponse,
                "Multiple Herdr panes advertise the same Pi session identity.",
            );
        }
        None => {
            return unavailable(
                pane_id,
                HerdrUnavailableReason::PaneMissing,
                "The hosted Pi session is absent from the current Herdr snapshot.",
            );
        }
    };
    let current = non_empty(&pane.pane_id);
    let tab_id = non_empty(&pane.tab_id);
    let workspace_id = non_empty(&pane.workspace_id);
    let tab_label = tab_id
        .and_then(|id| tabs_by_id.get(id).copied())
        .and_then(non_empty);
    let workspace_label = workspace_id
        .and_then(|id| workspaces_by_id.get(id).copied())
        .and_then(non_empty);
    match (current, tab_id, workspace_id, tab_label, workspace_label) {
        (
            Some(current),
            Some(tab_id),
            Some(workspace_id),
            Some(tab_label),
            Some(workspace_label),
        ) => HerdrLocation::Current {
            workspace: HerdrLabelRef {
                id: workspace_id.to_string(),
                label: workspace_label.to_string(),
            },
            tab: HerdrLabelRef {
                id: tab_id.to_string(),
                label: tab_label.to_string(),
            },
            pane_id: current.to_string(),
            refreshed_at: refreshed_at.into(),
        },
        _ => unavailable(
            pane_id,
            HerdrUnavailableReason::InvalidResponse,
            "Herdr snapshot could not resolve the pane's current labeled tab and workspace.",
        ),
    }
}

/// `formatHerdrLocation(session)` (`v0.14.0 index.ts:541-547`) — the `list` row's location term.
/// Empty when the roster carries no location (no hosted session, so Herdr was never asked).
#[must_use]
pub fn format_herdr_location(session: &SessionInfo) -> String {
    match &session.herdr_location {
        None => String::new(),
        Some(HerdrLocation::NotHosted) => "not under Herdr".to_string(),
        Some(HerdrLocation::Unavailable {
            pane_id, reason, ..
        }) => format!(
            "Herdr location unavailable: {} (pane {pane_id})",
            reason.as_str()
        ),
        Some(HerdrLocation::Current {
            workspace,
            tab,
            pane_id,
            ..
        }) => format!(
            "Herdr {} [{}] / {} [{}] / pane {pane_id}",
            workspace.label, workspace.id, tab.label, tab.id
        ),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    fn session(id: &str, herdr_pane_id: Option<&str>) -> SessionInfo {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "cwd": "/repo", "model": "test", "pid": 1,
            "startedAt": 1, "lastActivity": 1,
        }))
        .map(|mut s: SessionInfo| {
            s.herdr_pane_id = herdr_pane_id.map(str::to_string);
            s
        })
        .unwrap()
    }

    /// `herdr-location.test.ts`'s `snapshot(...)` fixture, shaped as herdr's `session.snapshot`.
    fn snapshot(
        ws: &str,
        ws_label: &str,
        tab: &str,
        tab_label: &str,
        pane: &str,
    ) -> SessionSnapshot {
        serde_json::from_value(serde_json::json!({
            "version": "0.9.1", "protocol": 1, "layouts": [], "agents": [],
            "panes": [
                { "pane_id": pane, "terminal_id": "t1", "tab_id": tab, "workspace_id": ws,
                  "focused": true, "agent_status": "idle", "revision": 0,
                  "agent_session": { "kind": "path", "source": "herdr:pi", "agent": "pi",
                                     "value": "/sessions/hosted.jsonl" } },
                { "pane_id": "pane-2", "terminal_id": "t2", "tab_id": tab, "workspace_id": ws,
                  "focused": false, "agent_status": "idle", "revision": 0 },
            ],
            "tabs": [{ "tab_id": tab, "workspace_id": ws, "number": 1, "label": tab_label,
                       "focused": true, "pane_count": 2, "agent_status": "idle" }],
            "workspaces": [{ "workspace_id": ws, "number": 1, "label": ws_label, "focused": true,
                             "pane_count": 2, "tab_count": 1, "active_tab_id": tab,
                             "agent_status": "idle" }],
        }))
        .expect("the fixture decodes as herdr's SessionSnapshot")
    }

    fn current(
        ws: &str,
        ws_label: &str,
        tab: &str,
        tab_label: &str,
        pane: &str,
        at: u64,
    ) -> HerdrLocation {
        HerdrLocation::Current {
            workspace: HerdrLabelRef {
                id: ws.into(),
                label: ws_label.into(),
            },
            tab: HerdrLabelRef {
                id: tab.into(),
                label: tab_label.into(),
            },
            pane_id: pane.into(),
            refreshed_at: at.into(),
        }
    }

    #[tokio::test]
    async fn an_all_non_herdr_roster_is_returned_untouched() {
        let inputs = vec![session("shell-a", None), session("shell-b", Some(""))];
        let resolved = resolve_current(inputs.clone(), &HashMap::new()).await;
        assert_eq!(resolved, inputs);
        assert!(resolved.iter().all(|s| s.herdr_location.is_none()));
    }

    #[test]
    fn hosted_and_non_herdr_sessions_resolve_from_one_snapshot() {
        let snap = Ok(snapshot(
            "workspace-1",
            "Platform",
            "tab-1",
            "API",
            "pane-1",
        ));
        let resolved = resolve_herdr_locations(
            vec![
                session("hosted", Some("pane-1")),
                session("second-hosted", Some("pane-2")),
                session("shell", None),
            ],
            &snap,
            &HashMap::new(),
            42,
        );
        assert_eq!(
            resolved[0].herdr_location,
            Some(current(
                "workspace-1",
                "Platform",
                "tab-1",
                "API",
                "pane-1",
                42
            ))
        );
        assert!(matches!(
            resolved[1].herdr_location,
            Some(HerdrLocation::Current { .. })
        ));
        assert_eq!(resolved[2].herdr_location, Some(HerdrLocation::NotHosted));
    }

    #[test]
    fn a_moved_pane_is_re_resolved_through_its_session_path() {
        let hosted = session("hosted", Some("launch-pane"));
        let paths = HashMap::from([(hosted.id.clone(), "/sessions/hosted.jsonl".to_string())]);
        let before = Ok(snapshot(
            "workspace-1",
            "Platform",
            "tab-1",
            "API",
            "pane-before",
        ));
        let after = Ok(snapshot(
            "workspace-2",
            "Research",
            "tab-2",
            "Review",
            "pane-after",
        ));
        let first = resolve_herdr_locations(vec![hosted.clone()], &before, &paths, 10);
        let second = resolve_herdr_locations(vec![hosted], &after, &paths, 20);
        assert_eq!(
            first[0].herdr_location,
            Some(current(
                "workspace-1",
                "Platform",
                "tab-1",
                "API",
                "pane-before",
                10
            ))
        );
        assert_eq!(
            second[0].herdr_location,
            Some(current(
                "workspace-2",
                "Research",
                "tab-2",
                "Review",
                "pane-after",
                20
            ))
        );
    }

    #[test]
    fn a_registered_pane_missing_from_the_snapshot_is_unavailable() {
        let snap = Ok(snapshot(
            "workspace-1",
            "Platform",
            "tab-1",
            "API",
            "other-pane",
        ));
        let resolved = resolve_herdr_locations(
            vec![session("stale", Some("gone-pane"))],
            &snap,
            &HashMap::new(),
            1,
        );
        assert_eq!(
            resolved[0].herdr_location,
            Some(HerdrLocation::Unavailable {
                pane_id: "gone-pane".into(),
                reason: HerdrUnavailableReason::PaneMissing,
                detail: Some(
                    "The hosted Pi session is absent from the current Herdr snapshot.".into()
                ),
            })
        );
    }

    #[test]
    fn duplicate_session_identities_are_not_guessed_between() {
        let mut snap = snapshot("workspace-1", "Platform", "tab-1", "API", "pane-1");
        snap.panes[1].agent_session = snap.panes[0].agent_session.clone();
        let hosted = session("hosted", Some("launch-pane"));
        let paths = HashMap::from([(hosted.id.clone(), "/sessions/hosted.jsonl".to_string())]);
        let resolved = resolve_herdr_locations(vec![hosted], &Ok(snap), &paths, 1);
        assert_eq!(
            resolved[0].herdr_location,
            Some(HerdrLocation::Unavailable {
                pane_id: "launch-pane".into(),
                reason: HerdrUnavailableReason::InvalidResponse,
                detail: Some("Multiple Herdr panes advertise the same Pi session identity.".into()),
            })
        );
    }

    #[test]
    fn a_failed_snapshot_keeps_the_roster_and_marks_hosted_sessions_unavailable() {
        let failed: SnapshotOutcome = Err((
            HerdrUnavailableReason::HerdrUnavailable,
            "missing binary".into(),
        ));
        let resolved = resolve_herdr_locations(
            vec![session("hosted", Some("pane-1")), session("shell", None)],
            &failed,
            &HashMap::new(),
            1,
        );
        assert_eq!(resolved.len(), 2);
        assert_eq!(
            resolved[0].herdr_location,
            Some(HerdrLocation::Unavailable {
                pane_id: "pane-1".into(),
                reason: HerdrUnavailableReason::HerdrUnavailable,
                detail: Some("missing binary".into()),
            })
        );
        assert_eq!(resolved[1].herdr_location, Some(HerdrLocation::NotHosted));
    }

    /// `formatHerdrLocation` (`v0.14.0 index.ts:541-547`) — and the wire shape `isHerdrLocation`
    /// accepts, round-tripped.
    #[test]
    fn the_location_term_and_wire_shape_are_upstreams() {
        let mut s = session("s", Some("w5:p4"));
        assert_eq!(format_herdr_location(&s), "");
        s.herdr_location = Some(current("w5", "Platform", "w5:t2", "API", "w5:p4", 7));
        assert_eq!(
            format_herdr_location(&s),
            "Herdr Platform [w5] / API [w5:t2] / pane w5:p4"
        );
        let wire = serde_json::to_value(&s.herdr_location).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({ "status": "current", "workspace": { "id": "w5", "label": "Platform" },
                                "tab": { "id": "w5:t2", "label": "API" }, "paneId": "w5:p4", "refreshedAt": 7 })
        );
        s.herdr_location = Some(HerdrLocation::NotHosted);
        assert_eq!(format_herdr_location(&s), "not under Herdr");
        s.herdr_location = Some(unavailable(
            "w1:p1",
            HerdrUnavailableReason::CommandFailed,
            "",
        ));
        assert_eq!(
            format_herdr_location(&s),
            "Herdr location unavailable: command_failed (pane w1:p1)"
        );
        // `isHerdrLocation`: an unknown status or reason, or an explicit `null` detail, is refused.
        for bad in [
            serde_json::json!({ "status": "somewhere" }),
            serde_json::json!({ "status": "unavailable", "paneId": "p", "reason": "nope" }),
            serde_json::json!({ "status": "unavailable", "paneId": "p", "reason": "pane_missing", "detail": null }),
            serde_json::json!({ "status": "current", "workspace": { "id": "w" }, "tab": { "id": "t", "label": "T" }, "paneId": "p", "refreshedAt": 1 }),
        ] {
            assert!(
                serde_json::from_value::<HerdrLocation>(bad.clone()).is_err(),
                "{bad}"
            );
        }
    }
}
