//! SESS-051/SEAM-131 — the `Cache warmed…: $x` transcript line, on BOTH the live and the replay
//! path.
//!
//! ```ts
//! // pi v1.0.4 coding-agent/src/modes/interactive/interactive-mode.ts
//! // :3428-3430 (live `entry_appended`)
//! } else if (event.entry.type === "usage" && event.entry.kind === "cache_warm") {
//!     this.addCacheWarmingUsage(event.entry);
//!     this.ui.requestRender();
//! }
//! // :4058 (replay flat-map)
//! if (entry.type === "custom" || (entry.type === "usage" && entry.kind === "cache_warm")) {
//!     return [entry];
//! }
//! // :4070-4076
//! private addCacheWarmingUsage(entry: UsageEntry): void {
//!     if (!this.settingsManager.getShowCacheMissNotices()) return;
//!     this.chatContainer.addChild(new Spacer(1));
//!     const usage = formatCacheWarmingUsage(entry);
//!     this.chatContainer.addChild(new ThemedText(() => theme.fg("dim", usage), 1, 0));
//! }
//! ```
//!
//! One renderer, two paths — the EXT-041 discipline. A single-path port passes a live-only test
//! while a resumed session shows nothing at all, which is why each arm is red-proved separately.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::transcript::Entry;
use crate::{App, UiTheme};
use ratatui::backend::TestBackend;
use serde_json::json;

/// A serialized `cache_warm` `usage` entry — the identical JSON the live
/// [`cyrup_session_svc::AgentSessionEvent::EntryAppended`] event and the replay
/// [`cyrup_session_svc::ReplayItem::UsageEntry`] both carry, which is the point: both paths parse
/// the same bytes through the same helper.
fn warm_entry(note: Option<&str>, total: f64) -> serde_json::Value {
    let mut v = json!({
        "type": "usage",
        "id": "w1",
        "parentId": "a1",
        "timestamp": "2026-10-08T00:00:02.000Z",
        "kind": "cache_warm",
        "provider": "anthropic",
        "model": "claude-sonnet-5",
        "usage": {
            "input": 0, "output": 1, "cacheRead": 42000, "cacheWrite": 0, "totalTokens": 42001,
            "cost": {"input": 0, "output": 0.000015, "cacheRead": 0.0126, "cacheWrite": 0,
                     "total": total}
        }
    });
    if let Some(n) = note {
        v["note"] = json!(n);
    }
    v
}

fn app(notices_on: bool) -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(80, 14), UiTheme::dark()).unwrap();
    app.state_mut().show_cache_miss_notices = notices_on;
    app
}

