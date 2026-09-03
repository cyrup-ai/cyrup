//! The child-mode registration gate and the opt-in install check that decide how much of the
//! extension surface is registered, plus the three factories the binary calls.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_ext::native::NativeExtension;

use crate::registration::SubagentExtensionConfig;
use crate::extension::host::SubagentsExtension;

/// How much of the extension surface [`NativeExtension::init`] registers — the child-mode gate
/// (T6, pi `extension/index.ts:243-245` + `extension/fanout-child.ts:131`).
///
/// A subagent child process re-execs the `cyrup` binary with `CYRUP_SUBAGENT_CHILD=1` set. In that
/// child, pi's root `registerSubagentExtension` returns immediately and registers NOTHING — a child
/// must never install the full orchestrator surface (its own `subagent` tool, the 12 slash commands,
/// the background-completion watcher, the session-lifecycle housekeeping), which would let it spawn
/// grandchildren freely and duplicate the parent's UI. The one exception is a **fanout-authorized**
/// child (`CYRUP_SUBAGENT_FANOUT_CHILD=1` as well), which pi's separate `fanout-child` entry point
/// gives a single **restricted** `subagent` tool: it may delegate/inspect but the agent-config
/// mutation actions (`create`/`update`/`delete`) are blocked, and it installs no slash commands or
/// watchers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistrationMode {
    /// The root orchestrator surface: the `subagent` tool, all 12 slash commands, the
    /// background-completion watcher, and session-start housekeeping (non-child process).
    Full,
    /// A fanout-authorized child: only the restricted, mutation-blocked `subagent` tool — no slash
    /// commands, no watchers, no session-lifecycle housekeeping.
    ChildSafe,
}

/// The child-mode registration decision (T6, pi `extension/index.ts:243-245` +
/// `extension/fanout-child.ts:131`), as a pure function of the two env flags so it is deterministic
/// and unit-testable without mutating the process environment:
/// - not a child (`child == false`) → [`RegistrationMode::Full`];
/// - a fanout-authorized child (`child && fanout_authorized`) → [`RegistrationMode::ChildSafe`];
/// - a plain child (`child && !fanout_authorized`) → `None`: register NOTHING at all.
#[must_use]
pub fn resolve_registration_mode(child: bool, fanout_authorized: bool) -> Option<RegistrationMode> {
    if !child {
        return Some(RegistrationMode::Full);
    }
    if fanout_authorized {
        return Some(RegistrationMode::ChildSafe);
    }
    None
}

/// Read the two child-mode env flags (`CYRUP_SUBAGENT_CHILD` / `CYRUP_SUBAGENT_FANOUT_CHILD`) and
/// resolve the [`RegistrationMode`] via [`resolve_registration_mode`]. `None` means the current
/// process is a plain subagent child that must register no subagent surface at all.
#[must_use]
pub fn registration_mode_from_env() -> Option<RegistrationMode> {
    let is_one = |name: &str| std::env::var(name).ok().as_deref() == Some("1");
    resolve_registration_mode(
        is_one(crate::spawn::nested_events::CHILD_ENV),
        is_one(crate::spawn::nested_events::FANOUT_CHILD_ENV),
    )
}

/// The opt-in install env var for the SubAgents extension, mirroring its two sibling companions
/// EXACTLY (`cyrup_intercom::INSTALL_ENV_VAR` = `CYRUP_INTERCOM`,
/// `cyrup_permission_system::INSTALL_ENV_VAR` = `CYRUP_PERMISSION_SYSTEM`). In `pi`, `pi-subagents`
/// is an OPTIONAL installable package; cyrup matches that — default OFF, attached for a plain
/// top-level session only when opted in (see [`is_installed`]). When truthy, the orchestrator
/// surface attaches even with no on-disk config file.
pub const INSTALL_ENV_VAR: &str = "CYRUP_SUBAGENTS";

/// `<agent_dir>/subagents/config.json` is the tier-3 per-installation extension config (R-SA-133
/// tier 3) that `crates/cyrup/src/subagent_config.rs` loads; its mere PRESENCE is an install signal
/// (the user created a subagents config, so they want the extension).
const CONFIG_SUBDIR: &str = "subagents";
const CONFIG_FILE: &str = "config.json";
/// The project-scope opt-in location: `<cwd>/.cyrup/subagents/config.json`.
const PROJECT_SUBDIR: &str = ".cyrup";

