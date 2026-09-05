//! The wire data model — a 1:1 port of `pi-intercom/types.ts` at **v0.9.2** (`v0.9.2 types.ts:1-136`)
//! plus the health handshake, which is not in either TS union
//! (`v0.9.2 broker/spawn.ts:104-113,302-306`, `v0.9.2 broker/paths.ts:8-9`).
//!
//! Field names cross the wire in pi's camelCase, so payload structs use
//! `#[serde(rename_all = "camelCase")]` and the message unions use
//! `#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]` (the `type`
//! discriminants are already snake_case: `cancel_ask`, `session_joined`, `presence_update`,
//! `delivery_failed`, `message_receipt`, `extension_state_result`, `health_ok`).
//!
//! # Why this file is mostly guards
//!
//! pi has no schema. Every frame is `JSON.parse`d and then hand-checked by a type guard
//! (`isMessage`, `isSessionInfo`, `isMessageReceipt`, `isMessageControl`,
//! `validateExtensionCapability`, …), and a guard that returns `false` becomes a `throw` in the
//! message switch, which `framing.ts:44-51` turns into `socket.destroy()`. So each guard defines
//! an *acceptance set*, and matching it is the whole job of this module. Both directions of a
//! mismatch are bugs, and they are different bugs:
//!
//! * **cyrup looser than pi** is an input-validation hole on a socket every local process can
//!   reach — cyrup would accept a frame upstream kills the connection over.
//! * **cyrup stricter than pi** is a denial of service. A required field that fails to decode is a
//!   fatal frame here (`FrameResult::protocol_error()` in [`crate::broker`], or a decode failure in
//!   the client's read task), so a value pi serves normally would destroy a connection. Worse, it
//!   *amplifies*: a [`SessionInfo`] is relayed to every attached client at four tags, so one
//!   hostile `register` could knock over peers that never spoke to it.
//!
//! Four invariants below exist only to keep serde on the correct side of that line. Each is marked
//! in the code with the tag used here.
//!
//! ## `[MAP-ONLY]` — a JSON array is never a payload
//!
//! serde's derived `Deserialize` implements `visit_seq`, so `["m1","queued",1,null]` would fill a
//! four-field struct *positionally*. pi does the opposite: `isMessageReceipt`,
//! `isMessageControl` and `isSessionRegistration` bail on `Array.isArray` outright
//! (`v0.9.2 broker/client.ts:57-59,68-70`, `v0.9.2 broker/broker.ts:108-110,191-193`) and the rest
//! reject an array because `[]["id"]` is `undefined`. Every guarded payload struct here therefore
//! carries a `#[serde(flatten)] extra` capture, which makes serde derive a **map-only** visitor —
//! and the invariant is pinned by this module's `every_guarded_payload_rejects_an_array` test
//! rather than left to that side effect being remembered.
//!
//! ## `[UNKNOWN-FIELDS]` — the relay must not strip what it does not model
//!
//! pi's broker re-forwards a message by object spread (`v0.9.2 broker/broker.ts:672-676`), so a
//! cyrup broker sitting between two pi sessions must carry the half of their envelope it has no
//! field for. The same `extra` capture does that, and [`Message`] round-trips unknown keys
//! verbatim.
//!
//! ## `[NON-NULL]` — an explicit `null` is not `undefined`
//!
//! Every optional field in pi's guards reads `x !== undefined && typeof x !== "T"`. In JavaScript
//! `null !== undefined` and `typeof null === "object"`, so an explicit `null` fails all of them:
//! pi rejects `{"replyTo": null}` exactly as it rejects `{"replyTo": 7}`, and accepts only an
//! *absent* key. serde's `Option<T>` does the opposite — it maps `null` to `None`, and with
//! `skip_serializing_if` the key is then deleted from anything cyrup re-emits, which silently
//! corrupts a relayed envelope. Optional fields therefore use
//! [`present_non_null`] (invoked only when the key is present) plus `#[serde(default)]` for the
//! absent case. The one exception is `presence`'s context trio, where upstream gives `null` its own
//! meaning — see [`ClientMessage::Presence`].
//!
//! ## `[JS-NUMBER]` — a wire number is an IEEE-754 double, not a `u32`
//!
//! pi guards every numeric field with `typeof x === "number"` and nothing else, so `-1`, `1.5`,
//! `2**32` and `1e300` are all values a conforming peer may send and a pi broker relays without
//! comment. Numeric wire fields are therefore [`serde_json::Number`] — not `f64`, so an integer
//! still relays *as* an integer rather than as `1700000000000.0`. Narrowing happens at the point of
//! use ([`as_os_pid`], [`as_epoch_ms`]), never at the wire boundary: a `pid` of `-1` must decode
//! fine and must never become a signallable pid, because that is what `kill(2)` reads as "every
//! process the caller may signal". The two exceptions are the revisions, which pi guards with
//! `Number.isSafeInteger` — see [`js_safe_integer`].

use serde::Deserialize as _;
use serde::de::Error as _;

/// `INTERCOM_PROTOCOL_NAME = "pi-intercom"` (`v0.9.2 broker/paths.ts:8`). The Rust broker answers
/// the health probe with this byte-identical value so the discovery contract holds across a mixed
/// pi/cyrup deployment on the same agent dir.
pub const PROTOCOL_NAME: &str = "pi-intercom";
/// `INTERCOM_PROTOCOL_VERSION = 1` (`v0.9.2 broker/paths.ts:9`).
pub const PROTOCOL_VERSION: u32 = 1;
/// `EXTENSION_BUS_FEATURE = "extension-bus-v1"` (`v0.9.2 types.ts:1`) — the single value pi's
/// broker advertises in `registered.features`.
///
/// ICOM-016: cyrup's broker DOES advertise it, on every `registered`
/// (`broker/session.rs`, matching `v0.9.2 broker/broker.ts:502-506`), and that is what admits the
/// frames — a conforming pi client gates every extension-bus frame on `supportsFeature()`
/// (`v0.9.2 broker/client.ts:648,817-819`). It could not be advertised until the bus effects
/// existed; they do now (owner election, publish fan-out and the revision-checked state store in
/// `broker::extension_state`). The constant exists so the negotiated name is stated once, next to
/// the union that carries it.
pub const EXTENSION_BUS_FEATURE: &str = "extension-bus-v1";
/// `EXACT_SEND_FEATURE = "exact-send-v1"` (`v0.13.0 types.ts:2`, added by `636f61e` at v0.11.0) —
/// the second value pi's broker advertises in `registered.features`.
///
/// ICOM-054: it tells a client this broker mints [`SessionInfo::endpoint_epoch`] and honours the
/// `targetId`/`targetEpoch` pair on `send`. Gated exactly like the bus feature — a conforming client
/// that does not see it never sends the pair (`v0.13.0 broker/client.ts:671`), so a v0.9.2 broker
/// keeps receiving the v0.9.2 frame byte-for-byte.
pub const EXACT_SEND_FEATURE: &str = "exact-send-v1";

/// `Number.MAX_SAFE_INTEGER` (2^53 - 1) — the bound `Number.isSafeInteger` enforces.
const JS_MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// The unmodelled remainder of a payload object: `[UNKNOWN-FIELDS]` on the way through and
/// `[MAP-ONLY]` on the way in.
///
/// A `serde_json::Map` rather than a `HashMap` so key order is whatever the crate's `preserve_order`
/// setting says and values stay `serde_json::Value` — i.e. an integer that arrived as an integer
/// leaves as one.
pub type UnknownFields = serde_json::Map<String, serde_json::Value>;

/// `[NON-NULL]` — accept a value only when the key is **present and not `null`**.
///
/// serde calls a field's `deserialize_with` only when the key actually appears in the map, so this
/// function never sees the absent case; `#[serde(default)]` covers that and yields `None`. When the
/// key *is* present, `T::deserialize` runs against the raw value, and JSON `null` is not a `String`
/// / `bool` / [`serde_json::Number`] / `Vec`, so it fails — which is pi's
/// `x !== undefined && typeof x !== "T"` exactly (`v0.9.2 broker/client.ts:117-135,170-188`,
/// `v0.9.2 broker/broker.ts:151-171,207-211`).
///
/// Fields using this **must** also carry `#[serde(default)]`, or an absent key becomes a missing-
/// field error and cyrup turns into the stricter side.
///
/// # Errors
/// Propagates `T`'s own error, which is what an explicit `null` produces.
fn present_non_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// `[NON-NULL]`, inverted — the one place upstream gives `null` a *meaning* rather than rejecting
/// it, so all three states have to survive decoding.
///
/// Used only by [`ClientMessage::Presence`]'s context trio, where `case "presence"` treats an
/// absent key as "leave the field alone", an explicit `null` as "CLEAR the field" and a number as
/// "set it" (`v0.9.2 broker/broker.ts:921-950`). A plain `Option<Option<T>>` cannot express that:
/// serde lets the outer `Option` swallow the `null`, collapsing "clear" into "absent". With
/// `#[serde(default)]` this yields `None` for absent, `Some(None)` for `null` and `Some(Some(v))`
/// for a value.
///
/// # Errors
/// Propagates `T`'s own error for a value that is neither `null` nor a `T`.
fn present_nullable<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// `Number.isSafeInteger(x) && x >= 0` (`v0.9.2 broker/client.ts:574-575,588-589`,
/// `v0.9.2 broker/broker.ts:1417`) — the one numeric field family pi bounds.
///
/// This is the `[JS-NUMBER]` exception. A bare `u64` would accept up to 2^64-1 and so be looser
/// than pi above 2^53-1; an `i64` would accept `-1`, which upstream's explicit `< 0` arm rejects.
/// Both revisions therefore decode through here.
///
/// `2.0` is deliberately accepted: JS has one numeric type, so `Number.isSafeInteger(2.0)` is
/// `true`. Rust's parser keeps a literal with a decimal point as an `f64`, so the integrality test
/// has to happen here rather than being implied by the type.
///
/// # Errors
/// Rejects a non-number, a non-integral or non-finite number, a negative number, and any magnitude
/// above [`JS_MAX_SAFE_INTEGER`].
fn js_safe_integer<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = serde_json::Number::deserialize(deserializer)?;
    let value = if let Some(unsigned) = raw.as_u64() {
        unsigned
    } else if let Some(float) = raw.as_f64() {
        if !float.is_finite() || float.fract() != 0.0 || float < 0.0 {
            return Err(D::Error::custom(format!(
                "expected a non-negative safe integer, got {raw}"
            )));
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "guarded above: finite, integral, non-negative, and bounded by the \
                      JS_MAX_SAFE_INTEGER check below — 2^53 is exactly representable in f64"
        )]
        if float > JS_MAX_SAFE_INTEGER as f64 {
            return Err(D::Error::custom(format!(
                "{raw} exceeds Number.MAX_SAFE_INTEGER"
            )));
        } else {
            float as u64
        }
    } else {
        // The remaining case is a negative integer, which `as_u64` refuses and pi's `< 0` arm does
        // too.
        return Err(D::Error::custom(format!(
            "expected a non-negative safe integer, got {raw}"
        )));
    };
    if value > JS_MAX_SAFE_INTEGER {
        return Err(D::Error::custom(format!(
            "{value} exceeds Number.MAX_SAFE_INTEGER"
        )));
    }
    Ok(value)
}

