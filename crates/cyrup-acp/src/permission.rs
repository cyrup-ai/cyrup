//! The permission seam: `UiRequest` in, `session/request_permission` out.
//!
//! **Owner: agent C (`ACP-144`…`ACP-150`).**
//!
//! Port of pi-acp v0.0.33 `src/acp/session.ts`'s `extension_ui_request` arm,
//! `handleExtensionUiRequest`, `handleExtensionConfirm` / `handleExtensionSelect`, the
//! `CONFIRM_PERMISSION_OPTIONS` constant, `requestExtensionPermission` and `extensionUiToolCall`.
//!
//! # This is the richest thing the in-process decision buys, and it is why the port exists
//!
//! Over RPC an ACP host sees an `extension_ui_request` with a title and a method and **cannot tell
//! a permission ask from any other dialog** — which is exactly why pi-acp's `handleExtensionSelect`
//! synthesizes `allow_once` options for *every* select. In-process the sink receives the real
//! `cyrup_session_svc::UiRequest` with its typed [`cyrup_session_svc::UiKind`] and its embedded
//! `oneshot::Sender<UiReply>`, so the mapping onto ACP's `PermissionOption.kind` can be faithful.
//! [`DialogOptionTable::mint`] is where that happens.
//!
//! # `ACP-145` — `UiKind::Select` *is* the tool-permission dialog in cyrup
//!
//! `LocalAskChannel::confirm` (`crates/cyrup-permission-system/src/ask.rs`) — the function
//! `PermissionSystemExtension`'s prompt path calls — reaches the human through
//! **`HostServices::select`, not `confirm`**. Its own doc says so: *"Maps to
//! `HostServices::select` + `HostServices::input` (NOT `confirm` — port doc §7.3)."* It builds a
//! four-option list (`"Allow Once"`, `"Allow Always"`, `"Reject"`, `"Reject with Reason"`) and
//! decides the grant by an exact string `match selected.as_deref()`, with
//! `PermissionDecisionState::{Once, Always}` and `approved: true` on the two approve arms.
//!
//! So an option round-trip that returns an approve string the user did not pick — an off-by-one, a
//! stale per-dialog map, or a non-`Selected` outcome falling through to the selected-arm logic — is
//! a real `Once`/`Always` grant the user never gave. That is the permission-bypass clause, and it
//! is why [`DialogChoice`] exists and why its constructor is private.
//!
//! # The invariant that survives the cut, and it is the one that hangs a session
//!
//! Upstream answers `{id, cancelled:true}` at six sites plus a `.catch`, because a dialog whose
//! child died never settled. Here there is no wire id to be missing, so the silent-drop case is
//! unrepresentable — **but the reply must still be sent on every exit path including the error
//! one**: `LiveHostServices::ui_roundtrip` with no `DialogOptions.timeout` does a bare
//! `reply_rx.await` inside `block_in_place`, so a dropped sender there parks a runtime worker
//! thread *and* the wasm guest forever, the turn never settles, and the prompt never resolves
//! (`ACP-144`). [`serve_dialog`] is the only consumer of that sender and it answers on every path;
//! see its doc for the one case where it deliberately answers nothing, which is the case where
//! there is no longer anybody to answer.
//!
//! ## [CYRUP-DELTA] — the pending map becomes a per-dialog abandonment race
//!
//! **What differs.** `cyrup_modes::rpc` keeps a `HashMap<String, PendingUi>` and prunes it with
//! `pending.retain(|_, p| !p.reply.is_closed())` (`crates/cyrup-modes/src/rpc/mod.rs`) because
//! `LiveHostServices::ui_roundtrip` races the reply against `DialogOptions.timeout` and drops the
//! receiver when the countdown wins. That map exists only because the RPC wire correlates by id
//! and the loop must hold the sender across an arbitrary number of unrelated input lines. Here
//! correlation *is* the call stack: one task owns one dialog and one sender, so there is nothing
//! to key and nothing to prune. The same hazard is answered by racing the client round trip
//! against `oneshot::Sender::closed` inside [`serve_dialog`].
//!
//! **What it costs.** Nothing observable, and it closes the leak the map's pruning only bounded:
//! a timed-out dialog stops waiting on the human *immediately* instead of holding a task until the
//! client eventually answers. ADR-0028 §5 asks that the RPC shape be shared rather than
//! re-derived; the shareable part of it is the `UiKind` → deny-default table, and that **is**
//! shared in spirit by [`deny_default`], which is asserted bit-identical to
//! `cyrup_modes::rpc::default_ui_reply` in this module's tests. Lifting the function itself needs
//! an edge from `cyrup-acp` to `cyrup-modes` that neither crate has today; see the report's
//! `interface_changes_needed`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, CreateElicitationRequest, CreateElicitationResponse,
    ElicitationAction, ElicitationContentValue, ElicitationFormMode, ElicitationSchema,
    ElicitationSessionScope, Meta, PermissionOption, PermissionOptionId, PermissionOptionKind,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse, SessionId,
    SessionUpdate, StringPropertySchema, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
    ToolKind,
};
use agent_client_protocol::{BoxFuture, Client, ConnectionTo};
use cyrup_session_svc::{NotifyKind, UiEffect, UiEffectSink, UiKind, UiReply, UiRequest, UiSink};
use serde_json::{Value, json};

use crate::connection::ClientView;
use crate::turn::TurnSink;

// ---------------------------------------------------------------------------------------------
// The strings
// ---------------------------------------------------------------------------------------------

/// The `optionId` prefix upstream mints for a select's options (`CHOICE_OPTION_PREFIX`,
/// pi-acp v0.0.33 `session.ts`).
///
/// Kept byte-for-byte even though nothing parses it back out any more (see [`DialogChoice`]): it
/// is what a client's logs and a protocol trace show, and a gratuitous rename would make a
/// cross-implementation diff of the wire noisier for no gain.
const CHOICE_OPTION_PREFIX: &str = "choice-";

/// `CONFIRM_PERMISSION_OPTIONS[0]` — pi-acp v0.0.33 `session.ts`'s module-level constant,
/// `{optionId:'yes', name:'Yes', kind:'allow_once'}`. Byte-for-byte (`ACP-146`).
const CONFIRM_YES_ID: &str = "yes";
/// See [`CONFIRM_YES_ID`].
const CONFIRM_YES_NAME: &str = "Yes";
/// `CONFIRM_PERMISSION_OPTIONS[1]` — `{optionId:'no', name:'No', kind:'reject_once'}`.
const CONFIRM_NO_ID: &str = "no";
/// See [`CONFIRM_NO_ID`].
const CONFIRM_NO_NAME: &str = "No";

/// The four option strings `LocalAskChannel::confirm` builds, in the order it builds them
/// (`APPROVE_ONCE_OPTION`, `APPROVE_ALWAYS_OPTION`, `REJECT_OPTION`, `REJECT_WITH_REASON_OPTION`
/// in `crates/cyrup-permission-system/src/ask.rs`).
///
/// # [CYRUP-DELTA] — these four strings are duplicated, and that is safe by construction
///
/// **What differs.** They are `const`s private to `cyrup-permission-system`, and `cyrup-acp` has no
/// dependency edge to that crate, so they are written out again here.
///
/// **What it costs.** Nothing that can produce a wrong grant, and the reason is worth stating
/// because the duplication otherwise looks like exactly the hazard `ACP-145` is about. The reply
/// sent back to `ask.rs` is **the option string this dialog was given**, carried through
/// [`DialogOptionTable`] unmodified — never a string reconstructed from a constant here, so a drift
/// could only mis-advertise the four `PermissionOptionKind` rendering hints, never fabricate a
/// grant.
///
/// The drift is nonetheless checked rather than reasoned about:
/// `the_permission_dialog_list_is_the_permission_systems_own` asserts these four are byte-identical
/// to `cyrup_permission_system::PERMISSION_DIALOG_OPTIONS`, which `LocalAskChannel::confirm` builds
/// its `select` from. They are still written out here rather than imported so that this module's
/// matcher can be read — and reviewed — without opening another crate.
const PERMISSION_DIALOG_OPTIONS: [&str; 4] =
    ["Allow Once", "Allow Always", "Reject", "Reject with Reason"];

/// The `_meta` namespace this crate writes under, replacing upstream's `piAcp` (`ACP-148`).
///
/// # [CYRUP-DELTA] — the namespace carries cyrup's name, not another product's
///
/// **What differs.** pi-acp writes `_meta.piAcp.notify.level`; this writes
/// `_meta.cyrupAcp.notify.level`.
///
/// **What it costs.** A client written against pi-acp's `_meta` reads nothing here. `_meta` is
/// explicitly non-normative — the spec says implementations *"MUST NOT make assumptions about
/// values at these keys"* — so nothing in the protocol breaks, and claiming another product's
/// namespace would be worse than being ignored.
const META_NAMESPACE: &str = "cyrupAcp";

/// The chat chunk emitted when a dialog has no ACP rendering and is cancelled (`ACP-147`).
///
/// # [CYRUP-DELTA] — the message is rewritten and its trigger narrowed
///
/// **What differs.** Upstream emits `Pi ${method} UI request is not supported in ACP yet;
/// cancelling it.` for **every** `input` and `editor` dialog. This says
/// `This client cannot display an extension {method} dialog; cancelling it.`, and only when the
/// client did not advertise `elicitation` — when it did, the dialog is really answered
/// ([`ask_elicitation`]) and no chunk is emitted at all.
///
/// **What it costs.** A byte-parity audit against pi-acp will flag this line; that is what this
/// comment is for. Two reasons it is rewritten: "Pi" is another product's name and must not appear
/// in a cyrup user's transcript (gap-analysis 15 §3), and *"not supported in ACP yet"* would now be
/// a false statement about cyrup — the support exists, and what is missing is on the client's side.
/// Note the deliberate asymmetry with `ACP-142`/`ACP-143`, whose strings contain no product name
/// and are ported byte-for-byte.
fn unsupported_dialog_message(kind: UiKind) -> String {
    format!(
        "This client cannot display an extension {} dialog; cancelling it.",
        dialog_method(kind)
    )
}

