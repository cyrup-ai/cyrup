//! `agent.view.set` / `agent.view.clear` — cyrup's fleet ordering, projected onto herdr's own
//! Agents sidebar.
//!
//! `[CYRUP-EXCEEDS-UPSTREAM]`. pi's herdr integration never sends either verb: its whole
//! `integrations/herdr-status.ts` (@v0.68.0) drives `pane report-metadata` and nothing else
//! (`:206,221`). The methods are herdr's, documented at
//! `tmp/herdr/docs/preview/website/src/content/docs/socket-api.mdx:418-495`, and they serve the
//! exact question this bridge exists to answer — *which pane needs me first*.
//!
//! # What the projection is, and what it deliberately is NOT
//!
//! It is a **sort, with no filter**: `attention` descending, then `state_change_seq` descending.
//! That is herdr's own attention rank (`AgentViewEntry::attention` is literally
//! `tab_attention_priority`, `tmp/herdr/src/app/agent_view.rs:170-176`) with the most recently
//! changed agent breaking ties, which is literally "whoever is blocked on a human, most recent
//! first".
//!
//! **It is therefore the same order herdr's own `AgentPanelSort::Priority` produces**
//! (`apply_agent_view`'s fallback sorts by `Reverse(tab_attention_priority)` then
//! `Reverse(last_agent_state_change_seq)`, `agent_view.rs:70-83`) — which is stated rather than
//! hidden, because it bounds what this buys: for a user already on the priority policy the
//! projection changes nothing, and for a user on any other policy it is what puts the blocked
//! pane at the top. Reproducing that order deliberately is also what makes it safe: cyrup is not
//! inventing a ranking, it is asserting herdr's.
//!
//! A filter is not sent, and that is a decision rather than an omission. There is exactly ONE
//! view server-wide (`state.agent_view_override`, a single `Option`,
//! `tmp/herdr/src/app/api/agent_view.rs:88-89`) and it governs "the expanded and collapsed
//! sidebar, mobile Agents list, mouse targets, indexed focus, and next/previous Agent navigation"
//! (`socket-api.mdx:421-424`). A filter installed by cyrup would therefore HIDE the user's other
//! agents — every one of them, including agents cyrup has nothing to do with — from their own
//! terminal, for as long as this session lives. Re-ordering degrades to "the list is in a
//! different order"; filtering degrades to "half my agents vanished". Only one of those is a
//! defensible thing to do to someone else's UI without asking.
//!
//! Omitting `sort` entirely would mean "leave `ui.agent_panel_sort` alone" (`socket-api.mdx:475-476`),
//! i.e. install nothing at all, so the sort is what makes the view exist.
//!
//! # Why it is OPT-IN
//!
//! Same reason, one step further: even a re-ordering is a change to the user's terminal that they
//! did not ask cyrup to make, it is global rather than scoped to cyrup's own pane, and it
//! atomically replaces whatever view another program installed (`socket-api.mdx:481`). So it
//! happens only when [`AGENT_VIEW_ENV`] is exactly `1`, read off the same [`EnvSource`] the
//! bridge's own gate uses — never the ambient process environment, for the reason
//! `cyrup_herdr::EnvSource` exists.
//!
//! # The clear is ownership-checked
//!
//! [`clear_params`] always names [`super::SOURCE`], so a shutdown removes cyrup's view and leaves
//! a view some other program installed in the meantime exactly where it is — herdr's own
//! *"a source mismatch leaves the active view unchanged"* (`socket-api.mdx:494`,
//! `app/api/agent_view.rs:49-62`). An unconditional clear would let a session that had already
//! been replaced delete the replacement on its way out.

use cyrup_herdr::EnvSource;
use cyrup_herdr::schema::{
    AgentViewBuiltinSortField, AgentViewClearParams, AgentViewSetParams, AgentViewSort,
};

/// The variable that opts this session into the sidebar projection. Exactly `1` enables it;
/// anything else, including unset, leaves the user's sidebar alone.
pub const AGENT_VIEW_ENV: &str = "CYRUP_HERDR_AGENT_VIEW";

/// The label herdr shows for the active view.
///
/// Inside herdr's bound — trimmed, non-empty, at most 32 characters
/// (`normalize_label`, `tmp/herdr/src/app/agent_view.rs:193-205`).
pub const AGENT_VIEW_LABEL: &str = "cyrup fleet";

