//! `cross-machine-envelope.ts` (`v0.16.0`) — the SSH-relay envelope and the identity it asserts.
//!
//! The relay's whole trust model lives in this file's types: an envelope says who it claims to be
//! from, and `trust: "ssh-asserted"` is the honest name for how much that claim is worth —
//! upstream's README at v0.16.0 puts it as "anyone with SSH access that can invoke the relay can
//! claim it". Nothing here verifies anything; the receiving side renders the origin as unverified.
//!
//! **Two envelope types, on purpose.** [`CrossMachineEnvelope`] is what the SENDER builds and
//! serialises: its fields are trusted because this process wrote them, and it has no `Deserialize`.
//! [`ValidatedRelayEnvelope`] is what the RECEIVER holds: the only way to get one is
//! [`parse_relay_envelope`], which enforces the byte caps and the exact key sets, so every holder
//! of one — [`relay_message`], the relay subcommand — can rely on the caps without re-checking
//! them. A byte cap that is not enforced on the path that reads the wire is worse than none, because
//! it reads as a guarantee; this one is reached from `cyrup-intercom-cli relay`.
//!
//! **Every refusal is a named variant** ([`RelayEnvelopeError`]) whose `Display` is upstream's
//! sentence. Upstream gives five distinct sentences; the many causes that share
//! `Invalid cross-machine relay envelope.` stay distinct variants here so a test (or a log line)
//! can say WHICH rule fired.

use std::borrow::Cow;

use crate::identity::{ENV_INTERCOM_SESSION_ID, ENV_SESSION_ID};
use crate::transport::protocol::{CrossMachineProvenance, ProvenanceOrigin, SessionInfo};

pub use crate::transport::protocol::RelayTrust;

use super::{is_js_whitespace, js_trim};

/// `MAX_RELAY_ENVELOPE_BYTES` (`v0.16.0 cross-machine-envelope.ts:13`) — the whole stdin payload.
pub const MAX_RELAY_ENVELOPE_BYTES: usize = 1024 * 1024;
/// `MAX_RELAY_TARGET_BYTES` (`:14`).
pub const MAX_RELAY_TARGET_BYTES: usize = 1024;
/// `MAX_RELAY_TEXT_BYTES` (`:15`).
pub const MAX_RELAY_TEXT_BYTES: usize = 256 * 1024;
/// `MAX_RELAY_ORIGIN_FIELD_BYTES` (`:16`) — each of `origin.name`, `origin.sessionId` and
/// `origin.machine`.
pub const MAX_RELAY_ORIGIN_FIELD_BYTES: usize = 1024;

/// `ENVELOPE_FIELDS` (`:18`) — sorted, because `hasExactlyFields` compares sorted key lists.
const ENVELOPE_FIELDS: [&str; 5] = ["origin", "target", "text", "trust", "version"];
/// `ORIGIN_FIELDS` (`:19`).
const ORIGIN_FIELDS: [&str; 3] = ["machine", "name", "sessionId"];

/// The first line of every relayed body (`relayMessage`, `:90-92`).
///
/// A prompt-injection defense: the receiving model reads this as the first thing in the message
/// and cannot mistake the body for a verified local peer's. Mandatory, and asserted byte for byte.
pub const RELAY_MESSAGE_PREFIX: &str = "[Unverified cross-machine origin]\n";

/// `CrossMachineOrigin` (`v0.16.0 types.ts:67-71`) as the SENDER writes it into an envelope — who
/// the sender claims to be.
///
/// Serialised with exactly these three keys and no `extra` capture, unlike the broker envelope
/// structs: `parseRelayEnvelope` enforces an EXACT key set on `origin` (`hasExactlyFields`,
/// `:78`), so an unknown key is a refusal upstream rather than something to round-trip. A flattened
/// capture here would let a cyrup sender emit a frame every pi receiver rejects. (The origin that
/// rides on a delivered MESSAGE is a different type, [`ProvenanceOrigin`], which tolerates extras
/// because `isCrossMachineProvenance` does.)
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CrossMachineOrigin {
    /// The origin session's intercom name (`:68`).
    pub name: String,
    /// The origin session's intercom session id (`:69`).
    pub session_id: String,
    /// `crossMachine.machineName` on the ORIGIN host (`:70`) — the label its peers know it by.
    pub machine: String,
}

/// `CrossMachineEnvelope` (`v0.16.0 cross-machine-envelope.ts:5-11`) as the SENDER builds it — the
/// single JSON object written to the remote relay's stdin.
///
/// `version` is a `u8` fixed at 1 by [`Self::new`] rather than a unit type, because `b92d945` added
/// the discriminant specifically so a future shape is REJECTED rather than silently misread, and
/// that only works if the number is a value on the wire.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CrossMachineEnvelope {
    /// Always [`RELAY_ENVELOPE_VERSION`].
    pub version: u8,
    /// The agent to deliver to on the REMOTE host — its session id when discovery found one, else
    /// its name (`cross-machine-transport.ts:89`).
    pub target: String,
    /// The message body, unprefixed. The receiver adds [`RELAY_MESSAGE_PREFIX`].
    pub text: String,
    /// Who the sender claims to be.
    pub origin: CrossMachineOrigin,
    /// Always [`RelayTrust::SshAsserted`].
    pub trust: RelayTrust,
}

/// `version: 1` (`v0.16.0 cross-machine-envelope.ts:6`).
pub const RELAY_ENVELOPE_VERSION: u8 = 1;

