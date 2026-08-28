//! The blocking body of [`discover`](super::discover), one collector per section.
//!
//! [`discover_blocking`] is a table of contents: it declares the seven accumulators, runs the ten
//! `collect_*`/`apply_*` sections below in order against them, then folds the result into a
//! [`DiscoveryReport`]. Each section is independent — none returns early, none reads a local the
//! previous one left behind — so the order here IS the precedence story, nothing more.

use std::path::PathBuf;

use cyrup_core::CancelToken;

use crate::error::{ResourceDiagnostic, ResourceError, ResourceKind, ResourceWarning};
use crate::package::store::installed_dir;
use crate::package::{
    ConfiguredPackage, InstalledPackage, PackageFilter, ResourceSelector, resolve_manifest,
};
use crate::prompt::PromptTemplate;
use crate::scope::{InstallScope, ResourceOrigin, ResourceScope};
use crate::skill::Skill;
use crate::theme::{Theme, builtin_themes};

use super::packages::{
    ConfiguredPackageResolution, PackageTree, PackageTreeRef, collect_ancestor_agents_skill_dirs,
    override_enabled, resolve_configured_package, retain_by_package_filter, scope_base_dir,
    subtract_delta_shadow,
};
use super::scan::{
    add_local_entries, add_prompt_path, add_skill_path, add_theme_path, emit_collisions,
    load_one_skill, name_disabled, scan_prompt_root, scan_skill_root, scan_theme_root,
};
use super::{
    DiscoveryConfig, DiscoveryReport, LooseExtension, ResourceRegistry, ResourceSet,
    discover_append_system_prompt_file, discover_system_prompt_file, scan_loose_extension_root,
};

/// The seven accumulators every section of [`discover_blocking`] appends to.
///
/// Threaded through the collectors as one `&mut` so each section keeps the exact body it had when
/// they were all inlined: every field is destructured back out to the same name at the top of the
/// collector that touches it.
#[derive(Default)]
struct Accum {
    skills: Vec<Skill>,
    prompts: Vec<PromptTemplate>,
    themes: Vec<Theme>,
    warnings: Vec<ResourceWarning>,
    diagnostics: Vec<ResourceDiagnostic>,
    ext_paths: Vec<PathBuf>,
    /// Auto-discovered loose extensions, ENABLED and DISABLED alike — see [`LooseExtension`].
    loose_extensions: Vec<LooseExtension>,
}

pub(super) fn discover_blocking(
    cfg: &DiscoveryConfig,
    cancel: &CancelToken,
) -> Result<DiscoveryReport, ResourceError> {
    let mut acc = Accum::default();

    collect_builtin_themes(cfg, &mut acc);
    collect_global_loose(cfg, &mut acc);
    collect_global_loose_extensions(cfg, &mut acc);
    collect_settings_listings(cfg, &mut acc);
    collect_packages(cfg, cancel, &mut acc);
    collect_project_loose(cfg, &mut acc);
    collect_project_loose_extensions(cfg, &mut acc);
    collect_discover_contributions(cfg, &mut acc);
    collect_cli_paths(cfg, &mut acc);
    apply_disabled_filter(cfg, &mut acc);

    let Accum {
        skills,
        prompts,
        themes,
        warnings,
        mut diagnostics,
        ext_paths,
        loose_extensions,
    } = acc;

    let registry = ResourceRegistry {
        skills: ResourceSet::build(skills),
        prompts: ResourceSet::build(prompts),
        themes: ResourceSet::build(themes),
        ext_crate_paths: ext_paths,
        loose_extensions,
    };

    // Same-name collision diagnostics (skills.ts:410-427; resource-loader.ts:913-964): each
    // shadowed candidate yields a `collision` diagnostic carrying winner/loser paths.
    emit_collisions(
        &registry.skills,
        ResourceKind::Skill,
        |s| s.skill_md.clone(),
        &mut diagnostics,
    );
    emit_collisions(
        &registry.prompts,
        ResourceKind::Prompt,
        |p| p.path.clone(),
        &mut diagnostics,
    );
    emit_collisions(
        &registry.themes,
        ResourceKind::Theme,
        |t| t.origin_path.clone().unwrap_or_default(),
        &mut diagnostics,
    );

    Ok(DiscoveryReport {
        registry,
        warnings,
        diagnostics,
        // Pi computes both inside the same `reload()` that produces the registry
        // (`resource-loader.ts:525`, `:531-535` @v0.83.0), off the same `cwd`, `agentDir` and
        // `isProjectTrusted()` this config carries, so they ride out on the same report (CFG-035).
        system_prompt_file: discover_system_prompt_file(
            &cfg.cwd,
            &cfg.global_dir,
            cfg.trusted_project,
        ),
        append_system_prompt_file: discover_append_system_prompt_file(
            &cfg.cwd,
            &cfg.global_dir,
            cfg.trusted_project,
        ),
    })
}

