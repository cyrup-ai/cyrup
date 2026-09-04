//! The `/permission-system` slash command — cyrup's expression of pi's settings modal over the
//! same two rows, plus the `schema` / `example` artifact dumps and the settings overlay.

use cyrup_ext::NotifyKind;

use crate::ext_config::ExtensionConfig;

use super::consts::{PERMISSIONS_EXAMPLE_CONFIG, PERMISSIONS_JSON_SCHEMA};
use super::paths::{POLICY_FILE, policy_agent_dir};
use super::{PermissionSystemExtension, guard};

/// The `/permission-system` usage line. The two setting ids and the `on`/`off` value set are pi's
/// (`config-modal.ts:18,27,34`); the textual framing is cyrup's, since upstream renders a modal.
const COMMAND_USAGE: &str = "Usage: /permission-system [debug|yoloMode on|off] [schema] [example]";

/// pi `toOnOff` (`config-modal.ts:20-22`).
fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}

impl PermissionSystemExtension {
    /// The `/permission-system` handler body (pi `index.ts:1504-1511` via
    /// `createPermissionSystemCommandHandler`, `common.ts:188-198`), reached from
    /// [`cyrup_ext::NativeExtension::execute_command`].
    ///
    /// \[CYRUP-DELTA] Upstream's body is `openPermissionSystemSettingsModal(ctx, controller)`
    /// (`config-modal.ts:63-123`): a `ctx.ui.custom` overlay rendering pi's own `ZellijSettingsModal`
    /// over two rows. This handler is a textual form of the same controller instead: the same two
    /// setting ids, the same `on`/`off` value set (`config-modal.ts:18`), the same `applySetting`
    /// mapping (`:43-56`), the same `setConfig` writer, and the same `Config file: <path>` help
    /// text (`:85`).
    ///
    /// **PERM-007 — the reason recorded here was STALE and is corrected.** It claimed
    /// "`HostServices` exposes no custom-overlay seam". One exists:
    /// `cyrup_ext::HostServices::open_overlay` (`cyrup-ext/src/host/services.rs`) over
    /// `cyrup_ext::host::overlay::InteractiveOverlay`, with a live implementation in
    /// `cyrup-session-svc`'s `LiveHostServices` and a production caller in
    /// `cyrup-ext-subagents`. The `cyrup-tui` half of the old reason still holds and is why the
    /// seam is shaped as serializable `OverlayLine`s rather than ratatui types, but it is not a
    /// reason the modal cannot be built. What remains is the work itself: an `InteractiveOverlay`
    /// implementation is `'static`, so it cannot borrow `&self` and needs the config writer
    /// extracted into a shared controller object — pi's own
    /// `{getConfig, setConfig, getConfigPath}` (`index.ts:1504-1511`) made explicit. Until that
    /// lands, the operator gets a read-only dump plus two blind toggles.
    ///
    /// Grammar (`<setting> <value>`; no args renders the modal's initial view):
    /// - `/permission-system` — current values + config path.
    /// - `/permission-system debug on|off` — pi `applySetting("debug", …)` → `setConfig`.
    /// - `/permission-system yoloMode on|off` — the yolo row.
    ///
    /// BOTH rows go through [`Self::save_extension_config`], matching upstream: the modal's
    /// `onChange` calls `controller.setConfig` for every setting id (`config-modal.ts:74-76`), and
    /// `setConfig` is registered as `saveExtensionConfig` (`index.ts:1508`). `setYoloMode` is a
    /// DIFFERENT surface — upstream's runtime API (`index.ts:1483-1484`), reachable by other
    /// extensions through `globalThis.__piPermissionSystem`, not by this command.
    ///
    /// An earlier revision routed the yolo row through [`Self::set_yolo_mode`] so that method would
    /// have a caller. That was the wrong trade: it changed the emitted debug event
    /// (`yolo_mode.updated` instead of `config.saved`) and the error surface, distorting ported
    /// behaviour to satisfy a reachability rule. [`Self::set_yolo_mode`],
    /// [`Self::toggle_yolo_mode`] and [`Self::yolo_mode`] belong to the OTHER surface, and
    /// PERM-011 half A has now given them upstream's own publish seam: they are registered on the
    /// process-global [`mod@crate::runtime_api`] registry (pi's `globalThis.__piPermissionSystem`,
    /// `yolo-mode-api.ts:20-43`) by [`Self::publish_runtime_api`], so a second extension reaches
    /// them through `crate::runtime_api::runtime_api()` — never through this command.
    ///
    /// Returns `Some(text)` for output the session surfaces as an **Info** notification, and `None`
    /// when this handler has ALREADY notified at its own level — the convention documented on
    /// [`cyrup_ext::NativeExtension::execute_command`]. The save-failure branches take the `None`
    /// route: they raise one [`NotifyKind::Error`] toast carrying both the human sentence and the
    /// raw cause, instead of returning a sentence that would arrive as a second, Info-level toast
    /// alongside the error (`cyrup-session-svc/src/session.rs:961-1004` surfaces every
    /// `Ok(Some(..))`).
    pub(super) fn run_permission_system_command(&self, args: &str) -> Option<String> {
        let mut parts = args.split_whitespace();
        let Some(setting) = parts.next() else {
            // PERM-007 — pi's bare `/permission-system` is
            // `openPermissionSystemSettingsModal(ctx, controller)` (`config-modal.ts:63-122`), a
            // live `ctx.ui.custom(…, { overlay: true, … })`. Hand the host the real overlay; the
            // text dump below is now only the fall-back for a host that owns no interactive
            // surface, which is precisely pi's own `if (!ctx.hasUI)` branch (`common.ts:188-198`)
            // and NOT an error.
            return self.open_settings_overlay();
        };
        // PERM-029 — two zero-argument emitters for the artifacts upstream ships as FILES and
        // documents in its README (`README.md:655`'s CLI validation recipe, `:659`'s "Add
        // `"$schema"`: … to your config for autocomplete support"). cyrup ships them as crate
        // files too, but a Rust binary has no `node_modules` path an operator can point an editor
        // at, so the command is how they are reached from a running install.
        match setting {
            "schema" => return Some(PERMISSIONS_JSON_SCHEMA.to_string()),
            "example" => return Some(PERMISSIONS_EXAMPLE_CONFIG.to_string()),
            _ => {}
        }
        let value = parts.next();
        if parts.next().is_some() {
            return Some(format!("Unexpected extra arguments.\n{COMMAND_USAGE}"));
        }

        // pi `ON_OFF = ["on", "off"]` (`config-modal.ts:18`) — the modal can only ever emit one of
        // these two, so anything else is a usage error rather than `applySetting`'s silent
        // `value === "on"` coercion.
        let enabled = match value {
            Some("on") => true,
            Some("off") => false,
            Some(other) => return Some(format!("Unknown value `{other}`.\n{COMMAND_USAGE}")),
            None => return Some(format!("`{setting}` needs a value.\n{COMMAND_USAGE}")),
        };

        match setting {
            // pi `applySetting` `case "debug"` (`config-modal.ts:49-50`) → `setConfig` (`:78`).
            "debug" => {
                let next = ExtensionConfig {
                    debug: enabled,
                    ..guard(&self.config).clone()
                };
                match self.save_extension_config(&next) {
                    Ok(()) => Some(format!(
                        "Debug logging {}.\n{}",
                        on_off(enabled),
                        self.config_path_line()
                    )),
                    // pi surfaces this through `ctx.ui.notify(saved.error, "error")` ONLY
                    // (`index.ts:1407`) — one error-level toast, nothing else. Same here: the
                    // sentence and the raw cause go out together at Error, and the handler returns
                    // `None` so the session adds no second Info toast.
                    Err(cause) => {
                        self.notify_save_failure(
                            &format!(
                                "Failed to save the permission-system config; debug logging is \
                                 unchanged ({}).",
                                on_off(guard(&self.config).debug)
                            ),
                            &cause,
                        );
                        None
                    }
                }
            }
            // pi `applySetting` `case "yoloMode"` (`config-modal.ts:51-52`) → `setConfig` (`:75`),
            // the SAME writer the debug row uses. Not `setYoloMode` — that is the runtime API.
            "yoloMode" => {
                let next = ExtensionConfig {
                    yolo_mode: enabled,
                    ..guard(&self.config).clone()
                };
                match self.save_extension_config(&next) {
                    Ok(()) => Some(format!(
                        "YOLO mode {}.\n{}",
                        on_off(enabled),
                        self.config_path_line()
                    )),
                    // Same failure shape as the debug row: pi notifies through `ctx.ui.notify` and
                    // leaves the live config untouched (`index.ts:1405-1409`), so the value reported
                    // here is the one still in effect.
                    Err(cause) => {
                        self.notify_save_failure(
                            &format!(
                                "Failed to save the permission-system config; YOLO mode is \
                                 unchanged ({}).",
                                on_off(guard(&self.config).yolo_mode)
                            ),
                            &cause,
                        );
                        None
                    }
                }
            }
            // pi `applySetting`'s `default: return config` (`config-modal.ts:53-54`) — the modal
            // cannot emit an unknown id, so cyrup's text form reports it instead of silently
            // no-oping.
            other => Some(format!("Unknown setting `{other}`.\n{COMMAND_USAGE}")),
        }
    }

