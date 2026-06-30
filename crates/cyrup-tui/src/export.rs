//! Standalone-HTML session export (spec/tui/04; Pi `core/export-html/index.ts`
//! `exportSessionToHtml`, wired from `interactive-mode.ts:5102-5116` `handleExportCommand`).
//!
//! The renderer now lives at the L5 seam (`cyrup_session_svc::export::session_jsonl_to_html`) so
//! every front-end shares ONE implementation (the RPC `export_html` command, the TUI `/export` /
//! `/share` paths). This module re-exports it for the TUI's existing `crate::export::` callers and
//! `cyrup_tui::session_jsonl_to_html` consumers; `app.rs` routes `/export` by extension exactly as Pi
//! (`.jsonl` → `export_to_jsonl`; else → HTML, the Pi default).

pub use cyrup_session_svc::session_jsonl_to_html;