/// The built-in themes compiled into the binary.
fn collect_builtin_themes(cfg: &DiscoveryConfig, acc: &mut Accum) {
    let themes = &mut acc.themes;

    // --- built-in themes (R-09-011) ---
    if cfg.enable_themes {
        themes.extend(builtin_themes());
    }
}

/// Auto-discovered loose resources under the global/global-agents/user-agents roots.
fn collect_global_loose(cfg: &DiscoveryConfig, acc: &mut Accum) {
    let skills = &mut acc.skills;
    let prompts = &mut acc.prompts;
    let themes = &mut acc.themes;
    let warnings = &mut acc.warnings;
    let diagnostics = &mut acc.diagnostics;

    // --- global loose resources (R-09-002/008/012) ---
    // Settings override patterns (Pi `globalSettings.{skills,prompts,themes}`) selectively
    // enable/disable these auto-discovered loose resources (package-manager.ts:2271-2304).
    if cfg.enable_skills {
        // The user-tier cross-tool `~/.agents/skills` (Pi `userAgentsSkillsDir`,
        // package-manager.ts:2286,2377-2389) is loaded as a USER/global-scope source when the
        // session-svc builder plumbs `user_agents_dir = $HOME/.agents`.
        let mut roots = vec![
            cfg.global_dir.join("skills"),
            cfg.global_agents_dir.join("skills"),
        ];
        if let Some(user_agents) = &cfg.user_agents_dir {
            roots.push(user_agents.join("skills"));
        }
        for root in roots {
            let mut buf = Vec::new();
            scan_skill_root(
                &root,
                ResourceScope::Global,
                &root,
                &mut buf,
                warnings,
                diagnostics,
            );
            buf.retain(|s| override_enabled(&s.skill_md, &cfg.global_overrides.skills, &root));
            skills.extend(buf);
        }
    }
    if cfg.enable_prompts {
        let root = cfg.global_dir.join("prompts");
        let mut buf = Vec::new();
        scan_prompt_root(&root, ResourceScope::Global, &root, &mut buf, warnings);
        buf.retain(|p| override_enabled(&p.path, &cfg.global_overrides.prompts, &root));
        prompts.extend(buf);
    }
    if cfg.enable_themes {
        let root = cfg.global_dir.join("themes");
        let mut buf = Vec::new();
        scan_theme_root(&root, ResourceScope::Global, &root, &mut buf, warnings);
        buf.retain(|t| {
            t.origin_path
                .as_deref()
                .is_none_or(|p| override_enabled(p, &cfg.global_overrides.themes, &root))
        });
        themes.extend(buf);
    }
}

/// Auto-discovered loose extensions under the **global** root `<agent_dir>/extensions`.
///
/// The counterpart of [`collect_global_loose`] for Pi's fourth `RESOURCE_TYPES` member
/// (`package-manager.ts:194`): `collectAutoExtensionEntries` (`package-manager.ts:587-630`) over the
/// same root `cyrup_ext::loader::discover_with_diagnostics` scans pre-trust
/// (`cyrup-session-svc/src/builder.rs:2244-2248` → `agent_dir.join("extensions")`), filtered by the
/// GLOBAL settings layer's `extensions` array.
///
/// Unlike its three siblings this does not `retain` — [`scan_loose_extension_root`] keeps the
/// disabled entries with `enabled: false`, because the extension loader needs the negative half to
/// honour a `-pattern` at load time (`DiscoveryRoots::disabled`).
///
/// Not gated on `enable_skills`/`enable_prompts`/`enable_themes`: `--no-extensions` is a separate
/// flag, applied by the session builder (which simply does not scan these roots), so gating here
/// would double-apply an unrelated switch.
fn collect_global_loose_extensions(cfg: &DiscoveryConfig, acc: &mut Accum) {
    let root = cfg.global_dir.join("extensions");
    acc.loose_extensions.extend(scan_loose_extension_root(
        &root,
        ResourceScope::Global,
        &cfg.global_overrides.extensions,
    ));
}

