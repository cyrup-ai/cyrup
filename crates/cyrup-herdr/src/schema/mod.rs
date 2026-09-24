//! herdr's wire types, mirrored file-for-file.
//!
//! Each module here has the same name as the `tmp/herdr/src/api/schema/` file it mirrors, and the
//! types inside keep herdr's own names and field spellings, so a `diff` against a future herdr pin
//! is line-for-line rather than a translation exercise. The one exception is [`request`], which
//! mirrors the top level of `tmp/herdr/src/api/schema.rs` (herdr keeps `Request` and `Method` in
//! the parent file rather than a submodule).
//!
//! **No `#[serde(deny_unknown_fields)]` anywhere.** herdr's own compatibility rule is *"JSON API
//! clients should ignore unknown fields"*
//! (`tmp/herdr/docs/preview/website/src/content/docs/socket-api.mdx:959`), so a newer herdr adding
//! a field must not turn every response into a parse error. The forward-compatible counterpart on
//! the enum side is [`response::ResponseResult::Unrecognised`].
//!
//! ## Regenerating this against a newer herdr
//!
//! herdr checks its own full JSON Schema into the tree — `src/cli/api.rs:1` is
//! `include_str!("../../docs/next/api/herdr-api.schema.json")` and
//! `src/api/schema/tests.rs:182-207` (`generated_protocol_schema_artifact_is_current`) fails herdr's
//! CI if it drifts from `schemars::schema_for!` over the live types. So: **bump the pin in
//! `tmp/herdr` and re-read `docs/next/api/herdr-api.schema.json`.** No `herdr` binary is needed. (In
//! a herdr checkout the artifact is refreshed with
//! `HERDR_UPDATE_API_SCHEMA=1 just test-one generated_protocol_schema_artifact_is_current`.)

//! ## One deliberate divergence: `BTreeMap` where herdr writes `HashMap`
//!
//! herdr's `env`, `tokens` and `state_labels` fields are `HashMap`
//! (`tmp/herdr/src/api/schema/panes.rs:41,120,123,180,183`). This mirror uses `BTreeMap`, which
//! is not a field spelling and not a wire shape — a JSON object is a JSON object — but it makes
//! the **bytes this client writes deterministic**, so a request can be asserted byte-for-byte in a
//! test and read in a log without its keys shuffling per process. herdr itself sorts the one map
//! whose order it could ever have cared about (`normalize_launch_env`,
//! `tmp/herdr/src/app/api/env.rs:30`), and reads every other one into an unordered map, so nothing
//! upstream observes the difference.

pub mod agents;
pub mod common;
pub mod events;
pub mod panes;
pub mod request;
pub mod response;
pub mod server;
pub mod session;
pub mod tabs;
pub mod workspaces;
pub mod worktrees;

pub use agents::{
    AgentInfo, AgentPromptParams, AgentPromptWaitOptions, AgentSessionInfo, AgentSessionRefKind,
    AgentStartParams, AgentView, AgentViewBuiltinField, AgentViewBuiltinSortField,
    AgentViewClearParams, AgentViewContext, AgentViewField, AgentViewFilter, AgentViewSetParams,
    AgentViewSort, AgentViewSortField, AgentViewSortOrder, AgentViewValue,
};
pub use common::{
    AgentStatus, AgentTarget, EmptyParams, PaneAgentState, PaneTarget, ReadFormat, ReadSource,
    SplitDirection, TabTarget,
};
pub use events::{
    Event, EventData, EventEnvelope, EventKind, EventsSubscribeParams, OutputMatch,
    PaneAgentStatusChangedEvent, PaneOutputMatchedEvent, PaneScrollChangedEvent,
    PaneWaitForOutputParams, Subscription, SubscriptionEventData, SubscriptionEventEnvelope,
    SubscriptionEventKind,
};
pub use panes::{
    PaneClearAgentAuthorityParams, PaneCurrentParams, PaneInfo, PaneLayoutPane, PaneLayoutRect,
    PaneLayoutSnapshot, PaneLayoutSplit, PaneListParams, PaneProcessInfo, PaneProcessInfoParams,
    PaneProcessInfoProcess, PaneReadParams, PaneReadResult, PaneReleaseAgentParams,
    PaneReportAgentParams, PaneReportAgentSessionParams, PaneReportMetadataParams,
    PaneRightClickTarget, PaneScrollInfo, PaneSendInputParams, PaneSplitParams,
};
pub use request::{Method, Request};
pub use response::{
    ErrorBody, ErrorResponse, OutputMatched, ResponseResult, SuccessResponse, WireResponse,
};
pub use server::{PingParams, ServerCapabilities};
pub use session::SessionSnapshot;
pub use tabs::{TabCreateParams, TabInfo, TabRenameParams};
pub use workspaces::{WorkspaceCreateParams, WorkspaceInfo, WorkspaceWorktreeInfo};
pub use worktrees::WorktreeInfo;
