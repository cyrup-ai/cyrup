//! Mirrors `tmp/herdr/src/api/schema/response.rs` — the two answer envelopes and the result union.

use serde::Deserialize;

use super::agents::{AgentInfo, AgentView};
use super::panes::{PaneInfo, PaneProcessInfo, PaneReadResult};
use super::server::ServerCapabilities;
use super::session::SessionSnapshot;
use super::tabs::TabInfo;
use super::workspaces::WorkspaceInfo;
use crate::error::{HerdrError, Result};

/// `SuccessResponse` (`tmp/herdr/src/api/schema/response.rs:24-28`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SuccessResponse {
    /// The `id` from the request, echoed.
    pub id: String,
    /// The typed result.
    pub result: ResponseResult,
}

/// `ErrorResponse` (`tmp/herdr/src/api/schema/response.rs:30-33`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ErrorResponse {
    /// The `id` from the request, echoed — **or empty**, when herdr could not recover one because
    /// the line did not deserialise (`tmp/herdr/src/api/server.rs:180-201`). An uncorrelated
    /// `{"id":"","error":{"code":"invalid_request",…}}` is still an answer and still ends the call;
    /// waiting for a correlated one would hang to the deadline.
    pub id: String,
    /// herdr's code and message.
    pub error: ErrorBody,
}

/// `ErrorBody` (`tmp/herdr/src/api/schema/response.rs:35-39`).
///
/// `code` is a bare `String` here, as it is in herdr — it is interpreted by
/// [`crate::ApiErrorCode::from_wire`] after decoding, never modelled as a closed enum on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ErrorBody {
    /// herdr's error code, verbatim.
    pub code: String,
    /// herdr's error message, verbatim.
    pub message: String,
}

