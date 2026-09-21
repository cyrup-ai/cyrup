//! The extension's keyboard-shortcut surface — pi `pi.registerShortcut(options.foregroundDetachShortcut
//! as KeyId, { description, handler })` (`slash/slash-commands.ts:1007-1012` @v0.68.0).
//!
//! Upstream binds the SAME closure the `/subagents-detach` command is registered with, called with
//! an empty argument: `handler: async (ctx) => detachForegroundRun("", ctx)` (`:1010`). That
//! sharing is the whole contract — the chord and the command must resolve the same target, refuse
//! for the same reasons and print the same sentences — so this module calls
//! [`SubagentsExtension::slash_subagents_detach`] with `""` and renders nothing of its own.
//!
//! # The registration path (VL-S11 R3)
//!
//! `pi.registerShortcut` has an exact cyrup counterpart and it was already complete on both ends:
//!
//! | pi | cyrup |
//! |---|---|
//! | `pi.registerShortcut(key, { description, handler })` | [`cyrup_ext::native::InitApi::register_shortcut`] (`crates/cyrup-ext/src/native.rs:399-408`) |
//! | the runner's per-extension `ext.shortcuts` map | [`cyrup_ext::ExtensionRegistry::register_shortcut`] (`crates/cyrup-ext/src/registry.rs:914-927`), fed by `facade.rs:501-503` |
//! | `getShortcuts(resolvedKeybindings)` conflict rules | [`cyrup_ext::ExtensionRegistry::resolve_shortcuts`] (`registry.rs:994`) |
//! | invoking the bound handler | [`cyrup_ext::native::NativeExtension::execute_shortcut`] (`native.rs:651-670`) |
//!
//! An earlier research pass recorded this as a missing capability by looking at
//! [`cyrup_ext::registry::CommandDescriptor`], which describes COMMANDS and has no bearing on
//! shortcuts.
//!
//! # `[CYRUP-DELTA]` — where the chord is configured, and why it is ON by default
//!
//! Upstream reads the chord from `config.foregroundDetachShortcut`
//! (`extension/index.ts:856` → `slash-commands.ts:1007`, validated at `extension/config.ts:152-155`
//! with `isValidKeyId`, typed at `shared/types.ts:2603`). It has **no default**: with the key
//! absent, `if (options.foregroundDetachShortcut)` is falsy and pi registers no chord at all.
//!
//! cyrup carries the key at the ENV tier instead ([`FOREGROUND_DETACH_SHORTCUT_ENV`], resolved
//! through [`SubagentsExtension::env_lookup`] like every other env-tier subagents knob — compare
//! [`crate::exec::run_fanout_budget::MAX_SPAWNS_PER_RUN_ENV`]), and it DEFAULTS the chord ON
//! ([`DEFAULT_FOREGROUND_DETACH_SHORTCUT`]). Two deliberate differences, both recorded rather than
//! inferred:
//!
//! * **The settings tier is not wired.** The `settings.json` sibling would be a field on
//!   [`crate::registration::SubagentExtensionConfig`]; it is not added here. Setting
//!   [`FOREGROUND_DETACH_SHORTCUT_ENV`] to the empty string is the documented opt-out, which is
//!   upstream's "key absent" state.
//! * **It defaults on.** Upstream's chord is opt-in because a pi user edits one config file to get
//!   both the extension and its keybinding; a cyrup user who never sets an env var would otherwise
//!   have a `/subagents-detach` command with no chord, i.e. the feature at half its surface. The
//!   default is only defensible because it collides with nothing — see
//!   [`DEFAULT_FOREGROUND_DETACH_SHORTCUT`]'s own doc.
//!
//! # Every other shortcut upstream registers on this surface: there are none
//!
//! `pi.registerShortcut` appears **exactly once** in the whole of pi-subagents at v0.68.0 — the
//! detach binding at `slash-commands.ts:1008`. The fleet inspector's `Key.ctrlAlt("f")` chord is a
//! v0.43.0 artefact (still cited by `extension/host/slash.rs:33`) and does **not** exist at the
//! pinned tag; `/subagents-fleet` is a command there and nothing else. So this is the extension's
//! complete shortcut surface, and binding a second chord would be an invention, not a port.

use cyrup_ext::ExtError;

use crate::extension::host::SubagentsExtension;

/// The env-tier home of pi's `config.foregroundDetachShortcut` (`shared/types.ts:2603`), in this
/// crate's `CYRUP_` naming family.
///
/// Set it to another chord to rebind; set it to the empty string to register no chord at all,
/// which is upstream's own "key absent" behaviour (`slash-commands.ts:1007`).
pub(crate) const FOREGROUND_DETACH_SHORTCUT_ENV: &str = "CYRUP_SUBAGENT_FOREGROUND_DETACH_SHORTCUT";

