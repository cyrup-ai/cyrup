//! The `system` theme: cyrup's colours derived from the terminal's own theme.
//!
//! TUI-130. The recipe and its generator — the port of pi's `modes/interactive/theme/system-theme.ts`
//! @v1.0.0 — are pure arithmetic over OKHSL colour, so they live in
//! [`cyrup_resources::system_theme`], where a headless HTML export (which has no terminal and cannot
//! depend on this crate) evaluates the same code. This module re-exports them unchanged for the
//! renderer. The golden tests that pin the generator to the output of pi's own code belong to that
//! crate; this one tests what it does with the result (`UiTheme::system`).

pub use cyrup_resources::system_theme::*;

#[cfg(test)]
#[path = "system_theme_tests.rs"]
mod tests;
