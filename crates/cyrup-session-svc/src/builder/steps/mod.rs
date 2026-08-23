//! The numbered steps [`SessionBuilder::build`] runs, one `pub(in crate::builder)` function per
//! banner, over the shared [`BuildCtx`] every step reads.
//!
//! The interfaces are deliberately WIDE. The measured fan-in is ~11 inbound values for resource
//! discovery alone and 27 bindings for the final assembly, so a narrow positional signature cannot
//! carry a step boundary: every step takes [`BuildCtx`] plus at most three further arguments — the
//! output struct of an earlier step, or a per-step `Params` bundle — and hands back one output
//! struct. The assembly itself stays inline in [`super::build`], where all 27 bindings are in scope
//! at once and naming them twice would buy nothing.

use std::path::Path;
use std::sync::Arc;

use cyrup_config::{
    decide_trust_with_extension, has_trust_requiring_resources, ExtensionTrust, Settings,
    SettingsManager, SettingsStore, TrustInputs, TrustOutcome,
};
use cyrup_config::trust::{trust_options, TrustStore};
use cyrup_ext::NativeExtension;
use cyrup_core::RunCancel;
use cyrup_provider::Provider;

use super::natives::pre_trust_extension_verdict;
use super::{SessionConfig, TrustPromptFn};

mod agent_loop;
mod context_prompt;
mod resources;
mod session_tree;

pub(super) use agent_loop::{
    agent_loop, tool_registry, wire_subscribers, AgentLoop, AgentParams, SubscriberWiring,
    ToolSurface,
};
pub(super) use context_prompt::{context_and_prompt, PromptParams, PromptSurface};
pub(super) use resources::{discover_resources, extension_stack, ExtStack, Resources};
pub(super) use session_tree::{
    open_session_tree, resolve_session_model, seed_session_entries, seed_transcript, session_dir_of,
    ModelPick, SessionTree,
};

/// The values every step from the session tree onward reads: the immutable configuration, the cwd
/// and cancel token derived from it, the provider handle, and the two things step 1 settles
/// (`settings` + the project-trust verdict).
///
/// Owned rather than borrowed so `build()` holds exactly one binding for all six; each step
/// destructures the fields it needs on its first line, which keeps the ported upstream bodies
/// character-identical to the single-function version they came from.
pub(super) struct BuildCtx {
    pub(super) cfg: SessionConfig,
    pub(super) cwd: std::path::PathBuf,
    pub(super) cancel: RunCancel,
    pub(super) provider: Arc<dyn Provider>,
    pub(super) settings: SettingsManager,
    pub(super) trusted: bool,
}

/// What step 1 settles: the layered settings view with the trust decision folded in, and the
/// verdict itself (which several later steps gate on directly).
pub(super) struct SettingsTrust {
    pub(super) settings: SettingsManager,
    pub(super) trusted: bool,
}

/// The five builder seams the tiered trust decision reads, borrowed as one bundle.
///
/// Deliberately NOT `&SessionBuilder`: the builder also holds the two `FnOnce` override closures
/// and the adopted `SessionManager`, none of which are `Sync`, so a shared reference to the whole
/// builder is not `Send` and cannot be held across this step's `await`.
pub(super) struct TrustSeams<'a> {
    pub(super) settings_store: &'a Arc<dyn SettingsStore>,
    pub(super) native_extensions: &'a [Arc<dyn NativeExtension>],
    pub(super) trust_store: Option<&'a Arc<TrustStore>>,
    pub(super) trust_prompt: Option<&'a TrustPromptFn>,
    pub(super) cli_settings: &'a Settings,
}