/// Auto-discovered loose extensions under the **project** root `<cwd>/.cyrup/extensions`,
/// trust-gated exactly like [`collect_project_loose`] — and like the loader's own project root,
/// which `DiscoveredExtension::is_trusted` gates post-trust
/// (`cyrup-ext/src/loader.rs`, `ExtOrigin::Project`).
fn collect_project_loose_extensions(cfg: &DiscoveryConfig, acc: &mut Accum) {
    if !cfg.trusted_project {
        return;
    }
    let root = cfg.cwd.join(".cyrup/extensions");
    acc.loose_extensions.extend(scan_loose_extension_root(
        &root,
        ResourceScope::Project,
        &cfg.project_overrides.extensions,
    ));
}

/// The plain-path positive listings declared in the project/global settings arrays.
fn collect_settings_listings(cfg: &DiscoveryConfig, acc: &mut Accum) {
    let skills = &mut acc.skills;
    let prompts = &mut acc.prompts;
    let themes = &mut acc.themes;
    let warnings = &mut acc.warnings;
    let diagnostics = &mut acc.diagnostics;
    let ext_paths = &mut acc.ext_paths;

    // --- settings positive listings (Pi `resolveLocalEntries`, package-manager.ts:906-928) ---
    // Plain paths listed in the project/global settings `skills`/`prompts`/`themes` arrays are
    // loaded as `source:"local"` entries, which Pi ranks *above* auto-discovered files of the same
    // scope (`resourcePrecedenceRank` 184-188). Project entries are trust-gated and resolve relative
    // to `<cwd>/.cyrup` (Pi `projectBaseDir = join(cwd, CONFIG_DIR)`, 900); global entries resolve
    // relative to the global config dir (Pi `globalBaseDir = agentDir`, 899).
    if cfg.trusted_project {
        let project_base = cfg.cwd.join(".cyrup");
        add_local_entries(
            &project_base,
            &cfg.project_overrides,
            ResourceScope::ProjectSettings,
            cfg,
            skills,
            prompts,
            themes,
            ext_paths,
            warnings,
            diagnostics,
        );
    }
    add_local_entries(
        &cfg.global_dir,
        &cfg.global_overrides,
        ResourceScope::GlobalSettings,
        cfg,
        skills,
        prompts,
        themes,
        ext_paths,
        warnings,
        diagnostics,
    );
}

