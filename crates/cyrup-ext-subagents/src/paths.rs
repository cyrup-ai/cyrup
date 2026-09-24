//! The crate's SINGLE port of pi's `shared/utils.ts` path helpers.
//!
//! This module answers "where is the user's home directory", "where is the agent directory" and
//! "where is a project's config directory". It answers the first two by asking
//! [`cyrup_config::paths`], which owns the workspace's one home ladder and one agent-dir ladder;
//! nothing here re-derives either.
//!
//! Most of this crate reaches those ladders through this module. One resolver goes straight to
//! `cyrup-config` instead, for a reason that is a type mismatch rather than an oversight — see
//! "Two more carried their own copies" below.
//!
//! # Why the ladders live in `cyrup-config` and not here
//!
//! They used to live in five places. `CYRUP_HOME` had its own ladder in this module, in
//! `background`, in `native_supervisor`, in `cyrup-intercom` and in `cyrup-mcp`. The agent dir had
//! FIVE resolvers reading three different key sets: `cyrup-config` read all three spellings, this
//! module and `cyrup-ext`'s `npx_resolver` read two, and `native_supervisor` and `cyrup-intercom`
//! read one.
//!
//! Two of the copies were byte-identical by intent, and this module's own doc said so — *"pinned
//! byte-identical to `cyrup_intercom::paths::agent_dir_path_from` across a dependency edge that
//! forbids importing it"*. `cyrup-intercom` depends on this crate, so this crate cannot import it
//! back; the shared answer had nowhere to live that both could reach.
//!
//! `cyrup-config` is where it lives now: it is the crate that owns layout resolution, and it
//! depends only on `cyrup-core` and `cyrup-provider`, so it sits below every crate that needed the
//! answer and nothing can cycle through it. Of the four crates that consume the ladders, two —
//! this one and `cyrup-mcp` — already depended on it; `cyrup-intercom` and `cyrup-ext` gained the
//! edge for this change.
//!
//! That consolidation fixed a real defect, not just duplication. `cyrup-config` read
//! `CYRUP_CODING_AGENT_DIR` (CFG-076, *"whichever spelling is set, core lands on the same
//! directory the siblings do"*) and [`crate::paths::resolve_agent_dir`] did not, so setting that
//! variable moved the binary's layout while leaving this crate's agent memory, run history,
//! settings, prompts and sessions behind in the un-relocated tree — MCP-139 gap 1.
//!
//! # Two siblings answer DIFFERENT questions, and are deliberately not this ladder
//!
//! Two resolvers remain separate, and neither is a copy — each answers a question those ladders
//! do not:
//!
//! - `crate::background::temp_root_dir` — the run-scratch root: `CYRUP_HOME` (non-blank), else a
//!   per-user temp directory. It has **no `HOME` rung on purpose**; that tree is reboot-disposable
//!   scratch, so falling back to `$HOME` would write it into the user's real home. It shares the
//!   home ladder's KEY (through `cyrup_config::paths::ENV_HOME`) but not its terminal, which is
//!   the only such exception in the workspace.
//! - `crate::spawn::nested_events::temp_root_dir` — reads `CYRUP_SUBAGENTS_TEMP_ROOT`, and never
//!   consults `CYRUP_HOME` at all. A different variable, so a different question entirely.
//!
//! Two more carried their own copies and no longer do, by two different routes:
//! `crate::discovery::skills` calls [`crate::paths::home_dir`] in this module, and
//! `crate::native_supervisor::intercom_agent_dir_from` calls
//! `cyrup_config::paths::cyrup_home_dir_from` + `cyrup_config::paths::cyrup_agent_dir_from`
//! directly — it cannot come through here, because the `String`-shaped `env` seam it takes is fed
//! by `SubagentsExtension::env_lookup` (which layers `SubagentExtensionConfig::env_overrides`),
//! while this module's seams are `OsString`-shaped.
//!
//! # Why `CYRUP_HOME` comes first, and why that check is load-bearing
//!
//! `CYRUP_HOME` is the crate's sandbox lever: an integration test points it at a `TempDir`
//! precisely so no run artifact, mission pointer, settings file or worktree lands in the
//! developer's real home. Any resolver that skips the check silently escapes that sandbox —
//! `missions/store.rs` was once the one copy that did, and that omission alone leaked mission
//! pointers into a real `~/.cyrup` through nineteen correctly-sandboxed tests.
//!
//! ONE resolver is what makes that class of bug structurally impossible, rather than a comment
//! asserting that N private copies agree. That resolver is
//! [`cyrup_config::paths::cyrup_home_dir_from`] — no longer this module, which now only spells it
//! for this crate's callers.

