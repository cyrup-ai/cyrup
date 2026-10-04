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
//! **What is NOT here, and why.** The RECEIVING half is ICOM-075: `parseRelayEnvelope`'s byte caps
//! and exact-key refusals, `relayMessage`'s `[Unverified cross-machine origin]` prefix,
//! `resolveOrigin`, and the `relay --envelope-stdin` subcommand on `cyrup-intercom-cli`. The
//! `Message.crossMachine` provenance a received relay carries is ICOM-071, and the inbound
//! attribution that reads it is ICOM-076.

pub mod discovery;
pub mod envelope;
pub mod transport;

pub use discovery::{
    DISCOVERY_TIMEOUT, DiscoveredRemoteAgent, DiscoveryDeps, DiscoveryError, RemoteAgent,
    SavedMachine, discover_remote_agent, list_machine_agents, list_saved_machines,
    parse_cross_machine_target, parse_remote_agents, parse_saved_machines,
};
pub use envelope::{
    CrossMachineEnvelope, CrossMachineOrigin, RELAY_ENVELOPE_VERSION, RelayTrust,
    default_machine_name, relay_sender_name,
};
pub use transport::{
    CommandResult, CommandRunner, CrossMachineDelivery, CrossMachineDeps, CrossMachineError,
    DELIVERY_TIMEOUT, SpawnRunner, send_cross_machine,
};

#[cfg(test)]
mod tests;
