//! Project trust: the persisted `trust.json` store with ancestor matching, the staged
//! pre-/post-trust resource split, and the pure trust decision (arch-07 §3.4/§6.2,
//! R-07-006…R-07-013).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::ConfigError;
use crate::settings::DefaultProjectTrust;

/// A persisted trust decision (`trust.json` stores `true`/`false`; `null`/absent = no decision).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrustDecision {
    Trusted,
    Untrusted,
}

impl TrustDecision {
    pub fn is_trusted(self) -> bool {
        matches!(self, TrustDecision::Trusted)
    }
    fn as_bool(self) -> bool {
        self.is_trusted()
    }
    fn from_bool(b: bool) -> Self {
        if b {
            TrustDecision::Trusted
        } else {
            TrustDecision::Untrusted
        }
    }
}

impl From<bool> for TrustDecision {
    fn from(b: bool) -> Self {
        TrustDecision::from_bool(b)
    }
}

/// A matched trust entry (the path whose key matched + the decision).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustEntry {
    pub path: PathBuf,
    pub decision: TrustDecision,
}

/// The `trust.json` store (arch-07 §3.4). Stateless: each op re-reads under a file lock.
pub struct TrustStore {
    path: PathBuf,
}

fn canonicalize(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

impl TrustStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn read_map(&self) -> Result<BTreeMap<String, Option<bool>>, ConfigError> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(e) => return Err(ConfigError::Io(e)),
        };
        if text.trim().is_empty() {
            return Ok(BTreeMap::new());
        }
        let value: Value =
            serde_json::from_str(&text).map_err(|e| ConfigError::Trust(format!("parse: {e}")))?;
        let obj = match value {
            Value::Object(o) => o,
            _ => return Ok(BTreeMap::new()),
        };
        let mut out = BTreeMap::new();
        for (k, v) in obj {
            let decision = match v {
                Value::Bool(b) => Some(b),
                Value::Null => None,
                _ => continue,
            };
            out.insert(k, decision);
        }
        Ok(out)
    }

    /// Walk cwd→root; the first canonical-path key with an explicit bool wins (R-07-013).
    pub fn nearest(&self, cwd: &Path) -> Result<Option<TrustEntry>, ConfigError> {
        let map = self.read_map()?;
        let cwd = canonicalize(cwd);
        let mut current: Option<&Path> = Some(cwd.as_path());
        while let Some(dir) = current {
            if let Some(key) = dir.to_str() {
                if let Some(Some(b)) = map.get(key) {
                    return Ok(Some(TrustEntry {
                        path: dir.to_path_buf(),
                        decision: TrustDecision::from_bool(*b),
                    }));
                }
            }
            current = dir.parent();
        }
        Ok(None)
    }

    /// Atomic multi-update under a cross-process lock; sorted pretty JSON + trailing newline
    /// (Pi byte-interop). A `None` decision deletes the key (R-07-010).
    pub fn set_many(
        &self,
        updates: &[(PathBuf, Option<TrustDecision>)],
    ) -> Result<(), ConfigError> {
        let _guard = crate::lock::FileLock::acquire(&self.path)?;
        let mut map = self.read_map()?;
        for (path, decision) in updates {
            let key = canonicalize(path).to_string_lossy().into_owned();
            match decision {
                Some(d) => {
                    map.insert(key, Some(d.as_bool()));
                }
                None => {
                    map.remove(&key);
                }
            }
        }
        // Re-serialize (BTreeMap is already sorted by key).
        let obj: serde_json::Map<String, Value> = map
            .into_iter()
            .map(|(k, v)| (k, v.map_or(Value::Null, Value::Bool)))
            .collect();
        let mut text = serde_json::to_string_pretty(&Value::Object(obj))?;
        text.push('\n');
        crate::lock::write_atomic(&self.path, text.as_bytes(), false)?;
        Ok(())
    }

    pub fn set(&self, cwd: &Path, decision: Option<TrustDecision>) -> Result<(), ConfigError> {
        self.set_many(&[(cwd.to_path_buf(), decision)])
    }
}