/// The dim status rows the transcript holds. `Entry::Status` is `Spacer(1)` + a dim `Text` at
/// paddingX 1, which is pi's two `addChild` calls in `addCacheWarmingUsage` exactly — and
/// deliberately NOT `Entry::Warning`, which the cache-MISS notice uses.
fn statuses(app: &App<TestBackend>) -> Vec<String> {
    app.state()
        .transcript
        .pending()
        .iter()
        .filter_map(|e| match e {
            Entry::Status(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

/// The LIVE path: `events_fold`'s `EntryAppended` arm must branch on the payload's `type`/`kind`
/// and route a warm to the notice instead of to `push_custom_entry`.
///
/// **Red-proved** by deleting the `cache_warm_usage_fields` branch from that arm (restoring the
/// unconditional `push_custom_entry(custom_entry_type(&entry), …)`): this test FAILED with no
/// status row at all, the warm having been swallowed as an unclaimed custom entry.
#[test]
fn seam131_the_live_entry_appended_arm_renders_the_warm() {
    let mut app = app(true);
    app.ingest_event_rendered_owned(
        cyrup_session_svc::AgentSessionEvent::EntryAppended {
            entry: warm_entry(None, 0.012_615),
        },
        crate::transcript::Rendered::None,
        crate::transcript::Rendered::None,
    );
    assert_eq!(statuses(&app), vec!["Cache warmed: $0.012615".to_string()]);
    assert!(
        !app.state()
            .transcript
            .pending()
            .iter()
            .any(|e| matches!(e, Entry::Warning(_))),
        "a warm is money SAVED: it renders dim like a status, not amber like a loss"
    );
}

/// The REPLAY path: `session_bind`'s walk must have its own arm for the same entry, so a resumed
/// session shows its past warms.
///
/// **Red-proved** by deleting that arm's body (leaving a bare `continue`): this test FAILED with no
/// status row, while the live test above kept passing — the exact asymmetry a single-path port
/// produces.
#[test]
fn seam131_the_replay_walk_renders_the_warm_identically() {
    let mut live = app(true);
    live.ingest_event_rendered_owned(
        cyrup_session_svc::AgentSessionEvent::EntryAppended {
            entry: warm_entry(Some("extension override"), 0.000_25),
        },
        crate::transcript::Rendered::None,
        crate::transcript::Rendered::None,
    );

    let mut replayed = app(true);
    replayed.replay_session_rendered(
        &[cyrup_session_svc::ReplayItem::UsageEntry(warm_entry(
            Some("extension override"),
            0.000_25,
        ))],
        &crate::app::ReplayRenders::default(),
    );

    assert_eq!(
        statuses(&replayed),
        vec!["Cache warmed (extension override): $0.00025".to_string()]
    );
    assert_eq!(
        statuses(&replayed),
        statuses(&live),
        "both paths must produce the identical line — one renderer, two arms"
    );
}

/// `if (!getShowCacheMissNotices()) return;` — re-read at EACH render site, so both paths go quiet.
///
/// **Red-proved** by removing the `self.state.show_cache_miss_notices` guard from the live arm
/// (the replay half then still passed) and separately from the replay arm.
#[test]
fn seam131_both_paths_respect_the_show_cache_miss_notices_gate() {
    let mut live = app(false);
    live.ingest_event_rendered_owned(
        cyrup_session_svc::AgentSessionEvent::EntryAppended {
            entry: warm_entry(None, 0.012_615),
        },
        crate::transcript::Rendered::None,
        crate::transcript::Rendered::None,
    );
    assert!(statuses(&live).is_empty(), "{:?}", statuses(&live));

    let mut replayed = app(false);
    replayed.replay_session_rendered(
        &[cyrup_session_svc::ReplayItem::UsageEntry(warm_entry(
            None, 0.012_615,
        ))],
        &crate::app::ReplayRenders::default(),
    );
    assert!(statuses(&replayed).is_empty(), "{:?}", statuses(&replayed));
}

/// A `usage` entry of some other kind is NOT a warm (pi's `entry.kind === "cache_warm"` guard), and
/// a `custom` entry must still reach the custom-entry surface rather than the notice.
#[test]
fn seam131_only_a_cache_warm_usage_entry_renders_the_notice() {
    let mut other = warm_entry(None, 0.01);
    other["kind"] = json!("something_else");
    let mut app_other = app(true);
    app_other.ingest_event_rendered_owned(
        cyrup_session_svc::AgentSessionEvent::EntryAppended { entry: other },
        crate::transcript::Rendered::None,
        crate::transcript::Rendered::None,
    );
    assert!(
        !statuses(&app_other)
            .iter()
            .any(|s| s.starts_with("Cache warmed")),
        "{:?}",
        statuses(&app_other)
    );

    let mut app_custom = app(true);
    app_custom.ingest_event_rendered_owned(
        cyrup_session_svc::AgentSessionEvent::EntryAppended {
            entry: json!({"type": "custom", "id": "c1", "parentId": null,
                          "timestamp": "2026-10-08T00:00:02.000Z", "customType": "mine"}),
        },
        crate::transcript::Rendered::None,
        crate::transcript::Rendered::None,
    );
    assert!(
        !statuses(&app_custom)
            .iter()
            .any(|s| s.starts_with("Cache warmed")),
        "a custom entry keeps its own path: {:?}",
        statuses(&app_custom)
    );
}