/// Whether this session installs the projection.
#[must_use]
pub fn enabled(env: &impl EnvSource) -> bool {
    env.var(AGENT_VIEW_ENV).as_deref() == Some("1")
}

/// The `agent.view.set` params: attention first, most recent transition second.
///
/// `source` is [`super::SOURCE`] — the SAME authority name every report from this bridge carries,
/// which is what makes [`clear_params`]' ownership check line up, and which herdr accepts as a
/// non-`plugin:` source (`socket-api.mdx:479-481`). It is inside `[A-Za-z0-9:._-]{1,120}`
/// (`normalize_source`, `tmp/herdr/src/app/agent_view.rs:178-191`).
#[must_use]
pub fn set_params() -> AgentViewSetParams {
    let mut params = AgentViewSetParams::new(super::SOURCE);
    params.label = Some(AGENT_VIEW_LABEL.to_owned());
    params.sort = vec![
        AgentViewSort::desc(AgentViewBuiltinSortField::Attention),
        AgentViewSort::desc(AgentViewBuiltinSortField::StateChangeSeq),
    ];
    params
}

/// The `agent.view.clear` params: clear **only** the view this source still owns.
#[must_use]
pub fn clear_params() -> AgentViewClearParams {
    AgentViewClearParams::owned_by(super::SOURCE)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::collections::BTreeMap;

    use super::*;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    /// The gate is exactly `1`, and it is read off the injected environment.
    ///
    /// *Gutted by*: accepting any non-empty value (`"0"` then enables it); reading
    /// `std::env::var` instead of the parameter (every row goes red in this process).
    #[test]
    fn the_projection_is_opt_in_on_exactly_one() {
        assert!(!enabled(&env(&[])));
        assert!(!enabled(&env(&[(AGENT_VIEW_ENV, "0")])));
        assert!(!enabled(&env(&[(AGENT_VIEW_ENV, "true")])));
        assert!(!enabled(&env(&[(AGENT_VIEW_ENV, "")])));
        assert!(enabled(&env(&[(AGENT_VIEW_ENV, "1")])));
    }

    /// The bytes herdr receives — a sort with no filter, in herdr's own spelling.
    ///
    /// Asserted as JSON rather than on the struct, because the whole risk here is the WIRE shape:
    /// `AgentViewSortField` is `#[serde(untagged)]`, so a built-in must appear as the bare string
    /// `"attention"` and NOT as `{"Builtin":"attention"}` — which is what a derive without the
    /// attribute produces, and which herdr answers `invalid_agent_view` for.
    ///
    /// *Gutted by*: dropping `#[serde(untagged)]` from `AgentViewSortField`; dropping
    /// `rename_all = "snake_case"` from either enum (`"StateChangeSeq"`, `"Desc"`); adding a
    /// `filter`, which is the change this module's doc argues against and which shows up here as
    /// a key that must not be present.
    #[test]
    fn the_set_payload_is_herdrs_own_shape_and_carries_no_filter() {
        let json = serde_json::to_value(set_params()).expect("serializes");
        assert_eq!(
            json,
            serde_json::json!({
                "source": "cyrup:subagents",
                "label": "cyrup fleet",
                "sort": [
                    {"field": "attention", "order": "desc"},
                    {"field": "state_change_seq", "order": "desc"},
                ],
            })
        );
        assert!(
            json.get("filter").is_none(),
            "a filter would hide the user's other agents; see this module's doc"
        );
    }

    /// The label and source are inside herdr's own validators, which refuse the whole call.
    #[test]
    fn the_source_and_label_are_inside_herdrs_bounds() {
        assert!(!super::super::SOURCE.is_empty());
        assert!(super::super::SOURCE.chars().count() <= 120);
        assert!(
            super::super::SOURCE
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ':' | '.' | '_' | '-'))
        );
        assert!(!super::super::SOURCE.starts_with("plugin:"));
        assert!(!AGENT_VIEW_LABEL.is_empty());
        assert!(AGENT_VIEW_LABEL.chars().count() <= 32);
        assert!(AGENT_VIEW_LABEL.chars().all(|ch| !ch.is_control()));
    }

    /// The clear is ownership-scoped, always.
    ///
    /// *Gutted by*: returning `AgentViewClearParams::default()` — which clears whatever view is
    /// active, including one another program installed after cyrup's was replaced.
    #[test]
    fn the_clear_names_this_source() {
        let json = serde_json::to_value(clear_params()).expect("serializes");
        assert_eq!(json, serde_json::json!({ "source": "cyrup:subagents" }));
    }
}
