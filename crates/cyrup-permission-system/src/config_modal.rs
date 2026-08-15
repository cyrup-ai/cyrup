//! PERM-007 — the `/permission-system` settings modal: a Rust port of
//! `pi-permission-system/src/config-modal.ts` @v0.8.0.
//!
//! Upstream's `/permission-system` handler is `openPermissionSystemSettingsModal(ctx, controller)`
//! (`config-modal.ts:63-122`), a live `await ctx.ui.custom<void>(…, { overlay: true, … })` that
//! builds a `ZellijSettingsModal` over a `pi-tui` `SettingsList`. cyrup's counterpart used to be a
//! formatted `String`, so an operator got a read-only dump plus two blind toggles instead of an
//! editor.
//!
//! # The two pieces upstream has and cyrup needed
//!
//! * **[`ConfigController`]** — pi's `PermissionSystemConfigController`
//!   (`config-modal.ts:8-12`): `{ getConfig, setConfig, getConfigPath }`, registered at
//!   `index.ts:1504-1511`. cyrup's config writer lived as an inherent `&self` method on the
//!   extension, and [`cyrup_ext::host::overlay::InteractiveOverlay`] is `'static` — so it could not
//!   borrow the extension. Making the writer an `Arc`-shared object is exactly upstream's own
//!   indirection, and it is what unblocks the overlay.
//! * **[`PermissionSystemSettingsOverlay`]** — the component. `SettingsList`'s behaviour
//!   (`pi/packages/tui/src/components/settings-list.ts` @v0.83.0) is ported directly: wrapping
//!   up/down navigation (`:180-186`), Enter **or** Space cycling the value ring (`:187`,
//!   `activateItem` `:199-222`), Esc cancelling (`:189`), the `enableSearch` input that filters on
//!   the LABEL and resets the selection (`:190-197`, `applyFilter` `:233-236`), the scroll
//!   indicator (`:147-151`), the selected item's wrapped description (`:153-161`) and the hint line
//!   (`:238-253`).
//!
//! # The commit loop is upstream's, including its failure behaviour
//!
//! pi's `onChange` (`config-modal.ts:74-81`) is: apply → `controller.setConfig(...)` → **re-read**
//! `controller.getConfig()` → `syncSettingValues(...)`. That re-read is the whole error story: a
//! refused write leaves the live config untouched, so the row snaps back to the value still in
//! effect. cyrup reproduces it literally — [`ConfigController::set_config`] returns nothing the
//! overlay branches on, and the overlay re-reads.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use cyrup_ext::host::HostServices;
use cyrup_ext::host::overlay::{
    InteractiveOverlay, OverlayColor, OverlayKey, OverlayKeyCode, OverlayLine, OverlayOutcome,
    OverlaySpan,
};
use serde_json::json;

use crate::ext_config::ExtensionConfig;
use crate::forwarding::SharedExtensionConfig;

/// pi `ON_OFF = ["on", "off"]` (`config-modal.ts:18`) — the value ring every row of this modal
/// cycles through.
pub const ON_OFF: [&str; 2] = ["on", "off"];

/// pi `toOnOff` (`config-modal.ts:20-22`).
#[must_use]
pub fn to_on_off(value: bool) -> &'static str {
    if value { ON_OFF[0] } else { ON_OFF[1] }
}

/// pi `SettingItem` (`pi/packages/tui/src/components/settings-list.ts:7-20`), reduced to the four
/// fields `buildSettingItems` actually populates. `submenu` is deliberately absent: upstream's
/// permission-system modal never sets one, so porting the branch would be inventing a surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingItem {
    /// Stable id — what `applySetting` switches on.
    pub id: &'static str,
    /// Display label (left column).
    pub label: &'static str,
    /// Long description, shown under the list when this row is selected.
    pub description: &'static str,
    /// The value shown in the right column; one of [`ON_OFF`].
    pub current_value: &'static str,
}