impl CrossMachineEnvelope {
    /// `{ version: 1, target, text, origin, trust: "ssh-asserted" }`
    /// (`v0.16.0 cross-machine-transport.ts:89`).
    #[must_use]
    pub fn new(
        target: impl Into<String>,
        text: impl Into<String>,
        origin: CrossMachineOrigin,
    ) -> Self {
        Self {
            version: RELAY_ENVELOPE_VERSION,
            target: target.into(),
            text: text.into(),
            origin,
            trust: RelayTrust::SshAsserted,
        }
    }
}

/// `defaultMachineName(host)` (`v0.16.0 cross-machine-envelope.ts:34-36`):
/// `host.split(".", 1)[0]!.toLowerCase()`.
///
/// JS's two-argument `split` keeps only the first segment, so this is the hostname's first
/// dot-segment lower-cased — `"Workstation.local"` becomes `"workstation"`. A hostname with no dot
/// is lower-cased whole, and the empty string stays empty (`"".split(".", 1)` is `[""]`), which is
/// why this returns a `String` rather than refusing: the refusal belongs to the config key, which
/// rejects a blank `crossMachine.machineName`.
#[must_use]
pub fn default_machine_name(host: &str) -> String {
    host.split('.').next().unwrap_or_default().to_lowercase()
}

/// `relaySenderName(origin)` (`v0.16.0 cross-machine-envelope.ts:86-88`):
/// `` `${origin.name}@${origin.machine}` ``.
///
/// Takes the two fields rather than a whole origin, matching upstream's
/// `Pick<CrossMachineOrigin, "name" | "machine">`: the sending arm builds this from a DISCOVERED
/// agent's name and its machine's label (`index.ts:1717`), which is not an origin at all.
#[must_use]
pub fn relay_sender_name(name: &str, machine: &str) -> String {
    format!("{name}@{machine}")
}

/// `resolveOrigin(sessions, fallbackName, machineName, excludeSessionId, env)`
/// (`v0.16.0 cross-machine-envelope.ts:38-54`) — which local session an operator running
/// `cyrup-intercom-cli send --to name@machine` is speaking for.
///
/// The session id comes from the environment first ([`ENV_INTERCOM_SESSION_ID`], then
/// [`ENV_SESSION_ID`] — pi's `PI_INTERCOM_SESSION_ID` then `PI_SESSION_ID`, each trimmed, the first
/// non-blank winning): a model that shells out to the CLI from its bash tool inherits its own
/// session's id, which is far better evidence than a name. Without one, the origin is found by
/// `fallback_name` (case-insensitive), skipping the CLI's OWN transient registration
/// (`exclude_session_id`) and any synthesized `runtimeFallbackAlias` row — an alias names no one.
///
/// The roster is consulted only for the `name`; an id that matches no row is still reported
/// (`source?.id || envSessionId || "unknown"`), because the caller did assert it.
#[must_use]
pub fn resolve_origin(
    sessions: &[SessionInfo],
    fallback_name: &str,
    machine_name: &str,
    exclude_session_id: Option<&str>,
    env: impl Fn(&str) -> Option<String>,
) -> CrossMachineOrigin {
    let from_env = |key: &str| {
        env(key)
            .map(|value| js_trim(&value).to_string())
            .filter(|value| !value.is_empty())
    };
    let env_session_id = from_env(ENV_INTERCOM_SESSION_ID).or_else(|| from_env(ENV_SESSION_ID));
    let fallback_lower = fallback_name.to_lowercase();
    let source = match &env_session_id {
        Some(id) => sessions.iter().find(|session| &session.id == id),
        None => sessions.iter().find(|session| {
            Some(session.id.as_str()) != exclude_session_id
                && session.runtime_fallback_alias != Some(true)
                && session
                    .name
                    .as_deref()
                    .is_some_and(|name| name.to_lowercase() == fallback_lower)
        }),
    };
    CrossMachineOrigin {
        // `source?.name?.trim() || fallbackName`
        name: source
            .and_then(|session| session.name.as_deref())
            .map(js_trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(fallback_name)
            .to_string(),
        // `source?.id || envSessionId || "unknown"`
        session_id: source
            .map(|session| session.id.clone())
            .filter(|id| !id.is_empty())
            .or(env_session_id)
            .unwrap_or_else(|| "unknown".to_string()),
        machine: machine_name.to_string(),
    }
}

/// Every way [`parse_relay_envelope`] refuses. `Display` is upstream's sentence for the arm
/// (`parseRelayEnvelope`, `v0.16.0 cross-machine-envelope.ts:56-84`), with the crate's
/// `pi-intercom` → `cyrup-intercom` product-name substitution in [`Self::UnsupportedVersion`].
///
/// The five distinct sentences are the ones a sender can act on: too big, not JSON, an envelope
/// from a future version (upgrade), a trust claim this receiver does not understand, and "no".
/// Everything in the last group shares one sentence upstream and one variant per cause here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RelayEnvelopeError {
    /// The raw payload is over [`MAX_RELAY_ENVELOPE_BYTES`] (`:57-59`).
    #[error("Cross-machine relay envelope exceeds 1048576 byte limit.")]
    TooLarge,
    /// The payload is not JSON (`:62-65`).
    #[error("Invalid cross-machine relay envelope JSON.")]
    NotJson,
    /// The payload is JSON but not an object — `null`, an array, a string, a number (`:66-68`).
    #[error("Invalid cross-machine relay envelope.")]
    NotAnObject,
    /// `version` is absent or not `1` (`:69-71`) — checked BEFORE the key set, so an envelope from
    /// a newer sender is told to upgrade rather than told it is malformed.
    #[error(
        "Unsupported cross-machine relay envelope version; upgrade cyrup-intercom on both machines."
    )]
    UnsupportedVersion,
    /// `trust` is absent or not `"ssh-asserted"` (`:72-74`).
    #[error("Unsupported cross-machine relay envelope trust.")]
    UnsupportedTrust,
    /// The envelope's keys are not exactly `origin`, `target`, `text`, `trust`, `version` (`:75`).
    #[error("Invalid cross-machine relay envelope.")]
    EnvelopeFields,
    /// `target` is not a string, is blank, or is over [`MAX_RELAY_TARGET_BYTES`] (`:75`).
    #[error("Invalid cross-machine relay envelope.")]
    Target,
    /// `text` is not a string or is over [`MAX_RELAY_TEXT_BYTES`] (`:76`). Blank is legal here:
    /// the text is the payload and its whitespace is preserved.
    #[error("Invalid cross-machine relay envelope.")]
    Text,
    /// `origin` is not an object (`:76`).
    #[error("Invalid cross-machine relay envelope.")]
    OriginNotAnObject,
    /// `origin`'s keys are not exactly `machine`, `name`, `sessionId` (`:77`).
    #[error("Invalid cross-machine relay envelope.")]
    OriginFields,
    /// One of `origin`'s three fields is not a string, is blank, or is over
    /// [`MAX_RELAY_ORIGIN_FIELD_BYTES`] (`:78-80`).
    #[error("Invalid cross-machine relay envelope.")]
    OriginField(OriginFieldName),
}

