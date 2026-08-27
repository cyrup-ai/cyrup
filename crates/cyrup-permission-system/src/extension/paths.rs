//! Where the permission system's files live on disk: the policy file, the extension `config.json`,
//! the JSONL log dir, and the `agent_dir` override that moves all of them.

use std::path::{Path, PathBuf};

use crate::ext_config::ExtensionConfig;
use crate::manager::ManagerPaths;

use super::PermissionSystemExtension;
use super::env::POLICY_AGENT_DIR_ENV_KEY;

/// The global policy file (pi `pi-permissions.jsonc`; cyrup analog).
pub(super) const POLICY_FILE: &str = "cyrup-permissions.jsonc";
/// The extension config dir + file (`<agent_dir>/cyrup-permission-system/config.json`).
pub(super) const CONFIG_DIR: &str = "cyrup-permission-system";
pub(super) const CONFIG_FILE: &str = "config.json";
/// The project-scoped policy dir (pi `<cwd>/.pi/agent`; cyrup `<cwd>/.cyrup/agent`).
pub(super) const PROJECT_AGENT_SUBDIR: [&str; 2] = [".cyrup", "agent"];

/// pi `defaultPolicyAgentDir()` (v0.8.0 `permission-manager.ts:31-33`):
/// `const override = process.env[KEY]?.trim(); return override ? resolve(override) : getAgentDir();`
///
/// The precedence is exactly upstream's: an env value that trims to the empty string is NOT an
/// override (JS `""` is falsy), and a non-empty one is `resolve`d — absolutized against the process
/// cwd — before use. [`std::path::absolute`] is the direct analog of node's `path.resolve` for a
/// single argument: it is purely lexical and never touches the filesystem, so a not-yet-created
/// policy root still resolves. On the (io-error) failure path the trimmed value is used as given,
/// which is what `resolve` would have produced for an already-absolute path.
///
/// **The probe and the engine must both go through this**, or they inspect different trees and
/// disagree — the PERM-018 hazard, one rung up: [`PermissionSystemExtension::manager_paths_for`]
/// builds the enforced paths from it and [`crate::is_installed`] probes it.
#[must_use]
pub(super) fn policy_agent_dir(agent_dir: &Path) -> PathBuf {
    let Some(raw) = std::env::var(POLICY_AGENT_DIR_ENV_KEY)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    else {
        return agent_dir.to_path_buf();
    };
    let raw = PathBuf::from(raw);
    std::path::absolute(&raw).unwrap_or(raw)
}

impl PermissionSystemExtension {
    /// Derive the [`ManagerPaths`] for `agent_dir` + `project_cwd` (pi
    /// `createPermissionManagerForCwd`'s path derivation, `index.ts:1536-1573`) — shared by every
    /// constructor AND by [`Self::refresh_config_and_manager`] (a `session_start` /
    /// `resources_discover` reload rebuilds this from the CURRENT cwd, not just the process's
    /// original one).
    ///
    /// `project_cwd` is `None` when the project scope must be withheld — pi
    /// `permissionManager.configureForCwd(projectTrusted ? ctx.cwd : undefined)`
    /// (`permission-session.ts:106-110`, `:132-136`, #644). The parameter is an `Option` rather
    /// than a companion `bool` because `cwd` is read for NOTHING ELSE here: withholding it IS
    /// withholding the project scope, so the two cannot drift apart.
    pub(super) fn manager_paths_for(agent_dir: &Path, project_cwd: Option<&Path>) -> ManagerPaths {
        let project_dir = project_cwd
            .map(|cwd| PROJECT_AGENT_SUBDIR.iter().fold(cwd.to_path_buf(), |acc, seg| acc.join(seg)));
        // PERM-025 / pi `defaultGlobalConfigPath` / `defaultAgentsDir` /
        // `defaultLegacyGlobalSettingsPath` / `defaultGlobalMcpConfigPath`
        // (v0.8.0 `permission-manager.ts:35-38`): all four GLOBAL artifacts hang off
        // `defaultPolicyAgentDir()`, i.e. the `POLICY_AGENT_DIR_ENV_KEY` override when set. The two
        // PROJECT paths are supplied explicitly upstream too (`index.ts:1296-1300`) and are NOT
        // relocated.
        let policy_dir = policy_agent_dir(agent_dir);
        ManagerPaths {
            global_config_path: policy_dir.join(POLICY_FILE),
            agents_dir: policy_dir.join("agents"),
            project_global_config_path: project_dir.as_ref().map(|d| d.join(POLICY_FILE)),
            project_agents_dir: project_dir.map(|d| d.join("agents")),
            legacy_global_settings_path: policy_dir.join("settings.json"),
            global_mcp_config_path: policy_dir.join("mcp.json"),
            mcp_server_names_override: None,
        }
    }

    /// The DEFAULT extension `config.json` path for `agent_dir` — cyrup's analog of pi's
    /// `CONFIG_PATH` constant (`extension-config.ts:41`, `join(EXTENSION_ROOT, "config.json")`).
    ///
    /// This is the *unresolved* default. Nothing outside this crate should read a config from it
    /// directly: `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH` can point the extension at a different file
    /// entirely, and only [`Self::resolved_config_path_for`] honours that. Every consumer here
    /// either goes through that helper or through [`ExtensionConfig::load`] /
    /// [`ExtensionConfig::save`], which resolve internally.
    pub(crate) fn config_path_for(agent_dir: &Path) -> PathBuf {
        agent_dir.join(CONFIG_DIR).join(CONFIG_FILE)
    }

    /// The RESOLVED extension `config.json` path for `agent_dir`: [`Self::config_path_for`] after
    /// the `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH` override, i.e. pi
    /// `getPermissionSystemConfigPath()` (v0.8.0 `extension-config.ts:51-53`, over
    /// `resolveOverridablePath`, `:46-49`).
    ///
    /// pi has exactly one such accessor and every consumer of the extension config funnels through
    /// it — `loadPermissionSystemConfig`'s default argument (`extension-config.ts:117`),
    /// `savePermissionSystemConfig`'s (`:240`), and the config modal's displayed `Config file:` path
    /// (`index.ts:1509`). cyrup's [`crate::is_installed`] probe was reading the RAW default path instead,
    /// so with the override set the install decision and the `enabled` decision could inspect two
    /// different files and disagree. This helper is the one accessor; use it, not
    /// [`Self::config_path_for`].
    pub(crate) fn resolved_config_path_for(agent_dir: &Path) -> PathBuf {
        ExtensionConfig::resolve_config_path(&Self::config_path_for(agent_dir))
    }

    /// The default audit/debug log directory for `agent_dir` (pi `LOGS_DIR =
    /// join(EXTENSION_ROOT, "logs")`, `extension-config.ts:38`). cyrup's analog of pi's
    /// `EXTENSION_ROOT` is `<agent_dir>/cyrup-permission-system/` — the directory
    /// [`Self::config_path_for`] puts `config.json` in — so the trail lands beside the config that
    /// enables it. Overridable per write via `CYRUP_PERMISSION_SYSTEM_LOGS_DIR`
    /// ([`crate::logging::resolve_logs_dir`]).
    pub(super) fn logs_dir_for(agent_dir: &Path) -> PathBuf {
        agent_dir.join(CONFIG_DIR).join(crate::logging::LOGS_DIR_NAME)
    }
}
