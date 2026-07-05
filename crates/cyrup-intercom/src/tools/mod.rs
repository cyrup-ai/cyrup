//! The two intercom tools registered via `InitApi::register_tool`:
//!
//! - [`intercom::IntercomTool`] (`intercom`) — always registered (`index.ts:1425-1806`).
//! - [`contact_supervisor::ContactSupervisorTool`] (`contact_supervisor`) — registered ONLY when
//!   child-orchestrator metadata is present (`index.ts:1162-1163`).
//!
//! Both are `cyrup_core::Tool` impls backed by the shared [`crate::session_state::SharedIntercomState`]
//! (its live [`crate::transport::client::IntercomClient`], [`crate::reply_tracker`] state, and the
//! outbound single-slot waiter).

pub mod contact_supervisor;
pub mod intercom;

use cyrup_core::{Content, ToolResult};

/// Build a plain-text [`ToolResult`].
pub(crate) fn text_result(text: impl Into<String>) -> ToolResult {
    ToolResult { content: vec![Content::text(text.into())], details: None, terminate: false }
}