/// The chord bound when [`FOREGROUND_DETACH_SHORTCUT_ENV`] is unset.
///
/// `ctrl+alt+d` — `d` for detach, in the one modifier family that is provably free on both sides:
///
/// * cyrup's default tables bind exactly ONE `ctrl+alt` chord, `ctrl+alt+]`
///   (`EditorKeymap::default`'s `JumpBackward`, `crates/cyrup-tui/src/keymap.rs:1778-1785`);
/// * pi v0.84.4 likewise binds only `ctrl+alt+]` (`tui.editor.jumpBackward`,
///   `packages/tui/src/keybindings.ts:110-112`), so a later port of a pi default cannot collide
///   either;
/// * it is not in [`cyrup_ext::registry::RESERVED_KEYBINDINGS_FOR_EXTENSION_CONFLICTS`]'s territory
///   (those are canonical `app.*` action ids, and none of cyrup's default bindings for them is a
///   `ctrl+alt` chord), so `resolve_shortcuts`' rule 2 cannot silently skip it;
/// * `alt+d` alone was rejected: it is `EditorKeymap`'s `DeleteWordForward` (`keymap.rs:1770`), and
///   the extension-shortcut tier fires BEFORE the editor, so binding it would silently take
///   delete-word-forward away from every emacs-motion user — the exact regression
///   `crates/cyrup-flux/src/extension.rs:21-47` documents for its own retired `ctrl+f`;
/// * `ctrl+alt+f` was rejected because `cyrup-flux` already owns it
///   (`crates/cyrup-flux/src/extension.rs:48`), and rule 4 of `resolve_shortcuts` would have made
///   the two extensions clobber each other with a diagnostic.
///
/// A legacy terminal delivers `ctrl+alt+<letter>` as `ESC` + the control byte, which crossterm
/// decodes as `CONTROL | ALT`, so the chord reaches the TUI without the kitty keyboard protocol.
pub(crate) const DEFAULT_FOREGROUND_DETACH_SHORTCUT: &str = "ctrl+alt+d";

/// pi's shortcut description, verbatim (`slash-commands.ts:1009`) — what `/hotkeys` renders in the
/// Action column ([`cyrup_ext::native::InitApi::register_shortcut`]'s own doc).
pub(crate) const FOREGROUND_DETACH_SHORTCUT_DESCRIPTION: &str =
    "Detach the active foreground subagent and keep it running in the background";

impl SubagentsExtension {
    /// The chord to bind, or `None` for upstream's "no shortcut" state.
    ///
    /// pi `options.foregroundDetachShortcut` as consumed by `if (options.foregroundDetachShortcut)`
    /// (`slash-commands.ts:1007`): a falsy value registers nothing. The empty string (and a
    /// whitespace-only value) is that falsy value here, since an env var cannot be "absent but
    /// set".
    pub(crate) fn foreground_detach_shortcut(&self) -> Option<String> {
        match (self.env_lookup())(FOREGROUND_DETACH_SHORTCUT_ENV) {
            Some(configured) => {
                let trimmed = configured.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            }
            None => Some(DEFAULT_FOREGROUND_DETACH_SHORTCUT.to_string()),
        }
    }

    /// `true` when `key` is the chord this extension bound — the test
    /// [`cyrup_ext::native::NativeExtension::execute_shortcut`] performs before running the
    /// handler.
    ///
    /// Case-insensitive, because the registry normalizes with `key.to_lowercase()` before it ever
    /// matches (`registry.rs:1044`, pi `runner.ts:510`), so the key handed back on a press need not
    /// be spelled the way it was registered.
    pub(crate) fn is_foreground_detach_shortcut(&self, key: &str) -> bool {
        self.foreground_detach_shortcut()
            .is_some_and(|bound| bound.eq_ignore_ascii_case(key.trim()))
    }

    /// pi `handler: async (ctx) => detachForegroundRun("", ctx)` (`slash-commands.ts:1010`).
    ///
    /// The SAME handler `/subagents-detach` dispatches to, with upstream's empty argument — which
    /// is what selects "the active foreground single run" rather than a named one. Nothing is
    /// re-implemented here and no sentence is invented: every string the user sees is the one the
    /// command would have printed.
    ///
    /// A shortcut handler has no output channel — pi's returns `void`, and
    /// [`cyrup_ext::native::NativeExtension::execute_shortcut`]'s own doc says a handler with
    /// something to say calls [`cyrup_ext::host::HostServices::notify`], exactly as upstream's does
    /// through `ctx.ui.notify`. So the handler's text is notified rather than returned.
    pub(crate) async fn run_foreground_detach_shortcut(&self) {
        let (message, kind) = match self.slash_subagents_detach("").await {
            Ok(text) => (text, cyrup_ext::NotifyKind::Info),
            // pi's own severity split inside `detachForegroundRun`: the ambiguity and
            // wrong-mode refusals notify as `error` (`:983`, `:993`), the rest as `info`. The
            // handler has already chosen between them by returning `Ok` or `Err`.
            Err(error) => (error.to_string(), cyrup_ext::NotifyKind::Error),
        };
        let trimmed = message.trim();
        if trimmed.is_empty() {
            return;
        }
        match self.executor.host_services() {
            Some(services) => services.notify(trimmed, kind),
            None => tracing::info!(
                target: "cyrup_ext_subagents::shortcuts",
                detach_message = trimmed,
                "no capability backend bound; the detach shortcut has nowhere to notify"
            ),
        }
    }