/// `ResponseResult` (`tmp/herdr/src/api/schema/response.rs:41-44`), internally tagged as
/// `#[serde(tag = "type", rename_all = "snake_case")]`.
///
/// herdr declares 65 variants. This mirror carries the ones cyrup decodes, in herdr's own
/// declaration order, plus [`Self::Unrecognised`].
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ResponseResult {
    /// The answer to `ping` (`tmp/herdr/src/api/schema/response.rs:45-50`, produced at
    /// `tmp/herdr/src/api/server.rs:356-363`).
    ///
    /// `version` and `protocol` are **required**. The docs' `{"id":"req_1","result":{"type":"pong"}}`
    /// (`socket-api.mdx:670`) is abridged; a real server always sends all three keys.
    Pong {
        /// herdr's own version string, e.g. `"0.9.1"`.
        version: String,
        /// The **binary** client-shell protocol generation (`PROTOCOL_VERSION`,
        /// `tmp/herdr/src/protocol/wire.rs:20`), not a JSON-method generation.
        protocol: u32,
        /// Absent on servers that predate the block.
        #[serde(default)]
        capabilities: Option<ServerCapabilities>,
    },
    /// The answer to `session.snapshot` (`response.rs:51-53`).
    ///
    /// `Box`ed in herdr too, and for the same reason: it is by far the largest variant, and an
    /// unboxed one would set the size of every `ResponseResult` on the stack.
    SessionSnapshot {
        /// The whole session.
        snapshot: Box<SessionSnapshot>,
    },
    /// The answer to `workspace.create` (`response.rs:57-61`).
    ///
    /// Each record `Box`ed for the same reason `SessionSnapshot` is: three unboxed records would
    /// set the size of every `ResponseResult` on the stack.
    WorkspaceCreated {
        /// The new workspace.
        workspace: Box<WorkspaceInfo>,
        /// Its first tab.
        tab: Box<TabInfo>,
        /// That tab's root pane — the pane a caller launches into.
        root_pane: Box<PaneInfo>,
    },
    /// The answer to `tab.get` **and** `tab.rename` (`response.rs:87-89`).
    TabInfo {
        /// The tab.
        tab: TabInfo,
    },
    /// The answer to `tab.create` (`response.rs:90-93`).
    TabCreated {
        /// The new tab (boxed, as `WorkspaceCreated`'s records are).
        tab: Box<TabInfo>,
        /// Its root pane.
        root_pane: Box<PaneInfo>,
    },
    /// The answer to `agent.get` (`response.rs:97-99`).
    AgentInfo {
        /// The agent.
        agent: AgentInfo,
    },
    /// The answer to `agent.start` (`response.rs:100-103`).
    AgentStarted {
        /// The agent, as herdr now records it.
        agent: AgentInfo,
        /// The argv herdr typed into the pane — `[<kind executable>, …args]`.
        argv: Vec<String>,
    },
    /// The answer to `agent.prompt` (`response.rs:104-106`).
    AgentPrompted {
        /// The agent after the prompt (after the wait, when one was asked for).
        agent: AgentInfo,
    },
    /// The answer to `agent.list` (`response.rs:107-109`).
    AgentList {
        /// Every agent herdr can see.
        agents: Vec<AgentInfo>,
    },
    /// The answer to `agent.view.set` **and** `agent.view.clear` (`response.rs:110-116`).
    ///
    /// Both verbs report the state AFTER the call, so a `clear` whose `source` did not own the
    /// view comes back `active: true` naming the owner that kept it
    /// (`tmp/herdr/src/app/api/agent_view.rs:63-71`). A caller that treated the answer as a bare
    /// acknowledgement would not be able to tell that apart from a clear that worked.
    AgentView {
        /// Whether a projection is installed.
        #[serde(default)]
        active: bool,
        /// Its owner.
        #[serde(default)]
        source: Option<String>,
        /// Its label.
        #[serde(default)]
        label: Option<String>,
    },
    /// The answer to `pane.get`, `pane.focus` **and `pane.split`** (`response.rs:117-119`).
    PaneInfo {
        /// The pane. For `pane.split` this is the **new** pane
        /// (`tmp/herdr/src/app/api/panes.rs:126-134`).
        pane: PaneInfo,
    },
    /// The answer to `pane.list` (`response.rs:120-122`).
    PaneList {
        /// The panes.
        panes: Vec<PaneInfo>,
    },
    /// The answer to `pane.current` (`response.rs:123-125`).
    ///
    /// A distinct `type` from [`Self::PaneInfo`] even though the payload is the same shape, so a
    /// client cannot confuse "the pane you named" with "the pane herdr chose for you".
    PaneCurrent {
        /// The current pane.
        pane: PaneInfo,
    },
    /// The answer to `pane.process_info` (`response.rs:138-140`).
    PaneProcessInfo {
        /// What herdr could see of the pane's processes.
        process_info: PaneProcessInfo,
    },
    /// The answer to `pane.read` (`response.rs:162-164`).
    PaneRead {
        /// The text and where it came from.
        read: PaneReadResult,
    },
    /// The **first** line of an `events.subscribe` stream (`response.rs:214`, produced at
    /// `tmp/herdr/src/api/server.rs:749-760`).
    ///
    /// It is an acknowledgement, not a result: every later line on that connection is a pushed
    /// [`super::events::Event`] with **no `id` field at all**
    /// (`tmp/herdr/src/api/subscriptions.rs:249-266` emits a bare `serde_json::Value`).
    ///
    /// On the wire it is `{"type":"subscription_started"}` — an empty struct variant, so the
    /// braces in herdr's declaration are load-bearing.
    SubscriptionStarted {},
    /// The answer to `pane.wait_for_output` (`response.rs:218-223`, produced at
    /// `tmp/herdr/src/api/wait.rs:100-107`).
    ///
    /// The fields are inline in herdr's enum; [`ResponseResult::output_matched`] lifts them into
    /// [`OutputMatched`] so a caller can hold one.
    OutputMatched {
        /// The pane that matched.
        pane_id: String,
        /// The content revision the match was seen at — **`0` at this pin.** `wait.rs:98` copies
        /// it out of the `PaneReadResult` the wait just read (`let revision = read.revision;`),
        /// and that read is the same `pane.read` dispatch that hard-codes `revision: 0`
        /// (`tmp/herdr/src/app/api/panes.rs:1540`). It is dead like every other
        /// pane-read-derived `revision`, and unlike [`super::panes::PaneInfo::revision`], which is
        /// the terminal's live counter. See [`super::panes::PaneReadResult`].
        revision: u64,
        /// The matching line. herdr only ever produces this variant with `Some`
        /// (`wait.rs:95-98`), but the field is declared `Option<String>` with no `skip`, so the
        /// key is always on the wire and is mirrored as declared.
        matched_line: Option<String>,
        /// The read the match was found in.
        read: PaneReadResult,
    },
    /// herdr's bare acknowledgement (`response.rs:307`) — the answer to every write verb this
    /// crate sends: `pane.report_agent`, `pane.report_agent_session`, `pane.report_metadata`,
    /// `pane.clear_agent_authority`, `pane.release_agent`, `pane.send_input` and `pane.close`.
    ///
    /// On the wire it is `{"type":"ok"}` — an empty struct variant, not a unit one, so the braces
    /// in herdr's declaration are load-bearing.
    Ok {},
    /// A `result.type` this client does not name.
    ///
    /// `#[serde(other)]` on a unit variant of an internally tagged enum is what makes
    /// *"JSON API clients should ignore unknown fields"* (`socket-api.mdx:959`) true here rather
    /// than aspirational: a newer herdr's result decodes instead of failing the line.
    ///
    /// **It is never a success.** Every typed accessor turns it into
    /// [`crate::HerdrError::UnexpectedResult`] — herdr's own client does the same
    /// (`tmp/herdr/src/api/client.rs:119`).
    #[serde(other)]
    Unrecognised,
}

