//! TUI-037 — persisting an implicitly-granted project trust on `/reload`, pi's
//! `maybeSaveImplicitProjectTrustAfterReload` (`interactive-mode.ts:4921-4941` @v0.84.4, landed
//! upstream in `38f18be44` "persist implicit project trust on reload").
//!
//! The scenario: cyrup was launched in a project with NO trust-requiring resources, so the trust
//! decision was never asked — the project is trusted implicitly (`decide_trust` step 2,
//! `cyrup-config/src/trust.rs`, pi's `!hasTrustRequiringProjectResources(cwd) → true` at
//! `project-trust.ts:50-52`). During the session the user creates `.cyrup/skills/…` and runs
//! `/reload`. pi keeps the session trusted across that reload and writes `cwd → true` into
//! `trust.json`, so the NEXT launch — which would now find resources and prompt — inherits the
//! decision the user already lived under; the reload status gains `; saved project trust`.
//!
//! ```ts
//! // main.ts:701-704 @v0.84.4 — armed once, at the composition root
//! const autoTrustOnReloadCwd =
//!     parsed.projectTrustOverride === undefined && !hasTrustRequiringProjectResources(sessionCwd)
//!         ? sessionCwd
//!         : undefined;
//!
//! // interactive-mode.ts:4921-4941 @v0.84.4
//! private maybeSaveImplicitProjectTrustAfterReload(): boolean {
//!     const cwd = this.sessionManager.getCwd();
//!     if (this.autoTrustOnReloadCwd !== cwd) return false;
//!     if (!this.settingsManager.isProjectTrusted() || !hasTrustRequiringProjectResources(cwd)) return false;
//!     const trustStore = new ProjectTrustStore(this.runtimeHost.services.agentDir);
//!     try {
//!         if (trustStore.get(cwd) !== null) { this.autoTrustOnReloadCwd = undefined; return false; }
//!         trustStore.set(cwd, true);
//!         this.autoTrustOnReloadCwd = undefined;
//!         return true;
//!     } catch (error) {
//!         this.showWarning(`Could not save project trust after reload: ${…}`);
//!         return false;
//!     }
//! }
//! ```
//!
//! The decision is the pure [`implicit_trust_after_reload`] over five explicit inputs; the
//! store read/write, the warning and the disarm are [`App::maybe_save_implicit_project_trust`].
//!
//! **[CYRUP-DELTA] — ordering.** pi runs this AFTER `session.reload()` (`interactive-mode.ts:5995`),
//! and can, because `AgentSession.reload` does not re-decide trust: `resourceLoader.reload()`
//! "preserves SettingsManager.projectTrusted" (`resource-loader.ts:404`), so the implicit grant is
//! still in force when the store is written. cyrup's `/reload` REBUILDS the session through the
//! `SessionFactory` (`runtime.rs` `reload` → `factory.build` → `SessionBuilder::build`), which
//! re-runs `decide_trust` from the store; with resources now present and nothing saved, the
//! rebuilt session would fall to the prompt or to untrusted, and a post-rebuild port of the guard
//! `!isProjectTrusted()` would then never save. cyrup therefore takes the decision and writes the
//! store BEFORE dispatching the rebuild: the inputs are identical (pi's post-reload
//! `isProjectTrusted()` IS the pre-reload value, and the resource scan and store read are
//! filesystem state the reload does not change), the store ends in the same state, and the rebuilt
//! session reads the saved `true` back — which is the closest cyrup can come to pi's carried
//! in-memory trust. The one observable difference: a reload that then FAILS has already written
//! the entry, where pi would not have. Recorded, not hidden.

use std::path::Path;

use super::*;

/// What [`implicit_trust_after_reload`] decided — the three exits of pi's function, named by
/// what the shell must do rather than by pi's `boolean` return (which conflates "not armed" with
/// "already decided, disarmed").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImplicitTrustReload {
    /// Nothing to do and the arm stays: the reload is for another cwd, the project is not
    /// trusted, or it still has no trust-requiring resources (pi's two `return false` before the
    /// store is touched, `:4923-4928`).
    Keep,
    /// A decision for this cwd (or an ancestor) is already saved: write nothing, drop the arm
    /// (`:4931-4934`).
    Disarm,
    /// Save `cwd → true` and drop the arm (`:4935-4937`); the caller reports it in the reload
    /// status.
    Persist,
}

/// The pure decision of pi's `maybeSaveImplicitProjectTrustAfterReload` (`:4922-4934`), over
/// inputs the shell has already read:
///
/// - `armed` — `this.autoTrustOnReloadCwd`, the cwd that was implicitly trusted at boot, if any;
/// - `cwd` — `this.sessionManager.getCwd()`, the session being reloaded;
/// - `project_trusted` — `this.settingsManager.isProjectTrusted()`;
/// - `has_resources` — `hasTrustRequiringProjectResources(cwd)`;
/// - `already_saved` — `trustStore.get(cwd) !== null`, a decision for the cwd or an ancestor.
pub fn implicit_trust_after_reload(
    armed: Option<&Path>,
    cwd: &Path,
    project_trusted: bool,
    has_resources: bool,
    already_saved: bool,
) -> ImplicitTrustReload {
    if armed != Some(cwd) {
        return ImplicitTrustReload::Keep;
    }
    if !project_trusted || !has_resources {
        return ImplicitTrustReload::Keep;
    }
    if already_saved {
        return ImplicitTrustReload::Disarm;
    }
    ImplicitTrustReload::Persist
}

