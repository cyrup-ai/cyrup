//! Standalone-HTML session export (spec/tui/04; Pi `core/export-html/index.ts`
//! `exportSessionToHtml`, wired from `interactive-mode.ts:5102-5116` `handleExportCommand`).
//!
//! The renderer lives at the L5 seam (`cyrup_session_svc::export`) so every front-end shares ONE
//! implementation (the RPC `export_html` command, the TUI `/export` / `/share` paths). This module
//! re-exports it for `cyrup_tui::session_jsonl_to_html` consumers; `app.rs` routes `/export` by
//! extension exactly as Pi (`.jsonl` → `export_to_jsonl`; else → HTML, the Pi default).
//!
//! The two TUI call sites use [`cyrup_session_svc::session_jsonl_to_html_with_theme`] with the
//! session's own [`cyrup_session_svc::ExportTheme`], so an interactive export carries the theme the
//! user is looking at — pi resolves the ACTIVE theme inside `generateHtml`
//! (`core/export-html/index.ts:151-157` @v0.84.4). This alias keeps the default-palette entry point
//! reachable for embedders that have no session (DRIFT-041).

pub use cyrup_session_svc::session_jsonl_to_html;