/// Step 1 — settings + trust (cyrup-config).
pub(super) async fn settings_and_trust(
    cfg: &SessionConfig,
    cwd: &Path,
    seams: TrustSeams<'_>,
) -> SettingsTrust {
    // Load global first (project untrusted) to read defaultProjectTrust, then decide trust.
    let mut settings = SettingsManager::load(seams.settings_store.clone(), false);
    let default_trust = settings.effective().default_project_trust();
    let has_resources = has_trust_requiring_resources(cwd, &cfg.home);
    // Pi's `shouldResolveProjectTrust` guard (main.ts:676-678): only pay for a pre-trust
    // extension pass when the answer is actually in doubt — no explicit `--approve/--no-approve`
    // and there IS something to gate. In every other case this is the exact previous code path.
    let ext_trust = if cfg.trust_override.is_none() && has_resources {
        pre_trust_extension_verdict(cfg, cwd, seams.native_extensions).await
    } else {
        None
    };
    // SEAM-065 — the saved-decision tier is read HERE, not by the caller, because pi reads it
    // at `project-trust.ts:72-75`, i.e. strictly AFTER `emitProjectTrustEvent` (`:54-70`).
    let saved = seams.trust_store
        .and_then(|store| store.nearest(cwd).ok().flatten());
    if let Some(d) = &ext_trust
        && d.remember
    {
        // Pi persists the extension's verdict itself when it asked to be remembered:
        // `if (result.remember) { trustStore.set(cwd, trusted); }` (project-trust.ts:64-66).
        match seams.trust_store {
            Some(store) => {
                let decision = if d.trusted {
                    cyrup_config::TrustDecision::Trusted
                } else {
                    cyrup_config::TrustDecision::Untrusted
                };
                if let Err(e) = store.set(cwd, Some(decision)) {
                    tracing::warn!(error = %e, "persisting extension project_trust verdict");
                }
            }
            None => tracing::warn!(
                extension = %d.by, trusted = d.trusted,
                "extension project_trust asked to `remember` the decision, but no trust store \
                 is wired into the session builder — the verdict applies to this session only"
            ),
        }
    }
    let inputs = TrustInputs {
        has_resources,
        trust_override: cfg.trust_override,
        saved: saved.as_ref().map(|e| e.decision),
        default_trust,
        mode: cfg.app_mode,
        prompt_choice: None,
    };
    let outcome = decide_trust_with_extension(
        inputs,
        ext_trust.map(|d| ExtensionTrust { trusted: d.trusted, remember: d.remember }),
    );
    let trusted = match outcome {
        TrustOutcome::Trusted => true,
        TrustOutcome::Untrusted => false,
        // Pi reaches its prompt LAST — `if (!hasUI) return false;` then
        // `selectProjectTrustOption(...)` (project-trust.ts:86-94). Both the `hasUI` gate and
        // the mode gate are already folded in: `decide_trust` only yields `NeedsPrompt` for an
        // interactive mode (`cyrup-config/src/trust.rs:294-299`), and a host with no terminal
        // wires no callback, which is pi's `hasUI === false` — proceed untrusted.
        TrustOutcome::NeedsPrompt => match seams.trust_prompt {
            Some(prompt) => {
                // `includeSessionOnly: true` — pi's PRE-LAUNCH prompt is the one call site that
                // asks for the two ephemeral rows (project-trust.ts:32). SEAM-064. pi's IN-APP
                // selector is the other call site and genuinely passes the default `false`
                // (trust-selector.ts:44) — do not "fix" that one to match.
                let options = trust_options(cwd, true);
                prompt(&options, &saved).unwrap_or(false)
            }
            None => false,
        },
    };
    settings.set_project_trusted(trusted);
    // Embedder-supplied overrides are applied HERE, after the trust decision has settled the
    // two persistent layers — pi's `applyOverrides` (settings-manager.ts:508-510), the same
    // shape its own harness uses (`test/test-harness.ts:395`:
    // `settingsManager.applyOverrides(options.settings)`). CFG-059: these used to be a THIRD
    // persistent layer inside `SettingsManager` that outranked project and survived every
    // recompute; upstream has no CLI settings tier, so the tier is gone and the transient
    // override path is the only one left. Applying after `set_project_trusted` matters: that
    // call recomputes from the layers, which would discard an override applied before it.
    if !seams.cli_settings.is_empty() {
        settings.apply_overrides(seams.cli_settings);
    }

    SettingsTrust { settings, trusted }
}