/// True if `cwd` has trust-requiring project resources (R-07-006):
/// `.cyrup/{settings.json,extensions,skills,prompts,themes,SYSTEM.md,APPEND_SYSTEM.md}` or a
/// `.agents/skills` directory in cwd or an ancestor (excluding `~/.agents/skills`).
pub fn has_trust_requiring_resources(cwd: &Path, home: &Path) -> bool {
    const CYRUP_MARKERS: &[&str] = &[
        "settings.json",
        "extensions",
        "skills",
        "prompts",
        "themes",
        "SYSTEM.md",
        "APPEND_SYSTEM.md",
    ];
    let cyrup_dir = cwd.join(".cyrup");
    if CYRUP_MARKERS.iter().any(|m| cyrup_dir.join(m).exists()) {
        return true;
    }
    // .agents/skills walk cwd → root, excluding the home directory's own ~/.agents/skills.
    let home_agents = home.join(".agents");
    let mut current: Option<&Path> = Some(cwd);
    while let Some(dir) = current {
        let agents = dir.join(".agents");
        if agents != home_agents && agents.join("skills").exists() {
            return true;
        }
        current = dir.parent();
    }
    false
}

/// Application mode that gates interactive prompting (R-07-009).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppMode {
    Interactive,
    Print,
    Json,
    Rpc,
}

impl AppMode {
    pub fn is_interactive(self) -> bool {
        matches!(self, AppMode::Interactive)
    }
}

/// Inputs to the pure trust decision (arch-07 §6.2). The interactive prompt UI is a front-end
/// concern; its result (if any) is supplied as `prompt_choice`.
#[derive(Clone, Copy, Debug)]
pub struct TrustInputs {
    /// `has_trust_requiring_resources(cwd)`.
    pub has_resources: bool,
    /// `--approve` (Some(true)) / `--no-approve` (Some(false)).
    pub trust_override: Option<bool>,
    /// Saved decision for the folder or an ancestor (R-07-013).
    pub saved: Option<TrustDecision>,
    /// Global-only `defaultProjectTrust`.
    pub default_trust: DefaultProjectTrust,
    pub mode: AppMode,
    /// Result of an interactive prompt if one was shown by the front end.
    pub prompt_choice: Option<bool>,
}

/// The resolved trust outcome. `NeedsPrompt` means the front end must run the interactive prompt
/// and re-decide with `prompt_choice` set (R-07-008).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrustOutcome {
    Trusted,
    Untrusted,
    NeedsPrompt,
}

/// The single staged-trust decision algorithm (arch-07 §6.2). Pure; the caller loads post-trust
/// resources iff the outcome is `Trusted`.
pub fn decide_trust(input: TrustInputs) -> TrustOutcome {
    // 1. explicit per-run override (--approve / --no-approve) wins (R-07-009).
    if let Some(o) = input.trust_override {
        return if o { TrustOutcome::Trusted } else { TrustOutcome::Untrusted };
    }
    // 2. nothing to gate (R-07-006).
    if !input.has_resources {
        return TrustOutcome::Trusted;
    }
    // 3. saved decision / ancestor match (R-07-013).
    if let Some(saved) = input.saved {
        return if saved.is_trusted() { TrustOutcome::Trusted } else { TrustOutcome::Untrusted };
    }
    // 4. non-interactive policy (R-07-009).
    match input.default_trust {
        DefaultProjectTrust::Always => return TrustOutcome::Trusted,
        DefaultProjectTrust::Never => return TrustOutcome::Untrusted,
        DefaultProjectTrust::Ask => {}
    }
    // 5. ask: only interactive mode may prompt; everything else is untrusted (R-07-009).
    if input.mode.is_interactive() {
        match input.prompt_choice {
            Some(true) => TrustOutcome::Trusted,
            Some(false) => TrustOutcome::Untrusted,
            None => TrustOutcome::NeedsPrompt,
        }
    } else {
        TrustOutcome::Untrusted
    }
}

