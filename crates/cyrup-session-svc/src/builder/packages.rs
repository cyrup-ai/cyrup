//! The package channels discovery reads: the on-disk installed-package registries the `install`
//! subcommand writes, and the packages DECLARED in settings (CFG-003).

use std::path::Path;

use cyrup_config::SettingsManager;
use cyrup_resources::{
    ConfiguredPackage, InstallScope, InstalledPackages, PackageFilter, PackageStore,
};

/// Load the on-disk installed-package registries the `install` subcommand writes — Global under
/// `<package_dir>/packages.json`, Project under `<cwd>/.cyrup/packages.json` (the exact paths
/// [`PackageStore::registry_path`] resolves for `PackageStore::new(package_dir, Some(cwd))`, the SAME
/// construction the bin's `install` uses at subcommands.rs:396) — and concatenate them in the fixed
/// project-then-global order discovery re-sorts into anyway (discovery.rs:435-439). This is the READ
/// half of C1 (gap-07 #1 / gap-13 C1): the write half already works (the bin persists correctly);
/// this threads the persisted registry into a live session, the missing wiring that made
/// `cyrup install` a runtime no-op for skill/prompt/theme/extension resources.
///
/// A missing registry file is an empty registry (the common "nothing installed" case) and a
/// malformed one is treated as "no packages from that scope" rather than aborting the whole session
/// build — mirroring the working `cyrup-ext-subagents::enumerate_installed_packages` precedent
/// (extension.rs:1269-1289) and `lock::load`'s own missing-file contract.
pub(super) fn load_installed_packages(package_dir: &Path, cwd: &Path) -> InstalledPackages {
    let store = PackageStore::new(package_dir.to_path_buf(), Some(cwd.to_path_buf()));
    let mut installed = InstalledPackages::default();
    for scope in [InstallScope::Project, InstallScope::Global] {
        let Some(registry_path) = store.registry_path(scope) else {
            continue;
        };
        if let Ok(registry) = cyrup_resources::package::lock::load(&registry_path) {
            installed.packages.extend(registry.packages);
        }
    }
    installed
}

