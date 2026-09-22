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
//! # `[CYRUP-DELTA]` — where the chord is configured, and why it is OFF until you configure it
//!
//! Upstream reads the chord from `config.foregroundDetachShortcut`
//! (`extension/index.ts:856` → `slash-commands.ts:1007`, validated at `extension/config.ts:152-155`
//! with `isValidKeyId`, typed at `shared/types.ts:2603`). It has **no default**: with the key
//! absent, `if (options.foregroundDetachShortcut)` is falsy and pi registers no chord at all.
//!
//! cyrup now matches that, through TWO rungs rather than one — the settings key upstream has, plus
//! the env var this crate already carried:
//!
//! | rung | where | wins over |
//! |---|---|---|
//! | 1 | [`crate::registration::SubagentExtensionConfig::foreground_detach_shortcut`] (`config.json`'s `foregroundDetachShortcut`) | everything |
//! | 2 | [`FOREGROUND_DETACH_SHORTCUT_ENV`], through [`SubagentsExtension::env_lookup`] | nothing |
//! | — | neither set: **no chord is registered**, exactly as upstream | |
//!
//! The settings rung outranks the env one because it is upstream's own home for the key, and
//! because an exported environment variable is machine-wide state a user did not write into THIS
//! project's config — it must not silently overrule a chord the project pinned. (This is the
//! reverse of `max_subagent_spawns_per_run`'s `env -> config -> default` ladder, which is upstream's
//! ordering for THAT key, `shared/types.ts:2807-2818`; upstream has no env rung for this one, so
//! nothing constrains where cyrup's addition goes, and the lower rung is where an addition belongs.)
//! Setting either rung to the empty string (or to whitespace) is upstream's falsy value and means
//! "register nothing"; setting the SETTINGS rung to it also stops the walk, so a project can turn
//! the chord off for everyone regardless of what any shell exports.
//!
//! ## Why the default is no chord, and not `ctrl+alt+d`
//!
//! Until this was decided, cyrup defaulted the chord ON at `ctrl+alt+d`. That was an artefact of
//! the writer not owning `registration/mod.rs` and having nowhere but an env var to put the key —
//! not a decision. It is now a decision, and it goes the other way, for three reasons:
//!
//! 1. **Upstream ships opt-in**, and no cyrup-specific fact argues for diverging on a keybinding.
//! 2. **A default-on chord can take a key its user already bound.** Rule 3 of
//!    [`cyrup_ext::ExtensionRegistry::resolve_shortcuts`] (pi `runner.ts:522-528`) is that an
//!    extension shortcut colliding with a NON-reserved built-in *warns but WINS*, and the
//!    extension tier fires BEFORE the editor. The collision check that justified `ctrl+alt+d`
//!    (below) proves the chord is free in cyrup's and pi's **default** keymaps — it cannot prove
//!    anything about a user's own `keybindings.json`, which is precisely where a rebind lives. A
//!    chord nobody asked for must not be able to shadow one somebody chose.
//! 3. **Nothing is lost.** `/subagents-detach` is registered unconditionally and does the same
//!    work through the same handler; the chord is one line of `config.json` away, which is exactly
//!    the cost upstream's user pays.
//!
//! `ctrl+alt+d` remains the RECOMMENDED value, and the collision check behind it stands, so a user
//! who wants the chord has a vetted one to copy:
//!
//! * cyrup's default tables bind exactly ONE `ctrl+alt` chord, `ctrl+alt+]`
//!   (`EditorKeymap::default`'s `JumpBackward`, `crates/cyrup-tui/src/keymap.rs:1778-1785`);
//! * pi v0.84.4 likewise binds only `ctrl+alt+]` (`tui.editor.jumpBackward`,
//!   `packages/tui/src/keybindings.ts:110-112`), so a later port of a pi default cannot collide
//!   either;
//! * it is not in [`cyrup_ext::registry::RESERVED_KEYBINDINGS_FOR_EXTENSION_CONFLICTS`]'s territory
//!   (those are canonical `app.*` action ids, and none of cyrup's default bindings for them is a
//!   `ctrl+alt` chord), so `resolve_shortcuts`' rule 2 cannot silently skip it;
//! * `alt+d` alone was rejected: it is `EditorKeymap`'s `DeleteWordForward` (`keymap.rs:1770`), and
//!   the extension-shortcut tier fires BEFORE the editor, so binding it would silently take
//!   delete-word-forward away from every emacs-motion user — the exact regression
//!   `crates/cyrup-flux/src/extension.rs:21-47` documents for its own retired `ctrl+f`;
//! * `ctrl+alt+f` was rejected because `cyrup-flux` already owns it
//!   (`crates/cyrup-flux/src/extension.rs:48`), and rule 4 of `resolve_shortcuts` would have made
//!   the two extensions clobber each other with a diagnostic.
//!
//! A legacy terminal delivers `ctrl+alt+<letter>` as `ESC` + the control byte, which crossterm
//! decodes as `CONTROL | ALT`, so such a chord reaches the TUI without the kitty keyboard protocol.
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