/// The dialog's method name — pi's own four (`select`, `confirm`, `input`, `editor`), the same
/// strings `cyrup_modes::rpc`'s `extension_ui_request_json` puts on the RPC wire.
///
/// Used for the synthetic tool call's `rawInput.method` (`ACP-149`) and for
/// [`unsupported_dialog_message`], so both spell the dialog the same way the RPC front-end does.
#[must_use]
pub fn dialog_method(kind: UiKind) -> &'static str {
    match kind {
        UiKind::Confirm => "confirm",
        UiKind::Input => "input",
        UiKind::Select => "select",
        UiKind::Editor => "editor",
    }
}

// ---------------------------------------------------------------------------------------------
// The choice, and the table that is the only way to make one
// ---------------------------------------------------------------------------------------------

/// One option the user may pick in one dialog, proven to have come from **that** dialog's own
/// table.
///
/// ADR-0028 §3's newtype. The only way to obtain one is [`DialogOptionTable::choose`], a lookup in
/// the table that minted the ids for this dialog, so a stale or fabricated `option_id` has no path
/// to a reply and a table that outlives its dialog is a borrow error rather than a wrong answer.
///
/// This replaces upstream's strict index round-trip (`Number.isSafeInteger`, `>= 0`,
/// `String(index) === rest`, so `choice-01` and `choice-1.0` are rejected) — there is nothing left
/// to validate when the id was never parsed in the first place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DialogChoice(String);

impl DialogChoice {
    /// The option **string** the guest receives — never an index. Upstream's
    /// `select(title, options, opts): Promise<string|undefined>` returns the chosen option string
    /// too (`extensions/types.ts:133` @v0.83.0), and so does cyrup's own guest ABI: the `select`
    /// function of the `host-ui` interface in `crates/cyrup-ext/wit/world.wit` returns
    /// `option<string>`. Zero index bookkeeping on either side.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The per-dialog option table: the ids this agent minted, and what each means.
///
/// Named `DialogOptionTable`, not `DialogOptions`, because `cyrup_ext::host::DialogOptions` is a
/// different thing on the same seam — pi's `ExtensionUIDialogOptions` bag (`{signal?, timeout?}`)
/// that rides on every `UiRequest`. Two types called `DialogOptions` one import apart is how a
/// reviewer reads the wrong invariant.
///
/// Owned by the dialog task and dropped with it, which is what makes a stale id unresolvable rather
/// than merely unlikely.
///
/// # [CYRUP-DELTA] — one vector, not two parallel ones
///
/// **What differs.** Upstream keeps `options: string[]` and `permissionOptions: PermissionOption[]`
/// side by side and re-joins them by index at reply time (`options.at(index)`). Here the join is
/// the table: `advertised` is what goes on the wire and `table` is the id → string map, both built
/// in one pass and never re-indexed.
///
/// **What it costs.** One `String` clone per option at mint time, against the entire class of
/// off-by-one that `ACP-145` is rated `critical` for.
pub struct DialogOptionTable {
    /// The id → option-string map. A `Vec` rather than a `HashMap`: a dialog has a handful of
    /// options, and a linear scan over four entries beats hashing them, with the same total
    /// behaviour on a miss.
    table: Vec<(PermissionOptionId, String)>,
    /// In mint order, for the `session/request_permission` payload.
    advertised: Vec<PermissionOption>,
}

impl DialogOptionTable {
    /// Mint `choice-<n>` ids for a `UiKind::Select`'s option strings (`ACP-145`).
    ///
    /// Port of pi-acp v0.0.33 `session.ts`'s `handleExtensionSelect`'s
    /// `options.map((name, index) => ({optionId: 'choice-' + index, name, kind: 'allow_once'}))`.
    ///
    /// # The `kind` mapping, which is the decision this unit asks for
    ///
    /// Upstream marks **every** option `allow_once` because it cannot tell a permission ask from
    /// any other select. cyrup can, and the recognition is deliberately **all-or-nothing**: the
    /// faithful mapping is applied only when the option list is exactly
    /// [`PERMISSION_DIALOG_OPTIONS`], in that order — which is the list, and the only list,
    /// `LocalAskChannel::confirm` builds. Any other select (an extension's own menu) keeps
    /// upstream's blanket `allow_once`.
    ///
    /// Matching the *whole list* rather than each string on its own is the point. A per-string
    /// match would let an extension menu that merely happens to contain `"Reject"` be advertised
    /// with `reject_once`, and a client that treats a reject-kinded option as auto-deniable, or
    /// that offers "always" bookkeeping against an `allow_always`, would then apply permission
    /// affordances to something that is not a permission dialog.
    ///
    /// `kind` is a **rendering hint and nothing else**: the reply is
    /// [`DialogChoice`]'s string, so the grant `ask.rs` computes is identical either way. See
    /// [`PERMISSION_DIALOG_OPTIONS`] for why that makes the duplicated strings safe.
    #[must_use]
    pub fn mint(options: &[String]) -> Self {
        let permission_dialog = options.len() == PERMISSION_DIALOG_OPTIONS.len()
            && options
                .iter()
                .zip(PERMISSION_DIALOG_OPTIONS)
                .all(|(got, want)| got == want);

        let mut table = Vec::with_capacity(options.len());
        let mut advertised = Vec::with_capacity(options.len());
        for (index, name) in options.iter().enumerate() {
            let id = PermissionOptionId::new(format!("{CHOICE_OPTION_PREFIX}{index}"));
            let kind = if permission_dialog {
                permission_option_kind(name)
            } else {
                PermissionOptionKind::AllowOnce
            };
            advertised.push(PermissionOption::new(id.clone(), name.clone(), kind));
            table.push((id, name.clone()));
        }
        Self { table, advertised }
    }

    /// The two fixed options a `UiKind::Confirm` offers, in this order (`ACP-146`).
    ///
    /// Port of pi-acp v0.0.33 `session.ts`'s module-level `CONFIRM_PERMISSION_OPTIONS`: exactly
    /// `{optionId:'yes', name:'Yes', kind:'allow_once'}` then
    /// `{optionId:'no', name:'No', kind:'reject_once'}`, and **nothing else**. Ids, names, kinds
    /// and order are byte-for-byte upstream's.
    #[must_use]
    pub fn confirm() -> Self {
        let yes = PermissionOptionId::new(CONFIRM_YES_ID);
        let no = PermissionOptionId::new(CONFIRM_NO_ID);
        Self {
            table: vec![
                (yes.clone(), CONFIRM_YES_NAME.to_string()),
                (no.clone(), CONFIRM_NO_NAME.to_string()),
            ],
            advertised: vec![
                PermissionOption::new(yes, CONFIRM_YES_NAME, PermissionOptionKind::AllowOnce),
                PermissionOption::new(no, CONFIRM_NO_NAME, PermissionOptionKind::RejectOnce),
            ],
        }
    }

    /// What to put on the wire.
    #[must_use]
    pub fn advertised(&self) -> &[PermissionOption] {
        &self.advertised
    }

    /// Whether this dialog offered anything at all. An empty select answers cancelled immediately
    /// with **no** permission request, matching upstream (`ACP-145`).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.advertised.is_empty()
    }

    /// Resolve a client outcome into a choice.
    ///
    /// Returns `None` for a cancelled outcome, an unknown id, **and** for any variant a later
    /// schema version adds behind `RequestPermissionOutcome`'s `#[non_exhaustive]`.
    ///
    /// # `ACP-146` — why this is an `if let` and not a `match` with a `_` arm
    ///
    /// The unit's clause is that a wildcard arm which falls through to the selected-arm logic turns
    /// a cancellation into an approval. A `match` with `Selected(..) => …, _ => None` is correct
    /// *today* and stays correct only as long as nobody widens the wildcard; an `if let` has no
    /// wildcard arm to widen. Every outcome that is not `Selected` — `Cancelled`, and anything the
    /// schema grows — cannot reach the lookup at all, so the deny is structural rather than
    /// defended.
    ///
    /// Note the second, independent guard: an outcome the schema does not know **fails to
    /// deserialize**, so it arrives at [`ask_permission`] as `Err` and lands on [`deny_default`]
    /// there. Both routes deny; the tests pin both.
    #[must_use]
    pub fn choose(&self, outcome: &RequestPermissionOutcome) -> Option<DialogChoice> {
        if let RequestPermissionOutcome::Selected(selected) = outcome {
            return self
                .table
                .iter()
                .find(|(id, _)| *id == selected.option_id)
                .map(|(_, name)| DialogChoice(name.clone()));
        }
        None
    }
}

/// Map one of `LocalAskChannel::confirm`'s four option strings onto ACP's rendering hint.
///
/// `"Reject with Reason"` is `RejectOnce`, not `RejectAlways`: `ask.rs` gives it
/// `PermissionDecisionState::Reject`, the same state plain `"Reject"` gets — the reason is a
/// message back to the agent, not a persisted denial. A `reject_always` here would tell the client
/// this choice is remembered when it is not.
///
/// The `_` arm is unreachable for a list that passed [`DialogOptionTable::mint`]'s whole-list
/// check; it exists because the check and this mapping are separate functions, and `AllowOnce` is
/// upstream's own blanket value, so an unrecognised string is advertised exactly as pi-acp
/// advertises everything.
fn permission_option_kind(name: &str) -> PermissionOptionKind {
    match name {
        "Allow Once" => PermissionOptionKind::AllowOnce,
        "Allow Always" => PermissionOptionKind::AllowAlways,
        "Reject" | "Reject with Reason" => PermissionOptionKind::RejectOnce,
        _ => PermissionOptionKind::AllowOnce,
    }
}

/// The deny default for a dialog kind — bit-identical to `cyrup_modes::rpc`'s `default_ui_reply`.
///
/// Fail-closed on every path: a cancelled dialog, a client that returned a JSON-RPC error
/// (`ACP-150`), a timeout, and a fabricated option id all land here.
#[must_use]
pub fn deny_default(kind: UiKind) -> UiReply {
    match kind {
        UiKind::Confirm => UiReply::Confirm(false),
        UiKind::Input | UiKind::Select | UiKind::Editor => UiReply::Text(None),
    }
}