use std::path::{Path, PathBuf};

/// The workspace's environment-lookup shape, re-exported so this crate's seams read in one
/// vocabulary. See [`cyrup_config::paths::EnvLookup`] for why it is `OsString`-shaped.
pub type EnvLookup<'a> = cyrup_config::paths::EnvLookup<'a>;

/// `os.homedir()` as this crate resolves it: `CYRUP_HOME` -> `HOME` -> the OS home directory ->
/// [`std::env::temp_dir`].
///
/// The OS-home rung is [`cyrup_config::paths::cyrup_home_dir_from`]'s last, and it is not
/// redundant with `HOME`: on unix `directories::BaseDirs` reads `$HOME` and the two agree, but on
/// Windows `HOME` is usually unset and that rung is the one that answers with the real user
/// profile.
///
/// Never returns an empty path: when no rung answers, the process temp dir does, so a caller
/// joining onto the result always gets an absolute path rather than a relative one rooted at the
/// process working directory.
#[must_use]
pub fn home_dir() -> PathBuf {
    home_dir_from(&|key| std::env::var_os(key))
}

/// [`home_dir`] with its one ambient input — the environment — passed in, so every rung is provable
/// without mutating process-global state. Mirrors the crate's existing injectable-core convention
/// (`background::temp_root_dir_from`, `native_supervisor::intercom_agent_dir_from`).
///
/// Takes `OsString` rather than `String` so a non-UTF-8 `CYRUP_HOME`/`HOME` resolves exactly as
/// [`home_dir`]'s own `var_os` does; a `String`-shaped seam would silently drop such a value and
/// fall to the next rung.
#[must_use]
pub fn home_dir_from(env: EnvLookup<'_>) -> PathBuf {
    // The LIBRARY terminal for the workspace's one home ladder. `cyrup_config::ConfigDirs::resolve`
    // takes the other one — it errors — and both are right: a binary that cannot locate home should
    // refuse, and a library must not panic. The ladder itself is shared; only the terminal differs.
    cyrup_config::paths::cyrup_home_dir_from(env).unwrap_or_else(std::env::temp_dir)
}

/// pi `getAgentDir()` (`shared/utils.ts:95-100`) against an explicitly supplied home: the first
/// non-blank [`cyrup_config::paths::ENV_AGENT_DIR_KEYS`] entry (with `~`/`~/` expansion against
/// `home`), else `<home>/.cyrup/agent`.
///
/// It names the shared key list rather than restating the spellings, so this doc cannot drift from
/// the ladder again — restating them is how it came to omit `CYRUP_CODING_AGENT_DIR` and describe
/// MCP-139 gap 1 as the behaviour. "Non-blank" rather than "non-empty" because the shared filter
/// trims: a whitespace-only value is unset.
///
/// The injectable-home shape exists for the callers that already hold a home path (and for tests
/// that want to resolve against a temp home without moving the process environment);
/// [`agent_dir`] is the process-environment form.
#[must_use]
pub fn resolve_agent_dir(home: &Path) -> PathBuf {
    resolve_agent_dir_from(home, &|key| std::env::var_os(key))
}

/// [`resolve_agent_dir`] with its environment supplied — the workspace's one agent-dir ladder
/// ([`cyrup_config::paths::cyrup_agent_dir_from`]) resolved against `home`.
///
/// Reads whatever [`cyrup_config::paths::ENV_AGENT_DIR_KEYS`] holds — today the two `CYRUP_*`
/// spellings (the upstream `PI_CODING_AGENT_DIR` leg was dropped in the hard rename). The second
/// one, `CYRUP_CODING_AGENT_DIR`, is the fix rather than a widening: `cyrup_config` has read it
/// since CFG-076 (back when the ladder was three keys, this function read only two of them), and
/// the gap meant an operator setting the long spelling moved the binary's layout while this
/// crate's agent memory, run history, settings, prompts and sessions stayed in the old tree.
#[must_use]
pub fn resolve_agent_dir_from(home: &Path, env: EnvLookup<'_>) -> PathBuf {
    cyrup_config::paths::cyrup_agent_dir_from(home, env)
}

/// pi `getAgentDir()` resolved against [`home_dir`] — the form nearly every caller wants.
#[must_use]
pub fn agent_dir() -> PathBuf {
    resolve_agent_dir(&home_dir())
}

/// pi `getProjectConfigDir(projectRoot)` (`shared/utils.ts:91-93`) — `<root>/.cyrup` (upstream
/// `<root>/.pi`), the same directory `cyrup_config::ConfigDirs::project_config_dir` names.
#[must_use]
pub fn project_config_dir(project_root: &Path) -> PathBuf {
    project_root.join(".cyrup")
}

