//! The two JSON shapers the host-services reads share with the rest of the crate:
//! [`builtin_tool_source_info`] (pi's synthetic `SourceInfo` for a tool the extension registry does
//! not own) and [`tree_node_to_json`] (one `SessionTreeNode` in pi's `getTree()` shape).
//!
//! Both live outside the [`LiveHostServices`] impl because each has a SECOND caller — the tool
//! serializer for the first, [`crate::AgentSession::tree_json`] for the second — and the whole
//! point is that the two emitters cannot drift apart.

use serde_json::{json, Value};

// Doc-only: the docs below name the backend these shape values for and the trait method the tree
// shaper feeds; neither is named in code here.
#[cfg(doc)]
use super::LiveHostServices;
#[cfg(doc)]
use cyrup_ext::host::HostServices;

/// pi's synthetic `SourceInfo` for a tool the extension registry does not own — the value
/// `createSyntheticSourceInfo("<builtin:NAME>", { source: "builtin" })` produces
/// (`core/agent-session.ts:2478`, defaults from `core/source-info.ts:24-40` @v0.83.0: scope
/// `"temporary"`, origin `"top-level"`, no `baseDir`).
///
/// CYRUP-DELTA (`core/agent-session.ts:2468` @v0.83.0): pi distinguishes an SDK-supplied custom
/// tool with a THIRD tag, `("<sdk:${name}>", {source: "sdk"})`. cyrup's dynamic-tool registry (the
/// port of `_toolDefinitions`, [`crate::tools::DynamicToolState`]) folds the caller's `custom_tools`
/// into the same by-name map as the built-ins at build time (`builder.rs`, "the SDK-supplied custom
/// tools go through the same registered-tool wrapper") and keeps no provenance column, so an SDK
/// tool is indistinguishable from a built-in at this seam and reports as `builtin`.
pub(crate) fn builtin_tool_source_info(name: &str) -> Value {
    json!({
        "path": format!("<builtin:{name}>"),
        "source": "builtin",
        "scope": "temporary",
        "origin": "top-level",
    })
}

/// One `SessionTreeNode` (pi `core/session-manager.ts:159-166`) as `{entry, children, label?,
/// labelTimestamp?}` — the shape pi's `getTree()` hands out and the wire contract names
/// (`modes/rpc/rpc-types.ts:202-208`).
///
/// Lives here rather than nested inside [`crate::AgentSession::tree_json`] because BOTH the RPC
/// `get_tree` reply and the extension seam's [`HostServices::tree`] must emit the identical shape;
/// two copies is exactly how SEAM-060's dropped `labelTimestamp` survived on one side after being
/// fixed on the other.
pub(crate) fn tree_node_to_json(node: &cyrup_session::manager::TreeNode) -> Value {
    let mut obj = serde_json::Map::new();
    if let Ok(entry) = serde_json::to_value(&node.entry) {
        obj.insert("entry".to_string(), entry);
    }
    obj.insert(
        "children".to_string(),
        Value::Array(node.children.iter().map(tree_node_to_json).collect()),
    );
    if let Some(label) = &node.label {
        obj.insert("label".to_string(), Value::String(label.clone()));
    }
    if let Some(ts) = &node.label_timestamp {
        obj.insert("labelTimestamp".to_string(), Value::String(ts.clone()));
    }
    Value::Object(obj)
}
