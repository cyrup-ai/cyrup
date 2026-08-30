//! `resolveTargetInCwd` / `formatSessionRefs` / `waitForProjectSession` — the cwd-scoped target
//! resolver and the post-launch registration wait from `pi-intercom/project-agent.ts` (`v0.12.0`,
//! 324 lines; the file is byte-identical at `v0.10.1`, `sha1 bb336e38`).
//!
//! Both halves of that file are now ported. The pure resolver is here (`resolveTargetInCwd` at
//! `:188-226`, `formatSessionRefs` at `:298-302`, `waitForProjectSession` at `:255-296`); the
//! launcher half — `HerdrClient`, `openProjectPane`, `resolveProjectRoot` — is
//! [`crate::project_pane`], split out because it spawns processes and this module is pure.
//!
//! ICOM-042 landed the launcher against **Herdr**, upstream's own backend, so
//! `intercom({action:"send"|"ask", cwd, openProjectPaneIfMissing:true})` opens a project pane and
//! starts cyrup in it exactly as pi does. [`wait_for_project_session`] is what makes that new
//! session addressable: it is not on the roster until the agent inside the pane connects and
//! registers.

use crate::cwd::same_cwd;
use crate::transport::protocol::SessionInfo;

/// `ProjectTargetResolution` (`v0.10.1 project-agent.ts:35-40`).
///
/// Upstream models this as one struct with an optional `session` and an optional `reason`; both
/// optionals are decided by `kind`, so the Rust shape is the enum upstream's two return sites
/// already form. Upstream's third outcome — ambiguity — is a `throw`, which is the `Err` of
/// [`resolve_target_in_cwd`]'s `Result`, not a variant here.
#[derive(Clone, Debug, PartialEq)]
pub enum ProjectTargetResolution {
    /// `{ kind: "found", session, targetCwd }`.
    Found {
        /// The one session in `target_cwd` the input addressed.
        session: Box<SessionInfo>,
        /// The normalized directory the lookup ran against.
        target_cwd: String,
    },
    /// `{ kind: "missing", targetCwd, reason }`. Upstream's `reason` is `string | undefined` in the
    /// type but is populated at both `missing` return sites (`:198`, `:225`), which is why the
    /// `existing.reason ?? …` fallback at `v0.10.1 index.ts:1205` is unreachable.
    Missing {
        /// The normalized directory the lookup ran against.
        target_cwd: String,
        /// The upstream-worded explanation, used verbatim as the caller's error prefix.
        reason: String,
    },
}