    /// Raise the ONE [`NotifyKind::Error`] toast a refused config write produces: the human sentence
    /// (what did not change, and what is still in effect), the config path, and the raw cause from
    /// [`Self::save_extension_config`] (why). pi emits only `ctx.ui.notify(saved.error, "error")`
    /// (`index.ts:1407`) — the raw cause alone — because its modal is still on screen to supply the
    /// context; cyrup's command has no modal, so the context has to travel in the toast.
    ///
    /// Silent when no [`cyrup_ext::HostServices`] backend is attached, which is the same no-op pi's
    /// `noOpUIContext` gives a headless run.
    fn notify_save_failure(&self, summary: &str, cause: &str) {
        if let Some(services) = self.host_services.get() {
            services.notify(
                &format!("{summary}\n{}\n{cause}", self.config_path_line()),
                NotifyKind::Error,
            );
        }
    }

    /// PERM-007 — hand [`crate::config_modal::PermissionSystemSettingsOverlay`] to the host and
    /// block until the human closes it, then report whatever the overlay could not commit.
    ///
    /// Returns `None` when the overlay ran (the human has already seen everything on screen, so a
    /// trailing Info toast would be noise — the `Ok(None)` convention on
    /// [`cyrup_ext::NativeExtension::execute_command`]), and `Some(text)` with the read-only dump
    /// when no interactive surface took it. [`cyrup_ext::HostServices::open_overlay`] returning
    /// `false` is exactly pi's `if (!ctx.hasUI)` case, not a failure.
    fn open_settings_overlay(&self) -> Option<String> {
        let Some(services) = self.host_services.get() else {
            return Some(self.render_settings());
        };
        let overlay = Box::new(crate::config_modal::PermissionSystemSettingsOverlay::new(
            self.config_controller(),
        ));
        // `open_overlay` consumes the box, so the commit failure cannot be read back off it. It is
        // read off the CONTROLLER's own last-error slot instead, which the overlay writes through.
        let controller = self.config_controller();
        if !services.open_overlay(overlay) {
            return Some(self.render_settings());
        }
        // pi's modal notifies inline through `ctx.ui.notify` while it is still on screen
        // (`index.ts:1407`); cyrup's overlay owns the whole screen and has already shown the cause,
        // so the toast here is the SAME one the text path raises and only for a failure that
        // survived to the close.
        if let Some(cause) = controller.take_last_error() {
            self.notify_save_failure(
                "Failed to save the permission-system config; the last change was not applied.",
                &cause,
            );
        }
        None
    }

