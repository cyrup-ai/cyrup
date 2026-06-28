//! cyrup-test-support — shared deterministic test harness (arch-00 §11; func-00 R-00-011).
//!
//! Provides (when implemented): the **faux provider** (scripted `EventStream<StreamEvent>`, no
//! network/no tokens), the **headless AgentSession harness** + golden-event recorder, ratatui
//! `TestBackend` helpers, and the Pi differential-test runner (func-00 R-00-012/013).
//!
//! `publish = false` — dev-only.
//!
//! The faux provider (func-01 §15) is re-exported here as the workspace's access point for
//! deterministic, network-free tests (arch-00 §11). The headless AgentSession harness, golden
//! recorders, and the Pi differential runner land alongside arch-02/04 implementation.

/// The scripted faux provider + helpers (`FauxProvider`, `faux_text`, `faux_assistant_message`, …).
pub use cyrup_provider::faux;