/// Which of `origin`'s three fields [`RelayEnvelopeError::OriginField`] refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OriginFieldName {
    /// `origin.name`.
    Name,
    /// `origin.sessionId`.
    SessionId,
    /// `origin.machine`.
    Machine,
}

/// A non-blank string within [`MAX_RELAY_ORIGIN_FIELD_BYTES`] (`isNonBlankWithinBytes`,
/// `:30-32`). The inner string is private: the only constructor is the parser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OriginField(String);

impl OriginField {
    /// The validated text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A non-blank `target` within [`MAX_RELAY_TARGET_BYTES`]: a local session name or session id on
/// THIS machine. Never contains an `@` once the relay subcommand has accepted it (`cli.ts:183`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayTarget(String);

impl RelayTarget {
    /// The validated text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The origin of a validated envelope — three [`OriginField`]s, no more and no fewer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayOrigin {
    name: OriginField,
    session_id: OriginField,
    machine: OriginField,
}

impl RelayOrigin {
    /// `origin.name`.
    #[must_use]
    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    /// `origin.sessionId`.
    #[must_use]
    pub fn session_id(&self) -> &str {
        self.session_id.as_str()
    }

    /// `origin.machine`.
    #[must_use]
    pub fn machine(&self) -> &str {
        self.machine.as_str()
    }
}

/// A relay envelope that has been through [`parse_relay_envelope`] — version 1, trust
/// `ssh-asserted`, exactly the documented keys, every field within its byte cap.
///
/// Not `Deserialize`, and no public constructor: `serde` is a construction path, and a derived
/// impl would let a caller skip the caps this type exists to certify.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedRelayEnvelope {
    target: RelayTarget,
    text: String,
    origin: RelayOrigin,
}

impl ValidatedRelayEnvelope {
    /// Where on THIS machine the sender wants the message to go.
    #[must_use]
    pub fn target(&self) -> &RelayTarget {
        &self.target
    }

    /// The body, unprefixed, whitespace preserved.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Who the sender claimed to be.
    #[must_use]
    pub fn origin(&self) -> &RelayOrigin {
        &self.origin
    }

    /// `trust` — always [`RelayTrust::SshAsserted`], the only value the parser accepts.
    #[must_use]
    pub fn trust(&self) -> RelayTrust {
        RelayTrust::SshAsserted
    }

    /// `relaySenderName(envelope.origin)` — `name@machine`.
    #[must_use]
    pub fn sender_name(&self) -> String {
        relay_sender_name(self.origin.name(), self.origin.machine())
    }

    /// The provenance the relay stamps on the delivered message (`cli.ts:219-224`).
    #[must_use]
    pub fn provenance(&self) -> CrossMachineProvenance {
        CrossMachineProvenance::ssh_relay(
            ProvenanceOrigin {
                name: self.origin.name().to_string(),
                session_id: self.origin.session_id().to_string(),
                machine: self.origin.machine().to_string(),
                extra: serde_json::Map::new(),
            },
            self.trust(),
        )
    }
}

/// `relayMessage(envelope)` (`v0.16.0 cross-machine-envelope.ts:90-92`): the body the recipient
/// sees — [`RELAY_MESSAGE_PREFIX`] then the text verbatim.
#[must_use]
pub fn relay_message(envelope: &ValidatedRelayEnvelope) -> String {
    format!("{RELAY_MESSAGE_PREFIX}{}", envelope.text())
}