/// Truthy-env test, identical to the two sibling companions' own `env_truthy`
/// (`cyrup_intercom` / `cyrup_permission_system`): `1`/`true`/`on`/`yes` (trimmed) are truthy.
/// `env_truthy` over an injected lookup.
fn env_truthy_with(get: &dyn Fn(&str) -> Option<String>, name: &str) -> bool {
    matches!(
        get(name).as_deref().map(str::trim),
        Some("1") | Some("true") | Some("on") | Some("yes")
    )
}

/// Whether the SubAgents extension is "installed" (opt-in) for a plain top-level session — mirrors
/// `cyrup_intercom::is_installed` / `cyrup_permission_system::is_installed` EXACTLY: an explicit
/// [`INSTALL_ENV_VAR`] (`CYRUP_SUBAGENTS`) opt-in, OR the presence of the tier-3 `config.json` at
/// user scope (`<agent_dir>/subagents/config.json`, the file `crates/cyrup/src/subagent_config.rs`
/// loads) OR project scope (`<cwd>/.cyrup/subagents/config.json`). NOT installed → a plain top-level
/// session attaches NOTHING (zero overhead, default OFF). A spawned fanout child
/// ([`RegistrationMode::ChildSafe`]) attaches REGARDLESS of this — exactly as intercom's
/// child-orchestrator-metadata presence bypasses its own `is_installed` (its already-installed
/// parent spawned it, so the child needs the restricted surface regardless).
#[must_use]
pub fn is_installed(agent_dir: &Path, cwd: &Path) -> bool {
    is_installed_with(&|k| std::env::var(k).ok(), agent_dir, cwd)
}

/// [`is_installed`] over an injected environment lookup.
///
/// The `CYRUP_SUBAGENTS` branch is the half a caller usually wants to pin, and pinning it here
/// costs nothing: the file-presence half below is unchanged, so a test proves the real precedence
/// (env first, then either config file) without moving a process-global variable that every
/// concurrent reader of the environment races.
#[must_use]
pub fn is_installed_with(
    get: &dyn Fn(&str) -> Option<String>,
    agent_dir: &Path,
    cwd: &Path,
) -> bool {
    if env_truthy_with(get, INSTALL_ENV_VAR) {
        return true;
    }
    [
        agent_dir.join(CONFIG_SUBDIR).join(CONFIG_FILE),
        cwd.join(PROJECT_SUBDIR).join(CONFIG_SUBDIR).join(CONFIG_FILE),
    ]
    .iter()
    .any(|p| p.exists())
}

/// Compose the child-mode gate ([`resolve_registration_mode`]) with the opt-in install signal
/// (item 2 of the opt-in fix): a top-level [`RegistrationMode::Full`] survives ONLY when `installed`;
/// a [`RegistrationMode::ChildSafe`] fanout child survives REGARDLESS (its already-installed parent
/// spawned it — mirroring intercom's metadata-present bypass). Pure over its inputs so the composed
/// gate is unit-testable without touching env or the filesystem.
#[must_use]
fn gate_on_install(mode: RegistrationMode, installed: bool) -> Option<RegistrationMode> {
    match mode {
        RegistrationMode::ChildSafe => Some(RegistrationMode::ChildSafe),
        RegistrationMode::Full => installed.then_some(RegistrationMode::Full),
    }
}

/// Build the subagent [`NativeExtension`] the `cyrup` binary should attach for the current process,
/// or `None` when it must attach nothing — the crate-side half of the T6 child-mode gate composed
/// with the opt-in install gate ([`is_installed`]), which `crates/cyrup/src/main.rs` calls at each of
/// its three session-build sites. `None` is returned for a plain subagent child (registers nothing),
/// and ALSO for a plain top-level session that has NOT opted in (default OFF). A fanout-authorized
/// child attaches its restricted surface REGARDLESS of `is_installed`. See [`subagent_extension_for`]
/// for the pure, env-free form.
#[must_use]
pub fn subagent_extension_for_env(
    agent_dir: &Path,
    config: SubagentExtensionConfig,
    cwd: PathBuf,
) -> Option<Arc<dyn NativeExtension>> {
    let installed = is_installed(agent_dir, &cwd);
    registration_mode_from_env()
        .and_then(|mode| gate_on_install(mode, installed))
        .map(|mode| Arc::new(SubagentsExtension::with_mode(config, cwd, mode)) as Arc<dyn NativeExtension>)
}

