//! The wire data model — a 1:1 port of `pi-intercom/types.ts:1-51` + the health handshake
//! (`spawn.ts:97-106`, `paths.ts:8-9`).
//!
//! Field names cross the wire in pi's camelCase, so payload structs use
//! `#[serde(rename_all = "camelCase")]` and the message unions use
//! `#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]` (the `type`
//! discriminants are already snake_case: `cancel_ask`, `session_joined`, `session_left`,
//! `presence_update`, `delivery_failed`, `health_ok`).

/// `INTERCOM_PROTOCOL_NAME = "pi-intercom"` (`paths.ts:8`). The Rust broker answers the health probe
/// with this byte-identical value so the discovery contract holds across a mixed pi/cyrup deployment
/// on the same agent dir (the port doc §1.2 item 2).
pub const PROTOCOL_NAME: &str = "pi-intercom";
/// `INTERCOM_PROTOCOL_VERSION = 1` (`paths.ts:9`).
pub const PROTOCOL_VERSION: u32 = 1;

/// `SessionInfo` (`types.ts:1-12`). `peer_uid`/`trusted_local` are **broker-owned** (`broker.ts:374`)
/// and are never accepted from a `register` payload.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    /// Broker-assigned session id.
    pub id: String,
    /// Optional presence name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The session's working directory.
    pub cwd: String,
    /// The session's active model ref.
    pub model: String,
    /// The session's OS pid.
    pub pid: u32,
    /// Epoch-ms session start time.
    pub started_at: u64,
    /// Epoch-ms of the most recent activity.
    pub last_activity: u64,
    /// Optional lifecycle status string (`tool:<name>` | `thinking` | `idle` | custom suffix).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Broker-owned peer uid (TCP only; never from `register`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer_uid: Option<u32>,
    /// Broker-owned trust flag (`unix && !windows`; never from `register`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trusted_local: Option<bool>,
}

/// `Message` (`types.ts:14-23`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// Message id (also the ask `questionId` when `expects_reply`).
    pub id: String,
    /// Epoch-ms timestamp.
    pub timestamp: u64,
    /// The message id this is a reply to, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    /// Whether the sender expects a reply (records an ask edge on the broker).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expects_reply: Option<bool>,
    /// The message body.
    pub content: MessageContent,
}

/// `Message.content` (`types.ts:19-22`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageContent {
    /// The message text.
    pub text: String,
    /// Optional structured attachments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<Attachment>>,
}

/// `Attachment` (`types.ts:25-30`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    /// The attachment kind.
    #[serde(rename = "type")]
    pub kind: AttachmentKind,
    /// A display name for the attachment.
    pub name: String,
    /// The attachment content.
    pub content: String,
    /// Optional language hint (for a `snippet`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

/// The three `Attachment.type` values (`types.ts:26`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttachmentKind {
    /// A whole file.
    File,
    /// A code snippet.
    Snippet,
    /// Free-form context.
    Context,
}

/// `SessionRegistration = Omit<SessionInfo, "id" | "peerUid" | "trustedLocal">` (`types.ts:32`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRegistration {
    /// Optional presence name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The session's working directory.
    pub cwd: String,
    /// The session's active model ref.
    pub model: String,
    /// The session's OS pid.
    pub pid: u32,
    /// Epoch-ms session start time.
    pub started_at: u64,
    /// Epoch-ms of the most recent activity.
    pub last_activity: u64,
    /// Optional lifecycle status string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// Client → broker messages (`types.ts:34-40`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum ClientMessage {
    /// Register a session (optionally re-adopting a stable `session_id`).
    Register {
        /// The session's registration payload.
        session: SessionRegistration,
        /// A stable session id to re-adopt (broker takeover), if any.
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        /// The opt-in-TCP endpoint credential (`stateId`, `client.ts:284`): the broker's per-run
        /// `BROKER_STATE_ID` from `broker.port.json`. Required over TCP (`broker.ts:263-266`
        /// otherwise throws `Invalid intercom TCP endpoint credentials`), and **omitted** — never
        /// null — over a Unix socket / named pipe. Filled in by
        /// [`crate::transport::client::IntercomClient::connect_target`] from the resolved
        /// [`crate::transport::target::BrokerConnectTarget`].
        #[serde(skip_serializing_if = "Option::is_none")]
        state_id: Option<String>,
    },
    /// Unregister this session.
    Unregister,
    /// List all connected sessions.
    List {
        /// Correlation id echoed back on the `sessions` response.
        request_id: String,
    },
    /// Send a message to a target (name / id / unique-prefix).
    Send {
        /// Target name or id.
        to: String,
        /// The message to deliver.
        message: Message,
    },
    /// Cancel an outstanding ask edge this session owns.
    CancelAsk {
        /// The ask's message id.
        message_id: String,
    },
    /// Update this session's presence (coalesced by the broker).
    Presence {
        /// New presence name, if changed.
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// New status string, if changed.
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        /// New model ref, if changed.
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
}