/// pi `buildSettingItems(config)` (`config-modal.ts:24-41`). Labels and descriptions are upstream's
/// verbatim.
#[must_use]
pub fn build_setting_items(config: &ExtensionConfig) -> Vec<SettingItem> {
    vec![
        SettingItem {
            id: "debug",
            label: "Debug logging",
            description: "Write diagnostics and permission review entries to the extension debug file",
            current_value: to_on_off(config.debug),
        },
        SettingItem {
            id: "yoloMode",
            label: "YOLO mode",
            description: "Auto-approve ask-state permission checks, including subagent approval forwarding",
            current_value: to_on_off(config.yolo_mode),
        },
    ]
}

/// pi `applySetting(config, id, value)` (`config-modal.ts:43-56`), including its
/// `default: return config` arm — an id the list cannot emit changes nothing rather than erroring.
///
/// Note upstream's coercion is `value === "on"`, not a membership test: anything that is not the
/// literal `"on"` reads as off. Reproduced exactly.
#[must_use]
pub fn apply_setting(config: &ExtensionConfig, id: &str, value: &str) -> ExtensionConfig {
    let enabled = value == ON_OFF[0];
    match id {
        "debug" => ExtensionConfig { debug: enabled, ..config.clone() },
        "yoloMode" => ExtensionConfig { yolo_mode: enabled, ..config.clone() },
        _ => config.clone(),
    }
}

/// pi's `PermissionSystemConfigController` (`config-modal.ts:8-12`), registered at
/// `index.ts:1504-1511` as `{ getConfig: () => extensionConfig, setConfig: saveExtensionConfig,
/// getConfigPath: getPermissionSystemConfigPath }`.
///
/// This owns nothing the extension does not already share by `Arc`; it exists so the writer can be
/// reached from a `'static` overlay without borrowing the extension. `PermissionSystemExtension`
/// holds one and delegates its own `save_extension_config` to it, so there is exactly ONE
/// implementation of the ordering contract (normalize → write → only then touch memory).
pub struct ConfigController {
    /// The live extension-config cell — the SAME `Arc` the gate and the forwarding watcher read.
    config: SharedExtensionConfig,
    /// The agent dir the config path is derived from.
    agent_dir: PathBuf,
    /// pi `lastConfigWarning` (`index.ts:1572`) — cleared by a successful write.
    last_config_warning: Arc<Mutex<Option<String>>>,
    /// The late-bound capability backend, for the status pill and the failure toast.
    host_services: Arc<OnceLock<Arc<dyn HostServices>>>,
    /// pi's module-scope `extensionLogger` (`index.ts:148-150`) — `config.saved` lands here.
    logger: Arc<crate::logging::AuditTrail>,
    /// The cause of the most recent REFUSED write, retained until someone takes it.
    ///
    /// \[CYRUP-DELTA] pi has no equivalent: its `setConfig` calls `ctx.ui.notify(saved.error,
    /// "error")` inline (`index.ts:1407`) while the modal is still on screen. cyrup's overlay is
    /// handed to the host by value, so the failure cannot be read back off it after it closes —
    /// this slot is how the command handler recovers the cause to raise the same one error toast
    /// the text path raises. Behaviourally identical, one frame later.
    last_error: Mutex<Option<String>>,
}

impl ConfigController {
    /// Assemble from the extension's already-shared parts.
    #[must_use]
    pub fn new(
        config: SharedExtensionConfig,
        agent_dir: PathBuf,
        last_config_warning: Arc<Mutex<Option<String>>>,
        host_services: Arc<OnceLock<Arc<dyn HostServices>>>,
        logger: Arc<crate::logging::AuditTrail>,
    ) -> Self {
        Self {
            config,
            agent_dir,
            last_config_warning,
            host_services,
            logger,
            last_error: Mutex::new(None),
        }
    }

    /// pi `getConfig: () => extensionConfig` (`index.ts:1506`).
    #[must_use]
    pub fn get_config(&self) -> ExtensionConfig {
        crate::extension::guard(&self.config).clone()
    }