/// Turn a resolved outcome into the guest's reply — **the one place the safe fallback lives**.
///
/// `ACP-145`/`ACP-146`. Every path that does not produce a [`DialogChoice`] from *this dialog's*
/// table lands on [`deny_default`]; the confirmed `bool` is computed only from a choice that
/// [`DialogOptionTable::choose`] produced, which for a confirm dialog can only ever be `"Yes"` or
/// `"No"`.
///
/// Upstream computes `selected.outcome.optionId === 'yes'` directly on the response. The difference
/// matters: `'yes'` is compared against an id the *client* sent, so any client string that happens
/// to be `yes` confirms, whereas here the id must have been minted by this dialog before the
/// comparison is reached at all.
#[must_use]
pub fn reply_for(
    kind: UiKind,
    table: &DialogOptionTable,
    outcome: &RequestPermissionOutcome,
) -> UiReply {
    match table.choose(outcome) {
        None => deny_default(kind),
        Some(choice) => match kind {
            UiKind::Confirm => UiReply::Confirm(choice.as_str() == CONFIRM_YES_NAME),
            // The `select` function of `crates/cyrup-ext/wit/world.wit`'s `host-ui` interface
            // returns `option<string>` — the chosen option STRING — and `input`/`editor` never
            // reach here (they go through `elicitation/create`).
            UiKind::Input | UiKind::Select | UiKind::Editor => {
                UiReply::Text(Some(choice.as_str().to_string()))
            }
        },
    }
}

// ---------------------------------------------------------------------------------------------
// The client seam
// ---------------------------------------------------------------------------------------------

/// The two client round trips a dialog can make, behind a trait so the whole seam is testable with
/// no connection.
///
/// It extends [`TurnSink`] rather than re-declaring a `notify`: `ACP-147`'s fallback chunk and
/// `ACP-148`'s notification are ordinary `session/update`s and must go out by the same synchronous,
/// never-awaited route the turn actor uses. `ConnectionTo<Client>` already implements `TurnSink`.
///
/// `Sync` is added on top of `TurnSink`'s `Send + 'static` because one dialog task per request
/// shares this by `Arc` (`ACP-155`).
pub trait DialogClient: TurnSink + Sync {
    /// `session/request_permission` (`ACP-145`, `ACP-146`, `ACP-150`).
    ///
    /// The returned future is `'static` so a dialog task can own it; `Err` is the client refusing,
    /// the connection dropping, or a response the schema cannot parse.
    fn request_permission(
        &self,
        request: RequestPermissionRequest,
    ) -> BoxFuture<'static, Result<RequestPermissionResponse, agent_client_protocol::Error>>;

    /// `elicitation/create` (`ACP-147`). Only called when the client advertised the capability.
    fn create_elicitation(
        &self,
        request: CreateElicitationRequest,
    ) -> BoxFuture<'static, Result<CreateElicitationResponse, agent_client_protocol::Error>>;
}

/// The production implementor.
///
/// `SentRequest` deliberately cannot be `.await`ed directly — the SDK's own anti-footgun, since
/// awaiting a client response inside the dispatch loop deadlocks the connection. `block_task()` is
/// the sanctioned consumption *from a task outside that loop*, which is exactly where
/// [`PermissionBridge::run`] and its per-dialog children live (`ACP-155`).
impl DialogClient for ConnectionTo<Client> {
    fn request_permission(
        &self,
        request: RequestPermissionRequest,
    ) -> BoxFuture<'static, Result<RequestPermissionResponse, agent_client_protocol::Error>> {
        Box::pin(self.send_request(request).block_task())
    }

    fn create_elicitation(
        &self,
        request: CreateElicitationRequest,
    ) -> BoxFuture<'static, Result<CreateElicitationResponse, agent_client_protocol::Error>> {
        Box::pin(self.send_request(request).block_task())
    }
}

/// What the client said it can render, for the dialog task to gate on.
///
/// A tiny value rather than a borrow of [`ClientView`] so a dialog task can own a copy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DialogCaps {
    /// `ClientCapabilities.elicitation` — `ACP-147`'s gate for [`UiKind::Input`] and
    /// [`UiKind::Editor`].
    pub elicitation: bool,
}