/// Narrow a wire number to an OS pid, or refuse.
///
/// `[JS-NUMBER]` says the *decoder* must accept whatever `typeof x === "number"` accepts, because
/// pi does and a stricter decoder destroys connections pi serves. This is the other half of that
/// bargain: the narrowing happens here, at the point of use, where refusing is cheap and local.
///
/// `None` for anything that is not a strictly positive integer inside `u32`. The two values that
/// matter are `-1` and `0`, and they matter for the same reason: `kill(-1, …)` signals *every
/// process the caller may signal* and `kill(0, …)` signals the caller's whole process group, so a
/// silent cast of a hostile `register` payload would turn a presence field into a remote kill
/// switch. Fractional and out-of-range values are refused too, since neither is a pid.
#[must_use]
pub fn as_os_pid(value: &serde_json::Number) -> Option<u32> {
    let pid = value.as_u64()?;
    if pid == 0 {
        return None;
    }
    u32::try_from(pid).ok()
}

/// Narrow a wire number to an epoch-millisecond timestamp, or refuse.
///
/// The [`as_os_pid`] bargain, for the time fields (`timestamp`, `startedAt`, `lastActivity`,
/// `brokerReceivedAt`, …). `None` for a negative value (before 1970, which nothing in this protocol
/// produces) and for a fractional one, so arithmetic downstream never has to wonder.
#[must_use]
pub fn as_epoch_ms(value: &serde_json::Number) -> Option<u64> {
    value.as_u64()
}

/// `SessionInfo` (`v0.9.2 types.ts:3-22`), guarded by `isSessionInfo`
/// (`v0.9.2 broker/client.ts:152-189`).
///
/// `peer_uid`/`trusted_local` are **broker-owned** (`v0.9.2 broker/broker.ts:481`) and are never
/// accepted from a `register` payload — [`SessionRegistration`] omits them, exactly as
/// `Omit<SessionInfo, "id" | "peerUid" | "trustedLocal">` does.
///
/// This is the type that makes strictness dangerous: the broker relays it at four tags
/// (`session_joined`, `presence_update`, `sessions[]`, `message.from`), so a field cyrup refuses is
/// a disconnect for every attached client, not just for whoever sent it.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    /// Broker-assigned session id (`v0.9.2 broker/client.ts:160`).
    pub id: String,
    /// `endpointEpoch` (`v0.13.0 types.ts:14-16`): "Broker-owned lifetime of this live endpoint."
    ///
    /// Minted fresh by the broker on EVERY `register` (`v0.13.0 broker/broker.ts:466`), a stable-id
    /// takeover included — the id names the identity, this names the particular socket binding of
    /// it, and that distinction is the only fact that makes a stale send detectable. Never supplied
    /// by a client: `SessionRegistration` omits it upstream (`v0.13.0 types.ts:102`) and
    /// [`SessionRegistration`] does not model it here.
    ///
    /// `[NON-NULL]`, per `isSessionInfo`'s guard (`v0.13.0 broker/protocol.ts:142-144`): absent is
    /// legal (a pre-v0.11.0 broker mints none, and the client then degrades to a plain name-routed
    /// send), a non-string is fatal.
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub endpoint_epoch: Option<String>,
    /// Optional presence name (`v0.9.2 broker/client.ts:170-172`).
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub name: Option<String>,
    /// `runtimeFallbackAlias` (`v0.10.1 types.ts:6-7`): "True only when the extension synthesized
    /// name for an unnamed runtime."
    ///
    /// Additive at v0.10.0 (`126875e`). Relayed by the broker at every tag that carries `SessionInfo`
    /// (`v0.10.1 broker/broker.ts:358` on register, `:779-787` on presence), and it is the input to
    /// the mailbox identity guard (`:1039-1047`, `if (!lowerName || info.runtimeFallbackAlias)
    /// return []`) that stops one unnamed session inheriting another's queued mail. A cyrup broker
    /// sitting between two pi v0.10 sessions must not strip it.
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub runtime_fallback_alias: Option<bool>,
    /// The session's working directory (`v0.9.2 broker/client.ts:161`).
    pub cwd: String,
    /// The session's active model ref (`v0.9.2 broker/client.ts:162`).
    pub model: String,
    /// The session's OS pid — `[JS-NUMBER]` (`v0.9.2 broker/client.ts:163`). Narrow with
    /// [`as_os_pid`] before ever handing it to a syscall.
    pub pid: serde_json::Number,
    /// Epoch-ms session start time — `[JS-NUMBER]` (`v0.9.2 broker/client.ts:164`).
    pub started_at: serde_json::Number,
    /// Epoch-ms of the most recent activity — `[JS-NUMBER]` (`v0.9.2 broker/client.ts:165`).
    pub last_activity: serde_json::Number,
    /// Optional lifecycle status string (`tool:<name>` | `thinking` | `idle` | custom suffix)
    /// (`v0.9.2 broker/client.ts:174-176`).
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub status: Option<String>,
    /// Broker-owned peer uid (TCP only; never from `register`) — `[JS-NUMBER]`
    /// (`v0.9.2 broker/client.ts:178-180`).
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub peer_uid: Option<serde_json::Number>,
    /// Broker-owned trust flag (`unix && !windows`; never from `register`)
    /// (`v0.9.2 broker/client.ts:188`, set at `v0.9.2 broker/broker.ts:481`).
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub trusted_local: Option<bool>,
    /// Live context-window usage, 0..100 rounded — `[JS-NUMBER]` (`v0.9.2 types.ts:19`).
    ///
    /// This field and its two siblings are guarded by a *loop* rather than by the per-field ladder
    /// above them (`v0.9.2 broker/client.ts:182-186`), which is exactly why they were missed the
    /// first time. Optional because the value is genuinely unknown right after a compaction, when
    /// no model is selected, and on older clients that never report it.
    ///
    /// Note the asymmetry with [`ClientMessage::Presence`]: `null` is **fatal** here
    /// (`typeof null === "object"`) and **legal** there. The two ladders are ported separately for
    /// that one reason.
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub context_pct: Option<serde_json::Number>,
    /// Raw context token count — `[JS-NUMBER]` (`v0.9.2 types.ts:20`, guarded at
    /// `v0.9.2 broker/client.ts:182-186`).
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub context_tokens: Option<serde_json::Number>,
    /// The model's context window in tokens — `[JS-NUMBER]` (`v0.9.2 types.ts:21`, guarded at
    /// `v0.9.2 broker/client.ts:182-186`).
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub context_window: Option<serde_json::Number>,
    /// `tmuxPane` (`v0.12.0 types.ts:42`): the tmux pane id (e.g. `"%212"`) of the session's
    /// terminal, read from `$TMUX_PANE` at registration and copied onto the stored `SessionInfo` by
    /// the broker (`v0.12.0 broker/broker.ts:475`).
    ///
    /// Additive at v0.11.0 (`4af53db`). Present only when the session runs inside a tmux pane;
    /// absent for cloud, headless and IDE-embedded sessions. Upstream's own note is the reason it is
    /// worth relaying: the pane id is IMMUTABLE for the process lifetime — unlike the window name —
    /// so a peer can live-resolve the current window from it via tmux.
    ///
    /// `[NON-NULL]` — `isSessionInfo` guards it with
    /// `value.tmuxPane !== undefined && typeof value.tmuxPane !== "string"`
    /// (`v0.12.0 broker/protocol.ts:168`), so an explicit `null` is fatal, exactly as for `status`.
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub tmux_pane: Option<String>,
    /// `[UNKNOWN-FIELDS]` + `[MAP-ONLY]`. A newer pi broker's additive keys survive a cyrup hop,
    /// and a JSON array can no longer fill this struct positionally.
    #[serde(flatten)]
    pub extra: UnknownFields,
}

/// The single `MessageProvenance.type` value (`v0.12.0 types.ts:65`). A CLOSED one-variant
/// vocabulary — `isMessageProvenance` compares it with `===` (`v0.12.0 broker/protocol.ts:78`), so
/// an unknown tag fails the whole message rather than being ignored, exactly like [`AttachmentKind`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceKind {
    /// Delivered on behalf of an extension through the outbox.
    ExtensionOutbox,
}

/// `MessageProvenance` (`v0.12.0 types.ts:64-69`), guarded by `isMessageProvenance`
/// (`v0.12.0 broker/protocol.ts:73-81`) and enforced inside `isMessage` at `:114-116`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageProvenance {
    /// Always `"extension_outbox"` (`v0.12.0 broker/protocol.ts:78`).
    #[serde(rename = "type")]
    pub kind: ProvenanceKind,
    /// The originating extension's id (`:79`).
    pub extension_id: String,
    /// The originating extension's display name (`:80`) — the string the inbound card renders.
    pub extension_name: String,
    /// The `IntercomOutboxRequestV1.requestId` this delivery answers (`:81`).
    pub request_id: String,
    /// `[UNKNOWN-FIELDS]` + `[MAP-ONLY]`, as every sibling envelope struct carries.
    #[serde(flatten)]
    pub extra: UnknownFields,
}