/// pi `pathWithin` — is `candidate` inside `base`, or `base` itself?
///
/// **The crate's ONE containment check**, and the claim is now checkable: `grep -rn 'fn
/// path_within\|fn contained_path' crates/cyrup-ext-subagents/src` returns this definition and
/// nothing else.
///
/// Upstream carries a private copy per module — SEVEN of them at v0.68.0
/// (`git grep -l 'function pathWithin' v0.68.0 -- src`: `inspectors/actions.ts`,
/// `runs/background/fleet-view.ts`, `runs/background/scheduled-runs.ts`,
/// `runs/foreground/subagent-executor.ts`, `shared/session-file-trust.ts`,
/// `tui/fleet-transcript.ts`, `tui/fleet.ts`) — and cyrup grew FIVE to match before this
/// consolidation: the four the ledger named plus `spawn/nested_events.rs`'s byte-identical
/// `contained_path`, which the same predicate under a different name hid from the first sweep.
/// That was five places for a security-relevant predicate to drift, and drift here is not
/// cosmetic: every caller uses it to
/// REFUSE a path that escaped a trusted root — a transcript outside the session roots
/// (`background::fleet_view`, `tui::fleet_transcript`), an inspector launched outside the async
/// root (`inspectors::actions::trusted_dir`), a schedule store outside the project
/// (`background::scheduled_runs::store`). A copy that quietly lost its `starts_with` leg, or
/// gained a `..`-tolerant one, would widen exactly one of those and nothing would fail.
///
/// Both sides are put through [`std::path::absolute`] first, so a relative input cannot slip past
/// by comparing unlike shapes. That is a no-op for a caller that already resolved its inputs —
/// `scheduled_runs::store` canonicalizes both before calling — and is what the other three
/// copies did inline.
///
/// It is **not** a symlink check. `absolute` resolves neither `..` nor symlinks (it is not
/// `canonicalize`), so a caller that must survive a planted symlink canonicalizes first and
/// applies this to the real paths — which is precisely what
/// [`crate::inspectors::actions::trusted_dir`] does on both legs, and why its doc calls the
/// realpath leg load-bearing.
#[must_use]
pub(crate) fn path_within(base: &Path, candidate: &Path) -> bool {
    let base = std::path::absolute(base).unwrap_or_else(|_| base.to_path_buf());
    let candidate = std::path::absolute(candidate).unwrap_or_else(|_| candidate.to_path_buf());
    candidate == base || candidate.starts_with(&base)
}

// =================================================================================================
// Roots — every filesystem root this crate derives from the environment, resolved ONCE
// =================================================================================================

/// The four base roots this crate derives from ambient state, decided at ONE boundary instead of
/// re-derived at every call site.
///
/// # Why this type exists
///
/// Before it, five separate resolvers each read the environment independently, and the
/// consequences were not hypothetical: `CYRUP_HOME` had five ladders that disagreed while this
/// module's own doc claimed there was one; the nested-events containment guard re-derived its
/// trusted root at four layers, so scoping a route in one of them failed in the next; and two
/// different `temp_root_dir` functions both documented themselves as pi's `TEMP_ROOT_DIR` while
/// resolving to different directories. Each was found by a test failing, one layer at a time.
///
/// A resolved value cannot drift from itself. That is the whole of the idea.
///
/// # The two scratch roots are genuinely different, and stay that way
///
/// [`Self::run_scratch`] (background run artifacts) keys off `CYRUP_HOME`; [`Self::nested_scratch`]
/// (nested events, supervisor channels) keys off `CYRUP_SUBAGENTS_TEMP_ROOT` and never consults
/// `CYRUP_HOME` at all. Collapsing them into one field would be tidier and WRONG — production
/// relocates them independently, and a test built on a merged root would pass against a layout the
/// shipped code never produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roots {
    home: PathBuf,
    agent_dir: PathBuf,
    run_scratch: PathBuf,
    nested_scratch: PathBuf,
    /// The home a CHILD process must be told about because it could not derive these roots itself.
    ///
    /// `None` for [`Self::from_env`] and [`Self::from_config_dirs`] — a child re-running the same
    /// ladders against the same environment lands on the same answer, so telling it anything is
    /// both redundant and harmful: `CYRUP_HOME` has no `HOME` rung in
    /// `background::temp_root_dir_from`, so setting it unconditionally would move every child's
    /// run scratch out of the OS temp dir and into the user's real `~/.cyrup` — the exact defect
    /// `temp_root_dir_lives_under_the_os_temp_dir_and_never_under_home` exists to catch, and the
    /// one that once left 59,321 files there.
    ///
    /// `Some(root)` for [`Self::sandboxed`], which is the only constructor producing roots no
    /// child can re-derive.
    child_home_override: Option<PathBuf>,
    /// R-SA-003 — the resolved `CYRUP_SUBAGENT_EXTRA_AGENT_DIRS` entries (pi
    /// `PI_SUBAGENT_EXTRA_AGENT_DIRS`, `agents.ts:2308-2322` @v0.68.0): read-only directories
    /// discovery scans as the LOWEST-precedence User-tier stream.
    ///
    /// A root like the other four, and for the same reason. Two readers need this answer —
    /// discovery (`SubagentExecutor::discovery_dirs_config`, which prepends these to the user
    /// agent dirs) and `/subagents`' read-only check (pi `isReadOnlyExtraAgent`,
    /// `subagents-admin.ts:139-148`) — and they must agree about which agents are extra-dir
    /// agents. They used to read it separately: discovery straight off `std::env`, the admin
    /// through the extension's `env_overrides`, with two parsers that differed on blank entries.
    /// So a pin reached one and not the other, and an agent could be discovered from an extra dir
    /// while the admin let an edit of it through as if it were the user's own. Both now read THIS
    /// field of the one `Roots` the session resolved.
    extra_agent_dirs: Vec<PathBuf>,
}