    /// pi `getConfigPath: getPermissionSystemConfigPath` (`index.ts:1509`) — the RESOLVED path, so
    /// the `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH` override is what the human is shown.
    #[must_use]
    pub fn get_config_path(&self) -> PathBuf {
        crate::extension::PermissionSystemExtension::resolved_config_path_for(&self.agent_dir)
    }

    /// pi `saveExtensionConfig(next, ctx)` (v0.8.0 `index.ts:1402-1420`) — registered as the modal's
    /// `setConfig` (`index.ts:1508`).
    ///
    /// The ORDER is the contract: normalize, WRITE, and only then touch anything in memory. A failed
    /// write returns the cause and has changed NOTHING — no live config, no status pill, no
    /// `lastConfigWarning` reset, no debug entry — so cyrup can never end a turn with an in-memory
    /// config that disagrees with the file the next `session_start` will re-read.
    ///
    /// # Errors
    ///
    /// The raw cause when the write is refused. pi notifies inline (`:1407`); cyrup hands the cause
    /// up so the caller can raise ONE error toast carrying both the what and the why.
    pub fn set_config(&self, next: &ExtensionConfig) -> Result<(), String> {
        // pi `const normalized = normalizePermissionSystemConfig(next)` (`:1403`).
        let normalized = next.normalized();
        // pi `const saved = savePermissionSystemConfig(normalized)` (`:1404`).
        let saved = normalized
            .save(&crate::extension::PermissionSystemExtension::config_path_for(&self.agent_dir));
        if !saved.success {
            // pi `:1405-1410`: report the error and return WITHOUT mutating in-memory state. A
            // failure that carries no message still reports one: the caller must never be left with
            // an empty explanation, which is what an `Option` here would allow.
            let cause = saved
                .error
                .unwrap_or_else(|| "the permission-system config could not be written".to_string());
            *crate::extension::guard(&self.last_error) = Some(cause.clone());
            return Err(cause);
        }
        // A write that landed clears the retained cause: the modal's next close must not report a
        // failure the human has already corrected.
        *crate::extension::guard(&self.last_error) = None;

        // pi `setExtensionConfig(normalized)` (`:1412`).
        *crate::extension::guard(&self.config) = normalized.clone();
        // pi `syncPermissionSystemStatusWhenPossible(normalized, ctx)` (`:1413`).
        if let Some(services) = self.host_services.get() {
            crate::status::sync_status(services, &normalized);
        }
        // pi `lastConfigWarning = null` (`:1414`): the file on disk is now this extension's own
        // output, so whatever the last load complained about is resolved.
        *crate::extension::guard(&self.last_config_warning) = None;
        // pi `writeDebugEntry("config.saved", {...})` (`:1416-1419`).
        self.logger.debug(
            "config.saved",
            &json!({ "debug": normalized.debug, "yoloMode": normalized.yolo_mode }),
        );
        Ok(())
    }

    /// Take the cause of the most recent refused write, clearing it — see [`Self::last_error`].
    #[must_use]
    pub fn take_last_error(&self) -> Option<String> {
        crate::extension::guard(&self.last_error).take()
    }
}

/// The maximum number of rows the list body shows at once — pi
/// `Math.min(Math.max(settings.length + 2, 6), 18)` (`zellij-modal.ts:855`), which for this modal's
/// two settings is `6`.
fn max_visible(settings: usize) -> usize {
    settings.saturating_add(2).max(6).min(18)
}

/// pi's `fuzzyFilter(items, query, item => item.label)` (`settings-list.ts:234`) reduced to the
/// subsequence test that filter is built on: every character of the query appears in the label, in
/// order, case-insensitively. An empty query matches everything.
fn label_matches(label: &str, query: &str) -> bool {
    let mut needle = query.chars().flat_map(char::to_lowercase).peekable();
    for c in label.chars().flat_map(char::to_lowercase) {
        if needle.peek().is_some_and(|n| *n == c) {
            needle.next();
        }
    }
    needle.peek().is_none()
}