/// Collect the packages DECLARED in settings into discovery's settings-package channel (CFG-003).
///
/// 1:1 with the head of Pi's `PackageManager.resolve()` (package-manager.ts:891-900): PROJECT
/// entries first, then GLOBAL, deduped by source identity so a project entry wins a collision — the
/// exact ordering that makes project-scope resources beat global ones under the shared package
/// precedence rank. Each entry's object-form include filters ride along
/// (`const filter = typeof pkg === "object" ? pkg : undefined`, :1231).
///
/// Reads the two RAW LAYERS, never the merged effective view: the merged view cannot say which
/// scope declared an entry, and discovery trust-gates project-scope packages.
///
/// A malformed entry is skipped with a message rather than dropping the array (or the settings
/// document) — the returned `Vec<String>` becomes startup diagnostics.
pub(super) fn configured_packages_from_settings(
    settings: &SettingsManager,
    cwd: &Path,
    agent_dir: &Path,
) -> (Vec<ConfiguredPackage>, Vec<String>) {
    let mut out: Vec<(String, ConfiguredPackage)> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for (layer, scope) in [
        (settings.project(), InstallScope::Project),
        (settings.global(), InstallScope::Global),
    ] {
        // Pi `getBaseDirForScope(entry.scope)` (package-manager.ts:2071-2080), the base the scope's
        // LOCAL sources resolve against before they can be compared.
        let base = cyrup_resources::scope_base_dir(cwd, agent_dir, scope);
        let (declared, layer_errors) = layer.packages_with_errors();
        errors.extend(layer_errors);
        for entry in declared {
            let source = entry.source().trim().to_string();
            if source.is_empty() {
                errors.push("settings `packages` entry has an empty `source`".to_string());
                continue;
            }
            let (extensions, skills, prompts, themes) = entry.filters();
            let built = ConfiguredPackage {
                source,
                scope,
                filter: PackageFilter {
                    // `autoload: false` flips the per-type lists from include filters to a delta
                    // (Pi `collectPackageResources`, package-manager.ts:2084-2085).
                    autoload: entry.autoload(),
                    extensions: extensions.map(<[String]>::to_vec),
                    skills: skills.map(<[String]>::to_vec),
                    prompts: prompts.map(<[String]>::to_vec),
                    themes: themes.map(<[String]>::to_vec),
                },
            };
            // Pi's `dedupePackages` (package-manager.ts:1681-1703), all three branches:
            //
            // - first sighting of an identity — keep it;
            // - the kept entry is PROJECT and this one is USER — normally drop this one, EXCEPT
            //   when the project entry is `autoload: false`, which its doc comment (:1676-1679)
            //   defines as "a delta over the global entry, so both are kept (delta first)". The
            //   base entry has to survive or the delta has nothing to layer over and the project
            //   patterns silently become the whole package;
            // - otherwise, a PROJECT entry replaces whatever is in the slot (`result[index] =
            //   entry`, :1698) — project wins, later project entry wins an intra-scope repeat.
            //
            // The key is Pi's `getPackageIdentity(source, entry.scope)` (:1676-1690), NOT the raw
            // source string (CFG-026): `npm:x@1`/`npm:x@2` and an SSH/HTTPS pair for one repo
            // collide, while `"./pack"` declared in BOTH scopes does not — it names
            // `<cwd>/.cyrup/pack` in project scope and `<agent_dir>/pack` in global scope, two
            // different trees, so pi keeps both and cyrup used to drop the global one.
            let identity = cyrup_resources::package_identity(&built.source, &base);
            match out.iter().position(|(id, _)| *id == identity) {
                None => out.push((identity, built)),
                Some(index) => {
                    let existing_is_project_delta = out.get(index).is_some_and(|(_, p)| {
                        p.scope == InstallScope::Project && p.filter.is_delta()
                    });
                    if existing_is_project_delta && built.scope == InstallScope::Global {
                        out.push((identity, built));
                    } else if built.scope == InstallScope::Project
                        && let Some(slot) = out.get_mut(index)
                    {
                        *slot = (identity, built);
                    }
                }
            }
        }
    }
    (out.into_iter().map(|(_, p)| p).collect(), errors)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use std::path::Path;

    /// CFG-010 (dedupe half) — Pi's `dedupePackages` keeps BOTH entries, delta first, when a
    /// PROJECT entry carrying `autoload: false` collides with a USER one for the same package
    /// identity: "A project entry with autoload=false is a delta over the global entry, so both
    /// are kept (delta first)" (package-manager.ts:1676-1679, code at :1691-1696). Dropping the
    /// global entry turns the delta form inside out — the project entry's patterns become the
    /// ONLY thing that loads instead of a layer over the full package.
    #[test]
    fn a_project_autoload_false_entry_is_a_delta_over_the_global_entry_not_a_replacement() {
        use cyrup_config::{InMemorySettingsStore, SettingsManager, SettingsScope};
        use cyrup_resources::InstallScope;
        use std::sync::Arc;

        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(
            SettingsScope::Project,
            r#"{"packages":[{"source":"npm:pi-tools","autoload":false,"extensions":["-extensions/foo.ts"]}]}"#,
        );
        store.seed(SettingsScope::Global, r#"{"packages":["npm:pi-tools"]}"#);
        let mgr = SettingsManager::load(store, true);

        let (pkgs, errors) = super::configured_packages_from_settings(&mgr, Path::new("/proj"), Path::new("/home/u/.cyrup/agent"));
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            pkgs.len(),
            2,
            "the global entry must survive so the project delta has something to layer over, got \
             {pkgs:?}"
        );
        assert_eq!(pkgs[0].scope, InstallScope::Project, "delta first");
        assert!(pkgs[0].filter.is_delta());
        assert_eq!(pkgs[1].scope, InstallScope::Global);
        assert!(pkgs[1].filter.is_empty(), "the base entry keeps no filter");
    }

    /// The other side of the same branch: without `autoload: false` a project entry still REPLACES
    /// the global one outright (`else if (entry.scope === "project")` / the plain drop of a later
    /// user entry, package-manager.ts:1694-1698).
    #[test]
    fn a_plain_project_entry_still_shadows_the_global_one() {
        use cyrup_config::{InMemorySettingsStore, SettingsManager, SettingsScope};
        use cyrup_resources::InstallScope;
        use std::sync::Arc;

        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(
            SettingsScope::Project,
            r#"{"packages":[{"source":"npm:pi-tools","skills":["skills/a"]}]}"#,
        );
        store.seed(SettingsScope::Global, r#"{"packages":["npm:pi-tools"]}"#);
        let mgr = SettingsManager::load(store, true);

        let (pkgs, _) = super::configured_packages_from_settings(&mgr, Path::new("/proj"), Path::new("/home/u/.cyrup/agent"));
        assert_eq!(pkgs.len(), 1, "{pkgs:?}");
        assert_eq!(pkgs[0].scope, InstallScope::Project);
    }

    /// CFG-026. The dedupe key is Pi `getPackageIdentity(source, entry.scope)`
    /// (package-manager.ts:1676-1690 @v0.83.0), which resolves a LOCAL source against that scope's
    /// base dir (`getBaseDirForScope`, :2071-2080). `"./pack"` in both scopes therefore names two
    /// different trees — `<cwd>/.cyrup/pack` and `<agent_dir>/pack` — and pi keeps BOTH.
    ///
    /// RED before the fix: the key was the trimmed source string, the two entries collided, and the
    /// project one replaced the global one, so `<agent_dir>/pack`'s skills/prompts/themes vanished.
    #[test]
    fn the_same_relative_local_source_in_both_scopes_is_two_packages() {
        use cyrup_config::{InMemorySettingsStore, SettingsManager, SettingsScope};
        use cyrup_resources::InstallScope;
        use std::sync::Arc;

        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(SettingsScope::Project, r#"{"packages":["./pack"]}"#);
        store.seed(SettingsScope::Global, r#"{"packages":["./pack"]}"#);
        let mgr = SettingsManager::load(store, true);

        let (pkgs, errors) = super::configured_packages_from_settings(
            &mgr,
            Path::new("/proj"),
            Path::new("/home/u/.cyrup/agent"),
        );
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            pkgs.len(),
            2,
            "`./pack` means /proj/.cyrup/pack in project scope and /home/u/.cyrup/agent/pack in \
             global scope — two packages, not one; got {pkgs:?}"
        );
        assert_eq!(pkgs[0].scope, InstallScope::Project);
        assert_eq!(pkgs[1].scope, InstallScope::Global);
    }

    /// The other direction of the same key change: an ABSOLUTE local source is scope-independent,
    /// so the two scopes still collide and the project entry wins — the behaviour a raw
    /// source-string key got right and must not lose.
    #[test]
    fn the_same_absolute_local_source_in_both_scopes_is_still_one_package() {
        use cyrup_config::{InMemorySettingsStore, SettingsManager, SettingsScope};
        use cyrup_resources::InstallScope;
        use std::sync::Arc;

        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(SettingsScope::Project, r#"{"packages":["/shared/pack"]}"#);
        // A different SPELLING of the same absolute path: `resolvePath` normalizes `.`/`..` before
        // the comparison, which a string key cannot do.
        store.seed(SettingsScope::Global, r#"{"packages":["/shared/sub/../pack"]}"#);
        let mgr = SettingsManager::load(store, true);

        let (pkgs, _) = super::configured_packages_from_settings(
            &mgr,
            Path::new("/proj"),
            Path::new("/home/u/.cyrup/agent"),
        );
        assert_eq!(pkgs.len(), 1, "{pkgs:?}");
        assert_eq!(pkgs[0].scope, InstallScope::Project);
    }

    /// And the version-ignoring half of `getPackageIdentity` (`npm:${parsed.name}`, :1678-1680):
    /// two entries pinning different versions of one npm package are ONE package, where a raw
    /// source-string key loaded both.
    #[test]
    fn two_npm_versions_of_one_package_dedupe_to_the_project_entry() {
        use cyrup_config::{InMemorySettingsStore, SettingsManager, SettingsScope};
        use cyrup_resources::InstallScope;
        use std::sync::Arc;

        let store = Arc::new(InMemorySettingsStore::new());
        store.seed(SettingsScope::Project, r#"{"packages":["npm:pi-tools@2"]}"#);
        store.seed(SettingsScope::Global, r#"{"packages":["npm:pi-tools@1"]}"#);
        let mgr = SettingsManager::load(store, true);

        let (pkgs, _) = super::configured_packages_from_settings(
            &mgr,
            Path::new("/proj"),
            Path::new("/home/u/.cyrup/agent"),
        );
        assert_eq!(pkgs.len(), 1, "{pkgs:?}");
        assert_eq!(pkgs[0].scope, InstallScope::Project);
        assert_eq!(pkgs[0].source, "npm:pi-tools@2");
    }
}