/// As [`subagent_extension_for_env`], but threads the intercom companion's real broker-backed
/// delivery + clarify + steer channels into the ROOT-orchestrator extension (item 2 of
/// reconciliation §4 step 5 / the port doc §8.4 item 1 handoff) — CLOSING
/// R-SA-037/086/119/120/123/124/125. The channels are handed only to a [`RegistrationMode::Full`]
/// root (the only surface that drives grouped tool results, surfaces a clarify to a live human, and
/// steers a live async child); a [`RegistrationMode::ChildSafe`] fanout child is built WITHOUT them
/// (it has no orchestrator surface), and a plain child still returns `None`.
/// `crates/cyrup/src/main.rs` calls this with
/// `IntercomExtension::{delivery_channel,clarify_channel,steer_channel}` when intercom is attached
/// this session, and falls back to [`subagent_extension_for_env`] when it is not.
#[must_use]
pub fn subagent_extension_for_env_with_channels(
    agent_dir: &Path,
    config: SubagentExtensionConfig,
    cwd: PathBuf,
    delivery: Arc<dyn crate::tui::intercom::DeliveryChannel>,
    clarify: Arc<dyn crate::tui::intercom::ClarifyChannel>,
    steer: Arc<dyn crate::tui::intercom::SteerChannel>,
) -> Option<Arc<dyn NativeExtension>> {
    let installed = is_installed(agent_dir, &cwd);
    registration_mode_from_env()
        .and_then(|mode| gate_on_install(mode, installed))
        .map(|mode| match mode {
            RegistrationMode::Full => {
                Arc::new(SubagentsExtension::with_channels(config, cwd, delivery, clarify, steer))
                    as Arc<dyn NativeExtension>
            }
            RegistrationMode::ChildSafe => {
                Arc::new(SubagentsExtension::with_mode(config, cwd, RegistrationMode::ChildSafe))
                    as Arc<dyn NativeExtension>
            }
        })
}