/// `parseRelayEnvelope(raw)` (`v0.16.0 cross-machine-envelope.ts:56-84`, hardened by `023fa84`).
///
/// Order is upstream's and is observable: the raw size, then JSON, then "is it an object", then
/// `version`, then `trust`, and only then the exact key sets and field caps — so a sender gets the
/// most actionable refusal first. The envelope and each field are measured in UTF-8 BYTES, as
/// `Buffer.byteLength` does, not characters.
///
/// # Errors
/// A [`RelayEnvelopeError`] naming the first rule the payload breaks.
pub fn parse_relay_envelope(raw: &str) -> Result<ValidatedRelayEnvelope, RelayEnvelopeError> {
    if raw.len() > MAX_RELAY_ENVELOPE_BYTES {
        return Err(RelayEnvelopeError::TooLarge);
    }
    let value: serde_json::Value = serde_json::from_str(&replace_lone_surrogate_escapes(raw))
        .map_err(|_| RelayEnvelopeError::NotJson)?;
    let serde_json::Value::Object(envelope) = value else {
        return Err(RelayEnvelopeError::NotAnObject);
    };
    // `value.version !== 1` — a number comparison, so `1.0` passes; an absent key, `"1"` and
    // `true` do not.
    if envelope
        .get("version")
        .and_then(serde_json::Value::as_number)
        .and_then(serde_json::Number::as_f64)
        != Some(1.0)
    {
        return Err(RelayEnvelopeError::UnsupportedVersion);
    }
    if envelope.get("trust").and_then(serde_json::Value::as_str) != Some("ssh-asserted") {
        return Err(RelayEnvelopeError::UnsupportedTrust);
    }
    if !has_exactly_fields(&envelope, &ENVELOPE_FIELDS) {
        return Err(RelayEnvelopeError::EnvelopeFields);
    }
    let target = non_blank_within(envelope.get("target"), MAX_RELAY_TARGET_BYTES)
        .ok_or(RelayEnvelopeError::Target)?;
    let text = envelope
        .get("text")
        .and_then(serde_json::Value::as_str)
        .filter(|text| text.len() <= MAX_RELAY_TEXT_BYTES)
        .ok_or(RelayEnvelopeError::Text)?;
    let origin = envelope
        .get("origin")
        .and_then(serde_json::Value::as_object)
        .ok_or(RelayEnvelopeError::OriginNotAnObject)?;
    if !has_exactly_fields(origin, &ORIGIN_FIELDS) {
        return Err(RelayEnvelopeError::OriginFields);
    }
    let origin_field = |key: &str, which: OriginFieldName| {
        non_blank_within(origin.get(key), MAX_RELAY_ORIGIN_FIELD_BYTES)
            .map(|value| OriginField(value.to_string()))
            .ok_or(RelayEnvelopeError::OriginField(which))
    };
    Ok(ValidatedRelayEnvelope {
        target: RelayTarget(target.to_string()),
        text: text.to_string(),
        origin: RelayOrigin {
            name: origin_field("name", OriginFieldName::Name)?,
            session_id: origin_field("sessionId", OriginFieldName::SessionId)?,
            machine: origin_field("machine", OriginFieldName::Machine)?,
        },
    })
}

