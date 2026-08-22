//! The extension's fixed identity strings and the two artifacts it ships embedded.
//!
//! Split out of the single-file `extension.rs`; the wiring narrative these belong to is in
//! [`super`]'s module doc.

// Doc-link-only imports: these are named by prose relocated verbatim from the single-file
// `extension.rs`, where they were in scope for real code. `#[cfg(doc)]` keeps those intra-doc
// links resolving without adding an import the compiled build does not use.
#[cfg(doc)]
use super::PermissionSystemExtension;
#[cfg(doc)]
use crate::manager::PermissionManager;

/// The extension's fixed id (pi `EXTENSION_ID`, `extension-config.ts:8`).
pub const EXTENSION_ID: &str = "cyrup-permission-system";

/// The slash command this extension registers (pi `pi.registerCommand("permission-system", …)`,
/// v0.8.0 `index.ts:1502`).
pub const PERMISSION_SYSTEM_COMMAND: &str = "permission-system";

/// PERM-011 half B / pi `PERMISSION_REQUEST_EVENT_CHANNEL` (v0.8.0 `index.ts:150`). This is the
/// topic a second extension subscribes to in order to observe every gated request and its outcome.
///
/// \[CYRUP-DELTA] The channel NAME diverges: upstream's literal is
/// `"pi-permission-system:permission-request"`, and this is
/// `"cyrup-permission-system:permission-request"`. It is the same rebrand every other identifier in
/// this crate carries ([`EXTENSION_ID`], the config dir, the log prefixes), and it is load-bearing
/// rather than cosmetic — the string IS the subscription key, so a guest ported verbatim from a pi
/// extension subscribes to a topic nothing publishes. Recorded here explicitly because a bus topic
/// is exactly the kind of constant a name-level parity diff scores as "present on both sides".
pub const PERMISSION_REQUEST_EVENT_CHANNEL: &str = "cyrup-permission-system:permission-request";

/// The `error` recorded on a `permission_request.event_emit_failed` entry when no host backend is
/// attached to emit through. See [`PermissionSystemExtension::emit_permission_request_event`] for
/// why this stands in for upstream's thrown-exception message.
pub(super) const NO_EVENT_BACKEND_ERROR: &str =
    "No host services backend is attached to emit through.";

/// The `source` label the `/permission-system` handler passes to
/// [`PermissionSystemExtension::set_yolo_mode`], so a `yolo_mode.updated` entry says which surface
/// moved the flag (pi `options.source`, `yolo-mode-api.ts:3`).
pub const COMMAND_YOLO_CONTROL_SOURCE: &str = "permission-system-command";

/// pi `saved.error ?? "Failed to persist pi-permission-system config."` (v0.8.0 `index.ts:1439`),
/// rebranded. Reached only if [`ExtensionConfig::save`] ever reports failure without an error
/// string, which it does not today — carried because upstream carries it.
pub(super) const YOLO_PERSIST_FALLBACK_ERROR: &str =
    "Failed to persist cyrup-permission-system config.";

/// PERM-029 — the shipped JSON Schema for `cyrup-permissions.jsonc`, a rebranded port of upstream's
/// `schemas/permissions.schema.json` @v0.8.0 (`$id` and title rebranded; every keyword, `$def`,
/// `patternProperties` entry and description otherwise upstream's). Embedded rather than merely
/// shipped so `/permission-system schema` can emit it from a running binary with no install-layout
/// assumptions, and so [`schema_is_wellformed`] can validate it at test time — cyrup's analog of
/// upstream's `scripts/validate-artifacts.mjs:50` wired into the package `check` script.
pub const PERMISSIONS_JSON_SCHEMA: &str =
    include_str!("../../schemas/cyrup-permissions.schema.json");

/// PERM-029 — the starter policy, a rebranded port of upstream's `config/config.example.json`
/// @v0.8.0. Every category the manager understands appears at least once, so an operator has a
/// working template rather than a blank file. Validated against a real [`PermissionManager`] by
/// this crate's own test, which is what upstream's `validate-artifacts.mjs:56` buys.
pub const PERMISSIONS_EXAMPLE_CONFIG: &str = include_str!("../../config/config.example.json");
