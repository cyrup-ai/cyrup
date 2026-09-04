//! The binary wiring entry point: the DI-5 install probe and the factory the three
//! `crates/cyrup/src/main.rs` session-build sites call.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_ext::NativeExtension;

use crate::ext_config::ExtensionConfig;

use super::PermissionSystemExtension;
use super::env::{INSTALL_ENV_VAR, env_truthy, is_subagent_child};
use super::paths::{POLICY_FILE, PROJECT_AGENT_SUBDIR, policy_agent_dir};

// Doc-link-only imports: these are named by prose relocated verbatim from the single-file
// `extension.rs`, where they were in scope for real code. `#[cfg(doc)]` keeps those intra-doc
// links resolving without adding an import the compiled build does not use.
#[cfg(doc)]
use crate::ask::{ForwardingAskChannel, LocalAskChannel};

/// True iff `dir` is a readable directory holding at least one entry (PERM-023's install signal).
///
/// An unreadable-but-present directory reports `false` here rather than the fail-safe `true`
/// [`ExtensionConfig::is_pristine_default_file`] uses for the ambiguous case, and deliberately so:
/// there, "I cannot read the file" means "I cannot rule out that it was configured"; here, an
/// `agents/` the process cannot even list is one the `PermissionManager` cannot load frontmatter
/// from either, so attaching the gate on it would advertise enforcement that will not happen.
fn dir_has_entry(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some())
}

/// DI-5 "installed" detection (opt-in): the gate attaches only when the user has installed it —
/// either the [`INSTALL_ENV_VAR`] is truthy, or a policy file exists, or the extension config has
/// been edited away from its auto-generated template. NOT installed → zero gating (unchanged core
/// behavior); installed → default-ASK per category (faithful to pi `permission-manager.ts:44-50`).
/// This keeps the crate compiled + wired at all three sites while never bricking the default
/// (policy-less) app with fail-closed asks on every tool.
///
/// **Every install signal is reversible** (PERM-002). Before this, the probe accepted the bare
/// EXISTENCE of `<agent_dir>/cyrup-permission-system/config.json` — but that file is written by
/// this crate itself, unconditionally, as a side effect of constructing the extension
/// (`ExtensionConfig::ensure_on_disk` via the load in [`PermissionSystemExtension::new`]). So a
/// single `CYRUP_PERMISSION_SYSTEM=1` run left a permanent artifact behind that kept the gate
/// armed forever after, with no supported way to turn it back off: unsetting the env var did
/// nothing, and the file silently reappeared on the very next run if deleted.
///
/// The chosen semantics: `config.json` counts as an install signal only once its bytes DIFFER
/// from the pristine template ([`ExtensionConfig::is_pristine_default_file`]) — i.e. once a human
/// actually configured something. Both directions of the security argument are covered:
/// - It cannot silently DISABLE a gate an operator intended. The env var is untouched; both
///   policy paths are files only a human writes; and a hand-authored (therefore non-pristine)
///   `config.json` still installs, so an operator whose only install signal was that file keeps
///   the gate. An unreadable `config.json` is likewise treated as configured (fail-safe).
/// - It cannot leave an operator PERMANENTLY stuck with a gate they never asked for: the only
///   case it newly returns `false` is the untouched, machine-written template, where no policy
///   file and no env opt-in exist either — a state in which the manager would have had no rules
///   at all and merely defaulted every category to `ask`.
///
/// Upstream `pi-permission-system` has no "installed" probe to copy (the extension gates whatever
/// loads it); its v0.8.0 answer to "how do I turn this off" is a separate `"enabled": false`
/// master switch in `config.json` (`extension-config.ts:11-12,88` → `index.ts:1473-1477`). That
/// switch is now ported too (see [`permission_extension_for_env`]) and is complementary to — not a
/// substitute for — un-latching this probe: it is an explicit operator decision recorded in the
/// file, whereas this probe is about a file the crate wrote to itself.
///
/// The two compose in the only order that works: `"enabled": false` is by definition NOT the
/// pristine template, so it reads as an install signal here and the `enabled` check downstream is
/// the thing that actually declines to attach.
#[must_use]
pub fn is_installed(agent_dir: &Path, cwd: &Path) -> bool {
    if env_truthy(INSTALL_ENV_VAR) {
        return true;
    }
    let project_dir = PROJECT_AGENT_SUBDIR
        .iter()
        .fold(cwd.to_path_buf(), |acc, seg| acc.join(seg));
    // PERM-025: the GLOBAL policy file is probed at the same relocatable root
    // `manager_paths_for` enforces from, so the probe and the engine can never inspect two
    // different trees (the PERM-018 property, one rung up).
    let policy_dir = policy_agent_dir(agent_dir);
    if [policy_dir.join(POLICY_FILE), project_dir.join(POLICY_FILE)]
        .iter()
        .any(|p| p.exists())
    {
        return true;
    }
    // PERM-023: agent-scoped `permission:` frontmatter is an ENFORCED policy layer —
    // `manager_paths_for` wires `agents_dir` / `project_agents_dir` and
    // `PermissionManager::load_agent_permissions` reads `<agents_dir>/<agent>.md` on every check
    // (pi `loadAgentPermissionsFrom` via `resolveAgentMarkdownPath`,
    // `permission-manager.ts:582-595`, `:715-745` @v0.8.0). Probing only the two `.jsonc` files
    // left an operator whose ONLY policy artifact is a persona's frontmatter with no extension
    // attached and their deny rules silently inert — a fail-open.
    //
    // "Non-empty", not "exists": neither directory is ever written by this crate (unlike
    // `config.json`, whose auto-materialization produced PERM-002's latch), but an empty
    // `agents/` left behind by another tool is not an authored policy.
    if [policy_dir.join("agents"), project_dir.join("agents")]
        .iter()
        .any(|p| dir_has_entry(p))
    {
        return true;
    }
    // The RESOLVED path, not the raw default: `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH` can point the
    // extension at a different file entirely, and both `ExtensionConfig::load` (the `enabled`
    // switch, below) and `ExtensionConfig::save` (the `/permission-system` writers) already honour
    // it. Reading the raw path here let the install decision and the on/off decision inspect two
    // different files and disagree — pi has one `getPermissionSystemConfigPath()` and every consumer
    // goes through it (`extension-config.ts:51-53`).
    let config_path = PermissionSystemExtension::resolved_config_path_for(agent_dir);
    config_path.exists() && !ExtensionConfig::is_pristine_default_file(&config_path)
}