/// Both package channels: the settings-declared trees first, then the install registry.
fn collect_packages(cfg: &DiscoveryConfig, cancel: &CancelToken, acc: &mut Accum) {
    let skills = &mut acc.skills;
    let prompts = &mut acc.prompts;
    let themes = &mut acc.themes;
    let warnings = &mut acc.warnings;
    let diagnostics = &mut acc.diagnostics;
    let ext_paths = &mut acc.ext_paths;

    // --- packages: settings-declared (CFG-003) + the install registry (R-09-015/016/017/018) ---
    // All packages share precedence rank 4 (Pi `resourcePrecedenceRank`, package-manager.ts:185).
    // Pi pushes project-scope packages before global ones (allPackages, 887-893), so under that
    // shared rank a project-local package wins a same-name tie with a global one. Stable-order both
    // channels project-first to reproduce that (config order is preserved within a scope).
    //
    // Pi's ONLY channel is the settings one — `resolve()` re-reads
    // `projectSettings.packages`/`globalSettings.packages` on every call (891-901) — so the
    // settings-declared trees are resolved FIRST and an install-registry record for the same working
    // tree is skipped as a duplicate.
    let mut trees: Vec<PackageTree> = Vec::new();
    let mut ordered_cfg: Vec<&ConfiguredPackage> = cfg.configured_packages.iter().collect();
    ordered_cfg.sort_by_key(|p| match p.scope {
        InstallScope::Project => 0u8,
        InstallScope::Global => 1u8,
    });
    // Project-scope `autoload: false` entries that ACTUALLY resolved, in declaration order. The
    // global entry each one deltas over carries it as its `delta_shadow` (Pi's `dedupePackages`
    // keeps both halves of the pair, package-manager.ts:1691-1696). Recorded from the admitted
    // trees, not from the raw settings, so an untrusted or unresolvable project entry cannot reach
    // in and suppress resources from the global package it names.
    //
    // Keyed by `getPackageIdentity` computed against EACH side's own scope base — the exact key
    // `dedupePackages` pairs on (`getPackageIdentity(source, entry.scope)`,
    // package-manager.ts:1703 @v0.83.0), and the same key `findAutoloadDeltaBase` used above to
    // decide the pair exists at all (`:1307` vs `:1311`). A raw source-string key disagreed with
    // that lookup in both directions: `git@github.com:acme/p.git` at project scope and
    // `https://github.com/acme/p.git` at global scope are ONE identity (`git:<host>/<path>`,
    // `:1682-1684`), so the delta base resolved them onto one tree while the string compare found
    // no pair — leaving the global half with no `delta_shadow`, hence skipped by the `seen_trees`
    // guard below, silently dropping every resource it autoloads. Conversely one RELATIVE local
    // string is two identities across scopes (`:1685-1688`), where pi makes no pair at all.
    let project_delta_base = scope_base_dir(&cfg.cwd, &cfg.global_dir, InstallScope::Project);
    let global_delta_base = scope_base_dir(&cfg.cwd, &cfg.global_dir, InstallScope::Global);
    let mut project_deltas: Vec<(String, PackageFilter)> = Vec::new();
    for declared in ordered_cfg {
        if declared.scope == InstallScope::Project && !cfg.trusted_project {
            continue; // fail-closed trust gate (Pi `assertProjectTrustedForScope`, 2055-2058)
        }
        match resolve_configured_package(declared, &cfg.configured_packages, cfg, cancel) {
            Ok(ConfiguredPackageResolution::Tree(tree)) => {
                let mut tree = *tree;
                match declared.scope {
                    InstallScope::Project if declared.filter.is_delta() => project_deltas.push((
                        crate::package::package_identity(&declared.source, &project_delta_base),
                        declared.filter.clone(),
                    )),
                    InstallScope::Global => {
                        let identity =
                            crate::package::package_identity(&declared.source, &global_delta_base);
                        tree.delta_shadow = project_deltas
                            .iter()
                            .find(|(candidate, _)| *candidate == identity)
                            .map(|(_, filter)| filter.clone());
                    }
                    InstallScope::Project => {}
                }
                trees.push(tree);
            }
            // package-manager.ts:1330-1334 — a local FILE entry registers as an extension directly,
            // bypassing `collectPackageResources` entirely. No manifest is read, no filter applies
            // (Pi hands `filter` only to `collectPackageResources`, `:1337`), and it takes no part in
            // the `dedupePackages` delta pairing, because a delta base is looked up by source string
            // and this entry never becomes a tree.
            Ok(ConfiguredPackageResolution::ExtensionFile(file)) => {
                if !ext_paths.contains(&file) {
                    ext_paths.push(file);
                }
            }
            // package-manager.ts:1324-1326 / `:1343-1345` — silent by construction.
            Ok(ConfiguredPackageResolution::Skip) => {}
            Err(diag) => diagnostics.push(*diag),
        }
    }
    let mut ordered_pkgs: Vec<&InstalledPackage> = cfg.installed.packages.iter().collect();
    ordered_pkgs.sort_by_key(|p| match p.scope {
        InstallScope::Project => 0u8,
        InstallScope::Global => 1u8,
    });
    for pkg in ordered_pkgs {
        if pkg.scope == InstallScope::Project && !cfg.trusted_project {
            continue; // fail-closed trust gate
        }
        let Some(dir) = installed_dir(
            &pkg.source,
            pkg.scope,
            &pkg.id,
            // The package-store global root the bin wrote to (`dirs.package_dir`), NOT `global_dir`
            // (the loose-resource agent root) — see the `package_global_dir` field docs.
            &cfg.package_global_dir,
            cfg.project_root.as_deref(),
        ) else {
            continue;
        };
        trees.push(PackageTree {
            dir,
            id: pkg.id.clone(),
            tier: pkg.scope.package_resource_scope(),
            disabled: pkg.disabled.clone(),
            filter: PackageFilter::default(),
            delta_shadow: None,
        });
    }
    let mut seen_trees: Vec<PathBuf> = Vec::new();
    for tree in trees {
        let PackageTree {
            dir,
            id,
            tier,
            disabled,
            filter,
            delta_shadow,
        } = tree;
        // The base half of a delta pair intentionally revisits a tree the project delta already
        // walked — Pi processes both entries (`dedupePackages`, package-manager.ts:1691-1696).
        // Every other repeat is a genuine duplicate (the install registry re-declaring a settings
        // package) and is skipped.
        if seen_trees.contains(&dir) && delta_shadow.is_none() {
            continue;
        }
        seen_trees.push(dir.clone());
        let pkg = PackageTreeRef {
            id: &id,
            disabled: &disabled,
            filter: &filter,
            delta_shadow: delta_shadow.as_ref(),
        };
        let manifest = match resolve_manifest(&dir) {
            Ok(m) => m,
            Err(e) => {
                warnings.push(ResourceWarning::new(
                    ResourceKind::Package,
                    dir,
                    e.to_string(),
                ));
                continue;
            }
        };
        let origin = ResourceOrigin::Package {
            id: pkg.id.clone(),
            scope: tier,
        };
        if cfg.enable_skills {
            for sdir in &manifest.skills {
                let mut buf = Vec::new();
                // A manifest entry may glob-resolve to a single `SKILL.md`/`.md` file as well as a
                // directory root (package-manager.ts collectFilesFromManifestEntries).
                if sdir.is_file() {
                    let root = sdir.parent().unwrap_or(sdir);
                    load_one_skill(sdir, tier, root, &mut buf, warnings, diagnostics);
                } else {
                    scan_skill_root(sdir, tier, sdir, &mut buf, warnings, diagnostics);
                }
                buf.retain(|s| {
                    !pkg.disabled
                        .is_disabled(&ResourceSelector::Skill(s.name.clone()))
                });
                retain_by_package_filter(
                    &mut buf,
                    |s| s.skill_md.clone(),
                    &dir,
                    pkg.filter.skills.as_deref(),
                    pkg.filter.is_delta(),
                );
                subtract_delta_shadow(
                    &mut buf,
                    |s| s.skill_md.clone(),
                    &dir,
                    pkg.delta_shadow.and_then(|f| f.skills.as_deref()),
                );
                for s in &mut buf {
                    s.origin = origin.clone();
                }
                skills.extend(buf);
            }
        }
        if cfg.enable_prompts {
            for pdir in &manifest.prompts {
                let mut buf = Vec::new();
                if pdir.is_file() {
                    let po = ResourceOrigin::LooseFile {
                        scope: tier,
                        root: pdir.parent().unwrap_or(pdir).to_path_buf(),
                    };
                    match PromptTemplate::load(pdir, tier, po) {
                        Ok(t) => buf.push(t),
                        Err(e) => warnings.push(ResourceWarning::new(
                            ResourceKind::Prompt,
                            pdir,
                            e.to_string(),
                        )),
                    }
                } else {
                    scan_prompt_root(pdir, tier, pdir, &mut buf, warnings);
                }
                buf.retain(|p| {
                    !pkg.disabled
                        .is_disabled(&ResourceSelector::Prompt(p.key.as_str().to_string()))
                });
                retain_by_package_filter(
                    &mut buf,
                    |p| p.path.clone(),
                    &dir,
                    pkg.filter.prompts.as_deref(),
                    pkg.filter.is_delta(),
                );
                subtract_delta_shadow(
                    &mut buf,
                    |p| p.path.clone(),
                    &dir,
                    pkg.delta_shadow.and_then(|f| f.prompts.as_deref()),
                );
                for p in &mut buf {
                    p.origin = origin.clone();
                }
                prompts.extend(buf);
            }
        }
        if cfg.enable_themes {
            for tdir in &manifest.themes {
                let mut buf = Vec::new();
                if tdir.is_file() {
                    let to = ResourceOrigin::LooseFile {
                        scope: tier,
                        root: tdir.parent().unwrap_or(tdir).to_path_buf(),
                    };
                    match Theme::load(tdir, tier, to) {
                        Ok(t) => buf.push(t),
                        Err(e) => warnings.push(ResourceWarning::new(
                            ResourceKind::Theme,
                            tdir,
                            e.to_string(),
                        )),
                    }
                } else {
                    scan_theme_root(tdir, tier, tdir, &mut buf, warnings);
                }
                buf.retain(|t| {
                    !pkg.disabled
                        .is_disabled(&ResourceSelector::Theme(t.data.name.clone()))
                });
                retain_by_package_filter(
                    &mut buf,
                    |t| t.origin_path.clone().unwrap_or_default(),
                    &dir,
                    pkg.filter.themes.as_deref(),
                    pkg.filter.is_delta(),
                );
                subtract_delta_shadow(
                    &mut buf,
                    |t| t.origin_path.clone().unwrap_or_default(),
                    &dir,
                    pkg.delta_shadow.and_then(|f| f.themes.as_deref()),
                );
                for t in &mut buf {
                    t.origin = origin.clone();
                }
                themes.extend(buf);
            }
        }
        let mut ext_buf: Vec<PathBuf> = Vec::new();
        for ext in &manifest.extensions {
            let name = ext
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            if !pkg.disabled.is_disabled(&ResourceSelector::Extension(name)) {
                ext_buf.push(ext.clone());
            }
        }
        retain_by_package_filter(
            &mut ext_buf,
            Clone::clone,
            &dir,
            pkg.filter.extensions.as_deref(),
            pkg.filter.is_delta(),
        );
        subtract_delta_shadow(
            &mut ext_buf,
            Clone::clone,
            &dir,
            pkg.delta_shadow.and_then(|f| f.extensions.as_deref()),
        );
        for e in ext_buf {
            if !ext_paths.contains(&e) {
                ext_paths.push(e);
            }
        }

        // A package tree with no manifest AND none of the conventional resource dirs is a BARE
        // EXTENSION directory: `collectPackageResources` returns false (package-manager.ts:2126-
        // 2140 @v0.83.0 — `hasAnyDir` stays false), and `resolveLocalExtensionSource` then does
        // `this.addResource(accumulator.extensions, resolved, metadata, true)` (`:1338-1341`),
        // registering the directory itself. cyrup pushed only `manifest.extensions` entries, so
        // `"packages": ["./my-ext"]` on a manifest-less extension loaded nothing (CFG-027).
        if manifest.kind == crate::package::manifest::ManifestKind::AutoDiscovered
            && manifest.extensions.is_empty()
            && manifest.skills.is_empty()
            && manifest.prompts.is_empty()
            && manifest.themes.is_empty()
            && manifest.agents.is_empty()
            && !ext_paths.contains(&dir)
        {
            ext_paths.push(dir.clone());
        }
    }
}