/// The four fields of [`ResponseResult::OutputMatched`], lifted out of the inline variant.
///
/// herdr declares them inline (`tmp/herdr/src/api/schema/response.rs:218-223`); a nameable type is
/// what lets [`crate::HerdrClient::pane_wait_for_output`] hand one back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputMatched {
    /// The pane that matched.
    pub pane_id: String,
    /// The content revision the match was seen at — `0` at this pin; see
    /// [`ResponseResult::OutputMatched`].
    pub revision: u64,
    /// The matching line.
    pub matched_line: Option<String>,
    /// The read the match was found in.
    pub read: PaneReadResult,
}

impl ResponseResult {
    /// The `type` tag, for error messages.
    #[must_use]
    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::Pong { .. } => "pong",
            Self::SessionSnapshot { .. } => "session_snapshot",
            Self::WorkspaceCreated { .. } => "workspace_created",
            Self::TabCreated { .. } => "tab_created",
            Self::AgentStarted { .. } => "agent_started",
            Self::AgentPrompted { .. } => "agent_prompted",
            Self::TabInfo { .. } => "tab_info",
            Self::AgentInfo { .. } => "agent_info",
            Self::AgentList { .. } => "agent_list",
            Self::AgentView { .. } => "agent_view",
            Self::PaneInfo { .. } => "pane_info",
            Self::PaneList { .. } => "pane_list",
            Self::PaneCurrent { .. } => "pane_current",
            Self::PaneProcessInfo { .. } => "pane_process_info",
            Self::PaneRead { .. } => "pane_read",
            Self::SubscriptionStarted {} => "subscription_started",
            Self::OutputMatched { .. } => "output_matched",
            Self::Ok {} => "ok",
            Self::Unrecognised => "an unrecognised result type",
        }
    }

    /// The failure every typed accessor produces for the wrong `result.type`.
    fn unexpected<T>(self, method: &'static str, want: &'static str) -> Result<T> {
        Err(HerdrError::UnexpectedResult {
            method,
            want,
            got: self.type_name().to_owned(),
        })
    }

    /// herdr's bare acknowledgement, [`Self::Ok`].
    ///
    /// # Errors
    /// [`HerdrError::UnexpectedResult`] for any other `type`, **including
    /// [`Self::Unrecognised`]** — a write verb that came back as something this client cannot read
    /// is not a write that succeeded.
    pub fn ok(self, method: &'static str) -> Result<()> {
        match self {
            Self::Ok {} => Ok(()),
            other => other.unexpected(method, "ok"),
        }
    }

    /// The session snapshot.
    ///
    /// # Errors
    /// [`HerdrError::UnexpectedResult`] for any other `type`.
    pub fn session_snapshot(self, method: &'static str) -> Result<SessionSnapshot> {
        match self {
            Self::SessionSnapshot { snapshot } => Ok(*snapshot),
            other => other.unexpected(method, "session_snapshot"),
        }
    }

    /// The new workspace, its first tab and root pane, from `workspace.create`.
    ///
    /// # Errors
    /// [`HerdrError::UnexpectedResult`] for any other `type`.
    pub fn workspace_created(
        self,
        method: &'static str,
    ) -> Result<(WorkspaceInfo, TabInfo, PaneInfo)> {
        match self {
            Self::WorkspaceCreated {
                workspace,
                tab,
                root_pane,
            } => Ok((*workspace, *tab, *root_pane)),
            other => other.unexpected(method, "workspace_created"),
        }
    }

    /// The new tab and its root pane, from `tab.create`.
    ///
    /// # Errors
    /// [`HerdrError::UnexpectedResult`] for any other `type`.
    pub fn tab_created(self, method: &'static str) -> Result<(TabInfo, PaneInfo)> {
        match self {
            Self::TabCreated { tab, root_pane } => Ok((*tab, *root_pane)),
            other => other.unexpected(method, "tab_created"),
        }
    }

    /// The started agent and the argv herdr typed, from `agent.start`.
    ///
    /// # Errors
    /// [`HerdrError::UnexpectedResult`] for any other `type`.
    pub fn agent_started(self, method: &'static str) -> Result<(AgentInfo, Vec<String>)> {
        match self {
            Self::AgentStarted { agent, argv } => Ok((agent, argv)),
            other => other.unexpected(method, "agent_started"),
        }
    }

    /// The prompted agent, from `agent.prompt`.
    ///
    /// # Errors
    /// [`HerdrError::UnexpectedResult`] for any other `type`.
    pub fn agent_prompted(self, method: &'static str) -> Result<AgentInfo> {
        match self {
            Self::AgentPrompted { agent } => Ok(agent),
            other => other.unexpected(method, "agent_prompted"),
        }
    }

    /// One tab record.
    ///
    /// # Errors
    /// [`HerdrError::UnexpectedResult`] for any other `type`.
    pub fn tab(self, method: &'static str) -> Result<TabInfo> {
        match self {
            Self::TabInfo { tab } => Ok(tab),
            other => other.unexpected(method, "tab_info"),
        }
    }

    /// The active sidebar projection, after a `set` or a `clear`.
    ///
    /// # Errors
    /// [`HerdrError::UnexpectedResult`] for any other `type`, including [`Self::Unrecognised`].
    pub fn agent_view(self, method: &'static str) -> Result<AgentView> {
        match self {
            Self::AgentView {
                active,
                source,
                label,
            } => Ok(AgentView {
                active,
                source,
                label,
            }),
            other => other.unexpected(method, "agent_view"),
        }
    }

    /// One agent record.
    ///
    /// # Errors
    /// [`HerdrError::UnexpectedResult`] for any other `type`.
    pub fn agent(self, method: &'static str) -> Result<AgentInfo> {
        match self {
            Self::AgentInfo { agent } => Ok(agent),
            other => other.unexpected(method, "agent_info"),
        }
    }

    /// The agent roster.
    ///
    /// # Errors
    /// [`HerdrError::UnexpectedResult`] for any other `type`.
    pub fn agent_list(self, method: &'static str) -> Result<Vec<AgentInfo>> {
        match self {
            Self::AgentList { agents } => Ok(agents),
            other => other.unexpected(method, "agent_list"),
        }
    }

    /// One pane record, from `pane.get`, `pane.focus` or `pane.split`.
    ///
    /// [`Self::PaneCurrent`] is **not** accepted here: it is a distinct `type` and it means
    /// something distinct. Use [`Self::pane_current`].
    ///
    /// # Errors
    /// [`HerdrError::UnexpectedResult`] for any other `type`.
    pub fn pane(self, method: &'static str) -> Result<PaneInfo> {
        match self {
            Self::PaneInfo { pane } => Ok(pane),
            other => other.unexpected(method, "pane_info"),
        }
    }

    /// The pane list.
    ///
    /// # Errors
    /// [`HerdrError::UnexpectedResult`] for any other `type`.
    pub fn pane_list(self, method: &'static str) -> Result<Vec<PaneInfo>> {
        match self {
            Self::PaneList { panes } => Ok(panes),
            other => other.unexpected(method, "pane_list"),
        }
    }

    /// The pane herdr considers current.
    ///
    /// # Errors
    /// [`HerdrError::UnexpectedResult`] for any other `type`.
    pub fn pane_current(self, method: &'static str) -> Result<PaneInfo> {
        match self {
            Self::PaneCurrent { pane } => Ok(pane),
            other => other.unexpected(method, "pane_current"),
        }
    }

    /// What herdr could see of a pane's processes.
    ///
    /// # Errors
    /// [`HerdrError::UnexpectedResult`] for any other `type`.
    pub fn pane_process_info(self, method: &'static str) -> Result<PaneProcessInfo> {
        match self {
            Self::PaneProcessInfo { process_info } => Ok(process_info),
            other => other.unexpected(method, "pane_process_info"),
        }
    }

    /// A pane's text.
    ///
    /// # Errors
    /// [`HerdrError::UnexpectedResult`] for any other `type`.
    pub fn pane_read(self, method: &'static str) -> Result<PaneReadResult> {
        match self {
            Self::PaneRead { read } => Ok(read),
            other => other.unexpected(method, "pane_read"),
        }
    }

    /// herdr's subscription acknowledgement, [`Self::SubscriptionStarted`].
    ///
    /// **No other success is accepted as an ack**, including [`Self::Unrecognised`]: a stream whose
    /// first line was not this one is a connection this client does not understand the state of,
    /// and treating it as live would hand the consumer a subscription herdr never started. pi pins
    /// the same thing (`src/runs/shared/herdr-connection.ts:100` @v0.68.0,
    /// `result.type !== "subscription_started"`).
    ///
    /// # Errors
    /// [`HerdrError::UnexpectedResult`] for any other `type`.
    pub fn subscription_started(self, method: &'static str) -> Result<()> {
        match self {
            Self::SubscriptionStarted {} => Ok(()),
            other => other.unexpected(method, "subscription_started"),
        }
    }

    /// The match `pane.wait_for_output` waited for.
    ///
    /// # Errors
    /// [`HerdrError::UnexpectedResult`] for any other `type`.
    pub fn output_matched(self, method: &'static str) -> Result<OutputMatched> {
        match self {
            Self::OutputMatched {
                pane_id,
                revision,
                matched_line,
                read,
            } => Ok(OutputMatched {
                pane_id,
                revision,
                matched_line,
                read,
            }),
            other => other.unexpected(method, "output_matched"),
        }
    }
}

