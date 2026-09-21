//! Focusing a herdr pane — **one call**, where pi needs two and still misses.
//!
//! Upstream: `src/inspectors/herdr/focus.ts` (55 lines @v0.68.0).
//!
//! # `[CYRUP-EXCEEDS-UPSTREAM]` — `pane.focus`, and the two pi sentences its existence deletes
//!
//! **The premise, greppable:** herdr v0.9.1 publishes
//! `#[serde(rename = "pane.focus")] PaneFocus(PaneTarget)` and `herdr pane focus <pane_id>`
//! (`tmp/herdr/src/cli/pane.rs:26` → `:201-211`). Its handler
//! (`tmp/herdr/src/app/api/panes.rs:484-500`) focuses the pane **across tabs and workspaces in a
//! single call** and sets `mode = Terminal`; herdr's own test for it is
//! `api_pane_focus_focuses_direct_target_across_tabs_and_workspaces`
//! (`tmp/herdr/src/app/api/panes.rs:4152`). The method is one of **16** that are live but absent
//! from the published method table (`socket-api.mdx:96-113` lists 89 of the 105 in
//! `src/api/schema.rs:47-271`; the other fifteen are `client_shell.surface.set`,
//! `command.invoke`, `integration.list`, `pane.clear`, `pane.copy_motion`, `pane.copy_search`,
//! `pane.edit_scrollback`, `pane.input.set`, `pane.link.activate`, `pane.link.resolve`,
//! `pane.scroll`, `pane.selection.read`, `product_announcement.dismiss`,
//! `release_notes.dismiss` and `server.live_handoff`). That is why a reader of the docs alone
//! would conclude it does not exist.
//!
//! **What pi does instead** (`focus.ts:32-55`): `pane get <id>`, then `tab focus <tab_id>` **or**
//! `workspace focus <workspace_id>`. Two round-trips, and they focus the *tab*, leaving the pane
//! unfocused inside it. Worse, both of pi's own refusals are factually false against 0.9.1:
//!
//! * `PANE_FOCUS_UNSUPPORTED` (`focus.ts:50-51`) is unreachable, because
//!   [`cyrup_herdr::schema::PaneInfo`] always carries `tab_id` and `workspace_id` as **required**
//!   fields (`crates/cyrup-herdr/src/schema/panes.rs:435-437`).
//! * `openHerdrInspector:100`'s *"Herdr cannot refocus an arbitrary raw pane id; select it in the
//!   Herdr UI."* is simply untrue — see [`super::actions`], which no longer emits it.
//!
//! **So cyrup sends `pane focus <id>` and nothing else.** [`HerdrFocusErrorCode::PaneFocusUnsupported`]
//! stays in the contract's vocabulary and stays reachable, for the one case that is real: a herdr
//! that refuses `pane.focus` AND whose pane record carries neither id. Which herdr versions those
//! are is NOT asserted here — `tmp/herdr` is pinned at one commit (`d59d060`, v0.9.1) with 50
//! commits of history and no release tags, so nothing in this tree can establish when a method
//! appeared. What is greppable is that at v0.9.1 the method exists and both ids are required, so
//! the arm is unreachable against THIS herdr; telling the user to select the pane by hand is the
//! honest answer for any build where it is not.
//!
//! **Why not keep pi's two-hop as a fallback.** It cannot be written here: `cyrup-herdr`'s
//! `Method` enum has no `tab.focus` and no `workspace.focus` variant
//! (`crates/cyrup-herdr/src/schema/request.rs:57-135`), because no caller had ever needed one.
//! Rebuilding them to reproduce a worse focus would be adding wire surface in order to do less.

use serde_json::{Map, Value};

use super::client::{self, HerdrFailure};
use crate::inspectors::plugins::HerdrClient;
use crate::inspectors::types::{HerdrErrorCode, HerdrFocusErrorCode};