/// The pure, env-free form of [`subagent_extension_for_env`]: resolve the [`RegistrationMode`] from
/// the two explicit child flags, compose it with the explicit `installed` opt-in signal
/// ([`gate_on_install`]), and build the extension (or `None` to register nothing). Kept separate so a
/// test can assert the full gate deterministically without touching the process environment or the
/// filesystem: a plain child registers nothing; a top-level session registers only when `installed`;
/// a fanout-authorized child registers its restricted surface REGARDLESS of `installed`.
#[must_use]
pub fn subagent_extension_for(
    config: SubagentExtensionConfig,
    cwd: PathBuf,
    child: bool,
    fanout_authorized: bool,
    installed: bool,
) -> Option<Arc<dyn NativeExtension>> {
    resolve_registration_mode(child, fanout_authorized)
        .and_then(|mode| gate_on_install(mode, installed))
        .map(|mode| Arc::new(SubagentsExtension::with_mode(config, cwd, mode)) as Arc<dyn NativeExtension>)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;

    /// T6 child-mode gate (pi `extension/index.ts:243-245` + `extension/fanout-child.ts:131`): the
    /// pure decision function encoding "a plain subagent child registers nothing; a fanout-authorized
    /// child gets the restricted tool; a non-child gets the full surface."
    #[test]
    fn resolve_registration_mode_encodes_the_child_gate() {
        // Not a child → full orchestrator surface (the fanout flag is irrelevant when not a child).
        assert_eq!(resolve_registration_mode(false, false), Some(RegistrationMode::Full));
        assert_eq!(resolve_registration_mode(false, true), Some(RegistrationMode::Full));
        // Fanout-authorized child → the restricted child-safe tool.
        assert_eq!(resolve_registration_mode(true, true), Some(RegistrationMode::ChildSafe));
        // Plain subagent child → register NOTHING.
        assert_eq!(resolve_registration_mode(true, false), None);
    }

    /// Opt-in gate, default OFF (mirrors `cyrup_permission_system` / `cyrup_intercom`): a plain
    /// TOP-LEVEL (non-child) session with NO `CYRUP_SUBAGENTS` env and NO `subagents/config.json`
    /// attaches NOTHING. Proven via the pure form with `installed = false` — deterministic, touching
    /// neither the env nor the filesystem — which is exactly the value `is_installed` yields in the
    /// no-opt-in state. (Requirement (a).)
    #[test]
    fn top_level_without_optin_attaches_nothing() {
        let cwd = std::env::temp_dir();
        let none = subagent_extension_for(
            SubagentExtensionConfig::default(),
            cwd,
            /* child */ false,
            /* fanout_authorized */ false,
            /* installed */ false,
        );
        assert!(none.is_none(), "a top-level session that has not opted in attaches nothing");
    }

    /// A fanout-authorized CHILD ([`RegistrationMode::ChildSafe`]) attaches its restricted surface
    /// REGARDLESS of the opt-in signal: with `installed = false` (no env, no config) it STILL yields an
    /// extension — mirroring intercom's child-orchestrator-metadata bypass of its own `is_installed`
    /// (its already-installed parent spawned it). (Requirement (d).)
    #[test]
    fn fanout_child_attaches_regardless_of_optin() {
        let cwd = std::env::temp_dir();
        let ext = subagent_extension_for(
            SubagentExtensionConfig::default(),
            cwd,
            /* child */ true,
            /* fanout_authorized */ true,
            /* installed */ false,
        );
        assert!(
            ext.is_some(),
            "a fanout-authorized child attaches its restricted surface regardless of is_installed"
        );
    }

    /// `is_installed`'s two config-file signals (the env branch is exercised in the `tests/`
    /// integration file, since this crate is `#![forbid(unsafe_code)]` and cannot mutate the process
    /// env in a `src/` test): a tier-3 `<agent_dir>/subagents/config.json` at user scope, OR
    /// `<cwd>/.cyrup/subagents/config.json` at project scope, each mark the extension installed; with
    /// neither present (and no env), it is NOT installed.
    #[test]
    fn is_installed_reads_the_config_file_signals() {
        let agent = tempfile::tempdir().expect("agent dir");
        let cwd = tempfile::tempdir().expect("cwd");
        // `is_installed` ORs the `CYRUP_SUBAGENTS` env signal with the config-file signals. This
        // used to read the AMBIENT value and assert the result merely matched it, which could not
        // fail either way — a developer shell with `CYRUP_SUBAGENTS=1` made every case below
        // vacuously true. `is_installed_with` pins the env half instead, so the file signals are
        // what is actually under test, with no `set_var` and no dependence on the caller's shell.
        let unset = |_: &str| None;

        // Neither file present, env not opted in → not installed.
        assert!(!is_installed_with(&unset, agent.path(), cwd.path()));

        // Env opted in on its own is sufficient, with no file present at all.
        assert!(is_installed_with(&|_| Some("1".to_string()), agent.path(), cwd.path()));

        // User-scope tier-3 config present → installed regardless of env.
        let user_cfg = agent.path().join("subagents");
        std::fs::create_dir_all(&user_cfg).expect("mkdir user subagents");
        std::fs::write(user_cfg.join("config.json"), "{}").expect("write user config");
        assert!(is_installed_with(&unset, agent.path(), cwd.path()));

        // Project-scope config present (with a FRESH agent dir that has no user config) → NOT
        // installed until the project config is written.
        let agent2 = tempfile::tempdir().expect("agent dir 2");
        assert!(
            !is_installed_with(&unset, agent2.path(), cwd.path()),
            "sanity: agent2 has no user config yet"
        );
        let proj_cfg = cwd.path().join(".cyrup").join("subagents");
        std::fs::create_dir_all(&proj_cfg).expect("mkdir project subagents");
        std::fs::write(proj_cfg.join("config.json"), "{}").expect("write project config");
        assert!(is_installed_with(&unset, agent2.path(), cwd.path()));
    }

    /// The composed install gate ([`gate_on_install`]) in isolation: a top-level `Full` survives ONLY
    /// when installed; a `ChildSafe` fanout child survives REGARDLESS.
    #[test]
    fn gate_on_install_only_gates_full() {
        assert_eq!(gate_on_install(RegistrationMode::Full, true), Some(RegistrationMode::Full));
        assert_eq!(gate_on_install(RegistrationMode::Full, false), None);
        assert_eq!(
            gate_on_install(RegistrationMode::ChildSafe, true),
            Some(RegistrationMode::ChildSafe)
        );
        assert_eq!(
            gate_on_install(RegistrationMode::ChildSafe, false),
            Some(RegistrationMode::ChildSafe)
        );
    }

}