/// pi `extraUserAgentDirs` (`agents.ts:2315-2322` @v0.68.0) and the identical split in
/// `isReadOnlyExtraAgent` (`subagents-admin.ts:143`): `raw.split(path.delimiter).map(trim)
/// .filter(nonEmpty)`, over [`crate::discovery::EXTRA_AGENT_DIRS_ENV_VAR`]. **The crate's one
/// parser of that variable** — [`Roots::from_lookup`] is its only caller.
///
/// [`std::env::split_paths`] is `path.delimiter` (`:` on Unix, `;` on Windows). Each entry is
/// trimmed as upstream trims it (a non-UTF-8 entry cannot carry the Unicode whitespace `trim`
/// removes in a way this could see, so it is kept verbatim), and an entry that is empty after the
/// trim is dropped — so an unset, empty or all-blank variable is no extra dirs at all, never the
/// relative empty path (which would scan the process cwd).
fn extra_agent_dirs_from(env: EnvLookup<'_>) -> Vec<PathBuf> {
    let Some(raw) = env(crate::discovery::EXTRA_AGENT_DIRS_ENV_VAR) else {
        return Vec::new();
    };
    std::env::split_paths(&raw)
        .filter_map(|entry| match entry.to_str() {
            Some(text) => {
                let trimmed = text.trim();
                (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
            }
            None => (!entry.as_os_str().is_empty()).then_some(entry),
        })
        .collect()
}

impl Roots {
    /// The roots this process's environment actually names.
    ///
    /// **The only constructor that reads the environment.** [`Self::sandboxed`] takes its answers;
    /// [`Self::from_config_dirs`] takes the two it carries and delegates here for the rest. So a
    /// `Roots` is derived from ambient state in exactly one place.
    ///
    /// A root added to [`Self::from_lookup`] therefore reaches [`Self::from_config_dirs`] for
    /// free, and forces [`Self::sandboxed`] to name it — a compile error rather than a silent
    /// omission, since that one builds every field explicitly. The drift this rules out is the one
    /// that matters: neither can silently LACK a root this constructor resolves.
    ///
    /// The free functions [`home_dir`] and [`resolve_agent_dir`] read the environment too — they
    /// are the zero-argument process forms this crate's callers ask for by name. Each is a
    /// one-line wrapper over the same `cyrup_config::paths` ladder this constructor uses, so they
    /// cannot answer differently; they are not a second source of truth, just a second spelling.
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_lookup(&|key| std::env::var_os(key))
    }

    /// Every rung of every ladder, against ONE supplied lookup.
    ///
    /// Deliberately delegates to the existing resolvers rather than restating their ladders: this
    /// constructor is faithful BY CONSTRUCTION, and each ladder's documented quirks (the missing
    /// `HOME` rung on the background scratch root, the `~` expansion in the agent dir) keep living
    /// with the function that owns them. It is the analogue of `cyrup_config::EnvVars::from_lookup`
    /// — the same read-once-into-a-value shape, one crate down.
    ///
    /// [`std::env::temp_dir`] stays a direct call rather than a fifth injected input: it is a read,
    /// not a hazard, and [`Self::sandboxed`] already serves the caller that wants a different temp
    /// root.
    #[must_use]
    pub fn from_lookup(env: EnvLookup<'_>) -> Self {
        let home = home_dir_from(env);
        Self {
            agent_dir: resolve_agent_dir_from(&home, env),
            run_scratch: crate::background::temp_root_dir_from(env, std::env::temp_dir()),
            nested_scratch: crate::spawn::nested_events::temp_root_dir_from(
                env,
                std::env::temp_dir(),
            ),
            home,
            child_home_override: None,
            extra_agent_dirs: extra_agent_dirs_from(env),
        }
    }

    /// The roots anchored on the layout the BINARY already resolved.
    ///
    /// Exact rather than approximate: both sides run the same two ladders, so this and
    /// [`Self::from_env`] agree in every state. They could not before — `ConfigDirs` had no
    /// `CYRUP_HOME` rung and this crate had no `CYRUP_CODING_AGENT_DIR` rung, so each was wrong
    /// about a case the other got right.
    ///
    /// The two scratch roots have no `ConfigDirs` counterpart — they are this crate's own trees —
    /// so they come from [`Self::from_env`] rather than being re-derived here. That delegation is
    /// the point: re-deriving them would make this a SECOND place where a root is read from the
    /// environment, and a third scratch root added to [`Self::from_lookup`] would then silently
    /// not reach this constructor.
    #[must_use]
    pub fn from_config_dirs(dirs: &cyrup_config::ConfigDirs) -> Self {
        Self {
            home: dirs.home.clone(),
            agent_dir: dirs.agent_dir.clone(),
            ..Self::from_env()
        }
    }

    /// Every root under one caller-owned directory, for a caller that wants total isolation.
    ///
    /// **This is deliberately MORE isolated than `CYRUP_HOME=<root>` is in production**, where that
    /// variable moves `home`/`agent_dir`/`run_scratch` and leaves the nested tree on the shared
    /// per-user temp root. That difference is the point — a test wants one directory it can delete —
    /// but it means a test asserting production's root COUPLING (that `CYRUP_HOME` does not relocate
    /// the nested tree) must build its roots from [`Self::from_env`] instead.
    #[must_use]
    pub fn sandboxed(root: &Path) -> Self {
        Self {
            home: root.to_path_buf(),
            agent_dir: root.join(".cyrup").join("agent"),
            run_scratch: root.join(".cyrup").join("subagents"),
            nested_scratch: root.join(".cyrup").join("nested"),
            child_home_override: Some(root.to_path_buf()),
            // A sandbox has no extra agent dirs: an ambient `CYRUP_SUBAGENT_EXTRA_AGENT_DIRS` on
            // the developer's or CI's machine must not add agents to an isolated test's User
            // tier. A caller that wants some names them with [`Self::with_extra_agent_dirs`].
            extra_agent_dirs: Vec::new(),
        }
    }

    /// These roots with `dirs` as the extra agent dirs, replacing whatever the constructor
    /// resolved — the in-process pin for R-SA-003, so a caller (a test, an embedder) names the
    /// directories explicitly instead of moving the process environment. Discovery and the
    /// `/subagents` read-only check both see exactly `dirs`, because both read
    /// [`Self::extra_agent_dirs`].
    #[must_use]
    pub fn with_extra_agent_dirs(mut self, dirs: Vec<PathBuf>) -> Self {
        self.extra_agent_dirs = dirs;
        self
    }

    /// The home a detached CHILD must be told about, or `None` when it can derive these roots on
    /// its own — see the field of the same name for why `None` is not merely an optimisation.
    #[must_use]
    pub fn child_home_override(&self) -> Option<&Path> {
        self.child_home_override.as_deref()
    }

    /// `os.homedir()` as this crate resolves it.
    #[must_use]
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// pi `getAgentDir()`.
    #[must_use]
    pub fn agent_dir(&self) -> &Path {
        &self.agent_dir
    }

    /// R-SA-003 — the extra agent dirs, lowest-precedence User tier first-to-last, in the order
    /// the variable lists them. Read by discovery AND by `/subagents`' read-only check; see the
    /// field for why they share this one answer.
    #[must_use]
    pub fn extra_agent_dirs(&self) -> &[PathBuf] {
        &self.extra_agent_dirs
    }

    /// The background run-artifact scratch root (async/results trees hang off this).
    #[must_use]
    pub fn run_scratch(&self) -> &Path {
        &self.run_scratch
    }

    /// The nested-events / supervisor-channel scratch root.
    #[must_use]
    pub fn nested_scratch(&self) -> &Path {
        &self.nested_scratch
    }

    /// The nested-events tree — and the containment root every route is validated against.
    #[must_use]
    pub fn nested_events(&self) -> PathBuf {
        self.nested_scratch.join("nested-subagent-events")
    }

    /// The nested RUN directories — the containment root a descendant's `async_dir` is checked
    /// against before any cascade writes into it.
    #[must_use]
    pub fn nested_runs(&self) -> PathBuf {
        self.nested_scratch.join("nested-subagent-runs")
    }

    /// pi `SUPERVISOR_CHANNEL_ROOT`.
    #[must_use]
    pub fn supervisor_channels(&self) -> PathBuf {
        self.nested_scratch.join("supervisor-channels")
    }
}