/// pi `herdrPaneRecord` (`focus.ts:9-15`): the `pane` member when there is one, else the value
/// itself.
///
/// Both shapes are real. herdr's CLI prints the whole `ResponseResult`, whose `pane.get`/
/// `pane.split`/`pane.focus` variant nests the record under `pane`
/// (`tmp/herdr/src/cli/runtime.rs:9-14`, `tmp/herdr/src/app/api/panes.rs:134`), and pi's client
/// then strips only the outer `result` (`client.ts:97-98`). The un-nested rung is the tolerance
/// upstream kept for a differently-shaped answer, and it is kept here for the same reason.
#[must_use]
pub fn herdr_pane_record(value: &Value) -> Option<&Map<String, Value>> {
    let record = value.as_object()?;
    match record.get("pane") {
        Some(Value::Object(pane)) => Some(pane),
        _ => Some(record),
    }
}

/// The first of `keys` that is a non-empty string — pi's `text` helper (`focus.ts:17-20`).
#[must_use]
pub fn record_text<'a>(record: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .filter_map(|key| record.get(*key))
        .filter_map(Value::as_str)
        .find(|text| !text.is_empty())
}

/// pi `herdrPaneFocusTarget` (`focus.ts:22-30`) — the three ids, each under both spellings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaneFocusTarget {
    /// `pane_id` / `paneId` / `id`.
    pub pane_id: Option<String>,
    /// `tab_id` / `tabId`.
    pub tab_id: Option<String>,
    /// `workspace_id` / `workspaceId`.
    pub workspace_id: Option<String>,
}

/// pi `herdrPaneFocusTarget` (`focus.ts:22-30`).
#[must_use]
pub fn herdr_pane_focus_target(value: &Value) -> PaneFocusTarget {
    let Some(pane) = herdr_pane_record(value) else {
        return PaneFocusTarget::default();
    };
    PaneFocusTarget {
        pane_id: record_text(pane, &["pane_id", "paneId", "id"]).map(str::to_owned),
        tab_id: record_text(pane, &["tab_id", "tabId"]).map(str::to_owned),
        workspace_id: record_text(pane, &["workspace_id", "workspaceId"]).map(str::to_owned),
    }
}

/// pi `paneId` (`herdr/actions.ts:74-82`) — the same probe, narrowed to the one id.
#[must_use]
pub fn pane_id_of(value: &Value) -> Option<String> {
    herdr_pane_focus_target(value).pane_id
}

/// What a successful focus reports back — pi's `HerdrPaneFocusResult` success payload
/// (`focus.ts:5-7`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusedPane {
    /// The pane that now has focus.
    pub pane_id: String,
    /// Its tab, when herdr named one.
    pub tab_id: Option<String>,
    /// Its workspace, when herdr named one.
    pub workspace_id: Option<String>,
}

/// A failed focus: the contract's focus vocabulary, plus the sentence.
///
/// Same shape and same reason as [`HerdrFailure`] — see its doc for why the message is carried
/// beside the code rather than inside the contract's seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusFailure {
    /// The contract's focus code.
    pub code: HerdrFocusErrorCode,
    /// The sentence to show.
    pub message: String,
}