/// A prompt option (arch-07 §3.4). `updates` empty = session-only (no persist).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustOption {
    pub label: String,
    pub trusted: bool,
    pub updates: Vec<(PathBuf, Option<TrustDecision>)>,
}

/// Build the standard trust prompt options for `cwd` (R-07-008/010).
pub fn trust_options(cwd: &Path, include_session_only: bool) -> Vec<TrustOption> {
    let cwd = canonicalize(cwd);
    let mut opts = vec![TrustOption {
        label: "Trust this folder".to_string(),
        trusted: true,
        updates: vec![(cwd.clone(), Some(TrustDecision::Trusted))],
    }];
    if let Some(parent) = cwd.parent() {
        opts.push(TrustOption {
            label: "Trust parent folder".to_string(),
            trusted: true,
            // parent=true, cwd=null (remove) — descendants inherit via ancestor match.
            updates: vec![
                (parent.to_path_buf(), Some(TrustDecision::Trusted)),
                (cwd.clone(), None),
            ],
        });
    }
    if include_session_only {
        opts.push(TrustOption {
            label: "Trust this session only".to_string(),
            trusted: true,
            updates: Vec::new(),
        });
    }
    opts.push(TrustOption {
        label: "Do not trust".to_string(),
        trusted: false,
        updates: vec![(cwd, Some(TrustDecision::Untrusted))],
    });
    opts
}

/// A resource category, classified by trust stage (R-07-007).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    GlobalContext,
    GlobalExtensions,
    CliExtensions,
    ProjectContext,
    ProjectSettings,
    ProjectExtensions,
    ProjectPackages,
}

/// Whether a resource loads before trust is resolved (R-07-007).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceStage {
    PreTrust,
    PostTrust,
}

pub fn resource_stage(kind: ResourceKind) -> ResourceStage {
    match kind {
        ResourceKind::GlobalContext
        | ResourceKind::GlobalExtensions
        | ResourceKind::CliExtensions => ResourceStage::PreTrust,
        ResourceKind::ProjectContext
        | ResourceKind::ProjectSettings
        | ResourceKind::ProjectExtensions
        | ResourceKind::ProjectPackages => ResourceStage::PostTrust,
    }
}

/// Pure staged-loading split (A-07-4): pre-trust resources always load; post-trust load only when
/// trusted.
pub fn should_load(kind: ResourceKind, trusted: bool) -> bool {
    match resource_stage(kind) {
        ResourceStage::PreTrust => true,
        ResourceStage::PostTrust => trusted,
    }
}