impl DialogCaps {
    /// Project what `initialize` recorded.
    ///
    /// # [CYRUP-DELTA] — the gate is `elicitation`, not `elicitation.form`
    ///
    /// **What differs.** `ElicitationCapabilities` has two independent halves, `form` and `url`,
    /// and everything this module sends is **form** mode. The precise gate is therefore
    /// `caps.elicitation.as_ref().is_some_and(|e| e.form.is_some())`, but [`ClientView`] projects
    /// the capability down to one `bool` (`elicitation: caps.elicitation.is_some()`) and that file
    /// belongs to another owner.
    ///
    /// **What it costs.** A client advertising `{"elicitation": {"url": {}}}` and nothing else is
    /// sent a form elicitation it did not ask for. It answers with a JSON-RPC error or a
    /// `decline`, both of which land on [`deny_default`] — the same place the un-advertised path
    /// lands — so the cost is one wasted round trip and the loss of the fallback chat chunk, never
    /// a wrong answer. The one-line fix is in the report's `interface_changes_needed`.
    #[must_use]
    pub fn from_client(view: &ClientView) -> Self {
        Self {
            elicitation: view.elicitation,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// `ACP-149` — the synthetic tool call
// ---------------------------------------------------------------------------------------------

/// Everything a dialog carries **except its reply channel**.
///
/// `ACP-149`'s verify line is that the synthesised `rawInput` contains exactly the fields present
/// on the `UiRequest` and no others — *no reply-channel state, no options bag*. That is made
/// structural here rather than checked: [`ui_tool_call`] takes this type, which cannot name the
/// `oneshot::Sender` or the `DialogOptions` bag, so there is no field for a future edit to leak.
#[derive(Clone, Debug)]
pub struct DialogRequest {
    /// Which of the four dialogs this is.
    pub kind: UiKind,
    /// The dialog title (pi `title`), for all four kinds.
    pub prompt: String,
    /// `select`'s JSON array of option strings; `Null` otherwise.
    pub options: Value,
    /// `confirm`'s message body; `editor`'s seed text; empty for `input`/`select`.
    pub message: String,
    /// `input`'s placeholder, when the guest supplied one.
    pub placeholder: Option<String>,
}

impl DialogRequest {
    /// Split a [`UiRequest`] into the part that may be shown and the part that must be answered.
    fn split(request: UiRequest) -> (Self, tokio::sync::oneshot::Sender<UiReply>) {
        let UiRequest {
            kind,
            prompt,
            options,
            message,
            placeholder,
            // The `{signal?, timeout?}` bag. Deliberately dropped: `LiveHostServices::ui_roundtrip`
            // already races the reply against `timeout` itself and drops the receiver when the
            // countdown wins, which this module observes through `oneshot::Sender::closed`
            // (`serve_dialog`). ACP has no dialog-timeout field to forward it to, so re-deriving a
            // second countdown here could only disagree with the first.
            opts: _,
            reply,
        } = request;
        (
            Self {
                kind,
                prompt,
                options,
                message,
                placeholder,
            },
            reply,
        )
    }

    /// The title shown on the synthetic tool call.
    ///
    /// # [CYRUP-DELTA] — the fallback title drops another product's name
    ///
    /// **What differs.** Upstream is `stringProp(ev, 'title') ?? 'Pi ${method}'`; this is the
    /// guest's prompt, or `Extension {method}` when the guest gave none.
    ///
    /// **What it costs.** A byte-parity audit will flag it. "Pi" must not appear in a cyrup user's
    /// transcript (gap-analysis 15 §3), and this string is rendered as a tool-call title in the
    /// editor's own UI, which is about as user-visible as copy gets.
    fn title(&self) -> String {
        if self.prompt.trim().is_empty() {
            format!("Extension {}", dialog_method(self.kind))
        } else {
            self.prompt.clone()
        }
    }
}

/// The synthetic tool call a dialog rides on (`ACP-149`).
///
/// Port of pi-acp v0.0.33 `session.ts`'s `extensionUiToolCall`, including its `rawInput` allowlist
/// (`EXTENSION_UI_RAW_INPUT_KEYS = ['title','message','options','placeholder','prefill']`) and its
/// `method` key. `session/request_permission` has no "just ask the user something" shape, so the
/// dialog is dressed as a pending `other`-kind tool call; that is upstream's trick and it is the
/// only thing the protocol offers.
///
/// # [CYRUP-DELTA] — the id is a local counter, and the allowlist becomes a per-kind projection
///
/// **What differs.** (1) Upstream's `toolCallId` is `pi-ui-${id}` where `id` is the RPC
/// correlation uuid; there is no wire id in-process, so this is `cyrup-ui-{n}` from a per-bridge
/// counter. (2) Upstream copies the five allowlisted keys with `Object.hasOwn`, because its event
/// is an untyped bag whose shape it cannot know. Here `UiKind` says exactly which fields exist, so
/// the projection is written per kind: `message` only for `confirm` (pi's `message`), `prefill`
/// only for `editor` (pi's `prefill`, which cyrup carries in the same `UiRequest.message` field),
/// `options` only for `select`, `placeholder` only for `input`.
///
/// **What it costs.** A client correlating on the `pi-ui-` prefix sees a different one. In exchange
/// the "and no others" half of the verify line is a property of the code rather than of a runtime
/// allowlist: an absent field is absent because the kind has no such field, not because a key
/// lookup missed.
#[must_use]
pub fn ui_tool_call(seq: u64, request: &DialogRequest) -> ToolCallUpdate {
    let mut raw = serde_json::Map::new();
    raw.insert(
        "method".to_string(),
        Value::String(dialog_method(request.kind).to_string()),
    );
    if !request.prompt.is_empty() {
        raw.insert("title".to_string(), Value::String(request.prompt.clone()));
    }
    match request.kind {
        UiKind::Confirm => {
            if !request.message.is_empty() {
                raw.insert(
                    "message".to_string(),
                    Value::String(request.message.clone()),
                );
            }
        }
        UiKind::Editor => {
            if !request.message.is_empty() {
                raw.insert(
                    "prefill".to_string(),
                    Value::String(request.message.clone()),
                );
            }
        }
        UiKind::Select => {
            if request.options.is_array() {
                raw.insert("options".to_string(), request.options.clone());
            }
        }
        UiKind::Input => {
            if let Some(placeholder) = &request.placeholder {
                raw.insert(
                    "placeholder".to_string(),
                    Value::String(placeholder.clone()),
                );
            }
        }
    }

    ToolCallUpdate::new(
        format!("cyrup-ui-{seq}"),
        ToolCallUpdateFields::new()
            .kind(ToolKind::Other)
            .status(ToolCallStatus::Pending)
            .title(request.title())
            .raw_input(Value::Object(raw)),
    )
}

// ---------------------------------------------------------------------------------------------
// `ACP-148` — the notify effect
// ---------------------------------------------------------------------------------------------

/// Render a fire-and-forget [`UiEffect`] as a chat chunk, or nothing (`ACP-148`).
///
/// Port of pi-acp v0.0.33 `session.ts`'s `notify` branch of `handleExtensionUiRequest`: one
/// `agent_message_chunk` carrying the guest's message plus a severity in `_meta`. Every other
/// effect produces `None` — `SetStatus`, `SetWidget`, `SetTitle`, `SetEditorText` and the rest have
/// no ACP surface, and upstream reaches none of them either (they are not `extension_ui_request`
/// methods it recognises, so they fall through its final cancel).
///
/// # [CYRUP-DELTA] — no acknowledgement, and no missing-message fallback
///
/// **What differs.** (1) Upstream follows the chunk with
/// `sendExtensionUiResponse({id, cancelled:true})` because pi's `notify` rides the same correlated
/// request channel as the four real dialogs. `UiEffect` arrives on
/// [`cyrup_session_svc::UiEffectSink`], which carries no reply channel at all, so there is no
/// acknowledgement to send and the "answer exactly once" invariant now covers only the four
/// `UiKind`s. (2) Upstream's `stringProp(ev,'message') ?? 'Pi notification'` guards a key that
/// might be missing from an untyped bag; `UiEffect::Notify.message` is a `String` that always
/// exists, so the fallback is unrepresentable and is cut — an empty message emits an empty chunk,
/// which is also what upstream does for a present-but-empty `message`.
///
/// **What it costs.** A notify emitted with no ACP client attached is dropped, exactly as
/// `LiveHostServices` drops it with no sink installed.
#[must_use]
pub fn notify_chunk(effect: &UiEffect) -> Option<SessionUpdate> {
    match effect {
        UiEffect::Notify { message, kind } => {
            let mut meta = Meta::new();
            meta.insert(
                META_NAMESPACE.to_string(),
                json!({ "notify": { "level": notify_level(*kind) } }),
            );
            Some(SessionUpdate::AgentMessageChunk(
                ContentChunk::new(ContentBlock::from(message.clone())).meta(meta),
            ))
        }
        _ => None,
    }
}

/// pi's exact `notifyType` wire strings (`types.ts:135`), byte-for-byte the ones
/// `cyrup_modes::rpc`'s `notify_kind_str` emits.
fn notify_level(kind: NotifyKind) -> &'static str {
    match kind {
        NotifyKind::Info => "info",
        NotifyKind::Warning => "warning",
        NotifyKind::Error => "error",
    }
}

// ---------------------------------------------------------------------------------------------
// The bridge
// ---------------------------------------------------------------------------------------------

/// The `UiSink` half installed on the session, plus the receivers the dialog task drains.
///
/// `cyrup_session_svc::UiSink` is `UnboundedSender<UiRequest>` and `UiEffectSink` is
/// `UnboundedSender<UiEffect>`, so "the impl type" is the pair of pairs: the senders go to
/// `LiveHostServices::set_ui_sink` / `set_ui_effect_sink` and the receivers are owned here.
///
/// `ACP-155` — **the dialog must not be awaited on the event pump.** Upstream detaches with
/// `void this.handleExtensionUiRequest(ev)`; here the requests arrive on their own channel, are
/// serviced by their own task, and each dialog is detached again onto a task of its own, so a
/// permission dialog parked unanswered cannot stop the agent reaching `AgentSettled` even past the
/// fanout's channel capacity — and cannot stop a *second* dialog being serviced either, which the
/// single-task shape would have serialised behind an unbounded human wait.
pub struct PermissionBridge {
    ui_sink: UiSink,
    ui_rx: tokio::sync::mpsc::UnboundedReceiver<UiRequest>,
    effect_sink: UiEffectSink,
    effect_rx: tokio::sync::mpsc::UnboundedReceiver<UiEffect>,
}

impl PermissionBridge {
    /// Create the pair. Hand [`PermissionBridge::sink`] to `LiveHostServices::set_ui_sink` and
    /// [`PermissionBridge::effect_sink`] to `set_ui_effect_sink`, then drive
    /// [`PermissionBridge::run`] on its own task.
    #[must_use]
    pub fn new() -> Self {
        let (ui_sink, ui_rx) = tokio::sync::mpsc::unbounded_channel();
        let (effect_sink, effect_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            ui_sink,
            ui_rx,
            effect_sink,
            effect_rx,
        }
    }

    /// The dialog sender to install on the session's host services.
    #[must_use]
    pub fn sink(&self) -> UiSink {
        self.ui_sink.clone()
    }

    /// The fire-and-forget effect sender to install on the session's host services (`ACP-148`).
    #[must_use]
    pub fn effect_sink(&self) -> UiEffectSink {
        self.effect_sink.clone()
    }

    /// Service dialogs and effects until both sinks are dropped.
    ///
    /// `ACP-144`. Returns `()`, not `Result`: this is spawned, and `ConnectionTo::spawn`'s own doc
    /// is that *"if the spawned task returns an error, the entire server will shut down"* — a
    /// dialog the client refuses must cost one dialog, never the connection. There is nothing this
    /// loop can fail at anyway: a dropped notification is swallowed by [`TurnSink`] (`ACP-122`) and
    /// a refused round trip is a [`deny_default`] (`ACP-150`).
    ///
    /// A dialog arriving after the session was replaced is still answered — with the deny default
    /// if the client has gone — because the alternative is the parked wasm guest this whole module
    /// exists to prevent.
    pub async fn run<C: DialogClient>(self, session_id: SessionId, client: Arc<C>) {
        self.run_with_caps(session_id, client, DialogCaps::default())
            .await;
    }

    /// [`PermissionBridge::run`] with the client's advertised capabilities (`ACP-147`).
    pub async fn run_with_caps<C: DialogClient>(
        mut self,
        session_id: SessionId,
        client: Arc<C>,
        caps: DialogCaps,
    ) {
        // Drop our own senders so the loop ends when the session's copies go away.
        drop(self.ui_sink);
        drop(self.effect_sink);

        // The synthetic tool-call id source (`ACP-149`). Per bridge, so ids are unique within a
        // connection's live session and mean nothing outside it.
        let seq = AtomicU64::new(0);

        loop {
            tokio::select! {
                Some(request) = self.ui_rx.recv() => {
                    let client = Arc::clone(&client);
                    let session_id = session_id.clone();
                    let seq = seq.fetch_add(1, Ordering::Relaxed);
                    // `ACP-155`, upstream's `void this.handleExtensionUiRequest(ev)`: one task per
                    // dialog, so a human sitting on one prompt blocks neither this drain nor
                    // another guest's dialog. Detached rather than joined — each task is bounded by
                    // the client answering or by the guest dropping its receiver, so there is
                    // nothing to leak, and aborting one at shutdown would drop its reply sender
                    // unanswered, which is the exact hang `ACP-144` is about.
                    tokio::spawn(async move {
                        serve_dialog(&*client, &session_id, caps, seq, request).await;
                    });
                }
                Some(effect) = self.effect_rx.recv() => {
                    // `ACP-148` — fire and forget, on the same synchronous notify route the turn
                    // actor uses. Nothing is awaited here.
                    if let Some(update) = notify_chunk(&effect) {
                        client.notify(&session_id, update);
                    }
                }
                else => break,
            }
        }
    }
}

impl Default for PermissionBridge {
    fn default() -> Self {
        Self::new()
    }
}

/// Service one dialog and answer it (`ACP-144`…`ACP-147`, `ACP-150`).
///
/// Port of pi-acp v0.0.33 `session.ts`'s `handleExtensionUiRequest` and the `.catch` wrapped around
/// it. Upstream's structure encodes "except for the missing-`id` case, every path answers exactly
/// once"; here the `id` cannot be missing and the method cannot be unrecognised (`UiKind` has four
/// variants), so what is left is the "exactly once" half, which is why `reply` is moved through
/// this function and consumed at exactly one statement.
///
/// # The one path that answers nothing, and why it is not the silent drop
///
/// `oneshot::Sender::closed` resolves when the guest's receiver is gone — which is precisely the
/// case `LiveHostServices::ui_roundtrip` creates when `DialogOptions.timeout` fires and it stops
/// polling. Racing against it means a timed-out dialog stops waiting on the human immediately
/// instead of holding a task open until the client eventually answers. There is nobody left to
/// answer at that point: `ui_roundtrip` has already returned the deny default to the guest. This is
/// the in-process replacement for `cyrup_modes::rpc`'s `pending.retain(|_, p| !p.reply.is_closed())`
/// (see the module doc).
async fn serve_dialog<C: DialogClient + ?Sized>(
    client: &C,
    session_id: &SessionId,
    caps: DialogCaps,
    seq: u64,
    request: UiRequest,
) {
    let (request, mut reply) = DialogRequest::split(request);
    let kind = request.kind;

    let answer = async {
        match kind {
            UiKind::Select => {
                let options = select_options(&request.options);
                let table = DialogOptionTable::mint(&options);
                if table.is_empty() {
                    // Upstream: an empty option list answers cancelled immediately, with **no**
                    // permission request at all (`handleExtensionSelect`). A permission prompt with
                    // no options is un-answerable, so asking would strand the client too.
                    return deny_default(kind);
                }
                ask_permission(client, session_id, seq, &request, &table).await
            }
            UiKind::Confirm => {
                let table = DialogOptionTable::confirm();
                ask_permission(client, session_id, seq, &request, &table).await
            }
            UiKind::Input | UiKind::Editor => {
                ask_elicitation(client, session_id, caps, &request).await
            }
        }
    };

    let answer = tokio::select! {
        biased;
        // Checked first: if the guest is already gone there is no reason to ask a human anything.
        () = reply.closed() => {
            tracing::debug!(
                ?kind,
                "acp: dialog abandoned by the guest before the client answered; nothing to reply to"
            );
            return;
        }
        answer = answer => answer,
    };

    // The single consumption of the sender. `Err` here means the guest gave up between the race
    // above and this line; there is nothing to do with it and nothing is leaked.
    let _ = reply.send(answer);
}

/// `session/request_permission`, with upstream's catch (`ACP-150`).
///
/// Port of pi-acp v0.0.33 `session.ts`'s `requestExtensionPermission`, whose try/catch is *"the
/// single place that guarantees a dialog cannot strand the extension when the ACP client
/// misbehaves"*.
///
/// # [CYRUP-DELTA] — `Option<Outcome>` becomes an unconditional `UiReply`
///
/// **What differs.** Upstream returns `PermissionResponse | null`, where `null` means "I already
/// answered pi, you must return"; every caller has to remember to check it. Here the reply is
/// computed on both arms and returned by value, so "already answered" is not a sentinel a caller
/// can forget — there is exactly one `reply.send` in [`serve_dialog`] and it is unconditional.
///
/// **What it costs.** Nothing. It is the shape `ACP-150` itself recommends ("make the reply sender
/// un-droppable: pass it by value into a helper that must consume it"), one step further: the
/// sender never enters this function at all, so it cannot be dropped here even by accident.
///
/// Note that a response the schema cannot parse — including an outcome variant this build does not
/// know — surfaces as `Err` and lands here, which is the second of
/// [`DialogOptionTable::choose`]'s two independent denies.
async fn ask_permission<C: DialogClient + ?Sized>(
    client: &C,
    session_id: &SessionId,
    seq: u64,
    request: &DialogRequest,
    table: &DialogOptionTable,
) -> UiReply {
    let payload = RequestPermissionRequest::new(
        session_id.clone(),
        ui_tool_call(seq, request),
        table.advertised().to_vec(),
    );
    match client.request_permission(payload).await {
        Ok(response) => reply_for(request.kind, table, &response.outcome),
        Err(err) => {
            // Upstream swallows the throw entirely; a `debug` line is the cyrup idiom and costs
            // nothing on the wire.
            tracing::debug!(
                error = %err,
                kind = ?request.kind,
                "acp: the client refused session/request_permission; answering the deny default"
            );
            deny_default(request.kind)
        }
    }
}

/// `elicitation/create` for [`UiKind::Input`] and [`UiKind::Editor`] (`ACP-147`).
///
/// Upstream has no counterpart: pi-acp emits a chat chunk and cancels both, because ACP had no
/// text-input carrier when it was written. Schema 1.7.0 has one.
///
/// # [CYRUP-DELTA] — `Editor` is degraded to a single string field
///
/// **What differs.** `UiKind::Editor`'s contract is a prefilled buffer opened in the user's editor.
/// `StringPropertySchema` has no multiline hint at all — `format` is
/// `email|uri|date|date-time|Other` — so the best available carrier is a string property with
/// `default = UiRequest.message` (the prefill) and no length cap.
///
/// **What it costs.** The real editor. The client renders a form field where the guest asked for a
/// buffer, which for a long prefill is a poor experience. It is still strictly better than the
/// alternative on offer — cancelling, which returns `Text(None)` and loses the user's intent
/// entirely — and the guest's contract (`option<string>`) is honoured either way.
///
/// A second, smaller delta: ACP has no *placeholder*. `default` is a **prefill**, which is a
/// different thing (it is submitted if the user types nothing), so `UiRequest.placeholder` is
/// mapped to the property `description` — visible help text — and never to `default`.
async fn ask_elicitation<C: DialogClient + ?Sized>(
    client: &C,
    session_id: &SessionId,
    caps: DialogCaps,
    request: &DialogRequest,
) -> UiReply {
    if !caps.elicitation {
        // Upstream's branch, with the rewritten string: one chat chunk, then cancel. The cancel
        // lands on `Text(None)`, identical to `default_ui_reply(UiKind::Input)`, so the fallback
        // needs no special case.
        client.notify(
            session_id,
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from(
                unsupported_dialog_message(request.kind),
            ))),
        );
        return deny_default(request.kind);
    }