/// `Message` (`v0.9.2 types.ts:24-40`), guarded by `isMessage`
/// (`v0.9.2 broker/client.ts:106-150`, mirrored at `v0.9.2 broker/broker.ts:140-184`).
///
/// Field order matches upstream's declaration order. The five counters are what a v0.9.x peer uses
/// to reason about message lifecycle and latency; `broker_received_at`/`broker_delivered_at` are
/// stamped by the broker on the way through (`v0.9.2 broker/broker.ts:672-676`), which is why they
/// are modelled here rather than left to the `extra` capture.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// Message id (also the ask `questionId` when `expects_reply`)
    /// (`v0.9.2 broker/client.ts:113`).
    pub id: String,
    /// Epoch-ms timestamp — `[JS-NUMBER]` (`v0.9.2 broker/client.ts:113`).
    pub timestamp: serde_json::Number,
    /// The sender's monotonic per-connection counter — `[JS-NUMBER]`
    /// (`v0.9.2 types.ts:27`, guarded at `v0.9.2 broker/client.ts:117-121`).
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub sender_sequence: Option<serde_json::Number>,
    /// Epoch-ms the broker accepted the `send` — broker-owned (`v0.9.2 broker/broker.ts:674`).
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub broker_received_at: Option<serde_json::Number>,
    /// Epoch-ms the broker wrote it to the target — broker-owned
    /// (`v0.9.2 broker/broker.ts:675`).
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub broker_delivered_at: Option<serde_json::Number>,
    /// Epoch-ms the receiver read it — `[JS-NUMBER]` (`v0.9.2 types.ts:30`).
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub receiver_received_at: Option<serde_json::Number>,
    /// Epoch-ms the receiver injected it into its own turn — `[JS-NUMBER]`
    /// (`v0.9.2 types.ts:31`).
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub injected_at: Option<serde_json::Number>,
    /// The message id this one replaces (`v0.9.2 types.ts:32`, guarded at
    /// `v0.9.2 broker/client.ts:123-125`).
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub supersedes: Option<String>,
    /// The message id this one retries (`v0.9.2 types.ts:33`, guarded at
    /// `v0.9.2 broker/client.ts:127-129`).
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub retry_of: Option<String>,
    /// The message id this is a reply to, if any (`v0.9.2 broker/client.ts:131-133`).
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub reply_to: Option<String>,
    /// Whether the sender expects a reply (records an ask edge on the broker)
    /// (`v0.9.2 broker/client.ts:135-137`).
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub expects_reply: Option<bool>,
    /// Who originated this message, when it was not the agent itself
    /// (`v0.12.0 types.ts:57`, guarded at `v0.12.0 broker/protocol.ts:114-116`).
    ///
    /// `[NON-NULL]`: absent is legal, an explicit `null` is FATAL — `isRecord(null)` is `false`, so
    /// `isMessageProvenance(null)` fails and `isMessage` rejects the envelope. That is precisely
    /// what [`present_non_null`] reproduces. Until this field existed the key round-tripped through
    /// [`Message::extra`], which is why a v0.12.0 peer was tolerated but unattributable.
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub provenance: Option<MessageProvenance>,
    /// The message body (`v0.9.2 broker/client.ts:139-141`).
    pub content: MessageContent,
    /// `[UNKNOWN-FIELDS]` + `[MAP-ONLY]`. This is the capture the relay claim rests on: pi
    /// re-forwards by object spread (`v0.9.2 broker/broker.ts:672-676`), so a cyrup broker between
    /// two pi sessions must not delete the half of their envelope it has no field for.
    #[serde(flatten)]
    pub extra: UnknownFields,
}

impl Default for Message {
    /// Hand-written because [`serde_json::Number`] has no `Default` — an empty message is
    /// `timestamp: 0`, which is also what a caller that forgets to stamp it would want to be
    /// obviously wrong.
    fn default() -> Self {
        Self {
            id: String::new(),
            timestamp: serde_json::Number::from(0u64),
            sender_sequence: None,
            broker_received_at: None,
            broker_delivered_at: None,
            receiver_received_at: None,
            injected_at: None,
            supersedes: None,
            retry_of: None,
            reply_to: None,
            expects_reply: None,
            provenance: None,
            content: MessageContent::default(),
            extra: UnknownFields::default(),
        }
    }
}

/// `Message.content` (`v0.9.2 types.ts:36-39`), guarded at
/// `v0.9.2 broker/client.ts:139-149`.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageContent {
    /// The message text (`v0.9.2 broker/client.ts:144`).
    pub text: String,
    /// Optional structured attachments (`v0.9.2 broker/client.ts:148-149`). Every element must pass
    /// `isAttachment`, so a bad one fails the whole message rather than being skipped.
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub attachments: Option<Vec<Attachment>>,
    /// `[UNKNOWN-FIELDS]` + `[MAP-ONLY]` — the content object is spread along with its parent.
    #[serde(flatten)]
    pub extra: UnknownFields,
}

/// `Attachment` (`v0.9.2 types.ts:42-47`), guarded by `isAttachment`
/// (`v0.9.2 broker/client.ts:84-104`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    /// The attachment kind (`v0.9.2 broker/client.ts:91-97`).
    #[serde(rename = "type")]
    pub kind: AttachmentKind,
    /// A display name for the attachment (`v0.9.2 broker/client.ts:99`).
    pub name: String,
    /// The attachment content (`v0.9.2 broker/client.ts:99`).
    pub content: String,
    /// Optional language hint (for a `snippet`) (`v0.9.2 broker/client.ts:103`).
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub language: Option<String>,
    /// `[UNKNOWN-FIELDS]` + `[MAP-ONLY]`.
    #[serde(flatten)]
    pub extra: UnknownFields,
}

/// The three `Attachment.type` values (`v0.9.2 types.ts:43`). A closed vocabulary upstream
/// (`v0.9.2 broker/client.ts:91-97`), so an unknown kind fails the message.
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

/// `MessageReceiptStatus` (`v0.9.2 types.ts:49`) — a closed vocabulary
/// (`isMessageReceiptStatus`, `v0.9.2 broker/client.ts:45-54`,
/// `v0.9.2 broker/broker.ts:96-105`), so `"teleported"` is fatal, not ignorable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageReceiptStatus {
    /// The receiver's transport read the frame.
    ReceiverReceived,
    /// The receiver queued it for a later turn.
    Queued,
    /// The receiver injected it into a turn.
    Injected,
    /// The receiver's agent acknowledged it.
    Acknowledged,
    /// It aged out before delivery.
    Expired,
    /// The sender cancelled it.
    Cancelled,
    /// A newer message replaced it.
    Superseded,
    /// A cancel was requested but not yet effected.
    CancellationRequested,
}

impl MessageReceiptStatus {
    /// The wire spelling — the same string `#[serde(rename_all = "snake_case")]` emits, and the same
    /// value pi's `latestDeliveryState` hands back to the model (`v0.10.1 index.ts:570-576`, where
    /// the status IS the JS string literal). Spelled out rather than derived through
    /// `serde_json::to_string` so the user-visible text cannot change with a serde attribute.
    #[must_use]
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::ReceiverReceived => "receiver_received",
            Self::Queued => "queued",
            Self::Injected => "injected",
            Self::Acknowledged => "acknowledged",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
            Self::Superseded => "superseded",
            Self::CancellationRequested => "cancellation_requested",
        }
    }
}

/// `MessageReceipt` (`v0.9.2 types.ts:51-56`), guarded by `isMessageReceipt`
/// (`v0.9.2 broker/client.ts:56-65`, mirrored at `v0.9.2 broker/broker.ts:107-116`).
///
/// A pi >= 0.9.0 client emits one of these on its very first inbound message —
/// `emitMessageReceipt(id, "receiver_received")` is unconditional and deliberately *not*
/// feature-gated (`v0.9.2 broker/client.ts:773-784`) — so this type is on the hot path for any
/// cyrup broker a pi peer attaches to, whether or not the extension bus is negotiated.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageReceipt {
    /// The message this receipt is about (`v0.9.2 broker/client.ts:61`).
    pub message_id: String,
    /// Where that message got to (`v0.9.2 broker/client.ts:61`).
    pub status: MessageReceiptStatus,
    /// Epoch-ms — `[JS-NUMBER]` (`v0.9.2 broker/client.ts:61`).
    pub timestamp: serde_json::Number,
    /// Free-form detail (`v0.9.2 broker/client.ts:64`).
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub detail: Option<String>,
    /// `[UNKNOWN-FIELDS]` + `[MAP-ONLY]`. The array shape is not hypothetical: a positional
    /// `["m1","queued",1,null]` was confirmed to leave a cyrup broker alive and serving, where pi
    /// bails on `Array.isArray` (`v0.9.2 broker/client.ts:57-59`) and destroys the socket.
    #[serde(flatten)]
    pub extra: UnknownFields,
}

/// `MessageControlAction` (`v0.9.2 types.ts:58`) — closed vocabulary
/// (`v0.9.2 broker/client.ts:75-77`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageControlAction {
    /// The sender withdrew the message.
    Cancel,
    /// A newer message replaces it.
    Supersede,
}

/// `MessageControl` (`v0.9.2 types.ts:60-66`), guarded by `isMessageControl`
/// (`v0.9.2 broker/client.ts:67-82`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageControl {
    /// The message being controlled (`v0.9.2 broker/client.ts:72`).
    pub message_id: String,
    /// What is being done to it (`v0.9.2 broker/client.ts:75-77`).
    pub action: MessageControlAction,
    /// Epoch-ms — `[JS-NUMBER]` (`v0.9.2 broker/client.ts:72`).
    pub timestamp: serde_json::Number,
    /// For `supersede`, the id of the replacement (`v0.9.2 broker/client.ts:78-80`).
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub superseded_by: Option<String>,
    /// Free-form detail (`v0.9.2 broker/client.ts:81`).
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub detail: Option<String>,
    /// `[UNKNOWN-FIELDS]` + `[MAP-ONLY]`.
    #[serde(flatten)]
    pub extra: UnknownFields,
}

/// `ExtensionCapability` (`v0.9.2 types.ts:68-71`), validated by `validateExtensionCapability`
/// (`v0.9.2 broker/broker.ts:1159-1168`).
///
/// cyrup never *advertises* a capability, but it must still validate one: pi runs this guard before
/// any bus effect, on both `register` (`v0.9.2 broker/broker.ts:446-456`) and
/// `extension_capabilities_update` (`:559-567`), and every failure is a `throw` → `socket.destroy`.
/// Ignoring a well-formed frame is a survivability choice; ignoring a malformed one would be an
/// input-validation hole. The namespace *pattern* half of the guard lives with its caller in
/// [`crate::broker`], since it is a broker rule rather than a wire shape.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionCapability {
    /// The namespace this session speaks (`v0.9.2 broker/broker.ts:1164`).
    pub namespace: String,
    /// Whether it is willing to own the namespace (`v0.9.2 broker/broker.ts:1164`).
    pub owner_eligible: bool,
    /// `[MAP-ONLY]` — without it `[["ns", true]]` would fill this struct positionally, and pi
    /// rejects it because `[]["namespace"]` is `undefined`.
    #[serde(flatten)]
    pub extra: UnknownFields,
}

/// The `ownerId`/`ownerEpoch` pair carried by `extension_owner` and `extension_message`.
///
/// Split out because upstream's guard is a **cross-field** one: `hasOwnerId !== hasOwnerEpoch`
/// throws (`v0.9.2 broker/client.ts:541-548`, `:557-565`), so "an owner id with no epoch" is
/// exactly as fatal as "an owner id that is a number". A pair of independent `Option<String>` fields
/// cannot express that; flattening one type that owns both invariants can.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionOwnerRef {
    /// The owning session's id — present iff [`Self::owner_epoch`] is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    /// The ownership epoch — present iff [`Self::owner_id`] is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_epoch: Option<String>,
}