/// Why [`read_relay_envelope`] gave no envelope.
#[derive(Debug, thiserror::Error)]
pub enum ReadRelayEnvelopeError {
    /// Reading the input failed; the message is the io error's.
    #[error("{0}")]
    Read(#[from] std::io::Error),
    /// The input was read and refused by [`parse_relay_envelope`].
    #[error("{0}")]
    Envelope(#[from] RelayEnvelopeError),
}

/// Read a relay envelope from `reader` (the relay subcommand's stdin) and parse it.
///
/// Reads AT MOST [`MAX_RELAY_ENVELOPE_BYTES`] + 1 bytes. Upstream collects the whole stream and
/// checks the length afterwards (`readProcessStdin` then `parseRelayEnvelope`); the outcome is the
/// same refusal, but a sender that streams gigabytes is stopped after one byte over the cap
/// instead of being buffered. Lossy UTF-8 decoding never shrinks its input, so more than the cap
/// in raw bytes is more than the cap in decoded bytes and [`parse_relay_envelope`] reports
/// [`RelayEnvelopeError::TooLarge`] for the truncated read exactly as it would for the whole.
///
/// # Errors
/// [`ReadRelayEnvelopeError::Read`] when the reader fails, else the parser's refusal.
pub fn read_relay_envelope(
    reader: impl std::io::Read,
) -> Result<ValidatedRelayEnvelope, ReadRelayEnvelopeError> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    reader
        .take(MAX_RELAY_ENVELOPE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    Ok(parse_relay_envelope(&String::from_utf8_lossy(&bytes))?)
}

/// `hasExactlyFields(value, fields)` (`:25-28`): the sorted keys equal the sorted list.
fn has_exactly_fields(value: &serde_json::Map<String, serde_json::Value>, fields: &[&str]) -> bool {
    let mut keys: Vec<&str> = value.keys().map(String::as_str).collect();
    keys.sort_unstable();
    keys == fields
}

/// `isNonBlankWithinBytes(value, maxBytes)` (`:30-32`): a string with a non-whitespace character
/// (JS `trim()`'s whitespace, see [`is_js_whitespace`]) and at most `max_bytes` UTF-8 bytes.
fn non_blank_within(value: Option<&serde_json::Value>, max_bytes: usize) -> Option<&str> {
    value
        .and_then(serde_json::Value::as_str)
        .filter(|text| !text.chars().all(is_js_whitespace) && text.len() <= max_bytes)
}

/// Rewrites every UNPAIRED `\uD800`–`\uDFFF` escape inside a JSON string to `�`.
///
/// `JSON.stringify` on the sending side is well-formed (`\ud83d` for a lone surrogate), and
/// `JSON.parse` on pi's receiving side accepts it, yielding a string whose UTF-8 form is U+FFFD. A
/// sender that truncated a message in the middle of an emoji therefore produces a perfectly good
/// pi envelope that `serde_json` alone would refuse as "not JSON". Rewriting to the replacement
/// character the JS side would end up with keeps the two receivers agreeing on the same bytes.
/// A correctly PAIRED surrogate escape is left alone.
fn replace_lone_surrogate_escapes(raw: &str) -> Cow<'_, str> {
    if !raw.contains("\\u") {
        return Cow::Borrowed(raw);
    }
    let bytes = raw.as_bytes();
    let hex4 = |at: usize| -> Option<u32> {
        let digits = raw.get(at..at + 4)?;
        if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u32::from_str_radix(digits, 16).ok()
    };
    let mut out = String::new();
    let mut copied_to = 0;
    let mut at = 0;
    let mut in_string = false;
    let mut changed = false;
    while let Some(&byte) = bytes.get(at) {
        match byte {
            b'"' => {
                in_string = !in_string;
                at += 1;
            }
            b'\\' if in_string => {
                if bytes.get(at + 1) == Some(&b'u')
                    && let Some(unit) = hex4(at + 2)
                {
                    let paired_high = (0xD800..0xDC00).contains(&unit)
                        && bytes.get(at + 6) == Some(&b'\\')
                        && bytes.get(at + 7) == Some(&b'u')
                        && hex4(at + 8).is_some_and(|low| (0xDC00..0xE000).contains(&low));
                    if paired_high {
                        at += 12;
                    } else if (0xD800..0xE000).contains(&unit) {
                        out.push_str(raw.get(copied_to..at).unwrap_or_default());
                        out.push_str("\\uFFFD");
                        at += 6;
                        copied_to = at;
                        changed = true;
                    } else {
                        at += 6;
                    }
                } else {
                    // Any other escape: skip the backslash AND the escaped byte, so `\"` and `\\`
                    // cannot toggle the string state or start a fake `\u`.
                    at += 2;
                }
            }
            _ => at += 1,
        }
    }
    if !changed {
        return Cow::Borrowed(raw);
    }
    out.push_str(raw.get(copied_to..).unwrap_or_default());
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    const FAKE_SESSION_ID: &str = "00000000-0000-4000-8000-000000000001";

    /// `envelope(overrides)` (`cross-machine-envelope.test.ts:17-26`): the valid envelope with
    /// top-level keys replaced (a `null` override REMOVES the key, which JS expresses with
    /// `undefined`).
    fn envelope(overrides: serde_json::Value) -> serde_json::Value {
        let mut base = serde_json::json!({
            "version": 1,
            "target": "reviewer",
            "text": "payload",
            "trust": "ssh-asserted",
            "origin": { "name": "worker", "sessionId": FAKE_SESSION_ID, "machine": "laptop" },
        });
        if let (Some(base), Some(overrides)) = (base.as_object_mut(), overrides.as_object()) {
            for (key, value) in overrides {
                base.insert(key.clone(), value.clone());
            }
        }
        base
    }

    fn parse(value: &serde_json::Value) -> Result<ValidatedRelayEnvelope, RelayEnvelopeError> {
        parse_relay_envelope(&value.to_string())
    }

    fn origin(name: &str, session_id: &str, machine: &str) -> serde_json::Value {
        serde_json::json!({ "name": name, "sessionId": session_id, "machine": machine })
    }

    fn session(id: &str, name: Option<&str>, alias: Option<bool>) -> SessionInfo {
        let mut row = serde_json::json!({
            "id": id, "cwd": "/w", "model": "m", "pid": 1, "startedAt": 1, "lastActivity": 1,
        });
        if let Some(name) = name {
            row["name"] = name.into();
        }
        if let Some(alias) = alias {
            row["runtimeFallbackAlias"] = alias.into();
        }
        serde_json::from_value(row).unwrap()
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    /// `machine names default to the lowercased short hostname`.
    #[test]
    fn default_machine_name_takes_the_first_dot_segment_lowercased() {
        assert_eq!(default_machine_name("Build-Host.EXAMPLE"), "build-host");
        assert_eq!(default_machine_name("Workstation.local"), "workstation");
        assert_eq!(default_machine_name("WORKSTATION"), "workstation");
        assert_eq!(default_machine_name("a.b.c"), "a");
        assert_eq!(default_machine_name(""), "");
    }

    /// The SENDER's envelope serialises to upstream's five keys exactly
    /// (`v0.16.0 cross-machine-transport.ts:89`), with `version: 1` and `trust: "ssh-asserted"` as
    /// literals — a pi v0.16.0 receiver's `parseRelayEnvelope` enforces an EXACT key set, so an
    /// extra or renamed key is an undeliverable message rather than a tolerated one.
    #[test]
    fn the_envelope_serialises_to_upstreams_exact_key_set() {
        let envelope = CrossMachineEnvelope::new(
            "reviewer",
            "ship it",
            CrossMachineOrigin {
                name: "alice".to_string(),
                session_id: "sess-1".to_string(),
                machine: "laptop".to_string(),
            },
        );
        assert_eq!(
            serde_json::to_value(&envelope).unwrap(),
            serde_json::json!({
                "version": 1,
                "target": "reviewer",
                "text": "ship it",
                "origin": { "name": "alice", "sessionId": "sess-1", "machine": "laptop" },
                "trust": "ssh-asserted",
            })
        );
    }

    /// `relaySenderName` (`:86-88`).
    #[test]
    fn relay_sender_name_is_name_at_machine() {
        assert_eq!(relay_sender_name("alice", "laptop"), "alice@laptop");
    }

    /// `relay envelope carries structured origin and a one-line fallback marker`
    /// (`cross-machine-envelope.test.ts:40-44`) — and the prefix is byte for byte.
    #[test]
    fn relay_envelope_carries_structured_origin_and_a_one_line_fallback_marker() {
        let parsed = parse(&envelope(serde_json::json!({}))).unwrap();
        assert_eq!(parsed.sender_name(), "worker@laptop");
        assert_eq!(
            relay_message(&parsed),
            "[Unverified cross-machine origin]\npayload"
        );
        assert_eq!(RELAY_MESSAGE_PREFIX, "[Unverified cross-machine origin]\n");
        assert_eq!(parsed.target().as_str(), "reviewer");
        assert_eq!(parsed.origin().session_id(), FAKE_SESSION_ID);
        assert_eq!(parsed.trust(), RelayTrust::SshAsserted);
    }

    /// `relay parser accepts exact field limits and preserves text whitespace`
    /// (`cross-machine-envelope.test.ts:46-64`).
    #[test]
    fn relay_parser_accepts_exact_field_limits_and_preserves_text_whitespace() {
        let parsed = parse(&envelope(serde_json::json!({
            "target": "t".repeat(MAX_RELAY_TARGET_BYTES),
            "text": format!(" {} ", "x".repeat(MAX_RELAY_TEXT_BYTES - 2)),
            "origin": origin(
                &"n".repeat(MAX_RELAY_ORIGIN_FIELD_BYTES),
                &"s".repeat(MAX_RELAY_ORIGIN_FIELD_BYTES),
                &"m".repeat(MAX_RELAY_ORIGIN_FIELD_BYTES),
            ),
        })))
        .unwrap();
        assert_eq!(parsed.target().as_str().len(), MAX_RELAY_TARGET_BYTES);
        assert_eq!(parsed.text().len(), MAX_RELAY_TEXT_BYTES);
        assert!(parsed.text().starts_with(' ') && parsed.text().ends_with(' '));

        let compact = envelope(serde_json::json!({})).to_string();
        let exact_raw = format!(
            "{compact}{}",
            " ".repeat(MAX_RELAY_ENVELOPE_BYTES - compact.len())
        );
        assert_eq!(exact_raw.len(), MAX_RELAY_ENVELOPE_BYTES);
        assert_eq!(parse_relay_envelope(&exact_raw).unwrap().text(), "payload");
    }

    /// `relay parser rejects over-limit raw and field values`
    /// (`cross-machine-envelope.test.ts:66-79`): the raw cap says "exceeds … byte limit", every
    /// field cap says "Invalid …".
    #[test]
    fn relay_parser_rejects_over_limit_raw_and_field_values() {
        let compact = envelope(serde_json::json!({})).to_string();
        let over = format!(
            "{compact}{}",
            " ".repeat(MAX_RELAY_ENVELOPE_BYTES - compact.len() + 1)
        );
        let error = parse_relay_envelope(&over).unwrap_err();
        assert_eq!(error, RelayEnvelopeError::TooLarge);
        assert_eq!(
            error.to_string(),
            format!("Cross-machine relay envelope exceeds {MAX_RELAY_ENVELOPE_BYTES} byte limit.")
        );

        let cases = [
            (
                envelope(serde_json::json!({ "target": "t".repeat(MAX_RELAY_TARGET_BYTES + 1) })),
                RelayEnvelopeError::Target,
            ),
            (
                envelope(serde_json::json!({ "text": "x".repeat(MAX_RELAY_TEXT_BYTES + 1) })),
                RelayEnvelopeError::Text,
            ),
            (
                envelope(serde_json::json!({
                    "origin": origin(&"n".repeat(MAX_RELAY_ORIGIN_FIELD_BYTES + 1), FAKE_SESSION_ID, "laptop")
                })),
                RelayEnvelopeError::OriginField(OriginFieldName::Name),
            ),
            (
                envelope(serde_json::json!({
                    "origin": origin("worker", &"s".repeat(MAX_RELAY_ORIGIN_FIELD_BYTES + 1), "laptop")
                })),
                RelayEnvelopeError::OriginField(OriginFieldName::SessionId),
            ),
            (
                envelope(serde_json::json!({
                    "origin": origin("worker", FAKE_SESSION_ID, &"m".repeat(MAX_RELAY_ORIGIN_FIELD_BYTES + 1))
                })),
                RelayEnvelopeError::OriginField(OriginFieldName::Machine),
            ),
        ];
        for (value, expected) in cases {
            let error = parse(&value).unwrap_err();
            assert_eq!(error, expected);
            assert_eq!(error.to_string(), "Invalid cross-machine relay envelope.");
        }
    }

    /// The caps count UTF-8 BYTES (`Buffer.byteLength`), not characters: 513 two-byte characters
    /// are 1026 bytes and over the 1 KiB target cap, though only 513 characters long.
    #[test]
    fn the_caps_count_utf8_bytes_not_characters() {
        let within = "é".repeat(MAX_RELAY_TARGET_BYTES / 2);
        assert!(parse(&envelope(serde_json::json!({ "target": within }))).is_ok());
        let over = "é".repeat(MAX_RELAY_TARGET_BYTES / 2 + 1);
        assert_eq!(
            parse(&envelope(serde_json::json!({ "target": over }))).unwrap_err(),
            RelayEnvelopeError::Target
        );
    }

    /// `relay parser rejects undocumented fields` (`cross-machine-envelope.test.ts:81-86`), and
    /// the missing-key half `hasExactlyFields` also refuses.
    #[test]
    fn relay_parser_rejects_undocumented_and_missing_fields() {
        assert_eq!(
            parse(&envelope(serde_json::json!({ "extra": true }))).unwrap_err(),
            RelayEnvelopeError::EnvelopeFields
        );
        assert_eq!(
            parse(&envelope(serde_json::json!({
                "origin": { "name": "worker", "sessionId": FAKE_SESSION_ID, "machine": "laptop", "extra": true }
            })))
            .unwrap_err(),
            RelayEnvelopeError::OriginFields
        );
        let mut missing = envelope(serde_json::json!({}));
        missing.as_object_mut().unwrap().remove("text");
        assert_eq!(
            parse(&missing).unwrap_err(),
            RelayEnvelopeError::EnvelopeFields
        );
        let mut missing_machine = envelope(serde_json::json!({}));
        missing_machine["origin"]
            .as_object_mut()
            .unwrap()
            .remove("machine");
        assert_eq!(
            parse(&missing_machine).unwrap_err(),
            RelayEnvelopeError::OriginFields
        );
    }

    /// `relay parser rejects malformed envelopes and blank routing identities`
    /// (`cross-machine-envelope.test.ts:88-101`).
    #[test]
    fn relay_parser_rejects_malformed_envelopes_and_blank_routing_identities() {
        for value in [
            serde_json::Value::Null,
            serde_json::json!([]),
            serde_json::json!("envelope"),
        ] {
            let error = parse(&value).unwrap_err();
            assert_eq!(error, RelayEnvelopeError::NotAnObject);
            assert_eq!(error.to_string(), "Invalid cross-machine relay envelope.");
        }
        assert_eq!(
            parse(&envelope(serde_json::json!({ "target": " \t" }))).unwrap_err(),
            RelayEnvelopeError::Target
        );
        assert_eq!(
            parse(&envelope(serde_json::json!({ "text": 1 }))).unwrap_err(),
            RelayEnvelopeError::Text
        );
        assert_eq!(
            parse(&envelope(serde_json::json!({ "origin": [] }))).unwrap_err(),
            RelayEnvelopeError::OriginNotAnObject
        );
        for (value, which) in [
            (
                origin(" ", FAKE_SESSION_ID, "laptop"),
                OriginFieldName::Name,
            ),
            (origin("worker", "\n", "laptop"), OriginFieldName::SessionId),
            (
                origin("worker", FAKE_SESSION_ID, "\t"),
                OriginFieldName::Machine,
            ),
            // U+FEFF is whitespace to JS `trim()` but not to Rust's `char::is_whitespace`.
            (
                origin("\u{feff}", FAKE_SESSION_ID, "laptop"),
                OriginFieldName::Name,
            ),
        ] {
            assert_eq!(
                parse(&envelope(serde_json::json!({ "origin": value }))).unwrap_err(),
                RelayEnvelopeError::OriginField(which)
            );
        }
        let error = parse_relay_envelope("{").unwrap_err();
        assert_eq!(error, RelayEnvelopeError::NotJson);
        assert_eq!(
            error.to_string(),
            "Invalid cross-machine relay envelope JSON."
        );
    }

    /// `relay parser rejects unsupported versions and trust claims`
    /// (`cross-machine-envelope.test.ts:103-106`) — and the ORDER: version and trust are judged
    /// before the key set, so an envelope with both an unknown version and an extra key is told to
    /// upgrade.
    #[test]
    fn relay_parser_rejects_unsupported_versions_and_trust_claims() {
        let version = parse(&envelope(serde_json::json!({ "version": 2 }))).unwrap_err();
        assert_eq!(version, RelayEnvelopeError::UnsupportedVersion);
        assert_eq!(
            version.to_string(),
            "Unsupported cross-machine relay envelope version; upgrade cyrup-intercom on both machines."
        );
        let trust = parse(&envelope(serde_json::json!({ "trust": "verified" }))).unwrap_err();
        assert_eq!(trust, RelayEnvelopeError::UnsupportedTrust);
        assert_eq!(
            trust.to_string(),
            "Unsupported cross-machine relay envelope trust."
        );

        assert_eq!(
            parse(&envelope(serde_json::json!({ "version": 2, "extra": 1 }))).unwrap_err(),
            RelayEnvelopeError::UnsupportedVersion
        );
        assert_eq!(
            parse(&envelope(serde_json::json!({ "version": "1" }))).unwrap_err(),
            RelayEnvelopeError::UnsupportedVersion
        );
        let mut absent = envelope(serde_json::json!({}));
        absent.as_object_mut().unwrap().remove("version");
        assert_eq!(
            parse(&absent).unwrap_err(),
            RelayEnvelopeError::UnsupportedVersion
        );
        // `1.0 === 1` in JS.
        assert!(
            parse_relay_envelope(
                &envelope(serde_json::json!({}))
                    .to_string()
                    .replace("\"version\":1", "\"version\":1.0")
            )
            .is_ok()
        );
    }

    /// A lone surrogate escape is well-formed JSON to `JSON.parse` (a truncated emoji from a pi
    /// sender) and lands as U+FFFD; a PAIRED escape is the real character; a backslash-escaped
    /// backslash before `u` is not an escape at all.
    #[test]
    fn lone_surrogate_escapes_become_the_replacement_character_as_on_the_js_side() {
        let raw = |text: &str| {
            format!(
                r#"{{"version":1,"target":"reviewer","text":"{text}","trust":"ssh-asserted","origin":{{"name":"w","sessionId":"s","machine":"m"}}}}"#
            )
        };
        assert_eq!(
            parse_relay_envelope(&raw(r"cut\ud83d")).unwrap().text(),
            "cut\u{fffd}"
        );
        assert_eq!(
            parse_relay_envelope(&raw(r"\ude00 then \ud83d x"))
                .unwrap()
                .text(),
            "\u{fffd} then \u{fffd} x"
        );
        assert_eq!(
            parse_relay_envelope(&raw(r"smile 😀")).unwrap().text(),
            "smile \u{1f600}"
        );
        assert_eq!(
            parse_relay_envelope(&raw(r"literal \\ud83d"))
                .unwrap()
                .text(),
            r"literal \ud83d"
        );
        assert_eq!(
            parse_relay_envelope(&raw(r#"quote \" then \ud83d"#))
                .unwrap()
                .text(),
            "quote \" then \u{fffd}"
        );
    }

    /// A reader that yields bytes forever and counts how many were taken from it.
    struct Endless(std::rc::Rc<std::cell::Cell<usize>>);

    impl std::io::Read for Endless {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            buf.fill(b' ');
            self.0.set(self.0.get() + buf.len());
            Ok(buf.len())
        }
    }

    /// The relay subcommand reads at most the cap plus one byte, so an endless stdin is refused
    /// as too large instead of being buffered, and a valid envelope read from a stream parses.
    #[test]
    fn reading_the_envelope_stops_one_byte_past_the_cap() {
        let taken = std::rc::Rc::new(std::cell::Cell::new(0));
        let error = read_relay_envelope(Endless(taken.clone())).unwrap_err();
        assert!(matches!(
            error,
            ReadRelayEnvelopeError::Envelope(RelayEnvelopeError::TooLarge)
        ));
        assert!(
            taken.get() <= MAX_RELAY_ENVELOPE_BYTES + 1 + 8192,
            "took {} bytes from an endless stream",
            taken.get()
        );

        let body = envelope(serde_json::json!({})).to_string();
        let parsed = read_relay_envelope(std::io::Cursor::new(format!("{body}\n"))).unwrap();
        assert_eq!(parsed.text(), "payload");
    }

    /// `operator origin lookup excludes its own transient CLI session`
    /// (`cross-machine-envelope.test.ts:32-38`).
    #[test]
    fn operator_origin_lookup_excludes_its_own_transient_cli_session() {
        let source = session(FAKE_SESSION_ID, Some("worker"), None);
        let transient = session("00000000-0000-4000-8000-000000000002", Some("worker"), None);
        let resolved = resolve_origin(
            &[transient.clone(), source],
            "worker",
            "laptop",
            Some(&transient.id),
            no_env,
        );
        assert_eq!(
            resolved,
            CrossMachineOrigin {
                name: "worker".to_string(),
                session_id: FAKE_SESSION_ID.to_string(),
                machine: "laptop".to_string(),
            }
        );
    }

    /// The rest of `resolveOrigin`'s contract (`:38-54`): a synthesized alias is never the origin,
    /// the name match ignores case but the reported name is the roster's, an unmatched name falls
    /// back to the id `"unknown"`.
    #[test]
    fn resolve_origin_skips_aliases_matches_names_case_insensitively_and_falls_back() {
        let alias = session("alias-id", Some("worker"), Some(true));
        let real = session("real-id", Some("Worker"), Some(false));
        let resolved = resolve_origin(&[alias.clone(), real], "WORKER", "laptop", None, no_env);
        assert_eq!(resolved.session_id, "real-id");
        assert_eq!(resolved.name, "Worker", "the roster's spelling wins");

        let none = resolve_origin(&[alias], "worker", "laptop", None, no_env);
        assert_eq!(
            none,
            CrossMachineOrigin {
                name: "worker".to_string(),
                session_id: "unknown".to_string(),
                machine: "laptop".to_string(),
            }
        );
    }

    /// The session id from the environment wins, `ENV_INTERCOM_SESSION_ID` before
    /// `ENV_SESSION_ID`, each trimmed and a blank one skipped; the roster supplies only the name,
    /// and an id that matches no row is still reported.
    #[test]
    fn resolve_origin_prefers_the_environment_session_id() {
        let env_of = |pairs: &'static [(&'static str, &'static str)]| {
            move |key: &str| {
                pairs
                    .iter()
                    .find(|(name, _)| *name == key)
                    .map(|(_, value)| (*value).to_string())
            }
        };
        let rows = [
            session("by-name", Some("worker"), None),
            session("env-id", Some("  Env Named  "), None),
        ];
        let from_primary = resolve_origin(
            &rows,
            "worker",
            "laptop",
            None,
            env_of(&[
                (ENV_INTERCOM_SESSION_ID, " env-id "),
                (ENV_SESSION_ID, "other"),
            ]),
        );
        assert_eq!(from_primary.session_id, "env-id");
        assert_eq!(from_primary.name, "Env Named");

        let from_secondary = resolve_origin(
            &rows,
            "worker",
            "laptop",
            None,
            env_of(&[(ENV_INTERCOM_SESSION_ID, "   "), (ENV_SESSION_ID, "env-id")]),
        );
        assert_eq!(from_secondary.session_id, "env-id");

        let unlisted = resolve_origin(
            &rows,
            "worker",
            "laptop",
            None,
            env_of(&[(ENV_SESSION_ID, "not-on-the-roster")]),
        );
        assert_eq!(unlisted.session_id, "not-on-the-roster");
        assert_eq!(unlisted.name, "worker", "no row, so the fallback name");
    }
}
