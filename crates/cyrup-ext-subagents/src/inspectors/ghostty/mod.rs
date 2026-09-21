//! The ghostty inspector backend — 91 upstream lines (`src/inspectors/ghostty/{plugin,actions}.ts`
//! @v0.68.0), and the second of the two built-in backends
//! [`crate::inspectors::plugins::builtin_inspector_plugins`] consults.
//!
//! Ghostty 1.3 exposes an AppleScript surface: split the front window's focused terminal, point
//! the new surface at a directory, run one command in it, focus it or not. That is all this
//! backend does — **one verb, `open`**. There is no pane registry to query and no binding file to
//! write, so `status` and `close` are absent here and the success sentence says so
//! (`ghostty/actions.ts:69`), and [`plugin::GhosttyInspectorPlugin::owns`] is a literal `false`.
//!
//! Nothing in this subtree is `#[cfg]`-ed out on a non-macOS host: the AppleScript, the argv
//! builder and [`actions::open_ghostty_inspector`] compile and are unit-tested on Linux through
//! the injected [`crate::inspectors::plugins::GhosttyRunner`], and only
//! [`actions::OsascriptRunner`] ever names `/usr/bin/osascript`. The platform gate lives in
//! `available()`, exactly where upstream puts it (`ghostty/plugin.ts:10,13`).

pub mod actions;
pub mod plugin;