/// The two shapes a herdr answer line can take.
///
/// herdr's own client declares this as `#[serde(untagged)]` over success-then-error
/// (`tmp/herdr/src/api/client.rs:231-236`). **This one does not, and the difference is a
/// diagnostic one** — `[CYRUP-EXCEEDS-UPSTREAM]`.
///
/// *Premise:* an untagged enum reports every failure as the single string
/// `"data did not match any variant of untagged enum WireResponse"`, with `line: 0, column: 0`
/// and no `source` — serde discards each arm's real error before it reports. So the day a newer
/// herdr adds one required field to a record this crate mirrors, an untagged decoder says exactly
/// what it says for a truncated line, a corrupt line, and a line from an entirely different
/// protocol: *nothing*. That is the one failure mode this crate exists to not have.
///
/// [`Self::decode`] therefore dispatches on the key that is actually discriminating — herdr's
/// server writes `error` or `result`, never both (`tmp/herdr/src/api/server.rs:180-201` versus
/// `:301`) — and then decodes **one** arm, so the error that reaches
/// [`crate::HerdrError::Malformed`] is serde's own: `missing field \`terminal_id\` at line 1
/// column 84`.
#[derive(Debug, Clone, PartialEq)]
pub enum WireResponse {
    /// `{"id":…,"result":{…}}`.
    Success(Box<SuccessResponse>),
    /// `{"id":…,"error":{…}}`.
    Error(ErrorResponse),
}

impl WireResponse {
    /// Decode one answer line, keeping serde's own error.
    ///
    /// # Errors
    /// The `serde_json::Error` of whichever arm the line claimed to be, or of the line itself
    /// when it is not a JSON object at all.
    pub fn decode(line: &str) -> serde_json::Result<Self> {
        let value: serde_json::Value = serde_json::from_str(line)?;
        if value.get("error").is_some() {
            serde_json::from_value(value).map(Self::Error)
        } else {
            serde_json::from_value(value).map(|success| Self::Success(Box::new(success)))
        }
    }
}