impl<'de> serde::Deserialize<'de> for ExtensionOwnerRef {
    /// Both-or-neither, plus `[NON-NULL]` on each half.
    ///
    /// Written as a wrapper around a derived struct rather than as a hand-rolled visitor so the
    /// field-name/`null` handling stays the same code every other payload uses; only the pairing
    /// rule is added on top.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire {
            #[serde(default, deserialize_with = "present_non_null")]
            owner_id: Option<String>,
            #[serde(default, deserialize_with = "present_non_null")]
            owner_epoch: Option<String>,
        }
        let wire = Wire::deserialize(deserializer)?;
        match (wire.owner_id, wire.owner_epoch) {
            (None, None) => Ok(Self {
                owner_id: None,
                owner_epoch: None,
            }),
            (Some(owner_id), Some(owner_epoch)) => Ok(Self {
                owner_id: Some(owner_id),
                owner_epoch: Some(owner_epoch),
            }),
            _ => Err(D::Error::custom(
                "ownerId and ownerEpoch must both be present or both be absent",
            )),
        }
    }
}

/// The `extension_publish` audience (`v0.9.2 types.ts:90`), checked at
/// `v0.9.2 broker/broker.ts:1293-1296`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExtensionAudience {
    /// Only the namespace owner.
    Owner,
    /// Every session that advertised the namespace.
    Capable,
}

/// `SessionRegistration = Omit<SessionInfo, "id" | "peerUid" | "trustedLocal"> & { extensions? }`
/// (`v0.9.2 types.ts:73-75`), guarded by `isSessionRegistration`
/// (`v0.9.2 broker/broker.ts:190-212`).
///
/// **The context trio is deliberately not modelled here.** `isSessionRegistration` checks only
/// `cwd`/`model`/`pid`/`startedAt`/`lastActivity`/`name`/`status` (`:197-211`), and the `SessionInfo`
/// the broker builds from it is a whitelist that never copies them
/// (`v0.9.2 broker/broker.ts:472-482`). So `contextPct: "not-a-number"` is *accepted and dropped*
/// upstream, and giving those fields the `isSessionInfo` treatment here would make cyrup stricter
/// than pi on the one frame every session sends first. They land in [`Self::extra`] and go no
/// further.
///
/// `extensions` is likewise unmodelled and reaches [`crate::broker`] through [`Self::extra`] — the
/// same place pi reads it from, since pi does not put it on `SessionInfo` either
/// (`v0.9.2 broker/broker.ts:484-490` puts it on `ConnectedSession`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRegistration {
    /// Optional presence name (`v0.9.2 broker/broker.ts:207-209`).
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub name: Option<String>,
    /// Whether `name` is a synthesized unnamed-runtime alias (`v0.10.1 types.ts:6-7`), carried on
    /// the registration by `buildRegistration`'s `...identity` spread
    /// (`v0.10.1 index.ts:772-774`) and copied onto the stored `SessionInfo` at
    /// `v0.10.1 broker/broker.ts:358`.
    ///
    /// Unmodelled by `isSessionRegistration` upstream (it validates only the pre-v0.10 keys), so a
    /// bad type is accepted-and-dropped there; modelled here because cyrup both SENDS it and copies
    /// it into the stored `SessionInfo`, and `present_non_null` keeps a `null` from being a decode
    /// failure.
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub runtime_fallback_alias: Option<bool>,
    /// The session's working directory (`v0.9.2 broker/broker.ts:198`).
    pub cwd: String,
    /// The session's active model ref (`v0.9.2 broker/broker.ts:199`).
    pub model: String,
    /// The session's OS pid — `[JS-NUMBER]` (`v0.9.2 broker/broker.ts:200`).
    pub pid: serde_json::Number,
    /// Epoch-ms session start time — `[JS-NUMBER]` (`v0.9.2 broker/broker.ts:201`).
    pub started_at: serde_json::Number,
    /// Epoch-ms of the most recent activity — `[JS-NUMBER]` (`v0.9.2 broker/broker.ts:202`).
    pub last_activity: serde_json::Number,
    /// Optional lifecycle status string (`v0.9.2 broker/broker.ts:211`).
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub status: Option<String>,
    /// `[UNKNOWN-FIELDS]` + `[MAP-ONLY]`. Carries `extensions` and the context trio, both of which
    /// upstream reads (or ignores) without modelling them on this type. The `[MAP-ONLY]` half is
    /// upstream's own explicit `Array.isArray(value)` bail (`v0.9.2 broker/broker.ts:191-193`).
    /// `tmuxPane` (`v0.12.0 types.ts:42`), carried on the registration by `buildRegistration`'s
    /// `...(tmuxPane ? { tmuxPane } : {})` spread (`v0.12.0 index.ts:900`) and copied onto the
    /// stored `SessionInfo` at `v0.12.0 broker/broker.ts:475`.
    ///
    /// `[NON-NULL]`, and unlike `runtimeFallbackAlias` this one IS validated by
    /// `isSessionRegistration` upstream (`v0.12.0 broker/protocol.ts:203`) — so modelling it with
    /// [`present_non_null`] puts cyrup neither looser nor stricter than pi: a non-string destroys
    /// the connection on both sides.
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub tmux_pane: Option<String>,
    #[serde(flatten)]
    pub extra: UnknownFields,
}

/// An intercom routing scope — the broker's opaque isolation boundary (`scopeId`,
/// `v0.13.0 types.ts:107`), resolved from `CYRUP_INTERCOM_SCOPE_ID` by
/// [`crate::config::intercom_scope_id`] and carried on the `register` frame only.
///
/// **Parsed, never validated after the fact.** The inner string is private and the only
/// constructor is [`ScopeId::parse`], which is `normalizeScopeId`'s trim
/// (`v0.13.0 broker/broker.ts:140-141`) and `getIntercomScopeId`'s `?.trim()` truthiness test
/// (`v0.13.0 config.ts:22-23`) in one place. That makes two states unrepresentable rather than
/// merely unlikely: an UNTRIMMED scope, which would silently split one intended boundary in two
/// (`"a"` and `" a"` are different map keys), and a BLANK scope, which would be an isolation class
/// no unscoped session could reach — the exact inverse of the opt-in guarantee, since upstream's
/// `trimmed ? trimmed : undefined` makes whitespace-only mean *unscoped*.
///
/// `Option<ScopeId>` is therefore the whole domain: `None` is the unscoped class every session
/// registers into today, and `sameScope` (`:144-146`, a plain `===` over `string | undefined`) is
/// the derived `PartialEq`.
///
/// There is deliberately no `Deserialize`: a wire value reaches this type only through
/// [`scope_id_field`], which applies the same parse.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
#[serde(transparent)]
pub struct ScopeId(String);

impl ScopeId {
    /// `normalizeScopeId`'s non-fatal half (`v0.13.0 broker/broker.ts:140-141`) and the whole of
    /// `getIntercomScopeId` (`v0.13.0 config.ts:21-24`): trim, and treat the empty result as
    /// UNSCOPED rather than as an error.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(Self(trimmed.to_string()))
        }
    }

    /// The normalized scope string, as it appears on the register frame.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// `normalizeScopeId(value)` (`v0.13.0 broker/broker.ts:133-142`) as a serde field deserializer,
/// for [`ClientMessage::Register`]'s `scopeId`.
///
/// All three of upstream's arms, in upstream's order: an ABSENT key is `undefined` → unscoped
/// (supplied by `#[serde(default)]`, so this function never runs for it); a PRESENT-but-not-a-string
/// value — an explicit `null` included, since `typeof null !== "string"` — is a `throw`, i.e. a
/// decode error, because a malformed scope must never silently degrade to "global"; and a
/// whitespace-only string trims to unscoped and is *not* fatal.
///
/// # Errors
/// Propagates serde's error for a present value that is not a string.
fn scope_id_field<'de, D>(deserializer: D) -> Result<Option<ScopeId>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    Ok(ScopeId::parse(&raw))
}

/// `DeliveryState` (`v0.13.0 types.ts:4`) — the crate-facing union of both acks' states.
///
/// ICOM-054. Kept wide because [`crate::transport::client::SendResult`] reports one value for
/// either ack; the two ack frames themselves are narrower and say so in their own types
/// ([`DeliveredState`], [`FailedState`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    /// Handed to a live target's socket.
    SocketDelivered,
    /// Parked in the broker mailbox for a disconnected target.
    Queued,
    /// Refused; the accompanying code says why.
    Failed,
    /// The outcome is genuinely not known. The BROKER never emits this — it is what
    /// [`crate::transport::client`] reports when a connection drops with sends in flight, the one
    /// case pi answers by rejecting the promise instead of resolving it.
    Unknown,
}

/// The `delivered` ack's acceptance set (`v0.13.0 broker/client.ts:375-386`): `socket_delivered` or
/// `queued` ONLY — a `delivered` frame carrying `"failed"` throws upstream, i.e. destroys the
/// connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveredState {
    /// Handed to a live target's socket.
    SocketDelivered,
    /// Parked in the broker mailbox for a disconnected target.
    Queued,
}

/// The `delivery_failed` ack's acceptance set (`v0.13.0 broker/client.ts:392-403`): `failed` or
/// `unknown` ONLY.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailedState {
    /// Refused by the broker.
    Failed,
    /// Outcome unknown.
    Unknown,
}

impl DeliveryState {
    /// The wire spelling, for the places the crate reports a delivery state inside prose
    /// (`latestDeliveryState`'s fallback, `v0.13.0 index.ts:2452`).
    #[must_use]
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::SocketDelivered => "socket_delivered",
            Self::Queued => "queued",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }
}

impl From<DeliveredState> for DeliveryState {
    fn from(value: DeliveredState) -> Self {
        match value {
            DeliveredState::SocketDelivered => Self::SocketDelivered,
            DeliveredState::Queued => Self::Queued,
        }
    }
}

impl From<FailedState> for DeliveryState {
    fn from(value: FailedState) -> Self {
        match value {
            FailedState::Failed => Self::Failed,
            FailedState::Unknown => Self::Unknown,
        }
    }
}

/// The optional exact-target pair on `send` (`v0.13.0 types.ts:111`): bind this send to one exact
/// live endpoint, so a broker that has since replaced that endpoint refuses it instead of routing
/// the message to whatever the NAME resolves to now.
///
/// ICOM-054. The both-or-neither rule is enforced by the **constructors**, not by `Deserialize`,
/// and that split is upstream's rather than a shortcut. pi checks `ownerId`/`ownerEpoch` inside a
/// type guard, so a half-set pair destroys the connection ([`ExtensionOwnerRef`] reproduces that);
/// it checks `targetId`/`targetEpoch` inside `case "send"` (`v0.13.0 broker/broker.ts:624-633`) and
/// answers a half-set, non-string or empty-string pair with a `delivery_failed` carrying
/// `E_INVALID_TARGET`. Rejecting it here would make cyrup fatally stricter than pi on a frame pi
/// merely refuses, so the decode stays permissive and the rule lives in [`crate::broker`]'s send
/// handler, where pi puts it.
///
/// The fields are private and the only two ways to build one in Rust are [`Self::default`]
/// (neither) and [`Self::bound`] (both), so a cyrup client cannot EMIT a half-set pair; the single
/// reader, [`Self::as_pair`], hands back both halves or nothing, so no caller can use one half
/// alone.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExactTarget {
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    target_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    target_epoch: Option<String>,
}

