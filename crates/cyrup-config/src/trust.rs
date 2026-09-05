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

/// Lexically resolve a path to an absolute, `.`/`..`-normalized form WITHOUT touching the
/// filesystem (Pi `resolvePath` → Node `path.resolve`, utils/paths.ts:81-84). Relative inputs are
/// absolutized against the process cwd; `..` is popped lexically (never above the root).
fn resolve_path(p: &Path) -> PathBuf {
    use std::path::Component;
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(base) => base.join(p),
            Err(_) => p.to_path_buf(),
        }
    };
    let mut out: Vec<Component> = Vec::new();
    for comp in abs.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                // A `..` cannot escape the root/prefix; keep leading `..` only on a relative path
                // (unreachable here since `abs` is absolute, but kept for total correctness).
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(comp),
            },
            other => out.push(other),
        }
    }
    out.iter().collect()
}

/// Pi `canonicalizePath(resolvePath(cwd))` (trust-manager.ts:39-41 `normalizeCwd`): first
/// absolutize + lexically normalize (so a relative or not-yet-existing cwd still yields a stable
/// absolute key), then resolve symlinks via the real filesystem, FALLING BACK to the
/// resolved-but-unreal path on failure (utils/paths.ts:28-34). Critically the fallback is the
/// normalized absolute path, NOT the raw input.
fn canonicalize(p: &Path) -> PathBuf {
    let resolved = resolve_path(p);
    std::fs::canonicalize(&resolved).unwrap_or(resolved)
}