    let mut property = StringPropertySchema::new();
    if let Some(placeholder) = &request.placeholder {
        property = property.description(placeholder.clone());
    }
    if request.kind == UiKind::Editor && !request.message.is_empty() {
        property = property.default_value(request.message.clone());
    }

    let schema = ElicitationSchema::new()
        .title(request.prompt.clone())
        .property(ELICITATION_FIELD, property, true);
    let payload = CreateElicitationRequest::new(
        ElicitationFormMode::new(ElicitationSessionScope::new(session_id.clone()), schema),
        request.prompt.clone(),
    );

    match client.create_elicitation(payload).await {
        Ok(response) => elicited_text(&response.action),
        Err(err) => {
            // `ACP-150`'s rule again: the client refusing is a deny, never an error out of the
            // dialog task.
            tracing::debug!(
                error = %err,
                kind = ?request.kind,
                "acp: the client refused elicitation/create; answering the deny default"
            );
            deny_default(request.kind)
        }
    }
}

/// The single property name the elicitation form carries. It is never shown to the user — the
/// prompt is the schema's `title` and the placeholder its `description` — so it is a stable
/// internal key, not copy.
const ELICITATION_FIELD: &str = "value";

/// Read the user's string back out of an elicitation response.
///
/// `ElicitationAction` is `#[non_exhaustive]` and its `Decline`/`Cancel`/`Other` arms all mean "no
/// text", so this is written as an `if let` for the same reason [`DialogOptionTable::choose`] is:
/// the no-text answer is what falls out when the accept path is not taken, rather than a wildcard
/// arm a later edit could widen. A non-string content value (the schema also permits numbers,
/// booleans and arrays) is likewise no text — the requested schema asked for a string, so anything
/// else is a client that answered a question we did not ask.
fn elicited_text(action: &ElicitationAction) -> UiReply {
    if let ElicitationAction::Accept(accepted) = action
        && let Some(content) = &accepted.content
        && let Some(ElicitationContentValue::String(text)) = content.get(ELICITATION_FIELD)
    {
        return UiReply::Text(Some(text.clone()));
    }
    UiReply::Text(None)
}

