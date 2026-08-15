//! The interactive TUI's implementation of [`cyrup_session_svc::ThemeAccess`] — the live theme
//! seam a loaded extension's `theme` / `getAllThemes` / `getTheme` / `setTheme` capability reads
//! and writes (SEAM-T01).
//!
//! Pi binds all four inside `createExtensionUIContext`, which ONLY the interactive mode builds
//! (`modes/interactive/interactive-mode.ts:2401-2417` @v0.84.2):
//!
//! ```text
//! get theme() { return theme },                                   // :2401-2403
//! getAllThemes: () => getAvailableThemesWithPaths(),              // :2404
//! getTheme: (name) => getThemeByName(name),                       // :2405
//! setTheme: (themeOrName) => {                                    // :2406-2417
//!     const result = this.themeController.setThemeName(themeOrName);
//!     if (result.success && this.settingsManager.getTheme() !== themeOrName) {
//!         this.settingsManager.setTheme(themeOrName);
//!     }
//!     return result;
//! },
//! ```
//!
//! Every other mode gets `noOpUIContext` (`core/extensions/runner.ts:261-263` @v0.83.0) or, for
//! RPC, the same three hard-coded empties (`modes/rpc/rpc-mode.ts:290-300` @v0.83.0) — which is
//! exactly what an unattached handle leaves the `HostServices` trait defaults answering, so this
//! type is installed by [`crate::App`] and by nothing else.
//!
//! **Read vs write split.** `list` and `by_name` are pure lookups against the session's immutable
//! [`ResourceRegistry`] snapshot — pi's `getAvailableThemesWithPaths()` / `getThemeByName()` are
//! likewise module-level functions over process-global registries, not instance state — so they are
//! answered on the calling thread with no run-loop involvement at all. `active` reads a cell the app
//! republishes every frame. Only `set` needs the run loop, because only `set` repaints; it validates
//! against the registry FIRST (which is what decides pi's `{success, error}`, since upstream's
//! failure is `loadThemeJson` throwing `Theme not found: {name}`, `theme.ts:622`) and only then
//! hands the resolved theme over.

use std::sync::{Arc, Mutex};

use cyrup_resources::{ResourceRegistry, Theme};
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

/// The channel a validated [`TuiThemeAccess::set`] hands the resolved theme to `App::run` on. The
/// run loop applies it live and persists it, mirroring the `/settings → theme` confirm path — pi's
/// `setThemeName` + `settingsManager.setTheme` pair (`interactive-mode.ts:2406-2417`).
pub type ThemeSwitchSink = UnboundedSender<Theme>;

/// [`cyrup_session_svc::ThemeAccess`] over the live TUI: the session's discovered themes, the
/// app's active theme name, and a switch channel back into the run loop. See the module docs.
#[derive(Debug)]
pub struct TuiThemeAccess {
    /// The session's discovered-theme snapshot — cyrup's `ResourceLoader.getThemes()`, which pi
    /// feeds to `setRegisteredThemes` (`interactive-mode.ts:597`, `:1910`, `:5787` @v0.83.0). It
    /// already contains the two compiled-in built-ins (`cyrup-resources`' `discover` extends the
    /// candidate list with `builtin_themes()`), so this single set is the whole of pi's
    /// built-ins ∪ custom ∪ registered union.
    resources: Arc<ResourceRegistry>,
    /// The ACTIVE theme's name, republished by `App::draw` every frame. A cell rather than a read
    /// of `AppState` because this is read from an extension's own task while the run loop owns
    /// `&mut self`.
    active: Mutex<String>,
    switch: ThemeSwitchSink,
}

impl TuiThemeAccess {
    /// Bind to one session's resources, seeded with the app's current theme name.
    pub fn new(resources: Arc<ResourceRegistry>, active: &str, switch: ThemeSwitchSink) -> Self {
        Self { resources, active: Mutex::new(active.to_string()), switch }
    }

    /// Republish the active theme name (the app, once per frame).
    pub fn publish_active(&self, name: &str) {
        let mut g = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if g.as_str() != name {
            g.clear();
            g.push_str(name);
        }
    }
}

