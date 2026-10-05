//! The explicit cross-machine SSH relay (ICOM-074) — `name@machine` targets.
//!
//! Three upstream files at `v0.16.0`, one per concern:
//!
//! | here | upstream | what it owns |
//! |---|---|---|
//! | [`envelope`] | `cross-machine-envelope.ts` | the envelope on the wire and the `name@machine` display name |
//! | [`discovery`] | `cross-machine-discovery.ts` | `name@machine` → exactly one enabled saved machine and one live agent |
//! | [`transport`] | `cross-machine-transport.ts` | the process runner and the `ssh … relay` send |
//!
//! **The trust model, stated once.** An envelope's `origin` is whatever the SSH caller wrote in it
//! and `trust: "ssh-asserted"` says so: anyone who can invoke the relay over SSH can claim any
//! name. Nothing in this module authenticates an origin, and the sending side therefore refuses
//! every feature that would imply a verified identity or a routing edge back — no `cwd`, no project
//! panes, no `replyTo`/`supersedes`/`retryOf`, no attachments
//! ([`explicit_cross_machine_send_restriction`](crate::tools::intercom::explicit_cross_machine_send_restriction)).
//!
//! **Both halves live here.** The SENDING half is [`transport::send_cross_machine`], reached from
//! `intercom{action:"send"}` and from `cyrup-intercom-cli send --to name@machine`. The RECEIVING
//! half is [`envelope::parse_relay_envelope`] and [`envelope::relay_message`], reached from
//! `cyrup-intercom-cli relay --envelope-stdin`, the command the sender's `ssh` invokes: its byte
//! caps, exact-key refusals and the `[Unverified cross-machine origin]` body prefix are enforced on
//! that path, not merely defined. The `Message.crossMachine` provenance a relayed message carries
//! is [`crate::transport::protocol::CrossMachineProvenance`]; the inbound attribution that reads it
//! is ICOM-076.

pub mod discovery;
pub mod envelope;
pub mod transport;

pub use discovery::{
    DISCOVERY_TIMEOUT, DiscoveredRemoteAgent, DiscoveryDeps, DiscoveryError, RemoteAgent,
    SavedMachine, discover_remote_agent, list_machine_agents, list_saved_machines,
    parse_cross_machine_target, parse_remote_agents, parse_saved_machines,
};
pub use envelope::{
    CrossMachineEnvelope, CrossMachineOrigin, MAX_RELAY_ENVELOPE_BYTES,
    MAX_RELAY_ORIGIN_FIELD_BYTES, MAX_RELAY_TARGET_BYTES, MAX_RELAY_TEXT_BYTES, OriginField,
    OriginFieldName, RELAY_ENVELOPE_VERSION, RELAY_MESSAGE_PREFIX, ReadRelayEnvelopeError,
    RelayEnvelopeError, RelayOrigin, RelayTarget, RelayTrust, ValidatedRelayEnvelope,
    default_machine_name, parse_relay_envelope, read_relay_envelope, relay_message,
    relay_sender_name, resolve_origin,
};
pub use transport::{
    CommandResult, CommandRunner, CrossMachineDelivery, CrossMachineDeps, CrossMachineError,
    DELIVERY_TIMEOUT, HERDR_BIN_PATH, SpawnRunner, herdr_bin_from, send_cross_machine,
};

/// Whether `ch` is whitespace to JavaScript: the set `String.prototype.trim` strips and the regex
/// class `\s` matches — `WhiteSpace` plus `LineTerminator` (ECMA-262 §12.2, §12.3).
///
/// Not [`char::is_whitespace`]: that is Unicode `White_Space`, which counts U+0085 (NEXT LINE) and
/// does NOT count U+FEFF (the byte-order mark). Upstream's blank checks (`value.trim().length > 0`,
/// `/\s/.test(part)`) use the JS set, and a name that is "blank" to one side but not the other is a
/// routing identity the two receivers disagree about.
#[must_use]
pub(crate) fn is_js_whitespace(ch: char) -> bool {
    matches!(
        ch,
        '\u{9}'..='\u{d}'
            | ' '
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}

/// `String.prototype.trim` ([`is_js_whitespace`] at both ends).
#[must_use]
pub(crate) fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_whitespace)
}

#[cfg(test)]
mod tests;