impl FocusFailure {
    /// A failure with an explicit message.
    #[must_use]
    pub fn new(code: HerdrFocusErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl From<HerdrFailure> for FocusFailure {
    fn from(failure: HerdrFailure) -> Self {
        Self {
            code: HerdrFocusErrorCode::Herdr(failure.code),
            message: failure.message,
        }
    }
}

/// Focus `pane_id`.
///
/// **One `pane focus` call**, for the reason in the module doc. On failure this asks `pane get`
/// once — not to retry the focus, but to classify: a record carrying neither `tab_id` nor
/// `workspace_id` is the pre-0.9 shape pi's [`HerdrFocusErrorCode::PaneFocusUnsupported`] was
/// written for, and it earns pi's own sentence. Anything else is the focus failure itself,
/// unchanged.
///
/// # Errors
/// [`HerdrFocusErrorCode::Herdr`] for a transport or API failure,
/// [`HerdrFocusErrorCode::InvalidPaneResponse`] when herdr answers with no pane id at all
/// (`focus.ts:36-38`), and [`HerdrFocusErrorCode::PaneFocusUnsupported`] for the pre-0.9 record.
pub async fn focus_herdr_pane(
    herdr: &dyn HerdrClient,
    pane_id: &str,
) -> Result<FocusedPane, FocusFailure> {
    let focused = match client::call(herdr, &["pane", "focus", pane_id]).await {
        Ok(value) => value,
        Err(failure) => return Err(classify_focus_failure(herdr, pane_id, failure).await),
    };
    let target = herdr_pane_focus_target(&focused);
    let Some(focused_pane_id) = target.pane_id else {
        return Err(FocusFailure::new(
            HerdrFocusErrorCode::InvalidPaneResponse,
            format!("Herdr pane focus returned no pane id for '{pane_id}'."),
        ));
    };
    Ok(FocusedPane {
        pane_id: focused_pane_id,
        tab_id: target.tab_id,
        workspace_id: target.workspace_id,
    })
}

/// Decide whether a failed `pane focus` is "this herdr has no `pane.focus`" or a real failure.
///
/// `NOT_FOUND` short-circuits: herdr found no such pane, which is not a focus-support question and
/// must not be relabelled as one.
async fn classify_focus_failure(
    herdr: &dyn HerdrClient,
    pane_id: &str,
    failure: HerdrFailure,
) -> FocusFailure {
    if failure.code == HerdrErrorCode::NotFound || failure.code == HerdrErrorCode::Unavailable {
        return failure.into();
    }
    let Ok(record) = client::call(herdr, &["pane", "get", pane_id]).await else {
        return failure.into();
    };
    let target = herdr_pane_focus_target(&record);
    if target.tab_id.is_none() && target.workspace_id.is_none() {
        // pi's own sentence (`focus.ts:51`), kept verbatim for the one build it is true of.
        return FocusFailure::new(
            HerdrFocusErrorCode::PaneFocusUnsupported,
            format!(
                "Herdr pane '{pane_id}' has no tab_id or workspace_id. Select it in Herdr \
                 manually, or upgrade Herdr focus support."
            ),
        );
    }
    failure.into()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::sync::Mutex;

    use serde_json::json;

    use super::*;

    #[derive(Default)]
    struct FakeHerdrClient {
        calls: Mutex<Vec<Vec<String>>>,
        script: Mutex<Vec<Result<Value, HerdrErrorCode>>>,
    }

    impl FakeHerdrClient {
        fn scripted(script: Vec<Result<Value, HerdrErrorCode>>) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                script: Mutex::new(script),
            }
        }

        fn calls(&self) -> Vec<Vec<String>> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl HerdrClient for FakeHerdrClient {
        async fn run(&self, args: &[&str]) -> Result<Value, HerdrErrorCode> {
            self.calls
                .lock()
                .unwrap()
                .push(args.iter().map(|a| (*a).to_owned()).collect());
            let mut script = self.script.lock().unwrap();
            if script.is_empty() {
                return Ok(json!({}));
            }
            script.remove(0)
        }
    }

    fn pane(pane_id: &str) -> Value {
        json!({ "pane": { "pane_id": pane_id, "tab_id": "w1:t1", "workspace_id": "w1" } })
    }

    /// **T-FOCUS-1.** GUT this to pi's two-hop (`pane get` → `tab focus`) and the argv assertion
    /// goes red — and nothing else in the suite would notice, because the two-hop *also* ends in a
    /// focused tab. This test is the whole guard on the `[CYRUP-EXCEEDS-UPSTREAM]` premise.
    #[tokio::test]
    async fn focus_uses_one_pane_focus_call() {
        let herdr = FakeHerdrClient::scripted(vec![Ok(pane("w1:p2"))]);
        let focused = focus_herdr_pane(&herdr, "w1:p2").await.unwrap();

        assert_eq!(herdr.calls(), vec![vec!["pane", "focus", "w1:p2"]]);
        assert_eq!(focused.pane_id, "w1:p2");
        assert_eq!(focused.tab_id.as_deref(), Some("w1:t1"));
        assert_eq!(focused.workspace_id.as_deref(), Some("w1"));

        // Explicitly: neither of pi's two hops was issued.
        let issued = herdr.calls();
        assert!(
            !issued
                .iter()
                .any(|call| call.first().is_some_and(|verb| verb == "tab"))
        );
        assert!(
            !issued
                .iter()
                .any(|call| call.first().is_some_and(|verb| verb == "workspace"))
        );
    }

    /// GUT the `pane_id.is_none()` branch and a herdr that answers with an empty envelope is
    /// reported as a successful focus of a pane that was never focused.
    #[tokio::test]
    async fn an_answer_with_no_pane_id_is_an_invalid_pane_response() {
        let herdr = FakeHerdrClient::scripted(vec![Ok(json!({ "pane": {} }))]);
        let failure = focus_herdr_pane(&herdr, "w1:p2").await.unwrap_err();
        assert_eq!(failure.code, HerdrFocusErrorCode::InvalidPaneResponse);
        assert_eq!(
            failure.message,
            "Herdr pane focus returned no pane id for 'w1:p2'."
        );
        assert_eq!(failure.code.as_str(), "INVALID_PANE_RESPONSE");
    }

    /// The one build `PANE_FOCUS_UNSUPPORTED` is true of. GUT `classify_focus_failure` to
    /// `failure.into()` and this goes red: a pre-0.9 herdr would report a bare
    /// `VALIDATION_ERROR` instead of telling the user what to do about it.
    #[tokio::test]
    async fn a_pre_0_9_herdr_with_no_tab_or_workspace_is_pane_focus_unsupported() {
        let herdr = FakeHerdrClient::scripted(vec![
            Err(HerdrErrorCode::ValidationError),
            Ok(json!({ "pane": { "pane_id": "w1:p2" } })),
        ]);
        let failure = focus_herdr_pane(&herdr, "w1:p2").await.unwrap_err();
        assert_eq!(failure.code, HerdrFocusErrorCode::PaneFocusUnsupported);
        assert_eq!(
            failure.message,
            "Herdr pane 'w1:p2' has no tab_id or workspace_id. Select it in Herdr manually, or \
             upgrade Herdr focus support."
        );
    }

    /// GUT the `NotFound` short-circuit and a genuinely missing pane is relabelled as a
    /// focus-support problem — which sends the user to the herdr UI to look for a pane that is not
    /// there.
    #[tokio::test]
    async fn a_missing_pane_is_not_relabelled_as_a_focus_support_problem() {
        let herdr = FakeHerdrClient::scripted(vec![Err(HerdrErrorCode::NotFound)]);
        let failure = focus_herdr_pane(&herdr, "w1:p9").await.unwrap_err();
        assert_eq!(
            failure.code,
            HerdrFocusErrorCode::Herdr(HerdrErrorCode::NotFound)
        );
        assert_eq!(failure.code.as_str(), "NOT_FOUND");
        // The classify hop must not have run: a missing pane needs no second question.
        assert_eq!(herdr.calls().len(), 1);
    }

    /// GUT `herdr_pane_record`'s nested rung and every consumer reads the envelope instead of the
    /// record — which is the exact shape herdr sends (`[AUG — verbs]` V15).
    #[test]
    fn the_pane_record_probe_accepts_both_of_upstreams_shapes() {
        let nested = json!({ "pane": { "pane_id": "w1:p2", "tab_id": "w1:t1" } });
        let flat = json!({ "pane_id": "w1:p2", "tabId": "w1:t1" });
        assert_eq!(pane_id_of(&nested).as_deref(), Some("w1:p2"));
        assert_eq!(pane_id_of(&flat).as_deref(), Some("w1:p2"));
        assert_eq!(
            herdr_pane_focus_target(&flat).tab_id.as_deref(),
            Some("w1:t1")
        );
        // An empty string is not an id — pi's `text` helper requires truthiness (`focus.ts:18`).
        assert_eq!(pane_id_of(&json!({ "pane_id": "" })), None);
        assert_eq!(pane_id_of(&json!([1, 2])), None);
        // The `id` fallback rung (`focus.ts:26`).
        assert_eq!(
            pane_id_of(&json!({ "id": "w1:p3" })).as_deref(),
            Some("w1:p3")
        );
    }
}
