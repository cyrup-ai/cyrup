//! The `session` WIT import: the read-only session view plus custom-entry persistence.

use serde::Serialize;
use serde_json::Value;

/// The read-only session view + state-persistence surface (Pi `ReadonlySessionManager` + R-08-026).
#[derive(Clone, Copy, Debug, Default)]
pub struct Session;

impl Session {
    /// Every entry except the session header, in file order (WIT `session.entries-json`,
    /// `wit/world.wit:756`; served by `cyrup-session-svc/src/host_services.rs:1602-1611`).
    ///
    /// [`Value::Null`] — NOT the empty array the host sends when there is no session
    /// (`cyrup-ext/src/host/live.rs:519`) — when the host sent JSON this SDK could not parse
    /// (`super::parse_json`'s fallback). A caller reading `as_array()` sees `None` for the
    /// unparseable case and `Some(&[])` for the empty one; they are different answers.
    pub fn entries(&self) -> Value {
        super::parse_json(session_call(SessionGet::Entries))
    }
    /// The root→leaf entry path from the CURRENT leaf (WIT `session.branch-json`,
    /// `wit/world.wit:757`; served by `cyrup-session-svc/src/host_services.rs:1613-1623`).
    ///
    /// [`Value::Null`] — NOT the empty array the host sends when there is no session
    /// (`cyrup-ext/src/host/live.rs:522`) — when the host sent JSON this SDK could not parse
    /// (`super::parse_json`'s fallback), so an unparseable answer never reads as an empty branch.
    pub fn branch(&self) -> Value {
        super::parse_json(session_call(SessionGet::Branch))
    }
    /// The session's entry TREE — a `SessionTreeNode[]` (WIT `session.tree-json`,
    /// `wit/world.wit:758`; served by `cyrup-session-svc/src/host_services.rs:1625-1631`).
    ///
    /// [`Value::Null`] covers TWO answers here, because it is also the host's own no-session
    /// fallback (`cyrup-ext/src/host/live.rs:525` sends the literal `"null"`): no tree to report,
    /// and JSON this SDK could not parse (`super::parse_json`'s fallback). Unlike
    /// [`Self::entries`] and [`Self::branch`], those two are indistinguishable to a caller.
    pub fn tree(&self) -> Value {
        super::parse_json(session_call(SessionGet::Tree))
    }
    /// Persist a custom (non-LLM) entry (R-08-026); returns the new entry id.
    ///
    /// `data` is author-supplied and its `serde_json` encoding is fallible; the failure is returned
    /// as `Err` rather than persisting an entry with a `null` body and reporting `Ok(entry_id)`.
    pub fn append_entry(&self, custom_type: &str, data: impl Serialize) -> Result<String, String> {
        let data_json = serde_json::to_string(&data).map_err(|e| format!("append_entry: {e}"))?;
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::session::append_entry(custom_type, &data_json);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (custom_type, data_json);
            Err("append_entry unavailable on host target".into())
        }
    }
    /// The session's name (WIT `session.get-session-name`, `wit/world.wit:762`); `None` when the
    /// session is unnamed, and always `None` on the host (non-`wasm32`) target.
    pub fn session_name(&self) -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::session::get_session_name();
        }
        #[cfg(not(target_arch = "wasm32"))]
        None
    }
    /// Rename the session (WIT `session.set-session-name`, `wit/world.wit:761`). The import
    /// returns nothing, so a failure is not observable here; a no-op on the host (non-`wasm32`)
    /// target.
    pub fn set_session_name(&self, name: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::session::set_session_name(name);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = name;
    }
    /// Set OR CLEAR a label on an entry — pi `setLabel(entryId: string, label: string | undefined)`
    /// (`extensions/types.ts:1314` @v0.83.0, "Set or clear a label on an entry. Labels are
    /// user-defined markers for bookmarking/navigation").
    ///
    /// EXT-046: `None` CLEARS. An empty string does NOT — it writes an empty label, leaving a
    /// marker the user cannot remove through the extension that created it.
    pub fn set_label(&self, entry_id: &str, label: Option<&str>) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::session::set_label(entry_id, label);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (entry_id, label);
    }
}

enum SessionGet {
    Entries,
    Branch,
    Tree,
}

fn session_call(which: SessionGet) -> String {
    #[cfg(target_arch = "wasm32")]
    {
        use crate::guest::bindings::cyrup::ext::session as s;
        return match which {
            SessionGet::Entries => s::entries_json(),
            SessionGet::Branch => s::branch_json(),
            SessionGet::Tree => s::tree_json(),
        };
    }
    // The host-target defaults are PER-VARIANT, matching the host's own no-session fallback
    // (`crates/cyrup-ext/src/host/live.rs:519`/`:522`/`:525`): `"[]"` for entries and branch,
    // `"null"` only for tree. A blanket `"null"` here would make `entries().as_array()` `None` on
    // the host target and always `Some` in the guest, so an `if let Some(..)` body would never be
    // exercised by a host-target test.
    #[cfg(not(target_arch = "wasm32"))]
    {
        match which {
            SessionGet::Entries | SessionGet::Branch => "[]".into(),
            SessionGet::Tree => "null".into(),
        }
    }
}