impl TrustStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn read_map(&self) -> Result<BTreeMap<String, Option<bool>>, ConfigError> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(e) => {
                return Err(ConfigError::Io {
                    path: self.path.clone(),
                    source: e,
                });
            }
        };
        if text.trim().is_empty() {
            return Ok(BTreeMap::new());
        }
        let value: Value =
            serde_json::from_str(&text).map_err(|e| ConfigError::Trust(format!("parse: {e}")))?;
        // Pi throws on a non-object top-level (trust-manager.ts:111-113).
        let obj = match value {
            Value::Object(o) => o,
            _ => {
                return Err(ConfigError::Trust(
                    "Invalid trust store: expected an object".into(),
                ));
            }
        };
        let mut out = BTreeMap::new();
        for (k, v) in obj {
            // Pi throws on any non-`true`/`false`/`null` value (trust-manager.ts:117-121) instead of
            // silently skipping — a malformed trust.json is a hard error.
            let decision = match v {
                Value::Bool(b) => Some(b),
                Value::Null => None,
                _ => {
                    return Err(ConfigError::Trust(format!(
                        "Invalid trust store: value for {k:?} must be true, false, or null"
                    )));
                }
            };
            out.insert(k, decision);
        }
        Ok(out)
    }

    /// Walk cwd→root; the first canonical-path key with an explicit bool wins (R-07-013).
    ///
    /// The read runs UNDER the cross-process file lock, as pi's does: `getEntry` wraps its read in
    /// `withTrustFileLock` (trust-manager.ts:219-222 @v0.83.0, lock defined `:168`) and `get()`
    /// (`:216`) routes through `getEntry`. CFG-013.
    pub async fn nearest(&self, cwd: &Path) -> Result<Option<TrustEntry>, ConfigError> {
        let _guard = crate::lock::FileLock::acquire(&self.path, None).await?;
        let map = self.read_map()?;
        let cwd = canonicalize(cwd);
        let mut current: Option<&Path> = Some(cwd.as_path());
        while let Some(dir) = current {
            if let Some(key) = dir.to_str()
                && let Some(Some(b)) = map.get(key)
            {
                return Ok(Some(TrustEntry {
                    path: dir.to_path_buf(),
                    decision: TrustDecision::from_bool(*b),
                }));
            }
            current = dir.parent();
        }
        Ok(None)
    }

    /// Atomic multi-update under a cross-process lock; sorted pretty JSON + trailing newline
    /// (Pi byte-interop). A `None` decision deletes the key (R-07-010).
    pub async fn set_many(
        &self,
        updates: &[(PathBuf, Option<TrustDecision>)],
    ) -> Result<(), ConfigError> {
        let _guard = crate::lock::FileLock::acquire(&self.path, None).await?;
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

    pub async fn set(
        &self,
        cwd: &Path,
        decision: Option<TrustDecision>,
    ) -> Result<(), ConfigError> {
        self.set_many(&[(cwd.to_path_buf(), decision)]).await
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
    /// The Agent Client Protocol host (`--acp` / `--mode acp`) — a JSON-RPC agent driven by an
    /// editor over stdio. ACP-002; see `cyrup_acp` and `crate::cli::runtime_mode::resolve_app_mode`
    /// in the `cyrup` bin, where this variant must be the FIRST branch (an ACP agent is launched
    /// with pipes on both ends and would otherwise resolve to `Print`).
    Acp,
}

impl AppMode {
    /// "This host owns a TTY and paints a terminal UI." Read by `should_take_over_stdout`,
    /// `config.persist`'s explicit-session ladder and the `!= Interactive` guards in the `cyrup`
    /// bin's `bootstrap.rs` / `startup_ui.rs` / `prelaunch.rs`.
    ///
    /// ACP-002 — **this deliberately stays `Interactive`-only** even though the ACP host CAN put a
    /// question to a human. Flipping it for [`AppMode::Acp`] would take every one of those four
    /// sites down the TTY branch: the stdout takeover would be skipped (letting a stray library
    /// write corrupt the JSON-RPC frame stream), and the startup-UI / session-picker guards would
    /// try to paint a selector into a pipe. The prompting half is [`AppMode::can_prompt`].
    pub fn is_interactive(self) -> bool {
        matches!(self, AppMode::Interactive)
    }

    /// "There is a human this host can put a question to, and a channel to reach them on."
    ///
    /// ACP-002 — the split half of [`AppMode::is_interactive`], and the ONLY consumer is
    /// [`decide_trust`]'s step 5. An ACP client renders `session/request_permission` and
    /// `elicitation/create`, so an untrusted project reaching an ACP host is a question, not a
    /// silent refusal — which is exactly what pi's own `hasUI` gate means (project-trust.ts:86-88)
    /// and what `is_interactive` used to stand in for while `Interactive` was the only such host.
    ///
    /// # CYRUP-DELTA
    ///
    /// pi-acp has no counterpart: its adapter spawns `pi --mode rpc`, so the child is a
    /// non-interactive host and every untrusted project silently resolves `Untrusted`. cyrup's ACP
    /// host is in-process and can ask, so it does. **The cost**: an ACP client that never answers
    /// the trust prompt leaves `decide_trust` at [`TrustOutcome::NeedsPrompt`], and the front end
    /// (not this function) owns the timeout that turns that back into `Untrusted` — a host that
    /// forgets to supply `prompt_choice` hangs its own `session/new` instead of refusing it.
    pub fn can_prompt(self) -> bool {
        matches!(self, AppMode::Interactive | AppMode::Acp)
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
        return if o {
            TrustOutcome::Trusted
        } else {
            TrustOutcome::Untrusted
        };
    }
    // 2. nothing to gate (R-07-006).
    if !input.has_resources {
        return TrustOutcome::Trusted;
    }
    // 3. saved decision / ancestor match (R-07-013).
    if let Some(saved) = input.saved {
        return if saved.is_trusted() {
            TrustOutcome::Trusted
        } else {
            TrustOutcome::Untrusted
        };
    }
    // 4. non-interactive policy (R-07-009).
    match input.default_trust {
        DefaultProjectTrust::Always => return TrustOutcome::Trusted,
        DefaultProjectTrust::Never => return TrustOutcome::Untrusted,
        DefaultProjectTrust::Ask => {}
    }
    // 5. ask: only a host that can reach a human may prompt; everything else is untrusted
    // (R-07-009). ACP-002 — the predicate is `can_prompt`, not `is_interactive`: `AppMode::Acp`
    // has a client that renders permission dialogs, and this is the ONE site that distinction is
    // for. Every other reader of `is_interactive` means "TUI host" and must stay unchanged.
    if input.mode.can_prompt() {
        match input.prompt_choice {
            Some(true) => TrustOutcome::Trusted,
            Some(false) => TrustOutcome::Untrusted,
            None => TrustOutcome::NeedsPrompt,
        }
    } else {
        TrustOutcome::Untrusted
    }
}

/// A prompt option (arch-07 §3.4; Pi `ProjectTrustOption`). `updates` empty = session-only (no
/// persist). `saved_path` is the path the option persists a decision for (`None` for session-only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustOption {
    pub label: String,
    pub trusted: bool,
    pub updates: Vec<(PathBuf, Option<TrustDecision>)>,
    /// The trust-store path this option writes (Pi `ProjectTrustOption.savedPath`).
    pub saved_path: Option<PathBuf>,
}