impl ExactTarget {
    /// Bind a send to `target_id` at exactly `target_epoch`
    /// (`{ targetId, targetEpoch }`, `v0.13.0 broker/client.ts:653`).
    #[must_use]
    pub fn bound(target_id: impl Into<String>, target_epoch: impl Into<String>) -> Self {
        Self {
            target_id: Some(target_id.into()),
            target_epoch: Some(target_epoch.into()),
        }
    }

    /// Both halves, or `None` when neither is set — the only reader.
    ///
    /// A decoded half-set pair (which only a non-cyrup peer can produce) also answers `None`; the
    /// broker distinguishes that case from "absent" by looking at the raw frame, exactly as
    /// upstream's `hasTargetId !== hasTargetEpoch` does.
    #[must_use]
    pub fn as_pair(&self) -> Option<(&str, &str)> {
        Some((self.target_id.as_deref()?, self.target_epoch.as_deref()?))
    }
}

/// Client → broker messages (`v0.9.2 types.ts:77-101`).
///
/// The full v0.9.2 tag set is modelled even where cyrup neither sends nor acts on a tag, because
/// the union is also the *acceptance* set: a tag missing from here is an `unknown variant` decode
/// error, i.e. a destroyed connection, for a frame a conforming pi peer sends as a matter of course.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ClientMessage {
    /// Register a session (optionally re-adopting a stable `session_id`)
    /// (`v0.9.2 types.ts:78`, handled at `v0.9.2 broker/broker.ts:429-533`).
    Register {
        /// The session's registration payload.
        session: SessionRegistration,
        /// A stable session id to re-adopt (broker takeover), if any. Must be a non-blank string
        /// when present (`isSessionId`, `v0.9.2 broker/broker.ts:186-188`).
        #[serde(
            default,
            deserialize_with = "present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        session_id: Option<String>,
        /// The opt-in-TCP endpoint credential (`stateId`, `v0.9.2 broker/client.ts:360`): the
        /// broker's per-run `BROKER_STATE_ID` from `broker.port.json`. Required over TCP
        /// (`v0.9.2 broker/broker.ts:420-422` otherwise throws
        /// `Invalid intercom TCP endpoint credentials`), and **omitted** — never null — over a Unix
        /// socket / named pipe. Filled in by
        /// [`crate::transport::client::IntercomClient::connect_target`] from the resolved
        /// [`crate::transport::target::BrokerConnectTarget`].
        #[serde(
            default,
            deserialize_with = "present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        state_id: Option<String>,
        /// The broker routing scope this session registers into (`scopeId`,
        /// `v0.13.0 types.ts:107`, filled in from `CYRUP_INTERCOM_SCOPE_ID` by
        /// [`crate::transport::client::IntercomClient::connect_target`], mirroring
        /// `...(scopeId ? { scopeId } : {})` at `v0.13.0 broker/client.ts:286-292`).
        ///
        /// **Absent — never null, never blank — for an unscoped session**, which is what keeps an
        /// unscoped register frame byte-identical to the pre-scope one and therefore keeps a
        /// scope-aware broker wire-compatible with an older client and vice versa. Normalized on
        /// the way in by [`scope_id_field`], this crate's `normalizeScopeId`
        /// (`v0.13.0 broker/broker.ts:133-142`).
        ///
        /// The broker does not enforce anything through THIS field: it reads raw
        /// `serde_json::Value` frames and never deserializes a `ClientMessage` (see
        /// `broker/js.rs`), so the enforcement copy lives at the register handler. This variant is
        /// the crate's statement of the wire shape, and the `skip_serializing_if` is what the write
        /// path depends on.
        #[serde(
            default,
            deserialize_with = "scope_id_field",
            skip_serializing_if = "Option::is_none"
        )]
        scope_id: Option<ScopeId>,
    },
    /// Unregister this session (`v0.9.2 types.ts:79`).
    Unregister,
    /// Re-advertise this session's extension-bus capabilities (`v0.9.2 types.ts:80`).
    ExtensionCapabilitiesUpdate {
        /// At most `MAX_EXTENSIONS_PER_SESSION` entries, each valid
        /// (`v0.9.2 broker/broker.ts:559-567`).
        extensions: Vec<ExtensionCapability>,
    },
    /// List all connected sessions (`v0.9.2 types.ts:81`).
    List {
        /// Correlation id echoed back on the `sessions` response.
        request_id: String,
    },
    /// Send a message to a target (name / id / unique-prefix) (`v0.9.2 types.ts:82`).
    Send {
        /// Target name or id.
        to: String,
        /// The message to deliver.
        message: Message,
        /// Bind this send to one exact live endpoint — both halves or neither
        /// (`v0.13.0 types.ts:111`). [`ExactTarget::default`] serialises to nothing, which is the
        /// v0.9.2 frame byte-for-byte, so an unbound send is unchanged on the wire.
        #[serde(flatten)]
        target: ExactTarget,
    },
    /// Report what happened to a message this session received (`v0.9.2 types.ts:83`). A pi >= 0.9.0
    /// client sends this unprompted on its first inbound message.
    MessageReceipt {
        /// The receipt.
        receipt: MessageReceipt,
    },
    /// Withdraw a message this session sent (`v0.9.2 types.ts:84`).
    CancelMessage {
        /// The message id.
        message_id: String,
    },
    /// Cancel an outstanding ask edge this session owns (`v0.9.2 types.ts:85`).
    CancelAsk {
        /// The ask's message id.
        message_id: String,
    },
    /// Update this session's presence, coalesced by the broker (`v0.9.2 types.ts:86`, applied at
    /// `v0.9.2 broker/broker.ts:884-959`).
    ///
    /// The context trio is the `[NON-NULL]` exception, and it is the only one. Upstream types them
    /// `number | null` and gives each state a distinct meaning
    /// (`v0.9.2 broker/broker.ts:921-950`): absent leaves the stored field untouched, `null`
    /// **clears** it — the right thing right after a compaction, when the value is unknown and
    /// carrying the stale-high one forward would be a lie — and a number sets it. Only a value that
    /// is neither throws (`:924`, `:934`, `:944`). Reusing `isSessionInfo`'s stricter rule here
    /// would disconnect a peer pi serves.
    Presence {
        /// New presence name, if changed (`v0.9.2 broker/broker.ts:891-894`).
        #[serde(
            default,
            deserialize_with = "present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        name: Option<String>,
        /// Whether the accompanying `name` is a synthesized unnamed-runtime alias
        /// (`v0.10.1 types.ts:88`, applied at `v0.10.1 broker/broker.ts:779-787`).
        #[serde(
            default,
            deserialize_with = "present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        runtime_fallback_alias: Option<bool>,
        /// New status string, if changed (`v0.9.2 broker/broker.ts:900-903`).
        #[serde(
            default,
            deserialize_with = "present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        status: Option<String>,
        /// New model ref, if changed (`v0.9.2 broker/broker.ts:909-912`).
        #[serde(
            default,
            deserialize_with = "present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        model: Option<String>,
        /// Context-usage percentage: `None` omits, `Some(None)` clears, `Some(n)` sets
        /// (`v0.9.2 broker/broker.ts:921-930`).
        #[serde(
            default,
            deserialize_with = "present_nullable",
            skip_serializing_if = "Option::is_none"
        )]
        context_pct: Option<Option<serde_json::Number>>,
        /// Context token count, same tri-state (`v0.9.2 broker/broker.ts:931-940`).
        #[serde(
            default,
            deserialize_with = "present_nullable",
            skip_serializing_if = "Option::is_none"
        )]
        context_tokens: Option<Option<serde_json::Number>>,
        /// Context window size, same tri-state (`v0.9.2 broker/broker.ts:941-950`).
        #[serde(
            default,
            deserialize_with = "present_nullable",
            skip_serializing_if = "Option::is_none"
        )]
        context_window: Option<Option<serde_json::Number>>,
    },
    /// Publish an extension-bus payload (`v0.9.2 types.ts:87-94`).
    ExtensionPublish {
        /// The target namespace.
        namespace: String,
        /// Who receives it.
        audience: ExtensionAudience,
        /// The sender's claimed ownership epoch, when it claims to be the owner.
        #[serde(
            default,
            deserialize_with = "present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        owner_epoch: Option<String>,
        /// Restrict delivery to the owner.
        #[serde(
            default,
            deserialize_with = "present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        owner_only: Option<bool>,
        /// Opaque body (`unknown` upstream — never type-checked).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payload: Option<serde_json::Value>,
    },
    /// Compare-and-swap the namespace's shared state (`v0.9.2 types.ts:95-101`).
    ExtensionStateCommit {
        /// The target namespace.
        namespace: String,
        /// The committer's ownership epoch (`v0.9.2 broker/broker.ts:1406`).
        owner_epoch: String,
        /// The revision the committer believes is current — safe-integer bounded upstream
        /// (`v0.9.2 broker/broker.ts:1417`).
        #[serde(deserialize_with = "js_safe_integer")]
        expected_revision: u64,
        /// Opaque body.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payload: Option<serde_json::Value>,
    },
}