impl Default for Roots {
    fn default() -> Self {
        Self::from_env()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// A fixed environment table. Nothing here writes the process environment — `set_var` is
    /// `unsafe` under Rust 2024 and this crate is `#![forbid(unsafe_code)]`, which is precisely why
    /// every resolver above takes a lookup.
    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<std::ffi::OsString> {
        let owned: Vec<(String, std::ffi::OsString)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), std::ffi::OsString::from(*v)))
            .collect();
        move |key: &str| owned.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    /// MCP-139 gap 1, from this crate's side — proven against `cyrup-config`, not against itself.
    ///
    /// `cyrup-config` has read `CYRUP_CODING_AGENT_DIR` since CFG-076, on the stated grounds that
    /// *"whichever spelling is set, core lands on the same directory the siblings do"*. This
    /// crate's resolver did not read it, so setting that variable moved the binary's layout while
    /// leaving the six production call sites of [`agent_dir`] — user-scope agent memory,
    /// `run-history.jsonl`, `settings.json`, the prompts dir, the sessions base and the agent dir
    /// the subagent tool advertises — behind in the un-relocated tree.
    ///
    /// # What this asserts, and why it is not asserted the easy way
    ///
    /// It drives BOTH crates through one fixed environment and requires the same directory:
    /// `ConfigDirs::resolve` on one side, [`resolve_agent_dir_from`] (the injected twin of what
    /// [`agent_dir`] computes) on the other. Comparing this crate's resolver against
    /// `cyrup_config::paths::cyrup_agent_dir_from` instead would be worthless — the former
    /// delegates to the latter and nothing else, so such an assertion cannot fail whatever either
    /// crate does.
    ///
    /// It therefore fails on the two ways these can drift apart, which are the two the defect was
    /// made of: a key present in one ladder and not the other, and a `~` anchored on a different
    /// home.
    #[test]
    fn every_agent_dir_spelling_lands_where_the_binarys_layout_lands() {
        for key in cyrup_config::paths::ENV_AGENT_DIR_KEYS {
            let pairs = [("HOME", "/home/u"), (key, "/opt/relocated")];
            let lookup = env(&pairs);

            // The layout the BINARY resolves, from that environment and nothing else.
            let dirs = cyrup_config::ConfigDirs::resolve(
                &cyrup_config::CliConfigOverrides {
                    cwd: Some(PathBuf::from("/")),
                    ..cyrup_config::CliConfigOverrides::default()
                },
                &cyrup_config::EnvVars::from_lookup(&lookup),
            )
            .expect("the fixture names a home");

            assert_eq!(
                dirs.home,
                PathBuf::from("/home/u"),
                "{key}: the fixture home must win"
            );
            assert_eq!(
                resolve_agent_dir_from(&dirs.home, &lookup),
                dirs.agent_dir,
                "{key}: this crate's agent dir and the binary's layout must be ONE directory"
            );
            assert_eq!(
                dirs.agent_dir,
                PathBuf::from("/opt/relocated"),
                "{key} must actually move the tree — otherwise both sides agree on the default \
                 and the assertion above is vacuous"
            );
        }
    }

    /// The axis-3 half of the same defect, at the boundary the test above cannot reach.
    ///
    /// A `~/…` agent dir was expanded against different homes by the two crates: this one against
    /// its own (`CYRUP_HOME`-aware) home, `ConfigDirs` through `normalize_path_buf`, which anchors
    /// on `paths::ambient_home` and never consults `CYRUP_HOME`. With BOTH a home override and a
    /// `~` agent dir set they landed in different directories.
    #[test]
    fn a_tilde_agent_dir_lands_where_the_binarys_layout_lands_under_a_home_override() {
        let pairs = [
            ("HOME", "/real"),
            ("CYRUP_HOME", "/sandbox"),
            ("CYRUP_AGENT_DIR", "~/agents"),
        ];
        let lookup = env(&pairs);
        let dirs = cyrup_config::ConfigDirs::resolve(
            &cyrup_config::CliConfigOverrides {
                cwd: Some(PathBuf::from("/")),
                ..cyrup_config::CliConfigOverrides::default()
            },
            &cyrup_config::EnvVars::from_lookup(&lookup),
        )
        .expect("the fixture names a home");

        assert_eq!(
            dirs.home,
            PathBuf::from("/sandbox"),
            "CYRUP_HOME must beat HOME"
        );
        assert_eq!(
            dirs.agent_dir,
            PathBuf::from("/sandbox/agents"),
            "the `~` must anchor on the OVERRIDDEN home, not the ambient one"
        );
        assert_eq!(
            resolve_agent_dir_from(&dirs.home, &lookup),
            dirs.agent_dir,
            "both crates must anchor the same `~` against the same home"
        );
    }

    /// The home ladder is `cyrup-config`'s, with this crate's terminal.
    ///
    /// The terminal is the ONLY difference that survives, and it is deliberate: a library must not
    /// panic, so an unresolvable home falls to the process temp dir, where
    /// `cyrup_config::ConfigDirs::resolve` errors instead.
    #[test]
    fn the_home_ladder_is_cyrup_configs_with_a_library_terminal() {
        let lookup = env(&[("CYRUP_HOME", "/sandbox"), ("HOME", "/real")]);
        assert_eq!(home_dir_from(&lookup), PathBuf::from("/sandbox"));
        assert_eq!(
            home_dir_from(&env(&[("HOME", "/real")])),
            PathBuf::from("/real")
        );
    }

    /// `Roots::from_lookup` resolves every root from ONE lookup, and each lands where its own
    /// resolver says — including the two scratch roots, which are deliberately NOT the home tree.
    #[test]
    fn from_lookup_resolves_all_four_roots_from_one_environment() {
        let roots = Roots::from_lookup(&env(&[
            ("CYRUP_HOME", "/sandbox"),
            ("CYRUP_SUBAGENTS_TEMP_ROOT", "/nested-root"),
        ]));
        assert_eq!(roots.home(), Path::new("/sandbox"));
        assert_eq!(roots.agent_dir(), Path::new("/sandbox/.cyrup/agent"));
        // `CYRUP_HOME` relocates the run scratch (that resolver's one rung) …
        assert_eq!(roots.run_scratch(), Path::new("/sandbox/.cyrup/subagents"));
        // … and does NOT relocate the nested tree, which keys off its own variable. Production
        // moves these two independently, and a `Roots` that merged them would let a test pass
        // against a layout the shipped code never produces.
        assert_eq!(roots.nested_scratch(), Path::new("/nested-root"));
    }

    /// A blank `CYRUP_HOME` is unset, on every rung that reads it.
    ///
    /// Load-bearing rather than defensive: `PathBuf::from("")` is the RELATIVE empty path, so a
    /// blank value taken verbatim would root the whole run-scratch tree at the process working
    /// directory.
    #[test]
    fn a_blank_home_falls_through_rather_than_rooting_everything_at_the_cwd() {
        let roots = Roots::from_lookup(&env(&[("CYRUP_HOME", "   "), ("HOME", "/real")]));
        assert_eq!(roots.home(), Path::new("/real"));
        assert!(
            roots.run_scratch().is_absolute(),
            "the run scratch must never be relative, got {:?}",
            roots.run_scratch()
        );
        assert!(
            !roots.run_scratch().starts_with("/real"),
            "and never under the real home"
        );
    }

    /// The parent -> child sandbox handoff, which is the invariant that could break silently when
    /// production started resolving `Roots` for itself.
    ///
    /// A detached child has no `ConfigDirs` — it returns before bootstrap — so it re-derives its
    /// roots with [`Roots::from_env`] against the environment it inherited. The parent therefore
    /// has to TELL it, and only when it has something to say: [`Roots::child_home_override`] is
    /// `Some` for a sandbox and `None` for roots the child would derive identically anyway.
    ///
    /// The `None` case is the load-bearing one. `background::temp_root_dir_from` reads `CYRUP_HOME`
    /// with **no `HOME` rung**, so putting the ambient home on every child's `Command` would move
    /// each child's run scratch out of the OS temp dir and into the user's real `~/.cyrup`.
    #[test]
    fn only_a_sandbox_is_handed_down_to_a_detached_child() {
        let sandbox = Roots::sandboxed(Path::new("/sandbox"));
        assert_eq!(sandbox.child_home_override(), Some(Path::new("/sandbox")));

        // A child told `CYRUP_HOME=/sandbox` re-derives the parent's own home and run scratch.
        let child = Roots::from_lookup(&env(&[("CYRUP_HOME", "/sandbox")]));
        assert_eq!(child.home(), sandbox.home());
        assert_eq!(child.run_scratch(), sandbox.run_scratch());

        // Roots the child can derive itself are NOT handed down.
        assert_eq!(Roots::from_env().child_home_override(), None);
    }

    /// R-SA-003 — the ONE parser of `CYRUP_SUBAGENT_EXTRA_AGENT_DIRS`, as upstream splits it in
    /// both of its readers (`agents.ts:2315-2322`, `subagents-admin.ts:143` @v0.68.0): on the
    /// platform delimiter, each entry trimmed, blanks dropped. `from_lookup` resolves it from the
    /// SAME lookup as every other root, a sandbox has none, and the builder replaces it.
    ///
    /// *Gutted by*: `extra_agent_dirs: Vec::new()` in `from_lookup` (row 1 empty); dropping the
    /// `trim` (row 1 keeps `" /b "`); dropping the empty filter (the blank variable yields `""`).
    #[test]
    fn extra_agent_dirs_are_one_resolved_root() {
        let joined = if cfg!(windows) {
            "/a; /b ;;  "
        } else {
            "/a: /b ::  "
        };
        let roots = Roots::from_lookup(&env(&[
            ("CYRUP_HOME", "/sandbox"),
            (crate::discovery::EXTRA_AGENT_DIRS_ENV_VAR, joined),
        ]));
        assert_eq!(
            roots.extra_agent_dirs(),
            [PathBuf::from("/a"), PathBuf::from("/b")]
        );
        for blank in ["", "   "] {
            let roots =
                Roots::from_lookup(&env(&[(crate::discovery::EXTRA_AGENT_DIRS_ENV_VAR, blank)]));
            assert!(roots.extra_agent_dirs().is_empty(), "{blank:?}");
        }
        assert!(
            Roots::from_lookup(&env(&[("CYRUP_HOME", "/sandbox")]))
                .extra_agent_dirs()
                .is_empty()
        );
        let sandbox = Roots::sandboxed(Path::new("/sandbox"));
        assert!(sandbox.extra_agent_dirs().is_empty());
        let pinned = sandbox.with_extra_agent_dirs(vec![PathBuf::from("/pinned")]);
        assert_eq!(pinned.extra_agent_dirs(), [PathBuf::from("/pinned")]);
        assert_eq!(
            pinned.home(),
            Path::new("/sandbox"),
            "the other roots are kept"
        );
    }

    /// **The crate's one containment predicate, both directions.**
    ///
    /// Four modules use it to REFUSE a path that escaped a trusted root, and it had four private
    /// copies until this batch. A table here is what keeps the one that is left honest: the
    /// `starts_with` leg (a descendant IS inside), the equality leg (the root itself is inside),
    /// a sibling whose name merely SHARES A PREFIX with the root (`/a/roots` is not inside
    /// `/a/root`, which a naive `to_string_lossy().starts_with` would get wrong), and a relative
    /// input, which `std::path::absolute` resolves against the cwd rather than letting it compare
    /// as an unlike shape.
    ///
    /// *Gutted by*: dropping the `starts_with` leg (row 2 goes red); dropping the `==` leg
    /// (row 1); returning `true` (rows 4 and 5); comparing the rendered strings instead of the
    /// components (row 5).
    #[test]
    fn path_within_is_component_wise_containment_over_absolute_paths() {
        let root = Path::new("/a/root");
        assert!(path_within(root, Path::new("/a/root")), "the root itself");
        assert!(
            path_within(root, Path::new("/a/root/child")),
            "a descendant"
        );
        assert!(
            path_within(root, Path::new("/a/root/deep/er/still")),
            "a deep descendant"
        );
        assert!(!path_within(root, Path::new("/a")), "the parent is outside");
        assert!(
            !path_within(root, Path::new("/a/roots")),
            "a sibling sharing the root's spelling is OUTSIDE — this is the string-prefix bug"
        );
        assert!(
            !path_within(root, Path::new("/b/root/child")),
            "another tree entirely"
        );

        // A relative candidate is absolutized against the cwd, so it is compared as a path rather
        // than as unlike shapes. Against the cwd itself it must be inside; against an unrelated
        // absolute root it must not.
        let cwd = std::env::current_dir().expect("a cwd");
        assert!(path_within(&cwd, Path::new("some/relative/leaf")));
        assert!(!path_within(
            Path::new("/definitely/not/the/cwd"),
            Path::new("x")
        ));
    }
}