    /// The whole of [`cyrup_ext::native::NativeExtension::execute_shortcut`]'s body, kept beside
    /// the registration so the two cannot drift.
    ///
    /// # Errors
    ///
    /// The trait's own default sentence for a key this extension did not bind — reached only if the
    /// host routes a press whose owner is some other extension.
    pub(crate) async fn dispatch_shortcut(&self, key: &str) -> Result<(), ExtError> {
        if !self.is_foreground_detach_shortcut(key) {
            return Err(ExtError::Component(format!(
                "native extension has no handler for shortcut `{key}`"
            )));
        }
        self.run_foreground_detach_shortcut().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::registration::SubagentExtensionConfig;

    /// An extension whose [`FOREGROUND_DETACH_SHORTCUT_ENV`] lookup is PINNED through
    /// `SubagentExtensionConfig::env_overrides`, so no test touches the process environment (and
    /// none of these tests has to be serialized against another that does).
    fn ext_with_env(pinned: Option<&str>) -> (tempfile::TempDir, SubagentsExtension) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut env_overrides = std::collections::BTreeMap::new();
        env_overrides.insert(
            FOREGROUND_DETACH_SHORTCUT_ENV.to_string(),
            pinned.map(str::to_string),
        );
        let config = SubagentExtensionConfig {
            env_overrides,
            ..SubagentExtensionConfig::default()
        };
        let ext = SubagentsExtension::with_config_and_cwd(config, dir.path().to_path_buf());
        (dir, ext)
    }

    /// MUTATION: dropping the default, or defaulting to a chord some other table already owns.
    /// The chord must be `ctrl+alt+d` with nothing configured — see
    /// [`DEFAULT_FOREGROUND_DETACH_SHORTCUT`]'s doc for why that specific chord.
    #[test]
    fn the_default_chord_is_bound_when_nothing_is_configured() {
        let (_dir, ext) = ext_with_env(None);
        assert_eq!(
            ext.foreground_detach_shortcut().as_deref(),
            Some("ctrl+alt+d")
        );
        assert_eq!(
            DEFAULT_FOREGROUND_DETACH_SHORTCUT, "ctrl+alt+d",
            "the constant IS the documented chord"
        );
    }

    /// MUTATION: ignoring the configured key, or failing to treat an empty value as upstream's
    /// falsy "register nothing" (`slash-commands.ts:1007`).
    #[test]
    fn the_configured_chord_wins_and_an_empty_value_registers_nothing() {
        let (_dir, ext) = ext_with_env(Some("  ctrl+alt+b  "));
        assert_eq!(
            ext.foreground_detach_shortcut().as_deref(),
            Some("ctrl+alt+b"),
            "a configured chord replaces the default, trimmed"
        );

        for opt_out in ["", "   "] {
            let (_dir, ext) = ext_with_env(Some(opt_out));
            assert_eq!(
                ext.foreground_detach_shortcut(),
                None,
                "{opt_out:?} is upstream's falsy value: no chord is registered"
            );
        }
    }

    /// MUTATION: matching the pressed key case-sensitively, or matching ANY key. The registry
    /// lowercases on resolution (`registry.rs:1044`), so a press may arrive spelled differently
    /// than it was registered — but a key this extension never bound must still be refused, which
    /// is what keeps `execute_shortcut`'s dispatch honest once a second chord exists.
    #[tokio::test]
    async fn the_shortcut_resolves_to_the_detach_handler_and_nothing_else() {
        let (_dir, ext) = ext_with_env(None);
        assert!(ext.is_foreground_detach_shortcut("ctrl+alt+d"));
        assert!(
            ext.is_foreground_detach_shortcut("Ctrl+Alt+D"),
            "the registry normalizes case, so the match must too"
        );
        assert!(!ext.is_foreground_detach_shortcut("ctrl+alt+f"));

        // Wiring, asserted without a live run: an unbound key never reaches the detach handler and
        // comes back with the trait's own sentence.
        let err = ext
            .dispatch_shortcut("ctrl+alt+f")
            .await
            .expect_err("an unbound key is refused");
        assert!(
            err.to_string()
                .contains("native extension has no handler for shortcut `ctrl+alt+f`"),
            "the refusal is the trait default's own sentence: {err}"
        );

        // And the chord this extension DID bind is the one the descriptor advertises, so
        // `/hotkeys` names the same action the command does.
        assert_eq!(
            FOREGROUND_DETACH_SHORTCUT_DESCRIPTION,
            "Detach the active foreground subagent and keep it running in the background",
            "pi `slash-commands.ts:1009`, verbatim"
        );
    }

    /// MUTATION: renaming the env key without updating its documentation. The key is the ONLY way
    /// to rebind or disable the chord until the `settings.json` tier is wired (see the module's
    /// `[CYRUP-DELTA]`), so a silent rename would strand every user who had set it.
    #[test]
    fn the_env_key_is_the_documented_one() {
        assert_eq!(
            FOREGROUND_DETACH_SHORTCUT_ENV,
            "CYRUP_SUBAGENT_FOREGROUND_DETACH_SHORTCUT"
        );
    }
}
