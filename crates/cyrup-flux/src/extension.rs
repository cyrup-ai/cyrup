//! The `NativeExtension` impl for Flux (port doc §3.4.1 skeleton).
//!
//! This first version registers no commands, no shortcuts, no tools — those arrive in
//! FLUX_07–FLUX_10. It subscribes to exactly one event and answers it: `ResourcesDiscover`,
//! contributing the bundled `prompts` DIRECTORY so the pipeline's prompt templates register
//! namespaced under `flux/…` (see [`crate::resources`]) — after first materialising the embedded
//! bundle there (FLUX-001, [`crate::install`]).

use std::sync::{Arc, OnceLock};

use cyrup_core::ExtensionId;
use cyrup_ext::registry::CommandDescriptor;
use cyrup_ext::{
    EventKind, ExtError, HandledValue, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension,
};

/// The Flux native extension.
pub struct FluxExtension {
    pub(crate) id: ExtensionId,
    /// Late-bound by the host before `init` (`native.rs:683`); FLUX_09's overlay and FLUX_10's
    /// tool both reach the live backend through this slot. The `cyrup-ext-subagents`
    /// `OnceLock` pattern (`extension.rs:139`, `:751-759`).
    pub(crate) host_services: Arc<OnceLock<Arc<dyn cyrup_ext::host::HostServices>>>,
    /// Where the bundled tree lives at run time — decided once, at construction
    /// (`crate::flux_extension`), from the agent dir the binary resolved for every extension and
    /// the `CYRUP_FLUX_RESOURCES_DIR` override. FLUX-001: never the build machine's source tree.
    pub(crate) root: crate::resources::BundledRoot,
}

#[async_trait::async_trait]
impl NativeExtension for FluxExtension {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::ResourcesDiscover]);
        api.register_command(
            "flux/status",
            CommandDescriptor {
                description: "Flux pipeline status panel (todo/done/review)".into(),
                completions: vec![],
            },
        );
        api.register_command(
            "flux/cheatsheet",
            CommandDescriptor {
                description: "Flux pipeline cheatsheet (stages A–D)".into(),
                completions: vec![],
            },
        );
        api.register_command(
            "flux/about",
            CommandDescriptor {
                description: "About the Flux pipeline".into(),
                completions: vec![],
            },
        );
        api.register_shortcut("ctrl+f", Some("Flux status overlay".into()));
        api.register_tool(Arc::new(crate::ask_tool::AskUserQuestionTool::new(
            Arc::clone(&self.host_services),
        )));
        Ok(())
    }

    fn set_host_services(&self, services: Arc<dyn cyrup_ext::host::HostServices>) {
        let _ = self.host_services.set(services);
    }

    /// Route the one command this task registers, `flux/status` (port doc §3.4.2). Session
    /// mutation is not needed, but `require_command_tier` still gates it — every registered
    /// command runs at command tier per the trait contract (`native.rs:459-589`).
    async fn execute_command(
        &self,
        name: &str,
        args: &str,
        ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        ctx.require_command_tier()?;
        match name {
            "flux/status" => match crate::render_status::parse_sections(args) {
                Ok((todo, done, review)) => {
                    let base = crate::state::derive_base();
                    Ok(Some(crate::render_status::render(
                        &base, todo, done, review,
                    )))
                }
                Err(bad) => {
                    // Self-issued Error notify, then `Ok(None)` — keeps the wording under this
                    // handler's control instead of the `command:<name>: ` prefix an `Err` would
                    // add (`native.rs:570-587`).
                    if let Some(hs) = self.host_services.get() {
                        hs.notify(
                            &format!(
                                "invalid section(s): {} (choose from done, review, todo)",
                                bad.join(", ")
                            ),
                            cyrup_ext::host::NotifyKind::Error,
                        );
                    }
                    Ok(None)
                }
            },
            "flux/cheatsheet" => match crate::render_cheatsheet::parse_arg(args) {
                Ok(filter) => Ok(Some(crate::render_cheatsheet::render(filter.as_deref()))),
                Err(bad) => {
                    if let Some(hs) = self.host_services.get() {
                        hs.notify(
                            &format!("invalid pipeline: {bad:?} (choose from A, B, C, D)"),
                            cyrup_ext::host::NotifyKind::Error,
                        );
                    }
                    Ok(None)
                }
            },
            "flux/about" => Ok(Some(crate::render_about::render())),
            _ => Err(ExtError::Component(format!(
                "native extension has no handler for command `{name}`"
            ))),
        }
    }

    /// Route the `ctrl+f` shortcut this task registers (port doc §3.4.3). `ctx` is COMMAND tier
    /// like any command handler; this overlay needs none of the session-replacing ops that tier
    /// permits.
    async fn execute_shortcut(&self, key: &str, ctx: &HostCtx) -> Result<(), ExtError> {
        ctx.require_command_tier()?;
        if key == "ctrl+f" {
            crate::overlay::open_status_overlay(&self.host_services);
        }
        Ok(())
    }

    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if matches!(ev, HostEvent::ResourcesDiscover { .. }) {
            // FLUX-001 — materialise the embedded bundle first (upstream's `startup` hook,
            // `register_callbacks.py:47-71` @v0.0.40): cheap when the marker already names this
            // bundle, one copy pass otherwise. Every outcome is reported, never thrown: a broken
            // install must not take down the session (`installer.py:19-20`).
            self.materialise_bundle();
            // Contribute the prompts DIRECTORY, never its files: `add_prompt_path` loads a file
            // by BASENAME (losing the `flux/` namespace) and a directory through the recursive
            // namespaced scanner (`discovery.rs:1929-1958`). This one line is why `/flux/new`
            // is `/flux/new` and not `/new`.
            let prompts = self.root.prompts_dir();
            let skill = self.root.skill_md();
            let mut payload = serde_json::Map::new();
            if prompts.is_dir() {
                payload.insert(
                    "promptPaths".into(),
                    serde_json::json!([prompts.display().to_string()]),
                );
            } else {
                self.notify(
                    &format!(
                        "flux: bundled prompt templates not found at {} — the /flux/* commands \
                         will be unavailable this session",
                        prompts.display()
                    ),
                    cyrup_ext::host::NotifyKind::Warning,
                );
            }
            // A FILE, not a directory: `add_skill_path` (discovery.rs:1899-1926) loads a
            // `SKILL.md` path directly, which is exactly what `cyrup-ext-subagents` contributes
            // (extension.rs:11013-11033). The `flux/` namespacing that forces `promptPaths` to be
            // a DIRECTORY has no skill analog — a skill's name comes from its own
            // frontmatter/dir, not from a scan-root-relative path.
            if skill.is_file() {
                payload.insert(
                    "skillPaths".into(),
                    serde_json::json!([skill.display().to_string()]),
                );
            } else {
                self.notify(
                    &format!(
                        "flux: bundled skill not found at {} — the flux skill will be \
                         unavailable this session",
                        skill.display()
                    ),
                    cyrup_ext::host::NotifyKind::Warning,
                );
            }
            if payload.is_empty() {
                return HookOutcome::Noop;
            }
            return HookOutcome::Handled(HandledValue(serde_json::Value::Object(payload)));
        }
        HookOutcome::Noop
    }
}