/// Broker → client messages (`v0.9.2 types.ts:103-136`), each guarded by its own arm of
/// `handleBrokerMessage` (`v0.9.2 broker/client.ts:385-601`).
// `Message` is THE dominant variant — every delivered message arrives through it — and it grew
// again with ICOM-056's `provenance`. Boxing it would force `box`-patterns (unstable) at every
// match site for a payload that dominates allocations anyway; the same tradeoff, and the same
// decision, as `cyrup-session/src/entry.rs:50` and `:217`.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum BrokerMessage {
    /// Registration acknowledged; carries the broker-assigned session id
    /// (`v0.9.2 types.ts:104`, `v0.9.2 broker/client.ts:386-411`).
    Registered {
        /// The assigned session id.
        session_id: String,
        /// The features this broker negotiated — in practice the single
        /// [`EXTENSION_BUS_FEATURE`]. Absent is legal and is what cyrup always sends;
        /// `null` is not, and neither is a non-string element
        /// (`v0.9.2 broker/client.ts:395-400`).
        #[serde(
            default,
            deserialize_with = "present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        features: Option<Vec<String>>,
    },
    /// A `list` response (`v0.9.2 broker/client.ts:414-428`).
    Sessions {
        /// The correlation id from the request.
        request_id: String,
        /// The connected sessions — every element must pass `isSessionInfo` (`:416`).
        sessions: Vec<SessionInfo>,
    },
    /// An inbound message routed from another session (`v0.9.2 broker/client.ts:431-439`).
    Message {
        /// The sender's session info.
        from: SessionInfo,
        /// The delivered message.
        message: Message,
    },
    /// A presence change broadcast (`v0.9.2 broker/client.ts:515-524`).
    PresenceUpdate {
        /// The updated session info.
        session: SessionInfo,
    },
    /// A session joined (`v0.9.2 broker/client.ts:493-502`).
    SessionJoined {
        /// The joined session info.
        session: SessionInfo,
    },
    /// A session left (`v0.9.2 broker/client.ts:504-513`).
    SessionLeft {
        /// The departed session id.
        session_id: String,
    },
    /// A broker-level error for this connection (`v0.9.2 broker/client.ts:526-536`).
    Error {
        /// The error text.
        error: String,
    },
    /// A `send` was delivered (`v0.9.2 broker/client.ts:441-456`; `DeliveryDetails` spread added at
    /// v0.11.0, `v0.13.0 types.ts:140`).
    ///
    /// ICOM-054: all four detail fields are OPTIONAL on the wire, because upstream's own
    /// `cancel_message` arms answer with a BARE `{ type: "delivered", messageId }`
    /// (`v0.13.0 broker/broker.ts:829,856`) and because a pre-v0.11.0 broker emits none. The
    /// client's defaults (`:386`) fill them in.
    Delivered {
        /// The delivered message id.
        message_id: String,
        /// Absent ⇒ `socket_delivered` (`v0.13.0 broker/client.ts:386`).
        #[serde(
            default,
            deserialize_with = "present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        delivery: Option<DeliveredState>,
        /// A broker code; never set on a success in practice, modelled because
        /// `DeliveryDetails` carries it on both acks.
        #[serde(
            default,
            deserialize_with = "present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        code: Option<String>,
        /// Absent ⇒ `false` (`v0.13.0 broker/client.ts:388`).
        #[serde(
            default,
            deserialize_with = "present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        retryable: Option<bool>,
        /// Absent ⇒ `true` (`v0.13.0 broker/client.ts:389`).
        #[serde(
            default,
            deserialize_with = "present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        outcome_known: Option<bool>,
    },
    /// A `send` could not be delivered (`v0.9.2 broker/client.ts:458-473`; `DeliveryDetails` spread
    /// added at v0.11.0, `v0.13.0 types.ts:141`).
    DeliveryFailed {
        /// The message id that failed.
        message_id: String,
        /// The failure reason.
        reason: String,
        /// Absent ⇒ `failed` (`v0.13.0 broker/client.ts:403`).
        #[serde(
            default,
            deserialize_with = "present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        delivery: Option<FailedState>,
        /// The machine-readable code, e.g. `E_TARGET_REBOUND` — what
        /// [`crate::transport::client::IntercomClient::send`] keys its single retry on.
        #[serde(
            default,
            deserialize_with = "present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        code: Option<String>,
        /// Absent ⇒ `false`.
        #[serde(
            default,
            deserialize_with = "present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        retryable: Option<bool>,
        /// Absent ⇒ `true`.
        #[serde(
            default,
            deserialize_with = "present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        outcome_known: Option<bool>,
    },
    /// A receipt forwarded back to the original sender (`v0.9.2 types.ts:113`, guarded at
    /// `v0.9.2 broker/client.ts:475-482`).
    MessageReceipt {
        /// The reporting session.
        from: SessionInfo,
        /// The receipt.
        receipt: MessageReceipt,
    },
    /// A cancel/supersede notice forwarded to the receiver (`v0.9.2 types.ts:114`, guarded at
    /// `v0.9.2 broker/client.ts:484-491`).
    MessageControl {
        /// The controlling session.
        from: SessionInfo,
        /// The control.
        control: MessageControl,
    },
    /// The current owner of an extension namespace (`v0.9.2 types.ts:115`, guarded at
    /// `v0.9.2 broker/client.ts:538-552`).
    ExtensionOwner {
        /// The namespace.
        namespace: String,
        /// The owner pair — both present (bound) or both absent (unowned), never one of each.
        #[serde(flatten)]
        owner: ExtensionOwnerRef,
    },
    /// An extension-bus payload routed to this session (`v0.9.2 types.ts:116-123`, guarded at
    /// `v0.9.2 broker/client.ts:554-569`).
    ExtensionMessage {
        /// The namespace.
        namespace: String,
        /// The publishing session.
        from_session_id: String,
        /// The owner pair, on the same both-or-neither terms as `extension_owner`.
        #[serde(flatten)]
        owner: ExtensionOwnerRef,
        /// Opaque body. **Absent is legal**: pi's guard (`:557-565`) never mentions `payload`, so
        /// requiring it would destroy a connection upstream serves.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payload: Option<serde_json::Value>,
    },
    /// A namespace's shared state (`v0.9.2 types.ts:124-129`, guarded at
    /// `v0.9.2 broker/client.ts:571-582`).
    ExtensionState {
        /// The namespace.
        namespace: String,
        /// The state revision — `Number.isSafeInteger` and `>= 0` (`:574-575`).
        #[serde(deserialize_with = "js_safe_integer")]
        revision: u64,
        /// Opaque body; absent is legal for the same reason as on `extension_message`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payload: Option<serde_json::Value>,
    },
    /// The outcome of an `extension_state_commit` (`v0.9.2 types.ts:130-136`, guarded at
    /// `v0.9.2 broker/client.ts:584-597`).
    ExtensionStateResult {
        /// The namespace.
        namespace: String,
        /// Whether the compare-and-swap took effect.
        committed: bool,
        /// The revision now current — `Number.isSafeInteger` and `>= 0` (`:588-589`).
        #[serde(deserialize_with = "js_safe_integer")]
        revision: u64,
        /// Why it was refused (`:590`).
        #[serde(
            default,
            deserialize_with = "present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        reason: Option<String>,
    },
}

impl BrokerMessage {
    /// `writeDeliverySuccess` (`v0.13.0 broker/broker.ts:1035-1037`) — a full `delivered` ack.
    #[must_use]
    pub fn delivered(message_id: impl Into<String>, delivery: DeliveredState) -> Self {
        Self::Delivered {
            message_id: message_id.into(),
            delivery: Some(delivery),
            code: None,
            retryable: Some(false),
            outcome_known: Some(true),
        }
    }

    /// The BARE `delivered` ack `cancel_message` answers with
    /// (`writeMessage(socket, { type: "delivered", messageId })`,
    /// `v0.13.0 broker/broker.ts:829,856`).
    ///
    /// `636f61e` rewrote every OTHER `delivered` through `writeDeliverySuccess` and deliberately
    /// left these two alone, so the client's absent-field defaults are what supply the details.
    /// Reproducing that asymmetry is why the four fields are optional on the wire.
    #[must_use]
    pub fn delivered_bare(message_id: impl Into<String>) -> Self {
        Self::Delivered {
            message_id: message_id.into(),
            delivery: None,
            code: None,
            retryable: None,
            outcome_known: None,
        }
    }

    /// `writeDeliveryFailure` (`v0.13.0 broker/broker.ts:1039-1041`), whose `retryable` parameter
    /// defaults to `false`.
    #[must_use]
    pub fn delivery_failed(
        message_id: impl Into<String>,
        reason: impl Into<String>,
        code: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self::DeliveryFailed {
            message_id: message_id.into(),
            reason: reason.into(),
            delivery: Some(FailedState::Failed),
            code: Some(code.into()),
            retryable: Some(retryable),
            outcome_known: Some(true),
        }
    }

    /// The BARE `delivery_failed` `cancel_message` answers an unowned message with
    /// (`v0.13.0 broker/broker.ts:835-839`), which `636f61e` also left without a code.
    #[must_use]
    pub fn delivery_failed_bare(message_id: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::DeliveryFailed {
            message_id: message_id.into(),
            reason: reason.into(),
            delivery: None,
            code: None,
            retryable: None,
            outcome_known: None,
        }
    }
}