/// Trust-gated loose project resources at `cwd`, plus the `.agents/skills` ancestor walk.
fn collect_project_loose(cfg: &DiscoveryConfig, acc: &mut Accum) {
    let skills = &mut acc.skills;
    let prompts = &mut acc.prompts;
    let themes = &mut acc.themes;
    let warnings = &mut acc.warnings;
    let diagnostics = &mut acc.diagnostics;

    // --- project loose resources (trust-gated) (R-09-002/003/008/012) ---
    // Pi loads loose project resources from the cwd, gated on TRUST alone — it has no `project_root`
    // precondition. Gating on `project_root.is_some()` left this block DEAD in production (the
    // session-svc builder sets `trusted_project` but never `project_root`), so cyrup discovered ZERO
    // project skills/prompts/themes live. `.cyrup/{skills,prompts,themes}` are read at `cwd`
    // (skills.ts:432 `resolve(resolvedCwd, CONFIG_DIR_NAME, …)`; resource-loader.ts:764-768); the
    // `.agents/skills` tree is walked cwd→git-root (`collectAncestorAgentsSkillDirs`,
    // package-manager.ts:440-459). The whole block already keys off `cfg.cwd`, never `project_root`.
    if cfg.trusted_project {
        let base = &cfg.cwd;
        {
            if cfg.enable_skills {
                // `.cyrup/skills` (Pi `.pi/skills`) is read at `cwd` only — `projectBaseDir =
                // join(cwd, CONFIG_DIR_NAME)` (package-manager.ts:900), no ancestor walk.
                {
                    let root = base.join(".cyrup/skills");
                    let mut buf = Vec::new();
                    scan_skill_root(
                        &root,
                        ResourceScope::Project,
                        &root,
                        &mut buf,
                        warnings,
                        diagnostics,
                    );
                    buf.retain(|s| {
                        override_enabled(&s.skill_md, &cfg.project_overrides.skills, &root)
                    });
                    skills.extend(buf);
                }
                // `.agents/skills` is walked up **every ancestor** from `cwd` to the git repo root
                // (or the filesystem root if there is none) — Pi `collectAncestorAgentsSkillDirs`
                // (package-manager.ts:440-459) feeding `projectAgentsSkillDirs`
                // (package-manager.ts:2286-2290), each loaded with its own `.agents` baseDir
                // (2326-2342). The user-tier skills dir is filtered out so it is not double-counted,
                // 1:1 with Pi's `.filter((dir) => resolve(dir) !== resolve(userAgentsSkillsDir))` (2289).
                //
                // USER-TIER DIR — DOCUMENTED DEFERRAL (residual #6). Pi's `userAgentsSkillsDir =
                // join(getHomeDir(), ".agents", "skills")` is literally `$HOME/.agents/skills`
                // (package-manager.ts:2286; `getHomeDir` = `process.env.HOME || homedir()`, :217) — the
                // user tier of the SAME cross-tool `.agents/skills` convention walked above. cyrup
                // instead uses `global_agents_dir/skills` = `agent_dir/agents/skills`
                // (= `~/.cyrup/agent/agents/skills` by default; DiscoveryConfig::new + session-svc
                // builder.rs:481). Pi-faithful is `~/.agents/skills`, and cyrup-config/trust.rs:173
                // ALREADY uses `home.join(".agents")` for the matching trust-walk exclusion — so this
                // is a genuine divergence (not a deliberate relocation), but the complete fix is
                // out-of-scope here: it needs `$HOME/.agents/skills` plumbed from cyrup-config into
                // `DiscoveryConfig` (a new field) and set by cyrup-session-svc — both outside the
                // editable crate set for this pass. Tracked in spec/gap-analysis/00-residual-ledger.md #6.
                // Dedup the ancestor walk against the user-tier `~/.agents/skills` (Pi
                // `.filter((dir) => resolve(dir) !== resolve(userAgentsSkillsDir))`,
                // package-manager.ts:2289) when plumbed; otherwise fall back to the legacy
                // `global_agents_dir/skills` exclusion.
                let user_agents_skills = cfg.user_agents_dir.as_ref().map_or_else(
                    || cfg.global_agents_dir.join("skills"),
                    |d| d.join("skills"),
                );
                for root in collect_ancestor_agents_skill_dirs(&cfg.cwd) {
                    if root == user_agents_skills {
                        continue;
                    }
                    let mut buf = Vec::new();
                    scan_skill_root(
                        &root,
                        ResourceScope::Project,
                        &root,
                        &mut buf,
                        warnings,
                        diagnostics,
                    );
                    buf.retain(|s| {
                        override_enabled(&s.skill_md, &cfg.project_overrides.skills, &root)
                    });
                    skills.extend(buf);
                }
            }
            if cfg.enable_prompts {
                let root = base.join(".cyrup/prompts");
                let mut buf = Vec::new();
                scan_prompt_root(&root, ResourceScope::Project, &root, &mut buf, warnings);
                buf.retain(|p| override_enabled(&p.path, &cfg.project_overrides.prompts, &root));
                prompts.extend(buf);
            }
            if cfg.enable_themes {
                let root = base.join(".cyrup/themes");
                let mut buf = Vec::new();
                scan_theme_root(&root, ResourceScope::Project, &root, &mut buf, warnings);
                buf.retain(|t| {
                    t.origin_path
                        .as_deref()
                        .is_none_or(|p| override_enabled(p, &cfg.project_overrides.themes, &root))
                });
                themes.extend(buf);
            }
        }
    }
}