impl cyrup_session_svc::ThemeAccess for TuiThemeAccess {
    fn active(&self) -> Option<String> {
        Some(self.active.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }

    /// Pi `getAvailableThemesWithPaths()` (`modes/interactive/theme/theme.ts:493-520` @v0.83.0):
    /// built-ins, then custom themes, then registered ones, deduped first-wins on `name` (`seen`,
    /// `:496-503`) and sorted `a.name.localeCompare(b.name)` (`:519`).
    ///
    /// [`cyrup_resources::ResourceSet::winners`] IS the dedupe — same first-wins rule, resolved
    /// through cyrup's scope precedence instead of upstream's fixed built-in/custom/registered
    /// order — so only the sort has to be re-applied here (`winners` is documented "order
    /// unspecified").
    ///
    /// `name` is the [`cyrup_resources::ResourceKey`], not `data.name`, because pi's `ThemeInfo.name`
    /// is by contract the string you hand back to `getTheme`/`setTheme` — and cyrup's key IS that
    /// string ([`Self::by_name`] and [`Self::set`] both normalize through `get_name`). `data.name`
    /// is not always usable for lookup: `Theme::parse` falls back to the file stem when the declared
    /// name is unusable (`cyrup-resources/src/theme.rs:328-337`), which would leave the listing
    /// naming a theme that cannot be loaded. It is also what cyrup's own `/settings → theme` picker
    /// lists and switches on (`selector.rs`'s `ListSelector::theme`), so the two agree.
    ///
    /// [CYRUP-DELTA] vs `theme.ts:506-508`: upstream gives a BUILT-IN the synthetic path
    /// `path.join(getThemesDir(), `${name}.json`)`, because pi's built-ins are real files under its
    /// themes dir. cyrup's two built-ins are compiled into the binary as string constants
    /// (`cyrup-resources`' `BUILTIN_DARK_JSON` / `BUILTIN_LIGHT_JSON`), so no such file exists and
    /// a synthesized path would name something a guest could not open. `null` is the honest answer,
    /// and it is the contract `HostServices::theme_list` and the SDK's `Ctx::theme_list` already
    /// document (EXT-021): `path` null ⇒ built-in.
    fn list(&self) -> Value {
        let mut rows: Vec<(String, Option<String>)> = self
            .resources
            .themes
            .winners()
            .map(|t| {
                (
                    t.key.as_str().to_string(),
                    t.origin_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
                )
            })
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        Value::Array(rows.into_iter().map(|(name, path)| json!({"name": name, "path": path})).collect())
    }

    /// Pi `getThemeByName(name)` (`theme.ts:671-677`) — load WITHOUT switching, `undefined` when it
    /// does not resolve. The serialized value is the theme's own source document
    /// (`{name, vars, colors, export}`, `cyrup_resources::ThemeData`), which is the shape both
    /// `Ctx::theme_by_name` and `Ctx::theme_json` hand the guest.
    fn by_name(&self, name: &str) -> Option<Value> {
        let theme = self.resources.themes.get_name(name)?;
        serde_json::to_value(&theme.data).ok()
    }

    /// Pi `setTheme(themeOrName)` (`interactive-mode.ts:2406-2417`).
    ///
    /// The validation is upstream's: `themeController.setThemeName` → `applyThemeName` →
    /// `setTheme(name)` → `loadTheme(name)`, which throws `Theme not found: {name}` for an
    /// unresolvable name (`theme.ts:622`) and is caught back into `{success: false, error}`
    /// (`:891-913`). Resolving against the registry here is the same test, run before anything is
    /// applied — so a bad name never reaches the run loop and never repaints.
    ///
    /// On success the resolved theme goes to the run loop, which applies it AND persists it. Both
    /// halves are pi's: `setThemeName` repaints, and the `settingsManager.setTheme(themeOrName)`
    /// guarded by `if (this.settingsManager.getTheme() !== themeOrName)` writes it back
    /// (`:2411-2414`).
    fn set(&self, name: &str) -> Result<(), String> {
        let theme = self
            .resources
            .themes
            .get_name(name)
            .ok_or_else(|| format!("Theme not found: {name}"))?;
        // The receiver is the run loop; it is gone only while the app is tearing down. Pi's
        // `setThemeName` cannot fail that way, and reporting a torn-down UI as a theme error would
        // be misleading, so a closed channel reports the no-UI state instead — the same string
        // `LiveHostServices::set_theme` uses when no handle is attached at all
        // (`core/extensions/runner.ts:263`).
        self.switch.send(theme.clone()).map_err(|_| "UI not available".to_string())
    }
}
