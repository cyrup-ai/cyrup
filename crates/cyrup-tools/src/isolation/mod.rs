//! Permissions & isolation seam pieces (arch-12; conformance: func-12 R-12-*).
//!
//! cyrup ships **no built-in permission system** and runs tools with the user's full permissions
//! (DI-5, R-12-001/002): by default nothing here is in the call path, so a built-in `bash`
//! `rm -rf`-style command runs with no prompt and no allow/deny list (see the default-stance test in
//! `tests/isolation.rs`). Safety is *opt-in* and *composable*, built from these reusable pieces that
//! plug into the existing [`crate::ops`] backend seam (arch-03) and the agent `tool_call` gate
//! (wired in `cyrup-agent`, not here):
//!
//! - [`ProtectedFs`] / [`ProtectedPaths`] — backend-seam decorator that blocks writes/edits to
//!   protected paths (`.env`, `.git/`, `node_modules/`) while passing reads through (R-12-006).
//!   **[CYRUP-DELTA], and off by default** (`SessionConfig::protect_paths: false`, ADR-0003 D5):
//!   pi has no protected-path concept — `core/tools/write.ts:195-225` @v0.83.0 writes whatever
//!   path it is handed. It decorates the **fs** seam ONLY; an embedder that opts in still leaves
//!   `bash 'echo x >> .env'` unaffected, because the process seam is passed through undecorated
//!   and no correct guard can be derived from arbitrary command text (ADR-0003 D6).
//! - [`TraversalFs`] — backend-seam decorator confining all fs operations to a root, rejecting
//!   `../` and symlink escapes (R-03-006).
//! - [`PermissionPolicy`] / [`PolicyDecision`] / [`Rule`] — the pure, unit-testable decision logic
//!   for the `tool_call` gate (`Proceed` / `Mutate` / `Block` / `Confirm`), plus the
//!   [`protected_path_rule`] helper (R-12-005/006).
//! - [`sandbox`] — a **deferred** cfg-gated OS-sandbox placeholder (R-12-013, A-12-9), shaped but
//!   without pulling `landlock`/`seccompiler`/Seatbelt.
//!
//! ## Routing / backend-swap (R-12-011/012)
//! There is no new routing trait: routing isolation *is* swapping the [`crate::ops::Backend`]
//! (`Arc<dyn FsOps>` / `Arc<dyn ProcOps>`). Every built-in tool already holds the backend by
//! trait object, so wrapping it ([`ProtectedFs`], [`TraversalFs`]) or replacing it (a remote/SSH/
//! container `FsOps`/`ProcOps`, or the recording backend in `tests/isolation.rs`) re-targets all
//! tools at once with **no contract change**. The decorators in this module are the composing
//! examples; an SSH/container backend is the same shape applied to a remote.
//!
//! These pieces compose with — and do not replace — the `tool_call` policy gate that already lives
//! in `cyrup-agent` (`before_tool_call`).

pub mod policy;
pub mod protected;
pub mod sandbox;
pub mod traversal;

// Re-export the backend-seam types these decorators are written against (arch-03 canonical).
pub use crate::ops::{Access, DirEntry, FsOps, ImageMime, Meta, WalkFlavor, WalkItem, WalkOpts};

pub use policy::{
    PermissionPolicy, PolicyDecision, Rule, RuleBuilder, bash_command_contains,
    dangerous_bash_rule, is_dangerous_command, is_tool, protected_path_rule,
};
pub use protected::{ProtectedFs, ProtectedPaths};
pub use sandbox::{DeferredSandbox, OsSandbox, SandboxError, SandboxKind, SandboxPolicy};
pub use traversal::TraversalFs;
