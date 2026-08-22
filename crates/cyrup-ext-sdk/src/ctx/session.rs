//! The `session` WIT import: the read-only session view plus custom-entry persistence.

use serde::Serialize;
use serde_json::Value;

/// The read-only session view + state-persistence surface (Pi `ReadonlySessionManager` + R-08-026).
#[derive(Clone, Copy, Debug, Default)]
pub struct Session;

impl Session {
    pub fn entries(&self) -> Value {
        super::parse_json(session_call(SessionGet::Entries))
    }
    pub fn branch(&self) -> Value {
        super::parse_json(session_call(SessionGet::Branch))
    }
    pub fn tree(&self) -> Value {
        super::parse_json(session_call(SessionGet::Tree))
    }
    /// Persist a custom (non-LLM) entry (R-08-026); returns the new entry id.
    pub fn append_entry(&self, custom_type: &str, data: impl Serialize) -> Result<String, String> {
        let data_json = serde_json::to_string(&data).unwrap_or_else(|_| "null".into());
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
    pub fn session_name(&self) -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::session::get_session_name();
        }
        #[cfg(not(target_arch = "wasm32"))]
        None
    }
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

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
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
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = which;
        "null".into()
    }
}