/// The health handshake (`v0.9.2 broker/spawn.ts:104-113,302-306`) — NOT in the TS
/// `ClientMessage`/`BrokerMessage` unions; used only by discovery (`transport::spawn`) and answered
/// by the broker (`v0.9.2 broker/broker.ts:404-417`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum HealthMessage {
    /// A health probe.
    Health {
        /// The probe correlation id.
        request_id: String,
        /// The opt-in-TCP endpoint credential (`stateId`, `v0.9.2 broker/spawn.ts:305`), on the same
        /// terms as `register`'s: required over TCP (`v0.9.2 broker/broker.ts:408-410`), omitted
        /// over a socket / pipe. Filled in by
        /// [`crate::transport::spawn::check_target_connectable`].
        #[serde(
            default,
            deserialize_with = "present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        state_id: Option<String>,
    },
    /// The broker's health response (`{type:"health_ok", requestId, protocol, version}`,
    /// `v0.9.2 broker/broker.ts:411-416`), checked by `isBrokerHealthOkMessage`
    /// (`v0.9.2 broker/spawn.ts:109-112`).
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
                runtime_fallback_alias: None,
                name: Some("alice".to_string()),
                cwd: "/w".to_string(),
                model: "m".to_string(),
                pid: 42u64.into(),
                started_at: 1u64.into(),
                last_activity: 2u64.into(),
                status: None,
                tmux_pane: None,
                extra: UnknownFields::default(),
            },
            session_id: Some("sess-1".to_string()),
            state_id: None,
            scope_id: None,
        };
        let v: serde_json::Value = serde_json::to_value(&msg).unwrap();
        assert_eq!(v["type"], "register");
        assert_eq!(v["sessionId"], "sess-1");
        assert_eq!(v["session"]["startedAt"], 1);
        assert_eq!(v["session"]["lastActivity"], 2);
        // state_id omitted when None.
        assert!(v.get("stateId").is_none());
        // ICOM-055 — and `scopeId` likewise, which is half of the opt-in guarantee: an unscoped
        // register frame must be byte-identical to the pre-scope one
        // (`...(scopeId ? { scopeId } : {})`, `v0.13.0 broker/client.ts:291`).
        assert!(v.get("scopeId").is_none());
    }

    /// ICOM-055 — `normalizeScopeId` (`v0.13.0 broker/broker.ts:133-142`) as this crate's
    /// [`scope_id_field`], on the one path that actually deserializes a `ClientMessage`.
    ///
    /// Three arms, upstream's: absent is unscoped, whitespace-only trims TO unscoped without being
    /// fatal, and a present non-string (an explicit `null` included, since
    /// `typeof null !== "string"`) is a decode error — upstream's `throw`, which the broker turns
    /// into `socket.destroy`. A malformed scope must never quietly become "global".
    #[test]
    fn register_scope_id_is_normalized_and_a_non_string_is_a_decode_error() {
        let base = serde_json::json!({
            "type": "register",
            "session": { "cwd": "/w", "model": "m", "pid": 1, "startedAt": 0, "lastActivity": 0 },
        });
        let with = |scope: serde_json::Value| {
            let mut v = base.clone();
            v["scopeId"] = scope;
            serde_json::from_value::<ClientMessage>(v)
        };
        // Extracted rather than matched-with-a-panic: this module's test block allows
        // `unwrap`/`expect` but the crate denies `panic!` everywhere (arch-00 §8).
        let scope_of = |msg: ClientMessage| match msg {
            ClientMessage::Register { scope_id, .. } => Some(scope_id),
            _ => None,
        };
        assert_eq!(
            scope_of(serde_json::from_value::<ClientMessage>(base.clone()).unwrap())
                .expect("a register frame"),
            None,
            "an absent scopeId is unscoped"
        );
        assert_eq!(
            scope_of(with(serde_json::json!("  alpha  ")).unwrap())
                .expect("a register frame")
                .as_ref()
                .map(ScopeId::as_str),
            Some("alpha"),
            "a present scopeId is trimmed"
        );
        assert_eq!(
            scope_of(with(serde_json::json!("   ")).unwrap()).expect("a register frame"),
            None,
            "a whitespace-only scopeId is unscoped, NOT an error"
        );
        assert!(with(serde_json::json!(7)).is_err(), "a number is fatal");
        assert!(
            with(serde_json::json!(null)).is_err(),
            "an explicit null is fatal"
        );
    }

    /// ICOM-055 — a scoped register frame carries the trimmed scope and nothing else changes.
    #[test]
    fn a_scoped_register_frame_carries_the_normalized_scope() {
        let msg = ClientMessage::Register {
            session: SessionRegistration {
                runtime_fallback_alias: None,
                name: None,
                cwd: "/w".to_string(),
                model: "m".to_string(),
                pid: 1u64.into(),
                started_at: 0u64.into(),
                last_activity: 0u64.into(),
                status: None,
                tmux_pane: None,
                extra: UnknownFields::default(),
            },
            session_id: None,
            state_id: None,
            scope_id: ScopeId::parse(" alpha "),
        };
        let v: serde_json::Value = serde_json::to_value(&msg).unwrap();
        assert_eq!(v["scopeId"], "alpha");
    }

    /// ICOM-054 — an unbound `send` must be the v0.9.2 frame BYTE-FOR-BYTE, or every pre-v0.11.0
    /// broker sees a shape it did not agree to. [`ExactTarget::default`] serialises to nothing
    /// because both halves are `skip_serializing_if = "Option::is_none"`.
    #[test]
    fn an_unbound_send_frame_carries_no_exact_target_keys() {
        let message: Message = serde_json::from_value(serde_json::json!({
            "id": "m1", "timestamp": 1, "content": { "text": "hi" },
        }))
        .expect("decodes");
        let unbound = serde_json::to_value(ClientMessage::Send {
            to: "peer".to_string(),
            message: message.clone(),
            target: ExactTarget::default(),
        })
        .expect("encodes");
        assert_eq!(
            unbound
                .as_object()
                .map(|o| o.keys().map(String::as_str).collect::<Vec<_>>()),
            // `serde_json::Map` is a `BTreeMap` in this build, so the assertion is on the KEY SET,
            // which is what matters: no `targetId`, no `targetEpoch`.
            Some(vec!["message", "to", "type"]),
        );

        let bound = serde_json::to_value(ClientMessage::Send {
            to: "peer".to_string(),
            message,
            target: ExactTarget::bound("peer-id", "epoch-1"),
        })
        .expect("encodes");
        assert_eq!(bound["targetId"], "peer-id");
        assert_eq!(bound["targetEpoch"], "epoch-1");
    }

    /// ICOM-054 — the pairing rule lives in the CONSTRUCTORS, not in `Deserialize`.
    ///
    /// A half-set pair off the wire decodes (pi answers it with `E_INVALID_TARGET` rather than
    /// destroying the connection, `v0.13.0 broker/broker.ts:624-633`) but reads back as "no exact
    /// target", so no caller can act on one half. There is no way to BUILD a half-set one in Rust.
    #[test]
    fn a_half_set_exact_target_decodes_but_never_reads_back_as_a_pair() {
        let half: ExactTarget =
            serde_json::from_value(serde_json::json!({ "targetId": "peer-id" })).expect("decodes");
        assert_eq!(half.as_pair(), None);
        assert_eq!(ExactTarget::default().as_pair(), None);
        assert_eq!(
            ExactTarget::bound("peer-id", "epoch-1").as_pair(),
            Some(("peer-id", "epoch-1"))
        );
        // `[NON-NULL]` still holds on each half.
        assert!(
            serde_json::from_value::<ExactTarget>(
                serde_json::json!({ "targetId": null, "targetEpoch": "e" })
            )
            .is_err()
        );
    }

    /// ICOM-054 — the two ack acceptance sets are ASYMMETRIC upstream
    /// (`v0.13.0 broker/client.ts:375,392`) and the two narrow wire enums encode that: `"failed"`
    /// on a `delivered` frame, or `"queued"` on a `delivery_failed` frame, is a decode error here
    /// exactly as it is a `throw` there.
    #[test]
    fn the_two_acks_accept_disjoint_delivery_states() {
        let ok: BrokerMessage = serde_json::from_value(serde_json::json!({
            "type": "delivered", "messageId": "m1", "delivery": "queued",
            "retryable": false, "outcomeKnown": true,
        }))
        .expect("`queued` is legal on a `delivered` ack");
        assert!(matches!(
            ok,
            BrokerMessage::Delivered {
                delivery: Some(DeliveredState::Queued),
                ..
            }
        ));
        assert!(
            serde_json::from_value::<BrokerMessage>(serde_json::json!({
                "type": "delivered", "messageId": "m1", "delivery": "failed",
            }))
            .is_err(),
            "`failed` on a `delivered` frame is fatal upstream"
        );
        assert!(
            serde_json::from_value::<BrokerMessage>(serde_json::json!({
                "type": "delivery_failed", "messageId": "m1", "reason": "r",
                "delivery": "socket_delivered",
            }))
            .is_err(),
            "`socket_delivered` on a `delivery_failed` frame is fatal upstream"
        );
        let unknown: BrokerMessage = serde_json::from_value(serde_json::json!({
            "type": "delivery_failed", "messageId": "m1", "reason": "r", "delivery": "unknown",
        }))
        .expect("`unknown` is legal on a failure ack");
        assert!(matches!(
            unknown,
            BrokerMessage::DeliveryFailed {
                delivery: Some(FailedState::Unknown),
                ..
            }
        ));
    }

    #[test]
    fn broker_delivery_failed_uses_snake_case_tag_and_camel_fields() {
        let msg = BrokerMessage::delivery_failed_bare("m1", "Session not found");
        let v = serde_json::to_value(&msg).unwrap();
        assert_eq!(v["type"], "delivery_failed");
        assert_eq!(v["messageId"], "m1");
        assert_eq!(v["reason"], "Session not found");
        // ICOM-054: a BARE ack still serialises to exactly the v0.9.2 three-key object, because
        // every detail field is `skip_serializing_if = "Option::is_none"`.
        assert_eq!(v.as_object().map(serde_json::Map::len), Some(3));
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
            timestamp: 9u64.into(),
            reply_to: Some("q".to_string()),
            expects_reply: Some(true),
            content: MessageContent {
                text: "hi".to_string(),
                attachments: Some(vec![Attachment {
                    kind: AttachmentKind::Snippet,
                    name: "f.rs".to_string(),
                    content: "fn main(){}".to_string(),
                    language: Some("rust".to_string()),
                    extra: UnknownFields::default(),
                }]),
                extra: UnknownFields::default(),
            },
            ..Default::default()
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["replyTo"], "q");
        assert_eq!(v["expectsReply"], true);
        assert_eq!(v["content"]["attachments"][0]["type"], "snippet");
        let back: Message = serde_json::from_value(v).unwrap();
        assert_eq!(back, m);
    }

    /// `[UNKNOWN-FIELDS]`, at the unit level. The socket-level proof is in
    /// `tests/protocol_forward_compat.rs`; this pins the struct itself so a future field addition
    /// cannot quietly drop the capture.
    #[test]
    fn a_message_round_trips_keys_it_does_not_model() {
        let raw = r#"{
            "id":"m1","timestamp":1700000000000,"senderSequence":7,"retryOf":"m0",
            "piFutureField":{"nested":[1,2,3]},
            "content":{"text":"hi","piFutureContentKey":"kept"}
        }"#;
        let msg: Message = serde_json::from_str(raw).unwrap();
        assert_eq!(msg.extra["piFutureField"]["nested"][2], 3);
        assert_eq!(msg.content.extra["piFutureContentKey"], "kept");
        let back = serde_json::to_value(&msg).unwrap();
        assert_eq!(back["piFutureField"]["nested"][2], 3);
        assert_eq!(back["content"]["piFutureContentKey"], "kept");
        assert_eq!(back["senderSequence"], 7);
        // An integer must relay AS an integer, not as `1700000000000.0`.
        assert!(back["timestamp"].is_u64());
    }

    /// `[MAP-ONLY]`, stated rather than inherited. serde derives `visit_seq` for a plain struct, so
    /// without the `extra` capture each of these arrays would deserialize *positionally*; pi bails
    /// on `Array.isArray` (or on `[]["field"]` being `undefined`) and destroys the socket. The
    /// element counts below are the field counts, i.e. the shapes that would actually have
    /// succeeded.
    #[test]
    fn every_guarded_payload_rejects_an_array() {
        assert!(
            serde_json::from_str::<SessionRegistration>(r#"[null,"/w","m",4242,0,0,null]"#)
                .is_err(),
            "`v0.9.2 broker/broker.ts:191-193` bails on Array.isArray"
        );
        assert!(
            serde_json::from_str::<SessionInfo>(r#"["s",null,"/w","m",1,2,3,null,null,null]"#)
                .is_err()
        );
        assert!(
            serde_json::from_str::<Message>(
                r#"["m1",1,null,null,null,null,null,null,null,null,null,{"text":"hi"}]"#
            )
            .is_err()
        );
        assert!(serde_json::from_str::<MessageContent>(r#"["hi",null]"#).is_err());
        assert!(serde_json::from_str::<Attachment>(r#"["snippet","n","c",null]"#).is_err());
        assert!(
            serde_json::from_str::<MessageReceipt>(r#"["m1","queued",1,null]"#).is_err(),
            "`v0.9.2 broker/client.ts:57-59` bails on Array.isArray"
        );
        assert!(
            serde_json::from_str::<MessageControl>(r#"["m1","cancel",1,null,null]"#).is_err(),
            "`v0.9.2 broker/client.ts:68-70` bails on Array.isArray"
        );
        assert!(serde_json::from_str::<ExtensionCapability>(r#"["ns",true]"#).is_err());
        // …and the well-formed map shape of each must still decode, or the invariant would have
        // been bought with a worse bug.
        serde_json::from_str::<MessageReceipt>(
            r#"{"messageId":"m1","status":"queued","timestamp":1}"#,
        )
        .expect("the map shape a pi peer actually sends");
        serde_json::from_str::<ExtensionCapability>(r#"{"namespace":"ns","ownerEligible":true}"#)
            .expect("the map shape a pi peer actually sends");
    }

    /// `[NON-NULL]`. `undefined` is accepted, `null` is not — the distinction serde's `Option`
    /// erases by default.
    #[test]
    fn an_explicit_null_is_rejected_where_an_absent_key_is_accepted() {
        // Absent: fine.
        serde_json::from_str::<SessionInfo>(
            r#"{"id":"s","cwd":"/w","model":"m","pid":1,"startedAt":2,"lastActivity":3}"#,
        )
        .expect("every optional field absent is the common case");
        for key in [
            "name",
            "status",
            "peerUid",
            "trustedLocal",
            "contextPct",
            "contextTokens",
            "contextWindow",
        ] {
            let raw = format!(
                r#"{{"id":"s","cwd":"/w","model":"m","pid":1,"startedAt":2,"lastActivity":3,"{key}":null}}"#
            );
            assert!(
                serde_json::from_str::<SessionInfo>(&raw).is_err(),
                "`session.{key}` = null must be rejected (`typeof null === \"object\"`)"
            );
        }
        for key in [
            "senderSequence",
            "brokerReceivedAt",
            "brokerDeliveredAt",
            "receiverReceivedAt",
            "injectedAt",
            "supersedes",
            "retryOf",
            "replyTo",
            "expectsReply",
        ] {
            let raw =
                format!(r#"{{"id":"m1","timestamp":1,"content":{{"text":"hi"}},"{key}":null}}"#);
            assert!(
                serde_json::from_str::<Message>(&raw).is_err(),
                "`message.{key}` = null must be rejected"
            );
        }
        assert!(
            serde_json::from_str::<MessageContent>(r#"{"text":"hi","attachments":null}"#).is_err()
        );
        assert!(
            serde_json::from_str::<Attachment>(
                r#"{"type":"snippet","name":"n","content":"c","language":null}"#
            )
            .is_err()
        );
    }

    /// `[JS-NUMBER]`. Everything `typeof x === "number"` accepts must decode, and everything it
    /// rejects must still fail — widening must not become "accept anything".
    #[test]
    fn numeric_wire_fields_accept_every_json_number_and_nothing_else() {
        for value in ["-1", "1.5", "4294967296", "-0.5", "1e300", "0"] {
            let raw = format!(
                r#"{{"id":"s","cwd":"/w","model":"m","pid":{value},"startedAt":{value},"lastActivity":{value}}}"#
            );
            let decoded = serde_json::from_str::<SessionInfo>(&raw);
            assert!(
                decoded.is_ok(),
                "pi accepts {value} as a number, got {decoded:?}"
            );
        }
        for value in ["\"1\"", "\"\"", "{}", "[]", "[1]", "true", "null"] {
            let raw = format!(
                r#"{{"id":"s","cwd":"/w","model":"m","pid":{value},"startedAt":2,"lastActivity":3}}"#
            );
            assert!(
                serde_json::from_str::<SessionInfo>(&raw).is_err(),
                "`typeof {value} !== \"number\"` upstream, so it must stay fatal here"
            );
        }
        // The fidelity half: an integer in, the same integer out.
        let info: SessionInfo = serde_json::from_str(
            r#"{"id":"s","cwd":"/w","model":"m","pid":4321,"startedAt":1700000000000,"lastActivity":3}"#,
        )
        .unwrap();
        let raw = serde_json::to_string(&info).unwrap();
        assert!(raw.contains("\"pid\":4321"), "got {raw}");
        assert!(raw.contains("\"startedAt\":1700000000000"), "got {raw}");
    }

    /// The `[JS-NUMBER]` exception: `Number.isSafeInteger` bounds both revisions.
    #[test]
    fn a_revision_is_bounded_by_max_safe_integer() {
        let ok = r#"{"type":"extension_state","namespace":"ns","revision":9007199254740991}"#;
        serde_json::from_str::<BrokerMessage>(ok).expect("MAX_SAFE_INTEGER is itself safe");
        // `2.0` is a safe integer in JS — one numeric type, so `2.0 === 2`.
        serde_json::from_str::<BrokerMessage>(
            r#"{"type":"extension_state","namespace":"ns","revision":2.0}"#,
        )
        .expect("Number.isSafeInteger(2.0) is true");
        for bad in ["9007199254740992", "-1", "1.5", "\"1\"", "null"] {
            let raw = format!(r#"{{"type":"extension_state","namespace":"ns","revision":{bad}}}"#);
            assert!(
                serde_json::from_str::<BrokerMessage>(&raw).is_err(),
                "revision {bad} fails `Number.isSafeInteger(x) && x >= 0`"
            );
        }
    }

    /// pi's guards never mention `payload`, so a frame without one must decode. cyrup was stricter
    /// and disconnected over it.
    #[test]
    fn an_absent_extension_payload_decodes() {
        serde_json::from_str::<BrokerMessage>(
            r#"{"type":"extension_state","namespace":"ns","revision":1}"#,
        )
        .expect("`v0.9.2 broker/client.ts:571-578` never checks payload");
        serde_json::from_str::<BrokerMessage>(
            r#"{"type":"extension_message","namespace":"ns","fromSessionId":"s1"}"#,
        )
        .expect("`v0.9.2 broker/client.ts:554-565` never checks payload");
    }

    /// The cross-field half of `extension_owner`/`extension_message`: `hasOwnerId !== hasOwnerEpoch`
    /// throws upstream, so one without the other is as fatal as a wrong-typed one.
    #[test]
    fn the_owner_pair_is_both_or_neither() {
        serde_json::from_str::<BrokerMessage>(r#"{"type":"extension_owner","namespace":"ns"}"#)
            .expect("unowned is legal");
        let bound = serde_json::from_str::<BrokerMessage>(
            r#"{"type":"extension_owner","namespace":"ns","ownerId":"o1","ownerEpoch":"e1"}"#,
        )
        .expect("bound is legal");
        // …and it survives a round trip with both keys intact.
        let back = serde_json::to_value(&bound).unwrap();
        assert_eq!(back["ownerId"], "o1");
        assert_eq!(back["ownerEpoch"], "e1");
        for bad in [
            r#"{"type":"extension_owner","namespace":"ns","ownerId":"o1"}"#,
            r#"{"type":"extension_owner","namespace":"ns","ownerEpoch":"e1"}"#,
            r#"{"type":"extension_owner","namespace":"ns","ownerId":null,"ownerEpoch":"e1"}"#,
            r#"{"type":"extension_owner","namespace":"ns","ownerId":7,"ownerEpoch":"e1"}"#,
        ] {
            assert!(
                serde_json::from_str::<BrokerMessage>(bad).is_err(),
                "`v0.9.2 broker/client.ts:541-548` throws for {bad}"
            );
        }
        // The unowned broadcast must not grow phantom keys on the way out.
        let unowned = BrokerMessage::ExtensionOwner {
            namespace: "ns".to_string(),
            owner: ExtensionOwnerRef::default(),
        };
        let v = serde_json::to_value(&unowned).unwrap();
        assert!(
            v.get("ownerId").is_none() && v.get("ownerEpoch").is_none(),
            "got {v}"
        );
    }

    /// `registered.features` — absent and a string list are what a real broker sends; `null` and a
    /// non-string element are what pi throws over (`v0.9.2 broker/client.ts:395-400`).
    #[test]
    fn registered_features_matches_pis_acceptance_set() {
        serde_json::from_str::<BrokerMessage>(r#"{"type":"registered","sessionId":"s1"}"#)
            .expect("absent features is what cyrup itself sends");
        serde_json::from_str::<BrokerMessage>(
            r#"{"type":"registered","sessionId":"s1","features":["extension-bus-v1"]}"#,
        )
        .expect("pi's broker advertises exactly this");
        for bad in ["null", "[1,2]", "\"extension-bus-v1\"", "{}"] {
            let raw = format!(r#"{{"type":"registered","sessionId":"s1","features":{bad}}}"#);
            assert!(
                serde_json::from_str::<BrokerMessage>(&raw).is_err(),
                "features {bad}"
            );
        }
        // The advertised name is the one pi negotiates on.
        assert_eq!(EXTENSION_BUS_FEATURE, "extension-bus-v1");
    }

    /// `presence`'s context trio is the one place `null` carries meaning, and all three states have
    /// to survive a round trip or the "clear" intent is lost on the wire.
    #[test]
    fn presence_context_fields_keep_all_three_states() {
        let msg = ClientMessage::Presence {
            runtime_fallback_alias: None,
            name: None,
            status: None,
            model: None,
            context_pct: Some(None),
            context_tokens: Some(Some(128_000u64.into())),
            context_window: None,
        };
        let v = serde_json::to_value(&msg).unwrap();
        assert_eq!(
            v["contextPct"],
            serde_json::Value::Null,
            "an explicit null CLEARS upstream"
        );
        assert_eq!(v["contextTokens"], 128_000);
        assert!(
            v.get("contextWindow").is_none(),
            "absent leaves the field untouched upstream"
        );
        let back: ClientMessage = serde_json::from_value(v).unwrap();
        assert_eq!(
            back, msg,
            "the tri-state must survive decoding, not collapse to None"
        );
    }

    /// The `[JS-NUMBER]` bargain's other half: the wire stays permissive, the *use* does not.
    /// Referenced from `tests/protocol_number_domain.rs`, which proves the wire side over a real
    /// socket and defers the accessors to here.
    #[test]
    fn the_point_of_use_accessors_refuse_what_is_not_an_integer() {
        // `-1` is `kill(2)`'s "every process the caller may signal"; `0` is "my whole process
        // group". Neither may ever come out of a presence field as a pid.
        assert_eq!(as_os_pid(&serde_json::Number::from(-1i64)), None);
        assert_eq!(as_os_pid(&serde_json::Number::from(0u64)), None);
        assert_eq!(as_os_pid(&serde_json::Number::from_f64(1.5).unwrap()), None);
        assert_eq!(
            as_os_pid(&serde_json::Number::from(4_294_967_296u64)),
            None,
            "beyond u32"
        );
        assert_eq!(as_os_pid(&serde_json::Number::from(4321u64)), Some(4321));

        assert_eq!(as_epoch_ms(&serde_json::Number::from(-1i64)), None);
        assert_eq!(
            as_epoch_ms(&serde_json::Number::from_f64(1.5).unwrap()),
            None
        );
        assert_eq!(as_epoch_ms(&serde_json::Number::from(0u64)), Some(0));
        assert_eq!(
            as_epoch_ms(&serde_json::Number::from(1_700_000_000_000u64)),
            Some(1_700_000_000_000)
        );
    }
}