/// Wrap `text` to `width` columns on whitespace — pi `wrapTextWithAnsi(description, width - 4)`
/// (`settings-list.ts:157`), without the ANSI handling cyrup does not need (an
/// [`OverlaySpan`] carries style out of band, so the text here is never escape-laden).
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if line.is_empty() {
            line.push_str(word);
        } else if line.chars().count() + 1 + word.chars().count() <= width {
            line.push(' ');
            line.push_str(word);
        } else {
            out.push(std::mem::take(&mut line));
            line.push_str(word);
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// PERM-007 — `openPermissionSystemSettingsModal`'s component (`config-modal.ts:63-122`) as a
/// [`InteractiveOverlay`].
///
/// Upstream composes `ZellijModal(ZellijSettingsModal(…))`; cyrup paints the same content directly
/// because [`OverlayLine`] is already the "styled terminal row" the Zellij pair exists to produce,
/// and the host draws the frame. Everything that is BEHAVIOUR — the rows, the value ring, the
/// commit-and-re-read loop, the search, the help text — is ported; only the border-drawing is the
/// host's.
pub struct PermissionSystemSettingsOverlay {
    controller: Arc<ConfigController>,
    /// pi's `current` binding (`config-modal.ts:66`), refreshed from `controller.getConfig()` after
    /// every commit (`:79`).
    current: ExtensionConfig,
    items: Vec<SettingItem>,
    /// Indices into [`Self::items`] surviving the search filter — pi `filteredItems`
    /// (`settings-list.ts:36`).
    filtered: Vec<usize>,
    selected: usize,
    /// pi's `Input` under `enableSearch` (`settings-list.ts:41`, armed at `:63-65`). The modal sets
    /// `enableSearch: true` (`config-modal.ts:86`).
    search: String,
    /// The last commit failure, shown in place of the help line. pi has no equivalent because its
    /// `setConfig` notifies through `ctx.ui.notify` while the modal is still on screen; cyrup's
    /// overlay owns the whole screen, so the cause is shown IN it (and the caller's toast is
    /// suppressed for this path — see [`Self::take_error`]).
    error: Option<String>,
}

impl PermissionSystemSettingsOverlay {
    /// Build the overlay over an already-shared controller. Reads the config once, exactly as pi's
    /// factory does at `config-modal.ts:66`.
    #[must_use]
    pub fn new(controller: Arc<ConfigController>) -> Self {
        let current = controller.get_config();
        let items = build_setting_items(&current);
        let filtered = (0..items.len()).collect();
        Self { controller, current, items, filtered, selected: 0, search: String::new(), error: None }
    }

    /// The most recent commit failure, if any — so the command handler can raise the same
    /// error-level toast the text path raises once the overlay closes.
    #[must_use]
    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    /// pi `syncSettingValues(settingsList, config)` (`config-modal.ts:58-61`): re-project EVERY
    /// row's displayed value off the freshly re-read config, not just the row that was touched.
    fn sync_setting_values(&mut self) {
        for item in &mut self.items {
            item.current_value = match item.id {
                "debug" => to_on_off(self.current.debug),
                "yoloMode" => to_on_off(self.current.yolo_mode),
                other => {
                    debug_assert!(false, "unknown setting id {other}");
                    item.current_value
                }
            };
        }
    }

    /// pi `SettingsList.activateItem` (`settings-list.ts:199-222`) for the `values` branch: cycle to
    /// the next value in the ring and fire `onChange`.
    ///
    /// `onChange` is `config-modal.ts:74-81` verbatim: apply → `setConfig` → **re-read**
    /// `getConfig()` → `syncSettingValues`. The re-read is what makes a refused write snap the row
    /// back to the value still in effect, so it is not an optimisation that can be skipped.
    fn activate_item(&mut self) -> OverlayOutcome {
        let Some(&index) = self.filtered.get(self.selected) else {
            return OverlayOutcome::Ignored;
        };
        let Some(item) = self.items.get(index) else {
            return OverlayOutcome::Ignored;
        };
        let ring_index = ON_OFF.iter().position(|v| *v == item.current_value).unwrap_or(0);
        // `.get()` rather than `ON_OFF[..]`: the `% len` already makes the index infallible, but
        // the crate denies `clippy::indexing_slicing` (lib.rs:69) and this line was tripping it.
        // `ON_OFF` is a non-empty const array, so the fallback is unreachable; it stays on the
        // item's CURRENT value, which is the inert, non-destructive answer if it ever were not.
        let Some(&next_value) = ON_OFF.get((ring_index + 1) % ON_OFF.len()) else {
            return OverlayOutcome::Ignored;
        };
        let id = item.id;

        let next = apply_setting(&self.current, id, next_value);
        match self.controller.set_config(&next) {
            Ok(()) => self.error = None,
            Err(cause) => self.error = Some(cause),
        }
        // pi `current = controller.getConfig()` (`:79`) — unconditional, on both outcomes.
        self.current = self.controller.get_config();
        self.sync_setting_values();
        OverlayOutcome::Redraw
    }

    /// pi `applyFilter` (`settings-list.ts:233-236`): re-filter on the label and reset the
    /// selection to the top.
    fn apply_filter(&mut self) {
        self.filtered = (0..self.items.len())
            .filter(|i| {
                self.items.get(*i).is_some_and(|item| label_matches(item.label, &self.search))
            })
            .collect();
        self.selected = 0;
    }

    fn hint(text: impl Into<String>) -> OverlayLine {
        OverlayLine::new(vec![OverlaySpan {
            text: text.into(),
            fg: Some(OverlayColor::DarkGray),
            dim: true,
            ..OverlaySpan::default()
        }])
    }
}

impl InteractiveOverlay for PermissionSystemSettingsOverlay {
    fn render(&mut self, width: usize, _height: usize) -> Vec<OverlayLine> {
        let inner = width.max(4);
        let mut lines = Vec::new();

        // pi `ZellijSettingsModal`'s title + description rows (`zellij-modal.ts:845-850`), with
        // upstream's exact strings (`config-modal.ts:70-71`).
        lines.push(OverlayLine::new(vec![OverlaySpan {
            text: "Permission System Settings".to_string(),
            fg: Some(OverlayColor::Cyan),
            bold: true,
            ..OverlaySpan::default()
        }]));
        lines.push(OverlayLine::default());
        lines.push(Self::hint(
            "Local extension options for debug logging and auto-approval behavior",
        ));
        lines.push(OverlayLine::default());

        // The search input (`settings-list.ts:91-94`).
        lines.push(OverlayLine::new(vec![OverlaySpan::raw(format!("> {}", self.search))]));
        lines.push(OverlayLine::default());

        if self.filtered.is_empty() {
            // pi `"  No matching settings"` (`settings-list.ts:107`).
            lines.push(Self::hint("  No matching settings"));
        } else {
            // pi's scroll window (`settings-list.ts:111-117`).
            let visible = max_visible(self.items.len());
            let start = self
                .selected
                .saturating_sub(visible / 2)
                .min(self.filtered.len().saturating_sub(visible));
            let end = (start + visible).min(self.filtered.len());
            // pi `Math.min(30, max(labelWidths))` (`settings-list.ts:120`).
            let label_width = self
                .items
                .iter()
                .map(|item| item.label.chars().count())
                .max()
                .unwrap_or(0)
                .min(30);

            for slot in start..end {
                let Some(item) = self.filtered.get(slot).and_then(|i| self.items.get(*i)) else {
                    continue;
                };
                let is_selected = slot == self.selected;
                // pi `theme.cursor` vs two spaces (`settings-list.ts:126`).
                let prefix = if is_selected { "> " } else { "  " };
                let pad = label_width.saturating_sub(item.label.chars().count());
                lines.push(OverlayLine::new(vec![
                    OverlaySpan::raw(prefix),
                    OverlaySpan {
                        text: format!("{}{}", item.label, " ".repeat(pad)),
                        bold: is_selected,
                        ..OverlaySpan::default()
                    },
                    OverlaySpan::raw("  "),
                    OverlaySpan {
                        text: item.current_value.to_string(),
                        fg: Some(if item.current_value == ON_OFF[0] {
                            OverlayColor::Green
                        } else {
                            OverlayColor::DarkGray
                        }),
                        bold: is_selected,
                        ..OverlaySpan::default()
                    },
                ]));
            }

            // pi's scroll indicator (`settings-list.ts:147-151`).
            if start > 0 || end < self.filtered.len() {
                lines.push(Self::hint(format!(
                    "  ({}/{})",
                    self.selected + 1,
                    self.filtered.len()
                )));
            }

            // pi's selected-item description block (`settings-list.ts:153-161`).
            if let Some(item) = self.filtered.get(self.selected).and_then(|i| self.items.get(*i)) {
                lines.push(OverlayLine::default());
                for line in wrap_text(item.description, inner.saturating_sub(4)) {
                    lines.push(Self::hint(format!("  {line}")));
                }
            }
        }

        lines.push(OverlayLine::default());
        // pi `helpText: \`Config file: ${controller.getConfigPath()}\`` (`config-modal.ts:85`).
        lines.push(Self::hint(format!(
            "Config file: {}",
            self.controller.get_config_path().display()
        )));
        // pi `helpUndertitle` (`config-modal.ts:98-101`), verbatim.
        lines.push(Self::hint("Esc: close | ↑↓: navigate | Space: toggle"));
        // pi's own hint row under `enableSearch` (`settings-list.ts:243-247`), verbatim.
        lines.push(Self::hint("  Type to search · Enter/Space to change · Esc to cancel"));

        if let Some(error) = &self.error {
            lines.push(OverlayLine::new(vec![OverlaySpan {
                text: format!("Save failed: {error}"),
                fg: Some(OverlayColor::Red),
                ..OverlaySpan::default()
            }]));
        }

        lines
    }

    fn handle_key(&mut self, key: OverlayKey) -> OverlayOutcome {
        // pi `SettingsList.handleInput` (`settings-list.ts:167-197`), in upstream's order — the
        // navigation/confirm/cancel bindings are tested BEFORE the search input consumes anything,
        // which is why arrows and Esc are never typed into the query.
        match key.code {
            // `tui.select.up` / `.down` wrap (`settings-list.ts:180-186`).
            OverlayKeyCode::Up => {
                if self.filtered.is_empty() {
                    return OverlayOutcome::Ignored;
                }
                self.selected = if self.selected == 0 {
                    self.filtered.len() - 1
                } else {
                    self.selected - 1
                };
                OverlayOutcome::Redraw
            }
            OverlayKeyCode::Down => {
                if self.filtered.is_empty() {
                    return OverlayOutcome::Ignored;
                }
                self.selected = if self.selected + 1 == self.filtered.len() {
                    0
                } else {
                    self.selected + 1
                };
                OverlayOutcome::Redraw
            }
            // `tui.select.confirm` OR a literal space (`settings-list.ts:187`).
            OverlayKeyCode::Enter | OverlayKeyCode::Char(' ') => self.activate_item(),
            // `tui.select.cancel` → `onCancel` → `done()` (`config-modal.ts:82`).
            OverlayKeyCode::Escape => OverlayOutcome::Close,
            // The search input. pi strips spaces from the data before feeding it
            // (`data.replace(/ /g, "")`, `settings-list.ts:191`) precisely because Space is the
            // toggle — which the arm above already claimed, so nothing can reach here as a space.
            OverlayKeyCode::Backspace => {
                if self.search.pop().is_none() {
                    return OverlayOutcome::Ignored;
                }
                self.apply_filter();
                OverlayOutcome::Redraw
            }
            OverlayKeyCode::Char(c) => {
                self.search.push(c);
                self.apply_filter();
                OverlayOutcome::Redraw
            }
            _ => OverlayOutcome::Ignored,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn controller(dir: &std::path::Path) -> Arc<ConfigController> {
        Arc::new(ConfigController::new(
            Arc::new(Mutex::new(ExtensionConfig::default())),
            dir.to_path_buf(),
            Arc::new(Mutex::new(None)),
            Arc::new(OnceLock::new()),
            Arc::new(crate::logging::AuditTrail::detached(dir.to_path_buf())),
        ))
    }

    /// pi `buildSettingItems` (`config-modal.ts:24-41`): two rows, upstream's ids, upstream's
    /// labels, and each row's value projected off the config.
    #[test]
    fn the_modal_offers_pis_two_rows_with_their_current_values() {
        let config = ExtensionConfig { debug: true, ..ExtensionConfig::default() };
        let items = build_setting_items(&config);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "debug");
        assert_eq!(items[0].label, "Debug logging");
        assert_eq!(items[0].current_value, "on");
        assert_eq!(items[1].id, "yoloMode");
        assert_eq!(items[1].label, "YOLO mode");
        assert_eq!(items[1].current_value, "off");
    }

    /// pi `applySetting` (`config-modal.ts:43-56`), including `value === "on"` coercion and the
    /// `default: return config` arm.
    #[test]
    fn apply_setting_matches_upstreams_switch_including_its_default_arm() {
        let base = ExtensionConfig::default();
        assert!(apply_setting(&base, "debug", "on").debug);
        assert!(!apply_setting(&base, "debug", "off").debug);
        // Upstream coerces on `=== "on"`, so any other string reads as OFF rather than erroring.
        assert!(!apply_setting(&base, "debug", "ON").debug);
        assert!(apply_setting(&base, "yoloMode", "on").yolo_mode);
        // An id the list cannot emit changes nothing.
        assert_eq!(apply_setting(&base, "nope", "on"), base);
    }

    /// PERM-007's core claim: toggling a row inside the overlay must reach the config file on disk
    /// AND the live in-memory config, through the same writer the text path uses.
    ///
    /// RED before the fix: there was no overlay at all — `/permission-system` returned a `String`
    /// from `render_settings`, so no key could commit anything.
    #[test]
    fn toggling_a_row_writes_the_config_and_updates_the_live_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let ctl = controller(dir.path());
        let mut overlay = PermissionSystemSettingsOverlay::new(Arc::clone(&ctl));

        assert!(!ctl.get_config().debug, "precondition: debug starts off");
        // Space is upstream's toggle (`settings-list.ts:187`).
        assert_eq!(
            overlay.handle_key(OverlayKey::plain(OverlayKeyCode::Char(' '))),
            OverlayOutcome::Redraw
        );

        assert!(ctl.get_config().debug, "the live config must be flipped");
        assert!(ctl.get_config_path().exists(), "the config file must have been written");
        let on_disk: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(ctl.get_config_path()).unwrap()).unwrap();
        assert_eq!(on_disk.get("debug"), Some(&serde_json::Value::Bool(true)));
        assert_eq!(overlay.take_error(), None);

        // …and the ROW re-reads the committed value (pi's `current = controller.getConfig()` +
        // `syncSettingValues`, `config-modal.ts:79-81`), rather than trusting what it just sent.
        assert_eq!(overlay.items[0].current_value, "on");
    }

    /// pi `SettingsList` navigation (`settings-list.ts:180-186`): up/down WRAP, and the second row
    /// is the one Space then toggles.
    #[test]
    fn navigation_wraps_and_the_toggle_applies_to_the_selected_row() {
        let dir = tempfile::tempdir().unwrap();
        let ctl = controller(dir.path());
        let mut overlay = PermissionSystemSettingsOverlay::new(Arc::clone(&ctl));

        overlay.handle_key(OverlayKey::plain(OverlayKeyCode::Down));
        assert_eq!(overlay.selected, 1);
        overlay.handle_key(OverlayKey::plain(OverlayKeyCode::Down));
        assert_eq!(overlay.selected, 0, "down from the last row wraps to the first");
        overlay.handle_key(OverlayKey::plain(OverlayKeyCode::Up));
        assert_eq!(overlay.selected, 1, "up from the first row wraps to the last");

        overlay.handle_key(OverlayKey::plain(OverlayKeyCode::Enter));
        assert!(ctl.get_config().yolo_mode, "Enter toggles the SELECTED row, not the first");
        assert!(!ctl.get_config().debug);
    }

    /// pi's `enableSearch` path (`settings-list.ts:190-197`, `applyFilter` `:233-236`): typing
    /// filters on the LABEL and resets the selection; Esc still closes rather than being typed.
    #[test]
    fn typing_filters_the_rows_and_escape_still_closes() {
        let dir = tempfile::tempdir().unwrap();
        let mut overlay = PermissionSystemSettingsOverlay::new(controller(dir.path()));

        for c in "yolo".chars() {
            overlay.handle_key(OverlayKey::plain(OverlayKeyCode::Char(c)));
        }
        assert_eq!(overlay.filtered, vec![1], "only the YOLO row survives the filter");
        assert_eq!(overlay.selected, 0);

        overlay.handle_key(OverlayKey::plain(OverlayKeyCode::Backspace));
        assert_eq!(overlay.search, "yol");

        assert_eq!(
            overlay.handle_key(OverlayKey::plain(OverlayKeyCode::Escape)),
            OverlayOutcome::Close,
            "Esc is upstream's cancel, never a search character"
        );
    }

    /// pi's commit loop re-reads `getConfig()` on BOTH outcomes (`config-modal.ts:79`), so a REFUSED
    /// write leaves the row showing the value still in effect rather than the one the user asked
    /// for.
    ///
    /// The refusal is forced the way the write path already refuses: the config path's parent is a
    /// FILE, so the directory cannot be created.
    #[test]
    fn a_refused_write_leaves_the_row_and_the_live_config_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        // `config_path_for` is `<agent_dir>/cyrup-permission-system/config.json`; make that middle
        // component a regular file so no directory can be created there.
        std::fs::write(dir.path().join("cyrup-permission-system"), "not a dir").unwrap();

        let ctl = controller(dir.path());
        let mut overlay = PermissionSystemSettingsOverlay::new(Arc::clone(&ctl));
        overlay.handle_key(OverlayKey::plain(OverlayKeyCode::Char(' ')));

        assert!(!ctl.get_config().debug, "a refused write must not mutate the live config");
        assert_eq!(overlay.items[0].current_value, "off", "the row snaps back to what is in effect");
        assert!(overlay.take_error().is_some(), "the cause must be available to the caller");
    }

    /// The rendered frame must actually carry the two rows, the resolved config path
    /// (`config-modal.ts:85`) and upstream's help undertitle (`:98-101`).
    #[test]
    fn the_rendered_frame_carries_the_rows_the_config_path_and_the_help_line() {
        let dir = tempfile::tempdir().unwrap();
        let ctl = controller(dir.path());
        let mut overlay = PermissionSystemSettingsOverlay::new(Arc::clone(&ctl));
        let text = overlay
            .render(80, 24)
            .iter()
            .map(OverlayLine::plain_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("Permission System Settings"), "{text}");
        assert!(text.contains("Debug logging"), "{text}");
        assert!(text.contains("YOLO mode"), "{text}");
        assert!(
            text.contains(&format!("Config file: {}", ctl.get_config_path().display())),
            "{text}"
        );
        assert!(text.contains("Esc: close | ↑↓: navigate | Space: toggle"), "{text}");
        // The selected row's description is upstream's, verbatim.
        assert!(
            text.contains("Write diagnostics and permission review entries"),
            "{text}"
        );
    }

    #[test]
    fn the_label_filter_is_an_ordered_case_insensitive_subsequence() {
        assert!(label_matches("YOLO mode", "yolo"));
        assert!(label_matches("YOLO mode", "ym"));
        assert!(label_matches("Debug logging", ""));
        assert!(!label_matches("Debug logging", "yolo"));
        assert!(!label_matches("YOLO mode", "my"), "order is load-bearing");
    }
}