    /// The modal's initial view (pi `buildSettingItems`, `config-modal.ts:24-41`, plus its
    /// `helpText: \`Config file: ${controller.getConfigPath()}\``, `:85`), as text.
    fn render_settings(&self) -> String {
        let config = guard(&self.config).clone();
        format!(
            "Permission System Settings\n  debug     {:<3}  Debug logging\n  yoloMode  {:<3}  YOLO \
             mode\n{}\n{}\n{COMMAND_USAGE}",
            on_off(config.debug),
            on_off(config.yolo_mode),
            self.config_path_line(),
            // PERM-029: name the policy file and its schema alongside the extension-config path,
            // upstream's `README.md:659` advice made reachable from the app.
            format_args!(
                "Policy file: {}\n  `/permission-system schema` prints the JSON Schema; \
                 `/permission-system example` prints a starter policy.",
                policy_agent_dir(&self.agent_dir)
                    .join(POLICY_FILE)
                    .display()
            )
        )
    }

    /// pi `helpText: \`Config file: ${controller.getConfigPath()}\`` (`config-modal.ts:85`, over
    /// `getPermissionSystemConfigPath`, `index.ts:1509`) — the RESOLVED path, so the
    /// `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH` override is what the human is told about.
    fn config_path_line(&self) -> String {
        format!(
            "Config file: {}",
            Self::resolved_config_path_for(&self.agent_dir).display()
        )
    }
}