/// The parent trust-store path for `cwd`, or `None` at the filesystem root (Pi
/// `getProjectTrustParentPath`, trust-manager.ts:59-63).
pub fn project_trust_parent_path(cwd: &Path) -> Option<PathBuf> {
    let cwd = canonicalize(cwd);
    cwd.parent()
        .filter(|p| *p != cwd.as_path())
        .map(Path::to_path_buf)
}

/// The interactive trust-prompt message (Pi `formatProjectTrustPrompt`, project-trust.ts:24-26).
pub fn format_project_trust_prompt(cwd: &Path) -> String {
    format!(
        "Trust project folder?\n{}\n\nThis allows cyrup to load .cyrup settings and resources, \
install missing project packages, and execute project extensions.",
        cwd.display()
    )
}

/// Build the standard trust prompt options for `cwd` (Pi `getProjectTrustOptions`,
/// trust-manager.ts:65-95; R-07-008/010).
pub fn trust_options(cwd: &Path, include_session_only: bool) -> Vec<TrustOption> {
    let cwd = canonicalize(cwd);
    let mut opts = vec![TrustOption {
        label: "Trust".to_string(),
        trusted: true,
        updates: vec![(cwd.clone(), Some(TrustDecision::Trusted))],
        saved_path: Some(cwd.clone()),
    }];
    if let Some(parent) = project_trust_parent_path(&cwd) {
        opts.push(TrustOption {
            label: format!("Trust parent folder ({})", parent.display()),
            trusted: true,
            // parent=true, cwd=null (remove) — descendants inherit via ancestor match.
            updates: vec![
                (parent.clone(), Some(TrustDecision::Trusted)),
                (cwd.clone(), None),
            ],
            saved_path: Some(parent),
        });
    }
    if include_session_only {
        opts.push(TrustOption {
            label: "Trust (this session only)".to_string(),
            trusted: true,
            updates: Vec::new(),
            saved_path: None,
        });
    }
    opts.push(TrustOption {
        label: "Do not trust".to_string(),
        trusted: false,
        updates: vec![(cwd.clone(), Some(TrustDecision::Untrusted))],
        saved_path: Some(cwd),
    });
    if include_session_only {
        opts.push(TrustOption {
            label: "Do not trust (this session only)".to_string(),
            trusted: false,
            updates: Vec::new(),
            saved_path: None,
        });
    }
    opts
}

/// An extension's `project_trust` verdict (Pi `emitProjectTrustEvent` result,
/// project-trust.ts:54-69). `remember` asks the caller to persist the decision to the trust store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExtensionTrust {
    pub trusted: bool,
    pub remember: bool,
}