impl<B: Backend> App<B> {
    /// Arm the implicit-trust save — pi's `autoTrustOnReloadCwd` constructor option
    /// (`interactive-mode.ts:344`, stored `:572` @v0.84.4). The host computes it once at boot, at
    /// its composition root, exactly as `main.ts:701-704` does: the session cwd when no
    /// `--approve`/`--no-approve` was given AND the cwd had no trust-requiring resources (so trust
    /// was granted without a decision), else `None`.
    pub fn set_auto_trust_on_reload_cwd(&mut self, cwd: Option<PathBuf>) {
        self.state.auto_trust_on_reload_cwd = cwd;
    }

    /// The shell of pi's `maybeSaveImplicitProjectTrustAfterReload` (`:4921-4941`): read the
    /// inputs off the CURRENT session, take [`implicit_trust_after_reload`]'s decision, and on
    /// [`ImplicitTrustReload::Persist`] write `cwd → true` through the session's trust store
    /// (`AgentSession::write_project_trust`, the same seam the `/trust` selector persists through).
    /// `Ok` carries pi's boolean — `true` only when an entry was written — which selects the
    /// `; saved project trust` variant of the reload status.
    ///
    /// `Err` is the store failure pi's `catch` turns into
    /// `showWarning("Could not save project trust after reload: …")` (`:4938-4940`); the arm
    /// stays in place, as it does upstream, and the caller frames and surfaces the message. Called
    /// from the `/reload` arm BEFORE the rebuild is dispatched — see the module doc's ordering
    /// note for why cyrup cannot run it after.
    pub(crate) async fn maybe_save_implicit_project_trust(
        &mut self,
        session: &Arc<AgentSession>,
    ) -> Result<bool, cyrup_session_svc::SessionServiceError> {
        let services = session.services();
        let has_resources =
            cyrup_config::trust::has_trust_requiring_resources(&services.cwd, &services.home);
        let armed = self.state.auto_trust_on_reload_cwd.as_deref();
        // pi opens the store only once the two cheap guards have passed (`:4923-4930`), so an
        // unarmed or unqualified reload never touches `trust.json`. Evaluated once with
        // `already_saved = false`: `Keep` cannot depend on that input, and the store is read only
        // for the two exits that do. A store that is unreadable here reports as "nothing saved"
        // and fails on the write below, which is where pi's `catch` lands too.
        let cheap = implicit_trust_after_reload(
            armed,
            &services.cwd,
            services.project_trusted,
            has_resources,
            false,
        );
        if cheap == ImplicitTrustReload::Keep {
            return Ok(false);
        }
        let already_saved = session.saved_trust_decision().await.is_some();
        match implicit_trust_after_reload(
            armed,
            &services.cwd,
            services.project_trusted,
            has_resources,
            already_saved,
        ) {
            ImplicitTrustReload::Keep => Ok(false),
            ImplicitTrustReload::Disarm => {
                self.state.auto_trust_on_reload_cwd = None;
                Ok(false)
            }
            ImplicitTrustReload::Persist => {
                let update = (
                    services.cwd.clone(),
                    Some(cyrup_config::trust::TrustDecision::Trusted),
                );
                session.write_project_trust(&[update]).await?;
                self.state.auto_trust_on_reload_cwd = None;
                Ok(true)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CWD: &str = "/work/project";

    #[test]
    fn not_armed_or_another_cwd_keeps_the_arm_and_writes_nothing() {
        let cwd = Path::new(CWD);
        assert_eq!(
            implicit_trust_after_reload(None, cwd, true, true, false),
            ImplicitTrustReload::Keep
        );
        assert_eq!(
            implicit_trust_after_reload(Some(Path::new("/work/other")), cwd, true, true, false),
            ImplicitTrustReload::Keep
        );
    }

    #[test]
    fn untrusted_or_resource_free_projects_keep_the_arm() {
        let cwd = Path::new(CWD);
        assert_eq!(
            implicit_trust_after_reload(Some(cwd), cwd, false, true, false),
            ImplicitTrustReload::Keep
        );
        assert_eq!(
            implicit_trust_after_reload(Some(cwd), cwd, true, false, false),
            ImplicitTrustReload::Keep
        );
    }

    #[test]
    fn a_saved_decision_disarms_without_a_write() {
        let cwd = Path::new(CWD);
        assert_eq!(
            implicit_trust_after_reload(Some(cwd), cwd, true, true, true),
            ImplicitTrustReload::Disarm
        );
    }

    #[test]
    fn an_armed_trusted_project_with_new_resources_persists() {
        let cwd = Path::new(CWD);
        assert_eq!(
            implicit_trust_after_reload(Some(cwd), cwd, true, true, false),
            ImplicitTrustReload::Persist
        );
    }
}