impl FluxExtension {
    /// Where this extension will look for (and, when managed, install) the bundled tree.
    #[must_use]
    pub fn bundled_root(&self) -> &crate::resources::BundledRoot {
        &self.root
    }

    /// Fire-and-forget user notice through the late-bound host services; silently dropped when
    /// no backend was bound (a host that never calls `set_host_services` has no UI to notify).
    fn notify(&self, message: &str, kind: cyrup_ext::host::NotifyKind) {
        if let Some(hs) = self.host_services.get() {
            hs.notify(message, kind);
        }
    }

    /// `register_callbacks.py:47-68` `_install_flux_commands` @v0.0.40, minus the command-cache
    /// rescan (`:64-66`) — cyrup's loader has not scanned yet when `ResourcesDiscover` fires, so
    /// there is no stale cache to refresh. Wording of the three notices is upstream's
    /// (`:58`, `:60-63`, `:68`).
    fn materialise_bundle(&self) {
        use crate::install::InstallOutcome;
        use cyrup_ext::host::NotifyKind;
        match self.root.ensure() {
            Ok(InstallOutcome::UpToDate | InstallOutcome::SkippedLocked) => {}
            Ok(InstallOutcome::Installed(report)) => {
                if report.changed() {
                    self.notify(
                        &format!(
                            "Flux commands installed -> {} ({})",
                            self.root.path().display(),
                            report.summary()
                        ),
                        NotifyKind::Info,
                    );
                    if !report.backed_up.is_empty() {
                        self.notify(
                            &format!(
                                "Backed up locally-modified Flux files (see *.bak): {}",
                                report.backed_up.join(", ")
                            ),
                            NotifyKind::Warning,
                        );
                    }
                }
            }
            Err(e) => self.notify(
                &format!(
                    "Flux bootstrap skipped (install failed): {} ({e})",
                    self.root.path().display()
                ),
                NotifyKind::Warning,
            ),
        }
    }
}