/// Staged trust decision WITH an optional extension `project_trust` hook consulted *before* the
/// saved decision (Pi `resolveProjectTrusted`, project-trust.ts:46-95). Additive companion to
/// [`decide_trust`] (whose `TrustInputs` is a stable public type and cannot gain a field). When the
/// extension returns a verdict it wins over the saved/default/prompt tiers; `ExtensionTrust.remember`
/// signals the caller to persist via the trust store.
pub fn decide_trust_with_extension(
    input: TrustInputs,
    extension: Option<ExtensionTrust>,
) -> TrustOutcome {
    // Steps 1-2 mirror `decide_trust`: explicit override, then "nothing to gate".
    if let Some(o) = input.trust_override {
        return if o {
            TrustOutcome::Trusted
        } else {
            TrustOutcome::Untrusted
        };
    }
    if !input.has_resources {
        return TrustOutcome::Trusted;
    }
    // Extension hook (before the saved decision).
    if let Some(ext) = extension {
        return if ext.trusted {
            TrustOutcome::Trusted
        } else {
            TrustOutcome::Untrusted
        };
    }
    // Remaining tiers (saved → default → prompt) are identical to `decide_trust`; `trust_override`
    // is `None` and `has_resources` is `true` here, so re-running those checks is a no-op.
    decide_trust(input)
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

    /// The returned guard owns the directory's lifetime — it MUST stay bound for the whole
    /// test (`let dir = tmp();`), never dropped into a temporary (`tmp().join(..)`).
    fn tmp() -> crate::test_util::TempDir {
        crate::test_util::temp_dir()
    }

    #[tokio::test]
    async fn ancestor_match() {
        // R-07-013 / A-07-2
        let dir = tmp();
        let trust = dir.join("trust.json");
        let store = TrustStore::new(trust);
        let root = dir.join("proj");
        let child = root.join("a").join("b");
        std::fs::create_dir_all(&child).unwrap();

        store
            .set(&root, Some(TrustDecision::Trusted))
            .await
            .unwrap();
        let found = store.nearest(&child).await.unwrap().unwrap();
        assert_eq!(found.decision, TrustDecision::Trusted);

        // None when no ancestor matches.
        let other_root = tmp();
        let other = other_root.join("unrelated");
        std::fs::create_dir_all(&other).unwrap();
        assert!(store.nearest(&other).await.unwrap().is_none());
    }

    #[test]
    fn canonicalize_absolutizes_and_normalizes_like_pi_resolve_path() {
        // Pi `normalizeCwd = canonicalizePath(resolvePath(cwd))` (trust-manager.ts:39-41). For a
        // non-existent absolute path with `..`, `resolvePath` lexically normalizes and
        // `realpathSync` fails → returns the resolved-but-unreal path. Ground truth captured from
        // Pi: `canonicalizePath(resolve("/nonexistent-xyz-123/../abc")) === "/abc"`.
        // The OLD crate code returned the RAW input ("/nonexistent-xyz-123/../abc") verbatim.
        assert_eq!(
            canonicalize(Path::new("/nonexistent-xyz-123/../abc")),
            PathBuf::from("/abc")
        );

        // For a relative input, Pi absolutizes against the process cwd: `resolve("foo/../bar")`
        // === `${process.cwd()}/bar`. The OLD code returned "foo/../bar" verbatim (not absolute).
        let got = canonicalize(Path::new("foo/../bar"));
        assert!(got.is_absolute(), "relative cwd must be absolutized");
        let expected = std::env::current_dir().unwrap().join("bar");
        assert_eq!(got, expected);
        assert_ne!(got, PathBuf::from("foo/../bar"));
    }

    #[tokio::test]
    async fn parent_trust_removes_child_key() {
        // R-07-010: "Trust parent" writes parent=true, cwd=null.
        let dir = tmp();
        let store = TrustStore::new(dir.join("trust.json"));
        let parent = dir.join("p");
        let cwd = parent.join("c");
        std::fs::create_dir_all(&cwd).unwrap();
        store
            .set(&cwd, Some(TrustDecision::Untrusted))
            .await
            .unwrap();

        let opts = trust_options(&cwd, true);
        let parent_opt = opts.iter().find(|o| o.label.contains("parent")).unwrap();
        store.set_many(&parent_opt.updates).await.unwrap();

        // child key removed, parent trusted → ancestor match yields trusted.
        let found = store.nearest(&cwd).await.unwrap().unwrap();
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
            decide_trust(TrustInputs {
                default_trust: DefaultProjectTrust::Always,
                ..base
            }),
            TrustOutcome::Trusted
        );
        // --approve forces trust for one run
        assert_eq!(
            decide_trust(TrustInputs {
                trust_override: Some(true),
                ..base
            }),
            TrustOutcome::Trusted
        );
        // --no-approve forces untrusted
        assert_eq!(
            decide_trust(TrustInputs {
                trust_override: Some(false),
                ..base
            }),
            TrustOutcome::Untrusted
        );
        // no trust-requiring resources → trusted
        assert_eq!(
            decide_trust(TrustInputs {
                has_resources: false,
                ..base
            }),
            TrustOutcome::Trusted
        );
        // saved decision wins
        assert_eq!(
            decide_trust(TrustInputs {
                saved: Some(TrustDecision::Trusted),
                ..base
            }),
            TrustOutcome::Trusted
        );
        // interactive + ask + no saved → needs prompt
        assert_eq!(
            decide_trust(TrustInputs {
                mode: AppMode::Interactive,
                ..base
            }),
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

    /// ACP-002 — the split concept. `can_prompt` admits the ACP host; `is_interactive` must NOT,
    /// because it also drives `should_take_over_stdout`, `config.persist` and the three
    /// `!= Interactive` guards in the `cyrup` bin. Both directions are asserted so a later
    /// "simplification" that collapses the two predicates fails here.
    #[test]
    fn acp_can_prompt_but_is_not_a_tui_host() {
        assert!(AppMode::Acp.can_prompt(), "an ACP client renders dialogs");
        assert!(
            !AppMode::Acp.is_interactive(),
            "`is_interactive` means TUI host; flipping it changes four unrelated sites"
        );
        assert!(AppMode::Interactive.can_prompt());
        assert!(AppMode::Interactive.is_interactive());
        for mode in [AppMode::Print, AppMode::Json, AppMode::Rpc] {
            assert!(!mode.can_prompt(), "{mode:?} has no channel to a human");
            assert!(!mode.is_interactive(), "{mode:?} paints no TUI");
        }
    }

    /// ACP-002 — `decide_trust`'s step 5 is the one consumer of [`AppMode::can_prompt`], and the
    /// other four modes must be byte-identical to what they were before the variant existed.
    #[test]
    fn acp_reaches_the_trust_prompt_and_the_other_modes_do_not() {
        let base = TrustInputs {
            has_resources: true,
            trust_override: None,
            saved: None,
            default_trust: DefaultProjectTrust::Ask,
            mode: AppMode::Acp,
            prompt_choice: None,
        };
        assert_eq!(decide_trust(base), TrustOutcome::NeedsPrompt);
        assert_eq!(
            decide_trust(TrustInputs {
                prompt_choice: Some(true),
                ..base
            }),
            TrustOutcome::Trusted
        );
        assert_eq!(
            decide_trust(TrustInputs {
                prompt_choice: Some(false),
                ..base
            }),
            TrustOutcome::Untrusted
        );
        for mode in [
            AppMode::Print,
            AppMode::Json,
            AppMode::Rpc,
            AppMode::Interactive,
        ] {
            let expected = if mode.is_interactive() {
                TrustOutcome::NeedsPrompt
            } else {
                TrustOutcome::Untrusted
            };
            assert_eq!(decide_trust(TrustInputs { mode, ..base }), expected, "{mode:?}");
        }
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

    #[tokio::test]
    async fn invalid_trust_value_errors() {
        // Gap 18 / trust-manager.ts:117-121: non-bool/null values are a hard error, not skipped.
        let dir = tmp();
        let path = dir.join("trust.json");
        std::fs::write(&path, r#"{ "/some/path": "yes" }"#).unwrap();
        let store = TrustStore::new(path);
        let cwd = dir.join("some").join("path");
        assert!(matches!(
            store.nearest(&cwd).await,
            Err(ConfigError::Trust(_))
        ));
    }

    #[test]
    fn extension_hook_overrides_saved_decision() {
        // project-trust.ts:54-69: extension verdict is consulted before the saved decision.
        let base = TrustInputs {
            has_resources: true,
            trust_override: None,
            saved: Some(TrustDecision::Untrusted),
            default_trust: DefaultProjectTrust::Ask,
            mode: AppMode::Print,
            prompt_choice: None,
        };
        // extension says trusted → trusted, beating the saved "untrusted".
        let out = decide_trust_with_extension(
            base,
            Some(ExtensionTrust {
                trusted: true,
                remember: false,
            }),
        );
        assert_eq!(out, TrustOutcome::Trusted);
        // no extension → falls back to saved decision (untrusted).
        assert_eq!(
            decide_trust_with_extension(base, None),
            TrustOutcome::Untrusted
        );
        // override still wins over the extension.
        let out = decide_trust_with_extension(
            TrustInputs {
                trust_override: Some(false),
                ..base
            },
            Some(ExtensionTrust {
                trusted: true,
                remember: true,
            }),
        );
        assert_eq!(out, TrustOutcome::Untrusted);
    }

    #[test]
    fn trust_options_match_pi_labels_and_saved_path() {
        let dir = tmp();
        let cwd = dir.join("p").join("c");
        std::fs::create_dir_all(&cwd).unwrap();
        let opts = trust_options(&cwd, true);
        assert_eq!(opts.first().unwrap().label, "Trust");
        assert!(
            opts.iter()
                .any(|o| o.label.starts_with("Trust parent folder ("))
        );
        assert!(opts.iter().any(|o| o.label == "Trust (this session only)"));
        assert!(
            opts.iter()
                .any(|o| o.label == "Do not trust (this session only)")
        );
        // session-only options carry no saved_path.
        let session = opts
            .iter()
            .find(|o| o.label.contains("this session only"))
            .unwrap();
        assert!(session.saved_path.is_none());
        assert!(opts.first().unwrap().saved_path.is_some());
    }

    #[test]
    fn project_trust_prompt_copy() {
        let p = format_project_trust_prompt(Path::new("/tmp/proj"));
        assert!(p.contains("install missing project packages"));
        assert!(p.contains("execute project extensions"));
    }

    #[test]
    fn trust_requiring_resources_detection() {
        // R-07-006
        let home = tmp();
        let cwd_root = tmp();
        let cwd = cwd_root.join("proj");
        std::fs::create_dir_all(&cwd).unwrap();
        assert!(!has_trust_requiring_resources(&cwd, &home));
        std::fs::create_dir_all(cwd.join(".cyrup")).unwrap();
        std::fs::write(cwd.join(".cyrup").join("settings.json"), "{}").unwrap();
        assert!(has_trust_requiring_resources(&cwd, &home));
    }

    #[test]
    fn home_agents_skills_excluded_but_project_ancestor_counted() {
        // Pi trust-manager.ts:184-206 (`hasTrustRequiringProjectResources`): the user/global
        // `~/.agents/skills` (anchored at `process.env.HOME || homedir()`, :185) is a trusted user
        // resource and is NEVER trust-requiring, even when cwd IS $HOME; a NON-home ancestor's
        // `.agents/skills` IS. This is why the REAL `$HOME` must be threaded (G1) rather than the
        // agent dir: a misresolved home turns the user skills dir into a false project trust gate.
        let home = tmp();
        std::fs::create_dir_all(home.join(".agents").join("skills")).unwrap();

        // cwd == home: its own `.agents/skills` is the excluded user dir → no trust required.
        assert!(!has_trust_requiring_resources(&home, &home));

        // A project below home with its own `.agents/skills` IS trust-requiring.
        let proj = home.join("proj");
        std::fs::create_dir_all(proj.join(".agents").join("skills")).unwrap();
        assert!(has_trust_requiring_resources(&proj, &home));

        // GAP GUARD: if `home` is misresolved to an unrelated dir (as it was when `SessionConfig.home`
        // silently fell back to the agent dir), the user's own `~/.agents/skills` is no longer
        // excluded and becomes a spurious trust gate.
        let wrong_home = tmp();
        assert!(has_trust_requiring_resources(&home, &wrong_home));
    }
}
