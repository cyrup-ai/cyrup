//! The broker's ported module-level constants (`broker.ts:25-42`), plus the two that have no named
//! upstream counterpart.
//!
//! Nine of the eleven are upstream's, named after upstream's constant and carrying its citation.
//! The exceptions are called out where they are defined: `SHUTDOWN_DELAY_MS` is upstream's 5 s
//! delay read off `broker.ts:295` rather than a named constant, and `READ_BUF` is a cyrup reader
//! detail with no upstream counterpart at all. They are gathered in one file because reading them
//! together is how a reviewer checks the port against `broker.ts` in one pass.

/// `MAX_SESSIONS = 128` (`broker.ts:25`).
pub(super) const MAX_SESSIONS: usize = 128;
/// `MAX_UNREGISTERED_CONNECTIONS = 32` (`broker.ts:26`).
pub(super) const MAX_UNREGISTERED_CONNECTIONS: usize = 32;
/// `REGISTRATION_TIMEOUT_MS = 1000` (`broker.ts:27`).
pub(super) const REGISTRATION_TIMEOUT_MS: u64 = 1000;
/// `PRESENCE_HEARTBEAT_MS = 1000` (`broker.ts:30`).
pub(super) const PRESENCE_HEARTBEAT_MS: u64 = 1000;
/// Auto-shutdown delay after the last session leaves (`broker.ts:295`, 5000ms).
pub(super) const SHUTDOWN_DELAY_MS: u64 = 5000;
/// `MESSAGE_RECEIPT_ROUTE_RETENTION_MS = 60 * 60 * 1000` (`v0.10.1 broker/broker.ts:39`).
pub(super) const MESSAGE_RECEIPT_ROUTE_RETENTION_MS: u64 = 60 * 60 * 1000;
/// `DISCONNECTED_SESSION_RETENTION_MS = 24 * 60 * 60 * 1000` (`v0.10.1 broker/broker.ts:40`).
pub(super) const DISCONNECTED_SESSION_RETENTION_MS: u64 = 24 * 60 * 60 * 1000;
/// `MAILBOX_MESSAGE_RETENTION_MS = 24 * 60 * 60 * 1000` (`v0.10.1 broker/broker.ts:41`).
pub(super) const MAILBOX_MESSAGE_RETENTION_MS: u64 = 24 * 60 * 60 * 1000;
/// `MAX_MAILBOX_MESSAGES = 256` (`v0.10.1 broker/broker.ts:42`).
pub(super) const MAX_MAILBOX_MESSAGES: usize = 256;
/// Reader read-buffer size (implementation detail; framing reassembles across chunk boundaries).
pub(super) const READ_BUF: usize = 16 * 1024;
/// `MAX_EXTENSIONS_PER_SESSION = 32` (`v0.9.2 broker/broker.ts:35`).
pub(super) const MAX_EXTENSIONS_PER_SESSION: usize = 32;