/// Broker → client messages (`types.ts:42-51`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum BrokerMessage {
    /// Registration acknowledged; carries the broker-assigned session id.
    Registered {
        /// The assigned session id.
        session_id: String,
    },
    /// A `list` response.
    Sessions {
        /// The correlation id from the request.
        request_id: String,
        /// The connected sessions.
        sessions: Vec<SessionInfo>,
    },
    /// An inbound message routed from another session.
    Message {
        /// The sender's session info.
        from: SessionInfo,
        /// The delivered message.
        message: Message,
    },
    /// A presence change broadcast.
    PresenceUpdate {
        /// The updated session info.
        session: SessionInfo,
    },
    /// A session joined.
    SessionJoined {
        /// The joined session info.
        session: SessionInfo,
    },
    /// A session left.
    SessionLeft {
        /// The departed session id.
        session_id: String,
    },
    /// A broker-level error for this connection.
    Error {
        /// The error text.
        error: String,
    },
    /// A `send` was delivered.
    Delivered {
        /// The delivered message id.
        message_id: String,
    },
    /// A `send` could not be delivered.
    DeliveryFailed {
        /// The message id that failed.
        message_id: String,
        /// The failure reason.
        reason: String,
    },
}

/// The health handshake (`spawn.ts:97-106`) — NOT in the TS `ClientMessage`/`BrokerMessage` unions;
/// used only by discovery (`transport::spawn`) and answered by the broker.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum HealthMessage {
    /// A health probe.
    Health {
        /// The probe correlation id.
        request_id: String,
        /// The opt-in-TCP endpoint credential (`stateId`, `spawn.ts:290`), on the same terms as
        /// `register`'s: required over TCP (`broker.ts:251-254`), omitted over a socket / pipe.
        /// Filled in by [`crate::transport::spawn::check_target_connectable`].
        #[serde(skip_serializing_if = "Option::is_none")]
        state_id: Option<String>,
    },
    /// The broker's health response (`{type:"health_ok", requestId, protocol, version}`).
    HealthOk {
        /// The probe correlation id, echoed.
        request_id: String,
        /// Always [`PROTOCOL_NAME`].
        protocol: String,
        /// Always [`PROTOCOL_VERSION`].
        version: u32,
    },
}

/// Current epoch time in milliseconds (pi `Date.now()`). Saturates to `0` before the epoch (never
/// reached in practice) so this is total and panic-free.
#[must_use]
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn client_register_serializes_with_pi_field_names() {
        let msg = ClientMessage::Register {
            session: SessionRegistration {
                name: Some("alice".to_string()),
                cwd: "/w".to_string(),
                model: "m".to_string(),
                pid: 42,
                started_at: 1,
                last_activity: 2,
                status: None,
            },
            session_id: Some("sess-1".to_string()),
            state_id: None,
        };
        let v: serde_json::Value = serde_json::to_value(&msg).unwrap();
        assert_eq!(v["type"], "register");
        assert_eq!(v["sessionId"], "sess-1");
        assert_eq!(v["session"]["startedAt"], 1);
        assert_eq!(v["session"]["lastActivity"], 2);
        // state_id omitted when None.
        assert!(v.get("stateId").is_none());
    }

    #[test]
    fn broker_delivery_failed_uses_snake_case_tag_and_camel_fields() {
        let msg = BrokerMessage::DeliveryFailed {
            message_id: "m1".to_string(),
            reason: "Session not found".to_string(),
        };
        let v = serde_json::to_value(&msg).unwrap();
        assert_eq!(v["type"], "delivery_failed");
        assert_eq!(v["messageId"], "m1");
        assert_eq!(v["reason"], "Session not found");
    }

    #[test]
    fn health_ok_matches_pi_byte_shape() {
        let msg = HealthMessage::HealthOk {
            request_id: "r1".to_string(),
            protocol: PROTOCOL_NAME.to_string(),
            version: PROTOCOL_VERSION,
        };
        let s = serde_json::to_string(&msg).unwrap();
        // Field order is struct-declaration order; assert the exact set/values pi requires.
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["type"], "health_ok");
        assert_eq!(v["requestId"], "r1");
        assert_eq!(v["protocol"], "pi-intercom");
        assert_eq!(v["version"], 1);
    }

    #[test]
    fn message_round_trips_with_attachments() {
        let m = Message {
            id: "m".to_string(),
            timestamp: 9,
            reply_to: Some("q".to_string()),
            expects_reply: Some(true),
            content: MessageContent {
                text: "hi".to_string(),
                attachments: Some(vec![Attachment {
                    kind: AttachmentKind::Snippet,
                    name: "f.rs".to_string(),
                    content: "fn main(){}".to_string(),
                    language: Some("rust".to_string()),
                }]),
            },
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["replyTo"], "q");
        assert_eq!(v["expectsReply"], true);
        assert_eq!(v["content"]["attachments"][0]["type"], "snippet");
        let back: Message = serde_json::from_value(v).unwrap();
        assert_eq!(back, m);
    }
}