/// Partition a labelled resource list into the set that loads, given the trust state (A-07-4).
pub fn select_loaded<T>(resources: &[(ResourceKind, T)], trusted: bool) -> Vec<&T> {
    resources
        .iter()
        .filter(|(k, _)| should_load(*k, trusted))
        .map(|(_, v)| v)
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        crate::test_util::temp_dir()
    }

    #[test]
    fn ancestor_match() {
        // R-07-013 / A-07-2
        let dir = tmp();
        let trust = dir.join("trust.json");
        let store = TrustStore::new(trust);
        let root = dir.join("proj");
        let child = root.join("a").join("b");
        std::fs::create_dir_all(&child).unwrap();

        store.set(&root, Some(TrustDecision::Trusted)).unwrap();
        let found = store.nearest(&child).unwrap().unwrap();
        assert_eq!(found.decision, TrustDecision::Trusted);

        // None when no ancestor matches.
        let other = tmp().join("unrelated");
        std::fs::create_dir_all(&other).unwrap();
        assert!(store.nearest(&other).unwrap().is_none());
    }

    #[test]
    fn parent_trust_removes_child_key() {
        // R-07-010: "Trust parent" writes parent=true, cwd=null.
        let dir = tmp();
        let store = TrustStore::new(dir.join("trust.json"));
        let parent = dir.join("p");
        let cwd = parent.join("c");
        std::fs::create_dir_all(&cwd).unwrap();
        store.set(&cwd, Some(TrustDecision::Untrusted)).unwrap();

        let opts = trust_options(&cwd, true);
        let parent_opt = opts.iter().find(|o| o.label.contains("parent")).unwrap();
        store.set_many(&parent_opt.updates).unwrap();

        // child key removed, parent trusted → ancestor match yields trusted.
        let found = store.nearest(&cwd).unwrap().unwrap();
        assert_eq!(found.decision, TrustDecision::Trusted);
    }

    #[test]
    fn decision_matrix() {
        // A-07-3 / A-07-10 decision-logic forms.
        let base = TrustInputs {
            has_resources: true,
            trust_override: None,
            saved: None,
            default_trust: DefaultProjectTrust::Ask,
            mode: AppMode::Print,
            prompt_choice: None,
        };
        // print + ask + no saved → untrusted (no prompt)
        assert_eq!(decide_trust(base), TrustOutcome::Untrusted);
        // print + always → trusted
        assert_eq!(
            decide_trust(TrustInputs { default_trust: DefaultProjectTrust::Always, ..base }),
            TrustOutcome::Trusted
        );
        // --approve forces trust for one run
        assert_eq!(
            decide_trust(TrustInputs { trust_override: Some(true), ..base }),
            TrustOutcome::Trusted
        );
        // --no-approve forces untrusted
        assert_eq!(
            decide_trust(TrustInputs { trust_override: Some(false), ..base }),
            TrustOutcome::Untrusted
        );
        // no trust-requiring resources → trusted
        assert_eq!(
            decide_trust(TrustInputs { has_resources: false, ..base }),
            TrustOutcome::Trusted
        );
        // saved decision wins
        assert_eq!(
            decide_trust(TrustInputs { saved: Some(TrustDecision::Trusted), ..base }),
            TrustOutcome::Trusted
        );
        // interactive + ask + no saved → needs prompt
        assert_eq!(
            decide_trust(TrustInputs { mode: AppMode::Interactive, ..base }),
            TrustOutcome::NeedsPrompt
        );
        // interactive + ask + prompt said yes → trusted (persist handled by caller)
        assert_eq!(
            decide_trust(TrustInputs {
                mode: AppMode::Interactive,
                prompt_choice: Some(true),
                ..base
            }),
            TrustOutcome::Trusted
        );
    }

    #[test]
    fn staged_resource_split() {
        // A-07-4
        let resources = [
            (ResourceKind::GlobalContext, "g-ctx"),
            (ResourceKind::GlobalExtensions, "g-ext"),
            (ResourceKind::CliExtensions, "cli-ext"),
            (ResourceKind::ProjectContext, "p-ctx"),
            (ResourceKind::ProjectSettings, "p-settings"),
            (ResourceKind::ProjectExtensions, "p-ext"),
            (ResourceKind::ProjectPackages, "p-pkg"),
        ];
        let pre = select_loaded(&resources, false);
        assert_eq!(pre, vec![&"g-ctx", &"g-ext", &"cli-ext"]);
        let post = select_loaded(&resources, true);
        assert_eq!(post.len(), 7);
    }

    #[test]
    fn trust_requiring_resources_detection() {
        // R-07-006
        let home = tmp();
        let cwd = tmp().join("proj");
        std::fs::create_dir_all(&cwd).unwrap();
        assert!(!has_trust_requiring_resources(&cwd, &home));
        std::fs::create_dir_all(cwd.join(".cyrup")).unwrap();
        std::fs::write(cwd.join(".cyrup").join("settings.json"), "{}").unwrap();
        assert!(has_trust_requiring_resources(&cwd, &home));
    }
}