/// `formatSessionRefs(sessions)` (`v0.10.1 project-agent.ts:298-302`):
///
/// ```text
/// sessions.map((s) => `${s.name || "Unnamed session"} (${s.id.slice(0, 8)})`).join(", ")
/// ```
///
/// The 8-character slice here is upstream's and is deliberately NOT
/// [`crate::identity::session_id_prefixes`]'s distinguishing prefix: this list is a disambiguation
/// hint, and upstream tells the caller to "Address one by session ID" rather than by this string.
#[must_use]
pub fn format_session_refs(sessions: &[&SessionInfo]) -> String {
    sessions
        .iter()
        .map(|session| {
            let name = session.name.as_deref().filter(|n| !n.is_empty()).unwrap_or("Unnamed session");
            let short: String = session.id.chars().take(8).collect();
            format!("{name} ({short})")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `resolveTargetInCwd(input)` (`v0.10.1 project-agent.ts:188-226`) — statement for statement.
///
/// The lookup ladder, in upstream's order, over the sessions whose cwd matches `target_cwd` under
/// [`same_cwd`]:
///
/// 1. no `to` → the sole OTHER session in that directory (0 ⇒ `Missing`, >1 ⇒ ambiguity error);
/// 2. exact id;
/// 3. case-insensitive exact name (>1 ⇒ ambiguity error);
/// 4. id prefix (>1 ⇒ ambiguity error);
/// 5. otherwise `Missing`.
///
/// Note that steps 2-4 do NOT exclude the caller's own session — upstream filters on
/// `currentSessionId` only in step 1 — so an explicit self-address resolves here and is refused by
/// the tool's shared `Cannot message the current session` guard, exactly as upstream orders it.
///
/// # Errors
/// The three ambiguity strings, verbatim from `:205`, `:215` and `:221`.
pub fn resolve_target_in_cwd(
    sessions: &[SessionInfo],
    current_session_id: &str,
    target_cwd: &str,
    to: Option<&str>,
) -> Result<ProjectTargetResolution, String> {
    let in_cwd: Vec<&SessionInfo> =
        sessions.iter().filter(|session| same_cwd(&session.cwd, target_cwd)).collect();
    // `const target = input.to?.trim();` then `if (!target)` — a whitespace-only `to` is falsy and
    // takes the no-target branch, it does not fall through to a doomed exact-id lookup.
    let target = to.map(str::trim).filter(|t| !t.is_empty());

    let Some(target) = target else {
        let candidates: Vec<&SessionInfo> =
            in_cwd.iter().copied().filter(|session| session.id != current_session_id).collect();
        return match candidates.as_slice() {
            [only] => Ok(ProjectTargetResolution::Found {
                session: Box::new((*only).clone()),
                target_cwd: target_cwd.to_string(),
            }),
            [] => Ok(ProjectTargetResolution::Missing {
                target_cwd: target_cwd.to_string(),
                reason: format!("No other intercom sessions are connected in {target_cwd}."),
            }),
            many => Err(format!(
                "Multiple intercom sessions are connected in {target_cwd}: {}. Specify 'to'.",
                format_session_refs(many)
            )),
        };
    };

    if let Some(by_id) = in_cwd.iter().copied().find(|session| session.id == target) {
        return Ok(ProjectTargetResolution::Found {
            session: Box::new(by_id.clone()),
            target_cwd: target_cwd.to_string(),
        });
    }

    // `session.name?.toLowerCase() === lowerName` — an unnamed session can never match, and the
    // comparison is case-insensitive on BOTH sides.
    let lower_name = target.to_lowercase();
    let by_name: Vec<&SessionInfo> = in_cwd
        .iter()
        .copied()
        .filter(|session| session.name.as_ref().is_some_and(|n| n.to_lowercase() == lower_name))
        .collect();
    match by_name.as_slice() {
        [only] => {
            return Ok(ProjectTargetResolution::Found {
                session: Box::new((*only).clone()),
                target_cwd: target_cwd.to_string(),
            });
        }
        [] => {}
        many => {
            return Err(format!(
                "Multiple intercom sessions named \"{target}\" are connected in {target_cwd}: {}. Address one by session ID.",
                format_session_refs(many)
            ));
        }
    }

    let by_prefix: Vec<&SessionInfo> =
        in_cwd.iter().copied().filter(|session| session.id.starts_with(target)).collect();
    match by_prefix.as_slice() {
        [only] => {
            return Ok(ProjectTargetResolution::Found {
                session: Box::new((*only).clone()),
                target_cwd: target_cwd.to_string(),
            });
        }
        [] => {}
        _ => {
            return Err(format!(
                "Multiple intercom sessions in {target_cwd} match ID prefix \"{target}\". Use a longer session ID prefix."
            ));
        }
    }

    Ok(ProjectTargetResolution::Missing {
        target_cwd: target_cwd.to_string(),
        reason: format!("No intercom session matching \"{target}\" is connected in {target_cwd}."),
    })
}

/// `waitForProjectSession(client, input)` (`v0.12.0 project-agent.ts:255-296`).
///
/// A launched pane is not yet addressable: the agent inside it has to connect and `register` before
/// the broker lists it. This polls the roster until it does.
///
/// `before_session_ids` is snapshotted BEFORE the launch (`index.ts:1532`), so the new session is
/// identified by DIFFERENCE. A cwd-only filter would happily return a peer that was already
/// starting there for its own reasons.
///
/// `launcher_name` parameterizes only the timeout sentence's vendor noun; with the Herdr backend
/// ICOM-042 shipped, that sentence is upstream's verbatim apart from `Pi` → `cyrup`.
///
/// # Errors
/// - `"Cancelled"` when `cancel` fires (`:269`), checked both at the top of each poll and while
///   sleeping between them.
/// - the ambiguity string at `:289` when more than one new session registers there.
/// - the timeout string at `:295`.
/// - the roster fetch's own error, when the broker connection fails outright.
pub async fn wait_for_project_session(
    client: &crate::transport::client::IntercomClient,
    project_root: &str,
    current_session_id: &str,
    before_session_ids: &std::collections::HashSet<String>,
    to: Option<&str>,
    cancel: &cyrup_core::CancelToken,
    launcher_name: &str,
) -> Result<SessionInfo, String> {
    use std::time::Duration;
    let timeout = Duration::from_millis(crate::project_pane::DEFAULT_PROJECT_AGENT_TIMEOUT_MS);
    let poll = Duration::from_millis(crate::project_pane::DEFAULT_PROJECT_AGENT_POLL_MS);
    // `Math.min(5_000, timeoutMs)` (`:270`) — one roster fetch may not outlive the whole wait.
    let list_timeout = timeout.min(Duration::from_secs(5));
    let started = tokio::time::Instant::now();

    while started.elapsed() < timeout {
        if cancel.is_cancelled() {
            return Err("Cancelled".to_string());
        }
        let sessions =
            client.list_sessions_with_timeout(list_timeout).await.map_err(|e| e.to_string())?;

        // `:272-282` — with an explicit `to`, reuse the SAME resolver the non-launch path uses, so
        // the id/name/prefix ladder cannot drift between the two. A resolver ERROR here (one of the
        // three ambiguity strings) is deliberately swallowed and retried: mid-launch ambiguity is
        // transient, and upstream's `resolved.kind === "found"` test ignores it identically.
        if let Some(to) = to.map(str::trim).filter(|t| !t.is_empty()) {
            if let Ok(ProjectTargetResolution::Found { session, .. }) =
                resolve_target_in_cwd(&sessions, current_session_id, project_root, Some(to))
            {
                return Ok(*session);
            }
        } else {
            let new_in_project: Vec<&SessionInfo> = sessions
                .iter()
                .filter(|s| !before_session_ids.contains(&s.id) && same_cwd(&s.cwd, project_root))
                .collect();
            match new_in_project.as_slice() {
                [only] => return Ok((*only).clone()),
                [] => {}
                many => {
                    return Err(format!(
                        "Multiple new intercom sessions registered in {project_root}: {}. Address one explicitly.",
                        format_session_refs(many)
                    ));
                }
            }
        }
        tokio::select! {
            () = tokio::time::sleep(poll) => {}
            () = cancel.cancelled() => return Err("Cancelled".to_string()),
        }
    }

    // `:295`, with the vendor noun parameterized and the product name substituted.
    Err(format!(
        "Timed out waiting for a cyrup intercom session to register in {project_root}. \
         The {launcher_name} pane may still be starting, or cyrup-intercom may not be loaded there."
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
    use super::*;
    use crate::transport::protocol::now_ms;

    fn session(id: &str, name: Option<&str>, cwd: &str) -> SessionInfo {
        SessionInfo {
            id: id.to_string(),
            name: name.map(str::to_string),
            runtime_fallback_alias: None,
            cwd: cwd.to_string(),
            model: "m".to_string(),
            pid: 1u32.into(),
            started_at: now_ms().into(),
            last_activity: now_ms().into(),
            status: None,
            peer_uid: None,
            trusted_local: None,
            context_pct: None,
            context_tokens: None,
            context_window: None,
            tmux_pane: None,
            extra: Default::default(),
        }
    }

    /// ICOM-042 / `v0.10.1 project-agent.ts:196-199`. With no `to`, the sole OTHER session in the
    /// directory is the target — this is what makes `send { cwd }` addressable without knowing the
    /// peer's name, and the caller's own session must not count towards "sole".
    #[test]
    fn no_target_resolves_the_sole_other_session_in_the_directory() {
        let sessions = vec![
            session("me", None, "/w/proj"),
            session("peer", Some("reviewer"), "/w/proj/"),
            session("elsewhere", Some("other"), "/w/other"),
        ];
        let resolved = resolve_target_in_cwd(&sessions, "me", "/w/proj", None).expect("resolves");
        match resolved {
            ProjectTargetResolution::Found { session, .. } => assert_eq!(session.id, "peer"),
            other => panic!("expected the sole peer, got {other:?}"),
        }
    }

    /// `:197-199` — zero OTHER sessions is `missing` with upstream's exact reason, NOT an error, so
    /// the caller can decide: offer the Herdr pane when `openProjectPaneIfMissing` is set, else
    /// report the reason. Both arms live in [`crate::tools::intercom`].
    #[test]
    fn no_peers_is_missing_with_upstreams_reason() {
        let sessions = vec![session("me", None, "/w/proj")];
        let resolved = resolve_target_in_cwd(&sessions, "me", "/w/proj", None).expect("resolves");
        assert_eq!(
            resolved,
            ProjectTargetResolution::Missing {
                target_cwd: "/w/proj".to_string(),
                reason: "No other intercom sessions are connected in /w/proj.".to_string(),
            }
        );
    }

    /// `:205` — the ambiguity error names the candidates as `name (id8)` pairs and tells the caller
    /// to pass `to`. A generic "ambiguous" string leaves no path forward, which is ICOM-013's point.
    #[test]
    fn two_peers_in_one_directory_is_an_ambiguity_error_naming_both() {
        let sessions = vec![
            session("me", None, "/w/proj"),
            session("aaaaaaaa-1111", Some("alpha"), "/w/proj"),
            session("bbbbbbbb-2222", None, "/w/proj"),
        ];
        let err = resolve_target_in_cwd(&sessions, "me", "/w/proj", None).expect_err("ambiguous");
        assert_eq!(
            err,
            "Multiple intercom sessions are connected in /w/proj: alpha (aaaaaaaa), Unnamed session (bbbbbbbb). Specify 'to'."
        );
    }

    /// `:209-221` — the id → name → id-prefix ladder, and that the name match is case-insensitive.
    #[test]
    fn explicit_target_walks_id_then_name_then_prefix() {
        let sessions = vec![
            session("me", None, "/w/proj"),
            session("aaaaaaaa-1111", Some("Reviewer"), "/w/proj"),
        ];
        let by_id = resolve_target_in_cwd(&sessions, "me", "/w/proj", Some("aaaaaaaa-1111"));
        assert!(matches!(by_id, Ok(ProjectTargetResolution::Found { .. })));
        let by_name = resolve_target_in_cwd(&sessions, "me", "/w/proj", Some("reviewer"));
        assert!(matches!(by_name, Ok(ProjectTargetResolution::Found { .. })));
        let by_prefix = resolve_target_in_cwd(&sessions, "me", "/w/proj", Some("aaaaaaaa"));
        assert!(matches!(by_prefix, Ok(ProjectTargetResolution::Found { .. })));
        // A whitespace-only `to` is JS-falsy (`input.to?.trim()`), so it takes the NO-target branch
        // and finds the sole peer rather than failing an exact-id lookup on "  ".
        let blank = resolve_target_in_cwd(&sessions, "me", "/w/proj", Some("   ")).expect("resolves");
        match blank {
            ProjectTargetResolution::Found { session, .. } => assert_eq!(session.id, "aaaaaaaa-1111"),
            other => panic!("a blank `to` must take the no-target branch, got {other:?}"),
        }
    }

    /// `:215` and `:221` — the two ambiguity errors below the no-target one are DISTINCT, and each
    /// names which kind of ambiguity was hit.
    #[test]
    fn name_and_prefix_ambiguity_produce_their_own_errors() {
        let by_name = vec![
            session("me", None, "/w/proj"),
            session("a-1", Some("twin"), "/w/proj"),
            session("b-2", Some("TWIN"), "/w/proj"),
        ];
        assert_eq!(
            resolve_target_in_cwd(&by_name, "me", "/w/proj", Some("twin")).expect_err("ambiguous"),
            "Multiple intercom sessions named \"twin\" are connected in /w/proj: twin (a-1), TWIN (b-2). Address one by session ID."
        );

        let by_prefix = vec![
            session("me", None, "/w/proj"),
            session("abc-1", None, "/w/proj"),
            session("abc-2", None, "/w/proj"),
        ];
        assert_eq!(
            resolve_target_in_cwd(&by_prefix, "me", "/w/proj", Some("abc")).expect_err("ambiguous"),
            "Multiple intercom sessions in /w/proj match ID prefix \"abc\". Use a longer session ID prefix."
        );
    }

    /// `:222-225` — an explicit target with no match in that directory is `missing`, and the reason
    /// names both the target and the directory.
    #[test]
    fn an_unmatched_explicit_target_is_missing_not_an_error() {
        let sessions = vec![session("me", None, "/w/proj"), session("peer", Some("x"), "/w/other")];
        let resolved =
            resolve_target_in_cwd(&sessions, "me", "/w/proj", Some("ghost")).expect("resolves");
        assert_eq!(
            resolved,
            ProjectTargetResolution::Missing {
                target_cwd: "/w/proj".to_string(),
                reason: "No intercom session matching \"ghost\" is connected in /w/proj.".to_string(),
            }
        );
    }
}