/// The binary-side wiring entry point `crates/cyrup/src/main.rs` calls at each of its three
/// session-build sites (mirrors `cyrup_ext_subagents::extension::subagent_extension_for_env`).
///
/// Role is selected by the `CYRUP_SUBAGENT_CHILD` / depth signal (port doc §3.1 item 4, pi's `hasUI`
/// vs `isSubagentExecutionContext` split, `index.ts:1506-1519`):
/// - **CHILD** (`CYRUP_SUBAGENT_CHILD`): loads the gate with a [`ForwardingAskChannel`]
///   ([`PermissionSystemExtension::new_forwarding_child`]) — an ask-tier decision FORWARDS up to the
///   parent's human via the spool instead of dying. (P-4; previously this returned `None`, leaving a
///   child's `ask` with no reachable human — the exact gap this build closes.)
/// - **PARENT** (root, `DEPTH == 0`): loads the gate with the [`LocalAskChannel`] in-session dialog +
///   the forwarding WATCHER ([`PermissionSystemExtension::new_forwarding_parent`]).
///
/// Returns `None` (attach nothing → DI-5 zero gating) when the gate is not installed, or when it
/// is installed but `config.json` sets the `enabled` master switch to `false`.
#[must_use]
pub fn permission_extension_for_env(
    agent_dir: PathBuf,
    cwd: PathBuf,
) -> Option<Arc<dyn NativeExtension>> {
    if !is_installed(&agent_dir, &cwd) {
        return None;
    }
    // pi's `enabled` master switch (`extension-config.ts:11-12` "When false, the extension skips
    // all registrations and startup work"): `index.ts:1473-1477` loads the extension config and
    // then `if (!extensionConfig.enabled) { return; }` — a bare early return out of the extension
    // entry point `piPermissionSystemExtension(pi)` (`index.ts:1308`), before
    // `applyExtensionConfigSideEffects` (`:1479`), before the runtime-API registration (`:1481`)
    // and before every handler / command / status registration.
    //
    // This function is cyrup's analog of that entry point: returning `None` means the binary
    // wiring attaches no `NativeExtension` at all, so nothing subscribes and no startup work runs
    // — the same observable outcome as pi's early return. Only the literal `false` disables; see
    // `ExtensionConfig::normalize`.
    //
    // Deliberately AFTER `is_installed`: an operator with no config at all must not pay a config
    // load (nor have the template materialized on their disk merely by our deciding not to attach),
    // and an `"enabled": false` file is non-pristine, so it passes the install probe and lands here
    // (which is exactly where it should be declined).
    //
    // This is THE load for the whole session — pi's single `loadExtensionConfigState()` at
    // `index.ts:1473`, whose result both the `enabled` test (`:1475-1477`) and every downstream
    // consumer reuse. It is threaded into the constructor below rather than re-read there; see
    // `PermissionSystemExtension::load_config`.
    let config = PermissionSystemExtension::load_config(&agent_dir);
    if !config.enabled {
        return None;
    }
    if is_subagent_child() {
        // CHILD: forward asks up to the parent (§7.4). The parent-session anchor
        // `CYRUP_SUBAGENT_PARENT_SESSION` (emitted by `cyrup-ext-subagents`, `exec/mod.rs`
        // `PARENT_SESSION_ENV_VAR`) addresses the parent's inbox; the `ForwardingAskChannel` reads it.
        // `into_shared` rather than `Arc::new` (PERM-011 half A): it installs the `Weak`
        // back-reference the published runtime API borrows through, so `init` has something to
        // publish. A child publishes too — upstream's registration is in the activation body,
        // which runs in both roles.
        return Some(
            PermissionSystemExtension::new_forwarding_child_with_config(agent_dir, cwd, config)
                .into_shared(),
        );
    }
    // PARENT: in-session dialog + the forwarding watcher (installed on SessionStart).
    Some(
        PermissionSystemExtension::new_forwarding_parent_with_config(agent_dir, cwd, config)
            .into_shared(),
    )
}
