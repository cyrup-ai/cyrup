//! `cross-machine-envelope.ts` (`v0.16.0`) — the SSH-relay envelope and the identity it asserts.
//!
//! The relay's whole trust model lives in this file's types: an envelope says who it claims to be
//! from, and `trust: "ssh-asserted"` is the honest name for how much that claim is worth —
//! upstream's README at v0.16.0 puts it as "anyone with SSH access that can invoke the relay can
//! claim it". Nothing here verifies anything; the receiving side is expected to render the origin
//! as unverified (ICOM-076).
//!
//! **Scope.** ICOM-074 is the SENDING half, so this module carries the envelope the sender
//! serialises, the `name@machine` display name, and the machine-name default the config needs.
//! `parseRelayEnvelope` (`:56-84`) and `relayMessage` (`:90-92`) are the RECEIVING half's byte caps
//! and body prefix, and `resolveOrigin` (`:38-54`) is read only by the relay CLI command; all three
//! belong to ICOM-075 and are deliberately absent rather than ported unused — a byte cap nothing
//! enforces is worse than no byte cap, because it reads as a guarantee.

/// `CrossMachineOrigin` (`v0.16.0 types.ts:67-71`) — who the sender claims to be.
///
/// Serialised with exactly these three keys and no `extra` capture, unlike the broker envelope
/// structs: `parseRelayEnvelope` enforces an EXACT key set on `origin` (`hasExactlyFields`,
/// `:78`), so an unknown key is a refusal upstream rather than something to round-trip. A flattened
/// capture here would let a cyrup sender emit a frame every pi receiver rejects.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CrossMachineOrigin {
    /// The origin session's intercom name (`:68`).
    pub name: String,
    /// The origin session's intercom session id (`:69`).
    pub session_id: String,
    /// `crossMachine.machineName` on the ORIGIN host (`:70`) — the label its peers know it by.
    pub machine: String,
}

/// The one `CrossMachineEnvelope.trust` value (`v0.16.0 cross-machine-envelope.ts:9`), compared
/// with `!==` at `:71` — a CLOSED one-variant vocabulary, like
/// [`ProvenanceKind`](crate::transport::protocol::ProvenanceKind).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RelayTrust {
    /// "ssh-asserted" — the origin is whatever the SSH caller said it was.
    #[serde(rename = "ssh-asserted")]
    SshAsserted,
}

/// `CrossMachineEnvelope` (`v0.16.0 cross-machine-envelope.ts:5-11`) — the single JSON object the
/// sender writes to the remote relay's stdin.
///
/// `version` is a `u8` fixed at 1 by [`Self::new`] rather than a unit type, because `b92d945` added
/// the discriminant specifically so a future shape is REJECTED rather than silently misread, and
/// that only works if the number is a value on the wire.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CrossMachineEnvelope {
    /// Always [`RELAY_ENVELOPE_VERSION`].
    pub version: u8,
    /// The agent to deliver to on the REMOTE host — its session id when discovery found one, else
    /// its name (`cross-machine-transport.ts:89`).
    pub target: String,
    /// The message body, unprefixed. The receiver adds `relayMessage`'s
    /// `[Unverified cross-machine origin]` line (ICOM-075).
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
/// Takes the two fields rather than a whole [`CrossMachineOrigin`], matching upstream's
/// `Pick<CrossMachineOrigin, "name" | "machine">`: the sending arm builds this from a DISCOVERED
/// agent's name and its machine's label (`index.ts:1717`), which is not an origin at all.
#[must_use]
pub fn relay_sender_name(name: &str, machine: &str) -> String {
    format!("{name}@{machine}")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// `defaultMachineName` (`v0.16.0 cross-machine-envelope.ts:34-36`) — the first dot-segment,
    /// lower-cased. `host.split(".", 1)` is the whole point: a FQDN must not become the machine
    /// label, because a peer's Herdr saved-machine list holds the short label.
    #[test]
    fn default_machine_name_takes_the_first_dot_segment_lowercased() {
        assert_eq!(default_machine_name("Workstation.local"), "workstation");
        assert_eq!(default_machine_name("WORKSTATION"), "workstation");
        assert_eq!(default_machine_name("a.b.c"), "a");
        assert_eq!(default_machine_name(""), "");
    }

    /// The envelope's wire shape is upstream's five keys exactly
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
}
