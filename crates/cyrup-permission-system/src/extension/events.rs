//! The two event-bus publications: the `permission-request` channel a UI subscribes to, and the
//! resolved `permission-state` record that follows it.

use serde_json::{Value, json};

use crate::dedup::DedupDetails;

use super::PermissionSystemExtension;
use super::consts::{NO_EVENT_BACKEND_ERROR, PERMISSION_REQUEST_EVENT_CHANNEL};

impl PermissionSystemExtension {
    /// PERM-011 half B / pi `emitPermissionRequestEvent` (`index.ts:1518-1529`): put one
    /// permission-request event on the inter-extension bus, and — exactly as upstream does in its
    /// `catch` — record a `permission_request.event_emit_failed` debug entry if it cannot be
    /// delivered.
    ///
    /// \[CYRUP-DELTA] pi's failure mode is `pi.events.emit` THROWING; cyrup's
    /// [`cyrup_ext::HostServices::emit_event`] cannot fail, so the one way an emit can be lost is
    /// that no host backend is attached (a by-value extension, or a session that never bound one).
    /// That is the same class of loss — "the event did not reach the bus" — so it takes the same
    /// entry, with the reason in upstream's `error` key. The three keys pi puts on the entry
    /// (`requestId`, `source`, `state`) are its own (`:1522-1527`).
    fn emit_permission_request_event(
        &self,
        payload: &Value,
        request_id: &str,
        source: &str,
        state: &str,
    ) {
        if let Some(services) = self.host_services.get() {
            services.emit_event(PERMISSION_REQUEST_EVENT_CHANNEL, payload);
            return;
        }
        self.write_debug_entry(
            "permission_request.event_emit_failed",
            &json!({
                "requestId": request_id,
                "source": source,
                "state": state,
                "error": NO_EVENT_BACKEND_ERROR,
            }),
        );
    }

    /// PERM-011 half B / pi `emitPermissionStateEvent(details, state)` (`index.ts:1531-1546`): the
    /// `PermissionRequestEvent` projection of a prompt's details, in upstream's key order
    /// (`:1536-1545`), plus the `state`.
    ///
    /// \[CYRUP-DELTA] pi hands the listener a live JS object, so its optional fields are `undefined`
    /// and vanish under `JSON.stringify`; cyrup's payload is a [`Value`], which has no `undefined`,
    /// so an absent field is `null`. Nothing else about the shape differs — the key set and their
    /// order are upstream's.
    pub(super) fn emit_permission_state_event(&self, details: &DedupDetails, state: &str) {
        let payload = json!({
            "requestId": details.request_id,
            "source": details.source,
            "state": state,
            "message": details.message,
            "toolCallId": details.tool_call_id,
            "toolName": details.tool_name,
            "skillName": details.skill_name,
            "path": details.path,
            "command": details.command,
            "target": details.target,
            "toolInput": details.tool_input,
            "agentName": details.agent_name,
        });
        self.emit_permission_request_event(&payload, &details.request_id, &details.source, state);
    }
}