/// Paths contributed by `resources_discover` extension hooks.
fn collect_discover_contributions(cfg: &DiscoveryConfig, acc: &mut Accum) {
    let skills = &mut acc.skills;
    let prompts = &mut acc.prompts;
    let themes = &mut acc.themes;
    let warnings = &mut acc.warnings;
    let diagnostics = &mut acc.diagnostics;

    // --- resources_discover contributions (R-09-022) ---
    if cfg.enable_skills {
        for p in &cfg.extra.skill_paths {
            add_skill_path(
                p,
                ResourceScope::Discovered,
                ResourceOrigin::Builtin,
                skills,
                warnings,
                diagnostics,
            );
        }
    }
    if cfg.enable_prompts {
        for p in &cfg.extra.prompt_paths {
            add_prompt_path(
                p,
                ResourceScope::Discovered,
                ResourceOrigin::Builtin,
                prompts,
                warnings,
            );
        }
    }
    if cfg.enable_themes {
        for p in &cfg.extra.theme_paths {
            add_theme_path(
                p,
                ResourceScope::Discovered,
                ResourceOrigin::Builtin,
                themes,
                warnings,
            );
        }
    }
}

/// The explicit `--skill`/`--prompt-template`/`--theme` CLI paths.
fn collect_cli_paths(cfg: &DiscoveryConfig, acc: &mut Accum) {
    let skills = &mut acc.skills;
    let prompts = &mut acc.prompts;
    let themes = &mut acc.themes;
    let warnings = &mut acc.warnings;
    let diagnostics = &mut acc.diagnostics;

    // --- explicit CLI flags (Pi `source:"cli", scope:"temporary"`; Pi appends `additionalSkillPaths`
    // after the entire sorted accumulator, resource-loader.ts:421, so under first-wins they lose to a
    // same-name resource of any rank, including a rank-4 package — modeled as rank 5)
    // (R-09-002/008/012) ---
    if cfg.enable_skills {
        for p in &cfg.cli.skills {
            add_skill_path(
                p,
                ResourceScope::Cli,
                ResourceOrigin::Cli { path: p.clone() },
                skills,
                warnings,
                diagnostics,
            );
        }
    }
    if cfg.enable_prompts {
        for p in &cfg.cli.prompts {
            add_prompt_path(
                p,
                ResourceScope::Cli,
                ResourceOrigin::Cli { path: p.clone() },
                prompts,
                warnings,
            );
        }
    }
    if cfg.enable_themes {
        for p in &cfg.cli.themes {
            add_theme_path(
                p,
                ResourceScope::Cli,
                ResourceOrigin::Cli { path: p.clone() },
                themes,
                warnings,
            );
        }
    }
}

/// The top-level settings enable/disable filter, applied once every source has contributed.
fn apply_disabled_filter(cfg: &DiscoveryConfig, acc: &mut Accum) {
    let skills = &mut acc.skills;
    let prompts = &mut acc.prompts;
    let themes = &mut acc.themes;

    // --- top-level enable/disable filter (R-09-018) ---
    let dis = &cfg.disabled;
    skills.retain(|s| !name_disabled(&dis.skills, &s.key));
    prompts.retain(|p| !name_disabled(&dis.prompts, &p.key));
    themes.retain(|t| !name_disabled(&dis.themes, &t.key));
}