/// Project `UiRequest.options` onto the option strings a select offers.
///
/// Port of upstream's `Array.isArray(rawOptions) ? rawOptions.map(option => String(option)) : []`.
///
/// # [CYRUP-DELTA] — a non-string option renders as JSON, not as `[object Object]`
///
/// **What differs.** JavaScript's `String(x)` gives `"[object Object]"` for an object and
/// `"1,2"` for an array; `serde_json`'s `to_string` gives the JSON text.
///
/// **What it costs.** Nothing reachable: the WIT `select` takes `list<string>`, so a guest cannot
/// send a non-string option through `cyrup-ext` at all, and a native extension building the bag by
/// hand gets a legible string instead of a useless one. The arm exists because `UiRequest.options`
/// is a `serde_json::Value` and total handling is cheaper than a partial one.
fn select_options(options: &Value) -> Vec<String> {
    let Some(array) = options.as_array() else {
        return Vec::new();
    };
    array
        .iter()
        .map(|option| match option {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        })
        .collect()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{ElicitationAcceptAction, SelectedPermissionOutcome};
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use tokio::sync::oneshot;

    /// A scripted [`DialogClient`]. Records every request it was sent and every notification, and
    /// answers from a queue of canned outcomes.
    struct FakeClient {
        permission: Mutex<Vec<Result<RequestPermissionResponse, agent_client_protocol::Error>>>,
        elicitation: Mutex<Vec<Result<CreateElicitationResponse, agent_client_protocol::Error>>>,
        asked: Mutex<Vec<RequestPermissionRequest>>,
        elicited: Mutex<Vec<CreateElicitationRequest>>,
        notified: Mutex<Vec<SessionUpdate>>,
    }

    impl FakeClient {
        fn new() -> Self {
            Self {
                permission: Mutex::new(Vec::new()),
                elicitation: Mutex::new(Vec::new()),
                asked: Mutex::new(Vec::new()),
                elicited: Mutex::new(Vec::new()),
                notified: Mutex::new(Vec::new()),
            }
        }

        fn answering(outcome: RequestPermissionOutcome) -> Arc<Self> {
            let client = Self::new();
            client
                .permission
                .lock()
                .unwrap()
                .push(Ok(RequestPermissionResponse::new(outcome)));
            Arc::new(client)
        }

        fn refusing() -> Arc<Self> {
            let client = Self::new();
            client
                .permission
                .lock()
                .unwrap()
                .push(Err(agent_client_protocol::Error::internal_error()));
            client
                .elicitation
                .lock()
                .unwrap()
                .push(Err(agent_client_protocol::Error::internal_error()));
            Arc::new(client)
        }

        fn asked(&self) -> Vec<RequestPermissionRequest> {
            self.asked.lock().unwrap().clone()
        }

        fn notified(&self) -> Vec<SessionUpdate> {
            self.notified.lock().unwrap().clone()
        }
    }

    impl TurnSink for FakeClient {
        fn notify(&self, _session_id: &SessionId, update: SessionUpdate) {
            self.notified.lock().unwrap().push(update);
        }
    }

    impl DialogClient for FakeClient {
        fn request_permission(
            &self,
            request: RequestPermissionRequest,
        ) -> BoxFuture<'static, Result<RequestPermissionResponse, agent_client_protocol::Error>>
        {
            self.asked.lock().unwrap().push(request);
            let answer = self
                .permission
                .lock()
                .unwrap()
                .pop()
                .unwrap_or_else(|| Err(agent_client_protocol::Error::internal_error()));
            Box::pin(async move { answer })
        }

        fn create_elicitation(
            &self,
            request: CreateElicitationRequest,
        ) -> BoxFuture<'static, Result<CreateElicitationResponse, agent_client_protocol::Error>>
        {
            self.elicited.lock().unwrap().push(request);
            let answer = self
                .elicitation
                .lock()
                .unwrap()
                .pop()
                .unwrap_or_else(|| Err(agent_client_protocol::Error::internal_error()));
            Box::pin(async move { answer })
        }
    }

    fn request(
        kind: UiKind,
        prompt: &str,
        options: Value,
    ) -> (UiRequest, oneshot::Receiver<UiReply>) {
        let (reply, rx) = oneshot::channel();
        (
            UiRequest {
                kind,
                prompt: prompt.to_string(),
                options,
                message: String::new(),
                placeholder: None,
                opts: Default::default(),
                reply,
            },
            rx,
        )
    }

    fn permission_options() -> Vec<String> {
        PERMISSION_DIALOG_OPTIONS
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    }

    // -----------------------------------------------------------------------------------------
    // ACP-146 — the deny default and the two fixed options
    // -----------------------------------------------------------------------------------------

    /// ACP-146 — the deny default is fail-closed for all four kinds and is bit-identical to
    /// `cyrup_modes::rpc`'s `default_ui_reply`.
    #[test]
    fn the_deny_default_is_fail_closed_for_every_kind() {
        assert_eq!(deny_default(UiKind::Confirm), UiReply::Confirm(false));
        assert_eq!(deny_default(UiKind::Input), UiReply::Text(None));
        assert_eq!(deny_default(UiKind::Select), UiReply::Text(None));
        assert_eq!(deny_default(UiKind::Editor), UiReply::Text(None));
    }

    /// ACP-146 — `CONFIRM_PERMISSION_OPTIONS` is byte-for-byte upstream's: two options, this
    /// order, these ids, these names, these kinds, and nothing else.
    #[test]
    fn the_confirm_options_are_upstreams_two_in_upstreams_order() {
        let table = DialogOptionTable::confirm();
        let advertised = table.advertised();
        assert_eq!(advertised.len(), 2, "exactly two options, never a third");
        assert_eq!(advertised[0].option_id, PermissionOptionId::new("yes"));
        assert_eq!(advertised[0].name, "Yes");
        assert_eq!(advertised[0].kind, PermissionOptionKind::AllowOnce);
        assert_eq!(advertised[1].option_id, PermissionOptionId::new("no"));
        assert_eq!(advertised[1].name, "No");
        assert_eq!(advertised[1].kind, PermissionOptionKind::RejectOnce);
        assert!(!table.is_empty());
    }

    /// ACP-146 — the cancelled branch, and the `#[non_exhaustive]` trap.
    ///
    /// The trap cannot be sprung with a synthesised variant: `RequestPermissionOutcome` is
    /// `#[non_exhaustive]` in the schema crate, so no downstream code — this test included — can
    /// construct a variant beyond the two it publishes. What *is* assertable, and is what actually
    /// protects the seam, is both of its doors:
    ///
    /// 1. `choose` reaches the option lookup only from an `if let Selected`, so no other outcome
    ///    — present or future — can reach the selected-arm logic. `Cancelled` denies here.
    /// 2. An outcome tag this build does not know **fails to deserialize**, so it can never arrive
    ///    as a `RequestPermissionOutcome` at all; it arrives at `ask_permission` as `Err` and is
    ///    denied there (`a_client_that_refuses_the_dialog_still_answers_the_guest`).
    #[test]
    fn a_cancelled_or_unknown_outcome_can_never_confirm() {
        let table = DialogOptionTable::confirm();
        assert_eq!(
            reply_for(
                UiKind::Confirm,
                &table,
                &RequestPermissionOutcome::Cancelled
            ),
            UiReply::Confirm(false),
            "a dismissed confirmation dialog is a denial, never an approval"
        );

        // Door 2: the schema itself refuses an outcome it does not know, so the wildcard case
        // cannot even be represented — it is routed to `ask_permission`'s `Err` arm instead.
        let unknown = serde_json::from_value::<RequestPermissionResponse>(json!({
            "outcome": "granted_forever",
            "optionId": "yes"
        }));
        assert!(
            unknown.is_err(),
            "an unknown outcome must not deserialize into anything, least of all Selected"
        );
    }

    /// ACP-146 — the `yes` id is compared against a name this dialog minted, so a client that
    /// echoes an id from some other dialog cannot confirm.
    #[test]
    fn only_this_dialogs_yes_confirms() {
        let table = DialogOptionTable::confirm();
        let yes = RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new("yes"));
        let no = RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new("no"));
        let foreign =
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new("choice-0"));
        assert_eq!(
            reply_for(UiKind::Confirm, &table, &yes),
            UiReply::Confirm(true)
        );
        assert_eq!(
            reply_for(UiKind::Confirm, &table, &no),
            UiReply::Confirm(false)
        );
        assert_eq!(
            reply_for(UiKind::Confirm, &table, &foreign),
            UiReply::Confirm(false),
            "an id from another dialog's table must not confirm"
        );
    }

    // -----------------------------------------------------------------------------------------
    // ACP-145 — the select round trip
    // -----------------------------------------------------------------------------------------

    /// ACP-145 — **the permission-bypass canary.** A fabricated option id has no path to a choice,
    /// because the only lookup is the table that minted the ids for THIS dialog. If this ever
    /// passes with a `Some`, an approve string the user did not pick has reached
    /// `LocalAskChannel::confirm`'s exact-string match and a `Once`/`Always` grant has been
    /// fabricated.
    #[test]
    fn a_fabricated_option_id_can_never_produce_a_choice() {
        let dialog = DialogOptionTable::mint(&["Allow Once".into(), "Reject".into()]);
        for fabricated in [
            "choice-2",                // past the end
            "choice-01", // upstream's `String(index) === rawIndex` rejection, now unrepresentable
            "choice-1.0", // ditto
            "choice-",   // empty index
            "choice--1", // negative
            "yes",       // the confirm dialog's id, in a select
            "",          // empty
            "choice-9007199254740993", // past `Number.isSafeInteger`
        ] {
            let outcome =
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(fabricated));
            assert!(
                dialog.choose(&outcome).is_none(),
                "an id this dialog did not mint must not resolve: {fabricated}"
            );
            assert_eq!(
                reply_for(UiKind::Select, &dialog, &outcome),
                UiReply::Text(None),
                "and it must land on the deny default: {fabricated}"
            );
        }
        assert!(
            dialog
                .choose(&RequestPermissionOutcome::Cancelled)
                .is_none()
        );
    }

    /// ACP-145 — the selected option's **string** comes back, at every index, and it is the string
    /// the guest sent rather than anything reconstructed. This is the assertion that would fail on
    /// an off-by-one.
    #[test]
    fn every_option_round_trips_to_its_own_string() {
        let options = permission_options();
        let table = DialogOptionTable::mint(&options);
        for (index, expected) in options.iter().enumerate() {
            let outcome = RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                format!("choice-{index}"),
            ));
            assert_eq!(
                reply_for(UiKind::Select, &table, &outcome),
                UiReply::Text(Some(expected.clone())),
                "choice-{index} must return option {index}"
            );
        }
    }

    /// ACP-145 — the option ids are upstream's `choice-<index>`, in mint order, and the advertised
    /// list is the option list.
    #[test]
    fn the_minted_ids_are_upstreams_and_in_order() {
        let table = DialogOptionTable::mint(&["a".into(), "b".into(), "c".into()]);
        let ids: Vec<_> = table
            .advertised()
            .iter()
            .map(|o| o.option_id.to_string())
            .collect();
        assert_eq!(ids, vec!["choice-0", "choice-1", "choice-2"]);
        let names: Vec<_> = table.advertised().iter().map(|o| o.name.clone()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    /// ACP-145 — the four strings this module matches on ARE the four `LocalAskChannel::confirm`
    /// puts in its `select`.
    ///
    /// Without this, the coupling is a cross-crate string literal with no compiler and no test
    /// behind it, which is the class of thing this unit is rated critical for. A drift cannot
    /// fabricate a grant — the reply is the guest's own option string, carried through
    /// [`DialogOptionTable`] unmodified — but it silently degrades every permission prompt in Zed
    /// to an undifferentiated four-way `allow_once` menu, with nothing anywhere saying why.
    #[test]
    fn the_permission_dialog_list_is_the_permission_systems_own() {
        assert_eq!(
            PERMISSION_DIALOG_OPTIONS,
            cyrup_permission_system::PERMISSION_DIALOG_OPTIONS,
            "ACP-145: `permission.rs`'s matcher and `ask.rs`'s dialog must be the same four \
             strings in the same order"
        );
    }

    /// ACP-145's decision — the permission dialog is advertised faithfully, and **only** when the
    /// whole option list is the permission dialog's. An extension menu that merely contains
    /// `"Reject"` keeps upstream's blanket `allow_once`.
    #[test]
    fn the_kind_mapping_is_faithful_for_the_permission_dialog_and_blanket_for_everything_else() {
        let table = DialogOptionTable::mint(&permission_options());
        let kinds: Vec<_> = table.advertised().iter().map(|o| o.kind).collect();
        assert_eq!(
            kinds,
            vec![
                PermissionOptionKind::AllowOnce,
                PermissionOptionKind::AllowAlways,
                PermissionOptionKind::RejectOnce,
                PermissionOptionKind::RejectOnce,
            ],
            "the four LocalAskChannel strings map onto the four ACP kinds"
        );

        // One string short of the permission dialog, and a menu that happens to share a label.
        for menu in [
            vec!["Allow Once".to_string(), "Reject".to_string()],
            vec![
                "Reject".to_string(),
                "Allow Once".to_string(),
                "Allow Always".to_string(),
                "Reject with Reason".to_string(),
            ],
            vec!["Pick a branch".to_string(), "Reject".to_string()],
        ] {
            let table = DialogOptionTable::mint(&menu);
            assert!(
                table
                    .advertised()
                    .iter()
                    .all(|o| o.kind == PermissionOptionKind::AllowOnce),
                "a non-permission select must not be dressed with permission affordances: {menu:?}"
            );
        }
    }

    /// ACP-145 — an empty select answers the deny default and sends **no** permission request.
    #[tokio::test]
    async fn an_empty_select_never_reaches_the_client() {
        let client = FakeClient::answering(RequestPermissionOutcome::Selected(
            SelectedPermissionOutcome::new("choice-0"),
        ));
        let (req, rx) = request(UiKind::Select, "pick", json!([]));
        serve_dialog(
            &*client,
            &SessionId::new("s1"),
            DialogCaps::default(),
            0,
            req,
        )
        .await;
        assert_eq!(rx.await.unwrap(), UiReply::Text(None));
        assert!(
            client.asked().is_empty(),
            "an option-less permission prompt is un-answerable; upstream does not send it either"
        );
    }

    /// ACP-145 end to end: three options in, three `PermissionOption`s out, the second selected,
    /// the second **string** back.
    #[tokio::test]
    async fn selecting_the_second_option_returns_the_second_string() {
        let client = FakeClient::answering(RequestPermissionOutcome::Selected(
            SelectedPermissionOutcome::new("choice-1"),
        ));
        let (req, rx) = request(
            UiKind::Select,
            "Which branch?",
            json!(["main", "release", "wip"]),
        );
        serve_dialog(
            &*client,
            &SessionId::new("s1"),
            DialogCaps::default(),
            7,
            req,
        )
        .await;
        assert_eq!(
            rx.await.unwrap(),
            UiReply::Text(Some("release".to_string()))
        );

        let asked = client.asked();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].options.len(), 3);
        assert_eq!(asked[0].session_id, SessionId::new("s1"));
    }

    // -----------------------------------------------------------------------------------------
    // ACP-144 / ACP-150 — every path answers
    // -----------------------------------------------------------------------------------------

    /// ACP-150 — a client returning a JSON-RPC error for `session/request_permission` still
    /// answers the guest, promptly, with the deny default. This is also the second door on
    /// ACP-146's `#[non_exhaustive]` outcome: an outcome the schema cannot parse arrives here.
    #[tokio::test]
    async fn a_client_that_refuses_the_dialog_still_answers_the_guest() {
        let client = FakeClient::refusing();
        let (req, rx) = request(UiKind::Confirm, "Run `rm -rf /`?", Value::Null);
        serve_dialog(
            &*client,
            &SessionId::new("s1"),
            DialogCaps::default(),
            0,
            req,
        )
        .await;
        assert_eq!(rx.await.unwrap(), UiReply::Confirm(false));
    }

    /// ACP-150 — the same for the elicitation carrier, which shares the rule.
    #[tokio::test]
    async fn a_client_that_refuses_an_elicitation_still_answers_the_guest() {
        let client = FakeClient::refusing();
        let (req, rx) = request(UiKind::Input, "Your name?", Value::Null);
        serve_dialog(
            &*client,
            &SessionId::new("s1"),
            DialogCaps { elicitation: true },
            0,
            req,
        )
        .await;
        assert_eq!(rx.await.unwrap(), UiReply::Text(None));
    }

    /// ACP-144 — the bridge answers rather than drops. A dropped `oneshot` parks a runtime worker
    /// thread inside `ui_roundtrip`'s `block_in_place` and the wasm guest forever.
    #[tokio::test]
    async fn every_dialog_is_answered_and_the_bridge_shuts_down_cleanly() {
        let client = FakeClient::new();
        // No canned answers: the fake falls back to an error, which is ACP-150's path.
        let client = Arc::new(client);
        let bridge = PermissionBridge::new();
        let sink = bridge.sink();
        let effects = bridge.effect_sink();
        let task = tokio::spawn(bridge.run(SessionId::new("s1"), Arc::clone(&client)));

        let (req, rx) = request(UiKind::Confirm, "Run `rm -rf /`?", Value::Null);
        sink.send(req).expect("the bridge is draining");
        assert_eq!(rx.await.expect("answered"), UiReply::Confirm(false));

        drop(sink);
        drop(effects);
        task.await.expect("joined");
    }

    /// ACP-155 — a dialog nobody answers must not stop the next one being serviced. The first
    /// dialog's client call never completes; the second still round-trips.
    #[tokio::test]
    async fn a_parked_dialog_does_not_block_the_next_one() {
        /// A client whose permission call never resolves.
        struct Parking;
        impl TurnSink for Parking {
            fn notify(&self, _session_id: &SessionId, _update: SessionUpdate) {}
        }
        impl DialogClient for Parking {
            fn request_permission(
                &self,
                _request: RequestPermissionRequest,
            ) -> BoxFuture<'static, Result<RequestPermissionResponse, agent_client_protocol::Error>>
            {
                Box::pin(std::future::pending())
            }
            fn create_elicitation(
                &self,
                _request: CreateElicitationRequest,
            ) -> BoxFuture<'static, Result<CreateElicitationResponse, agent_client_protocol::Error>>
            {
                Box::pin(std::future::pending())
            }
        }

        let bridge = PermissionBridge::new();
        let sink = bridge.sink();
        let effects = bridge.effect_sink();
        let task = tokio::spawn(bridge.run(SessionId::new("s1"), Arc::new(Parking)));

        let (first, first_rx) = request(UiKind::Confirm, "one", Value::Null);
        sink.send(first).unwrap();
        // A dialog with no options never reaches the client at all, so it answers even while the
        // first is parked — which is the property under test: the drain is still running.
        let (second, second_rx) = request(UiKind::Select, "two", json!([]));
        sink.send(second).unwrap();
        let answered = tokio::time::timeout(std::time::Duration::from_secs(5), second_rx)
            .await
            .expect("a parked dialog must not stall the drain")
            .unwrap();
        assert_eq!(answered, UiReply::Text(None));

        // The parked dialog is abandoned by the guest: the bridge notices and stops waiting.
        drop(first_rx);
        drop(sink);
        drop(effects);
        task.await.expect("joined");
    }

    /// ACP-144's `cyrup_modes::rpc` pruning rule, in its in-process form: a guest that has already
    /// given up (`ui_roundtrip`'s `DialogOptions.timeout` fired and dropped the receiver) stops the
    /// dialog immediately rather than leaving a task parked on the human.
    #[tokio::test]
    async fn an_abandoned_dialog_stops_waiting_on_the_client() {
        struct Parking;
        impl TurnSink for Parking {
            fn notify(&self, _session_id: &SessionId, _update: SessionUpdate) {}
        }
        impl DialogClient for Parking {
            fn request_permission(
                &self,
                _request: RequestPermissionRequest,
            ) -> BoxFuture<'static, Result<RequestPermissionResponse, agent_client_protocol::Error>>
            {
                Box::pin(std::future::pending())
            }
            fn create_elicitation(
                &self,
                _request: CreateElicitationRequest,
            ) -> BoxFuture<'static, Result<CreateElicitationResponse, agent_client_protocol::Error>>
            {
                Box::pin(std::future::pending())
            }
        }

        let (req, rx) = request(UiKind::Confirm, "one", Value::Null);
        drop(rx);
        // Without the `closed()` arm this hangs forever on `std::future::pending`; the timeout
        // turns that regression into a failure rather than a wedged test run.
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            serve_dialog(
                &Parking,
                &SessionId::new("s1"),
                DialogCaps::default(),
                0,
                req,
            ),
        )
        .await
        .expect("an abandoned dialog must not keep waiting on the client");
    }

    // -----------------------------------------------------------------------------------------
    // ACP-147 — input and editor
    // -----------------------------------------------------------------------------------------

    /// ACP-147 — with no `elicitation` capability, both kinds cancel and emit **exactly one** chat
    /// chunk, and that chunk does not name another product.
    #[tokio::test]
    async fn without_elicitation_input_and_editor_cancel_with_one_chunk() {
        for kind in [UiKind::Input, UiKind::Editor] {
            let client = Arc::new(FakeClient::new());
            let (req, rx) = request(kind, "Say something", Value::Null);
            serve_dialog(
                &*client,
                &SessionId::new("s1"),
                DialogCaps::default(),
                0,
                req,
            )
            .await;
            assert_eq!(rx.await.unwrap(), UiReply::Text(None));

            let notified = client.notified();
            assert_eq!(notified.len(), 1, "exactly one chunk for {kind:?}");
            let SessionUpdate::AgentMessageChunk(chunk) = &notified[0] else {
                panic!("expected an agent_message_chunk, got {:?}", notified[0]);
            };
            let ContentBlock::Text(text) = &chunk.content else {
                panic!("expected text content");
            };
            assert_eq!(
                text.text,
                format!(
                    "This client cannot display an extension {} dialog; cancelling it.",
                    dialog_method(kind)
                )
            );
            assert!(
                !text.text.contains("Pi "),
                "no other product's name in a cyrup transcript"
            );
        }
    }

    /// ACP-147 — with `elicitation`, `ui.input` produces an `elicitation/create` whose returned
    /// string reaches the guest, and no fallback chunk is emitted.
    #[tokio::test]
    async fn an_elicitation_answer_reaches_the_guest() {
        let client = Arc::new(FakeClient::new());
        let mut content = BTreeMap::new();
        content.insert(
            ELICITATION_FIELD.to_string(),
            ElicitationContentValue::String("Ada".to_string()),
        );
        client
            .elicitation
            .lock()
            .unwrap()
            .push(Ok(CreateElicitationResponse::new(
                ElicitationAcceptAction::new().content(content),
            )));

        let (mut req, rx) = request(UiKind::Input, "Your name?", Value::Null);
        req.placeholder = Some("e.g. Ada".to_string());
        serve_dialog(
            &*client,
            &SessionId::new("s1"),
            DialogCaps { elicitation: true },
            0,
            req,
        )
        .await;
        assert_eq!(rx.await.unwrap(), UiReply::Text(Some("Ada".to_string())));
        assert!(
            client.notified().is_empty(),
            "a dialog that was really answered emits no 'cannot display' chunk"
        );

        let elicited = client.elicited.lock().unwrap();
        assert_eq!(elicited.len(), 1);
        // The placeholder is the property `description`, never the `default` — `default` is a
        // prefill, which is a different thing.
        let wire = serde_json::to_value(&elicited[0]).unwrap();
        let property = &wire["requestedSchema"]["properties"][ELICITATION_FIELD];
        assert_eq!(property["description"], json!("e.g. Ada"));
        assert!(property.get("default").is_none());
        assert_eq!(wire["requestedSchema"]["title"], json!("Your name?"));
    }

    /// ACP-147 — an `editor` dialog's prefill becomes the property `default`, which is the
    /// documented degradation.
    #[tokio::test]
    async fn an_editor_prefill_becomes_the_elicitation_default() {
        let client = Arc::new(FakeClient::new());
        client
            .elicitation
            .lock()
            .unwrap()
            .push(Ok(CreateElicitationResponse::new(
                ElicitationAction::Cancel,
            )));

        let (mut req, rx) = request(UiKind::Editor, "Edit the message", Value::Null);
        req.message = "seed text".to_string();
        serve_dialog(
            &*client,
            &SessionId::new("s1"),
            DialogCaps { elicitation: true },
            0,
            req,
        )
        .await;
        assert_eq!(
            rx.await.unwrap(),
            UiReply::Text(None),
            "a cancelled elicitation is the deny default"
        );
        let elicited = client.elicited.lock().unwrap();
        let wire = serde_json::to_value(&elicited[0]).unwrap();
        assert_eq!(
            wire["requestedSchema"]["properties"][ELICITATION_FIELD]["default"],
            json!("seed text")
        );
    }

    /// ACP-147 — every non-accept action, and a wrongly typed accept, are "no text". Written
    /// against `ElicitationAction`'s `#[non_exhaustive]` shape the same way the outcome match is.
    #[test]
    fn only_an_accepted_string_becomes_text() {
        assert_eq!(
            elicited_text(&ElicitationAction::Decline),
            UiReply::Text(None)
        );
        assert_eq!(
            elicited_text(&ElicitationAction::Cancel),
            UiReply::Text(None)
        );
        assert_eq!(
            elicited_text(&ElicitationAction::Accept(ElicitationAcceptAction::new())),
            UiReply::Text(None),
            "an accept with no content is no text"
        );

        let mut wrong_field = BTreeMap::new();
        wrong_field.insert(
            "other".to_string(),
            ElicitationContentValue::String("x".to_string()),
        );
        assert_eq!(
            elicited_text(&ElicitationAction::Accept(
                ElicitationAcceptAction::new().content(wrong_field)
            )),
            UiReply::Text(None)
        );

        let mut wrong_type = BTreeMap::new();
        wrong_type.insert(
            ELICITATION_FIELD.to_string(),
            ElicitationContentValue::Boolean(true),
        );
        assert_eq!(
            elicited_text(&ElicitationAction::Accept(
                ElicitationAcceptAction::new().content(wrong_type)
            )),
            UiReply::Text(None),
            "the schema asked for a string; anything else answers a question we did not ask"
        );
    }

    // -----------------------------------------------------------------------------------------
    // ACP-148 — notify
    // -----------------------------------------------------------------------------------------

    /// ACP-148 — a `Notify` becomes one chunk with the severity in `_meta`; every other effect
    /// becomes nothing at all.
    #[test]
    fn notify_becomes_one_chunk_with_a_level_and_nothing_else_does() {
        for (kind, level) in [
            (NotifyKind::Info, "info"),
            (NotifyKind::Warning, "warning"),
            (NotifyKind::Error, "error"),
        ] {
            let update = notify_chunk(&UiEffect::Notify {
                message: "disk is full".to_string(),
                kind,
            })
            .expect("a notify always renders");
            let wire = serde_json::to_value(&update).unwrap();
            assert_eq!(wire["sessionUpdate"], json!("agent_message_chunk"));
            assert_eq!(wire["content"]["text"], json!("disk is full"));
            assert_eq!(wire["_meta"]["cyrupAcp"]["notify"]["level"], json!(level));
        }

        assert!(
            notify_chunk(&UiEffect::SetStatus {
                key: "k".to_string(),
                text: Some("busy".to_string()),
            })
            .is_none(),
            "a status effect produces no notification at all"
        );
        assert!(
            notify_chunk(&UiEffect::SetTitle {
                title: "t".to_string()
            })
            .is_none()
        );
    }

    /// ACP-148 — the effect channel is drained by the same task, and its updates reach the client.
    #[tokio::test]
    async fn the_bridge_drains_effects_too() {
        let client = Arc::new(FakeClient::new());
        let bridge = PermissionBridge::new();
        let sink = bridge.sink();
        let effects = bridge.effect_sink();
        let task = tokio::spawn(bridge.run(SessionId::new("s1"), Arc::clone(&client)));

        effects
            .send(UiEffect::Notify {
                message: "heads up".to_string(),
                kind: NotifyKind::Warning,
            })
            .unwrap();
        effects
            .send(UiEffect::SetTitle {
                title: "ignored".to_string(),
            })
            .unwrap();
        drop(sink);
        drop(effects);
        task.await.expect("joined");

        let notified = client.notified();
        assert_eq!(notified.len(), 1, "only the notify produced an update");
    }

    // -----------------------------------------------------------------------------------------
    // ACP-149 — the synthetic tool call
    // -----------------------------------------------------------------------------------------

    /// ACP-149 — the synthesised `rawInput` carries exactly the fields the `UiRequest` has and no
    /// others: no reply-channel state, no `DialogOptions` bag, and no key belonging to a different
    /// kind.
    #[test]
    fn the_synthetic_tool_call_carries_only_the_requests_own_fields() {
        let select = DialogRequest {
            kind: UiKind::Select,
            prompt: "Which branch?".to_string(),
            options: json!(["main", "wip"]),
            message: String::new(),
            placeholder: None,
        };
        let call = ui_tool_call(3, &select);
        let raw = serde_json::to_value(&call).unwrap();
        assert_eq!(raw["toolCallId"], json!("cyrup-ui-3"));
        assert_eq!(raw["kind"], json!("other"));
        assert_eq!(raw["status"], json!("pending"));
        assert_eq!(raw["title"], json!("Which branch?"));
        let input = raw["rawInput"].as_object().unwrap();
        let mut keys: Vec<_> = input.keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, vec!["method", "options", "title"]);
        assert_eq!(input["method"], json!("select"));

        let confirm = DialogRequest {
            kind: UiKind::Confirm,
            prompt: "Delete?".to_string(),
            options: Value::Null,
            message: "This cannot be undone.".to_string(),
            placeholder: None,
        };
        let input = serde_json::to_value(ui_tool_call(0, &confirm)).unwrap()["rawInput"].clone();
        let input = input.as_object().unwrap();
        let mut keys: Vec<_> = input.keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, vec!["message", "method", "title"]);

        let editor = DialogRequest {
            kind: UiKind::Editor,
            prompt: "Edit".to_string(),
            options: Value::Null,
            message: "seed".to_string(),
            placeholder: None,
        };
        let input = serde_json::to_value(ui_tool_call(0, &editor)).unwrap()["rawInput"].clone();
        let input = input.as_object().unwrap();
        let mut keys: Vec<_> = input.keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec!["method", "prefill", "title"],
            "editor sends prefill, never message"
        );

        let bare_input = DialogRequest {
            kind: UiKind::Input,
            prompt: String::new(),
            options: Value::Null,
            message: String::new(),
            placeholder: None,
        };
        let call = serde_json::to_value(ui_tool_call(0, &bare_input)).unwrap();
        assert_eq!(
            call["title"],
            json!("Extension input"),
            "the fallback title names no other product"
        );
        let input = call["rawInput"].as_object().unwrap();
        assert_eq!(
            input.keys().cloned().collect::<Vec<_>>(),
            vec!["method"],
            "a request with no fields synthesises no fields"
        );
    }

    /// ACP-149 — the tool call ids a bridge mints are distinct, so two live dialogs cannot collide
    /// in the client's tool-call view.
    #[tokio::test]
    async fn two_dialogs_get_two_tool_call_ids() {
        let client = Arc::new(FakeClient::new());
        for seq in 0..2u64 {
            let (req, rx) = request(UiKind::Confirm, "?", Value::Null);
            serve_dialog(
                &*client,
                &SessionId::new("s1"),
                DialogCaps::default(),
                seq,
                req,
            )
            .await;
            let _ = rx.await;
        }
        let asked = client.asked();
        assert_eq!(asked.len(), 2);
        assert_ne!(
            asked[0].tool_call.tool_call_id,
            asked[1].tool_call.tool_call_id
        );
    }

    /// A non-string option is legible rather than `[object Object]`, and a non-array `options`
    /// yields no options at all (upstream's `Array.isArray` guard).
    #[test]
    fn select_options_are_total() {
        assert_eq!(select_options(&Value::Null), Vec::<String>::new());
        assert_eq!(select_options(&json!("nope")), Vec::<String>::new());
        assert_eq!(
            select_options(&json!(["a", 1, true, {"k": "v"}])),
            vec!["a", "1", "true", "{\"k\":\"v\"}"]
        );
    }

    /// `DialogCaps` reads what `initialize` recorded, and its default denies.
    #[test]
    fn the_caps_default_is_no_elicitation() {
        assert!(!DialogCaps::default().elicitation);
        let mut view = ClientView::default();
        assert!(!DialogCaps::from_client(&view).elicitation);
        view.elicitation = true;
        assert!(DialogCaps::from_client(&view).elicitation);
    }
}