/// The LOWER-tier home of pi's `config.foregroundDetachShortcut` (`shared/types.ts:2603`), in this
/// crate's `CYRUP_` naming family — cyrup's own addition, for a caller that cannot edit
/// `config.json` (compare [`crate::exec::run_fanout_budget::MAX_SPAWNS_PER_RUN_ENV`]).
///
/// Set it to a chord to bind one; set it to the empty string to register no chord at all, which is
/// also what leaving it unset does (`slash-commands.ts:1007`'s falsy test). It is OUTRANKED by
/// [`crate::registration::SubagentExtensionConfig::foreground_detach_shortcut`] — see the module's
/// `[CYRUP-DELTA]` for why that way round.
pub(crate) const FOREGROUND_DETACH_SHORTCUT_ENV: &str = "CYRUP_SUBAGENT_FOREGROUND_DETACH_SHORTCUT";

/// pi's shortcut description, verbatim (`slash-commands.ts:1009`) — what `/hotkeys` renders in the
/// Action column ([`cyrup_ext::native::InitApi::register_shortcut`]'s own doc).
pub(crate) const FOREGROUND_DETACH_SHORTCUT_DESCRIPTION: &str =
    "Detach the active foreground subagent and keep it running in the background";

/// Upstream's falsy test (`slash-commands.ts:1007`) over a configured string: a value that is
/// empty, or whitespace only, registers nothing. Trimmed, because a chord read from a config file
/// or an environment variable can carry padding a `KeyId` never has.
fn configured_chord(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

impl SubagentsExtension {
    /// The chord to bind, or `None` for upstream's "no shortcut" state.
    ///
    /// pi `options.foregroundDetachShortcut` as consumed by `if (options.foregroundDetachShortcut)`
    /// (`slash-commands.ts:1007`): a falsy value registers nothing, and NOTHING CONFIGURED is the
    /// default — see the module's `[CYRUP-DELTA]` for the decision and its reasoning.
    ///
    /// Two rungs, highest first:
    ///
    /// 1. `config.json`'s `foregroundDetachShortcut`
    ///    ([`crate::registration::SubagentExtensionConfig::foreground_detach_shortcut`]), captured
    ///    at construction. **A key that is PRESENT ends the walk**, even when its value is falsy —
    ///    that is what lets a project turn the chord off for everyone regardless of what any shell
    ///    exports, and it is why this is a `match` on the option rather than an `or_else` chain over
    ///    the resolved strings.
    /// 2. [`FOREGROUND_DETACH_SHORTCUT_ENV`], through [`Self::env_lookup`].
    ///
    /// Neither present: `None`, and `init` registers no shortcut at all
    /// (`extension/host/native_impl.rs`'s `if let Some(key)`).
    pub(crate) fn foreground_detach_shortcut(&self) -> Option<String> {
        match self.foreground_detach_shortcut_setting.as_deref() {
            Some(configured) => configured_chord(configured),
            None => (self.env_lookup())(FOREGROUND_DETACH_SHORTCUT_ENV)
                .as_deref()
                .and_then(configured_chord),
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

    /// An extension whose two chord rungs are both PINNED: the settings key through
    /// [`SubagentExtensionConfig::foreground_detach_shortcut`] and
    /// [`FOREGROUND_DETACH_SHORTCUT_ENV`] through `SubagentExtensionConfig::env_overrides`, so no
    /// test touches the process environment (and none of these has to be serialized against
    /// another that does). `None` for the env override answers "unset", which is the case a
    /// set-only helper could not reach.
    fn ext_with(
        setting: Option<&str>,
        env: Option<&str>,
    ) -> (tempfile::TempDir, SubagentsExtension) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut env_overrides = std::collections::BTreeMap::new();
        env_overrides.insert(
            FOREGROUND_DETACH_SHORTCUT_ENV.to_string(),
            env.map(str::to_string),
        );
        let config = SubagentExtensionConfig {
            foreground_detach_shortcut: setting.map(str::to_string),
            env_overrides,
            ..SubagentExtensionConfig::default()
        };
        let ext = SubagentsExtension::with_config_and_cwd(config, dir.path().to_path_buf());
        (dir, ext)
    }

    /// VL-S11b R3 — upstream's default, now cyrup's: `if (options.foregroundDetachShortcut)`
    /// (`slash-commands.ts:1007`) registers NOTHING with the key absent, so a stock install binds
    /// no chord and cannot shadow a key its user already owns.
    ///
    /// MUTATION: restore the `None => Some(DEFAULT_…)` arm (register unconditionally) and this
    /// fails; it is the whole decision recorded in the module's `[CYRUP-DELTA]`.
    #[test]
    fn nothing_is_bound_when_neither_tier_is_configured() {
        let (_dir, ext) = ext_with(None, None);
        assert_eq!(
            ext.foreground_detach_shortcut(),
            None,
            "an unconfigured install registers no chord, exactly as upstream"
        );
        assert!(
            !ext.is_foreground_detach_shortcut("ctrl+alt+d"),
            "and no key dispatches to the detach handler"
        );
        assert_eq!(
            SubagentExtensionConfig::default().foreground_detach_shortcut,
            None,
            "the config default is the absent key, not a seeded chord"
        );
    }

    /// The SETTINGS tier outranks the ENV tier — the layering `registration/mod.rs`'s field doc
    /// states, and the reason it is that way round: an exported variable is machine-wide state
    /// that must not overrule a chord this project pinned.
    ///
    /// MUTATION: swap the two arms of `foreground_detach_shortcut` and every assertion here
    /// resolves to the env value instead.
    #[test]
    fn the_settings_tier_wins_over_the_env_tier() {
        let (_dir, ext) = ext_with(Some("ctrl+alt+b"), Some("ctrl+alt+z"));
        assert_eq!(
            ext.foreground_detach_shortcut().as_deref(),
            Some("ctrl+alt+b")
        );

        // A PRESENT settings key ends the walk even when its value is falsy — that is how a
        // project turns the chord off for everyone regardless of what any shell exports.
        for opt_out in ["", "   "] {
            let (_dir, ext) = ext_with(Some(opt_out), Some("ctrl+alt+z"));
            assert_eq!(
                ext.foreground_detach_shortcut(),
                None,
                "{opt_out:?} at the settings tier must not fall through to the env tier"
            );
        }

        // With the settings key ABSENT, the env tier is consulted and still works — it is the
        // lower rung, not a retired one.
        let (_dir, ext) = ext_with(None, Some("  ctrl+alt+z  "));
        assert_eq!(
            ext.foreground_detach_shortcut().as_deref(),
            Some("ctrl+alt+z"),
            "a configured chord is trimmed, and the env rung still binds"
        );
        for opt_out in ["", "   "] {
            let (_dir, ext) = ext_with(None, Some(opt_out));
            assert_eq!(
                ext.foreground_detach_shortcut(),
                None,
                "{opt_out:?} is upstream's falsy value at the env tier too"
            );
        }
    }

    /// MUTATION: matching the pressed key case-sensitively, or matching ANY key. The registry
    /// lowercases on resolution (`registry.rs:1044`), so a press may arrive spelled differently
    /// than it was registered — but a key this extension never bound must still be refused, which
    /// is what keeps `execute_shortcut`'s dispatch honest once a second chord exists.
    #[tokio::test]
    async fn the_shortcut_resolves_to_the_detach_handler_and_nothing_else() {
        let (_dir, ext) = ext_with(Some("ctrl+alt+d"), None);
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

        // An extension that bound NOTHING refuses even the recommended chord — `dispatch_shortcut`
        // does not fall back to a default the registration never registered.
        let (_dir, unbound) = ext_with(None, None);
        assert!(
            unbound.dispatch_shortcut("ctrl+alt+d").await.is_err(),
            "a chord that was never registered must not dispatch"
        );
    }

    /// MUTATION: renaming the env key without updating its documentation. It is the lower of the
    /// two rungs (see the module's `[CYRUP-DELTA]`), and a silent rename would strand every user
    /// who had set it.
    #[test]
    fn the_env_key_is_the_documented_one() {
        assert_eq!(
            FOREGROUND_DETACH_SHORTCUT_ENV,
            "CYRUP_SUBAGENT_FOREGROUND_DETACH_SHORTCUT"
        );
    }
}
